use crate::real_time::graph_handoff_status::GraphHandoffStatus;
use crate::real_time::graph_revision::GraphRevision;
use crate::real_time::prepared_graph::PreparedGraph;
use crate::real_time::structural_graph_boundary::{
    AudioStructuralGraphBoundary, ControlStructuralGraphBoundary, RetiredBoundaryFull,
    StructuralBoundaryFull, StructuralGraphBoundary,
};
use core::fmt;
use core::sync::atomic::{fence, AtomicU64, Ordering};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::sync::Arc;

/// Separate preallocated SPSC queues and coherent status for structural graphs.
pub struct LockFreeStructuralGraphBoundary {
    control: LockFreeStructuralControlHandle,
    audio: LockFreeStructuralAudioHandle,
}

impl LockFreeStructuralGraphBoundary {
    pub fn new(
        prepared_capacity: usize,
        retired_capacity: usize,
        initial_status: GraphHandoffStatus,
    ) -> Result<Self, StructuralBoundaryConfigurationError> {
        if prepared_capacity == 0 {
            return Err(StructuralBoundaryConfigurationError::InvalidPreparedCapacity);
        }
        if retired_capacity == 0 {
            return Err(StructuralBoundaryConfigurationError::InvalidRetiredCapacity);
        }

        let (prepared_producer, prepared_consumer) = RingBuffer::new(prepared_capacity);
        let (retired_producer, retired_consumer) = RingBuffer::new(retired_capacity);
        let status = Arc::new(AtomicGraphHandoffStatus::new(initial_status));
        Ok(Self {
            control: LockFreeStructuralControlHandle {
                prepared: prepared_producer,
                retired: retired_consumer,
                status: Arc::clone(&status),
            },
            audio: LockFreeStructuralAudioHandle {
                prepared: prepared_consumer,
                retired: retired_producer,
                status,
            },
        })
    }
}

impl StructuralGraphBoundary for LockFreeStructuralGraphBoundary {
    type ControlHandle = LockFreeStructuralControlHandle;
    type AudioHandle = LockFreeStructuralAudioHandle;

    fn into_handles(self) -> (Self::ControlHandle, Self::AudioHandle) {
        (self.control, self.audio)
    }
}

pub struct LockFreeStructuralControlHandle {
    prepared: Producer<PreparedGraph>,
    retired: Consumer<PreparedGraph>,
    status: Arc<AtomicGraphHandoffStatus>,
}

impl ControlStructuralGraphBoundary for LockFreeStructuralControlHandle {
    fn publish_prepared_on_control(
        &mut self,
        graph: PreparedGraph,
    ) -> Result<(), StructuralBoundaryFull> {
        match self.prepared.push(graph) {
            Ok(()) => Ok(()),
            Err(PushError::Full(graph)) => Err(StructuralBoundaryFull::new(graph)),
        }
    }

    fn collect_retired_on_control(&mut self) -> Option<GraphRevision> {
        let graph = self.retired.pop().ok()?;
        let revision = graph.revision();
        drop(graph);
        Some(revision)
    }

    fn read_status_on_control(&self) -> GraphHandoffStatus {
        self.status.read()
    }
}

pub struct LockFreeStructuralAudioHandle {
    prepared: Consumer<PreparedGraph>,
    retired: Producer<PreparedGraph>,
    status: Arc<AtomicGraphHandoffStatus>,
}

impl AudioStructuralGraphBoundary for LockFreeStructuralAudioHandle {
    fn take_prepared_on_audio(&mut self) -> Option<PreparedGraph> {
        self.prepared.pop().ok()
    }

    fn return_retired_on_audio(&mut self, graph: PreparedGraph) -> Result<(), RetiredBoundaryFull> {
        match self.retired.push(graph) {
            Ok(()) => Ok(()),
            Err(PushError::Full(graph)) => Err(RetiredBoundaryFull::new(graph)),
        }
    }

    fn publish_status_on_audio(&mut self, status: GraphHandoffStatus) {
        self.status.publish(status);
    }
}

/// Invalid preallocation limits for structural ownership transfer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StructuralBoundaryConfigurationError {
    InvalidPreparedCapacity,
    InvalidRetiredCapacity,
}

impl fmt::Display for StructuralBoundaryConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPreparedCapacity => {
                formatter.write_str("prepared graph queue capacity must be nonzero")
            }
            Self::InvalidRetiredCapacity => {
                formatter.write_str("retired graph queue capacity must be nonzero")
            }
        }
    }
}

impl std::error::Error for StructuralBoundaryConfigurationError {}

struct AtomicGraphHandoffStatus {
    version: AtomicU64,
    active_revision: AtomicU64,
    retired_revision: AtomicU64,
    swaps_applied: AtomicU64,
    retirement_retries: AtomicU64,
    incompatible_snapshots: AtomicU64,
}

impl AtomicGraphHandoffStatus {
    fn new(initial: GraphHandoffStatus) -> Self {
        Self {
            version: AtomicU64::new(0),
            active_revision: AtomicU64::new(raw_revision(initial.active_revision())),
            retired_revision: AtomicU64::new(raw_revision(initial.retired_revision())),
            swaps_applied: AtomicU64::new(initial.swaps_applied()),
            retirement_retries: AtomicU64::new(initial.retirement_retries()),
            incompatible_snapshots: AtomicU64::new(initial.incompatible_snapshots()),
        }
    }

    fn publish(&self, status: GraphHandoffStatus) {
        // The unique audio handle is the only writer. A release fence before
        // the payload pairs with the reader's acquire fence if it sees any
        // field from this publication, forcing its version check to retry.
        let version = self.version.load(Ordering::Relaxed);
        self.version
            .store(version.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        self.active_revision
            .store(raw_revision(status.active_revision()), Ordering::Relaxed);
        self.retired_revision
            .store(raw_revision(status.retired_revision()), Ordering::Relaxed);
        self.swaps_applied
            .store(status.swaps_applied(), Ordering::Relaxed);
        self.retirement_retries
            .store(status.retirement_retries(), Ordering::Relaxed);
        self.incompatible_snapshots
            .store(status.incompatible_snapshots(), Ordering::Relaxed);
        self.version
            .store(version.wrapping_add(2), Ordering::Release);
    }

    fn read(&self) -> GraphHandoffStatus {
        loop {
            let before = self.version.load(Ordering::Acquire);
            if before & 1 != 0 {
                core::hint::spin_loop();
                continue;
            }

            let status = GraphHandoffStatus::from_raw_parts(
                revision_from_raw(self.active_revision.load(Ordering::Relaxed)),
                revision_from_raw(self.retired_revision.load(Ordering::Relaxed)),
                self.swaps_applied.load(Ordering::Relaxed),
                self.retirement_retries.load(Ordering::Relaxed),
                self.incompatible_snapshots.load(Ordering::Relaxed),
            );
            // An acquire load alone does not order the preceding payload
            // loads before this final version check on weakly ordered CPUs.
            fence(Ordering::Acquire);
            let after = self.version.load(Ordering::Relaxed);
            if before == after {
                return status;
            }
            core::hint::spin_loop();
        }
    }
}

const fn raw_revision(revision: Option<GraphRevision>) -> u64 {
    match revision {
        Some(revision) => revision.value(),
        None => 0,
    }
}

fn revision_from_raw(value: u64) -> Option<GraphRevision> {
    GraphRevision::new(value).ok()
}

#[cfg(test)]
mod tests {
    use super::{
        AtomicGraphHandoffStatus, LockFreeStructuralGraphBoundary,
        StructuralBoundaryConfigurationError,
    };
    use crate::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID;
    use crate::kernel::midi_channel::MidiChannel;
    use crate::kernel::midi_message::MidiMessage;
    use crate::kernel::patch_id::PatchId;
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_state::MixerState;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::graph_handoff_status::GraphHandoffStatus;
    use crate::real_time::graph_revision::GraphRevision;
    use crate::real_time::parameter_snapshot::{ParameterSnapshot, RtPatchParameters};
    use crate::real_time::prepared_graph::PreparedGraph;
    use crate::real_time::prepared_graph_builder::PreparedGraphBuilder;
    use crate::real_time::structural_graph_boundary::{
        AudioStructuralGraphBoundary, ControlStructuralGraphBoundary, StructuralGraphBoundary,
    };
    use crate::synth::capability_id::CapabilityId;
    use crate::synth::instrument_preparer::{InstrumentPreparationError, InstrumentPreparer};
    use crate::synth::patch::Patch;
    use crate::synth::prepared_instrument::{PreparedInstrument, PreparedInstrumentError};
    use crate::synth::sound_font_instrument::SoundFontInstrument;
    use crate::testing::automatic_midi_test::create_soundfont_config;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct DropPreparer {
        capability_id: CapabilityId,
        drops: Arc<AtomicUsize>,
    }

    impl InstrumentPreparer for DropPreparer {
        fn capability_id(&self) -> &CapabilityId {
            &self.capability_id
        }

        fn prepare(
            &self,
            patch: &Patch,
            _sample_rate: f32,
            _max_frames: usize,
        ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
            Ok(Box::new(DropInstrument {
                patch_id: patch.id(),
                drops: Arc::clone(&self.drops),
            }))
        }
    }

    struct DropInstrument {
        patch_id: PatchId,
        drops: Arc<AtomicUsize>,
    }

    impl PreparedInstrument for DropInstrument {
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

    impl Drop for DropInstrument {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn graph(revision: u64, drops: &Arc<AtomicUsize>) -> PreparedGraph {
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        let registry = provider.registry().unwrap();
        let patch = Patch::new(
            PatchId::new(1).unwrap(),
            "Patch".to_owned(),
            create_soundfont_config(&provider, SoundFontInstrument::new(0, 0, false).unwrap())
                .unwrap(),
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        );
        let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(DropPreparer {
            capability_id: CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap(),
            drops: Arc::clone(drops),
        })];
        let revision = GraphRevision::new(revision).unwrap();
        let parameters = ParameterSnapshot::for_graph(
            revision.value(),
            revision,
            GlobalParameters::new(0.0).unwrap(),
            MixerState::default(),
            &[RtPatchParameters::new(patch.id(), PatchOutput::default())],
        )
        .unwrap();
        PreparedGraphBuilder::new(&registry, &preparers)
            .build(revision, &[patch], parameters, 48_000.0, 8)
            .unwrap()
    }

    #[test]
    fn distinct_queues_preserve_owned_graphs_and_only_control_collection_drops() {
        let drops = Arc::new(AtomicUsize::new(0));
        let boundary = LockFreeStructuralGraphBoundary::new(
            1,
            1,
            GraphHandoffStatus::with_active(GraphRevision::new(1).unwrap()),
        )
        .unwrap();
        let (mut control, mut audio) = boundary.into_handles();

        control
            .publish_prepared_on_control(graph(2, &drops))
            .unwrap();
        audio.return_retired_on_audio(graph(1, &drops)).unwrap();
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        let taken = audio.take_prepared_on_audio().unwrap();
        assert_eq!(taken.revision(), GraphRevision::new(2).unwrap());
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        // Explicit test cleanup occurs outside the boundary operation.
        drop(taken);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        assert_eq!(
            control.collect_retired_on_control(),
            Some(GraphRevision::new(1).unwrap())
        );
        assert_eq!(drops.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn full_queues_return_the_exact_graph_without_callback_destruction() {
        let drops = Arc::new(AtomicUsize::new(0));
        let boundary = LockFreeStructuralGraphBoundary::new(
            1,
            1,
            GraphHandoffStatus::with_active(GraphRevision::new(1).unwrap()),
        )
        .unwrap();
        let (mut control, mut audio) = boundary.into_handles();

        control
            .publish_prepared_on_control(graph(2, &drops))
            .unwrap();
        let rejected = control
            .publish_prepared_on_control(graph(3, &drops))
            .unwrap_err()
            .into_graph();
        assert_eq!(rejected.revision(), GraphRevision::new(3).unwrap());

        audio.return_retired_on_audio(graph(1, &drops)).unwrap();
        let retained = audio
            .return_retired_on_audio(graph(4, &drops))
            .unwrap_err()
            .into_graph();
        assert_eq!(retained.revision(), GraphRevision::new(4).unwrap());
        assert_eq!(drops.load(Ordering::Relaxed), 0);

        drop(rejected);
        drop(retained);
        assert_eq!(drops.load(Ordering::Relaxed), 2);
        assert_eq!(
            control.collect_retired_on_control(),
            Some(GraphRevision::new(1).unwrap())
        );
        assert_eq!(drops.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn status_is_latest_wins_coherent_and_never_backpressures_audio() {
        let status = Arc::new(AtomicGraphHandoffStatus::new(GraphHandoffStatus::default()));
        let writer = Arc::clone(&status);
        let thread = std::thread::spawn(move || {
            for value in 1..=20_000_u64 {
                writer.publish(GraphHandoffStatus::from_raw_parts(
                    GraphRevision::new(value).ok(),
                    GraphRevision::new(value).ok(),
                    value,
                    value,
                    value,
                ));
            }
        });

        while status.read().swaps_applied() < 20_000 {
            let snapshot = status.read();
            let value = snapshot.swaps_applied();
            assert_eq!(snapshot.retirement_retries(), value);
            assert_eq!(snapshot.incompatible_snapshots(), value);
            let expected = (value != 0).then_some(value);
            assert_eq!(
                snapshot.active_revision().map(GraphRevision::value),
                expected
            );
            assert_eq!(
                snapshot.retired_revision().map(GraphRevision::value),
                expected
            );
        }
        thread.join().unwrap();
    }

    #[test]
    fn boundary_rejects_zero_queue_capacities() {
        assert!(matches!(
            LockFreeStructuralGraphBoundary::new(0, 1, GraphHandoffStatus::default()),
            Err(StructuralBoundaryConfigurationError::InvalidPreparedCapacity)
        ));
        assert!(matches!(
            LockFreeStructuralGraphBoundary::new(1, 0, GraphHandoffStatus::default()),
            Err(StructuralBoundaryConfigurationError::InvalidRetiredCapacity)
        ));
    }
}
