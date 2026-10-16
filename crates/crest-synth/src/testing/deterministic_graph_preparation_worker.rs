use crate::control::EngineSelectionFailure;
use crate::real_time::graph_preparation_worker::prepare_graph_request_with_effects;
use crate::real_time::{
    GraphPreparationRequest, GraphPreparationResult, GraphPreparationWorker, WorkerBusy,
    WorkerBusyReason, WorkerShutdownError,
};
use crate::shell::audio_output::AudioDeviceConfig;
use crate::synth::{
    CapabilityRegistry, EffectCapabilityRegistry, EffectPreparer, InstrumentPreparer,
};
use std::cell::RefCell;
use std::rc::Rc;

/// Manually advanced worker adapter for deterministic scenes and acceptance.
pub struct DeterministicGraphPreparationWorker {
    inner: Rc<RefCell<DeterministicGraphPreparationState>>,
}

/// Retained control-side clock for one injected deterministic worker.
#[derive(Clone)]
pub struct DeterministicGraphPreparationHandle {
    inner: Rc<RefCell<DeterministicGraphPreparationState>>,
}

struct DeterministicGraphPreparationState {
    registry: CapabilityRegistry,
    preparers: Vec<Box<dyn InstrumentPreparer>>,
    effects: EffectCapabilityRegistry,
    effect_preparers: Vec<Box<dyn EffectPreparer>>,
    audio_config: AudioDeviceConfig,
    pending: Option<GraphPreparationRequest>,
    result: Option<GraphPreparationResult>,
    controlled_failure: Option<EngineSelectionFailure>,
    shutdown: bool,
}

impl DeterministicGraphPreparationWorker {
    pub fn new(
        registry: CapabilityRegistry,
        preparers: Vec<Box<dyn InstrumentPreparer>>,
        audio_config: AudioDeviceConfig,
    ) -> Self {
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
    ) -> Self {
        Self {
            inner: Rc::new(RefCell::new(DeterministicGraphPreparationState {
                registry,
                preparers,
                effects,
                effect_preparers,
                audio_config,
                pending: None,
                result: None,
                controlled_failure: None,
                shutdown: false,
            })),
        }
    }

    pub fn advance_handle(&self) -> DeterministicGraphPreparationHandle {
        DeterministicGraphPreparationHandle {
            inner: Rc::clone(&self.inner),
        }
    }

    /// Causes the next manual advancement to return this stable failure.
    pub fn fail_next(&mut self, failure: EngineSelectionFailure) {
        self.inner.borrow_mut().controlled_failure = Some(failure);
    }

    /// Performs exactly one pending unit of real graph preparation.
    pub fn advance(&mut self) -> bool {
        self.advance_handle().advance()
    }

    pub fn is_pending(&self) -> bool {
        self.inner.borrow().pending.is_some()
    }

    pub fn has_result(&self) -> bool {
        self.inner.borrow().result.is_some()
    }
}

impl DeterministicGraphPreparationHandle {
    pub fn fail_next(&self, failure: EngineSelectionFailure) {
        self.inner.borrow_mut().controlled_failure = Some(failure);
    }

    pub fn advance(&self) -> bool {
        let mut state = self.inner.borrow_mut();
        if state.shutdown || state.result.is_some() {
            return false;
        }
        let Some(request) = state.pending.take() else {
            return false;
        };
        let result = if let Some(failure) = state.controlled_failure.take() {
            GraphPreparationResult::Failed {
                correlation: request.correlation().clone(),
                failure,
            }
        } else {
            prepare_graph_request_with_effects(
                &state.registry,
                &state.preparers,
                &state.effects,
                &state.effect_preparers,
                state.audio_config,
                request,
            )
        };
        state.result = Some(result);
        true
    }

    pub fn is_pending(&self) -> bool {
        self.inner.borrow().pending.is_some()
    }

    pub fn has_result(&self) -> bool {
        self.inner.borrow().result.is_some()
    }
}

impl GraphPreparationWorker for DeterministicGraphPreparationWorker {
    fn try_submit(&mut self, request: GraphPreparationRequest) -> Result<(), WorkerBusy> {
        let mut state = self.inner.borrow_mut();
        if state.shutdown {
            return Err(WorkerBusy::new(WorkerBusyReason::Shutdown, request));
        }
        if state.pending.is_some() || state.result.is_some() {
            return Err(WorkerBusy::new(
                WorkerBusyReason::OutstandingRequest,
                request,
            ));
        }
        state.pending = Some(request);
        Ok(())
    }

    fn try_poll(&mut self) -> Option<GraphPreparationResult> {
        self.inner.borrow_mut().result.take()
    }

    fn shutdown_on_control(&mut self) -> Result<(), WorkerShutdownError> {
        let mut state = self.inner.borrow_mut();
        state.shutdown = true;
        state.pending = None;
        state.result = None;
        state.controlled_failure = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::DeterministicGraphPreparationWorker;
    use crate::adapter::braids_capability::BRAIDS_CAPABILITY_ID;
    use crate::adapter::braids_preparer::BraidsPreparer;
    use crate::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID;
    use crate::adapter::production_instruments::{
        production_capability_registry, production_instrument_providers,
    };
    use crate::control::{EngineSelectionFailure, EngineSelectionRequestId};
    use crate::kernel::{MidiChannel, PatchId};
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_state::MixerState;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::{
        GraphPreparationCorrelation, GraphPreparationRequest, GraphPreparationResult,
        GraphPreparationWorker, GraphRevision, WorkerBusyReason,
    };
    use crate::shell::audio_output::{AudioDeviceConfig, AudioSampleFormat};
    use crate::synth::{CapabilityId, DescriptorDefaultConfigFactory, InstrumentPreparer, Patch};

    fn audio_config() -> AudioDeviceConfig {
        AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 64).unwrap()
    }

    fn request(request_id: u64) -> GraphPreparationRequest {
        let registry = production_capability_registry().unwrap();
        let factory = DescriptorDefaultConfigFactory::new(
            registry.clone(),
            production_instrument_providers().unwrap(),
        );
        let patch_id = PatchId::new(1).unwrap();
        let source = factory
            .create(&CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap())
            .unwrap();
        let target = factory
            .create(&CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap())
            .unwrap();
        let active = [Patch::new(
            patch_id,
            "Deterministic worker".to_owned(),
            source,
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        )];
        GraphPreparationRequest::replacement(
            GraphPreparationCorrelation::new(
                EngineSelectionRequestId::new(request_id).unwrap(),
                patch_id,
                CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap(),
                CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap(),
                GraphRevision::INITIAL,
                GraphRevision::new(2).unwrap(),
            )
            .unwrap(),
            &active,
            target,
            2,
            GlobalParameters::new(0.0).unwrap(),
            MixerState::default(),
            audio_config(),
            &registry,
        )
        .unwrap()
    }

    #[test]
    fn deterministic_graph_preparation_worker_advances_real_work_and_controlled_failure_exactly() {
        let registry = production_capability_registry().unwrap();
        let preparers: Vec<Box<dyn InstrumentPreparer>> =
            vec![Box::new(BraidsPreparer::new().unwrap())];
        let mut worker =
            DeterministicGraphPreparationWorker::new(registry, preparers, audio_config());

        let first = request(1);
        worker.try_submit(first.clone()).unwrap();
        assert!(worker.is_pending());
        assert_eq!(worker.try_poll().map(|_| ()), None);
        let busy = worker.try_submit(first).unwrap_err();
        assert_eq!(busy.reason(), WorkerBusyReason::OutstandingRequest);
        assert!(worker.advance());
        assert!(!worker.is_pending());
        assert!(worker.has_result());
        match worker.try_poll().unwrap() {
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
            }
            GraphPreparationResult::Failed { failure, .. } => {
                panic!("healthy deterministic preparation failed: {failure:?}")
            }
        }

        worker.fail_next(EngineSelectionFailure::AssetUnavailable);
        worker.try_submit(request(2)).unwrap();
        assert!(worker.advance());
        match worker.try_poll().unwrap() {
            GraphPreparationResult::Failed {
                correlation,
                failure,
            } => {
                assert_eq!(correlation.request_id().value(), 2);
                assert_eq!(failure, EngineSelectionFailure::AssetUnavailable);
            }
            GraphPreparationResult::Prepared { .. } => {
                panic!("controlled failure returned a graph")
            }
        }

        worker.try_submit(request(3)).unwrap();
        worker.shutdown_on_control().unwrap();
        assert!(!worker.is_pending());
        assert!(!worker.has_result());
        let shutdown = worker.try_submit(request(4)).unwrap_err();
        assert_eq!(shutdown.reason(), WorkerBusyReason::Shutdown);
    }
}
