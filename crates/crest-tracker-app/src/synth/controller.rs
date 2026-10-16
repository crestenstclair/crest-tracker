//! Held controller input belongs to the host, outside composition and Synth state.
#[cfg(test)]
mod navigation_tests;
mod roll_input;
use crate::app::{App, Request, View};
use crest_synth::control::{ControllerBindings, ControllerButton, ControllerRole};
use crest_synth::shell::hosted_application::{HostControllerAxis, HostControllerInput};
use roll_input::AxisRepeat;
pub(crate) use roll_input::RollBindings;
use std::collections::HashMap;
use std::time::Duration;

const REPEAT_DELAY: Duration = Duration::from_millis(350);
const REPEAT_INTERVAL: Duration = Duration::from_millis(70);
type ButtonKey = (usize, ControllerButton);

struct Repeat {
    key: ButtonKey,
    role: ControllerRole,
    gesture: u64,
    due: Duration,
}

#[derive(Default)]
pub(super) struct ControllerInput {
    held: HashMap<ButtonKey, ControllerRole>,
    repeat: Option<Repeat>,
    elapsed: Duration,
    next_gesture: u64,
    edit_used: bool,
    context: Option<(View, bool, bool)>,
    roll_bindings: RollBindings,
    modifiers: HashMap<ButtonKey, bool>,
    axes: HashMap<(usize, HostControllerAxis), i8>,
    blocked_axes: std::collections::HashSet<(usize, HostControllerAxis)>,
    axis_repeat: Option<AxisRepeat>,
}

impl ControllerInput {
    /// Synth keeps its controls; the documented global Quick Jump remains
    /// available so a controller can return to Tracker without a keyboard.
    pub(super) fn synth_input(
        &mut self,
        app: &mut App,
        input: HostControllerInput,
        bindings: &ControllerBindings,
    ) -> bool {
        self.reconcile_context(app);
        match input {
            HostControllerInput::Pressed { button, .. }
            | HostControllerInput::Released { button, .. }
                if bindings.role(button) == Some(ControllerRole::Shift) =>
            {
                self.input(app, input, bindings);
                false
            }
            HostControllerInput::Pressed { button, .. }
                if app.shift_held && bindings.role(button) == Some(ControllerRole::Down) =>
            {
                app.ui_input("quick_jump");
                true
            }
            HostControllerInput::Axis {
                axis: HostControllerAxis::LeftY,
                value,
                ..
            } if app.shift_held && value >= 16_000 => {
                app.ui_input("quick_jump");
                true
            }
            HostControllerInput::Disconnected { .. } => {
                self.input(app, input, bindings);
                false
            }
            _ => false,
        }
    }
    /// Queue the end after already queued adjustments. Focus loss cannot split
    /// the final repeat from its own undo transaction or resurrect held input.
    pub(super) fn cancel(&mut self, app: &mut App) {
        self.end_repeat(app);
        self.held.clear();
        self.modifiers.clear();
        self.blocked_axes.extend(self.axes.keys().copied());
        self.axes.clear();
        self.edit_used = false;
        app.edit_held = false;
        app.shift_held = false;
        app.fine_held = false;
        app.range_held = false;
    }

    fn reconcile_context(&mut self, app: &mut App) {
        let context = (app.view, app.text_entry, app.controls_focused);
        if self.context.is_some_and(|old| old != context) {
            // Page changes end repeats, not a physically held navigation chord.
            // Retain modifiers, suppress their tap, and quarantine held sticks.
            self.end_repeat(app);
            self.held.retain(|_, role| {
                *role == ControllerRole::Shift
                    || (app.view != View::Instruments && *role == ControllerRole::Edit)
            });
            self.modifiers.clear();
            self.blocked_axes.extend(self.axes.keys().copied());
            self.axes.clear();
            self.edit_used = true;
            app.edit_held = self.held.values().any(|r| *r == ControllerRole::Edit);
            app.shift_held = self.held.values().any(|r| *r == ControllerRole::Shift);
            app.fine_held = false;
            app.range_held = false;
        }
        self.context = Some(context);
    }

    fn end_repeat(&mut self, app: &mut App) {
        if self.repeat.take().is_some() {
            app.requests.push(Request::EndGesture);
        }
        if self.axis_repeat.take().is_some() {
            app.requests.push(Request::EndGesture);
        }
    }

    /// One repeat per control tick; stalls never cause a burst of queued edits.
    pub(super) fn advance(&mut self, app: &mut App, elapsed: Duration) {
        self.reconcile_context(app);
        self.elapsed = self.elapsed.saturating_add(elapsed);
        if self.advance_axis(app) {
            return;
        }
        let Some(repeat) = self.repeat.as_mut() else {
            return;
        };
        if self.elapsed < repeat.due {
            return;
        }
        repeat.due = self.elapsed.saturating_add(REPEAT_INTERVAL);
        dispatch(app, repeat.role, repeat.gesture);
    }

    pub(super) fn input(
        &mut self,
        app: &mut App,
        input: HostControllerInput,
        bindings: &ControllerBindings,
    ) {
        self.reconcile_context(app);
        match input {
            HostControllerInput::Axis {
                device,
                axis,
                value,
            } => self.axis_input(app, device, axis, value),
            HostControllerInput::Disconnected { device } => {
                if self.held.keys().any(|(id, _)| *id == device)
                    || self.axes.keys().any(|(id, _)| *id == device)
                    || self.modifiers.keys().any(|(id, _)| *id == device)
                {
                    // A disconnect invalidates the whole chord of modifiers.
                    self.cancel(app);
                }
                self.blocked_axes.retain(|(id, _)| *id != device);
            }
            HostControllerInput::Pressed { device, button } => {
                let key = (device, button);
                if self.modifier_input(app, key, true, bindings) {
                    return;
                }
                if self.held.contains_key(&key) {
                    return;
                }
                if let Some(role) = bindings.role(button) {
                    self.held.insert(key, role);
                    self.press(app, key, role);
                }
            }
            HostControllerInput::Released { device, button } => {
                let key = (device, button);
                if self.modifier_input(app, key, false, bindings) {
                    return;
                }
                if let Some(role) = self.held.remove(&key) {
                    self.release(app, key, role);
                }
            }
        }
    }

    fn press(&mut self, app: &mut App, key: ButtonKey, role: ControllerRole) {
        self.end_repeat(app);
        if matches!(role, ControllerRole::Edit | ControllerRole::Shift) {
            if role == ControllerRole::Edit {
                self.edit_used = false;
            } else {
                self.edit_used |= app.edit_held;
            }
            app.edit_held = self.held.values().any(|r| *r == ControllerRole::Edit);
            app.shift_held = self.held.values().any(|r| *r == ControllerRole::Shift);
            return;
        }
        self.edit_used |= app.edit_held;
        let direction = matches!(
            role,
            ControllerRole::Up
                | ControllerRole::Down
                | ControllerRole::Left
                | ControllerRole::Right
        );
        if direction && (app.text_entry || !app.shift_held) {
            let Some(gesture) = self.next_gesture.checked_add(1) else {
                app.status =
                    "Controller gesture identity exhausted; restart the application".into();
                return;
            };
            self.next_gesture = gesture;
            dispatch(app, role, gesture);
            self.repeat = Some(Repeat {
                key,
                role,
                gesture,
                due: self.elapsed.saturating_add(REPEAT_DELAY),
            });
        } else {
            dispatch(app, role, 0);
        }
    }

    fn release(&mut self, app: &mut App, key: ButtonKey, role: ControllerRole) {
        if self.repeat.as_ref().is_some_and(|repeat| repeat.key == key)
            || matches!(role, ControllerRole::Edit | ControllerRole::Shift)
        {
            self.end_repeat(app);
        }
        let was_editing = app.edit_held;
        app.edit_held = self.held.values().any(|r| *r == ControllerRole::Edit);
        app.shift_held = self.held.values().any(|r| *r == ControllerRole::Shift);
        if role == ControllerRole::Edit && was_editing && !app.edit_held && !self.edit_used {
            if app.text_entry || app.controls_focused || app.view == View::Settings {
                app.ui_input("confirm");
            } else {
                app.requests.push(Request::Confirm);
            }
        }
    }
}

fn dispatch(app: &mut App, role: ControllerRole, gesture: u64) {
    if app.text_entry || (!app.shift_held && (app.controls_focused || app.view == View::Settings)) {
        let action = match role {
            ControllerRole::Up if app.shift_held => "shift_up",
            ControllerRole::Left if app.edit_held => "edit_previous",
            ControllerRole::Right if app.edit_held => "edit_next",
            ControllerRole::Up => "previous",
            ControllerRole::Down => "next",
            ControllerRole::Left => "decrease",
            ControllerRole::Right => "increase",
            ControllerRole::NextPatch | ControllerRole::Edit => "confirm",
            ControllerRole::PreviousPatch | ControllerRole::Shift => "back",
            _ => return,
        };
        if !app.ui_input(action) {
            app.status = "Controller input is waiting for the editor".into();
        }
        return;
    }
    let direction = match role {
        ControllerRole::Up => Some((0, -1)),
        ControllerRole::Down => Some((0, 1)),
        ControllerRole::Left => Some((-1, 0)),
        ControllerRole::Right => Some((1, 0)),
        _ => None,
    };
    if let Some((horizontal, vertical)) = direction {
        super::direction_gesture(app, horizontal, vertical, Some(gesture));
        return;
    }
    match role {
        ControllerRole::Preview => app.requests.push(Request::Play),
        ControllerRole::Settings => {
            app.requests.push(Request::View {
                view: View::Settings,
            });
        }
        ControllerRole::PreviousPatch => super::back(app),
        ControllerRole::NextPatch => app.requests.push(Request::Confirm),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn button(input: &mut ControllerInput, app: &mut App, button: ControllerButton, down: bool) {
        let event = if down {
            HostControllerInput::Pressed { device: 1, button }
        } else {
            HostControllerInput::Released { device: 1, button }
        };
        input.input(app, event, &ControllerBindings::default());
    }
    #[test]
    fn repeat_is_delayed_bounded_and_release_follows_last_edit() {
        let (mut input, mut app) = (ControllerInput::default(), App::new());
        button(&mut input, &mut app, ControllerButton::South, true);
        button(&mut input, &mut app, ControllerButton::DPadRight, true);
        button(&mut input, &mut app, ControllerButton::DPadRight, true);
        input.advance(&mut app, Duration::from_millis(349));
        assert_eq!(app.requests.len(), 1);
        input.advance(&mut app, Duration::from_millis(1));
        input.advance(&mut app, Duration::from_secs(10));
        assert_eq!(app.requests.len(), 3);
        assert!(app.requests.iter().all(|r| matches!(
            r,
            Request::AdjustGesture {
                delta: 1,
                gesture: 1
            }
        )));
        button(&mut input, &mut app, ControllerButton::DPadRight, false);
        button(&mut input, &mut app, ControllerButton::South, false);
        input.advance(&mut app, Duration::from_secs(10));
        assert!(matches!(
            app.requests.as_slice(),
            [
                Request::AdjustGesture { .. },
                Request::AdjustGesture { .. },
                Request::AdjustGesture { .. },
                Request::EndGesture
            ]
        ));
    }
    #[test]
    fn tap_confirms_but_focus_loss_and_disconnect_never_confirm_or_resume() {
        let (mut input, mut app) = (ControllerInput::default(), App::new());
        button(&mut input, &mut app, ControllerButton::South, true);
        button(&mut input, &mut app, ControllerButton::South, false);
        assert!(matches!(app.requests.pop(), Some(Request::Confirm)));
        button(&mut input, &mut app, ControllerButton::South, true);
        button(&mut input, &mut app, ControllerButton::DPadUp, true);
        input.cancel(&mut app);
        button(&mut input, &mut app, ControllerButton::South, false);
        input.advance(&mut app, Duration::from_secs(1));
        assert_eq!(app.requests.len(), 2);
        app.requests.clear();
        button(&mut input, &mut app, ControllerButton::DPadDown, true);
        input.input(
            &mut app,
            HostControllerInput::Disconnected { device: 1 },
            &ControllerBindings::default(),
        );
        input.advance(&mut app, Duration::from_secs(1));
        assert_eq!(app.requests.len(), 2);
        assert!(input.held.is_empty());
    }
    #[test]
    fn switching_view_or_form_cancels_held_direction() {
        let (mut input, mut app) = (ControllerInput::default(), App::new());
        button(&mut input, &mut app, ControllerButton::DPadDown, true);
        app.text_entry = true;
        input.advance(&mut app, Duration::from_secs(1));
        assert!(input.repeat.is_none());
        assert!(app.projection()["uiInputs"].as_array().unwrap().is_empty());
        button(&mut input, &mut app, ControllerButton::DPadDown, true);
        input.advance(&mut app, Duration::from_millis(350));
        assert_eq!(app.projection()["uiInputs"].as_array().unwrap().len(), 2);
        app.view = View::Instruments;
        input.advance(&mut app, Duration::from_secs(1));
        assert!(input.repeat.is_none());
    }

    #[test]
    fn drum_dpad_and_confirm_reach_visible_lane_and_step() {
        use crest_tracker::{composition::PhraseForm, editing::Command, time::Beat};
        let (mut input, mut app) = (ControllerInput::default(), App::new());
        app.apply(Request::Confirm).unwrap();
        app.apply(Request::Confirm).unwrap();
        let (id, source) = app.selected_phrase().unwrap();
        let mut phrase = source.clone();
        phrase.form = PhraseForm::Drum;
        app.command(Command::PutPhrase { id, phrase }).unwrap();
        for key in [
            ControllerButton::DPadRight,
            ControllerButton::DPadDown,
            ControllerButton::South,
        ] {
            button(&mut input, &mut app, key, true);
            button(&mut input, &mut app, key, false);
            for request in std::mem::take(&mut app.requests) {
                app.apply(request).unwrap();
            }
        }
        assert_eq!((app.row, app.column), (1, 1));
        let event = &app.selected_phrase().unwrap().1.events[0];
        assert_eq!(event.at, Beat::new(1, 4).unwrap());
        assert_eq!(event.note.as_ref().unwrap().pitch, 38);
        assert_eq!(event.note.as_ref().unwrap().velocity, 100);
        button(&mut input, &mut app, ControllerButton::South, true);
        button(&mut input, &mut app, ControllerButton::South, false);
        for request in std::mem::take(&mut app.requests) {
            app.apply(request).unwrap();
        }
        assert!(app.selected_phrase().unwrap().1.events.is_empty());
    }
}
