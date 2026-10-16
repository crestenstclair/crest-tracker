use crate::control::serialized_state::{
    SerializedGlobalParameters, SerializedInteractionState, SerializedPatch, SerializedState,
};
use crate::control::state_snapshot::StateSnapshot;
use crate::control::text_projection::TextProjection;
use crate::control::GraphicalShellProjection;
use crate::control::{
    EngineSelectionStatus, PatchPageProjection, SemanticControlId, TopLevelContext,
};
use crate::mixer::global_parameters::GlobalParameters;
use crate::mixer::mixer_state::MixerState;
use crate::mixer::patch_output::PatchOutput;
use crate::real_time::parameter_snapshot::ParameterSnapshot;
use crate::real_time::GraphRevision;
use crate::synth::instrument_capability::{CapabilityRegistry, InstrumentConfig};
use crate::synth::voice_envelope::VoiceEnvelope;
use crate::synth::{EffectCapabilityRegistry, PostEffectConfig};
use core::fmt;
use serde::Serialize;
use std::sync::{Arc, OnceLock};

/// A coherence violation while constructing the canonical observation tree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateTreeError {
    /// The accepted state snapshot was not valid canonical state JSON.
    StateDeserialization,
    /// The text projection did not originate from the supplied state snapshot.
    ProjectionHashMismatch,
    /// The audio parameters did not originate from the accepted generation.
    GenerationMismatch,
    /// A generation-only projection targeted a different prepared graph.
    GraphRevisionMismatch,
    /// The state and real-time projections contained different Patch counts.
    PatchCountMismatch,
    /// A real-time Patch identity did not match the state at the same position.
    PatchIdentityMismatch {
        index: usize,
    },
    /// A real-time Patch parameter set did not match the serialized state.
    PatchParametersMismatch {
        index: usize,
    },
    /// One fixed mixer track differed from the accepted state.
    MixerParametersMismatch {
        index: usize,
    },
    /// The global real-time parameters did not match the serialized state.
    GlobalParametersMismatch,
    /// One live bus-return entry did not attest the serialized return state.
    ReturnParametersMismatch {
        index: usize,
    },
    /// The complete tree could not be serialized.
    Serialization,
    /// A generation-only projection could not reuse the canonical tree shape.
    MidiTemplateMismatch,
    /// State, page, and text did not name the same reducer-owned context.
    ContextMismatch,
    /// PATCH page presence, identity, or snapshot hash was incoherent.
    PatchPageMismatch,
    /// The shell did not originate from the supplied accepted generation.
    GraphicalShellGenerationMismatch,
    /// The shell did not originate from the supplied accepted snapshot.
    GraphicalShellHashMismatch,
    /// State, shell, and retained diagnostic did not name the same context.
    GraphicalShellContextMismatch,
    /// The shell did not contain the exact supplied retained diagnostic.
    GraphicalShellDiagnosticMismatch,
    SemanticModelMismatch,
    InvalidInteractionPath,
}

impl fmt::Display for StateTreeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::StateDeserialization => {
                formatter.write_str("accepted state snapshot could not be decoded")
            }
            Self::ProjectionHashMismatch => {
                formatter.write_str("text projection does not match the state snapshot")
            }
            Self::GenerationMismatch => {
                formatter.write_str("parameter generation does not match accepted state")
            }
            Self::GraphRevisionMismatch => {
                formatter.write_str("parameter graph revision changed within one StateTree")
            }
            Self::PatchCountMismatch => {
                formatter.write_str("parameter Patch count does not match accepted state")
            }
            Self::PatchIdentityMismatch { index } => {
                write!(
                    formatter,
                    "parameter Patch identity differs at index {index}"
                )
            }
            Self::PatchParametersMismatch { index } => {
                write!(formatter, "parameter Patch values differ at index {index}")
            }
            Self::MixerParametersMismatch { index } => {
                write!(formatter, "mixer-track values differ at index {index}")
            }
            Self::GlobalParametersMismatch => {
                formatter.write_str("global parameter values do not match accepted state")
            }
            Self::ReturnParametersMismatch { index } => {
                write!(formatter, "bus-return values differ at index {index}")
            }
            Self::Serialization => formatter.write_str("state tree could not be serialized"),
            Self::MidiTemplateMismatch => {
                formatter.write_str("state tree generation template does not match canonical JSON")
            }
            Self::ContextMismatch => {
                formatter.write_str("state and text projection contexts do not match")
            }
            Self::PatchPageMismatch => {
                formatter.write_str("PATCH page does not match canonical context, focus, or hash")
            }
            Self::GraphicalShellGenerationMismatch => {
                formatter.write_str("graphical shell generation does not match accepted state")
            }
            Self::GraphicalShellHashMismatch => {
                formatter.write_str("graphical shell does not match the accepted state snapshot")
            }
            Self::GraphicalShellContextMismatch => {
                formatter.write_str("state and graphical shell contexts do not match")
            }
            Self::GraphicalShellDiagnosticMismatch => {
                formatter.write_str("graphical shell does not contain the retained diagnostic")
            }
            Self::SemanticModelMismatch => formatter
                .write_str("graphical shell semantic model does not match canonical interaction"),
            Self::InvalidInteractionPath => {
                formatter.write_str("serialized interaction contains an invalid semantic path")
            }
        }
    }
}

impl std::error::Error for StateTreeError {}

/// A canonical, LLM-readable tree of one complete accepted control generation.
///
/// Construction consumes only immutable projections already derived from the
/// same accepted AppState. It verifies their shared identity before producing
/// deterministic JSON with stable property names.
#[derive(Clone, Debug)]
pub struct StateTree {
    json: TreeJson,
    generation: u64,
    graph_revision: GraphRevision,
    patch_count: usize,
    selected_line: usize,
    context: TopLevelContext,
    patch_page_position: Option<crate::control::PatchPositionId>,
    state_hash: Arc<str>,
}

#[derive(Clone, Debug)]
enum TreeJson {
    Ready(Arc<str>),
    MidiGeneration {
        template: Arc<MidiTreeTemplate>,
        generation: u64,
        state_hash: Arc<str>,
        rendered: Arc<OnceLock<String>>,
    },
}

#[derive(Debug)]
struct MidiTreeTemplate {
    before_root_generation: Arc<str>,
    between_root_and_shell_generation: Arc<str>,
    between_shell_and_semantic_generation: Arc<str>,
    between_semantic_and_parameter_generation: Arc<str>,
    after_parameter_generation: Arc<str>,
    source_state_hash: Arc<str>,
}

impl StateTree {
    /// The stable schema version emitted in every serialized tree.
    ///
    /// Version 26: provider-owned instrument families and the active picker label.
    /// Version 27: effect families and the shared instrument/effect picker label.
    /// Version 28: descriptor-derived explanations for read-only controls.
    /// Version 29: optional display names for numeric stepped parameters.
    /// Version 32: named send chains, their full audio parameters, and send-screen interaction.
    /// Version 34: leading Detail controls and optional, grouped Sample assets.
    ///
    /// Version 23: browser metadata distinguishes unavailable files from
    /// invalid audio and carries download-required and empty-file causes.
    ///
    /// Version 20: PATCH focus gained the explicit trailing-empty position;
    /// `patchPage.patch` and semantic PATCH summaries now carry tagged
    /// created/empty ownership plus prospective and capacity facts. Empty
    /// shapes omit `PatchId` and MIDI identity entirely.
    ///
    /// Version 19: reducer-owned physical MIDI registry, preference intent,
    /// scan, request, and active-revision facts joined the canonical trace.
    /// Runtime handles, callback queues, timestamps, and observations remain
    /// outside the tree.
    ///
    /// Version 17: Sample Browser file controls gained typed catalog metadata
    /// status, admitted format fields, duration/channel data, and explicit
    /// failure text. The frame still contains no decoded PCM or absolute path.
    ///
    /// Version 16: capability descriptors and the semantic surface model gained
    /// ordered non-focusable visualization declarations/data plus semantic
    /// detail sections. Prepared waveform summaries remain bounded and contain
    /// no decoded PCM or absolute paths.
    ///
    /// Version 15: the subordinate detail surface became projectable, so the
    /// tree gained `patchPage.detail` and the semantic model's controls gained
    /// `requestedValue`, their own `validActions`, and the capability-declared
    /// `patchInteraction` the detail shell marks read-only rows from. All
    /// three tagged unions whose fields serialized snake_case inside a
    /// camelCase schema moved in the same bump — `PatchDetailSubject`,
    /// `SemanticSurfaceSummary`, and `MixerControlId`: `#[serde(rename_all)]`
    /// renames a tagged enum's variants and not a struct variant's fields, so
    /// `rename_all_fields` was needed on each. Fixing one and not the others
    /// would have left the schema *mixed* rather than uniformly wrong, which
    /// is harder to read; paying all three in a version this shape was
    /// changing anyway costs one bump instead of four. `controlId.id.track_id`
    /// is now `controlId.id.trackId` in every focus, remembered-focus, and
    /// return-path leaf.
    ///
    /// Version 14: `interaction` gained the `detailSubject` leaves. The
    /// subject decides what the detail surface shows, so a trace that cannot
    /// show which capability a detail surface was opened on cannot correlate a
    /// detail interaction with its consequence — the same reason `voiceLimit`
    /// was enumerated in version 13.
    ///
    /// Version 13: `parameters.patches[]` gained the `voiceLimit` leaf. A
    /// canonical value that crosses the real-time boundary and changes what is
    /// audible must be visible in the trace, or no measured proof can
    /// correlate a refused note with the limit that refused it.
    ///
    /// Version 12: the six retired reverb/delay `global` leaves are gone —
    /// return-owned state travels as the indexed top-level `returns` section.
    /// Version 24: asset-scoped capability catalogs and file-kind-correlated browser events.
    /// Version 25: source-specific `NavigatePage` actions and Settings-only
    /// PATCH page absence in full and generation-only projections.
    pub const SCHEMA_VERSION: u32 = 34;
    pub const SERIALIZED_PROPERTY_DESCRIPTOR: &'static [&'static str] = &[
        "schemaVersion",
        "generation",
        "capabilities",
        "effects",
        "patches",
        "mixer",
        "global",
        "returns",
        "interaction.activeFocus",
        "interaction.rememberedPatchMain",
        "interaction.rememberedMixerMain",
        "interaction.rememberedSend",
        "interaction.sendChoiceOrigin",
        "interaction.sendNameEditing",
        "interaction.mode",
        "interaction.returnPath",
        "interaction.detailSubject",
        "engineSelection.kind",
        "engineSelection.activeGraphRevision",
        "engineSelection.correlation",
        "engineSelection.correlation.intent",
        "engineSelection.correlation.requestId",
        "engineSelection.correlation.patchId",
        "engineSelection.correlation.sourceCapabilityId",
        "engineSelection.correlation.targetCapabilityId",
        "engineSelection.correlation.sourceGraphRevision",
        "engineSelection.correlation.targetGraphRevision",
        "engineSelection.failure",
        "midiInput",
        "controller",
        "patchPage",
        "graphicalShell",
        "projection.context",
        "projection.body",
        "projection.selectedLine",
        "projection.stateHash",
        "parameters.generation",
        "parameters.graphRevision",
        "parameters.patchCount",
        "parameters.patches",
        "parameters.mixerTracks",
        "parameters.returns",
        "parameters.global",
    ];
    /// The complete normalized nested schema, including tagged capability values.
    const BASE_SERIALIZED_LEAF_DESCRIPTOR: &'static [&'static str] = &[
        "schemaVersion",
        "generation",
        "capabilities.descriptors[].assetScopedChoices",
        "capabilities.descriptors[].instrumentCategory",
        "capabilities.descriptors[].id",
        "capabilities.descriptors[].label",
        "capabilities.descriptors[].semanticAccent",
        "capabilities.descriptors[].availability.kind",
        "capabilities.descriptors[].availability.reason",
        "capabilities.descriptors[].visualizations[].assetParameterId",
        "capabilities.descriptors[].visualizations[].id",
        "capabilities.descriptors[].visualizations[].kind",
        "capabilities.descriptors[].visualizations[].label",
        "capabilities.descriptors[].visualizations[].landmarks[].parameterId",
        "capabilities.descriptors[].visualizations[].landmarks[].role",
        "capabilities.descriptors[].sections[].id",
        "capabilities.descriptors[].sections[].label",
        "capabilities.descriptors[].sections[].leadingControls",
        "capabilities.descriptors[].sections[].parameters[].id",
        "capabilities.descriptors[].sections[].parameters[].label",
        "capabilities.descriptors[].sections[].parameters[].kind",
        "capabilities.descriptors[].sections[].parameters[].patchInteraction",
        "capabilities.descriptors[].sections[].parameters[].update",
        "capabilities.descriptors[].sections[].parameters[].defaultValue.kind",
        "capabilities.descriptors[].sections[].parameters[].defaultValue.value.kind",
        "capabilities.descriptors[].sections[].parameters[].defaultValue.value.value",
        "capabilities.descriptors[].sections[].parameters[].defaultValue.value.locator",
        "capabilities.descriptors[].sections[].parameters[].range.minimum",
        "capabilities.descriptors[].sections[].parameters[].range.maximum",
        "capabilities.descriptors[].sections[].parameters[].range",
        "capabilities.descriptors[].sections[].parameters[].choices[].id",
        "capabilities.descriptors[].sections[].parameters[].choices[].label",
        "capabilities.descriptors[].sections[].parameters[].steppedLabels[]",
        "capabilities.descriptors[].sections[].parameters[].continuousLabels[].range.minimum",
        "capabilities.descriptors[].sections[].parameters[].continuousLabels[].range.maximum",
        "capabilities.descriptors[].sections[].parameters[].continuousLabels[].label",
        "capabilities.descriptors[].sections[].parameters[].continuousLabels[].valueScale",
        "capabilities.descriptors[].sections[].parameters[].continuousLabels[].valueOffset",
        "capabilities.descriptors[].sections[].parameters[].continuousLabels[].valueUnit",
        "capabilities.descriptors[].sections[].parameters[].fineStep",
        "capabilities.descriptors[].sections[].parameters[].coarseStep",
        "capabilities.descriptors[].sections[].parameters[].unit",
        "capabilities.descriptors[].sections[].parameters[].formatter",
        "capabilities.descriptors[].sections[].parameters[].enabledWhen",
        "capabilities.descriptors[].sections[].parameters[].enabledWhen.parameterId",
        "capabilities.descriptors[].sections[].parameters[].enabledWhen.equals.kind",
        "capabilities.descriptors[].sections[].parameters[].enabledWhen.equals.value",
        "capabilities.descriptors[].sections[].parameters[].visibleWhen",
        "capabilities.descriptors[].sections[].parameters[].visibleWhen.parameterId",
        "capabilities.descriptors[].sections[].parameters[].visibleWhen.equals.kind",
        "capabilities.descriptors[].sections[].parameters[].visibleWhen.equals.value",
        "capabilities.descriptors[].assetRequirements[].parameterId",
        "capabilities.descriptors[].assetRequirements[].required",
        "capabilities.descriptors[].voicePolicy.kind",
        "capabilities.descriptors[].voicePolicy.defaultVoices",
        "capabilities.descriptors[].supportedMidiKinds[]",
        "effects.descriptors[].id",
        "effects.descriptors[].effectCategory",
        "effects.descriptors[].label",
        "effects.descriptors[].semanticAccent",
        "effects.descriptors[].availability.kind",
        "effects.descriptors[].availability.reason",
        "effects.descriptors[].sections[].id",
        "effects.descriptors[].sections[].label",
        "effects.descriptors[].sections[].leadingControls",
        "effects.descriptors[].sections[].parameters[].id",
        "effects.descriptors[].sections[].parameters[].label",
        "effects.descriptors[].sections[].parameters[].kind",
        "effects.descriptors[].sections[].parameters[].patchInteraction",
        "effects.descriptors[].sections[].parameters[].update",
        "effects.descriptors[].sections[].parameters[].defaultValue.kind",
        "effects.descriptors[].sections[].parameters[].defaultValue.value.kind",
        "effects.descriptors[].sections[].parameters[].defaultValue.value.value",
        "effects.descriptors[].sections[].parameters[].defaultValue.value.locator",
        "effects.descriptors[].sections[].parameters[].range.minimum",
        "effects.descriptors[].sections[].parameters[].range.maximum",
        "effects.descriptors[].sections[].parameters[].range",
        "effects.descriptors[].sections[].parameters[].choices[].id",
        "effects.descriptors[].sections[].parameters[].choices[].label",
        "effects.descriptors[].sections[].parameters[].steppedLabels[]",
        "effects.descriptors[].sections[].parameters[].continuousLabels[].range.minimum",
        "effects.descriptors[].sections[].parameters[].continuousLabels[].range.maximum",
        "effects.descriptors[].sections[].parameters[].continuousLabels[].label",
        "effects.descriptors[].sections[].parameters[].continuousLabels[].valueScale",
        "effects.descriptors[].sections[].parameters[].continuousLabels[].valueOffset",
        "effects.descriptors[].sections[].parameters[].continuousLabels[].valueUnit",
        "effects.descriptors[].sections[].parameters[].fineStep",
        "effects.descriptors[].sections[].parameters[].coarseStep",
        "effects.descriptors[].sections[].parameters[].unit",
        "effects.descriptors[].sections[].parameters[].formatter",
        "effects.descriptors[].sections[].parameters[].enabledWhen",
        "effects.descriptors[].sections[].parameters[].visibleWhen",
        "effects.descriptors[].assetRequirements[].parameterId",
        "effects.descriptors[].assetRequirements[].required",
        "patches[].id",
        "patches[].name",
        "patches[].channel",
        "patches[].instrument.capabilityId",
        "patches[].instrument.values[].parameterId",
        "patches[].instrument.values[].value.kind",
        "patches[].instrument.values[].value.value",
        "patches[].instrument.assetReferences[].parameterId",
        "patches[].instrument.assetReferences[].reference.kind",
        "patches[].instrument.assetReferences[].reference.locator",
        "patches[].postEffects[].slotId",
        "patches[].postEffects[].capabilityId",
        "patches[].postEffects[].values[].parameterId",
        "patches[].postEffects[].values[].value.kind",
        "patches[].postEffects[].values[].value.value",
        "patches[].postEffects[].assetReferences[].parameterId",
        "patches[].postEffects[].assetReferences[].reference.kind",
        "patches[].postEffects[].assetReferences[].reference.locator",
        "patches[].envelope.attackMilliseconds",
        "patches[].envelope.decayMilliseconds",
        "patches[].envelope.releaseMilliseconds",
        "patches[].envelope.sustain",
        "patches[].output.trackId",
        "patches[].output.trimGainDb",
        "patches[].sends[]",
        "mixer.tracks[].levelDb",
        "mixer.tracks[].pan",
        "mixer.tracks[].mute",
        "mixer.tracks[].solo",
        "mixer.tracks[].sends[]",
        "global.masterGainDb",
        "returns[].name",
        "returns[].effects[].slotId",
        "returns[].effects[].capabilityId",
        "returns[].effects[].values[].parameterId",
        "returns[].effects[].values[].value.kind",
        "returns[].effects[].values[].value.value",
        "returns[].effects[].assetReferences[].parameterId",
        "returns[].effects[].assetReferences[].reference.kind",
        "returns[].effects[].assetReferences[].reference.locator",
        "returns[].effect",
        "returns[].effect.slotId",
        "returns[].effect.capabilityId",
        "returns[].effect.values[].parameterId",
        "returns[].effect.values[].value.kind",
        "returns[].effect.values[].value.value",
        "returns[].effect.assetReferences[].parameterId",
        "returns[].effect.assetReferences[].reference.kind",
        "returns[].effect.assetReferences[].reference.locator",
        "returns[].returnLevel",
        "interaction.activeFocus.context",
        "interaction.activeFocus.surface",
        "interaction.activeFocus.patchId",
        "interaction.activeFocus.capabilityId",
        "interaction.activeFocus.controlId",
        "interaction.activeFocus.modalId",
        "interaction.rememberedPatchMain.context",
        "interaction.rememberedPatchMain.surface",
        "interaction.rememberedPatchMain.patchId",
        "interaction.rememberedPatchMain.capabilityId",
        "interaction.rememberedPatchMain.controlId",
        "interaction.rememberedPatchMain.modalId",
        "interaction.rememberedMixerMain.context",
        "interaction.rememberedMixerMain.surface",
        "interaction.rememberedMixerMain.patchId",
        "interaction.rememberedMixerMain.capabilityId",
        "interaction.rememberedMixerMain.controlId",
        "interaction.rememberedMixerMain.modalId",
        "interaction.mode",
        "interaction.returnPath.origin.context",
        "interaction.returnPath.origin.surface",
        "interaction.returnPath.origin.patchId",
        "interaction.returnPath.origin.capabilityId",
        "interaction.returnPath.origin.controlId",
        "interaction.returnPath.origin.modalId",
        "interaction.returnPath.enteredSurface",
        "interaction.returnPath",
        // The open detail surface's subject: `null` with no entry open, and
        // otherwise the subject's own leaves. `slot_id` belongs to the
        // `Effect` variant alone, so a tree that only ever opens an instrument
        // detail entry cannot discover it.
        "interaction.detailSubject",
        "interaction.detailSubject.kind",
        "interaction.detailSubject.capabilityId",
        "interaction.detailSubject.slotId",
        "engineSelection.kind",
        "engineSelection.activeGraphRevision",
        "engineSelection.correlation",
        "engineSelection.correlation.intent.capabilityId",
        "engineSelection.correlation.intent.choiceId",
        "engineSelection.correlation.intent.kind",
        "engineSelection.correlation.intent.parameterId",
        "engineSelection.correlation.intent.targetCapabilityId",
        "engineSelection.correlation.intent.patchId",
        "engineSelection.correlation.intent.slot",
        "engineSelection.correlation.intent.slotId",
        "engineSelection.correlation.intent.bus",
        "engineSelection.correlation.intent.entry",
        "engineSelection.correlation.intent.reference.kind",
        "engineSelection.correlation.intent.reference.locator",
        "engineSelection.correlation.requestId",
        "engineSelection.correlation.patchId",
        "engineSelection.correlation.sourceCapabilityId",
        "engineSelection.correlation.targetCapabilityId",
        "engineSelection.correlation.sourceGraphRevision",
        "engineSelection.correlation.targetGraphRevision",
        "engineSelection.failure",
        "patchPage",
        "patchPage.context",
        "patchPage.engine.activeCapabilityId",
        "patchPage.engine.activeGraphRevision",
        "patchPage.engine.activeLabel",
        "patchPage.engine.choices[].capabilityId",
        "patchPage.engine.choices[].label",
        "patchPage.engine.controlId",
        "patchPage.engine.editable",
        "patchPage.engine.failure",
        "patchPage.engine.requestId",
        "patchPage.engine.requestedCapabilityId",
        "patchPage.engine.status",
        "patchPage.engine.targetGraphRevision",
        "patchPage.envelope[].coarseStep",
        "patchPage.envelope[].controlId",
        "patchPage.envelope[].editable",
        "patchPage.envelope[].fineStep",
        "patchPage.envelope[].id",
        "patchPage.envelope[].label",
        "patchPage.envelope[].maximum",
        "patchPage.envelope[].minimum",
        "patchPage.envelope[].unit",
        "patchPage.envelope[].value",
        "patchPage.focusedControlId",
        "patchPage.effects[].capabilityId",
        "patchPage.effects[].editableIdentity",
        "patchPage.effects[].label",
        "patchPage.effects[].slotId",
        "patchPage.effects[].sections[].id",
        "patchPage.effects[].sections[].label",
        "patchPage.effects[].sections[].parameters[].coarseStep",
        "patchPage.effects[].sections[].parameters[].controlId",
        "patchPage.effects[].sections[].parameters[].editable",
        "patchPage.effects[].sections[].parameters[].enabled",
        "patchPage.effects[].sections[].parameters[].activeGraphRevision",
        "patchPage.effects[].sections[].parameters[].failure",
        "patchPage.effects[].sections[].parameters[].fineStep",
        "patchPage.effects[].sections[].parameters[].formatter",
        "patchPage.effects[].sections[].parameters[].id",
        "patchPage.effects[].sections[].parameters[].kind",
        "patchPage.effects[].sections[].parameters[].label",
        "patchPage.effects[].sections[].parameters[].patchInteraction",
        "patchPage.effects[].sections[].parameters[].requestId",
        "patchPage.effects[].sections[].parameters[].range.maximum",
        "patchPage.effects[].sections[].parameters[].range.minimum",
        "patchPage.effects[].sections[].parameters[].requestedChoiceId",
        "patchPage.effects[].sections[].parameters[].requestedLabel",
        "patchPage.effects[].sections[].parameters[].selectedChoiceId",
        "patchPage.effects[].sections[].parameters[].selectedLabel",
        "patchPage.effects[].sections[].parameters[].status",
        "patchPage.effects[].sections[].parameters[].targetGraphRevision",
        "patchPage.effects[].sections[].parameters[].unit",
        "patchPage.effects[].sections[].parameters[].update",
        "patchPage.effects[].sections[].parameters[].value.source",
        "patchPage.effects[].sections[].parameters[].value.value.kind",
        "patchPage.effects[].sections[].parameters[].value.value.value",
        "patchPage.effects[].sections[].parameters[].visible",
        "patchPage.patch.id",
        "patchPage.patch.midiChannel",
        "patchPage.patch.name",
        "patchPage.sections[].id",
        "patchPage.sections[].label",
        "patchPage.sections[].parameters[].activeGraphRevision",
        "patchPage.sections[].parameters[].choices[].id",
        "patchPage.sections[].parameters[].choices[].label",
        "patchPage.sections[].parameters[].coarseStep",
        "patchPage.sections[].parameters[].controlId",
        "patchPage.sections[].parameters[].editable",
        "patchPage.sections[].parameters[].enabled",
        "patchPage.sections[].parameters[].failure",
        "patchPage.sections[].parameters[].fineStep",
        "patchPage.sections[].parameters[].formatter",
        "patchPage.sections[].parameters[].id",
        "patchPage.sections[].parameters[].kind",
        "patchPage.sections[].parameters[].label",
        "patchPage.sections[].parameters[].patchInteraction",
        "patchPage.sections[].parameters[].range.maximum",
        "patchPage.sections[].parameters[].range.minimum",
        "patchPage.sections[].parameters[].range",
        "patchPage.sections[].parameters[].requestId",
        "patchPage.sections[].parameters[].requestedChoiceId",
        "patchPage.sections[].parameters[].requestedLabel",
        "patchPage.sections[].parameters[].selectedChoiceId",
        "patchPage.sections[].parameters[].selectedLabel",
        "patchPage.sections[].parameters[].status",
        "patchPage.sections[].parameters[].targetGraphRevision",
        "patchPage.sections[].parameters[].unit",
        "patchPage.sections[].parameters[].update",
        "patchPage.sections[].parameters[].value.reference.kind",
        "patchPage.sections[].parameters[].value.reference.locator",
        "patchPage.sections[].parameters[].value.source",
        "patchPage.sections[].parameters[].value.value.kind",
        "patchPage.sections[].parameters[].value.value.value",
        "patchPage.sections[].parameters[].visible",
        "patchPage.stateHash",
        "graphicalShell.generation",
        "graphicalShell.stateHash",
        "graphicalShell.context",
        "graphicalShell.contextLine.productLabel",
        "graphicalShell.contextLine.contextLabel",
        "graphicalShell.contextLine.statusLabel",
        "graphicalShell.identityHeader.primaryLabel",
        "graphicalShell.identityHeader.secondaryLabel",
        "graphicalShell.workspace.mainRegion",
        "graphicalShell.workspace.mainLabel",
        "graphicalShell.workspace.sideRegion",
        "graphicalShell.workspace.sideLabel",
        "graphicalShell.workspace.diagnostic.context",
        "graphicalShell.workspace.diagnostic.body",
        "graphicalShell.workspace.diagnostic.selectedLine",
        "graphicalShell.workspace.diagnostic.stateHash",
        "graphicalShell.footer.pathLabel",
        "graphicalShell.footer.actionHints[]",
        "projection.context",
        "projection.body",
        "projection.selectedLine",
        "projection.stateHash",
        "parameters.generation",
        "parameters.graphRevision",
        "parameters.patchCount",
        "parameters.patches[].patchId",
        "parameters.patches[].envelope.attackMilliseconds",
        "parameters.patches[].envelope.decayMilliseconds",
        "parameters.patches[].envelope.sustain",
        "parameters.patches[].envelope.releaseMilliseconds",
        "parameters.patches[].voiceLimit",
        "parameters.patches[].instrument.count",
        "parameters.patches[].instrument.values[]",
        "parameters.patches[].effects[].active",
        "parameters.patches[].effects[].slotId",
        "parameters.patches[].effects[].scalarCount",
        "parameters.patches[].effects[].scalars[]",
        "parameters.patches[].output.trackId",
        "parameters.patches[].output.trimGainDb",
        "parameters.patches[].sends[]",
        "parameters.mixerTracks[].levelDb",
        "parameters.mixerTracks[].pan",
        "parameters.mixerTracks[].mute",
        "parameters.mixerTracks[].solo",
        "parameters.mixerTracks[].sends[]",
        "parameters.returns[].active",
        "parameters.returns[].slotId",
        "parameters.returns[].scalarCount",
        "parameters.returns[].scalars[]",
        "parameters.returns[].returnLevel",
        "parameters.returns[].tail[].active",
        "parameters.returns[].tail[].slotId",
        "parameters.returns[].tail[].scalarCount",
        "parameters.returns[].tail[].scalars[]",
        "parameters.global.masterGainDb",
        "controller.devices[].id",
        "controller.devices[].name",
        "controller.bindings.version",
        "controller.bindings.buttons[]",
        "controller.capture",
        "controller.ready",
        "controller.preferenceStatus.kind",
        "controller.preferenceStatus.failure",
        "controller.backendFailure",
        "midiInput.active",
        "midiInput.active.identity.identity",
        "midiInput.active.identity.identitySchema",
        "midiInput.active.revision",
        "midiInput.connectionIntent",
        "midiInput.lastRequestId",
        "midiInput.lastRevision",
        "midiInput.lastScanId",
        "midiInput.operationFailure",
        "midiInput.operationFailure.identity.identity",
        "midiInput.operationFailure.identity.identitySchema",
        "midiInput.operationFailure.kind",
        "midiInput.operationFailure.revision",
        "midiInput.operationFailureIdentity",
        "midiInput.operationFailureIdentity.identity",
        "midiInput.operationFailureIdentity.identitySchema",
        "midiInput.preferenceFailure",
        "midiInput.preferenceFailure.kind",
        "midiInput.preferenceLoaded",
        "midiInput.registry[].descriptor.displayName",
        "midiInput.registry[].descriptor.id.identity",
        "midiInput.registry[].descriptor.id.identitySchema",
        "midiInput.registry[].descriptor.portFacts.manufacturer",
        "midiInput.registry[].descriptor.portFacts.product",
        "midiInput.registry[].descriptor.portFacts.transport",
        "midiInput.registry[].present",
        "midiInput.requestPrepared",
        "midiInput.requested",
        "midiInput.requested.identity.identity",
        "midiInput.requested.identity.identitySchema",
        "midiInput.requested.requestId",
        "midiInput.requested.revision",
        "midiInput.scan.failure.kind",
        "midiInput.scan.kind",
        "midiInput.scan.lastSuccessfulScanId",
        "midiInput.scan.scanId",
        "midiInput.selected",
        "midiInput.selected.identity",
        "midiInput.selected.identitySchema",
        "midiInput.selected.lastKnownDisplayName",
        "midiInput.shuttingDown",
    ];

    /// Returns the production-owned stable StateTree property surface.
    pub const fn serialized_property_descriptor() -> &'static [&'static str] {
        Self::SERIALIZED_PROPERTY_DESCRIPTOR
    }

    /// Returns the complete production-owned normalized nested schema.
    pub fn serialized_leaf_descriptor() -> &'static [&'static str] {
        static DESCRIPTOR: OnceLock<Vec<&'static str>> = OnceLock::new();
        DESCRIPTOR.get_or_init(|| {
            let mut descriptor = Self::BASE_SERIALIZED_LEAF_DESCRIPTOR
                .iter()
                .copied()
                .filter(|path| {
                    !path.starts_with("interaction.")
                        && !path.starts_with("graphicalShell.")
                        && !path.starts_with("patchPage.")
                })
                .collect::<Vec<_>>();
            descriptor.extend(
                Self::BASE_SERIALIZED_LEAF_DESCRIPTOR
                    .iter()
                    .filter(|path| path.starts_with("capabilities.descriptors[]"))
                    .map(|path| {
                        Box::leak(
                            path.replacen(
                                "capabilities.descriptors[]",
                                "capabilities.assetDescriptors[]",
                                1,
                            )
                            .into_boxed_str(),
                        ) as &'static str
                    }),
            );
            descriptor.extend([
                "interaction.activeFocus.capabilityId",
                "interaction.activeFocus.capabilityId.id",
                "interaction.activeFocus.capabilityId.kind",
                "interaction.activeFocus.context",
                "interaction.activeFocus.controlId.id",
                "interaction.activeFocus.controlId.id.bus",
                "interaction.activeFocus.controlId.id.entry",
                "interaction.activeFocus.controlId.id.id",
                "interaction.activeFocus.controlId.id.identity",
                "interaction.activeFocus.controlId.id.identitySchema",
                "interaction.activeFocus.controlId.id.kind",
                "interaction.activeFocus.controlId.id.parameter",
                "interaction.activeFocus.controlId.id.slotId",
                "interaction.activeFocus.controlId.id.trackId",
                "interaction.activeFocus.controlId.kind",
                "interaction.activeFocus.modalId",
                "interaction.activeFocus.patchId",
                "interaction.activeFocus.surface",
                "interaction.detailSubject",
                "interaction.detailSubject.capabilityId",
                "interaction.detailSubject.kind",
                "interaction.detailSubject.slotId",
                "interaction.mode",
                "interaction.rememberedMixerMain.capabilityId",
                "interaction.rememberedMixerMain.context",
                "interaction.rememberedMixerMain.controlId.id.kind",
                "interaction.rememberedMixerMain.controlId.id.parameter",
                "interaction.rememberedMixerMain.controlId.id.trackId",
                "interaction.rememberedMixerMain.controlId.kind",
                "interaction.rememberedMixerMain.modalId",
                "interaction.rememberedMixerMain.patchId",
                "interaction.rememberedMixerMain.surface",
                "interaction.rememberedPatchMain",
                "interaction.rememberedPatchMain.capabilityId",
                "interaction.rememberedPatchMain.context",
                "interaction.rememberedPatchMain.controlId.id",
                "interaction.rememberedPatchMain.controlId.kind",
                "interaction.rememberedPatchMain.modalId",
                "interaction.rememberedPatchMain.patchId",
                "interaction.rememberedPatchMain.surface",
                "interaction.rememberedSend.capabilityId",
                "interaction.rememberedSend.capabilityId.id",
                "interaction.rememberedSend.capabilityId.kind",
                "interaction.rememberedSend.context",
                "interaction.rememberedSend.controlId.id.bus",
                "interaction.rememberedSend.controlId.id.kind",
                "interaction.rememberedSend.controlId.id.parameter",
                "interaction.rememberedSend.controlId.id.slotId",
                "interaction.rememberedSend.controlId.kind",
                "interaction.rememberedSend.modalId",
                "interaction.rememberedSend.patchId",
                "interaction.rememberedSend.surface",
                "interaction.sendChoiceOrigin",
                "interaction.sendChoiceOrigin.capabilityId",
                "interaction.sendChoiceOrigin.context",
                "interaction.sendChoiceOrigin.controlId.id.bus",
                "interaction.sendChoiceOrigin.controlId.id.kind",
                "interaction.sendChoiceOrigin.controlId.id.slotId",
                "interaction.sendChoiceOrigin.controlId.kind",
                "interaction.sendChoiceOrigin.modalId",
                "interaction.sendChoiceOrigin.patchId",
                "interaction.sendChoiceOrigin.surface",
                "interaction.sendNameEditing",
                "interaction.returnPath",
                "interaction.returnPath.enteredSurface",
                "interaction.returnPath.origin.capabilityId",
                "interaction.returnPath.origin.capabilityId.id",
                "interaction.returnPath.origin.capabilityId.kind",
                "interaction.returnPath.origin.context",
                "interaction.returnPath.origin.controlId.id",
                "interaction.returnPath.origin.controlId.id.bus",
                "interaction.returnPath.origin.controlId.id.kind",
                "interaction.returnPath.origin.controlId.id.parameter",
                "interaction.returnPath.origin.controlId.id.slotId",
                "interaction.returnPath.origin.controlId.id.trackId",
                "interaction.returnPath.origin.controlId.kind",
                "interaction.returnPath.origin.modalId",
                "interaction.returnPath.origin.patchId",
                "interaction.returnPath.origin.surface",
            ]);
            descriptor.extend(
                PatchPageProjection::serialized_leaf_descriptor()
                    .iter()
                    .map(|path| {
                        Box::leak(format!("patchPage.{path}").into_boxed_str()) as &'static str
                    }),
            );
            descriptor.extend(
                GraphicalShellProjection::serialized_leaf_descriptor()
                    .iter()
                    .map(|path| {
                        Box::leak(format!("graphicalShell.{path}").into_boxed_str()) as &'static str
                    }),
            );
            descriptor.sort_unstable();
            descriptor.dedup();
            descriptor
        })
    }

    /// Builds one observation tree from a state snapshot and its GUI/audio
    /// projections.
    pub fn new(
        snapshot: &StateSnapshot,
        graphical_shell: &GraphicalShellProjection,
        projection: &TextProjection,
        parameters: &ParameterSnapshot,
    ) -> Result<Self, StateTreeError> {
        Self::with_patch_page_and_shell(snapshot, None, graphical_shell, projection, parameters)
    }

    /// Builds one observation tree with an explicit optional PATCH page and
    /// the canonical production-window shell.
    pub fn with_patch_page_and_shell(
        snapshot: &StateSnapshot,
        patch_page: Option<&PatchPageProjection>,
        graphical_shell: &GraphicalShellProjection,
        projection: &TextProjection,
        parameters: &ParameterSnapshot,
    ) -> Result<Self, StateTreeError> {
        let state: SerializedState<'_> = serde_json::from_str(snapshot.json())
            .map_err(|_| StateTreeError::StateDeserialization)?;

        Self::from_serialized_state(
            &state,
            snapshot,
            patch_page,
            graphical_shell,
            projection,
            parameters,
        )
    }

    /// Builds the production tree from the canonical typed state that produced
    /// the supplied snapshot. This internal path preserves all coherence checks
    /// without parsing Crest's own JSON back into a second state copy.
    pub(crate) fn from_serialized_state(
        state: &SerializedState<'_>,
        snapshot: &StateSnapshot,
        patch_page: Option<&PatchPageProjection>,
        graphical_shell: &GraphicalShellProjection,
        projection: &TextProjection,
        parameters: &ParameterSnapshot,
    ) -> Result<Self, StateTreeError> {
        if projection.state_hash() != snapshot.hash() {
            return Err(StateTreeError::ProjectionHashMismatch);
        }
        let interaction_context = state.interaction.active_focus.context();
        if state.interaction.active_focus.validate().is_err()
            || state
                .interaction
                .remembered_patch_main
                .as_ref()
                .is_some_and(|path| path.validate().is_err())
            || state.interaction.remembered_mixer_main.validate().is_err()
            || state.interaction.return_path.as_ref().is_some_and(|path| {
                path.origin().validate().is_err()
                    || path.entered_surface() != state.interaction.active_focus.surface()
            })
        {
            return Err(StateTreeError::InvalidInteractionPath);
        }
        if projection.context() != interaction_context {
            return Err(StateTreeError::ContextMismatch);
        }
        if graphical_shell.generation() != state.generation {
            return Err(StateTreeError::GraphicalShellGenerationMismatch);
        }
        if graphical_shell.state_hash() != snapshot.hash() {
            return Err(StateTreeError::GraphicalShellHashMismatch);
        }
        if graphical_shell.context() != interaction_context {
            return Err(StateTreeError::GraphicalShellContextMismatch);
        }
        if graphical_shell.workspace().diagnostic() != projection {
            return Err(StateTreeError::GraphicalShellDiagnosticMismatch);
        }
        let semantic = graphical_shell.semantic_model();
        if semantic.context() != interaction_context
            || semantic.active_surface() != state.interaction.active_focus.surface()
            || semantic.focus_path() != &state.interaction.active_focus
            || semantic.interaction_mode() != state.interaction.mode
            || semantic.return_path() != state.interaction.return_path.as_ref()
        {
            return Err(StateTreeError::SemanticModelMismatch);
        }
        let engine_targeted = state
            .engine_selection
            .correlation()
            .is_some_and(|correlation| {
                matches!(
                    correlation.intent(),
                    crate::control::StructuralEditIntent::ReplaceCapability { .. }
                ) || (correlation.intent().is_append_patch()
                    && state.interaction.active_focus.patch_position()
                        == Some(crate::control::PatchPositionId::TrailingEmpty))
            });
        let projected_engine_status = if engine_targeted {
            state.engine_selection.kind()
        } else {
            crate::control::EngineSelectionStatusKind::Ready
        };
        let projected_engine_correlation = state
            .engine_selection
            .correlation()
            .filter(|_| engine_targeted);
        match (interaction_context, patch_page) {
            (TopLevelContext::Mixer, None) => {}
            // Settings replaces the visible PATCH page while retaining PATCH
            // as the suspended performance context.
            (TopLevelContext::Patch, None)
                if state.interaction.active_focus.surface().is_system() => {}
            (TopLevelContext::Patch, Some(page))
                if !state.interaction.active_focus.surface().is_system()
                    && page.context() == TopLevelContext::Patch
                    && page.state_hash() == snapshot.hash()
                    && page.patch().id() == state.interaction.active_focus.patch_id()
                    && Some(page.focused_control_id())
                        == match state.interaction.active_focus.control_id() {
                            SemanticControlId::Patch(control) => Some(control.clone()),
                            SemanticControlId::Modal(_) => {
                                state.interaction.return_path.as_ref().and_then(|path| {
                                    match path.origin().control_id() {
                                        SemanticControlId::Patch(control) => Some(control.clone()),
                                        _ => None,
                                    }
                                })
                            }
                            _ => None,
                        }
                    && page.engine().status() == projected_engine_status
                    && page.engine().active_graph_revision()
                        == state.engine_selection.active_graph_revision()
                    && page.engine().requested_capability_id()
                        == projected_engine_correlation
                            .and_then(|correlation| correlation.target_capability_id())
                    && page.engine().request_id()
                        == projected_engine_correlation
                            .map(|correlation| correlation.request_id())
                    && page.engine().target_graph_revision()
                        == projected_engine_correlation
                            .and_then(|correlation| correlation.target_graph_revision())
                    && page.engine().failure()
                        == engine_targeted
                            .then(|| state.engine_selection.failure())
                            .flatten() => {}
            _ => return Err(StateTreeError::PatchPageMismatch),
        }
        validate_parameter_projection(state, parameters)?;

        let serializable =
            SerializableStateTree::new(state, patch_page, graphical_shell, projection, parameters);
        let json =
            serde_json::to_string(&serializable).map_err(|_| StateTreeError::Serialization)?;

        Ok(Self {
            json: TreeJson::Ready(Arc::from(json)),
            generation: state.generation,
            graph_revision: parameters.graph_revision(),
            patch_count: state.patches.len(),
            selected_line: projection.selected_line(),
            context: interaction_context,
            patch_page_position: patch_page.map(|page| {
                page.patch().id().map_or(
                    crate::control::PatchPositionId::TrailingEmpty,
                    crate::control::PatchPositionId::Created,
                )
            }),
            state_hash: Arc::from(snapshot.hash()),
        })
    }

    /// Advances a coherent tree whose accepted state changed only by MIDI generation.
    pub(crate) fn with_midi_generation(
        &self,
        snapshot: &StateSnapshot,
        patch_page: Option<&PatchPageProjection>,
        graphical_shell: &GraphicalShellProjection,
        projection: &TextProjection,
        parameters: &ParameterSnapshot,
    ) -> Result<Self, StateTreeError> {
        if projection.state_hash() != snapshot.hash() {
            return Err(StateTreeError::ProjectionHashMismatch);
        }
        if parameters.generation() != self.generation.saturating_add(1) {
            return Err(StateTreeError::GenerationMismatch);
        }
        if parameters.graph_revision() != self.graph_revision {
            return Err(StateTreeError::GraphRevisionMismatch);
        }
        if parameters.patch_count() != self.patch_count {
            return Err(StateTreeError::PatchCountMismatch);
        }
        if projection.selected_line() != self.selected_line {
            return Err(StateTreeError::MidiTemplateMismatch);
        }
        if projection.context() != self.context {
            return Err(StateTreeError::ContextMismatch);
        }
        if graphical_shell.generation() != parameters.generation() {
            return Err(StateTreeError::GraphicalShellGenerationMismatch);
        }
        if graphical_shell.state_hash() != snapshot.hash() {
            return Err(StateTreeError::GraphicalShellHashMismatch);
        }
        if graphical_shell.context() != self.context {
            return Err(StateTreeError::GraphicalShellContextMismatch);
        }
        if graphical_shell.workspace().diagnostic() != projection {
            return Err(StateTreeError::GraphicalShellDiagnosticMismatch);
        }
        match (self.context, self.patch_page_position, patch_page) {
            (TopLevelContext::Mixer, None, None) => {}
            (TopLevelContext::Patch, None, None)
                if graphical_shell
                    .semantic_model()
                    .active_surface()
                    .is_system() => {}
            (TopLevelContext::Patch, Some(expected), Some(page))
                if page.patch().id() == expected.patch_id()
                    && page.context() == TopLevelContext::Patch
                    && page.state_hash() == snapshot.hash() => {}
            _ => return Err(StateTreeError::PatchPageMismatch),
        }

        let template = match &self.json {
            TreeJson::MidiGeneration { template, .. } => Arc::clone(template),
            TreeJson::Ready(json) => Arc::new(
                MidiTreeTemplate::from_json(json, self.generation, self.state_hash())
                    .ok_or(StateTreeError::MidiTemplateMismatch)?,
            ),
        };
        let generation = parameters.generation();
        let state_hash: Arc<str> = Arc::from(snapshot.hash());

        Ok(Self {
            json: TreeJson::MidiGeneration {
                template,
                generation,
                state_hash: Arc::clone(&state_hash),
                rendered: Arc::new(OnceLock::new()),
            },
            generation,
            graph_revision: self.graph_revision,
            patch_count: self.patch_count,
            selected_line: self.selected_line,
            context: self.context,
            patch_page_position: self.patch_page_position,
            state_hash,
        })
    }

    /// Returns the stable JSON schema version.
    pub const fn schema_version(&self) -> u32 {
        Self::SCHEMA_VERSION
    }

    /// Returns the accepted AppState generation represented by the tree.
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns the prepared graph revision targeted by the parameter branch.
    pub const fn graph_revision(&self) -> GraphRevision {
        self.graph_revision
    }

    /// Returns the number of installed Patches represented by both projections.
    pub const fn patch_count(&self) -> usize {
        self.patch_count
    }

    /// Returns the selected line in the included text projection.
    pub const fn selected_line(&self) -> usize {
        self.selected_line
    }

    /// Returns the canonical StateSnapshot identity included in the tree.
    pub fn state_hash(&self) -> &str {
        &self.state_hash
    }

    /// Returns deterministic JSON containing every control and projection
    /// property.
    pub fn json(&self) -> &str {
        match &self.json {
            TreeJson::Ready(json) => json,
            TreeJson::MidiGeneration {
                template,
                generation,
                state_hash,
                rendered,
            } => rendered.get_or_init(|| template.render(*generation, state_hash)),
        }
    }

    /// Consumes the value and returns its deterministic JSON representation.
    pub fn into_json(self) -> String {
        self.json().to_owned()
    }
}

impl PartialEq for StateTree {
    fn eq(&self, other: &Self) -> bool {
        self.generation == other.generation
            && self.graph_revision == other.graph_revision
            && self.patch_count == other.patch_count
            && self.selected_line == other.selected_line
            && self.context == other.context
            && self.patch_page_position == other.patch_page_position
            && self.state_hash == other.state_hash
            && self.json() == other.json()
    }
}

impl MidiTreeTemplate {
    fn from_json(json: &str, generation: u64, state_hash: &str) -> Option<Self> {
        // The version is read out of the document and checked, not pinned as a
        // literal whose *length* silently drives every offset below it. A
        // pinned "12" kept working at 13 only because both are two digits.
        const ROOT_PREFIX: &str = "{\"schemaVersion\":";
        const GENERATION_KEY: &str = ",\"generation\":";
        const SHELL_MARKER: &str = "\"graphicalShell\":{\"generation\":";
        const SEMANTIC_MARKER: &str = "\"semanticModel\":{\"generation\":";
        const PARAMETER_MARKER: &str = "\"parameters\":{\"generation\":";

        let after_prefix = json.strip_prefix(ROOT_PREFIX)?;
        let version_end = after_prefix.find(GENERATION_KEY)?;
        if after_prefix.get(..version_end)?.parse::<u32>().ok()? != StateTree::SCHEMA_VERSION {
            return None;
        }
        let root_start = ROOT_PREFIX.len() + version_end + GENERATION_KEY.len();
        let root_end = json.get(root_start..)?.find(',')? + root_start;
        if json.get(root_start..root_end)?.parse::<u64>().ok()? != generation {
            return None;
        }

        let shell_marker_start = json.get(root_end..)?.find(SHELL_MARKER)? + root_end;
        let shell_start = shell_marker_start + SHELL_MARKER.len();
        let shell_end = json.get(shell_start..)?.find(',')? + shell_start;
        if json.get(shell_start..shell_end)?.parse::<u64>().ok()? != generation {
            return None;
        }

        let semantic_marker_start = json.get(shell_end..)?.find(SEMANTIC_MARKER)? + shell_end;
        let semantic_start = semantic_marker_start + SEMANTIC_MARKER.len();
        let semantic_end = json.get(semantic_start..)?.find(',')? + semantic_start;
        if json
            .get(semantic_start..semantic_end)?
            .parse::<u64>()
            .ok()?
            != generation
        {
            return None;
        }

        let parameter_marker_start =
            json.get(semantic_end..)?.find(PARAMETER_MARKER)? + semantic_end;
        let parameter_start = parameter_marker_start + PARAMETER_MARKER.len();
        let parameter_end = json.get(parameter_start..)?.find(',')? + parameter_start;
        if json
            .get(parameter_start..parameter_end)?
            .parse::<u64>()
            .ok()?
            != generation
        {
            return None;
        }

        let between_root_and_shell_generation = json.get(root_end..shell_start)?;
        let between_shell_and_semantic_generation = json.get(shell_end..semantic_start)?;
        let between_semantic_and_parameter_generation = json.get(semantic_end..parameter_start)?;
        let after_parameter_generation = json.get(parameter_end..)?;
        if !between_root_and_shell_generation.contains(state_hash)
            && !between_shell_and_semantic_generation.contains(state_hash)
            && !between_semantic_and_parameter_generation.contains(state_hash)
            && !after_parameter_generation.contains(state_hash)
        {
            return None;
        }

        Some(Self {
            before_root_generation: Arc::from(json.get(..root_start)?),
            between_root_and_shell_generation: Arc::from(between_root_and_shell_generation),
            between_shell_and_semantic_generation: Arc::from(between_shell_and_semantic_generation),
            between_semantic_and_parameter_generation: Arc::from(
                between_semantic_and_parameter_generation,
            ),
            after_parameter_generation: Arc::from(after_parameter_generation),
            source_state_hash: Arc::from(state_hash),
        })
    }

    fn render(&self, generation: u64, state_hash: &str) -> String {
        let generation = generation.to_string();
        let mut json = String::with_capacity(
            self.before_root_generation.len()
                + self.between_root_and_shell_generation.len()
                + self.between_shell_and_semantic_generation.len()
                + self.between_semantic_and_parameter_generation.len()
                + self.after_parameter_generation.len()
                + generation.len() * 4,
        );
        json.push_str(&self.before_root_generation);
        json.push_str(&generation);
        push_with_state_hash(
            &mut json,
            &self.between_root_and_shell_generation,
            &self.source_state_hash,
            state_hash,
        );
        json.push_str(&generation);
        push_with_state_hash(
            &mut json,
            &self.between_shell_and_semantic_generation,
            &self.source_state_hash,
            state_hash,
        );
        json.push_str(&generation);
        push_with_state_hash(
            &mut json,
            &self.between_semantic_and_parameter_generation,
            &self.source_state_hash,
            state_hash,
        );
        json.push_str(&generation);
        push_with_state_hash(
            &mut json,
            &self.after_parameter_generation,
            &self.source_state_hash,
            state_hash,
        );
        json
    }
}

fn push_with_state_hash(
    output: &mut String,
    template: &str,
    source_state_hash: &str,
    state_hash: &str,
) {
    let mut segments = template.split(source_state_hash);
    if let Some(first) = segments.next() {
        output.push_str(first);
    }
    for segment in segments {
        output.push_str(state_hash);
        output.push_str(segment);
    }
}

fn validate_parameter_projection(
    state: &SerializedState<'_>,
    parameters: &ParameterSnapshot,
) -> Result<(), StateTreeError> {
    if parameters.generation() != state.generation {
        return Err(StateTreeError::GenerationMismatch);
    }
    if parameters.graph_revision() != state.engine_selection.projection_graph_revision() {
        return Err(StateTreeError::GraphRevisionMismatch);
    }
    if parameters.patch_count() != state.patches.len() {
        return Err(StateTreeError::PatchCountMismatch);
    }

    for (index, (state_patch, parameter_patch)) in
        state.patches.iter().zip(parameters.patches()).enumerate()
    {
        if parameter_patch.patch_id().map(|patch_id| patch_id.value()) != Some(state_patch.id) {
            return Err(StateTreeError::PatchIdentityMismatch { index });
        }
        if state_patch.output != parameter_patch.output() {
            return Err(StateTreeError::PatchParametersMismatch { index });
        }
        if state_patch.sends.as_ref() != parameter_patch.sends() {
            return Err(StateTreeError::PatchParametersMismatch { index });
        }
        if state_patch.envelope != *parameter_patch.envelope() {
            return Err(StateTreeError::PatchParametersMismatch { index });
        }
        let descriptor = state
            .capabilities
            .descriptor_for_config(&state_patch.instrument)
            .ok_or(StateTreeError::PatchParametersMismatch { index })?;
        if parameter_patch.instrument().count() != descriptor.scalar_parameter_count()
            || descriptor
                .scalar_parameters()
                .enumerate()
                .any(|(scalar_index, spec)| {
                    let Some(value) = state_patch.instrument.value(spec.id()) else {
                        return true;
                    };
                    spec.scalar_value(value).ok()
                        != parameter_patch.instrument().value(scalar_index)
                })
        {
            return Err(StateTreeError::PatchParametersMismatch { index });
        }
        // The serialized state carries the occupied configs compacted in slot
        // order; the snapshot carries one entry per ordered position. The
        // active entries, in position order, must correspond one-to-one with
        // the compacted configs — same instance identity, scalar layout, and
        // scalar values — and every other position must be inactive.
        let active_entries: Vec<_> = parameter_patch
            .effects()
            .iter()
            .filter(|entry| entry.is_active())
            .collect();
        let configs = state_patch.post_effects.as_ref();
        if active_entries.len() != configs.len() {
            return Err(StateTreeError::PatchParametersMismatch { index });
        }
        for (entry, config) in active_entries.iter().zip(configs) {
            let descriptor = state
                .effects
                .descriptor(config.capability_id())
                .ok_or(StateTreeError::PatchParametersMismatch { index })?;
            if entry.slot_id() != Some(config.slot_id())
                || entry.scalar_count() != descriptor.scalar_parameter_count()
                || descriptor
                    .scalar_parameters()
                    .enumerate()
                    .any(|(scalar_index, spec)| {
                        let Some(value) = config.value(spec.id()) else {
                            return true;
                        };
                        spec.scalar_value(value).ok() != entry.scalar(scalar_index)
                    })
            {
                return Err(StateTreeError::PatchParametersMismatch { index });
            }
        }
    }

    if let Some(index) = state
        .mixer
        .tracks()
        .iter()
        .zip(parameters.mixer_tracks())
        .position(|(state_track, parameter_track)| state_track != parameter_track)
    {
        return Err(StateTreeError::MixerParametersMismatch { index });
    }

    if TreeGlobalParameters::from(&state.global) != TreeGlobalParameters::from(parameters.global())
    {
        return Err(StateTreeError::GlobalParametersMismatch);
    }

    // Every live return entry must attest exactly the serialized return-owned
    // state at its bus: occupancy, instance identity, scalar layout and
    // values, and the return-owned level.
    if !state.returns.is_complete() || state.returns.entries().len() != parameters.returns().len() {
        return Err(StateTreeError::ReturnParametersMismatch { index: 0 });
    }
    for (index, (serialized, live)) in state
        .returns
        .entries()
        .iter()
        .zip(parameters.returns())
        .enumerate()
    {
        // Old observational snapshots carry only the first effect.
        let configs: Vec<_> = if serialized.effects.is_empty() {
            serialized.effect.iter().collect()
        } else {
            serialized.effects.iter().collect()
        };
        if configs.len() != live.effect_count()
            || (!configs.is_empty() && live.return_level() != serialized.return_level)
        {
            return Err(StateTreeError::ReturnParametersMismatch { index });
        }
        for (config, values) in configs.iter().zip(live.effects()) {
            let descriptor = state
                .effects
                .descriptor(config.capability_id())
                .ok_or(StateTreeError::ReturnParametersMismatch { index })?;
            if values.slot_id() != Some(config.slot_id())
                || values.scalar_count() != descriptor.scalar_parameter_count()
                || descriptor
                    .scalar_parameters()
                    .enumerate()
                    .any(|(scalar_index, spec)| {
                        config
                            .value(spec.id())
                            .and_then(|value| spec.scalar_value(value).ok())
                            != values.scalar(scalar_index)
                    })
            {
                return Err(StateTreeError::ReturnParametersMismatch { index });
            }
        }
    }

    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SerializableStateTree<'a> {
    schema_version: u32,
    generation: u64,
    capabilities: &'a CapabilityRegistry,
    effects: &'a EffectCapabilityRegistry,
    patches: Vec<TreePatch<'a>>,
    mixer: MixerState,
    global: TreeGlobalParameters,
    returns: &'a crate::control::serialized_state::SerializedBusReturns,
    interaction: &'a SerializedInteractionState,
    engine_selection: &'a EngineSelectionStatus,
    midi_input: &'a crate::control::MidiInputState,
    controller: &'a crate::control::ControllerState,
    patch_page: Option<&'a PatchPageProjection>,
    graphical_shell: &'a GraphicalShellProjection,
    projection: &'a TextProjection,
    parameters: &'a ParameterSnapshot,
}

impl<'a> SerializableStateTree<'a> {
    fn new(
        state: &'a SerializedState<'_>,
        patch_page: Option<&'a PatchPageProjection>,
        graphical_shell: &'a GraphicalShellProjection,
        projection: &'a TextProjection,
        parameters: &'a ParameterSnapshot,
    ) -> Self {
        Self {
            schema_version: StateTree::SCHEMA_VERSION,
            generation: state.generation,
            capabilities: state.capabilities.as_ref(),
            effects: state.effects.as_ref(),
            patches: state.patches.iter().map(TreePatch::from).collect(),
            mixer: state.mixer.clone(),
            global: TreeGlobalParameters::from(&state.global),
            returns: &state.returns,
            interaction: &state.interaction,
            engine_selection: &state.engine_selection,
            midi_input: &state.midi_input,
            controller: &state.controller,
            patch_page,
            graphical_shell,
            projection,
            parameters,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TreePatch<'a> {
    id: u32,
    name: &'a str,
    channel: u8,
    instrument: &'a InstrumentConfig,
    post_effects: &'a [PostEffectConfig],
    envelope: VoiceEnvelope,
    output: PatchOutput,
    sends: &'a [f32],
}

impl<'a> From<&'a SerializedPatch<'_>> for TreePatch<'a> {
    fn from(patch: &'a SerializedPatch<'_>) -> Self {
        Self {
            id: patch.id,
            name: patch.name.as_ref(),
            channel: patch.channel,
            instrument: patch.instrument.as_ref(),
            post_effects: patch.post_effects.as_ref(),
            envelope: patch.envelope,
            output: patch.output,
            sends: patch.sends.as_ref(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TreeGlobalParameters {
    master_gain_db: f32,
}

impl From<&SerializedGlobalParameters> for TreeGlobalParameters {
    fn from(parameters: &SerializedGlobalParameters) -> Self {
        Self {
            master_gain_db: parameters.master_gain_db,
        }
    }
}

impl From<&GlobalParameters> for TreeGlobalParameters {
    fn from(parameters: &GlobalParameters) -> Self {
        Self {
            master_gain_db: parameters.master_gain_db(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{StateTree, StateTreeError};
    use crate::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID;
    use crate::control::state_snapshot::StateSnapshot;
    use crate::control::text_projection::TextProjection;
    use crate::control::{
        GraphicalShellProjection, ShellContextLine, ShellFooter, ShellIdentityHeader,
    };
    use crate::kernel::patch_id::PatchId;
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_state::MixerState;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::parameter_snapshot::{ParameterSnapshot, RtPatchParameters};
    use crate::synth::sound_font_instrument::SoundFontInstrument;
    use crate::testing::automatic_midi_test::create_soundfont_config;
    use serde_json::{json, Value};

    fn snapshot() -> StateSnapshot {
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        StateSnapshot::new(
            json!({
                "generation": 42,
                "capabilities": provider.registry().unwrap(),
                "effects":
                    crate::adapter::production_effects::production_effect_registry().unwrap(),
                "patches": [
                    {
                        "id": 7,
                        "name": "Lead",
                        "channel": 2,
                        "sends": vec![0.0; crate::mixer::bus_id::DEFAULT_BUS_RETURNS],
                        "instrument": create_soundfont_config(
                            &provider,
                            SoundFontInstrument::new(0, 80, false).unwrap()
                        ).unwrap(),
                        "output": {
                            "trackId": 2,
                            "trimGainDb": -6.0
                        }
                    },
                    {
                        "id": 9,
                        "name": "Drums",
                        "channel": 9,
                        "sends": vec![0.0; crate::mixer::bus_id::DEFAULT_BUS_RETURNS],
                        "instrument": create_soundfont_config(
                            &provider,
                            SoundFontInstrument::new(128, 0, true).unwrap()
                        ).unwrap(),
                        "output": {
                            "trackId": 9,
                            "trimGainDb": -12.0
                        }
                    }
                ],
                "mixer": MixerState::default(),
                "global": {
                    "masterGainDb": -3.0
                },
                "returns": fixture_returns_json(),
                "interaction": crate::control::serialized_state::SerializedInteractionState::default(),
                "engineSelection": {
                    "kind": "ready",
                    "activeGraphRevision": 7,
                    "correlation": null,
                    "failure": null
                }
            })
            .to_string(),
        )
    }

    fn global() -> GlobalParameters {
        GlobalParameters::new(-3.0).unwrap()
    }

    /// The fixture bank: production reverb defaults on bus 0 at level 0.25 and
    /// production delay defaults on bus 1 at level 0.2, remaining buses empty.
    fn fixture_bank() -> crate::mixer::bus_return::BusReturnBank {
        let registry = crate::adapter::production_effects::production_effect_registry().unwrap();
        let mut bank =
            crate::adapter::production_effects::production_default_bus_returns(&registry).unwrap();
        bank.set_return_level(crate::mixer::bus_id::BusId::new(0).unwrap(), 0.25)
            .unwrap();
        bank.set_return_level(crate::mixer::bus_id::BusId::new(1).unwrap(), 0.2)
            .unwrap();
        bank
    }

    fn fixture_returns_json() -> Value {
        let bank = fixture_bank();
        Value::Array(
            bank.returns()
                .iter()
                .map(|entry| {
                    json!({
                        "effect": entry.effect(),
                        "returnLevel": entry.return_level(),
                    })
                })
                .collect(),
        )
    }

    fn parameters() -> ParameterSnapshot {
        let registry = crate::adapter::production_effects::production_effect_registry().unwrap();
        let returns = ParameterSnapshot::project_returns(&registry, &fixture_bank()).unwrap();
        ParameterSnapshot::for_graph(
            42,
            crate::real_time::GraphRevision::new(7).unwrap(),
            global(),
            MixerState::default(),
            &[
                RtPatchParameters::new(
                    PatchId::new(7).unwrap(),
                    PatchOutput::new(MixerTrackId::new(2).unwrap(), -6.0).unwrap(),
                ),
                RtPatchParameters::new(
                    PatchId::new(9).unwrap(),
                    PatchOutput::new(MixerTrackId::new(9).unwrap(), -12.0).unwrap(),
                ),
            ],
        )
        .unwrap()
        .with_returns(returns)
    }

    /// A three-line MIXER body whose selected line is line 1, written in
    /// indexed vocabulary (`sends[n]`, the serialized-snapshot leaf name) and
    /// never a per-effect name. The rendered MIXER line is spelled `send[n]=`
    /// (see `render_mixer_text`); the fixture does not have to match it,
    /// because the tree assertions care only about this body travelling
    /// verbatim and the selection index travelling with it — nothing here
    /// parses the text.
    const PROJECTION_BODY: &str = "TRACK T00 routedPatches=[01:Lead]\n> sends[0]=0.4\nGLOBAL";

    fn projection(snapshot: &StateSnapshot) -> TextProjection {
        TextProjection::new(PROJECTION_BODY.to_owned(), 1, snapshot.hash().to_owned())
    }

    fn graphical_shell(
        _snapshot: &StateSnapshot,
        projection: &TextProjection,
    ) -> GraphicalShellProjection {
        graphical_shell_with(
            42,
            projection.context(),
            projection.state_hash(),
            projection,
        )
    }

    fn graphical_shell_with(
        generation: u64,
        context: crate::control::TopLevelContext,
        state_hash: &str,
        projection: &TextProjection,
    ) -> GraphicalShellProjection {
        GraphicalShellProjection::new(
            generation,
            state_hash,
            crate::control::SemanticGraphicalViewModel::fixture(generation, state_hash, context),
            ShellContextLine::new("CREST SYNTH", context.label(), "READY"),
            ShellIdentityHeader::new("MIXER · PATCH 09", "Drums · MIDI CH 10"),
            format!("{} WORKSPACE", context.label()),
            match context {
                crate::control::TopLevelContext::Patch => "UTILITY",
                crate::control::TopLevelContext::Mixer => "INSPECTOR",
            },
            projection.clone(),
            ShellFooter::new(
                "MIXER / PATCH 09 / PARAMETER 03",
                vec!["1 MIXER".to_owned(), "2 PATCH".to_owned()],
            ),
        )
        .unwrap()
    }

    fn state_tree(
        snapshot: &StateSnapshot,
        projection: &TextProjection,
        parameters: &ParameterSnapshot,
    ) -> Result<StateTree, StateTreeError> {
        StateTree::new(
            snapshot,
            &graphical_shell(snapshot, projection),
            projection,
            parameters,
        )
    }

    #[test]
    fn serializes_every_state_text_and_audio_property_with_stable_names() {
        let snapshot = snapshot();
        let tree = state_tree(&snapshot, &projection(&snapshot), &parameters()).unwrap();
        let value: Value = serde_json::from_str(tree.json()).unwrap();

        assert_eq!(tree.schema_version(), StateTree::SCHEMA_VERSION);
        assert_eq!(tree.generation(), 42);
        assert_eq!(tree.graph_revision().value(), 7);
        assert_eq!(tree.patch_count(), 2);
        assert_eq!(tree.selected_line(), 1);
        assert_eq!(tree.state_hash(), snapshot.hash());

        let root = value.as_object().unwrap();
        assert_eq!(root.len(), 16);
        for property in [
            "schemaVersion",
            "generation",
            "capabilities",
            "effects",
            "patches",
            "mixer",
            "global",
            "returns",
            "interaction",
            "engineSelection",
            "midiInput",
            "controller",
            "patchPage",
            "graphicalShell",
            "projection",
            "parameters",
        ] {
            assert!(root.contains_key(property), "missing {property}");
        }

        assert_eq!(
            value["patches"][0],
            json!({
                "id": 7,
                "name": "Lead",
                "channel": 2,
                "instrument": value["patches"][0]["instrument"].clone(),
                "sends": vec![0.0; crate::mixer::bus_id::DEFAULT_BUS_RETURNS],
                "postEffects": [],
                "envelope": {
                    "attackMilliseconds": 0.0,
                    "decayMilliseconds": 0.0,
                    "sustain": 1.0,
                    "releaseMilliseconds": 0.0
                },
                "output": {
                    "trackId": 2,
                    "trimGainDb": -6.0
                }
            })
        );
        assert_eq!(
            value["capabilities"]["descriptors"][0]["id"],
            HIDEF_CAPABILITY_ID
        );
        assert_eq!(
            value["patches"][0]["instrument"]["capabilityId"],
            HIDEF_CAPABILITY_ID
        );
        assert_eq!(
            value["patches"][1]["instrument"]["values"][0]["value"],
            json!({"kind": "choice", "value": "sf2.bank-128.program-0"})
        );
        assert_eq!(
            value["patches"][1]["instrument"]["values"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            value["patches"][1]["instrument"]["assetReferences"][0]["reference"],
            json!({"kind": "soundFont", "locator": "./sf2/HiDef.sf2"})
        );
        assert_eq!(
            value["global"],
            json!({
                "masterGainDb": -3.0
            })
        );
        assert_eq!(
            value["returns"].as_array().unwrap().len(),
            fixture_bank().len()
        );
        assert_eq!(value["returns"][0]["returnLevel"], json!(0.25));
        assert_eq!(
            value["returns"][1]["effect"]["capabilityId"],
            json!("effect.delay")
        );
        assert!(value["returns"][2]["effect"].is_null());
        assert_eq!(
            value["interaction"],
            json!({
                "activeFocus": {
                    "context": "mixer",
                    "surface": "mixerMain",
                    "patchId": null,
                    "capabilityId": null,
                    "controlId": {
                        "kind": "mixer",
                        "id": {
                            "kind": "track",
                            "trackId": 0,
                            "parameter": "level"
                        }
                    },
                    "modalId": null
                },
                "rememberedPatchMain": null,
                "rememberedSend": {
                    "context": "mixer",
                    "surface": "sends",
                    "patchId": null,
                    "capabilityId": null,
                    "controlId": {
                        "kind": "send",
                        "id": { "kind": "name", "bus": 0 }
                    },
                    "modalId": null
                },
                "sendChoiceOrigin": null,
                "sendNameEditing": false,
                "rememberedMixerMain": {
                    "context": "mixer",
                    "surface": "mixerMain",
                    "patchId": null,
                    "capabilityId": null,
                    "controlId": {
                        "kind": "mixer",
                        "id": {
                            "kind": "track",
                            "trackId": 0,
                            "parameter": "level"
                        }
                    },
                    "modalId": null
                },
                "mode": "navigate",
                "returnPath": null,
                "detailSubject": null
            })
        );
        assert_eq!(
            value["engineSelection"],
            json!({
                "kind": "ready",
                "activeGraphRevision": 7,
                "correlation": null,
                "failure": null
            })
        );
        assert!(value["patchPage"].is_null());
        assert_eq!(value["graphicalShell"]["generation"], 42);
        assert_eq!(value["graphicalShell"]["context"], "mixer");
        assert_eq!(
            value["graphicalShell"]["workspace"]["diagnostic"],
            value["projection"]
        );
        assert_eq!(value["projection"]["context"], "mixer");
        assert_eq!(value["projection"]["body"], PROJECTION_BODY);
        assert_eq!(value["projection"]["selectedLine"], 1);
        assert_eq!(value["projection"]["stateHash"], snapshot.hash());
        assert_eq!(value["parameters"]["generation"], 42);
        assert_eq!(value["parameters"]["graphRevision"], 7);
        assert_eq!(value["parameters"]["patchCount"], 2);
        let inactive_effect = json!({
            "active": false,
            "slotId": null,
            "scalarCount": 0,
            "scalars": []
        });
        assert_eq!(
            value["parameters"]["patches"][1],
            json!({
                "patchId": 9,
                "envelope": {
                    "attackMilliseconds": 0.0,
                    "decayMilliseconds": 0.0,
                    "sustain": 1.0,
                    "releaseMilliseconds": 0.0
                },
                // The canonical per-Patch ceiling rides the snapshot beside
                // the envelope and is published here, so a measured proof can
                // correlate a refused note with the limit that refused it.
                "voiceLimit": crate::synth::VoiceLimit::MAXIMUM,
                "instrument": {"count": 0, "values": []},
                "effects": [inactive_effect.clone(), inactive_effect.clone(), inactive_effect],
                "sends": vec![0.0; crate::mixer::bus_id::DEFAULT_BUS_RETURNS],
                "output": {
                    "trackId": 9,
                    "trimGainDb": -12.0
                }
            })
        );
        assert_eq!(value["parameters"]["mixerTracks"], value["mixer"]["tracks"]);
        assert_eq!(
            value["parameters"]["patches"][1]["sends"],
            value["patches"][1]["sends"]
        );
        // The snapshot's global object keeps only master gain; the retired
        // reverb/delay values travel as the indexed return entries.
        assert_eq!(
            value["parameters"]["global"],
            json!({"masterGainDb": value["global"]["masterGainDb"]})
        );
        assert_eq!(
            value["parameters"]["returns"].as_array().unwrap().len(),
            fixture_bank().len()
        );
        assert_eq!(
            value["parameters"]["returns"][0]["scalars"][0],
            value["returns"][0]["effect"]["values"][0]["value"]["value"]
        );
        assert_eq!(
            value["parameters"]["returns"][1]["returnLevel"],
            value["returns"][1]["returnLevel"]
        );
    }

    #[test]
    fn serialization_is_deterministic_and_consumable_as_an_owned_value() {
        let snapshot = snapshot();
        let first = state_tree(&snapshot, &projection(&snapshot), &parameters()).unwrap();
        let second = state_tree(&snapshot, &projection(&snapshot), &parameters()).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.json(), second.json());
        assert!(first.json().starts_with(&format!(
            "{{\"schemaVersion\":{},\"generation\":42,\"capabilities\":",
            StateTree::SCHEMA_VERSION
        )));
        assert_eq!(first.clone().into_json(), first.json());
    }

    #[test]
    fn nested_schema_descriptor_covers_registry_configs_and_tagged_values_once() {
        let descriptor = StateTree::serialized_leaf_descriptor();
        let unique = descriptor
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), descriptor.len());
        for required in [
            "capabilities.descriptors[].sections[].parameters[].defaultValue.value.kind",
            "patches[].instrument.values[].value.kind",
            "patches[].instrument.assetReferences[].reference.locator",
            "parameters.graphRevision",
            "parameters.patches[].output.trimGainDb",
            "patches[].sends[]",
            "parameters.patches[].sends[]",
            "parameters.mixerTracks[].levelDb",
        ] {
            assert!(unique.contains(required), "missing {required}");
        }

        let described_parameters = descriptor
            .iter()
            .filter_map(|path| path.strip_prefix("parameters."))
            .collect::<std::collections::BTreeSet<_>>();
        let parameter_descriptor = ParameterSnapshot::serialized_leaf_descriptor()
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(described_parameters, parameter_descriptor);
    }

    #[test]
    fn rejects_a_text_projection_from_another_snapshot() {
        let snapshot = snapshot();
        let other_projection =
            TextProjection::new("unrelated".to_owned(), 0, "other-hash".to_owned());

        assert_eq!(
            state_tree(&snapshot, &other_projection, &parameters()),
            Err(StateTreeError::ProjectionHashMismatch)
        );
    }

    #[test]
    fn rejects_stale_generation_and_mismatched_context_graphical_shells() {
        let snapshot = snapshot();
        let projection = projection(&snapshot);
        let stale = graphical_shell_with(
            41,
            crate::control::TopLevelContext::Mixer,
            snapshot.hash(),
            &projection,
        );
        assert_eq!(
            StateTree::new(&snapshot, &stale, &projection, &parameters()),
            Err(StateTreeError::GraphicalShellGenerationMismatch)
        );

        let patch_diagnostic = TextProjection::for_context(
            crate::control::TopLevelContext::Patch,
            "PATCH diagnostic".to_owned(),
            0,
            snapshot.hash().to_owned(),
        );
        let mismatched_context = graphical_shell_with(
            42,
            crate::control::TopLevelContext::Patch,
            snapshot.hash(),
            &patch_diagnostic,
        );
        assert_eq!(
            StateTree::new(&snapshot, &mismatched_context, &projection, &parameters()),
            Err(StateTreeError::GraphicalShellContextMismatch)
        );
    }

    #[test]
    fn rejects_parameter_generation_patch_order_and_values_that_do_not_match() {
        let snapshot = snapshot();
        let projection = projection(&snapshot);
        let revision = crate::real_time::GraphRevision::new(7).unwrap();
        let wrong_generation = ParameterSnapshot::for_graph(
            43,
            revision,
            global(),
            MixerState::new(parameters().mixer_tracks().clone()),
            parameters().patches(),
        )
        .unwrap();
        assert_eq!(
            state_tree(&snapshot, &projection, &wrong_generation),
            Err(StateTreeError::GenerationMismatch)
        );

        let reversed = [
            parameters().patches()[1].clone(),
            parameters().patches()[0].clone(),
        ];
        let wrong_order = ParameterSnapshot::for_graph(
            42,
            revision,
            global(),
            MixerState::new(parameters().mixer_tracks().clone()),
            &reversed,
        )
        .unwrap();
        assert_eq!(
            state_tree(&snapshot, &projection, &wrong_order),
            Err(StateTreeError::PatchIdentityMismatch { index: 0 })
        );

        let wrong_values = [
            parameters().patches()[0].clone(),
            RtPatchParameters::new(
                PatchId::new(9).unwrap(),
                PatchOutput::new(MixerTrackId::new(9).unwrap(), -10.0).unwrap(),
            ),
        ];
        let wrong_parameters = ParameterSnapshot::for_graph(
            42,
            revision,
            global(),
            MixerState::new(parameters().mixer_tracks().clone()),
            &wrong_values,
        )
        .unwrap();
        assert_eq!(
            state_tree(&snapshot, &projection, &wrong_parameters),
            Err(StateTreeError::PatchParametersMismatch { index: 1 })
        );
    }

    #[test]
    fn rejects_patch_send_level_or_bank_shape_drift() {
        let parameters = parameters();
        for sends in [
            {
                let mut levels = vec![0.0; crate::mixer::bus_id::DEFAULT_BUS_RETURNS];
                levels[3] = 0.75;
                levels
            },
            vec![0.0; crate::mixer::bus_id::DEFAULT_BUS_RETURNS + 1],
        ] {
            let mut state: Value = serde_json::from_str(snapshot().json()).unwrap();
            state["patches"][1]["sends"] = json!(sends);
            let snapshot = StateSnapshot::new(state.to_string());
            assert_eq!(
                state_tree(&snapshot, &projection(&snapshot), &parameters),
                Err(StateTreeError::PatchParametersMismatch { index: 1 })
            );
        }
    }

    #[test]
    fn rejects_malformed_state_and_mismatched_global_parameters() {
        let malformed = StateSnapshot::new("not-json");
        let malformed_projection = projection(&malformed);
        assert_eq!(
            state_tree(&malformed, &malformed_projection, &parameters()),
            Err(StateTreeError::StateDeserialization)
        );

        let snapshot = snapshot();
        let projection = projection(&snapshot);
        let different_global = GlobalParameters::new(-2.0).unwrap();
        let wrong_global = ParameterSnapshot::for_graph(
            42,
            crate::real_time::GraphRevision::new(7).unwrap(),
            different_global,
            MixerState::new(parameters().mixer_tracks().clone()),
            parameters().patches(),
        )
        .unwrap();
        assert_eq!(
            state_tree(&snapshot, &projection, &wrong_global),
            Err(StateTreeError::GlobalParametersMismatch)
        );
    }
}
