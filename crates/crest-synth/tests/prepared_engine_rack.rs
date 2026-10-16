use core::alloc::{GlobalAlloc, Layout};
use core::cell::Cell;
use crest_synth::adapter::atomic_audio_observation::AtomicAudioObservation;
use crest_synth::adapter::hidef_soundfont_capability::{
    HiDefSoundFontCapability, HIDEF_CAPABILITY_ID,
};
use crest_synth::adapter::hidef_soundfont_preparer::HiDefSoundFontPreparer;
use crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
use crest_synth::adapter::lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary;
use crest_synth::kernel::midi_channel::MidiChannel;
use crest_synth::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crest_synth::kernel::patch_id::PatchId;
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mixer_state::MixerState;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::audio_boundary::{AudioBoundary, ControlAudioBoundary};
use crest_synth::real_time::audio_command::AudioCommand;
use crest_synth::real_time::audio_observation::{AudioObservation, ControlAudioObservation};
use crest_synth::real_time::audio_renderer::AudioRenderer;
use crest_synth::real_time::graph_handoff_status::GraphHandoffStatus;
use crest_synth::real_time::graph_revision::GraphRevision;
use crest_synth::real_time::parameter_snapshot::{ParameterSnapshot, RtPatchParameters};
use crest_synth::real_time::prepared_graph::PreparedGraph;
use crest_synth::real_time::prepared_graph_builder::PreparedGraphBuilder;
use crest_synth::real_time::structural_graph_boundary::{
    AudioStructuralGraphBoundary, StructuralGraphBoundary,
};
use crest_synth::real_time::structural_graph_coordinator::{
    GraphPublicationFailure, StructuralGraphCoordinator,
};
use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
use crest_synth::synth::{
    CapabilityId, InstrumentPreparationError, InstrumentPreparer, Patch, PreparedEngineRackBuilder,
    PreparedInstrument, PreparedInstrumentError, RackPreparationError,
};
use crest_synth::testing::automatic_midi_test::create_soundfont_config;
use std::alloc::System;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const SAMPLE_RATE: f32 = 48_000.0;
const MAX_FRAMES: usize = 64;

thread_local! {
    static COUNT_MEMORY: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct AcceptanceAllocator;

#[global_allocator]
static ACCEPTANCE_ALLOCATOR: AcceptanceAllocator = AcceptanceAllocator;

unsafe impl GlobalAlloc for AcceptanceAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        record_deallocation();
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record_allocation();
        record_deallocation();
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

fn record_allocation() {
    let _ = COUNT_MEMORY.try_with(|enabled| {
        if enabled.get() {
            let _ = ALLOCATIONS.try_with(|count| count.set(count.get().saturating_add(1)));
        }
    });
}

fn record_deallocation() {
    let _ = COUNT_MEMORY.try_with(|enabled| {
        if enabled.get() {
            let _ = DEALLOCATIONS.try_with(|count| count.set(count.get().saturating_add(1)));
        }
    });
}

fn begin_memory_count() {
    ALLOCATIONS.with(|count| count.set(0));
    DEALLOCATIONS.with(|count| count.set(0));
    COUNT_MEMORY.with(|enabled| enabled.set(true));
}

fn finish_memory_count() -> (usize, usize) {
    COUNT_MEMORY.with(|enabled| enabled.set(false));
    (ALLOCATIONS.with(Cell::get), DEALLOCATIONS.with(Cell::get))
}

#[derive(Default)]
struct Probe {
    first_dispatches: AtomicUsize,
    second_dispatches: AtomicUsize,
    first_all_notes_off: AtomicUsize,
    second_all_notes_off: AtomicUsize,
    instrument_drops: AtomicUsize,
}

struct HeterogeneousPreparer {
    capability_id: CapabilityId,
    probe: Arc<Probe>,
    fail_patch: Option<PatchId>,
}

impl HeterogeneousPreparer {
    fn boxed(probe: &Arc<Probe>) -> Box<dyn InstrumentPreparer> {
        Self::boxed_with_failure(probe, None)
    }

    fn boxed_with_failure(
        probe: &Arc<Probe>,
        fail_patch: Option<PatchId>,
    ) -> Box<dyn InstrumentPreparer> {
        Box::new(Self {
            capability_id: CapabilityId::new(HIDEF_CAPABILITY_ID)
                .expect("the production capability identity is valid"),
            probe: Arc::clone(probe),
            fail_patch,
        })
    }
}

impl InstrumentPreparer for HeterogeneousPreparer {
    fn capability_id(&self) -> &CapabilityId {
        &self.capability_id
    }

    fn prepare(
        &self,
        patch: &Patch,
        _sample_rate: f32,
        _max_frames: usize,
    ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
        if self.fail_patch == Some(patch.id()) {
            return Err(InstrumentPreparationError::PreparationFailed {
                patch_id: patch.id(),
            });
        }
        if patch.id().value() % 2 == 1 {
            Ok(Box::new(FirstInstrument {
                patch_id: patch.id(),
                probe: Arc::clone(&self.probe),
            }))
        } else {
            Ok(Box::new(SecondInstrument {
                patch_id: patch.id(),
                probe: Arc::clone(&self.probe),
            }))
        }
    }
}

struct FirstInstrument {
    patch_id: PatchId,
    probe: Arc<Probe>,
}

impl PreparedInstrument for FirstInstrument {
    fn patch_id(&self) -> PatchId {
        self.patch_id
    }

    fn dispatch(
        &mut self,
        _message: MidiMessage,
        _parameters: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        self.probe.first_dispatches.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn render(
        &mut self,
        output: &mut [f32],
        _frame_count: usize,
        _parameters: &RtPatchParameters,
    ) -> Result<(), crest_synth::synth::PreparedInstrumentError> {
        output.fill(0.125);

        Ok(())
    }

    fn all_notes_off(&mut self) {
        self.probe
            .first_all_notes_off
            .fetch_add(1, Ordering::Relaxed);
    }
}

impl Drop for FirstInstrument {
    fn drop(&mut self) {
        self.probe.instrument_drops.fetch_add(1, Ordering::Relaxed);
    }
}

struct SecondInstrument {
    patch_id: PatchId,
    probe: Arc<Probe>,
}

impl PreparedInstrument for SecondInstrument {
    fn patch_id(&self) -> PatchId {
        self.patch_id
    }

    fn dispatch(
        &mut self,
        _message: MidiMessage,
        _parameters: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        self.probe.second_dispatches.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn render(
        &mut self,
        output: &mut [f32],
        _frame_count: usize,
        _parameters: &RtPatchParameters,
    ) -> Result<(), crest_synth::synth::PreparedInstrumentError> {
        output.fill(0.375);

        Ok(())
    }

    fn all_notes_off(&mut self) {
        self.probe
            .second_all_notes_off
            .fetch_add(1, Ordering::Relaxed);
    }
}

impl Drop for SecondInstrument {
    fn drop(&mut self) {
        self.probe.instrument_drops.fetch_add(1, Ordering::Relaxed);
    }
}

fn globals() -> GlobalParameters {
    GlobalParameters::new(0.0).expect("acceptance global parameters are valid")
}

fn patches(provider: &HiDefSoundFontCapability) -> [Patch; 2] {
    [patch(provider, 1, 0, 0), patch(provider, 2, 1, 48)]
}

fn patch(provider: &HiDefSoundFontCapability, id: u32, channel: u8, program: u8) -> Patch {
    Patch::new(
        PatchId::new(id).expect("acceptance Patch identity is valid"),
        format!("Acceptance Patch {id}"),
        create_soundfont_config(
            provider,
            SoundFontInstrument::new(0, program, false)
                .expect("acceptance SoundFont identity is valid"),
        )
        .expect("acceptance config matches the production descriptor"),
        MidiChannel::new(channel).expect("acceptance MIDI channel is valid"),
        PatchOutput::to_track(MixerTrackId::new(channel).expect("acceptance mixer track is valid")),
    )
}

fn parameters(patches: &[Patch], revision: GraphRevision, generation: u64) -> ParameterSnapshot {
    let values: Vec<_> = patches
        .iter()
        .map(|patch| RtPatchParameters::new(patch.id(), patch.output()))
        .collect();
    ParameterSnapshot::for_graph(
        generation,
        revision,
        globals(),
        MixerState::default(),
        &values,
    )
    .expect("acceptance parameters fit fixed storage")
}

fn graph(
    provider: &HiDefSoundFontCapability,
    patches: &[Patch],
    probe: &Arc<Probe>,
    revision: u64,
    generation: u64,
) -> PreparedGraph {
    let registry = provider
        .registry()
        .expect("production capability registry is valid");
    let preparers = vec![HeterogeneousPreparer::boxed(probe)];
    let revision = GraphRevision::new(revision).expect("graph revision is nonzero");
    PreparedGraphBuilder::new(&registry, &preparers)
        .build(
            revision,
            patches,
            parameters(patches, revision, generation),
            SAMPLE_RATE,
            MAX_FRAMES,
        )
        .expect("complete acceptance graph prepares")
}

fn note_on(channel: MidiChannel) -> MidiMessage {
    MidiMessage::try_new(channel, MidiMessageKind::NoteOn, 60, 100)
        .expect("acceptance MIDI message is valid")
}

#[test]
fn prepared_engine_rack_acceptance() {
    let provider = crest_synth::adapter::production_instruments::production_soundfont_capability()
        .expect("production capability is valid");
    let patches = patches(&provider);

    prove_atomic_capability_matching(&provider, &patches);
    prove_hidef_preparation(&patches[0]);

    let probe = Arc::new(Probe::default());
    let graph_one = graph(&provider, &patches, &probe, 1, 10);
    let graph_two = graph(&provider, &patches, &probe, 2, 20);
    let graph_three = graph(&provider, &patches, &probe, 3, 30);
    let retirement_blocker = graph(&provider, &patches, &probe, 8, 80);

    let initial_parameters = parameters(&patches, GraphRevision::INITIAL, 10);
    let boundary = LockFreeAudioBoundary::new(8, initial_parameters);
    let (mut control, audio) = boundary.into_handles();
    let structural = LockFreeStructuralGraphBoundary::new(
        1,
        1,
        GraphHandoffStatus::with_active(GraphRevision::INITIAL),
    )
    .expect("structural queues preallocate");
    let (structural_control, mut structural_audio) = structural.into_handles();
    let mut coordinator = StructuralGraphCoordinator::new(structural_control, &graph_one);
    coordinator
        .publish(graph_two)
        .expect("one compatible replacement publishes");
    let early = coordinator
        .publish(graph_three)
        .expect_err("a second graph is throttled while one is in flight");
    assert_eq!(early.reason(), GraphPublicationFailure::ReplacementInFlight);
    let graph_three = early.into_graph();
    structural_audio
        .return_retired_on_audio(retirement_blocker)
        .expect("the retirement blocker fills the return queue");
    let (observation_writer, observation_reader) = AtomicAudioObservation::default().into_handles();
    let mut renderer =
        AudioRenderer::with_observation(audio, structural_audio, graph_one, observation_writer);
    let mut output = [0.0_f32; MAX_FRAMES * 2];
    assert_eq!(renderer.active_revision(), GraphRevision::INITIAL);

    control
        .push_command(AudioCommand::patch_midi(
            patches[1].id(),
            note_on(patches[1].channel()),
        ))
        .expect("targeted command fits the discrete queue");
    control
        .push_command(AudioCommand::patch_midi(
            PatchId::new(99).expect("unknown probe identity is valid"),
            note_on(patches[0].channel()),
        ))
        .expect("unknown command still fits the discrete queue");
    control.publish_parameters(parameters(&patches, GraphRevision::new(2).unwrap(), 21));

    begin_memory_count();
    renderer.render(&mut output);
    let (allocations, deallocations) = finish_memory_count();
    assert_eq!((allocations, deallocations), (0, 0));
    assert_eq!(probe.instrument_drops.load(Ordering::Relaxed), 0);
    assert_eq!(probe.first_dispatches.load(Ordering::Relaxed), 0);
    assert_eq!(probe.second_dispatches.load(Ordering::Relaxed), 1);
    let routing = observation_reader.read_latest_on_control();
    assert_eq!(routing.routing_failures(), 1);
    assert_eq!(
        routing.last_unknown_patch_id(),
        Some(PatchId::new(99).unwrap())
    );
    assert_eq!(renderer.active_revision(), GraphRevision::new(2).unwrap());
    assert_eq!(renderer.parameters().generation(), 21);
    assert_eq!(
        renderer.pending_retirement_revision(),
        Some(GraphRevision::INITIAL)
    );
    assert_eq!(renderer.handoff_status().swaps_applied(), 1);
    assert_eq!(renderer.handoff_status().retirement_retries(), 1);
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(output
        .iter()
        .all(|sample| (*sample - 0.5).abs() < 0.000_001));
    let stems = renderer.active_patch_audio();
    assert!(stems
        .stem(0, patches[0].id())
        .expect("first exact stem exists")
        .samples()
        .iter()
        .all(|sample| (*sample - 0.125).abs() < f32::EPSILON));
    assert!(stems
        .stem(1, patches[1].id())
        .expect("second exact stem exists")
        .samples()
        .iter()
        .all(|sample| (*sample - 0.375).abs() < f32::EPSILON));

    control
        .push_command(AudioCommand::all_notes_off())
        .expect("global silence command fits");
    control.publish_parameters(parameters(&patches, GraphRevision::INITIAL, 12));
    begin_memory_count();
    renderer.render(&mut output);
    let (allocations, deallocations) = finish_memory_count();
    assert_eq!((allocations, deallocations), (0, 0));
    assert_eq!(probe.instrument_drops.load(Ordering::Relaxed), 0);
    assert_eq!(probe.first_all_notes_off.load(Ordering::Relaxed), 1);
    assert_eq!(probe.second_all_notes_off.load(Ordering::Relaxed), 1);
    assert_eq!(renderer.parameters().generation(), 21);
    assert_eq!(renderer.handoff_status().incompatible_snapshots(), 1);
    assert_eq!(
        renderer.pending_retirement_revision(),
        Some(GraphRevision::INITIAL)
    );
    assert_eq!(renderer.handoff_status().swaps_applied(), 1);
    assert_eq!(renderer.handoff_status().retirement_retries(), 2);

    let drained_blocker = coordinator.poll();
    assert_eq!(drained_blocker.collected_count(), 1);
    assert_eq!(drained_blocker.completed_revision(), None);
    assert_eq!(probe.instrument_drops.load(Ordering::Relaxed), 2);

    control.publish_parameters(parameters(&patches, GraphRevision::new(2).unwrap(), 22));
    begin_memory_count();
    renderer.render(&mut output);
    let (allocations, deallocations) = finish_memory_count();
    assert_eq!((allocations, deallocations), (0, 0));
    assert_eq!(probe.instrument_drops.load(Ordering::Relaxed), 2);
    assert_eq!(renderer.pending_retirement_revision(), None);
    assert_eq!(renderer.parameters().generation(), 22);

    let completed_two = coordinator.poll();
    assert_eq!(completed_two.collected_count(), 1);
    assert_eq!(
        completed_two.completed_revision(),
        Some(GraphRevision::new(2).unwrap())
    );
    assert_eq!(probe.instrument_drops.load(Ordering::Relaxed), 4);

    coordinator
        .publish(graph_three)
        .expect("the next graph publishes after retirement acknowledgement");
    control.publish_parameters(parameters(&patches, GraphRevision::new(3).unwrap(), 31));
    begin_memory_count();
    renderer.render(&mut output);
    let (allocations, deallocations) = finish_memory_count();
    assert_eq!((allocations, deallocations), (0, 0));
    assert_eq!(probe.instrument_drops.load(Ordering::Relaxed), 4);
    assert_eq!(renderer.active_revision(), GraphRevision::new(3).unwrap());
    assert_eq!(renderer.parameters().generation(), 31);

    let completed_three = coordinator.poll();
    assert_eq!(completed_three.collected_count(), 1);
    assert_eq!(
        completed_three.completed_revision(),
        Some(GraphRevision::new(3).unwrap())
    );
    assert_eq!(completed_three.status().swaps_applied(), 2);
    assert_eq!(
        completed_three.status().retired_revision(),
        Some(GraphRevision::new(2).unwrap())
    );
    assert_eq!(probe.instrument_drops.load(Ordering::Relaxed), 6);

    drop(renderer);
    assert_eq!(probe.instrument_drops.load(Ordering::Relaxed), 8);
    println!("CREST_ACCEPTANCE prepared_engine_rack passed");
}

fn prove_atomic_capability_matching(provider: &HiDefSoundFontCapability, patches: &[Patch]) {
    let registry = provider
        .registry()
        .expect("production capability registry is valid");
    let probe = Arc::new(Probe::default());
    let no_preparers: Vec<Box<dyn InstrumentPreparer>> = Vec::new();
    assert!(matches!(
        PreparedEngineRackBuilder::build(
            patches,
            &registry,
            &no_preparers,
            SAMPLE_RATE,
            MAX_FRAMES,
        ),
        Err(RackPreparationError::MissingPreparer { .. })
    ));

    let duplicates = vec![
        HeterogeneousPreparer::boxed(&probe),
        HeterogeneousPreparer::boxed(&probe),
    ];
    assert!(matches!(
        PreparedEngineRackBuilder::build(patches, &registry, &duplicates, SAMPLE_RATE, MAX_FRAMES,),
        Err(RackPreparationError::DuplicatePreparer { .. })
    ));
    assert_eq!(probe.instrument_drops.load(Ordering::Relaxed), 0);

    let partial_failure = vec![HeterogeneousPreparer::boxed_with_failure(
        &probe,
        Some(patches[1].id()),
    )];
    assert!(matches!(
        PreparedEngineRackBuilder::build(
            patches,
            &registry,
            &partial_failure,
            SAMPLE_RATE,
            MAX_FRAMES,
        ),
        Err(RackPreparationError::Instrument { .. })
    ));
    assert_eq!(probe.instrument_drops.load(Ordering::Relaxed), 1);
}

fn prove_hidef_preparation(patch: &Patch) {
    let preparer = HiDefSoundFontPreparer::new(
        crest_synth::adapter::production_instruments::production_soundfont_asset().unwrap(),
    )
    .expect("the production SoundFont parses");
    assert_eq!(preparer.parsed_bank_count(), 1);
    assert_eq!(preparer.prepared_shared_asset_count(), 1);
    let mut instrument = preparer
        .prepare(patch, SAMPLE_RATE, 512)
        .expect("the production SoundFont Patch prepares");
    let mut output = [0.0_f32; 1_024];
    let parameters = RtPatchParameters::new(patch.id(), patch.output());

    begin_memory_count();
    instrument
        .dispatch(note_on(patch.channel()), &parameters)
        .expect("prepared HiDef MIDI dispatch succeeds");
    instrument.render(&mut output, 512, &parameters).unwrap();
    instrument.all_notes_off();
    let (allocations, deallocations) = finish_memory_count();

    assert_eq!((allocations, deallocations), (0, 0));
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(output.iter().any(|sample| sample.abs() > 0.000_001));
}
