pub mod controller;
pub use controller::{
    ControllerBindings, ControllerButton, ControllerDevice, ControllerEvent, ControllerFailure,
    ControllerPreferenceStatus, ControllerRole, ControllerSettingId, ControllerState,
};
pub mod app_event;
pub mod app_state;
pub mod default_session;
pub mod engine_selection;
pub mod event_record;
pub mod file_browser_state;
pub mod graphical_shell_projection;
pub mod interaction_state;
pub mod midi_device;
pub mod midi_device_worker;
pub mod midi_scan_scheduler;
pub mod patch_control_id;
pub mod patch_page_projection;
pub mod patch_position_id;
pub mod saved_session;
mod session_command;
pub use session_command::SessionCommand;
pub mod semantic_action;
pub mod semantic_focus;
pub mod semantic_graphical_view_model;
pub mod semantic_resolver;
mod serialized_state;
pub mod session_candidate_worker;
mod session_replacement;
pub mod state_projector;
pub mod state_snapshot;
pub mod text_projection;
pub mod top_level_context;

pub use app_event::{AppEvent, AppEventPayloadShape, AppEventSurfaceDescriptor, Direction};
pub use app_state::{
    AppState, ApplyOutcome, EventRejection, FocusRepairStatus, SemanticActionAvailability,
    StateAccepted,
};
pub use default_session::{
    capture_default_session, DefaultSessionBlueprint, DefaultSessionError, PatchCreationBlueprint,
    PatchCreationError, ProspectivePatch,
};
pub use engine_selection::{
    EffectAssetTarget, EngineSelectionCorrelation, EngineSelectionEffect,
    EngineSelectionEffectKind, EngineSelectionFailure, EngineSelectionRequestId,
    EngineSelectionRequestIdError, EngineSelectionStatus, EngineSelectionStatusError,
    EngineSelectionStatusKind, StructuralEditIntent,
};
pub use event_record::{
    AudioEffect, EmittedEvent, EventDirection, EventInput, EventOutcome, EventRecord,
    EventRecordError, EventSource, MidiInput, MidiKind, PatchInput,
};
pub use file_browser_state::{
    AssetImportRequest, AssetImportResult, FileBrowserState, SampleAssetLifecycle,
    SamplePreviewState,
};
pub use graphical_shell_projection::{
    GraphicalShellProjection, GraphicalShellProjectionError, ShellContextLine, ShellFooter,
    ShellIdentityHeader, ShellMainRegion, ShellSideRegion, ShellWorkspace,
};
pub use interaction_state::{
    InteractionState, PatchSubordinateSession, Selection, SelectionSection, SettingsSession,
};
pub use midi_device::{
    ActiveMidiInput, ConnectMidiInput, MidiActiveInputIdentity, MidiActivityObservation,
    MidiActivitySnapshot, MidiCallbackDiagnostics, MidiConnectionFailureClass,
    MidiConnectionRequestId, MidiConnectionRevision, MidiDeviceContractError, MidiDeviceEffect,
    MidiDeviceFailure, MidiIdentifierKind, MidiInputConnectionIntent, MidiInputConnectionStatus,
    MidiInputDescriptor, MidiInputDeviceId, MidiInputDevicePort, MidiInputPortFacts,
    MidiInputPreference, MidiInputPreferencePort, MidiInputRegistryEntry, MidiInputRowAction,
    MidiInputRowState, MidiInputScanId, MidiInputScanState, MidiInputState, MidiInputStatusMarker,
    MidiInputTransport, MidiMessageDiagnosticClass, MidiPreferredInput, MidiStaleOperation,
    MidiTransportCapacityStage, PhysicalMidiEvent, PhysicalMidiIngress, PhysicalMidiIngressControl,
    PhysicalMidiIngressOutcome, PhysicalMidiPacket, PhysicalMidiTransportEvent,
    MAX_MIDI_DEVICE_ID_BYTES, MAX_MIDI_DISPLAY_NAME_BYTES, MAX_MIDI_IDENTITY_SCHEMA_BYTES,
    MAX_MIDI_PORT_FACT_BYTES, MIDI_INPUT_PREFERENCE_VERSION, PHYSICAL_MIDI_DRAIN_BUDGET,
    PHYSICAL_MIDI_QUEUE_CAPACITY,
};
pub use midi_device_worker::{
    MidiDeviceWorker, MidiDeviceWorkerBusy, MidiDeviceWorkerBusyReason, MidiDeviceWorkerCommand,
    MidiDeviceWorkerResult, MidiDeviceWorkerShutdownError,
};
pub use midi_scan_scheduler::{MidiScanScheduler, MIDI_SCAN_INTERVAL_MICROS};
pub use patch_control_id::PatchControlId;
pub use patch_page_projection::{
    PatchPageEffectSlot, PatchPageEngine, PatchPageEngineChoice, PatchPageEnvelopeRow,
    PatchPageIdentity, PatchPageOccupancyChoice, PatchPageParameterRow, PatchPageParameterValue,
    PatchPageProjection, PatchPageProjectionError, PatchPageSection, PatchPageSlotOccupancy,
    EMPTY_OCCUPANCY_CHOICE_ID,
};
pub use patch_position_id::PatchPositionId;
pub use saved_session::{
    PreparedSavedSession, SavedSession, SavedSessionError, SavedSessionRestoreError,
    SAVED_SESSION_VERSION,
};
pub use semantic_action::{InteractionMode, SemanticAction, SemanticActionKind, ValidAction};
pub use semantic_focus::{
    FocusCapabilityId, FocusPath, FocusPathError, MixerControlId, ModalControlId,
    PatchChoiceSubject, PatchDetailSubject, ReturnPath, SemanticControlId, SurfaceId,
};
pub use semantic_graphical_view_model::{
    MidiInputInspectorViewModel, MidiInputSettingsRowViewModel, SemanticBrowserMetadata,
    SemanticBrowserMetadataStatus, SemanticControlKind, SemanticControlValue,
    SemanticControlViewModel, SemanticError, SemanticErrorCode, SemanticFocusRepairStatus,
    SemanticGraphicalViewModel, SemanticGraphicalViewModelError, SemanticLifecycleStatus,
    SemanticNumericRange, SemanticSurfaceControlSummaryViewModel, SemanticSurfaceRole,
    SemanticSurfaceSectionViewModel, SemanticSurfaceSummary, SemanticSurfaceViewModel,
    SemanticVisualizationData, SemanticVisualizationViewModel, SemanticWaveformLandmark,
    SemanticWaveformPair,
};
pub use semantic_resolver::{
    ChoiceCategory, ResolvedChoiceOption, ResolvedChoiceSource, SemanticResolver,
};
pub use session_candidate_worker::{
    SessionCandidateFailure, SessionCandidateFailureKind, SessionCandidateRequest,
    SessionCandidateResult, SessionCandidateSource, SessionCandidateToken,
    SessionCandidateTokenError, SessionCandidateWorker, SessionCandidateWorkerBusy,
    SessionCandidateWorkerBusyReason,
};
pub use session_replacement::SessionReplacementPayload;
pub use state_projector::{StateProjectionError, StateProjector};
pub use state_snapshot::StateSnapshot;
pub use text_projection::TextProjection;
pub use top_level_context::TopLevelContext;
pub mod app_loop;
pub use app_loop::{
    AppLoop, DispatchResult, MidiDeviceAdvanceError, MidiDeviceProgress, MidiFanOutResult,
    StructuralAdvanceError, StructuralProgress,
};
pub mod state_tree;
pub use state_tree::{StateTree, StateTreeError};
pub mod event_log;
pub use event_log::{EventCoverage, EventLog, EventLogError};

mod sends;
pub use sends::{SendAction, SendControlId};
