//! Host-frame scheduling through the production queue, graph, renderer and
//! observation transport. The step instrument makes event positions exact;
//! this does not claim zero latency inside every upstream DSP implementation.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::Cell;
use crest_synth::adapter::atomic_audio_observation::{
    AtomicAudioObservation, AtomicAudioObservationReader, AtomicAudioObservationWriter,
};
use crest_synth::adapter::braids_capability::{
    BraidsCapability, BRAIDS_MODEL_PARAMETER_ID, BRAIDS_TIMBRE_PARAMETER_ID,
};
use crest_synth::adapter::braids_native::braids_lifecycle_counts;
use crest_synth::adapter::braids_preparer::BraidsPreparer;
use crest_synth::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID;
use crest_synth::adapter::lock_free_audio_boundary::{
    LockFreeAudioBoundary, LockFreeAudioHandle, LockFreeControlHandle,
};
use crest_synth::adapter::lock_free_structural_graph_boundary::{
    LockFreeStructuralAudioHandle, LockFreeStructuralControlHandle, LockFreeStructuralGraphBoundary,
};
use crest_synth::control::app_event::AppEvent;
use crest_synth::control::app_loop::AppLoop;
use crest_synth::control::app_state::AppState;
use crest_synth::control::event_record::EventSource;
use crest_synth::control::state_projector::StateProjector;
use crest_synth::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crest_synth::kernel::{MidiChannel, PatchId};
use crest_synth::mixer::bus_id::BusId;
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mixer_state::MixerState;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::audio_observation::{AudioObservation, ControlAudioObservation};
use crest_synth::real_time::parameter_snapshot::RtPatchParameters;
use crest_synth::real_time::scheduled_event::ScheduledEvent;
use crest_synth::real_time::{
    AudioBoundary, AudioCommand, AudioRenderer, ControlAudioBoundary,
    ControlStructuralGraphBoundary, GraphHandoffStatus, GraphRevision, ParameterSnapshot,
    PreparedGraph, PreparedGraphBuilder, RtBusReturnParameters, RtPostEffectParameters, SendLease,
    SendLeaseId, StructuralGraphBoundary,
};
use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
use crest_synth::synth::{
    CapabilityId, CapabilityRegistry, EffectSlotId, InstrumentCapabilityProvider,
    InstrumentPreparationError, InstrumentPreparer, MidiExpressionScope, MidiSource, MidiVoiceId,
    OwnedExpressionState, OwnedVoiceInitialization, ParameterId, ParameterValue, Patch,
    PreparedEffectError, PreparedInstrument, PreparedInstrumentError, PreparedPostEffect,
    VoiceEnvelope,
};
use crest_synth::testing::automatic_midi_test::create_soundfont_config;
use std::alloc::System;

thread_local! {
    static COUNT_MEMORY: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct AuditAllocator;
#[global_allocator]
static ALLOCATOR: AuditAllocator = AuditAllocator;

unsafe impl GlobalAlloc for AuditAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_memory(true);
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_memory(true);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        record_memory(false);
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record_memory(true);
        record_memory(false);
        unsafe { System.realloc(pointer, layout, size) }
    }
}

fn record_memory(allocation: bool) {
    let _ = COUNT_MEMORY.try_with(|enabled| {
        if enabled.get() {
            let counter = if allocation {
                &ALLOCATIONS
            } else {
                &DEALLOCATIONS
            };
            let _ = counter.try_with(|count| count.set(count.get() + 1));
        }
    });
}

struct StepPreparer(CapabilityId);

impl InstrumentPreparer for StepPreparer {
    fn capability_id(&self) -> &CapabilityId {
        &self.0
    }

    fn prepare(
        &self,
        patch: &Patch,
        _sample_rate: f32,
        _max_frames: usize,
    ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
        Ok(Box::new(StepInstrument {
            patch_id: patch.id(),
            voices: [[0; 128]; MidiSource::COUNT],
            held: [[false; 128]; MidiSource::COUNT],
            sustain: [false; MidiSource::COUNT],
        }))
    }
}

/// Each source owns its voices and sustain, as required by hosted playback.
struct StepInstrument {
    patch_id: PatchId,
    voices: [[u8; 128]; MidiSource::COUNT],
    held: [[bool; 128]; MidiSource::COUNT],
    sustain: [bool; MidiSource::COUNT],
}

impl PreparedInstrument for StepInstrument {
    fn patch_id(&self) -> PatchId {
        self.patch_id
    }

    fn dispatch(
        &mut self,
        message: MidiMessage,
        parameters: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        self.dispatch_from(MidiSource::Immediate, message, parameters)
    }

    fn dispatch_from(
        &mut self,
        source: MidiSource,
        message: MidiMessage,
        _: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        let lane = source.index();
        let note = usize::from(message.data1());
        match message.kind() {
            MidiMessageKind::NoteOn if message.data2() > 0 => {
                self.voices[lane][note] = self.voices[lane][note].saturating_add(1);
                self.held[lane][note] = true;
            }
            MidiMessageKind::NoteOn | MidiMessageKind::NoteOff => {
                self.held[lane][note] = false;
                if !self.sustain[lane] {
                    self.voices[lane][note] = 0;
                }
            }
            MidiMessageKind::ControlChange if message.data1() == 64 => {
                self.sustain[lane] = message.data2() >= 64;
                if !self.sustain[lane] {
                    self.release_unheld(source);
                }
            }
            MidiMessageKind::AllNotesOff => return self.all_notes_off_from(source),
            _ => {}
        }
        Ok(())
    }

    fn render(
        &mut self,
        output: &mut [f32],
        _: usize,
        _: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        let count: u32 = self
            .voices
            .iter()
            .flatten()
            .map(|count| u32::from(*count))
            .sum();
        output.fill(count as f32 * 0.1);
        Ok(())
    }

    fn all_notes_off(&mut self) {
        self.voices.fill([0; 128]);
        self.held.fill([false; 128]);
        self.sustain.fill(false);
    }

    fn all_notes_off_from(&mut self, source: MidiSource) -> Result<(), PreparedInstrumentError> {
        self.voices[source.index()].fill(0);
        self.held[source.index()].fill(false);
        self.sustain[source.index()] = false;
        Ok(())
    }
}

impl StepInstrument {
    fn release_unheld(&mut self, source: MidiSource) {
        for (voice, held) in self.voices[source.index()]
            .iter_mut()
            .zip(self.held[source.index()])
        {
            if !held {
                *voice = 0;
            }
        }
    }
}

type Renderer =
    AudioRenderer<LockFreeAudioHandle, LockFreeStructuralAudioHandle, AtomicAudioObservationWriter>;

struct Fixture<Control = LockFreeControlHandle> {
    renderer: Renderer,
    control: Control,
    structural: LockFreeStructuralControlHandle,
    observation: AtomicAudioObservationReader,
}

fn graph(revision: GraphRevision) -> PreparedGraph {
    let provider =
        crest_synth::adapter::production_instruments::production_soundfont_capability().unwrap();
    let patches: Vec<_> = (1..=2)
        .map(|id| {
            Patch::new(
                PatchId::new(id).unwrap(),
                format!("Timing Patch {id}"),
                create_soundfont_config(&provider, SoundFontInstrument::new(0, 0, false).unwrap())
                    .unwrap(),
                MidiChannel::new((id - 1) as u8).unwrap(),
                PatchOutput::to_track(MixerTrackId::new((id - 1) as u8).unwrap()),
            )
        })
        .collect();
    let registry = provider.registry().unwrap();
    let parameters = ParameterSnapshot::project_patches(
        revision.value(),
        revision,
        GlobalParameters::new(0.0).unwrap(),
        MixerState::default(),
        &patches,
        &registry,
    )
    .unwrap();
    let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(StepPreparer(
        CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap(),
    ))];
    PreparedGraphBuilder::new(&registry, &preparers)
        .build(revision, &patches, parameters, 48_000.0, 16)
        .unwrap()
}

impl Fixture {
    fn new(budget: usize) -> Self {
        Self::with_graph(graph(GraphRevision::INITIAL), budget)
    }

    fn with_graph(graph: PreparedGraph, budget: usize) -> Self {
        Self::with_graph_capacity(graph, budget, 16)
    }

    fn with_graph_capacity(graph: PreparedGraph, budget: usize, capacity: usize) -> Self {
        let (control, audio) =
            LockFreeAudioBoundary::new(capacity, graph.initial_parameters().clone()).into_handles();
        let (structural, structural_audio) = LockFreeStructuralGraphBoundary::new(
            1,
            1,
            GraphHandoffStatus::with_active(GraphRevision::INITIAL),
        )
        .unwrap()
        .into_handles();
        let (writer, observation) = AtomicAudioObservation::default().into_handles();
        let renderer = AudioRenderer::with_observation(audio, structural_audio, graph, writer)
            .with_command_budget(core::num::NonZeroUsize::new(budget).unwrap());
        Self {
            renderer,
            control,
            structural,
            observation,
        }
    }

    fn schedule(&mut self, frame: u64, patch: u32, kind: MidiMessageKind, note: u8) {
        self.schedule_message(frame, patch, midi(kind, note, 100));
    }

    fn schedule_message(&mut self, frame: u64, patch: u32, message: MidiMessage) {
        self.schedule_voice(frame, patch, None, message);
    }

    fn schedule_voice(
        &mut self,
        frame: u64,
        patch: u32,
        voice: Option<MidiVoiceId>,
        message: MidiMessage,
    ) {
        self.control
            .push_scheduled(
                ScheduledEvent::new(
                    frame,
                    self.control.scheduled_epoch(),
                    self.renderer.active_revision(),
                    PatchId::new(patch).unwrap(),
                    message,
                )
                .with_voice(voice),
            )
            .unwrap();
    }

    fn immediate(&mut self, patch: u32, message: MidiMessage) {
        self.control
            .push_command(AudioCommand::patch_midi(
                PatchId::new(patch).unwrap(),
                message,
            ))
            .unwrap();
    }
}

impl<Control> Fixture<Control> {
    fn render(&mut self) -> [f32; 32] {
        let mut output = [0.0; 32];
        ALLOCATIONS.with(|count| count.set(0));
        DEALLOCATIONS.with(|count| count.set(0));
        COUNT_MEMORY.with(|enabled| enabled.set(true));
        self.renderer.render(&mut output);
        COUNT_MEMORY.with(|enabled| enabled.set(false));
        assert_eq!(
            (ALLOCATIONS.with(Cell::get), DEALLOCATIONS.with(Cell::get)),
            (0, 0)
        );
        output
    }
}

fn midi(kind: MidiMessageKind, data1: u8, data2: u8) -> MidiMessage {
    MidiMessage::try_new(MidiChannel::new(0).unwrap(), kind, data1, data2).unwrap()
}

/// The return copies its input, isolating exact send arithmetic from DSP tails.
struct UnitySendReturn;

impl PreparedPostEffect for UnitySendReturn {
    fn patch_id(&self) -> PatchId {
        PatchId::new(u32::MAX).unwrap()
    }
    fn slot_id(&self) -> EffectSlotId {
        EffectSlotId::new(1).unwrap()
    }
    fn process(
        &mut self,
        _: &mut [f32],
        _: usize,
        _: &RtPostEffectParameters,
    ) -> Result<(), PreparedEffectError> {
        Ok(())
    }
}

fn with_base_send(parameters: &ParameterSnapshot, amount: f32) -> ParameterSnapshot {
    let mut patches = parameters.patches().to_vec();
    let mut sends = patches[0].sends().to_vec();
    sends[0] = amount;
    patches[0] = patches[0].clone().with_sends(&sends).unwrap();
    ParameterSnapshot::for_graph_with_returns(
        parameters.generation() + 1,
        parameters.graph_revision(),
        *parameters.global(),
        MixerState::new(parameters.mixer_tracks().clone()),
        &patches,
        parameters.returns().to_vec(),
    )
    .unwrap()
}

fn send_graph(revision: GraphRevision, base: f32) -> PreparedGraph {
    let mut graph = graph(revision);
    let slot = EffectSlotId::new(1).unwrap();
    graph
        .callback_parts_mut()
        .2
        .install_bus_return(
            BusId::default(),
            Box::new(UnitySendReturn),
            RtPostEffectParameters::new(slot, &[]).unwrap(),
            1.0,
        )
        .unwrap();
    let mut returns = graph.initial_parameters().returns().to_vec();
    returns[0] = RtBusReturnParameters::new(slot, &[], 1.0).unwrap();
    let parameters = with_base_send(graph.initial_parameters(), base).with_returns(returns);
    graph.refresh_initial_parameters(parameters).unwrap();
    graph
}

fn schedule_send(fixture: &mut Fixture, frame: u64, id: u64, amount: f32, end: u64) {
    fixture
        .control
        .push_scheduled(ScheduledEvent::send_lease(
            frame,
            fixture.control.scheduled_epoch(),
            fixture.renderer.active_revision(),
            PatchId::new(1).unwrap(),
            SendLease::new(SendLeaseId::new(id).unwrap(), BusId::default(), amount, end).unwrap(),
        ))
        .unwrap();
}

fn schedule_send_end(fixture: &mut Fixture, frame: u64, id: u64) {
    fixture
        .control
        .push_scheduled(ScheduledEvent::end_send_lease(
            frame,
            fixture.control.scheduled_epoch(),
            fixture.renderer.active_revision(),
            PatchId::new(1).unwrap(),
            SendLeaseId::new(id).unwrap(),
        ))
        .unwrap();
}

fn assert_send_frames(output: [f32; 32], sends: impl Fn(usize) -> f32) {
    for (frame, stereo) in output.chunks_exact(2).enumerate() {
        let expected = 0.1 * (1.0 + sends(frame));
        assert!(
            (stereo[0] - expected).abs() < 1e-7,
            "frame {frame}: {} != {expected}",
            stereo[0]
        );
        assert_eq!(stereo[0], stereo[1]);
    }
}

#[test]
fn temporary_send_start_and_expiry_are_exact_without_changing_base_snapshot() {
    let mut fixture = Fixture::with_graph(send_graph(GraphRevision::INITIAL, 0.2), 16);
    let before = fixture.renderer.parameters().clone();
    fixture.immediate(1, midi(MidiMessageKind::NoteOn, 60, 100));
    schedule_send(&mut fixture, 3, 1, 0.6, 11);
    assert_send_frames(fixture.render(), |frame| {
        if (3..11).contains(&frame) {
            0.6
        } else {
            0.2
        }
    });
    assert_eq!(fixture.renderer.parameters(), &before);
    assert_eq!(
        fixture
            .observation
            .read_latest_on_control()
            .routing_failures(),
        0
    );
}

#[test]
fn overlapping_sends_restore_latest_manual_base_and_reveal_older_active_owner() {
    let mut fixture = Fixture::with_graph(send_graph(GraphRevision::INITIAL, 0.2), 16);
    fixture.immediate(1, midi(MidiMessageKind::NoteOn, 60, 100));
    schedule_send(&mut fixture, 0, 1, 0.35, 22);
    schedule_send(&mut fixture, 4, 2, 0.6, u64::MAX);
    assert_send_frames(fixture.render(), |frame| if frame < 4 { 0.35 } else { 0.6 });
    fixture
        .control
        .publish_parameters(with_base_send(fixture.renderer.parameters(), 0.25));
    assert_send_frames(fixture.render(), |_| 0.6);
    assert_eq!(fixture.renderer.parameters().patches()[0].sends()[0], 0.25);
    schedule_send_end(&mut fixture, 37, 2);
    assert_send_frames(fixture.render(), |frame| if frame < 5 { 0.6 } else { 0.25 });

    schedule_send(&mut fixture, 48, 3, 0.7, 56);
    schedule_send(&mut fixture, 50, 4, 0.8, u64::MAX);
    schedule_send_end(&mut fixture, 54, 4);
    assert_send_frames(fixture.render(), |frame| match frame {
        0..2 | 6..8 => 0.7,
        2..6 => 0.8,
        _ => 0.25,
    });
    assert_eq!(
        fixture
            .observation
            .read_latest_on_control()
            .routing_failures(),
        0
    );
}

#[test]
fn fixed_send_expiry_runs_even_when_immediate_commands_exhaust_block_budget() {
    let mut fixture = Fixture::with_graph(send_graph(GraphRevision::INITIAL, 0.2), 2);
    fixture.immediate(1, midi(MidiMessageKind::NoteOn, 60, 100));
    schedule_send(&mut fixture, 0, 1, 0.7, 20);
    assert_send_frames(fixture.render(), |_| 0.7);
    for _ in 0..2 {
        fixture.immediate(1, midi(MidiMessageKind::ControlChange, 11, 100));
    }
    assert_send_frames(fixture.render(), |frame| if frame < 4 { 0.7 } else { 0.2 });
    assert_eq!(
        fixture
            .observation
            .read_latest_on_control()
            .commands_consumed(),
        4
    );
}

#[test]
fn epoch_cancellation_and_graph_replacement_remove_sends_without_stale_restore() {
    let mut fixture = Fixture::with_graph(send_graph(GraphRevision::INITIAL, 0.2), 16);
    fixture.immediate(1, midi(MidiMessageKind::NoteOn, 60, 100));
    schedule_send(&mut fixture, 0, 1, 0.7, u64::MAX);
    assert_send_frames(fixture.render(), |_| 0.7);
    fixture.control.cancel_scheduled().unwrap();
    assert_send_frames(fixture.render(), |_| 0.2);
    schedule_send(&mut fixture, 32, 2, 0.8, u64::MAX);
    schedule_send(&mut fixture, 48, 3, 0.9, u64::MAX);
    assert_send_frames(fixture.render(), |_| 0.8);
    let next = GraphRevision::new(2).unwrap();
    fixture
        .structural
        .publish_prepared_on_control(send_graph(next, 0.3))
        .unwrap();
    fixture.immediate(1, midi(MidiMessageKind::NoteOn, 60, 100));
    assert_send_frames(fixture.render(), |_| 0.3);
    assert_eq!(
        fixture
            .observation
            .read_latest_on_control()
            .routing_failures(),
        1
    );
    assert_eq!(
        fixture.structural.collect_retired_on_control().unwrap(),
        GraphRevision::INITIAL
    );
}

#[test]
fn full_send_bank_reports_refusal_and_exact_end_frees_capacity_without_cutting_new_owner() {
    let mut fixture = Fixture::with_graph(send_graph(GraphRevision::INITIAL, 0.2), 16);
    fixture.renderer = fixture
        .renderer
        .with_send_lease_capacity(core::num::NonZeroUsize::new(1).unwrap())
        .unwrap();
    fixture.immediate(1, midi(MidiMessageKind::NoteOn, 60, 100));
    schedule_send(&mut fixture, 0, 1, 0.4, u64::MAX);
    schedule_send(&mut fixture, 2, 2, 0.9, u64::MAX);
    schedule_send_end(&mut fixture, 5, 1);
    schedule_send(&mut fixture, 6, 3, 0.7, u64::MAX);
    schedule_send_end(&mut fixture, 8, 1);
    schedule_send_end(&mut fixture, 12, 3);
    assert_send_frames(fixture.render(), |frame| match frame {
        0..5 => 0.4,
        5 => 0.2,
        6..12 => 0.7,
        _ => 0.2,
    });
    assert_eq!(
        fixture
            .observation
            .read_latest_on_control()
            .routing_failures(),
        1
    );
    assert_eq!(fixture.renderer.parameters().patches()[0].sends()[0], 0.2);
}

fn native_braids_fixture() -> Fixture {
    let (graph, _) = native_braids_graph_and_state();
    Fixture::with_graph(graph, 16)
}

fn native_braids_graph_and_state() -> (PreparedGraph, AppState) {
    native_braids_graph_and_state_with_timbre(None)
}

fn native_braids_graph_and_state_with_timbre(timbre: Option<f32>) -> (PreparedGraph, AppState) {
    let provider = BraidsCapability::new().unwrap();
    let descriptor = provider.descriptor();
    // This deterministic native model allows independent voice sums to be
    // compared sample-for-sample, including the first rendered block.
    let mut config = provider
        .default_config()
        .unwrap()
        .with_scalar_value(
            &descriptor,
            &ParameterId::new(BRAIDS_MODEL_PARAMETER_ID).unwrap(),
            ParameterValue::Choice("braids.model.sine-triangle".to_owned()),
        )
        .unwrap();
    if let Some(timbre) = timbre {
        config = config
            .with_scalar_value(
                &descriptor,
                &ParameterId::new(BRAIDS_TIMBRE_PARAMETER_ID).unwrap(),
                ParameterValue::Continuous(f64::from(timbre)),
            )
            .unwrap();
    }
    let patches = [Patch::new(
        PatchId::new(1).unwrap(),
        "Owned native voices".to_owned(),
        config,
        MidiChannel::new(0).unwrap(),
        PatchOutput::default(),
    )
    .with_envelope(VoiceEnvelope::new(0.0, 0.0, 1.0, 10.0).unwrap())];
    let registry = CapabilityRegistry::new(vec![descriptor]).unwrap();
    let parameters = ParameterSnapshot::project_patches(
        0,
        GraphRevision::INITIAL,
        GlobalParameters::new(0.0).unwrap(),
        MixerState::default(),
        &patches,
        &registry,
    )
    .unwrap();
    let preparers: Vec<Box<dyn InstrumentPreparer>> =
        vec![Box::new(BraidsPreparer::new().unwrap())];
    let graph = PreparedGraphBuilder::new(&registry, &preparers)
        .build(GraphRevision::INITIAL, &patches, parameters, 48_000.0, 16)
        .unwrap();
    let mut state = AppState::new(registry, GlobalParameters::new(0.0).unwrap());
    state
        .apply(AppEvent::InstallPatches(patches.to_vec()))
        .unwrap();
    (graph, state)
}

fn assert_native_mix<Control>(
    mixed: &mut Fixture<Control>,
    references: &mut [Fixture; 4],
    active_notes: u32,
) {
    let lifecycle_before = braids_lifecycle_counts();
    let mut expected = [0.0; 32];
    for reference in references {
        for (sum, sample) in expected.iter_mut().zip(reference.render()) {
            *sum += sample;
        }
        assert_eq!(
            reference
                .observation
                .read_latest_on_control()
                .routing_failures(),
            0
        );
    }
    let actual = mixed.render();
    assert_eq!(braids_lifecycle_counts(), lifecycle_before);
    assert!(actual.iter().all(|sample| sample.is_finite()));
    assert!(expected.iter().any(|sample| sample.abs() > 1e-7));
    let observed = mixed.observation.read_latest_on_control();
    assert_eq!(observed.active_notes(), active_notes);
    assert_eq!(observed.routing_failures(), 0);
    assert_eq!(observed.voice_limit_refusals(), 0);
    let mismatch = actual
        .iter()
        .zip(expected)
        .enumerate()
        .find(|(_, (actual, expected))| (**actual - expected).abs() >= 1e-6);
    assert!(
        mismatch.is_none(),
        "native mismatch after frame {}: {mismatch:?}",
        observed.rendered_frames()
    );
}

type MidiEvent = (MidiMessageKind, u8, u8);
type RoutedFixture = Fixture<AppLoop<LockFreeControlHandle>>;

impl RoutedFixture {
    fn native() -> Self {
        Self::native_with_budget(128)
    }

    fn native_with_budget(budget: usize) -> Self {
        let (graph, state) = native_braids_graph_and_state();
        let Fixture {
            renderer,
            control,
            structural,
            observation,
        } = Fixture::with_graph_capacity(graph, budget, 128);
        Self {
            renderer,
            structural,
            observation,
            control: AppLoop::new(state, StateProjector::new(), control).unwrap(),
        }
    }

    fn schedule_owned(
        &mut self,
        frame: u64,
        id: u64,
        scope: MidiExpressionScope,
        event: MidiEvent,
    ) {
        let event = ScheduledEvent::new(
            frame,
            self.control.scheduled_epoch(),
            self.control.active_audio_graph_revision(),
            PatchId::new(1).unwrap(),
            midi(event.0, event.1, event.2),
        )
        .with_voice(Some(MidiVoiceId::new(id).unwrap()))
        .with_expression_scope(scope);
        let result = self
            .control
            .dispatch_scheduled_midi_from(event, EventSource::AutomaticMidi)
            .unwrap();
        assert_eq!(result.boundary_full(), None);
    }

    fn monitor(&mut self, id: u64, scope: MidiExpressionScope, event: MidiEvent) {
        let result = self
            .control
            .dispatch_voice_for_source(
                PatchId::new(1).unwrap(),
                midi(event.0, event.1, event.2),
                MidiSource::Monitor,
                MidiVoiceId::new(id).unwrap(),
                scope,
            )
            .unwrap();
        assert_eq!(result.boundary_full(), None);
    }

    fn live(&mut self, event: MidiEvent) {
        let result = self
            .control
            .dispatch_from(
                AppEvent::Midi {
                    patch_id: PatchId::new(1).unwrap(),
                    message: midi(event.0, event.1, event.2),
                },
                EventSource::AutomaticMidi,
            )
            .unwrap();
        assert_eq!(result.boundary_full(), None);
    }

    fn schedule_range(&mut self, frame: u64, id: u64, scope: MidiExpressionScope, semitones: u8) {
        for (controller, value) in [(101, 0), (100, 0), (6, semitones)] {
            self.schedule_owned(
                frame,
                id,
                scope,
                (MidiMessageKind::ControlChange, controller, value),
            );
        }
    }

    fn monitor_range(&mut self, id: u64, scope: MidiExpressionScope, semitones: u8) {
        for (controller, value) in [(101, 0), (100, 0), (6, semitones)] {
            self.monitor(
                id,
                scope,
                (MidiMessageKind::ControlChange, controller, value),
            );
        }
    }
}

struct ScopedRouteWitness {
    mixed: RoutedFixture,
    references: [Fixture; 4],
}

impl ScopedRouteWitness {
    fn new() -> Self {
        Self {
            mixed: RoutedFixture::native(),
            references: std::array::from_fn(|_| native_braids_fixture()),
        }
    }

    fn compare(&mut self, active: u32) {
        assert_native_mix(&mut self.mixed, &mut self.references, active);
    }

    fn reference(&mut self, index: usize, frame: u64, event: MidiEvent) {
        self.references[index].schedule_message(frame, 1, midi(event.0, event.1, event.2));
    }

    fn reference_boundary(&mut self, frame: u64) {
        // Keep native render chunk boundaries identical. CC1 is a no-op for
        // Braids and does not replace any expression used by the references.
        for reference in &mut self.references {
            reference.schedule_message(frame, 1, midi(MidiMessageKind::ControlChange, 1, 0));
        }
    }

    fn attack_and_reuse(&mut self) {
        use MidiExpressionScope::{Master, Member};
        use MidiMessageKind::{
            ChannelPressure, ControlChange as Cc, NoteOff, NoteOn, PitchBend, PolyPressure,
        };
        self.mixed.live((NoteOn, 60, 100));
        self.references[2].immediate(1, midi(NoteOn, 60, 100));
        self.mixed.monitor(1, Member, (NoteOn, 60, 100));
        self.mixed.monitor_range(1, Member, 12);
        self.mixed.monitor_range(1, Master, 4);
        self.mixed.monitor(1, Member, (PitchBend, 0, 32));
        self.mixed.monitor(1, Master, (PitchBend, 0, 96));
        self.mixed.monitor(1, Member, (ChannelPressure, 80, 0));
        self.mixed.monitor(1, Master, (ChannelPressure, 100, 0));
        self.mixed.monitor(1, Master, (Cc, 64, 127));
        // Monitor's member -6 plus master +2 yields independent native note 56.
        for event in [
            midi(NoteOn, 56, 100),
            midi(ChannelPressure, 80, 0),
            midi(PolyPressure, 56, 100),
        ] {
            self.references[3].immediate(1, event);
        }
        self.reference_boundary(5);
        self.mixed.schedule_owned(5, 1, Member, (NoteOn, 60, 100));
        self.mixed.schedule_range(5, 1, Member, 12);
        self.mixed.schedule_range(5, 1, Master, 4);
        for (scope, event) in [
            (Member, (PitchBend, 0, 96)),
            (Master, (PitchBend, 0, 32)),
            (Member, (ChannelPressure, 96, 0)),
            (Master, (ChannelPressure, 64, 0)),
            (Master, (Cc, 64, 127)),
        ] {
            self.mixed.schedule_owned(5, 1, scope, event);
        }
        for event in [
            (NoteOn, 64, 100),
            (ChannelPressure, 96, 0),
            (PolyPressure, 64, 64),
        ] {
            self.reference(0, 5, event);
        }
        // Physical release under master pedal precedes reuse of that same
        // member channel and pitch by attack 2, all within one callback.
        self.reference_boundary(9);
        self.mixed.schedule_owned(9, 1, Member, (NoteOff, 60, 0));
        self.reference_boundary(11);
        self.mixed.schedule_owned(11, 2, Member, (NoteOn, 60, 100));
        self.mixed.schedule_range(11, 2, Master, 12);
        for (scope, event) in [
            (Member, (PitchBend, 0, 32)),
            (Master, (PitchBend, 0, 96)),
            (Member, (ChannelPressure, 32, 0)),
            (Master, (ChannelPressure, 100, 0)),
        ] {
            self.mixed.schedule_owned(11, 2, scope, event);
        }
        for event in [
            (NoteOn, 65, 100),
            (ChannelPressure, 32, 0),
            (PolyPressure, 65, 100),
        ] {
            self.reference(1, 11, event);
        }
        self.compare(4);
    }

    fn change_independent_curves(&mut self) {
        use MidiExpressionScope::{Master, Member};
        use MidiMessageKind::{ChannelPressure, ControlChange as Cc, PitchBend, PolyPressure};
        self.mixed.monitor(1, Master, (PitchBend, 0, 80));
        self.references[3].immediate(1, midi(PitchBend, 0, 32));
        self.reference_boundary(21);
        self.mixed
            .schedule_owned(21, 1, Member, (PitchBend, 0, 104));
        self.mixed.schedule_owned(21, 2, Master, (PitchBend, 0, 88));
        self.reference(0, 21, (PitchBend, 0, 112)); // 60 +7.5 -2 =65.5
        self.reference(1, 21, (PitchBend, 0, 16)); // 60 -1 +4.5 =63.5
        self.reference_boundary(25);
        self.mixed.schedule_owned(25, 1, Master, (Cc, 6, 6));
        self.reference(0, 25, (PitchBend, 0, 80)); // 60 +7.5 -3 =64.5
        self.reference_boundary(27);
        for event in [(Cc, 6, 10), (Cc, 38, 50)] {
            self.mixed.schedule_owned(27, 1, Member, event);
        }
        self.mixed
            .schedule_owned(27, 1, Master, (ChannelPressure, 80, 0));
        self.mixed
            .schedule_owned(27, 2, Member, (ChannelPressure, 96, 0));
        self.reference(0, 27, (PitchBend, 0, 50)); // 60 +6.5625 -3 =63.5625
        self.reference(0, 27, (PolyPressure, 64, 80));
        self.reference(1, 27, (ChannelPressure, 96, 0));
        self.compare(4);
    }

    fn sustain_and_cancel_each_source(&mut self) {
        use MidiExpressionScope::{Master, Member};
        use MidiMessageKind::{ControlChange as Cc, NoteOff};
        self.mixed.monitor(1, Member, (NoteOff, 60, 0));
        self.reference_boundary(37);
        self.mixed.schedule_owned(37, 1, Member, (Cc, 64, 127));
        self.mixed.schedule_owned(37, 1, Master, (Cc, 64, 0));
        self.reference_boundary(41);
        self.mixed.schedule_owned(41, 1, Member, (Cc, 64, 0));
        self.reference(0, 41, (NoteOff, 64, 0));
        self.reference_boundary(43);
        self.mixed.schedule_owned(43, 2, Member, (Cc, 64, 127));
        self.mixed.schedule_owned(43, 2, Member, (NoteOff, 60, 0));
        self.mixed.schedule_owned(43, 2, Master, (Cc, 64, 0));
        self.reference_boundary(45);
        self.mixed.schedule_owned(45, 1, Master, (Cc, 64, 127));
        self.compare(3); // Pedal-down cannot revive attack 1's release tail.
        self.mixed.control.cancel_scheduled_midi().unwrap();
        for reference in &mut self.references[..2] {
            reference
                .control
                .push_command(AudioCommand::all_notes_off())
                .unwrap();
        }
        self.compare(2); // Scheduled cancel leaves sustained Monitor + Immediate.
        self.mixed
            .control
            .cancel_midi_source(PatchId::new(1).unwrap(), MidiSource::Monitor)
            .unwrap();
        self.references[3]
            .control
            .push_command(AudioCommand::all_notes_off())
            .unwrap();
        self.compare(1); // Monitor cancel leaves Immediate.
        self.mixed.live((NoteOff, 60, 0));
        for _ in 0..32 {
            self.mixed.render();
        }
        assert!(self.mixed.render().iter().all(|sample| *sample == 0.0));
        assert_eq!(
            self.mixed
                .observation
                .read_latest_on_control()
                .active_notes(),
            0
        );
        assert_eq!(
            self.mixed
                .observation
                .read_latest_on_control()
                .routing_failures(),
            0
        );
    }

    fn cancel_monitor_preserving_existing_scheduled_attacks(&mut self) {
        self.mixed
            .control
            .cancel_midi_source(PatchId::new(1).unwrap(), MidiSource::Monitor)
            .unwrap();
        self.references[3]
            .control
            .push_command(AudioCommand::all_notes_off())
            .unwrap();
        self.compare(3);
        self.mixed.control.cancel_scheduled_midi().unwrap();
        for reference in &mut self.references[..2] {
            reference
                .control
                .push_command(AudioCommand::all_notes_off())
                .unwrap();
        }
        self.compare(1);
    }
}

#[test]
fn native_scoped_expression_crosses_reducer_queues_sample_boundaries_and_source_cleanup() {
    const CHILD: &str = "CREST_BRAIDS_SCOPED_ROUTE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        // Native lifecycle counters are process-wide; other tests may prepare
        // and destroy banks concurrently, so this complete route runs alone.
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "native_scoped_expression_crosses_reducer_queues_sample_boundaries_and_source_cleanup", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(
            child.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        assert!(String::from_utf8_lossy(&child.stdout).contains("1 passed"));
        return;
    }
    let mut witness = ScopedRouteWitness::new();
    witness.attack_and_reuse();
    witness.change_independent_curves();
    witness.sustain_and_cancel_each_source();
    // Reverse the cancellation order with existing attacks. Native oscillator
    // reset preserves upstream interpolation history, so this witness compares
    // matching voice histories rather than asserting fresh-DSP equivalence.
    let mut reverse = ScopedRouteWitness::new();
    reverse.attack_and_reuse();
    reverse.change_independent_curves();
    reverse.cancel_monitor_preserving_existing_scheduled_attacks();
}

fn dispatch_atomic_attack(
    fixture: &mut RoutedFixture,
    source: MidiSource,
    frame: u64,
    initial: OwnedVoiceInitialization,
) {
    let patch = PatchId::new(1).unwrap();
    let voice = MidiVoiceId::new(1).unwrap();
    let result = match source {
        MidiSource::Scheduled => fixture
            .control
            .dispatch_scheduled_midi_from(
                ScheduledEvent::initialized_voice(
                    frame,
                    fixture.control.scheduled_epoch(),
                    fixture.control.active_audio_graph_revision(),
                    patch,
                    voice,
                    initial,
                ),
                EventSource::AutomaticMidi,
            )
            .unwrap(),
        MidiSource::Monitor => fixture
            .control
            .dispatch_initialized_voice_for_source(patch, source, voice, initial)
            .unwrap(),
        MidiSource::Immediate => unreachable!("this witness exercises host-owned sources"),
    };
    assert_eq!(result.boundary_full(), None);
}

fn dispatch_atomic_followup(
    fixture: &mut RoutedFixture,
    source: MidiSource,
    frame: u64,
    scope: MidiExpressionScope,
    event: MidiEvent,
) {
    match source {
        MidiSource::Scheduled => fixture.schedule_owned(frame, 1, scope, event),
        MidiSource::Monitor => fixture.monitor(1, scope, event),
        MidiSource::Immediate => unreachable!("this witness exercises host-owned sources"),
    }
}

fn compare_atomic_render(
    actual: &mut RoutedFixture,
    reference: &mut Fixture,
    commands: u64,
    failures: u64,
    active: u32,
) -> [f32; 32] {
    let lifecycle = braids_lifecycle_counts();
    let expected = reference.render();
    let rendered = actual.render();
    assert_eq!(braids_lifecycle_counts(), lifecycle);
    let observed = actual.observation.read_latest_on_control();
    assert_eq!(observed.commands_consumed(), commands);
    assert_eq!(observed.routing_failures(), failures);
    assert_eq!(observed.voice_limit_refusals(), 0);
    assert_eq!(observed.active_notes(), active);
    assert_eq!(
        reference
            .observation
            .read_latest_on_control()
            .routing_failures(),
        0
    );
    assert!(rendered.iter().all(|sample| sample.is_finite()));
    for (index, (actual, expected)) in rendered.iter().zip(expected).enumerate() {
        assert!(
            (actual - expected).abs() < 1e-6,
            "sample {index} after frame {}: {actual} != {expected}",
            observed.rendered_frames()
        );
    }
    rendered
}

#[test]
fn atomic_owned_attacks_render_complete_initial_expression_with_one_command_budget() {
    const CHILD: &str = "CREST_BRAIDS_ATOMIC_ROUTE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        // Native allocation/lifecycle counters are process-wide.
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "atomic_owned_attacks_render_complete_initial_expression_with_one_command_budget",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        assert!(String::from_utf8_lossy(&child.stdout).contains("1 passed"));
        return;
    }

    use MidiExpressionScope::{Master, Member};
    use MidiMessageKind::{
        ChannelPressure, ControlChange as Cc, NoteOff, NoteOn, PitchBend, PolyPressure,
    };
    for source in [MidiSource::Scheduled, MidiSource::Monitor] {
        let mut actual = RoutedFixture::native_with_budget(1);
        let (graph, _) = native_braids_graph_and_state_with_timbre(Some(25.0 / 127.0));
        let mut reference = Fixture::with_graph(graph, 16);
        let offset = if source == MidiSource::Scheduled {
            5
        } else {
            0
        };
        let (member_pedal, master_pedal, pedal_scope) = if source == MidiSource::Scheduled {
            (0, 127, Master)
        } else {
            (127, 0, Member)
        };
        // Member +90 and master -90 are each outside the native interval at
        // note 60. One complete attack must validate their sum without ever
        // rendering an intermediate bend, pressure, timbre or pedal state.
        let initial = OwnedVoiceInitialization::new(
            midi(NoteOn, 60, 100),
            OwnedExpressionState::new(14_336, 120, 0, Some(96), Some(25), member_pedal).unwrap(),
            OwnedExpressionState::new(2_048, 120, 0, Some(64), None, master_pedal).unwrap(),
        )
        .unwrap();
        dispatch_atomic_attack(&mut actual, source, offset, initial);
        // Independent native oracle: equivalent pitch, configured timbre,
        // and ordinary channel/key pressure. It does not use initialization.
        for (kind, data1, data2) in [
            (NoteOn, 60, 100),
            (ChannelPressure, 96, 0),
            (PolyPressure, 60, 64),
        ] {
            reference.schedule_message(offset, 1, midi(kind, data1, data2));
        }
        let first = compare_atomic_render(&mut actual, &mut reference, 1, 0, 1);
        assert!(first[..offset as usize * 2]
            .iter()
            .all(|sample| *sample == 0.0));
        assert!(first.iter().any(|sample| sample.abs() > 1e-7));

        // Later controls retain strict per-event admission: removing member
        // +90 would leave pitch -30, so reject without changing this voice.
        dispatch_atomic_followup(&mut actual, source, 16 + offset, Member, (PitchBend, 0, 64));
        reference.schedule_message(16 + offset, 1, midi(Cc, 1, 0));
        compare_atomic_render(&mut actual, &mut reference, 2, 1, 1);

        dispatch_atomic_followup(
            &mut actual,
            source,
            32 + offset,
            Master,
            (ChannelPressure, 80, 0),
        );
        reference.schedule_message(32 + offset, 1, midi(PolyPressure, 60, 80));
        compare_atomic_render(&mut actual, &mut reference, 3, 1, 1);

        // Either initialized pedal holds after physical key release. Keeping
        // the oracle's key down isolates sustain from envelope tail length.
        dispatch_atomic_followup(&mut actual, source, 48 + offset, Member, (NoteOff, 60, 0));
        reference.schedule_message(48 + offset, 1, midi(Cc, 1, 0));
        compare_atomic_render(&mut actual, &mut reference, 4, 1, 1);
        for _ in 0..32 {
            compare_atomic_render(&mut actual, &mut reference, 4, 1, 1);
        }

        let release_frame = actual
            .observation
            .read_latest_on_control()
            .rendered_frames()
            + offset;
        dispatch_atomic_followup(&mut actual, source, release_frame, pedal_scope, (Cc, 64, 0));
        reference.schedule_message(release_frame, 1, midi(NoteOff, 60, 0));
        compare_atomic_render(&mut actual, &mut reference, 5, 1, 0);
        let mut tail = [0.0; 32];
        for _ in 0..32 {
            tail = compare_atomic_render(&mut actual, &mut reference, 5, 1, 0);
        }
        assert!(tail.iter().all(|sample| *sample == 0.0));
    }
}

#[test]
fn native_scheduled_voice_ids_preserve_same_pitch_expression_release_and_source_cleanup() {
    let mut mixed = native_braids_fixture();
    let mut references = std::array::from_fn(|_| native_braids_fixture());
    let first = MidiVoiceId::new(1).unwrap();
    let second = MidiVoiceId::new(2).unwrap();
    let attack = midi(MidiMessageKind::NoteOn, 60, 100);
    let release = midi(MidiMessageKind::NoteOff, 60, 0);
    let initial_expression = [
        midi(MidiMessageKind::PitchBend, 127, 127),
        midi(MidiMessageKind::ChannelPressure, 64, 0),
        midi(MidiMessageKind::ControlChange, 11, 80),
    ];

    // Reference expression is initialized before attack. Scheduled expression
    // arrives immediately after its attack at the same absolute frame, before
    // any sample renders. The next same-pitch attack must stay independent.
    for event in initial_expression {
        references[0].immediate(1, event);
    }
    for reference in &mut references {
        reference.immediate(1, attack);
    }
    mixed.immediate(1, attack);
    mixed
        .control
        .push_command(AudioCommand::OwnedMidi {
            source: MidiSource::Monitor,
            patch_id: PatchId::new(1).unwrap(),
            message: attack,
        })
        .unwrap();
    mixed.schedule_voice(0, 1, Some(first), attack);
    for event in initial_expression {
        mixed.schedule_voice(0, 1, Some(first), event);
    }
    mixed.schedule_voice(0, 1, Some(second), attack);
    let pressure = midi(MidiMessageKind::PolyPressure, 60, 32);
    mixed.schedule_voice(0, 1, Some(second), pressure);
    references[1].immediate(1, pressure);
    assert_native_mix(&mut mixed, &mut references, 4);

    // A duration-end expression event follows release at the same frame and
    // must reach only that articulation's release tail.
    mixed.schedule_voice(16, 1, Some(first), release);
    references[0].immediate(1, release);
    let tail_expression = midi(MidiMessageKind::ChannelPressure, 20, 0);
    mixed.schedule_voice(16, 1, Some(first), tail_expression);
    references[0].immediate(1, tail_expression);
    let survivor_expression = midi(MidiMessageKind::PitchBend, 0, 0);
    mixed.schedule_voice(16, 1, Some(second), survivor_expression);
    references[1].immediate(1, survivor_expression);
    for _ in 0..32 {
        assert_native_mix(&mut mixed, &mut references, 3);
    }

    // Source-level pedal holds the second articulation. Cancelling the epoch
    // must clear sustained scheduled voices, leaving both live sources intact.
    let frame = mixed.observation.read_latest_on_control().rendered_frames();
    let pedal = midi(MidiMessageKind::ControlChange, 64, 127);
    mixed.schedule_message(frame, 1, pedal);
    mixed.schedule_voice(frame, 1, Some(second), release);
    references[1].immediate(1, pedal);
    references[1].immediate(1, release);
    assert_native_mix(&mut mixed, &mut references, 3);
    mixed.control.cancel_scheduled().unwrap();
    references[1]
        .control
        .push_command(AudioCommand::all_notes_off())
        .unwrap();
    assert_native_mix(&mut mixed, &mut references, 2);

    mixed
        .control
        .push_command(AudioCommand::ReleaseSource {
            source: MidiSource::Monitor,
            patch_id: PatchId::new(1).unwrap(),
        })
        .unwrap();
    references[3]
        .control
        .push_command(AudioCommand::all_notes_off())
        .unwrap();
    assert_native_mix(&mut mixed, &mut references, 1);
    mixed.immediate(1, release);
    for _ in 0..32 {
        mixed.render();
    }
    assert!(mixed.render().iter().all(|sample| *sample == 0.0));
    assert_eq!(mixed.observation.read_latest_on_control().active_notes(), 0);
}

#[test]
fn offsets_are_exact_and_observation_covers_the_complete_block() {
    let mut fixture = Fixture::new(16);
    fixture.schedule(3, 1, MidiMessageKind::NoteOn, 60);
    fixture.schedule(11, 1, MidiMessageKind::NoteOff, 60);
    let output = fixture.render();
    for (frame, samples) in output.chunks_exact(2).enumerate() {
        assert_eq!(samples[0] > 0.0, (3..11).contains(&frame));
        assert_eq!(samples[0], samples[1]);
    }
    let observed = fixture.observation.read_latest_on_control();
    let rms = (output.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / 32.0).sqrt() as f32;
    assert!((observed.output_rms() - rms).abs() < 1e-7);
    assert!((observed.primary_patch_rms() - 0.1 / 2.0_f32.sqrt()).abs() < 1e-7);
    assert_eq!(
        observed.left_peak(),
        output.iter().copied().fold(0.0_f32, f32::max)
    );
    assert_eq!(observed.rendered_blocks(), 1);
    assert_eq!(observed.rendered_frames(), 16);
    assert_eq!(observed.commands_consumed(), 2);
    assert_eq!(observed.active_notes(), 0);
}

#[test]
fn block_end_is_exclusive_ties_are_fifo_and_late_events_start_next_block() {
    let mut fixture = Fixture::new(16);
    fixture.schedule(16, 1, MidiMessageKind::NoteOn, 60);
    fixture.schedule(16, 1, MidiMessageKind::NoteOff, 60);
    assert!(fixture.render().iter().all(|sample| *sample == 0.0));
    assert_eq!(
        fixture
            .observation
            .read_latest_on_control()
            .commands_consumed(),
        0
    );
    assert!(fixture.render().iter().all(|sample| *sample == 0.0));
    let on_time = fixture.observation.read_latest_on_control();
    assert_eq!(on_time.scheduled_late_events(), 0);
    assert_eq!(on_time.scheduled_queue_high_water(), 2);
    fixture.schedule(16, 1, MidiMessageKind::NoteOff, 60);
    fixture.schedule(16, 1, MidiMessageKind::NoteOn, 60);
    assert!(fixture.render().iter().all(|sample| *sample > 0.0));
    let late = fixture.observation.read_latest_on_control();
    assert_eq!(late.scheduled_late_events(), 2);
    assert_eq!(late.scheduled_max_lateness_frames(), 16);
    assert_eq!(late.scheduled_queue_high_water(), 2);
}

#[test]
fn future_schedule_does_not_block_live_midi_and_cancel_preserves_unrelated_notes() {
    let mut fixture = Fixture::new(16);
    fixture.immediate(1, midi(MidiMessageKind::NoteOn, 67, 100));
    fixture.immediate(2, midi(MidiMessageKind::NoteOn, 72, 100));
    fixture.schedule(0, 1, MidiMessageKind::NoteOn, 60);
    fixture.schedule(100_000, 1, MidiMessageKind::NoteOn, 62);
    let before = fixture.render();
    fixture.control.cancel_scheduled().unwrap();
    let after = fixture.render();
    assert!(after.iter().all(|sample| *sample > 0.0));
    assert!(after[0] < before[0]);
    assert_eq!(
        fixture.observation.read_latest_on_control().active_notes(),
        2
    );
}

#[test]
fn canceled_queue_flood_consumes_bounded_budget_before_current_epoch() {
    let mut fixture = Fixture::new(2);
    for note in [60, 62, 64] {
        fixture.schedule(100_000, 1, MidiMessageKind::NoteOn, note);
    }
    fixture.control.cancel_scheduled().unwrap();
    fixture.schedule(0, 1, MidiMessageKind::NoteOn, 67);
    assert!(fixture.render().iter().all(|sample| *sample == 0.0));
    assert_eq!(
        fixture
            .observation
            .read_latest_on_control()
            .commands_consumed(),
        2
    );
    assert!(fixture.render().iter().all(|sample| *sample > 0.0));
    assert_eq!(
        fixture
            .observation
            .read_latest_on_control()
            .commands_consumed(),
        4
    );
}

#[test]
fn input_recovery_preserves_native_owned_accompaniment_and_monitor_expression() {
    let (graph, _) = native_braids_graph_and_state();
    let mut fixture = Fixture::with_graph_capacity(graph, 128, 128);
    let voice = MidiVoiceId::new(1).unwrap();
    fixture.immediate(1, midi(MidiMessageKind::NoteOn, 60, 100));
    fixture.schedule_voice(0, 1, Some(voice), midi(MidiMessageKind::NoteOn, 60, 100));
    fixture
        .control
        .push_command(AudioCommand::VoiceMidi {
            source: MidiSource::Monitor,
            patch_id: PatchId::new(1).unwrap(),
            voice,
            scope: MidiExpressionScope::Member,
            message: midi(MidiMessageKind::NoteOn, 60, 100),
        })
        .unwrap();
    fixture.render();
    assert_eq!(
        fixture.observation.read_latest_on_control().active_notes(),
        3
    );
    fixture.control.push_input_recovery_command().unwrap();
    fixture.schedule_voice(16, 1, Some(voice), midi(MidiMessageKind::PitchBend, 1, 64));
    fixture
        .control
        .push_command(AudioCommand::VoiceMidi {
            source: MidiSource::Monitor,
            patch_id: PatchId::new(1).unwrap(),
            voice,
            scope: MidiExpressionScope::Member,
            message: midi(MidiMessageKind::ChannelPressure, 77, 0),
        })
        .unwrap();
    fixture.render();
    let observed = fixture.observation.read_latest_on_control();
    assert_eq!(observed.active_notes(), 2);
    assert_eq!(observed.routing_failures(), 0);
    fixture.control.push_recovery_command().unwrap();
    fixture.render();
    assert_eq!(
        fixture.observation.read_latest_on_control().active_notes(),
        0
    );
}

#[test]
fn immediate_and_scheduled_lanes_share_one_admitted_command_budget() {
    let mut fixture = Fixture::new(2);
    fixture.immediate(1, midi(MidiMessageKind::ControlChange, 11, 100));
    fixture.immediate(1, midi(MidiMessageKind::ControlChange, 11, 100));
    fixture.schedule(0, 1, MidiMessageKind::NoteOn, 60);
    assert!(fixture.render().iter().all(|sample| *sample == 0.0));
    assert_eq!(
        fixture
            .observation
            .read_latest_on_control()
            .commands_consumed(),
        2
    );
    assert!(fixture.render().iter().all(|sample| *sample > 0.0));
    assert_eq!(
        fixture
            .observation
            .read_latest_on_control()
            .commands_consumed(),
        3
    );
    let late = fixture.observation.read_latest_on_control();
    assert_eq!(late.scheduled_late_events(), 1);
    assert_eq!(late.scheduled_max_lateness_frames(), 16);
    assert_eq!(late.scheduled_queue_high_water(), 1);
}

#[test]
fn graph_replacement_rejects_old_revision_events_and_retires_off_callback() {
    let mut fixture = Fixture::new(16);
    fixture.schedule(16, 1, MidiMessageKind::NoteOn, 60);
    fixture.render();
    fixture
        .structural
        .publish_prepared_on_control(graph(GraphRevision::new(2).unwrap()))
        .unwrap();
    assert!(fixture.render().iter().all(|sample| *sample == 0.0));
    assert_eq!(
        fixture.renderer.active_revision(),
        GraphRevision::new(2).unwrap()
    );
    assert_eq!(
        fixture
            .observation
            .read_latest_on_control()
            .routing_failures(),
        1
    );
    assert_eq!(
        fixture.structural.collect_retired_on_control().unwrap(),
        GraphRevision::INITIAL
    );
}

#[test]
fn scheduled_release_and_cancel_preserve_live_same_pitch_and_sustain() {
    let mut fixture = Fixture::new(16);
    fixture.immediate(1, midi(MidiMessageKind::NoteOn, 60, 100));
    fixture.schedule(0, 1, MidiMessageKind::NoteOn, 60);
    let both = fixture.render();
    fixture.schedule(16, 1, MidiMessageKind::NoteOff, 60);
    let live = fixture.render();
    assert!(live.iter().all(|sample| *sample > 0.0));
    assert!(live[0] < both[0]);

    // Both sources sustain the same pitch after their respective note-offs.
    fixture.immediate(1, midi(MidiMessageKind::ControlChange, 64, 127));
    fixture.immediate(1, midi(MidiMessageKind::NoteOff, 60, 0));
    fixture.schedule_message(32, 1, midi(MidiMessageKind::ControlChange, 64, 127));
    fixture.schedule(32, 1, MidiMessageKind::NoteOn, 60);
    fixture.schedule(32, 1, MidiMessageKind::NoteOff, 60);
    assert_eq!(fixture.render(), both);
    fixture.control.cancel_scheduled().unwrap();
    assert_eq!(fixture.render(), live);

    // Scheduled cancellation neither releases the live voice nor its pedal.
    fixture.immediate(1, midi(MidiMessageKind::ControlChange, 64, 0));
    assert!(fixture.render().iter().all(|sample| *sample == 0.0));
}
