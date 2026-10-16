use crate::control::app_event::AppEvent;
use crate::control::app_state::{AppState, EventRejection, StateAccepted};
use crate::control::engine_selection::{
    EngineSelectionEffect, EngineSelectionEffectKind, EngineSelectionFailure,
    EngineSelectionRequestId, EngineSelectionStatusError, EngineSelectionStatusKind,
    StructuralEditIntent,
};
use crate::control::event_log::EventLog;
use crate::control::event_record::{EventRecord, EventSource};
use crate::control::patch_page_projection::PatchPageProjection;
use crate::control::state_projector::{MidiProjectionSeed, StateProjectionError, StateProjector};
use crate::control::state_snapshot::StateSnapshot;
use crate::control::state_tree::StateTree;
use crate::control::text_projection::TextProjection;
use crate::control::{
    ActiveMidiInput, ConnectMidiInput, GraphicalShellProjection, MidiActivitySnapshot,
    MidiDeviceEffect, MidiDeviceFailure, MidiDeviceWorker, MidiDeviceWorkerCommand,
    MidiDeviceWorkerResult, MidiScanScheduler, PhysicalMidiEvent, PhysicalMidiIngress,
    PhysicalMidiIngressControl, SampleAssetLifecycle, SemanticAction, SessionReplacementPayload,
    SurfaceId, PHYSICAL_MIDI_DRAIN_BUDGET,
};
use crate::kernel::midi_message::MidiMessage;
use crate::kernel::PatchId;
use crate::real_time::audio_boundary::{BoundaryFull, ControlAudioBoundary};
use crate::real_time::scheduled_event::{
    ScheduledEvent, ScheduledEventEpochExhausted, ScheduledEventRejected,
    ScheduledEventRejectionReason,
};
use crate::real_time::AudioCommand;
use crate::real_time::{
    ControlStructuralGraphBoundary, GraphPreparationCorrelation, GraphPreparationRequest,
    GraphPreparationRequestError, GraphPreparationResult, GraphPreparationWorker, GraphRevision,
    GraphRevisionError, GraphStageOutcome, ParameterSnapshot, ParameterSnapshotError,
    PreparedGraph, PreparedGraphRefreshError, StructuralGraphCoordinator, WorkerShutdownError,
};
use crate::shell::audio_output::AudioDeviceConfig;
use crate::synth::instrument_capability::{CapabilityError, CapabilityRegistry};
use crate::synth::DescriptorDefaultConfigFactory;
use core::fmt;
use std::collections::VecDeque;

const DEFAULT_EVENT_LOG_CAPACITY: usize = 1024;
const MIDI_DEVICE_EFFECT_BUDGET: usize = 8;
const MIDI_ACTIVITY_PUBLISH_INTERVAL_MICROS: u64 = 33_334;
const MIDI_RECEIVING_WINDOW_MICROS: u64 = 500_000;

/// Observable effects of one accepted application event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DispatchResult {
    accepted: StateAccepted,
    snapshot: StateSnapshot,
    boundary_full: Option<BoundaryFull>,
    scheduled_rejection: Option<ScheduledEventRejected>,
}

/// Rejection at the validated, graph-correlated scheduled MIDI boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ScheduledEventDispatchError {
    #[error("scheduled MIDI was rejected by the control state: {0}")]
    Control(#[from] EventRejection),
    #[error("scheduled MIDI targets graph {requested:?}, but the active graph is {active:?}")]
    GraphRevision {
        requested: GraphRevision,
        active: GraphRevision,
    },
    #[error(transparent)]
    Publication(#[from] ScheduledEventRejected),
    #[error("scheduled event payload is not valid for this operation")]
    InvalidPayload,
    #[error("temporary send targets an unavailable bus")]
    UnknownSendBus,
    #[error(transparent)]
    Lease(#[from] crate::real_time::SendLeaseError),
}

/// Outcome of resolving one incoming MIDI channel message against the latest
/// accepted Patch subscriptions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MidiFanOutResult {
    subscriber_count: usize,
    boundary_full: Option<BoundaryFull>,
}

impl MidiFanOutResult {
    /// Number of installed Patches that subscribed to the message channel.
    pub const fn subscriber_count(self) -> usize {
        self.subscriber_count
    }

    /// Returns the first command the bounded real-time queue could not accept.
    pub const fn boundary_full(self) -> Option<BoundaryFull> {
        self.boundary_full
    }
}

/// Bounded observations from one nonblocking structural control tick.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StructuralProgress {
    sample_asset_lifecycle_advanced: Option<SampleAssetLifecycle>,
    engine_selection_lifecycle_advanced: Option<EngineSelectionStatusKind>,
    worker_result_polled: bool,
    failure_dispatched: bool,
    graph_stage: Option<GraphStageOutcome>,
    graph_published: Option<GraphRevision>,
    activation_acknowledged: Option<GraphRevision>,
    session_replacement_committed: bool,
    collected_count: u64,
    rejected_worker_event: Option<EventRejection>,
}

/// Bounded observations from one physical MIDI control tick.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MidiDeviceProgress {
    scan_requested: bool,
    worker_result_polled: bool,
    effect_advanced: bool,
    drained_events: usize,
    stale_events: usize,
    audio_saturated: bool,
    ingress_overflowed: bool,
    rejected_worker_event: Option<EventRejection>,
}

impl MidiDeviceProgress {
    pub const fn scan_requested(self) -> bool {
        self.scan_requested
    }

    pub const fn worker_result_polled(self) -> bool {
        self.worker_result_polled
    }

    pub const fn effect_advanced(self) -> bool {
        self.effect_advanced
    }

    pub const fn drained_events(self) -> usize {
        self.drained_events
    }

    pub const fn stale_events(self) -> usize {
        self.stale_events
    }

    pub const fn audio_saturated(self) -> bool {
        self.audio_saturated
    }

    pub const fn ingress_overflowed(self) -> bool {
        self.ingress_overflowed
    }

    pub const fn rejected_worker_event(self) -> Option<EventRejection> {
        self.rejected_worker_event
    }
}

impl StructuralProgress {
    pub const fn sample_asset_lifecycle_advanced(self) -> Option<SampleAssetLifecycle> {
        self.sample_asset_lifecycle_advanced
    }

    pub const fn engine_selection_lifecycle_advanced(self) -> Option<EngineSelectionStatusKind> {
        self.engine_selection_lifecycle_advanced
    }
    pub const fn worker_result_polled(self) -> bool {
        self.worker_result_polled
    }

    pub const fn failure_dispatched(self) -> bool {
        self.failure_dispatched
    }

    pub const fn graph_stage(self) -> Option<GraphStageOutcome> {
        self.graph_stage
    }

    pub const fn graph_published(self) -> Option<GraphRevision> {
        self.graph_published
    }

    pub const fn activation_acknowledged(self) -> Option<GraphRevision> {
        self.activation_acknowledged
    }

    pub const fn session_replacement_committed(self) -> bool {
        self.session_replacement_committed
    }

    pub const fn collected_count(self) -> u64 {
        self.collected_count
    }

    pub const fn rejected_worker_event(self) -> Option<EventRejection> {
        self.rejected_worker_event
    }
}

/// A control-side ownership or invariant failure while advancing structure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StructuralAdvanceError {
    AlreadyConfigured,
    RegistryMismatch,
    Revision(GraphRevisionError),
    Refresh(PreparedGraphRefreshError),
    CandidateParameters(ParameterSnapshotError),
    Publication(crate::real_time::GraphPublicationFailure),
    EventLog(crate::control::EventLogError),
    Status(EngineSelectionStatusError),
    WorkerShutdown(WorkerShutdownError),
    SessionUnavailable,
    SessionBusy,
    SessionRevisionMismatch,
    SessionParameterMismatch,
    SessionPreflight(EventRejection),
    SessionCommit(EventRejection),
    CandidatePreflight(EventRejection),
    CandidateParameterMismatch,
    Recovery(BoundaryFull),
}

/// Configuration or teardown failure at the physical MIDI orchestration seam.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MidiDeviceAdvanceError {
    AlreadyConfigured,
    RecoveryReserveUnavailable,
    WorkerUnavailable,
    WorkerShutdown,
}

impl fmt::Display for MidiDeviceAdvanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AlreadyConfigured => "physical MIDI orchestration is already configured",
            Self::RecoveryReserveUnavailable => {
                "the audio command boundary has no all-notes-off recovery reserve"
            }
            Self::WorkerUnavailable => "the MIDI device worker is unavailable",
            Self::WorkerShutdown => "the MIDI device worker failed during shutdown",
        })
    }
}

impl std::error::Error for MidiDeviceAdvanceError {}

impl fmt::Display for StructuralAdvanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyConfigured => {
                formatter.write_str("engine-selection orchestration is already configured")
            }
            Self::RegistryMismatch => formatter.write_str(
                "engine-selection factory registry does not match canonical application state",
            ),
            Self::Revision(error) => error.fmt(formatter),
            Self::Refresh(error) => error.fmt(formatter),
            Self::CandidateParameters(error) => error.fmt(formatter),
            Self::Publication(failure) => write!(
                formatter,
                "structural graph publication failed: {failure:?}"
            ),
            Self::EventLog(error) => error.fmt(formatter),
            Self::Status(error) => error.fmt(formatter),
            Self::WorkerShutdown(error) => error.fmt(formatter),
            Self::SessionUnavailable => {
                formatter.write_str("structural session replacement is not configured")
            }
            Self::SessionBusy => {
                formatter.write_str("another structural replacement is already in flight")
            }
            Self::SessionRevisionMismatch => {
                formatter.write_str("session payload and prepared graph revisions do not match")
            }
            Self::SessionParameterMismatch => formatter
                .write_str("session payload projection does not match the prepared graph snapshot"),
            Self::SessionPreflight(error) => {
                write!(
                    formatter,
                    "session replacement preflight was rejected: {error}"
                )
            }
            Self::SessionCommit(error) => {
                write!(
                    formatter,
                    "activated session replacement was rejected: {error}"
                )
            }
            Self::CandidatePreflight(error) => {
                write!(
                    formatter,
                    "structural candidate preflight was rejected: {error}"
                )
            }
            Self::CandidateParameterMismatch => formatter.write_str(
                "future reducer commit does not match the prepared structural candidate",
            ),
            Self::Recovery(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for StructuralAdvanceError {}

struct EngineSelectionRuntime {
    factory: DescriptorDefaultConfigFactory,
    worker: Box<dyn GraphPreparationWorker>,
    coordinator: StructuralGraphCoordinator<Box<dyn ControlStructuralGraphBoundary>>,
    audio_config: AudioDeviceConfig,
    activation_record_sequence: Option<(EngineSelectionRequestId, u64)>,
    pending_session_replacement: Option<SessionReplacementPayload>,
}

struct OwnedMidiConnection {
    request: ConnectMidiInput,
    ingress: PhysicalMidiIngressControl,
    active: ActiveMidiInput,
}

struct MidiDeviceRuntime {
    worker: Box<dyn MidiDeviceWorker>,
    scheduler: MidiScanScheduler,
    deferred_command: Option<MidiDeviceWorkerCommand>,
    pending_ingress: Option<(ConnectMidiInput, PhysicalMidiIngressControl)>,
    candidate: Option<OwnedMidiConnection>,
    active: Option<OwnedMidiConnection>,
    retiring: Vec<OwnedMidiConnection>,
    accepted_count: u64,
    last_event: Option<crate::control::PhysicalMidiEvent>,
    last_control_receipt_micros: u64,
    observed_overflow_epoch: u64,
    last_activity_publish_micros: Option<u64>,
    latest_activity: Option<MidiActivitySnapshot>,
}

impl MidiDeviceRuntime {
    fn new(worker: Box<dyn MidiDeviceWorker>) -> Self {
        Self {
            worker,
            scheduler: MidiScanScheduler::default(),
            deferred_command: None,
            pending_ingress: None,
            candidate: None,
            active: None,
            retiring: Vec::new(),
            accepted_count: 0,
            last_event: None,
            last_control_receipt_micros: 0,
            observed_overflow_epoch: 0,
            last_activity_publish_micros: None,
            latest_activity: None,
        }
    }
}

impl DispatchResult {
    /// Returns the reducer event identifying the accepted generation.
    pub const fn accepted(&self) -> StateAccepted {
        self.accepted
    }

    /// Returns the canonical serialization of the accepted state.
    pub const fn snapshot(&self) -> &StateSnapshot {
        &self.snapshot
    }

    /// Returns a rejected audio command when the bounded queue was full.
    ///
    /// Queue saturation is not an EventRejection because AppState was already
    /// accepted and its complete parameter projection was already published.
    pub const fn boundary_full(&self) -> Option<BoundaryFull> {
        self.boundary_full
    }

    /// Reports whether every accepted real-time effect was transferred.
    pub const fn audio_effects_published(&self) -> bool {
        self.boundary_full.is_none()
    }
}

/// The one-way control application service.
///
/// The loop is the only owner that exposes mutation of AppState. Input and view
/// adapters receive only dispatch and immutable projection and observation
/// operations.
pub struct AppLoop<Boundary>
where
    Boundary: ControlAudioBoundary,
{
    state: AppState,
    projector: StateProjector,
    boundary: Boundary,
    current_snapshot: StateSnapshot,
    current_patch_page: Option<PatchPageProjection>,
    current_text: TextProjection,
    current_graphical_shell: GraphicalShellProjection,
    current_parameters: crate::real_time::parameter_snapshot::ParameterSnapshot,
    // During activation this can target the candidate while current_parameters
    // still describes the source. MIDI changes only its generation, not values.
    last_published_parameters: ParameterSnapshot,
    current_state_tree: StateTree,
    event_log: EventLog,
    engine_selection_runtime: Option<EngineSelectionRuntime>,
    pending_engine_selection_request: Option<EngineSelectionEffect>,
    deferred_engine_failure: Option<AppEvent>,
    deferred_revision_error: Option<GraphRevisionError>,
    midi_device_runtime: Option<MidiDeviceRuntime>,
    pending_midi_device_effects: VecDeque<MidiDeviceEffect>,
    pending_session_file_actions: VecDeque<crate::control::SessionCommand>,
}

impl<Boundary> AppLoop<Boundary>
where
    Boundary: ControlAudioBoundary,
{
    /// Creates the loop with a bounded interactive event journal and publishes
    /// the complete startup parameter state.
    pub fn new(
        state: AppState,
        projector: StateProjector,
        boundary: Boundary,
    ) -> Result<Self, StateProjectionError> {
        let event_log = EventLog::new(DEFAULT_EVENT_LOG_CAPACITY)
            .expect("the default event-log capacity is nonzero");
        Self::with_event_log(state, projector, boundary, event_log)
    }

    /// Creates the loop with a caller-sized event journal.
    ///
    /// Deterministic scenes can supply an EventLog large enough for their full
    /// trace, while interactive composition roots use `new`'s bounded default.
    pub fn with_event_log(
        state: AppState,
        projector: StateProjector,
        mut boundary: Boundary,
        event_log: EventLog,
    ) -> Result<Self, StateProjectionError> {
        let (
            current_snapshot,
            current_patch_page,
            current_text,
            current_graphical_shell,
            parameters,
            current_state_tree,
        ) = projector.project_with_shell_tree(&state)?;
        if parameters.graph_revision() != projector.graph_revision() {
            return Err(StateProjectionError::StateTree(
                crate::control::StateTreeError::GraphRevisionMismatch,
            ));
        }
        boundary.publish_parameters(parameters.clone());

        Ok(Self {
            state,
            projector,
            boundary,
            current_snapshot,
            current_patch_page,
            current_text,
            current_graphical_shell,
            current_parameters: parameters.clone(),
            last_published_parameters: parameters,
            current_state_tree,
            event_log,
            engine_selection_runtime: None,
            pending_engine_selection_request: None,
            deferred_engine_failure: None,
            deferred_revision_error: None,
            midi_device_runtime: None,
            pending_midi_device_effects: VecDeque::new(),
            pending_session_file_actions: VecDeque::new(),
        })
    }

    /// Installs the one physical MIDI worker and starts preference restoration.
    pub fn configure_midi_devices<Worker>(
        &mut self,
        worker: Worker,
    ) -> Result<(), MidiDeviceAdvanceError>
    where
        Worker: MidiDeviceWorker + 'static,
    {
        if self.midi_device_runtime.is_some() {
            return Err(MidiDeviceAdvanceError::AlreadyConfigured);
        }
        if !self.boundary.has_recovery_reserve() {
            return Err(MidiDeviceAdvanceError::RecoveryReserveUnavailable);
        }
        let mut runtime = MidiDeviceRuntime::new(Box::new(worker));
        runtime
            .worker
            .try_submit(MidiDeviceWorkerCommand::LoadPreference)
            .map_err(|_| MidiDeviceAdvanceError::WorkerUnavailable)?;
        self.midi_device_runtime = Some(runtime);
        Ok(())
    }

    /// Advances physical MIDI orchestration in a fixed, bounded order:
    /// scheduling, deferred/effect submission, one worker result, resulting
    /// effects, one 64-event ingress drain, then a decimated observation.
    pub fn advance_midi_devices(
        &mut self,
        now_micros: u64,
    ) -> Result<MidiDeviceProgress, MidiDeviceAdvanceError> {
        self.advance_midi_devices_with_capture(now_micros, &mut |_| false)
    }

    /// Advances the existing device owner and offers each accepted event once,
    /// before channel fan-out. Returning true consumes default monitoring.
    /// The callback runs on the control thread, never in the backend callback.
    /// Consumed events still contribute to physical-input activity observations.
    pub fn advance_midi_devices_with_capture(
        &mut self,
        now_micros: u64,
        capture: &mut dyn FnMut(PhysicalMidiEvent) -> bool,
    ) -> Result<MidiDeviceProgress, MidiDeviceAdvanceError> {
        self.advance_midi_devices_with_packets(now_micros, &mut |packet| match packet {
            crate::control::PhysicalMidiPacket::Performance(event) => capture(event),
            crate::control::PhysicalMidiPacket::Transport(_) => true,
        })
    }

    /// Offers channel and transport packets in accepted backend order. Transport
    /// packets are host-owned and never enter the Synth reducer or renderer.
    pub fn advance_midi_devices_with_packets(
        &mut self,
        now_micros: u64,
        capture: &mut dyn FnMut(crate::control::PhysicalMidiPacket) -> bool,
    ) -> Result<MidiDeviceProgress, MidiDeviceAdvanceError> {
        let Some(mut runtime) = self.midi_device_runtime.take() else {
            return Ok(MidiDeviceProgress::default());
        };
        let mut progress = MidiDeviceProgress::default();

        let settings_open =
            self.state.interaction().active_surface() == SurfaceId::MidiDeviceSettings;
        if !self.state.midi_input().shutting_down()
            && runtime.scheduler.tick(
                now_micros,
                settings_open,
                self.state.midi_input().scan().in_flight(),
            )
        {
            match self.dispatch_from(AppEvent::MidiInputScanStarted, EventSource::System) {
                Ok(_) => progress.scan_requested = true,
                Err(rejection) => progress.rejected_worker_event = Some(rejection),
            }
        }

        if let Some(command) = runtime.deferred_command.take() {
            progress.effect_advanced |= submit_midi_worker_command(&mut runtime, command);
        }
        for _ in 0..MIDI_DEVICE_EFFECT_BUDGET {
            if runtime.deferred_command.is_some() {
                break;
            }
            let Some(effect) = self.pending_midi_device_effects.pop_front() else {
                break;
            };
            progress.effect_advanced |=
                self.advance_midi_device_effect(&mut runtime, effect, &mut progress);
        }

        if let Some(result) = runtime.worker.try_poll() {
            progress.worker_result_polled = true;
            self.handle_midi_worker_result(&mut runtime, result, &mut progress);
        }

        for _ in 0..MIDI_DEVICE_EFFECT_BUDGET {
            if runtime.deferred_command.is_some() {
                break;
            }
            let Some(effect) = self.pending_midi_device_effects.pop_front() else {
                break;
            };
            progress.effect_advanced |=
                self.advance_midi_device_effect(&mut runtime, effect, &mut progress);
        }

        self.drain_physical_midi(&mut runtime, now_micros, &mut progress, capture);
        publish_midi_activity(&mut runtime, now_micros);
        self.midi_device_runtime = Some(runtime);
        Ok(progress)
    }

    fn advance_midi_device_effect(
        &mut self,
        runtime: &mut MidiDeviceRuntime,
        effect: MidiDeviceEffect,
        progress: &mut MidiDeviceProgress,
    ) -> bool {
        match effect {
            MidiDeviceEffect::Scan { scan_id } => {
                submit_midi_worker_command(runtime, MidiDeviceWorkerCommand::Scan { scan_id })
            }
            MidiDeviceEffect::Connect { request } => {
                if runtime.pending_ingress.is_some() || runtime.candidate.is_some() {
                    self.pending_midi_device_effects
                        .push_front(MidiDeviceEffect::Connect { request });
                    return false;
                }
                let (ingress, control) = PhysicalMidiIngress::bounded(request.revision());
                runtime.pending_ingress = Some((request.clone(), control));
                submit_midi_worker_command(
                    runtime,
                    MidiDeviceWorkerCommand::Connect { request, ingress },
                )
            }
            MidiDeviceEffect::Activate { request } => {
                let matches_candidate = runtime
                    .candidate
                    .as_ref()
                    .is_some_and(|candidate| candidate.request == request);
                if !matches_candidate {
                    progress.rejected_worker_event =
                        Some(EventRejection::MismatchedEngineSelection);
                    return false;
                }
                if let Some(old) = runtime.active.as_mut() {
                    old.ingress.disable();
                    old.ingress.discard_all();
                    if self.boundary.push_input_recovery_command().is_err() {
                        self.pending_midi_device_effects
                            .push_front(MidiDeviceEffect::Activate { request });
                        return false;
                    }
                }
                match self.dispatch_from(
                    AppEvent::MidiInputActivationAcknowledged {
                        request_id: request.request_id(),
                        revision: request.revision(),
                    },
                    EventSource::Worker,
                ) {
                    Ok(_) => {
                        let candidate = runtime
                            .candidate
                            .take()
                            .expect("matching candidate was checked above");
                        if let Some(old) = runtime.active.replace(candidate) {
                            runtime.retiring.push(old);
                        }
                        let active = runtime.active.as_ref().expect("candidate was installed");
                        active.ingress.enable();
                        runtime.accepted_count = 0;
                        runtime.last_event = None;
                        runtime.last_control_receipt_micros = 0;
                        runtime.observed_overflow_epoch = active.ingress.overflow_epoch();
                        runtime.last_activity_publish_micros = None;
                        runtime.latest_activity = None;
                        true
                    }
                    Err(rejection) => {
                        progress.rejected_worker_event = Some(rejection);
                        if let Some(candidate) = runtime.candidate.take() {
                            retire_midi_connection(runtime, candidate);
                        }
                        false
                    }
                }
            }
            MidiDeviceEffect::Recover { revision, .. } => {
                disable_revision(runtime, revision);
                if self.boundary.push_input_recovery_command().is_err() {
                    self.pending_midi_device_effects.push_front(effect);
                    false
                } else {
                    true
                }
            }
            MidiDeviceEffect::Retire { revision, .. } => {
                if let Some(connection) = take_midi_connection(runtime, revision) {
                    retire_midi_connection(runtime, connection)
                } else {
                    true
                }
            }
            MidiDeviceEffect::CancelCandidate { request } => {
                if let Some((pending, control)) = runtime.pending_ingress.as_mut() {
                    if pending == &request {
                        control.disable();
                        control.discard_all();
                    }
                }
                if runtime
                    .candidate
                    .as_ref()
                    .is_some_and(|candidate| candidate.request == request)
                {
                    let candidate = runtime.candidate.take().expect("checked above");
                    retire_midi_connection(runtime, candidate)
                } else {
                    true
                }
            }
            MidiDeviceEffect::Persist { preference } => submit_midi_worker_command(
                runtime,
                MidiDeviceWorkerCommand::StorePreference { preference },
            ),
            MidiDeviceEffect::Shutdown => true,
        }
    }

    fn handle_midi_worker_result(
        &mut self,
        runtime: &mut MidiDeviceRuntime,
        result: MidiDeviceWorkerResult,
        progress: &mut MidiDeviceProgress,
    ) {
        let event = match result {
            MidiDeviceWorkerResult::ScanSucceeded {
                scan_id,
                descriptors,
            } => Some(AppEvent::MidiInputScanSucceeded {
                scan_id,
                descriptors,
            }),
            MidiDeviceWorkerResult::ScanFailed { scan_id, failure } => {
                Some(AppEvent::MidiInputScanFailed { scan_id, failure })
            }
            MidiDeviceWorkerResult::ConnectionPrepared { request, active } => {
                let control = match runtime.pending_ingress.take() {
                    Some((pending, control)) if pending == request => Some(control),
                    Some(other) => {
                        runtime.pending_ingress = Some(other);
                        None
                    }
                    None => None,
                };
                let current = self
                    .state
                    .midi_input()
                    .requested()
                    .is_some_and(|pending| pending == &request);
                if current {
                    if let Some(control) = control {
                        runtime.candidate = Some(OwnedMidiConnection {
                            request: request.clone(),
                            ingress: control,
                            active,
                        });
                        Some(AppEvent::MidiInputConnectionPrepared {
                            request_id: request.request_id(),
                            revision: request.revision(),
                        })
                    } else {
                        submit_midi_worker_command(
                            runtime,
                            MidiDeviceWorkerCommand::Retire { active },
                        );
                        progress.rejected_worker_event =
                            Some(EventRejection::MismatchedEngineSelection);
                        None
                    }
                } else {
                    if let Some(mut control) = control {
                        control.disable();
                        control.discard_all();
                    }
                    submit_midi_worker_command(runtime, MidiDeviceWorkerCommand::Retire { active });
                    progress.rejected_worker_event = Some(EventRejection::StaleEngineSelection);
                    None
                }
            }
            MidiDeviceWorkerResult::ConnectionFailed { request, failure } => {
                if runtime
                    .pending_ingress
                    .as_ref()
                    .is_some_and(|(pending, _)| pending == &request)
                {
                    if let Some((_pending, mut control)) = runtime.pending_ingress.take() {
                        control.disable();
                        control.discard_all();
                    }
                }
                Some(AppEvent::MidiInputOperationFailed {
                    identity: request.identity().clone(),
                    request_id: Some(request.request_id()),
                    revision: Some(request.revision()),
                    failure,
                })
            }
            MidiDeviceWorkerResult::Retired {
                identity,
                request_id,
                revision,
                failure: Some(failure),
            } => Some(AppEvent::MidiInputOperationFailed {
                identity,
                request_id: Some(request_id),
                revision: Some(revision),
                failure,
            }),
            MidiDeviceWorkerResult::Retired { failure: None, .. } => None,
            MidiDeviceWorkerResult::PreferenceLoaded {
                preference,
                failure,
            } => Some(AppEvent::MidiInputPreferenceRestored {
                preference,
                failure,
            }),
            MidiDeviceWorkerResult::PreferenceStored {
                preference: _,
                failure: Some(failure),
            } => Some(AppEvent::MidiInputPreferenceStoreFailed { failure }),
            MidiDeviceWorkerResult::PreferenceStored { failure: None, .. } => None,
        };
        if let Some(event) = event {
            if let Err(rejection) = self.dispatch_from(event, EventSource::Worker) {
                progress.rejected_worker_event = Some(rejection);
            }
        }
    }

    fn drain_physical_midi(
        &mut self,
        runtime: &mut MidiDeviceRuntime,
        now_micros: u64,
        progress: &mut MidiDeviceProgress,
        capture: &mut dyn FnMut(crate::control::PhysicalMidiPacket) -> bool,
    ) {
        let Some(active) = runtime.active.as_mut() else {
            return;
        };
        let canonical_revision = self
            .state
            .midi_input()
            .active()
            .map(|identity| identity.revision());
        if canonical_revision != Some(active.request.revision()) {
            active.ingress.disable();
            progress.stale_events += active.ingress.discard_all();
            return;
        }

        let overflow_epoch = active.ingress.overflow_epoch();
        if overflow_epoch != runtime.observed_overflow_epoch {
            let dropped = overflow_epoch.saturating_sub(runtime.observed_overflow_epoch);
            runtime.observed_overflow_epoch = overflow_epoch;
            active.ingress.disable();
            active.ingress.discard_all();
            progress.ingress_overflowed = true;
            if let Err(rejection) = self.dispatch_from(
                AppEvent::MidiInputOperationFailed {
                    identity: active.request.identity().clone(),
                    request_id: None,
                    revision: Some(active.request.revision()),
                    failure: MidiDeviceFailure::TransportCapacity {
                        stage: crate::control::MidiTransportCapacityStage::PhysicalIngress,
                        dropped,
                    },
                },
                EventSource::Worker,
            ) {
                progress.rejected_worker_event = Some(rejection);
            }
            return;
        }

        for _ in 0..PHYSICAL_MIDI_DRAIN_BUDGET {
            let Some(event) = active.ingress.try_pop_packet() else {
                break;
            };
            if event.revision() != canonical_revision.expect("checked above") {
                progress.stale_events += 1;
                continue;
            }
            if let crate::control::PhysicalMidiPacket::Transport(_) = event {
                progress.drained_events += 1;
                capture(event);
                continue;
            }
            let crate::control::PhysicalMidiPacket::Performance(event) = event else {
                unreachable!()
            };
            let event = event.received_on_control(now_micros);
            progress.drained_events += 1;
            runtime.accepted_count = runtime.accepted_count.saturating_add(1);
            runtime.last_event = Some(event);
            runtime.last_control_receipt_micros = now_micros;
            if capture(crate::control::PhysicalMidiPacket::Performance(event)) {
                continue;
            }
            match self.dispatch_midi_from(event.message(), EventSource::PhysicalMidi) {
                Ok(result) if result.boundary_full().is_none() => {}
                Ok(_) => {
                    active.ingress.disable();
                    progress.audio_saturated = true;
                    if let Err(rejection) = self.dispatch_from(
                        AppEvent::MidiInputOperationFailed {
                            identity: active.request.identity().clone(),
                            request_id: None,
                            revision: Some(active.request.revision()),
                            failure: MidiDeviceFailure::TransportCapacity {
                                stage: crate::control::MidiTransportCapacityStage::AudioCommand,
                                dropped: 1,
                            },
                        },
                        EventSource::Worker,
                    ) {
                        progress.rejected_worker_event = Some(rejection);
                    }
                    break;
                }
                Err(rejection) => {
                    progress.rejected_worker_event = Some(rejection);
                    break;
                }
            }
        }
    }

    /// Exact reducer-acknowledged device identity for physical capture context.
    /// The event's revision must match this identity before a host associates it.
    pub fn active_midi_input_identity(&self) -> Option<&crate::control::MidiActiveInputIdentity> {
        self.state.midi_input().active()
    }

    /// Latest decimated activity only when its revision is still canonical.
    pub fn current_midi_activity(&self) -> Option<MidiActivitySnapshot> {
        let snapshot = self.midi_device_runtime.as_ref()?.latest_activity?;
        self.state
            .midi_input()
            .active()
            .is_some_and(|active| active.revision() == snapshot.revision())
            .then_some(snapshot)
    }

    /// Presentation-only Receiving state; it never crosses the reducer.
    pub fn midi_is_receiving(&self, now_micros: u64) -> bool {
        self.current_midi_activity().is_some_and(|snapshot| {
            snapshot.last_event().is_some()
                && now_micros.saturating_sub(snapshot.last_control_receipt_micros())
                    <= MIDI_RECEIVING_WINDOW_MICROS
        })
    }

    /// Reads the latest-compatible display observation without changing the
    /// reducer or any product serialization.
    pub fn current_midi_activity_observation(
        &self,
        now_micros: u64,
    ) -> crate::control::MidiActivityObservation {
        let snapshot = self.current_midi_activity();
        crate::control::MidiActivityObservation::new(
            snapshot,
            snapshot.is_some() && self.midi_is_receiving(now_micros),
        )
    }

    /// Stops physical MIDI ownership on the calling control thread after all
    /// gates are disabled and active/candidate handles have been transferred
    /// to the device worker for consuming retirement.
    pub fn shutdown_midi_devices_on_control(&mut self) -> Result<(), MidiDeviceAdvanceError> {
        if self.midi_device_runtime.is_none() {
            return Ok(());
        }
        if !self.state.midi_input().shutting_down() {
            let _ = self.dispatch_from(AppEvent::MidiInputShutdownRequested, EventSource::System);
        }
        let mut runtime = self
            .midi_device_runtime
            .take()
            .expect("configuration was checked above");
        if let Some((_request, mut ingress)) = runtime.pending_ingress.take() {
            ingress.disable();
            ingress.discard_all();
        }
        let mut retirements = Vec::new();
        if let Some(MidiDeviceWorkerCommand::Retire { active }) = runtime.deferred_command.take() {
            retirements.push(active);
        }
        for mut connection in runtime
            .candidate
            .take()
            .into_iter()
            .chain(runtime.active.take())
            .chain(core::mem::take(&mut runtime.retiring))
        {
            connection.ingress.disable();
            connection.ingress.discard_all();
            retirements.push(connection.active);
        }
        let _ = self.boundary.push_recovery_command();
        runtime
            .worker
            .shutdown_with_retirements_on_control(retirements)
            .map_err(|_| MidiDeviceAdvanceError::WorkerShutdown)?;
        self.pending_midi_device_effects.clear();
        Ok(())
    }

    pub const fn midi_devices_configured(&self) -> bool {
        self.midi_device_runtime.is_some()
    }

    /// Counts control-owned backend handles. Completed teardown reports zero.
    pub fn owned_midi_connections_on_control(&self) -> usize {
        self.midi_device_runtime.as_ref().map_or(0, |runtime| {
            usize::from(runtime.candidate.is_some())
                + usize::from(runtime.active.is_some())
                + runtime.retiring.len()
                + usize::from(matches!(
                    runtime.deferred_command,
                    Some(MidiDeviceWorkerCommand::Retire { .. })
                ))
        })
    }

    /// Installs the one worker and one structural-control owner used by engine selection.
    pub fn configure_engine_selection<Worker, Structural>(
        &mut self,
        factory: DescriptorDefaultConfigFactory,
        worker: Worker,
        structural: Structural,
        initial_graph: &PreparedGraph,
        audio_config: AudioDeviceConfig,
    ) -> Result<(), StructuralAdvanceError>
    where
        Worker: GraphPreparationWorker + 'static,
        Structural: ControlStructuralGraphBoundary + 'static,
    {
        if self.engine_selection_runtime.is_some() {
            return Err(StructuralAdvanceError::AlreadyConfigured);
        }
        if factory.registry() != self.state.capabilities() {
            return Err(StructuralAdvanceError::RegistryMismatch);
        }
        let structural: Box<dyn ControlStructuralGraphBoundary> = Box::new(structural);
        self.engine_selection_runtime = Some(EngineSelectionRuntime {
            factory,
            worker: Box::new(worker),
            coordinator: StructuralGraphCoordinator::new(structural, initial_graph),
            audio_config,
            activation_record_sequence: None,
            pending_session_replacement: None,
        });
        Ok(())
    }

    /// Allocates the next graph revision from the shared structural owner.
    /// Engine/effect edits and session preparation both use this method, so
    /// two producers cannot independently choose the same target revision.
    pub fn next_structural_graph_revision(&self) -> Result<GraphRevision, StructuralAdvanceError> {
        let runtime = self
            .engine_selection_runtime
            .as_ref()
            .ok_or(StructuralAdvanceError::SessionUnavailable)?;
        if runtime.coordinator.has_pending_graph()
            || runtime.pending_session_replacement.is_some()
            || self.state.engine_selection().kind() != EngineSelectionStatusKind::Ready
        {
            return Err(StructuralAdvanceError::SessionBusy);
        }
        runtime
            .coordinator
            .status()
            .active_revision()
            .ok_or(StructuralAdvanceError::Publication(
                crate::real_time::GraphPublicationFailure::NoActiveGraph,
            ))?
            .checked_next()
            .map_err(StructuralAdvanceError::Revision)
    }

    /// Preflights and stages one complete prepared session through the same
    /// one-in-flight coordinator used by engine and effect topology changes.
    pub fn stage_session_replacement(
        &mut self,
        replacement: SessionReplacementPayload,
        mut graph: PreparedGraph,
    ) -> Result<GraphStageOutcome, StructuralAdvanceError> {
        let target_graph_revision = replacement.target_graph_revision();
        if graph.revision() != target_graph_revision {
            return Err(StructuralAdvanceError::SessionRevisionMismatch);
        }
        {
            let runtime = self
                .engine_selection_runtime
                .as_ref()
                .ok_or(StructuralAdvanceError::SessionUnavailable)?;
            if runtime.coordinator.has_pending_graph()
                || runtime.pending_session_replacement.is_some()
                || self.state.engine_selection().kind() != EngineSelectionStatusKind::Ready
            {
                return Err(StructuralAdvanceError::SessionBusy);
            }
        }

        let mut candidate = self.state.clone();
        candidate
            .apply(AppEvent::ReplacePersistedSession(Box::new(
                replacement.clone(),
            )))
            .map_err(StructuralAdvanceError::SessionPreflight)?;
        let projected = StateProjector::for_graph(target_graph_revision)
            .project(&candidate)
            .map_err(|error| match error {
                StateProjectionError::ParameterSnapshot(error) => {
                    StructuralAdvanceError::CandidateParameters(error)
                }
                _ => StructuralAdvanceError::SessionParameterMismatch,
            })?
            .2;
        let prepared_at_candidate_generation = graph
            .initial_parameters()
            .clone()
            .with_generation(projected.generation());
        if prepared_at_candidate_generation != projected {
            return Err(StructuralAdvanceError::SessionParameterMismatch);
        }
        graph
            .refresh_initial_parameters(projected.clone())
            .map_err(StructuralAdvanceError::Refresh)?;
        graph.set_carry_over_scope(crate::real_time::GraphReplacementScope::WholeSession);

        self.boundary
            .push_recovery_command()
            .map_err(StructuralAdvanceError::Recovery)?;
        let outcome = self
            .engine_selection_runtime
            .as_mut()
            .expect("the shared structural runtime was checked above")
            .coordinator
            .stage_replacement(graph, crate::real_time::GraphReplacementScope::WholeSession)
            .map_err(|error| {
                let reason = error.reason();
                drop(error.into_graph());
                StructuralAdvanceError::Publication(reason)
            })?;
        self.engine_selection_runtime
            .as_mut()
            .expect("the shared structural runtime was checked above")
            .pending_session_replacement = Some(replacement);
        Ok(outcome)
    }

    /// Applies one event using the stable source for legacy callers.
    pub fn dispatch(&mut self, event: AppEvent) -> Result<DispatchResult, EventRejection> {
        self.dispatch_from(event, EventSource::System)
    }

    /// Takes one accepted Settings file intent for shell-side execution.
    pub fn take_session_file_action(&mut self) -> Option<crate::control::SessionCommand> {
        self.pending_session_file_actions.pop_front()
    }

    /// Maps one normalized user intent to exactly one AppEvent before the
    /// canonical reducer and existing commit-before-project pipeline.
    pub fn dispatch_action(
        &mut self,
        action: SemanticAction,
    ) -> Result<DispatchResult, EventRejection> {
        self.dispatch_action_from(action, EventSource::Keyboard)
    }

    pub fn dispatch_action_from(
        &mut self,
        action: SemanticAction,
        source: EventSource,
    ) -> Result<DispatchResult, EventRejection> {
        let event = AppEvent::from_semantic_action(action.clone());
        self.dispatch_internal(event, source, Some(action), None, None)
    }

    /// Applies one sourced event and publishes effects only after complete acceptance.
    pub fn dispatch_from(
        &mut self,
        event: AppEvent,
        source: EventSource,
    ) -> Result<DispatchResult, EventRejection> {
        self.dispatch_internal(event, source, None, None, None)
    }

    /// Validates scheduled MIDI through the normal reducer and publishes only
    /// to the timed queue. A full queue remains an explicit failed delivery;
    /// it never falls back to an immediate attack.
    pub fn dispatch_scheduled_midi_from(
        &mut self,
        event: ScheduledEvent,
        source: EventSource,
    ) -> Result<DispatchResult, ScheduledEventDispatchError> {
        let active = self.active_audio_graph_revision();
        if event.graph_revision() != active {
            return Err(ScheduledEventDispatchError::GraphRevision {
                requested: event.graph_revision(),
                active,
            });
        }
        let current_epoch = self.scheduled_epoch();
        if event.epoch() != current_epoch {
            return Err(ScheduledEventRejected::new(
                event,
                ScheduledEventRejectionReason::StaleEpoch { current_epoch },
            )
            .into());
        }
        let result = self.dispatch_internal(
            AppEvent::Midi {
                patch_id: event.patch_id(),
                message: event
                    .message()
                    .ok_or(ScheduledEventDispatchError::InvalidPayload)?,
            },
            source,
            None,
            Some(event),
            match event.payload() {
                crate::real_time::scheduled_event::ScheduledPayload::InitializedVoice {
                    voice,
                    initial,
                } => Some(AudioCommand::InitializedVoice {
                    source: crate::synth::MidiSource::Scheduled,
                    patch_id: event.patch_id(),
                    voice,
                    initial,
                }),
                crate::real_time::scheduled_event::ScheduledPayload::Midi {
                    voice: Some(voice),
                    scope,
                    message,
                } => Some(AudioCommand::VoiceMidi {
                    source: crate::synth::MidiSource::Scheduled,
                    patch_id: event.patch_id(),
                    voice,
                    scope,
                    message,
                }),
                _ => None,
            },
        )?;
        if let Some(rejection) = result.scheduled_rejection {
            return Err(rejection.into());
        }
        Ok(result)
    }

    /// Effective render controls never edit saved Patch or mixer state. They
    /// share the timed event queue, graph fence and cancellation epoch with notes.
    pub fn dispatch_scheduled_send(
        &mut self,
        event: ScheduledEvent,
    ) -> Result<(), ScheduledEventDispatchError> {
        use crate::real_time::scheduled_event::ScheduledPayload;
        if self.session_replacement_pending() {
            return Err(EventRejection::StructuralEditBusy.into());
        }
        let active = self.active_audio_graph_revision();
        if event.graph_revision() != active {
            return Err(ScheduledEventDispatchError::GraphRevision {
                requested: event.graph_revision(),
                active,
            });
        }
        let current_epoch = self.scheduled_epoch();
        if event.epoch() != current_epoch {
            return Err(ScheduledEventRejected::new(
                event,
                ScheduledEventRejectionReason::StaleEpoch { current_epoch },
            )
            .into());
        }
        let patch = self
            .current_parameters
            .patches()
            .iter()
            .find(|p| p.patch_id() == Some(event.patch_id()))
            .ok_or(EventRejection::UnknownPatch)?;
        match event.payload() {
            ScheduledPayload::SendLease(lease) => {
                if lease.bus().index() >= patch.sends().len() {
                    return Err(ScheduledEventDispatchError::UnknownSendBus);
                }
                if lease.end_frame() <= event.absolute_frame() {
                    return Err(crate::real_time::SendLeaseError::AlreadyExpired {
                        end_frame: lease.end_frame(),
                        current_frame: event.absolute_frame(),
                    }
                    .into());
                }
            }
            ScheduledPayload::EndSendLease(_) => {}
            _ => return Err(ScheduledEventDispatchError::InvalidPayload),
        }
        self.boundary.push_scheduled(event)?;
        Ok(())
    }

    /// The callback-acknowledged graph, including during a pending activation.
    pub fn active_audio_graph_revision(&self) -> GraphRevision {
        self.engine_graph_handoff_status()
            .and_then(|status| status.active_revision())
            .unwrap_or_else(|| self.graph_revision())
    }

    /// Current generation of the cancellable scheduled MIDI transport.
    pub fn scheduled_epoch(&self) -> u64 {
        self.boundary.scheduled_epoch()
    }

    /// Invalidates queued scheduled events and releases their owned notes at
    /// the next audio block without silencing unrelated live input.
    pub fn cancel_scheduled_midi(&mut self) -> Result<u64, ScheduledEventEpochExhausted> {
        self.boundary.cancel_scheduled()
    }

    /// Fans one normalized MIDI message out to every current channel
    /// subscriber in stable Patch installation order.
    ///
    /// Channel resolution lives here rather than in a physical or fixture
    /// adapter, so every source shares the same many-listener behavior. Each
    /// recipient still crosses `AppState::apply` as one targeted event before
    /// its ordered real-time command is published. A channel with no
    /// subscribers is an accepted no-op with no generation change.
    pub fn dispatch_midi_from(
        &mut self,
        message: MidiMessage,
        source: EventSource,
    ) -> Result<MidiFanOutResult, EventRejection> {
        if self.session_replacement_pending() {
            return Err(EventRejection::StructuralEditBusy);
        }
        let mut patch_ids = [None::<PatchId>; crate::real_time::MAX_ACTIVE_PATCHES];
        let mut subscriber_count = 0;
        for patch in self
            .state
            .patches()
            .iter()
            .filter(|patch| patch.channel() == message.channel())
        {
            *patch_ids
                .get_mut(subscriber_count)
                .expect("accepted AppState cannot exceed the fixed Patch capacity") =
                Some(patch.id());
            subscriber_count += 1;
        }

        for patch_id in patch_ids.into_iter().take(subscriber_count).flatten() {
            let result = self.dispatch_from(AppEvent::Midi { patch_id, message }, source)?;
            if let Some(boundary_full) = result.boundary_full() {
                return Ok(MidiFanOutResult {
                    subscriber_count,
                    boundary_full: Some(boundary_full),
                });
            }
        }

        Ok(MidiFanOutResult {
            subscriber_count,
            boundary_full: None,
        })
    }

    /// Host-owned immediate MIDI uses the same reducer and journal validation.
    pub fn dispatch_midi_for_source(
        &mut self,
        patch_id: PatchId,
        message: MidiMessage,
        source: crate::synth::MidiSource,
    ) -> Result<DispatchResult, EventRejection> {
        self.dispatch_internal(
            AppEvent::Midi { patch_id, message },
            EventSource::AutomaticMidi,
            None,
            None,
            Some(AudioCommand::OwnedMidi {
                source,
                patch_id,
                message,
            }),
        )
    }
    /// Exact articulation ownership uses the same reducer/journal as raw MIDI.
    pub fn dispatch_voice_for_source(
        &mut self,
        patch_id: PatchId,
        message: MidiMessage,
        source: crate::synth::MidiSource,
        voice: crate::synth::MidiVoiceId,
        scope: crate::synth::MidiExpressionScope,
    ) -> Result<DispatchResult, EventRejection> {
        self.dispatch_internal(
            AppEvent::Midi { patch_id, message },
            EventSource::AutomaticMidi,
            None,
            None,
            Some(AudioCommand::VoiceMidi {
                source,
                patch_id,
                voice,
                scope,
                message,
            }),
        )
    }
    pub fn dispatch_initialized_voice_for_source(
        &mut self,
        patch_id: PatchId,
        source: crate::synth::MidiSource,
        voice: crate::synth::MidiVoiceId,
        initial: crate::synth::OwnedVoiceInitialization,
    ) -> Result<DispatchResult, EventRejection> {
        self.dispatch_internal(
            AppEvent::Midi {
                patch_id,
                message: initial.message(),
            },
            EventSource::AutomaticMidi,
            None,
            None,
            Some(AudioCommand::InitializedVoice {
                source,
                patch_id,
                voice,
                initial,
            }),
        )
    }
    pub fn cancel_midi_source(
        &mut self,
        patch_id: PatchId,
        source: crate::synth::MidiSource,
    ) -> Result<(), BoundaryFull> {
        self.boundary
            .push_command(AudioCommand::ReleaseSource { source, patch_id })
    }
    fn dispatch_internal(
        &mut self,
        event: AppEvent,
        source: EventSource,
        semantic_action: Option<SemanticAction>,
        scheduled: Option<ScheduledEvent>,
        midi_command: Option<AudioCommand>,
    ) -> Result<DispatchResult, EventRejection> {
        if self.session_replacement_pending()
            && matches!(
                event,
                AppEvent::Midi { .. }
                    | AppEvent::Adjust(_)
                    | AppEvent::Activate
                    | AppEvent::SetSlotOccupancy { .. }
                    | AppEvent::SetReturnOccupancy { .. }
                    | AppEvent::Send(
                        crate::control::SendAction::Rename { .. }
                            | crate::control::SendAction::SetEffect { .. }
                    )
                    | AppEvent::EngineSelectionLifecycleAdvanced { .. }
                    | AppEvent::EnginePrepared { .. }
                    | AppEvent::EnginePreparationFailed { .. }
                    | AppEvent::TopologyPrepared { .. }
                    | AppEvent::TopologyPreparationFailed { .. }
            )
        {
            return Err(EventRejection::StructuralEditBusy);
        }
        let generation_before = self.state.generation();
        let state_hash_before = self.current_state_tree.state_hash().to_owned();
        let midi_generation_only = matches!(event, AppEvent::Midi { .. });
        let parameters_published = event.publishes_parameters_on_acceptance()
            && !(matches!(event, AppEvent::Activate)
                && self.state.interaction().active_surface().is_system());

        let reduction = match semantic_action {
            Some(action) => self.state.apply_semantic_action(action),
            None => self.state.apply(event.clone()),
        };
        let outcome = match reduction {
            Ok(outcome) => outcome,
            Err(rejection) => {
                let record = EventRecord::rejected(
                    self.event_log.next_sequence(),
                    source,
                    &event,
                    generation_before,
                    state_hash_before,
                    generation_before,
                    &self.current_text,
                    rejection,
                )
                .expect("cached projections must describe the current rejected state");
                self.event_log
                    .append(record)
                    .expect("AppLoop must append a contiguous rejected event record");
                return Err(rejection);
            }
        };

        let (snapshot, patch_page, text, graphical_shell, parameters, state_tree) =
            if midi_generation_only {
                self.projector
                    .project_midi_generation(
                        &self.state,
                        MidiProjectionSeed::new(
                            &self.current_snapshot,
                            self.current_patch_page.as_ref(),
                            &self.current_text,
                            &self.current_graphical_shell,
                            self.current_parameters.clone(),
                            &self.current_state_tree,
                        ),
                    )
                    .expect("accepted MIDI must advance coherent generation-only projections")
            } else {
                self.projector
                    .project_with_shell_tree(&self.state)
                    .expect("an accepted AppState must produce coherent projections")
            };
        let accepted = outcome.accepted();
        let audio_command = outcome.audio_command().copied().map(|command| {
            if matches!(command, AudioCommand::PatchMidi { .. }) {
                midi_command.unwrap_or(command)
            } else {
                command
            }
        });
        let engine_selection_effect = outcome.engine_selection_effect().cloned();
        let midi_device_effects = outcome.midi_device_effects().to_vec();
        let record_sequence = self.event_log.next_sequence();
        let replacement_parameters =
            if midi_generation_only {
                // MIDI changes only generation. Retain the already published
                // values, including the active graph during a pending activation.
                None
            } else if parameters_published
                && self.state.engine_selection().kind() == EngineSelectionStatusKind::Activating
            {
                Some(self.latest_pending_candidate_parameters().expect(
                    "an accepted activating state has one valid candidate scalar projection",
                ))
            } else {
                Some(parameters.clone())
            };
        let published_parameters = replacement_parameters
            .as_ref()
            .unwrap_or(&self.last_published_parameters);
        let published_generation = if midi_generation_only {
            self.state.generation()
        } else {
            published_parameters.generation()
        };
        let record = EventRecord::accepted(
            record_sequence,
            source,
            &event,
            generation_before,
            state_hash_before,
            accepted,
            &snapshot,
            published_generation,
            published_parameters.graph_revision(),
            parameters_published,
            &text,
            audio_command,
            engine_selection_effect.clone(),
        )
        .expect("accepted reducer output and projections must form one coherent record");

        if parameters_published {
            if let Some(parameters) = replacement_parameters {
                self.last_published_parameters = parameters;
            } else {
                self.last_published_parameters
                    .update_generation(published_generation);
            }
            self.boundary
                .publish_parameters(self.last_published_parameters.clone());
        }
        let (boundary_full, scheduled_rejection) = if let Some(event) = scheduled {
            (None, self.boundary.push_scheduled(event).err())
        } else {
            (
                audio_command.and_then(|command| self.boundary.push_command(command).err()),
                None,
            )
        };
        self.current_snapshot = snapshot.clone();
        self.current_patch_page = patch_page;
        self.current_text = text;
        self.current_graphical_shell = graphical_shell;
        self.current_parameters = parameters;
        self.current_state_tree = state_tree;
        self.event_log
            .append(record)
            .expect("AppLoop must append a contiguous accepted event record");
        if let Some(effect) = engine_selection_effect
            .filter(|effect| effect.kind() == EngineSelectionEffectKind::PrepareRequested)
        {
            self.pending_engine_selection_request = Some(effect);
        }
        self.pending_midi_device_effects.extend(midi_device_effects);
        self.pending_session_file_actions
            .extend(outcome.session_file_action());

        Ok(DispatchResult {
            accepted,
            snapshot,
            boundary_full,
            scheduled_rejection,
        })
    }

    fn submit_engine_selection_request(&mut self, effect: &EngineSelectionEffect) {
        let target_graph_revision = match effect.source_graph_revision().checked_next() {
            Ok(revision) => revision,
            Err(error) => {
                self.deferred_revision_error = Some(error);
                return;
            }
        };
        let failure_event =
            |failure| structural_preparation_failed_event(effect, target_graph_revision, failure);
        let Some(runtime) = self.engine_selection_runtime.as_mut() else {
            self.deferred_engine_failure =
                Some(failure_event(EngineSelectionFailure::WorkerUnavailable));
            return;
        };

        if runtime
            .factory
            .use_asset_descriptors(self.state.capabilities())
            .is_err()
        {
            self.deferred_engine_failure =
                Some(failure_event(EngineSelectionFailure::ProviderMismatch));
            return;
        }
        if let StructuralEditIntent::AppendPatch { patch_id } = effect.intent() {
            let candidate = match self.state.pending_patch_creation() {
                Some(candidate) if candidate.id() == *patch_id => candidate.clone(),
                _ => {
                    self.deferred_engine_failure =
                        Some(failure_event(EngineSelectionFailure::GraphIncompatible));
                    return;
                }
            };
            let correlation = match GraphPreparationCorrelation::for_append(
                effect.request_id(),
                *patch_id,
                effect.source_graph_revision(),
                target_graph_revision,
            ) {
                Ok(correlation) => correlation,
                Err(error) => {
                    self.deferred_engine_failure = Some(failure_event(map_request_failure(error)));
                    return;
                }
            };
            let request = match GraphPreparationRequest::append_patch(
                correlation,
                self.state.patches(),
                candidate,
                self.state.bus_returns(),
                self.state.generation(),
                *self.state.global(),
                self.state.mixer().clone(),
                runtime.audio_config,
                runtime.factory.registry(),
                self.state.effects(),
            ) {
                Ok(request) => request,
                Err(error) => {
                    self.deferred_engine_failure = Some(failure_event(map_request_failure(error)));
                    return;
                }
            };
            if runtime.worker.try_submit(request).is_err() {
                self.deferred_engine_failure =
                    Some(failure_event(EngineSelectionFailure::WorkerUnavailable));
            }
            return;
        }

        // Occupancy intents carry the complete topology delta themselves;
        // the request applies it to the active Patch set and return bank.
        if effect.intent().is_occupancy() {
            let correlation = match GraphPreparationCorrelation::for_occupancy(
                effect.request_id(),
                effect.intent().clone(),
                effect.source_graph_revision(),
                target_graph_revision,
            ) {
                Ok(correlation) => correlation,
                Err(error) => {
                    self.deferred_engine_failure = Some(failure_event(map_request_failure(error)));
                    return;
                }
            };
            let request = match GraphPreparationRequest::occupancy(
                correlation,
                self.state.patches(),
                self.state.bus_returns(),
                self.state.generation(),
                *self.state.global(),
                self.state.mixer().clone(),
                runtime.audio_config,
                runtime.factory.registry(),
                self.state.effects(),
            ) {
                Ok(request) => request,
                Err(error) => {
                    self.deferred_engine_failure = Some(failure_event(map_request_failure(error)));
                    return;
                }
            };
            if runtime.worker.try_submit(request).is_err() {
                self.deferred_engine_failure =
                    Some(failure_event(EngineSelectionFailure::WorkerUnavailable));
            }
            return;
        }

        let (Some(patch_id), Some(source_capability_id), Some(target_capability_id)) = (
            effect.patch_id(),
            effect.source_capability_id(),
            effect.target_capability_id(),
        ) else {
            self.deferred_engine_failure =
                Some(failure_event(EngineSelectionFailure::GraphIncompatible));
            return;
        };
        let source_config = self
            .state
            .patches()
            .iter()
            .find(|patch| patch.id() == patch_id)
            .map(|patch| patch.instrument_config());
        let candidate_result = match effect.intent() {
            StructuralEditIntent::ReplaceCapability { .. } => {
                runtime.factory.create(target_capability_id)
            }
            StructuralEditIntent::ReplaceParameterChoice {
                parameter_id,
                choice_id,
                ..
            } => source_config
                .ok_or_else(|| CapabilityError::UnknownCapability(source_capability_id.clone()))
                .and_then(|source| {
                    runtime
                        .factory
                        .replace_structural_choice(source, parameter_id, choice_id)
                }),
            StructuralEditIntent::ReplaceAsset {
                parameter_id,
                reference,
                ..
            }
            | StructuralEditIntent::PrepareAudition {
                parameter_id,
                reference,
                ..
            } => source_config
                .ok_or_else(|| CapabilityError::UnknownCapability(source_capability_id.clone()))
                .and_then(|source| {
                    runtime
                        .factory
                        .replace_asset(source, parameter_id, reference.clone())
                }),
            StructuralEditIntent::SetVoiceBudget { .. }
            | StructuralEditIntent::ReplaceEffectAsset { .. }
            | StructuralEditIntent::SetSlotOccupancy { .. }
            | StructuralEditIntent::SetSendEffect { .. }
            | StructuralEditIntent::SetReturnOccupancy { .. }
            | StructuralEditIntent::AppendPatch { .. } => {
                unreachable!("occupancy intents were submitted above")
            }
        };
        let candidate_config = match candidate_result {
            Ok(config) => config,
            Err(error) => {
                self.deferred_engine_failure = Some(failure_event(
                    map_structural_capability_failure(effect.intent(), &error),
                ));
                return;
            }
        };
        let correlation = match GraphPreparationCorrelation::new_with_intent(
            effect.request_id(),
            patch_id,
            effect.intent().clone(),
            source_capability_id.clone(),
            target_capability_id.clone(),
            effect.source_graph_revision(),
            target_graph_revision,
        ) {
            Ok(correlation) => correlation,
            Err(error) => {
                self.deferred_engine_failure = Some(failure_event(map_request_failure(error)));
                return;
            }
        };
        let request = match GraphPreparationRequest::replacement_with_effects(
            correlation,
            self.state.patches(),
            candidate_config,
            self.state.generation(),
            *self.state.global(),
            self.state.mixer().clone(),
            runtime.audio_config,
            runtime.factory.registry(),
            self.state.effects(),
            self.state.bus_returns(),
        ) {
            Ok(request) => request,
            Err(error) => {
                self.deferred_engine_failure = Some(failure_event(map_request_failure(error)));
                return;
            }
        };
        if runtime.worker.try_submit(request).is_err() {
            self.deferred_engine_failure =
                Some(failure_event(EngineSelectionFailure::WorkerUnavailable));
        }
    }

    /// Advances at most one worker result and one structural handoff observation.
    ///
    /// The method never waits. Every lifecycle mutation is routed back through
    /// `dispatch_from` and the production reducer with `EventSource::Worker`.
    pub fn advance_structural(&mut self) -> Result<StructuralProgress, StructuralAdvanceError> {
        if let Some(error) = self.deferred_revision_error.take() {
            return Err(StructuralAdvanceError::Revision(error));
        }
        let mut progress = StructuralProgress::default();

        let next_sample_lifecycle = match self.state.file_browser().lifecycle() {
            SampleAssetLifecycle::Loading => Some(SampleAssetLifecycle::Validating),
            SampleAssetLifecycle::Validating => Some(SampleAssetLifecycle::Preparing),
            _ => None,
        };
        if let (Some(request_id), Some(lifecycle)) = (
            self.state.file_browser().request_id(),
            next_sample_lifecycle,
        ) {
            match self.dispatch_from(
                AppEvent::SampleAssetLifecycleAdvanced {
                    request_id,
                    lifecycle,
                },
                EventSource::Worker,
            ) {
                Ok(_) => progress.sample_asset_lifecycle_advanced = Some(lifecycle),
                Err(rejection) => progress.rejected_worker_event = Some(rejection),
            }
            return Ok(progress);
        }

        let engine_lifecycle = self.state.engine_selection().kind();
        let next_engine_lifecycle = match engine_lifecycle {
            EngineSelectionStatusKind::Loading => Some(EngineSelectionStatusKind::Validating),
            EngineSelectionStatusKind::Validating => Some(EngineSelectionStatusKind::Preparing),
            _ => None,
        };
        if let Some(lifecycle) = next_engine_lifecycle {
            let request_id = self
                .state
                .engine_selection()
                .correlation()
                .ok_or(StructuralAdvanceError::Status(
                    EngineSelectionStatusError::MissingCorrelation,
                ))?
                .request_id();
            match self.dispatch_from(
                AppEvent::EngineSelectionLifecycleAdvanced {
                    request_id,
                    lifecycle,
                },
                EventSource::Worker,
            ) {
                Ok(_) => {
                    progress.engine_selection_lifecycle_advanced = Some(lifecycle);
                    if lifecycle == EngineSelectionStatusKind::Preparing {
                        let effect = self.pending_engine_selection_request.take().ok_or(
                            StructuralAdvanceError::Status(
                                EngineSelectionStatusError::MissingCorrelation,
                            ),
                        )?;
                        self.submit_engine_selection_request(&effect);
                    }
                }
                Err(rejection) => progress.rejected_worker_event = Some(rejection),
            }
            return Ok(progress);
        }

        if let Some(event) = self.deferred_engine_failure.take() {
            match self.dispatch_from(event, EventSource::Worker) {
                Ok(_) => progress.failure_dispatched = true,
                Err(rejection) => progress.rejected_worker_event = Some(rejection),
            }
        }

        let worker_result = self
            .engine_selection_runtime
            .as_mut()
            .and_then(|runtime| runtime.worker.try_poll());
        if let Some(result) = worker_result {
            progress.worker_result_polled = true;
            self.handle_graph_preparation_result(result, &mut progress)?;
        }

        let coordinator_progress = self
            .engine_selection_runtime
            .as_mut()
            .map(|runtime| runtime.coordinator.poll());
        let Some(coordinator_progress) = coordinator_progress else {
            return Ok(progress);
        };
        progress.collected_count = coordinator_progress.collected_count();

        if let Some(failure) = coordinator_progress.publication_failure() {
            if let Some(runtime) = self.engine_selection_runtime.as_mut() {
                runtime.pending_session_replacement = None;
                runtime.activation_record_sequence = None;
            }
            return Err(StructuralAdvanceError::Publication(failure));
        }

        let session_replacement_pending = self.session_replacement_pending();
        if let Some(revision) = coordinator_progress.published_revision() {
            if session_replacement_pending {
                progress.graph_published = Some(revision);
            } else {
                let effect = self.structural_effect(EngineSelectionEffectKind::GraphPublished)?;
                let sequence = self
                    .engine_selection_runtime
                    .as_ref()
                    .and_then(|runtime| runtime.activation_record_sequence)
                    .filter(|(request_id, _)| *request_id == effect.request_id())
                    .map(|(_, sequence)| sequence)
                    .ok_or(StructuralAdvanceError::Status(
                        EngineSelectionStatusError::MissingCorrelation,
                    ))?;
                self.event_log
                    .append_engine_selection_effect(sequence, effect)
                    .map_err(StructuralAdvanceError::EventLog)?;
                progress.graph_published = Some(revision);
            }
        }

        if let Some(target_graph_revision) = coordinator_progress.completed_revision() {
            let session_replacement = self
                .engine_selection_runtime
                .as_ref()
                .and_then(|runtime| runtime.pending_session_replacement.as_ref())
                .cloned();
            if let Some(replacement) = session_replacement {
                if replacement.target_graph_revision() != target_graph_revision {
                    return Err(StructuralAdvanceError::SessionRevisionMismatch);
                }
                let prior_projector = self.projector;
                self.projector = StateProjector::for_graph(target_graph_revision);
                match self.dispatch_from(
                    AppEvent::ReplacePersistedSession(Box::new(replacement)),
                    EventSource::Worker,
                ) {
                    Ok(_) => {
                        self.engine_selection_runtime
                            .as_mut()
                            .expect("a pending session retains its structural runtime")
                            .pending_session_replacement = None;
                        progress.activation_acknowledged = Some(target_graph_revision);
                        progress.session_replacement_committed = true;
                    }
                    Err(rejection) => {
                        self.projector = prior_projector;
                        return Err(StructuralAdvanceError::SessionCommit(rejection));
                    }
                }
                return Ok(progress);
            }
            let correlation = self.state.engine_selection().correlation().cloned().ok_or(
                StructuralAdvanceError::Status(EngineSelectionStatusError::MissingCorrelation),
            )?;
            if correlation.target_graph_revision() != Some(target_graph_revision) {
                return Err(StructuralAdvanceError::Status(
                    EngineSelectionStatusError::InvalidTransition,
                ));
            }
            let event = AppEvent::EngineActivationAcknowledged {
                request_id: correlation.request_id(),
                intent: correlation.intent().clone(),
                target_graph_revision,
                retired_graph_revision: correlation.source_graph_revision(),
                collected: true,
            };
            match self.dispatch_from(event, EventSource::Worker) {
                Ok(_) => {
                    progress.activation_acknowledged = Some(target_graph_revision);
                    if let Some(runtime) = self.engine_selection_runtime.as_mut() {
                        runtime.activation_record_sequence = None;
                    }
                }
                Err(rejection) => progress.rejected_worker_event = Some(rejection),
            }
        }

        Ok(progress)
    }

    fn handle_graph_preparation_result(
        &mut self,
        result: GraphPreparationResult,
        progress: &mut StructuralProgress,
    ) -> Result<(), StructuralAdvanceError> {
        match result {
            GraphPreparationResult::Failed {
                correlation,
                failure,
            } => {
                let event = if correlation.intent().uses_topology_events() {
                    AppEvent::TopologyPreparationFailed {
                        request_id: correlation.request_id(),
                        intent: correlation.intent().clone(),
                        source_graph_revision: correlation.source_graph_revision(),
                        target_graph_revision: correlation.target_graph_revision(),
                        failure,
                    }
                } else {
                    AppEvent::EnginePreparationFailed {
                        request_id: correlation.request_id(),
                        patch_id: correlation
                            .patch_id()
                            .ok_or(StructuralAdvanceError::Status(
                                EngineSelectionStatusError::MissingCorrelation,
                            ))?,
                        intent: correlation.intent().clone(),
                        source_capability_id: correlation
                            .source_capability_id()
                            .ok_or(StructuralAdvanceError::Status(
                                EngineSelectionStatusError::MissingCorrelation,
                            ))?
                            .clone(),
                        target_capability_id: correlation
                            .target_capability_id()
                            .ok_or(StructuralAdvanceError::Status(
                                EngineSelectionStatusError::MissingCorrelation,
                            ))?
                            .clone(),
                        source_graph_revision: correlation.source_graph_revision(),
                        target_graph_revision: correlation.target_graph_revision(),
                        failure,
                    }
                };
                match self.dispatch_from(event, EventSource::Worker) {
                    Ok(_) => progress.failure_dispatched = true,
                    Err(rejection) => progress.rejected_worker_event = Some(rejection),
                }
            }
            GraphPreparationResult::Prepared {
                correlation,
                candidate_config,
                prepared_visualization,
                mut prepared_graph,
            } => {
                let pending_candidate_config = candidate_config.clone();
                let event = if correlation.intent().uses_topology_events() {
                    AppEvent::TopologyPrepared {
                        prepared_visualization,
                        request_id: correlation.request_id(),
                        intent: correlation.intent().clone(),
                        source_graph_revision: correlation.source_graph_revision(),
                        target_graph_revision: correlation.target_graph_revision(),
                    }
                } else {
                    AppEvent::EnginePrepared {
                        request_id: correlation.request_id(),
                        patch_id: correlation
                            .patch_id()
                            .ok_or(StructuralAdvanceError::Status(
                                EngineSelectionStatusError::MissingCorrelation,
                            ))?,
                        intent: correlation.intent().clone(),
                        source_capability_id: correlation
                            .source_capability_id()
                            .ok_or(StructuralAdvanceError::Status(
                                EngineSelectionStatusError::MissingCorrelation,
                            ))?
                            .clone(),
                        target_capability_id: correlation
                            .target_capability_id()
                            .ok_or(StructuralAdvanceError::Status(
                                EngineSelectionStatusError::MissingCorrelation,
                            ))?
                            .clone(),
                        source_graph_revision: correlation.source_graph_revision(),
                        target_graph_revision: correlation.target_graph_revision(),
                        candidate_config: candidate_config.ok_or(
                            StructuralAdvanceError::Status(
                                EngineSelectionStatusError::MissingCorrelation,
                            ),
                        )?,
                        prepared_visualization,
                    }
                };
                let preflight_parameters = if matches!(
                    correlation.intent(),
                    StructuralEditIntent::AppendPatch { .. }
                ) {
                    Some(self.preflight_append_commit(&correlation, &mut prepared_graph)?)
                } else {
                    None
                };
                let record_sequence = self.event_log.next_sequence();
                if let Err(rejection) = self.dispatch_from(event, EventSource::Worker) {
                    progress.rejected_worker_event = Some(rejection);
                    return Ok(());
                }

                let scope = replacement_scope(&correlation).ok_or(
                    StructuralAdvanceError::Status(EngineSelectionStatusError::MissingCorrelation),
                )?;
                let candidate_parameters = match preflight_parameters {
                    Some(parameters) => parameters,
                    None => self.latest_candidate_parameters(
                        &correlation,
                        pending_candidate_config.as_ref(),
                    )?,
                };
                prepared_graph
                    .refresh_initial_parameters(candidate_parameters)
                    .map_err(StructuralAdvanceError::Refresh)?;
                let outcome = self
                    .engine_selection_runtime
                    .as_mut()
                    .expect("a polled worker result retains its configured runtime")
                    .coordinator
                    .stage_replacement(prepared_graph, scope)
                    .map_err(|error| {
                        let failure = error.reason();
                        drop(error.into_graph());
                        StructuralAdvanceError::Publication(failure)
                    })?;
                let effect_kind = match outcome {
                    GraphStageOutcome::Published => EngineSelectionEffectKind::GraphPublished,
                    GraphStageOutcome::Staged => EngineSelectionEffectKind::GraphStaged,
                };
                let effect = self.structural_effect(effect_kind)?;
                self.event_log
                    .append_engine_selection_effect(record_sequence, effect)
                    .map_err(StructuralAdvanceError::EventLog)?;
                let runtime = self
                    .engine_selection_runtime
                    .as_mut()
                    .expect("a staged graph retains its configured runtime");
                runtime.activation_record_sequence =
                    Some((correlation.request_id(), record_sequence));
                progress.graph_stage = Some(outcome);
                if outcome == GraphStageOutcome::Published {
                    progress.graph_published = Some(correlation.target_graph_revision());
                }
            }
        }
        Ok(())
    }

    /// Proves the exact append commit before the candidate graph can cross the
    /// structural boundary. The clone runs the same prepared and activation
    /// events as production, then the future projection is checked against
    /// both the worker snapshot and the graph-owned fixed layout.
    fn preflight_append_commit(
        &self,
        correlation: &GraphPreparationCorrelation,
        prepared_graph: &mut PreparedGraph,
    ) -> Result<ParameterSnapshot, StructuralAdvanceError> {
        let StructuralEditIntent::AppendPatch { patch_id } = correlation.intent() else {
            return Err(StructuralAdvanceError::Status(
                EngineSelectionStatusError::IntentMismatch,
            ));
        };
        let mut future = self.state.clone();
        future
            .apply(AppEvent::TopologyPrepared {
                prepared_visualization: None,
                request_id: correlation.request_id(),
                intent: correlation.intent().clone(),
                source_graph_revision: correlation.source_graph_revision(),
                target_graph_revision: correlation.target_graph_revision(),
            })
            .map_err(StructuralAdvanceError::CandidatePreflight)?;
        future
            .apply(AppEvent::EngineActivationAcknowledged {
                request_id: correlation.request_id(),
                intent: correlation.intent().clone(),
                target_graph_revision: correlation.target_graph_revision(),
                retired_graph_revision: correlation.source_graph_revision(),
                collected: true,
            })
            .map_err(StructuralAdvanceError::CandidatePreflight)?;
        let projected = StateProjector::for_graph(correlation.target_graph_revision())
            .project(&future)
            .map_err(|error| match error {
                StateProjectionError::ParameterSnapshot(error) => {
                    StructuralAdvanceError::CandidateParameters(error)
                }
                _ => StructuralAdvanceError::CandidateParameterMismatch,
            })?
            .2;
        if !prepared_append_snapshot_matches(
            prepared_graph.initial_parameters(),
            &projected,
            *patch_id,
        ) {
            return Err(StructuralAdvanceError::CandidateParameterMismatch);
        }
        prepared_graph
            .refresh_initial_parameters(projected.clone())
            .map_err(StructuralAdvanceError::Refresh)?;
        Ok(projected)
    }

    fn latest_candidate_parameters(
        &self,
        correlation: &GraphPreparationCorrelation,
        candidate_config: Option<&crate::synth::InstrumentConfig>,
    ) -> Result<ParameterSnapshot, StructuralAdvanceError> {
        let mut patches = self.state.patches().to_vec();
        let mut returns = self.state.bus_returns().clone();
        match correlation.intent() {
            StructuralEditIntent::ReplaceCapability { .. }
            | StructuralEditIntent::ReplaceParameterChoice { .. }
            | StructuralEditIntent::ReplaceAsset { .. } => {
                let patch_id = correlation
                    .patch_id()
                    .ok_or(StructuralAdvanceError::Status(
                        EngineSelectionStatusError::MissingCorrelation,
                    ))?;
                let candidate = candidate_config.ok_or(StructuralAdvanceError::Status(
                    EngineSelectionStatusError::MissingCorrelation,
                ))?;
                patches
                    .iter_mut()
                    .find(|patch| patch.id() == patch_id)
                    .ok_or(StructuralAdvanceError::Status(
                        EngineSelectionStatusError::MissingCorrelation,
                    ))?
                    .set_instrument_config(candidate.clone());
            }
            StructuralEditIntent::PrepareAudition { .. } => {}
            StructuralEditIntent::SetVoiceBudget { patch_id, voices } => {
                patches
                    .iter_mut()
                    .find(|p| p.id() == *patch_id)
                    .ok_or(StructuralAdvanceError::Status(
                        EngineSelectionStatusError::MissingCorrelation,
                    ))?
                    .set_voice_limit(*voices)
                    .map_err(|_| {
                        StructuralAdvanceError::Status(
                            EngineSelectionStatusError::MissingCorrelation,
                        )
                    })?;
            }
            StructuralEditIntent::ReplaceEffectAsset {
                target,
                parameter_id,
                reference,
            } => {
                target
                    .assign(
                        &mut patches,
                        &mut returns,
                        self.state.effects(),
                        parameter_id,
                        reference.clone(),
                    )
                    .map_err(|_| {
                        StructuralAdvanceError::CandidateParameters(
                            ParameterSnapshotError::InvalidEffectConfig { index: 0 },
                        )
                    })?;
            }
            StructuralEditIntent::SetSlotOccupancy {
                patch_id,
                slot,
                entry,
            } => {
                let occupant = entry
                    .as_ref()
                    .map(|entry| {
                        self.state
                            .effects()
                            .descriptor(entry)
                            .ok_or(ParameterSnapshotError::InvalidEffectConfig { index: 0 })?
                            .default_config(slot.instance_identity())
                            .map_err(|_| ParameterSnapshotError::InvalidEffectConfig { index: 0 })
                    })
                    .transpose()
                    .map_err(StructuralAdvanceError::CandidateParameters)?;
                patches
                    .iter_mut()
                    .find(|patch| patch.id() == *patch_id)
                    .ok_or(StructuralAdvanceError::Status(
                        EngineSelectionStatusError::MissingCorrelation,
                    ))?
                    .set_slot_occupancy(*slot, occupant)
                    .map_err(|_| {
                        StructuralAdvanceError::CandidateParameters(
                            ParameterSnapshotError::InvalidEffectConfig { index: 0 },
                        )
                    })?;
            }
            StructuralEditIntent::SetSendEffect {
                bus,
                slot_id,
                entry,
            } => {
                returns
                    .set_effect_slot(self.state.effects(), *bus, *slot_id, entry.as_ref())
                    .map_err(|_| {
                        StructuralAdvanceError::CandidateParameters(
                            ParameterSnapshotError::InvalidEffectConfig { index: 0 },
                        )
                    })?;
            }
            StructuralEditIntent::SetReturnOccupancy { bus, entry } => {
                returns
                    .set_return_occupancy(self.state.effects(), *bus, entry.as_ref())
                    .map_err(|_| {
                        StructuralAdvanceError::CandidateParameters(
                            ParameterSnapshotError::InvalidEffectConfig { index: 0 },
                        )
                    })?;
            }
            StructuralEditIntent::AppendPatch { patch_id } => {
                let candidate =
                    self.state
                        .pending_patch_creation()
                        .ok_or(StructuralAdvanceError::Status(
                            EngineSelectionStatusError::MissingCorrelation,
                        ))?;
                if candidate.id() != *patch_id
                    || patches.iter().any(|patch| patch.id() == *patch_id)
                {
                    return Err(StructuralAdvanceError::Status(
                        EngineSelectionStatusError::IntentMismatch,
                    ));
                }
                patches.push(candidate.clone());
            }
        }
        ParameterSnapshot::project_patches_with_effects_and_returns(
            self.state.generation(),
            correlation.target_graph_revision(),
            *self.state.global(),
            self.state.mixer().clone(),
            &patches,
            self.state.capabilities(),
            self.state.effects(),
            &returns,
        )
        .map_err(StructuralAdvanceError::CandidateParameters)
    }

    fn latest_pending_candidate_parameters(
        &self,
    ) -> Result<ParameterSnapshot, StructuralAdvanceError> {
        let correlation =
            self.state
                .engine_selection()
                .correlation()
                .ok_or(StructuralAdvanceError::Status(
                    EngineSelectionStatusError::MissingCorrelation,
                ))?;
        let target_graph_revision =
            correlation
                .target_graph_revision()
                .ok_or(StructuralAdvanceError::Status(
                    EngineSelectionStatusError::MissingCorrelation,
                ))?;
        let preparation_correlation = if correlation.intent().is_occupancy() {
            GraphPreparationCorrelation::for_occupancy(
                correlation.request_id(),
                correlation.intent().clone(),
                correlation.source_graph_revision(),
                target_graph_revision,
            )
        } else if let StructuralEditIntent::AppendPatch { patch_id } = correlation.intent() {
            GraphPreparationCorrelation::for_append(
                correlation.request_id(),
                *patch_id,
                correlation.source_graph_revision(),
                target_graph_revision,
            )
        } else {
            GraphPreparationCorrelation::new_with_intent(
                correlation.request_id(),
                correlation
                    .patch_id()
                    .ok_or(StructuralAdvanceError::Status(
                        EngineSelectionStatusError::MissingCorrelation,
                    ))?,
                correlation.intent().clone(),
                correlation
                    .source_capability_id()
                    .ok_or(StructuralAdvanceError::Status(
                        EngineSelectionStatusError::MissingCorrelation,
                    ))?
                    .clone(),
                correlation
                    .target_capability_id()
                    .ok_or(StructuralAdvanceError::Status(
                        EngineSelectionStatusError::MissingCorrelation,
                    ))?
                    .clone(),
                correlation.source_graph_revision(),
                target_graph_revision,
            )
        }
        .map_err(|_| {
            StructuralAdvanceError::Status(EngineSelectionStatusError::MissingCorrelation)
        })?;
        self.latest_candidate_parameters(
            &preparation_correlation,
            self.state.pending_instrument_config(),
        )
    }

    fn structural_effect(
        &self,
        kind: EngineSelectionEffectKind,
    ) -> Result<EngineSelectionEffect, StructuralAdvanceError> {
        let correlation =
            self.state
                .engine_selection()
                .correlation()
                .ok_or(StructuralAdvanceError::Status(
                    EngineSelectionStatusError::MissingCorrelation,
                ))?;
        EngineSelectionEffect::from_correlation(kind, correlation)
            .map_err(StructuralAdvanceError::Status)
    }

    /// Joins and drains worker ownership on the calling control thread.
    pub fn shutdown_engine_selection_on_control(&mut self) -> Result<(), StructuralAdvanceError> {
        if let Some(runtime) = self.engine_selection_runtime.as_mut() {
            let _ = runtime.coordinator.poll();
            runtime
                .worker
                .shutdown_on_control()
                .map_err(StructuralAdvanceError::WorkerShutdown)?;
        }
        Ok(())
    }

    /// Counts structural graph obligations still retained by control-side
    /// orchestration. A completed teardown must report zero.
    pub fn owned_structural_graphs_on_control(&self) -> usize {
        self.engine_selection_runtime.as_ref().map_or(0, |runtime| {
            usize::from(runtime.coordinator.staged_revision().is_some())
                + usize::from(runtime.coordinator.in_flight_revision().is_some())
                + usize::from(runtime.pending_session_replacement.is_some())
        })
    }

    pub const fn engine_selection_configured(&self) -> bool {
        self.engine_selection_runtime.is_some()
    }

    /// Returns the newest complete immutable text projection.
    pub fn current_text(&self) -> TextProjection {
        self.current_text.clone()
    }

    /// Returns the newest immutable graphical projection consumed by the
    /// production window.
    pub fn current_graphical_shell(&self) -> GraphicalShellProjection {
        self.current_graphical_shell.clone()
    }

    /// Returns the canonical semantic model embedded in the newest shell.
    pub fn current_semantic_model(&self) -> crate::control::SemanticGraphicalViewModel {
        self.current_graphical_shell.semantic_model().clone()
    }

    /// Returns the newest host-neutral PATCH page exactly when PATCH is active.
    pub fn current_patch_page(&self) -> Option<PatchPageProjection> {
        self.current_patch_page.clone()
    }

    /// Returns the newest canonical state and projection tree.
    pub fn current_state_tree(&self) -> StateTree {
        self.current_state_tree.clone()
    }

    /// Returns the immutable capability metadata installed in canonical state.
    pub fn capabilities(&self) -> &CapabilityRegistry {
        self.state.capabilities()
    }

    /// Immutable file-work correlation used by asset adapters and observers.
    pub const fn file_browser(&self) -> &crate::control::FileBrowserState {
        self.state.file_browser()
    }

    pub fn effects(&self) -> &crate::synth::EffectCapabilityRegistry {
        self.state.effects()
    }

    /// Returns the immutable accepted Patch set used to prepare audio graphs.
    /// Returns the canonical eight-return bank owned by accepted state.
    pub fn bus_returns(&self) -> &crate::mixer::bus_return::BusReturnBank {
        self.state.bus_returns()
    }

    pub fn patches(&self) -> &[crate::synth::patch::Patch] {
        self.state.patches()
    }

    /// Captures only canonical persisted content for shell-owned document and
    /// save workflows.
    pub fn capture_saved_session(&self) -> crate::control::SavedSession {
        crate::control::SavedSession::capture(&self.state)
    }

    /// Returns the latest complete scalar projection published to audio.
    pub const fn current_parameters(
        &self,
    ) -> &crate::real_time::parameter_snapshot::ParameterSnapshot {
        &self.current_parameters
    }

    /// Returns the prepared graph revision targeted by every runtime projection.
    pub const fn graph_revision(&self) -> crate::real_time::GraphRevision {
        self.current_parameters.graph_revision()
    }

    /// Borrows the canonical one-in-flight structural lifecycle.
    pub const fn engine_selection_status(&self) -> &crate::control::EngineSelectionStatus {
        self.state.engine_selection()
    }

    /// Returns the callback-published structural status when engine selection
    /// is configured. Live observers use this read-only view; they never own
    /// or advance the coordinator.
    pub fn engine_graph_handoff_status(&self) -> Option<crate::real_time::GraphHandoffStatus> {
        self.engine_selection_runtime
            .as_ref()
            .map(|runtime| runtime.coordinator.status())
    }

    pub fn staged_graph_revision(&self) -> Option<crate::real_time::GraphRevision> {
        self.engine_selection_runtime
            .as_ref()
            .and_then(|runtime| runtime.coordinator.staged_revision())
    }

    pub fn in_flight_graph_revision(&self) -> Option<crate::real_time::GraphRevision> {
        self.engine_selection_runtime
            .as_ref()
            .and_then(|runtime| runtime.coordinator.in_flight_revision())
    }

    /// Whether a prepared whole-session replacement owns the MIDI/edit gate.
    pub fn session_replacement_pending(&self) -> bool {
        self.engine_selection_runtime
            .as_ref()
            .is_some_and(|runtime| runtime.pending_session_replacement.is_some())
    }

    pub(crate) const fn state(&self) -> &AppState {
        &self.state
    }

    /// Returns an immutable snapshot of the bounded control event journal.
    pub fn event_log(&self) -> EventLog {
        self.event_log.clone()
    }

    /// Borrows the immutable journal for control-side verification without
    /// cloning retained history.
    pub const fn event_log_ref(&self) -> &EventLog {
        &self.event_log
    }

    /// Enqueues a bounded system-recovery command without inventing a state
    /// transition. Deterministic verification uses this for the renderer-wide
    /// all-notes-off command, which is distinct from Patch-scoped MIDI.
    pub(crate) fn push_recovery_command(
        &mut self,
        command: crate::real_time::audio_command::AudioCommand,
    ) -> Result<(), BoundaryFull> {
        debug_assert_eq!(
            command,
            crate::real_time::audio_command::AudioCommand::AllNotesOff
        );
        self.boundary.push_recovery_command()
    }
}

fn submit_midi_worker_command(
    runtime: &mut MidiDeviceRuntime,
    command: MidiDeviceWorkerCommand,
) -> bool {
    match runtime.worker.try_submit(command) {
        Ok(()) => true,
        Err(busy) => {
            runtime.deferred_command = Some(busy.into_command());
            false
        }
    }
}

fn disable_revision(
    runtime: &mut MidiDeviceRuntime,
    revision: crate::control::MidiConnectionRevision,
) {
    if let Some(active) = runtime
        .active
        .as_mut()
        .filter(|active| active.request.revision() == revision)
    {
        active.ingress.disable();
        active.ingress.discard_all();
    }
    if let Some(candidate) = runtime
        .candidate
        .as_mut()
        .filter(|candidate| candidate.request.revision() == revision)
    {
        candidate.ingress.disable();
        candidate.ingress.discard_all();
    }
    for retiring in runtime
        .retiring
        .iter_mut()
        .filter(|retiring| retiring.request.revision() == revision)
    {
        retiring.ingress.disable();
        retiring.ingress.discard_all();
    }
}

fn take_midi_connection(
    runtime: &mut MidiDeviceRuntime,
    revision: crate::control::MidiConnectionRevision,
) -> Option<OwnedMidiConnection> {
    if runtime
        .active
        .as_ref()
        .is_some_and(|active| active.request.revision() == revision)
    {
        return runtime.active.take();
    }
    if runtime
        .candidate
        .as_ref()
        .is_some_and(|candidate| candidate.request.revision() == revision)
    {
        return runtime.candidate.take();
    }
    let index = runtime
        .retiring
        .iter()
        .position(|retiring| retiring.request.revision() == revision)?;
    Some(runtime.retiring.remove(index))
}

fn retire_midi_connection(
    runtime: &mut MidiDeviceRuntime,
    mut connection: OwnedMidiConnection,
) -> bool {
    connection.ingress.disable();
    connection.ingress.discard_all();
    submit_midi_worker_command(
        runtime,
        MidiDeviceWorkerCommand::Retire {
            active: connection.active,
        },
    )
}

fn publish_midi_activity(runtime: &mut MidiDeviceRuntime, now_micros: u64) {
    let Some(active) = runtime.active.as_ref() else {
        return;
    };
    if runtime
        .last_activity_publish_micros
        .is_some_and(|last| now_micros.saturating_sub(last) < MIDI_ACTIVITY_PUBLISH_INTERVAL_MICROS)
    {
        return;
    }
    runtime.latest_activity = Some(MidiActivitySnapshot::new(
        active.request.revision(),
        runtime.accepted_count,
        runtime.last_event,
        runtime.last_control_receipt_micros,
        active.ingress.diagnostics(),
        active.ingress.overflow_epoch(),
    ));
    runtime.last_activity_publish_micros = Some(now_micros);
}

/// Compares the worker's immutable append snapshot with the future reducer
/// commit. Existing scalar values may legitimately have advanced while the
/// worker prepared, but their identities and fixed scalar shapes may not; the
/// appended candidate itself is immutable and therefore must match exactly.
fn prepared_append_snapshot_matches(
    prepared: &ParameterSnapshot,
    future: &ParameterSnapshot,
    candidate_id: PatchId,
) -> bool {
    if prepared.graph_revision() != future.graph_revision()
        || prepared.patch_count() != future.patch_count()
        || prepared.patch_count() == 0
    {
        return false;
    }
    for (prepared_patch, future_patch) in prepared.patches().iter().zip(future.patches()) {
        if prepared_patch.patch_id() != future_patch.patch_id()
            || prepared_patch.instrument().count() != future_patch.instrument().count()
            || prepared_patch
                .effects()
                .iter()
                .zip(future_patch.effects())
                .any(|(prepared_effect, future_effect)| {
                    prepared_effect.slot_id() != future_effect.slot_id()
                        || prepared_effect.scalar_count() != future_effect.scalar_count()
                })
        {
            return false;
        }
    }
    let Some(prepared_candidate) = prepared.patches().last() else {
        return false;
    };
    let Some(future_candidate) = future.patches().last() else {
        return false;
    };
    if prepared_candidate.patch_id() != Some(candidate_id)
        || future_candidate.patch_id() != Some(candidate_id)
        || prepared_candidate != future_candidate
    {
        return false;
    }
    prepared
        .returns()
        .iter()
        .zip(future.returns())
        .all(|(prepared_return, future_return)| {
            prepared_return.slot_id() == future_return.slot_id()
                && prepared_return.scalar_count() == future_return.scalar_count()
        })
}

/// Derives the layout-admission scope for one correlated replacement.
///
/// Delegated to the correlation's canonical derivation so publication
/// admission and the renderer's voice carry-over share one scope vocabulary.
fn replacement_scope(
    correlation: &crate::real_time::GraphPreparationCorrelation,
) -> Option<crate::real_time::GraphReplacementScope> {
    correlation.replacement_scope()
}

fn structural_preparation_failed_event(
    effect: &EngineSelectionEffect,
    target_graph_revision: GraphRevision,
    failure: EngineSelectionFailure,
) -> AppEvent {
    if effect.intent().uses_topology_events() {
        return AppEvent::TopologyPreparationFailed {
            request_id: effect.request_id(),
            intent: effect.intent().clone(),
            source_graph_revision: effect.source_graph_revision(),
            target_graph_revision,
            failure,
        };
    }
    AppEvent::EnginePreparationFailed {
        request_id: effect.request_id(),
        patch_id: effect
            .patch_id()
            .expect("instrument intents carry their Patch identity"),
        intent: effect.intent().clone(),
        source_capability_id: effect
            .source_capability_id()
            .expect("instrument intents carry their source capability")
            .clone(),
        target_capability_id: effect
            .target_capability_id()
            .expect("instrument intents carry their target capability")
            .clone(),
        source_graph_revision: effect.source_graph_revision(),
        target_graph_revision,
        failure,
    }
}

fn map_structural_capability_failure(
    intent: &StructuralEditIntent,
    error: &CapabilityError,
) -> EngineSelectionFailure {
    if matches!(intent, StructuralEditIntent::ReplaceParameterChoice { .. })
        && matches!(
            error,
            CapabilityError::UnknownChoice(_)
                | CapabilityError::MissingParameter(_)
                | CapabilityError::UndeclaredParameter(_)
                | CapabilityError::WrongValueKind(_)
                | CapabilityError::StructuralParameter(_)
        )
    {
        EngineSelectionFailure::PresetUnavailable
    } else if matches!(intent, StructuralEditIntent::ReplaceAsset { .. }) {
        EngineSelectionFailure::InvalidAsset
    } else {
        map_capability_failure(error)
    }
}

fn map_capability_failure(error: &CapabilityError) -> EngineSelectionFailure {
    match error {
        CapabilityError::UnknownCapability(_) => EngineSelectionFailure::UnknownCapability,
        CapabilityError::MissingParameter(_) | CapabilityError::MissingAsset(_) => {
            EngineSelectionFailure::MissingDefault
        }
        CapabilityError::ProviderRegistryMismatch(_) => EngineSelectionFailure::ProviderMismatch,
        _ => EngineSelectionFailure::InvalidDefaultConfig,
    }
}

fn map_request_failure(error: GraphPreparationRequestError) -> EngineSelectionFailure {
    match error {
        GraphPreparationRequestError::SourceCapabilityMismatch
        | GraphPreparationRequestError::TargetCapabilityMismatch => {
            EngineSelectionFailure::ProviderMismatch
        }
        GraphPreparationRequestError::InvalidActiveConfig
        | GraphPreparationRequestError::InvalidActiveEffectConfig
        | GraphPreparationRequestError::InvalidCandidateConfig
        | GraphPreparationRequestError::InvalidOccupancy => {
            EngineSelectionFailure::InvalidDefaultConfig
        }
        GraphPreparationRequestError::UnknownEffectEntry => {
            EngineSelectionFailure::UnknownCapability
        }
        GraphPreparationRequestError::MissingRequestIdentity
        | GraphPreparationRequestError::CapabilityUnchanged
        | GraphPreparationRequestError::IntentMismatch
        | GraphPreparationRequestError::ConfigDeltaMismatch
        | GraphPreparationRequestError::TargetRevisionNotNewer
        | GraphPreparationRequestError::PatchCapacityExceeded
        | GraphPreparationRequestError::DuplicatePatchId
        | GraphPreparationRequestError::UnknownPatch => EngineSelectionFailure::GraphIncompatible,
    }
}

#[cfg(test)]
mod tests {
    use super::{prepared_append_snapshot_matches, AppLoop, DispatchResult};
    use crate::adapter::braids_capability::{BraidsCapability, BRAIDS_CAPABILITY_ID};
    use crate::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID;
    use crate::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
    use crate::adapter::lock_free_structural_graph_boundary::{
        LockFreeStructuralControlHandle, LockFreeStructuralGraphBoundary,
    };
    use crate::adapter::production_instruments::{
        production_capability_registry, production_instrument_preparers,
        production_instrument_providers,
    };
    use crate::control::app_event::{AppEvent, Direction};
    use crate::control::app_state::{AppState, EventRejection};
    use crate::control::engine_selection::{
        EngineSelectionEffectKind, EngineSelectionFailure, EngineSelectionStatusKind,
    };
    use crate::control::event_record::{EmittedEvent, EventOutcome, EventSource};
    use crate::control::state_projector::StateProjector;
    use crate::control::{
        ActiveMidiInput, MidiDeviceWorker, MidiDeviceWorkerBusy, MidiDeviceWorkerBusyReason,
        MidiDeviceWorkerCommand, MidiDeviceWorkerResult, MidiInputDescriptor, MidiInputDeviceId,
        PatchControlId, PhysicalMidiIngress, PhysicalMidiIngressOutcome, SavedSession,
        SemanticAction, SemanticSurfaceSummary, SurfaceId, TopLevelContext,
    };
    use crate::kernel::midi_channel::MidiChannel;
    use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
    use crate::kernel::patch_id::PatchId;
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_state::MixerState;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::mixer_track_parameters::{MixerTrackParameter, MixerTrackParameters};
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::audio_boundary::{BoundaryFull, ControlAudioBoundary};
    use crate::real_time::audio_command::AudioCommand;
    use crate::real_time::audio_renderer::AudioRenderer;
    use crate::real_time::parameter_snapshot::{
        ParameterSnapshot, RtInstrumentParameters, RtPatchParameters,
    };
    use crate::real_time::{
        AudioBoundary, ControlStructuralGraphBoundary, GraphHandoffStatus, GraphPublicationFailure,
        GraphRevision, GraphStageOutcome, NoStructuralGraphChanges, PreparedGraph,
        PreparedGraphBuilder, StructuralBoundaryFull, StructuralGraphBoundary,
    };
    use crate::shell::audio_output::{AudioDeviceConfig, AudioSampleFormat};
    use crate::synth::patch::Patch;
    use crate::synth::sound_font_instrument::SoundFontInstrument;
    use crate::synth::{
        CapabilityId, CapabilityRegistry, DescriptorDefaultConfigFactory,
        InstrumentCapabilityProvider, InstrumentPreparationError, InstrumentPreparer,
        PreparedInstrument, PreparedInstrumentError, VoiceEnvelope, VoiceEnvelopeParameter,
    };
    use crate::testing::automatic_midi_test::create_soundfont_config;
    use crate::testing::DeterministicGraphPreparationWorker;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::ThreadId;

    #[test]
    fn append_preflight_snapshot_requires_exact_identity_shape_revision_and_candidate_values() {
        let revision = GraphRevision::new(2).unwrap();
        let source_id = PatchId::new(1).unwrap();
        let candidate_id = PatchId::new(2).unwrap();
        let source = RtPatchParameters::new(source_id, PatchOutput::default());
        let candidate = RtPatchParameters::new(candidate_id, PatchOutput::default());
        let snapshot = |revision, patches: &[RtPatchParameters]| {
            ParameterSnapshot::for_graph(
                1,
                revision,
                GlobalParameters::new(0.0).unwrap(),
                MixerState::default(),
                patches,
            )
            .unwrap()
        };
        let prepared = snapshot(revision, &[source, candidate.clone()]);
        let source_latest = RtPatchParameters::new(
            source_id,
            PatchOutput::new(MixerTrackId::default(), -1.0).unwrap(),
        );
        let future = snapshot(revision, &[source_latest.clone(), candidate.clone()]);
        assert!(prepared_append_snapshot_matches(
            &prepared,
            &future,
            candidate_id
        ));

        let wrong_revision = snapshot(
            GraphRevision::new(3).unwrap(),
            &[source_latest.clone(), candidate.clone()],
        );
        assert!(!prepared_append_snapshot_matches(
            &prepared,
            &wrong_revision,
            candidate_id
        ));
        let wrong_order = snapshot(revision, &[candidate.clone(), source_latest.clone()]);
        assert!(!prepared_append_snapshot_matches(
            &prepared,
            &wrong_order,
            candidate_id
        ));
        let wrong_shape = RtPatchParameters::projected(
            source_id,
            source_latest.output(),
            VoiceEnvelope::DEFAULT,
            RtInstrumentParameters::new(&[0.0]).unwrap(),
        );
        assert!(!prepared_append_snapshot_matches(
            &prepared,
            &snapshot(revision, &[wrong_shape, candidate]),
            candidate_id
        ));
        let wrong_candidate_value = RtPatchParameters::new(
            candidate_id,
            PatchOutput::new(MixerTrackId::default(), -1.0).unwrap(),
        );
        assert!(!prepared_append_snapshot_matches(
            &prepared,
            &snapshot(revision, &[source_latest, wrong_candidate_value]),
            candidate_id
        ));
        assert!(!prepared_append_snapshot_matches(
            &prepared,
            &future,
            PatchId::new(3).unwrap()
        ));
    }

    fn enter_detail_at<Boundary>(app_loop: &mut AppLoop<Boundary>, target: &PatchControlId)
    where
        Boundary: ControlAudioBoundary,
    {
        let surface = app_loop.state().interaction().active_surface();
        let focus = app_loop.state().interaction().focus_path().clone();
        let subject =
            crate::control::SemanticResolver::new(app_loop.state()).detail_subject(&focus);
        app_loop
            .dispatch(AppEvent::EnterSurface(SurfaceId::PatchDetail))
            .unwrap_or_else(|error| {
                panic!(
                    "the Engine root opens instrument Detail: {error:?}; surface={surface:?}, focus={focus:?}, subject={subject:?}"
                )
            });
        for _ in 0..32 {
            if app_loop
                .state()
                .interaction()
                .patch_control_focus()
                .as_ref()
                == Some(target)
            {
                return;
            }
            app_loop
                .dispatch(AppEvent::Navigate(Direction::Down))
                .expect("the descriptor-backed detail order reaches the target");
        }
        panic!("Detail did not reach {target:?}");
    }

    fn advance_engine_admission<Boundary>(app_loop: &mut AppLoop<Boundary>)
    where
        Boundary: ControlAudioBoundary,
    {
        assert_eq!(
            app_loop.engine_selection_status().kind(),
            EngineSelectionStatusKind::Loading
        );
        assert_eq!(
            app_loop
                .advance_structural()
                .unwrap()
                .engine_selection_lifecycle_advanced(),
            Some(EngineSelectionStatusKind::Validating)
        );
        assert_eq!(
            app_loop
                .advance_structural()
                .unwrap()
                .engine_selection_lifecycle_advanced(),
            Some(EngineSelectionStatusKind::Preparing)
        );
    }

    #[derive(Clone, Debug, Default, PartialEq)]
    struct BoundaryObservations {
        parameters: Vec<ParameterSnapshot>,
        commands: Vec<AudioCommand>,
        order: Vec<&'static str>,
        reject_commands: bool,
    }

    #[derive(Clone, Debug)]
    struct TestBoundary {
        observations: Arc<Mutex<BoundaryObservations>>,
    }

    impl TestBoundary {
        fn new(observations: Arc<Mutex<BoundaryObservations>>) -> Self {
            Self { observations }
        }
    }

    impl ControlAudioBoundary for TestBoundary {
        fn push_command(&mut self, command: AudioCommand) -> Result<(), BoundaryFull> {
            let mut observations = self.observations.lock().unwrap();
            observations.order.push("command");
            if observations.reject_commands {
                Err(BoundaryFull::new(command))
            } else {
                observations.commands.push(command);
                Ok(())
            }
        }

        fn publish_parameters(&mut self, parameters: ParameterSnapshot) {
            let mut observations = self.observations.lock().unwrap();
            observations.order.push("parameters");
            observations.parameters.push(parameters);
        }

        fn has_recovery_reserve(&self) -> bool {
            true
        }
    }

    #[derive(Default)]
    struct FakeMidiWorkerState {
        ingress: Option<PhysicalMidiIngress>,
        retired: usize,
        stored: usize,
    }

    #[derive(Clone)]
    struct FakeMidiWorkerHandle(Arc<Mutex<FakeMidiWorkerState>>);

    impl FakeMidiWorkerHandle {
        fn receive_raw(&self, timestamp_micros: u64, raw: &[u8]) -> PhysicalMidiIngressOutcome {
            self.0
                .lock()
                .unwrap()
                .ingress
                .as_mut()
                .expect("the fake connection is active")
                .receive_raw(timestamp_micros, raw)
        }
    }

    struct FakeMidiWorker {
        shared: Arc<Mutex<FakeMidiWorkerState>>,
        descriptor: MidiInputDescriptor,
        results: VecDeque<MidiDeviceWorkerResult>,
        shutdown: bool,
    }

    impl FakeMidiWorker {
        fn new(descriptor: MidiInputDescriptor) -> (Self, FakeMidiWorkerHandle) {
            let shared = Arc::new(Mutex::new(FakeMidiWorkerState::default()));
            (
                Self {
                    shared: Arc::clone(&shared),
                    descriptor,
                    results: VecDeque::new(),
                    shutdown: false,
                },
                FakeMidiWorkerHandle(shared),
            )
        }
    }

    impl MidiDeviceWorker for FakeMidiWorker {
        fn try_submit(
            &mut self,
            command: MidiDeviceWorkerCommand,
        ) -> Result<(), MidiDeviceWorkerBusy> {
            if self.shutdown {
                return Err(MidiDeviceWorkerBusy::new(
                    MidiDeviceWorkerBusyReason::Shutdown,
                    command,
                ));
            }
            match command {
                MidiDeviceWorkerCommand::Scan { scan_id } => {
                    self.results
                        .push_back(MidiDeviceWorkerResult::ScanSucceeded {
                            scan_id,
                            descriptors: vec![self.descriptor.clone()],
                        });
                }
                MidiDeviceWorkerCommand::Connect { request, ingress } => {
                    self.shared.lock().unwrap().ingress = Some(ingress);
                    let active = ActiveMidiInput::from_backend(&request, ());
                    self.results
                        .push_back(MidiDeviceWorkerResult::ConnectionPrepared { request, active });
                }
                MidiDeviceWorkerCommand::Retire { active } => {
                    self.shared.lock().unwrap().ingress = None;
                    let identity = active.identity().clone();
                    let request_id = active.request_id();
                    let revision = active.revision();
                    assert!(active.into_backend::<()>().is_ok());
                    self.shared.lock().unwrap().retired += 1;
                    self.results.push_back(MidiDeviceWorkerResult::Retired {
                        identity,
                        request_id,
                        revision,
                        failure: None,
                    });
                }
                MidiDeviceWorkerCommand::LoadPreference => {
                    self.results
                        .push_back(MidiDeviceWorkerResult::PreferenceLoaded {
                            preference: None,
                            failure: None,
                        });
                }
                MidiDeviceWorkerCommand::StorePreference { preference } => {
                    self.shared.lock().unwrap().stored += 1;
                    self.results
                        .push_back(MidiDeviceWorkerResult::PreferenceStored {
                            preference,
                            failure: None,
                        });
                }
            }
            Ok(())
        }

        fn try_poll(&mut self) -> Option<MidiDeviceWorkerResult> {
            self.results.pop_front()
        }

        fn shutdown_with_retirements_on_control(
            &mut self,
            retirements: Vec<ActiveMidiInput>,
        ) -> Result<(), crate::control::MidiDeviceWorkerShutdownError> {
            for active in retirements {
                assert!(active.into_backend::<()>().is_ok());
                self.shared.lock().unwrap().retired += 1;
            }
            self.shutdown = true;
            self.shared.lock().unwrap().ingress = None;
            self.results.clear();
            Ok(())
        }
    }

    struct ObservedStructuralControl {
        inner: LockFreeStructuralControlHandle,
        blocked: Arc<AtomicBool>,
        attempted_parameters: Arc<Mutex<Vec<ParameterSnapshot>>>,
        status_override: Arc<Mutex<Option<GraphHandoffStatus>>>,
    }

    impl ControlStructuralGraphBoundary for ObservedStructuralControl {
        fn publish_prepared_on_control(
            &mut self,
            graph: PreparedGraph,
        ) -> Result<(), StructuralBoundaryFull> {
            self.attempted_parameters
                .lock()
                .unwrap()
                .push(graph.initial_parameters().clone());
            if self.blocked.load(Ordering::SeqCst) {
                Err(StructuralBoundaryFull::new(graph))
            } else {
                self.inner.publish_prepared_on_control(graph)
            }
        }

        fn collect_retired_on_control(&mut self) -> Option<GraphRevision> {
            self.inner.collect_retired_on_control()
        }

        fn read_status_on_control(&self) -> GraphHandoffStatus {
            self.status_override
                .lock()
                .unwrap()
                .unwrap_or_else(|| self.inner.read_status_on_control())
        }
    }

    struct DropRecordingPreparer {
        capability_id: CapabilityId,
        drop_threads: Arc<Mutex<Vec<ThreadId>>>,
    }

    impl InstrumentPreparer for DropRecordingPreparer {
        fn capability_id(&self) -> &CapabilityId {
            &self.capability_id
        }

        fn prepare(
            &self,
            patch: &Patch,
            _sample_rate: f32,
            _max_frames: usize,
        ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
            Ok(Box::new(DropRecordingInstrument {
                patch_id: patch.id(),
                sounding: false,
                drop_threads: Arc::clone(&self.drop_threads),
            }))
        }
    }

    struct DropRecordingInstrument {
        patch_id: PatchId,
        sounding: bool,
        drop_threads: Arc<Mutex<Vec<ThreadId>>>,
    }

    impl PreparedInstrument for DropRecordingInstrument {
        fn patch_id(&self) -> PatchId {
            self.patch_id
        }

        fn dispatch(
            &mut self,
            message: MidiMessage,
            _parameters: &crate::real_time::RtPatchParameters,
        ) -> Result<(), PreparedInstrumentError> {
            self.sounding = message.kind() == MidiMessageKind::NoteOn;
            Ok(())
        }

        fn render(
            &mut self,
            output: &mut [f32],
            _frame_count: usize,
            _parameters: &crate::real_time::RtPatchParameters,
        ) -> Result<(), crate::synth::PreparedInstrumentError> {
            if self.sounding {
                output.fill(0.125);
            }

            Ok(())
        }

        fn all_notes_off(&mut self) {
            self.sounding = false;
        }
    }

    impl Drop for DropRecordingInstrument {
        fn drop(&mut self) {
            self.drop_threads
                .lock()
                .unwrap()
                .push(std::thread::current().id());
        }
    }

    fn global_parameters() -> GlobalParameters {
        GlobalParameters::new(0.0).unwrap()
    }

    fn patch(id: u32, _gain_db: f32) -> Patch {
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        Patch::new(
            PatchId::new(id).unwrap(),
            format!("Patch {id}"),
            create_soundfont_config(
                &provider,
                SoundFontInstrument::new(0, (id - 1) as u8, false).unwrap(),
            )
            .unwrap(),
            MidiChannel::new(((id - 1) % 16) as u8).unwrap(),
            PatchOutput::to_track(MixerTrackId::new(((id - 1) % 16) as u8).unwrap()),
        )
    }

    fn installed_state_with_gains(gains: &[f32]) -> AppState {
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        let mut mixer = MixerState::default();
        for (index, gain_db) in gains.iter().copied().enumerate() {
            mixer.set_track(
                MixerTrackId::new(index as u8).unwrap(),
                MixerTrackParameters::default()
                    .with_scalar_value(MixerTrackParameter::Level, gain_db)
                    .unwrap(),
            );
        }
        let mut state = AppState::new(provider.registry().unwrap(), global_parameters())
            .with_initial_mixer(mixer);
        let patches = gains
            .iter()
            .enumerate()
            .map(|(index, gain_db)| patch(index as u32 + 1, *gain_db))
            .collect();
        state.apply(AppEvent::InstallPatches(patches)).unwrap();
        state
    }

    fn installed_state() -> AppState {
        installed_state_with_gains(&[0.0])
    }

    fn loop_with_state(
        state: AppState,
    ) -> (AppLoop<TestBoundary>, Arc<Mutex<BoundaryObservations>>) {
        let observations = Arc::new(Mutex::new(BoundaryObservations::default()));
        let app_loop = AppLoop::new(
            state,
            StateProjector::new(),
            TestBoundary::new(Arc::clone(&observations)),
        )
        .unwrap();
        (app_loop, observations)
    }

    fn loop_with_observations() -> (AppLoop<TestBoundary>, Arc<Mutex<BoundaryObservations>>) {
        loop_with_state(installed_state())
    }

    #[test]
    fn one_way_control_loop_publishes_one_coherent_edit() {
        let (mut app_loop, observations) = loop_with_observations();
        let initial_text = app_loop.current_text();
        let initial_shell = app_loop.current_graphical_shell();

        let result = app_loop
            .dispatch(AppEvent::Adjust(Direction::Right))
            .unwrap();
        let current_text = app_loop.current_text();
        let current_shell = app_loop.current_graphical_shell();
        let observations = observations.lock().unwrap();
        let published = observations.parameters.last().unwrap();

        assert_eq!(result.accepted().generation(), 2);
        assert_eq!(published.generation(), result.accepted().generation());
        assert_eq!(
            published.mixer_track(MixerTrackId::default()).level_db(),
            1.0
        );
        assert!(result.snapshot().json().contains("\"levelDb\":1.0"));
        assert_eq!(current_text.state_hash(), result.snapshot().hash());
        assert_ne!(current_text, initial_text);
        assert_ne!(current_shell, initial_shell);
        assert_eq!(current_shell.generation(), result.accepted().generation());
        assert_eq!(current_shell.state_hash(), result.snapshot().hash());
        assert_eq!(current_shell.workspace().diagnostic(), &current_text);
        assert!(current_text.body().contains("> levelDb=1"));
        assert!(observations.commands.is_empty());
        assert!(result.audio_effects_published());
    }

    #[test]
    fn context_dispatch_projects_page_and_publishes_same_values_without_audio_command() {
        let (mut app_loop, observations) = loop_with_observations();
        app_loop
            .dispatch(AppEvent::Navigate(Direction::Down))
            .unwrap();
        let retained_selection = app_loop.state().selection();
        let before_parameters = app_loop.current_parameters().clone();
        let before_patches = app_loop.patches().to_vec();
        let before_global = *app_loop.state().global();
        let command_count = observations.lock().unwrap().commands.len();

        let result = app_loop
            .dispatch_from(
                AppEvent::SelectContext(crate::control::TopLevelContext::Patch),
                EventSource::Keyboard,
            )
            .unwrap();
        let page = app_loop.current_patch_page().unwrap();
        let text = app_loop.current_text();
        let shell = app_loop.current_graphical_shell();
        let tree: serde_json::Value =
            serde_json::from_str(app_loop.current_state_tree().json()).unwrap();
        let after_parameters = app_loop.current_parameters().clone();

        assert_eq!(page.patch().id(), Some(PatchId::new(1).unwrap()));
        assert_eq!(page.state_hash(), result.snapshot().hash());
        assert_eq!(text.context(), crate::control::TopLevelContext::Patch);
        assert_eq!(text.state_hash(), result.snapshot().hash());
        assert_eq!(shell.context(), crate::control::TopLevelContext::Patch);
        assert_eq!(shell.generation(), result.accepted().generation());
        assert_eq!(shell.state_hash(), result.snapshot().hash());
        assert_eq!(shell.workspace().diagnostic(), &text);
        assert_eq!(tree["interaction"]["activeFocus"]["context"], "patch");
        assert_eq!(tree["patchPage"]["stateHash"], result.snapshot().hash());
        assert_eq!(
            tree["graphicalShell"],
            serde_json::to_value(&shell).unwrap()
        );
        assert_eq!(before_patches, app_loop.patches());
        assert_eq!(before_global, *app_loop.state().global());
        assert_eq!(retained_selection, app_loop.state().selection());
        assert_eq!(
            before_parameters.graph_revision(),
            after_parameters.graph_revision()
        );
        assert_eq!(before_parameters.patches(), after_parameters.patches());
        assert_eq!(before_parameters.global(), after_parameters.global());
        assert_eq!(observations.lock().unwrap().commands.len(), command_count);

        let rejected_tree = app_loop.current_state_tree();
        let rejected_shell = app_loop.current_graphical_shell();
        assert_eq!(
            app_loop.dispatch(AppEvent::Adjust(Direction::Down)),
            Err(EventRejection::ActionUnavailableInContext)
        );
        assert_eq!(app_loop.current_state_tree(), rejected_tree);
        assert_eq!(app_loop.current_graphical_shell(), rejected_shell);
        app_loop
            .dispatch(AppEvent::SelectContext(
                crate::control::TopLevelContext::Mixer,
            ))
            .unwrap();
        assert!(app_loop.current_patch_page().is_none());
        assert_eq!(
            app_loop.current_text().context(),
            crate::control::TopLevelContext::Mixer
        );
        assert_eq!(app_loop.state().selection(), retained_selection);
    }

    #[test]
    fn ready_and_recoverable_failed_patch_adsr_edits_publish_source_revision_to_renderer() {
        let registry = production_capability_registry().unwrap();
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        let patch_id = PatchId::new(1).unwrap();
        let patch = Patch::new(
            patch_id,
            "Envelope lifecycle".to_owned(),
            create_soundfont_config(&provider, SoundFontInstrument::new(0, 8, false).unwrap())
                .unwrap(),
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        )
        .with_envelope(VoiceEnvelope::new(500.0, 600.0, 0.5, 700.0).unwrap());
        let mut state = AppState::for_graph(
            registry.clone(),
            global_parameters(),
            GraphRevision::INITIAL,
        );
        state.apply(AppEvent::InstallPatches(vec![patch])).unwrap();

        let initial_transport =
            ParameterSnapshot::new(0, global_parameters(), MixerState::default(), &[]).unwrap();
        let boundary = LockFreeAudioBoundary::new(64, initial_transport);
        let (audio_control, audio_handle) = boundary.into_handles();
        let mut app_loop = AppLoop::new(
            state,
            StateProjector::for_graph(GraphRevision::INITIAL),
            audio_control,
        )
        .unwrap();
        let graph =
            PreparedGraphBuilder::new(&registry, &production_instrument_preparers().unwrap())
                .build(
                    GraphRevision::INITIAL,
                    app_loop.patches(),
                    app_loop.current_parameters().clone(),
                    48_000.0,
                    512,
                )
                .unwrap();
        let mut renderer = AudioRenderer::new(audio_handle, NoStructuralGraphChanges::new(), graph);
        let mut output = [0.0_f32; 1_024];

        app_loop
            .dispatch(AppEvent::SelectContext(TopLevelContext::Patch))
            .unwrap();
        enter_detail_at(
            &mut app_loop,
            &PatchControlId::Envelope(VoiceEnvelopeParameter::AttackMilliseconds),
        );
        app_loop
            .dispatch(AppEvent::Adjust(Direction::Right))
            .unwrap();
        let ready_edit = app_loop.event_log_ref().records().back().unwrap();
        assert_eq!(
            ready_edit.emitted_events(),
            &[
                EmittedEvent::StateAccepted {
                    generation: ready_edit.generation_after()
                },
                EmittedEvent::ParameterSnapshotPublished {
                    generation: ready_edit.generation_after(),
                    graph_revision: GraphRevision::INITIAL
                }
            ]
        );
        assert_eq!(
            app_loop
                .current_parameters()
                .patch(patch_id)
                .unwrap()
                .envelope()
                .attack_milliseconds(),
            501.0
        );
        let note = MidiMessage::try_new(
            MidiChannel::new(0).unwrap(),
            MidiMessageKind::NoteOn,
            60,
            110,
        )
        .unwrap();
        app_loop
            .dispatch(AppEvent::Midi {
                patch_id,
                message: note,
            })
            .unwrap();
        renderer.render(&mut output);
        assert_eq!(renderer.active_revision(), GraphRevision::INITIAL);
        assert_eq!(
            renderer
                .parameters()
                .patch(patch_id)
                .unwrap()
                .envelope()
                .attack_milliseconds(),
            501.0
        );
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(output.iter().any(|sample| sample.abs() > f32::EPSILON));

        app_loop.dispatch(AppEvent::Return).unwrap();
        app_loop
            .dispatch(AppEvent::Adjust(Direction::Right))
            .unwrap();
        let correlation = app_loop
            .state()
            .engine_selection()
            .correlation()
            .unwrap()
            .clone();
        let failure_focus = app_loop.state().interaction().patch_control_focus();
        let failure_envelope = *app_loop.patches()[0].envelope();
        app_loop
            .dispatch_from(
                AppEvent::EnginePreparationFailed {
                    request_id: correlation.request_id(),
                    patch_id: correlation.patch_id().unwrap(),
                    intent: correlation.intent().clone(),
                    source_capability_id: correlation.source_capability_id().unwrap().clone(),
                    target_capability_id: correlation.target_capability_id().unwrap().clone(),
                    source_graph_revision: correlation.source_graph_revision(),
                    target_graph_revision: GraphRevision::new(2).unwrap(),
                    failure: EngineSelectionFailure::AssetUnavailable,
                },
                EventSource::Worker,
            )
            .unwrap();
        assert_eq!(
            app_loop.state().engine_selection().kind(),
            EngineSelectionStatusKind::Unavailable
        );
        assert_eq!(
            app_loop.state().interaction().patch_control_focus(),
            failure_focus
        );
        assert_eq!(*app_loop.patches()[0].envelope(), failure_envelope);
        assert_eq!(
            app_loop.current_patch_page().unwrap().focused_control_id(),
            crate::control::PatchControlId::Engine
        );

        enter_detail_at(
            &mut app_loop,
            &PatchControlId::Envelope(VoiceEnvelopeParameter::AttackMilliseconds),
        );
        app_loop
            .dispatch(AppEvent::Adjust(Direction::Right))
            .unwrap();
        let failed_edit = app_loop.event_log_ref().records().back().unwrap();
        assert!(failed_edit
            .emitted_events()
            .iter()
            .all(|event| !matches!(event, EmittedEvent::EngineSelection { .. })));
        assert_eq!(
            app_loop.current_parameters().graph_revision(),
            GraphRevision::INITIAL
        );
        assert_eq!(
            app_loop
                .current_parameters()
                .patch(patch_id)
                .unwrap()
                .envelope()
                .attack_milliseconds(),
            502.0
        );
        renderer.render(&mut output);
        assert_eq!(renderer.active_revision(), GraphRevision::INITIAL);
        assert_eq!(
            renderer
                .parameters()
                .patch(patch_id)
                .unwrap()
                .envelope()
                .attack_milliseconds(),
            502.0
        );
        assert!(output.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn one_way_control_loop_edits_only_the_selected_non_first_patch() {
        let (mut app_loop, observations) =
            loop_with_state(installed_state_with_gains(&[0.0, -12.0]));

        app_loop
            .dispatch(AppEvent::Navigate(Direction::Right))
            .unwrap();
        let result = app_loop
            .dispatch(AppEvent::Adjust(Direction::Right))
            .unwrap();
        let observations = observations.lock().unwrap();
        let published = observations.parameters.last().unwrap();

        assert_eq!(
            published.mixer_track(MixerTrackId::default()).level_db(),
            0.0
        );
        assert_eq!(
            published
                .mixer_track(MixerTrackId::new(1).unwrap())
                .level_db(),
            -11.0
        );
        assert!(result.snapshot().json().contains("\"levelDb\":-11.0"));
    }

    #[test]
    fn midi_preserves_order_and_projection_generation_reusing_scalar_values() {
        let (mut app_loop, observations) = loop_with_observations();
        app_loop
            .dispatch(AppEvent::Adjust(Direction::Right))
            .unwrap();
        let before = observations.lock().unwrap().clone();
        let generation = app_loop.current_parameters().generation();
        let mut commands = Vec::new();
        for (index, kind) in [
            MidiMessageKind::NoteOn,
            MidiMessageKind::PitchBend,
            MidiMessageKind::NoteOff,
        ]
        .into_iter()
        .enumerate()
        {
            let message =
                MidiMessage::try_new(MidiChannel::new(0).unwrap(), kind, 60, 100).unwrap();
            commands.push(AudioCommand::patch_midi(PatchId::new(1).unwrap(), message));
            let result = app_loop
                .dispatch(AppEvent::Midi {
                    patch_id: PatchId::new(1).unwrap(),
                    message,
                })
                .unwrap();
            let expected = generation + index as u64 + 1;
            assert_eq!(result.accepted().generation(), expected);
            assert_eq!(app_loop.current_parameters().generation(), expected);
            assert_eq!(app_loop.current_graphical_shell().generation(), expected);
            assert_eq!(app_loop.current_state_tree().generation(), expected);
            assert!(result.audio_effects_published());
            assert!(app_loop
                .event_log_ref()
                .records()
                .back()
                .unwrap()
                .emitted_events()
                .iter()
                .any(|event| matches!(event, EmittedEvent::ParameterSnapshotPublished { .. })));
        }
        let observations = observations.lock().unwrap();
        assert_eq!(observations.parameters.len(), before.parameters.len() + 3);
        for (index, parameters) in observations.parameters[before.parameters.len()..]
            .iter()
            .enumerate()
        {
            assert_eq!(
                *parameters,
                before
                    .parameters
                    .last()
                    .unwrap()
                    .clone()
                    .with_generation(generation + index as u64 + 1)
            );
        }
        assert_eq!(
            &observations.order[before.order.len()..],
            &[
                "parameters",
                "command",
                "parameters",
                "command",
                "parameters",
                "command"
            ]
        );
        assert_eq!(&observations.commands[before.commands.len()..], commands);
    }

    #[test]
    fn one_way_control_loop_rejection_has_no_effects_or_view_change() {
        let (mut app_loop, observations) = loop_with_observations();
        let initial_text = app_loop.current_text();
        let initial_shell = app_loop.current_graphical_shell();
        let initial_observations = observations.lock().unwrap().clone();

        let result = app_loop.dispatch(AppEvent::InstallPatches(Vec::new()));

        assert_eq!(result, Err(EventRejection::InstallationClosed));
        assert_eq!(app_loop.current_text(), initial_text);
        assert_eq!(app_loop.current_graphical_shell(), initial_shell);
        assert_eq!(*observations.lock().unwrap(), initial_observations);
        assert_eq!(app_loop.event_log().len(), 1);
    }

    #[test]
    fn one_way_control_loop_boundary_rejection_is_nonfatal() {
        let (mut app_loop, observations) =
            loop_with_state(installed_state_with_gains(&[MixerTrackParameter::Level
                .descriptor()
                .maximum()]));
        let initial_observations = observations.lock().unwrap().clone();

        assert_eq!(
            app_loop.dispatch(AppEvent::Adjust(Direction::Right)),
            Err(EventRejection::ParameterAtBoundary)
        );
        assert_eq!(*observations.lock().unwrap(), initial_observations);

        let result = app_loop
            .dispatch(AppEvent::Adjust(Direction::Left))
            .unwrap();
        let log = app_loop.event_log();

        assert_eq!(result.accepted().generation(), 2);
        assert_eq!(
            observations
                .lock()
                .unwrap()
                .parameters
                .last()
                .unwrap()
                .mixer_track(MixerTrackId::default())
                .level_db(),
            5.0
        );
        assert_eq!(log.len(), 2);
        assert_eq!(log.records()[0].outcome(), EventOutcome::Rejected);
        assert_eq!(log.records()[1].outcome(), EventOutcome::Accepted);
        assert_eq!(
            log.records()[0].state_hash_after(),
            log.records()[1].state_hash_before()
        );
    }

    #[test]
    fn one_way_control_loop_reports_queue_saturation_after_acceptance() {
        let (mut app_loop, observations) = loop_with_observations();
        observations.lock().unwrap().reject_commands = true;
        let message = MidiMessage::try_new(
            MidiChannel::new(0).unwrap(),
            MidiMessageKind::NoteOn,
            64,
            90,
        )
        .unwrap();
        let command = AudioCommand::patch_midi(PatchId::new(1).unwrap(), message);

        let result: DispatchResult = app_loop
            .dispatch(AppEvent::Midi {
                patch_id: PatchId::new(1).unwrap(),
                message,
            })
            .unwrap();

        assert_eq!(result.boundary_full(), Some(BoundaryFull::new(command)));
        assert!(!result.audio_effects_published());
        assert_eq!(
            observations
                .lock()
                .unwrap()
                .parameters
                .last()
                .unwrap()
                .generation(),
            result.accepted().generation()
        );
    }

    #[test]
    fn control_observation_trace_records_exact_sources_hashes_and_generations() {
        let (mut app_loop, _) = loop_with_observations();
        let initial_tree = app_loop.current_state_tree();

        assert_eq!(
            app_loop.dispatch_from(AppEvent::InstallPatches(Vec::new()), EventSource::DemoScene,),
            Err(EventRejection::InstallationClosed)
        );
        let accepted = app_loop
            .dispatch_from(AppEvent::Adjust(Direction::Right), EventSource::Keyboard)
            .unwrap();

        let log = app_loop.event_log();
        let tree = app_loop.current_state_tree();
        assert_eq!(log.records().len(), 2);
        assert_eq!(log.records()[0].sequence(), 0);
        assert_eq!(log.records()[0].source(), EventSource::DemoScene);
        assert_eq!(log.records()[0].outcome(), EventOutcome::Rejected);
        assert_eq!(
            log.records()[0].generation_before(),
            initial_tree.generation()
        );
        assert_eq!(
            log.records()[0].generation_after(),
            initial_tree.generation()
        );
        assert_eq!(
            log.records()[0].state_hash_before(),
            log.records()[0].state_hash_after()
        );
        assert_eq!(log.records()[1].sequence(), 1);
        assert_eq!(log.records()[1].source(), EventSource::Keyboard);
        assert_eq!(log.records()[1].outcome(), EventOutcome::Accepted);
        assert_eq!(
            log.records()[0].generation_after(),
            log.records()[1].generation_before()
        );
        assert_eq!(
            log.records()[0].state_hash_after(),
            log.records()[1].state_hash_before()
        );
        assert_eq!(tree.generation(), accepted.accepted().generation());
        assert_eq!(tree.state_hash(), accepted.snapshot().hash());
        assert_eq!(
            log.records()[1].parameter_generation(),
            accepted.accepted().generation()
        );
        assert_eq!(
            log.records()[1].projection_state_hash(),
            accepted.snapshot().hash()
        );

        for property in [
            "\"patches\"",
            "\"global\"",
            "\"interaction\"",
            "\"patchPage\"",
            "\"projection\"",
            "\"parameters\"",
        ] {
            assert!(tree.json().contains(property));
        }
    }

    #[test]
    fn patch_adsr_coexists_with_preparing_staged_activation_and_latest_target_snapshot() {
        let registry = production_capability_registry().unwrap();
        let config_factory = DescriptorDefaultConfigFactory::new(
            registry.clone(),
            production_instrument_providers().unwrap(),
        );
        let patch_id = PatchId::new(1).unwrap();
        let soundfont_config = config_factory
            .create(&CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap())
            .unwrap();
        let mut state = AppState::for_graph(
            registry.clone(),
            global_parameters(),
            GraphRevision::INITIAL,
        );
        state
            .apply(AppEvent::InstallPatches(vec![Patch::new(
                patch_id,
                "Lifecycle envelope".to_owned(),
                soundfont_config,
                MidiChannel::new(0).unwrap(),
                PatchOutput::default(),
            )
            .with_envelope(
                VoiceEnvelope::new(500.0, 600.0, 0.5, 700.0).unwrap(),
            )]))
            .unwrap();

        let initial_transport =
            ParameterSnapshot::new(0, global_parameters(), MixerState::default(), &[]).unwrap();
        let audio_boundary = LockFreeAudioBoundary::new(64, initial_transport);
        let (audio_control, audio_handle) = audio_boundary.into_handles();
        let mut app_loop = AppLoop::new(
            state,
            StateProjector::for_graph(GraphRevision::INITIAL),
            audio_control,
        )
        .unwrap();
        let audio_config =
            AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 512).unwrap();
        let initial_graph =
            PreparedGraphBuilder::new(&registry, &production_instrument_preparers().unwrap())
                .build(
                    GraphRevision::INITIAL,
                    app_loop.patches(),
                    app_loop.current_parameters().clone(),
                    audio_config.sample_rate(),
                    audio_config.render_capacity_frames(),
                )
                .unwrap();
        let structural = LockFreeStructuralGraphBoundary::new(
            1,
            1,
            GraphHandoffStatus::with_active(GraphRevision::INITIAL),
        )
        .unwrap();
        let (structural_control, structural_audio) = structural.into_handles();
        let blocked = Arc::new(AtomicBool::new(true));
        let attempted_parameters = Arc::new(Mutex::new(Vec::new()));
        let worker = DeterministicGraphPreparationWorker::new(
            registry.clone(),
            production_instrument_preparers().unwrap(),
            audio_config,
        );
        let worker_handle = worker.advance_handle();
        app_loop
            .configure_engine_selection(
                DescriptorDefaultConfigFactory::new(
                    registry,
                    production_instrument_providers().unwrap(),
                ),
                worker,
                ObservedStructuralControl {
                    inner: structural_control,
                    blocked: Arc::clone(&blocked),
                    attempted_parameters: Arc::clone(&attempted_parameters),
                    status_override: Arc::new(Mutex::new(None)),
                },
                &initial_graph,
                audio_config,
            )
            .unwrap();
        let mut renderer = AudioRenderer::new(audio_handle, structural_audio, initial_graph);
        let mut output = [0.0_f32; 1_024];
        let note = MidiMessage::try_new(
            MidiChannel::new(0).unwrap(),
            MidiMessageKind::NoteOn,
            60,
            110,
        )
        .unwrap();

        app_loop
            .dispatch(AppEvent::SelectContext(TopLevelContext::Patch))
            .unwrap();
        while app_loop
            .current_patch_page()
            .is_some_and(|page| page.focused_control_id() != PatchControlId::Engine)
        {
            app_loop
                .dispatch(AppEvent::Navigate(Direction::Up))
                .expect("the PATCH Overview order reaches Engine");
        }
        app_loop
            .dispatch(AppEvent::Adjust(Direction::Right))
            .unwrap();
        advance_engine_admission(&mut app_loop);
        assert_eq!(
            app_loop.state().engine_selection().kind(),
            EngineSelectionStatusKind::Preparing
        );
        enter_detail_at(
            &mut app_loop,
            &PatchControlId::Envelope(VoiceEnvelopeParameter::AttackMilliseconds),
        );
        app_loop
            .dispatch(AppEvent::Adjust(Direction::Right))
            .unwrap();
        assert_eq!(
            app_loop.current_patch_page().unwrap().focused_control_id(),
            crate::control::PatchControlId::Envelope(VoiceEnvelopeParameter::AttackMilliseconds)
        );
        assert_eq!(
            app_loop.current_parameters().graph_revision(),
            GraphRevision::INITIAL
        );
        assert_eq!(
            app_loop
                .current_parameters()
                .patch(patch_id)
                .unwrap()
                .envelope()
                .attack_milliseconds(),
            501.0
        );

        app_loop.dispatch(AppEvent::Return).unwrap();
        assert_eq!(
            app_loop.state().interaction().active_surface(),
            SurfaceId::PatchMain,
            "Return closes the transient Detail before the busy Overview edit; session={:?}, return={:?}",
            app_loop.state().interaction().subordinate_session(),
            app_loop.state().interaction().return_path(),
        );
        let busy_state = app_loop.current_state_tree();
        assert_eq!(
            app_loop.dispatch(AppEvent::Adjust(Direction::Right)),
            Err(EventRejection::StructuralEditBusy)
        );
        assert_eq!(app_loop.current_state_tree(), busy_state);
        enter_detail_at(
            &mut app_loop,
            &PatchControlId::Envelope(VoiceEnvelopeParameter::AttackMilliseconds),
        );

        app_loop
            .dispatch(AppEvent::Midi {
                patch_id,
                message: note,
            })
            .unwrap();
        renderer.render(&mut output);
        assert_eq!(renderer.active_revision(), GraphRevision::INITIAL);
        assert_eq!(
            renderer
                .parameters()
                .patch(patch_id)
                .unwrap()
                .envelope()
                .attack_milliseconds(),
            501.0
        );
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(output.iter().any(|sample| sample.abs() > f32::EPSILON));

        assert!(worker_handle.advance());
        let prepared = app_loop.advance_structural().unwrap();
        let target_revision = GraphRevision::new(2).unwrap();
        assert!(prepared.worker_result_polled());
        assert_eq!(prepared.graph_stage(), Some(GraphStageOutcome::Staged));
        assert_eq!(prepared.graph_published(), None);
        assert_eq!(app_loop.staged_graph_revision(), Some(target_revision));
        assert_eq!(app_loop.in_flight_graph_revision(), None);
        assert_eq!(
            app_loop.state().engine_selection().kind(),
            EngineSelectionStatusKind::Activating
        );
        assert_eq!(
            app_loop.current_parameters().graph_revision(),
            target_revision
        );
        assert_eq!(
            app_loop.state().interaction().active_surface(),
            SurfaceId::PatchDetail,
            "preparing the candidate retains the still-active instrument Detail"
        );
        assert_eq!(
            app_loop.current_patch_page().unwrap().focused_control_id(),
            crate::control::PatchControlId::Envelope(VoiceEnvelopeParameter::AttackMilliseconds)
        );
        let first_attempt = attempted_parameters.lock().unwrap()[0].clone();
        assert_eq!(first_attempt.graph_revision(), target_revision);
        assert_eq!(
            first_attempt
                .patch(patch_id)
                .unwrap()
                .envelope()
                .attack_milliseconds(),
            501.0,
            "candidate activation fallback refreshes from the latest Preparing edit"
        );

        let activating_edit = app_loop
            .dispatch(AppEvent::Adjust(Direction::Right))
            .unwrap();
        assert_eq!(activating_edit.boundary_full(), None);
        assert_eq!(
            app_loop
                .current_parameters()
                .patch(patch_id)
                .unwrap()
                .envelope()
                .attack_milliseconds(),
            502.0
        );
        assert_eq!(
            app_loop.current_parameters().graph_revision(),
            target_revision
        );
        assert!(app_loop
            .event_log_ref()
            .records()
            .back()
            .unwrap()
            .emitted_events()
            .iter()
            .all(|event| !matches!(event, EmittedEvent::EngineSelection { .. })));

        app_loop.dispatch(AppEvent::Return).unwrap();
        assert_eq!(
            app_loop.dispatch(AppEvent::Adjust(Direction::Left)),
            Err(EventRejection::StructuralEditBusy)
        );
        enter_detail_at(
            &mut app_loop,
            &PatchControlId::Envelope(VoiceEnvelopeParameter::AttackMilliseconds),
        );

        output.fill(0.0);
        // A MIDI burst after scalar edits and navigation must preserve the
        // exact candidate values and revision while advancing each generation.
        for _ in 0..8 {
            app_loop
                .dispatch(AppEvent::Midi {
                    patch_id,
                    message: MidiMessage::try_new(
                        MidiChannel::new(0).unwrap(),
                        MidiMessageKind::PitchBend,
                        0,
                        64,
                    )
                    .unwrap(),
                })
                .unwrap();
            assert_eq!(
                app_loop.last_published_parameters,
                app_loop.latest_pending_candidate_parameters().unwrap()
            );
        }
        renderer.render(&mut output);
        assert_eq!(renderer.active_revision(), GraphRevision::INITIAL);
        assert_eq!(
            renderer
                .parameters()
                .patch(patch_id)
                .unwrap()
                .envelope()
                .attack_milliseconds(),
            501.0,
            "the source holds its last compatible source-revision snapshot"
        );
        assert!(renderer.handoff_status().incompatible_snapshots() > 0);
        assert!(output.iter().all(|sample| sample.is_finite()));

        blocked.store(false, Ordering::SeqCst);
        let published = app_loop.advance_structural().unwrap();
        assert_eq!(published.graph_published(), Some(target_revision));
        assert_eq!(app_loop.staged_graph_revision(), None);
        assert_eq!(app_loop.in_flight_graph_revision(), Some(target_revision));
        let attempts = attempted_parameters.lock().unwrap();
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0], attempts[1]);
        drop(attempts);

        app_loop
            .dispatch(AppEvent::Midi {
                patch_id,
                message: note,
            })
            .unwrap();
        output.fill(0.0);
        renderer.render(&mut output);
        assert_eq!(renderer.active_revision(), target_revision);
        assert_eq!(
            renderer
                .parameters()
                .patch(patch_id)
                .unwrap()
                .envelope()
                .attack_milliseconds(),
            502.0,
            "the activated graph consumes the latest target-revision edit"
        );
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(output.iter().any(|sample| sample.abs() > f32::EPSILON));

        let acknowledged = app_loop.advance_structural().unwrap();
        assert_eq!(
            acknowledged.activation_acknowledged(),
            Some(target_revision)
        );
        assert_eq!(
            app_loop.state().engine_selection().kind(),
            EngineSelectionStatusKind::Ready
        );
        assert_eq!(
            app_loop.current_patch_page().unwrap().focused_control_id(),
            crate::control::PatchControlId::Engine,
            "acknowledgement commits the new subject and closes stale Detail to its exact origin"
        );
        assert_eq!(
            app_loop.patches()[0].envelope().attack_milliseconds(),
            502.0
        );

        drop(renderer);
        app_loop.shutdown_engine_selection_on_control().unwrap();
    }

    #[test]
    fn app_loop_coordinates_real_bidirectional_graph_work_and_recoverable_failure() {
        let registry = production_capability_registry().unwrap();
        let config_factory = DescriptorDefaultConfigFactory::new(
            registry.clone(),
            production_instrument_providers().unwrap(),
        );
        let patch_id = PatchId::new(1).unwrap();
        let soundfont_config = config_factory
            .create(&CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap())
            .unwrap();
        let mut state = AppState::for_graph(
            registry.clone(),
            global_parameters(),
            GraphRevision::INITIAL,
        );
        state
            .apply(AppEvent::InstallPatches(vec![Patch::new(
                patch_id,
                "Orchestrated Patch".to_owned(),
                soundfont_config.clone(),
                MidiChannel::new(0).unwrap(),
                PatchOutput::default(),
            )]))
            .unwrap();

        let initial_transport =
            ParameterSnapshot::new(0, global_parameters(), MixerState::default(), &[]).unwrap();
        let audio_boundary = LockFreeAudioBoundary::new(64, initial_transport);
        let (audio_control, audio_handle) = audio_boundary.into_handles();
        let mut app_loop = AppLoop::new(
            state,
            StateProjector::for_graph(GraphRevision::INITIAL),
            audio_control,
        )
        .unwrap();
        let audio_config =
            AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 512).unwrap();
        let initial_preparers = production_instrument_preparers().unwrap();
        let initial_graph = PreparedGraphBuilder::new(&registry, &initial_preparers)
            .build(
                GraphRevision::INITIAL,
                app_loop.patches(),
                app_loop.current_parameters().clone(),
                audio_config.sample_rate(),
                audio_config.render_capacity_frames(),
            )
            .unwrap();
        let structural = LockFreeStructuralGraphBoundary::new(
            1,
            1,
            GraphHandoffStatus::with_active(GraphRevision::INITIAL),
        )
        .unwrap();
        let (structural_control, structural_audio) = structural.into_handles();
        let worker = DeterministicGraphPreparationWorker::new(
            registry.clone(),
            production_instrument_preparers().unwrap(),
            audio_config,
        );
        let worker_handle = worker.advance_handle();
        app_loop
            .configure_engine_selection(
                DescriptorDefaultConfigFactory::new(
                    registry,
                    production_instrument_providers().unwrap(),
                ),
                worker,
                structural_control,
                &initial_graph,
                audio_config,
            )
            .unwrap();
        let mut renderer = AudioRenderer::new(audio_handle, structural_audio, initial_graph);
        let mut output = [0.0; 1_024];

        app_loop
            .dispatch_from(
                AppEvent::SelectContext(TopLevelContext::Patch),
                EventSource::Keyboard,
            )
            .unwrap();
        app_loop
            .dispatch_from(AppEvent::Adjust(Direction::Right), EventSource::Keyboard)
            .unwrap();
        assert_eq!(
            app_loop.state().engine_selection().kind(),
            EngineSelectionStatusKind::Loading
        );
        advance_engine_admission(&mut app_loop);
        assert_eq!(
            app_loop.state().engine_selection().kind(),
            EngineSelectionStatusKind::Preparing
        );
        assert!(worker_handle.is_pending());
        assert_eq!(
            app_loop.dispatch_from(AppEvent::Adjust(Direction::Right), EventSource::Keyboard),
            Err(EventRejection::StructuralEditBusy)
        );

        app_loop
            .dispatch(AppEvent::SelectContext(TopLevelContext::Mixer))
            .unwrap();
        app_loop
            .dispatch(AppEvent::Adjust(Direction::Right))
            .unwrap();
        let edited_gain = app_loop
            .state()
            .mixer()
            .track(MixerTrackId::default())
            .level_db();
        let note = MidiMessage::try_new(
            MidiChannel::new(0).unwrap(),
            MidiMessageKind::NoteOn,
            60,
            100,
        )
        .unwrap();
        app_loop
            .dispatch(AppEvent::Midi {
                patch_id,
                message: note,
            })
            .unwrap();

        assert!(worker_handle.advance());
        let prepared = app_loop.advance_structural().unwrap();
        assert!(prepared.worker_result_polled());
        assert_eq!(prepared.graph_stage(), Some(GraphStageOutcome::Staged));
        assert_eq!(
            prepared.graph_published(),
            Some(GraphRevision::new(2).unwrap())
        );
        assert_eq!(
            app_loop.state().engine_selection().kind(),
            EngineSelectionStatusKind::Activating
        );
        assert_eq!(app_loop.graph_revision(), GraphRevision::new(2).unwrap());
        assert_eq!(
            app_loop
                .current_parameters()
                .mixer_track(MixerTrackId::default())
                .level_db(),
            edited_gain
        );
        renderer.render(&mut output);
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(output.iter().any(|sample| sample.abs() > f32::EPSILON));
        let acknowledged = app_loop.advance_structural().unwrap();
        assert_eq!(
            acknowledged.activation_acknowledged(),
            Some(GraphRevision::new(2).unwrap())
        );
        assert_eq!(
            app_loop.state().engine_selection().kind(),
            EngineSelectionStatusKind::Ready
        );
        assert_eq!(
            app_loop.patches()[0]
                .instrument_config()
                .capability_id()
                .as_str(),
            BRAIDS_CAPABILITY_ID
        );

        app_loop
            .dispatch(AppEvent::SelectContext(TopLevelContext::Patch))
            .unwrap();
        app_loop
            .dispatch(AppEvent::Adjust(Direction::Left))
            .unwrap();
        advance_engine_admission(&mut app_loop);
        assert!(worker_handle.advance());
        let reverse = app_loop.advance_structural().unwrap();
        assert_eq!(reverse.graph_stage(), Some(GraphStageOutcome::Staged));
        assert_eq!(
            reverse.graph_published(),
            Some(GraphRevision::new(3).unwrap())
        );
        app_loop
            .dispatch(AppEvent::Midi {
                patch_id,
                message: note,
            })
            .unwrap();
        renderer.render(&mut output);
        assert_eq!(
            app_loop
                .advance_structural()
                .unwrap()
                .activation_acknowledged(),
            Some(GraphRevision::new(3).unwrap())
        );
        assert_eq!(app_loop.patches()[0].instrument_config(), &soundfont_config);

        app_loop
            .dispatch(AppEvent::Adjust(Direction::Right))
            .unwrap();
        advance_engine_admission(&mut app_loop);
        worker_handle.fail_next(EngineSelectionFailure::AssetUnavailable);
        assert!(worker_handle.advance());
        let failed = app_loop.advance_structural().unwrap();
        assert!(failed.failure_dispatched());
        assert_eq!(
            app_loop.state().engine_selection().kind(),
            EngineSelectionStatusKind::Unavailable
        );
        assert_eq!(
            app_loop.state().engine_selection().failure(),
            Some(EngineSelectionFailure::AssetUnavailable)
        );
        assert_eq!(app_loop.graph_revision(), GraphRevision::new(3).unwrap());
        assert_eq!(app_loop.patches()[0].instrument_config(), &soundfont_config);

        app_loop
            .dispatch(AppEvent::Adjust(Direction::Right))
            .unwrap();
        advance_engine_admission(&mut app_loop);
        assert!(worker_handle.advance());
        app_loop.advance_structural().unwrap();
        renderer.render(&mut output);
        app_loop.advance_structural().unwrap();
        assert_eq!(
            app_loop.state().engine_selection().kind(),
            EngineSelectionStatusKind::Ready
        );

        let effects = app_loop
            .event_log_ref()
            .records()
            .iter()
            .flat_map(|record| record.emitted_events())
            .filter_map(|event| match event {
                EmittedEvent::EngineSelection { effect } => Some(effect.kind()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(effects.contains(&EngineSelectionEffectKind::PrepareRequested));
        assert!(effects.contains(&EngineSelectionEffectKind::CandidatePrepared));
        assert!(effects.contains(&EngineSelectionEffectKind::GraphPublished));
        assert!(effects.contains(&EngineSelectionEffectKind::ActivationAcknowledged));

        drop(renderer);
        app_loop.shutdown_engine_selection_on_control().unwrap();
    }

    #[test]
    fn implicit_creation_prepares_and_activates_one_complete_appended_graph_before_commit() {
        crate::real_time::callback_safety::reset_callback_safety_counts();
        let registry = production_capability_registry().unwrap();
        let blueprint_factory = DescriptorDefaultConfigFactory::new(
            registry.clone(),
            production_instrument_providers().unwrap(),
        );
        let default_id = CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap();
        let blueprint =
            crate::control::PatchCreationBlueprint::resolve(&default_id, &blueprint_factory)
                .unwrap();
        let first = blueprint.candidate(&[]).unwrap();
        let first_id = first.id();
        let mut state = AppState::for_graph(
            registry.clone(),
            global_parameters(),
            GraphRevision::INITIAL,
        )
        .with_patch_creation_blueprint(blueprint);
        state.apply(AppEvent::InstallPatches(vec![first])).unwrap();

        let initial_transport =
            ParameterSnapshot::new(0, global_parameters(), MixerState::default(), &[]).unwrap();
        let audio_boundary = LockFreeAudioBoundary::new(64, initial_transport);
        let (audio_control, audio_handle) = audio_boundary.into_handles();
        let mut app_loop = AppLoop::new(
            state,
            StateProjector::for_graph(GraphRevision::INITIAL),
            audio_control,
        )
        .unwrap();
        let audio_config =
            AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 512).unwrap();
        let initial_graph =
            PreparedGraphBuilder::new(&registry, &production_instrument_preparers().unwrap())
                .build(
                    GraphRevision::INITIAL,
                    app_loop.patches(),
                    app_loop.current_parameters().clone(),
                    audio_config.sample_rate(),
                    audio_config.render_capacity_frames(),
                )
                .unwrap();
        let structural = LockFreeStructuralGraphBoundary::new(
            1,
            1,
            GraphHandoffStatus::with_active(GraphRevision::INITIAL),
        )
        .unwrap();
        let (structural_control, structural_audio) = structural.into_handles();
        let worker = DeterministicGraphPreparationWorker::new(
            registry.clone(),
            production_instrument_preparers().unwrap(),
            audio_config,
        );
        let worker_handle = worker.advance_handle();
        app_loop
            .configure_engine_selection(
                DescriptorDefaultConfigFactory::new(
                    registry,
                    production_instrument_providers().unwrap(),
                ),
                worker,
                structural_control,
                &initial_graph,
                audio_config,
            )
            .unwrap();
        let mut renderer = AudioRenderer::new(audio_handle, structural_audio, initial_graph);
        let mut output = [0.0; 1_024];

        app_loop
            .dispatch(AppEvent::SelectContext(TopLevelContext::Patch))
            .unwrap();
        app_loop
            .dispatch(AppEvent::SelectPatch(Direction::Right))
            .unwrap();
        assert!(app_loop.current_patch_page().unwrap().patch().is_empty());
        let saved_before = app_loop.capture_saved_session();
        let document = crate::shell::SessionDocument::new_untitled(saved_before.clone());
        assert!(!document.is_dirty(&app_loop.capture_saved_session()));
        let source_parameters = app_loop.current_parameters().clone();

        let note = MidiMessage::try_new(
            MidiChannel::new(0).unwrap(),
            MidiMessageKind::NoteOn,
            60,
            100,
        )
        .unwrap();
        app_loop
            .dispatch(AppEvent::Midi {
                patch_id: first_id,
                message: note,
            })
            .unwrap();
        renderer.render(&mut output);
        assert!(renderer
            .active_patch_audio()
            .stems()
            .first()
            .unwrap()
            .samples()
            .iter()
            .any(|sample| sample.abs() > f32::EPSILON));

        app_loop
            .dispatch(AppEvent::SetInteractionMode(
                crate::control::InteractionMode::Adjust,
            ))
            .unwrap();
        app_loop.dispatch(AppEvent::Adjust(Direction::Up)).unwrap();
        app_loop.dispatch(AppEvent::Activate).unwrap();
        assert_eq!(app_loop.patches().len(), 1);
        assert_eq!(app_loop.capture_saved_session(), saved_before);
        assert!(!document.is_dirty(&app_loop.capture_saved_session()));
        assert_eq!(app_loop.current_parameters().patch_count(), 1);
        assert_eq!(
            app_loop.state().engine_selection().kind(),
            EngineSelectionStatusKind::Loading
        );
        let candidate_channel_note = MidiMessage::try_new(
            MidiChannel::new(1).unwrap(),
            MidiMessageKind::NoteOn,
            64,
            100,
        )
        .unwrap();
        assert_eq!(
            app_loop
                .dispatch_midi_from(candidate_channel_note, EventSource::PhysicalMidi)
                .unwrap()
                .subscriber_count(),
            0,
            "the pending candidate is not a MIDI subscriber before commit"
        );

        advance_engine_admission(&mut app_loop);
        worker_handle.fail_next(EngineSelectionFailure::AllocationFailed);
        assert!(worker_handle.advance());
        let failed = app_loop.advance_structural().unwrap();
        assert!(failed.failure_dispatched());
        assert_eq!(
            app_loop.state().engine_selection().kind(),
            EngineSelectionStatusKind::Failed
        );
        assert_eq!(
            app_loop.state().engine_selection().failure(),
            Some(EngineSelectionFailure::AllocationFailed)
        );
        assert_eq!(app_loop.patches().len(), 1);
        assert_eq!(app_loop.capture_saved_session(), saved_before);
        assert!(!document.is_dirty(&app_loop.capture_saved_session()));
        assert_eq!(app_loop.graph_revision(), GraphRevision::INITIAL);
        assert!(app_loop.current_patch_page().unwrap().patch().is_empty());

        app_loop.dispatch(AppEvent::Activate).unwrap();
        assert_eq!(
            app_loop.state().engine_selection().kind(),
            EngineSelectionStatusKind::Loading
        );
        advance_engine_admission(&mut app_loop);
        assert!(worker_handle.advance());
        let prepared = app_loop.advance_structural().unwrap();
        assert!(prepared.worker_result_polled());
        assert_eq!(prepared.graph_stage(), Some(GraphStageOutcome::Staged));
        assert_eq!(app_loop.patches().len(), 1);
        assert_eq!(app_loop.capture_saved_session(), saved_before);
        assert!(!document.is_dirty(&app_loop.capture_saved_session()));
        assert_eq!(app_loop.current_parameters().patch_count(), 1);
        assert_eq!(
            app_loop.state().engine_selection().kind(),
            EngineSelectionStatusKind::Activating
        );

        output.fill(0.0);
        renderer.render(&mut output);
        let target = GraphRevision::new(2).unwrap();
        assert_eq!(renderer.active_revision(), target);
        assert_eq!(renderer.parameters().patch_count(), 2);
        let stems = renderer.active_patch_audio().stems();
        assert!(stems[0]
            .samples()
            .iter()
            .any(|sample| sample.abs() > f32::EPSILON));
        assert!(stems[1]
            .samples()
            .iter()
            .all(|sample| sample.abs() <= f32::EPSILON));

        let acknowledged = app_loop.advance_structural().unwrap();
        assert_eq!(acknowledged.activation_acknowledged(), Some(target));
        assert_eq!(app_loop.patches().len(), 2);
        assert_eq!(app_loop.patches()[1].id(), PatchId::new(2).unwrap());
        assert_ne!(app_loop.capture_saved_session(), saved_before);
        assert!(document.is_dirty(&app_loop.capture_saved_session()));
        let committed_json = app_loop.capture_saved_session().to_json().unwrap();
        assert_eq!(
            crate::control::SavedSession::from_json(&committed_json, app_loop.capabilities(),)
                .unwrap(),
            app_loop.capture_saved_session()
        );
        assert!(!committed_json.contains("trailingEmpty"));
        assert_eq!(app_loop.current_parameters().patch_count(), 2);
        assert_eq!(app_loop.current_parameters().graph_revision(), target);
        assert_ne!(app_loop.current_parameters(), &source_parameters);
        assert_eq!(
            app_loop.current_patch_page().unwrap().patch().id(),
            Some(PatchId::new(2).unwrap())
        );
        assert_eq!(
            app_loop
                .dispatch_midi_from(candidate_channel_note, EventSource::PhysicalMidi)
                .unwrap()
                .subscriber_count(),
            1,
            "the created Patch joins MIDI fan-out only after acknowledgement"
        );
        let callback_safety = crate::real_time::callback_safety::callback_safety_snapshot();
        assert_eq!(callback_safety.allocations(), 0);
        assert_eq!(callback_safety.destructions(), 0);

        drop(renderer);
        app_loop.shutdown_engine_selection_on_control().unwrap();
    }

    fn loop_with_active_physical_input() -> (
        AppLoop<TestBoundary>,
        Arc<Mutex<BoundaryObservations>>,
        FakeMidiWorkerHandle,
    ) {
        let (mut app_loop, observations) = loop_with_observations();
        let identity = MidiInputDeviceId::new("midir-v1", "capture-a").unwrap();
        let descriptor = MidiInputDescriptor::new(identity.clone(), "Capture A", None).unwrap();
        let (worker, handle) = FakeMidiWorker::new(descriptor);
        app_loop.configure_midi_devices(worker).unwrap();
        app_loop.advance_midi_devices(0).unwrap();
        app_loop.advance_midi_devices(1).unwrap();
        app_loop
            .dispatch_from(
                AppEvent::MidiInputConnectRequested {
                    identity: identity.clone(),
                },
                EventSource::Keyboard,
            )
            .unwrap();
        app_loop.advance_midi_devices(2).unwrap();
        app_loop.advance_midi_devices(3).unwrap();
        (app_loop, observations, handle)
    }

    #[test]
    fn hosted_transport_preserves_ingress_order_without_synth_dispatch() {
        let (mut app_loop, observations, handle) = loop_with_active_physical_input();
        let packets: &[&[u8]] = &[&[0xfa], &[0xf8], &[0x90, 60, 99], &[0xf2, 3, 0], &[0xfc]];
        for (index, packet) in packets.iter().enumerate() {
            assert_eq!(
                handle.receive_raw(100 + index as u64, packet),
                PhysicalMidiIngressOutcome::Accepted
            );
        }
        let mut captured = Vec::new();
        let progress = app_loop
            .advance_midi_devices_with_packets(34_000, &mut |packet| {
                captured.push(match packet {
                    crate::control::PhysicalMidiPacket::Performance(event) => {
                        event.raw_bytes().to_vec()
                    }
                    crate::control::PhysicalMidiPacket::Transport(event) => {
                        event.raw_bytes().to_vec()
                    }
                });
                false
            })
            .unwrap();
        assert_eq!(progress.drained_events(), packets.len());
        assert_eq!(captured, packets);
        assert_eq!(
            observations
                .lock()
                .unwrap()
                .commands
                .iter()
                .filter(|command| matches!(command, AudioCommand::PatchMidi { .. }))
                .count(),
            1
        );
        app_loop.shutdown_midi_devices_on_control().unwrap();
    }

    #[test]
    fn physical_capture_preserves_raw_timing_and_can_consume_monitoring_once() {
        let (mut app_loop, observations, handle) = loop_with_active_physical_input();
        let active = app_loop.active_midi_input_identity().unwrap().clone();
        assert_eq!(active.identity().identity(), "capture-a");
        let packets = [
            [0x90, 60, 0],
            [0xb0, 120, 37],
            [0xb0, 123, 99],
            [0xa0, 60, 23],
            [0x90, 61, 100],
        ];
        for (index, packet) in packets.iter().enumerate() {
            assert_eq!(
                handle.receive_raw(100 + index as u64, packet),
                PhysicalMidiIngressOutcome::Accepted
            );
        }
        let mut captured = Vec::new();
        let progress = app_loop
            .advance_midi_devices_with_capture(34_000, &mut |event| {
                captured.push(event);
                event.message().kind() != MidiMessageKind::NoteOn
            })
            .unwrap();
        assert_eq!(progress.drained_events(), 5);
        assert_eq!(captured.len(), 5);
        for (index, event) in captured.iter().enumerate() {
            assert_eq!(event.raw_bytes(), &packets[index]);
            assert_eq!(event.revision(), active.revision());
            assert_eq!(event.timestamp_micros(), 100 + index as u64);
            assert_eq!(event.control_receipt_micros(), Some(34_000));
        }
        assert_eq!(captured[0].message().kind(), MidiMessageKind::NoteOff);
        assert_eq!(captured[1].message().kind(), MidiMessageKind::AllNotesOff);
        assert_eq!(captured[2].message().kind(), MidiMessageKind::AllNotesOff);
        assert_eq!(captured[3].message().kind(), MidiMessageKind::PolyPressure);
        let activity = app_loop.current_midi_activity().unwrap();
        assert_eq!(activity.accepted_count(), 5);
        assert_eq!(activity.last_event(), captured.last().copied());
        assert_eq!(
            observations
                .lock()
                .unwrap()
                .commands
                .iter()
                .filter(|command| { matches!(command, AudioCommand::PatchMidi { .. }) })
                .count(),
            1
        );
        assert_eq!(
            app_loop
                .event_log_ref()
                .records()
                .iter()
                .filter(|record| { record.source() == EventSource::PhysicalMidi })
                .count(),
            1
        );
        app_loop.shutdown_midi_devices_on_control().unwrap();
    }

    #[test]
    fn physical_capture_never_receives_a_retired_connection_revision() {
        let (mut app_loop, _, handle) = loop_with_active_physical_input();
        let identity = app_loop
            .active_midi_input_identity()
            .unwrap()
            .identity()
            .clone();
        assert_eq!(
            handle.receive_raw(100, &[0x90, 60, 100]),
            PhysicalMidiIngressOutcome::Accepted
        );
        app_loop
            .dispatch_from(
                AppEvent::MidiInputDisconnectRequested { identity },
                EventSource::Keyboard,
            )
            .unwrap();
        assert!(app_loop.active_midi_input_identity().is_none());
        // Exercise the drain before queued retirement work closes the handle:
        // reducer acknowledgement alone must prevent stale capture delivery.
        let mut runtime = app_loop.midi_device_runtime.take().unwrap();
        let mut progress = super::MidiDeviceProgress::default();
        let mut captured = 0;
        app_loop.drain_physical_midi(&mut runtime, 34_000, &mut progress, &mut |_| {
            captured += 1;
            true
        });
        assert_eq!(captured, 0);
        assert_eq!(progress.stale_events(), 1);
        assert_eq!(progress.drained_events(), 0);
        app_loop.midi_device_runtime = Some(runtime);
        app_loop.shutdown_midi_devices_on_control().unwrap();
    }

    #[test]
    fn physical_worker_activation_and_bounded_drain_use_the_production_fan_out() {
        let (mut app_loop, observations) = loop_with_observations();
        let identity = MidiInputDeviceId::new("midir-v1", "physical-a").unwrap();
        let descriptor = MidiInputDescriptor::new(identity.clone(), "Physical A", None).unwrap();
        let (worker, handle) = FakeMidiWorker::new(descriptor);
        app_loop.configure_midi_devices(worker).unwrap();

        assert!(app_loop.advance_midi_devices(0).unwrap().scan_requested());
        app_loop.advance_midi_devices(1).unwrap();
        assert!(app_loop
            .state()
            .midi_input()
            .entry(&identity)
            .is_some_and(|entry| entry.present()));

        app_loop
            .dispatch_from(
                AppEvent::MidiInputConnectRequested {
                    identity: identity.clone(),
                },
                EventSource::Keyboard,
            )
            .unwrap();
        app_loop.advance_midi_devices(2).unwrap();
        app_loop.advance_midi_devices(3).unwrap();
        assert_eq!(
            app_loop.state().midi_input().active().unwrap().identity(),
            &identity
        );
        assert!(app_loop.current_midi_activity().is_some());

        for timestamp in 0..65 {
            assert_eq!(
                handle.receive_raw(timestamp, &[0x90, 60, 100]),
                PhysicalMidiIngressOutcome::Accepted
            );
        }
        let first = app_loop.advance_midi_devices(34_000).unwrap();
        assert_eq!(first.drained_events(), 64);
        assert!(app_loop.midi_is_receiving(34_000));
        let activity = app_loop.current_midi_activity_observation(34_000);
        let snapshot = activity.snapshot().unwrap();
        assert!(activity.receiving());
        assert_eq!(
            snapshot.revision(),
            app_loop.state().midi_input().active().unwrap().revision()
        );
        assert_eq!(snapshot.accepted_count(), 64);
        assert_eq!(
            snapshot.last_event().unwrap().message().kind(),
            MidiMessageKind::NoteOn
        );
        assert_eq!(
            serde_json::to_value(activity).unwrap()["snapshot"]["lastEvent"]["message"]["channel"],
            0
        );
        assert!(!app_loop
            .current_midi_activity_observation(534_001)
            .receiving());
        let second = app_loop.advance_midi_devices(34_001).unwrap();
        assert_eq!(second.drained_events(), 1);

        let records = app_loop.event_log_ref().records();
        assert_eq!(
            records
                .iter()
                .filter(|record| record.source() == EventSource::PhysicalMidi)
                .count(),
            65
        );
        assert_eq!(
            observations
                .lock()
                .unwrap()
                .commands
                .iter()
                .filter(|command| matches!(command, AudioCommand::PatchMidi { .. }))
                .count(),
            65
        );

        let old_revision = app_loop.state().midi_input().active().unwrap().revision();
        for timestamp in 100..(100 + crate::control::PHYSICAL_MIDI_QUEUE_CAPACITY as u64) {
            assert_eq!(
                handle.receive_raw(timestamp, &[0x90, 61, 99]),
                PhysicalMidiIngressOutcome::Accepted
            );
        }
        assert_eq!(
            handle.receive_raw(10_000, &[0x90, 61, 99]),
            PhysicalMidiIngressOutcome::CapacityFailure
        );
        let overflow = app_loop.advance_midi_devices(68_000).unwrap();
        assert!(overflow.ingress_overflowed());
        assert!(app_loop.state().midi_input().active().is_none());
        assert!(app_loop
            .state()
            .midi_input()
            .requested()
            .is_some_and(|request| request.revision() > old_revision));
        assert_eq!(
            app_loop
                .current_midi_activity_observation(68_000)
                .snapshot(),
            None
        );

        app_loop.advance_midi_devices(68_001).unwrap();
        app_loop.advance_midi_devices(68_002).unwrap();
        assert!(observations
            .lock()
            .unwrap()
            .commands
            .contains(&AudioCommand::ReleaseImmediateNotes));
        assert!(app_loop
            .state()
            .midi_input()
            .active()
            .is_some_and(|active| active.revision() > old_revision));

        app_loop.shutdown_midi_devices_on_control().unwrap();
        assert!(!app_loop.midi_devices_configured());
        assert_eq!(app_loop.owned_midi_connections_on_control(), 0);
        assert_eq!(handle.0.lock().unwrap().retired, 2);
    }

    #[test]
    fn whole_session_preflight_gate_and_activation_share_the_structural_owner() {
        let registry = production_capability_registry().unwrap();
        let audio_config =
            AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 512).unwrap();
        let mut initial_state = AppState::for_graph(
            registry.clone(),
            global_parameters(),
            GraphRevision::INITIAL,
        );
        initial_state
            .apply(AppEvent::InstallPatches(vec![patch(1, 0.0)]))
            .unwrap();
        let initial_parameters = StateProjector::for_graph(GraphRevision::INITIAL)
            .project(&initial_state)
            .unwrap()
            .2;
        let initial_graph =
            PreparedGraphBuilder::new(&registry, &production_instrument_preparers().unwrap())
                .build(
                    GraphRevision::INITIAL,
                    initial_state.patches(),
                    initial_parameters.clone(),
                    audio_config.sample_rate(),
                    audio_config.render_capacity_frames(),
                )
                .unwrap();
        let audio_boundary = LockFreeAudioBoundary::new(64, initial_parameters);
        let (audio_control, audio_handle) = audio_boundary.into_handles();
        let structural = LockFreeStructuralGraphBoundary::new(
            1,
            1,
            GraphHandoffStatus::with_active(GraphRevision::INITIAL),
        )
        .unwrap();
        let (structural_control, structural_audio) = structural.into_handles();
        let worker = DeterministicGraphPreparationWorker::new(
            registry.clone(),
            production_instrument_preparers().unwrap(),
            audio_config,
        );
        let mut app_loop = AppLoop::new(
            initial_state,
            StateProjector::for_graph(GraphRevision::INITIAL),
            audio_control,
        )
        .unwrap();
        app_loop
            .configure_engine_selection(
                DescriptorDefaultConfigFactory::new(
                    registry.clone(),
                    production_instrument_providers().unwrap(),
                ),
                worker,
                structural_control,
                &initial_graph,
                audio_config,
            )
            .unwrap();
        let mut renderer = AudioRenderer::new(audio_handle, structural_audio, initial_graph);

        let target_revision = app_loop.next_structural_graph_revision().unwrap();
        assert_eq!(target_revision, GraphRevision::new(2).unwrap());
        let mut target_mixer = MixerState::default();
        target_mixer.set_track(
            MixerTrackId::default(),
            MixerTrackParameters::default()
                .with_scalar_value(MixerTrackParameter::Level, -6.0)
                .unwrap(),
        );
        let mut target_state =
            AppState::for_graph(registry.clone(), global_parameters(), target_revision)
                .with_initial_mixer(target_mixer.clone());
        target_state
            .apply(AppEvent::InstallPatches(vec![patch(1, 0.0), patch(2, 0.0)]))
            .unwrap();
        let target_saved = SavedSession::capture(&target_state);
        let (replacement, graph) = target_saved
            .prepare_restore(
                registry.clone(),
                crate::synth::EffectCapabilityRegistry::default(),
                &production_instrument_preparers().unwrap(),
                &[],
                target_revision,
                audio_config.sample_rate(),
                audio_config.render_capacity_frames(),
            )
            .unwrap()
            .into_replacement();

        let wrong_revision_graph = target_saved
            .prepare_restore(
                registry.clone(),
                crate::synth::EffectCapabilityRegistry::default(),
                &production_instrument_preparers().unwrap(),
                &[],
                GraphRevision::new(3).unwrap(),
                audio_config.sample_rate(),
                audio_config.render_capacity_frames(),
            )
            .unwrap()
            .into_replacement()
            .1;
        assert!(matches!(
            app_loop.stage_session_replacement(replacement.clone(), wrong_revision_graph),
            Err(super::StructuralAdvanceError::SessionRevisionMismatch)
        ));

        let mut mismatched_mixer = target_mixer;
        mismatched_mixer.set_track(
            MixerTrackId::default(),
            MixerTrackParameters::default()
                .with_scalar_value(MixerTrackParameter::Level, -12.0)
                .unwrap(),
        );
        let mut mismatched_state =
            AppState::for_graph(registry.clone(), global_parameters(), target_revision)
                .with_initial_mixer(mismatched_mixer);
        mismatched_state
            .apply(AppEvent::InstallPatches(vec![patch(1, 0.0), patch(2, 0.0)]))
            .unwrap();
        let mismatched_graph = SavedSession::capture(&mismatched_state)
            .prepare_restore(
                registry.clone(),
                crate::synth::EffectCapabilityRegistry::default(),
                &production_instrument_preparers().unwrap(),
                &[],
                target_revision,
                audio_config.sample_rate(),
                audio_config.render_capacity_frames(),
            )
            .unwrap()
            .into_replacement()
            .1;
        let prior_saved = SavedSession::capture(app_loop.state());
        assert!(matches!(
            app_loop.stage_session_replacement(replacement.clone(), mismatched_graph),
            Err(super::StructuralAdvanceError::SessionParameterMismatch)
        ));
        assert_eq!(SavedSession::capture(app_loop.state()), prior_saved);
        assert!(!app_loop.session_replacement_pending());
        assert_eq!(renderer.active_revision(), GraphRevision::INITIAL);

        assert_eq!(
            app_loop
                .stage_session_replacement(replacement, graph)
                .unwrap(),
            GraphStageOutcome::Staged
        );
        assert!(app_loop.session_replacement_pending());
        assert!(matches!(
            app_loop.next_structural_graph_revision(),
            Err(super::StructuralAdvanceError::SessionBusy)
        ));
        let note = MidiMessage::try_new(
            MidiChannel::new(0).unwrap(),
            MidiMessageKind::NoteOn,
            60,
            100,
        )
        .unwrap();
        assert_eq!(
            app_loop.dispatch(AppEvent::Midi {
                patch_id: PatchId::new(1).unwrap(),
                message: note,
            }),
            Err(EventRejection::StructuralEditBusy)
        );
        assert_eq!(
            app_loop.dispatch(AppEvent::Adjust(Direction::Down)),
            Err(EventRejection::StructuralEditBusy)
        );
        app_loop
            .dispatch(AppEvent::SelectContext(TopLevelContext::Patch))
            .unwrap();

        let published = app_loop.advance_structural().unwrap();
        assert_eq!(published.graph_published(), Some(target_revision));
        assert!(!published.session_replacement_committed());
        let mut output = [0.0; 1_024];
        renderer.render(&mut output);
        assert_eq!(renderer.active_revision(), target_revision);
        let committed = app_loop.advance_structural().unwrap();
        assert_eq!(committed.activation_acknowledged(), Some(target_revision));
        assert!(committed.session_replacement_committed());
        assert_eq!(SavedSession::capture(app_loop.state()), target_saved);
        assert_eq!(app_loop.patches().len(), 2);
        assert_eq!(app_loop.graph_revision(), target_revision);
        assert_eq!(
            app_loop.state().interaction().patch_control_focus(),
            Some(PatchControlId::Engine)
        );
        assert!(!app_loop.session_replacement_pending());
        assert_eq!(app_loop.owned_structural_graphs_on_control(), 0);
        assert!(output.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn busy_and_stale_session_handoffs_retire_candidates_on_control_and_preserve_audio() {
        crate::real_time::callback_safety::reset_callback_safety_counts();
        let provider = BraidsCapability::new().unwrap();
        let registry = CapabilityRegistry::new(vec![provider.descriptor()]).unwrap();
        let audio_config = AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 64).unwrap();
        let drop_threads = Arc::new(Mutex::new(Vec::<ThreadId>::new()));
        let control_thread = std::thread::current().id();
        let patch = Patch::new(
            PatchId::new(1).unwrap(),
            "Prior".to_owned(),
            provider.default_config().unwrap(),
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        );
        let mut initial_state = AppState::for_graph(
            registry.clone(),
            global_parameters(),
            GraphRevision::INITIAL,
        );
        initial_state
            .apply(AppEvent::InstallPatches(vec![patch]))
            .unwrap();
        let initial_parameters = StateProjector::for_graph(GraphRevision::INITIAL)
            .project(&initial_state)
            .unwrap()
            .2;
        let initial_preparers: Vec<Box<dyn InstrumentPreparer>> =
            vec![Box::new(DropRecordingPreparer {
                capability_id: CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap(),
                drop_threads: Arc::clone(&drop_threads),
            })];
        let initial_graph = PreparedGraphBuilder::new(&registry, &initial_preparers)
            .build(
                GraphRevision::INITIAL,
                initial_state.patches(),
                initial_parameters.clone(),
                audio_config.sample_rate(),
                audio_config.render_capacity_frames(),
            )
            .unwrap();
        let audio_boundary = LockFreeAudioBoundary::new(64, initial_parameters);
        let (audio_control, audio_handle) = audio_boundary.into_handles();
        let structural = LockFreeStructuralGraphBoundary::new(
            1,
            1,
            GraphHandoffStatus::with_active(GraphRevision::INITIAL),
        )
        .unwrap();
        let (structural_control, structural_audio) = structural.into_handles();
        let status_override = Arc::new(Mutex::new(None));
        let worker = DeterministicGraphPreparationWorker::new(
            registry.clone(),
            vec![Box::new(DropRecordingPreparer {
                capability_id: CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap(),
                drop_threads: Arc::clone(&drop_threads),
            })],
            audio_config,
        );
        let mut app_loop = AppLoop::new(
            initial_state,
            StateProjector::for_graph(GraphRevision::INITIAL),
            audio_control,
        )
        .unwrap();
        app_loop
            .configure_engine_selection(
                DescriptorDefaultConfigFactory::new(
                    registry.clone(),
                    vec![Box::new(BraidsCapability::new().unwrap())],
                ),
                worker,
                ObservedStructuralControl {
                    inner: structural_control,
                    blocked: Arc::new(AtomicBool::new(false)),
                    attempted_parameters: Arc::new(Mutex::new(Vec::new())),
                    status_override: Arc::clone(&status_override),
                },
                &initial_graph,
                audio_config,
            )
            .unwrap();
        let mut renderer = AudioRenderer::new(audio_handle, structural_audio, initial_graph);
        let note = MidiMessage::try_new(
            MidiChannel::new(0).unwrap(),
            MidiMessageKind::NoteOn,
            60,
            100,
        )
        .unwrap();
        assert_eq!(
            app_loop
                .dispatch_midi_from(note, EventSource::PhysicalMidi)
                .unwrap()
                .subscriber_count(),
            1
        );
        let mut before_audio = [0.0_f32; 128];
        renderer.render(&mut before_audio);
        assert!(before_audio.iter().any(|sample| sample.abs() > 0.01));

        let prior_capture = app_loop.capture_saved_session();
        let prior_tree = app_loop.current_state_tree();
        let prior_revision = app_loop.graph_revision();
        let target = prior_capture.clone();
        let target_revision = app_loop.next_structural_graph_revision().unwrap();
        let prepare_target = || {
            target
                .prepare_restore(
                    registry.clone(),
                    crate::synth::EffectCapabilityRegistry::default(),
                    &[Box::new(DropRecordingPreparer {
                        capability_id: CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap(),
                        drop_threads: Arc::clone(&drop_threads),
                    })],
                    &[],
                    target_revision,
                    audio_config.sample_rate(),
                    audio_config.render_capacity_frames(),
                )
                .unwrap()
                .into_replacement()
        };
        let (replacement, graph) = prepare_target();
        assert_eq!(
            app_loop
                .stage_session_replacement(replacement.clone(), graph)
                .unwrap(),
            GraphStageOutcome::Staged
        );
        let (_busy_replacement, busy_graph) = prepare_target();
        assert_eq!(
            app_loop.stage_session_replacement(replacement, busy_graph),
            Err(super::StructuralAdvanceError::SessionBusy)
        );
        assert_eq!(drop_threads.lock().unwrap().as_slice(), &[control_thread]);

        *status_override.lock().unwrap() = Some(GraphHandoffStatus::with_active(
            GraphRevision::new(9).unwrap(),
        ));
        assert_eq!(
            app_loop.advance_structural(),
            Err(super::StructuralAdvanceError::Publication(
                GraphPublicationFailure::StaleActiveRevision
            ))
        );
        assert!(!app_loop.session_replacement_pending());
        assert_eq!(app_loop.capture_saved_session(), prior_capture);
        assert_eq!(app_loop.current_state_tree(), prior_tree);
        assert_eq!(app_loop.graph_revision(), prior_revision);
        assert_eq!(renderer.active_revision(), prior_revision);
        assert_eq!(app_loop.owned_structural_graphs_on_control(), 0);
        assert_eq!(
            drop_threads.lock().unwrap().as_slice(),
            &[control_thread, control_thread]
        );

        *status_override.lock().unwrap() = None;
        assert_eq!(
            app_loop
                .dispatch_midi_from(note, EventSource::PhysicalMidi)
                .unwrap()
                .subscriber_count(),
            1
        );
        let mut after_audio = [0.0_f32; 128];
        renderer.render(&mut after_audio);
        assert_eq!(after_audio, before_audio);
        let callback_safety = crate::real_time::callback_safety::callback_safety_snapshot();
        assert_eq!(callback_safety.allocations(), 0);
        assert_eq!(callback_safety.destructions(), 0);
    }

    #[test]
    fn settings_action_reprojects_the_complete_system_surface_through_app_loop() {
        let (mut app_loop, _) = loop_with_observations();
        app_loop
            .dispatch_action(SemanticAction::OpenMidiSettings)
            .unwrap();

        let model = app_loop.current_semantic_model();
        assert_eq!(model.active_surface(), SurfaceId::MidiDeviceSettings);
        let surface = model.surface(SurfaceId::MidiDeviceSettings).unwrap();
        assert!(matches!(
            surface.summary(),
            SemanticSurfaceSummary::MidiDeviceSettings { rows, .. } if rows.is_empty()
        ));
        assert_eq!(
            app_loop
                .current_graphical_shell()
                .identity_header()
                .primary_label(),
            "SETTINGS · MIDI DEVICES"
        );
    }
}
