use crate::adapter::hidef_soundfont_capability::HiDefSoundFontCapability;
use crate::adapter::lock_free_audio_boundary::{
    LockFreeAudioBoundary, LockFreeAudioHandle, LockFreeControlHandle,
};
use crate::control::app_event::{AppEvent, Direction};
use crate::control::app_loop::AppLoop;
use crate::control::app_state::AppState;
use crate::control::event_record::{EventInput, EventSource};
use crate::control::state_projector::StateProjector;
use crate::kernel::midi_channel::MidiChannel;
use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crate::kernel::patch_id::PatchId;
use crate::mixer::bus_id::{BusId, MAX_BUS_RETURNS};
use crate::mixer::global_parameters::GlobalParameters;
use crate::mixer::mix_engine::MixEngine;
use crate::mixer::mixer_state::MixerState;
use crate::mixer::mixer_track_id::MixerTrackId;
use crate::mixer::mixer_track_parameters::MixerTrackParameters;
use crate::mixer::patch_output::PatchOutput;
use crate::real_time::audio_boundary::{AudioBoundary, AudioThreadBoundary};
use crate::real_time::audio_command::AudioCommand;
use crate::real_time::audio_renderer::AudioRenderer;
use crate::real_time::graph_revision::GraphRevision;
use crate::real_time::parameter_snapshot::{
    ParameterSnapshot, RtBusReturnParameters, RtPatchParameters, RtPostEffectParameters,
};
use crate::real_time::patch_audio_block::PatchAudioBlock;
use crate::real_time::prepared_graph_builder::PreparedGraphBuilder;
use crate::real_time::structural_graph_boundary::NoStructuralGraphChanges;
use crate::shell::keyboard_input_translator::KeyboardInputTranslator;
use crate::shell::window_input::{WindowInput, WindowKey};
use crate::synth::patch::Patch;
use crate::synth::sound_font_instrument::SoundFontInstrument;
use crate::synth::{
    CapabilityId, EffectSlotId, InstrumentPreparationError, InstrumentPreparer,
    PreparedEffectError, PreparedInstrument, PreparedInstrumentError, PreparedPostEffect,
};
use crate::testing::automatic_midi_test::create_soundfont_config;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

const FRAME_COUNT: usize = 16;
const SAMPLE_RATE: f32 = 48_000.0;
const ENERGY_EPSILON: f64 = 1.0e-12;

/// One focused healthy-or-mutant verification case.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BehavioralMutationCase {
    DroppedAdjustment,
    CrossTrackParameterLeak,
    PatchMisroute,
    OmittedStateTreeLeaf,
    DryToWetBypass,
    ZeroRenderer,
    SlotOrderSwap,
    EmptyReturnPassthrough,
    PreGateSend,
    MutedSendLeak,
    PermissiveStructuralMatch,
    RefusedTopology,
}

impl BehavioralMutationCase {
    pub const ALL: [Self; 12] = [
        Self::DroppedAdjustment,
        Self::CrossTrackParameterLeak,
        Self::PatchMisroute,
        Self::OmittedStateTreeLeaf,
        Self::DryToWetBypass,
        Self::ZeroRenderer,
        Self::SlotOrderSwap,
        Self::EmptyReturnPassthrough,
        Self::PreGateSend,
        Self::MutedSendLeak,
        Self::PermissiveStructuralMatch,
        Self::RefusedTopology,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DroppedAdjustment => "dropped-adjustment",
            Self::CrossTrackParameterLeak => "cross-track-parameter-leak",
            Self::PatchMisroute => "patch-misroute",
            Self::OmittedStateTreeLeaf => "omitted-state-tree-leaf",
            Self::DryToWetBypass => "dry-to-wet-bypass",
            Self::ZeroRenderer => "zero-renderer",
            Self::SlotOrderSwap => "slot-order-swap",
            Self::EmptyReturnPassthrough => "empty-return-passthrough",
            Self::PreGateSend => "pre-gate-send",
            Self::MutedSendLeak => "muted-send-leak",
            Self::PermissiveStructuralMatch => "permissive-structural-match",
            Self::RefusedTopology => "refused-topology",
        }
    }

    /// The CLI name of this case's typed counterexample. Most mutants share
    /// their case's name; the refused-topology mutant names the exact fault
    /// it injects — the refused change being published anyway.
    pub const fn mutant_cli(self) -> &'static str {
        match self {
            Self::RefusedTopology => "refused-topology-published",
            _ => self.as_str(),
        }
    }

    pub fn from_cli(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.as_str() == value)
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::from_cli(value)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DroppedAdjustmentObservation {
    pub case: String,
    pub adjustment_dispatched: bool,
    pub adjust_event_recorded: bool,
    pub selected_value_exact: bool,
    pub unrelated_values_unchanged: bool,
    pub projection_values_exact: bool,
    pub baseline_restored: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CrossTrackParameterLeakObservation {
    pub case: String,
    pub edited_track_id: u32,
    pub comparison_track_id: u32,
    pub track_ids_distinct: bool,
    pub parameter: String,
    pub parameter_cases_exercised: usize,
    pub edited_value_before: f64,
    pub edited_value_after: f64,
    pub comparison_value_before: f64,
    pub comparison_value_after: f64,
    pub published_edited_value: f64,
    pub published_comparison_value: f64,
    pub edited_track_energy_before: f64,
    pub edited_track_energy_after: f64,
    pub comparison_track_energy_before: f64,
    pub comparison_track_energy_after: f64,
    pub edited_value_changed: bool,
    pub comparison_value_unchanged: bool,
    pub state_values_exact: bool,
    pub published_values_exact: bool,
    pub edited_track_audio_changed: bool,
    pub unedited_track_audio_unchanged: bool,
    pub all_track_parameters_isolated: bool,
    pub dry_path_isolated: bool,
    pub reverb_path_isolated: bool,
    pub delay_path_isolated: bool,
    pub baseline_restored: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PatchMisrouteObservation {
    pub case: String,
    pub command_patch_matches_event: bool,
    pub target_patch_received_command: bool,
    pub target_stem_changed: bool,
    pub untargeted_stems_unchanged: bool,
    pub patch_routing_exact: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OmittedStateTreeLeafObservation {
    pub case: String,
    pub schema_surface_equal: bool,
    pub required_leaf_count: usize,
    pub missing_leaf_count: usize,
    pub unexpected_leaf_count: usize,
    pub state_values_exact: bool,
    pub projection_values_exact: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DryToWetBypassObservation {
    pub case: String,
    pub dry_input_energy: f64,
    pub zero_send_reverb_input_energy: f64,
    pub zero_send_delay_input_energy: f64,
    pub zero_send_wet_output_energy: f64,
    pub nonzero_send_reverb_input_energy: f64,
    pub nonzero_send_delay_input_energy: f64,
    pub nonzero_send_wet_output_energy: f64,
    pub identical_effect_state: bool,
    pub dry_bypass_absent: bool,
    pub finite_audio: bool,
    pub baseline_restored: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ZeroRendererObservation {
    pub case: String,
    pub control_trace_complete: bool,
    pub renderer_called: bool,
    pub renderer_nonzero: bool,
    pub render_peak: f64,
    pub finite_audio: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SlotOrderSwapObservation {
    pub case: String,
    pub forward_energy: f64,
    pub reversed_energy: f64,
    pub order_difference_energy: f64,
    pub order_sensitive: bool,
    pub finite_audio: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EmptyReturnPassthroughObservation {
    pub case: String,
    pub accumulated_send_energy: f64,
    pub unoccupied_wet_energy: f64,
    pub output_matches_dry_exactly: bool,
    pub unoccupied_return_silent: bool,
    pub finite_audio: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PreGateSendObservation {
    pub case: String,
    pub post_fader_reference_energy: f64,
    pub measured_send_energy: f64,
    pub pre_fader_reference_energy: f64,
    pub send_taken_post_fader: bool,
    pub finite_audio: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MutedSendLeakObservation {
    pub case: String,
    pub sounding_send_energy: f64,
    pub muted_send_energy: f64,
    pub muted_wet_energy: f64,
    pub mute_gates_sends: bool,
    pub finite_audio: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PermissiveStructuralMatchObservation {
    pub case: String,
    pub attested_wet_energy: f64,
    pub mismatched_wet_energy: f64,
    pub strict_matching_enforced: bool,
    pub finite_audio: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RefusedTopologyObservation {
    pub case: String,
    pub refusal_recorded: bool,
    pub rejection_reason: String,
    pub rejection_reason_attributable: bool,
    pub active_graph_preserved: bool,
    pub canonical_state_preserved: bool,
    pub render_preserved_exactly: bool,
    pub post_rejection_valid_change_accepted: bool,
    pub finite_audio: bool,
}

/// The exact schema selected by one mutation case.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum BehavioralMutationObservation {
    DroppedAdjustment(DroppedAdjustmentObservation),
    CrossTrackParameterLeak(CrossTrackParameterLeakObservation),
    PatchMisroute(PatchMisrouteObservation),
    OmittedStateTreeLeaf(OmittedStateTreeLeafObservation),
    DryToWetBypass(DryToWetBypassObservation),
    ZeroRenderer(ZeroRendererObservation),
    SlotOrderSwap(SlotOrderSwapObservation),
    EmptyReturnPassthrough(EmptyReturnPassthroughObservation),
    PreGateSend(PreGateSendObservation),
    MutedSendLeak(MutedSendLeakObservation),
    PermissiveStructuralMatch(PermissiveStructuralMatchObservation),
    RefusedTopology(RefusedTopologyObservation),
}

impl BehavioralMutationObservation {
    pub fn case(&self) -> BehavioralMutationCase {
        match self {
            Self::DroppedAdjustment(_) => BehavioralMutationCase::DroppedAdjustment,
            Self::CrossTrackParameterLeak(_) => BehavioralMutationCase::CrossTrackParameterLeak,
            Self::PatchMisroute(_) => BehavioralMutationCase::PatchMisroute,
            Self::OmittedStateTreeLeaf(_) => BehavioralMutationCase::OmittedStateTreeLeaf,
            Self::DryToWetBypass(_) => BehavioralMutationCase::DryToWetBypass,
            Self::ZeroRenderer(_) => BehavioralMutationCase::ZeroRenderer,
            Self::SlotOrderSwap(_) => BehavioralMutationCase::SlotOrderSwap,
            Self::EmptyReturnPassthrough(_) => BehavioralMutationCase::EmptyReturnPassthrough,
            Self::PreGateSend(_) => BehavioralMutationCase::PreGateSend,
            Self::MutedSendLeak(_) => BehavioralMutationCase::MutedSendLeak,
            Self::PermissiveStructuralMatch(_) => BehavioralMutationCase::PermissiveStructuralMatch,
            Self::RefusedTopology(_) => BehavioralMutationCase::RefusedTopology,
        }
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Evaluates the same positive predicates declared by the behavioral witnesses.
    pub fn satisfies_witness(&self) -> bool {
        match self {
            Self::DroppedAdjustment(value) => {
                value.adjustment_dispatched
                    && value.adjust_event_recorded
                    && value.selected_value_exact
                    && value.unrelated_values_unchanged
                    && value.projection_values_exact
                    && value.baseline_restored
            }
            Self::CrossTrackParameterLeak(value) => {
                value.edited_track_id < MixerTrackId::COUNT as u32
                    && value.comparison_track_id < MixerTrackId::COUNT as u32
                    && value.track_ids_distinct
                    && value.parameter_cases_exercised == 6
                    && value.edited_track_energy_before > 0.0
                    && value.comparison_track_energy_before > 0.0
                    && value.edited_value_changed
                    && value.comparison_value_unchanged
                    && value.state_values_exact
                    && value.published_values_exact
                    && value.edited_track_audio_changed
                    && value.unedited_track_audio_unchanged
                    && value.all_track_parameters_isolated
                    && value.dry_path_isolated
                    && value.reverb_path_isolated
                    && value.delay_path_isolated
                    && value.baseline_restored
            }
            Self::PatchMisroute(value) => {
                value.command_patch_matches_event
                    && value.target_patch_received_command
                    && value.target_stem_changed
                    && value.untargeted_stems_unchanged
                    && value.patch_routing_exact
            }
            Self::OmittedStateTreeLeaf(value) => {
                value.schema_surface_equal
                    && value.required_leaf_count > 0
                    && value.missing_leaf_count == 0
                    && value.unexpected_leaf_count == 0
                    && value.state_values_exact
                    && value.projection_values_exact
            }
            Self::DryToWetBypass(value) => {
                value.dry_input_energy > 0.0
                    && approximately_zero(value.zero_send_reverb_input_energy)
                    && approximately_zero(value.zero_send_delay_input_energy)
                    && approximately_zero(value.zero_send_wet_output_energy)
                    && value.nonzero_send_reverb_input_energy > 0.0
                    && value.nonzero_send_delay_input_energy > 0.0
                    && value.nonzero_send_wet_output_energy > 0.0
                    && value.identical_effect_state
                    && value.dry_bypass_absent
                    && value.finite_audio
                    && value.baseline_restored
            }
            Self::ZeroRenderer(value) => {
                value.control_trace_complete
                    && value.renderer_called
                    && value.renderer_nonzero
                    && value.render_peak > 0.001
                    && value.finite_audio
            }
            Self::SlotOrderSwap(value) => {
                value.forward_energy > 0.0
                    && value.reversed_energy > 0.0
                    && value.order_difference_energy > ENERGY_EPSILON
                    && value.order_sensitive
                    && value.finite_audio
            }
            Self::EmptyReturnPassthrough(value) => {
                value.accumulated_send_energy > 0.0
                    && approximately_zero(value.unoccupied_wet_energy)
                    && value.output_matches_dry_exactly
                    && value.unoccupied_return_silent
                    && value.finite_audio
            }
            Self::PreGateSend(value) => {
                value.post_fader_reference_energy > 0.0
                    && value.pre_fader_reference_energy > value.post_fader_reference_energy
                    && approximately_equal(
                        value.measured_send_energy,
                        value.post_fader_reference_energy,
                    )
                    && value.send_taken_post_fader
                    && value.finite_audio
            }
            Self::MutedSendLeak(value) => {
                value.sounding_send_energy > 0.0
                    && approximately_zero(value.muted_send_energy)
                    && approximately_zero(value.muted_wet_energy)
                    && value.mute_gates_sends
                    && value.finite_audio
            }
            Self::PermissiveStructuralMatch(value) => {
                value.attested_wet_energy > 0.0
                    && approximately_zero(value.mismatched_wet_energy)
                    && value.strict_matching_enforced
                    && value.finite_audio
            }
            Self::RefusedTopology(value) => {
                value.refusal_recorded
                    && !value.rejection_reason.is_empty()
                    && value.rejection_reason_attributable
                    && value.active_graph_preserved
                    && value.canonical_state_preserved
                    && value.render_preserved_exactly
                    && value.post_rejection_valid_change_accepted
                    && value.finite_audio
            }
        }
    }

    pub fn falsifies_witness(&self) -> bool {
        !self.satisfies_witness()
    }
}

/// One completed harness execution.
#[derive(Clone, Debug, PartialEq)]
pub struct BehavioralMutationRun {
    exit_code: i32,
    observation: BehavioralMutationObservation,
}

impl BehavioralMutationRun {
    pub const fn exit_code(&self) -> i32 {
        self.exit_code
    }

    pub const fn observation(&self) -> &BehavioralMutationObservation {
        &self.observation
    }

    pub fn into_observation(self) -> BehavioralMutationObservation {
        self.observation
    }

    pub fn observation_json(&self) -> Result<String, serde_json::Error> {
        self.observation.to_json()
    }
}

/// Fast verification-only runner for six isolated production seams.
#[derive(Clone, Copy, Debug, Default)]
pub struct BehavioralMutationHarness;

impl BehavioralMutationHarness {
    pub const fn new() -> Self {
        Self
    }

    pub fn run(&self, case: BehavioralMutationCase, mutant_enabled: bool) -> BehavioralMutationRun {
        let observation = match case {
            BehavioralMutationCase::DroppedAdjustment => {
                BehavioralMutationObservation::DroppedAdjustment(run_dropped_adjustment(
                    mutant_enabled,
                ))
            }
            BehavioralMutationCase::CrossTrackParameterLeak => {
                BehavioralMutationObservation::CrossTrackParameterLeak(
                    run_cross_track_parameter_leak(mutant_enabled),
                )
            }
            BehavioralMutationCase::PatchMisroute => {
                BehavioralMutationObservation::PatchMisroute(run_patch_misroute(mutant_enabled))
            }
            BehavioralMutationCase::OmittedStateTreeLeaf => {
                BehavioralMutationObservation::OmittedStateTreeLeaf(run_omitted_state_tree_leaf(
                    mutant_enabled,
                ))
            }
            BehavioralMutationCase::DryToWetBypass => {
                BehavioralMutationObservation::DryToWetBypass(run_dry_to_wet_bypass(mutant_enabled))
            }
            BehavioralMutationCase::ZeroRenderer => {
                BehavioralMutationObservation::ZeroRenderer(run_zero_renderer(mutant_enabled))
            }
            BehavioralMutationCase::SlotOrderSwap => {
                BehavioralMutationObservation::SlotOrderSwap(run_slot_order_swap(mutant_enabled))
            }
            BehavioralMutationCase::EmptyReturnPassthrough => {
                BehavioralMutationObservation::EmptyReturnPassthrough(run_empty_return_passthrough(
                    mutant_enabled,
                ))
            }
            BehavioralMutationCase::PreGateSend => {
                BehavioralMutationObservation::PreGateSend(run_pre_gate_send(mutant_enabled))
            }
            BehavioralMutationCase::MutedSendLeak => {
                BehavioralMutationObservation::MutedSendLeak(run_muted_send_leak(mutant_enabled))
            }
            BehavioralMutationCase::PermissiveStructuralMatch => {
                BehavioralMutationObservation::PermissiveStructuralMatch(
                    run_permissive_structural_match(mutant_enabled),
                )
            }
            BehavioralMutationCase::RefusedTopology => {
                BehavioralMutationObservation::RefusedTopology(run_refused_topology(mutant_enabled))
            }
        };

        let exit_code = if observation.satisfies_witness() {
            0
        } else {
            1
        };
        BehavioralMutationRun {
            exit_code,
            observation,
        }
    }
}

type FixtureLoop = AppLoop<LockFreeControlHandle>;

/// The retired fixture return levels, retained verbatim so the harness's
/// reference wet model keeps the exact arithmetic the mutation cases were
/// calibrated against. Live return levels are return-owned state now.
const FIXTURE_REVERB_RETURN: f32 = 0.5;
const FIXTURE_DELAY_RETURN: f32 = 0.5;

fn fixture_globals() -> GlobalParameters {
    GlobalParameters::new(0.0).expect("fixture global parameters are valid")
}

fn fixture_patch(provider: &HiDefSoundFontCapability, id: u32) -> Patch {
    Patch::new(
        PatchId::new(id).expect("fixture PatchId is nonzero"),
        format!("Fixture {id}"),
        create_soundfont_config(
            provider,
            SoundFontInstrument::new(0, (id * 8) as u8, false)
                .expect("fixture SoundFont instrument is valid"),
        )
        .expect("fixture config matches the production descriptor"),
        MidiChannel::new((id - 1) as u8).expect("fixture MIDI channel is valid"),
        PatchOutput::to_track(
            MixerTrackId::new((id - 1) as u8).expect("fixture route is a fixed track"),
        ),
    )
}

fn fixture_state() -> AppState {
    state_with_tracks(
        MixerTrackParameters::from_values(
            -12.0,
            -0.35,
            false,
            false,
            [0.2, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        )
        .expect("fixture track parameters are valid"),
        MixerTrackParameters::from_values(
            -6.0,
            0.35,
            false,
            false,
            [0.4, 0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        )
        .expect("fixture track parameters are valid"),
    )
}

fn route_state() -> AppState {
    state_with_tracks(
        MixerTrackParameters::from_values(0.0, -1.0, false, false, [0.0; MAX_BUS_RETURNS])
            .expect("route track parameters are valid"),
        MixerTrackParameters::from_values(0.0, 1.0, false, false, [0.0; MAX_BUS_RETURNS])
            .expect("route track parameters are valid"),
    )
}

fn state_with_tracks(first: MixerTrackParameters, second: MixerTrackParameters) -> AppState {
    let asset = crate::adapter::hidef_soundfont_asset::HiDefSoundFontAsset::load()
        .expect("fixture SoundFont asset is valid");
    let provider =
        HiDefSoundFontCapability::new(asset.catalog()).expect("fixture capability is valid");
    let mixer = MixerState::default()
        .with_track(MixerTrackId::new(0).unwrap(), first)
        .with_track(MixerTrackId::new(1).unwrap(), second);
    let mut state = AppState::new(
        provider.registry().expect("fixture registry is valid"),
        fixture_globals(),
    )
    .with_initial_mixer(mixer);
    state
        .apply(AppEvent::InstallPatches(vec![
            fixture_patch(&provider, 1),
            fixture_patch(&provider, 2),
        ]))
        .expect("fixture installation is accepted");
    state
}

fn installed_loop(state: AppState) -> (FixtureLoop, LockFreeAudioHandle) {
    let projector = StateProjector::new();
    let initial = projector
        .parameter_snapshot(&state)
        .expect("fixture parameters project");
    let boundary = LockFreeAudioBoundary::new(16, initial);
    let (control, audio) = boundary.into_handles();
    let app_loop =
        AppLoop::new(state, projector, control).expect("fixture state has coherent projections");
    (app_loop, audio)
}

fn tree_value(app_loop: &FixtureLoop) -> Value {
    serde_json::from_str(app_loop.current_state_tree().json())
        .expect("production StateTree is valid JSON")
}

fn value_at(tree: &Value, pointer: &str) -> f64 {
    tree.pointer(pointer)
        .and_then(Value::as_f64)
        .expect("fixture numeric leaf exists")
}

fn stable_control_values_equal(left: &Value, right: &Value) -> bool {
    ["/patches", "/mixer", "/global", "/interaction"]
        .into_iter()
        .all(|pointer| left.pointer(pointer) == right.pointer(pointer))
}

fn tree_parameter_projection_exact(tree: &Value) -> bool {
    let Some(patches) = tree.pointer("/patches").and_then(Value::as_array) else {
        return false;
    };
    let Some(projected) = tree
        .pointer("/parameters/patches")
        .and_then(Value::as_array)
    else {
        return false;
    };
    if patches.len() != projected.len() {
        return false;
    }

    let patch_values_match = patches
        .iter()
        .zip(projected)
        .all(|(state_patch, parameter_patch)| {
            state_patch.pointer("/id") == parameter_patch.pointer("/patchId")
                && state_patch.pointer("/output") == parameter_patch.pointer("/output")
        });

    patch_values_match
        && tree.pointer("/mixer/tracks") == tree.pointer("/parameters/mixerTracks")
        // The snapshot's global object keeps only master gain; every
        // return-owned value lives at the indexed /parameters/returns.
        && tree.pointer("/global/masterGainDb") == tree.pointer("/parameters/global/masterGainDb")
        && tree.pointer("/generation") == tree.pointer("/parameters/generation")
}

fn app_projection_exact(app_loop: &FixtureLoop) -> bool {
    let tree = app_loop.current_state_tree();
    let text = app_loop.current_text();
    let value: Value =
        serde_json::from_str(tree.json()).expect("production StateTree is valid JSON");

    tree.state_hash() == text.state_hash()
        && value.pointer("/projection/body").and_then(Value::as_str) == Some(text.body())
        && value
            .pointer("/projection/selectedLine")
            .and_then(Value::as_u64)
            == Some(text.selected_line() as u64)
        && tree_parameter_projection_exact(&value)
}

fn published_matches_tree(snapshot: &ParameterSnapshot, tree: &Value) -> bool {
    if tree
        .pointer("/parameters/graphRevision")
        .and_then(Value::as_u64)
        != Some(snapshot.graph_revision().value())
        || snapshot.patch_count()
            != tree
                .pointer("/parameters/patchCount")
                .and_then(Value::as_u64)
                .unwrap_or(u64::MAX) as usize
    {
        return false;
    }

    let patches_match = snapshot.patches().iter().enumerate().all(|(index, patch)| {
        let prefix = format!("/parameters/patches/{index}");
        let id_matches = patch.patch_id().map(PatchId::value)
            == tree
                .pointer(&format!("{prefix}/patchId"))
                .and_then(Value::as_u64)
                .map(|value| value as u32);
        let output = patch.output();
        id_matches
            && numeric_leaf_matches(
                tree,
                &format!("{prefix}/output/trimGainDb"),
                output.trim_gain_db(),
            )
            && tree.pointer(&format!("{prefix}/output/trackId"))
                == Some(&serde_json::json!(output.track_id().index()))
    });

    patches_match
        && MixerTrackId::ALL.into_iter().all(|track_id| {
            let prefix = format!("/parameters/mixerTracks/{}", track_id.index());
            let track = snapshot.mixer_track(track_id);
            numeric_leaf_matches(tree, &format!("{prefix}/levelDb"), track.level_db())
                && numeric_leaf_matches(tree, &format!("{prefix}/pan"), track.pan())
                && tree.pointer(&format!("{prefix}/mute")) == Some(&Value::Bool(track.mute()))
                && tree.pointer(&format!("{prefix}/solo")) == Some(&Value::Bool(track.solo()))
                && track.sends().iter().enumerate().all(|(send_index, send)| {
                    numeric_leaf_matches(tree, &format!("{prefix}/sends/{send_index}"), *send)
                })
        })
        && numeric_leaf_matches(
            tree,
            "/parameters/global/masterGainDb",
            snapshot.global().master_gain_db(),
        )
        // Return-owned values travel only as the indexed return entries:
        // every live scalar and level must appear at its generic address,
        // bus for bus.
        && snapshot.returns().iter().enumerate().all(|(bus, entry)| {
            let prefix = format!("/parameters/returns/{bus}");
            entry.scalars().iter().enumerate().all(|(scalar_index, scalar)| {
                numeric_leaf_matches(tree, &format!("{prefix}/scalars/{scalar_index}"), *scalar)
            }) && numeric_leaf_matches(
                tree,
                &format!("{prefix}/returnLevel"),
                entry.return_level(),
            )
        })
}

fn numeric_leaf_matches(tree: &Value, pointer: &str, expected: f32) -> bool {
    tree.pointer(pointer)
        .and_then(Value::as_f64)
        .is_some_and(|actual| actual as f32 == expected)
}

fn run_dropped_adjustment(mutant_enabled: bool) -> DroppedAdjustmentObservation {
    let (mut app_loop, _audio) = installed_loop(fixture_state());
    let baseline = tree_value(&app_loop);
    let before_level = value_at(&baseline, "/mixer/tracks/0/levelDb");
    let expected_level = before_level + 1.0;
    let mut translator = KeyboardInputTranslator::new();

    let enter_adjust = translator
        .translate(WindowInput::key_down(WindowKey::K))
        .expect("K down emits Adjust mode");
    app_loop
        .dispatch_action_from(enter_adjust, EventSource::Keyboard)
        .expect("Adjust mode is accepted");
    let translated = translator
        .translate(WindowInput::key_down(WindowKey::D))
        .expect("K+D translates to an adjustment");

    let adjustment_dispatched = if mutant_enabled {
        false
    } else {
        app_loop
            .dispatch_action_from(translated, EventSource::Keyboard)
            .expect("fixture adjustment is accepted");
        true
    };

    let after_forward = tree_value(&app_loop);
    let adjust_event_recorded = app_loop
        .event_log()
        .records()
        .iter()
        .any(|record| matches!(record.input(), EventInput::Adjust { .. }));
    let selected_value_exact = approximately_equal(
        value_at(&after_forward, "/mixer/tracks/0/levelDb"),
        expected_level,
    );
    let unrelated_values_unchanged = [
        "/mixer/tracks/0/pan",
        "/mixer/tracks/0/mute",
        "/mixer/tracks/0/solo",
        "/mixer/tracks/0/sends/0",
        "/mixer/tracks/0/sends/1",
        "/mixer/tracks/1",
        "/patches",
        "/global",
    ]
    .into_iter()
    .all(|pointer| after_forward.pointer(pointer) == baseline.pointer(pointer));
    let projection_values_exact = app_projection_exact(&app_loop)
        && app_loop
            .current_text()
            .body()
            .contains(&format!("> levelDb={expected_level}"));

    let reverse = translator
        .translate(WindowInput::key_down(WindowKey::A))
        .expect("K+A translates to an inverse adjustment");
    app_loop
        .dispatch_action_from(reverse, EventSource::Keyboard)
        .expect("fixture inverse adjustment is accepted");
    let leave_adjust = translator
        .translate(WindowInput::key_up(WindowKey::K))
        .expect("K release restores Navigate mode");
    app_loop
        .dispatch_action_from(leave_adjust, EventSource::Keyboard)
        .expect("Navigate mode restoration is accepted");
    let restored = tree_value(&app_loop);

    DroppedAdjustmentObservation {
        case: BehavioralMutationCase::DroppedAdjustment
            .as_str()
            .to_owned(),
        adjustment_dispatched,
        adjust_event_recorded,
        selected_value_exact,
        unrelated_values_unchanged,
        projection_values_exact,
        baseline_restored: stable_control_values_equal(&baseline, &restored),
    }
}

/// The per-track edit walk: the four `MAIN` fader classes plus the two
/// harness-audible indexed sends (the buses the reference wet model meters).
/// Sends enter the walk as `Send(BusId)`, so widening the bus count changes
/// which buses are walked, never the shape of this enum.
#[derive(Clone, Copy)]
enum TrackField {
    Level,
    Pan,
    Mute,
    Solo,
    Send(BusId),
}

impl TrackField {
    const ALL: [Self; 6] = [
        Self::Level,
        Self::Pan,
        Self::Mute,
        Self::Solo,
        Self::Send(BusId::ALL[0]),
        Self::Send(BusId::ALL[1]),
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Level => "levelDb",
            Self::Pan => "pan",
            Self::Mute => "mute",
            Self::Solo => "solo",
            Self::Send(bus) => match bus.index() {
                0 => "sends[0]",
                _ => "sends[1]",
            },
        }
    }

    fn value(self, parameters: &MixerTrackParameters) -> f64 {
        match self {
            Self::Level => f64::from(parameters.level_db()),
            Self::Pan => f64::from(parameters.pan()),
            Self::Mute => f64::from(u8::from(parameters.mute())),
            Self::Solo => f64::from(u8::from(parameters.solo())),
            Self::Send(bus) => f64::from(parameters.send(bus)),
        }
    }
}

/// Control-side probe state shared with one installed harness return.
///
/// `PreparedPostEffect` is `Send`, so the probe is `Mutex`-based; the harness
/// mixes on the control side only — never in the RT callback — exactly like
/// the retired `Rc<Cell>` port probe it replaces.
#[derive(Default)]
struct ReturnProbe {
    input_energy: Mutex<f64>,
    initial_state: Mutex<Option<u64>>,
}

impl ReturnProbe {
    fn input_energy(&self) -> f64 {
        *self
            .input_energy
            .lock()
            .expect("harness probe lock is control-side only")
    }

    fn initial_state(&self) -> u64 {
        self.initial_state
            .lock()
            .expect("harness probe lock is control-side only")
            .unwrap_or(0)
    }
}

/// The harness's reference wet model as an installed unity return: it meters
/// its accumulated bus input and passes it through unchanged, so the rack's
/// live return level supplies the exact retired fixture arithmetic
/// (`input * FIXTURE_*_RETURN`). Reference model and live path stay distinct:
/// live return levels are return-owned state; these fixture entries are
/// overlaid onto the published snapshot only inside the harness mix.
struct HarnessMeteringReturn {
    probe: Arc<ReturnProbe>,
    state: u64,
}

impl HarnessMeteringReturn {
    fn new(probe: Arc<ReturnProbe>) -> Self {
        Self { probe, state: 0 }
    }
}

impl PreparedPostEffect for HarnessMeteringReturn {
    fn patch_id(&self) -> PatchId {
        PatchId::new(u32::MAX).expect("the static harness Patch id is non-zero")
    }

    fn slot_id(&self) -> EffectSlotId {
        EffectSlotId::new(1).expect("the static harness slot id is non-zero")
    }

    fn process(
        &mut self,
        interleaved_stereo: &mut [f32],
        frame_count: usize,
        _parameters: &RtPostEffectParameters,
    ) -> Result<(), PreparedEffectError> {
        let sample_count = frame_count * 2;
        *self
            .probe
            .input_energy
            .lock()
            .expect("harness probe lock is control-side only") =
            energy(&interleaved_stereo[..sample_count]);
        self.probe
            .initial_state
            .lock()
            .expect("harness probe lock is control-side only")
            .get_or_insert(self.state);
        self.state = self.state.saturating_add(1);
        Ok(())
    }
}

/// The buses the harness's reference wet model occupies, with the retired
/// fixture return levels as the live values.
const HARNESS_RETURN_BUSES: [BusId; 2] = [BusId::ALL[0], BusId::ALL[1]];

fn harness_return_levels() -> [f32; 2] {
    [FIXTURE_REVERB_RETURN, FIXTURE_DELAY_RETURN]
}

fn harness_return_parameters() -> RtPostEffectParameters {
    RtPostEffectParameters::new(
        EffectSlotId::new(1).expect("the static harness slot id is non-zero"),
        &[],
    )
    .expect("the empty harness scalar layout is valid")
}

/// Overlays the harness's fixture return entries onto one published snapshot.
fn with_harness_returns(snapshot: &ParameterSnapshot) -> ParameterSnapshot {
    let mut returns = [const { RtBusReturnParameters::EMPTY }; MAX_BUS_RETURNS];
    for (bus, level) in HARNESS_RETURN_BUSES.iter().zip(harness_return_levels()) {
        returns[bus.index()] = RtBusReturnParameters::new(
            EffectSlotId::new(1).expect("the static harness slot id is non-zero"),
            &[],
            level,
        )
        .expect("the fixture return entry is valid");
    }
    snapshot.clone().with_returns(returns)
}

fn fill_measure_stems(
    block: &mut PatchAudioBlock,
    snapshot: &ParameterSnapshot,
    patch_index: usize,
) {
    for (index, patch) in snapshot.patches().iter().enumerate() {
        let stem = block
            .stem_mut(
                index,
                patch
                    .patch_id()
                    .expect("active parameters carry Patch identity"),
            )
            .expect("active Patch stem exists");
        let amplitude = if index == patch_index {
            0.2 + index as f32 * 0.15
        } else {
            0.0
        };
        stem.fill(amplitude);
    }
}

#[derive(Clone, Copy, Default)]
struct MixMeasurement {
    output: f64,
    dry: f64,
    bus_inputs: [f64; 2],
}

fn measure_patch(snapshot: &ParameterSnapshot, patch_index: usize) -> MixMeasurement {
    let mut block =
        PatchAudioBlock::prepare(FRAME_COUNT).expect("verification PatchAudioBlock prepares");
    block
        .begin_render(snapshot, FRAME_COUNT)
        .expect("verification snapshot fits PatchAudioBlock");
    fill_measure_stems(&mut block, snapshot, patch_index);

    // The dry stage, measured exactly: the same deterministic mix without the
    // reference returns installed.
    let mut dry_mixer = MixEngine::new();
    dry_mixer
        .prepare(SAMPLE_RATE, FRAME_COUNT)
        .expect("verification mixer prepares");
    let mut dry_output = [0.0_f32; FRAME_COUNT * 2];
    dry_mixer.mix(&block, snapshot, &mut dry_output);

    let probes: [Arc<ReturnProbe>; 2] = [
        Arc::new(ReturnProbe::default()),
        Arc::new(ReturnProbe::default()),
    ];
    let mut mixer = MixEngine::new();
    mixer
        .prepare(SAMPLE_RATE, FRAME_COUNT)
        .expect("verification mixer prepares");
    for (bus, probe) in HARNESS_RETURN_BUSES.iter().zip(&probes) {
        mixer
            .install_bus_return(
                *bus,
                Box::new(HarnessMeteringReturn::new(Arc::clone(probe))),
                harness_return_parameters(),
                1.0,
            )
            .expect("harness return installs");
    }
    let overlaid = with_harness_returns(snapshot);
    let mut output = [0.0_f32; FRAME_COUNT * 2];
    mixer.mix(&block, &overlaid, &mut output);

    MixMeasurement {
        output: energy(&output),
        dry: energy(&dry_output),
        bus_inputs: [probes[0].input_energy(), probes[1].input_energy()],
    }
}

fn relevant_energy(field: TrackField, measurement: MixMeasurement) -> f64 {
    match field {
        TrackField::Level | TrackField::Pan | TrackField::Mute | TrackField::Solo => {
            measurement.dry
        }
        TrackField::Send(bus) => measurement.bus_inputs[bus.index()],
    }
}

fn snapshot_with_cross_track_leak(
    published: ParameterSnapshot,
    mutant_enabled: bool,
) -> ParameterSnapshot {
    if !mutant_enabled {
        return published;
    }

    let mut tracks = published.mixer_tracks().clone();
    tracks[0] = tracks[1].clone();
    ParameterSnapshot::new(
        published.generation(),
        *published.global(),
        MixerState::new(tracks),
        published.patches(),
    )
    .expect("mutated ownership seam preserves bounded identities")
}

fn run_cross_track_parameter_leak(mutant_enabled: bool) -> CrossTrackParameterLeakObservation {
    let mut representative = None;
    let mut parameter_cases_exercised = 0;
    let mut state_values_exact = true;
    let mut published_values_exact = true;
    let mut edited_track_audio_changed = true;
    let mut unedited_track_audio_unchanged = true;
    let mut dry_path_isolated = true;
    let mut reverb_path_isolated = true;
    let mut delay_path_isolated = true;
    let mut baseline_restored = true;

    for field in TrackField::ALL {
        let state = if matches!(field, TrackField::Solo) {
            state_with_tracks(
                MixerTrackParameters::from_values(
                    -12.0,
                    -0.35,
                    false,
                    true,
                    [0.2, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                )
                .expect("solo comparison track parameters are valid"),
                MixerTrackParameters::from_values(
                    -6.0,
                    0.35,
                    false,
                    false,
                    [0.4, 0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                )
                .expect("solo edited track parameters are valid"),
            )
        } else {
            fixture_state()
        };
        let (mut app_loop, mut audio) = installed_loop(state);
        app_loop
            .dispatch(AppEvent::Navigate(Direction::Right))
            .expect("fixture selects the edited track");
        match field {
            TrackField::Level => {}
            TrackField::Pan => {
                app_loop
                    .dispatch(AppEvent::Navigate(Direction::Down))
                    .expect("fixture selects track Pan");
            }
            TrackField::Mute | TrackField::Solo => {
                let navigation_count = if matches!(field, TrackField::Mute) {
                    2
                } else {
                    3
                };
                for _ in 0..navigation_count {
                    app_loop
                        .dispatch(AppEvent::Navigate(Direction::Down))
                        .expect("fixture selects the track toggle");
                }
            }
            TrackField::Send(bus) => {
                // The Inspector's first region is the selected track's eight
                // indexed sends in ascending BusId order; entry focuses B0.
                app_loop
                    .dispatch(AppEvent::EnterSurface(
                        crate::control::SurfaceId::MixerInspector,
                    ))
                    .expect("fixture enters the selected track Inspector");
                for _ in 0..bus.index() {
                    app_loop
                        .dispatch(AppEvent::Navigate(Direction::Down))
                        .expect("fixture selects the indexed track send");
                }
            }
        }

        let baseline = tree_value(&app_loop);
        let before_snapshot = audio.read_latest_parameters().clone();
        let before_edited = field.value(before_snapshot.mixer_track(MixerTrackId::ALL[1]));
        let before_comparison = field.value(before_snapshot.mixer_track(MixerTrackId::ALL[0]));
        let edited_before_mix = measure_patch(&before_snapshot, 1);
        let comparison_before_mix = measure_patch(&before_snapshot, 0);

        app_loop
            .dispatch(AppEvent::Adjust(Direction::Right))
            .expect("fixture edit is accepted");
        let after_tree = tree_value(&app_loop);
        let published = audio.read_latest_parameters().clone();
        let after_edited = field.value(published.mixer_track(MixerTrackId::ALL[1]));
        let after_comparison = field.value(published.mixer_track(MixerTrackId::ALL[0]));
        let mix_snapshot = snapshot_with_cross_track_leak(published.clone(), mutant_enabled);
        let edited_after_mix = measure_patch(&mix_snapshot, 1);
        let comparison_after_mix = measure_patch(&mix_snapshot, 0);

        let edited_changed = !approximately_equal(before_edited, after_edited);
        let comparison_unchanged = approximately_equal(before_comparison, after_comparison);
        let edited_audio_changed = !approximately_equal(
            relevant_energy(field, edited_before_mix),
            relevant_energy(field, edited_after_mix),
        );
        let comparison_audio_unchanged = approximately_equal(
            relevant_energy(field, comparison_before_mix),
            relevant_energy(field, comparison_after_mix),
        );

        parameter_cases_exercised += 1;
        state_values_exact &= tree_parameter_projection_exact(&after_tree);
        published_values_exact &= published_matches_tree(&published, &after_tree);
        edited_track_audio_changed &= edited_changed && edited_audio_changed;
        unedited_track_audio_unchanged &= comparison_unchanged && comparison_audio_unchanged;

        match field {
            TrackField::Level | TrackField::Pan | TrackField::Mute | TrackField::Solo => {
                dry_path_isolated &= edited_audio_changed && comparison_audio_unchanged;
            }
            // The retained observation labels keep their exact strings: the
            // "reverb"/"delay" paths are the sends toward buses 0 and 1,
            // whose returns the production composition occupies by default.
            TrackField::Send(bus) if bus.index() == 0 => {
                reverb_path_isolated &= edited_audio_changed && comparison_audio_unchanged;
            }
            TrackField::Send(_) => {
                delay_path_isolated &= edited_audio_changed && comparison_audio_unchanged;
            }
        }

        if matches!(field, TrackField::Level) {
            representative = Some((
                before_edited,
                after_edited,
                before_comparison,
                after_comparison,
                field.value(published.mixer_track(MixerTrackId::ALL[1])),
                field.value(published.mixer_track(MixerTrackId::ALL[0])),
                edited_before_mix.output,
                edited_after_mix.output,
                comparison_before_mix.output,
                comparison_after_mix.output,
                edited_changed,
                comparison_unchanged,
            ));
        }

        app_loop
            .dispatch(AppEvent::Adjust(Direction::Left))
            .expect("fixture inverse edit is accepted");
        baseline_restored &= stable_control_values_equal(&baseline, &tree_value(&app_loop));
    }

    let (
        edited_value_before,
        edited_value_after,
        comparison_value_before,
        comparison_value_after,
        published_edited_value,
        published_comparison_value,
        edited_stem_energy_before,
        edited_stem_energy_after,
        comparison_stem_energy_before,
        comparison_stem_energy_after,
        edited_value_changed,
        comparison_value_unchanged,
    ) = representative.expect("gain case is always exercised");

    let all_track_parameters_isolated = edited_track_audio_changed
        && unedited_track_audio_unchanged
        && dry_path_isolated
        && reverb_path_isolated
        && delay_path_isolated;

    CrossTrackParameterLeakObservation {
        case: BehavioralMutationCase::CrossTrackParameterLeak
            .as_str()
            .to_owned(),
        edited_track_id: 1,
        comparison_track_id: 0,
        track_ids_distinct: true,
        parameter: TrackField::Level.name().to_owned(),
        parameter_cases_exercised,
        edited_value_before,
        edited_value_after,
        comparison_value_before,
        comparison_value_after,
        published_edited_value,
        published_comparison_value,
        edited_track_energy_before: edited_stem_energy_before,
        edited_track_energy_after: edited_stem_energy_after,
        comparison_track_energy_before: comparison_stem_energy_before,
        comparison_track_energy_after: comparison_stem_energy_after,
        edited_value_changed,
        comparison_value_unchanged,
        state_values_exact,
        published_values_exact,
        edited_track_audio_changed,
        unedited_track_audio_unchanged,
        all_track_parameters_isolated,
        dry_path_isolated,
        reverb_path_isolated,
        delay_path_isolated,
        baseline_restored,
    }
}

struct MisrouteBoundary<Boundary> {
    inner: Boundary,
    mutant_enabled: bool,
    alternate_patch_id: PatchId,
    rewritten: bool,
}

impl<Boundary> AudioThreadBoundary for MisrouteBoundary<Boundary>
where
    Boundary: AudioThreadBoundary,
{
    fn pop_command(&mut self) -> Option<AudioCommand> {
        let command = self.inner.pop_command()?;
        if !self.mutant_enabled || self.rewritten {
            return Some(command);
        }

        match command {
            AudioCommand::PatchMidi { message, .. } => {
                self.rewritten = true;
                Some(AudioCommand::PatchMidi {
                    patch_id: self.alternate_patch_id,
                    message,
                })
            }
            AudioCommand::AllNotesOff => Some(AudioCommand::AllNotesOff),
            AudioCommand::PreviewStart { .. }
            | AudioCommand::PreviewStop { .. }
            | AudioCommand::InitializedVoice { .. }
            | AudioCommand::VoiceMidi { .. }
            | AudioCommand::OwnedMidi { .. }
            | AudioCommand::ReleaseSource { .. }
            | AudioCommand::ReleaseImmediateNotes => Some(command),
        }
    }

    fn exchange_latest_parameters(&mut self, previous: &mut ParameterSnapshot) -> bool {
        self.inner.exchange_latest_parameters(previous)
    }
}

#[derive(Default)]
struct EngineProbe {
    dispatched_patch: AtomicU32,
}

struct RoutedVerificationPreparer {
    probe: Arc<EngineProbe>,
    capability_id: CapabilityId,
}

impl InstrumentPreparer for RoutedVerificationPreparer {
    fn capability_id(&self) -> &CapabilityId {
        &self.capability_id
    }

    fn prepare(
        &self,
        patch: &Patch,
        _sample_rate: f32,
        _max_frames: usize,
    ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
        Ok(Box::new(RoutedVerificationInstrument {
            patch_id: patch.id(),
            probe: Arc::clone(&self.probe),
        }))
    }
}

struct RoutedVerificationInstrument {
    patch_id: PatchId,
    probe: Arc<EngineProbe>,
}

impl PreparedInstrument for RoutedVerificationInstrument {
    fn patch_id(&self) -> PatchId {
        self.patch_id
    }

    fn dispatch(
        &mut self,
        _message: MidiMessage,
        _parameters: &crate::real_time::RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        self.probe
            .dispatched_patch
            .store(self.patch_id.value(), Ordering::Release);
        Ok(())
    }

    fn render(
        &mut self,
        output: &mut [f32],
        _frame_count: usize,
        _parameters: &crate::real_time::RtPatchParameters,
    ) -> Result<(), crate::synth::PreparedInstrumentError> {
        if self.probe.dispatched_patch.load(Ordering::Acquire) == self.patch_id.value() {
            output.fill(0.5);
        }

        Ok(())
    }

    fn all_notes_off(&mut self) {
        self.probe.dispatched_patch.store(0, Ordering::Release);
    }
}

struct RouteRender {
    app_loop: FixtureLoop,
    probe: Arc<EngineProbe>,
    output: [f32; FRAME_COUNT * 2],
}

fn render_routed_command(mutant_enabled: bool) -> RouteRender {
    let target_id = PatchId::new(2).expect("target PatchId is valid");
    let alternate_id = PatchId::new(1).expect("alternate PatchId is valid");
    let (mut app_loop, audio) = installed_loop(route_state());
    let message = MidiMessage::try_new(
        MidiChannel::new(1).expect("fixture MIDI channel is valid"),
        MidiMessageKind::NoteOn,
        60,
        100,
    )
    .expect("fixture MIDI message is valid");
    app_loop
        .dispatch_from(
            AppEvent::Midi {
                patch_id: target_id,
                message,
            },
            EventSource::AutomaticMidi,
        )
        .expect("fixture MIDI event is accepted");

    let boundary = MisrouteBoundary {
        inner: audio,
        mutant_enabled,
        alternate_patch_id: alternate_id,
        rewritten: false,
    };
    let probe = Arc::new(EngineProbe::default());
    let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(RoutedVerificationPreparer {
        probe: Arc::clone(&probe),
        capability_id: CapabilityId::new("instrument.soundfont.hidef").unwrap(),
    })];
    let graph = PreparedGraphBuilder::new(app_loop.capabilities(), &preparers)
        .build(
            GraphRevision::INITIAL,
            app_loop.patches(),
            app_loop.current_parameters().clone(),
            SAMPLE_RATE,
            FRAME_COUNT,
        )
        .expect("verification graph prepares");
    let mut renderer = AudioRenderer::new(boundary, NoStructuralGraphChanges::new(), graph);
    let mut output = [0.0_f32; FRAME_COUNT * 2];
    renderer.render(&mut output);

    RouteRender {
        app_loop,
        probe,
        output,
    }
}

fn run_patch_misroute(mutant_enabled: bool) -> PatchMisrouteObservation {
    let result = render_routed_command(mutant_enabled);
    let target_id = 2;
    let dispatched_id = result.probe.dispatched_patch.load(Ordering::Acquire);
    let (left_energy, right_energy) = stereo_energy(&result.output);
    let command_patch_matches_event = dispatched_id == target_id;
    let target_patch_received_command = dispatched_id == target_id;
    let target_stem_changed = right_energy > ENERGY_EPSILON;
    let untargeted_stems_unchanged = approximately_zero(left_energy);
    let patch_routing_exact = command_patch_matches_event
        && target_patch_received_command
        && target_stem_changed
        && untargeted_stems_unchanged;

    PatchMisrouteObservation {
        case: BehavioralMutationCase::PatchMisroute.as_str().to_owned(),
        command_patch_matches_event,
        target_patch_received_command,
        target_stem_changed,
        untargeted_stems_unchanged,
        patch_routing_exact,
    }
}

fn collect_leaf_paths(value: &Value) -> BTreeSet<String> {
    fn visit(value: &Value, path: &str, leaves: &mut BTreeSet<String>) {
        match value {
            Value::Object(object) => {
                for (name, child) in object {
                    visit(child, &format!("{path}/{name}"), leaves);
                }
            }
            Value::Array(array) => {
                for (index, child) in array.iter().enumerate() {
                    visit(child, &format!("{path}/{index}"), leaves);
                }
            }
            _ => {
                leaves.insert(path.to_owned());
            }
        }
    }

    let mut leaves = BTreeSet::new();
    visit(value, "", &mut leaves);
    leaves
}

fn run_omitted_state_tree_leaf(mutant_enabled: bool) -> OmittedStateTreeLeafObservation {
    let (app_loop, _audio) = installed_loop(fixture_state());
    let healthy_tree = tree_value(&app_loop);
    let required = collect_leaf_paths(&healthy_tree);
    let mut observed_tree = healthy_tree.clone();

    if mutant_enabled {
        observed_tree
            .pointer_mut("/mixer/tracks/0")
            .and_then(Value::as_object_mut)
            .expect("typed mixer-track parameter object exists")
            .remove("levelDb")
            .expect("the one omitted typed leaf exists");
    }

    let observed = collect_leaf_paths(&observed_tree);
    let missing_leaf_count = required.difference(&observed).count();
    let unexpected_leaf_count = observed.difference(&required).count();
    let schema_surface_equal = missing_leaf_count == 0 && unexpected_leaf_count == 0;
    let projection_values_exact =
        observed_tree.pointer("/projection") == healthy_tree.pointer("/projection");

    OmittedStateTreeLeafObservation {
        case: BehavioralMutationCase::OmittedStateTreeLeaf
            .as_str()
            .to_owned(),
        schema_surface_equal,
        required_leaf_count: required.len(),
        missing_leaf_count,
        unexpected_leaf_count,
        state_values_exact: tree_parameter_projection_exact(&observed_tree),
        projection_values_exact,
    }
}

#[derive(Clone, Copy)]
struct DryWetMeasurement {
    dry: f64,
    reverb: f64,
    delay: f64,
    wet: f64,
    initial_state: u64,
    finite: bool,
}

fn render_dry_wet(parameters: ParameterSnapshot, mutant_enabled: bool) -> DryWetMeasurement {
    let mut patch_audio = PatchAudioBlock::prepare(FRAME_COUNT).unwrap();
    patch_audio.begin_render(&parameters, FRAME_COUNT).unwrap();
    for (index, patch) in parameters.patches().iter().enumerate() {
        let Some(patch_id) = patch.patch_id() else {
            continue;
        };
        if let Some(stem) = patch_audio.stem_mut(index, patch_id) {
            stem.fill(0.2 + index as f32 * 0.1);
        }
    }

    // The dry stage, measured exactly: the same deterministic mix without
    // the reference returns installed.
    let mut dry_mixer = MixEngine::new();
    dry_mixer
        .prepare(SAMPLE_RATE, FRAME_COUNT)
        .expect("verification mixer prepares");
    let mut dry_output = [0.0_f32; FRAME_COUNT * 2];
    dry_mixer.mix(&patch_audio, &parameters, &mut dry_output);

    let probes: [Arc<ReturnProbe>; 2] = [
        Arc::new(ReturnProbe::default()),
        Arc::new(ReturnProbe::default()),
    ];
    let mut mixer = MixEngine::new();
    mixer
        .prepare(SAMPLE_RATE, FRAME_COUNT)
        .expect("verification mixer prepares");
    for (bus, probe) in HARNESS_RETURN_BUSES.iter().zip(&probes) {
        mixer
            .install_bus_return(
                *bus,
                Box::new(HarnessMeteringReturn::new(Arc::clone(probe))),
                harness_return_parameters(),
                1.0,
            )
            .expect("harness return installs");
    }
    let overlaid = with_harness_returns(&parameters);
    let mut output = [0.0_f32; FRAME_COUNT * 2];
    mixer.mix(&patch_audio, &overlaid, &mut output);

    let reverb_energy = probes[0].input_energy();
    let delay_energy = probes[1].input_energy();

    // The mutant models a wet-sum stage that derives wet output from the dry
    // mix when the declared send inputs are silent — the exact fault the
    // retired port mutant modeled. The bus-return rack makes that fault
    // structurally impossible inside a return (a return never sees the dry
    // mix), so the seam stands at the harness's wet-sum stage.
    if mutant_enabled && approximately_zero(reverb_energy) && approximately_zero(delay_energy) {
        for (sample, dry) in output.iter_mut().zip(dry_output.iter()) {
            *sample += dry * 0.25;
        }
    }

    let wet_output_energy = output
        .iter()
        .zip(dry_output.iter())
        .map(|(after, dry)| {
            let delta = f64::from(*after - *dry);
            delta * delta
        })
        .sum();

    DryWetMeasurement {
        dry: energy(&dry_output),
        reverb: reverb_energy,
        delay: delay_energy,
        wet: wet_output_energy,
        initial_state: probes[0].initial_state().max(probes[1].initial_state()),
        finite: output.iter().all(|sample| sample.is_finite()),
    }
}

fn parameter_snapshot_with_sends(bus0_send: f32, bus1_send: f32) -> ParameterSnapshot {
    let tracks = MixerState::default()
        .with_track(
            MixerTrackId::ALL[0],
            MixerTrackParameters::from_values(
                0.0,
                -0.25,
                false,
                false,
                [bus0_send, bus1_send, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            )
            .expect("verification sends are valid"),
        )
        .with_track(
            MixerTrackId::ALL[1],
            MixerTrackParameters::from_values(
                -3.0,
                0.25,
                false,
                false,
                [bus0_send, bus1_send, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            )
            .expect("verification sends are valid"),
        );
    let patches = [
        RtPatchParameters::new(
            PatchId::new(1).expect("fixture PatchId is valid"),
            PatchOutput::to_track(MixerTrackId::ALL[0]),
        ),
        RtPatchParameters::new(
            PatchId::new(2).expect("fixture PatchId is valid"),
            PatchOutput::to_track(MixerTrackId::ALL[1]),
        ),
    ];
    ParameterSnapshot::new(1, fixture_globals(), tracks, &patches)
        .expect("verification snapshot is valid")
}

fn faithful_effects_control_baseline_restored() -> bool {
    let (mut app_loop, mut audio) = installed_loop(fixture_state());
    app_loop
        .dispatch(AppEvent::EnterSurface(
            crate::control::SurfaceId::MixerInspector,
        ))
        .expect("faithful-effects fixture enters Mixer Inspector");
    let baseline_tree = tree_value(&app_loop);
    let baseline_parameters = audio.read_latest_parameters().clone();

    app_loop
        .dispatch(AppEvent::Adjust(Direction::Right))
        .expect("Reverb Send increase is accepted");
    app_loop
        .dispatch(AppEvent::Adjust(Direction::Left))
        .expect("Reverb Send restoration is accepted");
    app_loop
        .dispatch(AppEvent::Navigate(Direction::Down))
        .expect("Delay Send focus is accepted");
    app_loop
        .dispatch(AppEvent::Adjust(Direction::Right))
        .expect("Delay Send increase is accepted");
    app_loop
        .dispatch(AppEvent::Adjust(Direction::Left))
        .expect("Delay Send restoration is accepted");
    app_loop
        .dispatch(AppEvent::Navigate(Direction::Up))
        .expect("Reverb Send focus restoration is accepted");

    let restored_tree = tree_value(&app_loop);
    let restored_parameters = audio.read_latest_parameters().clone();
    let projected_values_restored = [
        "/patches",
        "/mixer",
        "/global",
        "/interaction",
        "/patchPage",
        "/projection/context",
        "/projection/body",
        "/projection/selectedLine",
        "/parameters/patches",
        "/parameters/mixerTracks",
        "/parameters/global",
        "/parameters/graphRevision",
    ]
    .into_iter()
    .all(|pointer| baseline_tree.pointer(pointer) == restored_tree.pointer(pointer));

    projected_values_restored
        && baseline_parameters.audio_values_equal(&restored_parameters)
        && tree_parameter_projection_exact(&restored_tree)
        && published_matches_tree(&restored_parameters, &restored_tree)
}

fn run_dry_to_wet_bypass(mutant_enabled: bool) -> DryToWetBypassObservation {
    let zero_send = parameter_snapshot_with_sends(0.0, 0.0);
    let nonzero_send = parameter_snapshot_with_sends(0.4, 0.3);
    let zero_measurement = render_dry_wet(zero_send.clone(), mutant_enabled);
    let nonzero_measurement = render_dry_wet(nonzero_send.clone(), mutant_enabled);
    let finite_audio = zero_measurement.finite && nonzero_measurement.finite;
    let baseline_restored = zero_send == parameter_snapshot_with_sends(0.0, 0.0)
        && nonzero_send == parameter_snapshot_with_sends(0.4, 0.3)
        && faithful_effects_control_baseline_restored();

    DryToWetBypassObservation {
        case: BehavioralMutationCase::DryToWetBypass.as_str().to_owned(),
        dry_input_energy: zero_measurement.dry,
        zero_send_reverb_input_energy: zero_measurement.reverb,
        zero_send_delay_input_energy: zero_measurement.delay,
        zero_send_wet_output_energy: zero_measurement.wet,
        nonzero_send_reverb_input_energy: nonzero_measurement.reverb,
        nonzero_send_delay_input_energy: nonzero_measurement.delay,
        nonzero_send_wet_output_energy: nonzero_measurement.wet,
        identical_effect_state: zero_measurement.initial_state == nonzero_measurement.initial_state,
        dry_bypass_absent: approximately_zero(zero_measurement.wet),
        finite_audio,
        baseline_restored,
    }
}

fn run_zero_renderer(mutant_enabled: bool) -> ZeroRendererObservation {
    let mut result = render_routed_command(false);
    let renderer_called = true;
    let target_received = result.probe.dispatched_patch.load(Ordering::Acquire) == 2;
    let control_trace_complete = result.app_loop.event_log().records().iter().any(|record| {
        matches!(
            record.input(),
            EventInput::Midi { patch_id, .. } if *patch_id == 2
        )
    }) && target_received;

    if mutant_enabled {
        result.output.fill(0.0);
    }

    let render_peak = result
        .output
        .iter()
        .map(|sample| f64::from(sample.abs()))
        .fold(0.0_f64, f64::max);
    let finite_audio = result.output.iter().all(|sample| sample.is_finite());

    ZeroRendererObservation {
        case: BehavioralMutationCase::ZeroRenderer.as_str().to_owned(),
        control_trace_complete,
        renderer_called,
        renderer_nonzero: render_peak > 0.0,
        render_peak,
        finite_audio,
    }
}

// ---- Topology and routing seam mutations ----------------------------------

/// Prepares one positional registry effect instance with its descriptor
/// defaults, ready for in-chain processing.
fn prepared_registry_effect(
    entry_index: usize,
    slot: EffectSlotId,
    sample_rate: f32,
    max_frames: usize,
) -> (Box<dyn PreparedPostEffect>, RtPostEffectParameters) {
    let registry = crate::adapter::production_effects::production_effect_registry()
        .expect("production effect registry composes");
    let preparers = crate::adapter::production_effects::production_effect_preparers()
        .expect("production effect preparers compose");
    let descriptor = &registry.descriptors()[entry_index];
    let config = descriptor
        .default_config(slot)
        .expect("descriptor default config is valid");
    let scalars: Vec<f32> = descriptor
        .scalar_parameters()
        .map(|spec| {
            spec.scalar_value(
                config
                    .value(spec.id())
                    .expect("default config covers every scalar"),
            )
            .expect("default scalar is in range")
        })
        .collect();
    let parameters =
        RtPostEffectParameters::new(slot, &scalars).expect("default scalar layout projects");
    let preparer = preparers
        .iter()
        .find(|preparer| preparer.capability_id() == config.capability_id())
        .expect("the registry entry has a preparer");
    let effect = preparer
        .prepare(
            PatchId::new(1).expect("fixture PatchId is nonzero"),
            &config,
            sample_rate,
            max_frames,
        )
        .expect("the registry entry prepares");
    (effect, parameters)
}

/// T048: swapping two slots' order must change the rendered output; a rack
/// that ignores slot order (the mutant) renders both orders identically and
/// is caught by the zero difference.
fn run_slot_order_swap(mutant_enabled: bool) -> SlotOrderSwapObservation {
    const ORDER_FRAMES: usize = 256;
    const ORDER_BLOCKS: usize = 96;

    let render_chain = |first_entry: usize, second_entry: usize| -> Vec<f32> {
        let slot_one = EffectSlotId::new(1).expect("slot id one is nonzero");
        let slot_two = EffectSlotId::new(2).expect("slot id two is nonzero");
        let (mut first, first_parameters) =
            prepared_registry_effect(first_entry, slot_one, SAMPLE_RATE, ORDER_FRAMES);
        let (mut second, second_parameters) =
            prepared_registry_effect(second_entry, slot_two, SAMPLE_RATE, ORDER_FRAMES);
        let mut state: u32 = 0x2F6E_1B45;
        let mut noise = move || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f32 / 16_777_216.0 - 0.5
        };
        let mut trace = Vec::with_capacity(ORDER_BLOCKS * ORDER_FRAMES * 2);
        for _ in 0..ORDER_BLOCKS {
            let mut block: Vec<f32> = (0..ORDER_FRAMES * 2).map(|_| noise()).collect();
            first
                .process(&mut block, ORDER_FRAMES, &first_parameters)
                .expect("first slot processes in place");
            second
                .process(&mut block, ORDER_FRAMES, &second_parameters)
                .expect("second slot processes in place");
            trace.extend_from_slice(&block);
        }
        trace
    };

    // Positional entries 0 and 2: two genuinely different processors.
    let forward = render_chain(0, 2);
    let reversed = if mutant_enabled {
        // The mutant ignores the exchanged order and renders the same
        // chain again.
        render_chain(0, 2)
    } else {
        render_chain(2, 0)
    };

    let order_difference_energy = forward
        .iter()
        .zip(reversed.iter())
        .map(|(first, second)| {
            let delta = f64::from(*first - *second);
            delta * delta
        })
        .sum::<f64>();
    let finite_audio = forward.iter().all(|sample| sample.is_finite())
        && reversed.iter().all(|sample| sample.is_finite());

    SlotOrderSwapObservation {
        case: BehavioralMutationCase::SlotOrderSwap.as_str().to_owned(),
        forward_energy: energy(&forward),
        reversed_energy: energy(&reversed),
        order_difference_energy,
        order_sensitive: order_difference_energy > ENERGY_EPSILON,
        finite_audio,
    }
}

/// One deterministic mixer run over fixed stems for the send-seam cases.
struct SendSeamRun {
    output: Vec<f32>,
    dry: Vec<f32>,
    bus_input_energy: f64,
}

fn run_send_seam_mix(
    track_parameters: MixerTrackParameters,
    bus: BusId,
    install_return: bool,
) -> SendSeamRun {
    let stem: Vec<f32> = (0..FRAME_COUNT * 2)
        .map(|index| if index % 2 == 0 { 0.4 } else { 0.2 })
        .collect();
    let track_id = MixerTrackId::ALL[0];
    let mixer_state = MixerState::default().with_track(track_id, track_parameters);
    let patches = [RtPatchParameters::new(
        PatchId::new(1).expect("fixture PatchId is nonzero"),
        PatchOutput::to_track(track_id),
    )];
    let base = ParameterSnapshot::new(1, fixture_globals(), mixer_state, &patches)
        .expect("send-seam snapshot is valid");
    let mut returns =
        [crate::real_time::parameter_snapshot::RtBusReturnParameters::EMPTY; MAX_BUS_RETURNS];
    if install_return {
        returns[bus.index()] = crate::real_time::parameter_snapshot::RtBusReturnParameters::new(
            EffectSlotId::new(1).expect("the static harness slot id is non-zero"),
            &[],
            1.0,
        )
        .expect("the fixture return entry is valid");
    }
    let snapshot = base.clone().with_returns(returns);

    let mut block = PatchAudioBlock::prepare(FRAME_COUNT).expect("send-seam block prepares");
    block
        .begin_render(&snapshot, FRAME_COUNT)
        .expect("send-seam snapshot fits");
    block
        .stem_mut(0, PatchId::new(1).expect("fixture PatchId is nonzero"))
        .expect("fixture stem exists")
        .copy_from_slice(&stem);

    let mut dry_mixer = MixEngine::new();
    dry_mixer
        .prepare(SAMPLE_RATE, FRAME_COUNT)
        .expect("send-seam mixer prepares");
    let mut dry = vec![0.0_f32; FRAME_COUNT * 2];
    dry_mixer.mix(&block, &base, &mut dry);

    let probe = Arc::new(ReturnProbe::default());
    let mut mixer = MixEngine::new();
    mixer
        .prepare(SAMPLE_RATE, FRAME_COUNT)
        .expect("send-seam mixer prepares");
    if install_return {
        mixer
            .install_bus_return(
                bus,
                Box::new(HarnessMeteringReturn::new(Arc::clone(&probe))),
                harness_return_parameters(),
                1.0,
            )
            .expect("send-seam return installs");
    }
    let mut output = vec![0.0_f32; FRAME_COUNT * 2];
    mixer.mix(&block, &snapshot, &mut output);

    SendSeamRun {
        output,
        dry,
        bus_input_energy: probe.input_energy(),
    }
}

/// T048: an unoccupied return must contribute silence, never its
/// accumulated input (C-BR-6); the mutant models the passthrough fault.
fn run_empty_return_passthrough(mutant_enabled: bool) -> EmptyReturnPassthroughObservation {
    let bus = BusId::ALL[6];
    let sends = MixerTrackParameters::default()
        .with_send(bus, 1.0)
        .expect("fixture send is valid");
    let run = run_send_seam_mix(sends, bus, false);

    let mut output = run.output.clone();
    if mutant_enabled {
        // The passthrough fault: the empty return hands its accumulated
        // input straight to the mix.
        for (index, sample) in output.iter_mut().enumerate() {
            let stem = if index % 2 == 0 { 0.4_f32 } else { 0.2_f32 };
            *sample += stem;
        }
    }

    let unoccupied_wet_energy = output
        .iter()
        .zip(run.dry.iter())
        .map(|(after, dry)| {
            let delta = f64::from(*after - *dry);
            delta * delta
        })
        .sum::<f64>();
    let output_matches_dry_exactly = output
        .iter()
        .zip(run.dry.iter())
        .all(|(after, dry)| after == dry);

    EmptyReturnPassthroughObservation {
        case: BehavioralMutationCase::EmptyReturnPassthrough
            .as_str()
            .to_owned(),
        accumulated_send_energy: energy(&[0.4, 0.2]) * (FRAME_COUNT as f64),
        unoccupied_wet_energy,
        output_matches_dry_exactly,
        unoccupied_return_silent: approximately_zero(unoccupied_wet_energy)
            && output_matches_dry_exactly,
        finite_audio: output.iter().all(|sample| sample.is_finite()),
    }
}

/// T048: sends are taken post-fader (C-BR-1); the mutant models a send
/// stage moved before the fader.
fn run_pre_gate_send(mutant_enabled: bool) -> PreGateSendObservation {
    let bus = BusId::ALL[5];
    let level_db = -6.020_600_3_f32;
    let send = 0.8_f32;
    let parameters = MixerTrackParameters::default()
        .with_scalar_value(
            crate::mixer::mixer_track_parameters::MixerTrackParameter::Level,
            level_db,
        )
        .expect("fixture level is valid")
        .with_send(bus, send)
        .expect("fixture send is valid");
    let run = run_send_seam_mix(parameters, bus, true);

    let gain = 10.0_f32.powf(level_db / 20.0);
    let reference = |apply_fader: bool| -> f64 {
        (0..FRAME_COUNT * 2)
            .map(|index| {
                let stem = if index % 2 == 0 { 0.4_f32 } else { 0.2_f32 };
                let faded = if apply_fader { stem * gain } else { stem };
                let bus_sample = faded * send;
                f64::from(bus_sample) * f64::from(bus_sample)
            })
            .sum()
    };
    let post_fader_reference_energy = reference(true);
    let pre_fader_reference_energy = reference(false);
    let measured_send_energy = if mutant_enabled {
        // The pre-fader fault: the send is accumulated before the fader
        // and gate stage.
        pre_fader_reference_energy
    } else {
        run.bus_input_energy
    };

    PreGateSendObservation {
        case: BehavioralMutationCase::PreGateSend.as_str().to_owned(),
        post_fader_reference_energy,
        measured_send_energy,
        pre_fader_reference_energy,
        send_taken_post_fader: approximately_equal(
            measured_send_energy,
            post_fader_reference_energy,
        ),
        finite_audio: run.output.iter().all(|sample| sample.is_finite()),
    }
}

/// T048: a muted track contributes nothing to any send (C-BR-2); the mutant
/// strips the mute from the snapshot the mix stage consumes — the same
/// ownership-seam fault shape as the cross-track leak.
fn run_muted_send_leak(mutant_enabled: bool) -> MutedSendLeakObservation {
    let bus = BusId::ALL[4];
    let sounding = MixerTrackParameters::default()
        .with_send(bus, 1.0)
        .expect("fixture send is valid");
    let muted = MixerTrackParameters::from_values(0.0, 0.0, true, false, sounding.sends())
        .expect("fixture muted track is valid");
    let sounding_run = run_send_seam_mix(sounding.clone(), bus, true);
    let muted_run = if mutant_enabled {
        // The gate fault: the send stage consumes a snapshot whose mute
        // was dropped.
        run_send_seam_mix(sounding, bus, true)
    } else {
        run_send_seam_mix(muted, bus, true)
    };

    let muted_wet_energy = muted_run
        .output
        .iter()
        .zip(muted_run.dry.iter())
        .map(|(after, dry)| {
            let delta = f64::from(*after - *dry);
            delta * delta
        })
        .sum::<f64>();

    MutedSendLeakObservation {
        case: BehavioralMutationCase::MutedSendLeak.as_str().to_owned(),
        sounding_send_energy: sounding_run.bus_input_energy,
        muted_send_energy: muted_run.bus_input_energy,
        muted_wet_energy,
        mute_gates_sends: approximately_zero(muted_run.bus_input_energy)
            && approximately_zero(muted_wet_energy),
        finite_audio: muted_run.output.iter().all(|sample| sample.is_finite()),
    }
}

/// T048: a live return entry that does not attest the prepared instance
/// contributes silence — never a wrong-values substitution; the mutant
/// models permissive structural matching.
fn run_permissive_structural_match(mutant_enabled: bool) -> PermissiveStructuralMatchObservation {
    let bus = BusId::ALL[2];
    let sends = MixerTrackParameters::default()
        .with_send(bus, 1.0)
        .expect("fixture send is valid");

    let run_with_live_slot = |live_slot: u16| -> SendSeamRun {
        let stem: Vec<f32> = (0..FRAME_COUNT * 2)
            .map(|index| if index % 2 == 0 { 0.4 } else { 0.2 })
            .collect();
        let track_id = MixerTrackId::ALL[0];
        let mixer_state = MixerState::default().with_track(track_id, sends.clone());
        let patches = [RtPatchParameters::new(
            PatchId::new(1).expect("fixture PatchId is nonzero"),
            PatchOutput::to_track(track_id),
        )];
        let base = ParameterSnapshot::new(1, fixture_globals(), mixer_state, &patches)
            .expect("match snapshot is valid");
        let mut returns =
            [crate::real_time::parameter_snapshot::RtBusReturnParameters::EMPTY; MAX_BUS_RETURNS];
        returns[bus.index()] = crate::real_time::parameter_snapshot::RtBusReturnParameters::new(
            EffectSlotId::new(live_slot).expect("the live slot id is non-zero"),
            &[],
            1.0,
        )
        .expect("the live return entry is valid");
        let snapshot = base.clone().with_returns(returns);

        let mut block = PatchAudioBlock::prepare(FRAME_COUNT).expect("match block prepares");
        block
            .begin_render(&snapshot, FRAME_COUNT)
            .expect("match snapshot fits");
        block
            .stem_mut(0, PatchId::new(1).expect("fixture PatchId is nonzero"))
            .expect("fixture stem exists")
            .copy_from_slice(&stem);

        let mut dry_mixer = MixEngine::new();
        dry_mixer
            .prepare(SAMPLE_RATE, FRAME_COUNT)
            .expect("match mixer prepares");
        let mut dry = vec![0.0_f32; FRAME_COUNT * 2];
        dry_mixer.mix(&block, &base, &mut dry);

        let probe = Arc::new(ReturnProbe::default());
        let mut mixer = MixEngine::new();
        mixer
            .prepare(SAMPLE_RATE, FRAME_COUNT)
            .expect("match mixer prepares");
        mixer
            .install_bus_return(
                bus,
                Box::new(HarnessMeteringReturn::new(Arc::clone(&probe))),
                harness_return_parameters(),
                1.0,
            )
            .expect("match return installs");
        let mut output = vec![0.0_f32; FRAME_COUNT * 2];
        mixer.mix(&block, &snapshot, &mut output);
        SendSeamRun {
            output,
            dry,
            bus_input_energy: probe.input_energy(),
        }
    };

    let attested = run_with_live_slot(1);
    let mismatched = if mutant_enabled {
        // Permissive matching: the mismatched attestation is silently
        // corrected to the prepared instance.
        run_with_live_slot(1)
    } else {
        run_with_live_slot(2)
    };
    let wet = |run: &SendSeamRun| -> f64 {
        run.output
            .iter()
            .zip(run.dry.iter())
            .map(|(after, dry)| {
                let delta = f64::from(*after - *dry);
                delta * delta
            })
            .sum()
    };
    let attested_wet_energy = wet(&attested);
    let mismatched_wet_energy = wet(&mismatched);

    PermissiveStructuralMatchObservation {
        case: BehavioralMutationCase::PermissiveStructuralMatch
            .as_str()
            .to_owned(),
        attested_wet_energy,
        mismatched_wet_energy,
        strict_matching_enforced: attested_wet_energy > 0.0
            && approximately_zero(mismatched_wet_energy),
        finite_audio: mismatched
            .output
            .iter()
            .chain(attested.output.iter())
            .all(|sample| sample.is_finite()),
    }
}

// ---- The declared refused-topology witness case ---------------------------

const TOPOLOGY_FRAME_COUNT: usize = 128;
const TOPOLOGY_SAMPLE_COUNT: usize = TOPOLOGY_FRAME_COUNT * 2;

struct TopologyFixture {
    app_loop: AppLoop<LockFreeControlHandle>,
    renderer: crate::real_time::AudioRenderer<
        crate::adapter::lock_free_audio_boundary::LockFreeAudioHandle,
        crate::adapter::lock_free_structural_graph_boundary::LockFreeStructuralAudioHandle,
        crate::adapter::atomic_audio_observation::AtomicAudioObservationWriter,
    >,
    worker:
        crate::testing::deterministic_graph_preparation_worker::DeterministicGraphPreparationHandle,
}

/// Composes the complete production-path topology fixture: reducer,
/// projector, deterministic worker, coordinator, and renderer, with the
/// production default return occupancy and one track sending to bus 3.
fn topology_fixture() -> TopologyFixture {
    use crate::real_time::audio_observation::AudioObservation as _;
    use crate::real_time::StructuralGraphBoundary as _;
    let registry = crate::adapter::production_instruments::production_capability_registry()
        .expect("production registry composes");
    let effects = crate::adapter::production_effects::production_effect_registry()
        .expect("production effect registry composes");
    let bank = crate::adapter::production_effects::production_default_bus_returns(&effects)
        .expect("production default returns occupy");

    let mut sends = [0.0_f32; MAX_BUS_RETURNS];
    sends[3] = 0.6;
    let mixer = MixerState::default().with_track(
        MixerTrackId::ALL[0],
        MixerTrackParameters::from_values(0.0, 0.0, false, false, sends)
            .expect("fixture sends are valid"),
    );

    let soundfont = crate::adapter::production_instruments::production_soundfont_capability()
        .expect("production SoundFont capability composes");
    let patch = Patch::new(
        PatchId::new(1).expect("fixture PatchId is nonzero"),
        "Topology 1".to_owned(),
        create_soundfont_config(
            &soundfont,
            SoundFontInstrument::new(0, 0, false).expect("fixture instrument is valid"),
        )
        .expect("fixture config matches the production descriptor"),
        MidiChannel::new(0).expect("fixture channel is valid"),
        PatchOutput::to_track(MixerTrackId::ALL[0]),
    );

    let mut state = AppState::for_graph_with_effects(
        registry.clone(),
        effects.clone(),
        fixture_globals(),
        GraphRevision::INITIAL,
    )
    .with_initial_returns(bank)
    .with_initial_mixer(mixer);
    state
        .apply(AppEvent::InstallPatches(vec![patch]))
        .expect("fixture Patch installs");

    let initial_transport =
        ParameterSnapshot::new(0, fixture_globals(), MixerState::default(), &[])
            .expect("initial transport parameters are valid");
    let boundary = LockFreeAudioBoundary::new(128, initial_transport);
    let (audio_control, audio_callback) = boundary.into_handles();
    let mut app_loop = AppLoop::new(
        state,
        StateProjector::for_graph(GraphRevision::INITIAL),
        audio_control,
    )
    .expect("fixture state projects");

    let audio_config = crate::shell::audio_output::AudioDeviceConfig::new(
        SAMPLE_RATE,
        2,
        crate::shell::audio_output::AudioSampleFormat::F32,
        TOPOLOGY_FRAME_COUNT,
    )
    .expect("fixture audio config is valid");
    let instrument_preparers =
        crate::adapter::production_instruments::production_instrument_preparers()
            .expect("production preparers compose");
    let effect_preparers = crate::adapter::production_effects::production_effect_preparers()
        .expect("production effect preparers compose");
    let initial_graph = PreparedGraphBuilder::new(&registry, &instrument_preparers)
        .with_effects(&effects, &effect_preparers)
        .with_returns(app_loop.bus_returns())
        .build(
            GraphRevision::INITIAL,
            app_loop.patches(),
            app_loop.current_parameters().clone(),
            SAMPLE_RATE,
            TOPOLOGY_FRAME_COUNT,
        )
        .expect("complete production graph prepares");

    let structural =
        crate::adapter::lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary::new(
            1,
            1,
            crate::real_time::GraphHandoffStatus::with_active(GraphRevision::INITIAL),
        )
        .expect("fixture structural boundary is valid");
    let (structural_control, structural_callback) = structural.into_handles();
    let worker = crate::testing::deterministic_graph_preparation_worker::DeterministicGraphPreparationWorker::new_with_effects(
        registry.clone(),
        crate::adapter::production_instruments::production_instrument_preparers()
            .expect("worker preparers compose"),
        effects.clone(),
        crate::adapter::production_effects::production_effect_preparers()
            .expect("worker effect preparers compose"),
        audio_config,
    );
    let worker_handle = worker.advance_handle();
    app_loop
        .configure_engine_selection(
            crate::synth::DescriptorDefaultConfigFactory::new(
                registry,
                crate::adapter::production_instruments::production_instrument_providers()
                    .expect("production providers compose"),
            ),
            worker,
            structural_control,
            &initial_graph,
            audio_config,
        )
        .expect("fixture engine-selection runtime configures");
    let (writer, _reader) =
        crate::adapter::atomic_audio_observation::AtomicAudioObservation::default().into_handles();
    let renderer = crate::real_time::AudioRenderer::with_observation(
        audio_callback,
        structural_callback,
        initial_graph,
        writer,
    );

    TopologyFixture {
        app_loop,
        renderer,
        worker: worker_handle,
    }
}

/// The declared goal-witness counterexample: a refused topology change must
/// leave the active graph, the canonical state, and the render untouched,
/// with an attributable reason, and a valid change immediately afterwards
/// must succeed. The mutant skips the refusal injection, so the refused
/// change is prepared and published anyway — every preservation predicate
/// collapses and the witness exits 1.
fn run_refused_topology(mutant_enabled: bool) -> RefusedTopologyObservation {
    let mut fixture = topology_fixture();
    let mut untouched = topology_fixture();
    let mut output = vec![0.0_f32; TOPOLOGY_SAMPLE_COUNT];
    let mut twin_output = vec![0.0_f32; TOPOLOGY_SAMPLE_COUNT];
    let patch_id = PatchId::new(1).expect("fixture PatchId is nonzero");
    let channel = MidiChannel::new(0).expect("fixture channel is valid");
    let note = MidiMessage::try_new(channel, MidiMessageKind::NoteOn, 64, 112)
        .expect("fixture note is valid");

    for target in [&mut fixture, &mut untouched] {
        target
            .app_loop
            .dispatch_from(
                AppEvent::Midi {
                    patch_id,
                    message: note,
                },
                EventSource::System,
            )
            .expect("fixture note dispatches");
        target.renderer.render(&mut output);
    }

    let bank_before = fixture.app_loop.bus_returns().clone();
    let patches_before = fixture.app_loop.patches().to_vec();
    let revision_before = fixture.renderer.active_revision();
    let entry = fixture.app_loop.effects().descriptors()[1].id().clone();
    let bus = BusId::ALL[3];

    fixture
        .app_loop
        .dispatch_action_from(
            crate::control::SemanticAction::SetReturnOccupancy {
                bus,
                entry: Some(entry.clone()),
            },
            EventSource::System,
        )
        .expect("occupancy request is accepted into the lifecycle");
    for expected in [
        crate::control::EngineSelectionStatusKind::Validating,
        crate::control::EngineSelectionStatusKind::Preparing,
    ] {
        assert_eq!(
            fixture
                .app_loop
                .advance_structural()
                .expect("admission phase advances")
                .engine_selection_lifecycle_advanced(),
            Some(expected)
        );
    }
    if !mutant_enabled {
        fixture
            .worker
            .fail_next(crate::control::EngineSelectionFailure::PreparationFailed);
    }
    assert!(fixture.worker.advance(), "worker advances one request");
    let progress = fixture
        .app_loop
        .advance_structural()
        .expect("structural control tick advances");

    // Render three further blocks on both fixtures; a refused change leaves
    // them sample-exactly identical.
    let mut render_preserved_exactly = true;
    let mut finite_audio = true;
    for _ in 0..3 {
        fixture.renderer.render(&mut output);
        untouched.renderer.render(&mut twin_output);
        render_preserved_exactly &= output == twin_output;
        finite_audio &= output.iter().all(|sample| sample.is_finite());
    }
    let _ = progress;
    let post_refusal_ack = fixture
        .app_loop
        .advance_structural()
        .expect("structural control tick advances");
    let _ = post_refusal_ack;

    let status = fixture.app_loop.engine_selection_status();
    let refusal_recorded = status.kind() == crate::control::EngineSelectionStatusKind::Failed
        && status
            .correlation()
            .is_some_and(|correlation| correlation.intent().is_occupancy());
    let rejection_reason = status
        .failure()
        .map(|failure| failure.name().to_owned())
        .unwrap_or_default();
    let tree: serde_json::Value =
        serde_json::from_str(fixture.app_loop.current_state_tree().json())
            .unwrap_or(serde_json::Value::Null);
    let rejection_reason_attributable = tree
        .pointer("/engineSelection/failure")
        .and_then(serde_json::Value::as_str)
        == Some("preparationFailed");
    let active_graph_preserved = fixture.renderer.active_revision() == revision_before;
    let canonical_state_preserved = fixture.app_loop.bus_returns() == &bank_before
        && fixture.app_loop.patches() == patches_before.as_slice();

    // SC-006: a valid change immediately afterwards succeeds through the
    // complete canonical lifecycle.
    let recovery_accepted = fixture
        .app_loop
        .dispatch_action_from(
            crate::control::SemanticAction::SetReturnOccupancy {
                bus,
                entry: Some(entry),
            },
            EventSource::System,
        )
        .is_ok();
    let mut recovery_completed = false;
    if recovery_accepted {
        for expected in [
            crate::control::EngineSelectionStatusKind::Validating,
            crate::control::EngineSelectionStatusKind::Preparing,
        ] {
            assert_eq!(
                fixture
                    .app_loop
                    .advance_structural()
                    .expect("recovery admission phase advances")
                    .engine_selection_lifecycle_advanced(),
                Some(expected)
            );
        }
    }
    if recovery_accepted && fixture.worker.advance() {
        if let Ok(staged) = fixture.app_loop.advance_structural() {
            if staged.graph_stage().is_some() {
                fixture.renderer.render(&mut output);
                finite_audio &= output.iter().all(|sample| sample.is_finite());
                if let Ok(ack) = fixture.app_loop.advance_structural() {
                    recovery_completed = ack.activation_acknowledged().is_some()
                        && fixture.app_loop.engine_selection_status().kind()
                            == crate::control::EngineSelectionStatusKind::Ready
                        && fixture.app_loop.bus_returns().bus_return(bus).is_occupied();
                }
            }
        }
    }

    RefusedTopologyObservation {
        case: BehavioralMutationCase::RefusedTopology.as_str().to_owned(),
        refusal_recorded,
        rejection_reason,
        rejection_reason_attributable,
        active_graph_preserved,
        canonical_state_preserved,
        render_preserved_exactly,
        post_rejection_valid_change_accepted: recovery_accepted && recovery_completed,
        finite_audio,
    }
}

fn energy(samples: &[f32]) -> f64 {
    samples
        .iter()
        .map(|sample| {
            let sample = f64::from(*sample);
            sample * sample
        })
        .sum()
}

fn stereo_energy(samples: &[f32]) -> (f64, f64) {
    samples
        .chunks_exact(2)
        .fold((0.0, 0.0), |(left, right), frame| {
            (
                left + f64::from(frame[0]) * f64::from(frame[0]),
                right + f64::from(frame[1]) * f64::from(frame[1]),
            )
        })
}

fn approximately_equal(left: f64, right: f64) -> bool {
    (left - right).abs() <= ENERGY_EPSILON
}

fn approximately_zero(value: f64) -> bool {
    value.abs() <= ENERGY_EPSILON
}

#[cfg(test)]
mod tests {
    use super::{BehavioralMutationCase, BehavioralMutationHarness, BehavioralMutationObservation};
    use serde_json::Value;
    use std::collections::BTreeSet;

    fn keys(observation: &BehavioralMutationObservation) -> BTreeSet<String> {
        serde_json::to_value(observation)
            .expect("observation serializes")
            .as_object()
            .expect("observation is a JSON object")
            .keys()
            .cloned()
            .collect()
    }

    fn expected(fields: &[&str]) -> BTreeSet<String> {
        fields.iter().map(|field| (*field).to_owned()).collect()
    }

    #[test]
    fn healthy_cases_satisfy_and_matching_mutants_falsify_the_same_predicates() {
        let harness = BehavioralMutationHarness::new();

        for case in BehavioralMutationCase::ALL {
            let healthy = harness.run(case, false);
            let mutant = harness.run(case, true);
            assert_eq!(healthy.exit_code(), 0, "{case:?}");
            assert!(healthy.observation().satisfies_witness(), "{case:?}");
            assert_eq!(mutant.exit_code(), 1, "{case:?}");
            assert!(mutant.observation().falsifies_witness(), "{case:?}");
            assert_eq!(keys(healthy.observation()), keys(mutant.observation()));
        }
    }

    #[test]
    fn faithful_effects_nonzero_sends_and_baseline_restoration() {
        let observation = BehavioralMutationHarness::new()
            .run(BehavioralMutationCase::DryToWetBypass, false)
            .into_observation();
        let BehavioralMutationObservation::DryToWetBypass(observation) = observation else {
            panic!("the dry-to-wet case retains its typed schema");
        };

        assert!(observation.dry_input_energy > 0.0);
        assert!(super::approximately_zero(
            observation.zero_send_reverb_input_energy
        ));
        assert!(super::approximately_zero(
            observation.zero_send_delay_input_energy
        ));
        assert!(observation.nonzero_send_reverb_input_energy > 0.0);
        assert!(observation.nonzero_send_delay_input_energy > 0.0);
        assert!(observation.nonzero_send_wet_output_energy > 0.0);
        assert!(observation.identical_effect_state);
        assert!(observation.dry_bypass_absent);
        assert!(observation.baseline_restored);
    }

    #[test]
    fn every_case_emits_its_exact_declared_schema() {
        let harness = BehavioralMutationHarness::new();
        let cases = [
            (
                BehavioralMutationCase::DroppedAdjustment,
                expected(&[
                    "case",
                    "adjustment_dispatched",
                    "adjust_event_recorded",
                    "selected_value_exact",
                    "unrelated_values_unchanged",
                    "projection_values_exact",
                    "baseline_restored",
                ]),
            ),
            (
                BehavioralMutationCase::CrossTrackParameterLeak,
                expected(&[
                    "case",
                    "edited_track_id",
                    "comparison_track_id",
                    "track_ids_distinct",
                    "parameter",
                    "parameter_cases_exercised",
                    "edited_value_before",
                    "edited_value_after",
                    "comparison_value_before",
                    "comparison_value_after",
                    "published_edited_value",
                    "published_comparison_value",
                    "edited_track_energy_before",
                    "edited_track_energy_after",
                    "comparison_track_energy_before",
                    "comparison_track_energy_after",
                    "edited_value_changed",
                    "comparison_value_unchanged",
                    "state_values_exact",
                    "published_values_exact",
                    "edited_track_audio_changed",
                    "unedited_track_audio_unchanged",
                    "all_track_parameters_isolated",
                    "dry_path_isolated",
                    "reverb_path_isolated",
                    "delay_path_isolated",
                    "baseline_restored",
                ]),
            ),
            (
                BehavioralMutationCase::PatchMisroute,
                expected(&[
                    "case",
                    "command_patch_matches_event",
                    "target_patch_received_command",
                    "target_stem_changed",
                    "untargeted_stems_unchanged",
                    "patch_routing_exact",
                ]),
            ),
            (
                BehavioralMutationCase::OmittedStateTreeLeaf,
                expected(&[
                    "case",
                    "schema_surface_equal",
                    "required_leaf_count",
                    "missing_leaf_count",
                    "unexpected_leaf_count",
                    "state_values_exact",
                    "projection_values_exact",
                ]),
            ),
            (
                BehavioralMutationCase::DryToWetBypass,
                expected(&[
                    "case",
                    "dry_input_energy",
                    "zero_send_reverb_input_energy",
                    "zero_send_delay_input_energy",
                    "zero_send_wet_output_energy",
                    "nonzero_send_reverb_input_energy",
                    "nonzero_send_delay_input_energy",
                    "nonzero_send_wet_output_energy",
                    "identical_effect_state",
                    "dry_bypass_absent",
                    "finite_audio",
                    "baseline_restored",
                ]),
            ),
            (
                BehavioralMutationCase::ZeroRenderer,
                expected(&[
                    "case",
                    "control_trace_complete",
                    "renderer_called",
                    "renderer_nonzero",
                    "render_peak",
                    "finite_audio",
                ]),
            ),
            (
                BehavioralMutationCase::SlotOrderSwap,
                expected(&[
                    "case",
                    "forward_energy",
                    "reversed_energy",
                    "order_difference_energy",
                    "order_sensitive",
                    "finite_audio",
                ]),
            ),
            (
                BehavioralMutationCase::EmptyReturnPassthrough,
                expected(&[
                    "case",
                    "accumulated_send_energy",
                    "unoccupied_wet_energy",
                    "output_matches_dry_exactly",
                    "unoccupied_return_silent",
                    "finite_audio",
                ]),
            ),
            (
                BehavioralMutationCase::PreGateSend,
                expected(&[
                    "case",
                    "post_fader_reference_energy",
                    "measured_send_energy",
                    "pre_fader_reference_energy",
                    "send_taken_post_fader",
                    "finite_audio",
                ]),
            ),
            (
                BehavioralMutationCase::MutedSendLeak,
                expected(&[
                    "case",
                    "sounding_send_energy",
                    "muted_send_energy",
                    "muted_wet_energy",
                    "mute_gates_sends",
                    "finite_audio",
                ]),
            ),
            (
                BehavioralMutationCase::PermissiveStructuralMatch,
                expected(&[
                    "case",
                    "attested_wet_energy",
                    "mismatched_wet_energy",
                    "strict_matching_enforced",
                    "finite_audio",
                ]),
            ),
            (
                BehavioralMutationCase::RefusedTopology,
                expected(&[
                    "case",
                    "refusal_recorded",
                    "rejection_reason",
                    "rejection_reason_attributable",
                    "active_graph_preserved",
                    "canonical_state_preserved",
                    "render_preserved_exactly",
                    "post_rejection_valid_change_accepted",
                    "finite_audio",
                ]),
            ),
        ];

        for (case, schema) in cases {
            let observation = harness.run(case, false);
            assert_eq!(keys(observation.observation()), schema, "{case:?}");
            let json: Value = serde_json::from_str(
                &observation
                    .observation_json()
                    .expect("observation serializes"),
            )
            .expect("observation JSON parses");
            assert_eq!(
                json["case"],
                Value::String(case.as_str().to_owned()),
                "{case:?}"
            );
        }
    }
}
