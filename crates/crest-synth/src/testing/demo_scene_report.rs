use crate::control::event_log::EventLog;
use crate::control::state_tree::StateTree;
use crate::control::{
    EngineSelectionFailure, EngineSelectionRequestId, EngineSelectionStatusKind, PatchControlId,
    StructuralEditIntent,
};
use crate::real_time::GraphRevision;
use crate::synth::{CapabilityId, ParameterChoice, ParameterId, VoiceEnvelopeParameter};
use core::fmt;
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

/// One stable section of the exhaustive demo coverage matrix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DemoCoverageGroup {
    Inputs,
    Events,
    Contexts,
    Directions,
    MidiKinds,
    EditableParameters,
    PatchControls,
    SerializedProperties,
    Rejections,
    Projections,
    AudioEffects,
}

/// Control-side evidence sampled from the production mixed-engine render path.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoAudioEvidence {
    mixed_engine_stems_nonzero: bool,
    mixed_engine_parameter_isolation: bool,
    patch_effect_target_exact: bool,
    patch_effect_difference_nonzero: bool,
    patch_effect_side_nonzero: bool,
    patch_effect_before_mix_stem_exact: bool,
    unconfigured_patch_isolated: bool,
    patch_effect_structural_preservation: bool,
}

impl DemoAudioEvidence {
    pub const fn new(
        mixed_engine_stems_nonzero: bool,
        mixed_engine_parameter_isolation: bool,
    ) -> Self {
        Self {
            mixed_engine_stems_nonzero,
            mixed_engine_parameter_isolation,
            patch_effect_target_exact: false,
            patch_effect_difference_nonzero: false,
            patch_effect_side_nonzero: false,
            patch_effect_before_mix_stem_exact: false,
            unconfigured_patch_isolated: false,
            patch_effect_structural_preservation: false,
        }
    }

    pub const fn with_patch_effect(
        mut self,
        target_exact: bool,
        difference_nonzero: bool,
        side_nonzero: bool,
        before_mix_stem_exact: bool,
        unconfigured_patch_isolated: bool,
        structural_preservation: bool,
    ) -> Self {
        self.patch_effect_target_exact = target_exact;
        self.patch_effect_difference_nonzero = difference_nonzero;
        self.patch_effect_side_nonzero = side_nonzero;
        self.patch_effect_before_mix_stem_exact = before_mix_stem_exact;
        self.unconfigured_patch_isolated = unconfigured_patch_isolated;
        self.patch_effect_structural_preservation = structural_preservation;
        self
    }

    pub const fn mixed_engine_stems_nonzero(self) -> bool {
        self.mixed_engine_stems_nonzero
    }

    pub const fn mixed_engine_parameter_isolation(self) -> bool {
        self.mixed_engine_parameter_isolation
    }

    pub const fn patch_effect_target_exact(self) -> bool {
        self.patch_effect_target_exact
    }

    pub const fn patch_effect_difference_nonzero(self) -> bool {
        self.patch_effect_difference_nonzero
    }

    pub const fn patch_effect_side_nonzero(self) -> bool {
        self.patch_effect_side_nonzero
    }

    pub const fn patch_effect_before_mix_stem_exact(self) -> bool {
        self.patch_effect_before_mix_stem_exact
    }

    pub const fn unconfigured_patch_isolated(self) -> bool {
        self.unconfigured_patch_isolated
    }

    pub const fn patch_effect_structural_preservation(self) -> bool {
        self.patch_effect_structural_preservation
    }
}

/// Sorted expected, exercised, and missing identifiers for one coverage group.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoCoverageSet {
    expected: Vec<String>,
    exercised: Vec<String>,
    missing: Vec<String>,
    unexpected: Vec<String>,
}

impl DemoCoverageSet {
    /// Declares the complete expected surface for one coverage group.
    pub fn new<I, S>(expected: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut expected: Vec<String> = expected.into_iter().map(Into::into).collect();
        expected.sort();
        expected.dedup();

        Self {
            missing: expected.clone(),
            expected,
            exercised: Vec::new(),
            unexpected: Vec::new(),
        }
    }

    /// Returns expected identifiers in deterministic order.
    pub fn expected(&self) -> &[String] {
        &self.expected
    }

    /// Returns exercised identifiers in deterministic order.
    pub fn exercised(&self) -> &[String] {
        &self.exercised
    }

    /// Returns expected identifiers not yet exercised.
    pub fn missing(&self) -> &[String] {
        &self.missing
    }

    /// Returns exercised identifiers outside the declared group surface.
    pub fn unexpected(&self) -> &[String] {
        &self.unexpected
    }

    /// Records an identifier, returning true only for its first observation.
    pub fn mark_exercised(&mut self, identifier: impl Into<String>) -> bool {
        let identifier = identifier.into();
        if !insert_sorted_unique(&mut self.exercised, identifier.clone()) {
            return false;
        }
        if let Ok(index) = self.missing.binary_search(&identifier) {
            self.missing.remove(index);
        } else if self.expected.binary_search(&identifier).is_err() {
            insert_sorted_unique(&mut self.unexpected, identifier);
        }
        true
    }

    /// Reports whether every declared identifier was exercised.
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty() && self.unexpected.is_empty()
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

/// Exhaustive coverage grouped by every current observable surface.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoSceneCoverage {
    inputs: DemoCoverageSet,
    events: DemoCoverageSet,
    contexts: DemoCoverageSet,
    directions: DemoCoverageSet,
    midi_kinds: DemoCoverageSet,
    editable_parameters: DemoCoverageSet,
    patch_controls: DemoCoverageSet,
    serialized_properties: DemoCoverageSet,
    rejections: DemoCoverageSet,
    projections: DemoCoverageSet,
    audio_effects: DemoCoverageSet,
}

impl DemoSceneCoverage {
    /// Creates an empty matrix. Callers declare each expected group before use.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces one group's expected surface before observations are recorded.
    pub fn declare_expected<I, S>(&mut self, group: DemoCoverageGroup, expected: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        *self.group_mut(group) = DemoCoverageSet::new(expected);
    }

    /// Marks one stable identifier as exercised in its coverage group.
    pub fn mark_exercised(
        &mut self,
        group: DemoCoverageGroup,
        identifier: impl Into<String>,
    ) -> bool {
        self.group_mut(group).mark_exercised(identifier)
    }

    /// Returns one complete coverage group.
    pub fn group(&self, group: DemoCoverageGroup) -> &DemoCoverageSet {
        match group {
            DemoCoverageGroup::Inputs => &self.inputs,
            DemoCoverageGroup::Events => &self.events,
            DemoCoverageGroup::Contexts => &self.contexts,
            DemoCoverageGroup::Directions => &self.directions,
            DemoCoverageGroup::MidiKinds => &self.midi_kinds,
            DemoCoverageGroup::EditableParameters => &self.editable_parameters,
            DemoCoverageGroup::PatchControls => &self.patch_controls,
            DemoCoverageGroup::SerializedProperties => &self.serialized_properties,
            DemoCoverageGroup::Rejections => &self.rejections,
            DemoCoverageGroup::Projections => &self.projections,
            DemoCoverageGroup::AudioEffects => &self.audio_effects,
        }
    }

    /// Returns the number of expected identifiers in every group.
    pub fn expected_count(&self) -> usize {
        self.groups()
            .iter()
            .map(|group| group.expected().len())
            .sum()
    }

    /// Returns the number of unique exercised identifiers in every group.
    pub fn exercised_count(&self) -> usize {
        self.groups()
            .iter()
            .map(|group| group.exercised().len())
            .sum()
    }

    /// Returns the number of named coverage gaps in every group.
    pub fn missing_count(&self) -> usize {
        self.groups()
            .iter()
            .map(|group| group.missing().len())
            .sum()
    }

    /// Returns the number of observed identifiers outside declared groups.
    pub fn unexpected_count(&self) -> usize {
        self.groups()
            .iter()
            .map(|group| group.unexpected().len())
            .sum()
    }

    /// Reports whether every expected identifier in every group was exercised.
    pub fn is_complete(&self) -> bool {
        self.groups().iter().all(|group| group.is_complete())
    }

    fn group_mut(&mut self, group: DemoCoverageGroup) -> &mut DemoCoverageSet {
        match group {
            DemoCoverageGroup::Inputs => &mut self.inputs,
            DemoCoverageGroup::Events => &mut self.events,
            DemoCoverageGroup::Contexts => &mut self.contexts,
            DemoCoverageGroup::Directions => &mut self.directions,
            DemoCoverageGroup::MidiKinds => &mut self.midi_kinds,
            DemoCoverageGroup::EditableParameters => &mut self.editable_parameters,
            DemoCoverageGroup::PatchControls => &mut self.patch_controls,
            DemoCoverageGroup::SerializedProperties => &mut self.serialized_properties,
            DemoCoverageGroup::Rejections => &mut self.rejections,
            DemoCoverageGroup::Projections => &mut self.projections,
            DemoCoverageGroup::AudioEffects => &mut self.audio_effects,
        }
    }

    fn groups(&self) -> [&DemoCoverageSet; 11] {
        [
            &self.inputs,
            &self.events,
            &self.contexts,
            &self.directions,
            &self.midi_kinds,
            &self.editable_parameters,
            &self.patch_controls,
            &self.serialized_properties,
            &self.rejections,
            &self.projections,
            &self.audio_effects,
        ]
    }
}

/// Invalid data supplied for a deterministic scene checkpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DemoSceneCheckpointError {
    EmptyStep,
    EmptyStateHash,
    GenerationMismatch {
        generation: u64,
        parameter_generation: u64,
    },
    NonFiniteAudioMeasurement,
    PatchAdsrMismatch,
    PresetMismatch,
}

impl fmt::Display for DemoSceneCheckpointError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::EmptyStep => formatter.write_str("checkpoint step must not be empty"),
            Self::EmptyStateHash => {
                formatter.write_str("checkpoint state hash must not be empty")
            }
            Self::GenerationMismatch {
                generation,
                parameter_generation,
            } => write!(
                formatter,
                "checkpoint state generation {generation} does not match parameter generation {parameter_generation}"
            ),
            Self::NonFiniteAudioMeasurement => {
                formatter.write_str("checkpoint audio measurement must be finite")
            }
            Self::PatchAdsrMismatch => formatter.write_str(
                "PATCH ADSR checkpoint state, projection, snapshot, effect, or revision mismatch",
            ),
            Self::PresetMismatch => formatter.write_str(
                "SoundFont preset checkpoint state, projection, intent, config delta, or revision mismatch",
            ),
        }
    }
}

impl std::error::Error for DemoSceneCheckpointError {}

/// One deterministic observation made after a named demo step.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoSceneCheckpoint {
    step: String,
    state_hash: String,
    generation: u64,
    selected_line: usize,
    parameter_generation: u64,
    audio_measurement: f64,
    engine_selection: Option<DemoEngineCheckpoint>,
    preset_selection: Option<DemoPresetCheckpoint>,
    patch_adsr: Option<DemoPatchAdsrCheckpoint>,
}

impl DemoSceneCheckpoint {
    /// Builds a self-consistent checkpoint from one accepted or rejected state.
    pub fn new(
        step: impl Into<String>,
        state_hash: impl Into<String>,
        generation: u64,
        selected_line: usize,
        parameter_generation: u64,
        audio_measurement: f64,
    ) -> Result<Self, DemoSceneCheckpointError> {
        let step = step.into();
        if step.trim().is_empty() {
            return Err(DemoSceneCheckpointError::EmptyStep);
        }

        let state_hash = state_hash.into();
        if state_hash.is_empty() {
            return Err(DemoSceneCheckpointError::EmptyStateHash);
        }
        if generation != parameter_generation {
            return Err(DemoSceneCheckpointError::GenerationMismatch {
                generation,
                parameter_generation,
            });
        }
        if !audio_measurement.is_finite() {
            return Err(DemoSceneCheckpointError::NonFiniteAudioMeasurement);
        }

        Ok(Self {
            step,
            state_hash,
            generation,
            selected_line,
            parameter_generation,
            audio_measurement,
            engine_selection: None,
            preset_selection: None,
            patch_adsr: None,
        })
    }

    pub fn with_engine_selection(mut self, observation: DemoEngineCheckpoint) -> Self {
        self.engine_selection = Some(observation);
        self
    }

    pub fn with_patch_adsr(mut self, observation: DemoPatchAdsrCheckpoint) -> Self {
        self.patch_adsr = Some(observation);
        self
    }

    pub fn with_preset_selection(mut self, observation: DemoPresetCheckpoint) -> Self {
        self.preset_selection = Some(observation);
        self
    }

    pub fn step(&self) -> &str {
        &self.step
    }

    pub fn state_hash(&self) -> &str {
        &self.state_hash
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub const fn selected_line(&self) -> usize {
        self.selected_line
    }

    pub const fn parameter_generation(&self) -> u64 {
        self.parameter_generation
    }

    pub const fn audio_measurement(&self) -> f64 {
        self.audio_measurement
    }

    pub const fn engine_selection(&self) -> Option<&DemoEngineCheckpoint> {
        self.engine_selection.as_ref()
    }

    pub const fn preset_selection(&self) -> Option<&DemoPresetCheckpoint> {
        self.preset_selection.as_ref()
    }

    pub const fn patch_adsr(&self) -> Option<&DemoPatchAdsrCheckpoint> {
        self.patch_adsr.as_ref()
    }
}

/// Exact descriptor, projection, lifecycle, and audio evidence for one preset checkpoint.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoPresetCheckpoint {
    patch_id: crate::kernel::PatchId,
    control_id: PatchControlId,
    parameter_id: ParameterId,
    status: EngineSelectionStatusKind,
    selected_choice_id: String,
    selected_label: String,
    requested_choice_id: Option<String>,
    requested_label: Option<String>,
    choices: Vec<ParameterChoice>,
    intent: Option<StructuralEditIntent>,
    request_id: Option<EngineSelectionRequestId>,
    state_graph_revision: GraphRevision,
    renderer_graph_revision: GraphRevision,
    failure: Option<EngineSelectionFailure>,
    target_patch_peak: f32,
    authored_order_exact: bool,
    focus_projection_exact: bool,
    assignment_delta_exact: bool,
    untargeted_patches_exact: bool,
}

impl DemoPresetCheckpoint {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        patch_id: crate::kernel::PatchId,
        parameter_id: ParameterId,
        status: EngineSelectionStatusKind,
        selected_choice_id: String,
        selected_label: String,
        requested_choice_id: Option<String>,
        requested_label: Option<String>,
        choices: Vec<ParameterChoice>,
        intent: Option<StructuralEditIntent>,
        request_id: Option<EngineSelectionRequestId>,
        state_graph_revision: GraphRevision,
        renderer_graph_revision: GraphRevision,
        failure: Option<EngineSelectionFailure>,
        target_patch_peak: f32,
        authored_order_exact: bool,
        focus_projection_exact: bool,
        assignment_delta_exact: bool,
        untargeted_patches_exact: bool,
    ) -> Result<Self, DemoSceneCheckpointError> {
        if !target_patch_peak.is_finite()
            || target_patch_peak < 0.0
            || !authored_order_exact
            || !focus_projection_exact
            || !assignment_delta_exact
            || !untargeted_patches_exact
            || state_graph_revision != renderer_graph_revision
        {
            return Err(DemoSceneCheckpointError::PresetMismatch);
        }
        Ok(Self {
            patch_id,
            control_id: PatchControlId::Capability(parameter_id.clone()),
            parameter_id,
            status,
            selected_choice_id,
            selected_label,
            requested_choice_id,
            requested_label,
            choices,
            intent,
            request_id,
            state_graph_revision,
            renderer_graph_revision,
            failure,
            target_patch_peak,
            authored_order_exact,
            focus_projection_exact,
            assignment_delta_exact,
            untargeted_patches_exact,
        })
    }

    pub const fn patch_id(&self) -> crate::kernel::PatchId {
        self.patch_id
    }

    pub fn control_id(&self) -> PatchControlId {
        self.control_id.clone()
    }

    pub const fn parameter_id(&self) -> &ParameterId {
        &self.parameter_id
    }

    pub const fn status(&self) -> EngineSelectionStatusKind {
        self.status
    }

    pub fn selected_choice_id(&self) -> &str {
        &self.selected_choice_id
    }

    pub fn selected_label(&self) -> &str {
        &self.selected_label
    }

    pub fn requested_choice_id(&self) -> Option<&str> {
        self.requested_choice_id.as_deref()
    }

    pub fn requested_label(&self) -> Option<&str> {
        self.requested_label.as_deref()
    }

    pub fn choices(&self) -> &[ParameterChoice] {
        &self.choices
    }

    pub const fn intent(&self) -> Option<&StructuralEditIntent> {
        self.intent.as_ref()
    }

    pub const fn request_id(&self) -> Option<EngineSelectionRequestId> {
        self.request_id
    }

    pub const fn state_graph_revision(&self) -> GraphRevision {
        self.state_graph_revision
    }

    pub const fn renderer_graph_revision(&self) -> GraphRevision {
        self.renderer_graph_revision
    }

    pub const fn failure(&self) -> Option<EngineSelectionFailure> {
        self.failure
    }

    pub const fn target_patch_peak(&self) -> f32 {
        self.target_patch_peak
    }

    pub const fn authored_order_exact(&self) -> bool {
        self.authored_order_exact
    }

    pub const fn assignment_delta_exact(&self) -> bool {
        self.assignment_delta_exact
    }

    pub const fn untargeted_patches_exact(&self) -> bool {
        self.untargeted_patches_exact
    }
}

/// Exact cross-projection evidence for one normalized PATCH ADSR checkpoint.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoPatchAdsrCheckpoint {
    patch_id: crate::kernel::PatchId,
    control_id: PatchControlId,
    parameter: VoiceEnvelopeParameter,
    expected_value: f32,
    state_value: f32,
    page_value: f32,
    snapshot_value: f32,
    renderer_value: f32,
    lifecycle: Option<EngineSelectionStatusKind>,
    state_graph_revision: GraphRevision,
    parameter_graph_revision: GraphRevision,
    renderer_graph_revision: GraphRevision,
    focus_projection_exact: bool,
    all_envelope_values_exact: bool,
    scalar_only: bool,
    untargeted_patches_exact: bool,
    audio_finite: bool,
}

impl DemoPatchAdsrCheckpoint {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        patch_id: crate::kernel::PatchId,
        parameter: VoiceEnvelopeParameter,
        expected_value: f32,
        state_value: f32,
        page_value: f32,
        snapshot_value: f32,
        renderer_value: f32,
        lifecycle: Option<EngineSelectionStatusKind>,
        state_graph_revision: GraphRevision,
        parameter_graph_revision: GraphRevision,
        renderer_graph_revision: GraphRevision,
        focus_projection_exact: bool,
        all_envelope_values_exact: bool,
        scalar_only: bool,
        untargeted_patches_exact: bool,
        audio_finite: bool,
    ) -> Result<Self, DemoSceneCheckpointError> {
        let values_exact = [state_value, page_value, snapshot_value, renderer_value]
            .into_iter()
            .all(|value| value == expected_value);
        if !expected_value.is_finite()
            || !values_exact
            || state_graph_revision != parameter_graph_revision
            || parameter_graph_revision != renderer_graph_revision
            || !focus_projection_exact
            || !all_envelope_values_exact
            || !scalar_only
            || !untargeted_patches_exact
            || !audio_finite
        {
            return Err(DemoSceneCheckpointError::PatchAdsrMismatch);
        }
        Ok(Self {
            patch_id,
            control_id: PatchControlId::Envelope(parameter),
            parameter,
            expected_value,
            state_value,
            page_value,
            snapshot_value,
            renderer_value,
            lifecycle,
            state_graph_revision,
            parameter_graph_revision,
            renderer_graph_revision,
            focus_projection_exact,
            all_envelope_values_exact,
            scalar_only,
            untargeted_patches_exact,
            audio_finite,
        })
    }

    pub const fn patch_id(&self) -> crate::kernel::PatchId {
        self.patch_id
    }

    pub fn control_id(&self) -> PatchControlId {
        self.control_id.clone()
    }

    pub const fn parameter(&self) -> VoiceEnvelopeParameter {
        self.parameter
    }

    pub const fn expected_value(&self) -> f32 {
        self.expected_value
    }

    pub const fn lifecycle(&self) -> Option<EngineSelectionStatusKind> {
        self.lifecycle
    }

    pub const fn focus_projection_exact(&self) -> bool {
        self.focus_projection_exact
    }

    pub const fn scalar_only(&self) -> bool {
        self.scalar_only
    }

    pub const fn all_envelope_values_exact(&self) -> bool {
        self.all_envelope_values_exact
    }

    pub const fn untargeted_patches_exact(&self) -> bool {
        self.untargeted_patches_exact
    }

    pub fn graph_revision_exact(&self) -> bool {
        self.state_graph_revision == self.parameter_graph_revision
            && self.parameter_graph_revision == self.renderer_graph_revision
    }

    pub const fn renderer_graph_revision(&self) -> GraphRevision {
        self.renderer_graph_revision
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DemoEngineCheckpoint {
    status: EngineSelectionStatusKind,
    active_capability_id: CapabilityId,
    requested_capability_id: Option<CapabilityId>,
    request_id: Option<EngineSelectionRequestId>,
    state_graph_revision: GraphRevision,
    renderer_graph_revision: GraphRevision,
    failure: Option<EngineSelectionFailure>,
    target_patch_peak: f32,
}

impl DemoEngineCheckpoint {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        status: EngineSelectionStatusKind,
        active_capability_id: CapabilityId,
        requested_capability_id: Option<CapabilityId>,
        request_id: Option<EngineSelectionRequestId>,
        state_graph_revision: GraphRevision,
        renderer_graph_revision: GraphRevision,
        failure: Option<EngineSelectionFailure>,
        target_patch_peak: f32,
    ) -> Result<Self, DemoSceneCheckpointError> {
        if !target_patch_peak.is_finite() || target_patch_peak < 0.0 {
            return Err(DemoSceneCheckpointError::NonFiniteAudioMeasurement);
        }
        Ok(Self {
            status,
            active_capability_id,
            requested_capability_id,
            request_id,
            state_graph_revision,
            renderer_graph_revision,
            failure,
            target_patch_peak,
        })
    }

    pub const fn status(&self) -> EngineSelectionStatusKind {
        self.status
    }

    pub const fn active_capability_id(&self) -> &CapabilityId {
        &self.active_capability_id
    }

    pub const fn requested_capability_id(&self) -> Option<&CapabilityId> {
        self.requested_capability_id.as_ref()
    }

    pub const fn request_id(&self) -> Option<EngineSelectionRequestId> {
        self.request_id
    }

    pub const fn state_graph_revision(&self) -> GraphRevision {
        self.state_graph_revision
    }

    pub const fn renderer_graph_revision(&self) -> GraphRevision {
        self.renderer_graph_revision
    }

    pub const fn failure(&self) -> Option<EngineSelectionFailure> {
        self.failure
    }

    pub const fn target_patch_peak(&self) -> f32 {
        self.target_patch_peak
    }
}

/// A structural contradiction that prevents a coherent demo report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DemoSceneReportError {
    EmptyScene,
    EmptyEventLog,
    EmptyCheckpoints,
    FinalEventStateMismatch,
    ReportSerialization,
}

impl fmt::Display for DemoSceneReportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::EmptyScene => formatter.write_str("demo scene name must not be empty"),
            Self::EmptyEventLog => {
                formatter.write_str("demo report requires at least one event record")
            }
            Self::EmptyCheckpoints => {
                formatter.write_str("demo report requires at least one checkpoint")
            }
            Self::FinalEventStateMismatch => formatter.write_str(
                "final state tree does not match the event journal generation and hash endpoint",
            ),
            Self::ReportSerialization => {
                formatter.write_str("demo scene report could not be serialized")
            }
        }
    }
}

impl std::error::Error for DemoSceneReportError {}

/// The complete machine-readable result of one exhaustive GUI demo run.
#[derive(Clone, Debug, PartialEq)]
pub struct DemoSceneReport {
    scene: String,
    complete: bool,
    coverage: DemoSceneCoverage,
    checkpoints: Vec<DemoSceneCheckpoint>,
    event_log: EventLog,
    final_state_tree: StateTree,
    audio_evidence: DemoAudioEvidence,
}

impl DemoSceneReport {
    /// Stable schema version for the top-level report.
    pub const SCHEMA_VERSION: u32 = 7;

    /// Packages a scene only after checking the journal/tree endpoint.
    ///
    /// Coverage gaps, dropped journal records, or inconsistent checkpoints
    /// produce a valid diagnostic report with complete=false. A final tree that
    /// is not the journal endpoint is rejected because it cannot describe the
    /// scene's final state.
    pub fn new(
        scene: impl Into<String>,
        coverage: DemoSceneCoverage,
        checkpoints: Vec<DemoSceneCheckpoint>,
        event_log: EventLog,
        final_state_tree: StateTree,
    ) -> Result<Self, DemoSceneReportError> {
        let scene = scene.into();
        if scene.trim().is_empty() {
            return Err(DemoSceneReportError::EmptyScene);
        }
        let endpoint = event_log
            .records()
            .back()
            .ok_or(DemoSceneReportError::EmptyEventLog)?;
        if checkpoints.is_empty() {
            return Err(DemoSceneReportError::EmptyCheckpoints);
        }
        if endpoint.generation_after() != final_state_tree.generation()
            || endpoint.parameter_generation() != final_state_tree.generation()
            || endpoint.state_hash_after() != final_state_tree.state_hash()
            || endpoint.projection_state_hash() != final_state_tree.state_hash()
            || endpoint.selected_line() != final_state_tree.selected_line()
        {
            return Err(DemoSceneReportError::FinalEventStateMismatch);
        }

        let checkpoints_agree =
            checkpoint_chain_agrees(&checkpoints, &event_log, &final_state_tree);
        let complete = coverage.is_complete()
            && event_log.coverage().is_complete()
            && event_log.dropped_records() == 0
            && event_log.total_observed() == event_log.records().len() as u64
            && checkpoints_agree;

        Ok(Self {
            scene,
            complete,
            coverage,
            checkpoints,
            event_log,
            final_state_tree,
            audio_evidence: DemoAudioEvidence::default(),
        })
    }

    pub const fn schema_version(&self) -> u32 {
        Self::SCHEMA_VERSION
    }

    pub fn scene(&self) -> &str {
        &self.scene
    }

    pub const fn is_complete(&self) -> bool {
        self.complete
    }

    pub const fn coverage(&self) -> &DemoSceneCoverage {
        &self.coverage
    }

    pub fn checkpoints(&self) -> &[DemoSceneCheckpoint] {
        &self.checkpoints
    }

    pub const fn event_log(&self) -> &EventLog {
        &self.event_log
    }

    pub const fn final_state_tree(&self) -> &StateTree {
        &self.final_state_tree
    }

    pub const fn audio_evidence(&self) -> DemoAudioEvidence {
        self.audio_evidence
    }

    pub fn with_audio_evidence(mut self, audio_evidence: DemoAudioEvidence) -> Self {
        self.audio_evidence = audio_evidence;
        self
    }

    /// Serializes stable schema fields without timestamps, paths, or maps.
    pub fn to_json(&self) -> Result<String, DemoSceneReportError> {
        serde_json::to_string(self).map_err(|_| DemoSceneReportError::ReportSerialization)
    }
}

fn checkpoint_chain_agrees(
    checkpoints: &[DemoSceneCheckpoint],
    event_log: &EventLog,
    final_state_tree: &StateTree,
) -> bool {
    let ordered = checkpoints.windows(2).all(|pair| {
        pair[0].generation() <= pair[1].generation()
            && (pair[0].generation() != pair[1].generation()
                || pair[0].state_hash() == pair[1].state_hash())
    });

    let final_checkpoint_matches = checkpoints.last().is_some_and(|checkpoint| {
        checkpoint.generation() == final_state_tree.generation()
            && checkpoint.parameter_generation() == final_state_tree.generation()
            && checkpoint.state_hash() == final_state_tree.state_hash()
            && checkpoint.selected_line() == final_state_tree.selected_line()
    });

    let retained_records_match = event_log.dropped_records() != 0
        || checkpoints.iter().all(|checkpoint| {
            event_log.records().iter().any(|record| {
                checkpoint.generation() == record.generation_after()
                    && checkpoint.parameter_generation() == record.parameter_generation()
                    && checkpoint.state_hash() == record.state_hash_after()
                    && checkpoint.selected_line() == record.selected_line()
            })
        });

    ordered && final_checkpoint_matches && retained_records_match
}

impl Serialize for DemoSceneReport {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let final_state_tree: serde_json::Value =
            serde_json::from_str(self.final_state_tree.json())
                .map_err(serde::ser::Error::custom)?;

        let mut report = serializer.serialize_struct("DemoSceneReport", 8)?;
        report.serialize_field("schemaVersion", &Self::SCHEMA_VERSION)?;
        report.serialize_field("scene", &self.scene)?;
        report.serialize_field("complete", &self.complete)?;
        report.serialize_field("coverage", &self.coverage)?;
        report.serialize_field("checkpoints", &self.checkpoints)?;
        report.serialize_field("eventLog", &self.event_log)?;
        report.serialize_field("finalStateTree", &final_state_tree)?;
        report.serialize_field("audioEvidence", &self.audio_evidence)?;
        report.end()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DemoCoverageGroup, DemoSceneCheckpoint, DemoSceneCoverage, DemoSceneReport,
        DemoSceneReportError,
    };
    use crate::control::app_event::{AppEvent, Direction};
    use crate::control::app_state::{AppState, EventRejection};
    use crate::control::event_log::{EventCoverage, EventLog};
    use crate::control::event_record::{EventRecord, EventSource};
    use crate::control::state_projector::StateProjector;
    use crate::control::state_tree::StateTree;
    use crate::control::text_projection::TextProjection;
    use crate::kernel::midi_channel::MidiChannel;
    use crate::kernel::patch_id::PatchId;
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::GraphRevision;
    use crate::synth::patch::Patch;
    use crate::synth::sound_font_instrument::SoundFontInstrument;
    use crate::testing::automatic_midi_test::create_soundfont_config;

    fn tree_and_projection() -> (StateTree, TextProjection) {
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        let global = GlobalParameters::new(-3.0).unwrap();
        let mut state =
            AppState::for_graph(provider.registry().unwrap(), global, GraphRevision::INITIAL);
        let patches = vec![
            Patch::new(
                PatchId::new(7).unwrap(),
                "Lead".to_owned(),
                create_soundfont_config(&provider, SoundFontInstrument::new(0, 80, false).unwrap())
                    .unwrap(),
                MidiChannel::new(2).unwrap(),
                PatchOutput::new(MixerTrackId::new(2).unwrap(), -6.0).unwrap(),
            ),
            Patch::new(
                PatchId::new(9).unwrap(),
                "Drums".to_owned(),
                create_soundfont_config(&provider, SoundFontInstrument::new(128, 0, true).unwrap())
                    .unwrap(),
                MidiChannel::new(9).unwrap(),
                PatchOutput::new(MixerTrackId::new(9).unwrap(), -12.0).unwrap(),
            ),
        ];
        state.apply(AppEvent::InstallPatches(patches)).unwrap();
        for _ in 0..3 {
            state
                .apply(AppEvent::SelectContext(
                    crate::control::TopLevelContext::Mixer,
                ))
                .unwrap();
        }
        let (_, _, projection, _, tree) = StateProjector::for_graph(GraphRevision::INITIAL)
            .project_with_tree(&state)
            .unwrap();
        assert_eq!(tree.generation(), 4);
        (tree, projection)
    }

    fn event_log(projection: &TextProjection, capacity: usize, count: u64) -> EventLog {
        let mut log =
            EventLog::with_coverage(capacity, EventCoverage::new(["event.adjust.right"])).unwrap();
        log.mark_exercised("event.adjust.right");

        for sequence in 0..count {
            let record = EventRecord::rejected(
                sequence,
                EventSource::DemoScene,
                &AppEvent::Adjust(Direction::Right),
                4,
                projection.state_hash(),
                4,
                projection,
                EventRejection::ParameterAtBoundary,
            )
            .unwrap();
            log.append(record).unwrap();
        }
        log
    }

    fn coverage(complete: bool) -> DemoSceneCoverage {
        let mut coverage = DemoSceneCoverage::new();
        coverage.declare_expected(DemoCoverageGroup::Events, ["adjust.right"]);
        if complete {
            coverage.mark_exercised(DemoCoverageGroup::Events, "adjust.right");
        }
        coverage
    }

    fn checkpoint(tree: &StateTree) -> DemoSceneCheckpoint {
        DemoSceneCheckpoint::new(
            "boundary-rejection",
            tree.state_hash(),
            tree.generation(),
            tree.selected_line(),
            tree.generation(),
            0.125,
        )
        .unwrap()
    }

    #[test]
    fn complete_report_embeds_stable_grouped_trace_objects() {
        let (tree, projection) = tree_and_projection();
        let report = DemoSceneReport::new(
            "exhaustive-gui",
            coverage(true),
            vec![checkpoint(&tree)],
            event_log(&projection, 2, 1),
            tree,
        )
        .unwrap();

        assert!(report.is_complete());
        assert_eq!(report.coverage().missing_count(), 0);
        assert_eq!(report.event_log().dropped_records(), 0);

        let first = report.to_json().unwrap();
        let second = report.to_json().unwrap();
        let json: serde_json::Value = serde_json::from_str(&first).unwrap();

        assert_eq!(first, second);
        assert_eq!(json["schemaVersion"], 7);
        assert_eq!(json["scene"], "exhaustive-gui");
        assert_eq!(json["complete"], true);
        assert_eq!(json["coverage"]["events"]["missing"], serde_json::json!([]));
        assert_eq!(json["checkpoints"][0]["audioMeasurement"], 0.125);
        assert_eq!(json["eventLog"]["records"].as_array().unwrap().len(), 1);
        assert_eq!(json["finalStateTree"]["generation"], 4);
        assert_eq!(
            json["finalStateTree"]["patches"].as_array().unwrap().len(),
            2
        );
    }

    #[test]
    fn missing_coverage_or_dropped_history_can_never_claim_completion() {
        let (tree, projection) = tree_and_projection();
        let report = DemoSceneReport::new(
            "diagnostic",
            coverage(false),
            vec![checkpoint(&tree)],
            event_log(&projection, 1, 2),
            tree,
        )
        .unwrap();

        assert!(!report.is_complete());
        assert_eq!(report.coverage().missing_count(), 1);
        assert_eq!(report.event_log().dropped_records(), 1);
    }

    #[test]
    fn rejects_a_final_tree_that_is_not_the_event_endpoint() {
        let (tree, projection) = tree_and_projection();
        let mismatched = EventRecord::rejected(
            0,
            EventSource::DemoScene,
            &AppEvent::Adjust(Direction::Right),
            3,
            projection.state_hash(),
            3,
            &projection,
            EventRejection::ParameterAtBoundary,
        )
        .unwrap();
        let mut log = EventLog::new(1).unwrap();
        log.append(mismatched).unwrap();

        assert_eq!(
            DemoSceneReport::new(
                "mismatch",
                coverage(true),
                vec![checkpoint(&tree)],
                log,
                tree,
            ),
            Err(DemoSceneReportError::FinalEventStateMismatch)
        );
    }
}
