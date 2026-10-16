use crate::control::app_state::EventRejection;
use crate::control::event_log::EventLog;
use crate::control::event_record::{EmittedEvent, EventInput, EventOutcome, EventSource, MidiKind};
use crate::control::state_tree::StateTree;
use crate::control::{
    GraphicalShellProjection, InteractionMode, MixerControlId, SampleAssetLifecycle,
    SamplePreviewState, SemanticAction, SemanticControlId, SemanticSurfaceSummary,
    SemanticVisualizationData, SurfaceId, TopLevelContext,
};
use crate::kernel::patch_id::PatchId;
use crate::mixer::mixer_track_id::MixerTrackId;
use crate::mixer::mixer_track_parameters::MixerTrackParameter;
use crate::mixer::patch_output::{PatchOutput, PatchOutputParameter};
use crate::real_time::audio_observation_snapshot::AudioObservationSnapshot;
use crate::real_time::callback_safety::{callback_safety_snapshot, CallbackSafetySnapshot};
use crate::real_time::GraphRevision;
use crate::testing::live_demo_checkpoint::{LiveCheckpoint, LiveDemoCheckpoint};
use crate::testing::live_demo_scene::{LiveEditableParameter, LiveEngineTransition};
use crate::testing::live_mixer_routing_measurement::LiveMixerDspEvidence;
use core::fmt;
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

/// Rendered-frame coverage accepted only after projection and audio correlation.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveShellCoverage {
    context_line_visible: bool,
    identity_header_visible: bool,
    main_workspace_visible: bool,
    persistent_side_region_visible: bool,
    footer_visible: bool,
    patch_context_observed: bool,
    mixer_context_observed: bool,
    patch_main_observed: bool,
    patch_utility_observed: bool,
    mixer_main_observed: bool,
    mixer_inspector_observed: bool,
    navigate_observed: bool,
    adjust_observed: bool,
    patch_return_observed: bool,
    mixer_return_observed: bool,
    healthy_empty_errors_observed: bool,
    focus_recovery_observed: bool,
    physical_audio_nonzero: bool,
    qualifying_frames: u64,
    /// The subordinate detail surface was seen painted. WP06's scene is what
    /// drives it; the four persistent surfaces above never reach it.
    patch_detail_observed: bool,
    /// A qualifying frame whose measured viewport resolved to the authored
    /// desktop policy. The seat, not an exact height: the shipped window
    /// hands the page a viewport shorter than the display's own 1080 (F-39),
    /// and the authored policy is what "desktop" means here.
    desktop_viewport_painted: bool,
    /// The largest group count the painting page reported for the PATCH
    /// strip, and whether it ever reported a flat run. Both are transported
    /// from the page, which is grouping's only producer (F-44); `None` until
    /// a frame carried the evidence, so absent never reads as a measured 0.
    strip_groups_painted: Option<u32>,
    strip_flat_control_run: Option<bool>,
}

impl LiveShellCoverage {
    pub(crate) fn mark_qualifying_frame(&mut self, frame: &crate::shell::ShellFrameObservation) {
        self.context_line_visible = true;
        self.identity_header_visible = true;
        self.main_workspace_visible = true;
        self.persistent_side_region_visible = true;
        self.footer_visible = true;
        self.physical_audio_nonzero = true;
        match frame.context() {
            TopLevelContext::Patch => self.patch_context_observed = true,
            TopLevelContext::Mixer => self.mixer_context_observed = true,
        }
        match frame.active_surface() {
            SurfaceId::PatchMain => {
                self.patch_main_observed = true;
                if self.patch_utility_observed && frame.return_path().is_none() {
                    self.patch_return_observed = true;
                }
            }
            SurfaceId::PatchUtility => self.patch_utility_observed = true,
            SurfaceId::PatchDetail => self.patch_detail_observed = true,
            SurfaceId::Sends | SurfaceId::PatchChoice | SurfaceId::FileBrowser => {}
            SurfaceId::MixerMain => {
                self.mixer_main_observed = true;
                if self.mixer_inspector_observed && frame.return_path().is_none() {
                    self.mixer_return_observed = true;
                }
            }
            SurfaceId::MixerInspector => self.mixer_inspector_observed = true,
            SurfaceId::MidiDeviceSettings
            | SurfaceId::ControllerSettings
            | SurfaceId::SaveLoadSettings => {}
        }
        match frame.interaction_mode() {
            InteractionMode::Navigate => self.navigate_observed = true,
            InteractionMode::Adjust => self.adjust_observed = true,
            InteractionMode::Modal | InteractionMode::MultiSelect => {}
        }
        self.healthy_empty_errors_observed |= frame.errors().is_empty();
        self.desktop_viewport_painted |=
            crate::shell::density::ResponsiveLayoutMode::resolve(frame.viewport_width())
                == crate::shell::density::ResponsiveLayoutMode::Wide;
        if let Some(strip) = frame.strip() {
            self.strip_groups_painted = Some(
                self.strip_groups_painted
                    .map_or(strip.groups_painted(), |seen| {
                        seen.max(strip.groups_painted())
                    }),
            );
            self.strip_flat_control_run =
                Some(self.strip_flat_control_run.unwrap_or(false) || strip.flat_control_run());
        }
        self.qualifying_frames = self.qualifying_frames.saturating_add(1);
    }

    /// Whether the subordinate detail surface was seen painted.
    pub const fn patch_detail_observed(&self) -> bool {
        self.patch_detail_observed
    }

    /// Whether a qualifying frame painted at the authored desktop viewport.
    pub const fn desktop_viewport_painted(&self) -> bool {
        self.desktop_viewport_painted
    }

    /// The largest group count the page reported painting, or `None` when no
    /// qualifying frame carried strip evidence.
    pub const fn strip_groups_painted(&self) -> Option<u32> {
        self.strip_groups_painted
    }

    /// Whether the page ever reported painting a flat control run, or `None`
    /// when no qualifying frame carried strip evidence.
    pub const fn strip_flat_control_run(&self) -> Option<bool> {
        self.strip_flat_control_run
    }

    pub const fn context_line_visible(&self) -> bool {
        self.context_line_visible
    }

    pub const fn identity_header_visible(&self) -> bool {
        self.identity_header_visible
    }

    pub const fn main_workspace_visible(&self) -> bool {
        self.main_workspace_visible
    }

    pub const fn persistent_side_region_visible(&self) -> bool {
        self.persistent_side_region_visible
    }

    pub const fn footer_visible(&self) -> bool {
        self.footer_visible
    }

    pub const fn patch_context_observed(&self) -> bool {
        self.patch_context_observed
    }

    pub const fn mixer_context_observed(&self) -> bool {
        self.mixer_context_observed
    }

    pub const fn all_semantic_surfaces_observed(&self) -> bool {
        self.patch_main_observed
            && self.patch_utility_observed
            && self.mixer_main_observed
            && self.mixer_inspector_observed
    }

    pub const fn both_reachable_modes_observed(&self) -> bool {
        self.navigate_observed && self.adjust_observed
    }

    pub const fn both_return_round_trips_observed(&self) -> bool {
        self.patch_return_observed && self.mixer_return_observed
    }

    pub const fn healthy_empty_errors_observed(&self) -> bool {
        self.healthy_empty_errors_observed
    }

    pub const fn focus_recovery_observed(&self) -> bool {
        self.focus_recovery_observed
    }

    pub(crate) fn mark_focus_recovery(&mut self) {
        self.focus_recovery_observed = true;
    }

    pub const fn physical_audio_nonzero(&self) -> bool {
        self.physical_audio_nonzero
    }

    pub const fn mixer_inspector_observed(&self) -> bool {
        self.mixer_inspector_observed
    }

    pub const fn mixer_return_observed(&self) -> bool {
        self.mixer_return_observed
    }

    pub const fn qualifying_frames(&self) -> u64 {
        self.qualifying_frames
    }

    pub const fn is_complete(&self) -> bool {
        self.context_line_visible
            && self.identity_header_visible
            && self.main_workspace_visible
            && self.persistent_side_region_visible
            && self.footer_visible
            && self.patch_context_observed
            && self.mixer_context_observed
            && self.patch_main_observed
            && self.patch_utility_observed
            && self.mixer_main_observed
            && self.mixer_inspector_observed
            && self.navigate_observed
            && self.adjust_observed
            && self.patch_return_observed
            && self.mixer_return_observed
            && self.healthy_empty_errors_observed
            && self.focus_recovery_observed
            && self.physical_audio_nonzero
            && self.qualifying_frames >= 8
    }
}

/// Exact bidirectional coverage for the frozen editable-parameter surface.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveDemoCoverage {
    expected_editable_parameters: Vec<String>,
    exercised_editable_parameters: Vec<String>,
    missing_editable_parameters: Vec<String>,
    unexpected_editable_parameters: Vec<String>,
    duplicate_expected_parameters: Vec<String>,
    expected_engine_transitions: Vec<String>,
    exercised_engine_transitions: Vec<String>,
    missing_engine_transitions: Vec<String>,
    unexpected_engine_transitions: Vec<String>,
    duplicate_expected_engine_transitions: Vec<String>,
    #[serde(default)]
    expected_topology_transitions: Vec<String>,
    #[serde(default)]
    exercised_topology_transitions: Vec<String>,
    #[serde(default)]
    missing_topology_transitions: Vec<String>,
    #[serde(default)]
    unexpected_topology_transitions: Vec<String>,
}

/// Measured sixteen-track routing evidence retained by the live report.
///
/// Each predicate is assembled from a canonical StateTree/ParameterSnapshot
/// pair, exact semantic control identities, immutable generation-correlated
/// audio checkpoints, the lossless EventLog, and callback instrumentation.
/// Window, stream, and graph teardown remain shell-owned evidence because they
/// occur only after the report has completed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct LiveMixerRoutingEvidence {
    /// The measured canonical mixer track count; `None` when the StateTree
    /// carried no mixer track evidence (absent, never a fabricated `0`).
    track_count: Option<usize>,
    track_ids_exact: bool,
    all_tracks_projected: bool,
    empty_tracks_addressable: bool,
    /// The measured number of Patches sharing the observed reroute target;
    /// `None` when no reroute checkpoint identified a shared destination.
    shared_track_patch_count: Option<usize>,
    shared_track_sum_exact: bool,
    track_level_controls_shared_sum: bool,
    patch_trim_isolated: bool,
    patch_reroute_isolated: bool,
    invalid_route_rejected: bool,
    track_parameter_classes_exercised: usize,
    mute_wins: bool,
    any_solo_exact: bool,
    post_gate_sends_exact: bool,
    pre_gate_meters_exact: bool,
    fixed_snapshot_exact: bool,
    stable_focus_exact: bool,
    callback_allocations: usize,
    callback_destructions: usize,
}

/// Truthful result for the optional multi-select interaction in the dedicated
/// Mixer scene. The current reducer does not advertise that action, so the
/// scene reports `notImplemented` rather than fabricating selection marks.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LiveMixerMultiSelectResult {
    Exercised,
    NotImplemented,
}

/// Mixer-specific scene facts retained before physical teardown. Counts are
/// measured from the descriptor-derived expected/exercised coverage sets;
/// Inspector correlation comes from the canonical semantic projection and a
/// generation-correlated painted-frame witness.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveMixerSceneEvidence {
    focus_pairs_expected: usize,
    focus_pairs_exercised: usize,
    focus_matrix_exact: bool,
    level_edits_exact: bool,
    pan_edits_exact: bool,
    mute_edits_exact: bool,
    solo_edits_exact: bool,
    indexed_send_edits_expected: usize,
    indexed_send_edits_exercised: usize,
    inspector_painted: bool,
    inspector_return_painted: bool,
    inspector_projection_exact: bool,
    selected_track_meter_correlated: bool,
    multi_select: LiveMixerMultiSelectResult,
    multi_select_truthful: bool,
    milestone_timeout_ms: u64,
    total_timeout_ms: u64,
}

impl LiveMixerSceneEvidence {
    pub const FOCUS_PAIRS: usize = MixerTrackId::COUNT * MixerTrackParameter::MAIN.len();
    pub const INDEXED_SEND_EDITS: usize = MixerTrackId::COUNT * 2;

    pub const fn is_complete(&self) -> bool {
        self.focus_pairs_expected == Self::FOCUS_PAIRS
            && self.focus_pairs_exercised == Self::FOCUS_PAIRS
            && self.focus_matrix_exact
            && self.level_edits_exact
            && self.pan_edits_exact
            && self.mute_edits_exact
            && self.solo_edits_exact
            && self.indexed_send_edits_expected == Self::INDEXED_SEND_EDITS
            && self.indexed_send_edits_exercised == Self::INDEXED_SEND_EDITS
            && self.inspector_painted
            && self.inspector_return_painted
            && self.inspector_projection_exact
            && self.selected_track_meter_correlated
            && self.multi_select_truthful
            && self.milestone_timeout_ms > 0
            && self.total_timeout_ms > self.milestone_timeout_ms
    }

    pub const fn focus_pairs_exercised(&self) -> usize {
        self.focus_pairs_exercised
    }

    pub const fn indexed_send_edits_exercised(&self) -> usize {
        self.indexed_send_edits_exercised
    }

    pub const fn multi_select(&self) -> LiveMixerMultiSelectResult {
        self.multi_select
    }
}

impl LiveMixerRoutingEvidence {
    pub(crate) const fn with_callback_safety(
        mut self,
        callback_safety: CallbackSafetySnapshot,
    ) -> Self {
        self.callback_allocations = callback_safety.allocations();
        self.callback_destructions = callback_safety.destructions();
        self
    }

    /// The measured canonical mixer track count.
    ///
    /// # Panics
    ///
    /// Panics when the measurement is absent (the StateTree carried no mixer
    /// track evidence). Absent evidence has no numeric value and already
    /// fails [`Self::is_complete`]; gate on completeness before reading.
    pub const fn track_count(self) -> usize {
        match self.track_count {
            Some(count) => count,
            None => panic!(
                "track_count is absent: the canonical StateTree carried no mixer track evidence"
            ),
        }
    }

    pub const fn track_ids_exact(self) -> bool {
        self.track_ids_exact
    }

    pub const fn all_tracks_projected(self) -> bool {
        self.all_tracks_projected
    }

    pub const fn empty_tracks_addressable(self) -> bool {
        self.empty_tracks_addressable
    }

    /// The measured number of Patches sharing the observed reroute target.
    ///
    /// # Panics
    ///
    /// Panics when the measurement is absent (no reroute checkpoint
    /// identified a shared destination track). Absent evidence has no
    /// numeric value and already fails [`Self::is_complete`]; gate on
    /// completeness before reading.
    pub const fn shared_track_patch_count(self) -> usize {
        match self.shared_track_patch_count {
            Some(count) => count,
            None => panic!(
                "shared_track_patch_count is absent: no reroute checkpoint identified a shared destination track"
            ),
        }
    }

    pub const fn shared_track_sum_exact(self) -> bool {
        self.shared_track_sum_exact
    }

    pub const fn track_level_controls_shared_sum(self) -> bool {
        self.track_level_controls_shared_sum
    }

    pub const fn patch_trim_isolated(self) -> bool {
        self.patch_trim_isolated
    }

    pub const fn patch_reroute_isolated(self) -> bool {
        self.patch_reroute_isolated
    }

    pub const fn invalid_route_rejected(self) -> bool {
        self.invalid_route_rejected
    }

    pub const fn track_parameter_classes_exercised(self) -> usize {
        self.track_parameter_classes_exercised
    }

    pub const fn mute_wins(self) -> bool {
        self.mute_wins
    }

    pub const fn any_solo_exact(self) -> bool {
        self.any_solo_exact
    }

    pub const fn post_gate_sends_exact(self) -> bool {
        self.post_gate_sends_exact
    }

    pub const fn pre_gate_meters_exact(self) -> bool {
        self.pre_gate_meters_exact
    }

    pub const fn fixed_snapshot_exact(self) -> bool {
        self.fixed_snapshot_exact
    }

    pub const fn stable_focus_exact(self) -> bool {
        self.stable_focus_exact
    }

    pub const fn callback_allocations(self) -> usize {
        self.callback_allocations
    }

    pub const fn callback_destructions(self) -> usize {
        self.callback_destructions
    }

    pub const fn is_complete(self) -> bool {
        // Absent measurements (`None`) can never satisfy an expectation; a
        // measured value is judged on its own merits.
        let track_count_complete = match self.track_count {
            Some(count) => count == MixerTrackId::COUNT,
            None => false,
        };
        let shared_track_patch_count_complete = match self.shared_track_patch_count {
            Some(count) => count > 1,
            None => false,
        };
        track_count_complete
            && self.track_ids_exact
            && self.all_tracks_projected
            && self.empty_tracks_addressable
            && shared_track_patch_count_complete
            && self.shared_track_sum_exact
            && self.track_level_controls_shared_sum
            && self.patch_trim_isolated
            && self.patch_reroute_isolated
            && self.invalid_route_rejected
            && self.track_parameter_classes_exercised == TRACK_PARAMETER_CLASSES
            && self.mute_wins
            && self.any_solo_exact
            && self.post_gate_sends_exact
            && self.pre_gate_meters_exact
            && self.fixed_snapshot_exact
            && self.stable_focus_exact
            && self.callback_allocations == 0
            && self.callback_destructions == 0
    }
}

/// Final sixteen-track routing witness emitted after physical teardown.
///
/// Routing predicates are frozen by `LiveDemoReport`; the remaining fields
/// are attached only after the native window returns, the stream is dropped,
/// the preparation worker is shut down, and graph ownership is collected.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct SixteenTrackMixerRoutingObservation {
    #[serde(flatten)]
    routing: LiveMixerRoutingEvidence,
    physical_audio_nonzero: bool,
    active_notes_after_cleanup: u32,
    window_closed: bool,
    stream_released: bool,
    owned_graphs_remaining: usize,
}

impl SixteenTrackMixerRoutingObservation {
    pub const fn new(
        routing: LiveMixerRoutingEvidence,
        physical_audio_nonzero: bool,
        active_notes_after_cleanup: u32,
        window_closed: bool,
        stream_released: bool,
        owned_graphs_remaining: usize,
    ) -> Self {
        Self {
            routing,
            physical_audio_nonzero,
            active_notes_after_cleanup,
            window_closed,
            stream_released,
            owned_graphs_remaining,
        }
    }

    pub const fn is_complete(self) -> bool {
        self.routing.is_complete()
            && self.physical_audio_nonzero
            && self.active_notes_after_cleanup == 0
            && self.window_closed
            && self.stream_released
            && self.owned_graphs_remaining == 0
    }
}

impl LiveDemoCoverage {
    pub fn new(expected: &[LiveEditableParameter]) -> Self {
        Self::with_engine_transitions(expected, &[])
    }

    pub fn with_engine_transitions(
        expected: &[LiveEditableParameter],
        engine_transitions: &[LiveEngineTransition],
    ) -> Self {
        Self::with_engine_and_topology_transitions(expected, engine_transitions, &[])
    }

    pub fn with_engine_and_topology_transitions(
        expected: &[LiveEditableParameter],
        engine_transitions: &[LiveEngineTransition],
        topology_transitions: &[crate::testing::live_effects_and_buses_scene::LiveTopologyTransition],
    ) -> Self {
        let mut expected_ids = Vec::with_capacity(expected.len());
        let mut duplicate_expected = Vec::new();
        for parameter in expected {
            let identifier = parameter.identifier();
            if expected_ids.contains(&identifier) {
                insert_sorted_unique(&mut duplicate_expected, identifier);
            } else {
                expected_ids.push(identifier);
            }
        }

        let mut expected_engines = Vec::with_capacity(engine_transitions.len());
        let mut duplicate_expected_engines = Vec::new();
        for transition in engine_transitions {
            let identifier = transition.identifier().to_owned();
            if expected_engines.contains(&identifier) {
                insert_sorted_unique(&mut duplicate_expected_engines, identifier);
            } else {
                expected_engines.push(identifier);
            }
        }

        let mut expected_topology = Vec::with_capacity(topology_transitions.len());
        for transition in topology_transitions {
            let identifier = transition.identifier().to_owned();
            if !expected_topology.contains(&identifier) {
                expected_topology.push(identifier);
            }
        }

        Self {
            missing_editable_parameters: expected_ids.clone(),
            expected_editable_parameters: expected_ids,
            duplicate_expected_parameters: duplicate_expected,
            missing_engine_transitions: expected_engines.clone(),
            expected_engine_transitions: expected_engines,
            duplicate_expected_engine_transitions: duplicate_expected_engines,
            missing_topology_transitions: expected_topology.clone(),
            expected_topology_transitions: expected_topology,
            ..Self::default()
        }
    }

    pub fn mark_exercised(&mut self, parameter: &LiveEditableParameter) {
        let identifier = parameter.identifier();
        if !insert_sorted_unique(&mut self.exercised_editable_parameters, identifier.clone()) {
            return;
        }
        if let Some(index) = self
            .missing_editable_parameters
            .iter()
            .position(|expected| expected == &identifier)
        {
            self.missing_editable_parameters.remove(index);
        } else {
            insert_sorted_unique(&mut self.unexpected_editable_parameters, identifier);
        }
    }

    pub fn mark_unexpected(&mut self, identifier: impl Into<String>) {
        let identifier = identifier.into();
        insert_sorted_unique(&mut self.exercised_editable_parameters, identifier.clone());
        insert_sorted_unique(&mut self.unexpected_editable_parameters, identifier);
    }

    pub fn mark_engine_exercised(&mut self, transition: &LiveEngineTransition) {
        let identifier = transition.identifier().to_owned();
        if !insert_sorted_unique(&mut self.exercised_engine_transitions, identifier.clone()) {
            return;
        }
        if let Some(index) = self
            .missing_engine_transitions
            .iter()
            .position(|expected| expected == &identifier)
        {
            self.missing_engine_transitions.remove(index);
        } else {
            insert_sorted_unique(&mut self.unexpected_engine_transitions, identifier);
        }
    }

    pub fn expected(&self) -> &[String] {
        &self.expected_editable_parameters
    }

    pub fn exercised(&self) -> &[String] {
        &self.exercised_editable_parameters
    }

    pub fn missing(&self) -> &[String] {
        &self.missing_editable_parameters
    }

    pub fn unexpected(&self) -> &[String] {
        &self.unexpected_editable_parameters
    }

    pub fn duplicate_expected(&self) -> &[String] {
        &self.duplicate_expected_parameters
    }

    pub fn mark_topology_exercised(&mut self, identifier: &str) {
        let identifier = identifier.to_owned();
        if !insert_sorted_unique(&mut self.exercised_topology_transitions, identifier.clone()) {
            return;
        }
        if let Some(index) = self
            .missing_topology_transitions
            .iter()
            .position(|expected| expected == &identifier)
        {
            self.missing_topology_transitions.remove(index);
        } else {
            insert_sorted_unique(&mut self.unexpected_topology_transitions, identifier);
        }
    }

    pub fn expected_topology_transitions(&self) -> &[String] {
        &self.expected_topology_transitions
    }

    pub fn exercised_topology_transitions(&self) -> &[String] {
        &self.exercised_topology_transitions
    }

    pub fn missing_topology_transitions(&self) -> &[String] {
        &self.missing_topology_transitions
    }

    pub fn unexpected_topology_transitions(&self) -> &[String] {
        &self.unexpected_topology_transitions
    }

    pub fn expected_engine_transitions(&self) -> &[String] {
        &self.expected_engine_transitions
    }

    pub fn exercised_engine_transitions(&self) -> &[String] {
        &self.exercised_engine_transitions
    }

    pub fn missing_engine_transitions(&self) -> &[String] {
        &self.missing_engine_transitions
    }

    pub fn unexpected_engine_transitions(&self) -> &[String] {
        &self.unexpected_engine_transitions
    }

    pub fn is_complete(&self) -> bool {
        self.missing_editable_parameters.is_empty()
            && self.unexpected_editable_parameters.is_empty()
            && self.duplicate_expected_parameters.is_empty()
            && self.exercised_editable_parameters.len() == self.expected_editable_parameters.len()
            && self.missing_engine_transitions.is_empty()
            && self.unexpected_engine_transitions.is_empty()
            && self.duplicate_expected_engine_transitions.is_empty()
            && self.exercised_engine_transitions.len() == self.expected_engine_transitions.len()
            && self.missing_topology_transitions.is_empty()
            && self.unexpected_topology_transitions.is_empty()
            && self.exercised_topology_transitions.len() == self.expected_topology_transitions.len()
    }
}

fn insert_sorted_unique(values: &mut Vec<String>, value: String) -> bool {
    match values.binary_search(&value) {
        Ok(_) => false,
        Err(index) => {
            values.insert(index, value);
            true
        }
    }
}

/// Compact control-side evidence for the prepared runtime used by a live run.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAudioWitness {
    prepared_shared_assets: usize,
    prepared_instruments: usize,
    engine_managed_patches: usize,
    fixed_per_patch_patches: usize,
    adjacent_capabilities_distinct: bool,
    initial_graph_revision: GraphRevision,
    active_graph_revision: GraphRevision,
    engine_switches: usize,
    fallbacks: usize,
    callback_allocations: usize,
    callback_destructions: usize,
    #[serde(skip)]
    callback_safety_source: CallbackSafetySource,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum CallbackSafetySource {
    #[default]
    Fixed,
    Process,
}

impl RuntimeAudioWitness {
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        prepared_shared_assets: usize,
        prepared_instruments: usize,
        engine_managed_patches: usize,
        fixed_per_patch_patches: usize,
        adjacent_capabilities_distinct: bool,
        active_graph_revision: GraphRevision,
        callback_allocations: usize,
        callback_destructions: usize,
    ) -> Self {
        Self {
            prepared_shared_assets,
            prepared_instruments,
            engine_managed_patches,
            fixed_per_patch_patches,
            adjacent_capabilities_distinct,
            initial_graph_revision: active_graph_revision,
            active_graph_revision,
            engine_switches: 0,
            fallbacks: 0,
            callback_allocations,
            callback_destructions,
            callback_safety_source: CallbackSafetySource::Fixed,
        }
    }

    pub(crate) const fn with_process_callback_safety(mut self) -> Self {
        self.callback_safety_source = CallbackSafetySource::Process;
        self
    }

    pub(crate) fn measured(mut self) -> Self {
        if self.callback_safety_source == CallbackSafetySource::Process {
            let callback_safety = callback_safety_snapshot();
            self.callback_allocations = callback_safety.allocations();
            self.callback_destructions = callback_safety.destructions();
            self.callback_safety_source = CallbackSafetySource::Fixed;
        }
        self
    }

    pub const fn prepared_shared_assets(self) -> usize {
        self.prepared_shared_assets
    }

    pub const fn prepared_instruments(self) -> usize {
        self.prepared_instruments
    }

    pub const fn engine_managed_patches(self) -> usize {
        self.engine_managed_patches
    }

    pub const fn fixed_per_patch_patches(self) -> usize {
        self.fixed_per_patch_patches
    }

    pub const fn adjacent_capabilities_distinct(self) -> bool {
        self.adjacent_capabilities_distinct
    }

    pub const fn active_graph_revision(self) -> GraphRevision {
        self.active_graph_revision
    }

    pub const fn initial_graph_revision(self) -> GraphRevision {
        self.initial_graph_revision
    }

    pub const fn engine_switches(self) -> usize {
        self.engine_switches
    }

    pub const fn fallbacks(self) -> usize {
        self.fallbacks
    }

    pub const fn callback_destructions(self) -> usize {
        self.callback_destructions
    }

    pub const fn callback_allocations(self) -> usize {
        self.callback_allocations
    }

    pub const fn with_active_graph_revision(mut self, revision: GraphRevision) -> Self {
        self.active_graph_revision = revision;
        self
    }

    pub fn record_ready_capability(
        &mut self,
        capability_id: &crate::synth::CapabilityId,
        revision: GraphRevision,
    ) -> bool {
        if capability_id.as_str().is_empty() || self.engine_switches >= 3 {
            return false;
        }
        self.engine_switches = self.engine_switches.saturating_add(1);
        self.active_graph_revision = revision;
        true
    }
}

const LIVE_DETAIL_ASSET_CHECKPOINT_CAPACITY: usize = 128;

/// One bounded Phase 7 correlation sample. It joins reducer-owned focus and
/// asset lifecycle, the visible semantic focus marker, the active graph, and
/// callback energy from the same control-loop observation point.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveDetailAssetsCheckpoint {
    generation: u64,
    graph_revision: GraphRevision,
    focus: crate::control::FocusPath,
    visible_focus_matches: bool,
    active_asset: Option<String>,
    requested_asset: Option<String>,
    lifecycle: SampleAssetLifecycle,
    preview: SamplePreviewState,
    preview_revision_compatible: bool,
    primary_patch_id: Option<PatchId>,
    primary_patch_rms: f32,
    origin_track_rms: f32,
    bus_input_rms: [f32; 2],
    output_rms: f32,
}

impl LiveDetailAssetsCheckpoint {
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub const fn graph_revision(&self) -> GraphRevision {
        self.graph_revision
    }

    pub const fn focus(&self) -> &crate::control::FocusPath {
        &self.focus
    }

    pub const fn visible_focus_matches(&self) -> bool {
        self.visible_focus_matches
    }

    pub const fn lifecycle(&self) -> SampleAssetLifecycle {
        self.lifecycle
    }

    pub const fn preview_revision_compatible(&self) -> bool {
        self.preview_revision_compatible
    }

    pub const fn primary_patch_id(&self) -> Option<PatchId> {
        self.primary_patch_id
    }

    pub const fn primary_patch_rms(&self) -> f32 {
        self.primary_patch_rms
    }

    pub const fn origin_track_rms(&self) -> f32 {
        self.origin_track_rms
    }

    pub const fn bus_input_rms(&self) -> [f32; 2] {
        self.bus_input_rms
    }
}

/// Cumulative Phase 7 evidence sampled from canonical state, the production
/// shell projection, and revision-compatible callback observations.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveDetailAssetsEvidence {
    patch_id: PatchId,
    track_id: MixerTrackId,
    initial_asset: String,
    final_asset: Option<String>,
    active_requested_distinction_observed: bool,
    cancelled_observed: bool,
    invalid_observed: bool,
    ready_after_asset_change: bool,
    preview_revision_compatible: bool,
    preview_track_rms_peak: f32,
    preview_release_observed: bool,
    committed_sample_track_rms_peak: f32,
    waveform_correlated: bool,
    exact_return_observed: bool,
    checkpoints: Vec<LiveDetailAssetsCheckpoint>,
    #[serde(skip)]
    pending_return_origin: Option<crate::control::FocusPath>,
}

impl LiveDetailAssetsEvidence {
    pub(crate) fn new(
        patch_id: PatchId,
        track_id: MixerTrackId,
        initial_asset: impl Into<String>,
    ) -> Self {
        Self {
            patch_id,
            track_id,
            initial_asset: initial_asset.into(),
            final_asset: None,
            active_requested_distinction_observed: false,
            cancelled_observed: false,
            invalid_observed: false,
            ready_after_asset_change: false,
            preview_revision_compatible: false,
            preview_track_rms_peak: 0.0,
            preview_release_observed: false,
            committed_sample_track_rms_peak: 0.0,
            waveform_correlated: false,
            exact_return_observed: false,
            checkpoints: Vec::new(),
            pending_return_origin: None,
        }
    }

    pub(crate) fn observe(
        &mut self,
        state: &crate::control::AppState,
        shell: &GraphicalShellProjection,
        audio: AudioObservationSnapshot,
    ) {
        let browser = state.file_browser();
        self.cancelled_observed |= browser.lifecycle() == SampleAssetLifecycle::Cancelled;
        self.invalid_observed |= browser.lifecycle() == SampleAssetLifecycle::Invalid;

        let active_asset = state
            .patches()
            .iter()
            .find(|patch| patch.id() == self.patch_id)
            .and_then(|patch| {
                patch
                    .instrument_config()
                    .asset_references()
                    .iter()
                    .find(|assignment| {
                        assignment.reference().kind() == crate::synth::AssetKind::Sample
                    })
            })
            .map(|assignment| assignment.reference().locator().to_owned());
        if state.interaction().active_surface() == SurfaceId::FileBrowser {
            self.pending_return_origin = state
                .interaction()
                .return_path()
                .map(|path| path.origin().clone());
        } else if self.pending_return_origin.as_ref().is_some_and(|origin| {
            state.interaction().focus_path() == origin
                && shell.semantic_model().focus_path() == origin
        }) {
            self.exact_return_observed = true;
            self.pending_return_origin = None;
        }
        self.final_asset = active_asset.clone();
        self.active_requested_distinction_observed |=
            browser.requested_asset().is_some_and(|requested| {
                active_asset.as_deref() == Some(self.initial_asset.as_str())
                    && requested.as_str() != self.initial_asset
            });
        self.ready_after_asset_change |= browser.lifecycle() == SampleAssetLifecycle::Ready
            && active_asset
                .as_deref()
                .is_some_and(|asset| asset != self.initial_asset);
        if browser.lifecycle() == SampleAssetLifecycle::Ready
            && active_asset
                .as_deref()
                .is_some_and(|asset| asset != self.initial_asset)
            && audio.active_graph_revision() == shell.semantic_model().status().graph_revision()
        {
            self.committed_sample_track_rms_peak = self
                .committed_sample_track_rms_peak
                .max(audio.track(self.track_id).rms());
        }

        let mut compatible_preview = false;
        if matches!(browser.preview(), SamplePreviewState::Playing { .. }) {
            if let Some(request_id) = browser.preview_request_id() {
                if audio
                    .compatible_preview(
                        shell.generation(),
                        shell.semantic_model().status().graph_revision(),
                        request_id.value(),
                        self.patch_id,
                    )
                    .is_some_and(|preview| preview.playing())
                {
                    compatible_preview = true;
                    self.preview_revision_compatible = true;
                    self.preview_track_rms_peak = self
                        .preview_track_rms_peak
                        .max(audio.track(self.track_id).rms());
                }
            }
        }
        self.preview_release_observed |= self.preview_revision_compatible
            && matches!(browser.preview(), SamplePreviewState::Idle)
            && !audio.preview_playing();

        self.waveform_correlated |= shell
            .semantic_model()
            .surface(SurfaceId::PatchDetail)
            .into_iter()
            .flat_map(|surface| surface.visualizations())
            .any(|visualization| {
                matches!(
                    visualization.data(),
                    SemanticVisualizationData::Waveform {
                        asset: Some(asset),
                        pairs,
                        status,
                        ..
                    } if active_asset.as_deref() == Some(asset.locator())
                        && !pairs.is_empty()
                        && status == "READY"
                )
            });

        let phase7_surface = matches!(
            state.interaction().active_surface(),
            SurfaceId::PatchDetail | SurfaceId::PatchChoice | SurfaceId::FileBrowser
        );
        if phase7_surface && self.checkpoints.len() < LIVE_DETAIL_ASSET_CHECKPOINT_CAPACITY {
            let checkpoint = LiveDetailAssetsCheckpoint {
                generation: state.generation(),
                graph_revision: audio.active_graph_revision(),
                focus: state.interaction().focus_path().clone(),
                visible_focus_matches: shell.semantic_model().focus_path()
                    == state.interaction().focus_path(),
                active_asset,
                requested_asset: browser
                    .requested_asset()
                    .map(|asset| asset.as_str().to_owned()),
                lifecycle: browser.lifecycle(),
                preview: browser.preview().clone(),
                preview_revision_compatible: compatible_preview,
                primary_patch_id: audio.primary_patch_id(),
                primary_patch_rms: audio.primary_patch_rms(),
                origin_track_rms: audio.track(self.track_id).rms(),
                bus_input_rms: [audio.reverb_input_rms(), audio.delay_input_rms()],
                output_rms: audio.output_rms(),
            };
            let meaningfully_changed = self.checkpoints.last().is_none_or(|previous| {
                previous.graph_revision != checkpoint.graph_revision
                    || previous.focus != checkpoint.focus
                    || previous.active_asset != checkpoint.active_asset
                    || previous.requested_asset != checkpoint.requested_asset
                    || previous.lifecycle != checkpoint.lifecycle
                    || previous.preview != checkpoint.preview
                    || previous.preview_revision_compatible
                        != checkpoint.preview_revision_compatible
            });
            if meaningfully_changed {
                self.checkpoints.push(checkpoint);
            }
        }
    }

    pub fn checkpoints(&self) -> &[LiveDetailAssetsCheckpoint] {
        &self.checkpoints
    }

    pub const fn exact_return_observed(&self) -> bool {
        self.exact_return_observed
    }

    pub const fn preview_revision_compatible(&self) -> bool {
        self.preview_revision_compatible
    }

    pub const fn preview_release_observed(&self) -> bool {
        self.preview_release_observed
    }

    pub const fn waveform_correlated(&self) -> bool {
        self.waveform_correlated
    }

    pub fn is_complete(&self) -> bool {
        self.active_requested_distinction_observed
            && self.cancelled_observed
            && self.invalid_observed
            && self.ready_after_asset_change
            && self.preview_revision_compatible
            && self.preview_track_rms_peak > 1.0e-5
            && self.preview_release_observed
            && self.committed_sample_track_rms_peak > 1.0e-5
            && self.waveform_correlated
            && self.exact_return_observed
            && !self.checkpoints.is_empty()
            && self
                .checkpoints
                .iter()
                .all(LiveDetailAssetsCheckpoint::visible_focus_matches)
            && self
                .final_asset
                .as_deref()
                .is_some_and(|asset| asset != self.initial_asset)
    }
}

/// The complete structured result of one live observable demo.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveDemoReport {
    scene: String,
    complete: bool,
    checkpoints: Vec<LiveCheckpoint>,
    event_log: EventLog,
    state_tree: StateTree,
    graphical_shell: GraphicalShellProjection,
    coverage: LiveDemoCoverage,
    shell_coverage: LiveShellCoverage,
    final_audio_observation: AudioObservationSnapshot,
    runtime_audio: RuntimeAudioWitness,
    mixer_routing: LiveMixerRoutingEvidence,
    effects_and_buses: Option<LiveEffectsAndBusesEvidence>,
    /// The functional Patch editor measurement, present only for the scene
    /// that declares it. Not serialized: the host resolves it into the
    /// emitted observation after teardown, when the facts only the host knows
    /// are available.
    patch_editor: Option<crate::testing::PatchEditorMeasurement>,
    detail_and_assets: Option<LiveDetailAssetsEvidence>,
    summary: String,
}

/// Measured effects-and-buses evidence retained by the cumulative live
/// report: per-transition topology lifecycle coverage, NFR-008 edit
/// responsiveness, and the numeric eight-destination routing measurements
/// (SC-004, SC-005, NFR-007).
///
/// Every derived measurement distinguishes absent evidence from a measured
/// zero: a summary computed over an empty observation set is `None`,
/// serializes as `null` under its retained key name, and can never satisfy
/// a presence or performance expectation.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct LiveEffectsAndBusesEvidence {
    topology_checkpoints: usize,
    slot_fills_exercised: usize,
    startup_occupant_cleared: bool,
    slot_order_exchange_exercised: bool,
    same_entry_twin_exercised: bool,
    sends_raised_toward_destinations: bool,
    return_content_changes: usize,
    empty_return_occupied_and_restored: bool,
    controlled_rejection_observed: bool,
    /// The attributable reason of the observed controlled rejection; `None`
    /// when no rejected transition was observed (absent evidence, never an
    /// empty placeholder).
    rejection_reason: Option<String>,
    post_rejection_recovery_observed: bool,
    reroute_chain_followed: bool,
    /// NFR-008: worst measured acceptance-to-projection window (frames);
    /// `None` when no observed transition carried the measurement.
    frames_to_projection_max: Option<u64>,
    /// NFR-008: worst observed activation-sequence gap (render blocks);
    /// exactly 1 when the observer sees every block. `None` when no observed
    /// transition carried the measurement.
    activation_sequence_gap_max: Option<u64>,
    /// NFR-008: worst observed distance from the target-note dispatch on an
    /// activated graph to its first audible block; exactly 1 when the
    /// observer sees every block. `None` when no observed transition carried
    /// the measurement.
    render_blocks_to_audible_max: Option<u64>,
    all_audible_on_activated_graph: bool,
    send_isolation_exact: bool,
    max_off_target_bus_dbfs: f32,
    accumulation_exact: bool,
    gated_wet_contribution: f32,
    unoccupied_return_silent: bool,
}

impl LiveEffectsAndBusesEvidence {
    pub const fn topology_checkpoints(&self) -> usize {
        self.topology_checkpoints
    }

    /// The measured worst acceptance-to-projection window (frames).
    ///
    /// # Panics
    ///
    /// Panics when the measurement is absent (no observed transition carried
    /// it). Absent evidence has no numeric value and already fails
    /// [`Self::is_complete`]; gate on completeness before reading.
    pub const fn frames_to_projection_max(&self) -> u64 {
        match self.frames_to_projection_max {
            Some(frames) => frames,
            None => panic!(
                "frames_to_projection_max is absent: no observed topology transition carried the measurement"
            ),
        }
    }

    /// The measured worst activation-sequence gap (render blocks).
    ///
    /// # Panics
    ///
    /// Panics when the measurement is absent (no observed transition carried
    /// it). Absent evidence has no numeric value and already fails
    /// [`Self::is_complete`]; gate on completeness before reading.
    pub const fn activation_sequence_gap_max(&self) -> u64 {
        match self.activation_sequence_gap_max {
            Some(gap) => gap,
            None => panic!(
                "activation_sequence_gap_max is absent: no observed topology transition carried the measurement"
            ),
        }
    }

    /// The measured worst distance to the first audible block.
    ///
    /// # Panics
    ///
    /// Panics when the measurement is absent (no observed transition carried
    /// it). Absent evidence has no numeric value and already fails
    /// [`Self::is_complete`]; gate on completeness before reading.
    pub const fn render_blocks_to_audible_max(&self) -> u64 {
        match self.render_blocks_to_audible_max {
            Some(blocks) => blocks,
            None => panic!(
                "render_blocks_to_audible_max is absent: no observed topology transition carried the measurement"
            ),
        }
    }

    pub const fn max_off_target_bus_dbfs(&self) -> f32 {
        self.max_off_target_bus_dbfs
    }

    pub const fn gated_wet_contribution(&self) -> f32 {
        self.gated_wet_contribution
    }

    pub fn is_complete(&self) -> bool {
        self.topology_checkpoints > 0
            && self.slot_fills_exercised >= 3
            && self.startup_occupant_cleared
            && self.slot_order_exchange_exercised
            && self.same_entry_twin_exercised
            && self.sends_raised_toward_destinations
            && self.return_content_changes >= 2
            && self.empty_return_occupied_and_restored
            && self.controlled_rejection_observed
            && self
                .rejection_reason
                .as_deref()
                .is_some_and(|reason| !reason.is_empty())
            && self.post_rejection_recovery_observed
            && self.reroute_chain_followed
            && self
                .frames_to_projection_max
                .is_some_and(|frames| frames <= 1)
            && self.activation_sequence_gap_max.is_some_and(|gap| gap >= 1)
            && self
                .render_blocks_to_audible_max
                .is_some_and(|blocks| blocks >= 1)
            && self.all_audible_on_activated_graph
            && self.send_isolation_exact
            && self.max_off_target_bus_dbfs
                < crate::testing::live_mixer_routing_measurement::ISOLATION_FLOOR_DBFS
            && self.accumulation_exact
            && self.gated_wet_contribution == 0.0
            && self.unoccupied_return_silent
    }
}

/// Whether this scene is the one that declares the effects-and-buses phase, and
/// so the one whose topology checkpoints are graded against the eight-destination
/// bus contract (mission finding F-49).
///
/// The gate is load-bearing in both directions and is pinned as such, because a
/// regression in it is silent rather than loud:
///
/// - Narrowed (a renamed or never-matching literal), the effects-and-buses
///   evidence block vanishes from that scene's report *and takes its `complete`
///   requirement with it* — the report still says complete, having stopped
///   asking.
/// - Widened (any always-true rule), every other scene that reuses the topology
///   machinery for its own journey — the functional Patch editor's effect-slot
///   occupancy walk above all — is graded against a bus contract it never
///   claimed to meet, and reports a shortfall in something it does not do.
fn scene_declares_the_effects_and_buses_phase(scene: &str) -> bool {
    scene == crate::testing::EFFECTS_AND_BUSES_SCENE_NAME
}

fn measure_effects_and_buses(
    checkpoints: &[LiveCheckpoint],
    dsp: &LiveMixerDspEvidence,
) -> Option<LiveEffectsAndBusesEvidence> {
    let topology: Vec<_> = checkpoints
        .iter()
        .filter_map(LiveCheckpoint::as_topology)
        .collect();
    if topology.is_empty() {
        return None;
    }
    let by_id = |prefix: &str| {
        topology
            .iter()
            .filter(|checkpoint| checkpoint.transition().starts_with(prefix))
            .count()
    };
    let complete_by_id = |id: &str| {
        topology
            .iter()
            .any(|checkpoint| checkpoint.transition() == id && checkpoint.agrees())
    };
    let rejection = topology
        .iter()
        .find(|checkpoint| checkpoint.outcome() == EventOutcome::Rejected);
    let rejection_index = topology
        .iter()
        .position(|checkpoint| checkpoint.outcome() == EventOutcome::Rejected);
    let post_rejection_recovery_observed = rejection_index.is_some_and(|index| {
        topology.iter().skip(index + 1).any(|checkpoint| {
            checkpoint.outcome() == EventOutcome::Accepted
                && checkpoint.target_graph_revision().is_some()
                && checkpoint.agrees()
        })
    });
    // Absent-vs-zero: each derived measurement stays `None` when no observed
    // transition carried the underlying observation — an empty observation
    // set must never fabricate the strongest possible pass (a measured `0`).
    let frames_to_projection_max = topology
        .iter()
        .filter_map(|checkpoint| checkpoint.frames_to_projection())
        .max();
    let activation_sequence_gap_max = topology
        .iter()
        .filter_map(|checkpoint| checkpoint.observed_activation_sequence_gap())
        .max();
    let render_blocks_to_audible_max = topology
        .iter()
        .filter_map(|checkpoint| checkpoint.render_blocks_to_audible())
        .max();
    Some(LiveEffectsAndBusesEvidence {
        topology_checkpoints: topology.len(),
        slot_fills_exercised: by_id("SlotFill."),
        startup_occupant_cleared: complete_by_id("Slot.startupOccupantCleared"),
        slot_order_exchange_exercised: complete_by_id("SlotOrder.exchangeFirst")
            && complete_by_id("SlotOrder.exchangeSecond"),
        same_entry_twin_exercised: complete_by_id("SlotTwin.sameEntry"),
        sends_raised_toward_destinations: complete_by_id("Send.towardDestinations"),
        return_content_changes: by_id("Return.contentChanged")
            + by_id("Topology.recoveredAfterRefusal"),
        empty_return_occupied_and_restored: complete_by_id("Return.emptyOccupied")
            && complete_by_id("Return.emptyRestored"),
        controlled_rejection_observed: rejection.is_some_and(|checkpoint| checkpoint.agrees()),
        rejection_reason: rejection
            .and_then(|checkpoint| checkpoint.rejection())
            .map(str::to_owned),
        post_rejection_recovery_observed,
        reroute_chain_followed: complete_by_id("Reroute.chainFollows"),
        frames_to_projection_max,
        activation_sequence_gap_max,
        render_blocks_to_audible_max,
        all_audible_on_activated_graph: topology
            .iter()
            .all(|checkpoint| checkpoint.audible_on_activated_graph()),
        send_isolation_exact: dsp.send_isolation_exact,
        max_off_target_bus_dbfs: dsp.max_off_target_bus_dbfs,
        accumulation_exact: dsp.accumulation_exact,
        gated_wet_contribution: dsp.gated_wet_contribution,
        unoccupied_return_silent: dsp.unoccupied_return_silent,
    })
}

/// Compact terminal evidence for a potentially large retained live journal.
///
/// The complete EventLog remains available on LiveDemoReport and is exercised
/// by deterministic verification; interactive output reports its lossless
/// bounds and canonical endpoints without printing every performance event.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveEventLogSummary {
    schema_version: u32,
    event_log_schema_version: u32,
    total_observed: u64,
    retained_records: usize,
    dropped_records: u64,
    first_sequence: Option<u64>,
    last_sequence: Option<u64>,
    generation_before: Option<u64>,
    generation_after: Option<u64>,
    state_hash_before: Option<String>,
    state_hash_after: Option<String>,
    active_graph_revision: GraphRevision,
    lossless: bool,
}

impl LiveEventLogSummary {
    pub const SCHEMA_VERSION: u32 = 3;

    fn from_event_log(event_log: &EventLog, active_graph_revision: GraphRevision) -> Self {
        let first = event_log.records().front();
        let last = event_log.records().back();
        Self {
            schema_version: Self::SCHEMA_VERSION,
            event_log_schema_version: event_log.schema_version(),
            total_observed: event_log.total_observed(),
            retained_records: event_log.records().len(),
            dropped_records: event_log.dropped_records(),
            first_sequence: first.map(|record| record.sequence()),
            last_sequence: last.map(|record| record.sequence()),
            generation_before: first.map(|record| record.generation_before()),
            generation_after: last.map(|record| record.generation_after()),
            state_hash_before: first.map(|record| record.state_hash_before().to_owned()),
            state_hash_after: last.map(|record| record.state_hash_after().to_owned()),
            active_graph_revision,
            lossless: event_log.dropped_records() == 0
                && event_log.total_observed() == event_log.records().len() as u64,
        }
    }
}

impl LiveDemoReport {
    pub const SCHEMA_VERSION: u32 = 10;

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        scene: impl Into<String>,
        checkpoints: Vec<LiveCheckpoint>,
        event_log: EventLog,
        state_tree: StateTree,
        graphical_shell: GraphicalShellProjection,
        coverage: LiveDemoCoverage,
        shell_coverage: LiveShellCoverage,
        installed_patches: &[PatchId],
        cleanup_sequence_before: u64,
        final_observation: AudioObservationSnapshot,
        runtime_audio: RuntimeAudioWitness,
        patch_editor: Option<crate::testing::PatchEditorMeasurement>,
        detail_and_assets: Option<LiveDetailAssetsEvidence>,
    ) -> Result<Self, LiveDemoReportError> {
        let scene = scene.into();
        if scene.trim().is_empty() {
            return Err(LiveDemoReportError::EmptyScene);
        }
        let endpoint = event_log
            .records()
            .back()
            .ok_or(LiveDemoReportError::EmptyEventLog)?;
        if endpoint.generation_after() != state_tree.generation()
            || endpoint.parameter_generation() != state_tree.generation()
            || endpoint.state_hash_after() != state_tree.state_hash()
            || endpoint.projection_state_hash() != state_tree.state_hash()
            || endpoint.selected_line() != state_tree.selected_line()
        {
            return Err(LiveDemoReportError::FinalStateMismatch);
        }
        let tree_value: serde_json::Value = serde_json::from_str(state_tree.json())
            .map_err(|_| LiveDemoReportError::FinalShellMismatch)?;
        // Compare the two documents after the same JSON text round trip.
        // `serde_json::to_value` retains an f32 as an exact f64 decimal while
        // StateTree stores the canonical serialized JSON spelling; waveform
        // pairs exposed that representation-only difference even though both
        // projections came from the same f32 values.
        let shell_json = serde_json::to_string(&graphical_shell)
            .map_err(|_| LiveDemoReportError::FinalShellMismatch)?;
        let shell_value: serde_json::Value = serde_json::from_str(&shell_json)
            .map_err(|_| LiveDemoReportError::FinalShellMismatch)?;
        if graphical_shell.generation() != state_tree.generation()
            || graphical_shell.state_hash() != state_tree.state_hash()
            || tree_value.get("graphicalShell") != Some(&shell_value)
        {
            return Err(LiveDemoReportError::FinalShellMismatch);
        }

        let accepted = event_log
            .records()
            .iter()
            .any(|record| record.outcome() == EventOutcome::Accepted);
        let rejected = event_log
            .records()
            .iter()
            .any(|record| record.outcome() == EventOutcome::Rejected);
        let cleanup_complete = installed_patches.iter().all(|expected_patch| {
            event_log.records().iter().any(|record| {
                record.source() == EventSource::DemoScene
                    && record.outcome() == EventOutcome::Accepted
                    && matches!(
                        record.input(),
                        EventInput::Midi { patch_id, message }
                            if *patch_id == expected_patch.value()
                                && message.kind() == MidiKind::AllNotesOff
                    )
            })
        });
        let final_audio_complete = final_observation.sequence() > cleanup_sequence_before
            && final_observation.parameter_generation() == state_tree.generation()
            && final_observation.active_notes() == 0
            && final_observation.output_rms().is_finite()
            && final_observation.non_finite_samples() == 0;
        let checkpoints_agree = !checkpoints.is_empty()
            && checkpoints.iter().all(LiveCheckpoint::agrees)
            && engine_checkpoint_sequence_is_complete(&checkpoints, &coverage)
            && patch_control_checkpoint_sequence_is_complete(
                &checkpoints,
                installed_patches,
                &state_tree,
            );
        let runtime_complete = runtime_audio.prepared_shared_assets() > 0
            && runtime_audio.prepared_instruments() == installed_patches.len()
            && runtime_composition_matches(&state_tree, runtime_audio)
            && runtime_audio.initial_graph_revision() < runtime_audio.active_graph_revision()
            && runtime_audio.active_graph_revision() == state_tree.graph_revision()
            && runtime_audio.engine_switches() == 3
            && runtime_audio.fallbacks() == 0
            && runtime_audio.callback_allocations() == 0
            && runtime_audio.callback_destructions() == 0;
        let final_engine_complete = final_soundfont_config_is_default(&state_tree);
        let mixer_routing = measure_mixer_routing(
            &checkpoints,
            &event_log,
            &state_tree,
            &graphical_shell,
            runtime_audio,
        );
        let effects_and_buses = scene_declares_the_effects_and_buses_phase(&scene)
            .then(|| measure_effects_and_buses(&checkpoints, &LiveMixerDspEvidence::measure()))
            .flatten();
        let lossless = event_log.dropped_records() == 0
            && event_log.total_observed() == event_log.records().len() as u64;
        let complete = coverage.is_complete()
            && shell_coverage.is_complete()
            && accepted
            && rejected
            && checkpoints_agree
            && lossless
            && cleanup_complete
            && final_audio_complete
            && runtime_complete
            && final_engine_complete
            && mixer_routing.is_complete()
            && effects_and_buses
                .as_ref()
                .is_none_or(LiveEffectsAndBusesEvidence::is_complete)
            && detail_and_assets
                .as_ref()
                .is_none_or(LiveDetailAssetsEvidence::is_complete);
        let summary = format!(
            "live demo {}: {}/{} editable parameters, {}/{} engine transitions, {} qualifying shell frames, {} checkpoints, {} events, {} dropped, preparedSharedAssets={}, instruments={}, engineManagedPatches={}, fixedPerPatchPatches={}, adjacentCapabilitiesDistinct={}, initialGraphRevision={}, graphRevision={}, engineSwitches={}, fallbacks={}, callbackAllocations={}, callbackDestructions={}, cleanup={}, activeNotes={}",
            if complete { "complete" } else { "incomplete" },
            coverage.exercised().len(),
            coverage.expected().len(),
            coverage.exercised_engine_transitions().len(),
            coverage.expected_engine_transitions().len(),
            shell_coverage.qualifying_frames(),
            checkpoints.len(),
            event_log.total_observed(),
            event_log.dropped_records(),
            runtime_audio.prepared_shared_assets(),
            runtime_audio.prepared_instruments(),
            runtime_audio.engine_managed_patches(),
            runtime_audio.fixed_per_patch_patches(),
            runtime_audio.adjacent_capabilities_distinct(),
            runtime_audio.initial_graph_revision(),
            runtime_audio.active_graph_revision(),
            runtime_audio.engine_switches(),
            runtime_audio.fallbacks(),
            runtime_audio.callback_allocations(),
            runtime_audio.callback_destructions(),
            cleanup_complete,
            final_observation.active_notes(),
        );

        Ok(Self {
            scene,
            complete,
            checkpoints,
            event_log,
            state_tree,
            graphical_shell,
            coverage,
            shell_coverage,
            final_audio_observation: final_observation,
            runtime_audio,
            mixer_routing,
            effects_and_buses,
            patch_editor,
            detail_and_assets,
            summary,
        })
    }

    pub const fn schema_version(&self) -> u32 {
        Self::SCHEMA_VERSION
    }

    pub fn scene(&self) -> &str {
        &self.scene
    }

    pub const fn complete(&self) -> bool {
        self.complete
    }

    pub fn checkpoints(&self) -> &[LiveCheckpoint] {
        &self.checkpoints
    }

    pub const fn event_log(&self) -> &EventLog {
        &self.event_log
    }

    pub fn event_log_summary(&self) -> LiveEventLogSummary {
        LiveEventLogSummary::from_event_log(
            &self.event_log,
            self.runtime_audio.active_graph_revision(),
        )
    }

    pub const fn state_tree(&self) -> &StateTree {
        &self.state_tree
    }

    pub const fn graphical_shell(&self) -> &GraphicalShellProjection {
        &self.graphical_shell
    }

    pub const fn coverage(&self) -> &LiveDemoCoverage {
        &self.coverage
    }

    pub const fn shell_coverage(&self) -> &LiveShellCoverage {
        &self.shell_coverage
    }

    pub const fn final_audio_observation(&self) -> AudioObservationSnapshot {
        self.final_audio_observation
    }

    pub const fn runtime_audio(&self) -> RuntimeAudioWitness {
        self.runtime_audio
    }

    /// The functional Patch editor measurement, present only for the scene
    /// that declares it.
    pub const fn patch_editor(&self) -> Option<&crate::testing::PatchEditorMeasurement> {
        self.patch_editor.as_ref()
    }

    pub const fn effects_and_buses(&self) -> Option<&LiveEffectsAndBusesEvidence> {
        self.effects_and_buses.as_ref()
    }

    pub const fn detail_and_assets(&self) -> Option<&LiveDetailAssetsEvidence> {
        self.detail_and_assets.as_ref()
    }

    pub const fn mixer_routing(&self) -> LiveMixerRoutingEvidence {
        self.mixer_routing
    }

    /// Dedicated Mixer-scene evidence. Other retained scene identities do
    /// not acquire this report implicitly, which keeps their schemas stable.
    pub fn live_mixer(&self) -> Option<LiveMixerSceneEvidence> {
        (self.scene == "live-mixer").then(|| measure_live_mixer(self))
    }

    pub fn summary(&self) -> &str {
        &self.summary
    }

    pub fn to_json(&self) -> Result<String, LiveDemoReportError> {
        serde_json::to_string(self).map_err(|_| LiveDemoReportError::Serialization)
    }
}

fn measure_live_mixer(report: &LiveDemoReport) -> LiveMixerSceneEvidence {
    let is_main =
        |identifier: &&String| identifier.starts_with("track.T") && !identifier.contains(".sends[");
    let is_send = |identifier: &&String| identifier.contains(".sends[");
    let expected_main = report.coverage.expected().iter().filter(is_main).count();
    let exercised_main = report.coverage.exercised().iter().filter(is_main).count();
    let expected_sends = report.coverage.expected().iter().filter(is_send).count();
    let exercised_sends = report.coverage.exercised().iter().filter(is_send).count();
    let class_exact = |name: &str| {
        let suffix = format!(".{name}");
        MixerTrackId::ALL.into_iter().all(|track_id| {
            report
                .coverage
                .exercised()
                .contains(&format!("track.{track_id}{suffix}"))
        })
    };

    let semantic = report.graphical_shell.semantic_model();
    let inspector_projection_exact =
        mixer_inspector_projection_is_exact(semantic, report.state_tree.json());
    let multi_select_advertised = semantic.valid_actions().iter().any(|action| {
        action.action() == &SemanticAction::SetInteractionMode(InteractionMode::MultiSelect)
    });
    let multi_select_exercised = report.event_log.records().iter().any(|record| {
        record.outcome() == EventOutcome::Accepted
            && matches!(
                record.input(),
                EventInput::SetInteractionMode {
                    mode: InteractionMode::MultiSelect
                }
            )
    });
    let multi_select = if multi_select_advertised && multi_select_exercised {
        LiveMixerMultiSelectResult::Exercised
    } else {
        LiveMixerMultiSelectResult::NotImplemented
    };

    LiveMixerSceneEvidence {
        focus_pairs_expected: expected_main,
        focus_pairs_exercised: exercised_main,
        focus_matrix_exact: expected_main == LiveMixerSceneEvidence::FOCUS_PAIRS
            && exercised_main == LiveMixerSceneEvidence::FOCUS_PAIRS
            && report
                .coverage
                .missing()
                .iter()
                .all(|id| !id.starts_with("track.T")),
        level_edits_exact: class_exact(MixerTrackParameter::Level.name()),
        pan_edits_exact: class_exact(MixerTrackParameter::Pan.name()),
        mute_edits_exact: class_exact(MixerTrackParameter::Mute.name()),
        solo_edits_exact: class_exact(MixerTrackParameter::Solo.name()),
        indexed_send_edits_expected: expected_sends,
        indexed_send_edits_exercised: exercised_sends,
        inspector_painted: report.shell_coverage.mixer_inspector_observed(),
        inspector_return_painted: report.shell_coverage.mixer_return_observed(),
        inspector_projection_exact,
        selected_track_meter_correlated: report.mixer_routing.pre_gate_meters_exact(),
        multi_select,
        multi_select_truthful: !multi_select_advertised || multi_select_exercised,
        milestone_timeout_ms: u64::try_from(
            crate::testing::live_demo_runner::LIVE_DEMO_NO_PROGRESS_TIMEOUT.as_millis(),
        )
        .unwrap_or(u64::MAX),
        total_timeout_ms: u64::try_from(
            crate::testing::live_demo_runner::LIVE_DEMO_TOTAL_TIMEOUT.as_millis(),
        )
        .unwrap_or(u64::MAX),
    }
}

fn mixer_inspector_projection_is_exact(
    semantic: &crate::control::SemanticGraphicalViewModel,
    state_tree_json: &str,
) -> bool {
    let Some(main) = semantic.surface(SurfaceId::MixerMain) else {
        return false;
    };
    let Some(inspector) = semantic.surface(SurfaceId::MixerInspector) else {
        return false;
    };
    let Some(focused) = main.controls().iter().find(|control| control.focused()) else {
        return false;
    };
    let SemanticControlId::Mixer(focused_id) = focused.path().control_id() else {
        return false;
    };
    let Some(focused_track) = focused_id.track_id() else {
        return false;
    };
    let SemanticSurfaceSummary::MixerInspector {
        focused_control,
        focused_track: summary_track,
        routed_patches,
        ..
    } = inspector.summary()
    else {
        return false;
    };
    if focused_control != focused_id || *summary_track != focused_track {
        return false;
    }
    let sends_exact = inspector.controls().len() >= crate::mixer::bus_id::BusId::COUNT
        && crate::mixer::bus_id::BusId::ALL
            .into_iter()
            .zip(inspector.controls().iter())
            .all(|(bus, control)| {
                matches!(
                    control.path().control_id(),
                    SemanticControlId::Mixer(MixerControlId::Send {
                        track_id,
                        bus: control_bus,
                    }) if *track_id == focused_track && *control_bus == bus
                ) && control.numeric_range().is_some()
            });
    let main_values_exact = main.controls().iter().all(|control| {
        matches!(
            control.path().control_id(),
            SemanticControlId::Mixer(MixerControlId::Track { parameter, .. })
                if control.numeric_range().is_some()
                    == matches!(
                        parameter,
                        MixerTrackParameter::Level | MixerTrackParameter::Pan
                    )
        )
    });
    let expected_routes = serde_json::from_str::<serde_json::Value>(state_tree_json)
        .ok()
        .and_then(|tree| {
            tree.get("patches")
                .and_then(serde_json::Value::as_array)
                .cloned()
        })
        .map(|patches| {
            patches
                .iter()
                .filter(|patch| {
                    patch
                        .pointer("/output/trackId")
                        .and_then(serde_json::Value::as_u64)
                        == Some(focused_track.index() as u64)
                })
                .filter_map(|patch| {
                    Some((
                        u32::try_from(patch.get("id")?.as_u64()?).ok()?,
                        patch.get("name")?.as_str()?.to_owned(),
                    ))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let projected_routes = routed_patches
        .iter()
        .map(|patch| (patch.patch_id().value(), patch.patch_name().to_owned()))
        .collect::<Vec<_>>();
    sends_exact && main_values_exact && projected_routes == expected_routes
}

fn measure_mixer_routing(
    checkpoints: &[LiveCheckpoint],
    event_log: &EventLog,
    state_tree: &StateTree,
    graphical_shell: &GraphicalShellProjection,
    runtime_audio: RuntimeAudioWitness,
) -> LiveMixerRoutingEvidence {
    let dsp = LiveMixerDspEvidence::measure();
    let tree = serde_json::from_str::<serde_json::Value>(state_tree.json()).ok();
    // Absent-vs-zero: `None` when the StateTree carries no mixer track
    // array; a parsed empty array is a genuinely measured `Some(0)`.
    let track_count = tree
        .as_ref()
        .and_then(|value| value.pointer("/mixer/tracks"))
        .and_then(serde_json::Value::as_array)
        .map(Vec::len);

    let expected_main_controls = MixerTrackId::ALL
        .into_iter()
        .flat_map(|track_id| {
            MixerTrackParameter::MAIN
                .into_iter()
                .map(move |parameter| (track_id, parameter))
        })
        .collect::<Vec<_>>();
    let projected_main_controls = graphical_shell
        .semantic_model()
        .surface(SurfaceId::MixerMain)
        .map(|surface| {
            surface
                .controls()
                .iter()
                .filter_map(|control| match control.path().control_id() {
                    SemanticControlId::Mixer(MixerControlId::Track {
                        track_id,
                        parameter,
                    }) => Some((*track_id, *parameter)),
                    SemanticControlId::Mixer(_)
                    | SemanticControlId::Patch(_)
                    | SemanticControlId::Send(_)
                    | SemanticControlId::Modal(_)
                    | SemanticControlId::MidiInputDevice(_)
                    | SemanticControlId::ControllerSetting(_)
                    | SemanticControlId::SessionFileAction(_)
                    | SemanticControlId::MidiInputListRoot
                    | SemanticControlId::SurfaceRoot => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let track_ids_exact = projected_main_controls == expected_main_controls;

    let parameter_checkpoints = checkpoints
        .iter()
        .filter_map(LiveCheckpoint::as_parameter)
        .collect::<Vec<_>>();
    let mut exercised = [[false; TRACK_PARAMETER_CLASSES]; MixerTrackId::COUNT];
    let mut mute_observed = [false; MixerTrackId::COUNT];
    let mut solo_observed = [false; MixerTrackId::COUNT];
    let mut reverb_observed = [false; MixerTrackId::COUNT];
    let mut delay_observed = [false; MixerTrackId::COUNT];
    let mut every_meter_set_is_finite = true;

    for checkpoint in &parameter_checkpoints {
        let observation = checkpoint.audio_observation();
        every_meter_set_is_finite &= observation.tracks().iter().all(|meter| {
            meter.left_peak().is_finite()
                && meter.right_peak().is_finite()
                && meter.rms().is_finite()
                && meter.left_peak() >= 0.0
                && meter.right_peak() >= 0.0
                && meter.rms() >= 0.0
        });
        if let Some(LiveEditableParameter::Send { track_id, bus }) =
            checkpoint.expected_transition().editable_parameter()
        {
            // The scene's two audible indexed sends carry the send-coverage
            // evidence the retired per-name aliases carried before.
            let send_observed = checkpoint.agrees()
                && checkpoint
                    .expected_transition()
                    .value_after()
                    .is_some_and(|value| value > 0.0)
                && observation.track(*track_id).rms() > 0.0;
            match bus.index() {
                0 => {
                    exercised[track_id.index()][4] = true;
                    reverb_observed[track_id.index()] = send_observed
                        && observation.reverb_input_rms() > 0.0
                        && matches!(
                            checkpoint.audio_predicate(),
                            crate::testing::live_demo_scene::LiveAudioPredicate::TrackReverbInput {
                                track_id: observed
                            } if observed == *track_id
                        );
                }
                1 => {
                    exercised[track_id.index()][5] = true;
                    delay_observed[track_id.index()] = send_observed
                        && observation.delay_input_rms() > 0.0
                        && matches!(
                            checkpoint.audio_predicate(),
                            crate::testing::live_demo_scene::LiveAudioPredicate::TrackDelayInput {
                                track_id: observed
                            } if observed == *track_id
                        );
                }
                _ => {}
            }
            continue;
        }
        let Some(LiveEditableParameter::Track {
            track_id,
            parameter,
        }) = checkpoint.expected_transition().editable_parameter()
        else {
            continue;
        };
        exercised[track_id.index()][track_parameter_index(*parameter)] = true;
        let meter_nonzero = observation.track(*track_id).rms() > 0.0;
        let accepted_toggle = checkpoint.expected_transition().value_after() == Some(1.0);
        match parameter {
            MixerTrackParameter::Mute => {
                mute_observed[track_id.index()] = checkpoint.agrees()
                    && accepted_toggle
                    && meter_nonzero
                    && matches!(
                        checkpoint.audio_predicate(),
                        crate::testing::live_demo_scene::LiveAudioPredicate::TrackPreGateMeter {
                            track_id: observed
                        } if observed == *track_id
                    );
            }
            MixerTrackParameter::Solo => {
                solo_observed[track_id.index()] = checkpoint.agrees()
                    && accepted_toggle
                    && meter_nonzero
                    && observation.output_rms() > 0.0
                    && matches!(
                        checkpoint.audio_predicate(),
                        crate::testing::live_demo_scene::LiveAudioPredicate::TrackOutput {
                            track_id: observed
                        } if observed == *track_id
                    );
            }
            MixerTrackParameter::Level | MixerTrackParameter::Pan => {}
        }
    }

    let all_track_parameters_exercised = exercised.iter().all(|track| track.iter().all(|set| *set));
    let track_parameter_classes_exercised = (0..TRACK_PARAMETER_CLASSES)
        .filter(|parameter| exercised.iter().any(|track| track[*parameter]))
        .count();
    let all_tracks_projected = track_count == Some(MixerTrackId::COUNT)
        && track_ids_exact
        && all_track_parameters_exercised;

    let installed_outputs = installed_patch_outputs(event_log);
    let final_outputs = tree
        .as_ref()
        .and_then(tree_patch_outputs)
        .unwrap_or_default();
    let outputs_restored = !installed_outputs.is_empty() && final_outputs == installed_outputs;
    let final_routes = final_outputs
        .iter()
        .map(|(_, output)| output.track_id())
        .collect::<Vec<_>>();
    let empty_tracks_addressable = MixerTrackId::ALL.into_iter().any(|track_id| {
        !final_routes.contains(&track_id) && exercised[track_id.index()].iter().all(|set| *set)
    });

    let route_checkpoint = parameter_checkpoints.iter().copied().find(|checkpoint| {
        matches!(
            checkpoint.expected_transition().editable_parameter(),
            Some(LiveEditableParameter::PatchOutput {
                parameter: PatchOutputParameter::OutputTrack,
                ..
            })
        )
    });
    let trim_checkpoint = parameter_checkpoints.iter().copied().find(|checkpoint| {
        matches!(
            checkpoint.expected_transition().editable_parameter(),
            Some(LiveEditableParameter::PatchOutput {
                parameter: PatchOutputParameter::TrimGain,
                ..
            })
        )
    });

    let route_identity = route_checkpoint.and_then(|checkpoint| {
        let LiveEditableParameter::PatchOutput { patch_id, .. } =
            checkpoint.expected_transition().editable_parameter()?
        else {
            return None;
        };
        let value = checkpoint.expected_transition().value_after()?;
        if !value.is_finite() || value.fract() != 0.0 {
            return None;
        }
        let value = u8::try_from(value as i64).ok()?;
        MixerTrackId::new(value)
            .ok()
            .map(|track_id| (*patch_id, track_id))
    });
    // Absent-vs-zero: `None` when no reroute checkpoint identified a shared
    // destination track; a measured count (even `Some(0)`) is judged on its
    // own merits by the completeness expectation.
    let shared_track_patch_count = route_identity.map(|(patch_id, target)| {
        installed_outputs
            .iter()
            .filter(|(candidate, output)| {
                output.track_id() == target
                    || (*candidate == patch_id && output.track_id() != target)
            })
            .count()
    });
    let shared_track_measured = shared_track_patch_count.is_some_and(|count| count > 1);

    let route_audio_exact =
        route_checkpoint
            .zip(route_identity)
            .is_some_and(|(checkpoint, (patch_id, track_id))| {
                let observation = checkpoint.audio_observation();
                scalar_checkpoint_matches_graph(checkpoint, runtime_audio.initial_graph_revision())
                    && observation.primary_patch_id() == Some(patch_id)
                    && observation.primary_active_notes() > 0
                    && observation.primary_patch_rms() > 0.0
                    && observation.track(track_id).rms() > 0.0
                    && observation.output_rms() > 0.0
            });
    let shared_level_exact = route_identity.is_some_and(|(_, shared_track)| {
        parameter_checkpoints.iter().any(|checkpoint| {
            matches!(
                checkpoint.expected_transition().editable_parameter(),
                Some(LiveEditableParameter::Track {
                    track_id,
                    parameter: MixerTrackParameter::Level,
                }) if *track_id == shared_track
            ) && scalar_checkpoint_matches_graph(checkpoint, runtime_audio.initial_graph_revision())
                && checkpoint.audio_observation().active_notes() >= 2
                && checkpoint.audio_observation().track(shared_track).rms() > 0.0
                && checkpoint.audio_observation().output_rms() > 0.0
        })
    });

    let patch_trim_isolated = trim_checkpoint.is_some_and(|checkpoint| {
        outputs_restored
            && scalar_checkpoint_matches_graph(checkpoint, runtime_audio.initial_graph_revision())
            && checkpoint.expected_transition().value_before()
                != checkpoint.expected_transition().value_after()
            && checkpoint.audio_observation().output_rms() > 0.0
    });
    let patch_reroute_isolated = route_checkpoint.is_some_and(|checkpoint| {
        outputs_restored
            && scalar_checkpoint_matches_graph(checkpoint, runtime_audio.initial_graph_revision())
            && checkpoint.expected_transition().value_before()
                != checkpoint.expected_transition().value_after()
            && route_audio_exact
    });
    let invalid_route_rejected = event_log.records().iter().any(|record| {
        record.source() == EventSource::DemoScene
            && record.outcome() == EventOutcome::Rejected
            && record.rejection() == Some(EventRejection::ParameterAtBoundary)
            && matches!(record.input(), EventInput::Adjust { .. })
            && record.generation_before() == record.generation_after()
            && record.parameter_generation() == record.generation_after()
            && record.state_hash_before() == record.state_hash_after()
            && record.emitted_events().is_empty()
    });

    let fixed_snapshot_exact = tree.as_ref().is_some_and(|value| {
        value.pointer("/mixer/tracks") == value.pointer("/parameters/mixerTracks")
            && value
                .pointer("/patches")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|patches| {
                    value
                        .pointer("/parameters/patches")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|parameters| {
                            patches.len() == parameters.len()
                                && patches.iter().zip(parameters).all(|(patch, parameter)| {
                                    patch.get("output") == parameter.get("output")
                                })
                        })
                })
            && value
                .pointer("/parameters/patchCount")
                .and_then(serde_json::Value::as_u64)
                == value
                    .pointer("/patches")
                    .and_then(serde_json::Value::as_array)
                    .map(|patches| patches.len() as u64)
            && track_count == Some(MixerTrackId::COUNT)
            && parameter_checkpoints
                .iter()
                .all(|checkpoint| checkpoint.agrees())
    });
    let stable_focus_exact = all_track_parameters_exercised
        && parameter_checkpoints
            .iter()
            .filter(|checkpoint| {
                matches!(
                    checkpoint.expected_transition().editable_parameter(),
                    Some(LiveEditableParameter::Track { .. } | LiveEditableParameter::Send { .. })
                )
            })
            .count()
            == MixerTrackId::COUNT * TRACK_PARAMETER_CLASSES;

    LiveMixerRoutingEvidence {
        track_count,
        track_ids_exact,
        all_tracks_projected,
        empty_tracks_addressable,
        shared_track_patch_count,
        shared_track_sum_exact: shared_track_measured
            && shared_level_exact
            && dsp.shared_track_sum_exact,
        track_level_controls_shared_sum: shared_track_measured
            && shared_level_exact
            && dsp.track_level_controls_shared_sum,
        patch_trim_isolated: patch_trim_isolated && dsp.patch_trim_isolated,
        patch_reroute_isolated: patch_reroute_isolated && dsp.patch_reroute_isolated,
        invalid_route_rejected,
        track_parameter_classes_exercised,
        mute_wins: mute_observed.into_iter().all(|observed| observed) && dsp.mute_wins,
        any_solo_exact: solo_observed.into_iter().all(|observed| observed) && dsp.any_solo_exact,
        post_gate_sends_exact: reverb_observed.into_iter().all(|observed| observed)
            && delay_observed.into_iter().all(|observed| observed)
            && dsp.post_gate_sends_exact,
        pre_gate_meters_exact: every_meter_set_is_finite
            && mute_observed.into_iter().all(|observed| observed)
            && dsp.pre_gate_meters_exact,
        fixed_snapshot_exact,
        stable_focus_exact,
        callback_allocations: runtime_audio.callback_allocations(),
        callback_destructions: runtime_audio.callback_destructions(),
    }
}

/// The retained sixteen-track evidence matrix width: the four `MAIN` fader
/// classes plus the scene's two production-audible indexed sends (the buses
/// whose returns are occupied by default). The retired per-name send aliases
/// held positions 4 and 5; the indexed sends toward buses 0 and 1 keep those
/// exact positions so the retained evidence stays comparable.
const TRACK_PARAMETER_CLASSES: usize = MixerTrackParameter::MAIN.len() + 2;

const fn track_parameter_index(parameter: MixerTrackParameter) -> usize {
    match parameter {
        MixerTrackParameter::Level => 0,
        MixerTrackParameter::Pan => 1,
        MixerTrackParameter::Mute => 2,
        MixerTrackParameter::Solo => 3,
    }
}

fn scalar_checkpoint_matches_graph(
    checkpoint: &LiveDemoCheckpoint,
    graph_revision: GraphRevision,
) -> bool {
    let generation = checkpoint.generation();
    checkpoint.agrees()
        && checkpoint.audio_observation().active_graph_revision() == graph_revision
        && matches!(
            checkpoint.emitted_effects(),
            [
                EmittedEvent::StateAccepted {
                    generation: accepted
                },
                EmittedEvent::ParameterSnapshotPublished {
                    generation: published,
                    graph_revision: published_graph,
                },
            ] if *accepted == generation
                && *published == generation
                && *published_graph == graph_revision
        )
}

fn installed_patch_outputs(event_log: &EventLog) -> Vec<(PatchId, PatchOutput)> {
    event_log
        .records()
        .iter()
        .find_map(|record| match record.input() {
            EventInput::InstallPatches { patches } => Some(
                patches
                    .iter()
                    .filter_map(|patch| {
                        PatchId::new(patch.id())
                            .ok()
                            .map(|patch_id| (patch_id, patch.output()))
                    })
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

fn tree_patch_outputs(value: &serde_json::Value) -> Option<Vec<(PatchId, PatchOutput)>> {
    value
        .pointer("/patches")?
        .as_array()?
        .iter()
        .map(|patch| {
            let patch_id = u32::try_from(patch.get("id")?.as_u64()?).ok()?;
            let patch_id = PatchId::new(patch_id).ok()?;
            let output = serde_json::from_value(patch.get("output")?.clone()).ok()?;
            Some((patch_id, output))
        })
        .collect()
}

fn runtime_composition_matches(tree: &StateTree, runtime: RuntimeAudioWitness) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(tree.json()) else {
        return false;
    };
    let Some(patches) = value.get("patches").and_then(serde_json::Value::as_array) else {
        return false;
    };
    let capabilities: Vec<&str> = patches
        .iter()
        .filter_map(|patch| {
            patch
                .pointer("/instrument/capabilityId")
                .and_then(serde_json::Value::as_str)
        })
        .collect();
    if capabilities.len() != patches.len() {
        return false;
    }
    let Ok(registry) = serde_json::from_value::<crate::synth::CapabilityRegistry>(
        value.get("capabilities").cloned().unwrap_or_default(),
    ) else {
        return false;
    };
    let engine_managed = capabilities
        .iter()
        .filter(|capability| {
            registry.descriptors().iter().any(|descriptor| {
                descriptor.id().as_str() == **capability
                    && matches!(
                        descriptor.voice_policy(),
                        crate::synth::VoicePolicy::EngineManaged
                            | crate::synth::VoicePolicy::Configurable { .. }
                    )
            })
        })
        .count();
    let fixed_per_patch = capabilities
        .iter()
        .filter(|capability| {
            registry.descriptors().iter().any(|descriptor| {
                descriptor.id().as_str() == **capability
                    && matches!(
                        descriptor.voice_policy(),
                        crate::synth::VoicePolicy::FixedPerPatch { .. }
                    )
            })
        })
        .count();
    let adjacent_distinct =
        capabilities.len() > 1 && capabilities.windows(2).all(|pair| pair[0] != pair[1]);
    engine_managed.saturating_add(fixed_per_patch) == patches.len()
        && runtime.engine_managed_patches() == engine_managed
        && runtime.fixed_per_patch_patches() == fixed_per_patch
        && runtime.adjacent_capabilities_distinct() == adjacent_distinct
}

fn engine_checkpoint_sequence_is_complete(
    checkpoints: &[LiveCheckpoint],
    coverage: &LiveDemoCoverage,
) -> bool {
    let engines: Vec<_> = checkpoints
        .iter()
        .filter_map(LiveCheckpoint::as_engine)
        .collect();
    if engines.len()
        != coverage
            .expected_engine_transitions()
            .len()
            .saturating_mul(3)
    {
        return false;
    }
    for (index, expected) in coverage.expected_engine_transitions().iter().enumerate() {
        let offset = index * 3;
        let lifecycle = &engines[offset..offset + 3];
        if lifecycle.iter().any(|checkpoint| {
            checkpoint.transition_index() != index || checkpoint.transition() != expected
        }) || lifecycle[0].status() != crate::control::EngineSelectionStatusKind::Loading
            || lifecycle[1].status() != crate::control::EngineSelectionStatusKind::Activating
            || lifecycle[2].status() != crate::control::EngineSelectionStatusKind::Ready
            || !lifecycle[2].target_audio_nonzero()
        {
            return false;
        }
        if expected == "SoundFontPresetToNext"
            && (!lifecycle[0].source_audio_nonzero()
                || lifecycle.iter().any(|checkpoint| {
                    checkpoint.preset().is_none()
                        || !matches!(
                            checkpoint.intent(),
                            crate::control::StructuralEditIntent::ReplaceParameterChoice { .. }
                        )
                }))
        {
            return false;
        }
        if expected != "SoundFontPresetToNext"
            && lifecycle.iter().any(|checkpoint| {
                checkpoint.preset().is_some()
                    || !matches!(
                        checkpoint.intent(),
                        crate::control::StructuralEditIntent::ReplaceCapability { .. }
                    )
            })
        {
            return false;
        }
        if index > 0 && lifecycle[2].graph_revision() <= engines[offset - 1].graph_revision() {
            return false;
        }
    }
    true
}

fn patch_control_checkpoint_sequence_is_complete(
    checkpoints: &[LiveCheckpoint],
    installed_patches: &[PatchId],
    state_tree: &StateTree,
) -> bool {
    let Some(focused_patch) = installed_patches.first().copied() else {
        return false;
    };
    let patch_adsr = checkpoints
        .iter()
        .filter_map(LiveCheckpoint::as_parameter)
        .filter(|checkpoint| {
            matches!(
                checkpoint.expected_transition().patch_control_id(),
                Some(crate::control::PatchControlId::Envelope(_))
            )
        })
        .collect::<Vec<_>>();
    if patch_adsr.len() != crate::synth::VoiceEnvelope::surface_descriptor().len() {
        return false;
    }
    let adsr_complete = patch_adsr
        .iter()
        .zip(crate::synth::VoiceEnvelope::surface_descriptor())
        .all(|(checkpoint, descriptor)| {
            let parameter = descriptor.parameter();
            checkpoint.expected_transition().patch_control_id()
                == Some(crate::control::PatchControlId::Envelope(parameter))
                && matches!(
                    checkpoint.expected_transition().editable_parameter(),
                    Some(LiveEditableParameter::Patch {
                        patch_id,
                        target: crate::synth::PatchEditableTarget::Envelope(actual),
                    }) if *patch_id == focused_patch && *actual == parameter
                )
        });
    if !adsr_complete {
        return false;
    }

    let Ok(value) = serde_json::from_str::<serde_json::Value>(state_tree.json()) else {
        return false;
    };
    let Ok(registry) = serde_json::from_value::<crate::synth::EffectCapabilityRegistry>(
        value.get("effects").cloned().unwrap_or_default(),
    ) else {
        return false;
    };
    let Some(configs) = value
        .pointer("/patches/0/postEffects")
        .cloned()
        .and_then(|configs| {
            serde_json::from_value::<Vec<crate::synth::PostEffectConfig>>(configs).ok()
        })
    else {
        return false;
    };
    let mut expected_effects = Vec::new();
    for config in &configs {
        let Some(descriptor) = registry.descriptor(config.capability_id()) else {
            return false;
        };
        for spec in descriptor.parameters() {
            let predicate_satisfied = |predicate: Option<&crate::synth::ParameterPredicate>| {
                predicate.is_none_or(|predicate| {
                    config.value(predicate.parameter_id()) == Some(predicate.equals())
                })
            };
            if spec.patch_interaction() == crate::synth::PatchInteraction::ScalarEdit
                && predicate_satisfied(spec.visible_when())
                && predicate_satisfied(spec.enabled_when())
            {
                expected_effects.push((
                    crate::control::PatchControlId::Effect(config.slot_id(), spec.id().clone()),
                    crate::testing::live_demo_scene::LiveEditableParameter::effect(
                        focused_patch,
                        config.slot_id(),
                        spec.id().clone(),
                    ),
                ));
            }
        }
    }
    let actual_effects = checkpoints
        .iter()
        .filter_map(LiveCheckpoint::as_parameter)
        .filter(|checkpoint| {
            matches!(
                checkpoint.expected_transition().patch_control_id(),
                Some(crate::control::PatchControlId::Effect(_, _))
            )
        })
        .collect::<Vec<_>>();
    actual_effects.len() == expected_effects.len()
        && actual_effects
            .iter()
            .zip(expected_effects)
            .all(|(checkpoint, (control, parameter))| {
                checkpoint.expected_transition().patch_control_id() == Some(control)
                    && checkpoint.expected_transition().editable_parameter() == Some(&parameter)
                    && matches!(
                        checkpoint.audio_predicate(),
                        crate::testing::live_demo_scene::LiveAudioPredicate::PatchEffect {
                            patch_id
                        } if patch_id == focused_patch
                    )
            })
}

fn final_soundfont_config_is_default(tree: &StateTree) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(tree.json()) else {
        return false;
    };
    let Some(instrument) = value.pointer("/patches/0/instrument").cloned() else {
        return false;
    };
    let Ok(actual) = serde_json::from_value::<crate::synth::InstrumentConfig>(instrument) else {
        return false;
    };
    let Ok(registry) = serde_json::from_value::<crate::synth::CapabilityRegistry>(
        value.get("capabilities").cloned().unwrap_or_default(),
    ) else {
        return false;
    };
    let Ok(effect_registry) = serde_json::from_value::<crate::synth::EffectCapabilityRegistry>(
        value.get("effects").cloned().unwrap_or_default(),
    ) else {
        return false;
    };
    let Some(post_effects) = value
        .pointer("/patches/0/postEffects")
        .cloned()
        .and_then(|effects| {
            serde_json::from_value::<Vec<crate::synth::PostEffectConfig>>(effects).ok()
        })
    else {
        return false;
    };
    let Some(descriptor) = registry.descriptor(actual.capability_id()) else {
        return false;
    };
    let mut values = Vec::new();
    let mut assets = Vec::new();
    for parameter in descriptor.parameters() {
        match parameter.default_value() {
            crate::synth::ParameterDefault::Value(value) => values.push(
                crate::synth::ParameterAssignment::new(parameter.id().clone(), value.clone()),
            ),
            crate::synth::ParameterDefault::Asset(reference) => assets.push(
                crate::synth::AssetAssignment::new(parameter.id().clone(), reference.clone()),
            ),
        }
    }
    let Ok(expected) = descriptor.create_config(&values, &assets) else {
        return false;
    };
    value
        .pointer("/engineSelection/kind")
        .and_then(serde_json::Value::as_str)
        == Some("ready")
        && actual == expected
        && post_effects.len() == 1
        && effect_registry
            .validate_patch_effects(&post_effects)
            .is_ok()
}

impl Serialize for LiveDemoReport {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let state_tree: serde_json::Value =
            serde_json::from_str(self.state_tree.json()).map_err(serde::ser::Error::custom)?;
        let mut report = serializer.serialize_struct("LiveDemoReport", 15)?;
        report.serialize_field("schemaVersion", &Self::SCHEMA_VERSION)?;
        report.serialize_field("scene", &self.scene)?;
        report.serialize_field("complete", &self.complete)?;
        report.serialize_field("checkpoints", &self.checkpoints)?;
        report.serialize_field("eventLog", &self.event_log)?;
        report.serialize_field("stateTree", &state_tree)?;
        report.serialize_field("graphicalShell", &self.graphical_shell)?;
        report.serialize_field("coverage", &self.coverage)?;
        report.serialize_field("shellCoverage", &self.shell_coverage)?;
        report.serialize_field("finalAudioObservation", &self.final_audio_observation)?;
        report.serialize_field("runtimeAudio", &self.runtime_audio)?;
        report.serialize_field("mixerRouting", &self.mixer_routing)?;
        report.serialize_field("effectsAndBuses", &self.effects_and_buses)?;
        report.serialize_field("detailAndAssets", &self.detail_and_assets)?;
        report.serialize_field("summary", &self.summary)?;
        report.end()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveDemoReportError {
    EmptyScene,
    EmptyEventLog,
    FinalStateMismatch,
    FinalShellMismatch,
    Serialization,
}

impl fmt::Display for LiveDemoReportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyScene => formatter.write_str("live demo scene name must not be empty"),
            Self::EmptyEventLog => formatter.write_str("live demo report requires event evidence"),
            Self::FinalStateMismatch => {
                formatter.write_str("final StateTree is not the EventLog chain endpoint")
            }
            Self::FinalShellMismatch => formatter
                .write_str("final graphical shell is not the canonical StateTree shell endpoint"),
            Self::Serialization => formatter.write_str("live demo report could not be serialized"),
        }
    }
}

impl std::error::Error for LiveDemoReportError {}

#[cfg(test)]
mod tests {
    use super::{
        measure_effects_and_buses, scene_declares_the_effects_and_buses_phase, LiveDemoCoverage,
        LiveEffectsAndBusesEvidence, LiveMixerRoutingEvidence, TRACK_PARAMETER_CLASSES,
    };
    use crate::control::event_record::EventOutcome;
    use crate::control::{EngineSelectionRequestId, SemanticAction};
    use crate::mixer::global_parameters::GlobalParameter;
    use crate::mixer::mix_observation::MixObservation;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::mixer_track_parameters::MixerTrackParameter;
    use crate::mixer::track_meter::TrackMeter;
    use crate::real_time::audio_observation_snapshot::AudioObservationSnapshot;
    use crate::real_time::GraphRevision;
    use crate::testing::live_demo_checkpoint::{LiveCheckpoint, LiveTopologyCheckpoint};
    use crate::testing::live_demo_scene::LiveEditableParameter;
    use crate::testing::live_effects_and_buses_scene::LiveTopologyAudibleWitness;
    use crate::testing::live_mixer_routing_measurement::LiveMixerDspEvidence;

    fn snapshot(sequence: u64, revision: GraphRevision) -> AudioObservationSnapshot {
        let mix = MixObservation::new(
            [TrackMeter::default(); MixerTrackId::COUNT],
            0.25,
            0.25,
            0.25,
            0.0,
            0.0,
            0.0,
            0,
            0,
        );
        AudioObservationSnapshot::from_mix_with_graph_and_routing(
            sequence,
            sequence,
            sequence * 64,
            1,
            revision,
            0,
            1,
            0,
            None,
            mix,
        )
    }

    /// A support-driven scalar transition legitimately carries none of the
    /// NFR-008 lifecycle measurements: an observation set built only from
    /// such transitions is empty for every derived maximum.
    fn scalar_send_checkpoint() -> LiveCheckpoint {
        let checkpoint = LiveTopologyCheckpoint::new(
            "Send.towardDestinations",
            0,
            None,
            EventOutcome::Accepted,
            None,
            None,
            GraphRevision::INITIAL,
            None,
            7,
            "state-hash",
            None,
            None,
            None,
            true,
            LiveTopologyAudibleWitness::WetReturn,
            snapshot(1, GraphRevision::INITIAL),
            snapshot(2, GraphRevision::INITIAL),
            true,
            0,
            0,
            None,
        )
        .expect("support-driven scalar checkpoint agrees without lifecycle measurements");
        LiveCheckpoint::topology(checkpoint)
    }

    fn lifecycle_checkpoint(transition: &str) -> LiveCheckpoint {
        let target = GraphRevision::INITIAL
            .checked_next()
            .expect("a second graph revision is available");
        let checkpoint = LiveTopologyCheckpoint::new(
            transition,
            1,
            Some(SemanticAction::Return),
            EventOutcome::Accepted,
            None,
            Some(EngineSelectionRequestId::FIRST),
            GraphRevision::INITIAL,
            Some(target),
            8,
            "state-hash",
            Some(1),
            Some(1),
            Some(1),
            true,
            LiveTopologyAudibleWitness::PatchChain,
            snapshot(3, GraphRevision::INITIAL),
            snapshot(4, target),
            true,
            0,
            0,
            None,
        )
        .expect("lifecycle checkpoint agrees with measured responsiveness");
        LiveCheckpoint::topology(checkpoint)
    }

    fn rejected_checkpoint() -> LiveCheckpoint {
        let checkpoint = LiveTopologyCheckpoint::new(
            "Occupancy.invalidRefused",
            2,
            None,
            EventOutcome::Rejected,
            Some("invalidEffectConfig".to_owned()),
            None,
            GraphRevision::INITIAL,
            None,
            9,
            "state-hash",
            None,
            None,
            None,
            true,
            LiveTopologyAudibleWitness::DryContinuity,
            snapshot(5, GraphRevision::INITIAL),
            snapshot(6, GraphRevision::INITIAL),
            true,
            0,
            0,
            None,
        )
        .expect("controlled rejection checkpoint agrees");
        LiveCheckpoint::topology(checkpoint)
    }

    fn dsp_evidence() -> LiveMixerDspEvidence {
        LiveMixerDspEvidence {
            shared_track_sum_exact: true,
            track_level_controls_shared_sum: true,
            patch_trim_isolated: true,
            patch_reroute_isolated: true,
            mute_wins: true,
            any_solo_exact: true,
            post_gate_sends_exact: true,
            pre_gate_meters_exact: true,
            send_isolation_exact: true,
            max_off_target_bus_dbfs: -120.0,
            accumulation_exact: true,
            gated_wet_contribution: 0.0,
            unoccupied_return_silent: true,
        }
    }

    fn complete_effects_evidence() -> LiveEffectsAndBusesEvidence {
        LiveEffectsAndBusesEvidence {
            topology_checkpoints: 12,
            slot_fills_exercised: 3,
            startup_occupant_cleared: true,
            slot_order_exchange_exercised: true,
            same_entry_twin_exercised: true,
            sends_raised_toward_destinations: true,
            return_content_changes: 2,
            empty_return_occupied_and_restored: true,
            controlled_rejection_observed: true,
            rejection_reason: Some("invalidEffectConfig".to_owned()),
            post_rejection_recovery_observed: true,
            reroute_chain_followed: true,
            frames_to_projection_max: Some(1),
            activation_sequence_gap_max: Some(1),
            render_blocks_to_audible_max: Some(1),
            all_audible_on_activated_graph: true,
            send_isolation_exact: true,
            max_off_target_bus_dbfs: -120.0,
            accumulation_exact: true,
            gated_wet_contribution: 0.0,
            unoccupied_return_silent: true,
        }
    }

    fn complete_routing_evidence() -> LiveMixerRoutingEvidence {
        LiveMixerRoutingEvidence {
            track_count: Some(MixerTrackId::COUNT),
            track_ids_exact: true,
            all_tracks_projected: true,
            empty_tracks_addressable: true,
            shared_track_patch_count: Some(2),
            shared_track_sum_exact: true,
            track_level_controls_shared_sum: true,
            patch_trim_isolated: true,
            patch_reroute_isolated: true,
            invalid_route_rejected: true,
            track_parameter_classes_exercised: TRACK_PARAMETER_CLASSES,
            mute_wins: true,
            any_solo_exact: true,
            post_gate_sends_exact: true,
            pre_gate_meters_exact: true,
            fixed_snapshot_exact: true,
            stable_focus_exact: true,
            callback_allocations: 0,
            callback_destructions: 0,
        }
    }

    /// F-49's gate, pinned in both directions against the shipped scene names.
    ///
    /// Both mutations were run rather than argued, and they are not symmetric:
    ///
    /// - **Narrowed** to a never-matching literal, `tests/effects_and_buses.rs`
    ///   already fails ("the cumulative scene retains effects-and-buses
    ///   evidence"). That direction was covered before this test existed — by
    ///   the suite that owns the phase, which is why a sweep over the lib,
    ///   topology, mixer and shell suites misses it.
    /// - **Widened** to always-true, nothing failed: the `effects_and_buses`,
    ///   `live_demo_scene`, `topology_change_lifecycle` and
    ///   `live_patch_editor_scene` suites all stayed green. That is the gap
    ///   this test closes, and it is the direction that would break *this*
    ///   package's scene.
    #[test]
    fn effects_and_buses_evidence_is_gated_to_the_scene_that_declares_the_phase() {
        assert!(scene_declares_the_effects_and_buses_phase(
            crate::testing::EFFECTS_AND_BUSES_SCENE_NAME
        ));
        assert!(!scene_declares_the_effects_and_buses_phase(
            crate::testing::live_patch_editor_scene::PATCH_EDITOR_SCENE_NAME
        ));
        assert!(!scene_declares_the_effects_and_buses_phase(
            "sixteen-track-mixer-routing-live-demo"
        ));
        assert!(!scene_declares_the_effects_and_buses_phase(""));
    }

    /// Why the gate above is load-bearing rather than tidy, executed rather
    /// than argued: a scene that reuses the topology machinery for its own
    /// journey — the functional Patch editor's effect-slot occupancy walk —
    /// produces topology checkpoints that `measure_effects_and_buses` will
    /// happily grade, and they fail the eight-destination bus contract they
    /// never claimed to meet. Ungated, that shortfall would land in that
    /// scene's report and make it incomplete.
    #[test]
    fn a_non_effects_scenes_topology_checkpoints_would_fail_the_bus_contract_ungated() {
        let evidence = measure_effects_and_buses(
            &[
                lifecycle_checkpoint("SlotFill.reverbHall"),
                lifecycle_checkpoint("SlotFill.reverbHall"),
            ],
            &dsp_evidence(),
        )
        .expect("topology checkpoints of any scene's shape measure to evidence");
        assert!(
            !evidence.is_complete(),
            "an occupancy-walk checkpoint set does not satisfy the bus contract",
        );
    }

    #[test]
    fn empty_observation_set_derives_absent_measurements_that_render_null_and_fail() {
        let evidence = measure_effects_and_buses(&[scalar_send_checkpoint()], &dsp_evidence())
            .expect("topology checkpoints retain effects evidence");

        assert_eq!(evidence.frames_to_projection_max, None);
        assert_eq!(evidence.activation_sequence_gap_max, None);
        assert_eq!(evidence.render_blocks_to_audible_max, None);
        assert_eq!(evidence.rejection_reason, None);
        assert!(!evidence.is_complete());

        let json = serde_json::to_value(&evidence).expect("effects evidence serializes");
        for key in [
            "frames_to_projection_max",
            "activation_sequence_gap_max",
            "render_blocks_to_audible_max",
            "rejection_reason",
        ] {
            let value = json.get(key).expect("serialized key name is retained");
            assert!(value.is_null(), "{key} must render as explicitly absent");
        }
    }

    #[test]
    fn measured_lifecycle_observations_derive_unchanged_nonzero_maximums() {
        let evidence = measure_effects_and_buses(
            &[
                scalar_send_checkpoint(),
                lifecycle_checkpoint("SlotFill.reverbHall"),
                rejected_checkpoint(),
            ],
            &dsp_evidence(),
        )
        .expect("topology checkpoints retain effects evidence");

        assert_eq!(evidence.frames_to_projection_max, Some(1));
        assert_eq!(evidence.activation_sequence_gap_max, Some(1));
        assert_eq!(evidence.render_blocks_to_audible_max, Some(1));
        assert_eq!(
            evidence.rejection_reason.as_deref(),
            Some("invalidEffectConfig")
        );
        assert_eq!(evidence.frames_to_projection_max(), 1);
        assert_eq!(evidence.activation_sequence_gap_max(), 1);
        assert_eq!(evidence.render_blocks_to_audible_max(), 1);

        let json = serde_json::to_value(&evidence).expect("effects evidence serializes");
        assert_eq!(json.get("frames_to_projection_max"), Some(&1.into()));
        assert_eq!(json.get("activation_sequence_gap_max"), Some(&1.into()));
        assert_eq!(json.get("render_blocks_to_audible_max"), Some(&1.into()));
    }

    #[test]
    fn absent_measurement_fails_every_effects_completeness_expectation() {
        assert!(complete_effects_evidence().is_complete());

        let mut absent_frames = complete_effects_evidence();
        absent_frames.frames_to_projection_max = None;
        assert!(!absent_frames.is_complete());

        let mut absent_gap = complete_effects_evidence();
        absent_gap.activation_sequence_gap_max = None;
        assert!(!absent_gap.is_complete());

        let mut absent_blocks = complete_effects_evidence();
        absent_blocks.render_blocks_to_audible_max = None;
        assert!(!absent_blocks.is_complete());

        let mut absent_reason = complete_effects_evidence();
        absent_reason.rejection_reason = None;
        assert!(!absent_reason.is_complete());

        let mut empty_reason = complete_effects_evidence();
        empty_reason.rejection_reason = Some(String::new());
        assert!(!empty_reason.is_complete());
    }

    #[test]
    fn measured_zero_renders_as_zero_and_is_judged_on_its_merits() {
        // A genuinely measured zero-frame projection window satisfies the
        // `<= 1` expectation on its own merits and renders as `0`, not null.
        let mut zero_frames = complete_effects_evidence();
        zero_frames.frames_to_projection_max = Some(0);
        assert!(zero_frames.is_complete());
        let json = serde_json::to_value(&zero_frames).expect("effects evidence serializes");
        assert_eq!(json.get("frames_to_projection_max"), Some(&0.into()));
        assert_eq!(zero_frames.frames_to_projection_max(), 0);

        // A measured zero gap or zero blocks-to-audible fails its `>= 1`
        // expectation on its merits — as a measured value, not as absence.
        let mut zero_gap = complete_effects_evidence();
        zero_gap.activation_sequence_gap_max = Some(0);
        assert!(!zero_gap.is_complete());
        let json = serde_json::to_value(&zero_gap).expect("effects evidence serializes");
        assert_eq!(json.get("activation_sequence_gap_max"), Some(&0.into()));

        let mut zero_blocks = complete_effects_evidence();
        zero_blocks.render_blocks_to_audible_max = Some(0);
        assert!(!zero_blocks.is_complete());
    }

    #[test]
    #[should_panic(expected = "frames_to_projection_max is absent")]
    fn absent_frames_to_projection_never_renders_through_the_accessor() {
        let mut evidence = complete_effects_evidence();
        evidence.frames_to_projection_max = None;
        let _ = evidence.frames_to_projection_max();
    }

    #[test]
    fn mixer_routing_distinguishes_absent_counts_from_measured_zero() {
        let complete = complete_routing_evidence();
        assert!(complete.is_complete());
        assert_eq!(complete.track_count(), MixerTrackId::COUNT);
        assert_eq!(complete.shared_track_patch_count(), 2);

        let mut absent_tracks = complete;
        absent_tracks.track_count = None;
        assert!(!absent_tracks.is_complete());
        let json = serde_json::to_value(absent_tracks).expect("routing evidence serializes");
        let value = json
            .get("track_count")
            .expect("serialized key name is retained");
        assert!(value.is_null(), "absent track_count must render as null");

        let mut zero_tracks = complete;
        zero_tracks.track_count = Some(0);
        assert!(!zero_tracks.is_complete());
        let json = serde_json::to_value(zero_tracks).expect("routing evidence serializes");
        assert_eq!(json.get("track_count"), Some(&0.into()));

        let mut absent_shared = complete;
        absent_shared.shared_track_patch_count = None;
        assert!(!absent_shared.is_complete());
        let json = serde_json::to_value(absent_shared).expect("routing evidence serializes");
        assert!(json
            .get("shared_track_patch_count")
            .is_some_and(serde_json::Value::is_null));

        let mut solitary_shared = complete;
        solitary_shared.shared_track_patch_count = Some(1);
        assert!(!solitary_shared.is_complete());
    }

    #[test]
    #[should_panic(expected = "track_count is absent")]
    fn absent_track_count_never_renders_through_the_accessor() {
        let mut evidence = complete_routing_evidence();
        evidence.track_count = None;
        let _ = evidence.track_count();
    }

    #[test]
    fn exact_coverage_reports_missing_unexpected_and_duplicate_expected_values() {
        let gain =
            LiveEditableParameter::track(MixerTrackId::default(), MixerTrackParameter::Level);
        let master = LiveEditableParameter::global(GlobalParameter::MasterGainDb);
        let mut coverage = LiveDemoCoverage::new(&[gain.clone(), master.clone()]);

        coverage.mark_exercised(&gain);
        assert_eq!(coverage.missing(), &["global.masterGainDb"]);
        assert!(!coverage.is_complete());

        coverage.mark_unexpected("patch.1.pan");
        assert_eq!(coverage.unexpected(), &["patch.1.pan"]);
        coverage.mark_exercised(&master);
        assert!(!coverage.is_complete());

        let duplicate = LiveDemoCoverage::new(&[gain.clone(), gain]);
        assert_eq!(duplicate.duplicate_expected(), &["track.T00.levelDb"]);
        assert!(!duplicate.is_complete());
    }
}
