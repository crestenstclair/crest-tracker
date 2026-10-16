use crate::control::{
    SavedSession, SessionCandidateFailure, SessionCandidateRequest, SessionCandidateResult,
    SessionCandidateSource, SessionCandidateWorker, SessionCandidateWorkerBusy,
    SessionCandidateWorkerBusyReason,
};
use crate::real_time::WorkerShutdownError;
use crate::shell::audio_output::AudioDeviceConfig;
use crate::synth::{
    CapabilityRegistry, EffectCapabilityRegistry, EffectPreparer, InstrumentPreparer,
};
use core::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Capacity-one production worker for Open/New decode, migration, validation,
/// capability/asset resolution, projection, and complete graph preparation.
pub struct ThreadedSessionCandidateWorker {
    request_sender: Option<SyncSender<SessionCandidateRequest>>,
    result_receiver: Receiver<SessionCandidateResult>,
    outstanding: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl ThreadedSessionCandidateWorker {
    pub fn new(
        capabilities: CapabilityRegistry,
        instrument_preparers: Vec<Box<dyn InstrumentPreparer>>,
        effects: EffectCapabilityRegistry,
        effect_preparers: Vec<Box<dyn EffectPreparer>>,
        audio_config: AudioDeviceConfig,
    ) -> Result<Self, ThreadedSessionCandidateWorkerError> {
        let (request_sender, request_receiver) = mpsc::sync_channel(1);
        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        let outstanding = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let resources = WorkerResources {
            capabilities,
            instrument_preparers,
            effects,
            effect_preparers,
            audio_config,
        };
        let worker = thread::Builder::new()
            .name("crest-session-candidate".to_owned())
            .spawn(move || {
                worker_main(resources, request_receiver, result_sender, &worker_shutdown);
            })
            .map_err(|_| ThreadedSessionCandidateWorkerError::ThreadStartFailed)?;
        Ok(Self {
            request_sender: Some(request_sender),
            result_receiver,
            outstanding,
            shutdown,
            worker: Some(worker),
        })
    }

    pub fn has_outstanding_request(&self) -> bool {
        self.outstanding.load(Ordering::Acquire)
    }

    fn shutdown_inner(&mut self) -> Result<(), WorkerShutdownError> {
        self.shutdown.store(true, Ordering::Release);
        self.request_sender.take();
        let join_result = self.worker.take().map(JoinHandle::join);
        while self.result_receiver.try_recv().is_ok() {}
        self.outstanding.store(false, Ordering::Release);
        match join_result {
            Some(Err(_)) => Err(WorkerShutdownError::ThreadPanicked),
            Some(Ok(())) | None => Ok(()),
        }
    }
}

impl SessionCandidateWorker for ThreadedSessionCandidateWorker {
    fn try_submit(
        &mut self,
        request: SessionCandidateRequest,
    ) -> Result<(), SessionCandidateWorkerBusy> {
        if self.shutdown.load(Ordering::Acquire) {
            return Err(SessionCandidateWorkerBusy::new(
                SessionCandidateWorkerBusyReason::Shutdown,
                request,
            ));
        }
        if self
            .outstanding
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(SessionCandidateWorkerBusy::new(
                SessionCandidateWorkerBusyReason::OutstandingRequest,
                request,
            ));
        }
        let Some(sender) = &self.request_sender else {
            self.outstanding.store(false, Ordering::Release);
            return Err(SessionCandidateWorkerBusy::new(
                SessionCandidateWorkerBusyReason::Shutdown,
                request,
            ));
        };
        match sender.try_send(request) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(request)) => {
                self.outstanding.store(false, Ordering::Release);
                Err(SessionCandidateWorkerBusy::new(
                    SessionCandidateWorkerBusyReason::OutstandingRequest,
                    request,
                ))
            }
            Err(TrySendError::Disconnected(request)) => {
                self.outstanding.store(false, Ordering::Release);
                Err(SessionCandidateWorkerBusy::new(
                    SessionCandidateWorkerBusyReason::WorkerUnavailable,
                    request,
                ))
            }
        }
    }

    fn try_poll(&mut self) -> Option<SessionCandidateResult> {
        match self.result_receiver.try_recv() {
            Ok(result) => {
                self.outstanding.store(false, Ordering::Release);
                Some(result)
            }
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    fn shutdown_on_control(&mut self) -> Result<(), WorkerShutdownError> {
        self.shutdown_inner()
    }
}

impl Drop for ThreadedSessionCandidateWorker {
    fn drop(&mut self) {
        let _ = self.shutdown_inner();
    }
}

struct WorkerResources {
    capabilities: CapabilityRegistry,
    instrument_preparers: Vec<Box<dyn InstrumentPreparer>>,
    effects: EffectCapabilityRegistry,
    effect_preparers: Vec<Box<dyn EffectPreparer>>,
    audio_config: AudioDeviceConfig,
}

fn worker_main(
    resources: WorkerResources,
    request_receiver: Receiver<SessionCandidateRequest>,
    result_sender: SyncSender<SessionCandidateResult>,
    shutdown: &AtomicBool,
) {
    while !shutdown.load(Ordering::Acquire) {
        let request = match request_receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(request) => request,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if shutdown.load(Ordering::Acquire) {
            drop(request);
            break;
        }
        let mut result = prepare_candidate(&resources, request);
        loop {
            match result_sender.try_send(result) {
                Ok(()) => break,
                Err(TrySendError::Full(returned)) => {
                    result = returned;
                    if shutdown.load(Ordering::Acquire) {
                        drop(result);
                        return;
                    }
                    thread::yield_now();
                }
                Err(TrySendError::Disconnected(returned)) => {
                    drop(returned);
                    return;
                }
            }
        }
    }
}

fn prepare_candidate(
    resources: &WorkerResources,
    request: SessionCandidateRequest,
) -> SessionCandidateResult {
    let (token, source, target_graph_revision) = request.into_parts();
    let saved = match source {
        SessionCandidateSource::Default(saved) => saved,
        SessionCandidateSource::OpenedBytes(bytes) => {
            let json = match String::from_utf8(bytes) {
                Ok(json) => json,
                Err(_) => {
                    return SessionCandidateResult::Failed {
                        token,
                        failure: SessionCandidateFailure::DecodeEncoding,
                    }
                }
            };
            match SavedSession::from_json(&json, &resources.capabilities) {
                Ok(saved) => Box::new(saved),
                Err(error) => {
                    return SessionCandidateResult::Failed {
                        token,
                        failure: SessionCandidateFailure::Saved(error),
                    }
                }
            }
        }
    };
    match saved.prepare_restore(
        resources.capabilities.clone(),
        resources.effects.clone(),
        &resources.instrument_preparers,
        &resources.effect_preparers,
        target_graph_revision,
        resources.audio_config.sample_rate(),
        resources.audio_config.render_capacity_frames(),
    ) {
        Ok(prepared) => SessionCandidateResult::Prepared {
            token,
            saved,
            prepared: Box::new(prepared),
        },
        Err(error) => SessionCandidateResult::Failed {
            token,
            failure: SessionCandidateFailure::Restore(error),
        },
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadedSessionCandidateWorkerError {
    ThreadStartFailed,
}

impl fmt::Display for ThreadedSessionCandidateWorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the session candidate worker thread could not start")
    }
}

impl std::error::Error for ThreadedSessionCandidateWorkerError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::braids_capability::BraidsCapability;
    use crate::adapter::braids_preparer::BraidsPreparer;
    use crate::control::{AppEvent, AppState, SessionCandidateFailureKind, SessionCandidateToken};
    use crate::kernel::{MidiChannel, PatchId};
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::GraphRevision;
    use crate::shell::audio_output::{AudioDeviceConfig, AudioSampleFormat};
    use crate::synth::{
        CapabilityId, CapabilityRegistry, InstrumentCapabilityProvider, InstrumentPreparationError,
        InstrumentPreparer, Patch, PreparedInstrument,
    };
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    fn setup() -> (CapabilityRegistry, SavedSession) {
        let provider = BraidsCapability::new().unwrap();
        let registry = CapabilityRegistry::new(vec![provider.descriptor()]).unwrap();
        let mut state = AppState::new(registry.clone(), GlobalParameters::new(0.0).unwrap());
        state
            .apply(AppEvent::InstallPatches(vec![Patch::new(
                PatchId::new(1).unwrap(),
                "Worker".to_owned(),
                provider.default_config().unwrap(),
                MidiChannel::new(0).unwrap(),
                PatchOutput::default(),
            )]))
            .unwrap();
        (registry, SavedSession::capture(&state))
    }

    fn worker(registry: CapabilityRegistry) -> ThreadedSessionCandidateWorker {
        ThreadedSessionCandidateWorker::new(
            registry,
            vec![Box::new(BraidsPreparer::new().unwrap())],
            EffectCapabilityRegistry::default(),
            vec![],
            AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 64).unwrap(),
        )
        .unwrap()
    }

    fn poll(worker: &mut ThreadedSessionCandidateWorker) -> SessionCandidateResult {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(result) = worker.try_poll() {
                return result;
            }
            assert!(Instant::now() < deadline, "worker result timed out");
            std::thread::yield_now();
        }
    }

    #[test]
    fn default_and_opened_bytes_share_one_capacity_one_preparation_path() {
        let (registry, saved) = setup();
        let mut worker = worker(registry);
        let first = SessionCandidateRequest::new(
            SessionCandidateToken::new(1).unwrap(),
            SessionCandidateSource::Default(Box::new(saved.clone())),
            GraphRevision::new(2).unwrap(),
        );
        worker.try_submit(first).unwrap();
        let busy = worker
            .try_submit(SessionCandidateRequest::new(
                SessionCandidateToken::new(2).unwrap(),
                SessionCandidateSource::OpenedBytes(saved.to_json().unwrap().into_bytes()),
                GraphRevision::new(3).unwrap(),
            ))
            .unwrap_err();
        assert_eq!(
            busy.reason(),
            SessionCandidateWorkerBusyReason::OutstandingRequest
        );
        match poll(&mut worker) {
            SessionCandidateResult::Prepared {
                token,
                saved: prepared_saved,
                prepared,
            } => {
                assert_eq!(token.value(), 1);
                assert_eq!(*prepared_saved, saved);
                assert_eq!(prepared.graph().revision(), GraphRevision::new(2).unwrap());
            }
            SessionCandidateResult::Failed { failure, .. } => {
                panic!("valid default failed: {failure}")
            }
        }

        worker
            .try_submit(SessionCandidateRequest::new(
                SessionCandidateToken::new(3).unwrap(),
                SessionCandidateSource::OpenedBytes(saved.to_json().unwrap().into_bytes()),
                GraphRevision::new(3).unwrap(),
            ))
            .unwrap();
        assert!(matches!(
            poll(&mut worker),
            SessionCandidateResult::Prepared { .. }
        ));
        worker.shutdown_on_control().unwrap();
    }

    #[test]
    fn decode_version_shape_and_capability_failures_remain_distinct() {
        let (registry, saved) = setup();
        let mut worker = worker(registry);
        let mut invalid_shape: serde_json::Value =
            serde_json::from_str(&saved.to_json().unwrap()).unwrap();
        invalid_shape["returns"] = serde_json::Value::Array(Vec::new());
        let mut unknown_capability: serde_json::Value =
            serde_json::from_str(&saved.to_json().unwrap()).unwrap();
        unknown_capability["patches"][0]["instrument"]["capabilityId"] =
            serde_json::Value::String("instrument.missing".to_owned());
        let cases = vec![
            (vec![0xff], SessionCandidateFailureKind::Decode),
            (
                br#"{"version":99}"#.to_vec(),
                SessionCandidateFailureKind::Version,
            ),
            (
                serde_json::to_vec(&invalid_shape).unwrap(),
                SessionCandidateFailureKind::Shape,
            ),
            (
                serde_json::to_vec(&unknown_capability).unwrap(),
                SessionCandidateFailureKind::Capability,
            ),
        ];
        for (index, (bytes, expected)) in cases.into_iter().enumerate() {
            worker
                .try_submit(SessionCandidateRequest::new(
                    SessionCandidateToken::new(index as u64 + 1).unwrap(),
                    SessionCandidateSource::OpenedBytes(bytes),
                    GraphRevision::new(index as u64 + 2).unwrap(),
                ))
                .unwrap();
            let SessionCandidateResult::Failed { failure, .. } = poll(&mut worker) else {
                panic!("invalid candidate unexpectedly prepared")
            };
            assert_eq!(failure.kind(), expected);
        }
        worker.shutdown_on_control().unwrap();
    }

    struct RecordingPreparer {
        inner: BraidsPreparer,
        thread: Arc<Mutex<Option<std::thread::ThreadId>>>,
        failure: Option<InstrumentPreparationError>,
    }

    impl InstrumentPreparer for RecordingPreparer {
        fn capability_id(&self) -> &CapabilityId {
            self.inner.capability_id()
        }

        fn prepare(
            &self,
            patch: &Patch,
            sample_rate: f32,
            max_frames: usize,
        ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
            *self.thread.lock().unwrap() = Some(std::thread::current().id());
            if let Some(failure) = self.failure {
                Err(failure)
            } else {
                self.inner.prepare(patch, sample_rate, max_frames)
            }
        }
    }

    fn recording_worker(
        registry: CapabilityRegistry,
        thread: Arc<Mutex<Option<std::thread::ThreadId>>>,
        failure: Option<InstrumentPreparationError>,
    ) -> ThreadedSessionCandidateWorker {
        ThreadedSessionCandidateWorker::new(
            registry,
            vec![Box::new(RecordingPreparer {
                inner: BraidsPreparer::new().unwrap(),
                thread,
                failure,
            })],
            EffectCapabilityRegistry::default(),
            vec![],
            AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 64).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn preparation_runs_off_caller_and_keeps_asset_vs_generic_failure_typed() {
        let caller = std::thread::current().id();
        for (index, (failure, expected)) in [
            (
                InstrumentPreparationError::AssetUnavailable {
                    patch_id: PatchId::new(1).unwrap(),
                },
                SessionCandidateFailureKind::Asset,
            ),
            (
                InstrumentPreparationError::PreparationFailed {
                    patch_id: PatchId::new(1).unwrap(),
                },
                SessionCandidateFailureKind::Preparation,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let (registry, saved) = setup();
            let thread = Arc::new(Mutex::new(None));
            let mut worker = recording_worker(registry, Arc::clone(&thread), Some(failure));
            worker
                .try_submit(SessionCandidateRequest::new(
                    SessionCandidateToken::new(index as u64 + 1).unwrap(),
                    SessionCandidateSource::Default(Box::new(saved)),
                    GraphRevision::new(index as u64 + 2).unwrap(),
                ))
                .unwrap();
            let SessionCandidateResult::Failed { failure, .. } = poll(&mut worker) else {
                panic!("failing preparer unexpectedly succeeded")
            };
            assert_eq!(failure.kind(), expected);
            assert_ne!(thread.lock().unwrap().as_ref(), Some(&caller));
            worker.shutdown_on_control().unwrap();
        }
    }
}
