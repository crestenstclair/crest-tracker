//! Braids edits must reach the production reducer, immutable UI and live graph.
use crest_synth::adapter::braids_capability::*;
use crest_synth::adapter::braids_preparer::BraidsPreparer;
use crest_synth::adapter::lock_free_audio_boundary::{
    LockFreeAudioBoundary, LockFreeControlHandle,
};
use crest_synth::control::{
    AppEvent, AppLoop, AppState, Direction, SavedSession, StateProjector, SurfaceId,
    TopLevelContext,
};
use crest_synth::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crest_synth::kernel::{MidiChannel, PatchId};
use crest_synth::mixer::{global_parameters::GlobalParameters, patch_output::PatchOutput};
use crest_synth::real_time::{
    AudioBoundary, AudioRenderer, GraphRevision, NoStructuralGraphChanges, PreparedGraphBuilder,
};
use crest_synth::shell::{KeyboardInputTranslator, WindowInput, WindowKey};
use crest_synth::synth::{
    CapabilityRegistry, InstrumentCapabilityProvider, InstrumentPreparer, Patch,
};

fn key(
    app: &mut AppLoop<LockFreeControlHandle>,
    keyboard: &mut KeyboardInputTranslator,
    key: WindowKey,
    modified: bool,
) {
    if modified {
        if let Some(action) = keyboard.translate(WindowInput::key_down(WindowKey::K)) {
            app.dispatch_action(action).unwrap();
        }
    }
    let action = keyboard.translate(WindowInput::key_down(key)).unwrap();
    let result = app.dispatch_action(action).unwrap();
    assert!(result.boundary_full().is_none());
    keyboard.translate(WindowInput::key_up(key));
    if modified {
        if let Some(action) = keyboard.translate(WindowInput::key_up(WindowKey::K)) {
            let result = app.dispatch_action(action);
            // Opening a Choice already returns to Navigate; modifier release
            // then has no further product effect.
            assert!(
                result.is_ok()
                    || result
                        == Err(crest_synth::control::EventRejection::ActionUnavailableInContext)
            );
        }
    }
}

#[test]
fn every_braids_control_edits_from_keyboard_without_rebuilding_and_persists() {
    let provider = BraidsCapability::new().unwrap();
    let registry = CapabilityRegistry::new(vec![provider.descriptor()]).unwrap();
    let mut state = AppState::new(registry.clone(), GlobalParameters::new(-12.0).unwrap());
    state
        .apply(AppEvent::InstallPatches(vec![Patch::new(
            PatchId::new(7).unwrap(),
            "Braids".into(),
            provider.default_config().unwrap(),
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        )]))
        .unwrap();
    state
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    let parameters = StateProjector::new().parameter_snapshot(&state).unwrap();
    let preparers: Vec<Box<dyn InstrumentPreparer>> =
        vec![Box::new(BraidsPreparer::new().unwrap())];
    let graph = PreparedGraphBuilder::new(&registry, &preparers)
        .build(
            GraphRevision::INITIAL,
            state.patches(),
            parameters.clone(),
            48_000.0,
            256,
        )
        .unwrap();
    let (control, audio) = LockFreeAudioBoundary::new(64, parameters).into_handles();
    let mut renderer = AudioRenderer::new(audio, NoStructuralGraphChanges::new(), graph);
    let mut app = AppLoop::new(state, StateProjector::new(), control).unwrap();
    let mut keyboard = KeyboardInputTranslator::new();
    key(&mut app, &mut keyboard, WindowKey::Return, false);
    let model_origin = app
        .current_graphical_shell()
        .semantic_model()
        .focus_path()
        .clone();
    let model = app.current_graphical_shell();
    let detail = model
        .semantic_model()
        .surface(SurfaceId::PatchDetail)
        .unwrap();
    assert!(
        detail.controls().iter().all(|control| control.editable()),
        "all Braids and envelope controls must be editable"
    );
    assert_eq!(detail.controls().len(), 7);

    // Adjacent scalar choice, then every authored model through the option page.
    key(&mut app, &mut keyboard, WindowKey::D, true);
    assert_eq!(
        app.current_parameters().patches()[0].instrument().values()[0],
        1.0
    );
    for index in 0..BRAIDS_MODELS.len() {
        key(&mut app, &mut keyboard, WindowKey::W, true);
        let current = app.current_parameters().patches()[0].instrument().values()[0] as usize;
        for _ in index..current {
            key(&mut app, &mut keyboard, WindowKey::W, false);
        }
        for _ in current..index {
            key(&mut app, &mut keyboard, WindowKey::S, false);
        }
        key(&mut app, &mut keyboard, WindowKey::Return, false);
        assert_eq!(
            app.current_graphical_shell().semantic_model().focus_path(),
            &model_origin
        );
        renderer.render(&mut [0.0; 512]);
        assert_eq!(
            renderer.parameters().patches()[0].instrument().values()[0],
            index as f32
        );
        assert_eq!(renderer.active_revision(), GraphRevision::INITIAL);
    }
    // Return to CSAW, whose Timbre and Color both affect the audio.
    key(&mut app, &mut keyboard, WindowKey::W, true);
    for _ in 1..BRAIDS_MODELS.len() {
        key(&mut app, &mut keyboard, WindowKey::W, false);
    }
    key(&mut app, &mut keyboard, WindowKey::Return, false);
    let note = MidiMessage::try_new(
        MidiChannel::new(0).unwrap(),
        MidiMessageKind::NoteOn,
        60,
        100,
    )
    .unwrap();
    app.dispatch(AppEvent::Midi {
        patch_id: PatchId::new(7).unwrap(),
        message: note,
    })
    .unwrap();
    let mut output = [0.0; 512];
    renderer.render(&mut output);
    assert!(output.iter().any(|sample| sample.abs() > 1e-6));
    for scalar in 1..=2 {
        key(&mut app, &mut keyboard, WindowKey::S, false);
        let origin = app
            .current_graphical_shell()
            .semantic_model()
            .focus_path()
            .clone();
        key(&mut app, &mut keyboard, WindowKey::D, true);
        key(&mut app, &mut keyboard, WindowKey::W, true);
        assert_eq!(
            app.current_parameters().patches()[0].instrument().values()[scalar],
            0.61
        );
        key(&mut app, &mut keyboard, WindowKey::A, true);
        key(&mut app, &mut keyboard, WindowKey::S, true);
        assert_eq!(
            app.current_parameters().patches()[0].instrument().values()[scalar],
            0.5
        );
        key(&mut app, &mut keyboard, WindowKey::W, true);
        assert_eq!(
            app.current_graphical_shell().semantic_model().focus_path(),
            &origin
        );
        renderer.render(&mut output);
        assert_eq!(
            renderer.parameters().patches()[0].instrument().values()[scalar],
            0.6
        );
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(output.iter().any(|sample| sample.abs() > 1e-6));
        for _ in 0..4 {
            key(&mut app, &mut keyboard, WindowKey::W, true);
        }
        let at_max = app.current_parameters().clone();
        assert_eq!(
            app.dispatch(AppEvent::Adjust(Direction::Right)),
            Err(crest_synth::control::EventRejection::ParameterAtBoundary)
        );
        assert_eq!(app.current_parameters().clone(), at_max);
        for _ in 0..10 {
            key(&mut app, &mut keyboard, WindowKey::S, true);
        }
        let at_min = app.current_parameters().clone();
        assert_eq!(
            app.dispatch(AppEvent::Adjust(Direction::Left)),
            Err(crest_synth::control::EventRejection::ParameterAtBoundary)
        );
        assert_eq!(app.current_parameters().clone(), at_min);
        for _ in 0..6 {
            key(&mut app, &mut keyboard, WindowKey::W, true);
        }
    }
    // Envelope rows remain editable through the same keyboard grammar.
    for key_direction in [WindowKey::D, WindowKey::D, WindowKey::A, WindowKey::D] {
        key(&mut app, &mut keyboard, WindowKey::S, false);
        key(&mut app, &mut keyboard, key_direction, true);
    }
    let saved = app.capture_saved_session();
    assert_eq!(
        SavedSession::from_json(&saved.to_json().unwrap(), &registry).unwrap(),
        saved
    );
    assert_eq!(
        app.current_parameters().graph_revision(),
        GraphRevision::INITIAL
    );
    assert_eq!(
        app.current_patch_page().unwrap().engine().status(),
        crest_synth::control::EngineSelectionStatusKind::Ready
    );
    let before = app.current_parameters().generation();
    assert!(app.dispatch(AppEvent::Adjust(Direction::Right)).is_ok());
    assert!(app.current_parameters().generation() > before);
}
