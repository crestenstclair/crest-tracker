//! Physical SDL edges through the composed host, editor, transport and DSP.
use super::*;
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
    let steps: Vec<(Button, bool, Check)> = vec![
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
            assert_eq!(r.app.borrow().row, 1)
        }),
        (Button::DPadDown, false, |_| {}),
        (Button::North, true, |r| {
            assert!(r.app.borrow().playing, "{}", r.app.borrow().status)
        }),
        (Button::North, false, |r| assert!(r.app.borrow().playing)),
        (Button::North, true, |r| assert!(!r.app.borrow().playing)),
        (Button::North, false, |_| {}),
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
            assert_eq!(r.app.borrow().view, View::Instruments)
        }),
        (Button::DPadDown, false, |_| {}),
        (Button::DPadUp, true, |_| {}),
        (Button::DPadUp, false, |_| {}),
        (Button::East, false, |_| {}),
    ];
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
                let dwell = if button == Button::North { 80 } else { 4 };
                if ticks >= dwell {
                    check(runtime);
                    index += 1;
                    issued = false;
                }
            }
            false
        }),
        Duration::from_secs(30),
        crest_synth::adapter::braids_capability::BRAIDS_CAPABILITY_ID,
        true,
    );
    evidence.check_cleanup();
    drop(connection);
    drop(sdl);
    std::fs::remove_dir_all(root).unwrap();
}
