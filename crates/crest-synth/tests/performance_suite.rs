//! Repeatable production workloads. Run explicitly, serially, in release mode.
//! Timing surrounds the callback; instrumentation never enters production DSP.
use crest_synth::adapter::atomic_audio_observation::AtomicAudioObservation;
use crest_synth::adapter::braids_capability::{
    BRAIDS_CAPABILITY_ID, BRAIDS_MODELS, BRAIDS_MODEL_PARAMETER_ID,
};
use crest_synth::adapter::lock_free_audio_boundary::{
    LockFreeAudioBoundary, LockFreeAudioHandle, LockFreeControlHandle,
};
use crest_synth::adapter::lock_free_structural_graph_boundary::{
    LockFreeStructuralAudioHandle, LockFreeStructuralControlHandle, LockFreeStructuralGraphBoundary,
};
use crest_synth::adapter::production_effects::{
    production_effect_preparers, production_effect_registry,
};
use crest_synth::adapter::production_instruments::{
    production_capability_registry, production_instrument_preparers,
    production_instrument_providers,
};
use crest_synth::adapter::sample_capability::{
    SAMPLE_CAPABILITY_ID, SAMPLE_LOOP_FORWARD_CHOICE_ID, SAMPLE_LOOP_MODE_PARAMETER_ID,
};
use crest_synth::control::{
    AppEvent, AppLoop, AppState, Direction, EventSource, StateProjector, SurfaceId, TopLevelContext,
};
use crest_synth::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crest_synth::kernel::{MidiChannel, PatchId};
use crest_synth::mixer::{
    bus_id::BusId, bus_return::BusReturnBank, global_parameters::GlobalParameters,
    mixer_state::MixerState, mixer_track_id::MixerTrackId, patch_output::PatchOutput,
};
use crest_synth::real_time::{
    AudioBoundary, AudioObservation, AudioRenderer, ControlAudioObservation,
    ControlStructuralGraphBoundary, GraphHandoffStatus, GraphRevision, PreparedGraph,
    PreparedGraphBuilder, StructuralGraphBoundary,
};
use crest_synth::synth::{
    effect_slot_id::EffectSlotIndex, CapabilityId, CapabilityRegistry,
    DescriptorDefaultConfigFactory, EffectCapabilityRegistry, EffectPreparer, InstrumentPreparer,
    ParameterId, ParameterValue, Patch, VoiceEnvelope,
};
use serde::Serialize;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::{Duration, Instant};

thread_local! {
    static MEASURING: Cell<bool> = const { Cell::new(false) };
    static MEMORY: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
}
struct CountingAllocator;
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
fn count(allocations: u64, frees: u64) {
    let _ = MEASURING.try_with(|enabled| {
        if enabled.get() {
            MEMORY.with(|count| {
                let (a, f) = count.get();
                count.set((a + allocations, f + frees));
            });
        }
    });
}
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(1, 0);
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count(1, 0);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        count(0, 1);
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count(1, 1);
        unsafe { System.realloc(pointer, layout, size) }
    }
}

#[derive(Serialize)]
struct Measurement {
    name: String,
    samples: usize,
    durations_ns: Vec<u64>,
    p50_us: f64,
    p95_us: f64,
    p99_us: f64,
    max_us: f64,
    deadline_us: f64,
    deadline_misses: usize,
    p99_budget_us: f64,
    allocations: Option<u64>,
    deallocations: Option<u64>,
    passed: bool,
}
impl Measurement {
    fn tracker_callback_target_passed(&self) -> bool {
        self.p99_us < self.deadline_us * 0.7
            && self.deadline_misses == 0
            && self.allocations == Some(0)
            && self.deallocations == Some(0)
    }

    fn new(
        name: impl Into<String>,
        mut times: Vec<Duration>,
        deadline: Duration,
        budget: Duration,
        memory: Option<(u64, u64)>,
    ) -> Self {
        assert!(!times.is_empty());
        let durations_ns = times.iter().map(|time| time.as_nanos() as u64).collect();
        times.sort_unstable();
        let us = |duration: Duration| duration.as_secs_f64() * 1e6;
        let percentile = |percent: usize| us(times[(times.len() * percent).div_ceil(100) - 1]);
        let misses = times.iter().filter(|time| **time > deadline).count();
        Self {
            name: name.into(),
            samples: times.len(),
            durations_ns,
            p50_us: percentile(50),
            p95_us: percentile(95),
            p99_us: percentile(99),
            max_us: us(*times.last().unwrap()),
            deadline_us: us(deadline),
            deadline_misses: misses,
            p99_budget_us: us(budget),
            allocations: memory.map(|counts| counts.0),
            deallocations: memory.map(|counts| counts.1),
            // A scheduler outlier is reported, but sustained misses fail. Leave
            // 20% callback headroom for the host and transport conversion.
            passed: percentile(99) <= us(budget)
                && misses * 100 <= times.len()
                && memory.is_none_or(|counts| counts == (0, 0)),
        }
    }
}

struct Fixtures {
    registry: CapabilityRegistry,
    effects: EffectCapabilityRegistry,
    instruments: Vec<Box<dyn InstrumentPreparer>>,
    effect_preparers: Vec<Box<dyn EffectPreparer>>,
    factory: DescriptorDefaultConfigFactory,
}
impl Fixtures {
    fn new() -> Self {
        let registry = production_capability_registry().unwrap();
        Self {
            factory: DescriptorDefaultConfigFactory::new(
                registry.clone(),
                production_instrument_providers().unwrap(),
            ),
            registry,
            effects: production_effect_registry().unwrap(),
            instruments: production_instrument_preparers().unwrap(),
            effect_preparers: production_effect_preparers().unwrap(),
        }
    }
    fn state(
        &self,
        engine_ids: &[&str],
        patches: usize,
        model: usize,
        slots: usize,
        returns: usize,
    ) -> AppState {
        let mut bank = BusReturnBank::default();
        let mut mixer = MixerState::default();
        for bus in &BusId::ALL[..returns] {
            let effect =
                &self.effects.descriptors()[bus.index() % self.effects.descriptors().len()];
            bank.set_return_occupancy(&self.effects, *bus, Some(effect.id()))
                .unwrap();
        }
        for track in MixerTrackId::ALL {
            let mut values = mixer.track(track).clone();
            for bus in &BusId::ALL[..returns] {
                values = values.with_send(*bus, 0.15).unwrap();
            }
            mixer.set_track(track, values);
        }
        let patches = (0..patches)
            .map(|index| {
                let id = CapabilityId::new(engine_ids[index % engine_ids.len()]).unwrap();
                let mut config = self.factory.create(&id).unwrap();
                let descriptor = self.registry.descriptor(&id).unwrap();
                if id.as_str() == BRAIDS_CAPABILITY_ID {
                    config = config
                        .with_scalar_value(
                            descriptor,
                            &ParameterId::new(BRAIDS_MODEL_PARAMETER_ID).unwrap(),
                            ParameterValue::Choice(BRAIDS_MODELS[model].id.into()),
                        )
                        .unwrap();
                }
                if id.as_str() == SAMPLE_CAPABILITY_ID {
                    config = config
                        .with_scalar_value(
                            descriptor,
                            &ParameterId::new(SAMPLE_LOOP_MODE_PARAMETER_ID).unwrap(),
                            ParameterValue::Choice(SAMPLE_LOOP_FORWARD_CHOICE_ID.into()),
                        )
                        .unwrap();
                }
                let mut patch = Patch::new(
                    PatchId::new(index as u32 + 1).unwrap(),
                    format!("Load {}", index + 1),
                    config,
                    MidiChannel::new(0).unwrap(),
                    PatchOutput::to_track(MixerTrackId::ALL[index]),
                );
                patch = patch.with_envelope(VoiceEnvelope::new(2.0, 20.0, 0.8, 40.0).unwrap());
                for slot in &EffectSlotIndex::ALL[..slots] {
                    let descriptor = &self.effects.descriptors()
                        [slot.index() % self.effects.descriptors().len()];
                    patch = patch.with_effect_slot(
                        *slot,
                        descriptor.default_config(slot.instance_identity()).unwrap(),
                    );
                }
                patch
            })
            .collect();
        let mut state = AppState::new_with_effects(
            self.registry.clone(),
            self.effects.clone(),
            GlobalParameters::new(-24.0).unwrap(),
        )
        .with_initial_returns(bank)
        .with_initial_mixer(mixer);
        state.apply(AppEvent::InstallPatches(patches)).unwrap();
        state
            .apply(AppEvent::SelectContext(TopLevelContext::Patch))
            .unwrap();
        state
    }
    fn graph(&self, state: &AppState, frames: usize, revision: GraphRevision) -> PreparedGraph {
        let parameters =
            crest_synth::real_time::ParameterSnapshot::project_patches_with_effects_and_returns(
                state.generation(),
                revision,
                *state.global(),
                state.mixer().clone(),
                state.patches(),
                state.capabilities(),
                state.effects(),
                state.bus_returns(),
            )
            .unwrap();
        PreparedGraphBuilder::new(&self.registry, &self.instruments)
            .with_effects(&self.effects, &self.effect_preparers)
            .with_returns(state.bus_returns())
            .build(revision, state.patches(), parameters, 48_000.0, frames)
            .unwrap()
    }
}

type Renderer = AudioRenderer<
    LockFreeAudioHandle,
    LockFreeStructuralAudioHandle,
    crest_synth::adapter::atomic_audio_observation::AtomicAudioObservationWriter,
>;
struct Rig {
    app: AppLoop<LockFreeControlHandle>,
    renderer: Renderer,
    structural: LockFreeStructuralControlHandle,
    observation: crest_synth::adapter::atomic_audio_observation::AtomicAudioObservationReader,
}
impl Rig {
    fn new(state: AppState, graph: PreparedGraph) -> Self {
        let parameters = graph.initial_parameters().clone();
        let (control, audio) = LockFreeAudioBoundary::new(4096, parameters).into_handles();
        let (structural, structural_audio) = LockFreeStructuralGraphBoundary::new(
            1,
            1,
            GraphHandoffStatus::with_active(graph.revision()),
        )
        .unwrap()
        .into_handles();
        let (writer, observation) = AtomicAudioObservation::default().into_handles();
        Self {
            app: AppLoop::new(state, StateProjector::new(), control).unwrap(),
            renderer: AudioRenderer::with_observation(audio, structural_audio, graph, writer),
            structural,
            observation,
        }
    }
    fn notes(&mut self, voices: usize, on: bool) {
        for index in 0..voices {
            let message = message(
                if on {
                    MidiMessageKind::NoteOn
                } else {
                    MidiMessageKind::NoteOff
                },
                36 + index as u8,
                if on { 96 } else { 0 },
            );
            assert!(self
                .app
                .dispatch_midi_from(message, EventSource::AutomaticMidi)
                .unwrap()
                .boundary_full()
                .is_none());
        }
    }
}
fn message(kind: MidiMessageKind, data1: u8, data2: u8) -> MidiMessage {
    MidiMessage::try_new(MidiChannel::new(0).unwrap(), kind, data1, data2).unwrap()
}
fn measure_render(renderer: &mut Renderer, output: &mut [f32]) -> (Duration, (u64, u64)) {
    MEMORY.with(|count| count.set((0, 0)));
    MEASURING.with(|enabled| enabled.set(true));
    let start = Instant::now();
    renderer.render(output);
    let elapsed = start.elapsed();
    MEASURING.with(|enabled| enabled.set(false));
    let memory = MEMORY.with(Cell::get);
    assert!(output.iter().all(|sample| sample.is_finite()));
    (elapsed, memory)
}
fn audio_case(
    fixtures: &Fixtures,
    state: AppState,
    frames: usize,
    voices: usize,
    name: String,
    churn: bool,
) -> Measurement {
    let (state, graph) = if churn {
        // Keep the requested held-note load AND its overlapping release tails.
        // A bank sized only for held notes correctly refuses retriggers while
        // those tails occupy its voices. Prepare the complete workload budget
        // through the production session restore path, without stealing voices
        // or reducing note counts, release duration, or timing requirements.
        let mut saved =
            serde_json::to_value(crest_synth::control::SavedSession::capture(&state)).unwrap();
        for (document, patch) in saved["patches"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .zip(state.patches())
        {
            let release_frames =
                (f64::from(patch.envelope().release_milliseconds()) * 48.0).round() as usize;
            let overlapping_batches = 1 + release_frames.div_ceil(8 * frames);
            let budget = u16::try_from(voices * overlapping_batches).unwrap();
            document["voiceLimit"] = budget.max(patch.voice_limit().value()).into();
        }
        let prepared =
            crest_synth::control::SavedSession::from_json(&saved.to_string(), &fixtures.registry)
                .unwrap()
                .prepare_restore(
                    fixtures.registry.clone(),
                    fixtures.effects.clone(),
                    &fixtures.instruments,
                    &fixtures.effect_preparers,
                    GraphRevision::INITIAL,
                    48_000.0,
                    frames,
                )
                .unwrap();
        let state = prepared.state().clone();
        let (_, graph) = prepared.into_replacement();
        (state, graph)
    } else {
        let graph = fixtures.graph(&state, frames, GraphRevision::INITIAL);
        (state, graph)
    };
    let mut rig = Rig::new(state, graph);
    rig.notes(voices, true);
    let mut output = vec![0.0; frames * 2];
    let mut total_memory = (0, 0);
    let mut peak = 0.0_f32;
    let mut times = Vec::with_capacity(512);
    // Include attack and first-use memory proof, excluding warmup timing.
    for block in 0..544 {
        if churn && block % 8 == 0 {
            rig.notes(voices, false);
            rig.notes(voices, true);
            assert!(rig
                .app
                .dispatch_midi_from(
                    message(MidiMessageKind::PitchBend, (block % 128) as u8, 64),
                    EventSource::AutomaticMidi
                )
                .unwrap()
                .boundary_full()
                .is_none());
        }
        let (elapsed, memory) = measure_render(&mut rig.renderer, &mut output);
        total_memory.0 += memory.0;
        total_memory.1 += memory.1;
        peak = output
            .iter()
            .fold(peak, |peak, sample| peak.max(sample.abs()));
        if block >= 32 {
            times.push(elapsed);
        }
    }
    let observation = rig.observation.read_latest_on_control();
    assert_eq!(observation.routing_failures(), 0, "{name}: routing failure");
    assert_eq!(
        observation.non_finite_samples(),
        0,
        "{name}: invalid DSP output"
    );
    assert_eq!(
        observation.voice_limit_refusals(),
        0,
        "{name}: workload silently lost voices"
    );
    assert_eq!(
        observation.active_notes(),
        (voices * rig.app.current_parameters().patch_count()) as u32
    );
    assert!(
        peak > 1e-6,
        "{name}: silent workloads cannot prove performance"
    );
    rig.notes(voices, false);
    for _ in 0..64 {
        let (_, memory) = measure_render(&mut rig.renderer, &mut output);
        total_memory.0 += memory.0;
        total_memory.1 += memory.1;
    }
    assert_eq!(
        rig.observation.read_latest_on_control().active_notes(),
        0,
        "{name}: stuck MIDI notes"
    );
    let deadline = Duration::from_secs_f64(frames as f64 / 48_000.0);
    Measurement::new(
        name,
        times,
        deadline,
        deadline.mul_f64(0.8),
        Some(total_memory),
    )
}

fn emit(row: Measurement, rows: &mut Vec<Measurement>) {
    let mut summary = serde_json::to_value(&row).unwrap();
    summary.as_object_mut().unwrap().remove("durations_ns");
    println!("CREST_PERFORMANCE {summary}");
    rows.push(row);
}

#[test]
fn performance_gate_rejects_slow_or_allocating_substitutes() {
    let deadline = Duration::from_millis(5);
    let good = vec![Duration::from_millis(1); 100];
    assert!(
        Measurement::new(
            "good",
            good.clone(),
            deadline,
            deadline.mul_f64(0.8),
            Some((0, 0))
        )
        .passed
    );
    assert!(
        !Measurement::new(
            "slow",
            vec![deadline * 2; 100],
            deadline,
            deadline,
            Some((0, 0))
        )
        .passed
    );
    assert!(!Measurement::new("allocation", good.clone(), deadline, deadline, Some((1, 0))).passed);
    assert!(!Measurement::new("destruction", good, deadline, deadline, Some((0, 1))).passed);

    let strict = Measurement::new(
        "strict",
        vec![Duration::from_millis(1); 100],
        deadline,
        deadline,
        Some((0, 0)),
    );
    assert!(strict.tracker_callback_target_passed());
    let mut one_miss = vec![Duration::from_millis(1); 100];
    one_miss.push(Duration::from_millis(6));
    let missed = Measurement::new(
        "one miss",
        one_miss,
        deadline,
        deadline.mul_f64(0.8),
        Some((0, 0)),
    );
    assert!(missed.passed, "existing Synth allowance remains unchanged");
    assert!(!missed.tracker_callback_target_passed());
    assert!(!Measurement::new(
        "headroom",
        vec![Duration::from_micros(3500); 100],
        deadline,
        deadline.mul_f64(0.8),
        Some((0, 0))
    )
    .tracker_callback_target_passed());
}

#[test]
#[ignore = "dedicated performance run: make test-performance"]
fn production_performance_matrix() {
    let group = std::env::var("CREST_PERFORMANCE_GROUP").unwrap_or_else(|_| "all".into());
    assert!(
        [
            "all",
            "braids",
            "scale",
            "mixed",
            "control",
            "graph",
            "concurrent"
        ]
        .contains(&group.as_str()),
        "unknown performance group: {group}"
    );
    let selected = |name: &str| group == "all" || group == name;
    let profiled = std::env::var("CREST_PERFORMANCE_PROFILE").as_deref() == Ok("1");
    let fixtures = Fixtures::new();
    let mut rows = Vec::new();
    let mut contract_failures = Vec::new();
    // Keep each production assertion authoritative, but retain evidence from
    // later workloads too. A failed contract never becomes a timing-only row
    // and remains fatal even when timings are collected under a profiler.
    let mut audio = |state, frames, voices, name: String, churn| match std::panic::catch_unwind(
        std::panic::AssertUnwindSafe(|| {
            audio_case(&fixtures, state, frames, voices, name.clone(), churn)
        }),
    ) {
        Ok(row) => emit(row, &mut rows),
        Err(error) => {
            let detail = error
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| error.downcast_ref::<&str>().copied())
                .unwrap_or("non-string panic");
            contract_failures.push(format!("{name}: {detail}"));
        }
    };
    // Every shipped Braids algorithm at its complete Patch-local voice capacity.
    if selected("braids") {
        for (index, model) in BRAIDS_MODELS.iter().enumerate() {
            for patches in [1, 16] {
                let state = fixtures.state(&[BRAIDS_CAPABILITY_ID], patches, index, 0, 0);
                audio(
                    state,
                    256,
                    16,
                    format!("braids/model/{}/p{patches}", model.label),
                    false,
                );
            }
        }
    }
    let engines = fixtures
        .registry
        .descriptors()
        .iter()
        .map(|descriptor| descriptor.id().as_str())
        .collect::<Vec<_>>();
    assert!(engines.contains(&SAMPLE_CAPABILITY_ID));
    if selected("scale") {
        for engine in &engines {
            // Configurable policies declare a representable ceiling, not the
            // budget prepared for this workload. Saturate the actual Patch.
            let capacity = fixtures.state(&[engine], 1, 0, 0, 0).patches()[0]
                .voice_limit()
                .value() as usize;
            for patches in [1, 4, 8, 16] {
                for frames in [64, 128, 256, 512] {
                    for voices in [1, capacity] {
                        audio(
                            fixtures.state(&[engine], patches, 0, 0, 0),
                            frames,
                            voices,
                            format!("scale/{engine}/p{patches}/v{voices}/f{frames}"),
                            false,
                        );
                    }
                }
            }
        }
    }
    if selected("mixed") {
        for frames in [64, 128, 256, 512] {
            for (slots, returns) in [(0, 0), (1, 0), (2, 0), (3, 0), (0, 8), (3, 8)] {
                audio(
                    fixtures.state(&engines, 16, 0, slots, returns),
                    frames,
                    16,
                    format!("mixed/p16/v16/f{frames}/slots{slots}/returns{returns}"),
                    false,
                );
            }
            audio(
                fixtures.state(&engines, 16, 0, 3, 8),
                frames,
                16,
                format!("midi-churn/p16/v16/f{frames}/slots3/returns8"),
                true,
            );
        }
    }
    if selected("control") {
        control_cases(&fixtures, &mut rows);
        control_stage_cases(&fixtures, &mut rows);
    }
    if selected("graph") {
        graph_swap_case(&fixtures, &mut rows);
    }
    let concurrent_workload =
        selected("concurrent").then(|| concurrent_worker_case(&fixtures, &mut rows));
    assert!(!rows.is_empty());
    let failures = rows
        .iter()
        .filter(|row| !row.passed)
        .map(|row| row.name.as_str())
        .collect::<Vec<_>>();
    let report = serde_json::json!({
        "schema_version": 1, "sample_rate": 48_000, "debug_assertions": cfg!(debug_assertions),
        "group": group, "profiled": profiled,
        "arch": std::env::consts::ARCH, "os": std::env::consts::OS,
        "cpu": std::process::Command::new("sysctl").args(["-n", "machdep.cpu.brand_string"]).output().ok().filter(|output| output.status.success()).map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned()),
        "rustc": std::process::Command::new("rustc").arg("--version").output().ok().filter(|output| output.status.success()).map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned()),
        "timing_scope": "offline production callback duration; excludes physical device scheduling and native webview paint",
        "rows": rows, "failures": failures, "contract_failures": contract_failures,
        "concurrent_workload": concurrent_workload,
    });
    let path = std::env::var_os("CREST_PERFORMANCE_REPORT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "target/performance/latest.json".into());
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!("CREST_PERFORMANCE_REPORT {}", path.display());
    assert!(
        contract_failures.is_empty(),
        "production audio contracts failed: {contract_failures:?}; report retains all workload failures"
    );
    assert!(concurrent_workload.as_ref().is_none_or(|evidence| evidence["contracts_passed"] == true),
        "concurrent production audio contracts failed; report retains actual admission and recovery counters");
    assert!(
        rows.iter()
            .all(|row| row.allocations.is_none_or(|count| count == 0)
                && row.deallocations.is_none_or(|count| count == 0)),
        "callback memory contract failed"
    );
    assert!(
        profiled || failures.is_empty(),
        "performance budgets failed: {failures:?}; report contains every workload"
    );
    assert!(
        concurrent_workload
            .as_ref()
            .and_then(|evidence| evidence["strict_tracker_target"]["passed"].as_bool())
            .unwrap_or(true),
        "strict Tracker callback execution target failed; see retained matrix evidence"
    );
}

fn control_cases(fixtures: &Fixtures, rows: &mut Vec<Measurement>) {
    for patches in [1, 16] {
        let state = fixtures.state(&[BRAIDS_CAPABILITY_ID], patches, 0, 3, 8);
        let graph = fixtures.graph(&state, 256, GraphRevision::INITIAL);
        let mut rig = Rig::new(state, graph);
        rig.app
            .dispatch(AppEvent::EnterSurface(SurfaceId::PatchDetail))
            .unwrap();
        rig.app
            .dispatch(AppEvent::Navigate(Direction::Down))
            .unwrap(); // Timbre
        let mut output = [0.0; 512];
        rig.notes(16, true);
        for workload in [
            "midi-fanout",
            "scalar-edit-and-serialize",
            "navigation-and-serialize",
        ] {
            let mut times = Vec::with_capacity(256);
            for index in 0..288 {
                let start = Instant::now();
                match workload {
                    "midi-fanout" => {
                        assert!(rig
                            .app
                            .dispatch_midi_from(
                                message(MidiMessageKind::PitchBend, (index % 128) as u8, 64),
                                EventSource::AutomaticMidi
                            )
                            .unwrap()
                            .boundary_full()
                            .is_none());
                    }
                    "scalar-edit-and-serialize" => {
                        rig.app
                            .dispatch(AppEvent::Adjust(if index % 2 == 0 {
                                Direction::Right
                            } else {
                                Direction::Left
                            }))
                            .unwrap();
                        std::hint::black_box(
                            serde_json::to_vec(rig.app.current_graphical_shell().semantic_model())
                                .unwrap(),
                        );
                    }
                    _ => {
                        rig.app
                            .dispatch(AppEvent::Navigate(if index % 2 == 0 {
                                Direction::Down
                            } else {
                                Direction::Up
                            }))
                            .unwrap();
                        std::hint::black_box(
                            serde_json::to_vec(rig.app.current_graphical_shell().semantic_model())
                                .unwrap(),
                        );
                    }
                }
                let elapsed = start.elapsed();
                if index >= 32 {
                    times.push(elapsed);
                }
                let (_, memory) = measure_render(&mut rig.renderer, &mut output);
                assert_eq!(memory, (0, 0));
            }
            emit(
                Measurement::new(
                    format!("control/{workload}/p{patches}"),
                    times,
                    Duration::from_millis(16),
                    Duration::from_millis(8),
                    None,
                ),
                rows,
            );
        }
    }
}

fn graph_swap_case(fixtures: &Fixtures, rows: &mut Vec<Measurement>) {
    let engines = fixtures
        .registry
        .descriptors()
        .iter()
        .map(|descriptor| descriptor.id().as_str())
        .collect::<Vec<_>>();
    let state = fixtures.state(&engines, 16, 0, 3, 8);
    let graph = fixtures.graph(&state, 256, GraphRevision::INITIAL);
    let mut rig = Rig::new(state.clone(), graph);
    let mut preparation = Vec::new();
    let mut activation = Vec::new();
    let mut output = [0.0; 512];
    let mut memory = (0, 0);
    for index in 0..32 {
        let revision = GraphRevision::new(index + 2).unwrap();
        let start = Instant::now();
        let graph = fixtures.graph(&state, 256, revision);
        preparation.push(start.elapsed());
        let parameters = graph.initial_parameters().clone();
        rig.structural.publish_prepared_on_control(graph).unwrap();
        // Graph activation uses its own complete initial scalars; stale
        // control snapshots must not overwrite this revision.
        let (elapsed, counts) = measure_render(&mut rig.renderer, &mut output);
        activation.push(elapsed);
        memory.0 += counts.0;
        memory.1 += counts.1;
        assert_eq!(rig.renderer.active_revision(), revision);
        assert!(rig.renderer.parameters().audio_values_equal(&parameters));
        assert!(rig.structural.collect_retired_on_control().is_some());
    }
    emit(
        Measurement::new(
            "graph/prepare/p16/slots3/returns8",
            preparation,
            Duration::from_secs(2),
            Duration::from_secs(1),
            None,
        ),
        rows,
    );
    let deadline = Duration::from_secs_f64(256.0 / 48_000.0);
    emit(
        Measurement::new(
            "graph/activate-retire/p16/slots3/returns8",
            activation,
            deadline,
            deadline.mul_f64(0.8),
            Some(memory),
        ),
        rows,
    );
}

fn concurrent_worker_case(fixtures: &Fixtures, rows: &mut Vec<Measurement>) -> serde_json::Value {
    use crest_synth::adapter::threaded_graph_preparation_worker::ThreadedGraphPreparationWorker;
    use crest_synth::shell::audio_output::{AudioDeviceConfig, AudioSampleFormat};
    use std::sync::atomic::{AtomicBool, Ordering};
    struct StopOnDrop<'a>(&'a AtomicBool);
    impl Drop for StopOnDrop<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let soak_seconds = std::env::var("CREST_PERFORMANCE_SOAK_SECONDS")
        .ok()
        .map(|value| {
            let seconds: u32 = value
                .parse()
                .expect("soak duration must be a positive integer");
            assert!(seconds > 0);
            assert_ne!(
                std::env::var("CREST_PERFORMANCE_PROFILE").as_deref(),
                Ok("1"),
                "soak timings must be unprofiled"
            );
            seconds
        });
    let voices = if soak_seconds.is_some() {
        let value = std::env::var("CREST_PERFORMANCE_SOAK_VOICES")
            .expect("soak requires an explicit voice workload");
        let voices: u8 = value.parse().expect("soak voices must be an integer");
        assert!(
            (1..=92).contains(&voices),
            "fixture pitches 36..127 admit 1..92 voices per Patch"
        );
        voices
    } else {
        16
    };
    let requested_duration = soak_seconds.map(|seconds| Duration::from_secs(u64::from(seconds)));
    let watchdog = requested_duration.unwrap_or(Duration::from_secs(40)) + Duration::from_secs(60);
    let max_blocks = (watchdog.as_secs() * 48_000).div_ceil(256) as usize + 64;
    let engines = fixtures
        .registry
        .descriptors()
        .iter()
        .map(|descriptor| descriptor.id().as_str())
        .collect::<Vec<_>>();
    let state = fixtures.state(&engines, 16, 0, 3, 8);
    let patches: Vec<_> = state.patches().iter().map(|patch| {
        assert!(patch.voice_limit().value() >= u16::from(voices), "declared workload exceeds prepared Patch capacity");
        serde_json::json!({ "id": patch.id(), "engine": patch.instrument_config().capability_id(), "voice_limit": patch.voice_limit().value() })
    }).collect();
    let declared_held_voices = usize::from(voices) * patches.len();
    let graph = fixtures.graph(&state, 256, GraphRevision::INITIAL);
    let parameters = graph.initial_parameters().clone();
    let (control, audio) = LockFreeAudioBoundary::new(4096, parameters).into_handles();
    let (structural, structural_audio) = LockFreeStructuralGraphBoundary::new(
        1,
        1,
        GraphHandoffStatus::with_active(graph.revision()),
    )
    .unwrap()
    .into_handles();
    let (writer, observation) = AtomicAudioObservation::default().into_handles();
    let mut app = AppLoop::new(state, StateProjector::new(), control).unwrap();
    let config = AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 256).unwrap();
    let worker = ThreadedGraphPreparationWorker::new_with_effects(
        fixtures.registry.clone(),
        production_instrument_preparers().unwrap(),
        fixtures.effects.clone(),
        production_effect_preparers().unwrap(),
        config,
    )
    .unwrap();
    app.configure_engine_selection(
        DescriptorDefaultConfigFactory::new(
            fixtures.registry.clone(),
            production_instrument_providers().unwrap(),
        ),
        worker,
        structural,
        &graph,
        config,
    )
    .unwrap();
    let mut renderer = AudioRenderer::with_observation(audio, structural_audio, graph, writer);
    for note in 36..36 + voices {
        assert!(app
            .dispatch_midi_from(
                message(MidiMessageKind::NoteOn, note, 96),
                EventSource::AutomaticMidi
            )
            .unwrap()
            .boundary_full()
            .is_none());
    }
    let stop = AtomicBool::new(false);
    let mut tick_times = Vec::with_capacity(256);
    let mut request_times = Vec::new();
    let mut advance_times = Vec::with_capacity(256);
    let mut retrigger_times = Vec::new();
    let mut midi_times = Vec::with_capacity(256);
    let mut serialization_times = Vec::with_capacity(256);
    let mut worker_times = Vec::new();
    let mut requested = None;
    let mut swaps = 0;
    let mut requests = 0;
    let started = Instant::now();
    let mut ticks = 0;
    let mut next_progress = Duration::from_secs(30);
    let mut final_audio_observation = None;
    let audio_observation = observation.clone();
    let (audio_row, audio_wall, active_histogram, active_audio_blocks) = std::thread::scope(
        |scope| {
            let stop = &stop;
            let _stop_on_exit = StopOnDrop(stop);
            let render_thread = scope.spawn(move || {
                let deadline = Duration::from_secs_f64(256.0 / 48_000.0);
                let mut output = [0.0; 512];
                let mut times = Vec::with_capacity(max_blocks);
                let mut memory = (0, 0);
                let mut peak = 0.0_f32;
                let mut stopped_blocks = 0;
                let mut active_histogram = vec![0_u64; declared_held_voices + 1];
                let mut active_audio_blocks = 0_u64;
                let audio_started = Instant::now();
                for block in 0..max_blocks {
                    let (elapsed, counts) = measure_render(&mut renderer, &mut output);
                    times.push(elapsed);
                    memory.0 += counts.0;
                    memory.1 += counts.1;
                    peak = output
                        .iter()
                        .fold(peak, |peak, sample| peak.max(sample.abs()));
                    if stop.load(Ordering::Acquire) {
                        stopped_blocks += 1;
                        if stopped_blocks >= 64 {
                            break;
                        }
                    } else {
                        let active =
                            audio_observation.read_latest_on_control().active_notes() as usize;
                        *active_histogram
                            .get_mut(active)
                            .expect("observed voices exceed the explicitly requested workload") +=
                            1;
                        active_audio_blocks += 1;
                    }
                    // Host cadence is simulated outside the measured callback.
                    // OS wakeup jitter is not counted as DSP execution time.
                    let next = Duration::from_secs_f64((block + 1) as f64 * 256.0 / 48_000.0);
                    if let Some(remaining) = next.checked_sub(audio_started.elapsed()) {
                        std::thread::sleep(remaining);
                    }
                    assert!(
                        audio_started.elapsed() < watchdog,
                        "audio workload watchdog expired"
                    );
                }
                assert!(
                    stopped_blocks >= 64,
                    "control/worker soak exceeded its bounded duration"
                );
                assert!(peak > 1e-6);
                (
                    Measurement::new(
                        "concurrent/callback/p16/slots3/returns8",
                        times,
                        deadline,
                        deadline.mul_f64(0.8),
                        Some(memory),
                    ),
                    audio_started.elapsed(),
                    active_histogram,
                    active_audio_blocks,
                )
            });
            loop {
                if requested_duration.map_or(ticks >= 256, |duration| {
                    started.elapsed() >= duration && requested.is_none()
                }) {
                    break;
                }
                assert!(
                    started.elapsed() < watchdog,
                    "control workload watchdog expired"
                );
                let tick = ticks;
                let start = Instant::now();
                // Do not begin an activation in the final half-second; complete
                // all accepted requests before normal shutdown and evidence capture.
                let request_allowed = requested_duration.is_none_or(|duration| {
                    started.elapsed() + Duration::from_millis(512) < duration
                });
                if tick % 32 == 0 && request_allowed {
                    assert!(
                        requested.is_none(),
                        "previous worker request failed to finish in 32 ticks"
                    );
                    let entry = if tick % 64 == 0 {
                        None
                    } else {
                        Some(fixtures.effects.descriptors()[0].id().clone())
                    };
                    app.dispatch(AppEvent::SetSlotOccupancy {
                        patch_id: PatchId::new(1).unwrap(),
                        slot: EffectSlotIndex::ALL[0],
                        entry,
                    })
                    .unwrap();
                    request_times.push(start.elapsed());
                    requested = Some(Instant::now());
                    requests += 1;
                }
                let stage_start = Instant::now();
                let progress = app.advance_structural().unwrap();
                advance_times.push(stage_start.elapsed());
                if progress.activation_acknowledged().is_some() {
                    let stage_start = Instant::now();
                    swaps += 1;
                    worker_times.push(requested.take().unwrap().elapsed());
                    // Graph replacement may reset voices; explicitly retrigger
                    // through the reducer, preserving live-load evidence.
                    for note in 36..36 + voices {
                        for kind in [MidiMessageKind::NoteOff, MidiMessageKind::NoteOn] {
                            assert!(app
                                .dispatch_midi_from(
                                    message(kind, note, 96),
                                    EventSource::AutomaticMidi
                                )
                                .unwrap()
                                .boundary_full()
                                .is_none());
                        }
                    }
                    retrigger_times.push(stage_start.elapsed());
                }
                let stage_start = Instant::now();
                for index in 0..8 {
                    assert!(app
                        .dispatch_midi_from(
                            message(MidiMessageKind::PitchBend, ((tick + index) % 128) as u8, 64),
                            EventSource::AutomaticMidi
                        )
                        .unwrap()
                        .boundary_full()
                        .is_none());
                }
                midi_times.push(stage_start.elapsed());
                let stage_start = Instant::now();
                std::hint::black_box(
                    serde_json::to_vec(app.current_graphical_shell().semantic_model()).unwrap(),
                );
                serialization_times.push(stage_start.elapsed());
                tick_times.push(start.elapsed());
                ticks += 1;
                if requested_duration.is_some() && started.elapsed() >= next_progress {
                    let observed = observation.read_latest_on_control();
                    println!(
                        "CREST_SOAK_PROGRESS {}",
                        serde_json::json!({
                            "wall_seconds": started.elapsed().as_secs_f64(), "control_ticks": ticks,
                            "graph_activations": swaps, "active_notes": observed.active_notes(),
                            "rendered_frames": observed.rendered_frames(), "routing_failures": observed.routing_failures(),
                            "voice_limit_refusals": observed.voice_limit_refusals(), "non_finite_samples": observed.non_finite_samples()
                        })
                    );
                    next_progress += Duration::from_secs(30);
                }
                std::thread::sleep(Duration::from_millis(16));
            }
            assert!(requested.is_none());
            assert_eq!(swaps, requests, "all admitted graph changes must activate");
            if requested_duration.is_none() {
                assert_eq!(swaps, 8);
            }
            assert!(
                swaps > 0,
                "concurrent workload must exercise graph preparation"
            );
            for note in 36..36 + voices {
                assert!(app
                    .dispatch_midi_from(
                        message(MidiMessageKind::NoteOff, note, 0),
                        EventSource::AutomaticMidi
                    )
                    .unwrap()
                    .boundary_full()
                    .is_none());
            }
            stop.store(true, Ordering::Release);
            let row = render_thread.join().unwrap();
            let final_observation = observation.read_latest_on_control();
            final_audio_observation = Some(final_observation);
            row
        },
    );
    app.shutdown_engine_selection_on_control().unwrap();
    let final_observation = final_audio_observation.unwrap();
    let contracts_passed = final_observation.routing_failures() == 0
        && final_observation.non_finite_samples() == 0
        && final_observation.voice_limit_refusals() == 0
        && final_observation.active_notes() == 0;
    let completed_duration = requested_duration.is_none_or(|duration| audio_wall >= duration);
    let strict_target = soak_seconds.map(|seconds| serde_json::json!({
        "requested_seconds": seconds, "completed_requested_duration": completed_duration,
        "thirty_minutes_observed": seconds >= 1800 && audio_wall >= Duration::from_secs(1800),
        "p99_budget_us": audio_row.deadline_us * 0.7,
        "p99_us": audio_row.p99_us, "deadline_misses": audio_row.deadline_misses,
        "required_deadline_misses": 0, "rust_allocations": audio_row.allocations,
        "rust_deallocations": audio_row.deallocations,
        "passed": completed_duration && contracts_passed && audio_row.tracker_callback_target_passed(),
        "scope": "headless callback execution only; not physical device deadlines or the complete Tracker release gate"
    }));
    let evidence = serde_json::json!({
        "requested_seconds": soak_seconds, "wall_seconds": audio_wall.as_secs_f64(),
        "contracts_passed": contracts_passed,
        "sample_rate": 48000, "block_frames": 256, "patches": patches,
        "requested_held_voices_per_patch": voices, "requested_held_voices_total": declared_held_voices,
        "active_notes_histogram_by_count": active_histogram,
        "observed_active_notes_max": active_histogram.iter().rposition(|count| *count > 0),
        "active_audio_blocks": active_audio_blocks,
        "blocks_at_requested_held_voices": active_histogram[declared_held_voices],
        "persistent_full_voice_load": active_histogram[declared_held_voices] == active_audio_blocks,
        "graph_requests": requests, "graph_activations": swaps, "control_ticks": ticks,
        "rendered_frames": final_observation.rendered_frames(), "rendered_blocks": final_observation.rendered_blocks(),
        "nominal_rendered_seconds": final_observation.rendered_frames() as f64 / 48000.0,
        "commands_consumed": final_observation.commands_consumed(),
        "routing_failures": final_observation.routing_failures(), "voice_limit_refusals": final_observation.voice_limit_refusals(),
        "non_finite_samples": final_observation.non_finite_samples(), "final_active_notes": final_observation.active_notes(),
        "queue_high_water": null, "scheduled_late_events": null,
        "unmeasured": ["queue occupancy (not exposed by this seam)", "scheduled event lateness (immediate MIDI workload)", "native C/C++ allocation", "CPAL/device scheduling", "physical MIDI latency", "Tracker tracks", "painted UI"],
        "voice_scope": "actual native active-note observations; graph replacement resets voices before reducer-driven retrigger; one-shot engines may finish earlier",
        "strict_tracker_target": strict_target,
    });
    emit(audio_row, rows);
    for (stage, times) in [
        ("request", request_times),
        ("advance", advance_times),
        ("retrigger", retrigger_times),
        ("midi", midi_times),
        ("serialization", serialization_times),
    ] {
        emit(
            Measurement::new(
                format!("concurrent/control-stage/{stage}"),
                times,
                Duration::from_millis(16),
                Duration::from_millis(12),
                None,
            ),
            rows,
        );
    }
    emit(
        Measurement::new(
            "concurrent/control-tick/midi-serialize-worker",
            tick_times,
            Duration::from_millis(16),
            Duration::from_millis(12),
            None,
        ),
        rows,
    );
    emit(
        Measurement::new(
            "concurrent/worker-request-to-activation",
            worker_times,
            Duration::from_millis(512),
            Duration::from_millis(400),
            None,
        ),
        rows,
    );
    evidence
}

fn control_stage_cases(fixtures: &Fixtures, rows: &mut Vec<Measurement>) {
    for patches in [1, 16] {
        let mut state = fixtures.state(&[BRAIDS_CAPABILITY_ID], patches, 0, 3, 8);
        state
            .apply(AppEvent::EnterSurface(SurfaceId::PatchDetail))
            .unwrap();
        state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
        let mut reducer = Vec::with_capacity(256);
        let mut projection = Vec::with_capacity(256);
        let mut serialization = Vec::with_capacity(256);
        for index in 0..288 {
            let start = Instant::now();
            state
                .apply(AppEvent::Adjust(if index % 2 == 0 {
                    Direction::Right
                } else {
                    Direction::Left
                }))
                .unwrap();
            let reducer_elapsed = start.elapsed();
            let start = Instant::now();
            let projected = StateProjector::new()
                .project_with_shell_tree(&state)
                .unwrap();
            let projection_elapsed = start.elapsed();
            let start = Instant::now();
            let encoded = serde_json::to_vec(projected.3.semantic_model()).unwrap();
            let serialization_elapsed = start.elapsed();
            std::hint::black_box(encoded);
            if index >= 32 {
                reducer.push(reducer_elapsed);
                projection.push(projection_elapsed);
                serialization.push(serialization_elapsed);
            }
        }
        for (stage, times) in [
            ("reducer", reducer),
            ("projection", projection),
            ("serialization", serialization),
        ] {
            emit(
                Measurement::new(
                    format!("control-stage/{stage}/p{patches}"),
                    times,
                    Duration::from_millis(16),
                    Duration::from_millis(8),
                    None,
                ),
                rows,
            );
        }
    }
}
