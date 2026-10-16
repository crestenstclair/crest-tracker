use crate::control::top_level_context::TopLevelContext;
use crate::control::{
    EngineSelectionFailure, EngineSelectionRequestId, EngineSelectionStatusKind, InteractionMode,
    MidiConnectionRequestId, MidiConnectionRevision, MidiDeviceFailure, MidiInputDescriptor,
    MidiInputDeviceId, MidiInputPreference, MidiInputScanId, PatchControlId, SampleAssetLifecycle,
    SemanticAction, SessionReplacementPayload, StructuralEditIntent, SurfaceId,
};
use crate::kernel::midi_message::MidiMessage;
use crate::kernel::patch_id::PatchId;
use crate::mixer::bus_id::BusId;
use crate::real_time::GraphRevision;
use crate::synth::effect_slot_id::EffectSlotIndex;
use crate::synth::{
    CapabilityId, EffectCapabilityId, FileBrowserFolderId, FileBrowserListing, InstrumentConfig,
    Patch, SampleAssetError,
};
use serde::{Deserialize, Serialize};

/// A semantic direction emitted by an input adapter.
///
/// The meaning of a direction depends on the event variant: navigation moves
/// selection, while adjustment changes the currently selected bounded value.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

impl Direction {
    pub const ALL: [Self; 4] = [Self::Up, Self::Down, Self::Left, Self::Right];
}

/// The payload types carried by non-directional application events.
///
/// Keeping these shapes typed avoids a second string-based event schema in
/// deterministic demos and acceptance tests.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AppEventPayloadShape {
    SendAction,
    ControllerEvent,
    PatchList,
    SessionReplacement,
    PatchId,
    MidiMessage,
    EngineSelectionRequestId,
    CapabilityId,
    GraphRevision,
    InstrumentConfig,
    PreparedSampleVisualization,
    SampleAssetLifecycle,
    FileBrowserFolderId,
    AssetKind,
    FileBrowserListing,
    SampleAssetError,
    AssetImportResult,
    EngineSelectionFailure,
    EngineSelectionStatusKind,
    Boolean,
    StructuralEditIntent,
    InteractionMode,
    SurfaceId,
    EffectSlotIndex,
    BusId,
    OptionalEffectEntry,
    PatchControlId,
    MidiInputPreference,
    MidiInputScanId,
    MidiInputDescriptorList,
    MidiInputDeviceId,
    MidiConnectionRequestId,
    MidiConnectionRevision,
    MidiDeviceFailure,
    OptionalMidiDeviceFailure,
    OptionalMidiConnectionRequestId,
    OptionalMidiConnectionRevision,
}

/// One typed entry in the exhaustive application-event surface.
///
/// Directional variants have one entry for every direction. The remaining
/// variants retain the complete names and types of their payload fields.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AppEventSurfaceDescriptor {
    Send {
        action: AppEventPayloadShape,
    },
    Controller {
        event: AppEventPayloadShape,
    },
    SelectContext {
        context: TopLevelContext,
    },
    SelectPatch {
        direction: Direction,
    },
    NavigatePage {
        direction: Direction,
    },
    Navigate {
        direction: Direction,
    },
    Adjust {
        direction: Direction,
    },
    SetInteractionMode {
        mode: InteractionMode,
    },
    OpenRelated,
    OpenMidiSettings,
    Activate,
    AssetImported {
        selection: AppEventPayloadShape,
    },
    PreviewStart,
    ToggleTestMidi,
    PreviewStop,
    EnterSurface {
        surface: SurfaceId,
    },
    Return,
    InstallPatches {
        patches: AppEventPayloadShape,
    },
    ReplacePersistedSession {
        replacement: AppEventPayloadShape,
    },
    Midi {
        patch_id: AppEventPayloadShape,
        message: AppEventPayloadShape,
    },
    SetPatchOverviewOriginEnabled {
        patch_id: AppEventPayloadShape,
        control: AppEventPayloadShape,
        enabled: AppEventPayloadShape,
    },
    EngineSelectionLifecycleAdvanced {
        request_id: AppEventPayloadShape,
        lifecycle: AppEventPayloadShape,
    },
    EnginePrepared {
        request_id: AppEventPayloadShape,
        patch_id: AppEventPayloadShape,
        intent: AppEventPayloadShape,
        source_capability_id: AppEventPayloadShape,
        target_capability_id: AppEventPayloadShape,
        source_graph_revision: AppEventPayloadShape,
        target_graph_revision: AppEventPayloadShape,
        candidate_config: AppEventPayloadShape,
        prepared_visualization: AppEventPayloadShape,
    },
    SampleAssetLifecycleAdvanced {
        request_id: AppEventPayloadShape,
        lifecycle: AppEventPayloadShape,
    },
    FileCatalogRefreshed {
        asset_kind: AppEventPayloadShape,
        folder: AppEventPayloadShape,
        listing: AppEventPayloadShape,
        failure: AppEventPayloadShape,
    },
    EnginePreparationFailed {
        request_id: AppEventPayloadShape,
        patch_id: AppEventPayloadShape,
        intent: AppEventPayloadShape,
        source_capability_id: AppEventPayloadShape,
        target_capability_id: AppEventPayloadShape,
        source_graph_revision: AppEventPayloadShape,
        target_graph_revision: AppEventPayloadShape,
        failure: AppEventPayloadShape,
    },
    EngineActivationAcknowledged {
        request_id: AppEventPayloadShape,
        intent: AppEventPayloadShape,
        target_graph_revision: AppEventPayloadShape,
        retired_graph_revision: AppEventPayloadShape,
        collected: AppEventPayloadShape,
    },
    SetSlotOccupancy {
        patch_id: AppEventPayloadShape,
        slot: AppEventPayloadShape,
        entry: AppEventPayloadShape,
    },
    SetReturnOccupancy {
        bus: AppEventPayloadShape,
        entry: AppEventPayloadShape,
    },
    TopologyPrepared {
        request_id: AppEventPayloadShape,
        intent: AppEventPayloadShape,
        source_graph_revision: AppEventPayloadShape,
        target_graph_revision: AppEventPayloadShape,
        prepared_visualization: AppEventPayloadShape,
    },
    TopologyPreparationFailed {
        request_id: AppEventPayloadShape,
        intent: AppEventPayloadShape,
        source_graph_revision: AppEventPayloadShape,
        target_graph_revision: AppEventPayloadShape,
        failure: AppEventPayloadShape,
    },
    MidiInputPreferenceRestored {
        preference: AppEventPayloadShape,
        failure: AppEventPayloadShape,
    },
    MidiInputPreferenceStoreFailed {
        failure: AppEventPayloadShape,
    },
    MidiInputScanStarted,
    MidiInputScanSucceeded {
        scan_id: AppEventPayloadShape,
        descriptors: AppEventPayloadShape,
    },
    MidiInputScanFailed {
        scan_id: AppEventPayloadShape,
        failure: AppEventPayloadShape,
    },
    MidiInputConnectRequested {
        identity: AppEventPayloadShape,
    },
    MidiInputConnectionPrepared {
        request_id: AppEventPayloadShape,
        revision: AppEventPayloadShape,
    },
    MidiInputActivationAcknowledged {
        request_id: AppEventPayloadShape,
        revision: AppEventPayloadShape,
    },
    MidiInputDisconnectRequested {
        identity: AppEventPayloadShape,
    },
    MidiInputConnectionLost {
        identity: AppEventPayloadShape,
        revision: AppEventPayloadShape,
    },
    MidiInputOperationFailed {
        identity: AppEventPayloadShape,
        request_id: AppEventPayloadShape,
        revision: AppEventPayloadShape,
        failure: AppEventPayloadShape,
    },
    MidiInputShutdownRequested,
}

const APP_EVENT_SURFACE_DESCRIPTOR: [AppEventSurfaceDescriptor; 57] = [
    AppEventSurfaceDescriptor::Send {
        action: AppEventPayloadShape::SendAction,
    },
    AppEventSurfaceDescriptor::Controller {
        event: AppEventPayloadShape::ControllerEvent,
    },
    AppEventSurfaceDescriptor::AssetImported {
        selection: AppEventPayloadShape::AssetImportResult,
    },
    AppEventSurfaceDescriptor::SelectContext {
        context: TopLevelContext::Patch,
    },
    AppEventSurfaceDescriptor::SelectContext {
        context: TopLevelContext::Mixer,
    },
    AppEventSurfaceDescriptor::SelectPatch {
        direction: Direction::Left,
    },
    AppEventSurfaceDescriptor::SelectPatch {
        direction: Direction::Right,
    },
    AppEventSurfaceDescriptor::NavigatePage {
        direction: Direction::Up,
    },
    AppEventSurfaceDescriptor::NavigatePage {
        direction: Direction::Down,
    },
    AppEventSurfaceDescriptor::NavigatePage {
        direction: Direction::Left,
    },
    AppEventSurfaceDescriptor::NavigatePage {
        direction: Direction::Right,
    },
    AppEventSurfaceDescriptor::Navigate {
        direction: Direction::Up,
    },
    AppEventSurfaceDescriptor::Navigate {
        direction: Direction::Down,
    },
    AppEventSurfaceDescriptor::Navigate {
        direction: Direction::Left,
    },
    AppEventSurfaceDescriptor::Navigate {
        direction: Direction::Right,
    },
    AppEventSurfaceDescriptor::Adjust {
        direction: Direction::Up,
    },
    AppEventSurfaceDescriptor::Adjust {
        direction: Direction::Down,
    },
    AppEventSurfaceDescriptor::Adjust {
        direction: Direction::Left,
    },
    AppEventSurfaceDescriptor::Adjust {
        direction: Direction::Right,
    },
    AppEventSurfaceDescriptor::SetInteractionMode {
        mode: InteractionMode::Navigate,
    },
    AppEventSurfaceDescriptor::SetInteractionMode {
        mode: InteractionMode::Adjust,
    },
    AppEventSurfaceDescriptor::OpenRelated,
    AppEventSurfaceDescriptor::OpenMidiSettings,
    AppEventSurfaceDescriptor::Activate,
    AppEventSurfaceDescriptor::PreviewStart,
    AppEventSurfaceDescriptor::ToggleTestMidi,
    AppEventSurfaceDescriptor::PreviewStop,
    AppEventSurfaceDescriptor::EnterSurface {
        surface: SurfaceId::PatchUtility,
    },
    AppEventSurfaceDescriptor::EnterSurface {
        surface: SurfaceId::PatchDetail,
    },
    AppEventSurfaceDescriptor::EnterSurface {
        surface: SurfaceId::MixerInspector,
    },
    AppEventSurfaceDescriptor::Return,
    AppEventSurfaceDescriptor::InstallPatches {
        patches: AppEventPayloadShape::PatchList,
    },
    AppEventSurfaceDescriptor::ReplacePersistedSession {
        replacement: AppEventPayloadShape::SessionReplacement,
    },
    AppEventSurfaceDescriptor::Midi {
        patch_id: AppEventPayloadShape::PatchId,
        message: AppEventPayloadShape::MidiMessage,
    },
    AppEventSurfaceDescriptor::SetPatchOverviewOriginEnabled {
        patch_id: AppEventPayloadShape::PatchId,
        control: AppEventPayloadShape::PatchControlId,
        enabled: AppEventPayloadShape::Boolean,
    },
    AppEventSurfaceDescriptor::EngineSelectionLifecycleAdvanced {
        request_id: AppEventPayloadShape::EngineSelectionRequestId,
        lifecycle: AppEventPayloadShape::EngineSelectionStatusKind,
    },
    AppEventSurfaceDescriptor::EnginePrepared {
        request_id: AppEventPayloadShape::EngineSelectionRequestId,
        patch_id: AppEventPayloadShape::PatchId,
        intent: AppEventPayloadShape::StructuralEditIntent,
        source_capability_id: AppEventPayloadShape::CapabilityId,
        target_capability_id: AppEventPayloadShape::CapabilityId,
        source_graph_revision: AppEventPayloadShape::GraphRevision,
        target_graph_revision: AppEventPayloadShape::GraphRevision,
        candidate_config: AppEventPayloadShape::InstrumentConfig,
        prepared_visualization: AppEventPayloadShape::PreparedSampleVisualization,
    },
    AppEventSurfaceDescriptor::SampleAssetLifecycleAdvanced {
        request_id: AppEventPayloadShape::EngineSelectionRequestId,
        lifecycle: AppEventPayloadShape::SampleAssetLifecycle,
    },
    AppEventSurfaceDescriptor::FileCatalogRefreshed {
        asset_kind: AppEventPayloadShape::AssetKind,
        folder: AppEventPayloadShape::FileBrowserFolderId,
        listing: AppEventPayloadShape::FileBrowserListing,
        failure: AppEventPayloadShape::SampleAssetError,
    },
    AppEventSurfaceDescriptor::EnginePreparationFailed {
        request_id: AppEventPayloadShape::EngineSelectionRequestId,
        intent: AppEventPayloadShape::StructuralEditIntent,
        patch_id: AppEventPayloadShape::PatchId,
        source_capability_id: AppEventPayloadShape::CapabilityId,
        target_capability_id: AppEventPayloadShape::CapabilityId,
        source_graph_revision: AppEventPayloadShape::GraphRevision,
        target_graph_revision: AppEventPayloadShape::GraphRevision,
        failure: AppEventPayloadShape::EngineSelectionFailure,
    },
    AppEventSurfaceDescriptor::EngineActivationAcknowledged {
        request_id: AppEventPayloadShape::EngineSelectionRequestId,
        intent: AppEventPayloadShape::StructuralEditIntent,
        target_graph_revision: AppEventPayloadShape::GraphRevision,
        retired_graph_revision: AppEventPayloadShape::GraphRevision,
        collected: AppEventPayloadShape::Boolean,
    },
    AppEventSurfaceDescriptor::SetSlotOccupancy {
        patch_id: AppEventPayloadShape::PatchId,
        slot: AppEventPayloadShape::EffectSlotIndex,
        entry: AppEventPayloadShape::OptionalEffectEntry,
    },
    AppEventSurfaceDescriptor::SetReturnOccupancy {
        bus: AppEventPayloadShape::BusId,
        entry: AppEventPayloadShape::OptionalEffectEntry,
    },
    AppEventSurfaceDescriptor::TopologyPrepared {
        request_id: AppEventPayloadShape::EngineSelectionRequestId,
        intent: AppEventPayloadShape::StructuralEditIntent,
        source_graph_revision: AppEventPayloadShape::GraphRevision,
        target_graph_revision: AppEventPayloadShape::GraphRevision,
        prepared_visualization: AppEventPayloadShape::PreparedSampleVisualization,
    },
    AppEventSurfaceDescriptor::TopologyPreparationFailed {
        request_id: AppEventPayloadShape::EngineSelectionRequestId,
        intent: AppEventPayloadShape::StructuralEditIntent,
        source_graph_revision: AppEventPayloadShape::GraphRevision,
        target_graph_revision: AppEventPayloadShape::GraphRevision,
        failure: AppEventPayloadShape::EngineSelectionFailure,
    },
    AppEventSurfaceDescriptor::MidiInputPreferenceRestored {
        preference: AppEventPayloadShape::MidiInputPreference,
        failure: AppEventPayloadShape::OptionalMidiDeviceFailure,
    },
    AppEventSurfaceDescriptor::MidiInputPreferenceStoreFailed {
        failure: AppEventPayloadShape::MidiDeviceFailure,
    },
    AppEventSurfaceDescriptor::MidiInputScanStarted,
    AppEventSurfaceDescriptor::MidiInputScanSucceeded {
        scan_id: AppEventPayloadShape::MidiInputScanId,
        descriptors: AppEventPayloadShape::MidiInputDescriptorList,
    },
    AppEventSurfaceDescriptor::MidiInputScanFailed {
        scan_id: AppEventPayloadShape::MidiInputScanId,
        failure: AppEventPayloadShape::MidiDeviceFailure,
    },
    AppEventSurfaceDescriptor::MidiInputConnectRequested {
        identity: AppEventPayloadShape::MidiInputDeviceId,
    },
    AppEventSurfaceDescriptor::MidiInputConnectionPrepared {
        request_id: AppEventPayloadShape::MidiConnectionRequestId,
        revision: AppEventPayloadShape::MidiConnectionRevision,
    },
    AppEventSurfaceDescriptor::MidiInputActivationAcknowledged {
        request_id: AppEventPayloadShape::MidiConnectionRequestId,
        revision: AppEventPayloadShape::MidiConnectionRevision,
    },
    AppEventSurfaceDescriptor::MidiInputDisconnectRequested {
        identity: AppEventPayloadShape::MidiInputDeviceId,
    },
    AppEventSurfaceDescriptor::MidiInputConnectionLost {
        identity: AppEventPayloadShape::MidiInputDeviceId,
        revision: AppEventPayloadShape::MidiConnectionRevision,
    },
    AppEventSurfaceDescriptor::MidiInputOperationFailed {
        identity: AppEventPayloadShape::MidiInputDeviceId,
        request_id: AppEventPayloadShape::OptionalMidiConnectionRequestId,
        revision: AppEventPayloadShape::OptionalMidiConnectionRevision,
        failure: AppEventPayloadShape::MidiDeviceFailure,
    },
    AppEventSurfaceDescriptor::MidiInputShutdownRequested,
];
/// The closed semantic input union accepted by the application reducer.
///
/// This type intentionally contains only domain values. Raw key codes, window
/// state, clocks, files, and audio devices are normalized by adapters before an
/// event reaches the control layer.
#[derive(Clone, Debug, PartialEq)]
pub enum AppEvent {
    Send(crate::control::SendAction),
    Controller(crate::control::ControllerEvent),
    /// Select one of the two reducer-owned top-level contexts directly.
    SelectContext(TopLevelContext),
    /// Move the focused Patch one position along the installed order.
    ///
    /// This is the only event that changes which Patch is focused. At either
    /// end of the installed order it is a typed unchanged rejection rather
    /// than a clamp or a wrap, exactly like every other adjacent choice.
    SelectPatch(Direction),
    /// Resolve one page connection from the active surface.
    NavigatePage(Direction),
    /// Move the current selection without changing a synth parameter.
    Navigate(Direction),
    /// Adjust exactly the currently selected bounded parameter.
    Adjust(Direction),
    /// Select the reducer-owned interpretation of subsequent directions.
    SetInteractionMode(InteractionMode),
    OpenRelated,
    OpenMidiSettings,
    Activate,
    PreviewStart,
    ToggleTestMidi,
    PreviewStop,
    /// Enter one context-compatible persistent side surface.
    EnterSurface(SurfaceId),
    /// Restore the exact main-surface origin of the current side surface.
    Return,
    /// Install the startup patch set in fixture discovery order.
    ///
    /// Whether installation is still permitted is enforced by AppState::apply
    /// on the control thread.
    InstallPatches(Vec<Patch>),
    AssetImported(crate::control::AssetImportResult),
    /// Commits all persisted content only after the payload's correlated
    /// complete graph has activated. The payload can be produced only by the
    /// saved-session preparation boundary.
    ReplacePersistedSession(Box<SessionReplacementPayload>),
    /// Route one normalized MIDI message to its target patch.
    Midi {
        patch_id: PatchId,
        message: MidiMessage,
    },
    /// Enables or disables one canonical Patch Overview origin in the
    /// reducer-owned semantic schema.
    SetPatchOverviewOriginEnabled {
        patch_id: PatchId,
        control: PatchControlId,
        enabled: bool,
    },
    /// Advances one correlated structural request through reducer-owned
    /// Loading → Validating → Preparing admission phases.
    EngineSelectionLifecycleAdvanced {
        request_id: EngineSelectionRequestId,
        lifecycle: EngineSelectionStatusKind,
    },
    /// Records one provider-validated candidate after off-callback
    /// preparation. Canonical Patch state is committed only by the correlated
    /// activation acknowledgement.
    EnginePrepared {
        request_id: EngineSelectionRequestId,
        patch_id: PatchId,
        intent: StructuralEditIntent,
        source_capability_id: CapabilityId,
        target_capability_id: CapabilityId,
        source_graph_revision: GraphRevision,
        target_graph_revision: GraphRevision,
        candidate_config: InstrumentConfig,
        prepared_visualization: Option<Vec<crate::synth::PreparedSampleVisualization>>,
    },
    /// Advances one correlated Sample assignment through observable worker-side
    /// admission stages. Only Loading → Validating → Preparing is accepted.
    SampleAssetLifecycleAdvanced {
        request_id: EngineSelectionRequestId,
        lifecycle: SampleAssetLifecycle,
    },
    /// Replaces one folder's correlated catalog result through the reducer.
    /// Stable row focus is retained when the identity remains present and is
    /// repaired deterministically when it does not.
    FileCatalogRefreshed {
        asset_kind: crate::synth::AssetKind,
        folder: FileBrowserFolderId,
        listing: Result<FileBrowserListing, SampleAssetError>,
    },
    /// Records one correlated typed preparation failure without adapter detail.
    EnginePreparationFailed {
        request_id: EngineSelectionRequestId,
        patch_id: PatchId,
        intent: StructuralEditIntent,
        source_capability_id: CapabilityId,
        target_capability_id: CapabilityId,
        source_graph_revision: GraphRevision,
        target_graph_revision: GraphRevision,
        failure: EngineSelectionFailure,
    },
    /// Reaches Ready only after activation, retirement, and control collection.
    EngineActivationAcknowledged {
        request_id: EngineSelectionRequestId,
        intent: StructuralEditIntent,
        target_graph_revision: GraphRevision,
        retired_graph_revision: GraphRevision,
        collected: bool,
    },
    /// Requests one Patch effect-slot occupancy change: occupy or replace the
    /// validated position with a registry entry, or clear it with `None`.
    /// Structural: the change enters the prepared-graph lifecycle, never the
    /// scalar snapshot path.
    SetSlotOccupancy {
        patch_id: PatchId,
        slot: EffectSlotIndex,
        entry: Option<EffectCapabilityId>,
    },
    /// Requests one bus-return occupancy change: occupy or replace the
    /// validated bus with a registry entry, or clear it with `None`.
    /// Structural, exactly like a slot change.
    SetReturnOccupancy {
        bus: BusId,
        entry: Option<EffectCapabilityId>,
    },
    /// Records one prepared occupancy candidate after off-callback
    /// preparation. Canonical occupancy is committed only by the correlated
    /// activation acknowledgement.
    TopologyPrepared {
        request_id: EngineSelectionRequestId,
        intent: StructuralEditIntent,
        source_graph_revision: GraphRevision,
        target_graph_revision: GraphRevision,
        prepared_visualization: Option<Vec<crate::synth::PreparedSampleVisualization>>,
    },
    /// Records one correlated typed occupancy refusal without adapter detail.
    TopologyPreparationFailed {
        request_id: EngineSelectionRequestId,
        intent: StructuralEditIntent,
        source_graph_revision: GraphRevision,
        target_graph_revision: GraphRevision,
        failure: EngineSelectionFailure,
    },
    /// Restores the separately persisted selected-input preference, or its
    /// typed load/decode failure, through the sole mutation boundary.
    MidiInputPreferenceRestored {
        preference: Option<MidiInputPreference>,
        failure: Option<MidiDeviceFailure>,
    },
    /// Records a typed post-acceptance preference write failure.
    MidiInputPreferenceStoreFailed {
        failure: MidiDeviceFailure,
    },
    /// Allocates and starts one reducer-correlated discovery scan.
    MidiInputScanStarted,
    /// Reconciles one exact successful scan result by opaque identity.
    MidiInputScanSucceeded {
        scan_id: MidiInputScanId,
        descriptors: Vec<MidiInputDescriptor>,
    },
    /// Records a typed discovery failure without treating it as device loss.
    MidiInputScanFailed {
        scan_id: MidiInputScanId,
        failure: MidiDeviceFailure,
    },
    /// Requests a connection to one exact present opaque identity.
    MidiInputConnectRequested {
        identity: MidiInputDeviceId,
    },
    /// Records a prepared disabled candidate; no callback data is active yet.
    MidiInputConnectionPrepared {
        request_id: MidiConnectionRequestId,
        revision: MidiConnectionRevision,
    },
    /// Commits one matching candidate after controlled switch/recovery work.
    MidiInputActivationAcknowledged {
        request_id: MidiConnectionRequestId,
        revision: MidiConnectionRevision,
    },
    /// Explicitly disconnects the selected active identity for this process.
    MidiInputDisconnectRequested {
        identity: MidiInputDeviceId,
    },
    /// Invalidates one exact active revision after definitive device loss.
    MidiInputConnectionLost {
        identity: MidiInputDeviceId,
        revision: MidiConnectionRevision,
    },
    /// Reports a typed correlated worker/transport/retirement failure.
    MidiInputOperationFailed {
        identity: MidiInputDeviceId,
        request_id: Option<MidiConnectionRequestId>,
        revision: Option<MidiConnectionRevision>,
        failure: MidiDeviceFailure,
    },
    /// Invalidates every physical-input correlation before runtime teardown.
    MidiInputShutdownRequested,
}

impl AppEvent {
    /// Performs the single typed user-intent to reducer-event translation.
    pub fn from_semantic_action(action: SemanticAction) -> Self {
        match action {
            SemanticAction::Send(action) => Self::Send(action),
            SemanticAction::SelectContext(context) => Self::SelectContext(context),
            SemanticAction::SelectPatch(direction) => Self::SelectPatch(direction),
            SemanticAction::NavigatePage(direction) => Self::NavigatePage(direction),
            SemanticAction::Navigate(direction) => Self::Navigate(direction),
            SemanticAction::Adjust(direction) => Self::Adjust(direction),
            SemanticAction::SetInteractionMode(mode) => Self::SetInteractionMode(mode),
            SemanticAction::OpenRelated => Self::OpenRelated,
            SemanticAction::OpenMidiSettings => Self::OpenMidiSettings,
            SemanticAction::Activate => Self::Activate,
            SemanticAction::PreviewStart => Self::PreviewStart,
            SemanticAction::ToggleTestMidi => Self::ToggleTestMidi,
            SemanticAction::PreviewStop => Self::PreviewStop,
            SemanticAction::EnterSurface(surface) => Self::EnterSurface(surface),
            SemanticAction::Return => Self::Return,
            SemanticAction::SetSlotOccupancy {
                patch_id,
                slot,
                entry,
            } => Self::SetSlotOccupancy {
                patch_id,
                slot,
                entry,
            },
            SemanticAction::SetReturnOccupancy { bus, entry } => {
                Self::SetReturnOccupancy { bus, entry }
            }
        }
    }

    /// Reports whether accepting this event publishes a new scalar snapshot
    /// to the real-time boundary.
    ///
    /// Semantic navigation still advances the canonical control generation,
    /// but it cannot alter audio values and therefore must not publish across
    /// any audio transport.
    pub const fn publishes_parameters_on_acceptance(&self) -> bool {
        !matches!(
            self,
            Self::Controller(_)
                | Self::SelectContext(_)
                | Self::SelectPatch(_)
                | Self::NavigatePage(_)
                | Self::Navigate(_)
                | Self::SetInteractionMode(_)
                | Self::OpenRelated
                | Self::OpenMidiSettings
                | Self::ToggleTestMidi
                | Self::PreviewStart
                | Self::PreviewStop
                | Self::SetPatchOverviewOriginEnabled { .. }
                | Self::EngineSelectionLifecycleAdvanced { .. }
                | Self::SampleAssetLifecycleAdvanced { .. }
                | Self::FileCatalogRefreshed { .. }
                | Self::EnterSurface(_)
                | Self::Return
                | Self::MidiInputPreferenceRestored { .. }
                | Self::MidiInputPreferenceStoreFailed { .. }
                | Self::MidiInputScanStarted
                | Self::MidiInputScanSucceeded { .. }
                | Self::MidiInputScanFailed { .. }
                | Self::MidiInputConnectRequested { .. }
                | Self::MidiInputConnectionPrepared { .. }
                | Self::MidiInputActivationAcknowledged { .. }
                | Self::MidiInputDisconnectRequested { .. }
                | Self::MidiInputConnectionLost { .. }
                | Self::MidiInputOperationFailed { .. }
                | Self::MidiInputShutdownRequested
        )
    }

    /// Returns the unique exhaustive descriptor entries for the closed event union.
    pub const fn surface_descriptor() -> &'static [AppEventSurfaceDescriptor] {
        &APP_EVENT_SURFACE_DESCRIPTOR
    }

    /// Returns the descriptor entry matching this concrete event.
    ///
    /// The exhaustive match intentionally makes a newly added event variant a
    /// compile error until its surface descriptor is updated as well.
    pub fn surface_entry(&self) -> AppEventSurfaceDescriptor {
        match self {
            Self::Controller(_) => AppEventSurfaceDescriptor::Controller {
                event: AppEventPayloadShape::ControllerEvent,
            },
            Self::SelectContext(context) => {
                AppEventSurfaceDescriptor::SelectContext { context: *context }
            }
            Self::SelectPatch(direction) => AppEventSurfaceDescriptor::SelectPatch {
                direction: *direction,
            },
            Self::NavigatePage(direction) => AppEventSurfaceDescriptor::NavigatePage {
                direction: *direction,
            },
            Self::Navigate(direction) => AppEventSurfaceDescriptor::Navigate {
                direction: *direction,
            },
            Self::Adjust(direction) => AppEventSurfaceDescriptor::Adjust {
                direction: *direction,
            },
            Self::SetInteractionMode(mode) => {
                AppEventSurfaceDescriptor::SetInteractionMode { mode: *mode }
            }
            Self::OpenRelated => AppEventSurfaceDescriptor::OpenRelated,
            Self::OpenMidiSettings => AppEventSurfaceDescriptor::OpenMidiSettings,
            Self::Activate => AppEventSurfaceDescriptor::Activate,
            Self::AssetImported(_) => AppEventSurfaceDescriptor::AssetImported {
                selection: AppEventPayloadShape::AssetImportResult,
            },
            Self::PreviewStart => AppEventSurfaceDescriptor::PreviewStart,
            Self::ToggleTestMidi => AppEventSurfaceDescriptor::ToggleTestMidi,
            Self::PreviewStop => AppEventSurfaceDescriptor::PreviewStop,
            Self::EnterSurface(surface) => {
                AppEventSurfaceDescriptor::EnterSurface { surface: *surface }
            }
            Self::Return => AppEventSurfaceDescriptor::Return,
            Self::InstallPatches(_) => AppEventSurfaceDescriptor::InstallPatches {
                patches: AppEventPayloadShape::PatchList,
            },
            Self::ReplacePersistedSession(_) => {
                AppEventSurfaceDescriptor::ReplacePersistedSession {
                    replacement: AppEventPayloadShape::SessionReplacement,
                }
            }
            Self::Midi { .. } => AppEventSurfaceDescriptor::Midi {
                patch_id: AppEventPayloadShape::PatchId,
                message: AppEventPayloadShape::MidiMessage,
            },
            Self::SetPatchOverviewOriginEnabled { .. } => {
                AppEventSurfaceDescriptor::SetPatchOverviewOriginEnabled {
                    patch_id: AppEventPayloadShape::PatchId,
                    control: AppEventPayloadShape::PatchControlId,
                    enabled: AppEventPayloadShape::Boolean,
                }
            }
            Self::EngineSelectionLifecycleAdvanced { .. } => {
                AppEventSurfaceDescriptor::EngineSelectionLifecycleAdvanced {
                    request_id: AppEventPayloadShape::EngineSelectionRequestId,
                    lifecycle: AppEventPayloadShape::EngineSelectionStatusKind,
                }
            }
            Self::EnginePrepared { .. } => AppEventSurfaceDescriptor::EnginePrepared {
                request_id: AppEventPayloadShape::EngineSelectionRequestId,
                patch_id: AppEventPayloadShape::PatchId,
                intent: AppEventPayloadShape::StructuralEditIntent,
                source_capability_id: AppEventPayloadShape::CapabilityId,
                target_capability_id: AppEventPayloadShape::CapabilityId,
                source_graph_revision: AppEventPayloadShape::GraphRevision,
                target_graph_revision: AppEventPayloadShape::GraphRevision,
                candidate_config: AppEventPayloadShape::InstrumentConfig,
                prepared_visualization: AppEventPayloadShape::PreparedSampleVisualization,
            },
            Self::SampleAssetLifecycleAdvanced { .. } => {
                AppEventSurfaceDescriptor::SampleAssetLifecycleAdvanced {
                    request_id: AppEventPayloadShape::EngineSelectionRequestId,
                    lifecycle: AppEventPayloadShape::SampleAssetLifecycle,
                }
            }
            Self::FileCatalogRefreshed { .. } => AppEventSurfaceDescriptor::FileCatalogRefreshed {
                asset_kind: AppEventPayloadShape::AssetKind,
                folder: AppEventPayloadShape::FileBrowserFolderId,
                listing: AppEventPayloadShape::FileBrowserListing,
                failure: AppEventPayloadShape::SampleAssetError,
            },
            Self::EnginePreparationFailed { .. } => {
                AppEventSurfaceDescriptor::EnginePreparationFailed {
                    request_id: AppEventPayloadShape::EngineSelectionRequestId,
                    patch_id: AppEventPayloadShape::PatchId,
                    intent: AppEventPayloadShape::StructuralEditIntent,
                    source_capability_id: AppEventPayloadShape::CapabilityId,
                    target_capability_id: AppEventPayloadShape::CapabilityId,
                    source_graph_revision: AppEventPayloadShape::GraphRevision,
                    target_graph_revision: AppEventPayloadShape::GraphRevision,
                    failure: AppEventPayloadShape::EngineSelectionFailure,
                }
            }
            Self::EngineActivationAcknowledged { .. } => {
                AppEventSurfaceDescriptor::EngineActivationAcknowledged {
                    request_id: AppEventPayloadShape::EngineSelectionRequestId,
                    intent: AppEventPayloadShape::StructuralEditIntent,
                    target_graph_revision: AppEventPayloadShape::GraphRevision,
                    retired_graph_revision: AppEventPayloadShape::GraphRevision,
                    collected: AppEventPayloadShape::Boolean,
                }
            }
            Self::SetSlotOccupancy { .. } => AppEventSurfaceDescriptor::SetSlotOccupancy {
                patch_id: AppEventPayloadShape::PatchId,
                slot: AppEventPayloadShape::EffectSlotIndex,
                entry: AppEventPayloadShape::OptionalEffectEntry,
            },
            Self::Send(_) => AppEventSurfaceDescriptor::Send {
                action: AppEventPayloadShape::SendAction,
            },
            Self::SetReturnOccupancy { .. } => AppEventSurfaceDescriptor::SetReturnOccupancy {
                bus: AppEventPayloadShape::BusId,
                entry: AppEventPayloadShape::OptionalEffectEntry,
            },
            Self::TopologyPrepared { .. } => AppEventSurfaceDescriptor::TopologyPrepared {
                request_id: AppEventPayloadShape::EngineSelectionRequestId,
                intent: AppEventPayloadShape::StructuralEditIntent,
                source_graph_revision: AppEventPayloadShape::GraphRevision,
                target_graph_revision: AppEventPayloadShape::GraphRevision,
                prepared_visualization: AppEventPayloadShape::PreparedSampleVisualization,
            },
            Self::TopologyPreparationFailed { .. } => {
                AppEventSurfaceDescriptor::TopologyPreparationFailed {
                    request_id: AppEventPayloadShape::EngineSelectionRequestId,
                    intent: AppEventPayloadShape::StructuralEditIntent,
                    source_graph_revision: AppEventPayloadShape::GraphRevision,
                    target_graph_revision: AppEventPayloadShape::GraphRevision,
                    failure: AppEventPayloadShape::EngineSelectionFailure,
                }
            }
            Self::MidiInputPreferenceRestored { .. } => {
                AppEventSurfaceDescriptor::MidiInputPreferenceRestored {
                    preference: AppEventPayloadShape::MidiInputPreference,
                    failure: AppEventPayloadShape::OptionalMidiDeviceFailure,
                }
            }
            Self::MidiInputPreferenceStoreFailed { .. } => {
                AppEventSurfaceDescriptor::MidiInputPreferenceStoreFailed {
                    failure: AppEventPayloadShape::MidiDeviceFailure,
                }
            }
            Self::MidiInputScanStarted => AppEventSurfaceDescriptor::MidiInputScanStarted,
            Self::MidiInputScanSucceeded { .. } => {
                AppEventSurfaceDescriptor::MidiInputScanSucceeded {
                    scan_id: AppEventPayloadShape::MidiInputScanId,
                    descriptors: AppEventPayloadShape::MidiInputDescriptorList,
                }
            }
            Self::MidiInputScanFailed { .. } => AppEventSurfaceDescriptor::MidiInputScanFailed {
                scan_id: AppEventPayloadShape::MidiInputScanId,
                failure: AppEventPayloadShape::MidiDeviceFailure,
            },
            Self::MidiInputConnectRequested { .. } => {
                AppEventSurfaceDescriptor::MidiInputConnectRequested {
                    identity: AppEventPayloadShape::MidiInputDeviceId,
                }
            }
            Self::MidiInputConnectionPrepared { .. } => {
                AppEventSurfaceDescriptor::MidiInputConnectionPrepared {
                    request_id: AppEventPayloadShape::MidiConnectionRequestId,
                    revision: AppEventPayloadShape::MidiConnectionRevision,
                }
            }
            Self::MidiInputActivationAcknowledged { .. } => {
                AppEventSurfaceDescriptor::MidiInputActivationAcknowledged {
                    request_id: AppEventPayloadShape::MidiConnectionRequestId,
                    revision: AppEventPayloadShape::MidiConnectionRevision,
                }
            }
            Self::MidiInputDisconnectRequested { .. } => {
                AppEventSurfaceDescriptor::MidiInputDisconnectRequested {
                    identity: AppEventPayloadShape::MidiInputDeviceId,
                }
            }
            Self::MidiInputConnectionLost { .. } => {
                AppEventSurfaceDescriptor::MidiInputConnectionLost {
                    identity: AppEventPayloadShape::MidiInputDeviceId,
                    revision: AppEventPayloadShape::MidiConnectionRevision,
                }
            }
            Self::MidiInputOperationFailed { .. } => {
                AppEventSurfaceDescriptor::MidiInputOperationFailed {
                    identity: AppEventPayloadShape::MidiInputDeviceId,
                    request_id: AppEventPayloadShape::OptionalMidiConnectionRequestId,
                    revision: AppEventPayloadShape::OptionalMidiConnectionRevision,
                    failure: AppEventPayloadShape::MidiDeviceFailure,
                }
            }
            Self::MidiInputShutdownRequested => {
                AppEventSurfaceDescriptor::MidiInputShutdownRequested
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AppEvent, AppEventPayloadShape, AppEventSurfaceDescriptor, Direction};
    use crate::control::{InteractionMode, SurfaceId, TopLevelContext};
    use crate::kernel::midi_channel::MidiChannel;
    use crate::kernel::midi_message::MidiMessage;
    use crate::kernel::patch_id::PatchId;
    use crate::synth::patch::Patch;

    #[test]
    fn navigate_and_adjust_retain_their_semantic_direction() {
        let navigate = AppEvent::Navigate(Direction::Up);
        let adjust = AppEvent::Adjust(Direction::Left);

        assert_eq!(navigate, AppEvent::Navigate(Direction::Up));
        assert_eq!(adjust, AppEvent::Adjust(Direction::Left));
        assert_ne!(navigate, adjust);
    }

    #[test]
    fn all_direction_payloads_are_explicit_and_copyable() {
        let directions = [
            Direction::Up,
            Direction::Down,
            Direction::Left,
            Direction::Right,
        ];
        let copied = directions;

        assert_eq!(copied.len(), 4);
        assert_eq!(
            copied,
            [
                Direction::Up,
                Direction::Down,
                Direction::Left,
                Direction::Right,
            ]
        );
    }

    #[test]
    fn surface_descriptor_is_unique_and_exhaustive() {
        let descriptor = AppEvent::surface_descriptor();

        assert_eq!(descriptor.len(), 57);
        assert!(descriptor.contains(&AppEventSurfaceDescriptor::Controller {
            event: AppEventPayloadShape::ControllerEvent,
        }));
        for (index, entry) in descriptor.iter().enumerate() {
            assert!(
                !descriptor[..index].contains(entry),
                "duplicate descriptor entry: {entry:?}"
            );
        }

        for context in TopLevelContext::surface_descriptor() {
            assert!(descriptor
                .contains(&AppEventSurfaceDescriptor::SelectContext { context: *context }));
        }
        let send = AppEvent::Send(crate::control::SendAction::Open).surface_entry();
        assert_eq!(
            send,
            AppEventSurfaceDescriptor::Send {
                action: AppEventPayloadShape::SendAction,
            }
        );
        assert!(descriptor.contains(&send));

        for direction in [
            Direction::Up,
            Direction::Down,
            Direction::Left,
            Direction::Right,
        ] {
            assert!(descriptor.contains(&AppEventSurfaceDescriptor::Navigate { direction }));
            assert!(descriptor.contains(&AppEventSurfaceDescriptor::Adjust { direction }));
        }
        for mode in InteractionMode::PHASE_TWO {
            assert!(descriptor.contains(&AppEventSurfaceDescriptor::SetInteractionMode { mode }));
        }
        for surface in [
            SurfaceId::PatchUtility,
            SurfaceId::PatchDetail,
            SurfaceId::MixerInspector,
        ] {
            assert!(descriptor.contains(&AppEventSurfaceDescriptor::EnterSurface { surface }));
        }
        assert!(descriptor.contains(&AppEventSurfaceDescriptor::Return));
        assert!(
            descriptor.contains(&AppEventSurfaceDescriptor::InstallPatches {
                patches: AppEventPayloadShape::PatchList,
            })
        );
        assert!(
            descriptor.contains(&AppEventSurfaceDescriptor::ReplacePersistedSession {
                replacement: AppEventPayloadShape::SessionReplacement,
            })
        );
        assert!(descriptor.contains(&AppEventSurfaceDescriptor::Midi {
            patch_id: AppEventPayloadShape::PatchId,
            message: AppEventPayloadShape::MidiMessage,
        }));
        assert!(
            descriptor.contains(&AppEventSurfaceDescriptor::SetPatchOverviewOriginEnabled {
                patch_id: AppEventPayloadShape::PatchId,
                control: AppEventPayloadShape::PatchControlId,
                enabled: AppEventPayloadShape::Boolean,
            })
        );
        assert!(descriptor.contains(
            &AppEventSurfaceDescriptor::EngineSelectionLifecycleAdvanced {
                request_id: AppEventPayloadShape::EngineSelectionRequestId,
                lifecycle: AppEventPayloadShape::EngineSelectionStatusKind,
            }
        ));
        assert!(
            descriptor.contains(&AppEventSurfaceDescriptor::EnginePrepared {
                request_id: AppEventPayloadShape::EngineSelectionRequestId,
                patch_id: AppEventPayloadShape::PatchId,
                intent: AppEventPayloadShape::StructuralEditIntent,
                source_capability_id: AppEventPayloadShape::CapabilityId,
                target_capability_id: AppEventPayloadShape::CapabilityId,
                source_graph_revision: AppEventPayloadShape::GraphRevision,
                target_graph_revision: AppEventPayloadShape::GraphRevision,
                candidate_config: AppEventPayloadShape::InstrumentConfig,
                prepared_visualization: AppEventPayloadShape::PreparedSampleVisualization,
            })
        );
        assert!(
            descriptor.contains(&AppEventSurfaceDescriptor::SampleAssetLifecycleAdvanced {
                request_id: AppEventPayloadShape::EngineSelectionRequestId,
                lifecycle: AppEventPayloadShape::SampleAssetLifecycle,
            })
        );
        assert!(
            descriptor.contains(&AppEventSurfaceDescriptor::FileCatalogRefreshed {
                asset_kind: AppEventPayloadShape::AssetKind,
                folder: AppEventPayloadShape::FileBrowserFolderId,
                listing: AppEventPayloadShape::FileBrowserListing,
                failure: AppEventPayloadShape::SampleAssetError,
            })
        );
        assert!(
            descriptor.contains(&AppEventSurfaceDescriptor::EnginePreparationFailed {
                request_id: AppEventPayloadShape::EngineSelectionRequestId,
                patch_id: AppEventPayloadShape::PatchId,
                intent: AppEventPayloadShape::StructuralEditIntent,
                source_capability_id: AppEventPayloadShape::CapabilityId,
                target_capability_id: AppEventPayloadShape::CapabilityId,
                source_graph_revision: AppEventPayloadShape::GraphRevision,
                target_graph_revision: AppEventPayloadShape::GraphRevision,
                failure: AppEventPayloadShape::EngineSelectionFailure,
            })
        );
        assert!(
            descriptor.contains(&AppEventSurfaceDescriptor::EngineActivationAcknowledged {
                request_id: AppEventPayloadShape::EngineSelectionRequestId,
                intent: AppEventPayloadShape::StructuralEditIntent,
                target_graph_revision: AppEventPayloadShape::GraphRevision,
                retired_graph_revision: AppEventPayloadShape::GraphRevision,
                collected: AppEventPayloadShape::Boolean,
            })
        );
    }

    #[test]
    fn context_event_preserves_its_semantic_payload() {
        for context in TopLevelContext::surface_descriptor() {
            let event = AppEvent::SelectContext(*context);
            assert_eq!(
                event.surface_entry(),
                AppEventSurfaceDescriptor::SelectContext { context: *context }
            );
        }
    }
    #[test]
    fn install_event_preserves_patch_order_payload() {
        let patches: Vec<Patch> = Vec::new();
        let event = AppEvent::InstallPatches(patches);

        assert_eq!(
            event.surface_entry(),
            AppEventSurfaceDescriptor::InstallPatches {
                patches: AppEventPayloadShape::PatchList,
            }
        );

        match event {
            AppEvent::InstallPatches(installed) => assert!(installed.is_empty()),
            _ => panic!("expected patch installation event"),
        }
    }

    #[test]
    fn midi_event_preserves_its_typed_target_and_message() {
        let patch_id = PatchId::new(7).unwrap();
        let message = MidiMessage::all_notes_off(MidiChannel::new(3).unwrap());
        let event = AppEvent::Midi { patch_id, message };

        assert_eq!(
            event.surface_entry(),
            AppEventSurfaceDescriptor::Midi {
                patch_id: AppEventPayloadShape::PatchId,
                message: AppEventPayloadShape::MidiMessage,
            }
        );
        assert_eq!(event, AppEvent::Midi { patch_id, message });
    }
}
