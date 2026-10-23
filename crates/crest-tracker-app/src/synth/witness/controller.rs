//! Physical SDL edges through the composed host, editor, transport and DSP.
use super::*;
use crest_synth::control::SurfaceId;
use sdl3::gamepad::{Axis, Button};
use sdl3::joystick::{JoystickType, VirtualJoystickDescription};

#[test]
#[ignore = "uses system SDL and saved default controller layout; run alone with --test-threads=1"]
fn face_buttons_create_navigate_edit_and_play_through_the_production_host() {
    let root = scenarios::temp_root("controller-host");
    let sdl = sdl3::init().unwrap();
    let joysticks = sdl.joystick().unwrap();
    let connection = joysticks
        .attach_virtual_joystick(
            VirtualJoystickDescription::new()
                .name("Crest Tracker controller witness")
                .joystick_type(JoystickType::Gamepad)
                .with_buttons([
                    Button::South,
                    Button::East,
                    Button::West,
                    Button::North,
                    Button::Back,
                    Button::Guide,
                    Button::Start,
                    Button::LeftStick,
                    Button::RightStick,
                    Button::LeftShoulder,
                    Button::RightShoulder,
                    Button::DPadUp,
                    Button::DPadDown,
                    Button::DPadLeft,
                    Button::DPadRight,
                ])
                .with_axes([
                    Axis::LeftX,
                    Axis::LeftY,
                    Axis::RightX,
                    Axis::RightY,
                    Axis::TriggerLeft,
                    Axis::TriggerRight,
                ]),
        )
        .unwrap();
    let pad = joysticks.open(connection.id()).unwrap();
    type Check = fn(&Runtime);
    let mut steps: Vec<(Button, bool, Check)> = vec![
        (Button::West, true, |r| {
            assert!(crate::workspace::project(&r.app.borrow())
                .workspace
                .unwrap()
                .rows
                .iter()
                .any(|row| row.id == "slot_rest"));
        }),
        (Button::West, false, |_| {}),
        (Button::West, true, |r| {
            assert!(
                !crate::workspace::project(&r.app.borrow())
                    .workspace
                    .unwrap()
                    .overlay
            );
        }),
        (Button::West, false, |_| {}),
        (Button::South, true, |r| assert!(r.app.borrow().edit_held)),
        (Button::DPadRight, true, |r| {
            let app = r.app.borrow();
            assert_eq!(app.view, View::Song);
            assert!(app.selected_chain().is_some(), "{}", app.status);
        }),
        (Button::DPadRight, false, |_| {}),
        (Button::South, false, |r| assert!(!r.app.borrow().edit_held)),
        (Button::East, true, |_| {}),
        (Button::DPadRight, true, |r| {
            assert_eq!(r.app.borrow().view, View::Chain)
        }),
        (Button::DPadRight, false, |_| {}),
        (Button::East, false, |_| {}),
        (Button::South, true, |_| {}),
        (Button::DPadUp, true, |r| {
            let app = r.app.borrow();
            assert_eq!(app.view, View::Chain);
            assert!(app.selected_phrase().is_some(), "{}", app.status);
        }),
        (Button::DPadUp, false, |_| {}),
        (Button::South, false, |_| {}),
        (Button::East, true, |_| {}),
        (Button::DPadRight, true, |r| {
            assert_eq!(r.app.borrow().view, View::Phrase)
        }),
        (Button::DPadRight, false, |_| {}),
        (Button::East, false, |_| {}),
        (Button::South, true, |_| {}),
        (Button::DPadUp, true, |r| {
            assert_eq!(r.app.borrow().selected_phrase().unwrap().1.events.len(), 1)
        }),
        (Button::DPadUp, false, |_| {}),
        (Button::DPadUp, true, |r| {
            assert!(
                r.app.borrow().selected_phrase().unwrap().1.events[0]
                    .note
                    .as_ref()
                    .unwrap()
                    .pitch
                    > 60
            )
        }),
        (Button::DPadUp, false, |_| {}),
        (Button::South, false, |r| {
            let app = r.app.borrow();
            assert!(!app.edit_held);
            assert_eq!(app.selected_phrase().unwrap().1.events.len(), 1);
        }),
        (Button::DPadDown, true, |r| {
            assert!(r.app.borrow().row > 1, "held direction must repeat")
        }),
        (Button::DPadDown, false, |_| {}),
        (Button::North, true, |r| {
            assert!(r.app.borrow().playing, "{}", r.app.borrow().status)
        }),
        (Button::North, false, |r| assert!(r.app.borrow().playing)),
        (Button::North, true, |r| assert!(!r.app.borrow().playing)),
        (Button::North, false, |_| {}),
        (Button::Start, true, |r| {
            assert!(r.app.borrow().playing, "Start must play, not open Settings")
        }),
        (Button::Start, false, |r| assert!(r.app.borrow().playing)),
        (Button::Start, true, |r| assert!(!r.app.borrow().playing)),
        (Button::Start, false, |_| {}),
        (Button::East, true, |_| {}),
        (Button::DPadLeft, true, |r| {
            assert_eq!(r.app.borrow().view, View::Chain)
        }),
        (Button::DPadLeft, false, |_| {}),
        (Button::DPadLeft, true, |r| {
            assert_eq!(r.app.borrow().view, View::Song)
        }),
        (Button::DPadLeft, false, |_| {}),
        (Button::DPadDown, true, |r| {
            assert_eq!(r.app.borrow().view, View::Song);
            assert_eq!(
                crate::workspace::project(&r.app.borrow())
                    .workspace
                    .unwrap()
                    .title,
                "MENU"
            );
        }),
        (Button::DPadDown, false, |_| {}),
        (Button::DPadUp, true, |r| {
            assert_eq!(r.app.borrow().view, View::Song)
        }),
        (Button::DPadUp, false, |_| {}),
        (Button::East, false, |_| {}),
    ];
    // Keep Shift held across native detail, host menu, and exact native return.
    steps.extend([
        (Button::East, true, (|_| {}) as Check),
        (Button::DPadDown, true, |_| {}),
        (Button::DPadDown, false, |_| {}),
        (Button::East, false, |_| {}),
        (Button::DPadDown, true, |_| {}),
        (Button::DPadDown, false, |_| {}),
        (Button::South, true, |_| {}),
        (Button::South, false, |r| {
            assert_eq!(
                r.app.borrow().workspace.synth_surface,
                Some(SurfaceId::PatchMain)
            )
        }),
        (Button::East, true, |_| {}),
        (Button::DPadUp, true, |r| {
            assert_eq!(
                r.app.borrow().workspace.synth_surface,
                Some(SurfaceId::PatchDetail)
            )
        }),
        (Button::DPadUp, false, |_| {}),
        (Button::DPadDown, true, |r| {
            assert_eq!(
                r.app.borrow().workspace.synth_surface,
                Some(SurfaceId::PatchMain)
            )
        }),
        (Button::DPadDown, false, |_| {}),
        (Button::DPadDown, true, |r| {
            assert_eq!(
                crate::workspace::project(&r.app.borrow())
                    .workspace
                    .unwrap()
                    .title,
                "MENU"
            )
        }),
        (Button::DPadDown, false, |_| {}),
        (Button::DPadUp, true, |r| {
            assert!(crate::workspace::project(&r.app.borrow()).synth_visible())
        }),
        (Button::DPadUp, false, |_| {}),
        (Button::DPadDown, true, |_| {}),
        (Button::DPadDown, false, |_| {}),
        (Button::East, false, |_| {}),
        (Button::DPadDown, true, |_| {}),
        (Button::DPadDown, false, |_| {}),
        (Button::DPadDown, true, |_| {}),
        (Button::DPadDown, false, |_| {}),
        (Button::DPadDown, true, |_| {}),
        (Button::DPadDown, false, |_| {}),
        (Button::South, true, |_| {}),
        (Button::South, false, |r| {
            assert_eq!(
                r.app.borrow().workspace.synth_surface,
                Some(SurfaceId::MixerMain)
            )
        }),
        (Button::East, true, |_| {}),
        (Button::DPadUp, true, |r| {
            assert_eq!(
                r.app.borrow().workspace.synth_surface,
                Some(SurfaceId::MixerInspector)
            )
        }),
        (Button::DPadUp, false, |_| {}),
        (Button::DPadUp, true, |r| {
            assert_eq!(
                r.app.borrow().workspace.synth_surface,
                Some(SurfaceId::MixerInspector)
            )
        }),
        (Button::DPadUp, false, |_| {}),
        (Button::DPadDown, true, |r| {
            assert_eq!(
                r.app.borrow().workspace.synth_surface,
                Some(SurfaceId::MixerMain)
            )
        }),
        (Button::DPadDown, false, |_| {}),
        (Button::DPadDown, true, |r| {
            assert_eq!(
                crate::workspace::project(&r.app.borrow())
                    .workspace
                    .unwrap()
                    .title,
                "MENU"
            )
        }),
        (Button::DPadDown, false, |_| {}),
        (Button::DPadUp, true, |r| {
            assert!(crate::workspace::project(&r.app.borrow()).synth_visible());
            assert_eq!(
                r.app.borrow().workspace.synth_surface,
                Some(SurfaceId::MixerMain)
            );
        }),
        (Button::DPadUp, false, |_| {}),
        (Button::East, false, |_| {}),
    ]);
    // Physical Start and the mapped Preview role remain transport in native Synth.
    steps.extend([
        (
            Button::Start,
            true,
            (|r: &Runtime| {
                assert!(r.app.borrow().playing);
                assert!(crate::workspace::project(&r.app.borrow()).synth_visible());
            }) as Check,
        ),
        (Button::Start, false, |r| assert!(r.app.borrow().playing)),
        (Button::East, true, |_| {}),
        (Button::Start, true, |r| {
            assert!(!r.app.borrow().playing);
            assert!(crate::workspace::project(&r.app.borrow()).synth_visible());
        }),
        (Button::Start, false, |_| {}),
        (Button::North, true, |r| assert!(r.app.borrow().playing)),
        (Button::North, false, |r| assert!(r.app.borrow().playing)),
        (Button::North, true, |r| assert!(!r.app.borrow().playing)),
        (Button::North, false, |_| {}),
        (Button::East, false, |_| {}),
    ]);
    let mut queued_navigation = false;
    let mut index = 0;
    let mut issued = false;
    let mut ticks = 0;
    let started = Instant::now();
    let evidence = run_with_devices(
        &root,
        Box::new(move |runtime, audio, evidence| {
            // Let asynchronous device discovery/preferences and the initial Synth
            // graph finish before producing a physical press.
            if started.elapsed() < Duration::from_millis(250)
                || runtime.initial_sound.is_none()
                || !runtime.sync.ready()
            {
                return false;
            }
            if index == steps.len() {
                if !queued_navigation {
                    queued_navigation = true;
                    for direction in [
                        crest_synth::control::Direction::Up,
                        crest_synth::control::Direction::Down,
                        crest_synth::control::Direction::Down,
                        crest_synth::control::Direction::Up,
                    ] {
                        crate::workspace::action(
                            &mut runtime.app.borrow_mut(),
                            SemanticAction::NavigatePage(direction),
                        );
                    }
                    return false;
                }
                assert!(crate::workspace::project(&runtime.app.borrow()).synth_visible());
                assert_eq!(
                    runtime.app.borrow().workspace.synth_surface,
                    Some(SurfaceId::MixerMain)
                );
                assert!(
                    evidence.peak_voices > 0,
                    "controller-created note never rendered"
                );
                assert!(
                    evidence.audible_tracks[0],
                    "controller-created note produced no audio"
                );
                return audio.active_notes() == 0;
            }
            evidence.measured = true;
            let (button, down, check) = steps[index];
            if !issued {
                pad.set_virtual_button(button as u32, down).unwrap();
                joysticks.update();
                // The production SDL joystick thread observes physical state
                // asynchronously; this witness otherwise advances virtual audio
                // and UI ticks much faster than wall time.
                std::thread::sleep(Duration::from_millis(10));
                issued = true;
                ticks = 0;
            } else {
                ticks += 1;
                // Include scheduling lead time while preview/transport is held.
                let dwell = if button == Button::North {
                    80
                } else if button == Button::DPadDown
                    && down
                    && runtime.app.borrow().view == View::Phrase
                {
                    100
                } else {
                    4
                };
                if ticks >= dwell {
                    check(runtime);
                    index += 1;
                    issued = false;
                }
            }
            false
        }),
        Duration::from_secs(30),
        DEFAULT_INSTRUMENT,
        true,
    );
    evidence.check_cleanup();
    drop(connection);
    drop(sdl);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "production queued-input witness; run with --test-threads=1"]
fn queued_tracker_actions_resolve_each_layer_before_the_next_event() {
    use crest_synth::control::{Direction::*, SemanticAction::*};
    let root = scenarios::temp_root("queued-navigation");
    let mut stage = 0;
    let evidence = run_with_instrument(
        &root,
        Box::new(move |runtime, audio, _| {
            if runtime.initial_sound.is_none() || !runtime.sync.ready() {
                return false;
            }
            match stage {
                0 => {
                    let handler = runtime.action_handler().unwrap();
                    for action in [
                        Adjust(Right),
                        NavigatePage(Right),
                        Adjust(Right),
                        NavigatePage(Right),
                        NavigatePage(Up),
                    ] {
                        assert!(handler(action));
                    }
                    stage = 1;
                    false
                }
                1 => {
                    let view = crate::workspace::project(&runtime.app.borrow())
                        .workspace
                        .unwrap();
                    assert!(
                        view.rows.iter().any(|row| row.id == "phrase_form"),
                        "expected Phrase Settings, got {}",
                        view.context
                    );
                    let handler = runtime.action_handler().unwrap();
                    for action in [
                        NavigatePage(Down),
                        NavigatePage(Down),
                        NavigatePage(Up),
                        NavigatePage(Left),
                        NavigatePage(Left),
                    ] {
                        assert!(handler(action));
                    }
                    stage = 2;
                    false
                }
                _ => {
                    let app = runtime.app.borrow();
                    let view = crate::workspace::project(&app).workspace.unwrap();
                    assert_eq!(app.view, View::Song);
                    assert!(
                        !view.grid.is_empty(),
                        "must return to Song grid, got {}",
                        view.context
                    );
                    assert_eq!(app.editor.song().chains.len(), 1);
                    assert_eq!(app.editor.song().phrases.len(), 1);
                    audio.active_notes() == 0
                }
            }
        }),
        Duration::from_secs(30),
        DEFAULT_INSTRUMENT,
    );
    evidence.check_cleanup();
    std::fs::remove_dir_all(root).unwrap();
}
