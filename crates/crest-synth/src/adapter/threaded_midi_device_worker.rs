use crate::control::{
    ActiveMidiInput, MidiDeviceWorker, MidiDeviceWorkerBusy, MidiDeviceWorkerBusyReason,
    MidiDeviceWorkerCommand, MidiDeviceWorkerResult, MidiDeviceWorkerShutdownError,
    MidiInputDevicePort, MidiInputPreferencePort,
};
use core::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const MIDI_DEVICE_WORKER_COMMAND_CAPACITY: usize = 8;
pub const MIDI_DEVICE_WORKER_RESULT_CAPACITY: usize = 8;

/// Bounded production worker for enumeration, connection, retirement, and
/// preference I/O. Every control-facing operation is nonblocking.
pub struct ThreadedMidiDeviceWorker {
    command_sender: Option<SyncSender<MidiDeviceWorkerCommand>>,
    result_receiver: Receiver<MidiDeviceWorkerResult>,
    scan_outstanding: Arc<AtomicBool>,
    connect_outstanding: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    shutdown_retirements: Arc<Mutex<Vec<ActiveMidiInput>>>,
    shutdown_retirement_failed: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl ThreadedMidiDeviceWorker {
    pub fn new(
        device: Box<dyn MidiInputDevicePort>,
        preference: Box<dyn MidiInputPreferencePort>,
    ) -> Result<Self, ThreadedMidiDeviceWorkerError> {
        let (command_sender, command_receiver) =
            mpsc::sync_channel(MIDI_DEVICE_WORKER_COMMAND_CAPACITY);
        let (result_sender, result_receiver) =
            mpsc::sync_channel(MIDI_DEVICE_WORKER_RESULT_CAPACITY);
        let scan_outstanding = Arc::new(AtomicBool::new(false));
        let connect_outstanding = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let shutdown_retirements = Arc::new(Mutex::new(Vec::new()));
        let worker_shutdown_retirements = Arc::clone(&shutdown_retirements);
        let shutdown_retirement_failed = Arc::new(AtomicBool::new(false));
        let worker_shutdown_retirement_failed = Arc::clone(&shutdown_retirement_failed);
        let worker = thread::Builder::new()
            .name("crest-midi-device".to_owned())
            .spawn(move || {
                worker_main(
                    device,
                    preference,
                    command_receiver,
                    result_sender,
                    &worker_shutdown,
                    &worker_shutdown_retirements,
                    &worker_shutdown_retirement_failed,
                );
            })
            .map_err(|_| ThreadedMidiDeviceWorkerError::ThreadStartFailed)?;
        Ok(Self {
            command_sender: Some(command_sender),
            result_receiver,
            scan_outstanding,
            connect_outstanding,
            shutdown,
            shutdown_retirements,
            shutdown_retirement_failed,
            worker: Some(worker),
        })
    }

    pub fn scan_outstanding(&self) -> bool {
        self.scan_outstanding.load(Ordering::Acquire)
    }

    pub fn connect_outstanding(&self) -> bool {
        self.connect_outstanding.load(Ordering::Acquire)
    }

    fn shutdown_inner(
        &mut self,
        mut retirements: Vec<ActiveMidiInput>,
    ) -> Result<(), MidiDeviceWorkerShutdownError> {
        retirements.extend(self.reclaim_prepared_handles());
        self.shutdown_retirements
            .lock()
            .expect("a MIDI shutdown retirement owner must not be poisoned")
            .extend(retirements);
        self.shutdown.store(true, Ordering::Release);
        self.command_sender.take();
        let joined = self.worker.take().map(JoinHandle::join);
        while self.result_receiver.try_recv().is_ok() {}
        self.scan_outstanding.store(false, Ordering::Release);
        self.connect_outstanding.store(false, Ordering::Release);
        match joined {
            Some(Err(_)) => Err(MidiDeviceWorkerShutdownError::ThreadPanicked),
            Some(Ok(())) | None if self.shutdown_retirement_failed.load(Ordering::Acquire) => {
                Err(MidiDeviceWorkerShutdownError::RetirementFailed)
            }
            Some(Ok(())) | None => Ok(()),
        }
    }

    fn reclaim_prepared_handles(&mut self) -> Vec<ActiveMidiInput> {
        let mut prepared = Vec::new();
        while self.connect_outstanding.load(Ordering::Acquire) {
            match self.result_receiver.recv_timeout(Duration::from_millis(10)) {
                Ok(result) => {
                    if result.is_connect() {
                        self.connect_outstanding.store(false, Ordering::Release);
                    }
                    if result.is_scan() {
                        self.scan_outstanding.store(false, Ordering::Release);
                    }
                    if let MidiDeviceWorkerResult::ConnectionPrepared { active, .. } = result {
                        prepared.push(active);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        while let Ok(result) = self.result_receiver.try_recv() {
            if let MidiDeviceWorkerResult::ConnectionPrepared { active, .. } = result {
                prepared.push(active);
            }
        }
        prepared
    }
}

impl MidiDeviceWorker for ThreadedMidiDeviceWorker {
    fn try_submit(&mut self, command: MidiDeviceWorkerCommand) -> Result<(), MidiDeviceWorkerBusy> {
        if self.shutdown.load(Ordering::Acquire) {
            return Err(MidiDeviceWorkerBusy::new(
                MidiDeviceWorkerBusyReason::Shutdown,
                command,
            ));
        }
        let outstanding = if command.is_scan() {
            Some((
                &self.scan_outstanding,
                MidiDeviceWorkerBusyReason::ScanOutstanding,
            ))
        } else if command.is_connect() {
            Some((
                &self.connect_outstanding,
                MidiDeviceWorkerBusyReason::ConnectOutstanding,
            ))
        } else {
            None
        };
        if let Some((flag, reason)) = outstanding {
            if flag
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return Err(MidiDeviceWorkerBusy::new(reason, command));
            }
        }
        let Some(sender) = &self.command_sender else {
            clear_outstanding(&command, &self.scan_outstanding, &self.connect_outstanding);
            return Err(MidiDeviceWorkerBusy::new(
                MidiDeviceWorkerBusyReason::Shutdown,
                command,
            ));
        };
        match sender.try_send(command) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(command)) => {
                clear_outstanding(&command, &self.scan_outstanding, &self.connect_outstanding);
                Err(MidiDeviceWorkerBusy::new(
                    MidiDeviceWorkerBusyReason::QueueFull,
                    command,
                ))
            }
            Err(TrySendError::Disconnected(command)) => {
                clear_outstanding(&command, &self.scan_outstanding, &self.connect_outstanding);
                Err(MidiDeviceWorkerBusy::new(
                    MidiDeviceWorkerBusyReason::WorkerUnavailable,
                    command,
                ))
            }
        }
    }

    fn try_poll(&mut self) -> Option<MidiDeviceWorkerResult> {
        match self.result_receiver.try_recv() {
            Ok(result) => {
                if result.is_scan() {
                    self.scan_outstanding.store(false, Ordering::Release);
                }
                if result.is_connect() {
                    self.connect_outstanding.store(false, Ordering::Release);
                }
                Some(result)
            }
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    fn shutdown_with_retirements_on_control(
        &mut self,
        retirements: Vec<ActiveMidiInput>,
    ) -> Result<(), MidiDeviceWorkerShutdownError> {
        self.shutdown_inner(retirements)
    }
}

impl Drop for ThreadedMidiDeviceWorker {
    fn drop(&mut self) {
        let _ = self.shutdown_inner(Vec::new());
    }
}

fn clear_outstanding(command: &MidiDeviceWorkerCommand, scan: &AtomicBool, connect: &AtomicBool) {
    if command.is_scan() {
        scan.store(false, Ordering::Release);
    }
    if command.is_connect() {
        connect.store(false, Ordering::Release);
    }
}

fn worker_main(
    mut device: Box<dyn MidiInputDevicePort>,
    mut preference: Box<dyn MidiInputPreferencePort>,
    command_receiver: Receiver<MidiDeviceWorkerCommand>,
    result_sender: SyncSender<MidiDeviceWorkerResult>,
    shutdown: &AtomicBool,
    shutdown_retirements: &Mutex<Vec<ActiveMidiInput>>,
    shutdown_retirement_failed: &AtomicBool,
) {
    while !shutdown.load(Ordering::Acquire) {
        let command = match command_receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(command) => command,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let mut result = execute(&mut *device, &mut *preference, command);
        loop {
            match result_sender.try_send(result) {
                Ok(()) => break,
                Err(TrySendError::Full(returned)) => {
                    result = returned;
                    if shutdown.load(Ordering::Acquire) {
                        cleanup_result(&mut *device, result, shutdown_retirement_failed);
                        drain_commands(&mut *device, &command_receiver, shutdown_retirement_failed);
                        drain_shutdown_retirements(
                            &mut *device,
                            shutdown_retirements,
                            shutdown_retirement_failed,
                        );
                        return;
                    }
                    thread::yield_now();
                }
                Err(TrySendError::Disconnected(returned)) => {
                    cleanup_result(&mut *device, returned, shutdown_retirement_failed);
                    drain_commands(&mut *device, &command_receiver, shutdown_retirement_failed);
                    drain_shutdown_retirements(
                        &mut *device,
                        shutdown_retirements,
                        shutdown_retirement_failed,
                    );
                    return;
                }
            }
        }
    }
    drain_commands(&mut *device, &command_receiver, shutdown_retirement_failed);
    drain_shutdown_retirements(
        &mut *device,
        shutdown_retirements,
        shutdown_retirement_failed,
    );
}

fn execute(
    device: &mut dyn MidiInputDevicePort,
    preference_port: &mut dyn MidiInputPreferencePort,
    command: MidiDeviceWorkerCommand,
) -> MidiDeviceWorkerResult {
    match command {
        MidiDeviceWorkerCommand::Scan { scan_id } => match device.enumerate() {
            Ok(descriptors) => MidiDeviceWorkerResult::ScanSucceeded {
                scan_id,
                descriptors,
            },
            Err(failure) => MidiDeviceWorkerResult::ScanFailed { scan_id, failure },
        },
        MidiDeviceWorkerCommand::Connect { request, ingress } => {
            match device.connect(request.clone(), ingress) {
                Ok(active) => MidiDeviceWorkerResult::ConnectionPrepared { request, active },
                Err(failure) => MidiDeviceWorkerResult::ConnectionFailed { request, failure },
            }
        }
        MidiDeviceWorkerCommand::Retire { active } => {
            let identity = active.identity().clone();
            let request_id = active.request_id();
            let revision = active.revision();
            let failure = device.disconnect(active).err();
            MidiDeviceWorkerResult::Retired {
                identity,
                request_id,
                revision,
                failure,
            }
        }
        MidiDeviceWorkerCommand::LoadPreference => match preference_port.load() {
            Ok(preference) => MidiDeviceWorkerResult::PreferenceLoaded {
                preference,
                failure: None,
            },
            Err(failure) => MidiDeviceWorkerResult::PreferenceLoaded {
                preference: None,
                failure: Some(failure),
            },
        },
        MidiDeviceWorkerCommand::StorePreference { preference } => {
            let failure = preference_port.store(&preference).err();
            MidiDeviceWorkerResult::PreferenceStored {
                preference,
                failure,
            }
        }
    }
}

fn cleanup_result(
    device: &mut dyn MidiInputDevicePort,
    result: MidiDeviceWorkerResult,
    retirement_failed: &AtomicBool,
) {
    if let MidiDeviceWorkerResult::ConnectionPrepared { active, .. } = result {
        if device.disconnect(active).is_err() {
            retirement_failed.store(true, Ordering::Release);
        }
    }
}

fn drain_commands(
    device: &mut dyn MidiInputDevicePort,
    receiver: &Receiver<MidiDeviceWorkerCommand>,
    retirement_failed: &AtomicBool,
) {
    while let Ok(command) = receiver.try_recv() {
        if let MidiDeviceWorkerCommand::Retire { active } = command {
            if device.disconnect(active).is_err() {
                retirement_failed.store(true, Ordering::Release);
            }
        }
    }
}

fn drain_shutdown_retirements(
    device: &mut dyn MidiInputDevicePort,
    retirements: &Mutex<Vec<ActiveMidiInput>>,
    retirement_failed: &AtomicBool,
) {
    for active in retirements
        .lock()
        .expect("a MIDI shutdown retirement owner must not be poisoned")
        .drain(..)
    {
        if device.disconnect(active).is_err() {
            retirement_failed.store(true, Ordering::Release);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadedMidiDeviceWorkerError {
    ThreadStartFailed,
}

impl fmt::Display for ThreadedMidiDeviceWorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the MIDI device worker thread could not start")
    }
}

impl std::error::Error for ThreadedMidiDeviceWorkerError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{
        ActiveMidiInput, ConnectMidiInput, MidiConnectionRequestId, MidiConnectionRevision,
        MidiDeviceFailure, MidiInputDescriptor, MidiInputDeviceId, MidiInputPreference,
        MidiInputScanId, MidiPreferredInput, PhysicalMidiIngress,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    fn id(value: &str) -> MidiInputDeviceId {
        MidiInputDeviceId::new("midir-v1", value).unwrap()
    }

    fn request(value: &str, number: u64) -> ConnectMidiInput {
        ConnectMidiInput::new(
            id(value),
            MidiConnectionRequestId::new(number).unwrap(),
            MidiConnectionRevision::new(number).unwrap(),
        )
    }

    struct FakeDevicePort {
        live: Arc<AtomicUsize>,
    }

    struct BlockingScanPort {
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    }

    impl MidiInputDevicePort for BlockingScanPort {
        fn enumerate(&mut self) -> Result<Vec<MidiInputDescriptor>, MidiDeviceFailure> {
            self.entered.send(()).unwrap();
            self.release.recv().unwrap();
            Ok(Vec::new())
        }

        fn connect(
            &mut self,
            _request: ConnectMidiInput,
            _ingress: PhysicalMidiIngress,
        ) -> Result<ActiveMidiInput, MidiDeviceFailure> {
            Err(MidiDeviceFailure::InitializationUnavailable)
        }

        fn disconnect(&mut self, _active: ActiveMidiInput) -> Result<(), MidiDeviceFailure> {
            Ok(())
        }
    }

    struct FailingRetirementPort;

    impl MidiInputDevicePort for FailingRetirementPort {
        fn enumerate(&mut self) -> Result<Vec<MidiInputDescriptor>, MidiDeviceFailure> {
            Ok(Vec::new())
        }

        fn connect(
            &mut self,
            request: ConnectMidiInput,
            _ingress: PhysicalMidiIngress,
        ) -> Result<ActiveMidiInput, MidiDeviceFailure> {
            Ok(ActiveMidiInput::from_backend(&request, ()))
        }

        fn disconnect(&mut self, active: ActiveMidiInput) -> Result<(), MidiDeviceFailure> {
            let identity = active.identity().clone();
            let revision = active.revision();
            assert!(active.into_backend::<()>().is_ok());
            Err(MidiDeviceFailure::DisconnectionFailed { identity, revision })
        }
    }

    impl MidiInputDevicePort for FakeDevicePort {
        fn enumerate(&mut self) -> Result<Vec<MidiInputDescriptor>, MidiDeviceFailure> {
            Ok(vec![MidiInputDescriptor::new(id("a"), "A", None).unwrap()])
        }

        fn connect(
            &mut self,
            request: ConnectMidiInput,
            _ingress: PhysicalMidiIngress,
        ) -> Result<ActiveMidiInput, MidiDeviceFailure> {
            self.live.fetch_add(1, Ordering::SeqCst);
            Ok(ActiveMidiInput::from_backend(
                &request,
                Arc::clone(&self.live),
            ))
        }

        fn disconnect(&mut self, active: ActiveMidiInput) -> Result<(), MidiDeviceFailure> {
            let _ = active.into_backend::<Arc<AtomicUsize>>().map_err(|_| {
                MidiDeviceFailure::StaleCorrelation {
                    operation: crate::control::MidiStaleOperation::Retirement,
                }
            })?;
            self.live.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakePreferencePort(Option<MidiInputPreference>);

    impl MidiInputPreferencePort for FakePreferencePort {
        fn load(&mut self) -> Result<Option<MidiInputPreference>, MidiDeviceFailure> {
            Ok(self.0.clone())
        }

        fn store(&mut self, value: &MidiInputPreference) -> Result<(), MidiDeviceFailure> {
            self.0 = Some(value.clone());
            Ok(())
        }
    }

    fn poll(worker: &mut ThreadedMidiDeviceWorker) -> MidiDeviceWorkerResult {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(result) = worker.try_poll() {
                return result;
            }
            assert!(Instant::now() < deadline, "worker result timed out");
            thread::yield_now();
        }
    }

    #[test]
    fn worker_is_nonblocking_and_separately_bounds_scan_and_connect() {
        let live = Arc::new(AtomicUsize::new(0));
        let mut worker = ThreadedMidiDeviceWorker::new(
            Box::new(FakeDevicePort {
                live: Arc::clone(&live),
            }),
            Box::new(FakePreferencePort::default()),
        )
        .unwrap();

        worker
            .try_submit(MidiDeviceWorkerCommand::Scan {
                scan_id: MidiInputScanId::FIRST,
            })
            .unwrap();
        let duplicate = worker
            .try_submit(MidiDeviceWorkerCommand::Scan {
                scan_id: MidiInputScanId::new(2).unwrap(),
            })
            .unwrap_err();
        assert_eq!(
            duplicate.reason(),
            MidiDeviceWorkerBusyReason::ScanOutstanding
        );
        assert!(matches!(
            duplicate.into_command(),
            MidiDeviceWorkerCommand::Scan { .. }
        ));

        let connect = request("a", 1);
        worker
            .try_submit(MidiDeviceWorkerCommand::Connect {
                ingress: PhysicalMidiIngress::from_backend(connect.revision(), ()),
                request: connect.clone(),
            })
            .unwrap();
        let duplicate = worker
            .try_submit(MidiDeviceWorkerCommand::Connect {
                ingress: PhysicalMidiIngress::from_backend(connect.revision(), ()),
                request: connect,
            })
            .unwrap_err();
        assert_eq!(
            duplicate.reason(),
            MidiDeviceWorkerBusyReason::ConnectOutstanding
        );
        assert!(matches!(
            duplicate.into_command(),
            MidiDeviceWorkerCommand::Connect { .. }
        ));

        let mut prepared = None;
        for _ in 0..2 {
            match poll(&mut worker) {
                MidiDeviceWorkerResult::ScanSucceeded { descriptors, .. } => {
                    assert_eq!(descriptors.len(), 1)
                }
                MidiDeviceWorkerResult::ConnectionPrepared { active, .. } => {
                    prepared = Some(active)
                }
                other => panic!("unexpected result: {other:?}"),
            }
        }
        let active = prepared.unwrap();
        assert_eq!(live.load(Ordering::SeqCst), 1);
        worker
            .try_submit(MidiDeviceWorkerCommand::Retire { active })
            .unwrap();
        assert!(matches!(
            poll(&mut worker),
            MidiDeviceWorkerResult::Retired { failure: None, .. }
        ));
        assert_eq!(live.load(Ordering::SeqCst), 0);

        let preference = MidiInputPreference::new(MidiPreferredInput::new(id("a"), "A").unwrap());
        worker
            .try_submit(MidiDeviceWorkerCommand::StorePreference {
                preference: preference.clone(),
            })
            .unwrap();
        assert!(matches!(
            poll(&mut worker),
            MidiDeviceWorkerResult::PreferenceStored { failure: None, .. }
        ));
        worker
            .try_submit(MidiDeviceWorkerCommand::LoadPreference)
            .unwrap();
        assert!(matches!(
            poll(&mut worker),
            MidiDeviceWorkerResult::PreferenceLoaded {
                preference: Some(value),
                failure: None
            } if value == preference
        ));
        worker.shutdown_on_control().unwrap();
        let rejected = worker
            .try_submit(MidiDeviceWorkerCommand::LoadPreference)
            .unwrap_err();
        assert_eq!(rejected.reason(), MidiDeviceWorkerBusyReason::Shutdown);
    }

    #[test]
    fn shutdown_reclaims_an_unpolled_candidate_on_the_device_worker() {
        let live = Arc::new(AtomicUsize::new(0));
        let mut worker = ThreadedMidiDeviceWorker::new(
            Box::new(FakeDevicePort {
                live: Arc::clone(&live),
            }),
            Box::new(FakePreferencePort::default()),
        )
        .unwrap();
        let request = request("a", 1);
        worker
            .try_submit(MidiDeviceWorkerCommand::Connect {
                ingress: PhysicalMidiIngress::from_backend(request.revision(), ()),
                request,
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while live.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "candidate connect timed out");
            thread::yield_now();
        }
        worker.shutdown_on_control().unwrap();
        assert_eq!(
            live.load(Ordering::SeqCst),
            0,
            "an unpolled candidate is returned to worker-side disconnect"
        );
    }

    #[test]
    fn full_command_channel_returns_the_complete_owned_cleanup_value() {
        let (entered_sender, entered_receiver) = std::sync::mpsc::channel();
        let (release_sender, release_receiver) = std::sync::mpsc::channel();
        let mut worker = ThreadedMidiDeviceWorker::new(
            Box::new(BlockingScanPort {
                entered: entered_sender,
                release: release_receiver,
            }),
            Box::new(FakePreferencePort::default()),
        )
        .unwrap();
        worker
            .try_submit(MidiDeviceWorkerCommand::Scan {
                scan_id: MidiInputScanId::FIRST,
            })
            .unwrap();
        entered_receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        for _ in 0..MIDI_DEVICE_WORKER_COMMAND_CAPACITY {
            worker
                .try_submit(MidiDeviceWorkerCommand::LoadPreference)
                .unwrap();
        }
        let correlation = request("owned-retire", 9);
        let active = ActiveMidiInput::from_backend(&correlation, ());
        let busy = worker
            .try_submit(MidiDeviceWorkerCommand::Retire { active })
            .unwrap_err();
        assert_eq!(busy.reason(), MidiDeviceWorkerBusyReason::QueueFull);
        let MidiDeviceWorkerCommand::Retire { active } = busy.into_command() else {
            panic!("the refused cleanup command changed shape")
        };
        assert_eq!(active.identity(), correlation.identity());
        assert_eq!(active.request_id(), correlation.request_id());
        assert_eq!(active.revision(), correlation.revision());
        assert!(active.into_backend::<()>().is_ok());
        release_sender.send(()).unwrap();
        worker.shutdown_on_control().unwrap();
    }

    #[test]
    fn abnormal_retirement_is_typed_after_worker_side_consumption() {
        let mut worker = ThreadedMidiDeviceWorker::new(
            Box::new(FailingRetirementPort),
            Box::new(FakePreferencePort::default()),
        )
        .unwrap();
        let request = request("retirement-failure", 11);
        worker
            .try_submit(MidiDeviceWorkerCommand::Connect {
                ingress: PhysicalMidiIngress::from_backend(request.revision(), ()),
                request: request.clone(),
            })
            .unwrap();
        let MidiDeviceWorkerResult::ConnectionPrepared { active, .. } = poll(&mut worker) else {
            panic!("connection should prepare")
        };
        worker
            .try_submit(MidiDeviceWorkerCommand::Retire { active })
            .unwrap();
        assert!(matches!(
            poll(&mut worker),
            MidiDeviceWorkerResult::Retired {
                failure: Some(MidiDeviceFailure::DisconnectionFailed { identity, revision }),
                ..
            } if identity == *request.identity() && revision == request.revision()
        ));
        worker.shutdown_on_control().unwrap();
    }

    #[test]
    fn shutdown_transfers_control_owned_handles_and_reports_retirement_failure() {
        let live = Arc::new(AtomicUsize::new(1));
        let correlation = request("shutdown-owned", 12);
        let active = ActiveMidiInput::from_backend(&correlation, Arc::clone(&live));
        let mut worker = ThreadedMidiDeviceWorker::new(
            Box::new(FakeDevicePort {
                live: Arc::clone(&live),
            }),
            Box::new(FakePreferencePort::default()),
        )
        .unwrap();
        worker
            .shutdown_with_retirements_on_control(vec![active])
            .unwrap();
        assert_eq!(live.load(Ordering::SeqCst), 0);

        let correlation = request("shutdown-failure", 13);
        let active = ActiveMidiInput::from_backend(&correlation, ());
        let mut failing = ThreadedMidiDeviceWorker::new(
            Box::new(FailingRetirementPort),
            Box::new(FakePreferencePort::default()),
        )
        .unwrap();
        assert_eq!(
            failing.shutdown_with_retirements_on_control(vec![active]),
            Err(MidiDeviceWorkerShutdownError::RetirementFailed)
        );
    }
}
