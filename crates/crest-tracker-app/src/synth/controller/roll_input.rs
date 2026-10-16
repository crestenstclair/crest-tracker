use super::*;
use crate::app::PianoControl;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollBindings {
    pub fine: ControllerButton,
    pub range: ControllerButton,
}
impl Default for RollBindings {
    fn default() -> Self {
        Self {
            fine: ControllerButton::RightShoulder,
            range: ControllerButton::Select,
        }
    }
}
impl RollBindings {
    pub fn validate(&self) -> Result<(), String> {
        if self.fine == self.range {
            return Err("Fine and Range need different buttons".into());
        }
        Ok(())
    }
    pub fn button_options() -> serde_json::Value {
        use ControllerButton::*;
        serde_json::json!([
            South,
            East,
            North,
            West,
            LeftShoulder,
            RightShoulder,
            LeftTrigger,
            RightTrigger,
            Select,
            Start,
            Mode,
            LeftThumb,
            RightThumb,
            DPadUp,
            DPadDown,
            DPadLeft,
            DPadRight
        ]
        .map(|button| serde_json::json!({"value":button,"label":button.label()})))
    }
}

pub(super) struct AxisRepeat {
    key: (usize, HostControllerAxis),
    direction: i8,
    gesture: u64,
    due: Duration,
}

impl ControllerInput {
    pub(crate) fn configure(&mut self, app: &mut App, bindings: RollBindings) {
        if self.roll_bindings != bindings {
            self.cancel(app);
            self.roll_bindings = bindings;
        }
    }

    pub(super) fn modifier_input(
        &mut self,
        app: &mut App,
        key: ButtonKey,
        down: bool,
        bindings: &ControllerBindings,
    ) -> bool {
        if !down && self.modifiers.remove(&key).is_some() {
            app.fine_held = self.modifiers.values().any(|fine| *fine);
            app.range_held = self.modifiers.values().any(|fine| !*fine);
            return true;
        }
        if !app.is_piano_roll() || app.text_entry {
            return false;
        }
        let fine = key.1 == self.roll_bindings.fine;
        if !fine && key.1 != self.roll_bindings.range {
            return false;
        }
        if matches!(
            bindings.role(key.1),
            Some(ControllerRole::Edit | ControllerRole::Shift)
        ) {
            app.status="Fine/Range conflicts with Synth Edit/Shift; choose another button in Tracker settings".into();
            return false;
        }
        if down {
            self.modifiers.insert(key, fine);
        }
        app.fine_held = self.modifiers.values().any(|fine| *fine);
        app.range_held = self.modifiers.values().any(|fine| !*fine);
        true
    }

    pub(super) fn axis_input(
        &mut self,
        app: &mut App,
        device: usize,
        axis: HostControllerAxis,
        value: i16,
    ) {
        let key = (device, axis);
        if self.blocked_axes.contains(&key) {
            if value.unsigned_abs() < 10_000 {
                self.blocked_axes.remove(&key);
            }
            return;
        }
        let old = self.axes.get(&key).copied().unwrap_or(0);
        let direction = if value >= 16_000 {
            1
        } else if value <= -16_000 {
            -1
        } else if value.unsigned_abs() < 10_000 {
            0
        } else {
            old
        };
        if direction == old {
            return;
        }
        self.end_repeat(app);
        if direction == 0 {
            self.axes.remove(&key);
            return;
        }
        self.axes.insert(key, direction);
        self.edit_used |= app.edit_held;
        let Some(gesture) = self.next_gesture.checked_add(1) else {
            app.status = "Controller gesture identity exhausted".into();
            return;
        };
        self.next_gesture = gesture;
        dispatch_axis(app, axis, direction, gesture);
        self.axis_repeat = Some(AxisRepeat {
            key,
            direction,
            gesture,
            due: self.elapsed.saturating_add(REPEAT_DELAY),
        });
    }

    pub(super) fn advance_axis(&mut self, app: &mut App) -> bool {
        let Some(repeat) = self.axis_repeat.as_mut() else {
            return false;
        };
        if self.elapsed >= repeat.due {
            repeat.due = self.elapsed.saturating_add(REPEAT_INTERVAL);
            dispatch_axis(app, repeat.key.1, repeat.direction, repeat.gesture);
        }
        true
    }
}

fn dispatch_axis(app: &mut App, axis: HostControllerAxis, direction: i8, gesture: u64) {
    use HostControllerAxis::*;
    let delta = i32::from(direction);
    if app.text_entry || (!app.shift_held && (app.controls_focused || app.view == View::Settings)) {
        app.ui_input(match axis {
            LeftY if app.shift_held && delta < 0 => "shift_up",
            LeftX if app.edit_held && delta < 0 => "edit_previous",
            LeftX if app.edit_held => "edit_next",
            LeftY => {
                if delta < 0 {
                    "previous"
                } else {
                    "next"
                }
            }
            RightY => {
                if delta < 0 {
                    "increase"
                } else {
                    "decrease"
                }
            }
            _ => {
                if delta < 0 {
                    "decrease"
                } else {
                    "increase"
                }
            }
        });
        return;
    }
    match axis {
        LeftX | LeftY => {
            let role = match (axis, direction < 0) {
                (LeftX, true) => ControllerRole::Left,
                (LeftX, false) => ControllerRole::Right,
                (_, true) => ControllerRole::Up,
                (_, false) => ControllerRole::Down,
            };
            dispatch(app, role, gesture);
        }
        RightX | RightY if app.is_piano_roll() => {
            let control = if axis == RightX {
                PianoControl::Resize { delta }
            } else {
                PianoControl::Adjust { delta: -delta }
            };
            app.requests
                .push(Request::PianoGesture { control, gesture });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crest_tracker::{composition::PhraseForm, editing::Command};
    fn app() -> App {
        let mut app = App::new();
        app.apply(Request::Confirm).unwrap();
        app.apply(Request::Confirm).unwrap();
        let (id, source) = app.selected_phrase().unwrap();
        let mut phrase = source.clone();
        phrase.form = PhraseForm::PianoRoll;
        app.command(Command::PutPhrase { id, phrase }).unwrap();
        app
    }
    #[test]
    fn stick_hysteresis_repeat_focus_loss_and_disconnect_never_replay_held_edits() {
        let mut input = ControllerInput::default();
        let mut app = app();
        let bindings = ControllerBindings::default();
        let motion = |value| HostControllerInput::Axis {
            device: 1,
            axis: HostControllerAxis::RightX,
            value,
        };
        input.input(&mut app, motion(20_000), &bindings);
        input.input(&mut app, motion(18_000), &bindings);
        assert_eq!(app.requests.len(), 1);
        assert!(matches!(
            app.requests[0],
            Request::PianoGesture {
                control: PianoControl::Resize { delta: 1 },
                gesture: 1
            }
        ));
        input.advance(&mut app, Duration::from_secs(10));
        assert_eq!(app.requests.len(), 2);
        input.cancel(&mut app);
        let stopped = app.requests.len();
        input.input(&mut app, motion(25_000), &bindings);
        input.advance(&mut app, Duration::from_secs(10));
        assert_eq!(app.requests.len(), stopped);
        input.input(&mut app, motion(0), &bindings);
        input.input(&mut app, motion(-20_000), &bindings);
        assert!(matches!(
            app.requests.last(),
            Some(Request::PianoGesture {
                control: PianoControl::Resize { delta: -1 },
                ..
            })
        ));
        input.input(
            &mut app,
            HostControllerInput::Disconnected { device: 1 },
            &bindings,
        );
        let stopped = app.requests.len();
        input.advance(&mut app, Duration::from_secs(10));
        assert_eq!(app.requests.len(), stopped);
    }
    #[test]
    fn fine_range_are_configurable_and_never_steal_edit_or_shift() {
        let mut input = ControllerInput::default();
        let mut app = app();
        let bindings = ControllerBindings::default();
        input.input(
            &mut app,
            HostControllerInput::Pressed {
                device: 1,
                button: ControllerButton::RightShoulder,
            },
            &bindings,
        );
        assert!(app.fine_held);
        input.input(
            &mut app,
            HostControllerInput::Pressed {
                device: 1,
                button: ControllerButton::Select,
            },
            &bindings,
        );
        assert!(app.range_held);
        input.input(
            &mut app,
            HostControllerInput::Disconnected { device: 1 },
            &bindings,
        );
        assert!(!app.fine_held && !app.range_held);
        input.configure(
            &mut app,
            RollBindings {
                fine: ControllerButton::South,
                range: ControllerButton::Select,
            },
        );
        input.input(
            &mut app,
            HostControllerInput::Pressed {
                device: 1,
                button: ControllerButton::South,
            },
            &bindings,
        );
        assert!(app.edit_held && !app.fine_held);
        assert!(app.status.contains("conflicts"));
    }
}
