use crest_synth::adapter::drum_rack_capability::*;
use crest_synth::adapter::drum_rack_preparer::DrumRackPreparer;
use crest_synth::adapter::lock_free_audio_boundary::{
    LockFreeAudioBoundary, LockFreeControlHandle,
};
use crest_synth::adapter::lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary;
use crest_synth::adapter::sample_capability::*;
use crest_synth::control::*;
use crest_synth::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crest_synth::kernel::{MidiChannel, PatchId};
use crest_synth::mixer::{
    global_parameters::GlobalParameters, mixer_state::MixerState, patch_output::PatchOutput,
};
use crest_synth::real_time::*;
use crest_synth::shell::audio_output::{AudioDeviceConfig, AudioSampleFormat};
use crest_synth::synth::*;
use crest_synth::testing::{
    DeterministicGraphPreparationWorker, DeterministicSampleCatalog, DeterministicSampleDecoder,
};
use std::cell::Cell;
use std::sync::Arc;

thread_local! { static RT_COUNTS: Cell<Option<(usize, usize)>> = const { Cell::new(None) }; }
struct CountingAllocator;
unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        RT_COUNTS.with(|c| {
            if let Some((a, d)) = c.get() {
                c.set(Some((a + 1, d)));
            }
        });
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        RT_COUNTS.with(|c| {
            if let Some((a, d)) = c.get() {
                c.set(Some((a, d + 1)));
            }
        });
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn provider() -> DrumRackCapability {
    DrumRackCapability::new(AssetFileId::new("Kick.wav").unwrap()).unwrap()
}
fn patch(config: InstrumentConfig) -> Patch {
    Patch::new(
        PatchId::new(1).unwrap(),
        "Drum Rack".into(),
        config,
        MidiChannel::new(0).unwrap(),
        PatchOutput::default(),
    )
}
fn assets() -> (
    Arc<DeterministicSampleCatalog>,
    Arc<DeterministicSampleDecoder>,
) {
    let decoded = |name: &str, frequency: f32| {
        let id = AssetFileId::new(name).unwrap();
        let samples = (0..4096)
            .map(|i| (i as f32 * frequency).sin() * 0.4)
            .collect();
        (
            id.clone(),
            Ok(DecodedSample::new(
                SampleMetadata::new(id, 16428, 48_000, 1, 32, SampleEncoding::Float, 4096).unwrap(),
                samples,
            )
            .unwrap()),
        )
    };
    (
        Arc::new(DeterministicSampleCatalog::new(
            [],
            [
                (AssetFileId::new("Kick.wav").unwrap(), Ok(vec![1])),
                (AssetFileId::new("Hat.wav").unwrap(), Ok(vec![2])),
            ],
        )),
        Arc::new(DeterministicSampleDecoder::new([
            decoded("Kick.wav", 0.03),
            decoded("Hat.wav", 0.3),
        ])),
    )
}
fn preparer() -> DrumRackPreparer {
    let (catalog, decoder) = assets();
    DrumRackPreparer::new(AssetFileId::new("Kick.wav").unwrap(), catalog, decoder).unwrap()
}
fn assigned(pads: &[(usize, &str)]) -> InstrumentConfig {
    let provider = provider();
    let registry = CapabilityRegistry::new(vec![provider.descriptor()]).unwrap();
    pads.iter()
        .fold(provider.default_config().unwrap(), |config, (pad, name)| {
            registry
                .replace_asset(
                    &config,
                    &pad_parameter_id(*pad, SAMPLE_ASSET_PARAMETER_ID),
                    AssetReference::new(AssetKind::Sample, *name).unwrap(),
                )
                .unwrap()
        })
}
fn state(config: InstrumentConfig) -> AppState {
    let registry = CapabilityRegistry::new(vec![provider().descriptor()]).unwrap();
    let folder = FileBrowserFolderId::default();
    let rows = ["Kick.wav", "Hat.wav", "Missing.wav"]
        .iter()
        .map(|name| {
            FileBrowserRow::new(
                format!("file:{name}"),
                *name,
                FileBrowserRowKind::File(AssetFileId::new(*name).unwrap()),
                Some(16428),
            )
            .unwrap()
        })
        .collect();
    let mut state = AppState::for_graph(
        registry,
        GlobalParameters::new(0.0).unwrap(),
        GraphRevision::INITIAL,
    )
    .with_sample_catalog([(
        folder.clone(),
        Ok(FileBrowserListing::new(folder, rows).unwrap()),
    )]);
    state
        .apply(AppEvent::InstallPatches(vec![patch(config)]))
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::SelectContext(TopLevelContext::Patch))
        .unwrap();
    state
}
fn snapshot(patch: &Patch, schema: &CapabilityDescriptor) -> RtPatchParameters {
    let values = schema
        .scalar_parameters()
        .map(|spec| {
            spec.scalar_value(patch.instrument_config().value(spec.id()).unwrap())
                .unwrap()
        })
        .collect::<Vec<_>>();
    RtPatchParameters::projected(
        patch.id(),
        patch.output(),
        *patch.envelope(),
        RtInstrumentParameters::new(&values).unwrap(),
    )
}
fn midi(kind: MidiMessageKind, note: u8, velocity: u8) -> MidiMessage {
    MidiMessage::try_new(MidiChannel::new(0).unwrap(), kind, note, velocity).unwrap()
}
fn focus_state(state: &mut AppState, id: &ParameterId) {
    if state.interaction().active_surface() != SurfaceId::PatchDetail {
        state
            .apply_semantic_action(SemanticAction::OpenRelated)
            .unwrap();
    }
    for _ in 0..32 {
        if state.interaction().focus_path().control_id()
            == &SemanticControlId::Patch(PatchControlId::Capability(id.clone()))
        {
            return;
        }
        state
            .apply_semantic_action(SemanticAction::Navigate(Direction::Up))
            .ok();
    }
    for _ in 0..32 {
        if state.interaction().focus_path().control_id()
            == &SemanticControlId::Patch(PatchControlId::Capability(id.clone()))
        {
            return;
        }
        state
            .apply_semantic_action(SemanticAction::Navigate(Direction::Down))
            .ok();
    }
    panic!("missing focus {id:?}");
}

#[test]
fn drum_rack_schema_selector_keeps_independent_edits_and_empty_assets() {
    let provider = provider();
    let descriptor = provider.descriptor();
    assert_eq!(descriptor.label(), "Drum Rack");
    assert_eq!(descriptor.asset_requirements().len(), 16);
    let selector = ParameterId::new(DRUM_RACK_PAD_PARAMETER_ID).unwrap();
    let choices = descriptor.parameter(&selector).unwrap().choices();
    assert_eq!(choices.len(), 16);
    assert_eq!(choices[0].label(), "C1 · Kick Drum");
    assert_eq!(choices[15].label(), "D#2 · Right");
    let registry = CapabilityRegistry::new(vec![descriptor.clone()]).unwrap();
    let default = DescriptorDefaultConfigFactory::new(registry, vec![Box::new(provider)])
        .create(descriptor.id())
        .unwrap();
    assert!(default.asset_references().is_empty());
    let mut state = state(default);
    focus_state(
        &mut state,
        &pad_parameter_id(0, SAMPLE_ROOT_NOTE_PARAMETER_ID),
    );
    state
        .apply_semantic_action(SemanticAction::SetInteractionMode(InteractionMode::Adjust))
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::Adjust(Direction::Right))
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::SetInteractionMode(
            InteractionMode::Navigate,
        ))
        .unwrap();
    focus_state(&mut state, &selector);
    let origin = state.interaction().focus_path().clone();
    state
        .apply_semantic_action(SemanticAction::SetInteractionMode(InteractionMode::Adjust))
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::Adjust(Direction::Up))
        .unwrap();
    assert_eq!(state.interaction().active_surface(), SurfaceId::PatchChoice);
    state
        .apply_semantic_action(SemanticAction::Navigate(Direction::Down))
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::Activate)
        .unwrap();
    assert_eq!(state.interaction().focus_path(), &origin);
    let config = state.patches()[0].instrument_config();
    assert_eq!(DrumRackCapability::selected_pad(config), Some(1));
    assert_eq!(
        config.value(&pad_parameter_id(0, SAMPLE_ROOT_NOTE_PARAMETER_ID)),
        Some(&ParameterValue::continuous(60.01).unwrap())
    );
    assert_eq!(
        config.value(&pad_parameter_id(1, SAMPLE_ROOT_NOTE_PARAMETER_ID)),
        Some(&ParameterValue::continuous(60.0).unwrap())
    );
    let shell = StateProjector::new().project_with_shell(&state).unwrap().3;
    let detail = shell
        .semantic_model()
        .surface(SurfaceId::PatchDetail)
        .unwrap();
    assert_eq!(detail.sections().len(), 4);
    assert!(detail
        .controls()
        .iter()
        .all(|row| !format!("{:?}", row.path()).contains("pad-0")));
    let wire = serde_json::to_value(detail).unwrap();
    assert_eq!(wire["sections"][0]["leadingControls"], true);
    assert_eq!(
        detail
            .visualizations()
            .iter()
            .filter(|v| matches!(v.data(), SemanticVisualizationData::Waveform { .. }))
            .count(),
        1
    );
    assert!(detail
        .controls()
        .iter()
        .any(|row| row.value() == &SemanticControlValue::Summary("EMPTY".into())));
}

#[test]
fn drum_rack_routes_all_sixteen_notes_at_original_pitch_and_mixes_independent_samples() {
    let config = assigned(
        &(0..16)
            .map(|pad| (pad, if pad == 6 { "Hat.wav" } else { "Kick.wav" }))
            .collect::<Vec<_>>(),
    );
    let patch = patch(config);
    let schema = provider().descriptor();
    let params = snapshot(&patch, &schema);
    let mut engine = preparer().prepare(&patch, 48_000.0, 256).unwrap();
    assert_eq!(engine.prepared_sample_visualizations().len(), 16);
    let mut reference = [0.0; 512];
    engine
        .dispatch(midi(MidiMessageKind::NoteOn, 36, 127), &params)
        .unwrap();
    engine.render(&mut reference, 256, &params).unwrap();
    assert!(reference.iter().any(|v| v.abs() > 0.01));
    for note in 36..52 {
        engine.all_notes_off();
        engine
            .dispatch(midi(MidiMessageKind::NoteOn, note, 127), &params)
            .unwrap();
        let mut output = [0.0; 512];
        engine.render(&mut output, 256, &params).unwrap();
        assert!(output.iter().all(|v| v.is_finite()));
        if note != 42 {
            assert_eq!(
                output, reference,
                "pad pitch must not follow mapping note {note}"
            );
        } else {
            assert_ne!(output, reference);
        }
    }
    engine.all_notes_off();
    engine
        .dispatch(midi(MidiMessageKind::NoteOn, 42, 127), &params)
        .unwrap();
    let mut hat = [0.0; 512];
    engine.render(&mut hat, 256, &params).unwrap();
    let mut hat_tail = [0.0; 512];
    engine.render(&mut hat_tail, 256, &params).unwrap();
    engine.all_notes_off();
    RT_COUNTS.with(|c| c.set(Some((0, 0))));
    engine
        .dispatch(midi(MidiMessageKind::NoteOn, 36, 127), &params)
        .unwrap();
    engine
        .dispatch(midi(MidiMessageKind::NoteOn, 42, 127), &params)
        .unwrap();
    let mut mix = [0.0; 512];
    engine.render(&mut mix, 256, &params).unwrap();
    engine
        .dispatch(midi(MidiMessageKind::NoteOff, 36, 0), &params)
        .unwrap();
    let mut remaining = [0.0; 512];
    engine.render(&mut remaining, 256, &params).unwrap();
    engine
        .dispatch(midi(MidiMessageKind::NoteOn, 42, 0), &params)
        .unwrap();
    engine.all_notes_off();
    let counts = RT_COUNTS.with(|c| c.replace(None).unwrap());
    assert_eq!(counts, (0, 0), "Rust callback allocation/destruction");
    assert_eq!(
        remaining, hat_tail,
        "kick note-off must not release the hat"
    );
    for ((mixed, kick), hat) in mix.iter().zip(reference).zip(hat) {
        assert!((*mixed - kick - hat).abs() < 0.00001);
    }
    // A compatible scalar snapshot edits only its addressed pad, regardless of the selector.
    let edited_config = patch
        .instrument_config()
        .with_scalar_value(
            &schema,
            &pad_parameter_id(6, SAMPLE_PLAYBACK_START_PARAMETER_ID),
            ParameterValue::continuous(0.25).unwrap(),
        )
        .unwrap()
        .with_scalar_value(
            &schema,
            &ParameterId::new(DRUM_RACK_PAD_PARAMETER_ID).unwrap(),
            ParameterValue::Choice("pad-15".into()),
        )
        .unwrap();
    let edited_patch = Patch::new(
        patch.id(),
        patch.name().into(),
        edited_config,
        patch.channel(),
        patch.output(),
    );
    let edited = snapshot(&edited_patch, &schema);
    engine
        .dispatch(midi(MidiMessageKind::NoteOn, 36, 127), &edited)
        .unwrap();
    engine.render(&mut mix, 256, &edited).unwrap();
    assert_eq!(mix, reference, "other pads preserve exact playback");
    engine.all_notes_off();
    engine
        .dispatch(midi(MidiMessageKind::NoteOn, 42, 127), &edited)
        .unwrap();
    engine.render(&mut mix, 256, &edited).unwrap();
    assert_ne!(
        mix, hat,
        "edited playback start reaches the selected Sample runtime"
    );
    engine.all_notes_off();
    for note in [0, 35, 52, 127] {
        assert!(!engine.accepts_note(note));
        engine
            .dispatch(midi(MidiMessageKind::NoteOn, note, 127), &params)
            .unwrap();
    }
    engine.render(&mut mix, 256, &params).unwrap();
    assert!(mix.iter().all(|v| *v == 0.0));
}

#[test]
fn drum_rack_restore_hydrates_all_assets_and_rejects_missing_sample() {
    let mut state = state(assigned(&[(0, "Kick.wav"), (6, "Hat.wav")]));
    focus_state(
        &mut state,
        &ParameterId::new(DRUM_RACK_PAD_PARAMETER_ID).unwrap(),
    );
    let saved = SavedSession::capture(&state);
    let json = serde_json::to_string(&saved).unwrap();
    let saved: SavedSession = SavedSession::from_json(&json, state.capabilities()).unwrap();
    let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(preparer())];
    let prepared = saved
        .prepare_restore(
            state.capabilities().clone(),
            state.effects().clone(),
            &preparers,
            &[],
            GraphRevision::INITIAL.checked_next().unwrap(),
            48_000.0,
            64,
        )
        .unwrap();
    let (payload, _graph) = prepared.into_replacement();
    let mut restored = state.clone();
    restored
        .apply(AppEvent::ReplacePersistedSession(Box::new(payload)))
        .unwrap();
    for name in ["Kick.wav", "Hat.wav"] {
        assert!(restored
            .sample_visualization(
                PatchId::new(1).unwrap(),
                &AssetReference::new(AssetKind::Sample, name).unwrap()
            )
            .is_some());
    }
    assert_eq!(SavedSession::capture(&restored), saved);
    let bad = patch(assigned(&[(0, "Missing.wav")]));
    assert!(matches!(
        preparer().prepare(&bad, 48_000.0, 64),
        Err(InstrumentPreparationError::SampleAsset { .. })
    ));
}

type LiveApp = AppLoop<LockFreeControlHandle>;
type Renderer = AudioRenderer<
    crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioHandle,
    crest_synth::adapter::lock_free_structural_graph_boundary::LockFreeStructuralAudioHandle,
>;
fn settle(
    app: &mut LiveApp,
    renderer: &mut Renderer,
    worker: &crest_synth::testing::DeterministicGraphPreparationHandle,
) {
    for _ in 0..12 {
        app.advance_structural().unwrap();
        worker.advance();
        renderer.render(&mut [0.0; 128]);
    }
}
fn app_focus(app: &mut LiveApp, id: &ParameterId) {
    for direction in [Direction::Up, Direction::Down] {
        for _ in 0..20 {
            if app
                .current_graphical_shell()
                .semantic_model()
                .focus_path()
                .control_id()
                == &SemanticControlId::Patch(PatchControlId::Capability(id.clone()))
            {
                return;
            }
            app.dispatch_action(SemanticAction::Navigate(direction))
                .ok();
        }
    }
    panic!("missing focus {id:?}");
}
fn select_pad(app: &mut LiveApp, pad: usize) {
    app_focus(app, &ParameterId::new(DRUM_RACK_PAD_PARAMETER_ID).unwrap());
    app.dispatch_action(SemanticAction::SetInteractionMode(InteractionMode::Adjust))
        .unwrap();
    while DrumRackCapability::selected_pad(app.patches()[0].instrument_config()) != Some(pad) {
        let current =
            DrumRackCapability::selected_pad(app.patches()[0].instrument_config()).unwrap();
        app.dispatch_action(SemanticAction::Adjust(if current < pad {
            Direction::Right
        } else {
            Direction::Left
        }))
        .unwrap();
    }
    app.dispatch_action(SemanticAction::SetInteractionMode(
        InteractionMode::Navigate,
    ))
    .unwrap();
}

#[test]
fn drum_rack_browser_worker_activation_preview_failure_and_channel_midi_use_production_path() {
    let state = state(provider().default_config().unwrap());
    let registry = state.capabilities().clone();
    let factory = DescriptorDefaultConfigFactory::new(registry.clone(), vec![Box::new(provider())]);
    let audio_config = AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 64).unwrap();
    let initial = ParameterSnapshot::new(
        0,
        GlobalParameters::new(0.0).unwrap(),
        MixerState::default(),
        &[],
    )
    .unwrap();
    let (control, audio) = LockFreeAudioBoundary::new(64, initial).into_handles();
    let mut app = AppLoop::new(
        state,
        StateProjector::for_graph(GraphRevision::INITIAL),
        control,
    )
    .unwrap();
    let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(preparer())];
    let graph = PreparedGraphBuilder::new(&registry, &preparers)
        .build(
            GraphRevision::INITIAL,
            app.patches(),
            app.current_parameters().clone(),
            48_000.0,
            64,
        )
        .unwrap();
    let (structural, structural_audio) = LockFreeStructuralGraphBoundary::new(
        1,
        1,
        GraphHandoffStatus::with_active(GraphRevision::INITIAL),
    )
    .unwrap()
    .into_handles();
    let worker = DeterministicGraphPreparationWorker::new(
        registry,
        vec![Box::new(preparer())],
        audio_config,
    );
    let handle = worker.advance_handle();
    app.configure_engine_selection(factory, worker, structural, &graph, audio_config)
        .unwrap();
    let mut renderer = AudioRenderer::new(audio, structural_audio, graph);
    app.dispatch_action(SemanticAction::OpenRelated).unwrap();
    app_focus(&mut app, &pad_parameter_id(0, SAMPLE_ASSET_PARAMETER_ID));
    let origin = app
        .current_graphical_shell()
        .semantic_model()
        .focus_path()
        .clone();
    app.dispatch_action(SemanticAction::OpenRelated).unwrap();
    let saved = app.capture_saved_session();
    app.dispatch_action(SemanticAction::PreviewStart).unwrap();
    settle(&mut app, &mut renderer, &handle);
    let mut preview = [0.0; 128];
    renderer.render(&mut preview);
    assert!(preview.iter().any(|v| v.abs() > 0.0001));
    assert_eq!(app.capture_saved_session(), saved);
    app.dispatch_action(SemanticAction::PreviewStop).unwrap();
    app.dispatch_action(SemanticAction::Return).unwrap();
    assert_eq!(
        app.current_graphical_shell().semantic_model().focus_path(),
        &origin
    );
    app.dispatch_action(SemanticAction::OpenRelated).unwrap();
    app.dispatch_action(SemanticAction::Activate).unwrap();
    assert!(app.patches()[0]
        .instrument_config()
        .asset_references()
        .is_empty());
    for _ in 0..4 {
        app.advance_structural().unwrap();
    }
    assert!(handle.advance());
    app.advance_structural().unwrap();
    assert!(
        app.patches()[0]
            .instrument_config()
            .asset_references()
            .is_empty(),
        "publication cannot commit assignment"
    );
    renderer.render(&mut [0.0; 128]);
    app.advance_structural().unwrap();
    assert_eq!(
        app.patches()[0]
            .instrument_config()
            .asset_reference(&pad_parameter_id(0, SAMPLE_ASSET_PARAMETER_ID))
            .unwrap()
            .locator(),
        "Kick.wav"
    );
    select_pad(&mut app, 6);
    app_focus(&mut app, &pad_parameter_id(6, SAMPLE_ASSET_PARAMETER_ID));
    app.dispatch_action(SemanticAction::OpenRelated).unwrap();
    app.dispatch_action(SemanticAction::Navigate(Direction::Down))
        .unwrap();
    app.dispatch_action(SemanticAction::Activate).unwrap();
    settle(&mut app, &mut renderer, &handle);
    let config = app.patches()[0].instrument_config();
    assert_eq!(
        config
            .asset_reference(&pad_parameter_id(0, SAMPLE_ASSET_PARAMETER_ID))
            .unwrap()
            .locator(),
        "Kick.wav"
    );
    assert_eq!(
        config
            .asset_reference(&pad_parameter_id(6, SAMPLE_ASSET_PARAMETER_ID))
            .unwrap()
            .locator(),
        "Hat.wav"
    );
    for pad in [0, 6] {
        select_pad(&mut app, pad);
        let shell = app.current_graphical_shell();
        let waveform = shell
            .semantic_model()
            .surface(SurfaceId::PatchDetail)
            .unwrap()
            .visualizations()
            .iter()
            .find(|v| matches!(v.data(), SemanticVisualizationData::Waveform { .. }))
            .unwrap();
        assert!(
            matches!(waveform.data(), SemanticVisualizationData::Waveform { pairs, status, .. } if !pairs.is_empty() && status == "READY")
        );
    }
    // Silent/unmapped notes cannot fill the host note budget and block loaded pads.
    for note in [
        35, 37, 38, 39, 40, 41, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 127,
    ] {
        app.dispatch_midi_from(
            midi(MidiMessageKind::NoteOn, note, 127),
            EventSource::PhysicalMidi,
        )
        .unwrap();
    }
    app.dispatch_midi_from(
        midi(MidiMessageKind::NoteOn, 36, 127),
        EventSource::PhysicalMidi,
    )
    .unwrap();
    let mut output = [0.0; 128];
    RT_COUNTS.with(|c| c.set(Some((0, 0))));
    renderer.render(&mut output);
    let counts = RT_COUNTS.with(|c| c.replace(None).unwrap());
    assert_eq!(counts, (0, 0));
    assert!(output.iter().any(|v| v.abs() > 0.0001));
    app.dispatch_midi_from(
        midi(MidiMessageKind::AllNotesOff, 0, 0),
        EventSource::PhysicalMidi,
    )
    .unwrap();
    renderer.render(&mut output);
    assert!(output.iter().all(|v| *v == 0.0));
    // Missing replacement leaves both assignments and waveform intact.
    app_focus(&mut app, &pad_parameter_id(6, SAMPLE_ASSET_PARAMETER_ID));
    let before = app.capture_saved_session();
    app.dispatch_action(SemanticAction::OpenRelated).unwrap();
    for _ in 0..2 {
        app.dispatch_action(SemanticAction::Navigate(Direction::Down))
            .unwrap();
    }
    app.dispatch_action(SemanticAction::Activate).unwrap();
    settle(&mut app, &mut renderer, &handle);
    assert_eq!(app.capture_saved_session(), before);
    assert!(
        serde_json::to_string(app.current_graphical_shell().semantic_model())
            .unwrap()
            .contains("UNAVAILABLE")
    );
    drop(renderer);
    app.shutdown_engine_selection_on_control().unwrap();
}
