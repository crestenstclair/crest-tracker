//! Requirement-linked Phase 7 acceptance tests through production reducers.

#[allow(dead_code)]
mod support;

use crest_synth::adapter::production_effects::production_effect_registry;
use crest_synth::adapter::production_instruments::{
    production_capability_registry, production_soundfont_capability,
};
use crest_synth::control::{
    AppEvent, AppState, Direction, InteractionMode, ModalControlId, PatchControlId,
    PatchSubordinateSession, SemanticAction, SemanticControlId, SemanticResolver, SurfaceId,
    TopLevelContext,
};
use crest_synth::kernel::{MidiChannel, PatchId};
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::patch_output::{PatchOutput, PatchOutputParameter};
use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
use crest_synth::synth::Patch;
use crest_synth::testing::automatic_midi_test::create_soundfont_config;

fn installed_state() -> AppState {
    let provider = production_soundfont_capability().unwrap();
    let patch = Patch::new(
        PatchId::new(1).unwrap(),
        "Phase 7".to_owned(),
        create_soundfont_config(&provider, SoundFontInstrument::new(0, 0, false).unwrap()).unwrap(),
        MidiChannel::new(0).unwrap(),
        PatchOutput::to_track(MixerTrackId::new(3).unwrap()),
    );
    let mut state = AppState::new_with_effects(
        production_capability_registry().unwrap(),
        production_effect_registry().unwrap(),
        support::globals(),
    );
    state.apply(AppEvent::InstallPatches(vec![patch])).unwrap();
    state
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    state
}

fn enter_adjust(state: &mut AppState) {
    state
        .apply_semantic_action(SemanticAction::SetInteractionMode(InteractionMode::Adjust))
        .unwrap();
}

/// `Choice modals use installed and descriptor-owned choices` —
/// `Open engine choices`; and `Choice focus is trapped and returns exactly` —
/// `Choose an option`.
#[test]
fn phase7_open_engine_choices_uses_registry_order_and_choose_returns_exactly() {
    let mut state = installed_state();
    let origin = state.interaction().focus_path().clone();
    assert_eq!(
        origin.control_id(),
        &SemanticControlId::Patch(PatchControlId::Engine)
    );

    enter_adjust(&mut state);
    state
        .apply_semantic_action(SemanticAction::Adjust(Direction::Up))
        .unwrap();
    assert_eq!(state.interaction().active_surface(), SurfaceId::PatchChoice);
    assert_eq!(state.interaction().mode(), InteractionMode::Modal);
    assert!(matches!(
        state.interaction().subordinate_session(),
        Some(PatchSubordinateSession::Choice { .. })
    ));

    let subject = match state.interaction().subordinate_session().unwrap() {
        PatchSubordinateSession::Choice { subject, .. } => subject,
        _ => unreachable!(),
    };
    let source = SemanticResolver::new(&state)
        .choice_source(subject)
        .unwrap();
    let installed = state.capabilities().descriptors();
    assert_eq!(source.options().len(), installed.len());
    assert!(source
        .options()
        .iter()
        .zip(installed)
        .all(|(option, descriptor)| option.id() == descriptor.id().to_string()));
    assert!(source.options()[0].is_current());

    state
        .apply_semantic_action(SemanticAction::Navigate(Direction::Down))
        .unwrap();
    let focused_option = match state.interaction().focus_path().control_id() {
        SemanticControlId::Modal(ModalControlId::Choice(id)) => id.clone(),
        other => panic!("expected choice focus, got {other:?}"),
    };
    let outcome = state
        .apply_semantic_action(SemanticAction::Activate)
        .unwrap();
    assert!(outcome.engine_selection_effect().is_some());
    assert_eq!(state.interaction().focus_path(), &origin);
    assert_eq!(state.interaction().active_surface(), SurfaceId::PatchMain);
    assert_eq!(
        state
            .engine_selection()
            .correlation()
            .unwrap()
            .target_capability_id()
            .unwrap()
            .to_string(),
        focused_option
    );
}

/// `Choice modals use installed and descriptor-owned choices` —
/// `Open route choices`; and `Choice focus is trapped and returns exactly` —
/// `Cancel an option` / `Attempt to leave the modal spatially`.
#[test]
fn phase7_route_choice_has_all_tracks_and_cancel_is_exact_and_unchanged() {
    let mut state = installed_state();
    state
        .apply_semantic_action(SemanticAction::Navigate(Direction::Right))
        .unwrap();
    for _ in 0..2 {
        state
            .apply_semantic_action(SemanticAction::Navigate(Direction::Down))
            .unwrap();
    }
    assert_eq!(
        state.interaction().patch_control_focus(),
        Some(PatchControlId::Output(PatchOutputParameter::OutputTrack))
    );
    let origin = state.interaction().focus_path().clone();
    let before_route = state.patches()[0].output();
    enter_adjust(&mut state);
    state
        .apply_semantic_action(SemanticAction::Adjust(Direction::Up))
        .unwrap();

    let subject = match state.interaction().subordinate_session().unwrap() {
        PatchSubordinateSession::Choice { subject, .. } => subject,
        _ => unreachable!(),
    };
    let source = SemanticResolver::new(&state)
        .choice_source(subject)
        .unwrap();
    assert_eq!(source.options().len(), MixerTrackId::COUNT);
    assert_eq!(source.options().first().unwrap().id(), "T00");
    assert_eq!(source.options().last().unwrap().id(), "T0F");

    let trapped = state.clone();
    assert!(state
        .apply_semantic_action(SemanticAction::Navigate(Direction::Right))
        .is_err());
    assert_eq!(state, trapped);
    state.apply_semantic_action(SemanticAction::Return).unwrap();
    assert_eq!(state.interaction().focus_path(), &origin);
    assert_eq!(state.patches()[0].output(), before_route);
}
