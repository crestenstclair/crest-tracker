use crate::control::event_record::{
    EmittedEvent, EventInput, EventOutcome, EventRecord, EventSource,
};
use crate::control::state_tree::StateTree;
use crate::control::text_projection::TextProjection;
use crate::real_time::audio_observation_snapshot::AudioObservationSnapshot;
use crate::real_time::GraphRevision;
use crate::synth::CapabilityId;
use crate::testing::live_demo_scene::{
    projected_parameter_values, LiveAudioPredicate, LiveDemoSceneError, LiveExpectedTransition,
};
use core::fmt;
use serde::Serialize;

use crate::control::{
    EngineSelectionRequestId, EngineSelectionStatusKind, PatchControlId, StructuralEditIntent,
};
use crate::kernel::PatchId;

/// Exact selected values copied from the canonical state, text, and RT projections.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveProjectedValue {
    selected_line: usize,
    selected_text: String,
    patch_control_id: Option<PatchControlId>,
    state_value: f32,
    parameter_value: f32,
}

impl LiveProjectedValue {
    pub const fn selected_line(&self) -> usize {
        self.selected_line
    }

    pub fn selected_text(&self) -> &str {
        &self.selected_text
    }

    pub fn patch_control_id(&self) -> Option<PatchControlId> {
        self.patch_control_id.clone()
    }

    pub const fn state_value(&self) -> f32 {
        self.state_value
    }

    pub const fn parameter_value(&self) -> f32 {
        self.parameter_value
    }
}

/// One immutable production-path correlation for an accepted parameter edit.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveDemoCheckpoint {
    step: usize,
    input: EventInput,
    expected_transition: LiveExpectedTransition,
    outcome: EventOutcome,
    generation: u64,
    state_hash: String,
    projected_value: LiveProjectedValue,
    parameter_generation: u64,
    emitted_effects: Vec<EmittedEvent>,
    audio_observation: AudioObservationSnapshot,
    audio_predicate: LiveAudioPredicate,
    audio_predicate_passed: bool,
}

impl LiveDemoCheckpoint {
    /// Correlates independently frozen expectations with canonical values and
    /// a newer callback observation. No expectation is derived from `record`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        step: usize,
        expected_transition: LiveExpectedTransition,
        record: &EventRecord,
        state_tree: &StateTree,
        text: &TextProjection,
        audio_sequence_before: u64,
        audio_observation: AudioObservationSnapshot,
        audio_predicate: LiveAudioPredicate,
    ) -> Result<Self, LiveDemoCheckpointError> {
        if record.source() != EventSource::DemoScene {
            return Err(LiveDemoCheckpointError::WrongEventSource);
        }
        if record.input() != expected_transition.input()
            || record.outcome() != expected_transition.outcome()
            || record.generation_before() != expected_transition.generation_before()
            || record.generation_after() != expected_transition.generation_after()
            || record.parameter_generation() != expected_transition.parameter_generation()
            || record.emitted_events() != expected_transition.emitted_effects()
        {
            return Err(LiveDemoCheckpointError::TransitionMismatch);
        }
        if record.rejection().map(|value| value.name()) != expected_transition.rejection() {
            return Err(LiveDemoCheckpointError::TransitionMismatch);
        }
        if state_tree.generation() != record.generation_after()
            || state_tree.state_hash() != record.state_hash_after()
            || state_tree.selected_line() != record.selected_line()
            || text.state_hash() != record.projection_state_hash()
            || text.selected_line() != record.selected_line()
            || record.parameter_generation() != record.generation_after()
        {
            return Err(LiveDemoCheckpointError::CanonicalProjectionMismatch);
        }

        let parameter = expected_transition
            .editable_parameter()
            .ok_or(LiveDemoCheckpointError::MissingEditableParameter)?;
        let expected_value = expected_transition
            .value_after()
            .ok_or(LiveDemoCheckpointError::MissingEditableParameter)?;
        let (state_value, parameter_value) = projected_parameter_values(state_tree, parameter)?;
        if state_value != expected_value || parameter_value != expected_value {
            return Err(LiveDemoCheckpointError::ProjectedValueMismatch {
                expected: expected_value,
                state: state_value,
                parameters: parameter_value,
            });
        }

        if audio_observation.sequence() <= audio_sequence_before {
            return Err(LiveDemoCheckpointError::StaleAudioObservation);
        }
        if audio_observation.parameter_generation() != record.parameter_generation() {
            return Err(LiveDemoCheckpointError::AudioGenerationMismatch {
                expected: record.parameter_generation(),
                actual: audio_observation.parameter_generation(),
            });
        }
        if !audio_fields_are_finite(audio_observation) {
            return Err(LiveDemoCheckpointError::NonFiniteAudioObservation);
        }
        let audio_predicate_passed = audio_predicate.evaluate(audio_observation);
        if !audio_predicate_passed {
            return Err(LiveDemoCheckpointError::AudioPredicateFailed(
                audio_predicate,
            ));
        }

        let selected_text = text
            .body()
            .lines()
            .nth(text.selected_line())
            .ok_or(LiveDemoCheckpointError::CanonicalProjectionMismatch)?
            .to_owned();
        if expected_transition.selected_line() != Some(record.selected_line())
            || expected_transition.selected_text() != Some(selected_text.as_str())
        {
            return Err(LiveDemoCheckpointError::ExpectedProjectionMismatch {
                step,
                expected_line: expected_transition.selected_line(),
                actual_line: record.selected_line(),
                expected_text: expected_transition
                    .selected_text()
                    .unwrap_or("<missing>")
                    .to_owned(),
                actual_text: selected_text,
            });
        }
        let patch_control_id = expected_transition.patch_control_id();
        if let Some(control) = patch_control_id.as_ref() {
            let control_id = control.as_str();
            let tree: serde_json::Value = serde_json::from_str(state_tree.json())
                .map_err(|_| LiveDemoCheckpointError::CanonicalProjectionMismatch)?;
            if tree
                .pointer("/interaction/activeFocus/controlId/id")
                .and_then(serde_json::Value::as_str)
                != Some(control_id.as_ref())
                || tree
                    .pointer("/patchPage/focusedControlId")
                    .and_then(serde_json::Value::as_str)
                    != Some(control_id.as_ref())
                || !(selected_text.starts_with("> ENVELOPE ")
                    || selected_text.starts_with("> OUTPUT ")
                    || selected_text.starts_with("> EFFECT_PARAMETER ")
                    || selected_text.starts_with("> DETAIL_PARAMETER "))
            {
                return Err(LiveDemoCheckpointError::CanonicalProjectionMismatch);
            }
        }

        Ok(Self {
            step,
            input: record.input().clone(),
            expected_transition,
            outcome: record.outcome(),
            generation: record.generation_after(),
            state_hash: record.state_hash_after().to_owned(),
            projected_value: LiveProjectedValue {
                selected_line: record.selected_line(),
                selected_text,
                patch_control_id,
                state_value,
                parameter_value,
            },
            parameter_generation: record.parameter_generation(),
            emitted_effects: record.emitted_events().to_vec(),
            audio_observation,
            audio_predicate,
            audio_predicate_passed,
        })
    }

    pub const fn step(&self) -> usize {
        self.step
    }

    pub const fn expected_transition(&self) -> &LiveExpectedTransition {
        &self.expected_transition
    }

    pub const fn outcome(&self) -> EventOutcome {
        self.outcome
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub fn state_hash(&self) -> &str {
        &self.state_hash
    }

    pub const fn projected_value(&self) -> &LiveProjectedValue {
        &self.projected_value
    }

    pub const fn parameter_generation(&self) -> u64 {
        self.parameter_generation
    }

    pub fn emitted_effects(&self) -> &[EmittedEvent] {
        &self.emitted_effects
    }

    pub const fn audio_observation(&self) -> AudioObservationSnapshot {
        self.audio_observation
    }

    pub const fn audio_predicate(&self) -> LiveAudioPredicate {
        self.audio_predicate
    }

    pub fn agrees(&self) -> bool {
        self.input.eq(self.expected_transition.input())
            && self.outcome == self.expected_transition.outcome()
            && self.generation == self.expected_transition.generation_after()
            && self.parameter_generation == self.expected_transition.parameter_generation()
            && self.emitted_effects.as_slice() == self.expected_transition.emitted_effects()
            && self.expected_transition.selected_line() == Some(self.projected_value.selected_line)
            && self.expected_transition.selected_text()
                == Some(self.projected_value.selected_text.as_str())
            && self.expected_transition.patch_control_id() == self.projected_value.patch_control_id
            && self.projected_value.state_value == self.projected_value.parameter_value
            && self.audio_observation.parameter_generation() == self.generation
            && self.audio_predicate_passed
    }
}

/// A serialized live checkpoint is one scalar observation, one
/// engine-selection lifecycle observation, or one topology-transition
/// observation. The evidence sets remain distinct even though the
/// standalone callback presents them uniformly.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LiveCheckpoint {
    Parameter {
        checkpoint: Box<LiveDemoCheckpoint>,
    },
    Engine {
        checkpoint: Box<LiveEngineCheckpoint>,
    },
    Topology {
        checkpoint: Box<LiveTopologyCheckpoint>,
    },
    /// One patch-editor correlation: the destination Patch a switch reached,
    /// the focus that resulted, and the audible consequence, together.
    PatchEditor {
        checkpoint: Box<LivePatchEditorCheckpoint>,
    },
}

impl LiveCheckpoint {
    pub fn parameter(checkpoint: LiveDemoCheckpoint) -> Self {
        Self::Parameter {
            checkpoint: Box::new(checkpoint),
        }
    }

    pub fn engine(checkpoint: LiveEngineCheckpoint) -> Self {
        Self::Engine {
            checkpoint: Box::new(checkpoint),
        }
    }

    pub fn topology(checkpoint: LiveTopologyCheckpoint) -> Self {
        Self::Topology {
            checkpoint: Box::new(checkpoint),
        }
    }

    pub fn patch_editor(checkpoint: LivePatchEditorCheckpoint) -> Self {
        Self::PatchEditor {
            checkpoint: Box::new(checkpoint),
        }
    }

    /// The patch-editor correlation this checkpoint carries, if any.
    pub fn as_patch_editor(&self) -> Option<&LivePatchEditorCheckpoint> {
        match self {
            Self::PatchEditor { checkpoint } => Some(checkpoint),
            Self::Parameter { .. } | Self::Engine { .. } | Self::Topology { .. } => None,
        }
    }

    pub const fn as_parameter(&self) -> Option<&LiveDemoCheckpoint> {
        match self {
            Self::Parameter { checkpoint } => Some(checkpoint),
            Self::Engine { .. } | Self::Topology { .. } | Self::PatchEditor { .. } => None,
        }
    }

    pub const fn as_engine(&self) -> Option<&LiveEngineCheckpoint> {
        match self {
            Self::Engine { checkpoint } => Some(checkpoint),
            Self::Parameter { .. } | Self::Topology { .. } | Self::PatchEditor { .. } => None,
        }
    }

    pub const fn as_topology(&self) -> Option<&LiveTopologyCheckpoint> {
        match self {
            Self::Topology { checkpoint } => Some(checkpoint),
            Self::Parameter { .. } | Self::Engine { .. } | Self::PatchEditor { .. } => None,
        }
    }

    pub fn agrees(&self) -> bool {
        match self {
            Self::Parameter { checkpoint } => checkpoint.agrees(),
            Self::Engine { checkpoint } => checkpoint.agrees(),
            Self::Topology { checkpoint } => checkpoint.agrees(),
            Self::PatchEditor { checkpoint } => checkpoint.agrees(),
        }
    }
}

/// One immutable correlation of a patch switch, the focus that resulted from
/// it, and the audible consequence measured on each Patch's own output track.
///
/// This is the gate's exact demand — "checkpoints must correlate the patch
/// switch, the resulting focus, and the audible consequence" — carried as one
/// value rather than assembled by a reader from three others. A checkpoint is
/// *correlating* only when all three agree with each other
/// ([`Self::correlates`]); emitting one does not make it so.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LivePatchEditorCheckpoint {
    transition: String,
    /// The Patch the projection speaks for after this transition — read from
    /// the canonical focus path, never from what the scene intended.
    destination_patch_id: Option<PatchId>,
    focus_before: crate::control::FocusPath,
    focus_after: crate::control::FocusPath,
    generation: u64,
    graph_revision: GraphRevision,
    /// The occupancy position this transition changed, when it changed one.
    slot: Option<crate::synth::effect_slot_id::EffectSlotIndex>,
    /// The parameter identity and value change, for the audible edit.
    edited_parameter: Option<PatchControlId>,
    /// Per-mixer-track RMS delta across this transition's own observation
    /// window. Each Patch's delta is read out of this by its own output track
    /// identity, so no Patch's consequence is ever read off another's meter.
    track_rms_deltas: Vec<f32>,
    /// The destination Patch's own output track, and the delta measured on it.
    destination_track_delta: f32,
    /// The largest delta measured on any *other* installed Patch's own output
    /// track over the same window.
    other_patch_track_delta: f32,
    audio_before_sequence: u64,
    audio_after_sequence: u64,
}

impl LivePatchEditorCheckpoint {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        transition: impl Into<String>,
        destination_patch_id: Option<PatchId>,
        focus_before: crate::control::FocusPath,
        focus_after: crate::control::FocusPath,
        generation: u64,
        graph_revision: GraphRevision,
        slot: Option<crate::synth::effect_slot_id::EffectSlotIndex>,
        edited_parameter: Option<PatchControlId>,
        track_rms_deltas: Vec<f32>,
        destination_track_delta: f32,
        other_patch_track_delta: f32,
        audio_before_sequence: u64,
        audio_after_sequence: u64,
    ) -> Self {
        Self {
            transition: transition.into(),
            destination_patch_id,
            focus_before,
            focus_after,
            generation,
            graph_revision,
            slot,
            edited_parameter,
            track_rms_deltas,
            destination_track_delta,
            other_patch_track_delta,
            audio_before_sequence,
            audio_after_sequence,
        }
    }

    pub fn transition(&self) -> &str {
        &self.transition
    }

    pub const fn destination_patch_id(&self) -> Option<PatchId> {
        self.destination_patch_id
    }

    pub const fn focus_after(&self) -> &crate::control::FocusPath {
        &self.focus_after
    }

    pub const fn destination_track_delta(&self) -> f32 {
        self.destination_track_delta
    }

    pub const fn other_patch_track_delta(&self) -> f32 {
        self.other_patch_track_delta
    }

    /// Whether this checkpoint actually correlates all three facts: it names a
    /// destination Patch, the resulting focus speaks for that same Patch, and
    /// the destination's own output moved further than any other Patch's over
    /// the same window by the declared margin.
    ///
    /// Counting emitted checkpoints instead of this is the defect T037 names.
    pub fn correlates(&self, reached: Option<PatchId>) -> bool {
        self.destination_patch_id.is_some()
            && self.destination_patch_id == reached
            && self.focus_after.patch_id() == self.destination_patch_id
            && self.destination_track_delta.is_finite()
            && self.other_patch_track_delta.is_finite()
            && self.destination_track_delta - self.other_patch_track_delta
                >= crate::testing::functional_patch_editor_observation::AUDIBLE_EDIT_DELTA_MARGIN
    }

    pub fn agrees(&self) -> bool {
        !self.transition.trim().is_empty()
            && self.audio_after_sequence > self.audio_before_sequence
            && self.track_rms_deltas.iter().all(|delta| delta.is_finite())
            && self.focus_after.patch_id() == self.destination_patch_id
    }
}

/// One immutable observation of a planned topology transition:
/// correlates the dispatched semantic cause, the accepted lifecycle
/// (request id, source and target graph revisions), the visible projection,
/// the measured edit responsiveness (NFR-008), and the audible consequence.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveTopologyCheckpoint {
    transition: String,
    transition_index: usize,
    action: Option<crate::control::SemanticAction>,
    outcome: EventOutcome,
    rejection: Option<String>,
    request_id: Option<EngineSelectionRequestId>,
    source_graph_revision: GraphRevision,
    target_graph_revision: Option<GraphRevision>,
    generation: u64,
    state_hash: String,
    /// NFR-008: 1-based index of the first painted frame after acceptance
    /// whose semantic generation covers the accepted edit. Must be 1 — the
    /// very next painted frame reflects the acceptance.
    frames_to_projection: Option<u64>,
    /// NFR-008: observation-sequence gap between the last block rendered on
    /// the source graph and the first observed block on the target graph.
    /// The deterministic scene drives the renderer block-by-block, where
    /// this is exactly 1; the physical scene records the honest sampled gap.
    observed_activation_sequence_gap: Option<u64>,
    /// NFR-008: observation-sequence distance from the target-note dispatch
    /// on the activated graph (or, for a held-note transition, from
    /// activation reaching Ready) to the first observed block satisfying the
    /// audible witness.
    render_blocks_to_audible: Option<u64>,
    /// The first observed block after the target note reached the activated
    /// graph (or, for lifecycle-free transitions, after the dispatch) already
    /// satisfies the transition's audible witness.
    audible_on_activated_graph: bool,
    audible_witness: crate::testing::live_effects_and_buses_scene::LiveTopologyAudibleWitness,
    audio_before: AudioObservationSnapshot,
    audio_after: AudioObservationSnapshot,
    /// The audio path never stopped across the transition: the observation
    /// sequence advanced, every field stayed finite, and the probe note kept
    /// ringing (rejections must leave held notes and the active graph
    /// untouched).
    audio_uninterrupted: bool,
    callback_allocations: usize,
    callback_destructions: usize,
    /// `Some(true)` when this transition held its probe note through
    /// activation — never re-sounding it — and the note was still observed
    /// active on the activated graph, so the sounding voice itself survived
    /// the swap. `None` for transitions that use the re-sound model, which is
    /// why this is an option rather than a plain flag.
    held_note_carried_across_activation: Option<bool>,
}

impl LiveTopologyCheckpoint {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        transition: impl Into<String>,
        transition_index: usize,
        action: Option<crate::control::SemanticAction>,
        outcome: EventOutcome,
        rejection: Option<String>,
        request_id: Option<EngineSelectionRequestId>,
        source_graph_revision: GraphRevision,
        target_graph_revision: Option<GraphRevision>,
        generation: u64,
        state_hash: impl Into<String>,
        frames_to_projection: Option<u64>,
        observed_activation_sequence_gap: Option<u64>,
        render_blocks_to_audible: Option<u64>,
        audible_on_activated_graph: bool,
        audible_witness: crate::testing::live_effects_and_buses_scene::LiveTopologyAudibleWitness,
        audio_before: AudioObservationSnapshot,
        audio_after: AudioObservationSnapshot,
        audio_uninterrupted: bool,
        callback_allocations: usize,
        callback_destructions: usize,
        held_note_carried_across_activation: Option<bool>,
    ) -> Result<Self, LiveDemoCheckpointError> {
        let checkpoint = Self {
            transition: transition.into(),
            transition_index,
            action,
            outcome,
            rejection,
            request_id,
            source_graph_revision,
            target_graph_revision,
            generation,
            state_hash: state_hash.into(),
            frames_to_projection,
            observed_activation_sequence_gap,
            render_blocks_to_audible,
            audible_on_activated_graph,
            audible_witness,
            audio_before,
            audio_after,
            audio_uninterrupted,
            callback_allocations,
            callback_destructions,
            held_note_carried_across_activation,
        };
        if !checkpoint.agrees() {
            return Err(LiveDemoCheckpointError::TopologyLifecycleMismatch);
        }
        Ok(checkpoint)
    }

    pub fn transition(&self) -> &str {
        &self.transition
    }

    pub const fn transition_index(&self) -> usize {
        self.transition_index
    }

    pub const fn outcome(&self) -> EventOutcome {
        self.outcome
    }

    pub fn rejection(&self) -> Option<&str> {
        self.rejection.as_deref()
    }

    pub const fn request_id(&self) -> Option<EngineSelectionRequestId> {
        self.request_id
    }

    pub const fn source_graph_revision(&self) -> GraphRevision {
        self.source_graph_revision
    }

    pub const fn target_graph_revision(&self) -> Option<GraphRevision> {
        self.target_graph_revision
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub const fn frames_to_projection(&self) -> Option<u64> {
        self.frames_to_projection
    }

    pub const fn observed_activation_sequence_gap(&self) -> Option<u64> {
        self.observed_activation_sequence_gap
    }

    pub const fn render_blocks_to_audible(&self) -> Option<u64> {
        self.render_blocks_to_audible
    }

    pub const fn audible_on_activated_graph(&self) -> bool {
        self.audible_on_activated_graph
    }

    pub const fn audible_witness(
        &self,
    ) -> crate::testing::live_effects_and_buses_scene::LiveTopologyAudibleWitness {
        self.audible_witness
    }

    pub const fn audio_before(&self) -> AudioObservationSnapshot {
        self.audio_before
    }

    pub const fn audio_after(&self) -> AudioObservationSnapshot {
        self.audio_after
    }

    pub const fn audio_uninterrupted(&self) -> bool {
        self.audio_uninterrupted
    }

    /// `Some(true)` when the held probe note survived activation without
    /// being re-sounded; `None` for re-sound-model transitions.
    pub const fn held_note_carried_across_activation(&self) -> Option<bool> {
        self.held_note_carried_across_activation
    }

    pub fn agrees(&self) -> bool {
        if self.transition.trim().is_empty()
            || self.state_hash.is_empty()
            || self.callback_allocations != 0
            || self.callback_destructions != 0
            || !self.audio_uninterrupted
            || !audio_fields_are_finite(self.audio_before)
            || !audio_fields_are_finite(self.audio_after)
            || self.audio_after.sequence() <= self.audio_before.sequence()
            // A held-note transition whose measured note did not survive
            // activation must fail loudly, never pass silently.
            || self.held_note_carried_across_activation == Some(false)
        {
            return false;
        }
        match self.outcome {
            EventOutcome::Rejected => {
                // A refused topology mutates nothing: no lifecycle identity,
                // no revision change, the reason attributable by name.
                self.rejection.is_some()
                    && self.request_id.is_none()
                    && self.target_graph_revision.is_none()
                    && self.frames_to_projection.is_none()
                    && self.observed_activation_sequence_gap.is_none()
                    && self.render_blocks_to_audible.is_none()
                    && self.audible_on_activated_graph
                    && self.audio_after.active_graph_revision() == self.source_graph_revision
            }
            EventOutcome::Accepted => match self.target_graph_revision {
                // An accepted occupancy transition: correlated request, a
                // strictly newer activated revision, the projection visible
                // within one frame, and the change audible in the very first
                // observed block on the target graph.
                Some(target) => {
                    self.rejection.is_none()
                        && self.action.is_some()
                        && self.request_id.is_some()
                        && target > self.source_graph_revision
                        && self.frames_to_projection == Some(1)
                        && self
                            .observed_activation_sequence_gap
                            .is_some_and(|gap| gap >= 1)
                        && self
                            .render_blocks_to_audible
                            .is_some_and(|blocks| blocks >= 1)
                        && self.audible_on_activated_graph
                        && self.audio_after.active_graph_revision() == target
                }
                // A support-driven scalar transition (send raise, reroute):
                // no lifecycle identity; the revision is unchanged and the
                // witness holds on the after-observation.
                None => {
                    self.rejection.is_none()
                        && self.request_id.is_none()
                        && self.frames_to_projection.is_none()
                        && self.observed_activation_sequence_gap.is_none()
                        && self.render_blocks_to_audible.is_none()
                        && self.audible_on_activated_graph
                        && self.audio_after.active_graph_revision() == self.source_graph_revision
                }
            },
        }
    }
}

/// One immutable observation of a planned live engine transition.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveEngineCheckpoint {
    transition: String,
    transition_index: usize,
    status: EngineSelectionStatusKind,
    request_id: EngineSelectionRequestId,
    patch_id: PatchId,
    focused_control_id: PatchControlId,
    source_capability_id: CapabilityId,
    target_capability_id: CapabilityId,
    intent: StructuralEditIntent,
    preset: Option<LivePresetProjection>,
    active_capability_id: CapabilityId,
    requested_capability_id: Option<CapabilityId>,
    generation: u64,
    state_hash: String,
    graph_revision: GraphRevision,
    handoff_active_revision: Option<GraphRevision>,
    handoff_retired_revision: Option<GraphRevision>,
    staged_revision: Option<GraphRevision>,
    in_flight_revision: Option<GraphRevision>,
    event_sequence: u64,
    source_audio_observation: Option<AudioObservationSnapshot>,
    source_audio_nonzero: bool,
    audio_observation: Option<AudioObservationSnapshot>,
    target_audio_nonzero: bool,
    callback_allocations: usize,
    callback_destructions: usize,
}

/// Exact authored and projected choice evidence for a structural preset replacement.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LivePresetProjection {
    source_choice_id: String,
    source_label: String,
    target_choice_id: String,
    target_label: String,
    active_choice_id: String,
    active_label: String,
    requested_choice_id: Option<String>,
    requested_label: Option<String>,
}

impl LivePresetProjection {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        source_choice_id: impl Into<String>,
        source_label: impl Into<String>,
        target_choice_id: impl Into<String>,
        target_label: impl Into<String>,
        active_choice_id: impl Into<String>,
        active_label: impl Into<String>,
        requested_choice_id: Option<String>,
        requested_label: Option<String>,
    ) -> Self {
        Self {
            source_choice_id: source_choice_id.into(),
            source_label: source_label.into(),
            target_choice_id: target_choice_id.into(),
            target_label: target_label.into(),
            active_choice_id: active_choice_id.into(),
            active_label: active_label.into(),
            requested_choice_id,
            requested_label,
        }
    }

    pub fn source_choice_id(&self) -> &str {
        &self.source_choice_id
    }

    pub fn source_label(&self) -> &str {
        &self.source_label
    }

    pub fn target_choice_id(&self) -> &str {
        &self.target_choice_id
    }

    pub fn target_label(&self) -> &str {
        &self.target_label
    }

    pub fn active_choice_id(&self) -> &str {
        &self.active_choice_id
    }

    pub fn active_label(&self) -> &str {
        &self.active_label
    }

    pub fn requested_choice_id(&self) -> Option<&str> {
        self.requested_choice_id.as_deref()
    }

    pub fn requested_label(&self) -> Option<&str> {
        self.requested_label.as_deref()
    }
}

impl LiveEngineCheckpoint {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        transition: impl Into<String>,
        transition_index: usize,
        status: EngineSelectionStatusKind,
        request_id: EngineSelectionRequestId,
        patch_id: PatchId,
        focused_control_id: PatchControlId,
        source_capability_id: CapabilityId,
        target_capability_id: CapabilityId,
        intent: StructuralEditIntent,
        preset: Option<LivePresetProjection>,
        active_capability_id: CapabilityId,
        requested_capability_id: Option<CapabilityId>,
        generation: u64,
        state_hash: impl Into<String>,
        graph_revision: GraphRevision,
        handoff_active_revision: Option<GraphRevision>,
        handoff_retired_revision: Option<GraphRevision>,
        staged_revision: Option<GraphRevision>,
        in_flight_revision: Option<GraphRevision>,
        event_sequence: u64,
        source_audio_observation: Option<AudioObservationSnapshot>,
        audio_observation: Option<AudioObservationSnapshot>,
        callback_allocations: usize,
        callback_destructions: usize,
    ) -> Result<Self, LiveDemoCheckpointError> {
        let source_audio_nonzero = source_audio_observation.is_some_and(|observation| {
            let effect = observation.patch_effect();
            audio_fields_are_finite(observation)
                && observation.primary_patch_id() == Some(patch_id)
                && observation.primary_active_notes() > 0
                && observation.primary_patch_rms() > 0.0
                && effect.patch_id() == Some(patch_id)
                && effect.input_rms() > 0.0
                && effect.output_rms() > 0.0
                && effect.difference_rms() > 0.0
                && effect.side_rms() > 0.0
        });
        let target_audio_nonzero = audio_observation.is_some_and(|observation| {
            let effect = observation.patch_effect();
            audio_fields_are_finite(observation)
                && observation.active_graph_revision() == graph_revision
                && observation.parameter_generation() == generation
                && observation.active_notes() > 0
                && observation.primary_patch_id() == Some(patch_id)
                && observation.primary_active_notes() > 0
                && observation.primary_patch_rms() > 0.0
                && effect.patch_id() == Some(patch_id)
                && effect.input_rms() > 0.0
                && effect.output_rms() > 0.0
                && effect.difference_rms() > 0.0
                && effect.side_rms() > 0.0
        });
        let checkpoint = Self {
            transition: transition.into(),
            transition_index,
            status,
            request_id,
            patch_id,
            focused_control_id,
            source_capability_id,
            target_capability_id,
            intent,
            preset,
            active_capability_id,
            requested_capability_id,
            generation,
            state_hash: state_hash.into(),
            graph_revision,
            handoff_active_revision,
            handoff_retired_revision,
            staged_revision,
            in_flight_revision,
            event_sequence,
            source_audio_observation,
            source_audio_nonzero,
            audio_observation,
            target_audio_nonzero,
            callback_allocations,
            callback_destructions,
        };
        if !checkpoint.agrees() {
            return Err(LiveDemoCheckpointError::EngineLifecycleMismatch);
        }
        Ok(checkpoint)
    }

    pub fn transition(&self) -> &str {
        &self.transition
    }

    pub const fn transition_index(&self) -> usize {
        self.transition_index
    }

    pub const fn status(&self) -> EngineSelectionStatusKind {
        self.status
    }

    pub const fn request_id(&self) -> EngineSelectionRequestId {
        self.request_id
    }

    pub const fn patch_id(&self) -> PatchId {
        self.patch_id
    }

    pub fn focused_control_id(&self) -> PatchControlId {
        self.focused_control_id.clone()
    }

    pub const fn source_capability_id(&self) -> &CapabilityId {
        &self.source_capability_id
    }

    pub const fn target_capability_id(&self) -> &CapabilityId {
        &self.target_capability_id
    }

    pub const fn intent(&self) -> &StructuralEditIntent {
        &self.intent
    }

    pub const fn preset(&self) -> Option<&LivePresetProjection> {
        self.preset.as_ref()
    }

    pub const fn graph_revision(&self) -> GraphRevision {
        self.graph_revision
    }

    pub const fn audio_observation(&self) -> Option<AudioObservationSnapshot> {
        self.audio_observation
    }

    pub const fn source_audio_observation(&self) -> Option<AudioObservationSnapshot> {
        self.source_audio_observation
    }

    pub const fn source_audio_nonzero(&self) -> bool {
        self.source_audio_nonzero
    }

    pub const fn target_audio_nonzero(&self) -> bool {
        self.target_audio_nonzero
    }

    pub const fn callback_allocations(&self) -> usize {
        self.callback_allocations
    }

    pub const fn callback_destructions(&self) -> usize {
        self.callback_destructions
    }

    pub fn agrees(&self) -> bool {
        let expected_control = match &self.intent {
            StructuralEditIntent::ReplaceCapability { .. } => PatchControlId::Engine,
            StructuralEditIntent::ReplaceParameterChoice { parameter_id, .. } => {
                PatchControlId::Capability(parameter_id.clone())
            }
            StructuralEditIntent::ReplaceAsset { parameter_id, .. } => {
                PatchControlId::Capability(parameter_id.clone())
            }
            StructuralEditIntent::SetVoiceBudget { .. }
            | StructuralEditIntent::ReplaceEffectAsset { .. }
            | StructuralEditIntent::SetSlotOccupancy { .. }
            | StructuralEditIntent::SetSendEffect { .. }
            | StructuralEditIntent::SetReturnOccupancy { .. }
            | StructuralEditIntent::PrepareAudition { .. }
            | StructuralEditIntent::AppendPatch { .. } => return false,
        };
        if self.transition.trim().is_empty()
            || self.request_id.is_none()
            || self.state_hash.is_empty()
            || self.focused_control_id != expected_control
            || self.callback_allocations != 0
            || self.callback_destructions != 0
        {
            return false;
        }
        let target_pending = self.staged_revision == Some(self.graph_revision)
            || self.in_flight_revision == Some(self.graph_revision);
        let capability_projection_exact = match &self.intent {
            StructuralEditIntent::SetVoiceBudget { .. }
            | StructuralEditIntent::ReplaceEffectAsset { .. }
            | StructuralEditIntent::SetSlotOccupancy { .. }
            | StructuralEditIntent::SetSendEffect { .. }
            | StructuralEditIntent::SetReturnOccupancy { .. }
            | StructuralEditIntent::AppendPatch { .. } => return false,
            StructuralEditIntent::ReplaceCapability {
                target_capability_id,
            } => {
                target_capability_id == &self.target_capability_id
                    && self.source_capability_id != self.target_capability_id
                    && self.preset.is_none()
                    && self.source_audio_observation.is_none()
                    && !self.source_audio_nonzero
                    && self.active_capability_id
                        == if self.status != EngineSelectionStatusKind::Ready {
                            self.source_capability_id.clone()
                        } else {
                            self.target_capability_id.clone()
                        }
                    && self.requested_capability_id
                        == (self.status != EngineSelectionStatusKind::Ready)
                            .then_some(self.target_capability_id.clone())
            }
            StructuralEditIntent::ReplaceParameterChoice {
                capability_id,
                parameter_id: _,
                choice_id,
            } => {
                let Some(preset) = &self.preset else {
                    return false;
                };
                capability_id == &self.source_capability_id
                    && self.source_capability_id == self.target_capability_id
                    && self.active_capability_id == self.source_capability_id
                    && self.requested_capability_id.is_none()
                    && choice_id == preset.target_choice_id()
                    && preset.active_choice_id()
                        == if self.status != EngineSelectionStatusKind::Ready {
                            preset.source_choice_id()
                        } else {
                            preset.target_choice_id()
                        }
                    && preset.requested_choice_id()
                        == (self.status != EngineSelectionStatusKind::Ready)
                            .then_some(preset.target_choice_id())
                    && preset.requested_label()
                        == (self.status != EngineSelectionStatusKind::Ready)
                            .then_some(preset.target_label())
                    && !preset.source_label().is_empty()
                    && !preset.target_label().is_empty()
                    && !preset.active_label().is_empty()
                    && (!matches!(
                        self.status,
                        EngineSelectionStatusKind::Loading
                            | EngineSelectionStatusKind::Validating
                            | EngineSelectionStatusKind::Preparing
                    ) || self.source_audio_nonzero)
            }
            StructuralEditIntent::ReplaceAsset { .. }
            | StructuralEditIntent::PrepareAudition { .. } => return false,
        };
        capability_projection_exact
            && match self.status {
                EngineSelectionStatusKind::Loading
                | EngineSelectionStatusKind::Validating
                | EngineSelectionStatusKind::Preparing => {
                    self.handoff_active_revision == Some(self.graph_revision)
                        && self.staged_revision.is_none()
                        && self.in_flight_revision.is_none()
                        && self.audio_observation.is_none()
                        && !self.target_audio_nonzero
                }
                EngineSelectionStatusKind::Activating => {
                    self.handoff_active_revision.is_some()
                        && target_pending
                        && self.audio_observation.is_none()
                        && !self.target_audio_nonzero
                }
                EngineSelectionStatusKind::Ready => {
                    self.handoff_active_revision == Some(self.graph_revision)
                        && self.staged_revision.is_none()
                        && self.in_flight_revision.is_none()
                        && self.audio_observation.is_some()
                        && self.target_audio_nonzero
                }
                EngineSelectionStatusKind::Unavailable | EngineSelectionStatusKind::Failed => false,
            }
    }
}

fn audio_fields_are_finite(observation: AudioObservationSnapshot) -> bool {
    let effect = observation.patch_effect();
    observation.left_peak().is_finite()
        && observation.right_peak().is_finite()
        && observation.output_rms().is_finite()
        && observation.reverb_input_rms().is_finite()
        && observation.delay_input_rms().is_finite()
        && observation.wet_output_rms().is_finite()
        && effect.input_rms().is_finite()
        && effect.output_rms().is_finite()
        && effect.difference_rms().is_finite()
        && effect.side_rms().is_finite()
        && observation.non_finite_samples() == 0
}

/// A falsifiable mismatch while constructing one live checkpoint.
#[derive(Clone, Debug, PartialEq)]
pub enum LiveDemoCheckpointError {
    WrongEventSource,
    TransitionMismatch,
    CanonicalProjectionMismatch,
    ExpectedProjectionMismatch {
        step: usize,
        expected_line: Option<usize>,
        actual_line: usize,
        expected_text: String,
        actual_text: String,
    },
    MissingEditableParameter,
    ProjectedValueMismatch {
        expected: f32,
        state: f32,
        parameters: f32,
    },
    StaleAudioObservation,
    AudioGenerationMismatch {
        expected: u64,
        actual: u64,
    },
    NonFiniteAudioObservation,
    AudioPredicateFailed(LiveAudioPredicate),
    EngineLifecycleMismatch,
    TopologyLifecycleMismatch,
    Scene(LiveDemoSceneError),
}

impl From<LiveDemoSceneError> for LiveDemoCheckpointError {
    fn from(error: LiveDemoSceneError) -> Self {
        Self::Scene(error)
    }
}

impl fmt::Display for LiveDemoCheckpointError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongEventSource => formatter.write_str("checkpoint event did not come from DemoScene"),
            Self::TransitionMismatch => formatter.write_str("actual transition differs from its pre-dispatch expectation"),
            Self::CanonicalProjectionMismatch => formatter.write_str("event record, StateTree, and TextProjection are not one canonical generation"),
            Self::ExpectedProjectionMismatch {
                step,
                expected_line,
                actual_line,
                expected_text,
                actual_text,
            } => write!(
                formatter,
                "selected TextProjection at live step {step} differs from its pre-dispatch expectation: line {actual_line} (expected {expected_line:?}), text {actual_text:?} (expected {expected_text:?})"
            ),
            Self::MissingEditableParameter => formatter.write_str("parameter checkpoint has no editable parameter expectation"),
            Self::ProjectedValueMismatch { expected, state, parameters } => write!(formatter, "expected value {expected}, StateTree has {state}, ParameterSnapshot has {parameters}"),
            Self::StaleAudioObservation => formatter.write_str("audio observation did not advance after dispatch"),
            Self::AudioGenerationMismatch { expected, actual } => write!(formatter, "audio observation generation {actual} does not match checkpoint generation {expected}"),
            Self::NonFiniteAudioObservation => formatter.write_str("audio observation contains non-finite output"),
            Self::AudioPredicateFailed(predicate) => write!(formatter, "audio observation did not satisfy {predicate:?}"),
            Self::EngineLifecycleMismatch => formatter.write_str(
                "engine checkpoint does not match its lifecycle, revision, or target-audio contract",
            ),
            Self::TopologyLifecycleMismatch => formatter.write_str(
                "topology checkpoint does not match its lifecycle, revision, responsiveness, or audible contract",
            ),
            Self::Scene(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for LiveDemoCheckpointError {}
