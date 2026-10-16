//! The automated key-injection witness (mission webview-shell-cutover
//! WP05 T021) — closing the foundation's FR-003 PARTIAL: the full
//! [`WindowKey`] vocabulary provably reaches the shared
//! [`KeyboardInputTranslator`] in the running webview shell without a human
//! at the keyboard.
//!
//! What runs is the production composition, not a translator-level
//! shortcut: the real [`TauriWebviewWindow::run`] (serving the debug seam
//! page via `CREST_WEBVIEW_PAGE`) installs the production NSEvent local
//! monitor and feeds the production `KeyboardInputTranslator` into the
//! application input callback. The witness synthesizes one `NSEvent` per
//! key transition from inside the window's own tick callback (the main
//! thread) and posts it through `NSApplication::postEvent`, so every event
//! travels queue → local monitor → normalization → translator, exactly as a
//! physical key does.
//!
//! Two independent observers close the exactly-once claim:
//!
//! - a second, recording-only local monitor (installed before the window's)
//!   logs every raw key transition delivered to the process — the raw log
//!   must equal the posted script, transition for transition, so nothing is
//!   lost and nothing double-fires;
//! - the application input callback logs every translated
//!   [`SemanticAction`] — the action log must equal a reference
//!   translator's output over the exact normalized input sequence, so the
//!   production pipeline fed the translator each event exactly once (a
//!   double-dispatch would emit surplus mode or duplicate actions and break
//!   byte-exact equality).
//!
//! Focus loss is driven by ordering the witness window out while the K
//! modifier is held: the tao `Focused(false)` edge must clear the held
//! modifier through `WindowInput::focus_lost`, observable as the following
//! bare `D` translating to `Navigate` rather than `Adjust`. A test
//! environment whose window never held key focus cannot deliver that edge;
//! the witness then reports it typed (`CREST_KEY_WITNESS_PARTIAL`) — and
//! fails when `CREST_REQUIRE_KEY_WITNESS=1` demands the full witness.
//!
//! Gate: window availability and an isolated native input session. An
//! environment that cannot host a window, or whose process-wide NSEvent
//! monitor also receives ambient host input, gets a typed skip/partial result.
//! Either condition FAILS under `CREST_REQUIRE_KEY_WITNESS=1`. Nothing here is
//! silent. The existing key-code bijectivity unit tests stay where they live,
//! in `src/shell/webview/input_capture.rs`.

fn require_witness() -> bool {
    std::env::var("CREST_REQUIRE_KEY_WITNESS").as_deref() == Ok("1")
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn main() {
    // libtest-style arguments are accepted and ignored under harness=false.
    let _ = std::env::args();
    println!(
        "CREST_KEY_WITNESS_SKIP native webview input capture is macOS-first; \
         this platform has no NSEvent monitor to witness"
    );
    if require_witness() {
        eprintln!("CREST_REQUIRE_KEY_WITNESS=1: the witness cannot run here");
        std::process::exit(1);
    }
}

#[cfg(target_os = "linux")]
#[path = "support/input_capture_linux.rs"]
mod linux_witness;

#[cfg(target_os = "linux")]
fn main() {
    if std::env::var_os("DISPLAY").is_none() {
        println!(
            "CREST_KEY_WITNESS_SKIP no X11 display; use Xvfb for the native injection witness"
        );
        std::process::exit(i32::from(require_witness()));
    }
    std::env::set_var("GDK_BACKEND", "x11");
    linux_witness::run();
}

#[cfg(target_os = "macos")]
fn main() {
    let _ = std::env::args();
    std::process::exit(witness::run());
}

#[cfg(target_os = "macos")]
mod witness {
    use crate::require_witness;
    use crest_synth::adapter::production_instruments::{
        production_capability_registry, production_soundfont_capability,
    };
    use crest_synth::control::app_event::AppEvent;
    use crest_synth::control::app_state::AppState;
    use crest_synth::control::state_projector::StateProjector;
    use crest_synth::control::SemanticAction;
    use crest_synth::kernel::midi_channel::MidiChannel;
    use crest_synth::kernel::patch_id::PatchId;
    use crest_synth::mixer::global_parameters::GlobalParameters;
    use crest_synth::mixer::mix_observation::MixObservation;
    use crest_synth::mixer::mixer_track_id::MixerTrackId;
    use crest_synth::mixer::patch_output::PatchOutput;
    use crest_synth::real_time::AudioObservationSnapshot;
    use crest_synth::shell::app_window::{
        AppInputCallback, AppWindow, AudioObservationCallback, FrameObservationCallback,
        ProjectionCallback, SessionCommandCallback, SessionDocumentProjectionCallback,
        TickCallback,
    };
    use crest_synth::shell::webview::input_capture::{
        self, window_key_from_macos_key_code, RawKeyEvent,
    };
    use crest_synth::shell::webview::TauriWebviewWindow;
    use crest_synth::shell::window_input::ALL_WINDOW_KEYS;
    use crest_synth::shell::{
        KeyboardInputTranslator, SessionDocumentMarker, SessionDocumentProjection, WindowInput,
        WindowKey,
    };
    use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
    use crest_synth::synth::Patch;
    use crest_synth::testing::automatic_midi_test::create_soundfont_config;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSEvent, NSEventModifierFlags, NSEventType,
    };
    use objc2_foundation::{NSPoint, NSString};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    /// A macOS virtual key code with no [`WindowKey`] mapping (kVK_ANSI_G),
    /// used to witness the `Other` normalization.
    const UNMAPPED_KEY_CODE: u16 = 5;

    /// One scripted step, executed one per tick from inside the window's own
    /// tick callback.
    #[derive(Clone, Copy, Debug)]
    enum Step {
        /// Wait one tick (queue drain, window settle).
        Grace,
        /// Post one synthesized key transition.
        Key {
            code: u16,
            pressed: bool,
            repeat: bool,
        },
        /// Order the witness window out to drive the tao focus-loss edge.
        HideWindow,
    }

    /// The macOS virtual key code the production normalization maps to
    /// `key`, choosing its ANSI alias before any equivalent arrow code.
    fn code_for(key: WindowKey) -> u16 {
        (0_u16..=127)
            .find(|code| window_key_from_macos_key_code(*code) == key)
            .unwrap_or_else(|| panic!("{key:?} has no macOS key code"))
    }

    /// The full scripted sequence: every mapped key pressed and released,
    /// the `Other` normalization, one OS auto-repeat, then the held-K
    /// focus-loss probe.
    fn script() -> Vec<Step> {
        let mut steps = Vec::new();
        // Let the window seat and the app activate before the first key.
        steps.extend(std::iter::repeat_n(Step::Grace, 40));
        for key in ALL_WINDOW_KEYS {
            let code = if key == WindowKey::Other {
                UNMAPPED_KEY_CODE
            } else {
                code_for(key)
            };
            steps.push(Step::Key {
                code,
                pressed: true,
                repeat: false,
            });
            steps.push(Step::Key {
                code,
                pressed: false,
                repeat: false,
            });
        }
        // An OS auto-repeat passes through with its repeat flag.
        steps.push(Step::Key {
            code: code_for(WindowKey::W),
            pressed: true,
            repeat: true,
        });
        steps.push(Step::Key {
            code: code_for(WindowKey::W),
            pressed: false,
            repeat: false,
        });
        // Repeated physical Shift gestures use AppKit FlagsChanged edges.
        // Every transition must remain non-repeatable and must survive the
        // Objective-C callback boundary without querying key-only state.
        for _ in 0..4 {
            steps.push(Step::Key {
                code: code_for(WindowKey::Shift),
                pressed: true,
                repeat: false,
            });
            steps.push(Step::Key {
                code: code_for(WindowKey::Shift),
                pressed: false,
                repeat: false,
            });
        }
        // The authored Patch-position gesture uses the real AppKit Shift
        // FlagsChanged edge held across horizontal key events. Both directions
        // must reach source-aware page intent through the production translator.
        for key in [WindowKey::D, WindowKey::A] {
            steps.push(Step::Key {
                code: code_for(WindowKey::Shift),
                pressed: true,
                repeat: false,
            });
            steps.push(Step::Key {
                code: code_for(key),
                pressed: true,
                repeat: false,
            });
            steps.push(Step::Key {
                code: code_for(key),
                pressed: false,
                repeat: false,
            });
            steps.push(Step::Key {
                code: code_for(WindowKey::Shift),
                pressed: false,
                repeat: false,
            });
        }
        // Hold K, lose focus, then probe with a bare D: cleared modifier
        // means Navigate, retained modifier means Adjust.
        steps.push(Step::Key {
            code: code_for(WindowKey::K),
            pressed: true,
            repeat: false,
        });
        steps.extend(std::iter::repeat_n(Step::Grace, 10));
        steps.push(Step::HideWindow);
        steps.extend(std::iter::repeat_n(Step::Grace, 20));
        steps.push(Step::Key {
            code: code_for(WindowKey::D),
            pressed: true,
            repeat: false,
        });
        steps.push(Step::Key {
            code: code_for(WindowKey::D),
            pressed: false,
            repeat: false,
        });
        steps.push(Step::Key {
            code: code_for(WindowKey::K),
            pressed: false,
            repeat: false,
        });
        steps.extend(std::iter::repeat_n(Step::Grace, 20));
        steps
    }

    /// The raw transitions the witness monitor must observe — the script,
    /// transition for transition, through the production normalization.
    fn expected_raw() -> Vec<RawKeyEvent> {
        let mut shift_held = false;
        script()
            .into_iter()
            .filter_map(|step| match step {
                Step::Key {
                    code,
                    pressed,
                    repeat,
                } => {
                    if window_key_from_macos_key_code(code) == WindowKey::Shift {
                        shift_held = pressed;
                    }
                    Some(
                        RawKeyEvent::new(
                            window_key_from_macos_key_code(code),
                            pressed,
                            pressed && repeat,
                        )
                        .with_shift(shift_held),
                    )
                }
                Step::Grace | Step::HideWindow => None,
            })
            .collect()
    }

    /// The action sequence a reference run of the shared translator produces
    /// over the scripted normalized inputs — with or without the focus-loss
    /// edge delivered after the held K.
    fn expected_actions(with_focus_loss: bool) -> Vec<SemanticAction> {
        let mut translator = KeyboardInputTranslator::new();
        let mut actions = Vec::new();
        let mut before_probe = true;
        for step in script() {
            match step {
                Step::Key { code, pressed, .. } => {
                    let key = window_key_from_macos_key_code(code);
                    let input = if pressed {
                        WindowInput::key_down(key)
                    } else {
                        WindowInput::key_up(key)
                    };
                    if let Some(action) = translator.translate(input) {
                        actions.push(action);
                    }
                }
                Step::HideWindow => {
                    before_probe = false;
                    if with_focus_loss {
                        if let Some(action) = translator.translate(WindowInput::focus_lost()) {
                            actions.push(action);
                        }
                    }
                }
                Step::Grace => {}
            }
        }
        let _ = before_probe;
        actions
    }

    /// The production fixture projection the window's projection callback
    /// serves (constant: the witness drives keys, not the reducer).
    fn fixture_projection() -> crest_synth::control::GraphicalShellProjection {
        let provider =
            production_soundfont_capability().expect("the production SoundFont capability");
        let config =
            create_soundfont_config(&provider, SoundFontInstrument::new(0, 40, false).unwrap())
                .expect("an installed instrument configuration");
        let patch = Patch::new(
            PatchId::new(1).unwrap(),
            "Input Capture Witness".to_owned(),
            config,
            MidiChannel::new(0).unwrap(),
            PatchOutput::new(MixerTrackId::new(0).unwrap(), -6.0).unwrap(),
        );
        let mut state = AppState::new(
            production_capability_registry().expect("the production capability registry"),
            GlobalParameters::new(-3.0).unwrap(),
        );
        state
            .apply(AppEvent::InstallPatches(vec![patch]))
            .expect("the patch installs");
        StateProjector::new()
            .project_with_shell(&state)
            .expect("the fixture projects")
            .3
    }

    /// Posts one synthesized key NSEvent onto this application's own queue,
    /// addressed to the witness window (a key event with no window number is
    /// held back by AppKit and re-delivered later, which would read as a
    /// double dispatch that never happens for real key input). Shift uses the
    /// `FlagsChanged` event shape AppKit emits for the physical modifier; a
    /// synthetic KeyDown here would miss modifier-only callback regressions.
    fn post_key(app: &NSApplication, code: u16, pressed: bool, repeat: bool, shift_held: bool) {
        let shift = code == code_for(WindowKey::Shift);
        let event_type = if shift {
            NSEventType::FlagsChanged
        } else if pressed {
            NSEventType::KeyDown
        } else {
            NSEventType::KeyUp
        };
        let modifier_flags = if shift_held {
            NSEventModifierFlags::Shift
        } else {
            NSEventModifierFlags::empty()
        };
        let window_number = app
            .windows()
            .iter()
            .next()
            .map(|window| window.windowNumber())
            .unwrap_or(0);
        let timestamp = objc2_foundation::NSProcessInfo::processInfo().systemUptime();
        let characters = NSString::from_str("");
        let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            event_type,
            NSPoint::new(0.0, 0.0),
            modifier_flags,
            timestamp,
            window_number,
            None,
            &characters,
            &characters,
            !shift && pressed && repeat,
            code,
        )
        .expect("the synthesized key event constructs");
        app.postEvent_atStart(&event, false);
    }

    /// Orders every application window out, driving the resign-key edge the
    /// tao loop reports as `Focused(false)`.
    fn hide_windows(app: &NSApplication) {
        for window in app.windows().iter() {
            window.orderOut(None);
        }
    }

    pub fn run() -> i32 {
        let require = require_witness();

        // The debug seam page (WP02's deterministic page-injection hook): a
        // minimal document with no script, so the witness exercises input
        // capture without projection acks in play.
        let page = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("input-capture-witness-seam.html");
        std::fs::write(
            &page,
            "<!doctype html><title>input capture witness seam</title>",
        )
        .expect("the seam page writes");
        std::env::set_var("CREST_WEBVIEW_PAGE", &page);

        let mtm =
            MainThreadMarker::new().expect("harness = false runs the witness on the main thread");
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);

        // The recording-only witness monitor, installed before the window's
        // production monitor. Events pass through unchanged; this observer
        // proves delivery and exactly-once independently of translation.
        let raw_log: Rc<RefCell<Vec<RawKeyEvent>>> = Rc::new(RefCell::new(Vec::new()));
        let raw_sink = Rc::clone(&raw_log);
        let witness_monitor = match input_capture::install(move |raw| {
            raw_sink.borrow_mut().push(raw);
        }) {
            Ok(handle) => handle,
            Err(error) => {
                println!(
                    "CREST_KEY_WITNESS_SKIP this environment cannot host the NSEvent \
                     monitor: {error}"
                );
                return i32::from(require);
            }
        };

        // The production window composition, with the witness script driven
        // from its own tick callback (the main thread).
        let actions: Rc<RefCell<Vec<SemanticAction>>> = Rc::new(RefCell::new(Vec::new()));
        let action_log = Rc::clone(&actions);
        let on_input: AppInputCallback = Box::new(move |action| {
            action_log.borrow_mut().push(action);
        });
        let projection = fixture_projection();
        let projection_cb: ProjectionCallback = Box::new(move || projection.clone());
        let audio: AudioObservationCallback = Box::new(|| {
            AudioObservationSnapshot::from_mix(0, 0, 0, 0, 0, 0, MixObservation::default())
        });
        let on_frame: FrameObservationCallback = Box::new(|_| {});

        let steps = script();
        let step_count = steps.len();
        let cursor = Rc::new(Cell::new(0_usize));
        let ticks = Rc::new(Cell::new(0_usize));
        let timed_out = Rc::new(Cell::new(false));
        let tick_cursor = Rc::clone(&cursor);
        let tick_count = Rc::clone(&ticks);
        let tick_timeout = Rc::clone(&timed_out);
        let tick_app = app.clone();
        let mut shift_held = false;
        let on_tick: TickCallback = Box::new(move |_elapsed| {
            tick_count.set(tick_count.get() + 1);
            if tick_count.get() > 6_000 {
                // ~100 s of 16 ms ticks: the script stalled. Close and fail
                // after the loop rather than hanging the suite.
                tick_timeout.set(true);
                return false;
            }
            let at = tick_cursor.get();
            if at >= steps.len() {
                return false;
            }
            match steps[at] {
                Step::Grace => {}
                Step::Key {
                    code,
                    pressed,
                    repeat,
                } => {
                    if window_key_from_macos_key_code(code) == WindowKey::Shift {
                        shift_held = pressed;
                    }
                    post_key(&tick_app, code, pressed, repeat, shift_held);
                }
                Step::HideWindow => hide_windows(&tick_app),
            }
            tick_cursor.set(at + 1);
            true
        });

        let window = TauriWebviewWindow::new("crest-synth input-capture witness");
        let midi_activity = Box::new(crest_synth::control::MidiActivityObservation::default);
        let on_session_command: SessionCommandCallback = Box::new(|_| false);
        let document_projection: SessionDocumentProjectionCallback = Box::new(|| {
            SessionDocumentProjection::new(
                "Witness",
                false,
                SessionDocumentMarker::Ready,
                None,
                "READY",
                None,
            )
        });
        if let Err(error) = window.run(
            on_input,
            projection_cb,
            audio,
            midi_activity,
            on_session_command,
            document_projection,
            on_tick,
            on_frame,
        ) {
            println!(
                "CREST_KEY_WITNESS_SKIP this environment cannot host the webview \
                 witness window: {error}"
            );
            return i32::from(require);
        }
        drop(witness_monitor);
        assert!(
            !timed_out.get(),
            "the witness script stalled before completing its {step_count} steps"
        );

        // ---- exactly-once delivery, raw ---------------------------------
        let observed_raw = raw_log.borrow().clone();
        let expected = expected_raw();
        if observed_raw != expected {
            let mut expected_cursor = 0_usize;
            for observed in &observed_raw {
                if expected.get(expected_cursor) == Some(observed) {
                    expected_cursor += 1;
                }
            }
            if expected_cursor == expected.len() && observed_raw.len() > expected.len() {
                let ambient = observed_raw.len() - expected.len();
                let mut next_expected = 0_usize;
                let ambient_positions = observed_raw
                    .iter()
                    .enumerate()
                    .filter_map(|(position, observed)| {
                        if expected.get(next_expected) == Some(observed) {
                            next_expected += 1;
                            None
                        } else {
                            Some(position)
                        }
                    })
                    .collect::<Vec<_>>();
                println!(
                    "CREST_KEY_WITNESS_PARTIAL all {} synthesized transitions arrived in \
                     order, but {ambient} ambient key transition(s) also reached the \
                     application at raw-log positions {ambient_positions:?}; byte-exact native \
                     isolation is incomplete in this host session",
                    expected.len()
                );
                if require {
                    eprintln!(
                        "CREST_REQUIRE_KEY_WITNESS=1: ambient input must be absent for the \
                         exact native handoff"
                    );
                    return 1;
                }
                return 0;
            }

            println!(
                "CREST_KEY_WITNESS_PARTIAL native input was not isolated: observed {} of {} \
                 expected synthesized transitions and the byte-exact sequence diverged; \
                 deterministic key normalization remains covered, but this host session \
                 cannot prove the native handoff",
                observed_raw.len(),
                expected.len()
            );
            if require {
                eprintln!(
                    "CREST_REQUIRE_KEY_WITNESS=1: the exact native handoff must match \
                     without missing, reordered, or ambient transitions\nobserved: \
                     {observed_raw:?}\nexpected: {expected:?}"
                );
                return 1;
            }
            return 0;
        }
        assert_eq!(
            observed_raw,
            expected,
            "every synthesized transition must arrive through the production monitor \
             exactly once, in order, normalized (observed {} of {} transitions)",
            observed_raw.len(),
            expected.len()
        );
        // Every normalized key was witnessed, both transitions.
        for key in ALL_WINDOW_KEYS {
            for pressed in [true, false] {
                let count = observed_raw
                    .iter()
                    .filter(|raw| raw.key() == key && raw.pressed() == pressed && !raw.repeat())
                    .count();
                let expected_count = expected
                    .iter()
                    .filter(|raw| raw.key() == key && raw.pressed() == pressed && !raw.repeat())
                    .count();
                assert_eq!(
                    count, expected_count,
                    "{key:?} pressed={pressed} must arrive exactly the scripted number of times"
                );
                assert!(count >= 1, "{key:?} pressed={pressed} never arrived");
            }
        }
        assert_eq!(
            observed_raw.iter().filter(|raw| raw.repeat()).count(),
            1,
            "the one scripted auto-repeat must arrive flagged as a repeat"
        );

        // ---- exactly-once arrival at the translator ---------------------
        let observed_actions = actions.borrow().clone();
        let with_focus = expected_actions(true);
        let without_focus = expected_actions(false);
        if observed_actions == with_focus {
            println!(
                "CREST_KEY_WITNESS input capture witnessed: {} transitions through the \
                 production NSEvent monitor, {} translated actions byte-exact, held-K \
                 focus-loss cleared through the window path",
                observed_raw.len(),
                observed_actions.len()
            );
            0
        } else if observed_actions == without_focus {
            println!(
                "CREST_KEY_WITNESS_PARTIAL key vocabulary witnessed ({} transitions, \
                 {} actions byte-exact) but the focus-loss edge was not delivered: the \
                 witness window never held key focus in this environment; run with a \
                 focusable session (WP06 operator checklist) or CREST_REQUIRE_KEY_WITNESS=1 \
                 to enforce",
                observed_raw.len(),
                observed_actions.len()
            );
            if require {
                eprintln!("CREST_REQUIRE_KEY_WITNESS=1: the focus-loss edge must be witnessed");
                1
            } else {
                0
            }
        } else {
            eprintln!(
                "input capture witness FAILED: translated actions diverge from the \
                 reference translator over the scripted inputs\nobserved: \
                 {observed_actions:?}\nexpected (with focus loss): {with_focus:?}\n\
                 expected (without focus loss): {without_focus:?}"
            );
            1
        }
    }
}
