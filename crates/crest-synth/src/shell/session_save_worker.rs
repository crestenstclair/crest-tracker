use crate::control::{SavedSession, SavedSessionError};
use crate::real_time::WorkerShutdownError;
use crate::shell::{SessionFileFailure, SessionFilePort};
use core::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionContentToken(u64);

impl SessionContentToken {
    pub fn new(value: u64) -> Result<Self, SessionContentTokenError> {
        (value != 0)
            .then_some(Self(value))
            .ok_or(SessionContentTokenError)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("session content token must be non-zero")]
pub struct SessionContentTokenError;

#[derive(Clone, Debug, PartialEq)]
pub struct SessionSaveRequest {
    token: SessionContentToken,
    destination: PathBuf,
    capture: Box<SavedSession>,
}

impl SessionSaveRequest {
    pub fn new(token: SessionContentToken, destination: PathBuf, capture: SavedSession) -> Self {
        Self {
            token,
            destination,
            capture: Box::new(capture),
        }
    }

    pub const fn token(&self) -> SessionContentToken {
        self.token
    }

    pub fn destination(&self) -> &Path {
        &self.destination
    }

    pub const fn capture(&self) -> &SavedSession {
        &self.capture
    }

    fn into_parts(self) -> (SessionContentToken, PathBuf, SavedSession) {
        (self.token, self.destination, *self.capture)
    }
}

pub enum SessionSaveResult {
    Saved {
        token: SessionContentToken,
        destination: PathBuf,
        written: Box<SavedSession>,
    },
    Failed {
        token: SessionContentToken,
        destination: PathBuf,
        failure: SessionSaveFailure,
    },
}

impl SessionSaveResult {
    pub const fn token(&self) -> SessionContentToken {
        match self {
            Self::Saved { token, .. } | Self::Failed { token, .. } => *token,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SessionSaveFailure {
    #[error("active session encoding failed: {0}")]
    Encode(SavedSessionError),
    #[error("atomic session write failed: {0}")]
    File(SessionFileFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionSaveWorkerBusyReason {
    OutstandingRequest,
    WorkerUnavailable,
    Shutdown,
}

pub struct SessionSaveWorkerBusy {
    reason: SessionSaveWorkerBusyReason,
    request: Box<SessionSaveRequest>,
}

impl SessionSaveWorkerBusy {
    pub fn new(reason: SessionSaveWorkerBusyReason, request: SessionSaveRequest) -> Self {
        Self {
            reason,
            request: Box::new(request),
        }
    }

    pub const fn reason(&self) -> SessionSaveWorkerBusyReason {
        self.reason
    }

    pub fn into_request(self) -> SessionSaveRequest {
        *self.request
    }
}

impl fmt::Debug for SessionSaveWorkerBusy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionSaveWorkerBusy")
            .field("reason", &self.reason)
            .field("token", &self.request.token())
            .finish()
    }
}

pub trait SessionSaveWorker: Send {
    fn try_submit(&mut self, request: SessionSaveRequest) -> Result<(), SessionSaveWorkerBusy>;
    fn try_poll(&mut self) -> Option<SessionSaveResult>;
    fn shutdown_on_control(&mut self) -> Result<(), WorkerShutdownError>;
}

pub struct ThreadedSessionSaveWorker {
    request_sender: Option<SyncSender<SessionSaveRequest>>,
    result_receiver: Receiver<SessionSaveResult>,
    outstanding: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl ThreadedSessionSaveWorker {
    pub fn new(files: Arc<dyn SessionFilePort>) -> Result<Self, ThreadedSessionSaveWorkerError> {
        let (request_sender, request_receiver) = mpsc::sync_channel(1);
        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        let outstanding = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let worker = thread::Builder::new()
            .name("crest-session-save".to_owned())
            .spawn(move || {
                worker_main(files, request_receiver, result_sender, &worker_shutdown);
            })
            .map_err(|_| ThreadedSessionSaveWorkerError::ThreadStartFailed)?;
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

impl SessionSaveWorker for ThreadedSessionSaveWorker {
    fn try_submit(&mut self, request: SessionSaveRequest) -> Result<(), SessionSaveWorkerBusy> {
        if self.shutdown.load(Ordering::Acquire) {
            return Err(SessionSaveWorkerBusy::new(
                SessionSaveWorkerBusyReason::Shutdown,
                request,
            ));
        }
        if self
            .outstanding
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(SessionSaveWorkerBusy::new(
                SessionSaveWorkerBusyReason::OutstandingRequest,
                request,
            ));
        }
        let Some(sender) = &self.request_sender else {
            self.outstanding.store(false, Ordering::Release);
            return Err(SessionSaveWorkerBusy::new(
                SessionSaveWorkerBusyReason::Shutdown,
                request,
            ));
        };
        match sender.try_send(request) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(request)) => {
                self.outstanding.store(false, Ordering::Release);
                Err(SessionSaveWorkerBusy::new(
                    SessionSaveWorkerBusyReason::OutstandingRequest,
                    request,
                ))
            }
            Err(TrySendError::Disconnected(request)) => {
                self.outstanding.store(false, Ordering::Release);
                Err(SessionSaveWorkerBusy::new(
                    SessionSaveWorkerBusyReason::WorkerUnavailable,
                    request,
                ))
            }
        }
    }

    fn try_poll(&mut self) -> Option<SessionSaveResult> {
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

impl Drop for ThreadedSessionSaveWorker {
    fn drop(&mut self) {
        let _ = self.shutdown_inner();
    }
}

fn worker_main(
    files: Arc<dyn SessionFilePort>,
    request_receiver: Receiver<SessionSaveRequest>,
    result_sender: SyncSender<SessionSaveResult>,
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
        let (token, destination, capture) = request.into_parts();
        let mut result = match capture.to_json() {
            Ok(json) => match files.write_atomic(&destination, json.as_bytes()) {
                Ok(()) => SessionSaveResult::Saved {
                    token,
                    destination,
                    written: Box::new(capture),
                },
                Err(error) => SessionSaveResult::Failed {
                    token,
                    destination,
                    failure: SessionSaveFailure::File(error),
                },
            },
            Err(error) => SessionSaveResult::Failed {
                token,
                destination,
                failure: SessionSaveFailure::Encode(error),
            },
        };
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadedSessionSaveWorkerError {
    ThreadStartFailed,
}

impl fmt::Display for ThreadedSessionSaveWorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the session save worker thread could not start")
    }
}

impl std::error::Error for ThreadedSessionSaveWorkerError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::braids_capability::BraidsCapability;
    use crate::control::{AppEvent, AppState};
    use crate::kernel::{MidiChannel, PatchId};
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::patch_output::PatchOutput;
    use crate::shell::{SessionFileFailureKind, StandardSessionFileSystem};
    use crate::synth::{CapabilityRegistry, InstrumentCapabilityProvider, Patch};
    use std::fs;
    use std::time::{Duration, Instant};

    fn capture() -> SavedSession {
        let provider = BraidsCapability::new().unwrap();
        let registry = CapabilityRegistry::new(vec![provider.descriptor()]).unwrap();
        let mut state = AppState::new(registry, GlobalParameters::new(0.0).unwrap());
        state
            .apply(AppEvent::InstallPatches(vec![Patch::new(
                PatchId::new(1).unwrap(),
                "Save".to_owned(),
                provider.default_config().unwrap(),
                MidiChannel::new(0).unwrap(),
                PatchOutput::default(),
            )]))
            .unwrap();
        SavedSession::capture(&state)
    }

    fn poll(worker: &mut ThreadedSessionSaveWorker) -> SessionSaveResult {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(result) = worker.try_poll() {
                return result;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    #[test]
    fn bounded_worker_writes_the_immutable_capture_and_shuts_down_cleanly() {
        let directory = std::env::temp_dir().join(format!(
            "crest-save-worker-{}-{}",
            std::process::id(),
            SessionContentToken::new(77).unwrap().value()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir(&directory).unwrap();
        let destination = directory.join("saved.crest");
        let saved = capture();
        let mut worker =
            ThreadedSessionSaveWorker::new(Arc::new(StandardSessionFileSystem)).unwrap();
        worker
            .try_submit(SessionSaveRequest::new(
                SessionContentToken::new(1).unwrap(),
                destination.clone(),
                saved.clone(),
            ))
            .unwrap();
        assert_eq!(
            worker
                .try_submit(SessionSaveRequest::new(
                    SessionContentToken::new(2).unwrap(),
                    destination.clone(),
                    saved.clone(),
                ))
                .unwrap_err()
                .reason(),
            SessionSaveWorkerBusyReason::OutstandingRequest
        );
        match poll(&mut worker) {
            SessionSaveResult::Saved { written, .. } => assert_eq!(*written, saved),
            SessionSaveResult::Failed { failure, .. } => panic!("save failed: {failure}"),
        }
        assert_eq!(
            SavedSession::from_json(&fs::read_to_string(&destination).unwrap(), &{
                let provider = BraidsCapability::new().unwrap();
                CapabilityRegistry::new(vec![provider.descriptor()]).unwrap()
            })
            .unwrap(),
            saved
        );
        worker.shutdown_on_control().unwrap();
        assert!(!worker.has_outstanding_request());
        fs::remove_file(destination).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    struct FailingFiles;

    impl SessionFilePort for FailingFiles {
        fn read(&self, _path: &Path) -> Result<Vec<u8>, SessionFileFailure> {
            unreachable!()
        }

        fn write_atomic(&self, _path: &Path, _bytes: &[u8]) -> Result<(), SessionFileFailure> {
            Err(SessionFileFailure::new_for_test(
                SessionFileFailureKind::AtomicReplace,
            ))
        }
    }

    #[test]
    fn filesystem_failure_returns_the_exact_capture_and_destination_context() {
        let destination = PathBuf::from("failure.crest");
        let mut worker = ThreadedSessionSaveWorker::new(Arc::new(FailingFiles)).unwrap();
        worker
            .try_submit(SessionSaveRequest::new(
                SessionContentToken::new(1).unwrap(),
                destination.clone(),
                capture(),
            ))
            .unwrap();
        let SessionSaveResult::Failed {
            destination: failed_destination,
            failure,
            ..
        } = poll(&mut worker)
        else {
            panic!("failing filesystem unexpectedly saved")
        };
        assert_eq!(failed_destination, destination);
        assert!(matches!(failure, SessionSaveFailure::File(_)));
        worker.shutdown_on_control().unwrap();
    }
}
