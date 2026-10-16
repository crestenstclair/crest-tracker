use crate::kernel::midi_message::MidiMessageKind;
use crate::synth::capability_id::CapabilityId;
use crate::synth::instrument_capability::{
    AssetAssignment, CapabilityDescriptor, CapabilityError, CapabilitySection, InstrumentConfig,
    ParameterAssignment, ParameterChoice, ParameterDefault, ParameterKind, ParameterRange,
    ParameterSpec, ParameterUpdate, ParameterValue, PatchInteraction, VoicePolicy,
};
use crate::synth::instrument_capability_provider::InstrumentCapabilityProvider;
use crate::synth::parameter_id::ParameterId;

pub const BRAIDS_CAPABILITY_ID: &str = "instrument.braids";
pub const BRAIDS_MODEL_PARAMETER_ID: &str = "braids.model";
pub const BRAIDS_TIMBRE_PARAMETER_ID: &str = "braids.timbre";
pub const BRAIDS_COLOR_PARAMETER_ID: &str = "braids.color";
pub const BRAIDS_FIXED_VOICES: u16 = 16;

pub const BRAIDS_SUPPORTED_MIDI_KINDS: [MidiMessageKind; 7] = [
    MidiMessageKind::NoteOn,
    MidiMessageKind::NoteOff,
    MidiMessageKind::ControlChange,
    MidiMessageKind::ChannelPressure,
    MidiMessageKind::PolyPressure,
    MidiMessageKind::PitchBend,
    MidiMessageKind::AllNotesOff,
];

/// Stable source-order identity and readable label for one playable upstream model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BraidsModel {
    pub id: &'static str,
    pub label: &'static str,
}

pub const BRAIDS_MODELS: [BraidsModel; 47] = [
    BraidsModel {
        id: "braids.model.csaw",
        label: "CSAW",
    },
    BraidsModel {
        id: "braids.model.morph",
        label: "Morph",
    },
    BraidsModel {
        id: "braids.model.saw-square",
        label: "Saw / Square",
    },
    BraidsModel {
        id: "braids.model.sine-triangle",
        label: "Sine / Triangle",
    },
    BraidsModel {
        id: "braids.model.buzz",
        label: "Buzz",
    },
    BraidsModel {
        id: "braids.model.square-sub",
        label: "Square Sub",
    },
    BraidsModel {
        id: "braids.model.saw-sub",
        label: "Saw Sub",
    },
    BraidsModel {
        id: "braids.model.square-sync",
        label: "Square Sync",
    },
    BraidsModel {
        id: "braids.model.saw-sync",
        label: "Saw Sync",
    },
    BraidsModel {
        id: "braids.model.triple-saw",
        label: "Triple Saw",
    },
    BraidsModel {
        id: "braids.model.triple-square",
        label: "Triple Square",
    },
    BraidsModel {
        id: "braids.model.triple-triangle",
        label: "Triple Triangle",
    },
    BraidsModel {
        id: "braids.model.triple-sine",
        label: "Triple Sine",
    },
    BraidsModel {
        id: "braids.model.triple-ring-mod",
        label: "Triple Ring Mod",
    },
    BraidsModel {
        id: "braids.model.saw-swarm",
        label: "Saw Swarm",
    },
    BraidsModel {
        id: "braids.model.saw-comb",
        label: "Saw Comb",
    },
    BraidsModel {
        id: "braids.model.toy",
        label: "Toy",
    },
    BraidsModel {
        id: "braids.model.digital-filter-lp",
        label: "Digital Filter LP",
    },
    BraidsModel {
        id: "braids.model.digital-filter-pk",
        label: "Digital Filter Peak",
    },
    BraidsModel {
        id: "braids.model.digital-filter-bp",
        label: "Digital Filter BP",
    },
    BraidsModel {
        id: "braids.model.digital-filter-hp",
        label: "Digital Filter HP",
    },
    BraidsModel {
        id: "braids.model.vosim",
        label: "VOSIM",
    },
    BraidsModel {
        id: "braids.model.vowel",
        label: "Vowel",
    },
    BraidsModel {
        id: "braids.model.vowel-fof",
        label: "Vowel FOF",
    },
    BraidsModel {
        id: "braids.model.harmonics",
        label: "Harmonics",
    },
    BraidsModel {
        id: "braids.model.fm",
        label: "FM",
    },
    BraidsModel {
        id: "braids.model.feedback-fm",
        label: "Feedback FM",
    },
    BraidsModel {
        id: "braids.model.chaotic-feedback-fm",
        label: "Chaotic Feedback FM",
    },
    BraidsModel {
        id: "braids.model.plucked",
        label: "Plucked",
    },
    BraidsModel {
        id: "braids.model.bowed",
        label: "Bowed",
    },
    BraidsModel {
        id: "braids.model.blown",
        label: "Blown",
    },
    BraidsModel {
        id: "braids.model.fluted",
        label: "Fluted",
    },
    BraidsModel {
        id: "braids.model.struck-bell",
        label: "Struck Bell",
    },
    BraidsModel {
        id: "braids.model.struck-drum",
        label: "Struck Drum",
    },
    BraidsModel {
        id: "braids.model.kick",
        label: "Kick",
    },
    BraidsModel {
        id: "braids.model.cymbal",
        label: "Cymbal",
    },
    BraidsModel {
        id: "braids.model.snare",
        label: "Snare",
    },
    BraidsModel {
        id: "braids.model.wavetables",
        label: "Wavetables",
    },
    BraidsModel {
        id: "braids.model.wave-map",
        label: "Wave Map",
    },
    BraidsModel {
        id: "braids.model.wave-line",
        label: "Wave Line",
    },
    BraidsModel {
        id: "braids.model.wave-paraphonic",
        label: "Wave Paraphonic",
    },
    BraidsModel {
        id: "braids.model.filtered-noise",
        label: "Filtered Noise",
    },
    BraidsModel {
        id: "braids.model.twin-peaks-noise",
        label: "Twin Peaks Noise",
    },
    BraidsModel {
        id: "braids.model.clocked-noise",
        label: "Clocked Noise",
    },
    BraidsModel {
        id: "braids.model.granular-cloud",
        label: "Granular Cloud",
    },
    BraidsModel {
        id: "braids.model.particle-noise",
        label: "Particle Noise",
    },
    BraidsModel {
        id: "braids.model.qpsk",
        label: "QPSK",
    },
];

/// Control-side descriptor/config provider for the pinned Braids engine.
#[derive(Clone, Debug)]
pub struct BraidsCapability {
    descriptor: CapabilityDescriptor,
}

impl BraidsCapability {
    pub fn new() -> Result<Self, CapabilityError> {
        let model = ParameterSpec::new_with_patch_interaction(
            parameter_id(BRAIDS_MODEL_PARAMETER_ID)?,
            "Model",
            ParameterKind::Choice,
            ParameterUpdate::Scalar,
            PatchInteraction::ScalarEdit,
            ParameterDefault::Value(ParameterValue::Choice(BRAIDS_MODELS[0].id.to_owned())),
            None,
            BRAIDS_MODELS
                .iter()
                .map(|model| ParameterChoice::new(model.id, model.label))
                .collect::<Result<Vec<_>, _>>()?,
            None,
            None,
            None,
            "choice",
            None,
            None,
        )?;
        let timbre = continuous_parameter(BRAIDS_TIMBRE_PARAMETER_ID, "Timbre", 0.5)?;
        let color = continuous_parameter(BRAIDS_COLOR_PARAMETER_ID, "Color", 0.5)?;
        let descriptor = CapabilityDescriptor::new(
            CapabilityId::new(BRAIDS_CAPABILITY_ID).map_err(|_| {
                CapabilityError::InvalidMetadataIdentifier(BRAIDS_CAPABILITY_ID.to_owned())
            })?,
            "Mutable Instruments Braids",
            "instrument.braids",
            vec![CapabilitySection::new(
                "oscillator",
                "Oscillator",
                vec![model, timbre, color],
            )?],
            Vec::new(),
            VoicePolicy::Configurable {
                default_voices: BRAIDS_FIXED_VOICES,
            },
            BRAIDS_SUPPORTED_MIDI_KINDS.to_vec(),
        )?;
        Ok(Self { descriptor })
    }

    pub fn default_config(&self) -> Result<InstrumentConfig, CapabilityError> {
        self.create_config(
            &[
                ParameterAssignment::new(
                    parameter_id(BRAIDS_MODEL_PARAMETER_ID)?,
                    ParameterValue::Choice(BRAIDS_MODELS[0].id.to_owned()),
                ),
                ParameterAssignment::new(
                    parameter_id(BRAIDS_TIMBRE_PARAMETER_ID)?,
                    ParameterValue::continuous(0.5)?,
                ),
                ParameterAssignment::new(
                    parameter_id(BRAIDS_COLOR_PARAMETER_ID)?,
                    ParameterValue::continuous(0.5)?,
                ),
            ],
            &[],
        )
    }
}

impl InstrumentCapabilityProvider for BraidsCapability {
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

fn continuous_parameter(
    id: &str,
    label: &str,
    default: f64,
) -> Result<ParameterSpec, CapabilityError> {
    ParameterSpec::new_with_patch_interaction(
        parameter_id(id)?,
        label,
        ParameterKind::Continuous,
        ParameterUpdate::Scalar,
        PatchInteraction::ScalarEdit,
        ParameterDefault::Value(ParameterValue::continuous(default)?),
        Some(ParameterRange::new(0.0, 1.0)?),
        Vec::new(),
        Some(0.01),
        Some(0.1),
        None,
        "normalized",
        None,
        None,
    )
}

fn parameter_id(value: &str) -> Result<ParameterId, CapabilityError> {
    ParameterId::new(value)
        .map_err(|_| CapabilityError::InvalidMetadataIdentifier(value.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn braids_capability_declares_all_playable_models_and_exact_scalar_shape() {
        let provider = BraidsCapability::new().unwrap();
        let descriptor = provider.descriptor();
        assert_eq!(descriptor.id().as_str(), BRAIDS_CAPABILITY_ID);
        assert_eq!(
            descriptor.voice_policy(),
            VoicePolicy::Configurable { default_voices: 16 }
        );
        assert_eq!(
            descriptor.supported_midi_kinds(),
            BRAIDS_SUPPORTED_MIDI_KINDS
        );
        assert!(descriptor.asset_requirements().is_empty());
        assert_eq!(descriptor.scalar_parameter_count(), 3);

        let parameters = descriptor.sections()[0].parameters();
        assert_eq!(
            parameters
                .iter()
                .map(|parameter| parameter.id().as_str())
                .collect::<Vec<_>>(),
            [
                BRAIDS_MODEL_PARAMETER_ID,
                BRAIDS_TIMBRE_PARAMETER_ID,
                BRAIDS_COLOR_PARAMETER_ID,
            ]
        );
        assert_eq!(parameters[0].choices().len(), 47);
        assert_eq!(parameters[0].choices().first().unwrap().label(), "CSAW");
        assert_eq!(parameters[0].choices().last().unwrap().label(), "QPSK");
        assert!(parameters
            .iter()
            .all(|parameter| parameter.update() == ParameterUpdate::Scalar));
        assert!(parameters
            .iter()
            .all(|parameter| parameter.patch_interaction() == PatchInteraction::ScalarEdit));
        assert!(parameters[1..].iter().all(|parameter| {
            parameter.kind() == ParameterKind::Continuous
                && parameter.range() == Some(ParameterRange::new(0.0, 1.0).unwrap())
        }));
    }

    #[test]
    fn braids_config_is_canonical_and_never_substitutes_bad_values() {
        let provider = BraidsCapability::new().unwrap();
        let config = provider.default_config().unwrap();
        assert_eq!(config.values().len(), 3);
        assert!(config.asset_references().is_empty());
        assert_eq!(
            config.value(&parameter_id(BRAIDS_MODEL_PARAMETER_ID).unwrap()),
            Some(&ParameterValue::Choice(BRAIDS_MODELS[0].id.to_owned()))
        );

        let malformed = [
            ParameterAssignment::new(
                parameter_id(BRAIDS_MODEL_PARAMETER_ID).unwrap(),
                ParameterValue::Choice("braids.model.question-mark".to_owned()),
            ),
            ParameterAssignment::new(
                parameter_id(BRAIDS_TIMBRE_PARAMETER_ID).unwrap(),
                ParameterValue::continuous(0.5).unwrap(),
            ),
            ParameterAssignment::new(
                parameter_id(BRAIDS_COLOR_PARAMETER_ID).unwrap(),
                ParameterValue::continuous(0.5).unwrap(),
            ),
        ];
        assert!(matches!(
            provider.create_config(&malformed, &[]),
            Err(CapabilityError::UnknownChoice(_))
        ));

        let missing_color = &config.values()[..2];
        assert!(matches!(
            provider.create_config(missing_color, &[]),
            Err(CapabilityError::MissingParameter(_))
        ));
        let out_of_range = [
            config.values()[0].clone(),
            config.values()[1].clone(),
            ParameterAssignment::new(
                parameter_id(BRAIDS_COLOR_PARAMETER_ID).unwrap(),
                ParameterValue::continuous(1.01).unwrap(),
            ),
        ];
        assert!(matches!(
            provider.create_config(&out_of_range, &[]),
            Err(CapabilityError::ValueOutOfRange(_))
        ));
    }
}
