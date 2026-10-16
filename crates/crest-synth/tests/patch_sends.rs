//! Patch sends traverse the reducer, scalar transport, prepared FX, and renderer.
//! DSP probes at the production capability ports make tap position and shared
//! chain execution numerically distinguishable without depending on effect tails.
use core::alloc::{GlobalAlloc, Layout};
use core::cell::Cell;
use crest_synth::adapter::atomic_audio_observation::{
    AtomicAudioObservation, AtomicAudioObservationReader, AtomicAudioObservationWriter,
};
use crest_synth::adapter::braids_capability::{BraidsCapability, BRAIDS_CAPABILITY_ID};
use crest_synth::adapter::lock_free_audio_boundary::{
    LockFreeAudioBoundary, LockFreeAudioHandle, LockFreeControlHandle,
};
use crest_synth::adapter::production_effects::{
    production_effect_preparers, production_effect_registry,
};
use crest_synth::adapter::production_instruments::production_capability_registry;
use crest_synth::control::{
    AppEvent, AppLoop, AppState, Direction, InteractionMode, PatchControlId, SemanticAction,
    SemanticControlId, SendAction, SendControlId, StateProjector, SurfaceId, TopLevelContext,
};
use crest_synth::kernel::midi_message::MidiMessage;
use crest_synth::kernel::{MidiChannel, PatchId};
use crest_synth::mixer::bus_id::BusId;
use crest_synth::mixer::bus_return::BusReturnBank;
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mixer_state::MixerState;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::mixer_track_parameters::MixerTrackParameters;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::audio_observation::{AudioObservation, ControlAudioObservation};
use crest_synth::real_time::{
    AudioBoundary, AudioObservationSnapshot, AudioRenderer, GraphRevision,
    NoStructuralGraphChanges, PreparedGraphBuilder, RtPatchParameters, RtPostEffectParameters,
};
use crest_synth::synth::effect_slot_id::EffectSlotIndex;
use crest_synth::synth::{
    CapabilityId, EffectCapabilityId, EffectPreparationError, EffectPreparer, EffectSlotId,
    InstrumentPreparationError, InstrumentPreparer, Patch, PostEffectConfig, PreparedEffectError,
    PreparedInstrument, PreparedInstrumentError, PreparedPostEffect,
};
use std::alloc::System;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

const FRAMES: usize = 32;
const HALF_GAIN_DB: f32 = -6.020_6;

thread_local! {
    static COUNT_MEMORY: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}
struct CallbackAllocator;
#[global_allocator]
static ALLOCATOR: CallbackAllocator = CallbackAllocator;

fn count_memory(allocation: bool, deallocation: bool) {
    let _ = COUNT_MEMORY.try_with(|enabled| {
        if enabled.get() {
            if allocation {
                let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
            }
            if deallocation {
                let _ = DEALLOCATIONS.try_with(|count| count.set(count.get() + 1));
            }
        }
    });
}
unsafe impl GlobalAlloc for CallbackAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count_memory(true, false);
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count_memory(true, false);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        count_memory(false, true);
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count_memory(true, true);
        unsafe { System.realloc(pointer, layout, size) }
    }
}

#[derive(Default)]
struct Probe {
    patch_fx: AtomicUsize,
    return_square: AtomicUsize,
    return_double: AtomicUsize,
    drops: AtomicUsize,
}
impl Probe {
    fn calls(&self) -> [usize; 3] {
        [
            self.patch_fx.load(Ordering::Relaxed),
            self.return_square.load(Ordering::Relaxed),
            self.return_double.load(Ordering::Relaxed),
        ]
    }
}

struct ConstantPreparer {
    capability: CapabilityId,
    probe: Arc<Probe>,
}
impl InstrumentPreparer for ConstantPreparer {
    fn capability_id(&self) -> &CapabilityId {
        &self.capability
    }
    fn prepare(
        &self,
        patch: &Patch,
        _rate: f32,
        _frames: usize,
    ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
        Ok(Box::new(ConstantInstrument {
            id: patch.id(),
            amplitude: patch.id().value() as f32 * 0.1,
            probe: Arc::clone(&self.probe),
        }))
    }
}
struct ConstantInstrument {
    id: PatchId,
    amplitude: f32,
    probe: Arc<Probe>,
}
impl Drop for ConstantInstrument {
    fn drop(&mut self) {
        self.probe.drops.fetch_add(1, Ordering::Relaxed);
    }
}
impl PreparedInstrument for ConstantInstrument {
    fn patch_id(&self) -> PatchId {
        self.id
    }
    fn dispatch(
        &mut self,
        _message: MidiMessage,
        _parameters: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        Ok(())
    }
    fn render(
        &mut self,
        output: &mut [f32],
        _frames: usize,
        _parameters: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        output.fill(self.amplitude);
        Ok(())
    }
    fn all_notes_off(&mut self) {}
}

#[derive(Clone, Copy)]
enum Operation {
    PatchDouble,
    ReturnSquare,
    ReturnDouble,
}
struct ProbePreparer {
    capability: EffectCapabilityId,
    operation: Operation,
    probe: Arc<Probe>,
}
impl EffectPreparer for ProbePreparer {
    fn capability_id(&self) -> &EffectCapabilityId {
        &self.capability
    }
    fn prepare(
        &self,
        patch_id: PatchId,
        config: &PostEffectConfig,
        _rate: f32,
        _frames: usize,
    ) -> Result<Box<dyn PreparedPostEffect>, EffectPreparationError> {
        Ok(Box::new(ProbeEffect {
            patch_id,
            slot_id: config.slot_id(),
            operation: self.operation,
            probe: Arc::clone(&self.probe),
        }))
    }
}
struct ProbeEffect {
    patch_id: PatchId,
    slot_id: EffectSlotId,
    operation: Operation,
    probe: Arc<Probe>,
}
impl Drop for ProbeEffect {
    fn drop(&mut self) {
        self.probe.drops.fetch_add(1, Ordering::Relaxed);
    }
}
impl PreparedPostEffect for ProbeEffect {
    fn patch_id(&self) -> PatchId {
        self.patch_id
    }
    fn slot_id(&self) -> EffectSlotId {
        self.slot_id
    }
    fn process(
        &mut self,
        samples: &mut [f32],
        _frames: usize,
        _parameters: &RtPostEffectParameters,
    ) -> Result<(), PreparedEffectError> {
        let counter = match self.operation {
            Operation::PatchDouble => &self.probe.patch_fx,
            Operation::ReturnSquare => &self.probe.return_square,
            Operation::ReturnDouble => &self.probe.return_double,
        };
        counter.fetch_add(1, Ordering::Relaxed);
        for sample in samples {
            *sample = match self.operation {
                Operation::ReturnSquare => *sample * *sample,
                _ => *sample * 2.0,
            };
        }
        Ok(())
    }
}

struct Fixture {
    app: AppLoop<LockFreeControlHandle>,
    renderer:
        AudioRenderer<LockFreeAudioHandle, NoStructuralGraphChanges, AtomicAudioObservationWriter>,
    observation: AtomicAudioObservationReader,
    probe: Arc<Probe>,
}
impl Fixture {
    fn new(
        patch_sends: [f32; 2],
        track_send: f32,
        mute: bool,
        solo: bool,
        other_solo: bool,
    ) -> Self {
        let bus = BusId::new(0).unwrap();
        let track = MixerTrackId::new(3).unwrap();
        let effects = production_effect_registry().unwrap();
        let registry = production_capability_registry().unwrap();
        let mut returns = BusReturnBank::default();
        for capability in ["effect.reverb", "effect.delay"] {
            let slot_id = returns.bus_return(bus).next_slot_id().unwrap();
            returns
                .set_effect_slot(
                    &effects,
                    bus,
                    slot_id,
                    Some(&EffectCapabilityId::new(capability).unwrap()),
                )
                .unwrap();
        }
        returns.set_return_level(bus, 0.5).unwrap();
        let slot = EffectSlotIndex::ALL[0];
        let patches = patch_sends
            .into_iter()
            .enumerate()
            .map(|(index, send)| {
                let mut sends = vec![0.0; returns.len()];
                sends[bus.index()] = send;
                Patch::new(
                    PatchId::new(index as u32 + 1).unwrap(),
                    format!("Patch {}", index + 1),
                    BraidsCapability::new().unwrap().default_config().unwrap(),
                    MidiChannel::new(0).unwrap(),
                    PatchOutput::new(track, if index == 0 { HALF_GAIN_DB } else { 0.0 }).unwrap(),
                )
                .with_effect_slot(
                    slot,
                    effects
                        .descriptor(&EffectCapabilityId::new("effect.chorus").unwrap())
                        .unwrap()
                        .default_config(slot.instance_identity())
                        .unwrap(),
                )
                .with_sends(sends)
                .unwrap()
            })
            .collect();
        let mut sends = vec![0.0; returns.len()];
        sends[bus.index()] = track_send;
        let mut mixer = MixerState::default().with_track(
            track,
            MixerTrackParameters::from_values(HALF_GAIN_DB, 0.25, mute, solo, sends).unwrap(),
        );
        if other_solo {
            mixer.set_track(
                MixerTrackId::new(4).unwrap(),
                MixerTrackParameters::from_values(0.0, 0.0, false, true, vec![0.0; returns.len()])
                    .unwrap(),
            );
        }
        let mut state = AppState::new_with_effects(
            registry.clone(),
            effects.clone(),
            GlobalParameters::new(0.0).unwrap(),
        )
        .with_initial_returns(returns)
        .with_initial_mixer(mixer);
        state.apply(AppEvent::InstallPatches(patches)).unwrap();
        state
            .apply(AppEvent::SelectContext(TopLevelContext::Patch))
            .unwrap();
        let parameters = StateProjector::new().parameter_snapshot(&state).unwrap();
        for (patch, expected) in parameters.patches().iter().zip(patch_sends) {
            assert_eq!(patch.sends()[bus.index()], expected);
        }
        let (control, audio) = LockFreeAudioBoundary::new(32, parameters).into_handles();
        let app = AppLoop::new(state, StateProjector::new(), control).unwrap();
        let probe = Arc::new(Probe::default());
        let instruments: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(ConstantPreparer {
            capability: CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap(),
            probe: Arc::clone(&probe),
        })];
        let effect_preparers: Vec<Box<dyn EffectPreparer>> = production_effect_preparers()
            .unwrap()
            .into_iter()
            .map(|preparer| {
                let operation = match preparer.capability_id().as_str() {
                    "effect.chorus" => Operation::PatchDouble,
                    "effect.reverb" => Operation::ReturnSquare,
                    "effect.delay" => Operation::ReturnDouble,
                    _ => return preparer,
                };
                Box::new(ProbePreparer {
                    capability: preparer.capability_id().clone(),
                    operation,
                    probe: Arc::clone(&probe),
                }) as Box<dyn EffectPreparer>
            })
            .collect();
        let graph = PreparedGraphBuilder::new(&registry, &instruments)
            .with_effects(&effects, &effect_preparers)
            .with_returns(app.bus_returns())
            .build(
                GraphRevision::INITIAL,
                app.patches(),
                app.current_parameters().clone(),
                48_000.0,
                FRAMES,
            )
            .unwrap();
        let (writer, observation) = AtomicAudioObservation::default().into_handles();
        Self {
            app,
            renderer: AudioRenderer::with_observation(
                audio,
                NoStructuralGraphChanges::new(),
                graph,
                writer,
            ),
            observation,
            probe,
        }
    }

    fn render(&mut self) -> ([f32; FRAMES * 2], AudioObservationSnapshot) {
        let before = self.probe.calls();
        let drops = self.probe.drops.load(Ordering::Relaxed);
        let mut output = [0.0; FRAMES * 2];
        ALLOCATIONS.with(|count| count.set(0));
        DEALLOCATIONS.with(|count| count.set(0));
        COUNT_MEMORY.with(|enabled| enabled.set(true));
        self.renderer.render(&mut output);
        COUNT_MEMORY.with(|enabled| enabled.set(false));
        assert_eq!(
            (ALLOCATIONS.with(Cell::get), DEALLOCATIONS.with(Cell::get)),
            (0, 0),
            "callback memory activity"
        );
        assert_eq!(
            self.probe.drops.load(Ordering::Relaxed),
            drops,
            "callback destruction"
        );
        let after = self.probe.calls();
        assert_eq!(
            [
                after[0] - before[0],
                after[1] - before[1],
                after[2] - before[2]
            ],
            [2, 1, 1],
            "each Patch FX and shared return stage executes once per block"
        );
        assert_eq!(self.renderer.active_revision(), GraphRevision::INITIAL);
        (output, self.observation.read_latest_on_control())
    }

    fn focus(&mut self, surface: SurfaceId, control: SemanticControlId) {
        let count = self
            .app
            .current_semantic_model()
            .surface(surface)
            .unwrap()
            .controls()
            .len();
        for _ in 0..count {
            if self.app.current_semantic_model().focus_path().control_id() == &control {
                return;
            }
            self.app
                .dispatch_action(SemanticAction::Navigate(Direction::Down))
                .unwrap();
        }
        panic!("fixture control is unreachable: {control:?}");
    }

    fn increase_patch_send(&mut self, id: u32) {
        self.app
            .dispatch_action(SemanticAction::SelectContext(TopLevelContext::Patch))
            .unwrap();
        while self.app.current_semantic_model().focus_path().patch_id()
            != Some(PatchId::new(id).unwrap())
        {
            let current = self
                .app
                .current_semantic_model()
                .focus_path()
                .patch_id()
                .unwrap()
                .value();
            self.app
                .dispatch_action(SemanticAction::SelectPatch(if current < id {
                    Direction::Right
                } else {
                    Direction::Left
                }))
                .unwrap();
        }
        self.app
            .dispatch_action(SemanticAction::EnterSurface(SurfaceId::PatchUtility))
            .unwrap();
        self.focus(
            SurfaceId::PatchUtility,
            SemanticControlId::Patch(PatchControlId::Send(BusId::new(0).unwrap())),
        );
        self.app
            .dispatch_action(SemanticAction::SetInteractionMode(InteractionMode::Adjust))
            .unwrap();
        self.app
            .dispatch_action(SemanticAction::Adjust(Direction::Up))
            .unwrap();
        self.app
            .dispatch_action(SemanticAction::SetInteractionMode(
                InteractionMode::Navigate,
            ))
            .unwrap();
        assert_eq!(
            self.app
                .current_parameters()
                .patch(PatchId::new(id).unwrap())
                .unwrap()
                .sends()[0],
            0.1
        );
        assert_eq!(
            self.app
                .current_parameters()
                .mixer_track(MixerTrackId::new(3).unwrap())
                .send(BusId::new(0).unwrap()),
            0.0
        );
    }

    fn lower_return_level(&mut self) {
        self.app
            .dispatch_action(SemanticAction::Send(SendAction::Open))
            .unwrap();
        self.focus(
            SurfaceId::Sends,
            SemanticControlId::Send(SendControlId::Level {
                bus: BusId::new(0).unwrap(),
            }),
        );
        self.app
            .dispatch_action(SemanticAction::SetInteractionMode(InteractionMode::Adjust))
            .unwrap();
        self.app
            .dispatch_action(SemanticAction::Adjust(Direction::Down))
            .unwrap();
        self.app
            .dispatch_action(SemanticAction::SetInteractionMode(
                InteractionMode::Navigate,
            ))
            .unwrap();
    }
}

fn near(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1.0e-6,
        "actual {actual}, expected {expected}"
    );
}
fn rms(stereo: [f32; 2]) -> f32 {
    ((stereo[0] * stereo[0] + stereo[1] * stereo[1]) * 0.5).sqrt()
}
fn assert_render(
    output: &[f32],
    observed: AudioObservationSnapshot,
    dry: [f32; 2],
    input: [f32; 2],
    return_level: f32,
) {
    let wet = input.map(|sample| sample * sample * 2.0 * return_level);
    for frame in output.chunks_exact(2) {
        near(frame[0], dry[0] + wet[0]);
        near(frame[1], dry[1] + wet[1]);
    }
    near(observed.reverb_input_rms(), rms(input));
    near(observed.wet_output_rms(), rms(wet));
}

#[test]
fn independent_patch_sends_tap_post_fx_trim_track_gain_pan_and_sum_before_shared_chain() {
    let mut fixture = Fixture::new([0.0, 0.0], 0.0, false, false, false);
    let dry = [0.1875, 0.25];
    let (baseline, observed) = fixture.render();
    assert_render(&baseline, observed, dry, [0.0, 0.0], 0.5);
    assert_eq!(observed.wet_output_rms(), 0.0);

    fixture.increase_patch_send(1);
    assert_eq!(fixture.app.patches()[1].send(BusId::new(0).unwrap()), 0.0);
    let (first_only, observed) = fixture.render();
    // Patch 1: 0.1 instrument * 2 Patch FX * 0.5 trim * 0.5 track gain
    // * [0.75, 1] pan * 0.1 Patch send. Patch 2 stays entirely dry.
    assert_render(&first_only, observed, dry, [0.00375, 0.005], 0.5);

    fixture.increase_patch_send(2);
    assert_eq!(fixture.app.patches()[0].send(BusId::new(0).unwrap()), 0.1);
    let (both, observed) = fixture.render();
    // Squaring AFTER summing distinguishes shared processing from processing
    // each Patch's send separately: (a+b)^2 differs from a^2+b^2.
    let summed_input = [0.01875, 0.025];
    assert_render(&both, observed, dry, summed_input, 0.5);

    fixture.lower_return_level();
    let level = fixture
        .app
        .bus_returns()
        .bus_return(BusId::new(0).unwrap())
        .return_level();
    near(level, 0.4);
    let (quieter, observed) = fixture.render();
    assert_render(&quieter, observed, dry, summed_input, level);
    for ((baseline, full), quiet) in baseline.iter().zip(both).zip(quieter) {
        near(quiet - *baseline, (full - *baseline) * 0.8);
    }
}

#[test]
fn patch_and_legacy_track_sends_remain_distinct_and_follow_track_mute_solo_gates() {
    let mut legacy = Fixture::new([0.0, 0.0], 0.1, false, false, false);
    let (output, observed) = legacy.render();
    assert_render(&output, observed, [0.1875, 0.25], [0.01875, 0.025], 0.5);
    let mut combined = Fixture::new([0.1, 0.0], 0.1, false, false, false);
    let (output, observed) = combined.render();
    assert_render(&output, observed, [0.1875, 0.25], [0.0225, 0.03], 0.5);

    for (mute, solo, other_solo, audible) in [
        (true, false, false, false),
        (false, false, true, false),
        (false, true, true, true),
        (true, true, true, false),
    ] {
        let mut fixture = Fixture::new([0.1, 0.1], 0.0, mute, solo, other_solo);
        let (output, observed) = fixture.render();
        if audible {
            assert_render(&output, observed, [0.1875, 0.25], [0.01875, 0.025], 0.5);
        } else {
            assert_eq!(output, [0.0; FRAMES * 2]);
            assert_eq!(observed.reverb_input_rms(), 0.0);
            assert_eq!(observed.wet_output_rms(), 0.0);
        }
    }
}
