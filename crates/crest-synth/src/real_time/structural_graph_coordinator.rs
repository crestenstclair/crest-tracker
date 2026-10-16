use crate::real_time::graph_handoff_status::GraphHandoffStatus;
use crate::real_time::graph_revision::GraphRevision;
use crate::real_time::prepared_graph::{PreparedGraph, PreparedGraphLayout};
use crate::real_time::structural_graph_boundary::ControlStructuralGraphBoundary;
use core::fmt;

/// Control/worker-side one-in-flight structural publication coordinator.
pub struct StructuralGraphCoordinator<Boundary> {
    boundary: Boundary,
    required_layout: PreparedGraphLayout,
    staged: Option<StagedGraph>,
    in_flight: Option<InFlightGraph>,
}

impl<Boundary> StructuralGraphCoordinator<Boundary>
where
    Boundary: ControlStructuralGraphBoundary,
{
    pub fn new(boundary: Boundary, active_graph: &PreparedGraph) -> Self {
        Self {
            boundary,
            required_layout: active_graph.layout(),
            staged: None,
            in_flight: None,
        }
    }

    /// Publishes one replacement only when no prior graph awaits activation and
    /// off-callback retirement acknowledgement.
    pub fn publish(&mut self, graph: PreparedGraph) -> Result<(), GraphPublicationError> {
        self.poll();
        if self.staged.is_some() || self.in_flight.is_some() {
            return Err(GraphPublicationError::new(
                GraphPublicationFailure::ReplacementInFlight,
                graph,
            ));
        }

        let status = self.boundary.read_status_on_control();
        let Some(previous_revision) = status.active_revision() else {
            return Err(GraphPublicationError::new(
                GraphPublicationFailure::NoActiveGraph,
                graph,
            ));
        };
        if graph.revision() <= previous_revision {
            return Err(GraphPublicationError::new(
                GraphPublicationFailure::NonMonotonicRevision,
                graph,
            ));
        }
        if graph.layout() != self.required_layout {
            return Err(GraphPublicationError::new(
                GraphPublicationFailure::IncompatibleLayout,
                graph,
            ));
        }
        let revision = graph.revision();
        if let Err(full) = self.boundary.publish_prepared_on_control(graph) {
            return Err(GraphPublicationError::new(
                GraphPublicationFailure::BoundaryFull,
                full.into_graph(),
            ));
        }
        self.in_flight = Some(InFlightGraph {
            revision,
            previous_revision,
            accepted_layout: self.required_layout.clone(),
            active_acknowledged: false,
            retired_acknowledged: false,
            retired_collected: false,
        });
        Ok(())
    }

    /// Stages one scoped structural replacement for the next control poll.
    ///
    /// The scope names exactly the position the correlated request declared —
    /// a selected engine, one Patch effect slot, or one bus return — and the
    /// layout admission rejects any other delta. The explicit staged step
    /// makes ownership observable before publication; a full structural queue
    /// retains the exact graph in the same single slot for nonblocking retry
    /// on later polls.
    pub fn stage_replacement(
        &mut self,
        graph: PreparedGraph,
        scope: crate::real_time::GraphReplacementScope,
    ) -> Result<GraphStageOutcome, GraphPublicationError> {
        if self.staged.is_some() || self.in_flight.is_some() {
            return Err(GraphPublicationError::new(
                GraphPublicationFailure::ReplacementInFlight,
                graph,
            ));
        }
        let status = self.boundary.read_status_on_control();
        let Some(previous_revision) = status.active_revision() else {
            return Err(GraphPublicationError::new(
                GraphPublicationFailure::NoActiveGraph,
                graph,
            ));
        };
        if graph.revision() <= previous_revision {
            return Err(GraphPublicationError::new(
                GraphPublicationFailure::NonMonotonicRevision,
                graph,
            ));
        }
        let accepted_layout = graph.layout();
        if !self
            .required_layout
            .permits_replacement(accepted_layout.clone(), scope)
        {
            return Err(GraphPublicationError::new(
                GraphPublicationFailure::IncompatibleLayout,
                graph,
            ));
        }
        self.staged = Some(StagedGraph {
            graph,
            previous_revision,
            accepted_layout,
        });
        Ok(GraphStageOutcome::Staged)
    }

    /// Drains every ready returned graph on control ownership and advances the
    /// one-in-flight acknowledgement state.
    pub fn poll(&mut self) -> CoordinatorProgress {
        let mut collected_count = 0_u64;
        while let Some(revision) = self.boundary.collect_retired_on_control() {
            collected_count = collected_count.saturating_add(1);
            if let Some(in_flight) = &mut self.in_flight {
                if revision == in_flight.previous_revision {
                    in_flight.retired_collected = true;
                }
            }
        }

        let status = self.boundary.read_status_on_control();
        let (published_revision, publication_failure) = self.retry_staged(status);
        let status = self.boundary.read_status_on_control();
        let mut completed_revision = None;
        let mut completed_layout = None;
        if let Some(in_flight) = &mut self.in_flight {
            if status.active_revision() == Some(in_flight.revision) {
                in_flight.active_acknowledged = true;
            }
            if status.retired_revision() == Some(in_flight.previous_revision) {
                in_flight.retired_acknowledged = true;
            }
            if in_flight.active_acknowledged
                && in_flight.retired_acknowledged
                && in_flight.retired_collected
            {
                completed_revision = Some(in_flight.revision);
                completed_layout = Some(in_flight.accepted_layout.clone());
            }
        }
        if completed_revision.is_some() {
            self.in_flight = None;
        }
        if let Some(layout) = completed_layout {
            self.required_layout = layout;
        }

        CoordinatorProgress {
            status,
            collected_count,
            published_revision,
            completed_revision,
            publication_failure,
        }
    }

    fn retry_staged(
        &mut self,
        status: GraphHandoffStatus,
    ) -> (Option<GraphRevision>, Option<GraphPublicationFailure>) {
        let Some(staged) = self.staged.take() else {
            return (None, None);
        };
        if self.in_flight.is_some() {
            self.staged = Some(staged);
            return (None, None);
        }
        if status.active_revision() != Some(staged.previous_revision) {
            drop(staged.graph);
            return (None, Some(GraphPublicationFailure::StaleActiveRevision));
        }
        let revision = staged.graph.revision();
        match self.boundary.publish_prepared_on_control(staged.graph) {
            Ok(()) => {
                self.in_flight = Some(InFlightGraph {
                    revision,
                    previous_revision: staged.previous_revision,
                    accepted_layout: staged.accepted_layout,
                    active_acknowledged: false,
                    retired_acknowledged: false,
                    retired_collected: false,
                });
                (Some(revision), None)
            }
            Err(full) => {
                self.staged = Some(StagedGraph {
                    graph: full.into_graph(),
                    previous_revision: staged.previous_revision,
                    accepted_layout: staged.accepted_layout,
                });
                (None, None)
            }
        }
    }

    pub fn in_flight_revision(&self) -> Option<GraphRevision> {
        self.in_flight.as_ref().map(|in_flight| in_flight.revision)
    }

    pub fn staged_revision(&self) -> Option<GraphRevision> {
        self.staged.as_ref().map(|staged| staged.graph.revision())
    }

    pub const fn has_pending_graph(&self) -> bool {
        self.staged.is_some() || self.in_flight.is_some()
    }

    pub fn status(&self) -> GraphHandoffStatus {
        self.boundary.read_status_on_control()
    }

    pub fn into_boundary(self) -> Boundary {
        self.boundary
    }
}

#[derive(Clone, Debug)]
struct InFlightGraph {
    revision: GraphRevision,
    previous_revision: GraphRevision,
    accepted_layout: PreparedGraphLayout,
    active_acknowledged: bool,
    retired_acknowledged: bool,
    retired_collected: bool,
}

#[derive(Debug)]
struct StagedGraph {
    graph: PreparedGraph,
    previous_revision: GraphRevision,
    accepted_layout: PreparedGraphLayout,
}

/// Fixed-size result of one coordinator poll.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CoordinatorProgress {
    status: GraphHandoffStatus,
    collected_count: u64,
    published_revision: Option<GraphRevision>,
    completed_revision: Option<GraphRevision>,
    publication_failure: Option<GraphPublicationFailure>,
}

impl CoordinatorProgress {
    pub const fn status(self) -> GraphHandoffStatus {
        self.status
    }

    pub const fn collected_count(self) -> u64 {
        self.collected_count
    }

    pub const fn published_revision(self) -> Option<GraphRevision> {
        self.published_revision
    }

    pub const fn completed_revision(self) -> Option<GraphRevision> {
        self.completed_revision
    }

    pub const fn publication_failure(self) -> Option<GraphPublicationFailure> {
        self.publication_failure
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphStageOutcome {
    Published,
    Staged,
}

/// Why a coordinator returned ownership of an unpublished graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphPublicationFailure {
    ReplacementInFlight,
    NoActiveGraph,
    NonMonotonicRevision,
    IncompatibleLayout,
    BoundaryFull,
    StaleActiveRevision,
}

/// Publication failure that preserves the complete candidate graph.
#[must_use = "the complete unpublished graph remains owned by control"]
#[derive(Debug)]
pub struct GraphPublicationError {
    reason: GraphPublicationFailure,
    graph: PreparedGraph,
}

impl GraphPublicationError {
    const fn new(reason: GraphPublicationFailure, graph: PreparedGraph) -> Self {
        Self { reason, graph }
    }

    pub const fn reason(&self) -> GraphPublicationFailure {
        self.reason
    }

    pub const fn graph(&self) -> &PreparedGraph {
        &self.graph
    }

    pub fn into_graph(self) -> PreparedGraph {
        self.graph
    }
}

impl fmt::Display for GraphPublicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.reason {
            GraphPublicationFailure::ReplacementInFlight => {
                "a structural graph replacement is already in flight"
            }
            GraphPublicationFailure::NoActiveGraph => {
                "structural publication has no acknowledged active graph"
            }
            GraphPublicationFailure::NonMonotonicRevision => {
                "structural graph revision must advance monotonically"
            }
            GraphPublicationFailure::IncompatibleLayout => {
                "replacement graph layout or callback capacities are incompatible"
            }
            GraphPublicationFailure::BoundaryFull => {
                "the structural graph publication queue is full"
            }
            GraphPublicationFailure::StaleActiveRevision => {
                "the active graph revision changed before staged publication"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for GraphPublicationError {}

#[cfg(test)]
mod tests {
    use super::{GraphPublicationFailure, GraphStageOutcome, StructuralGraphCoordinator};
    use crate::adapter::braids_capability::BRAIDS_CAPABILITY_ID;
    use crate::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID;
    use crate::adapter::production_instruments::{
        production_capability_registry, production_instrument_providers,
    };
    use crate::kernel::midi_message::MidiMessage;
    use crate::kernel::{MidiChannel, PatchId};
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_state::MixerState;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::graph_handoff_status::GraphHandoffStatus;
    use crate::real_time::graph_revision::GraphRevision;
    use crate::real_time::parameter_snapshot::ParameterSnapshot;
    use crate::real_time::prepared_graph::PreparedGraph;
    use crate::real_time::prepared_graph_builder::PreparedGraphBuilder;
    use crate::real_time::structural_graph_boundary::{
        ControlStructuralGraphBoundary, StructuralBoundaryFull,
    };
    use crate::synth::instrument_preparer::{InstrumentPreparationError, InstrumentPreparer};
    use crate::synth::{
        CapabilityId, DescriptorDefaultConfigFactory, Patch, PreparedInstrument,
        PreparedInstrumentError,
    };
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    struct TestControlBoundary {
        shared: Arc<Mutex<TestBoundaryState>>,
    }

    struct TestBoundaryState {
        prepared: Option<PreparedGraph>,
        retired: VecDeque<PreparedGraph>,
        status: GraphHandoffStatus,
    }

    impl ControlStructuralGraphBoundary for TestControlBoundary {
        fn publish_prepared_on_control(
            &mut self,
            graph: PreparedGraph,
        ) -> Result<(), StructuralBoundaryFull> {
            let mut shared = self.shared.lock().unwrap();
            if shared.prepared.is_some() {
                Err(StructuralBoundaryFull::new(graph))
            } else {
                shared.prepared = Some(graph);
                Ok(())
            }
        }

        fn collect_retired_on_control(&mut self) -> Option<GraphRevision> {
            let graph = self.shared.lock().unwrap().retired.pop_front()?;
            let revision = graph.revision();
            drop(graph);
            Some(revision)
        }

        fn read_status_on_control(&self) -> GraphHandoffStatus {
            self.shared.lock().unwrap().status
        }
    }

    fn globals() -> GlobalParameters {
        GlobalParameters::new(0.0).unwrap()
    }

    fn graph(value: u64) -> PreparedGraph {
        graph_with_frames(value, 1)
    }

    fn graph_with_frames(value: u64, max_frames: usize) -> PreparedGraph {
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        let registry = provider.registry().unwrap();
        let preparers: Vec<Box<dyn InstrumentPreparer>> = Vec::new();
        let revision = GraphRevision::new(value).unwrap();
        let parameters =
            ParameterSnapshot::for_graph(value, revision, globals(), MixerState::default(), &[])
                .unwrap();
        PreparedGraphBuilder::new(&registry, &preparers)
            .build(revision, &[], parameters, 16_000.0, max_frames)
            .unwrap()
    }

    fn coordinator(
        active_revision: u64,
    ) -> (
        StructuralGraphCoordinator<TestControlBoundary>,
        Arc<Mutex<TestBoundaryState>>,
    ) {
        let shared = Arc::new(Mutex::new(TestBoundaryState {
            prepared: None,
            retired: VecDeque::new(),
            status: GraphHandoffStatus::with_active(GraphRevision::new(active_revision).unwrap()),
        }));
        let active_graph = graph(active_revision);
        let coordinator = StructuralGraphCoordinator::new(
            TestControlBoundary {
                shared: Arc::clone(&shared),
            },
            &active_graph,
        );
        (coordinator, shared)
    }

    struct CapabilityPreparer {
        capability_id: CapabilityId,
    }

    impl InstrumentPreparer for CapabilityPreparer {
        fn capability_id(&self) -> &CapabilityId {
            &self.capability_id
        }

        fn prepare(
            &self,
            patch: &Patch,
            _sample_rate: f32,
            _max_frames: usize,
        ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
            Ok(Box::new(SilentInstrument {
                patch_id: patch.id(),
            }))
        }
    }

    struct SilentInstrument {
        patch_id: PatchId,
    }

    impl PreparedInstrument for SilentInstrument {
        fn patch_id(&self) -> PatchId {
            self.patch_id
        }

        fn dispatch(
            &mut self,
            _message: MidiMessage,
            _parameters: &crate::real_time::RtPatchParameters,
        ) -> Result<(), PreparedInstrumentError> {
            Ok(())
        }

        fn render(
            &mut self,
            output: &mut [f32],
            _frame_count: usize,
            _parameters: &crate::real_time::RtPatchParameters,
        ) -> Result<(), crate::synth::PreparedInstrumentError> {
            output.fill(0.0);

            Ok(())
        }

        fn all_notes_off(&mut self) {}
    }

    fn capability_graph(value: u64, capability_id: &str) -> PreparedGraph {
        let registry = production_capability_registry().unwrap();
        let factory = DescriptorDefaultConfigFactory::new(
            registry.clone(),
            production_instrument_providers().unwrap(),
        );
        let patch = Patch::new(
            PatchId::new(1).unwrap(),
            "Selected Patch".to_owned(),
            factory
                .create(&CapabilityId::new(capability_id).unwrap())
                .unwrap(),
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        );
        let patches = [patch];
        let revision = GraphRevision::new(value).unwrap();
        let parameters = ParameterSnapshot::project_patches(
            value,
            revision,
            globals(),
            MixerState::default(),
            &patches,
            &registry,
        )
        .unwrap();
        let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![
            Box::new(CapabilityPreparer {
                capability_id: CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap(),
            }),
            Box::new(CapabilityPreparer {
                capability_id: CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap(),
            }),
        ];
        PreparedGraphBuilder::new(&registry, &preparers)
            .build(revision, &patches, parameters, 16_000.0, 1)
            .unwrap()
    }

    #[test]
    fn one_in_flight_graph_requires_activation_retirement_and_collection() {
        let (mut coordinator, shared) = coordinator(1);
        coordinator.publish(graph(2)).unwrap();
        assert_eq!(
            coordinator.in_flight_revision(),
            Some(GraphRevision::new(2).unwrap())
        );

        let rejected = coordinator.publish(graph(3)).unwrap_err();
        assert_eq!(
            rejected.reason(),
            GraphPublicationFailure::ReplacementInFlight
        );
        assert_eq!(rejected.graph().revision(), GraphRevision::new(3).unwrap());
        drop(rejected.into_graph());

        let replacement = shared.lock().unwrap().prepared.take().unwrap();
        assert_eq!(replacement.revision(), GraphRevision::new(2).unwrap());
        shared.lock().unwrap().status =
            GraphHandoffStatus::with_active(GraphRevision::new(2).unwrap());
        let activation_only = coordinator.poll();
        assert_eq!(activation_only.completed_revision(), None);
        assert_eq!(
            coordinator.in_flight_revision(),
            Some(replacement.revision())
        );

        let mut completed = GraphHandoffStatus::with_active(replacement.revision());
        completed.record_retired(GraphRevision::new(1).unwrap());
        let mut shared_state = shared.lock().unwrap();
        shared_state.status = completed;
        shared_state.retired.push_back(graph(1));
        drop(shared_state);

        let progress = coordinator.poll();
        assert_eq!(progress.collected_count(), 1);
        assert_eq!(progress.completed_revision(), Some(replacement.revision()));
        assert_eq!(coordinator.in_flight_revision(), None);

        drop(replacement);
        coordinator.publish(graph(3)).unwrap();
        assert_eq!(
            coordinator.in_flight_revision(),
            Some(GraphRevision::new(3).unwrap())
        );
    }

    #[test]
    fn publication_preserves_graph_on_full_missing_or_nonmonotonic_state() {
        let (mut coordinator, shared) = coordinator(1);
        shared.lock().unwrap().prepared = Some(graph(2));
        let full = coordinator.publish(graph(3)).unwrap_err();
        assert_eq!(full.reason(), GraphPublicationFailure::BoundaryFull);
        assert_eq!(full.graph().revision(), GraphRevision::new(3).unwrap());
        drop(full.into_graph());
        shared.lock().unwrap().prepared.take();

        let old = coordinator.publish(graph(1)).unwrap_err();
        assert_eq!(old.reason(), GraphPublicationFailure::NonMonotonicRevision);
        assert_eq!(old.graph().revision(), GraphRevision::new(1).unwrap());
        drop(old.into_graph());

        shared.lock().unwrap().status = GraphHandoffStatus::default();
        let missing = coordinator.publish(graph(4)).unwrap_err();
        assert_eq!(missing.reason(), GraphPublicationFailure::NoActiveGraph);
        assert_eq!(missing.graph().revision(), GraphRevision::new(4).unwrap());
    }

    #[test]
    fn publication_rejects_a_graph_with_different_callback_capacities() {
        let (mut coordinator, shared) = coordinator(1);
        let incompatible = coordinator.publish(graph_with_frames(2, 2)).unwrap_err();

        assert_eq!(
            incompatible.reason(),
            GraphPublicationFailure::IncompatibleLayout
        );
        assert_eq!(incompatible.graph().max_frames(), 2);
        assert!(shared.lock().unwrap().prepared.is_none());
    }

    #[test]
    fn selected_layout_change_stages_retries_and_is_adopted_only_after_collection() {
        let source = capability_graph(1, HIDEF_CAPABILITY_ID);
        let shared = Arc::new(Mutex::new(TestBoundaryState {
            prepared: Some(graph(9)),
            retired: VecDeque::new(),
            status: GraphHandoffStatus::with_active(GraphRevision::INITIAL),
        }));
        let mut coordinator = StructuralGraphCoordinator::new(
            TestControlBoundary {
                shared: Arc::clone(&shared),
            },
            &source,
        );
        let selected_patch_id = PatchId::new(1).unwrap();

        assert_eq!(
            coordinator
                .stage_replacement(
                    capability_graph(2, BRAIDS_CAPABILITY_ID),
                    crate::real_time::GraphReplacementScope::SelectedEngine(selected_patch_id),
                )
                .unwrap(),
            GraphStageOutcome::Staged
        );
        assert_eq!(coordinator.staged_revision(), GraphRevision::new(2).ok());
        assert_eq!(coordinator.in_flight_revision(), None);

        drop(shared.lock().unwrap().prepared.take());
        let published = coordinator.poll();
        assert_eq!(
            published.published_revision(),
            Some(GraphRevision::new(2).unwrap())
        );
        assert_eq!(coordinator.staged_revision(), None);
        assert_eq!(
            coordinator.in_flight_revision(),
            Some(GraphRevision::new(2).unwrap())
        );

        let replacement = shared.lock().unwrap().prepared.take().unwrap();
        shared.lock().unwrap().status =
            GraphHandoffStatus::with_active(GraphRevision::new(2).unwrap());
        assert_eq!(coordinator.poll().completed_revision(), None);
        let mut completed = GraphHandoffStatus::with_active(GraphRevision::new(2).unwrap());
        completed.record_retired(GraphRevision::INITIAL);
        let mut state = shared.lock().unwrap();
        state.status = completed;
        state.retired.push_back(source);
        drop(state);
        assert_eq!(
            coordinator.poll().completed_revision(),
            Some(GraphRevision::new(2).unwrap())
        );

        assert_eq!(
            coordinator
                .stage_replacement(
                    capability_graph(3, HIDEF_CAPABILITY_ID),
                    crate::real_time::GraphReplacementScope::SelectedEngine(selected_patch_id),
                )
                .unwrap(),
            GraphStageOutcome::Staged
        );
        assert_eq!(
            coordinator.poll().published_revision(),
            Some(GraphRevision::new(3).unwrap())
        );
        assert_eq!(
            coordinator.in_flight_revision(),
            Some(GraphRevision::new(3).unwrap())
        );
        drop(replacement);
    }
}
