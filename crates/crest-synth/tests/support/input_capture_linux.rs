//! XTest events enter the production GTK capture, translator and window loop.
use crest_synth::adapter::production_instruments::{
    production_capability_registry, production_soundfont_capability,
};
use crest_synth::control::{AppEvent, AppState, SemanticAction, SendAction, StateProjector};
use crest_synth::kernel::{MidiChannel, PatchId};
use crest_synth::mixer::{
    global_parameters::GlobalParameters, mix_observation::MixObservation,
    mixer_track_id::MixerTrackId, patch_output::PatchOutput,
};
use crest_synth::real_time::AudioObservationSnapshot;
use crest_synth::shell::app_window::AppWindow;
use crest_synth::shell::webview::TauriWebviewWindow;
use crest_synth::shell::{
    KeyboardInputTranslator, SessionDocumentMarker, SessionDocumentProjection, WindowInput,
    WindowKey,
};
use crest_synth::synth::{sound_font_instrument::SoundFontInstrument, Patch};
use crest_synth::testing::automatic_midi_test::create_soundfont_config;
use std::cell::{Cell, RefCell};
use std::process::Command;
use std::rc::Rc;
use std::time::{Duration, Instant};

fn inject(args: &[&str]) {
    let result = Command::new("xdotool")
        .args(args)
        .output()
        .expect("install xdotool for the Linux native witness");
    assert!(
        result.status.success(),
        "native injection {args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

pub fn run() {
    use WindowKey::*;
    // XTest drops duplicate keydowns. Exercise real X server autorepeat, with
    // one repeat after 150 ms and a 500 ms interval before a second can occur.
    assert!(Command::new("xset")
        .args(["r", "rate", "150", "2"])
        .status()
        .unwrap()
        .success());
    let provider = production_soundfont_capability().unwrap();
    let config =
        create_soundfont_config(&provider, SoundFontInstrument::new(0, 40, false).unwrap())
            .unwrap();
    let patch = Patch::new(
        PatchId::new(1).unwrap(),
        "Linux Input Witness".to_owned(),
        config,
        MidiChannel::new(0).unwrap(),
        PatchOutput::new(MixerTrackId::new(0).unwrap(), -6.0).unwrap(),
    );
    let mut state = AppState::new(
        production_capability_registry().unwrap(),
        GlobalParameters::new(-3.0).unwrap(),
    );
    state.apply(AppEvent::InstallPatches(vec![patch])).unwrap();
    let projection = Rc::new(RefCell::new(
        StateProjector::new().project_with_shell(&state).unwrap().3,
    ));
    state.apply(AppEvent::Send(SendAction::Open)).unwrap();
    state.apply(AppEvent::Activate).unwrap();
    assert!(state.interaction().send_name_editing());
    let rename_projection = StateProjector::new().project_with_shell(&state).unwrap().3;
    let initial_generation = projection.borrow().generation();
    let rename_generation = rename_projection.generation();
    let painted_generation = Rc::new(Cell::new(None));
    let frame_generation = Rc::clone(&painted_generation);
    let renamed_state = Rc::new(RefCell::new(state));
    let input_state = Rc::clone(&renamed_state);
    let tick_projection = Rc::clone(&projection);
    // Exercise the shipping send-name form and its native input/IPC path.
    std::env::remove_var("CREST_WEBVIEW_PAGE");

    let mut steps: Vec<(Vec<&str>, Vec<WindowInput>)> = Vec::new();
    for (name, key) in [
        ("1", Digit1),
        ("2", Digit2),
        ("3", Digit3),
        ("4", Digit4),
        ("5", Digit5),
        ("6", Digit6),
        ("7", Digit7),
        ("8", Digit8),
        ("9", Digit9),
        ("0", Digit0),
        ("bracketleft", BracketLeft),
        ("bracketright", BracketRight),
        ("q", Q),
        ("e", E),
        ("w", W),
        ("s", S),
        ("a", A),
        ("d", D),
        ("k", K),
        ("Shift_L", Shift),
        ("Return", Return),
        ("space", Space),
        ("t", T),
        ("g", Other),
        ("Up", W),
        ("Down", S),
        ("Left", A),
        ("Right", D),
    ] {
        steps.push((
            vec!["keydown", name, "keyup", name],
            vec![WindowInput::key_down(key), WindowInput::key_up(key)],
        ));
    }
    steps.push((
        vec!["keydown", "w", "sleep", "0.3", "keyup", "w"],
        vec![
            WindowInput::key_down(W),
            WindowInput::key_down(W),
            WindowInput::key_up(W),
        ],
    ));
    steps.push((
        vec![
            "keydown", "Shift_L", "keydown", "Right", "keyup", "Shift_L", "sleep", "0.3", "keyup",
            "Right",
        ],
        vec![
            WindowInput::key_down(Shift),
            WindowInput::key_down(D),
            WindowInput::key_up(Shift),
            WindowInput::key_down(D),
            WindowInput::key_up(D),
        ],
    ));
    steps.push((
        vec!["keydown", "q", "sleep", "0.3", "keyup", "q"],
        vec![
            WindowInput::key_down(Q),
            WindowInput::key_down(Q),
            WindowInput::key_up(Q),
        ],
    ));
    // Ctrl+A is outside the native File menu: neither a synth action nor a
    // browser-selected replacement for semantic focus may be manufactured.
    steps.push((vec!["key", "ctrl+a"], vec![]));
    steps.push((
        vec!["key", "ctrl+4"],
        vec![WindowInput::key_down(Digit4), WindowInput::key_up(Digit4)],
    ));
    steps.push((
        vec!["keydown", "k", "keydown", "space"],
        vec![WindowInput::key_down(K), WindowInput::key_down(Space)],
    ));

    let actions = Rc::new(RefCell::new(Vec::new()));
    let recording = Rc::new(Cell::new(false));
    let recorded_actions = Rc::clone(&actions);
    let record_input = Rc::clone(&recording);
    let expected = Rc::new(RefCell::new(Vec::new()));
    let tick_expected = Rc::clone(&expected);
    let tick_actions = Rc::clone(&actions);
    let tick_recording = Rc::clone(&recording);
    let started = Instant::now();
    let mut ticks = 0;
    let mut cursor = 0;
    let mut reference = KeyboardInputTranslator::new();
    let mut native_id = String::new();
    let script_len = steps.len();
    let window = TauriWebviewWindow::new("crest-synth Linux input witness");
    window
        .run(
            Box::new(move |action| {
                if record_input.get() {
                    if matches!(action, SemanticAction::Send(SendAction::Rename { .. })) {
                        input_state
                            .borrow_mut()
                            .apply_semantic_action(action.clone())
                            .unwrap();
                    }
                    recorded_actions.borrow_mut().push(action);
                }
            }),
            Box::new(move || projection.borrow().clone()),
            Box::new(|| {
                AudioObservationSnapshot::from_mix(0, 0, 0, 0, 0, 0, MixObservation::default())
            }),
            Box::new(crest_synth::control::MidiActivityObservation::default),
            Box::new(|_| false),
            Box::new(|| {
                SessionDocumentProjection::new(
                    "Witness",
                    false,
                    SessionDocumentMarker::Ready,
                    None,
                    "READY",
                    None,
                )
            }),
            Box::new(move |_| {
                assert!(
                    started.elapsed() < Duration::from_secs(30),
                    "Linux input witness timed out"
                );
                ticks += 1;
                if ticks < 40 || ticks % 6 != 0 {
                    return true;
                }
                if native_id.is_empty() {
                    if painted_generation.get() != Some(initial_generation) {
                        return true;
                    }
                    let output = Command::new("xdotool")
                        .args([
                            "search",
                            "--onlyvisible",
                            "--pid",
                            &std::process::id().to_string(),
                        ])
                        .output()
                        .unwrap();
                    assert!(output.status.success(), "owned native window exists");
                    native_id = String::from_utf8(output.stdout)
                        .unwrap()
                        .lines()
                        .next()
                        .unwrap()
                        .to_owned();
                    inject(&["windowactivate", "--sync", &native_id]);
                    tick_actions.borrow_mut().clear();
                    tick_recording.set(true);
                    return true;
                }
                if cursor < steps.len() {
                    let (args, inputs) = &steps[cursor];
                    for input in inputs {
                        if let Some(action) = reference.translate(*input) {
                            tick_expected.borrow_mut().push(action);
                        }
                    }
                    inject(args);
                } else if cursor == steps.len() {
                    inject(&["windowminimize", &native_id]);
                    tick_expected
                        .borrow_mut()
                        .push(reference.translate(WindowInput::focus_lost()).unwrap());
                    // The production KeyPipeline also releases Edit when focus
                    // loss's immediate action was owned preview cancellation.
                    tick_expected
                        .borrow_mut()
                        .push(SemanticAction::SetInteractionMode(
                            crest_synth::control::InteractionMode::Navigate,
                        ));
                } else if cursor == steps.len() + 1 {
                    inject(&[
                        "keyup",
                        "k",
                        "keyup",
                        "space",
                        "windowmap",
                        &native_id,
                        "windowactivate",
                        "--sync",
                        &native_id,
                    ]);
                } else if cursor == steps.len() + 2 {
                    inject(&["key", "d"]);
                    tick_expected
                        .borrow_mut()
                        .push(reference.translate(WindowInput::key_down(D)).unwrap());
                    reference.translate(WindowInput::key_up(D));
                } else if cursor == steps.len() + 3 {
                    *tick_projection.borrow_mut() = rename_projection.clone();
                } else if cursor == steps.len() + 4 {
                    if painted_generation.get() != Some(rename_generation) {
                        return true;
                    }
                    // These synth vocabulary keys must reach the focused name
                    // field without producing navigation, performance or Edit.
                    inject(&["key", "ctrl+a", "BackSpace"]);
                    inject(&["type", "--clearmodifiers", "wasdqekt401"]);
                    inject(&["key", "Return"]);
                    tick_expected
                        .borrow_mut()
                        .push(SemanticAction::Send(SendAction::Rename {
                            bus: crest_synth::mixer::bus_id::BusId::default(),
                            name: "wasdqekt401".to_owned(),
                        }));
                } else {
                    if !tick_actions.borrow().iter().any(|action| {
                        matches!(action, SemanticAction::Send(SendAction::Rename { .. }))
                    }) {
                        return true;
                    }
                    tick_recording.set(false);
                    return false;
                }
                cursor += 1;
                true
            }),
            Box::new(move |frame| frame_generation.set(Some(frame.generation()))),
        )
        .expect("production Linux window and input capture must run");
    assert_eq!(
        *actions.borrow(),
        *expected.borrow(),
        "native keys arrive exactly once; text entry, repeats, shortcuts and focus loss preserve semantics"
    );
    assert_eq!(
        renamed_state
            .borrow()
            .bus_returns()
            .bus_return(crest_synth::mixer::bus_id::BusId::default())
            .name(),
        "wasdqekt401",
        "native text input commits the exact send name through the reducer"
    );
    assert!(expected.borrow().len() > 20);
    println!("CREST_KEY_WITNESS_PASS Linux: {script_len} native key sequences, send-name text entry, focus loss, preview/Edit release, shortcuts and owned shutdown");
}
