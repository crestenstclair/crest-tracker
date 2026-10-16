use core::alloc::{GlobalAlloc, Layout};
use core::cell::Cell;
use crest_synth::adapter::atomic_audio_observation::AtomicAudioObservation;
use crest_synth::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID;
use crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
use crest_synth::adapter::production_instruments::{
    production_capability_registry, production_soundfont_capability,
};
use crest_synth::control::{
    AppEvent, AppLoop, AppState, Direction, EventRejection, FocusPath, InteractionMode,
    PatchControlId, SemanticAction, SemanticControlId, SemanticSurfaceSummary, StateProjector,
    SurfaceId, TopLevelContext,
};
use crest_synth::kernel::midi_message::MidiMessage;
use crest_synth::kernel::{MidiChannel, PatchId};
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mix_engine::MixEngine;
use crest_synth::mixer::mix_observation::MixObservation;
use crest_synth::mixer::mixer_state::MixerState;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::mixer_track_parameters::{MixerTrackParameter, MixerTrackParameters};
use crest_synth::mixer::patch_output::{PatchOutput, PatchOutputParameter};
use crest_synth::real_time::audio_boundary::{
    AudioBoundary, AudioThreadBoundary, ControlAudioBoundary,
};
use crest_synth::real_time::audio_observation::{AudioObservation, ControlAudioObservation};
use crest_synth::real_time::{
    AudioObservationSnapshot, AudioRenderer, GraphRevision, NoStructuralGraphChanges,
    ParameterSnapshot, PatchAudioBlock, PreparedGraphBuilder, RtPatchParameters,
};
use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
use crest_synth::synth::{
    CapabilityId, InstrumentPreparationError, InstrumentPreparer, Patch, PreparedInstrument,
    PreparedInstrumentError,
};
use crest_synth::testing::automatic_midi_test::create_soundfont_config;
use std::alloc::System;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const SAMPLE_RATE: f32 = 48_000.0;
const FRAME_COUNT: usize = 2;
const SAMPLE_COUNT: usize = FRAME_COUNT * 2;
const HALF_GAIN_DB: f32 = -6.020_600_3;

thread_local! {
    static COUNT_MEMORY: Cell<bool> = const { Cell::new(false) };
    static ALLOCATION_COUNT: Cell<usize> = const { Cell::new(0) };
    static DEALLOCATION_COUNT: Cell<usize> = const { Cell::new(0) };
}

struct TestAllocator;

#[global_allocator]
static TEST_ALLOCATOR: TestAllocator = TestAllocator;

unsafe impl GlobalAlloc for TestAllocator {
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
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

fn record_allocation() {
    let _ = COUNT_MEMORY.try_with(|enabled| {
        if enabled.get() {
            let _ = ALLOCATION_COUNT.try_with(|count| count.set(count.get() + 1));
        }
    });
}

fn record_deallocation() {
    let _ = COUNT_MEMORY.try_with(|enabled| {
        if enabled.get() {
            let _ = DEALLOCATION_COUNT.try_with(|count| count.set(count.get() + 1));
        }
    });
}

fn begin_memory_count() {
    ALLOCATION_COUNT.with(|count| count.set(0));
    DEALLOCATION_COUNT.with(|count| count.set(0));
    COUNT_MEMORY.with(|enabled| enabled.set(true));
}

fn finish_memory_count() -> (usize, usize) {
    COUNT_MEMORY.with(|enabled| enabled.set(false));
    (
        ALLOCATION_COUNT.with(Cell::get),
        DEALLOCATION_COUNT.with(Cell::get),
    )
}

fn globals() -> GlobalParameters {
    GlobalParameters::new(0.0).unwrap()
}

fn track(index: u8) -> MixerTrackId {
    MixerTrackId::new(index).unwrap()
}

fn patch(id: u32, channel: u8, output: PatchOutput) -> Patch {
    let provider = production_soundfont_capability().unwrap();
    Patch::new(
        PatchId::new(id).unwrap(),
        format!("Routing Patch {id}"),
        create_soundfont_config(
            &provider,
            SoundFontInstrument::new(0, (id as u8 - 1) * 8, false).unwrap(),
        )
        .unwrap(),
        MidiChannel::new(channel).unwrap(),
        output,
    )
}

fn installed_state(outputs: &[PatchOutput], mixer: MixerState) -> AppState {
    let mut state = AppState::new(production_capability_registry().unwrap(), globals())
        .with_initial_mixer(mixer);
    state
        .apply(AppEvent::InstallPatches(
            outputs
                .iter()
                .copied()
                .enumerate()
                .map(|(index, output)| patch(index as u32 + 1, index as u8, output))
                .collect(),
        ))
        .unwrap();
    state
}

/// Navigates from the PATCH Utility entry row (TrimGain) to the output-track
/// row through the one declared five-row order, rather than assuming the two
/// are adjacent — MIDI input is seated between them.
fn navigate_to_output_track(state: &mut AppState) {
    let order = PatchControlId::utility_surface_descriptor();
    let index = |control: &PatchControlId| {
        order
            .iter()
            .position(|candidate| candidate == control)
            .expect("the declared Utility order hosts this row")
    };
    let steps = index(&PatchControlId::Output(PatchOutputParameter::OutputTrack))
        - index(&PatchControlId::Output(PatchOutputParameter::TrimGain));
    for _ in 0..steps {
        state
            .apply_semantic_action(SemanticAction::Navigate(Direction::Down))
            .unwrap();
    }
}

fn enter_patch_utility(state: &mut AppState) {
    state
        .apply_semantic_action(SemanticAction::SelectContext(TopLevelContext::Patch))
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::Navigate(Direction::Right))
        .unwrap();
    assert_eq!(
        state.interaction().active_surface(),
        SurfaceId::PatchUtility
    );
}

fn set_mode(state: &mut AppState, mode: InteractionMode) {
    state
        .apply_semantic_action(SemanticAction::SetInteractionMode(mode))
        .unwrap();
}

fn approximately(actual: f32, expected: f32) -> bool {
    (actual - expected).abs() <= 1.0e-6
}

fn sample_rms(samples: &[f32]) -> f32 {
    let energy: f64 = samples
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum();
    (energy / samples.len() as f64).sqrt() as f32
}

fn assert_samples(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert!(
            approximately(*actual, *expected),
            "sample {index}: expected {expected}, got {actual}"
        );
    }
}

/// A drop-counting unity return: proves `mix` never destroys its prepared
/// effects (the guarantee the retired port probe carried) while the per-bus
/// send evidence now comes from `MixObservation`'s indexed measurements.
struct DropProbeReturn {
    drops: Arc<AtomicUsize>,
}

impl Drop for DropProbeReturn {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::Relaxed);
    }
}

impl crest_synth::synth::PreparedPostEffect for DropProbeReturn {
    fn patch_id(&self) -> PatchId {
        PatchId::new(u32::MAX).unwrap()
    }

    fn slot_id(&self) -> crest_synth::synth::EffectSlotId {
        crest_synth::synth::EffectSlotId::new(1).unwrap()
    }

    fn process(
        &mut self,
        _interleaved_stereo: &mut [f32],
        _frame_count: usize,
        _parameters: &crest_synth::real_time::RtPostEffectParameters,
    ) -> Result<(), crest_synth::synth::PreparedEffectError> {
        Ok(())
    }
}

struct MixRun {
    output: [f32; SAMPLE_COUNT],
    observation: MixObservation,
}

fn parameter_snapshot(
    generation: u64,
    outputs: &[PatchOutput],
    mixer: MixerState,
) -> ParameterSnapshot {
    let patches = outputs
        .iter()
        .copied()
        .enumerate()
        .map(|(index, output)| {
            RtPatchParameters::new(PatchId::new(index as u32 + 1).unwrap(), output)
        })
        .collect::<Vec<_>>();
    ParameterSnapshot::for_graph(
        generation,
        GraphRevision::INITIAL,
        globals(),
        mixer,
        &patches,
    )
    .unwrap()
}

fn run_mix(
    outputs: &[PatchOutput],
    mixer_state: MixerState,
    stems: &[[f32; SAMPLE_COUNT]],
) -> MixRun {
    assert_eq!(outputs.len(), stems.len());
    let parameters = parameter_snapshot(1, outputs, mixer_state);
    let mut patch_audio = PatchAudioBlock::prepare(FRAME_COUNT).unwrap();
    patch_audio.begin_render(&parameters, FRAME_COUNT).unwrap();
    for (index, (patch, stem)) in parameters.patches().iter().zip(stems).enumerate() {
        patch_audio
            .stem_mut(index, patch.patch_id().unwrap())
            .unwrap()
            .copy_from_slice(stem);
    }

    let drops = Arc::new(AtomicUsize::new(0));
    let mut mixer = MixEngine::new();
    mixer.prepare(SAMPLE_RATE, FRAME_COUNT).unwrap();
    mixer
        .install_bus_return(
            crest_synth::mixer::bus_id::BusId::ALL[7],
            Box::new(DropProbeReturn {
                drops: Arc::clone(&drops),
            }),
            crest_synth::real_time::RtPostEffectParameters::new(
                crest_synth::synth::EffectSlotId::new(1).unwrap(),
                &[],
            )
            .unwrap(),
            1.0,
        )
        .unwrap();
    let mut output = [0.0; SAMPLE_COUNT];
    let observation = mixer.mix(&patch_audio, &parameters, &mut output);
    assert_eq!(
        drops.load(Ordering::Relaxed),
        0,
        "mix must not destroy its prepared effects"
    );

    MixRun {
        output,
        observation,
    }
}

struct ConstantPreparer {
    capability_id: CapabilityId,
    instrument_drops: Arc<AtomicUsize>,
}

impl InstrumentPreparer for ConstantPreparer {
    fn capability_id(&self) -> &CapabilityId {
        &self.capability_id
    }

    fn prepare(
        &self,
        patch: &Patch,
        _sample_rate: f32,
        _max_frames: usize,
    ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
        Ok(Box::new(ConstantInstrument {
            patch_id: patch.id(),
            amplitude: 0.1 * patch.id().value() as f32,
            drops: Arc::clone(&self.instrument_drops),
        }))
    }
}

struct ConstantInstrument {
    patch_id: PatchId,
    amplitude: f32,
    drops: Arc<AtomicUsize>,
}

impl Drop for ConstantInstrument {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::Relaxed);
    }
}

impl PreparedInstrument for ConstantInstrument {
    fn patch_id(&self) -> PatchId {
        self.patch_id
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
        interleaved_stereo: &mut [f32],
        _frame_count: usize,
        _parameters: &RtPatchParameters,
    ) -> Result<(), crest_synth::synth::PreparedInstrumentError> {
        for sample in interleaved_stereo {
            *sample = self.amplitude;
        }

        Ok(())
    }

    fn all_notes_off(&mut self) {}
}

#[test]
fn production_path_proves_canonical_sixteen_track_routing() {
    let shared_outputs = [
        PatchOutput::to_track(track(3)),
        PatchOutput::to_track(track(3)),
        PatchOutput::to_track(track(7)),
    ];
    let mut distinctive_mixer = MixerState::default();
    for track_id in MixerTrackId::ALL {
        let index = track_id.index() as f32;
        distinctive_mixer.set_track(
            track_id,
            MixerTrackParameters::from_values(
                -index,
                (index - 7.5) / 10.0,
                track_id.index() % 3 == 0,
                track_id.index() % 5 == 0,
                [
                    index / 30.0,
                    (15.0 - index) / 30.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                ],
            )
            .unwrap(),
        );
    }

    let mut state = installed_state(&shared_outputs, distinctive_mixer.clone());
    assert_eq!(*state.mixer(), distinctive_mixer);
    assert_eq!(state.mixer().tracks().len(), MixerTrackId::COUNT);
    assert_eq!(
        MixerTrackId::ALL.map(|track_id| track_id.to_string()),
        [
            "T00", "T01", "T02", "T03", "T04", "T05", "T06", "T07", "T08", "T09", "T0A", "T0B",
            "T0C", "T0D", "T0E", "T0F",
        ]
    );
    assert!(MixerTrackId::new(16).is_err());

    let projector = StateProjector::new();
    let (_snapshot, _page, _text, shell, parameters, tree) =
        projector.project_with_shell_tree(&state).unwrap();
    assert_eq!(parameters.mixer_tracks(), distinctive_mixer.tracks());
    assert_eq!(parameters.patch_count(), shared_outputs.len());
    for (projected, expected) in parameters.patches().iter().zip(shared_outputs) {
        assert_eq!(projected.output(), expected);
    }
    let tree_json: serde_json::Value = serde_json::from_str(tree.json()).unwrap();
    assert_eq!(
        tree_json["mixer"]["tracks"].as_array().unwrap().len(),
        MixerTrackId::COUNT
    );
    assert_eq!(
        tree_json["parameters"]["mixerTracks"]
            .as_array()
            .unwrap()
            .len(),
        MixerTrackId::COUNT
    );

    let semantic = shell.semantic_model();
    let main = semantic.surface(SurfaceId::MixerMain).unwrap();
    assert_eq!(
        main.controls().len(),
        MixerTrackId::COUNT * MixerTrackParameter::MAIN.len()
    );
    for (track_id, controls) in MixerTrackId::ALL.into_iter().zip(
        main.controls()
            .chunks_exact(MixerTrackParameter::MAIN.len()),
    ) {
        for (parameter, control) in MixerTrackParameter::MAIN.into_iter().zip(controls) {
            assert_eq!(control.path(), &FocusPath::mixer_track(track_id, parameter));
        }
    }
    let inspector = semantic.surface(SurfaceId::MixerInspector).unwrap();
    match inspector.summary() {
        SemanticSurfaceSummary::MixerInspector {
            focused_track,
            routed_patches,
            ..
        } => {
            assert_eq!(*focused_track, track(0));
            assert!(routed_patches.is_empty(), "T00 is intentionally empty");
        }
        other => panic!("expected Mixer Inspector summary, got {other:?}"),
    }
    assert!(state
        .patches()
        .iter()
        .all(|patch| !matches!(patch.output().track_id().index(), 0 | 15)));

    let mixer_before_navigation = state.mixer().clone();
    let parameters_before_navigation = projector.parameter_snapshot(&state).unwrap();
    assert_eq!(
        state.interaction().focus_path(),
        &FocusPath::mixer_track(track(0), MixerTrackParameter::Level)
    );
    for _ in 1..MixerTrackId::COUNT {
        state
            .apply_semantic_action(SemanticAction::Navigate(Direction::Right))
            .unwrap();
    }
    assert_eq!(
        state.interaction().focus_path(),
        &FocusPath::mixer_track(track(15), MixerTrackParameter::Level)
    );
    let boundary_state = state.clone();
    assert_eq!(
        state.apply_semantic_action(SemanticAction::Navigate(Direction::Right)),
        Err(EventRejection::ActionUnavailableInContext)
    );
    assert_eq!(state, boundary_state);
    assert_eq!(*state.mixer(), mixer_before_navigation);
    assert!(parameters_before_navigation
        .audio_values_equal(&projector.parameter_snapshot(&state).unwrap()));

    let default_outputs = [
        PatchOutput::to_track(track(3)),
        PatchOutput::to_track(track(3)),
    ];
    let mut control_state = installed_state(&default_outputs, MixerState::default());
    for (index, _parameter) in MixerTrackParameter::MAIN.into_iter().enumerate() {
        set_mode(&mut control_state, InteractionMode::Adjust);
        control_state
            .apply_semantic_action(SemanticAction::Adjust(Direction::Right))
            .unwrap();
        set_mode(&mut control_state, InteractionMode::Navigate);
        if index + 1 < MixerTrackParameter::MAIN.len() {
            control_state
                .apply_semantic_action(SemanticAction::Navigate(Direction::Down))
                .unwrap();
        }
    }
    control_state
        .apply_semantic_action(SemanticAction::EnterSurface(SurfaceId::MixerInspector))
        .unwrap();
    // The Inspector's first region is the eight indexed sends of the selected
    // track, in ascending BusId order; each accepts the same fine edit.
    for (index, _) in crest_synth::mixer::bus_id::BusId::ALL
        .into_iter()
        .enumerate()
    {
        set_mode(&mut control_state, InteractionMode::Adjust);
        control_state
            .apply_semantic_action(SemanticAction::Adjust(Direction::Right))
            .unwrap();
        set_mode(&mut control_state, InteractionMode::Navigate);
        if index + 1 < crest_synth::mixer::bus_id::BusId::COUNT {
            control_state
                .apply_semantic_action(SemanticAction::Navigate(Direction::Down))
                .unwrap();
        }
    }
    let edited_track = control_state.mixer().track(track(0)).clone();
    assert_eq!(
        edited_track,
        MixerTrackParameters::from_values(
            1.0,
            0.01,
            true,
            true,
            [0.01; crest_synth::mixer::bus_id::MAX_BUS_RETURNS]
        )
        .unwrap()
    );
    assert!(
        MixerTrackId::ALL[1..]
            .iter()
            .all(|track_id| control_state.mixer().track(*track_id)
                == &MixerTrackParameters::default())
    );
    assert_eq!(MixerTrackParameter::MAIN.len(), 4);
    let projected_controls = projector.project_with_shell_tree(&control_state).unwrap();
    assert_eq!(projected_controls.4.mixer_track(track(0)), &edited_track);

    let mut output_state = installed_state(&default_outputs, MixerState::default());
    let output_mixer_before = output_state.mixer().clone();
    let patches_before = output_state.patches().to_vec();
    let graph_before = output_state.engine_selection().projection_graph_revision();
    enter_patch_utility(&mut output_state);
    set_mode(&mut output_state, InteractionMode::Adjust);
    output_state
        .apply_semantic_action(SemanticAction::Adjust(Direction::Right))
        .unwrap();
    set_mode(&mut output_state, InteractionMode::Navigate);
    navigate_to_output_track(&mut output_state);
    set_mode(&mut output_state, InteractionMode::Adjust);
    output_state
        .apply_semantic_action(SemanticAction::Adjust(Direction::Right))
        .unwrap();
    let edited_output = output_state.patches()[0].output();
    assert!(approximately(
        edited_output.trim_gain_db(),
        PatchOutputParameter::TrimGain
            .descriptor()
            .fine_step()
            .unwrap()
    ));
    assert_eq!(edited_output.track_id(), track(4));
    assert_eq!(output_state.patches()[1], patches_before[1]);
    assert_eq!(*output_state.mixer(), output_mixer_before);
    assert_eq!(
        output_state.engine_selection().projection_graph_revision(),
        graph_before
    );
    let output_projection = projector.project_with_shell_tree(&output_state).unwrap();
    assert_eq!(output_projection.4.patches()[0].output(), edited_output);
    assert_eq!(
        output_projection.4.mixer_tracks(),
        output_mixer_before.tracks()
    );
    let utility = output_projection
        .3
        .semantic_model()
        .surface(SurfaceId::PatchUtility)
        .unwrap();
    assert_eq!(
        utility
            .controls()
            .iter()
            .map(|control| control.path().control_id().clone())
            .collect::<Vec<_>>(),
        PatchControlId::utility_surface_descriptor()
            .iter()
            .cloned()
            .map(SemanticControlId::Patch)
            .collect::<Vec<_>>()
    );

    let mut invalid_state =
        installed_state(&[PatchOutput::to_track(track(15))], MixerState::default());
    enter_patch_utility(&mut invalid_state);
    navigate_to_output_track(&mut invalid_state);
    set_mode(&mut invalid_state, InteractionMode::Adjust);
    let invalid_parameters = projector.parameter_snapshot(&invalid_state).unwrap();
    let boundary = LockFreeAudioBoundary::new(4, invalid_parameters.clone());
    let (control, mut audio) = boundary.into_handles();
    let mut app_loop = AppLoop::new(invalid_state, StateProjector::new(), control).unwrap();
    let rejected_tree = app_loop.current_state_tree();
    assert_eq!(
        app_loop.dispatch_action(SemanticAction::Adjust(Direction::Right)),
        Err(EventRejection::ParameterAtBoundary)
    );
    assert_eq!(
        app_loop.current_state_tree().state_hash(),
        rejected_tree.state_hash()
    );
    assert_eq!(app_loop.current_parameters().clone(), invalid_parameters);
    assert_eq!(audio.read_latest_parameters(), &invalid_parameters);
    app_loop
        .dispatch_action(SemanticAction::Adjust(Direction::Left))
        .unwrap();
    let recovered_parameters = audio.read_latest_parameters();
    assert_eq!(
        recovered_parameters.patches()[0].output().track_id(),
        track(14)
    );
    assert_eq!(
        recovered_parameters.graph_revision(),
        invalid_parameters.graph_revision()
    );
    assert_eq!(audio.pop_command(), None);

    let first_stem = [0.2, 0.4, 0.2, 0.4];
    let second_stem = [0.3, 0.1, 0.3, 0.1];
    let shared = run_mix(
        &default_outputs,
        MixerState::default(),
        &[first_stem, second_stem],
    );
    assert_samples(&shared.output, &[0.5, 0.5, 0.5, 0.5]);
    assert!(approximately(shared.observation.track(track(3)).rms(), 0.5));
    assert!(MixerTrackId::ALL
        .iter()
        .filter(|track_id| **track_id != track(3))
        .all(|track_id| shared.observation.track(*track_id).rms() == 0.0));

    let half_level = MixerState::default().with_track(
        track(3),
        MixerTrackParameters::from_values(HALF_GAIN_DB, 0.0, false, false, [0.0; 8]).unwrap(),
    );
    let shared_half = run_mix(&default_outputs, half_level, &[first_stem, second_stem]);
    assert_samples(&shared_half.output, &[0.25, 0.25, 0.25, 0.25]);
    assert!(approximately(
        shared_half.observation.track(track(3)).rms(),
        shared.observation.track(track(3)).rms() * 0.5
    ));

    let trimmed_outputs = [
        PatchOutput::new(track(3), HALF_GAIN_DB).unwrap(),
        PatchOutput::to_track(track(3)),
    ];
    let trimmed = run_mix(
        &trimmed_outputs,
        MixerState::default(),
        &[first_stem, second_stem],
    );
    assert_samples(&trimmed.output, &[0.4, 0.3, 0.4, 0.3]);

    let rerouted_outputs = [
        PatchOutput::to_track(track(3)),
        PatchOutput::to_track(track(4)),
    ];
    let rerouted = run_mix(
        &rerouted_outputs,
        MixerState::default(),
        &[first_stem, second_stem],
    );
    assert_samples(&rerouted.output, &shared.output);
    assert!(approximately(
        rerouted.observation.track(track(3)).rms(),
        0.1_f32.sqrt()
    ));
    assert!(approximately(
        rerouted.observation.track(track(4)).rms(),
        0.05_f32.sqrt()
    ));

    let pan_state = MixerState::default().with_track(
        track(3),
        MixerTrackParameters::from_values(0.0, 0.5, false, false, [0.0; 8]).unwrap(),
    );
    let panned = run_mix(&[PatchOutput::to_track(track(3))], pan_state, &[first_stem]);
    assert_samples(&panned.output, &[0.1, 0.4, 0.1, 0.4]);

    let send_parameters = MixerTrackParameters::from_values(
        HALF_GAIN_DB,
        0.0,
        false,
        false,
        [0.25, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    )
    .unwrap();
    let send_state = MixerState::default().with_track(track(3), send_parameters);
    let send_stem = [0.4, 0.2, 0.4, 0.2];
    let sends = run_mix(&[PatchOutput::to_track(track(3))], send_state, &[send_stem]);
    assert_samples(&sends.output, &[0.2, 0.1, 0.2, 0.1]);
    assert!(approximately(
        sends
            .observation
            .bus_input_rms(crest_synth::mixer::bus_id::BusId::ALL[0])
            .expect("bus belongs to observed default bank"),
        sample_rms(&[0.05, 0.025, 0.05, 0.025]),
    ));
    assert!(approximately(
        sends
            .observation
            .bus_input_rms(crest_synth::mixer::bus_id::BusId::ALL[1])
            .expect("bus belongs to observed default bank"),
        sample_rms(&[0.1, 0.05, 0.1, 0.05]),
    ));
    let sounding_meter = sends.observation.track(track(3));
    assert!(approximately(sounding_meter.left_peak(), 0.2));
    assert!(approximately(sounding_meter.right_peak(), 0.1));
    assert!(approximately(sounding_meter.rms(), 0.025_f32.sqrt()));

    let muted_state = MixerState::default().with_track(
        track(3),
        MixerTrackParameters::from_values(
            HALF_GAIN_DB,
            0.0,
            true,
            false,
            [0.25, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        )
        .unwrap(),
    );
    let muted = run_mix(
        &[PatchOutput::to_track(track(3))],
        muted_state,
        &[send_stem],
    );
    assert_samples(&muted.output, &[0.0; SAMPLE_COUNT]);
    for bus in crest_synth::mixer::bus_id::BusId::ALL {
        assert_eq!(
            muted
                .observation
                .bus_input_rms(bus)
                .expect("bus belongs to observed default bank"),
            0.0
        );
        assert_eq!(
            muted
                .observation
                .bus_output_rms(bus)
                .expect("bus belongs to observed default bank"),
            0.0
        );
    }
    assert_eq!(muted.observation.track(track(3)), sounding_meter);

    let solo_state = MixerState::default().with_track(
        track(3),
        MixerTrackParameters::from_values(0.0, 0.0, false, true, [0.0; 8]).unwrap(),
    );
    let soloed = run_mix(&rerouted_outputs, solo_state, &[first_stem, second_stem]);
    assert_samples(&soloed.output, &first_stem);

    let muted_solo_state = MixerState::default().with_track(
        track(3),
        MixerTrackParameters::from_values(0.0, 0.0, true, true, [0.5; 8]).unwrap(),
    );
    let muted_solo = run_mix(
        &rerouted_outputs,
        muted_solo_state,
        &[first_stem, second_stem],
    );
    assert_samples(&muted_solo.output, &[0.0; SAMPLE_COUNT]);
    assert!(muted_solo.observation.track(track(3)).rms() > 0.0);
    assert!(muted_solo.observation.track(track(4)).rms() > 0.0);

    let correlated = AudioObservationSnapshot::from_mix_with_graph_and_routing(
        9,
        9,
        FRAME_COUNT as u64,
        77,
        GraphRevision::INITIAL,
        0,
        0,
        0,
        None,
        muted.observation,
    );
    assert_eq!(correlated.parameter_generation(), 77);
    assert_eq!(correlated.active_graph_revision(), GraphRevision::INITIAL);
    assert_eq!(correlated.tracks(), muted.observation.tracks());

    let renderer_outputs = [
        PatchOutput::to_track(track(3)),
        PatchOutput::to_track(track(3)),
    ];
    let mut renderer_state = installed_state(&renderer_outputs, MixerState::default());
    let renderer_parameters = projector.parameter_snapshot(&renderer_state).unwrap();
    let instrument_drops = Arc::new(AtomicUsize::new(0));
    let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(ConstantPreparer {
        capability_id: CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap(),
        instrument_drops: Arc::clone(&instrument_drops),
    })];
    let graph = PreparedGraphBuilder::new(renderer_state.capabilities(), &preparers)
        .build(
            GraphRevision::INITIAL,
            renderer_state.patches(),
            renderer_parameters.clone(),
            SAMPLE_RATE,
            FRAME_COUNT,
        )
        .unwrap();
    let boundary = LockFreeAudioBoundary::new(4, renderer_parameters.clone());
    let (mut control, audio) = boundary.into_handles();
    let (observation_writer, observation_reader) = AtomicAudioObservation::default().into_handles();
    let mut renderer = AudioRenderer::with_observation(
        audio,
        NoStructuralGraphChanges::new(),
        graph,
        observation_writer,
    );
    let mut renderer_output = [0.0; SAMPLE_COUNT];

    begin_memory_count();
    renderer.render(&mut renderer_output);
    let (initial_allocations, initial_deallocations) = finish_memory_count();
    assert_eq!(initial_allocations, 0);
    assert_eq!(initial_deallocations, 0);
    assert_eq!(instrument_drops.load(Ordering::Relaxed), 0);
    assert!(renderer_output.iter().all(|sample| sample.is_finite()));
    assert!(renderer_output.iter().any(|sample| *sample != 0.0));
    let initial_observation = observation_reader.read_latest_on_control();
    assert_eq!(
        initial_observation.parameter_generation(),
        renderer_parameters.generation()
    );
    assert!(initial_observation.track(track(3)).rms() > 0.0);
    assert_eq!(initial_observation.track(track(4)).rms(), 0.0);

    enter_patch_utility(&mut renderer_state);
    navigate_to_output_track(&mut renderer_state);
    set_mode(&mut renderer_state, InteractionMode::Adjust);
    renderer_state
        .apply_semantic_action(SemanticAction::Adjust(Direction::Right))
        .unwrap();
    let route_parameters = projector.parameter_snapshot(&renderer_state).unwrap();
    assert_eq!(route_parameters.graph_revision(), GraphRevision::INITIAL);
    assert_eq!(route_parameters.patches()[0].output().track_id(), track(4));
    assert_eq!(route_parameters.patches()[1].output().track_id(), track(3));
    control.publish_parameters(route_parameters.clone());

    begin_memory_count();
    renderer.render(&mut renderer_output);
    let (route_allocations, route_deallocations) = finish_memory_count();
    assert_eq!(route_allocations, 0);
    assert_eq!(route_deallocations, 0);
    assert_eq!(instrument_drops.load(Ordering::Relaxed), 0);
    assert_eq!(renderer.active_revision(), GraphRevision::INITIAL);
    assert_eq!(renderer.parameters().clone(), route_parameters);
    let route_observation = observation_reader.read_latest_on_control();
    assert_eq!(
        route_observation.parameter_generation(),
        route_parameters.generation()
    );
    assert_eq!(
        route_observation.active_graph_revision(),
        GraphRevision::INITIAL
    );
    assert!(route_observation.track(track(3)).rms() > 0.0);
    assert!(route_observation.track(track(4)).rms() > 0.0);
    assert!(route_observation
        .tracks()
        .iter()
        .all(|meter| meter.rms().is_finite()));
    assert!(renderer_output.iter().all(|sample| sample.is_finite()));

    println!("CREST_ACCEPTANCE sixteen_track_mixer_routing passed");
}
