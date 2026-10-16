use crate::control::app_state::AppState;
use crate::control::{
    EngineSelectionStatus, FocusPath, InteractionMode, MidiInputState, PatchDetailSubject,
    ReturnPath,
};
use crate::mixer::bus_id::BusId;
use crate::mixer::bus_return::BusReturnBank;
use crate::mixer::global_parameters::GlobalParameters;
use crate::mixer::mixer_state::MixerState;
use crate::mixer::mixer_track_id::MixerTrackId;
use crate::mixer::mixer_track_parameters::MixerTrackParameter;
use crate::mixer::patch_output::PatchOutput;
use crate::synth::instrument_capability::{CapabilityRegistry, InstrumentConfig};
use crate::synth::patch::Patch;
use crate::synth::voice_envelope::VoiceEnvelope;
use crate::synth::{EffectCapabilityRegistry, PostEffectConfig};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// Canonical serde representation shared by snapshots, text, and observation trees.
///
/// Production projection borrows immutable registry, Patch, and config storage.
/// Deserialization owns the same shape for round-trip and external-snapshot tests.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SerializedState<'a> {
    pub(crate) generation: u64,
    #[serde(borrow)]
    pub(crate) capabilities: Cow<'a, CapabilityRegistry>,
    #[serde(borrow, default)]
    pub(crate) effects: Cow<'a, EffectCapabilityRegistry>,
    #[serde(borrow)]
    pub(crate) patches: Vec<SerializedPatch<'a>>,
    pub(crate) mixer: MixerState,
    pub(crate) global: SerializedGlobalParameters,
    #[serde(default)]
    pub(crate) returns: SerializedBusReturns,
    #[serde(default)]
    pub(crate) interaction: SerializedInteractionState,
    pub(crate) engine_selection: EngineSelectionStatus,
    #[serde(default)]
    pub(crate) midi_input: MidiInputState,
    #[serde(default)]
    pub(crate) controller: crate::control::ControllerState,
}

impl<'a> From<&'a AppState> for SerializedState<'a> {
    fn from(state: &'a AppState) -> Self {
        Self {
            generation: state.generation(),
            capabilities: Cow::Borrowed(state.capabilities()),
            effects: Cow::Borrowed(state.effects()),
            patches: state.patches().iter().map(SerializedPatch::from).collect(),
            mixer: state.mixer().clone(),
            global: SerializedGlobalParameters::from(state.global()),
            returns: SerializedBusReturns::from(state.bus_returns()),
            interaction: SerializedInteractionState::from_state(state),
            engine_selection: state.engine_selection().clone(),
            midi_input: state.midi_input().clone(),
            controller: state.controller().clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SerializedInteractionState {
    #[serde(default = "default_send_focus")]
    pub(crate) remembered_send: FocusPath,
    #[serde(default)]
    pub(crate) send_choice_origin: Option<FocusPath>,
    #[serde(default)]
    pub(crate) send_name_editing: bool,
    #[serde(default = "default_mixer_focus")]
    pub(crate) active_focus: FocusPath,
    #[serde(default)]
    pub(crate) remembered_patch_main: Option<FocusPath>,
    #[serde(default = "default_mixer_focus")]
    pub(crate) remembered_mixer_main: FocusPath,
    #[serde(default)]
    pub(crate) mode: InteractionMode,
    #[serde(default)]
    pub(crate) return_path: Option<ReturnPath>,
    /// The capability an open detail surface was opened on.
    ///
    /// Carried for the same reason `voiceLimit` is: it is reducer-owned state
    /// that decides what is on screen, and a trace that cannot show which
    /// capability a detail surface was opened on cannot correlate a detail
    /// interaction with its consequence.
    #[serde(default)]
    pub(crate) detail_subject: Option<PatchDetailSubject>,
}

impl Default for SerializedInteractionState {
    fn default() -> Self {
        Self {
            remembered_send: default_send_focus(),
            send_choice_origin: None,
            send_name_editing: false,
            active_focus: default_mixer_focus(),
            remembered_patch_main: None,
            remembered_mixer_main: default_mixer_focus(),
            mode: InteractionMode::Navigate,
            return_path: None,
            detail_subject: None,
        }
    }
}

impl SerializedInteractionState {
    fn from_state(state: &AppState) -> Self {
        let interaction = state.interaction();
        Self {
            remembered_send: interaction.remembered_send.clone(),
            send_choice_origin: interaction.send_choice_origin().cloned(),
            send_name_editing: interaction.send_name_editing(),
            active_focus: interaction.focus_path().clone(),
            remembered_patch_main: interaction.remembered_patch_main().cloned(),
            remembered_mixer_main: interaction.remembered_mixer_main().clone(),
            mode: interaction.mode(),
            return_path: interaction.return_path().cloned(),
            detail_subject: interaction.detail_subject().cloned(),
        }
    }
}

fn default_send_focus() -> FocusPath {
    FocusPath::send(
        crate::control::SendControlId::Name {
            bus: crate::mixer::bus_id::BusId::default(),
        },
        None,
    )
}

fn default_mixer_focus() -> FocusPath {
    FocusPath::mixer_track(MixerTrackId::default(), MixerTrackParameter::Level)
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SerializedPatch<'a> {
    pub(crate) id: u32,
    #[serde(borrow)]
    pub(crate) name: Cow<'a, str>,
    pub(crate) channel: u8,
    #[serde(borrow)]
    pub(crate) instrument: Cow<'a, InstrumentConfig>,
    #[serde(borrow, default)]
    pub(crate) post_effects: Cow<'a, [PostEffectConfig]>,
    #[serde(default)]
    pub(crate) envelope: VoiceEnvelope,
    pub(crate) output: PatchOutput,
    #[serde(borrow, default)]
    pub(crate) sends: Cow<'a, [f32]>,
}

impl<'a> From<&'a Patch> for SerializedPatch<'a> {
    fn from(patch: &'a Patch) -> Self {
        Self {
            id: patch.id().value(),
            name: Cow::Borrowed(patch.name()),
            channel: patch.channel().value(),
            instrument: Cow::Borrowed(patch.instrument_config()),
            // The frozen `postEffects` leaf shape: the occupied positions of
            // the per-position chain in position order. Each entry carries
            // its stable slot identity, so true positions survive the dense
            // serialized shape; the payload is derived once for output and
            // never indexed back into by dense position.
            post_effects: Cow::Owned(patch.effect_slots().iter().flatten().cloned().collect()),
            envelope: *patch.envelope(),
            output: patch.output(),
            sends: Cow::Borrowed(patch.sends()),
        }
    }
}

/// The one genuinely global serialized value.
///
/// The retired reverb/delay fields are gone from this shape: their state is
/// return-owned and travels as the indexed `returns` section.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SerializedGlobalParameters {
    pub(crate) master_gain_db: f32,
}

impl From<&GlobalParameters> for SerializedGlobalParameters {
    fn from(parameters: &GlobalParameters) -> Self {
        Self {
            master_gain_db: parameters.master_gain_db(),
        }
    }
}

/// One named ordered return chain plus its output level. Array position is the
/// `BusId`; `effect` preserves the first-occupant observation for older readers.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SerializedBusReturn {
    #[serde(default = "default_return_name")]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) effects: Vec<PostEffectConfig>,
    #[serde(default)]
    pub(crate) effect: Option<PostEffectConfig>,
    pub(crate) return_level: f32,
}

fn default_return_name() -> String {
    "INIT".to_owned()
}

/// The configured serialized return bank in ascending `BusId` order.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct SerializedBusReturns(pub(crate) Vec<SerializedBusReturn>);

impl Default for SerializedBusReturns {
    fn default() -> Self {
        Self::from(&BusReturnBank::default())
    }
}

impl From<&BusReturnBank> for SerializedBusReturns {
    fn from(bank: &BusReturnBank) -> Self {
        Self(
            bank.returns()
                .iter()
                .map(|bus_return| SerializedBusReturn {
                    name: bus_return.name().to_owned(),
                    effects: bus_return.effects().to_vec(),
                    effect: bus_return.effect().cloned(),
                    return_level: bus_return.return_level(),
                })
                .collect(),
        )
    }
}

impl SerializedBusReturns {
    /// Returns the serialized entries in ascending `BusId` order.
    pub(crate) fn entries(&self) -> &[SerializedBusReturn] {
        &self.0
    }

    /// Reports whether every entry has a representable positional identity.
    pub(crate) fn is_complete(&self) -> bool {
        !self.0.is_empty() && self.0.len() <= usize::from(BusId::MAX) + 1
    }
}

#[cfg(test)]
mod tests {
    use super::{SerializedBusReturns, SerializedPatch};
    use crate::adapter::braids_capability::BraidsCapability;
    use crate::adapter::production_effects::production_chorus_config;
    use crate::kernel::midi_channel::MidiChannel;
    use crate::kernel::patch_id::PatchId;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::patch_output::PatchOutput;
    use crate::synth::effect_slot_id::EffectSlotIndex;
    use crate::synth::patch::Patch;
    use crate::synth::EffectSlotId;

    fn test_patch() -> Patch {
        Patch::new(
            PatchId::new(1).unwrap(),
            "Serialized".to_owned(),
            BraidsCapability::new().unwrap().default_config().unwrap(),
            MidiChannel::new(0).unwrap(),
            PatchOutput::new(MixerTrackId::new(0).unwrap(), -6.0).unwrap(),
        )
    }

    #[test]
    fn patch_sends_preserve_configured_count_and_levels_in_serialization() {
        let mut sends = vec![0.0; 20];
        sends[0] = 0.125;
        sends[19] = 0.75;
        let patch = test_patch().with_sends(sends.clone()).unwrap();
        let serialized = SerializedPatch::from(&patch);
        assert_eq!(serialized.sends.as_ref(), sends.as_slice());
        let json = serde_json::to_string(&serialized).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["sends"], serde_json::json!(sends));
        let restored: SerializedPatch<'_> = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.sends, serialized.sends);
    }

    #[test]
    fn gapped_chain_serializes_the_occupant_with_its_stable_identity() {
        // Slot 0 empty, slot 1 occupied: the shape a compacting accessor
        // used to silently squeeze down to position 0.
        let mut patch = test_patch();
        patch
            .set_slot_occupancy(
                EffectSlotIndex::new(1).unwrap(),
                Some(production_chorus_config(EffectSlotId::new(2).unwrap()).unwrap()),
            )
            .unwrap();

        let serialized = SerializedPatch::from(&patch);

        // The frozen dense `postEffects` payload lists exactly the occupied
        // configuration, and its stable slot identity still names position
        // 1 — an identity of 1 here would mean serialization renumbered the
        // chain.
        assert_eq!(serialized.post_effects.len(), 1);
        assert_eq!(serialized.post_effects[0].slot_id().value(), 2);
        let json = serde_json::to_value(&serialized).unwrap();
        assert_eq!(json["postEffects"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn interior_gap_serializes_neighbors_in_position_order() {
        let mut patch = test_patch();
        patch
            .set_slot_occupancy(
                EffectSlotIndex::new(0).unwrap(),
                Some(production_chorus_config(EffectSlotId::new(1).unwrap()).unwrap()),
            )
            .unwrap();
        patch
            .set_slot_occupancy(
                EffectSlotIndex::new(2).unwrap(),
                Some(production_chorus_config(EffectSlotId::new(3).unwrap()).unwrap()),
            )
            .unwrap();

        let serialized = SerializedPatch::from(&patch);

        // Occupants surround an empty middle position: the payload keeps
        // position order and both stable identities, so the gap is fully
        // recoverable from the dense serialized shape.
        assert_eq!(serialized.post_effects.len(), 2);
        assert_eq!(serialized.post_effects[0].slot_id().value(), 1);
        assert_eq!(serialized.post_effects[1].slot_id().value(), 3);
    }

    #[test]
    fn named_return_chain_preserves_order_and_first_effect_observation() {
        use crate::mixer::bus_id::BusId;
        use crate::mixer::bus_return::{BusReturn, BusReturnBank};

        let bus = BusId::new(18).unwrap();
        let effects = [9, 2]
            .into_iter()
            .map(|id| production_chorus_config(EffectSlotId::new(id).unwrap()).unwrap())
            .collect::<Vec<_>>();
        let mut bank = BusReturnBank::with_count(19).unwrap();
        bank.replace_return(
            BusReturn::unoccupied(bus)
                .with_name("Room")
                .unwrap()
                .with_effects(effects.clone())
                .unwrap(),
        )
        .unwrap();
        let serialized = SerializedBusReturns::from(&bank);
        assert!(serialized.is_complete());
        assert_eq!(serialized.entries().len(), 19);
        assert_eq!(serialized.entries()[18].name, "Room");
        assert_eq!(serialized.entries()[18].effects, effects);
        assert_eq!(serialized.entries()[18].effect.as_ref(), effects.first());
        let json = serde_json::to_string(&serialized).unwrap();
        assert_eq!(
            serde_json::from_str::<SerializedBusReturns>(&json).unwrap(),
            serialized
        );
    }
}
