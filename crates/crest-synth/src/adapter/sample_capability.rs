use crate::kernel::midi_message::MidiMessageKind;
use crate::synth::{
    AssetAssignment, AssetFileId, AssetKind, AssetReference, AssetRequirement,
    CapabilityDescriptor, CapabilityError, CapabilityId, CapabilitySection,
    CapabilityVisualization, InstrumentCapabilityProvider, InstrumentConfig, ParameterAssignment,
    ParameterChoice, ParameterDefault, ParameterId, ParameterKind, ParameterPredicate,
    ParameterRange, ParameterSpec, ParameterUpdate, ParameterValue, PatchInteraction,
    SampleAssetError, SampleLoopMode, SamplePlaybackConfig, VoicePolicy, WaveformLandmarkRole,
    WaveformLandmarkSpec, SAMPLE_VOICE_COUNT,
};

pub const SAMPLE_CAPABILITY_ID: &str = "instrument.sample";
pub const SAMPLE_ASSET_PARAMETER_ID: &str = "sample.asset";
pub const SAMPLE_ROOT_NOTE_PARAMETER_ID: &str = "sample.root-note";
pub const SAMPLE_PLAYBACK_START_PARAMETER_ID: &str = "sample.playback-start";
pub const SAMPLE_PLAYBACK_END_PARAMETER_ID: &str = "sample.playback-end";
pub const SAMPLE_LOOP_MODE_PARAMETER_ID: &str = "sample.loop-mode";
pub const SAMPLE_LOOP_START_PARAMETER_ID: &str = "sample.loop-start";
pub const SAMPLE_LOOP_END_PARAMETER_ID: &str = "sample.loop-end";
pub const SAMPLE_CROSSFADE_PARAMETER_ID: &str = "sample.crossfade-ms";
pub const SAMPLE_LOOP_OFF_CHOICE_ID: &str = "sample.loop.off";
pub const SAMPLE_LOOP_FORWARD_CHOICE_ID: &str = "sample.loop.forward";

pub const SAMPLE_SUPPORTED_MIDI_KINDS: [MidiMessageKind; 7] = [
    MidiMessageKind::NoteOn,
    MidiMessageKind::NoteOff,
    MidiMessageKind::ControlChange,
    MidiMessageKind::ChannelPressure,
    MidiMessageKind::PolyPressure,
    MidiMessageKind::PitchBend,
    MidiMessageKind::AllNotesOff,
];

/// Schema/config provider for the bounded one-asset Sample instrument.
///
/// The initial asset is supplied by the composition root only after its real
/// catalog and decoder have admitted it. Later assignments retain this generic
/// schema and replace only the library-relative `AssetReference` locator.
#[derive(Clone, Debug)]
pub struct SampleCapability {
    descriptor: CapabilityDescriptor,
}

impl SampleCapability {
    pub fn new(initial_asset: AssetFileId) -> Result<Self, CapabilityError> {
        let asset_id = parameter_id(SAMPLE_ASSET_PARAMETER_ID)?;
        let asset = ParameterSpec::new(
            asset_id.clone(),
            "Sample File",
            ParameterKind::Asset,
            ParameterUpdate::Structural,
            ParameterDefault::Asset(AssetReference::new(
                AssetKind::Sample,
                initial_asset.as_str(),
            )?),
            None,
            Vec::new(),
            None,
            None,
            None,
            "asset",
            None,
            None,
        )?;
        let root_note = continuous_parameter(
            SAMPLE_ROOT_NOTE_PARAMETER_ID,
            "Root Note",
            60.0,
            0.0,
            127.0,
            0.01,
            1.0,
            Some("semitones"),
            "pitch",
            None,
        )?;
        let playback_start =
            normalized_parameter(SAMPLE_PLAYBACK_START_PARAMETER_ID, "Start", 0.0, None)?;
        let playback_end =
            normalized_parameter(SAMPLE_PLAYBACK_END_PARAMETER_ID, "End", 1.0, None)?;
        let loop_mode_id = parameter_id(SAMPLE_LOOP_MODE_PARAMETER_ID)?;
        let loop_mode = ParameterSpec::new_with_patch_interaction(
            loop_mode_id.clone(),
            "Loop",
            ParameterKind::Choice,
            ParameterUpdate::Scalar,
            PatchInteraction::ScalarEdit,
            ParameterDefault::Value(ParameterValue::Choice(SAMPLE_LOOP_OFF_CHOICE_ID.to_owned())),
            None,
            vec![
                ParameterChoice::new(SAMPLE_LOOP_OFF_CHOICE_ID, "OFF")?,
                ParameterChoice::new(SAMPLE_LOOP_FORWARD_CHOICE_ID, "FORWARD")?,
            ],
            None,
            None,
            None,
            "choice",
            None,
            None,
        )?;
        let forward = Some(ParameterPredicate::new(
            loop_mode_id,
            ParameterValue::Choice(SAMPLE_LOOP_FORWARD_CHOICE_ID.to_owned()),
        ));
        let loop_start = normalized_parameter(
            SAMPLE_LOOP_START_PARAMETER_ID,
            "Loop Start",
            0.0,
            forward.clone(),
        )?;
        let loop_end = normalized_parameter(
            SAMPLE_LOOP_END_PARAMETER_ID,
            "Loop End",
            1.0,
            forward.clone(),
        )?;
        let crossfade = continuous_parameter(
            SAMPLE_CROSSFADE_PARAMETER_ID,
            "Crossfade",
            0.0,
            0.0,
            200.0,
            1.0,
            10.0,
            Some("ms"),
            "milliseconds",
            forward,
        )?;
        let descriptor = CapabilityDescriptor::new(
            CapabilityId::new(SAMPLE_CAPABILITY_ID).map_err(|_| {
                CapabilityError::InvalidMetadataIdentifier(SAMPLE_CAPABILITY_ID.to_owned())
            })?,
            "Sample",
            "instrument.sample",
            vec![
                CapabilitySection::new(
                    "sample.playback",
                    "Playback",
                    vec![asset, root_note, playback_start, playback_end],
                )?,
                CapabilitySection::new(
                    "sample.loop",
                    "Loop",
                    vec![loop_mode, loop_start, loop_end, crossfade],
                )?,
            ],
            vec![AssetRequirement::new(asset_id, true)],
            VoicePolicy::Configurable {
                default_voices: SAMPLE_VOICE_COUNT as u16,
            },
            SAMPLE_SUPPORTED_MIDI_KINDS.to_vec(),
        )?
        .with_instrument_category(crate::synth::InstrumentCategory::Samplers)
        .with_visualizations([
            CapabilityVisualization::waveform(
                "sample.waveform",
                "Waveform",
                parameter_id(SAMPLE_ASSET_PARAMETER_ID)?,
                vec![
                    WaveformLandmarkSpec::new(
                        WaveformLandmarkRole::PlaybackStart,
                        parameter_id(SAMPLE_PLAYBACK_START_PARAMETER_ID)?,
                    ),
                    WaveformLandmarkSpec::new(
                        WaveformLandmarkRole::PlaybackEnd,
                        parameter_id(SAMPLE_PLAYBACK_END_PARAMETER_ID)?,
                    ),
                    WaveformLandmarkSpec::new(
                        WaveformLandmarkRole::LoopStart,
                        parameter_id(SAMPLE_LOOP_START_PARAMETER_ID)?,
                    ),
                    WaveformLandmarkSpec::new(
                        WaveformLandmarkRole::LoopEnd,
                        parameter_id(SAMPLE_LOOP_END_PARAMETER_ID)?,
                    ),
                ],
            )?,
            CapabilityVisualization::status("sample.status", "Preparation")?,
        ])?;
        Ok(Self { descriptor })
    }

    pub fn default_config(&self) -> Result<InstrumentConfig, CapabilityError> {
        let values = self
            .descriptor
            .parameters()
            .filter_map(|parameter| match parameter.default_value() {
                ParameterDefault::Value(value) => Some(ParameterAssignment::new(
                    parameter.id().clone(),
                    value.clone(),
                )),
                ParameterDefault::Asset(_) => None,
            })
            .collect::<Vec<_>>();
        let assets = self
            .descriptor
            .parameters()
            .filter_map(|parameter| match parameter.default_value() {
                ParameterDefault::Asset(reference) => Some(AssetAssignment::new(
                    parameter.id().clone(),
                    reference.clone(),
                )),
                ParameterDefault::Value(_) => None,
            })
            .collect::<Vec<_>>();
        self.create_config(&values, &assets)
    }

    pub fn playback_config(
        &self,
        config: &InstrumentConfig,
    ) -> Result<SamplePlaybackConfig, SampleAssetError> {
        let canonical = self
            .create_config(config.values(), config.asset_references())
            .map_err(|_| SampleAssetError::InvalidLandmark)?;
        if canonical != *config {
            return Err(SampleAssetError::InvalidLandmark);
        }
        let reference = config
            .asset_reference(&sample_parameter_id(SAMPLE_ASSET_PARAMETER_ID)?)
            .filter(|reference| reference.kind() == AssetKind::Sample)
            .ok_or(SampleAssetError::InvalidRelativeId)?;
        let continuous = |id: &str| -> Result<f32, SampleAssetError> {
            match config.value(&sample_parameter_id(id)?) {
                Some(ParameterValue::Continuous(value)) if value.is_finite() => Ok(*value as f32),
                _ => Err(SampleAssetError::InvalidLandmark),
            }
        };
        let loop_mode = match config.value(&sample_parameter_id(SAMPLE_LOOP_MODE_PARAMETER_ID)?) {
            Some(ParameterValue::Choice(choice)) if choice == SAMPLE_LOOP_OFF_CHOICE_ID => {
                SampleLoopMode::Off
            }
            Some(ParameterValue::Choice(choice)) if choice == SAMPLE_LOOP_FORWARD_CHOICE_ID => {
                SampleLoopMode::Forward
            }
            _ => return Err(SampleAssetError::InvalidLandmark),
        };
        let playback = SamplePlaybackConfig {
            asset_id: AssetFileId::new(reference.locator())?,
            root_note: continuous(SAMPLE_ROOT_NOTE_PARAMETER_ID)?,
            playback_start: continuous(SAMPLE_PLAYBACK_START_PARAMETER_ID)?,
            playback_end: continuous(SAMPLE_PLAYBACK_END_PARAMETER_ID)?,
            loop_mode,
            loop_start: continuous(SAMPLE_LOOP_START_PARAMETER_ID)?,
            loop_end: continuous(SAMPLE_LOOP_END_PARAMETER_ID)?,
            crossfade_milliseconds: continuous(SAMPLE_CROSSFADE_PARAMETER_ID)?,
        };
        playback.validate()?;
        Ok(playback)
    }
}

impl InstrumentCapabilityProvider for SampleCapability {
    fn descriptor(&self) -> CapabilityDescriptor {
        self.descriptor.clone()
    }

    fn create_config(
        &self,
        values: &[ParameterAssignment],
        asset_references: &[AssetAssignment],
    ) -> Result<InstrumentConfig, CapabilityError> {
        self.descriptor.create_config(values, asset_references)
    }
}

#[allow(clippy::too_many_arguments)]
fn continuous_parameter(
    id: &str,
    label: &str,
    default: f64,
    minimum: f64,
    maximum: f64,
    fine_step: f64,
    coarse_step: f64,
    unit: Option<&str>,
    formatter: &str,
    enabled_when: Option<ParameterPredicate>,
) -> Result<ParameterSpec, CapabilityError> {
    ParameterSpec::new_with_patch_interaction(
        parameter_id(id)?,
        label,
        ParameterKind::Continuous,
        ParameterUpdate::Scalar,
        PatchInteraction::ScalarEdit,
        ParameterDefault::Value(ParameterValue::continuous(default)?),
        Some(ParameterRange::new(minimum, maximum)?),
        Vec::new(),
        Some(fine_step),
        Some(coarse_step),
        unit.map(str::to_owned),
        formatter,
        enabled_when,
        None,
    )
}

fn normalized_parameter(
    id: &str,
    label: &str,
    default: f64,
    enabled_when: Option<ParameterPredicate>,
) -> Result<ParameterSpec, CapabilityError> {
    continuous_parameter(
        id,
        label,
        default,
        0.0,
        1.0,
        0.001,
        0.01,
        None,
        "normalized",
        enabled_when,
    )
}

fn parameter_id(value: &str) -> Result<ParameterId, CapabilityError> {
    ParameterId::new(value)
        .map_err(|_| CapabilityError::InvalidMetadataIdentifier(value.to_owned()))
}

fn sample_parameter_id(value: &str) -> Result<ParameterId, SampleAssetError> {
    ParameterId::new(value).map_err(|_| SampleAssetError::InvalidLandmark)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> SampleCapability {
        SampleCapability::new(AssetFileId::new("Factory/Init.wav").unwrap()).unwrap()
    }

    #[test]
    fn descriptor_declares_exact_first_slice_schema_dependencies_and_bounds() {
        let provider = provider();
        let descriptor = provider.descriptor();
        assert_eq!(descriptor.id().as_str(), SAMPLE_CAPABILITY_ID);
        assert_eq!(
            descriptor.voice_policy(),
            VoicePolicy::Configurable { default_voices: 16 }
        );
        assert_eq!(descriptor.scalar_parameter_count(), 7);
        assert_eq!(descriptor.asset_requirements().len(), 1);
        assert!(descriptor.asset_requirements()[0].required());
        assert_eq!(
            descriptor
                .parameters()
                .map(|parameter| parameter.id().as_str())
                .collect::<Vec<_>>(),
            [
                SAMPLE_ASSET_PARAMETER_ID,
                SAMPLE_ROOT_NOTE_PARAMETER_ID,
                SAMPLE_PLAYBACK_START_PARAMETER_ID,
                SAMPLE_PLAYBACK_END_PARAMETER_ID,
                SAMPLE_LOOP_MODE_PARAMETER_ID,
                SAMPLE_LOOP_START_PARAMETER_ID,
                SAMPLE_LOOP_END_PARAMETER_ID,
                SAMPLE_CROSSFADE_PARAMETER_ID,
            ]
        );
        let loop_mode = descriptor
            .parameter(&parameter_id(SAMPLE_LOOP_MODE_PARAMETER_ID).unwrap())
            .unwrap();
        assert_eq!(
            loop_mode
                .choices()
                .iter()
                .map(|choice| (choice.id(), choice.label()))
                .collect::<Vec<_>>(),
            [
                (SAMPLE_LOOP_OFF_CHOICE_ID, "OFF"),
                (SAMPLE_LOOP_FORWARD_CHOICE_ID, "FORWARD"),
            ]
        );
        for id in [
            SAMPLE_LOOP_START_PARAMETER_ID,
            SAMPLE_LOOP_END_PARAMETER_ID,
            SAMPLE_CROSSFADE_PARAMETER_ID,
        ] {
            let parameter = descriptor.parameter(&parameter_id(id).unwrap()).unwrap();
            assert_eq!(
                parameter.enabled_when().unwrap().parameter_id().as_str(),
                SAMPLE_LOOP_MODE_PARAMETER_ID
            );
        }
        assert!(matches!(
            &descriptor.visualizations()[0],
            CapabilityVisualization::Envelope { id, label }
                if id == "shared.envelope" && label == "Envelope"
        ));
        let CapabilityVisualization::Waveform {
            id,
            label,
            asset_parameter_id,
            landmarks,
        } = &descriptor.visualizations()[1]
        else {
            panic!("Sample's second informative declaration is its waveform");
        };
        assert_eq!(
            (id.as_str(), label.as_str()),
            ("sample.waveform", "Waveform")
        );
        assert_eq!(asset_parameter_id.as_str(), SAMPLE_ASSET_PARAMETER_ID);
        assert_eq!(
            landmarks
                .iter()
                .map(WaveformLandmarkSpec::role)
                .collect::<Vec<_>>(),
            [
                WaveformLandmarkRole::PlaybackStart,
                WaveformLandmarkRole::PlaybackEnd,
                WaveformLandmarkRole::LoopStart,
                WaveformLandmarkRole::LoopEnd,
            ]
        );
        assert!(matches!(
            &descriptor.visualizations()[2],
            CapabilityVisualization::Status { id, label }
                if id == "sample.status" && label == "Preparation"
        ));
    }

    #[test]
    fn configs_accept_same_kind_relative_asset_and_validate_playback_contract() {
        let provider = provider();
        let mut config = provider.default_config().unwrap();
        let assets = [AssetAssignment::new(
            parameter_id(SAMPLE_ASSET_PARAMETER_ID).unwrap(),
            AssetReference::new(AssetKind::Sample, "User/Snare.wav").unwrap(),
        )];
        config = provider.create_config(config.values(), &assets).unwrap();
        let playback = provider.playback_config(&config).unwrap();
        assert_eq!(playback.asset_id.as_str(), "User/Snare.wav");
        assert_eq!(playback.root_note, 60.0);
        assert_eq!(playback.playback_start, 0.0);
        assert_eq!(playback.playback_end, 1.0);
        assert_eq!(playback.loop_mode, SampleLoopMode::Off);
    }
}
