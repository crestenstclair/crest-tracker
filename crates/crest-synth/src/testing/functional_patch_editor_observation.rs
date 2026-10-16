//! The structured result the functional Patch editor's live acceptance emits,
//! and the accumulator that measures it.
//!
//! Every field here is **measured** from the production reducer, the canonical
//! projection, the prepared renderer's audio observation, the painting page's
//! own acknowledgment, and the host's teardown — never copied from expected
//! data. Where a fact has one producer it is transported from that producer
//! rather than recomputed beside it: the PATCH strip's
//! group structure is decided by the page and carried in the paint ack, and
//! this observation copies what arrived.
//!
//! # Keying, and why it is the sharpest rule here
//!
//! Every "second Patch" counter is keyed by the PatchId resolved from the
//! **final state's installed order**, never from the scene's own subject. A
//! scene whose patch switch has been defeated still
//! visits three slots and still makes an audible edit — just on the wrong
//! instrument. A counter that counted the work without checking where it
//! landed would pass under defeat, which is exactly the shape of defect this
//! mission has spent its length finding.
//!
//! # Wire form
//!
//! Snake case, matching the acceptance schema and the shipped
//! `SixteenTrackMixerRoutingObservation` beside it.

use crate::control::app_state::{AppState, SemanticActionAvailability};
use crate::control::{
    FocusPath, InteractionMode, MixerControlId, PatchControlId, PatchDetailSubject,
    SemanticControlId, SemanticControlKind, SemanticControlValue, SemanticGraphicalViewModel,
    SemanticSurfaceSummary, SurfaceId,
};
use crate::kernel::patch_id::PatchId;
use crate::mixer::mixer_track_id::MixerTrackId;
use crate::real_time::audio_observation_snapshot::AudioObservationSnapshot;
use crate::synth::effect_slot_id::EffectSlotIndex;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// The stdout marker the acceptance runner reads. Emitted only after semantic cleanup,
/// stream release, worker shutdown, graph collection, window close, and a
/// successful parent-process return.
pub const FUNCTIONAL_PATCH_EDITOR_OBSERVATION_MARKER: &str =
    "CREST_FUNCTIONAL_PATCH_EDITOR_LIVE_OBSERVATION ";

/// How far the second Patch's measured audible delta must exceed the first
/// Patch's for the edit to count as having reached the second instrument and
/// only it.
///
/// **This is a declared threshold, not a measured one.** An exact zero on the
/// untargeted Patch is unattainable on a live decaying voice — natural
/// block-to-block RMS drift is nonzero — so the pair uses a bounded comparison:
/// both deltas are measured on their own output
/// tracks over the same window, the second required to exceed the first by
/// this margin. The number is set above the block-to-block drift of a silent
/// or decaying track and far below the smallest edit this scene makes. The
/// first live run is what confirms it; until then it is a declaration.
pub const AUDIBLE_EDIT_DELTA_MARGIN: f32 = 1.0e-3;

/// The declared row count of the PATCH Utility panel.
const DECLARED_UTILITY_ROWS: u32 = 5;

/// The focused machine-readable result of the Phase 5 functional Patch editor
/// live acceptance.
///
/// The field order and names are the declared acceptance schema.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FunctionalPatchEditorObservation {
    schema_version: u32,
    patches_installed: u32,
    patches_focused: u32,
    patch_switch_generations_exact: bool,
    patch_switch_schema_mismatch_projections: u32,
    patch_switch_refused_at_end: bool,
    second_patch_id_distinct: bool,
    overview_sections_projected: u32,
    overview_root_controls_projected: u32,
    overview_projection_mismatches: u32,
    utility_rows_projected: u32,
    utility_rows_unavailable: u32,
    utility_serialization_key_labels: u32,
    master_volume_single_owner: bool,
    midi_input_rechannelled: bool,
    numeric_rows_missing_range_or_unit: u32,
    per_row_valid_actions_agree_at_focus: bool,
    requested_value_present_while_in_flight: bool,
    requested_value_present_when_settled: u32,
    second_patch_slots_visited: u32,
    second_patch_occupancy_transitions: u32,
    second_patch_focus_verified_transitions: u32,
    second_patch_audible_edit_delta: f32,
    first_patch_audible_edit_delta: f32,
    audible_edit_isolated_to_second_patch: bool,
    checkpoints_correlating_switch_focus_audio: u32,
    detail_subjects_served: u32,
    detail_surface_identities: u32,
    detail_return_exact: bool,
    detail_entry_refused_from_empty_slot: bool,
    voice_limit_refusals: u64,
    voice_limit_truncated_latched_voices: u32,
    note_offs_refused: u64,
    events_dropped: u64,
    callback_allocations: u64,
    callback_destructions: u64,
    qualifying_webview_frames: u32,
    projection_generation_gaps: u32,
    physical_audio_nonzero: bool,
    desktop_viewport_painted: bool,
    active_notes_after_cleanup: u32,
    window_closed: bool,
    stream_released: bool,
    owned_graphs_remaining: u32,
}

impl FunctionalPatchEditorObservation {
    pub const SCHEMA_VERSION: u32 = 1;

    /// Every declared predicate that this observation does **not** satisfy,
    /// named by its acceptance field.
    ///
    /// The controlled negative reports this list rather than a bare exit code,
    /// so a negative that failed for an unrelated reason is visible as such
    /// instead of being credited as falsification (T039).
    pub fn shortfalls(&self) -> Vec<&'static str> {
        let mut failed = Vec::new();
        let mut require = |holds: bool, field: &'static str| {
            if !holds {
                failed.push(field);
            }
        };
        require(self.patches_installed > 1, "patches_installed");
        require(self.patches_focused > 1, "patches_focused");
        require(
            self.patch_switch_generations_exact,
            "patch_switch_generations_exact",
        );
        require(
            self.patch_switch_schema_mismatch_projections == 0,
            "patch_switch_schema_mismatch_projections",
        );
        require(
            self.patch_switch_refused_at_end,
            "patch_switch_refused_at_end",
        );
        require(self.second_patch_id_distinct, "second_patch_id_distinct");
        require(
            self.overview_sections_projected == 2,
            "overview_sections_projected",
        );
        require(
            self.overview_root_controls_projected == 4,
            "overview_root_controls_projected",
        );
        require(
            self.overview_projection_mismatches == 0,
            "overview_projection_mismatches",
        );
        require(
            self.utility_rows_projected == DECLARED_UTILITY_ROWS,
            "utility_rows_projected",
        );
        require(
            self.utility_rows_unavailable == 0,
            "utility_rows_unavailable",
        );
        require(
            self.utility_serialization_key_labels == 0,
            "utility_serialization_key_labels",
        );
        require(
            self.master_volume_single_owner,
            "master_volume_single_owner",
        );
        require(self.midi_input_rechannelled, "midi_input_rechannelled");
        require(
            self.numeric_rows_missing_range_or_unit == 0,
            "numeric_rows_missing_range_or_unit",
        );
        require(
            self.per_row_valid_actions_agree_at_focus,
            "per_row_valid_actions_agree_at_focus",
        );
        require(
            self.requested_value_present_while_in_flight,
            "requested_value_present_while_in_flight",
        );
        require(
            self.requested_value_present_when_settled == 0,
            "requested_value_present_when_settled",
        );
        require(
            self.second_patch_slots_visited == EffectSlotIndex::ALL.len() as u32,
            "second_patch_slots_visited",
        );
        require(
            self.second_patch_occupancy_transitions > 1,
            "second_patch_occupancy_transitions",
        );
        require(
            self.second_patch_focus_verified_transitions > 1,
            "second_patch_focus_verified_transitions",
        );
        require(
            self.second_patch_audible_edit_delta > 0.0,
            "second_patch_audible_edit_delta",
        );
        // F-47's bounded comparison stands in for the unattainable exact zero:
        // the edit moved *this* instrument and not *that* one. The verdict is
        // carried as its own field so the predicate asserts the claim rather
        // than a proxy for it; both raw deltas stay reported beside it, because
        // a verdict without its inputs cannot be argued with (F-57).
        require(
            self.audible_edit_isolated_to_second_patch,
            "audible_edit_isolated_to_second_patch",
        );
        require(
            self.checkpoints_correlating_switch_focus_audio > 0,
            "checkpoints_correlating_switch_focus_audio",
        );
        require(self.detail_subjects_served == 2, "detail_subjects_served");
        require(
            self.detail_surface_identities == 1,
            "detail_surface_identities",
        );
        require(self.detail_return_exact, "detail_return_exact");
        require(
            self.detail_entry_refused_from_empty_slot,
            "detail_entry_refused_from_empty_slot",
        );
        require(self.voice_limit_refusals > 0, "voice_limit_refusals");
        require(
            self.voice_limit_truncated_latched_voices == 0,
            "voice_limit_truncated_latched_voices",
        );
        require(self.note_offs_refused == 0, "note_offs_refused");
        require(self.events_dropped == 0, "events_dropped");
        require(self.callback_allocations == 0, "callback_allocations");
        require(self.callback_destructions == 0, "callback_destructions");
        require(
            self.qualifying_webview_frames > 0,
            "qualifying_webview_frames",
        );
        require(
            self.projection_generation_gaps == 0,
            "projection_generation_gaps",
        );
        require(self.physical_audio_nonzero, "physical_audio_nonzero");
        require(self.desktop_viewport_painted, "desktop_viewport_painted");
        require(
            self.active_notes_after_cleanup == 0,
            "active_notes_after_cleanup",
        );
        require(self.window_closed, "window_closed");
        require(self.stream_released, "stream_released");
        require(self.owned_graphs_remaining == 0, "owned_graphs_remaining");
        failed
    }

    pub fn is_complete(&self) -> bool {
        self.shortfalls().is_empty()
    }

    pub const fn patches_focused(&self) -> u32 {
        self.patches_focused
    }

    pub const fn second_patch_id_distinct(&self) -> bool {
        self.second_patch_id_distinct
    }

    pub const fn second_patch_slots_visited(&self) -> u32 {
        self.second_patch_slots_visited
    }

    pub const fn second_patch_audible_edit_delta(&self) -> f32 {
        self.second_patch_audible_edit_delta
    }

    pub const fn first_patch_audible_edit_delta(&self) -> f32 {
        self.first_patch_audible_edit_delta
    }

    pub const fn audible_edit_isolated_to_second_patch(&self) -> bool {
        self.audible_edit_isolated_to_second_patch
    }

    pub const fn checkpoints_correlating_switch_focus_audio(&self) -> u32 {
        self.checkpoints_correlating_switch_focus_audio
    }
}

/// One occupancy transition observed on a named Patch, with whether focus was
/// verified on the transition's own row before and after it committed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ObservedOccupancy {
    pub(crate) patch_id: PatchId,
    pub(crate) slot: EffectSlotIndex,
    pub(crate) focus_verified: bool,
}

/// The audible consequence of one edit, measured on each Patch's own output
/// track over the same observation window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ObservedAudibleEdit {
    pub(crate) patch_id: PatchId,
    pub(crate) deltas_by_track: [f32; MixerTrackId::COUNT],
}

/// The live accumulator behind [`FunctionalPatchEditorObservation`].
///
/// It records **raw facts keyed by the PatchId they actually happened on**.
/// Nothing here knows which Patch the scene intended to reach; the second
/// Patch is resolved from the final installed order at [`Self::resolve`] time,
/// which is what makes a defeated scene's work fail to count (F-47).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PatchEditorMeasurement {
    focused_patch_ids: BTreeSet<PatchId>,
    /// Every change of focused Patch observed in the projection stream, as
    /// `(from, to)`. Identity comes from what was projected; the generations
    /// below come from the event log, because more than one accepted event
    /// can land between two samples and a sampled generation delta would
    /// report a false inexactness.
    focus_transitions: Vec<(PatchId, PatchId)>,
    /// Every accepted `SelectPatch` record's `(generation before, after)`.
    switch_generations: Vec<(u64, u64)>,
    last_focused_patch: Option<PatchId>,
    switch_refused_at_end: bool,
    schema_mismatch_projections: u32,
    overview_sections_projected: Option<u32>,
    overview_root_controls_projected: Option<u32>,
    overview_projection_mismatches: u32,
    utility_rows_projected: Option<u32>,
    utility_rows_unavailable: u32,
    utility_serialization_key_labels: u32,
    master_volume_owner_counts: BTreeSet<usize>,
    midi_input_values: BTreeSet<String>,
    numeric_rows_missing_range_or_unit: u32,
    valid_actions_disagreements: u32,
    valid_actions_samples: u32,
    requested_value_while_in_flight: bool,
    requested_value_when_settled: u32,
    /// Distinct slot indices whose occupancy row held verified focus, per
    /// Patch.
    slots_visited: BTreeMap<PatchId, BTreeSet<EffectSlotIndex>>,
    occupancies: Vec<ObservedOccupancy>,
    audible_edits: Vec<ObservedAudibleEdit>,
    correlating_checkpoints: u32,
    detail_subjects: BTreeSet<String>,
    detail_surfaces: BTreeSet<&'static str>,
    detail_return_origins: Vec<(FocusPath, Option<FocusPath>)>,
    /// The focus the run was on when the detail surface opened, and the last
    /// focus seen outside it.
    detail_entry_origin: Option<FocusPath>,
    last_non_detail_focus: Option<FocusPath>,
    detail_entry_refused_from_empty_slot: bool,
    projection_generations: Vec<u64>,
    /// The previous audio sample's `(voice-limit refusals, active notes)`.
    last_audio_sample: Option<(u64, u32)>,
    truncated_latched_voices: u32,
    note_offs_refused: u64,
    /// Output track per installed Patch, read from the production state.
    track_by_patch: BTreeMap<PatchId, MixerTrackId>,
}

impl PatchEditorMeasurement {
    /// Samples one canonical projection and the reducer that produced it.
    ///
    /// Called on every control advance while the patch-editor scene runs, so
    /// the projected facts are measured across the whole run rather than at
    /// one convenient moment.
    pub(crate) fn observe_projection(
        &mut self,
        state: &AppState,
        model: &SemanticGraphicalViewModel,
    ) {
        self.projection_generations.push(model.generation());
        for patch in state.patches() {
            self.track_by_patch
                .insert(patch.id(), patch.output().track_id());
        }
        if let Some(patch_id) = model.focus_path().patch_id() {
            self.focused_patch_ids.insert(patch_id);
            if let Some(previous) = self.last_focused_patch {
                if previous != patch_id {
                    self.focus_transitions.push((previous, patch_id));
                }
            }
            self.last_focused_patch = Some(patch_id);
        }

        // FR-013: exactly one owner of the master volume across every surface.
        let master_owners = model
            .surfaces()
            .iter()
            .flat_map(|surface| surface.controls())
            .filter(|control| {
                matches!(
                    control.path().control_id(),
                    SemanticControlId::Patch(PatchControlId::Global(
                        crate::mixer::global_parameters::GlobalParameter::MasterGainDb
                    )) | SemanticControlId::Mixer(MixerControlId::Global {
                        parameter: crate::mixer::global_parameters::GlobalParameter::MasterGainDb
                    })
                )
            })
            .count();
        self.master_volume_owner_counts.insert(master_owners);

        if let Some(utility) = model.surface(SurfaceId::PatchUtility) {
            self.utility_rows_projected = Some(utility.controls().len() as u32);
            for control in utility.controls() {
                if !control.visible() {
                    self.utility_rows_unavailable = self.utility_rows_unavailable.saturating_add(1);
                }
                if label_reads_as_a_serialization_key(control.label()) {
                    self.utility_serialization_key_labels =
                        self.utility_serialization_key_labels.saturating_add(1);
                }
                if let SemanticControlId::Patch(PatchControlId::MidiInput) =
                    control.path().control_id()
                {
                    self.midi_input_values
                        .insert(control_value_text(control.value()));
                }
            }
        }

        if let Some(main) = model.surface(SurfaceId::PatchMain) {
            self.overview_sections_projected = Some(main.sections().len() as u32);
            self.overview_root_controls_projected = Some(main.controls().len() as u32);
            let expected_controls = [
                PatchControlId::Engine,
                PatchControlId::EffectSlot(EffectSlotIndex::ALL[0]),
                PatchControlId::EffectSlot(EffectSlotIndex::ALL[1]),
                PatchControlId::EffectSlot(EffectSlotIndex::ALL[2]),
            ];
            let controls_match = main
                .controls()
                .iter()
                .map(|control| control.path().control_id().clone())
                .eq(expected_controls.into_iter().map(SemanticControlId::Patch));
            let sections_match =
                main.sections()
                    .iter()
                    .map(|section| section.id())
                    .eq(["overview.engine", "overview.effects"])
                    && main.sections().iter().all(|section| {
                        section.control_paths().iter().all(|path| {
                            main.controls().iter().any(|control| control.path() == path)
                        })
                    });
            if !controls_match || !sections_match {
                self.overview_projection_mismatches =
                    self.overview_projection_mismatches.saturating_add(1);
            }
        }

        for surface in model.surfaces() {
            for control in surface.controls() {
                // A numeric row — continuous or stepped — must project the
                // range it moves within; a unit is presentation and a voice
                // count legitimately has none.
                if matches!(
                    control.kind(),
                    SemanticControlKind::Continuous | SemanticControlKind::Stepped
                ) && control.numeric_range().is_none()
                {
                    self.numeric_rows_missing_range_or_unit =
                        self.numeric_rows_missing_range_or_unit.saturating_add(1);
                }
                if control.requested_value().is_some() {
                    if state.engine_selection().is_in_flight() {
                        self.requested_value_while_in_flight = true;
                    } else {
                        self.requested_value_when_settled =
                            self.requested_value_when_settled.saturating_add(1);
                    }
                }
            }
            // FR-014: the open detail surface names its subject and the Patch
            // it belongs to. Two subjects served by one surface identity is
            // the claim, so both are recorded from the projection itself.
            if let SemanticSurfaceSummary::PatchDetail { subject, .. } = surface.summary() {
                self.detail_subjects.insert(detail_subject_key(subject));
                self.detail_surfaces.insert(surface_identity(surface.id()));
            }
        }

        // The detail round trip, read from the projection stream rather than
        // declared by the scene: the focus the run was on when detail opened,
        // and the focus it landed on when detail closed. `detail_return_exact`
        // is those two being the same path.
        let on_detail = model.focus_path().surface() == SurfaceId::PatchDetail;
        match (self.detail_entry_origin.clone(), on_detail) {
            (None, true) => {
                self.detail_entry_origin = self.last_non_detail_focus.clone();
            }
            (Some(origin), false) => {
                self.detail_return_origins
                    .push((origin, Some(model.focus_path().clone())));
                self.detail_entry_origin = None;
            }
            _ => {}
        }
        if !on_detail {
            self.last_non_detail_focus = Some(model.focus_path().clone());
        }

        // The focused row's projected valid actions against the production
        // reducer's own answer, on the state that produced this projection.
        if let Some(focused) = model.focused_control() {
            let mut availability = SemanticActionAvailability::new(state);
            let projected: BTreeSet<String> = focused
                .valid_actions()
                .iter()
                .map(|valid| format!("{:?}", valid.action()))
                .collect();
            let reduced: BTreeSet<String> = crate::control::SemanticAction::surface_descriptor()
                .iter()
                .filter(|action| availability.accepts(action))
                .map(|action| format!("{action:?}"))
                .collect();
            self.valid_actions_samples = self.valid_actions_samples.saturating_add(1);
            if projected != reduced {
                self.valid_actions_disagreements =
                    self.valid_actions_disagreements.saturating_add(1);
            }
        }

        // A projection whose focused control identity the focused Patch's own
        // schema does not host is a schema mismatch after a switch — the
        // defect that would show one Patch's capability under another's name.
        if model.focus_path().surface() == SurfaceId::PatchMain
            && model.interaction_mode() == InteractionMode::Navigate
        {
            if let (Some(patch_id), SemanticControlId::Patch(control)) = (
                model.focus_path().patch_id(),
                model.focus_path().control_id(),
            ) {
                let hosted = state
                    .focused_patch_controls()
                    .map(|controls| controls.contains(control))
                    .unwrap_or(false);
                let focused_is_projection_subject =
                    state.interaction().patch_focus() == Some(patch_id);
                if !hosted || !focused_is_projection_subject {
                    self.schema_mismatch_projections =
                        self.schema_mismatch_projections.saturating_add(1);
                }
            }
        }
    }

    /// The Patch the run's focus most recently reached, as observed — never
    /// the Patch the scene intended to reach.
    pub(crate) fn reached_patch_id(&self) -> Option<PatchId> {
        self.focus_transitions.last().map(|(_, to)| *to)
    }

    /// Reads every accepted `SelectPatch` record's exact generation pair and
    /// every synchronously projected accepted generation out of the canonical
    /// event log.
    ///
    /// The log is the only place the exact pairs exist: one fixture poll can
    /// dispatch more than one accepted MIDI event between runner samples. An
    /// accepted record is projection evidence rather than an input proxy:
    /// `AppLoop::dispatch_internal` constructs the coherent production
    /// projections before it constructs and appends that record. Filling those
    /// certified intermediate generations prevents batching from being
    /// misreported as a skipped projection.
    pub(crate) fn observe_switches_from_event_log(
        &mut self,
        event_log: &crate::control::event_log::EventLog,
    ) {
        self.projection_generations.extend(
            event_log
                .records()
                .iter()
                .filter(|record| {
                    record.outcome() == crate::control::event_record::EventOutcome::Accepted
                })
                .flat_map(|record| [record.generation_before(), record.generation_after()]),
        );
        self.switch_generations = event_log
            .records()
            .iter()
            .filter(|record| {
                record.outcome() == crate::control::event_record::EventOutcome::Accepted
                    && matches!(
                        record.input(),
                        crate::control::event_record::EventInput::SelectPatch { .. }
                    )
            })
            .map(|record| (record.generation_before(), record.generation_after()))
            .collect();
    }

    /// Records the typed unchanged rejection of a switch past the last Patch.
    pub(crate) fn observe_switch_refused_at_end(&mut self, focus_unchanged: bool) {
        self.switch_refused_at_end = focus_unchanged;
    }

    /// Records one committed occupancy transition and whether the scene held
    /// verified focus on that slot's own row across it.
    pub(crate) fn observe_occupancy(&mut self, occupancy: ObservedOccupancy) {
        if occupancy.focus_verified {
            self.slots_visited
                .entry(occupancy.patch_id)
                .or_default()
                .insert(occupancy.slot);
        }
        self.occupancies.push(occupancy);
    }

    /// Records one audible edit's measured per-track deltas.
    pub(crate) fn observe_audible_edit(&mut self, edit: ObservedAudibleEdit) {
        self.audible_edits.push(edit);
    }

    /// Records one checkpoint that carried the switch, the resulting focus,
    /// and the audible consequence together. Counted here, never derived from
    /// how many checkpoints were emitted.
    pub(crate) fn observe_correlating_checkpoint(&mut self) {
        self.correlating_checkpoints = self.correlating_checkpoints.saturating_add(1);
    }

    /// Records that detail entry from an empty slot row was refused.
    pub(crate) fn observe_detail_entry_refused_from_empty_slot(&mut self) {
        self.detail_entry_refused_from_empty_slot = true;
    }

    /// Samples the production audio observation.
    ///
    /// The voice limit refuses rather than steals: a note arriving at the
    /// ceiling is not started and nothing sounding is ended. So a latched
    /// voice count that *fell* across a block in which refusals accrued is
    /// truncation, and it is counted here from the stream itself rather than
    /// from anything the scene declares about its own burst.
    pub(crate) fn observe_audio(&mut self, snapshot: AudioObservationSnapshot) {
        if let Some((previous_refusals, previous_notes)) = self.last_audio_sample {
            if snapshot.voice_limit_refusals() > previous_refusals
                && snapshot.active_notes() < previous_notes
            {
                self.truncated_latched_voices = self
                    .truncated_latched_voices
                    .saturating_add(previous_notes - snapshot.active_notes());
            }
        }
        self.last_audio_sample = Some((snapshot.voice_limit_refusals(), snapshot.active_notes()));
    }

    /// Counts note-offs the reducer refused, from the canonical event log.
    ///
    /// A refused note-off would strand a latched voice, which is why the
    /// renderer's limit lets every note-off through untested. Zero is the
    /// claim; this counts what actually happened.
    pub(crate) fn observe_note_offs_from_event_log(
        &mut self,
        event_log: &crate::control::event_log::EventLog,
    ) {
        use crate::control::event_record::{EventInput, EventOutcome, MidiKind};
        self.note_offs_refused = event_log
            .records()
            .iter()
            .filter(|record| {
                record.outcome() == EventOutcome::Rejected
                    && matches!(
                        record.input(),
                        EventInput::Midi { message, .. }
                            if matches!(
                                message.kind(),
                                MidiKind::NoteOff | MidiKind::AllNotesOff
                            )
                    )
            })
            .count() as u64;
    }

    /// Resolves the accumulated facts into the emitted observation.
    ///
    /// `installed_order` is the **final state's** installed Patch order. Every
    /// second-Patch counter keys off `installed_order[1]`, never off whatever
    /// the scene believed its subject was (F-47).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn resolve(
        &self,
        installed_order: &[PatchId],
        teardown: PatchEditorTeardown,
    ) -> FunctionalPatchEditorObservation {
        let first = installed_order.first().copied();
        let second = installed_order.get(1).copied();
        let second_slots = second
            .and_then(|id| self.slots_visited.get(&id))
            .map_or(0, BTreeSet::len) as u32;
        let second_occupancies = self
            .occupancies
            .iter()
            .filter(|occupancy| Some(occupancy.patch_id) == second)
            .count() as u32;
        let second_focus_verified = self
            .occupancies
            .iter()
            .filter(|occupancy| Some(occupancy.patch_id) == second && occupancy.focus_verified)
            .count() as u32;

        // The audible pair, measured on each Patch's own output track over
        // the same window. The edit credited here is the one the scene made
        // on the second Patch; a run with no such edit reports zero for both
        // rather than borrowing another edit's numbers.
        let edit = second.and_then(|id| {
            self.audible_edits
                .iter()
                .find(|edit| edit.patch_id == id)
                .copied()
        });
        let delta_on = |patch: Option<PatchId>| match (
            edit,
            patch.and_then(|id| self.track_by_patch.get(&id)),
        ) {
            (Some(edit), Some(track)) => edit.deltas_by_track[track.index()],
            _ => 0.0,
        };

        let switch_reached_second = self
            .focus_transitions
            .iter()
            .any(|(from, to)| Some(*to) == second && from != to);
        let generations_exact = !self.switch_generations.is_empty()
            && self
                .switch_generations
                .iter()
                .all(|(before, after)| after.saturating_sub(*before) == 1);

        let mut sorted_generations = self.projection_generations.clone();
        sorted_generations.sort_unstable();
        sorted_generations.dedup();
        let generation_gaps = sorted_generations
            .windows(2)
            .filter(|pair| pair[1].saturating_sub(pair[0]) > 1)
            .count() as u32;

        FunctionalPatchEditorObservation {
            schema_version: FunctionalPatchEditorObservation::SCHEMA_VERSION,
            patches_installed: installed_order.len() as u32,
            patches_focused: self.focused_patch_ids.len() as u32,
            patch_switch_generations_exact: generations_exact,
            patch_switch_schema_mismatch_projections: self.schema_mismatch_projections,
            patch_switch_refused_at_end: self.switch_refused_at_end,
            second_patch_id_distinct: switch_reached_second,
            overview_sections_projected: self.overview_sections_projected.unwrap_or(0),
            overview_root_controls_projected: self.overview_root_controls_projected.unwrap_or(0),
            overview_projection_mismatches: self.overview_projection_mismatches,
            utility_rows_projected: self.utility_rows_projected.unwrap_or(0),
            utility_rows_unavailable: self.utility_rows_unavailable,
            utility_serialization_key_labels: self.utility_serialization_key_labels,
            master_volume_single_owner: self.master_volume_owner_counts == BTreeSet::from([1]),
            // The scene now makes and reverses a real MIDI-input edit even
            // though the fixture's adjacent channel already has another
            // subscriber. Seeing more than one projected value therefore
            // proves both Patch-local reprojection and shared-channel
            // editability through the production reducer.
            midi_input_rechannelled: self.midi_input_values.len() > 1,
            numeric_rows_missing_range_or_unit: self.numeric_rows_missing_range_or_unit,
            per_row_valid_actions_agree_at_focus: self.valid_actions_samples > 0
                && self.valid_actions_disagreements == 0,
            requested_value_present_while_in_flight: self.requested_value_while_in_flight,
            requested_value_present_when_settled: self.requested_value_when_settled,
            second_patch_slots_visited: second_slots,
            second_patch_occupancy_transitions: second_occupancies,
            second_patch_focus_verified_transitions: second_focus_verified,
            second_patch_audible_edit_delta: delta_on(second),
            first_patch_audible_edit_delta: delta_on(first),
            // The isolation verdict itself, not the raw pair it is drawn from.
            // A run with no edit on the second Patch reports both deltas at
            // zero, and zero does not clear the margin — so absent evidence
            // reads as "not isolated" rather than as isolation by default.
            audible_edit_isolated_to_second_patch: delta_on(second) - delta_on(first)
                >= AUDIBLE_EDIT_DELTA_MARGIN,
            checkpoints_correlating_switch_focus_audio: self.correlating_checkpoints,
            detail_subjects_served: self.detail_subjects.len() as u32,
            detail_surface_identities: self.detail_surfaces.len() as u32,
            detail_return_exact: !self.detail_return_origins.is_empty()
                && self
                    .detail_return_origins
                    .iter()
                    .all(|(origin, landed)| landed.as_ref() == Some(origin)),
            detail_entry_refused_from_empty_slot: self.detail_entry_refused_from_empty_slot,
            voice_limit_refusals: teardown.voice_limit_refusals,
            voice_limit_truncated_latched_voices: self.truncated_latched_voices,
            note_offs_refused: self.note_offs_refused,
            events_dropped: teardown.events_dropped,
            callback_allocations: teardown.callback_allocations,
            callback_destructions: teardown.callback_destructions,
            qualifying_webview_frames: teardown.qualifying_webview_frames,
            projection_generation_gaps: generation_gaps,
            physical_audio_nonzero: teardown.physical_audio_nonzero,
            desktop_viewport_painted: teardown.desktop_viewport_painted,
            active_notes_after_cleanup: teardown.active_notes_after_cleanup,
            window_closed: teardown.window_closed,
            stream_released: teardown.stream_released,
            owned_graphs_remaining: teardown.owned_graphs_remaining,
        }
    }
}

/// The facts only the host knows, attached after the window returns, the
/// stream is dropped, the worker is shut down, and graph ownership is
/// collected.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PatchEditorTeardown {
    pub voice_limit_refusals: u64,
    pub events_dropped: u64,
    pub callback_allocations: u64,
    pub callback_destructions: u64,
    pub qualifying_webview_frames: u32,
    pub physical_audio_nonzero: bool,
    pub desktop_viewport_painted: bool,
    pub active_notes_after_cleanup: u32,
    pub window_closed: bool,
    pub stream_released: bool,
    pub owned_graphs_remaining: u32,
}

/// The RMS delta on every mixer track between two observations of the same
/// stream. Each Patch's own output track is read out of this by identity, so
/// no Patch's delta is ever read off another Patch's meter.
pub(crate) fn per_track_rms_deltas(
    before: AudioObservationSnapshot,
    after: AudioObservationSnapshot,
) -> [f32; MixerTrackId::COUNT] {
    let mut deltas = [0.0; MixerTrackId::COUNT];
    let (before_tracks, after_tracks) = (before.tracks(), after.tracks());
    for (index, delta) in deltas.iter_mut().enumerate() {
        *delta = (after_tracks[index].rms() - before_tracks[index].rms()).abs();
    }
    deltas
}

/// Whether a projected label reads as a serialization key rather than as
/// authored presentation text: dotted or camel-cased identifier shapes with no
/// spaces (`patch.output.trimGainDb`), which is the defect FR-014 closes.
fn label_reads_as_a_serialization_key(label: &str) -> bool {
    let trimmed = label.trim();
    if trimmed.is_empty() || trimmed.contains(' ') {
        return false;
    }
    trimmed.contains('.')
        || trimmed.contains('_')
        || (trimmed
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_lowercase())
            && trimmed
                .chars()
                .any(|character| character.is_ascii_uppercase()))
}

fn control_value_text(value: &SemanticControlValue) -> String {
    format!("{value:?}")
}

fn detail_subject_key(subject: &PatchDetailSubject) -> String {
    format!("{subject:?}")
}

/// The stable identity of one surface, as the projection names it. Used as a
/// set key so `detail_surface_identities` counts identities rather than
/// occurrences.
const fn surface_identity(id: SurfaceId) -> &'static str {
    match id {
        SurfaceId::Sends => "sends",
        SurfaceId::PatchMain => "patchMain",
        SurfaceId::PatchUtility => "patchUtility",
        SurfaceId::PatchDetail => "patchDetail",
        SurfaceId::PatchChoice => "patchChoice",
        SurfaceId::FileBrowser => "fileBrowser",
        SurfaceId::MixerMain => "mixerMain",
        SurfaceId::MixerInspector => "mixerInspector",
        SurfaceId::MidiDeviceSettings => "midiDeviceSettings",
        SurfaceId::ControllerSettings => "controllerSettings",
        SurfaceId::SaveLoadSettings => "saveLoadSettings",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn teardown() -> PatchEditorTeardown {
        PatchEditorTeardown {
            voice_limit_refusals: 3,
            events_dropped: 0,
            callback_allocations: 0,
            callback_destructions: 0,
            qualifying_webview_frames: 12,
            physical_audio_nonzero: true,
            desktop_viewport_painted: true,
            active_notes_after_cleanup: 0,
            window_closed: true,
            stream_released: true,
            owned_graphs_remaining: 0,
        }
    }

    fn patch(value: u32) -> PatchId {
        PatchId::new(value).expect("a one-based fixture id is valid")
    }

    /// A measurement carrying every fact a healthy run produces, so the tests
    /// below can remove exactly one and see the predicate that answers for it.
    fn healthy(first: PatchId, second: PatchId) -> PatchEditorMeasurement {
        let mut measurement = PatchEditorMeasurement::default();
        measurement.track_by_patch.insert(
            first,
            MixerTrackId::new(0).expect("track 0 is always valid"),
        );
        measurement.track_by_patch.insert(
            second,
            MixerTrackId::new(1).expect("track 1 is always valid"),
        );
        measurement.focused_patch_ids.insert(first);
        measurement.focused_patch_ids.insert(second);
        measurement.focus_transitions.push((first, second));
        measurement.switch_generations.push((10, 11));
        measurement.observe_switch_refused_at_end(true);
        measurement.overview_sections_projected = Some(2);
        measurement.overview_root_controls_projected = Some(4);
        measurement.utility_rows_projected = Some(DECLARED_UTILITY_ROWS);
        measurement.master_volume_owner_counts.insert(1);
        measurement
            .midi_input_values
            .insert("Channel(1)".to_owned());
        measurement
            .midi_input_values
            .insert("Channel(2)".to_owned());
        measurement.valid_actions_samples = 40;
        measurement.requested_value_while_in_flight = true;
        for slot in EffectSlotIndex::ALL {
            measurement.observe_occupancy(ObservedOccupancy {
                patch_id: second,
                slot,
                focus_verified: true,
            });
        }
        let mut deltas = [0.0; MixerTrackId::COUNT];
        deltas[1] = 0.05;
        deltas[0] = 0.000_01;
        measurement.observe_audible_edit(ObservedAudibleEdit {
            patch_id: second,
            deltas_by_track: deltas,
        });
        measurement.observe_correlating_checkpoint();
        measurement.detail_subjects.insert("instrument".to_owned());
        measurement.detail_subjects.insert("effect".to_owned());
        measurement
            .detail_surfaces
            .insert(surface_identity(SurfaceId::PatchDetail));
        let origin = FocusPath::patch_main(first, None, PatchControlId::Engine);
        measurement
            .detail_return_origins
            .push((origin.clone(), Some(origin)));
        measurement.observe_detail_entry_refused_from_empty_slot();
        measurement.projection_generations.extend([1, 2, 3, 4]);
        measurement
    }

    #[test]
    fn a_healthy_measurement_satisfies_every_declared_predicate() {
        let (first, second) = (patch(1), patch(2));
        let observation = healthy(first, second).resolve(&[first, second], teardown());
        assert_eq!(observation.shortfalls(), Vec::<&str>::new());
        assert!(observation.is_complete());
    }

    /// The whole point of the keying rule: a scene that did all the work on
    /// the *first* Patch credits none of it, because the counters key off the
    /// final installed order rather than the scene's own subject (F-47).
    #[test]
    fn work_done_on_the_first_patch_credits_nothing_to_the_second() {
        let (first, second) = (patch(1), patch(2));
        let mut measurement = PatchEditorMeasurement::default();
        measurement.track_by_patch.insert(
            first,
            MixerTrackId::new(0).expect("track 0 is always valid"),
        );
        measurement.track_by_patch.insert(
            second,
            MixerTrackId::new(1).expect("track 1 is always valid"),
        );
        measurement.focused_patch_ids.insert(first);
        // Three slots visited, three occupancy transitions, an audible edit —
        // all of it on the first Patch.
        for slot in EffectSlotIndex::ALL {
            measurement.observe_occupancy(ObservedOccupancy {
                patch_id: first,
                slot,
                focus_verified: true,
            });
        }
        let mut deltas = [0.0; MixerTrackId::COUNT];
        deltas[0] = 0.05;
        measurement.observe_audible_edit(ObservedAudibleEdit {
            patch_id: first,
            deltas_by_track: deltas,
        });

        let observation = measurement.resolve(&[first, second], teardown());
        assert_eq!(observation.second_patch_slots_visited(), 0);
        assert_eq!(observation.second_patch_audible_edit_delta(), 0.0);
        assert!(!observation.second_patch_id_distinct());
        assert!(!observation.audible_edit_isolated_to_second_patch());
        assert_eq!(observation.patches_focused(), 1);
        for expected in [
            "patches_focused",
            "second_patch_id_distinct",
            "second_patch_slots_visited",
            "second_patch_audible_edit_delta",
            "audible_edit_isolated_to_second_patch",
        ] {
            assert!(
                observation.shortfalls().contains(&expected),
                "the defeated run must fail {expected}",
            );
        }
    }

    /// Switching and switching back credits one reach, not two.
    #[test]
    fn patches_focused_counts_distinct_ids_across_a_switch_and_return() {
        let (first, second) = (patch(1), patch(2));
        let mut measurement = healthy(first, second);
        measurement.focus_transitions.push((second, first));
        measurement.focus_transitions.push((first, second));
        measurement.switch_generations.push((11, 12));
        measurement.switch_generations.push((12, 13));
        let observation = measurement.resolve(&[first, second], teardown());
        assert_eq!(observation.patches_focused(), 2);
    }

    /// An edit that moved both signals is credited to neither: the bounded
    /// comparison is what distinguishes reach from a renamed path (F-47).
    ///
    /// Both raw deltas stay reported through the failure, because the verdict
    /// is only arguable with its inputs in hand (F-57).
    #[test]
    fn an_edit_that_moved_both_patches_fails_the_bounded_comparison() {
        let (first, second) = (patch(1), patch(2));
        let mut measurement = healthy(first, second);
        let mut deltas = [0.0; MixerTrackId::COUNT];
        deltas[0] = 0.05;
        deltas[1] = 0.05;
        measurement.audible_edits.clear();
        measurement.observe_audible_edit(ObservedAudibleEdit {
            patch_id: second,
            deltas_by_track: deltas,
        });
        let observation = measurement.resolve(&[first, second], teardown());
        assert!(!observation.audible_edit_isolated_to_second_patch());
        assert!(observation
            .shortfalls()
            .contains(&"audible_edit_isolated_to_second_patch"));
        assert_eq!(observation.second_patch_audible_edit_delta(), 0.05);
        assert_eq!(observation.first_patch_audible_edit_delta(), 0.05);
    }

    /// The isolation verdict is the margin comparison and nothing else: a
    /// second-Patch delta that clears the first's by less than the declared
    /// margin is not isolation, however large either number is on its own.
    ///
    /// Both cases sit clear of the margin rather than on it. An exact-boundary
    /// case would be asserting a property of f32 rounding — `0.05 + 1.0e-3`
    /// less `0.05` is `0.000_999_998`, under the margin — and not a property of
    /// the rule.
    #[test]
    fn the_isolation_verdict_is_the_declared_margin_not_the_raw_size() {
        let (first, second) = (patch(1), patch(2));
        let gap_of = |gap: f32| {
            let mut measurement = healthy(first, second);
            let mut deltas = [0.0; MixerTrackId::COUNT];
            deltas[0] = 0.05;
            deltas[1] = 0.05 + gap;
            measurement.audible_edits.clear();
            measurement.observe_audible_edit(ObservedAudibleEdit {
                patch_id: second,
                deltas_by_track: deltas,
            });
            measurement.resolve(&[first, second], teardown())
        };
        // Well under the margin: both signals moved together on a large edit,
        // so the size of either delta buys nothing and neither is credited.
        let short = gap_of(AUDIBLE_EDIT_DELTA_MARGIN / 10.0);
        assert!(!short.audible_edit_isolated_to_second_patch());
        assert!(short
            .shortfalls()
            .contains(&"audible_edit_isolated_to_second_patch"));
        // Well over it: the second Patch moved and the first did not follow.
        let cleared = gap_of(AUDIBLE_EDIT_DELTA_MARGIN * 10.0);
        assert!(cleared.audible_edit_isolated_to_second_patch());
        assert!(!cleared
            .shortfalls()
            .contains(&"audible_edit_isolated_to_second_patch"));
    }

    /// The counter counts correlating checkpoints. Nothing increments it as a
    /// side effect of emitting a checkpoint.
    #[test]
    fn the_correlating_counter_counts_only_what_was_recorded_as_correlating() {
        let (first, second) = (patch(1), patch(2));
        let mut measurement = healthy(first, second);
        measurement.correlating_checkpoints = 0;
        let observation = measurement.resolve(&[first, second], teardown());
        assert_eq!(observation.checkpoints_correlating_switch_focus_audio(), 0);
        assert!(observation
            .shortfalls()
            .contains(&"checkpoints_correlating_switch_focus_audio"));
    }

    /// A skipped generation between two sampled projections is a gap.
    #[test]
    fn projection_generation_gaps_count_skipped_generations() {
        let (first, second) = (patch(1), patch(2));
        let mut measurement = healthy(first, second);
        measurement.projection_generations.push(9);
        let observation = measurement.resolve(&[first, second], teardown());
        assert!(observation
            .shortfalls()
            .contains(&"projection_generation_gaps"));
    }

    #[test]
    fn serialization_key_labels_are_recognised_and_authored_labels_are_not() {
        assert!(label_reads_as_a_serialization_key(
            "patch.output.trimGainDb"
        ));
        assert!(label_reads_as_a_serialization_key("voiceLimit"));
        assert!(label_reads_as_a_serialization_key("master_gain_db"));
        assert!(!label_reads_as_a_serialization_key("Voice Limit"));
        assert!(!label_reads_as_a_serialization_key("Trim"));
        assert!(!label_reads_as_a_serialization_key(""));
    }

    #[test]
    fn per_track_deltas_are_absolute_and_track_local() {
        let before = AudioObservationSnapshot::default();
        let after = AudioObservationSnapshot::default();
        assert_eq!(
            per_track_rms_deltas(before, after),
            [0.0; MixerTrackId::COUNT]
        );
    }

    /// The emitted wire names are the declared acceptance schema, exactly.
    #[test]
    fn the_emitted_schema_matches_the_declared_acceptance_fields() {
        let (first, second) = (patch(1), patch(2));
        let observation = healthy(first, second).resolve(&[first, second], teardown());
        let value = serde_json::to_value(&observation).expect("the observation serializes");
        let object = value.as_object().expect("the observation is a JSON object");
        for field in ACCEPTANCE_SCHEMA_FIELDS {
            assert!(
                object.contains_key(field),
                "the emitted observation must carry the declared acceptance field {field}",
            );
        }
        // Only `schema_version` may be carried beyond the declared schema.
        for key in object.keys() {
            assert!(
                key == "schema_version" || ACCEPTANCE_SCHEMA_FIELDS.contains(&key.as_str()),
                "the emitted observation carries {key}, which acceptance does not declare",
            );
        }
    }

    /// The declared observation schema, in order. Written out so a field
    /// renamed in the serializer fails here rather than on the physical rig.
    const ACCEPTANCE_SCHEMA_FIELDS: [&str; 43] = [
        "patches_installed",
        "patches_focused",
        "patch_switch_generations_exact",
        "patch_switch_schema_mismatch_projections",
        "patch_switch_refused_at_end",
        "second_patch_id_distinct",
        "overview_sections_projected",
        "overview_root_controls_projected",
        "overview_projection_mismatches",
        "utility_rows_projected",
        "utility_rows_unavailable",
        "utility_serialization_key_labels",
        "master_volume_single_owner",
        "midi_input_rechannelled",
        "numeric_rows_missing_range_or_unit",
        "per_row_valid_actions_agree_at_focus",
        "requested_value_present_while_in_flight",
        "requested_value_present_when_settled",
        "second_patch_slots_visited",
        "second_patch_occupancy_transitions",
        "second_patch_focus_verified_transitions",
        "second_patch_audible_edit_delta",
        "first_patch_audible_edit_delta",
        "audible_edit_isolated_to_second_patch",
        "checkpoints_correlating_switch_focus_audio",
        "detail_subjects_served",
        "detail_surface_identities",
        "detail_return_exact",
        "detail_entry_refused_from_empty_slot",
        "voice_limit_refusals",
        "voice_limit_truncated_latched_voices",
        "note_offs_refused",
        "events_dropped",
        "callback_allocations",
        "callback_destructions",
        "qualifying_webview_frames",
        "projection_generation_gaps",
        "physical_audio_nonzero",
        "desktop_viewport_painted",
        "active_notes_after_cleanup",
        "window_closed",
        "stream_released",
        "owned_graphs_remaining",
    ];
}
