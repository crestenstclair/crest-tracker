use super::*;

fn button(input: &mut ControllerInput, app: &mut App, role: ControllerRole, down: bool) {
    let bindings = ControllerBindings::default();
    let button = bindings.button(role);
    input.input(
        app,
        if down {
            HostControllerInput::Pressed { device: 1, button }
        } else {
            HostControllerInput::Released { device: 1, button }
        },
        &bindings,
    );
}

fn tap(input: &mut ControllerInput, app: &mut App, role: ControllerRole) {
    button(input, app, role, true);
    button(input, app, role, false);
}

fn reduce(app: &mut App) {
    for request in std::mem::take(&mut app.requests) {
        app.apply(request).unwrap();
    }
}

#[test]
fn held_shift_navigates_song_chain_phrase_and_back_without_repressing() {
    let (mut input, mut app) = (ControllerInput::default(), App::new());
    // Create an existing hierarchy before proving navigation never edits it.
    app.apply(Request::Confirm).unwrap();
    app.apply(Request::Confirm).unwrap();
    app.apply(Request::View { view: View::Song }).unwrap();
    let song = app.editor.song().clone();
    button(&mut input, &mut app, ControllerRole::Shift, true);
    for (role, view) in [
        (ControllerRole::Right, View::Chain),
        (ControllerRole::Right, View::Phrase),
        (ControllerRole::Left, View::Chain),
        (ControllerRole::Left, View::Song),
    ] {
        tap(&mut input, &mut app, role);
        reduce(&mut app);
        input.advance(&mut app, Duration::from_secs(1));
        assert_eq!(app.view, view);
        assert!(
            app.shift_held,
            "page changes must preserve a physically held Shift"
        );
    }
    button(&mut input, &mut app, ControllerRole::Shift, false);
    assert!(!app.shift_held);
    assert_eq!(app.editor.song(), &song);
}

#[test]
fn shift_back_opens_documented_main_menu_and_settings_opens_settings() {
    let (mut input, mut app) = (ControllerInput::default(), App::new());
    button(&mut input, &mut app, ControllerRole::Shift, true);
    tap(&mut input, &mut app, ControllerRole::PreviousPatch);
    assert_eq!(app.projection()["uiInputs"][0]["action"], "menu");
    assert!(app.requests.is_empty());
    button(&mut input, &mut app, ControllerRole::Shift, false);
    tap(&mut input, &mut app, ControllerRole::Settings);
    assert!(matches!(
        app.requests.last(),
        Some(Request::View {
            view: View::Settings
        })
    ));
}

#[test]
fn edit_stick_chord_never_turns_its_release_into_a_confirm() {
    let (mut input, mut app) = (ControllerInput::default(), App::new());
    button(&mut input, &mut app, ControllerRole::Edit, true);
    for value in [20_000, 0] {
        input.input(
            &mut app,
            HostControllerInput::Axis {
                device: 1,
                axis: HostControllerAxis::LeftX,
                value,
            },
            &ControllerBindings::default(),
        );
    }
    button(&mut input, &mut app, ControllerRole::Edit, false);
    assert!(!app
        .requests
        .iter()
        .any(|request| matches!(request, Request::Confirm)));
}

#[test]
fn note_edit_up_changes_pitch_and_horizontal_changes_exact_octave() {
    let (mut input, mut app) = (ControllerInput::default(), App::new());
    app.apply(Request::Confirm).unwrap();
    app.apply(Request::Confirm).unwrap();
    use crest_tracker::{composition::*, editing::Command, time::Beat};
    let phrase = app.selected_phrase().unwrap().0;
    app.command(Command::PutEvent {
        phrase,
        event: Event::note(
            EventId::new(app.editor.song().next_identity().unwrap()).unwrap(),
            Beat::ZERO,
            Target::Channel {
                channel: Channel::new(1).unwrap(),
                endpoint: None,
            },
            Note::new(60, 100, Beat::new(1, 4).unwrap()).unwrap(),
        ),
    })
    .unwrap();
    let pitch = 60;
    button(&mut input, &mut app, ControllerRole::Edit, true);
    tap(&mut input, &mut app, ControllerRole::Right);
    reduce(&mut app);
    assert_eq!(
        app.selected_phrase().unwrap().1.events[0]
            .note
            .as_ref()
            .unwrap()
            .pitch,
        pitch + 12
    );
    tap(&mut input, &mut app, ControllerRole::Up);
    reduce(&mut app);
    assert!(
        app.selected_phrase().unwrap().1.events[0]
            .note
            .as_ref()
            .unwrap()
            .pitch
            > pitch + 12
    );
    assert!(app.projection()["uiInputs"].as_array().unwrap().is_empty());
}

#[test]
fn shift_navigation_and_dialog_edit_have_distinct_lifetimes() {
    let (mut input, mut app) = (ControllerInput::default(), App::new());
    button(&mut input, &mut app, ControllerRole::Shift, true);
    tap(&mut input, &mut app, ControllerRole::Down);
    assert!(matches!(
        app.requests.pop(),
        Some(Request::SynthPage {
            page: crate::app::SynthPage::Mixer
        })
    ));
    app.view = View::Chain;
    tap(&mut input, &mut app, ControllerRole::Down);
    assert_eq!(app.projection()["uiInputs"][0]["action"], "quick_jump");
    input.cancel(&mut app);
    app.view = View::Phrase;
    app.column = 3;
    button(&mut input, &mut app, ControllerRole::Edit, true);
    tap(&mut input, &mut app, ControllerRole::Up);
    app.apply(Request::TextEntry { active: true }).unwrap();
    input.advance(&mut app, Duration::from_secs(1));
    tap(&mut input, &mut app, ControllerRole::Right);
    let inputs = app.projection()["uiInputs"].as_array().unwrap().clone();
    assert_eq!(inputs.last().unwrap()["action"], "edit_next");
    button(&mut input, &mut app, ControllerRole::Edit, false);
    tap(&mut input, &mut app, ControllerRole::Down);
    assert_eq!(
        app.projection()["uiInputs"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["action"],
        "next"
    );
}

#[test]
fn synth_keeps_ordinary_buttons_and_exposes_global_quick_jump() {
    let (mut input, mut app) = (ControllerInput::default(), App::new());
    app.view = View::Instruments;
    let bindings = ControllerBindings::default();
    let press = |role| HostControllerInput::Pressed {
        device: 1,
        button: bindings.button(role),
    };
    assert!(!input.synth_input(&mut app, press(ControllerRole::Edit), &bindings));
    assert!(!input.synth_input(&mut app, press(ControllerRole::Down), &bindings));
    assert!(!input.synth_input(&mut app, press(ControllerRole::Shift), &bindings));
    assert!(input.synth_input(&mut app, press(ControllerRole::Down), &bindings));
    assert_eq!(app.projection()["uiInputs"][0]["action"], "quick_jump");
    input.cancel(&mut app);
    assert!(!input.synth_input(&mut app, press(ControllerRole::Down), &bindings));
}

#[test]
fn keyboard_shift_back_and_settings_use_the_same_navigation() {
    use crest_synth::shell::{WindowInput, WindowKey};
    let mut app = App::new();
    super::super::keyboard(&mut app, WindowInput::key_down(WindowKey::Shift));
    super::super::keyboard(&mut app, WindowInput::key_down(WindowKey::Q));
    assert_eq!(app.projection()["uiInputs"][0]["action"], "menu");
    super::super::keyboard(&mut app, WindowInput::key_up(WindowKey::Shift));
    app.view = View::Settings;
    super::super::keyboard(&mut app, WindowInput::key_down(WindowKey::Return));
    assert_eq!(app.projection()["uiInputs"][1]["action"], "confirm");
}
