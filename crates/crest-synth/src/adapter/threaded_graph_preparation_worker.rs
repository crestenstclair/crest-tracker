use crate::real_time::graph_preparation_worker::prepare_graph_request_with_effects;
use crate::real_time::{
    GraphPreparationRequest, GraphPreparationResult, GraphPreparationWorker, WorkerBusy,
    WorkerBusyReason, WorkerShutdownError,
};
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

/// Production capacity-one graph worker backed only by standard-library primitives.
pub struct ThreadedGraphPreparationWorker {
    request_sender: Option<SyncSender<GraphPreparationRequest>>,
    result_receiver: Receiver<GraphPreparationResult>,
    outstanding: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl ThreadedGraphPreparationWorker {
    pub fn new(
        registry: CapabilityRegistry,
        preparers: Vec<Box<dyn InstrumentPreparer>>,
        audio_config: AudioDeviceConfig,
    ) -> Result<Self, ThreadedGraphPreparationWorkerError> {
        Self::new_with_effects(
            registry,
            preparers,
            EffectCapabilityRegistry::default(),
            Vec::new(),
            audio_config,
        )
    }

    pub fn new_with_effects(
        registry: CapabilityRegistry,
        preparers: Vec<Box<dyn InstrumentPreparer>>,
        effects: EffectCapabilityRegistry,
        effect_preparers: Vec<Box<dyn EffectPreparer>>,
        audio_config: AudioDeviceConfig,
    ) -> Result<Self, ThreadedGraphPreparationWorkerError> {
        let (request_sender, request_receiver) = mpsc::sync_channel(1);
        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        let outstanding = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let resources = WorkerResources {
            registry,
            preparers,
            effects,
            effect_preparers,
            audio_config,
        };
        let worker = thread::Builder::new()
            .name("crest-graph-preparation".to_owned())
            .spawn(move || {
                worker_main(resources, request_receiver, result_sender, &worker_shutdown);
            })
            .map_err(|_| ThreadedGraphPreparationWorkerError::ThreadStartFailed)?;

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

impl GraphPreparationWorker for ThreadedGraphPreparationWorker {
    fn try_submit(&mut self, request: GraphPreparationRequest) -> Result<(), WorkerBusy> {
        if self.shutdown.load(Ordering::Acquire) {
            return Err(WorkerBusy::new(WorkerBusyReason::Shutdown, request));
        }
        if self
            .outstanding
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(WorkerBusy::new(
                WorkerBusyReason::OutstandingRequest,
                request,
            ));
        }
        let Some(sender) = &self.request_sender else {
            self.outstanding.store(false, Ordering::Release);
            return Err(WorkerBusy::new(WorkerBusyReason::Shutdown, request));
        };
        match sender.try_send(request) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(request)) => {
                self.outstanding.store(false, Ordering::Release);
                Err(WorkerBusy::new(
                    WorkerBusyReason::OutstandingRequest,
                    request,
                ))
            }
            Err(TrySendError::Disconnected(request)) => {
                self.outstanding.store(false, Ordering::Release);
                Err(WorkerBusy::new(
                    WorkerBusyReason::WorkerUnavailable,
                    request,
                ))
            }
        }
    }

    fn try_poll(&mut self) -> Option<GraphPreparationResult> {
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

impl Drop for ThreadedGraphPreparationWorker {
    fn drop(&mut self) {
        let _ = self.shutdown_inner();
    }
}

struct WorkerResources {
    registry: CapabilityRegistry,
    preparers: Vec<Box<dyn InstrumentPreparer>>,
    effects: EffectCapabilityRegistry,
    effect_preparers: Vec<Box<dyn EffectPreparer>>,
    audio_config: AudioDeviceConfig,
}

fn worker_main(
    resources: WorkerResources,
    request_receiver: Receiver<GraphPreparationRequest>,
    result_sender: SyncSender<GraphPreparationResult>,
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
        let mut result = prepare_graph_request_with_effects(
            &resources.registry,
            &resources.preparers,
            &resources.effects,
            &resources.effect_preparers,
            resources.audio_config,
            request,
        );
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
pub enum ThreadedGraphPreparationWorkerError {
    ThreadStartFailed,
}

impl fmt::Display for ThreadedGraphPreparationWorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the graph preparation worker thread could not start")
    }
}

impl std::error::Error for ThreadedGraphPreparationWorkerError {}

#[cfg(test)]
mod tests {
    use super::ThreadedGraphPreparationWorker;
    use crate::adapter::braids_capability::BRAIDS_CAPABILITY_ID;
    use crate::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID;
    use crate::adapter::production_instruments::{
        production_capability_registry, production_instrument_preparers,
        production_instrument_providers,
    };
    use crate::control::EngineSelectionRequestId;
    use crate::kernel::{MidiChannel, PatchId};
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_state::MixerState;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::{
        GraphPreparationCorrelation, GraphPreparationRequest, GraphPreparationResult,
        GraphPreparationWorker, GraphRevision, WorkerBusyReason,
    };
    use crate::shell::audio_output::{AudioDeviceConfig, AudioSampleFormat};
    use crate::synth::{CapabilityId, DescriptorDefaultConfigFactory, Patch};
    use std::time::{Duration, Instant};

    fn audio_config() -> AudioDeviceConfig {
        AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 64).unwrap()
    }

    fn globals() -> GlobalParameters {
        GlobalParameters::new(0.0).unwrap()
    }

    fn config(capability_id: &str) -> crate::synth::InstrumentConfig {
        let registry = production_capability_registry().unwrap();
        DescriptorDefaultConfigFactory::new(registry, production_instrument_providers().unwrap())
            .create(&CapabilityId::new(capability_id).unwrap())
            .unwrap()
    }

    fn request(
        request_id: u64,
        source_capability_id: &str,
        target_capability_id: &str,
        source_revision: u64,
        target_revision: u64,
    ) -> GraphPreparationRequest {
        let registry = production_capability_registry().unwrap();
        let patch_id = PatchId::new(1).unwrap();
        let active = [Patch::new(
            patch_id,
            "Threaded worker".to_owned(),
            config(source_capability_id),
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        )];
        let correlation = GraphPreparationCorrelation::new(
            EngineSelectionRequestId::new(request_id).unwrap(),
            patch_id,
            CapabilityId::new(source_capability_id).unwrap(),
            CapabilityId::new(target_capability_id).unwrap(),
            GraphRevision::new(source_revision).unwrap(),
            GraphRevision::new(target_revision).unwrap(),
        )
        .unwrap();
        GraphPreparationRequest::replacement(
            correlation,
            &active,
            config(target_capability_id),
            request_id + 10,
            globals(),
            MixerState::default(),
            audio_config(),
            &registry,
        )
        .unwrap()
    }

    fn poll_result(worker: &mut ThreadedGraphPreparationWorker) -> GraphPreparationResult {
        let started = Instant::now();
        loop {
            if let Some(result) = worker.try_poll() {
                return result;
            }
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "threaded graph preparation did not finish"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn threaded_graph_preparation_worker_is_capacity_one_and_prepares_both_real_directions() {
        let registry = production_capability_registry().unwrap();
        let preparers = production_instrument_preparers().unwrap();
        let mut worker =
            ThreadedGraphPreparationWorker::new(registry, preparers, audio_config()).unwrap();

        let forward = request(1, HIDEF_CAPABILITY_ID, BRAIDS_CAPABILITY_ID, 1, 2);
        let duplicate = forward.clone();
        worker.try_submit(forward).unwrap();
        assert!(worker.has_outstanding_request());
        let busy = worker.try_submit(duplicate).unwrap_err();
        assert_eq!(busy.reason(), WorkerBusyReason::OutstandingRequest);
        let forward = poll_result(&mut worker);
        match forward {
            GraphPreparationResult::Prepared {
                correlation,
                candidate_config,
                prepared_graph,
                ..
            } => {
                assert_eq!(correlation.request_id(), EngineSelectionRequestId::FIRST);
                assert_eq!(
                    candidate_config.unwrap().capability_id().as_str(),
                    BRAIDS_CAPABILITY_ID
                );
                assert_eq!(prepared_graph.revision(), GraphRevision::new(2).unwrap());
                assert_eq!(prepared_graph.engine_rack().patch_count(), 1);
            }
            GraphPreparationResult::Failed { failure, .. } => {
                panic!("real Braids preparation failed: {failure:?}")
            }
        }
        assert!(!worker.has_outstanding_request());

        worker
            .try_submit(request(2, BRAIDS_CAPABILITY_ID, HIDEF_CAPABILITY_ID, 2, 3))
            .unwrap();
        let reverse = poll_result(&mut worker);
        match reverse {
            GraphPreparationResult::Prepared {
                correlation,
                candidate_config,
                prepared_graph,
                ..
            } => {
                assert_eq!(correlation.request_id().value(), 2);
                assert_eq!(
                    candidate_config.unwrap().capability_id().as_str(),
                    HIDEF_CAPABILITY_ID
                );
                assert_eq!(prepared_graph.revision(), GraphRevision::new(3).unwrap());
                assert_eq!(prepared_graph.engine_rack().patch_count(), 1);
            }
            GraphPreparationResult::Failed { failure, .. } => {
                panic!("real SoundFont preparation failed: {failure:?}")
            }
        }

        worker.shutdown_on_control().unwrap();
        let shutdown = worker
            .try_submit(request(3, HIDEF_CAPABILITY_ID, BRAIDS_CAPABILITY_ID, 3, 4))
            .unwrap_err();
        assert_eq!(shutdown.reason(), WorkerBusyReason::Shutdown);
    }
}
