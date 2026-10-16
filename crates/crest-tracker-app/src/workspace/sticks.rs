//! Analog edges join the same workspace command boundary as buttons/keyboard.
//! No controller reads: SDL axis events and elapsed ticks drive hysteresis/repeat.
use super::*;
use crest_synth::control::{ControllerBindings, ControllerButton, ControllerRole};
use crest_synth::shell::hosted_application::{HostControllerAxis, HostControllerInput};
use std::{collections::HashMap, time::Duration};

#[derive(Default)]
pub(crate) struct Sticks {
    axes: HashMap<(usize, HostControllerAxis), AxisHold>,
    shifts: HashMap<usize, bool>,
    fine: HashMap<usize, bool>,
    range: HashMap<usize, bool>,
    starts: std::collections::HashSet<usize>,
    context: Option<(View, Option<Panel>)>,
    gesture: u64,
}
struct AxisHold {
    sign: i32,
    remaining: Duration,
    suppressed: bool,
}
impl Sticks {
    fn synchronize(&mut self, app: &App) {
        let context = (app.view, app.workspace.panel);
        if self.context != Some(context) {
            for axis in self.axes.values_mut() {
                axis.suppressed = true;
            }
            self.context = Some(context);
        }
    }
    pub(crate) fn event(
        &mut self,
        app: &mut App,
        input: HostControllerInput,
        bindings: &ControllerBindings,
        roll: &crate::preferences::RollBindings,
    ) -> bool {
        self.synchronize(app);
        match input {
            HostControllerInput::Pressed { device, button }
            | HostControllerInput::Released { device, button } => {
                if bindings.role(button) == Some(ControllerRole::Shift) {
                    self.shifts
                        .insert(device, matches!(input, HostControllerInput::Pressed { .. }));
                }
                let pressed = matches!(input, HostControllerInput::Pressed { .. });
                if button == ControllerButton::Start
                    && bindings.role(button) == Some(ControllerRole::Settings)
                {
                    if pressed && owns_input(app) {
                        if self.starts.insert(device) {
                            if self.shifts.get(&device) == Some(&true) {
                                action(app, SemanticAction::OpenMidiSettings);
                            } else {
                                action(app, SemanticAction::PreviewStart);
                            }
                        }
                        return true;
                    }
                    if !pressed && self.starts.remove(&device) {
                        return true;
                    }
                }
                // Supplemental roll gestures never steal a configured global role.
                if bindings.role(button).is_none() && (button == roll.fine || button == roll.range)
                {
                    let held = if button == roll.fine {
                        &mut self.fine
                    } else {
                        &mut self.range
                    };
                    if !pressed {
                        held.remove(&device);
                    }
                    if pressed && app.is_piano_roll() && app.workspace.panel.is_none() {
                        held.insert(device, true);
                    }
                    if button == roll.fine {
                        app.fine_held = !self.fine.is_empty();
                    }
                    if button == roll.range {
                        app.range_held = !self.range.is_empty();
                    }
                    return app.is_piano_roll();
                }
                false
            }
            HostControllerInput::Disconnected { device } => {
                let had_axes = !self.axes.is_empty();
                self.axes.retain(|(id, _), _| *id != device);
                self.shifts.remove(&device);
                self.fine.remove(&device);
                self.range.remove(&device);
                self.starts.remove(&device);
                app.fine_held = !self.fine.is_empty();
                app.range_held = !self.range.is_empty();
                if had_axes && self.axes.is_empty() {
                    app.requests.push(Request::EndGesture);
                }
                false
            }
            HostControllerInput::Axis {
                device,
                axis,
                value,
            } => {
                if value.unsigned_abs() < 8_000 {
                    if self.axes.remove(&(device, axis)).is_some() && self.axes.is_empty() {
                        app.requests.push(Request::EndGesture);
                    }
                    return owns_input(app);
                }
                if !owns_input(app) || value.unsigned_abs() < 16_000 {
                    return false;
                }
                let sign = i32::from(value.signum());
                if self
                    .axes
                    .get(&(device, axis))
                    .is_some_and(|held| held.sign == sign)
                {
                    return true;
                }
                let shift = self.shifts.get(&device) == Some(&true);
                if self.axes.is_empty() {
                    app.workspace.gesture = app.workspace.gesture.checked_add(1).unwrap_or(1);
                    self.gesture = app.workspace.gesture;
                }
                self.axes.insert(
                    (device, axis),
                    AxisHold {
                        sign,
                        remaining: Duration::from_millis(280),
                        suppressed: shift,
                    },
                );
                Self::dispatch(app, axis, sign, shift, self.gesture);
                true
            }
        }
    }
    pub(crate) fn advance(&mut self, app: &mut App, elapsed: Duration) {
        self.synchronize(app);
        if !owns_input(app) {
            return;
        }
        for ((device, axis), held) in &mut self.axes {
            if held.suppressed || self.shifts.get(device) == Some(&true) {
                continue;
            }
            if held.remaining > elapsed {
                held.remaining -= elapsed;
                continue;
            }
            held.remaining = Duration::from_millis(45);
            Self::dispatch(app, *axis, held.sign, false, self.gesture);
        }
    }
    fn dispatch(app: &mut App, axis: HostControllerAxis, sign: i32, shift: bool, gesture: u64) {
        use HostControllerAxis::*;
        let direction = match (axis, sign > 0) {
            (LeftX | RightX, false) => Direction::Left,
            (LeftX | RightX, true) => Direction::Right,
            (_, false) => Direction::Up,
            (_, true) => Direction::Down,
        };
        if shift {
            action(app, SemanticAction::NavigatePage(direction));
        } else if matches!(axis, RightX | RightY)
            && app.is_piano_roll()
            && app.workspace.panel.is_none()
        {
            let control = if axis == RightX {
                PianoControl::Resize { delta: sign }
            } else {
                PianoControl::Velocity {
                    delta: -sign * if app.fine_held { 1 } else { 10 },
                }
            };
            app.requests
                .push(Request::PianoGesture { control, gesture });
        } else if matches!(axis, RightX | RightY) || app.edit_held {
            app.workspace.gesture = gesture;
            action(app, SemanticAction::Adjust(direction));
        } else {
            action(app, SemanticAction::Navigate(direction));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn physical_start_toggles_once_and_roll_modifiers_clear_on_disconnect() {
        let mut app = App::new();
        let mut sticks = Sticks::default();
        let bindings = ControllerBindings::default();
        let roll = crate::preferences::RollBindings::default();
        let press = HostControllerInput::Pressed {
            device: 1,
            button: ControllerButton::Start,
        };
        assert!(sticks.event(&mut app, press, &bindings, &roll));
        assert!(sticks.event(&mut app, press, &bindings, &roll));
        assert_eq!(
            app.requests
                .iter()
                .filter(|request| matches!(request, Request::Play))
                .count(),
            1
        );
        assert!(app.workspace.panel.is_none());
        assert!(sticks.event(
            &mut app,
            HostControllerInput::Released {
                device: 1,
                button: ControllerButton::Start
            },
            &bindings,
            &roll
        ));
        app.requests.clear();
        sticks.event(
            &mut app,
            HostControllerInput::Pressed {
                device: 1,
                button: bindings.button(ControllerRole::Shift),
            },
            &bindings,
            &roll,
        );
        sticks.event(&mut app, press, &bindings, &roll);
        assert_eq!(app.workspace.panel, Some(Panel::Settings));
        assert!(!app
            .requests
            .iter()
            .any(|request| matches!(request, Request::Play)));
        sticks.event(
            &mut app,
            HostControllerInput::Disconnected { device: 1 },
            &bindings,
            &roll,
        );
        assert!(sticks.starts.is_empty() && sticks.shifts.is_empty());
    }
    #[test]
    fn sticks_repeat_semantic_navigation_and_release_or_page_change_stops_motion() {
        let mut app = App::new();
        let mut sticks = Sticks::default();
        let bindings = ControllerBindings::default();
        let roll = crate::preferences::RollBindings::default();
        let edge = |value| HostControllerInput::Axis {
            device: 1,
            axis: HostControllerAxis::LeftX,
            value,
        };
        sticks.event(&mut app, edge(25_000), &bindings, &roll);
        assert!(matches!(
            app.requests.pop(),
            Some(Request::Move {
                horizontal: 1,
                vertical: 0
            })
        ));
        sticks.event(&mut app, edge(26_000), &bindings, &roll);
        assert!(app.requests.is_empty());
        sticks.advance(&mut app, Duration::from_millis(280));
        assert!(matches!(
            app.requests.pop(),
            Some(Request::Move {
                horizontal: 1,
                vertical: 0
            })
        ));
        sticks.event(&mut app, edge(0), &bindings, &roll);
        assert!(matches!(app.requests.pop(), Some(Request::EndGesture)));
        sticks.advance(&mut app, Duration::from_secs(1));
        assert!(app.requests.is_empty());
        sticks.event(&mut app, edge(25_000), &bindings, &roll);
        app.requests.clear();
        app.workspace.panel = Some(Panel::Menu);
        sticks.advance(&mut app, Duration::from_secs(1));
        assert!(app.workspace.focus.is_none());
        sticks.event(
            &mut app,
            HostControllerInput::Disconnected { device: 1 },
            &bindings,
            &roll,
        );
        assert!(sticks.axes.is_empty());
    }
}
