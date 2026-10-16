use crate::control::{EngineSelectionFailure, EngineSelectionRequestId, StructuralEditIntent};
use crate::kernel::PatchId;
use crate::mixer::bus_return::BusReturnBank;
use crate::mixer::global_parameters::GlobalParameters;
use crate::mixer::mixer_state::MixerState;
use crate::real_time::{
    AuditionPreparationRequest, GraphPreparationError, GraphRevision, ParameterSnapshot,
    PreparedGraph, PreparedGraphBuilder,
};
use crate::shell::audio_output::AudioDeviceConfig;
use crate::synth::effect_slot_id::MAX_EFFECT_SLOTS;
use crate::synth::{
    CapabilityId, CapabilityRegistry, EffectCapabilityError, EffectCapabilityRegistry,
    EffectPreparer, InstrumentConfig, InstrumentPreparationError, InstrumentPreparer, Patch,
    PostEffectConfig, RackPreparationError,
};
use core::fmt;

/// Validates one Patch's canonical per-position effect chain directly against
/// the installed registry: every occupied position must satisfy its
/// descriptor, and no two positions may claim the same instance identity.
/// Position-direct — the gapped view is never compacted into a transitional
/// list, so an empty position stays a validated gap.
fn validate_patch_effect_slots(
    effects: &EffectCapabilityRegistry,
    slots: &[Option<PostEffectConfig>; MAX_EFFECT_SLOTS],
) -> Result<(), EffectCapabilityError> {
    for (position, occupant) in slots.iter().enumerate() {
        let Some(config) = occupant else {
            continue;
        };
        if slots[..position]
            .iter()
            .flatten()
            .any(|prior| prior.slot_id() == config.slot_id())
        {
            return Err(EffectCapabilityError::DuplicateSlot(config.slot_id()));
        }
        effects.validate_config(config)?;
    }
    Ok(())
}

/// Complete immutable correlation shared by one worker request and result.
///
/// Instrument intents carry the full Patch and capability context; occupancy
/// intents carry their position in the intent, so a return edit has no Patch
/// context and nothing is fabricated to stand in for one.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphPreparationCorrelation {
    request_id: EngineSelectionRequestId,
    patch_id: Option<PatchId>,
    intent: StructuralEditIntent,
    source_capability_id: Option<CapabilityId>,
    target_capability_id: Option<CapabilityId>,
    source_graph_revision: GraphRevision,
    target_graph_revision: GraphRevision,
}

impl GraphPreparationCorrelation {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        request_id: EngineSelectionRequestId,
        patch_id: PatchId,
        source_capability_id: CapabilityId,
        target_capability_id: CapabilityId,
        source_graph_revision: GraphRevision,
        target_graph_revision: GraphRevision,
    ) -> Result<Self, GraphPreparationRequestError> {
        if source_capability_id == target_capability_id {
            return Err(GraphPreparationRequestError::CapabilityUnchanged);
        }
        let intent = StructuralEditIntent::ReplaceCapability {
            target_capability_id: target_capability_id.clone(),
        };
        Self::new_with_intent(
            request_id,
            patch_id,
            intent,
            source_capability_id,
            target_capability_id,
            source_graph_revision,
            target_graph_revision,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_intent(
        request_id: EngineSelectionRequestId,
        patch_id: PatchId,
        intent: StructuralEditIntent,
        source_capability_id: CapabilityId,
        target_capability_id: CapabilityId,
        source_graph_revision: GraphRevision,
        target_graph_revision: GraphRevision,
    ) -> Result<Self, GraphPreparationRequestError> {
        Self::new_with_context(
            request_id,
            Some(patch_id),
            intent,
            Some(source_capability_id),
            Some(target_capability_id),
            source_graph_revision,
            target_graph_revision,
        )
    }

    /// Creates one occupancy-change correlation; the intent itself names the
    /// position (and, for a slot change, the Patch).
    pub fn for_occupancy(
        request_id: EngineSelectionRequestId,
        intent: StructuralEditIntent,
        source_graph_revision: GraphRevision,
        target_graph_revision: GraphRevision,
    ) -> Result<Self, GraphPreparationRequestError> {
        let patch_id = match &intent {
            StructuralEditIntent::SetVoiceBudget { patch_id, .. } => Some(*patch_id),
            StructuralEditIntent::ReplaceEffectAsset { target, .. } => target.patch_id(),
            StructuralEditIntent::SetSlotOccupancy { patch_id, .. } => Some(*patch_id),
            StructuralEditIntent::SetReturnOccupancy { .. }
            | StructuralEditIntent::SetSendEffect { .. } => None,
            _ => return Err(GraphPreparationRequestError::IntentMismatch),
        };
        Self::new_with_context(
            request_id,
            patch_id,
            intent,
            None,
            None,
            source_graph_revision,
            target_graph_revision,
        )
    }

    pub fn for_append(
        request_id: EngineSelectionRequestId,
        patch_id: PatchId,
        source_graph_revision: GraphRevision,
        target_graph_revision: GraphRevision,
    ) -> Result<Self, GraphPreparationRequestError> {
        Self::new_with_context(
            request_id,
            Some(patch_id),
            StructuralEditIntent::AppendPatch { patch_id },
            None,
            None,
            source_graph_revision,
            target_graph_revision,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_context(
        request_id: EngineSelectionRequestId,
        patch_id: Option<PatchId>,
        intent: StructuralEditIntent,
        source_capability_id: Option<CapabilityId>,
        target_capability_id: Option<CapabilityId>,
        source_graph_revision: GraphRevision,
        target_graph_revision: GraphRevision,
    ) -> Result<Self, GraphPreparationRequestError> {
        if request_id.is_none() {
            return Err(GraphPreparationRequestError::MissingRequestIdentity);
        }
        if !intent.matches_context(
            patch_id,
            source_capability_id.as_ref(),
            target_capability_id.as_ref(),
        ) {
            return Err(GraphPreparationRequestError::IntentMismatch);
        }
        if target_graph_revision <= source_graph_revision {
            return Err(GraphPreparationRequestError::TargetRevisionNotNewer);
        }
        Ok(Self {
            request_id,
            patch_id,
            intent,
            source_capability_id,
            target_capability_id,
            source_graph_revision,
            target_graph_revision,
        })
    }

    pub const fn request_id(&self) -> EngineSelectionRequestId {
        self.request_id
    }

    pub const fn patch_id(&self) -> Option<PatchId> {
        self.patch_id
    }

    pub const fn intent(&self) -> &StructuralEditIntent {
        &self.intent
    }

    pub const fn source_capability_id(&self) -> Option<&CapabilityId> {
        self.source_capability_id.as_ref()
    }

    pub const fn target_capability_id(&self) -> Option<&CapabilityId> {
        self.target_capability_id.as_ref()
    }

    pub const fn source_graph_revision(&self) -> GraphRevision {
        self.source_graph_revision
    }

    pub const fn target_graph_revision(&self) -> GraphRevision {
        self.target_graph_revision
    }

    /// Derives the one scoped delta this correlation declares — the same
    /// vocabulary the coordinator admits and the renderer's voice carry-over
    /// honors, derived in exactly one place so admission and carry-over can
    /// never disagree.
    pub fn replacement_scope(&self) -> Option<crate::real_time::GraphReplacementScope> {
        match &self.intent {
            StructuralEditIntent::SetVoiceBudget { patch_id, .. } => Some(
                crate::real_time::GraphReplacementScope::SelectedEngine(*patch_id),
            ),
            StructuralEditIntent::ReplaceEffectAsset { target, .. } => Some(match target {
                crate::control::EffectAssetTarget::PatchSlot { patch_id, slot } => {
                    crate::real_time::GraphReplacementScope::PatchSlot {
                        patch_id: *patch_id,
                        slot: *slot,
                    }
                }
                crate::control::EffectAssetTarget::BusReturn { bus }
                | crate::control::EffectAssetTarget::SendSlot { bus, .. } => {
                    crate::real_time::GraphReplacementScope::BusReturn(*bus)
                }
            }),
            StructuralEditIntent::ReplaceCapability { .. }
            | StructuralEditIntent::ReplaceParameterChoice { .. }
            | StructuralEditIntent::ReplaceAsset { .. } => Some(
                crate::real_time::GraphReplacementScope::SelectedEngine(self.patch_id?),
            ),
            StructuralEditIntent::PrepareAudition { .. } => {
                Some(crate::real_time::GraphReplacementScope::Audition)
            }
            StructuralEditIntent::SetSlotOccupancy { patch_id, slot, .. } => {
                Some(crate::real_time::GraphReplacementScope::PatchSlot {
                    patch_id: *patch_id,
                    slot: *slot,
                })
            }
            StructuralEditIntent::SetReturnOccupancy { bus, .. }
            | StructuralEditIntent::SetSendEffect { bus, .. } => {
                Some(crate::real_time::GraphReplacementScope::BusReturn(*bus))
            }
            StructuralEditIntent::AppendPatch { patch_id } => Some(
                crate::real_time::GraphReplacementScope::AppendPatch(*patch_id),
            ),
        }
    }
}

/// One frozen off-callback request for a complete replacement graph.
///
/// The request carries the complete candidate topology — every Patch with its
/// ordered effect slots plus the complete bus-return bank — never a partial
/// delta, so the worker always builds and exchanges a whole graph (FR-012).
#[derive(Clone, Debug, PartialEq)]
pub struct GraphPreparationRequest {
    correlation: GraphPreparationCorrelation,
    candidate_patches: Vec<Patch>,
    candidate_returns: BusReturnBank,
    candidate_parameters: ParameterSnapshot,
    /// Transient preview configuration prepared into the graph-owned
    /// audition slot. It is intentionally separate from `candidate_patches`:
    /// previewing a file never changes the complete canonical Patch topology.
    audition_candidate: Option<InstrumentConfig>,
    audio_config: AudioDeviceConfig,
}

impl GraphPreparationRequest {
    /// Replaces exactly one selected config while copying all other Patch data.
    #[allow(clippy::too_many_arguments)]
    pub fn replacement(
        correlation: GraphPreparationCorrelation,
        active_patches: &[Patch],
        candidate_config: InstrumentConfig,
        generation: u64,
        global: GlobalParameters,
        mixer: MixerState,
        audio_config: AudioDeviceConfig,
        registry: &CapabilityRegistry,
    ) -> Result<Self, GraphPreparationRequestError> {
        let effects = EffectCapabilityRegistry::default();
        Self::replacement_with_effects(
            correlation,
            active_patches,
            candidate_config,
            generation,
            global,
            mixer,
            audio_config,
            registry,
            &effects,
            &BusReturnBank::default(),
        )
    }

    /// Replaces exactly one instrument config while preserving and validating
    /// every Patch-local post-effect config, the complete bus-return bank,
    /// and the fixed scalar layout.
    #[allow(clippy::too_many_arguments)]
    pub fn replacement_with_effects(
        correlation: GraphPreparationCorrelation,
        active_patches: &[Patch],
        candidate_config: InstrumentConfig,
        generation: u64,
        global: GlobalParameters,
        mixer: MixerState,
        audio_config: AudioDeviceConfig,
        registry: &CapabilityRegistry,
        effects: &EffectCapabilityRegistry,
        returns: &BusReturnBank,
    ) -> Result<Self, GraphPreparationRequestError> {
        if active_patches.len() > crate::real_time::MAX_ACTIVE_PATCHES {
            return Err(GraphPreparationRequestError::PatchCapacityExceeded);
        }
        for (index, patch) in active_patches.iter().enumerate() {
            if active_patches[..index]
                .iter()
                .any(|prior| prior.id() == patch.id())
            {
                return Err(GraphPreparationRequestError::DuplicatePatchId);
            }
            registry
                .validate_config(patch.instrument_config())
                .map_err(|_| GraphPreparationRequestError::InvalidActiveConfig)?;
            validate_patch_effect_slots(effects, patch.effect_slots())
                .map_err(|_| GraphPreparationRequestError::InvalidActiveEffectConfig)?;
        }
        let selected_index = active_patches
            .iter()
            .position(|patch| Some(patch.id()) == correlation.patch_id())
            .ok_or(GraphPreparationRequestError::UnknownPatch)?;
        if Some(
            active_patches[selected_index]
                .instrument_config()
                .capability_id(),
        ) != correlation.source_capability_id()
        {
            return Err(GraphPreparationRequestError::SourceCapabilityMismatch);
        }
        if Some(candidate_config.capability_id()) != correlation.target_capability_id() {
            return Err(GraphPreparationRequestError::TargetCapabilityMismatch);
        }
        registry
            .validate_config(&candidate_config)
            .map_err(|_| GraphPreparationRequestError::InvalidCandidateConfig)?;
        validate_candidate_delta(
            active_patches[selected_index].instrument_config(),
            &candidate_config,
            correlation.intent(),
            registry,
        )?;

        let audition = matches!(
            correlation.intent(),
            StructuralEditIntent::PrepareAudition { .. }
        );
        let mut candidate_patches = active_patches.to_vec();
        let audition_candidate = if audition {
            Some(candidate_config)
        } else {
            candidate_patches[selected_index].set_instrument_config(candidate_config);
            None
        };
        let candidate_parameters = ParameterSnapshot::project_patches_with_effects_and_returns(
            generation,
            correlation.target_graph_revision(),
            global,
            mixer,
            &candidate_patches,
            registry,
            effects,
            returns,
        )
        .map_err(|_| GraphPreparationRequestError::InvalidCandidateConfig)?;

        Ok(Self {
            correlation,
            candidate_patches,
            candidate_returns: returns.clone(),
            candidate_parameters,
            audition_candidate,
            audio_config,
        })
    }

    /// Applies exactly one occupancy delta — a Patch effect slot or a bus
    /// return — to the complete active topology, validating everything before
    /// anything can be published (FR-013). The unresolvable and the invalid
    /// are refused, never substituted; every other Patch, slot, and return is
    /// copied unchanged so the worker prepares one complete candidate graph.
    #[allow(clippy::too_many_arguments)]
    pub fn occupancy(
        correlation: GraphPreparationCorrelation,
        active_patches: &[Patch],
        active_returns: &BusReturnBank,
        generation: u64,
        global: GlobalParameters,
        mixer: MixerState,
        audio_config: AudioDeviceConfig,
        registry: &CapabilityRegistry,
        effects: &EffectCapabilityRegistry,
    ) -> Result<Self, GraphPreparationRequestError> {
        if active_patches.len() > crate::real_time::MAX_ACTIVE_PATCHES {
            return Err(GraphPreparationRequestError::PatchCapacityExceeded);
        }
        for (index, patch) in active_patches.iter().enumerate() {
            if active_patches[..index]
                .iter()
                .any(|prior| prior.id() == patch.id())
            {
                return Err(GraphPreparationRequestError::DuplicatePatchId);
            }
            registry
                .validate_config(patch.instrument_config())
                .map_err(|_| GraphPreparationRequestError::InvalidActiveConfig)?;
            validate_patch_effect_slots(effects, patch.effect_slots())
                .map_err(|_| GraphPreparationRequestError::InvalidActiveEffectConfig)?;
        }

        let mut candidate_patches = active_patches.to_vec();
        let mut candidate_returns = active_returns.clone();
        match correlation.intent() {
            StructuralEditIntent::SetVoiceBudget { patch_id, voices } => {
                let patch = candidate_patches
                    .iter_mut()
                    .find(|p| p.id() == *patch_id)
                    .ok_or(GraphPreparationRequestError::UnknownPatch)?;
                if *voices
                    > registry
                        .descriptor_for_config(patch.instrument_config())
                        .ok_or(GraphPreparationRequestError::InvalidActiveConfig)?
                        .voice_policy()
                        .polyphony_ceiling()
                {
                    return Err(GraphPreparationRequestError::InvalidOccupancy);
                }
                patch
                    .set_voice_limit(*voices)
                    .map_err(|_| GraphPreparationRequestError::InvalidOccupancy)?;
            }
            StructuralEditIntent::ReplaceEffectAsset {
                target,
                parameter_id,
                reference,
            } => {
                target
                    .assign(
                        &mut candidate_patches,
                        &mut candidate_returns,
                        effects,
                        parameter_id,
                        reference.clone(),
                    )
                    .map_err(|_| GraphPreparationRequestError::InvalidOccupancy)?;
            }
            StructuralEditIntent::SetSlotOccupancy {
                patch_id,
                slot,
                entry,
            } => {
                let selected_index = candidate_patches
                    .iter()
                    .position(|patch| patch.id() == *patch_id)
                    .ok_or(GraphPreparationRequestError::UnknownPatch)?;
                let occupant = build_slot_occupant(effects, *slot, entry.as_ref())?;
                candidate_patches[selected_index]
                    .set_slot_occupancy(*slot, occupant)
                    .map_err(|_| GraphPreparationRequestError::InvalidOccupancy)?;
            }
            StructuralEditIntent::SetSendEffect {
                bus,
                slot_id,
                entry,
            } => {
                candidate_returns
                    .set_effect_slot(effects, *bus, *slot_id, entry.as_ref())
                    .map_err(|error| match error {
                        crate::mixer::bus_return::BusReturnError::UnknownRegistryEntry {
                            ..
                        } => GraphPreparationRequestError::UnknownEffectEntry,
                        _ => GraphPreparationRequestError::InvalidOccupancy,
                    })?;
            }
            StructuralEditIntent::SetReturnOccupancy { bus, entry } => {
                candidate_returns
                    .set_return_occupancy(effects, *bus, entry.as_ref())
                    .map_err(|error| match error {
                        crate::mixer::bus_return::BusReturnError::UnknownRegistryEntry {
                            ..
                        } => GraphPreparationRequestError::UnknownEffectEntry,
                        _ => GraphPreparationRequestError::InvalidOccupancy,
                    })?;
            }
            StructuralEditIntent::ReplaceCapability { .. }
            | StructuralEditIntent::ReplaceParameterChoice { .. }
            | StructuralEditIntent::ReplaceAsset { .. }
            | StructuralEditIntent::PrepareAudition { .. }
            | StructuralEditIntent::AppendPatch { .. } => {
                return Err(GraphPreparationRequestError::IntentMismatch)
            }
        }

        let candidate_parameters = ParameterSnapshot::project_patches_with_effects_and_returns(
            generation,
            correlation.target_graph_revision(),
            global,
            mixer,
            &candidate_patches,
            registry,
            effects,
            &candidate_returns,
        )
        .map_err(|_| GraphPreparationRequestError::InvalidCandidateConfig)?;

        Ok(Self {
            correlation,
            candidate_patches,
            candidate_returns,
            candidate_parameters,
            audition_candidate: None,
            audio_config,
        })
    }

    /// Appends one complete candidate after the exact active order and
    /// projects the full target-revision snapshot before worker submission.
    #[allow(clippy::too_many_arguments)]
    pub fn append_patch(
        correlation: GraphPreparationCorrelation,
        active_patches: &[Patch],
        candidate: Patch,
        active_returns: &BusReturnBank,
        generation: u64,
        global: GlobalParameters,
        mixer: MixerState,
        audio_config: AudioDeviceConfig,
        registry: &CapabilityRegistry,
        effects: &EffectCapabilityRegistry,
    ) -> Result<Self, GraphPreparationRequestError> {
        if active_patches.len() >= crate::real_time::MAX_ACTIVE_PATCHES {
            return Err(GraphPreparationRequestError::PatchCapacityExceeded);
        }
        let candidate_id = correlation
            .patch_id()
            .ok_or(GraphPreparationRequestError::IntentMismatch)?;
        if correlation.intent()
            != &(StructuralEditIntent::AppendPatch {
                patch_id: candidate_id,
            })
            || candidate.id() != candidate_id
        {
            return Err(GraphPreparationRequestError::IntentMismatch);
        }
        for (index, patch) in active_patches.iter().enumerate() {
            if active_patches[..index]
                .iter()
                .any(|prior| prior.id() == patch.id())
                || patch.id() == candidate_id
            {
                return Err(GraphPreparationRequestError::DuplicatePatchId);
            }
            registry
                .validate_config(patch.instrument_config())
                .map_err(|_| GraphPreparationRequestError::InvalidActiveConfig)?;
            validate_patch_effect_slots(effects, patch.effect_slots())
                .map_err(|_| GraphPreparationRequestError::InvalidActiveEffectConfig)?;
        }
        registry
            .validate_config(candidate.instrument_config())
            .map_err(|_| GraphPreparationRequestError::InvalidCandidateConfig)?;
        validate_patch_effect_slots(effects, candidate.effect_slots())
            .map_err(|_| GraphPreparationRequestError::InvalidCandidateConfig)?;

        let mut candidate_patches = Vec::with_capacity(active_patches.len() + 1);
        candidate_patches.extend_from_slice(active_patches);
        candidate_patches.push(candidate);
        let candidate_parameters = ParameterSnapshot::project_patches_with_effects_and_returns(
            generation,
            correlation.target_graph_revision(),
            global,
            mixer,
            &candidate_patches,
            registry,
            effects,
            active_returns,
        )
        .map_err(|_| GraphPreparationRequestError::InvalidCandidateConfig)?;
        Ok(Self {
            correlation,
            candidate_patches,
            candidate_returns: active_returns.clone(),
            candidate_parameters,
            audition_candidate: None,
            audio_config,
        })
    }

    pub const fn correlation(&self) -> &GraphPreparationCorrelation {
        &self.correlation
    }

    pub fn candidate_patches(&self) -> &[Patch] {
        &self.candidate_patches
    }

    /// Returns the complete candidate bus-return bank this request prepares.
    pub const fn candidate_returns(&self) -> &BusReturnBank {
        &self.candidate_returns
    }

    pub const fn candidate_parameters(&self) -> &ParameterSnapshot {
        &self.candidate_parameters
    }

    pub const fn audition_candidate(&self) -> Option<&InstrumentConfig> {
        self.audition_candidate.as_ref()
    }

    pub const fn audio_config(&self) -> AudioDeviceConfig {
        self.audio_config
    }

    /// Returns the selected candidate instrument config for instrument
    /// intents; occupancy intents change no instrument config.
    pub fn candidate_config(&self) -> Option<&InstrumentConfig> {
        if self.correlation.intent().uses_topology_events() {
            return None;
        }
        if let Some(candidate) = self.audition_candidate.as_ref() {
            return Some(candidate);
        }
        self.candidate_patches
            .iter()
            .find(|patch| Some(patch.id()) == self.correlation.patch_id())
            .map(Patch::instrument_config)
    }

    fn validate_for_worker(
        &self,
        registry: &CapabilityRegistry,
        effects: &EffectCapabilityRegistry,
        audio_config: AudioDeviceConfig,
    ) -> Result<(), EngineSelectionFailure> {
        if self.audio_config != audio_config {
            return Err(EngineSelectionFailure::UnsupportedAudioConfig);
        }
        if let Some(patch_id) = self.correlation.patch_id() {
            let selected = self
                .candidate_patches
                .iter()
                .find(|patch| patch.id() == patch_id)
                .ok_or(EngineSelectionFailure::GraphIncompatible)?;
            if !self.correlation.intent().uses_topology_events()
                && Some(selected.instrument_config().capability_id())
                    != self.correlation.target_capability_id()
            {
                return Err(EngineSelectionFailure::ProviderMismatch);
            }
        }
        if self
            .candidate_patches
            .iter()
            .any(|patch| registry.validate_config(patch.instrument_config()).is_err())
        {
            return Err(EngineSelectionFailure::InvalidDefaultConfig);
        }
        if self
            .candidate_patches
            .iter()
            .any(|patch| validate_patch_effect_slots(effects, patch.effect_slots()).is_err())
        {
            return Err(EngineSelectionFailure::InvalidDefaultConfig);
        }
        let expected = ParameterSnapshot::project_patches_with_effects_and_returns(
            self.candidate_parameters.generation(),
            self.correlation.target_graph_revision(),
            *self.candidate_parameters.global(),
            MixerState::new(self.candidate_parameters.mixer_tracks().clone()),
            &self.candidate_patches,
            registry,
            effects,
            &self.candidate_returns,
        )
        .map_err(|_| EngineSelectionFailure::InvalidDefaultConfig)?;
        if expected != self.candidate_parameters {
            return Err(EngineSelectionFailure::GraphIncompatible);
        }
        Ok(())
    }
}

/// One ownership-bearing result. PreparedGraph never enters AppState.
#[derive(Debug)]
pub enum GraphPreparationResult {
    Prepared {
        correlation: GraphPreparationCorrelation,
        /// The prepared candidate instrument config for instrument intents;
        /// occupancy intents change no instrument config. The reducer keeps
        /// it pending until activation acknowledgement.
        candidate_config: Option<InstrumentConfig>,
        prepared_visualization: Option<Vec<crate::synth::PreparedSampleVisualization>>,
        prepared_graph: PreparedGraph,
    },
    Failed {
        correlation: GraphPreparationCorrelation,
        failure: EngineSelectionFailure,
    },
}

impl GraphPreparationResult {
    pub const fn correlation(&self) -> &GraphPreparationCorrelation {
        match self {
            Self::Prepared { correlation, .. } | Self::Failed { correlation, .. } => correlation,
        }
    }

    pub const fn failure(&self) -> Option<EngineSelectionFailure> {
        match self {
            Self::Prepared { .. } => None,
            Self::Failed { failure, .. } => Some(*failure),
        }
    }

    pub const fn prepared_graph(&self) -> Option<&PreparedGraph> {
        match self {
            Self::Prepared { prepared_graph, .. } => Some(prepared_graph),
            Self::Failed { .. } => None,
        }
    }
}

/// Nonblocking control-facing worker port.
pub trait GraphPreparationWorker {
    fn try_submit(&mut self, request: GraphPreparationRequest) -> Result<(), WorkerBusy>;
    fn try_poll(&mut self) -> Option<GraphPreparationResult>;
    fn shutdown_on_control(&mut self) -> Result<(), WorkerShutdownError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerBusyReason {
    OutstandingRequest,
    WorkerUnavailable,
    Shutdown,
}

/// A rejected nonblocking submission that preserves the complete request.
#[must_use = "the rejected preparation request remains owned by control"]
#[derive(Debug)]
pub struct WorkerBusy {
    reason: WorkerBusyReason,
    request: Box<GraphPreparationRequest>,
}

impl WorkerBusy {
    pub(crate) fn new(reason: WorkerBusyReason, request: GraphPreparationRequest) -> Self {
        Self {
            reason,
            request: Box::new(request),
        }
    }

    pub const fn reason(&self) -> WorkerBusyReason {
        self.reason
    }

    pub const fn request(&self) -> &GraphPreparationRequest {
        &self.request
    }

    pub fn into_request(self) -> GraphPreparationRequest {
        *self.request
    }
}

impl fmt::Display for WorkerBusy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.reason {
            WorkerBusyReason::OutstandingRequest => "a graph preparation request is outstanding",
            WorkerBusyReason::WorkerUnavailable => "the graph preparation worker is unavailable",
            WorkerBusyReason::Shutdown => "the graph preparation worker is shut down",
        })
    }
}

impl std::error::Error for WorkerBusy {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerShutdownError {
    ThreadPanicked,
}

impl fmt::Display for WorkerShutdownError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the graph preparation worker thread panicked")
    }
}

impl std::error::Error for WorkerShutdownError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphPreparationRequestError {
    MissingRequestIdentity,
    CapabilityUnchanged,
    IntentMismatch,
    ConfigDeltaMismatch,
    TargetRevisionNotNewer,
    PatchCapacityExceeded,
    DuplicatePatchId,
    UnknownPatch,
    SourceCapabilityMismatch,
    TargetCapabilityMismatch,
    InvalidActiveConfig,
    InvalidActiveEffectConfig,
    InvalidCandidateConfig,
    UnknownEffectEntry,
    InvalidOccupancy,
}

impl fmt::Display for GraphPreparationRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MissingRequestIdentity => "graph preparation requires a request identity",
            Self::CapabilityUnchanged => "graph preparation must change capability identity",
            Self::IntentMismatch => {
                "graph preparation intent does not match source and target capabilities"
            }
            Self::ConfigDeltaMismatch => {
                "graph preparation candidate exceeds its declared structural intent"
            }
            Self::TargetRevisionNotNewer => {
                "graph preparation target revision must be newer than its source"
            }
            Self::PatchCapacityExceeded => "graph preparation exceeds the fixed Patch capacity",
            Self::DuplicatePatchId => "graph preparation repeats a Patch identity",
            Self::UnknownPatch => "graph preparation does not contain its selected Patch",
            Self::SourceCapabilityMismatch => "selected Patch does not match the source capability",
            Self::TargetCapabilityMismatch => {
                "candidate config does not match the target capability"
            }
            Self::InvalidActiveConfig => "an active Patch config is invalid",
            Self::InvalidActiveEffectConfig => "an active Patch effect config is invalid",
            Self::InvalidCandidateConfig => "the candidate Patch config is invalid",
            Self::UnknownEffectEntry => {
                "the requested registry entry is not installed in the effect registry"
            }
            Self::InvalidOccupancy => "the requested occupancy change is invalid at its position",
        })
    }
}

fn build_slot_occupant(
    effects: &EffectCapabilityRegistry,
    slot: crate::synth::effect_slot_id::EffectSlotIndex,
    entry: Option<&crate::synth::EffectCapabilityId>,
) -> Result<Option<crate::synth::PostEffectConfig>, GraphPreparationRequestError> {
    let Some(entry_id) = entry else {
        return Ok(None);
    };
    let descriptor = effects
        .descriptor(entry_id)
        .ok_or(GraphPreparationRequestError::UnknownEffectEntry)?;
    descriptor
        .default_config(slot.instance_identity())
        .map(Some)
        .map_err(|_| GraphPreparationRequestError::InvalidOccupancy)
}

fn validate_candidate_delta(
    source: &InstrumentConfig,
    candidate: &InstrumentConfig,
    intent: &StructuralEditIntent,
    registry: &CapabilityRegistry,
) -> Result<(), GraphPreparationRequestError> {
    match intent {
        StructuralEditIntent::ReplaceCapability {
            target_capability_id,
        } => {
            if source.capability_id() == candidate.capability_id()
                || candidate.capability_id() != target_capability_id
            {
                return Err(GraphPreparationRequestError::ConfigDeltaMismatch);
            }
        }
        StructuralEditIntent::ReplaceParameterChoice {
            capability_id,
            parameter_id,
            choice_id,
        } => {
            if source.capability_id() != capability_id
                || candidate.capability_id() != capability_id
                || source.asset_references() != candidate.asset_references()
                || source.values().len() != candidate.values().len()
                || source.value(parameter_id)
                    == Some(&crate::synth::ParameterValue::Choice(choice_id.clone()))
                || candidate.value(parameter_id)
                    != Some(&crate::synth::ParameterValue::Choice(choice_id.clone()))
                || candidate
                    .values()
                    .iter()
                    .zip(source.values())
                    .any(|(next, prior)| {
                        next.parameter_id() != prior.parameter_id()
                            || (next.parameter_id() != parameter_id && next != prior)
                    })
            {
                return Err(GraphPreparationRequestError::ConfigDeltaMismatch);
            }
        }
        StructuralEditIntent::ReplaceAsset {
            capability_id,
            parameter_id,
            reference,
        } => {
            if source.capability_id() != capability_id
                || source.asset_reference(parameter_id) == Some(reference)
                || !registry
                    .replace_asset(source, parameter_id, reference.clone())
                    .is_ok_and(|expected| expected == *candidate)
            {
                return Err(GraphPreparationRequestError::ConfigDeltaMismatch);
            }
        }
        StructuralEditIntent::PrepareAudition {
            capability_id,
            parameter_id,
            reference,
        } => {
            if source.capability_id() != capability_id
                || !registry
                    .replace_asset(source, parameter_id, reference.clone())
                    .is_ok_and(|expected| expected == *candidate)
            {
                return Err(GraphPreparationRequestError::ConfigDeltaMismatch);
            }
        }
        StructuralEditIntent::SetVoiceBudget { .. }
        | StructuralEditIntent::ReplaceEffectAsset { .. }
        | StructuralEditIntent::SetSlotOccupancy { .. }
        | StructuralEditIntent::SetSendEffect { .. }
        | StructuralEditIntent::SetReturnOccupancy { .. }
        | StructuralEditIntent::AppendPatch { .. } => {
            return Err(GraphPreparationRequestError::IntentMismatch);
        }
    }
    Ok(())
}

impl std::error::Error for GraphPreparationRequestError {}

/// Executes one effect-aware complete-graph request on worker ownership.
pub(crate) fn prepare_graph_request_with_effects(
    registry: &CapabilityRegistry,
    preparers: &[Box<dyn InstrumentPreparer>],
    effects: &EffectCapabilityRegistry,
    effect_preparers: &[Box<dyn EffectPreparer>],
    audio_config: AudioDeviceConfig,
    request: GraphPreparationRequest,
) -> GraphPreparationResult {
    let correlation = request.correlation.clone();
    let mut resolved = registry.clone();
    for patch in request.candidate_patches() {
        let Some(preparer) = preparers
            .iter()
            .find(|p| p.capability_id() == patch.instrument_config().capability_id())
        else {
            continue;
        };
        match preparer.asset_descriptor(patch.instrument_config()) {
            Ok(Some(descriptor)) => match resolved.clone().with_asset_descriptor(descriptor) {
                Ok(next) => resolved = next,
                Err(_) => {
                    return GraphPreparationResult::Failed {
                        correlation,
                        failure: EngineSelectionFailure::InvalidDefaultConfig,
                    }
                }
            },
            Ok(None) => {}
            Err(error) => {
                return GraphPreparationResult::Failed {
                    correlation,
                    failure: match error {
                        crate::synth::InstrumentPreparationError::AssetLoadFailed => {
                            EngineSelectionFailure::AssetUnavailable
                        }
                        _ => EngineSelectionFailure::InvalidAsset,
                    },
                }
            }
        }
    }
    let registry = &resolved;

    if let Err(failure) = request.validate_for_worker(registry, effects, audio_config) {
        return GraphPreparationResult::Failed {
            correlation,
            failure,
        };
    }
    let candidate_config = request.candidate_config().cloned();
    let audition_request = match (
        correlation.intent(),
        correlation.patch_id(),
        request.audition_candidate().cloned(),
    ) {
        (StructuralEditIntent::PrepareAudition { .. }, Some(patch_id), Some(candidate)) => {
            match AuditionPreparationRequest::new(
                correlation.request_id().value(),
                patch_id,
                candidate,
            ) {
                Ok(request) => Some(request),
                Err(error) => {
                    return GraphPreparationResult::Failed {
                        correlation,
                        failure: map_graph_preparation_failure(&error),
                    };
                }
            }
        }
        (StructuralEditIntent::PrepareAudition { .. }, _, _) => {
            return GraphPreparationResult::Failed {
                correlation,
                failure: EngineSelectionFailure::GraphIncompatible,
            };
        }
        (_, _, None) => None,
        (_, _, Some(_)) => {
            return GraphPreparationResult::Failed {
                correlation,
                failure: EngineSelectionFailure::GraphIncompatible,
            };
        }
    };
    let mut builder = PreparedGraphBuilder::new(registry, preparers)
        .with_effects(effects, effect_preparers)
        .with_returns(request.candidate_returns());
    if let Some(audition) = audition_request.as_ref() {
        builder = builder.with_audition(audition);
    }
    let result = builder.build(
        correlation.target_graph_revision(),
        request.candidate_patches(),
        request.candidate_parameters().clone(),
        audio_config.sample_rate(),
        audio_config.render_capacity_frames(),
    );
    match result {
        Ok(mut prepared_graph) => {
            // The replacement declares its exact correlated delta so
            // block-boundary activation can carry every live instance the
            // delta leaves unchanged. Set on worker ownership, never on the
            // callback.
            if let Some(scope) = correlation.replacement_scope() {
                prepared_graph.set_carry_over_scope(scope);
            }
            let prepared_visualization = correlation
                .patch_id()
                .map(|patch_id| prepared_graph.prepared_sample_visualizations(patch_id))
                .filter(|values| !values.is_empty());
            GraphPreparationResult::Prepared {
                correlation,
                candidate_config,
                prepared_visualization,
                prepared_graph,
            }
        }
        Err(error) => GraphPreparationResult::Failed {
            correlation,
            failure: map_graph_preparation_failure(&error),
        },
    }
}

fn map_graph_preparation_failure(error: &GraphPreparationError) -> EngineSelectionFailure {
    match error {
        GraphPreparationError::InvalidSampleRate | GraphPreparationError::InvalidFrameCapacity => {
            EngineSelectionFailure::UnsupportedAudioConfig
        }
        GraphPreparationError::RevisionMismatch { .. }
        | GraphPreparationError::ParameterLayoutMismatch => {
            EngineSelectionFailure::GraphIncompatible
        }
        GraphPreparationError::UnrecordableCapabilityIdentity => {
            EngineSelectionFailure::InvalidDefaultConfig
        }
        GraphPreparationError::InvalidAuditionIdentity
        | GraphPreparationError::InvalidAuditionConfiguration => {
            EngineSelectionFailure::GraphIncompatible
        }
        GraphPreparationError::AuditionPreparerMissing => EngineSelectionFailure::PreparerMissing,
        GraphPreparationError::Audition(source) => {
            map_instrument_preparation_failure(source)
        }
        GraphPreparationError::Rack(error) => match error {
            RackPreparationError::MissingPreparer { .. } => EngineSelectionFailure::PreparerMissing,
            RackPreparationError::InvalidConfiguration { .. } => {
                EngineSelectionFailure::InvalidDefaultConfig
            }
            RackPreparationError::Instrument { source, .. } => {
                map_instrument_preparation_failure(source)
            }
            RackPreparationError::InvalidSampleRate
            | RackPreparationError::InvalidFrameCapacity => {
                EngineSelectionFailure::UnsupportedAudioConfig
            }
            RackPreparationError::PatchCapacityExceeded { .. }
            | RackPreparationError::DuplicatePatchId { .. }
            | RackPreparationError::DuplicatePreparer { .. }
            | RackPreparationError::ExtraPreparer { .. }
            | RackPreparationError::PreparedPatchMismatch { .. } => {
                EngineSelectionFailure::GraphIncompatible
            }
            RackPreparationError::PreparedAssetCapacityExceeded { .. } => {
                EngineSelectionFailure::GraphCapacityExceeded
            }
        },
        GraphPreparationError::PatchAudio(_) | GraphPreparationError::Effects(_) => {
            EngineSelectionFailure::PreparationFailed
        }
        GraphPreparationError::BusReturn { source, .. } => match source {
            crate::real_time::prepared_graph_builder::BusReturnPreparationError::UnknownRegistryEntry
            | crate::real_time::prepared_graph_builder::BusReturnPreparationError::InvalidConfiguration => {
                EngineSelectionFailure::InvalidDefaultConfig
            }
            crate::real_time::prepared_graph_builder::BusReturnPreparationError::MissingPreparer => {
                EngineSelectionFailure::PreparerMissing
            }
            crate::real_time::prepared_graph_builder::BusReturnPreparationError::Preparation(_)
            | crate::real_time::prepared_graph_builder::BusReturnPreparationError::Install(_) => {
                EngineSelectionFailure::PreparationFailed
            }
        },
        GraphPreparationError::EffectRack(error) => match error {
            crate::synth::EffectRackPreparationError::InvalidSampleRate
            | crate::synth::EffectRackPreparationError::InvalidFrameCapacity => {
                EngineSelectionFailure::UnsupportedAudioConfig
            }
            crate::synth::EffectRackPreparationError::MissingPreparer { .. }
            | crate::synth::EffectRackPreparationError::ExtraPreparer { .. } => {
                EngineSelectionFailure::PreparerMissing
            }
            crate::synth::EffectRackPreparationError::InvalidConfiguration { .. } => {
                EngineSelectionFailure::InvalidDefaultConfig
            }
            crate::synth::EffectRackPreparationError::Effect { .. }
            | crate::synth::EffectRackPreparationError::StorageAllocationFailed { .. } => {
                EngineSelectionFailure::PreparationFailed
            }
            crate::synth::EffectRackPreparationError::PatchCapacityExceeded { .. }
            | crate::synth::EffectRackPreparationError::DuplicatePatchId { .. }
            | crate::synth::EffectRackPreparationError::DuplicatePreparer { .. }
            | crate::synth::EffectRackPreparationError::PreparedIdentityMismatch { .. } => {
                EngineSelectionFailure::GraphIncompatible
            }
        },
    }
}

fn map_instrument_preparation_failure(
    source: &InstrumentPreparationError,
) -> EngineSelectionFailure {
    match source {
        InstrumentPreparationError::AssetLoadFailed
        | InstrumentPreparationError::AssetParseFailed
        | InstrumentPreparationError::AssetUnavailable { .. }
        | InstrumentPreparationError::InvalidAsset { .. }
        | InstrumentPreparationError::PresetUnavailable { .. } => {
            EngineSelectionFailure::AssetUnavailable
        }
        InstrumentPreparationError::InvalidSampleRate
        | InstrumentPreparationError::InvalidFrameCapacity => {
            EngineSelectionFailure::UnsupportedAudioConfig
        }
        InstrumentPreparationError::UnsupportedCapability { .. } => {
            EngineSelectionFailure::PreparerMissing
        }
        InstrumentPreparationError::InvalidConfiguration { .. } => {
            EngineSelectionFailure::InvalidDefaultConfig
        }
        InstrumentPreparationError::VoiceCapacityExceeded { .. }
        | InstrumentPreparationError::StorageAllocationFailed { .. }
        | InstrumentPreparationError::PreparationFailed { .. } => {
            EngineSelectionFailure::PreparationFailed
        }
        InstrumentPreparationError::SampleAsset { cause, .. } => map_sample_asset_failure(*cause),
    }
}

fn map_sample_asset_failure(error: crate::synth::SampleAssetError) -> EngineSelectionFailure {
    use crate::synth::SampleAssetError;
    match error {
        SampleAssetError::Unavailable | SampleAssetError::DownloadRequired => {
            EngineSelectionFailure::AssetUnavailable
        }
        SampleAssetError::UnsupportedContainer
        | SampleAssetError::UnsupportedEncoding
        | SampleAssetError::UnsupportedBitDepth
        | SampleAssetError::UnsupportedChannelCount
        | SampleAssetError::UnsupportedSampleRate => EngineSelectionFailure::UnsupportedAssetFormat,
        SampleAssetError::SourceTooLarge
        | SampleAssetError::DurationTooLong
        | SampleAssetError::AssetScalarCapacityExceeded => {
            EngineSelectionFailure::AssetCapacityExceeded
        }
        SampleAssetError::GraphPcmCapacityExceeded => EngineSelectionFailure::GraphCapacityExceeded,
        SampleAssetError::Cancelled => EngineSelectionFailure::Cancelled,
        SampleAssetError::AllocationFailed => EngineSelectionFailure::AllocationFailed,
        SampleAssetError::EmptyFile
        | SampleAssetError::InvalidRelativeId
        | SampleAssetError::NonFinitePcm
        | SampleAssetError::MalformedPcm
        | SampleAssetError::MalformedSoundFont
        | SampleAssetError::MalformedSysEx
        | SampleAssetError::MalformedModel
        | SampleAssetError::MalformedWave
        | SampleAssetError::MalformedCatalog
        | SampleAssetError::PathEscape
        | SampleAssetError::ArithmeticOverflow
        | SampleAssetError::InvalidLandmark
        | SampleAssetError::InvalidLoopRange
        | SampleAssetError::CrossfadeTooLong => EngineSelectionFailure::InvalidAsset,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        map_graph_preparation_failure, EngineSelectionFailure, GraphPreparationCorrelation,
        GraphPreparationError, GraphPreparationRequest, GraphPreparationRequestError,
    };
    use crate::adapter::braids_capability::BRAIDS_CAPABILITY_ID;
    use crate::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID;
    use crate::adapter::production_instruments::{
        production_capability_registry, production_instrument_providers,
    };
    use crate::adapter::sample_capability::{
        SampleCapability, SAMPLE_ASSET_PARAMETER_ID, SAMPLE_CAPABILITY_ID,
    };
    use crate::control::{EngineSelectionRequestId, StructuralEditIntent};
    use crate::kernel::{MidiChannel, PatchId};
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_state::MixerState;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::GraphRevision;
    use crate::shell::audio_output::{AudioDeviceConfig, AudioSampleFormat};
    use crate::synth::{
        AssetFileId, AssetKind, AssetReference, CapabilityId, DescriptorDefaultConfigFactory,
        InstrumentCapabilityProvider, ParameterId, Patch,
    };

    fn config(id: &str) -> crate::synth::InstrumentConfig {
        let registry = production_capability_registry().unwrap();
        DescriptorDefaultConfigFactory::new(registry, production_instrument_providers().unwrap())
            .create(&CapabilityId::new(id).unwrap())
            .unwrap()
    }

    fn patch(id: u32, channel: u8, capability_id: &str) -> Patch {
        Patch::new(
            PatchId::new(id).unwrap(),
            format!("Worker Patch {id}"),
            config(capability_id),
            MidiChannel::new(channel).unwrap(),
            PatchOutput::new(
                MixerTrackId::new(channel).unwrap(),
                -3.0 * ((id - 1) % 16 + 1) as f32,
            )
            .unwrap(),
        )
        .with_envelope(crate::synth::VoiceEnvelope::new(1.0, 2.0, 0.7, 3.0).unwrap())
    }

    fn globals() -> GlobalParameters {
        GlobalParameters::new(-3.0).unwrap()
    }

    fn audio_config() -> AudioDeviceConfig {
        AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 64).unwrap()
    }

    /// A per-position capability identity the graph cannot record exactly is
    /// a candidate-configuration refusal, and the worker reports it as one:
    /// the selection fails visibly with `InvalidDefaultConfig` rather than
    /// being reported as an incompatible graph — or, worse, succeeding with a
    /// truncated identity.
    #[test]
    fn carry_over_capability_identity_refusal_reports_an_invalid_candidate_config() {
        assert_eq!(
            map_graph_preparation_failure(&GraphPreparationError::UnrecordableCapabilityIdentity),
            EngineSelectionFailure::InvalidDefaultConfig
        );
    }

    #[test]
    fn graph_preparation_request_freezes_exactly_one_validated_replacement() {
        let registry = production_capability_registry().unwrap();
        let active = [
            patch(1, 0, HIDEF_CAPABILITY_ID),
            patch(2, 1, BRAIDS_CAPABILITY_ID),
        ];
        let correlation = GraphPreparationCorrelation::new(
            EngineSelectionRequestId::FIRST,
            active[0].id(),
            CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap(),
            CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap(),
            GraphRevision::INITIAL,
            GraphRevision::new(2).unwrap(),
        )
        .unwrap();
        let request = GraphPreparationRequest::replacement(
            correlation.clone(),
            &active,
            config(BRAIDS_CAPABILITY_ID),
            9,
            globals(),
            MixerState::default(),
            audio_config(),
            &registry,
        )
        .unwrap();

        assert_eq!(request.correlation(), &correlation);
        assert_eq!(request.candidate_patches().len(), active.len());
        assert_eq!(request.candidate_patches()[1], active[1]);
        assert_eq!(request.candidate_patches()[0].id(), active[0].id());
        assert_eq!(request.candidate_patches()[0].name(), active[0].name());
        assert_eq!(
            request.candidate_patches()[0].channel(),
            active[0].channel()
        );
        assert_eq!(request.candidate_patches()[0].output(), active[0].output());
        assert_eq!(
            request.candidate_patches()[0].envelope(),
            active[0].envelope()
        );
        assert_eq!(
            request.candidate_config().unwrap().capability_id().as_str(),
            BRAIDS_CAPABILITY_ID
        );
        assert_eq!(request.candidate_parameters().generation(), 9);
        assert_eq!(
            request.candidate_parameters().graph_revision(),
            GraphRevision::new(2).unwrap()
        );
        assert_eq!(request.candidate_parameters().patch_count(), active.len());
        assert_eq!(request.audio_config(), audio_config());
    }

    #[test]
    fn append_request_freezes_the_exact_prior_order_plus_one_candidate() {
        let registry = production_capability_registry().unwrap();
        let active = [
            patch(1, 0, HIDEF_CAPABILITY_ID),
            patch(2, 1, BRAIDS_CAPABILITY_ID),
        ];
        let candidate = patch(7, 2, HIDEF_CAPABILITY_ID);
        let correlation = GraphPreparationCorrelation::for_append(
            EngineSelectionRequestId::FIRST,
            candidate.id(),
            GraphRevision::INITIAL,
            GraphRevision::new(2).unwrap(),
        )
        .unwrap();
        let request = GraphPreparationRequest::append_patch(
            correlation.clone(),
            &active,
            candidate.clone(),
            &crate::mixer::bus_return::BusReturnBank::default(),
            11,
            globals(),
            MixerState::default(),
            audio_config(),
            &registry,
            &crate::synth::EffectCapabilityRegistry::default(),
        )
        .unwrap();

        assert_eq!(
            request.candidate_patches(),
            &[active[0].clone(), active[1].clone(), candidate]
        );
        assert_eq!(request.candidate_parameters().patch_count(), 3);
        assert_eq!(request.candidate_parameters().generation(), 11);
        assert_eq!(request.candidate_config(), None);
        assert_eq!(
            correlation.replacement_scope(),
            Some(crate::real_time::GraphReplacementScope::AppendPatch(
                PatchId::new(7).unwrap()
            ))
        );

        assert_eq!(
            GraphPreparationRequest::append_patch(
                GraphPreparationCorrelation::for_append(
                    EngineSelectionRequestId::FIRST,
                    active[0].id(),
                    GraphRevision::INITIAL,
                    GraphRevision::new(2).unwrap(),
                )
                .unwrap(),
                &active,
                active[0].clone(),
                &crate::mixer::bus_return::BusReturnBank::default(),
                11,
                globals(),
                MixerState::default(),
                audio_config(),
                &registry,
                &crate::synth::EffectCapabilityRegistry::default(),
            ),
            Err(GraphPreparationRequestError::DuplicatePatchId)
        );

        assert_eq!(
            GraphPreparationRequest::append_patch(
                GraphPreparationCorrelation::for_append(
                    EngineSelectionRequestId::FIRST,
                    PatchId::new(8).unwrap(),
                    GraphRevision::INITIAL,
                    GraphRevision::new(2).unwrap(),
                )
                .unwrap(),
                &active,
                patch(7, 2, HIDEF_CAPABILITY_ID),
                &crate::mixer::bus_return::BusReturnBank::default(),
                11,
                globals(),
                MixerState::default(),
                audio_config(),
                &registry,
                &crate::synth::EffectCapabilityRegistry::default(),
            ),
            Err(GraphPreparationRequestError::IntentMismatch)
        );

        let full = (1..=crate::kernel::MAX_ACTIVE_PATCHES as u32)
            .map(|id| patch(id, ((id - 1) % 16) as u8, HIDEF_CAPABILITY_ID))
            .collect::<Vec<_>>();
        assert_eq!(
            GraphPreparationRequest::append_patch(
                GraphPreparationCorrelation::for_append(
                    EngineSelectionRequestId::FIRST,
                    PatchId::new(17).unwrap(),
                    GraphRevision::INITIAL,
                    GraphRevision::new(2).unwrap(),
                )
                .unwrap(),
                &full,
                patch(17, 0, HIDEF_CAPABILITY_ID),
                &crate::mixer::bus_return::BusReturnBank::default(),
                11,
                globals(),
                MixerState::default(),
                audio_config(),
                &registry,
                &crate::synth::EffectCapabilityRegistry::default(),
            ),
            Err(GraphPreparationRequestError::PatchCapacityExceeded)
        );
    }

    #[test]
    fn audition_request_keeps_canonical_patch_topology_and_carries_only_transient_candidate() {
        let provider = SampleCapability::new(AssetFileId::new("active.wav").unwrap()).unwrap();
        let registry = crate::synth::CapabilityRegistry::new(vec![provider.descriptor()]).unwrap();
        let active = [Patch::new(
            PatchId::new(3).unwrap(),
            "Sample origin".to_owned(),
            provider.default_config().unwrap(),
            MidiChannel::new(2).unwrap(),
            PatchOutput::default(),
        )];
        let candidate = crate::synth::DescriptorDefaultConfigFactory::new(
            registry.clone(),
            vec![Box::new(
                SampleCapability::new(AssetFileId::new("active.wav").unwrap()).unwrap(),
            )],
        )
        .replace_asset(
            active[0].instrument_config(),
            &ParameterId::new(SAMPLE_ASSET_PARAMETER_ID).unwrap(),
            AssetReference::new(AssetKind::Sample, "preview.wav").unwrap(),
        )
        .unwrap();
        let capability_id = CapabilityId::new(SAMPLE_CAPABILITY_ID).unwrap();
        let intent = StructuralEditIntent::PrepareAudition {
            capability_id: capability_id.clone(),
            parameter_id: ParameterId::new(SAMPLE_ASSET_PARAMETER_ID).unwrap(),
            reference: AssetReference::new(AssetKind::Sample, "preview.wav").unwrap(),
        };
        let correlation = GraphPreparationCorrelation::new_with_intent(
            EngineSelectionRequestId::FIRST,
            active[0].id(),
            intent,
            capability_id.clone(),
            capability_id,
            GraphRevision::INITIAL,
            GraphRevision::new(2).unwrap(),
        )
        .unwrap();
        let request = GraphPreparationRequest::replacement(
            correlation.clone(),
            &active,
            candidate.clone(),
            4,
            globals(),
            MixerState::default(),
            audio_config(),
            &registry,
        )
        .unwrap();

        assert_eq!(request.candidate_patches(), &active);
        assert_eq!(request.audition_candidate(), Some(&candidate));
        assert_eq!(request.candidate_config(), Some(&candidate));
        assert_eq!(
            request.candidate_parameters().patches()[0]
                .instrument()
                .count(),
            provider.descriptor().scalar_parameter_count()
        );
        assert_eq!(
            correlation.replacement_scope(),
            Some(crate::real_time::GraphReplacementScope::Audition)
        );
    }

    #[test]
    fn graph_preparation_correlation_and_request_reject_mismatches_without_fallback() {
        let registry = production_capability_registry().unwrap();
        let source = CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap();
        let target = CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap();
        assert_eq!(
            GraphPreparationCorrelation::new(
                EngineSelectionRequestId::NONE,
                PatchId::new(1).unwrap(),
                source.clone(),
                target.clone(),
                GraphRevision::INITIAL,
                GraphRevision::new(2).unwrap(),
            ),
            Err(GraphPreparationRequestError::MissingRequestIdentity)
        );
        assert_eq!(
            GraphPreparationCorrelation::new(
                EngineSelectionRequestId::FIRST,
                PatchId::new(1).unwrap(),
                source.clone(),
                source.clone(),
                GraphRevision::INITIAL,
                GraphRevision::new(2).unwrap(),
            ),
            Err(GraphPreparationRequestError::CapabilityUnchanged)
        );
        let active = [patch(1, 0, HIDEF_CAPABILITY_ID)];
        let correlation = GraphPreparationCorrelation::new(
            EngineSelectionRequestId::FIRST,
            active[0].id(),
            source,
            target,
            GraphRevision::INITIAL,
            GraphRevision::new(2).unwrap(),
        )
        .unwrap();
        assert_eq!(
            GraphPreparationRequest::replacement(
                correlation,
                &active,
                config(HIDEF_CAPABILITY_ID),
                2,
                globals(),
                MixerState::default(),
                audio_config(),
                &registry,
            ),
            Err(GraphPreparationRequestError::TargetCapabilityMismatch)
        );
    }
}
