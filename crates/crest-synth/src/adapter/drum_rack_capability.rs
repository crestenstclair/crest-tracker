use super::sample_capability::{SampleCapability, SAMPLE_ASSET_PARAMETER_ID};
use crate::synth::*;

pub const DRUM_RACK_CAPABILITY_ID: &str = "instrument.drum-rack";
pub const DRUM_RACK_PAD_PARAMETER_ID: &str = "drum-rack.pad";
pub const DRUM_RACK_FIRST_NOTE: u8 = 36;
/// The sixteen pads are the requested instrument layout, not a resource budget.
pub const DRUM_RACK_PADS: [(&str, &str); 16] = [
    ("C1", "Kick Drum"),
    ("C#1", "Side Stick Snare"),
    ("D1", "Snare Drum"),
    ("D#1", "Snare Roll"),
    ("E1", "Snare Off"),
    ("F1", "Low Tom"),
    ("F#1", "Closed Hat"),
    ("G1", "Mid Tom"),
    ("G#1", "Foot Hat"),
    ("A1", "High Tom"),
    ("A#1", "Open Hat"),
    ("B1", "Stick Rim"),
    ("C2", "Hand Clap"),
    ("C#2", "Crash"),
    ("D2", "Ride Bell"),
    ("D#2", "Right"),
];

pub fn pad_parameter_id(pad: usize, sample_parameter: &str) -> ParameterId {
    ParameterId::new(format!("drum-rack.pad-{pad}.{sample_parameter}"))
        .expect("static namespaced Sample parameter")
}

#[derive(Clone, Debug)]
pub struct DrumRackCapability {
    descriptor: CapabilityDescriptor,
    sample: SampleCapability,
}

impl DrumRackCapability {
    pub fn new(initial_asset: AssetFileId) -> Result<Self, CapabilityError> {
        let sample = SampleCapability::new(initial_asset)?;
        let schema = sample.descriptor();
        let selector = ParameterId::new(DRUM_RACK_PAD_PARAMETER_ID).unwrap();
        let choices = DRUM_RACK_PADS
            .iter()
            .enumerate()
            .map(|(pad, (note, name))| {
                ParameterChoice::new(format!("pad-{pad}"), format!("{note} · {name}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut sections = vec![CapabilitySection::new(
            "drum-rack.selection",
            "Pad",
            vec![ParameterSpec::new_with_patch_interaction(
                selector.clone(),
                "Pad",
                ParameterKind::Choice,
                ParameterUpdate::Scalar,
                PatchInteraction::ScalarEdit,
                ParameterDefault::Value(ParameterValue::Choice("pad-0".into())),
                None,
                choices,
                None,
                None,
                None,
                "choice",
                None,
                None,
            )?],
        )?
        .with_leading_controls()];
        let mut assets = Vec::new();
        let mut visualizations = Vec::new();
        for pad in 0..DRUM_RACK_PADS.len() {
            let visible = ParameterPredicate::new(
                selector.clone(),
                ParameterValue::Choice(format!("pad-{pad}")),
            );
            for section in schema.sections() {
                let parameters = section
                    .parameters()
                    .iter()
                    .map(|spec| {
                        let enabled = spec.enabled_when().map(|predicate| {
                            ParameterPredicate::new(
                                pad_parameter_id(pad, predicate.parameter_id().as_str()),
                                predicate.equals().clone(),
                            )
                        });
                        ParameterSpec::new_with_patch_interaction(
                            pad_parameter_id(pad, spec.id().as_str()),
                            spec.label(),
                            spec.kind(),
                            spec.update(),
                            spec.patch_interaction(),
                            spec.default_value().clone(),
                            spec.range(),
                            spec.choices().to_vec(),
                            spec.fine_step(),
                            spec.coarse_step(),
                            spec.unit().map(str::to_owned),
                            spec.formatter(),
                            enabled,
                            Some(visible.clone()),
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                sections.push(CapabilitySection::new(
                    format!("drum-rack.pad-{pad}.{}", section.id()),
                    section.label(),
                    parameters,
                )?);
            }
            assets.push(AssetRequirement::new(
                pad_parameter_id(pad, SAMPLE_ASSET_PARAMETER_ID),
                false,
            ));
            for visualization in schema.visualizations() {
                if let CapabilityVisualization::Waveform {
                    id,
                    label,
                    asset_parameter_id,
                    landmarks,
                } = visualization
                {
                    visualizations.push(CapabilityVisualization::waveform(
                        format!("drum-rack.pad-{pad}.{id}"),
                        label,
                        pad_parameter_id(pad, asset_parameter_id.as_str()),
                        landmarks
                            .iter()
                            .map(|landmark| {
                                WaveformLandmarkSpec::new(
                                    landmark.role(),
                                    pad_parameter_id(pad, landmark.parameter_id().as_str()),
                                )
                            })
                            .collect(),
                    )?);
                }
            }
        }
        visualizations.push(CapabilityVisualization::status(
            "drum-rack.status",
            "Preparation",
        )?);
        let descriptor = CapabilityDescriptor::new(
            CapabilityId::new(DRUM_RACK_CAPABILITY_ID).unwrap(),
            "Drum Rack",
            "instrument.sample",
            sections,
            assets,
            schema.voice_policy(),
            schema.supported_midi_kinds().to_vec(),
        )?
        .with_instrument_category(InstrumentCategory::Samplers)
        .with_visualizations(visualizations)?;
        Ok(Self { descriptor, sample })
    }

    pub(crate) fn id(&self) -> &CapabilityId {
        self.descriptor.id()
    }
    pub(crate) fn sample_descriptor(&self) -> CapabilityDescriptor {
        self.sample.descriptor()
    }

    pub fn default_config(&self) -> Result<InstrumentConfig, CapabilityError> {
        let values = self
            .descriptor
            .parameters()
            .filter_map(|spec| match spec.default_value() {
                ParameterDefault::Value(value) => {
                    Some(ParameterAssignment::new(spec.id().clone(), value.clone()))
                }
                ParameterDefault::Asset(_) => None,
            })
            .collect::<Vec<_>>();
        self.descriptor.create_config(&values, &[])
    }

    /// Extract one independent Sample configuration on the control/worker thread.
    pub fn sample_config(
        &self,
        config: &InstrumentConfig,
        pad: usize,
    ) -> Result<Option<InstrumentConfig>, CapabilityError> {
        if config.capability_id() != self.descriptor.id() || pad >= DRUM_RACK_PADS.len() {
            return Err(CapabilityError::ProviderRegistryMismatch(
                config.capability_id().clone(),
            ));
        }
        let Some(asset) = config.asset_reference(&pad_parameter_id(pad, SAMPLE_ASSET_PARAMETER_ID))
        else {
            return Ok(None);
        };
        let schema = self.sample.descriptor();
        let values = schema
            .parameters()
            .filter(|spec| spec.kind() != ParameterKind::Asset)
            .map(|spec| {
                config
                    .value(&pad_parameter_id(pad, spec.id().as_str()))
                    .cloned()
                    .map(|value| ParameterAssignment::new(spec.id().clone(), value))
                    .ok_or_else(|| CapabilityError::MissingParameter(spec.id().clone()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.sample
            .create_config(
                &values,
                &[AssetAssignment::new(
                    ParameterId::new(SAMPLE_ASSET_PARAMETER_ID).unwrap(),
                    asset.clone(),
                )],
            )
            .map(Some)
    }

    pub fn selected_pad(config: &InstrumentConfig) -> Option<usize> {
        let ParameterValue::Choice(value) =
            config.value(&ParameterId::new(DRUM_RACK_PAD_PARAMETER_ID).ok()?)?
        else {
            return None;
        };
        value
            .strip_prefix("pad-")?
            .parse()
            .ok()
            .filter(|pad| *pad < DRUM_RACK_PADS.len())
    }
}

impl InstrumentCapabilityProvider for DrumRackCapability {
    fn descriptor(&self) -> CapabilityDescriptor {
        self.descriptor.clone()
    }
    fn create_config(
        &self,
        values: &[ParameterAssignment],
        assets: &[AssetAssignment],
    ) -> Result<InstrumentConfig, CapabilityError> {
        self.descriptor.create_config(values, assets)
    }
}
