//! Page edges use production input, reducer, projection, and audio ownership.
#[allow(dead_code)]
mod support;

use crest_synth::adapter::braids_capability::BraidsCapability;
use crest_synth::adapter::production_effects::{
    production_chorus_config, production_effect_registry,
};
use crest_synth::adapter::production_instruments::production_capability_registry;
use crest_synth::control::{
    AppEvent, AppState, Direction, EventRejection, InteractionMode, MidiDeviceEffect,
    MidiInputDescriptor, MidiInputDeviceId, PatchControlId, PatchDetailSubject, PatchPositionId,
    SavedSession, SemanticAction, SemanticResolver, StateProjector, SurfaceId, TopLevelContext,
};
use crest_synth::kernel::{MidiChannel, PatchId};
use crest_synth::mixer::{mixer_track_id::MixerTrackId, patch_output::PatchOutput};
use crest_synth::shell::{KeyboardInputTranslator, WindowInput, WindowKey};
use crest_synth::synth::{effect_slot_id::EffectSlotIndex, Patch};

fn state() -> AppState {
    let config = BraidsCapability::new().unwrap().default_config().unwrap();
    let patches = [7, 42]
        .into_iter()
        .map(|id| {
            let mut patch = Patch::new(
                PatchId::new(id).unwrap(),
                format!("Patch {id}"),
                config.clone(),
                MidiChannel::new(0).unwrap(),
                PatchOutput::to_track(MixerTrackId::new(3).unwrap()),
            );
            for slot in &EffectSlotIndex::ALL[..2] {
                patch = patch.with_effect_slot(
                    *slot,
                    production_chorus_config(slot.instance_identity()).unwrap(),
                );
            }
            patch
        })
        .collect();
    let mut state = AppState::new_with_effects(
        production_capability_registry().unwrap(),
        production_effect_registry().unwrap(),
        support::globals(),
    );
    let factory = crest_synth::synth::DescriptorDefaultConfigFactory::new(
        state.capabilities().clone(),
        crest_synth::adapter::production_instruments::production_instrument_providers().unwrap(),
    );
    let blueprint =
        crest_synth::control::PatchCreationBlueprint::resolve(config.capability_id(), &factory)
            .unwrap();
    state = state.with_patch_creation_blueprint(blueprint);
    state.apply(AppEvent::InstallPatches(patches)).unwrap();
    state
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    state
}

fn page(state: &mut AppState, direction: Direction) {
    let saved = SavedSession::capture(state);
    let generation = state.generation();
    let outcome = state
        .apply_semantic_action(SemanticAction::NavigatePage(direction))
        .unwrap();
    assert_eq!(state.generation(), generation + 1);
    assert!(!outcome.accepted().saved_session_changed());
    assert_eq!(outcome.audio_command(), None);
    assert_eq!(outcome.engine_selection_effect(), None);
    assert_eq!(SavedSession::capture(state), saved);
    let model = StateProjector::new().project_with_shell(state).unwrap().3;
    assert_eq!(
        model.semantic_model().focus_path(),
        state.interaction().focus_path()
    );
    assert!(SemanticResolver::new(state).resolves(state.interaction().focus_path()));
}

fn key(
    state: &mut AppState,
    keyboard: &mut KeyboardInputTranslator,
    key: WindowKey,
) -> Result<(), EventRejection> {
    let action = keyboard.translate(WindowInput::key_down(key)).unwrap();
    keyboard.translate(WindowInput::key_up(key));
    state.apply_semantic_action(action).map(|_| ())
}

#[test]
fn each_subject_returns_exactly_one_edge_and_empty_effect_rejects() {
    let mut state = state();
    for row in 0..4 {
        let origin = state.interaction().focus_path().clone();
        if row == 3 {
            let before = state.clone();
            assert_eq!(
                state.apply(AppEvent::NavigatePage(Direction::Up)),
                Err(EventRejection::ActionUnavailableInContext)
            );
            assert_eq!(state, before);
        } else {
            page(&mut state, Direction::Up);
            assert_eq!(state.interaction().active_surface(), SurfaceId::PatchDetail);
            match state.interaction().detail_subject().unwrap() {
                PatchDetailSubject::Instrument { .. } => assert_eq!(row, 0),
                PatchDetailSubject::Effect { slot_id, .. } => {
                    assert_eq!(*slot_id, EffectSlotIndex::ALL[row - 1].instance_identity())
                }
            }
            assert_eq!(state.interaction().return_path().unwrap().origin(), &origin);
            page(&mut state, Direction::Down);
            assert_eq!(state.interaction().active_surface(), SurfaceId::PatchMain);
            assert_eq!(state.interaction().focus_path(), &origin);
        }
        if row < 3 {
            state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
        }
    }
}

#[test]
fn repeated_context_round_trips_recover_both_noninitial_roots() {
    let mut state = state();
    state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
    state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
    let patch = state.interaction().focus_path().clone();
    page(&mut state, Direction::Down);
    state.apply(AppEvent::Navigate(Direction::Right)).unwrap();
    state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
    let mixer = state.interaction().focus_path().clone();
    for _ in 0..3 {
        page(&mut state, Direction::Up);
        assert_eq!(state.interaction().focus_path(), &patch);
        assert_eq!(state.interaction().active_surface(), SurfaceId::PatchMain);
        page(&mut state, Direction::Down);
        assert_eq!(state.interaction().focus_path(), &mixer);
    }
    state
        .apply(AppEvent::EnterSurface(SurfaceId::MixerInspector))
        .unwrap();
    page(&mut state, Direction::Up);
    assert_eq!(state.interaction().focus_path(), &patch);
}

#[test]
fn repaired_origin_is_exposed_then_page_return_lands_on_that_identity() {
    let mut state = state();
    let patch_id = state.patches()[0].id();
    page(&mut state, Direction::Up);
    state
        .apply(AppEvent::SetPatchOverviewOriginEnabled {
            patch_id,
            control: PatchControlId::Engine,
            enabled: false,
        })
        .unwrap();
    let repair = state.focus_repair_status().unwrap();
    let destination = repair.replacement_origin().clone();
    assert_eq!(
        destination.control_id(),
        &crest_synth::control::SemanticControlId::Patch(PatchControlId::EffectSlot(
            EffectSlotIndex::ALL[0]
        ))
    );
    assert!(StateProjector::new()
        .project_with_shell(&state)
        .unwrap()
        .3
        .semantic_model()
        .focus_repair()
        .is_some());
    page(&mut state, Direction::Down);
    assert_eq!(state.interaction().focus_path(), &destination);
}

#[test]
fn settings_entry_keeps_patch_and_roots_through_discovery_and_authored_return() {
    let mut state = state();
    state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
    let origin = state.interaction().clone();
    let outcome = state
        .apply(AppEvent::NavigatePage(Direction::Left))
        .unwrap();
    let [MidiDeviceEffect::Scan { scan_id }] = outcome.midi_device_effects() else {
        panic!("one discovery effect")
    };
    state
        .apply(AppEvent::MidiInputScanSucceeded {
            scan_id: *scan_id,
            descriptors: vec![MidiInputDescriptor::new(
                MidiInputDeviceId::new("midir-v1", "port").unwrap(),
                "Test input",
                None,
            )
            .unwrap()],
        })
        .unwrap();
    assert_eq!(state.interaction().patch_focus(), origin.patch_focus());
    assert_eq!(
        state.interaction().remembered_patch_main(),
        origin.remembered_patch_main()
    );
    assert_eq!(
        state.interaction().remembered_mixer_main(),
        origin.remembered_mixer_main()
    );
    for direction in [Direction::Left, Direction::Up] {
        let before = state.clone();
        assert!(state.apply(AppEvent::NavigatePage(direction)).is_err());
        assert_eq!(state, before);
    }
    let midi = state.midi_input().clone();
    let mut compatibility = state.clone();
    page(&mut state, Direction::Right);
    assert_eq!(state.interaction(), &origin);
    assert_eq!(state.midi_input(), &midi);
    page(&mut compatibility, Direction::Down);
    assert_eq!(compatibility.interaction(), &origin);
}

#[test]
fn physical_settings_return_restores_suspended_detail_and_mixer_surfaces() {
    for surface in [
        SurfaceId::PatchDetail,
        SurfaceId::MixerMain,
        SurfaceId::MixerInspector,
    ] {
        let mut state = state();
        if surface == SurfaceId::PatchDetail {
            page(&mut state, Direction::Up);
        } else {
            page(&mut state, Direction::Down);
            state.apply(AppEvent::Navigate(Direction::Right)).unwrap();
            state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
            if surface == SurfaceId::MixerInspector {
                state.apply(AppEvent::EnterSurface(surface)).unwrap();
            }
        }
        let origin = state.interaction().clone();
        state.apply(AppEvent::OpenMidiSettings).unwrap();
        let midi = state.midi_input().clone();
        let mut keyboard = KeyboardInputTranslator::new();
        keyboard.translate(WindowInput::key_down(WindowKey::Shift));
        let action = keyboard
            .translate(WindowInput::key_down(WindowKey::D))
            .unwrap();
        assert_eq!(action, SemanticAction::NavigatePage(Direction::Right));
        let outcome = state.apply_semantic_action(action).unwrap();
        assert!(outcome.midi_device_effects().is_empty());
        assert_eq!(state.interaction(), &origin);
        assert_eq!(state.midi_input(), &midi);
        assert_eq!(
            keyboard.translate(WindowInput::key_down(WindowKey::D)),
            None
        );
        keyboard.translate(WindowInput::key_up(WindowKey::D));
        keyboard.translate(WindowInput::key_up(WindowKey::Shift));
        StateProjector::new()
            .project_with_shell_tree(&state)
            .unwrap();
    }
}

#[test]
fn settings_right_return_uses_exposed_repair_after_suspended_origin_is_removed() {
    let mut state = state();
    let patch_id = state.patches()[0].id();
    page(&mut state, Direction::Left);
    state
        .apply(AppEvent::SetPatchOverviewOriginEnabled {
            patch_id,
            control: PatchControlId::Engine,
            enabled: false,
        })
        .unwrap();
    let repair = state.focus_repair_status().unwrap();
    let destination = repair.replacement_origin().clone();
    assert_eq!(
        destination.control_id(),
        &crest_synth::control::SemanticControlId::Patch(PatchControlId::EffectSlot(
            EffectSlotIndex::ALL[0]
        ))
    );
    assert!(StateProjector::new()
        .project_with_shell(&state)
        .unwrap()
        .3
        .semantic_model()
        .focus_repair()
        .is_some());
    page(&mut state, Direction::Right);
    assert_eq!(state.interaction().focus_path(), &destination);
}

#[test]
fn page_chords_cannot_discard_adjustment_or_choice() {
    let mut state = state();
    state
        .apply(AppEvent::SetInteractionMode(InteractionMode::Adjust))
        .unwrap();
    for direction in Direction::ALL {
        let before = state.clone();
        assert!(state.apply(AppEvent::NavigatePage(direction)).is_err());
        assert_eq!(state, before);
    }
    state.apply(AppEvent::Adjust(Direction::Up)).unwrap();
    assert_eq!(state.interaction().active_surface(), SurfaceId::PatchChoice);
    for direction in [Direction::Up, Direction::Left, Direction::Right] {
        let before = state.clone();
        assert!(state.apply(AppEvent::NavigatePage(direction)).is_err());
        assert_eq!(state, before);
    }
    page(&mut state, Direction::Down);
    assert_eq!(state.interaction().active_surface(), SurfaceId::PatchMain);
}

#[test]
fn q_e_follow_sparse_ids_and_trailing_empty_without_wrapping_or_creating() {
    let mut state = state();
    let saved = SavedSession::capture(&state);
    let mut keyboard = KeyboardInputTranslator::new();
    let before = state.clone();
    assert_eq!(
        key(&mut state, &mut keyboard, WindowKey::Q),
        Err(EventRejection::ParameterAtBoundary)
    );
    assert_eq!(state, before);
    state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
    let control = state.interaction().patch_control_focus();
    key(&mut state, &mut keyboard, WindowKey::E).unwrap();
    assert_eq!(
        state.interaction().patch_focus(),
        Some(PatchId::new(42).unwrap())
    );
    assert_eq!(state.interaction().patch_control_focus(), control);
    key(&mut state, &mut keyboard, WindowKey::E).unwrap();
    assert_eq!(
        state.interaction().patch_position_focus(),
        Some(PatchPositionId::TrailingEmpty)
    );
    let before = state.clone();
    assert_eq!(
        key(&mut state, &mut keyboard, WindowKey::E),
        Err(EventRejection::ParameterAtBoundary)
    );
    assert_eq!(state, before);
    key(&mut state, &mut keyboard, WindowKey::Q).unwrap();
    assert_eq!(
        state.interaction().patch_focus(),
        Some(PatchId::new(42).unwrap())
    );
    key(&mut state, &mut keyboard, WindowKey::Q).unwrap();
    assert_eq!(
        state.interaction().patch_focus(),
        Some(PatchId::new(7).unwrap())
    );
    assert_eq!(state.patches().len(), 2);
    assert_eq!(SavedSession::capture(&state), saved);
}

#[test]
fn projected_page_guidance_matches_admission_and_source() {
    let mut state = state();
    for (direction, label) in [
        (Direction::Up, "Open highlighted Detail"),
        (Direction::Down, "Open MIXER"),
        (Direction::Left, "Open Settings"),
    ] {
        let actions = SemanticResolver::new(&state).valid_actions();
        let action = actions
            .iter()
            .find(|a| a.action() == &SemanticAction::NavigatePage(direction))
            .unwrap();
        assert_eq!(action.label(), label);
        assert!(action.hint().unwrap().contains("Shift+"));
    }
    assert!(!SemanticResolver::new(&state)
        .valid_actions()
        .iter()
        .any(|a| a.action() == &SemanticAction::SelectPatch(Direction::Left)));
    page(&mut state, Direction::Left);
    let actions = SemanticResolver::new(&state).valid_actions();
    let returned = actions
        .iter()
        .find(|a| a.action() == &SemanticAction::NavigatePage(Direction::Right))
        .unwrap();
    assert_eq!(returned.label(), "Return to performance");
    assert_eq!(returned.hint(), Some("Shift+Right / Shift+D"));
    page(&mut state, Direction::Right);
    page(&mut state, Direction::Down);
    let actions = SemanticResolver::new(&state).valid_actions();
    assert_eq!(
        actions
            .iter()
            .find(|a| a.action() == &SemanticAction::NavigatePage(Direction::Up))
            .unwrap()
            .label(),
        "Return to Overview"
    );
    assert!(!actions
        .iter()
        .any(|a| a.action() == &SemanticAction::NavigatePage(Direction::Left)));
}

#[test]
fn patch_observation_requires_its_page_outside_system_settings() {
    use crest_synth::control::state_tree::{StateTree, StateTreeError};
    let mut state = state();
    let projector = StateProjector::new();
    let (snapshot, page, text, shell, parameters, _) =
        projector.project_with_shell_tree(&state).unwrap();
    assert!(page.is_some());
    assert_eq!(
        StateTree::with_patch_page_and_shell(&snapshot, None, &shell, &text, &parameters),
        Err(StateTreeError::PatchPageMismatch)
    );
    state
        .apply(AppEvent::NavigatePage(Direction::Left))
        .unwrap();
    let (snapshot, page, text, shell, parameters, _) =
        projector.project_with_shell_tree(&state).unwrap();
    assert!(page.is_none());
    StateTree::with_patch_page_and_shell(&snapshot, None, &shell, &text, &parameters).unwrap();
}

#[test]
fn navigation_journey_preserves_saved_content_parameters_graph_and_sustained_audio() {
    use crest_synth::adapter::production_effects::production_effect_preparers;
    use crest_synth::adapter::{
        braids_preparer::BraidsPreparer, lock_free_audio_boundary::LockFreeAudioBoundary,
        lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary,
    };
    use crest_synth::control::AppLoop;
    use crest_synth::kernel::midi_message::{MidiMessage, MidiMessageKind};
    use crest_synth::real_time::{
        AudioBoundary, AudioRenderer, GraphHandoffStatus, GraphRevision, PreparedGraphBuilder,
        StructuralGraphBoundary,
    };
    use crest_synth::synth::InstrumentPreparer;
    let state = state();
    let saved = SavedSession::capture(&state);
    let parameters = StateProjector::new()
        .project_with_shell_tree(&state)
        .unwrap()
        .4;
    let (control, audio) = LockFreeAudioBoundary::new(32, parameters).into_handles();
    let mut app = AppLoop::new(state, StateProjector::new(), control).unwrap();
    let preparers: Vec<Box<dyn InstrumentPreparer>> =
        vec![Box::new(BraidsPreparer::new().unwrap())];
    let effects = production_effect_preparers().unwrap();
    let graph = PreparedGraphBuilder::new(app.capabilities(), &preparers)
        .with_effects(app.effects(), &effects)
        .with_returns(app.bus_returns())
        .build(
            GraphRevision::INITIAL,
            app.patches(),
            app.current_parameters().clone(),
            48_000.0,
            256,
        )
        .unwrap();
    let (_structural_control, structural_audio) = LockFreeStructuralGraphBoundary::new(
        1,
        1,
        GraphHandoffStatus::with_active(GraphRevision::INITIAL),
    )
    .unwrap()
    .into_handles();
    let mut renderer = AudioRenderer::new(audio, structural_audio, graph);
    app.dispatch(AppEvent::Midi {
        patch_id: PatchId::new(7).unwrap(),
        message: MidiMessage::try_new(
            MidiChannel::new(0).unwrap(),
            MidiMessageKind::NoteOn,
            60,
            100,
        )
        .unwrap(),
    })
    .unwrap();
    let parameters = app.current_parameters().clone();
    let journey = [
        AppEvent::NavigatePage(Direction::Up),
        AppEvent::NavigatePage(Direction::Down),
        AppEvent::NavigatePage(Direction::Down),
        AppEvent::NavigatePage(Direction::Up),
        AppEvent::NavigatePage(Direction::Left),
        AppEvent::NavigatePage(Direction::Right),
        AppEvent::SelectPatch(Direction::Right),
        AppEvent::SelectPatch(Direction::Right),
        AppEvent::SelectPatch(Direction::Left),
        AppEvent::SelectPatch(Direction::Left),
    ];
    let mut samples = [0.0; 512];
    for event in journey {
        assert!(!event.publishes_parameters_on_acceptance());
        app.dispatch(event).unwrap();
        assert!(app.current_parameters().audio_values_equal(&parameters));
        assert_eq!(app.capture_saved_session(), saved);
        let log = app.event_log();
        assert!(!log
            .records()
            .back()
            .unwrap()
            .input()
            .publishes_parameters_on_acceptance());
        assert!(!log
            .records()
            .back()
            .unwrap()
            .emitted_events()
            .iter()
            .any(|event| matches!(
                event,
                crest_synth::control::event_record::EmittedEvent::ParameterSnapshotPublished { .. }
            )));
        if app
            .current_graphical_shell()
            .semantic_model()
            .active_surface()
            == SurfaceId::MidiDeviceSettings
        {
            // The generation-only performance path must also project while
            // Settings replaces the PATCH page.
            app.dispatch(AppEvent::Midi {
                patch_id: PatchId::new(7).unwrap(),
                message: MidiMessage::try_new(
                    MidiChannel::new(0).unwrap(),
                    MidiMessageKind::NoteOn,
                    60,
                    100,
                )
                .unwrap(),
            })
            .unwrap();
            assert_eq!(app.capture_saved_session(), saved);
        }
        renderer.render(&mut samples);
        assert_eq!(renderer.active_revision(), GraphRevision::INITIAL);
        assert!(samples.iter().all(|value| value.is_finite()));
        assert!(samples.iter().any(|value| value.abs() > 0.00001));
    }
}
