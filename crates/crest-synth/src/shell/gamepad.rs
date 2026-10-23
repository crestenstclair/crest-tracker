//! Native gamepad polling belongs to the control tick, never the audio callback.
//! This adapter owns physical holds only; all product state stays reducer-owned.
use crate::adapter::controller_preferences::ControllerPreferenceWorker;
use crate::control::{
    AppEvent, AppLoop, ControllerBindings, ControllerButton, ControllerDevice, ControllerEvent,
    ControllerFailure, ControllerRole, ControllerState, Direction, EngineSelectionRequestId,
    EventSource, InteractionMode, SemanticAction, SurfaceId,
};
use crate::real_time::{BoundaryFull, ControlAudioBoundary};
use crate::shell::hosted_application::{HostControllerAxis, HostControllerInput};
use crate::shell::{ControllerGesture, ControllerInput, ControllerInputTranslator};
use sdl3::event::Event;
use sdl3::gamepad::{Axis, Button, Gamepad};
use sdl3::joystick::JoystickId;
use std::{collections::HashMap, time::Duration};

// A scheduling budget, not a product/device capacity. Remaining events stay in
// the backend queue for the next UI tick; all connected devices are represented.
const EVENTS_PER_TICK: usize = 128;

const STANDARD_BUTTONS: [(Button, ControllerButton); 15] = [
    (Button::South, ControllerButton::South),
    (Button::East, ControllerButton::East),
    (Button::North, ControllerButton::North),
    (Button::West, ControllerButton::West),
    (Button::LeftShoulder, ControllerButton::LeftShoulder),
    (Button::RightShoulder, ControllerButton::RightShoulder),
    (Button::Back, ControllerButton::Select),
    (Button::Start, ControllerButton::Start),
    (Button::Guide, ControllerButton::Mode),
    (Button::LeftStick, ControllerButton::LeftThumb),
    (Button::RightStick, ControllerButton::RightThumb),
    (Button::DPadUp, ControllerButton::DPadUp),
    (Button::DPadDown, ControllerButton::DPadDown),
    (Button::DPadLeft, ControllerButton::DPadLeft),
    (Button::DPadRight, ControllerButton::DPadRight),
];

// SDL normalizes triggers to 0..32767. Crest treats them as Patch-cycle
// buttons, with hysteresis so resting near the press threshold cannot repeat.
const TRIGGER_PRESS: i16 = (i16::MAX as f32 * 0.75) as i16;
const TRIGGER_RELEASE: i16 = (i16::MAX as f32 * 0.65) as i16;
const TRIGGERS: [(Axis, ControllerButton); 2] = [
    (Axis::TriggerLeft, ControllerButton::LeftTrigger),
    (Axis::TriggerRight, ControllerButton::RightTrigger),
];

fn host_stick_axis(axis: Axis) -> Option<HostControllerAxis> {
    match axis {
        Axis::LeftX => Some(HostControllerAxis::LeftX),
        Axis::LeftY => Some(HostControllerAxis::LeftY),
        Axis::RightX => Some(HostControllerAxis::RightX),
        Axis::RightY => Some(HostControllerAxis::RightY),
        _ => None,
    }
}

// All SDL objects stay on the control thread. Close handles before the event
// pump and subsystems; never initialize SDL video/audio or replace Tauri's loop.
struct SdlGamepads {
    devices: HashMap<JoystickId, Gamepad>,
    events: sdl3::EventPump,
    subsystem: sdl3::GamepadSubsystem,
    _sdl: sdl3::Sdl,
}

impl SdlGamepads {
    fn connected(&self, id: JoystickId) -> bool {
        self.devices.get(&id).is_some_and(Gamepad::connected)
    }

    fn new() -> Result<Self, ControllerFailure> {
        // SDL owns gamepads only. Its default SIGINT/SIGTERM handlers turn
        // terminal interrupts into SDL Quit events, which this adapter ignores.
        sdl3::hint::set("SDL_NO_SIGNAL_HANDLERS", "1");
        sdl3::hint::set("SDL_JOYSTICK_THREAD", "1");
        sdl3::hint::set("SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "1");
        let sdl = sdl3::init().map_err(|_| ControllerFailure::BackendUnavailable)?;
        let subsystem = sdl
            .gamepad()
            .map_err(|_| ControllerFailure::BackendUnavailable)?;
        let events = sdl
            .event_pump()
            .map_err(|_| ControllerFailure::BackendUnavailable)?;
        let mut backend = Self {
            devices: HashMap::new(),
            events,
            subsystem,
            _sdl: sdl,
        };
        for id in backend
            .subsystem
            .gamepads()
            .map_err(|_| ControllerFailure::BackendUnavailable)?
        {
            backend.open(id)?;
        }
        Ok(backend)
    }

    fn open(&mut self, id: JoystickId) -> Result<(), ControllerFailure> {
        if self.devices.contains_key(&id) {
            return Ok(());
        }
        match self.subsystem.open(id) {
            Ok(gamepad) => {
                self.devices.insert(id, gamepad);
                Ok(())
            }
            // An add event can outlive its device. Do not report an unplug as
            // an initialization failure, or invent a mapping for an unknown pad.
            Err(_) if !self.subsystem.is_gamepad(id) => Ok(()),
            Err(_) => Err(ControllerFailure::DeviceOpenFailed),
        }
    }
}

fn button(button: Button) -> Option<ControllerButton> {
    STANDARD_BUTTONS
        .iter()
        .find_map(|(native, canonical)| (*native == button).then_some(*canonical))
}

fn device_name(native: &str) -> String {
    let name: String = native
        .chars()
        .map(|value| {
            if value.is_control() {
                '\u{fffd}'
            } else {
                value
            }
        })
        .collect();
    if name.trim().is_empty() {
        "Unnamed controller".to_owned()
    } else {
        name
    }
}

#[derive(Clone, Copy, Debug)]
enum Hold {
    Suppressed,
    Modifier(ControllerRole),
    Edit { used: bool },
    Gesture(ControllerGesture),
    Action,
}

#[derive(Default)]
struct DeviceInput {
    host_navigation: bool,
    held: HashMap<ControllerButton, Hold>,
    translator: ControllerInputTranslator,
    preview_request: Option<EngineSelectionRequestId>,
    repeat: Option<(ControllerButton, Duration)>,
}

#[derive(Debug, PartialEq)]
enum InputOutcome {
    Action(SemanticAction),
    Capture(ControllerButton),
}

impl DeviceInput {
    fn shift_held(&self) -> bool {
        self.held
            .values()
            .any(|hold| matches!(hold, Hold::Modifier(ControllerRole::Shift)))
    }

    fn edit_held(&self) -> bool {
        self.held
            .values()
            .any(|hold| matches!(hold, Hold::Edit { .. }))
    }

    fn consume_edit(&mut self) {
        for hold in self.held.values_mut() {
            if let Hold::Edit { used } = hold {
                *used = true;
            }
        }
    }

    fn press(
        &mut self,
        button: ControllerButton,
        bindings: &ControllerBindings,
        capture: bool,
        ready: bool,
    ) -> Option<InputOutcome> {
        if self.held.contains_key(&button) {
            return None;
        }
        self.held.insert(button, Hold::Suppressed);
        if capture {
            return Some(InputOutcome::Capture(button));
        }
        if !ready {
            return None;
        }
        let role = bindings.role(button)?;
        let gesture = match role {
            ControllerRole::Up
            | ControllerRole::Down
            | ControllerRole::Left
            | ControllerRole::Right => {
                let direction = match role {
                    ControllerRole::Up => Direction::Up,
                    ControllerRole::Down => Direction::Down,
                    ControllerRole::Left => Direction::Left,
                    _ => Direction::Right,
                };
                let edit = self.edit_held();
                self.consume_edit();
                if self.shift_held() {
                    ControllerGesture::ShiftDirection(direction)
                } else if edit {
                    ControllerGesture::EditDirection(direction)
                } else {
                    ControllerGesture::Direction(direction)
                }
            }
            ControllerRole::Edit => {
                // Pressing Edit while a direction is already held is not a tap.
                let used = self.held.values().any(|hold| {
                    matches!(
                        hold,
                        Hold::Gesture(
                            ControllerGesture::Direction(_)
                                | ControllerGesture::EditDirection(_)
                                | ControllerGesture::ShiftDirection(_)
                        )
                    )
                });
                self.held.insert(button, Hold::Edit { used });
                return None;
            }
            ControllerRole::Shift => {
                self.held.insert(button, Hold::Modifier(role));
                return None;
            }
            ControllerRole::Preview => {
                self.consume_edit();
                if self.shift_held() && !self.host_navigation {
                    ControllerGesture::ShiftStart
                } else {
                    ControllerGesture::Start
                }
            }
            ControllerRole::Popover
            | ControllerRole::PreviousPatch
            | ControllerRole::NextPatch
            | ControllerRole::Settings => {
                self.consume_edit();
                self.held.insert(button, Hold::Action);
                return Some(InputOutcome::Action(match role {
                    ControllerRole::Popover => SemanticAction::OpenPopover,
                    ControllerRole::PreviousPatch if self.shift_held() && self.host_navigation => {
                        SemanticAction::Return
                    }
                    ControllerRole::PreviousPatch => SemanticAction::SelectPatch(Direction::Left),
                    ControllerRole::NextPatch => SemanticAction::SelectPatch(Direction::Right),
                    _ => SemanticAction::OpenMidiSettings,
                }));
            }
        };
        if matches!(
            gesture,
            ControllerGesture::Direction(_) | ControllerGesture::EditDirection(_)
        ) {
            self.repeat = Some((button, Duration::from_millis(280)));
        }
        self.held.insert(button, Hold::Gesture(gesture));
        self.translator
            .translate(ControllerInput::pressed(gesture))
            .map(InputOutcome::Action)
    }

    /// Repeats come from elapsed control time and recorded press/release edges.
    /// Page chords and one-shot actions never repeat. A delayed tick emits at
    /// most one action, avoiding a burst of edits after a stalled UI.
    fn repeat(&mut self, elapsed: Duration) -> Option<SemanticAction> {
        let (button, remaining) = self.repeat.as_mut()?;
        if *remaining > elapsed {
            *remaining -= elapsed;
            return None;
        }
        *remaining = Duration::from_millis(45);
        let direction = match self.held.get(button)? {
            Hold::Gesture(
                ControllerGesture::Direction(d) | ControllerGesture::EditDirection(d),
            ) => *d,
            _ => return None,
        };
        if self.shift_held() {
            return None;
        }
        Some(if self.edit_held() {
            self.consume_edit();
            SemanticAction::Adjust(direction)
        } else {
            SemanticAction::Navigate(direction)
        })
    }

    fn release(&mut self, button: ControllerButton) -> Option<SemanticAction> {
        if self.repeat.is_some_and(|(held, _)| held == button) {
            self.repeat = None;
        }
        match self.held.remove(&button)? {
            Hold::Edit { used: false } => self
                .translator
                .translate(ControllerInput::pressed(ControllerGesture::Edit)),
            Hold::Gesture(gesture) => self
                .translator
                .translate(ControllerInput::released(gesture)),
            _ => None,
        }
    }

    fn quarantine(&mut self) -> Option<SemanticAction> {
        self.suspend(false)
    }

    fn suspend(&mut self, preserve_shift: bool) -> Option<SemanticAction> {
        self.repeat = None;
        for hold in self.held.values_mut() {
            if !preserve_shift || !matches!(hold, Hold::Modifier(ControllerRole::Shift)) {
                *hold = Hold::Suppressed;
            }
        }
        self.translator.translate(ControllerInput::disconnected())
    }

    fn disconnect(&mut self) -> Option<SemanticAction> {
        self.repeat = None;
        self.held.clear();
        self.translator.translate(ControllerInput::disconnected())
    }
}

/// Coordinates only adapter-owned holds. The canonical interaction mode and
/// preview request remain reducer-owned and are changed through semantic actions.
#[derive(Clone, Copy)]
enum InputOrigin {
    Keyboard,
    Controller(usize),
}

impl InputOrigin {
    fn source(self) -> EventSource {
        match self {
            Self::Keyboard => EventSource::Keyboard,
            Self::Controller(_) => EventSource::Controller,
        }
    }
}

#[derive(Default)]
struct GamepadInputs {
    host_action: Option<std::rc::Rc<dyn Fn(SemanticAction) -> bool>>,
    devices: HashMap<usize, DeviceInput>,
    edit_mode_owned: bool,
    keyboard_edit_held: bool,
    keyboard_preview_request: Option<EngineSelectionRequestId>,
}

impl GamepadInputs {
    fn route_host(&self, action: SemanticAction) -> bool {
        self.host_action.as_ref().is_some_and(|route| route(action))
    }

    fn any_edit_held(&self) -> bool {
        self.keyboard_edit_held || self.devices.values().any(DeviceInput::edit_held)
    }

    fn enter_adjust<Boundary: ControlAudioBoundary>(
        &mut self,
        app: &mut AppLoop<Boundary>,
        source: EventSource,
    ) -> Result<(), BoundaryFull> {
        if self.route_host(SemanticAction::SetInteractionMode(InteractionMode::Adjust)) {
            self.edit_mode_owned = true;
            return Ok(());
        }
        if !matches!(
            app.state().interaction().active_surface(),
            SurfaceId::MidiDeviceSettings | SurfaceId::ControllerSettings
        ) && app.state().interaction().mode() == InteractionMode::Navigate
            && dispatch_action(
                app,
                SemanticAction::SetInteractionMode(InteractionMode::Adjust),
                false,
                source,
            )?
        {
            self.edit_mode_owned = true;
        }
        Ok(())
    }

    fn release_adjust<Boundary: ControlAudioBoundary>(
        &mut self,
        app: &mut AppLoop<Boundary>,
        source: EventSource,
    ) -> Result<(), BoundaryFull> {
        if !self.any_edit_held()
            && self.edit_mode_owned
            && self.route_host(SemanticAction::SetInteractionMode(
                InteractionMode::Navigate,
            ))
        {
            self.edit_mode_owned = false;
            return Ok(());
        }
        if !self.any_edit_held()
            && core::mem::take(&mut self.edit_mode_owned)
            && app.state().interaction().mode() == InteractionMode::Adjust
        {
            dispatch_action(
                app,
                SemanticAction::SetInteractionMode(InteractionMode::Navigate),
                false,
                source,
            )?;
        }
        Ok(())
    }

    fn keyboard_action<Boundary: ControlAudioBoundary>(
        &mut self,
        app: &mut AppLoop<Boundary>,
        action: SemanticAction,
        blocked: bool,
    ) -> Result<(), BoundaryFull> {
        match action {
            SemanticAction::SetInteractionMode(InteractionMode::Adjust) => {
                self.keyboard_edit_held = true;
                self.enter_adjust(app, EventSource::Keyboard)
            }
            SemanticAction::SetInteractionMode(InteractionMode::Navigate) => {
                self.keyboard_edit_held = false;
                self.release_adjust(app, EventSource::Keyboard)
            }
            action => self.action(app, InputOrigin::Keyboard, action, blocked),
        }
    }

    fn preview_request_mut(
        &mut self,
        origin: InputOrigin,
    ) -> Option<&mut Option<EngineSelectionRequestId>> {
        match origin {
            InputOrigin::Keyboard => Some(&mut self.keyboard_preview_request),
            InputOrigin::Controller(id) => self
                .devices
                .get_mut(&id)
                .map(|device| &mut device.preview_request),
        }
    }

    fn press<Boundary: ControlAudioBoundary>(
        &mut self,
        app: &mut AppLoop<Boundary>,
        id: usize,
        button: ControllerButton,
        blocked: bool,
    ) -> Result<(), BoundaryFull> {
        let state = app.state().controller();
        let Some(device) = self.devices.get_mut(&id) else {
            return Ok(());
        };
        device.host_navigation = self.host_action.is_some();
        let was_edit = device.edit_held();
        let outcome = device.press(
            button,
            state.bindings(),
            state.capture().is_some(),
            state.ready(),
        );
        if !was_edit && device.edit_held() {
            self.enter_adjust(app, EventSource::Controller)?;
        }
        match outcome {
            Some(InputOutcome::Action(action)) => {
                self.action(app, InputOrigin::Controller(id), action, blocked)?
            }
            Some(InputOutcome::Capture(button)) => dispatch_event(
                app,
                ControllerEvent::ButtonCaptured {
                    device_id: id,
                    button,
                },
            )?,
            None => {}
        }
        Ok(())
    }

    fn release<Boundary: ControlAudioBoundary>(
        &mut self,
        app: &mut AppLoop<Boundary>,
        id: usize,
        button: ControllerButton,
        blocked: bool,
    ) -> Result<(), BoundaryFull> {
        let Some(device) = self.devices.get_mut(&id) else {
            return Ok(());
        };
        let was_edit = device.edit_held();
        let action = device.release(button);
        if was_edit && !device.edit_held() {
            self.release_adjust(app, EventSource::Controller)?;
        }
        // Tap confirmation follows mode restoration: Activate may enter a modal
        // surface, where a later Navigate-mode reset would be invalid.
        if let Some(action) = action {
            self.action(app, InputOrigin::Controller(id), action, blocked)?;
        }
        Ok(())
    }

    fn action<Boundary: ControlAudioBoundary>(
        &mut self,
        app: &mut AppLoop<Boundary>,
        origin: InputOrigin,
        action: SemanticAction,
        blocked: bool,
    ) -> Result<(), BoundaryFull> {
        if self.route_host(action.clone()) {
            return Ok(());
        }
        if blocked && action.may_change_saved_session() {
            return Ok(());
        }
        if action == SemanticAction::PreviewStop {
            let owned = self.preview_request_mut(origin).and_then(Option::take);
            if owned.is_none()
                || owned != app.state().file_browser().preview_request_id()
                || !app.state().file_browser().preview_is_held()
            {
                return Ok(());
            }
        }
        if matches!(action, SemanticAction::Adjust(_)) {
            self.enter_adjust(app, origin.source())?;
        }
        let normalize = self.edit_mode_owned
            // A keyboard chord that explicitly holds K retains its existing
            // reducer admission. Normalization serves independent input sources.
            && !(matches!(origin, InputOrigin::Keyboard) && self.keyboard_edit_held)
            && app.state().interaction().mode() == InteractionMode::Adjust
            && matches!(
                action,
                SemanticAction::Navigate(_)
                    | SemanticAction::NavigatePage(_)
                    | SemanticAction::Activate
                    | SemanticAction::OpenMidiSettings
                    | SemanticAction::SelectPatch(_)
            );
        let resume = normalize && self.any_edit_held();
        if normalize {
            dispatch_action(
                app,
                SemanticAction::SetInteractionMode(InteractionMode::Navigate),
                false,
                origin.source(),
            )?;
            self.edit_mode_owned = false;
        }
        let starts_preview = action == SemanticAction::PreviewStart;
        let accepted = dispatch_action(app, action, blocked, origin.source())?;
        if starts_preview {
            if let Some(request) = self.preview_request_mut(origin) {
                *request = accepted
                    .then(|| app.state().file_browser().preview_request_id())
                    .flatten();
            }
        }
        if app.state().interaction().mode() != InteractionMode::Adjust {
            self.edit_mode_owned = false;
        }
        if resume && app.state().controller().capture().is_none() {
            self.enter_adjust(app, origin.source())?;
        }
        Ok(())
    }

    fn quarantine<Boundary: ControlAudioBoundary>(
        &mut self,
        app: &mut AppLoop<Boundary>,
    ) -> Result<(), BoundaryFull> {
        self.suspend(app, false)
    }

    fn suspend<Boundary: ControlAudioBoundary>(
        &mut self,
        app: &mut AppLoop<Boundary>,
        preserve_shift: bool,
    ) -> Result<(), BoundaryFull> {
        let cleanup: Vec<_> = self
            .devices
            .iter_mut()
            .filter_map(|(id, device)| {
                let action = if preserve_shift {
                    device.suspend(true)
                } else {
                    device.quarantine()
                };
                action.map(|action| (*id, action))
            })
            .collect();
        self.release_adjust(app, EventSource::Controller)?;
        for (id, action) in cleanup {
            self.action(app, InputOrigin::Controller(id), action, false)?;
        }
        Ok(())
    }

    fn disconnect<Boundary: ControlAudioBoundary>(
        &mut self,
        app: &mut AppLoop<Boundary>,
        id: usize,
    ) -> Result<(), BoundaryFull> {
        let action = self.devices.get_mut(&id).and_then(DeviceInput::disconnect);
        self.release_adjust(app, EventSource::Controller)?;
        if let Some(action) = action {
            self.action(app, InputOrigin::Controller(id), action, false)?;
        }
        self.devices.remove(&id);
        Ok(())
    }
}

pub(crate) struct GamepadRuntime {
    backend: Option<SdlGamepads>,
    backend_failure: Option<ControllerFailure>,
    preferences: ControllerPreferenceWorker,
    inputs: GamepadInputs,
    published_devices: Vec<ControllerDevice>,
    input_context: Option<(ControllerBindings, Option<ControllerRole>, bool)>,
    host_has_input: bool,
}

impl GamepadRuntime {
    pub(crate) fn new() -> Self {
        let (backend, backend_failure) = match SdlGamepads::new() {
            Ok(backend) => (Some(backend), None),
            Err(failure) => (None, Some(failure)),
        };
        Self {
            backend,
            backend_failure,
            preferences: ControllerPreferenceWorker::new(),
            inputs: GamepadInputs::default(),
            published_devices: Vec::new(),
            input_context: None,
            host_has_input: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn advance<Boundary: ControlAudioBoundary>(
        &mut self,
        app_loop: &mut AppLoop<Boundary>,
        persisted_edits_blocked: bool,
    ) -> Result<(), BoundaryFull> {
        self.advance_hosted(
            app_loop,
            persisted_edits_blocked,
            false,
            Duration::ZERO,
            &mut |_, _| false,
        )
    }

    pub(crate) fn set_host_action_handler(
        &mut self,
        handler: Option<std::rc::Rc<dyn Fn(SemanticAction) -> bool>>,
    ) {
        self.inputs.host_action = handler;
    }

    pub(crate) fn advance_hosted<Boundary: ControlAudioBoundary>(
        &mut self,
        app_loop: &mut AppLoop<Boundary>,
        persisted_edits_blocked: bool,
        host_has_input: bool,
        elapsed: Duration,
        route: &mut dyn FnMut(HostControllerInput, &ControllerBindings) -> bool,
    ) -> Result<(), BoundaryFull> {
        if self.host_has_input != host_has_input {
            // Shared semantic input keeps its page modifier across workspace
            // changes. Cancel actions/edits, but Shift is not owned by either page.
            self.inputs
                .suspend(app_loop, self.inputs.host_action.is_some())?;
            self.host_has_input = host_has_input;
        }
        self.publish_failure(app_loop)?;
        // At most the initial load and one save can be awaiting consumption.
        for _ in 0..2 {
            let Some(event) = self.preferences.poll() else {
                break;
            };
            dispatch_event(app_loop, event)?;
        }
        self.synchronize_context(app_loop)?;
        self.synchronize_devices(app_loop)?;
        for _ in 0..EVENTS_PER_TICK {
            let Some(event) = self
                .backend
                .as_mut()
                .and_then(|backend| backend.events.poll_event())
            else {
                break;
            };
            match event {
                // SDL recenters buttons before queuing device removal. Check
                // liveness after pumping so that synthetic release cannot turn
                // an unplugged Edit hold into a tap/confirmation.
                Event::GamepadButtonDown { which, .. }
                | Event::GamepadButtonUp { which, .. }
                | Event::GamepadAxisMotion { which, .. }
                    if !self
                        .backend
                        .as_ref()
                        .is_some_and(|backend| backend.connected(which)) =>
                {
                    self.synchronize_devices(app_loop)?;
                }
                Event::GamepadAdded { which, .. } => {
                    if let Some(backend) = self.backend.as_mut() {
                        if let Err(failure) = backend.open(which) {
                            self.backend_failure = Some(failure);
                            self.publish_failure(app_loop)?;
                            break;
                        }
                    }
                    self.synchronize_devices(app_loop)?;
                }
                Event::GamepadRemoved { which, .. } => {
                    route(
                        HostControllerInput::Disconnected {
                            device: which.raw() as usize,
                        },
                        app_loop.state().controller().bindings(),
                    );
                    if let Some(backend) = self.backend.as_mut() {
                        backend.devices.remove(&which);
                    }
                    self.synchronize_devices(app_loop)?;
                }
                Event::GamepadRemapped { which, .. } => {
                    let id = which.raw() as usize;
                    route(
                        HostControllerInput::Disconnected { device: id },
                        app_loop.state().controller().bindings(),
                    );
                    if self.inputs.devices.contains_key(&id) {
                        self.inputs.disconnect(app_loop, id)?;
                        if app_loop.state().controller().capture().is_some() {
                            dispatch_action(
                                app_loop,
                                SemanticAction::Return,
                                false,
                                EventSource::System,
                            )?;
                        }
                        self.synchronize_devices(app_loop)?;
                    }
                }
                Event::GamepadButtonDown {
                    which,
                    button: native,
                    ..
                } => {
                    if let Some(button) = button(native) {
                        if route(
                            HostControllerInput::Pressed {
                                device: which.raw() as usize,
                                button,
                            },
                            app_loop.state().controller().bindings(),
                        ) {
                            continue;
                        }
                        self.inputs.press(
                            app_loop,
                            which.raw() as usize,
                            button,
                            persisted_edits_blocked,
                        )?;
                    }
                }
                Event::GamepadButtonUp {
                    which,
                    button: native,
                    ..
                } => {
                    if let Some(button) = button(native) {
                        if route(
                            HostControllerInput::Released {
                                device: which.raw() as usize,
                                button,
                            },
                            app_loop.state().controller().bindings(),
                        ) {
                            continue;
                        }
                        self.inputs.release(
                            app_loop,
                            which.raw() as usize,
                            button,
                            persisted_edits_blocked,
                        )?;
                    }
                }
                Event::GamepadAxisMotion {
                    which, axis, value, ..
                } => {
                    let id = which.raw() as usize;
                    if let Some(axis) = host_stick_axis(axis) {
                        route(
                            HostControllerInput::Axis {
                                device: id,
                                axis,
                                value,
                            },
                            app_loop.state().controller().bindings(),
                        );
                        continue;
                    }
                    if let Some((_, button)) = TRIGGERS.iter().find(|(trigger, _)| *trigger == axis)
                    {
                        if value >= TRIGGER_PRESS {
                            if route(
                                HostControllerInput::Pressed {
                                    device: id,
                                    button: *button,
                                },
                                app_loop.state().controller().bindings(),
                            ) {
                                continue;
                            }
                            self.inputs
                                .press(app_loop, id, *button, persisted_edits_blocked)?;
                        } else if value <= TRIGGER_RELEASE {
                            if route(
                                HostControllerInput::Released {
                                    device: id,
                                    button: *button,
                                },
                                app_loop.state().controller().bindings(),
                            ) {
                                continue;
                            }
                            self.inputs
                                .release(app_loop, id, *button, persisted_edits_blocked)?;
                        }
                    }
                }
                // SDL owns hats, layouts and hardware quirks. Unmapped controls
                // have no standalone action; stick semantics belong to hosts.
                _ => {}
            }
            // A press can enter/exit capture or change bindings. Apply its new
            // interpretation before the next queued physical edge in this tick.
            self.synchronize_context(app_loop)?;
        }
        let repeats: Vec<_> = self
            .inputs
            .devices
            .iter_mut()
            .filter_map(|(id, input)| input.repeat(elapsed).map(|action| (*id, action)))
            .collect();
        for (id, action) in repeats {
            self.inputs.action(
                app_loop,
                InputOrigin::Controller(id),
                action,
                persisted_edits_blocked,
            )?;
        }
        self.observe_preferences(app_loop.state().controller());
        Ok(())
    }

    /// Keyboard and controller holds share mode and preview arbitration while
    /// retaining their original event provenance at the canonical boundary.
    pub(crate) fn dispatch_keyboard_action<Boundary: ControlAudioBoundary>(
        &mut self,
        app: &mut AppLoop<Boundary>,
        action: SemanticAction,
        blocked: bool,
    ) -> Result<(), BoundaryFull> {
        self.inputs.keyboard_action(app, action, blocked)
    }

    /// Also called at owned shutdown, after the last UI input and before Drop
    /// joins the preference worker, so an immediate close cannot lose a reset.
    pub(crate) fn observe_preferences(&mut self, state: &ControllerState) {
        self.preferences
            .observe(state.preference_status(), state.bindings());
    }

    fn synchronize_context<Boundary: ControlAudioBoundary>(
        &mut self,
        app_loop: &mut AppLoop<Boundary>,
    ) -> Result<(), BoundaryFull> {
        let state = app_loop.state().controller();
        let context = (state.bindings().clone(), state.capture(), state.ready());
        if self.input_context.as_ref() == Some(&context) {
            return Ok(());
        }
        self.input_context = Some(context);
        self.inputs.quarantine(app_loop)
    }

    fn synchronize_devices<Boundary: ControlAudioBoundary>(
        &mut self,
        app_loop: &mut AppLoop<Boundary>,
    ) -> Result<(), BoundaryFull> {
        let Some(backend) = self.backend.as_mut() else {
            return Ok(());
        };
        backend.devices.retain(|_, gamepad| gamepad.connected());
        let mut connected = Vec::new();
        for (id, gamepad) in &backend.devices {
            let id = id.raw() as usize;
            self.inputs.devices.entry(id).or_insert_with(|| {
                let mut input = DeviceInput::default();
                for (native, canonical) in STANDARD_BUTTONS {
                    if gamepad.button(native) {
                        input.held.insert(canonical, Hold::Suppressed);
                    }
                }
                for (axis, canonical) in TRIGGERS {
                    if gamepad.axis(axis) > TRIGGER_RELEASE {
                        input.held.insert(canonical, Hold::Suppressed);
                    }
                }
                input
            });
            connected.push(ControllerDevice {
                id,
                name: device_name(gamepad.name().as_deref().unwrap_or_default()),
            });
        }
        connected.sort_by_key(|device| device.id);
        let disconnected: Vec<_> = self
            .inputs
            .devices
            .keys()
            .copied()
            .filter(|id| !connected.iter().any(|device| device.id == *id))
            .collect();
        for id in disconnected {
            self.inputs.disconnect(app_loop, id)?;
        }
        if connected != self.published_devices {
            dispatch_event(
                app_loop,
                ControllerEvent::DevicesChanged {
                    devices: connected.clone(),
                },
            )?;
            self.published_devices = connected;
        }
        Ok(())
    }

    fn publish_failure<Boundary: ControlAudioBoundary>(
        &mut self,
        app_loop: &mut AppLoop<Boundary>,
    ) -> Result<(), BoundaryFull> {
        if let Some(failure) = self.backend_failure.take() {
            let ids: Vec<_> = self.inputs.devices.keys().copied().collect();
            for id in ids {
                self.inputs.disconnect(app_loop, id)?;
            }
            self.backend = None;
            dispatch_event(app_loop, ControllerEvent::BackendFailed { failure })?;
            self.published_devices.clear();
        }
        Ok(())
    }
}

fn dispatch_event<Boundary: ControlAudioBoundary>(
    app_loop: &mut AppLoop<Boundary>,
    event: ControllerEvent,
) -> Result<(), BoundaryFull> {
    let source = match event {
        ControllerEvent::ButtonCaptured { .. } => EventSource::Controller,
        ControllerEvent::PreferencesLoaded { .. } | ControllerEvent::PreferencesSaved { .. } => {
            EventSource::Worker
        }
        _ => EventSource::System,
    };
    if let Ok(result) = app_loop.dispatch_from(AppEvent::Controller(event), source) {
        if let Some(error) = result.boundary_full() {
            return Err(error);
        }
    }
    Ok(())
}

fn dispatch_action<Boundary: ControlAudioBoundary>(
    app_loop: &mut AppLoop<Boundary>,
    action: SemanticAction,
    persisted_edits_blocked: bool,
    source: EventSource,
) -> Result<bool, BoundaryFull> {
    if persisted_edits_blocked && action.may_change_saved_session() {
        return Ok(false);
    }
    if let Ok(result) = app_loop.dispatch_action_from(action, source) {
        if let Some(error) = result.boundary_full() {
            return Err(error);
        }
        return Ok(true);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::control::{AppState, FocusPath, StateProjector, SurfaceId, TopLevelContext};
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::mixer_track_parameters::MixerTrackParameter;
    use crate::real_time::{AudioCommand, ParameterSnapshot};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct Probe(Arc<AtomicUsize>);
    impl ControlAudioBoundary for Probe {
        fn push_command(&mut self, _: AudioCommand) -> Result<(), BoundaryFull> {
            Ok(())
        }
        fn publish_parameters(&mut self, _: ParameterSnapshot) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn mixer_state() -> AppState {
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        AppState::new(
            provider.registry().unwrap(),
            GlobalParameters::new(0.0).unwrap(),
        )
    }

    fn sample_state() -> AppState {
        use crate::adapter::sample_capability::SampleCapability;
        use crate::kernel::{MidiChannel, PatchId};
        use crate::mixer::patch_output::PatchOutput;
        use crate::synth::{
            AssetFileId, CapabilityRegistry, FileBrowserFolderId, FileBrowserListing,
            FileBrowserRow, FileBrowserRowKind, InstrumentCapabilityProvider, Patch,
        };
        let provider = SampleCapability::new(AssetFileId::new("Factory.wav").unwrap()).unwrap();
        let folder = FileBrowserFolderId::default();
        let listing = FileBrowserListing::new(
            folder.clone(),
            vec![
                FileBrowserRow::new(
                    "file:Alternate.wav",
                    "Alternate.wav",
                    FileBrowserRowKind::File(AssetFileId::new("Alternate.wav").unwrap()),
                    Some(128),
                )
                .unwrap(),
                FileBrowserRow::new("cancel:", "Cancel", FileBrowserRowKind::Cancel, None).unwrap(),
            ],
        )
        .unwrap();
        let mut state = AppState::new(
            CapabilityRegistry::new(vec![provider.descriptor()]).unwrap(),
            GlobalParameters::new(0.0).unwrap(),
        )
        .with_sample_catalog([(folder, Ok(listing))]);
        state
            .apply(AppEvent::InstallPatches(vec![Patch::new(
                PatchId::new(1).unwrap(),
                "Sample".into(),
                provider.default_config().unwrap(),
                MidiChannel::new(0).unwrap(),
                PatchOutput::default(),
            )]))
            .unwrap();
        state
            .apply_semantic_action(SemanticAction::SelectContext(TopLevelContext::Patch))
            .unwrap();
        state
    }

    fn inputs(mut state: AppState) -> (AppLoop<Probe>, GamepadInputs, Arc<AtomicUsize>) {
        state
            .apply(AppEvent::Controller(ControllerEvent::PreferencesLoaded {
                result: Ok(None),
            }))
            .unwrap();
        state
            .apply(AppEvent::Controller(ControllerEvent::DevicesChanged {
                devices: vec![
                    ControllerDevice {
                        id: 1,
                        name: "First".into(),
                    },
                    ControllerDevice {
                        id: 2,
                        name: "Second".into(),
                    },
                ],
            }))
            .unwrap();
        let snapshots = Arc::new(AtomicUsize::new(0));
        let app = AppLoop::new(state, StateProjector::new(), Probe(snapshots.clone())).unwrap();
        let inputs = GamepadInputs {
            devices: [(1, DeviceInput::default()), (2, DeviceInput::default())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        (app, inputs, snapshots)
    }

    #[test]
    fn hosted_keyboard_and_gamepad_share_gestures_without_mutating_synth() {
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;
        let (mut app, mut input, snapshots) = inputs(mixer_state());
        let generation = app.state().generation();
        let count = snapshots.load(Ordering::Relaxed);
        let actions = Rc::new(RefCell::new(Vec::new()));
        let active = Rc::new(Cell::new(true));
        let captured = actions.clone();
        let enabled = active.clone();
        input.host_action = Some(Rc::new(move |action| {
            if !enabled.get() {
                return false;
            }
            captured.borrow_mut().push(action);
            true
        }));
        input
            .press(&mut app, 1, ControllerButton::East, false)
            .unwrap();
        input
            .press(&mut app, 1, ControllerButton::DPadRight, false)
            .unwrap();
        input
            .press(&mut app, 1, ControllerButton::DPadRight, false)
            .unwrap();
        input
            .release(&mut app, 1, ControllerButton::DPadRight, false)
            .unwrap();
        input
            .release(&mut app, 1, ControllerButton::East, false)
            .unwrap();
        assert_eq!(
            &*actions.borrow(),
            &[SemanticAction::NavigatePage(Direction::Right)]
        );
        actions.borrow_mut().clear();
        input
            .press(&mut app, 1, ControllerButton::South, false)
            .unwrap();
        input
            .keyboard_action(
                &mut app,
                SemanticAction::SetInteractionMode(InteractionMode::Adjust),
                false,
            )
            .unwrap();
        input
            .press(&mut app, 1, ControllerButton::DPadUp, false)
            .unwrap();
        input
            .release(&mut app, 1, ControllerButton::South, false)
            .unwrap();
        assert!(!actions
            .borrow()
            .contains(&SemanticAction::SetInteractionMode(
                InteractionMode::Navigate
            )));
        input
            .keyboard_action(
                &mut app,
                SemanticAction::SetInteractionMode(InteractionMode::Navigate),
                false,
            )
            .unwrap();
        assert_eq!(
            actions.borrow().last(),
            Some(&SemanticAction::SetInteractionMode(
                InteractionMode::Navigate
            ))
        );
        assert!(actions
            .borrow()
            .contains(&SemanticAction::Adjust(Direction::Up)));
        assert_eq!(app.state().generation(), generation);
        assert_eq!(snapshots.load(Ordering::Relaxed), count);
        input.quarantine(&mut app).unwrap();
        active.set(false);
        input
            .press(&mut app, 1, ControllerButton::DPadDown, false)
            .unwrap();
        assert!(app.state().generation() > generation);
    }

    #[test]
    fn physical_edges_reach_production_reducer_projection_and_audio_boundary_once() {
        let (mut app, mut input, snapshots) = inputs(mixer_state());
        let initial_snapshots = snapshots.load(Ordering::Relaxed);
        input
            .press(&mut app, 1, ControllerButton::DPadDown, false)
            .unwrap();
        input
            .release(&mut app, 1, ControllerButton::DPadDown, false)
            .unwrap();
        assert_eq!(
            app.current_semantic_model().focus_path(),
            &FocusPath::mixer_track(MixerTrackId::default(), MixerTrackParameter::Pan)
        );
        assert_eq!(snapshots.load(Ordering::Relaxed), initial_snapshots);
        let original_pan = app.state().mixer().track(MixerTrackId::default()).pan();
        input
            .press(&mut app, 1, ControllerButton::South, false)
            .unwrap();
        assert_eq!(app.state().interaction().mode(), InteractionMode::Adjust);
        input
            .press(&mut app, 1, ControllerButton::DPadRight, true)
            .unwrap();
        assert_eq!(
            app.state().mixer().track(MixerTrackId::default()).pan(),
            original_pan
        );
        input
            .release(&mut app, 1, ControllerButton::DPadRight, false)
            .unwrap();
        input
            .press(&mut app, 1, ControllerButton::DPadRight, false)
            .unwrap();
        assert!(app.state().mixer().track(MixerTrackId::default()).pan() > original_pan);
        assert_eq!(snapshots.load(Ordering::Relaxed), initial_snapshots + 1);
        let edited_generation = app.state().generation();
        input
            .press(&mut app, 1, ControllerButton::DPadRight, false)
            .unwrap();
        assert_eq!(app.state().generation(), edited_generation);
        input
            .release(&mut app, 1, ControllerButton::South, false)
            .unwrap();
        assert_eq!(app.state().interaction().mode(), InteractionMode::Navigate);
        let released_generation = app.state().generation();
        input
            .release(&mut app, 1, ControllerButton::DPadRight, false)
            .unwrap();
        assert_eq!(app.state().generation(), released_generation);
        assert!(app
            .event_log_ref()
            .records()
            .iter()
            .all(|record| record.source() == EventSource::Controller));
    }

    #[test]
    fn held_edits_remain_device_local_and_last_disconnect_or_quarantine_cleans_mode() {
        let (mut app, mut input, _) = inputs(mixer_state());
        input
            .press(&mut app, 1, ControllerButton::South, false)
            .unwrap();
        input
            .press(&mut app, 2, ControllerButton::South, false)
            .unwrap();
        input.disconnect(&mut app, 1).unwrap();
        assert_eq!(app.state().interaction().mode(), InteractionMode::Adjust);
        input.disconnect(&mut app, 2).unwrap();
        assert_eq!(app.state().interaction().mode(), InteractionMode::Navigate);
        input.devices.insert(1, DeviceInput::default());
        input.devices.insert(2, DeviceInput::default());
        input
            .press(&mut app, 1, ControllerButton::South, false)
            .unwrap();
        input
            .press(&mut app, 2, ControllerButton::DPadDown, false)
            .unwrap();
        assert_eq!(
            app.current_semantic_model().focus_path(),
            &FocusPath::mixer_track(MixerTrackId::default(), MixerTrackParameter::Pan)
        );
        assert_eq!(app.state().interaction().mode(), InteractionMode::Adjust);
        input.quarantine(&mut app).unwrap();
        assert_eq!(app.state().interaction().mode(), InteractionMode::Navigate);
        app.dispatch_action(SemanticAction::SetInteractionMode(InteractionMode::Adjust))
            .unwrap();
        input.quarantine(&mut app).unwrap();
        assert_eq!(
            app.state().interaction().mode(),
            InteractionMode::Adjust,
            "no owned controller mode must preserve keyboard mode"
        );
    }

    #[test]
    fn edit_tap_enters_settings_capture_and_choice_chords_keep_modal_after_release() {
        let (mut app, mut input, _) = inputs(mixer_state());
        input
            .press(&mut app, 1, ControllerButton::Start, false)
            .unwrap();
        input
            .press(&mut app, 1, ControllerButton::DPadRight, false)
            .unwrap();
        assert_eq!(
            app.state().interaction().active_surface(),
            SurfaceId::ControllerSettings
        );
        input
            .release(&mut app, 1, ControllerButton::DPadRight, false)
            .unwrap();
        input
            .press(&mut app, 1, ControllerButton::South, false)
            .unwrap();
        input
            .release(&mut app, 1, ControllerButton::South, false)
            .unwrap();
        assert_eq!(app.state().controller().capture(), Some(ControllerRole::Up));
        assert_eq!(app.state().interaction().mode(), InteractionMode::Navigate);

        for chord in [false, true] {
            let (mut app, mut input, _) = inputs(sample_state());
            input
                .press(&mut app, 1, ControllerButton::South, false)
                .unwrap();
            if chord {
                input
                    .press(&mut app, 1, ControllerButton::West, false)
                    .unwrap();
            }
            input
                .release(&mut app, 1, ControllerButton::South, false)
                .unwrap();
            if !chord {
                assert_eq!(
                    app.state().interaction().active_surface(),
                    SurfaceId::PatchDetail
                );
                assert_eq!(app.state().interaction().mode(), InteractionMode::Navigate);
                input
                    .press(&mut app, 1, ControllerButton::South, false)
                    .unwrap();
                input
                    .release(&mut app, 1, ControllerButton::South, false)
                    .unwrap();
            }
            assert_eq!(
                app.state().interaction().active_surface(),
                if chord {
                    SurfaceId::PatchChoice
                } else {
                    SurfaceId::FileBrowser
                }
            );
            assert_eq!(app.state().interaction().mode(), InteractionMode::Modal);
            input
                .release(&mut app, 1, ControllerButton::West, false)
                .unwrap();
            input.quarantine(&mut app).unwrap();
            assert_eq!(app.state().interaction().mode(), InteractionMode::Modal);
        }
    }

    #[test]
    fn rejected_controller_preview_never_stops_another_controller_or_keyboard_request() {
        for keyboard_owner in [false, true] {
            for disconnect in [false, true] {
                let mut state = sample_state();
                state
                    .apply_semantic_action(SemanticAction::OpenRelated)
                    .unwrap();
                state
                    .apply_semantic_action(SemanticAction::OpenRelated)
                    .unwrap();
                let (mut app, mut input, _) = inputs(state);
                if keyboard_owner {
                    input
                        .keyboard_action(&mut app, SemanticAction::PreviewStart, false)
                        .unwrap();
                } else {
                    input
                        .press(&mut app, 1, ControllerButton::North, false)
                        .unwrap();
                }
                assert!(app.state().file_browser().preview_is_held());
                let request = app.state().file_browser().preview_request_id();
                input
                    .press(&mut app, 2, ControllerButton::North, false)
                    .unwrap();
                assert_eq!(
                    input.devices[&2].preview_request, None,
                    "rejected starts cannot own a request"
                );
                if disconnect {
                    input.disconnect(&mut app, 2).unwrap();
                } else {
                    input
                        .release(&mut app, 2, ControllerButton::North, false)
                        .unwrap();
                }
                assert!(app.state().file_browser().preview_is_held());
                assert_eq!(app.state().file_browser().preview_request_id(), request);
                if !keyboard_owner {
                    input
                        .release(&mut app, 1, ControllerButton::North, false)
                        .unwrap();
                    assert!(!app.state().file_browser().preview_is_held());
                } else {
                    input
                        .keyboard_action(&mut app, SemanticAction::PreviewStop, false)
                        .unwrap();
                    assert!(!app.state().file_browser().preview_is_held());
                }
            }
        }
    }

    #[test]
    fn keyboard_and_controller_edit_holds_survive_both_press_and_release_orders() {
        for keyboard_first in [false, true] {
            for keyboard_release_first in [false, true] {
                for disconnect in [false, true] {
                    let (mut app, mut input, _) = inputs(mixer_state());
                    for keyboard in [keyboard_first, !keyboard_first] {
                        if keyboard {
                            input
                                .keyboard_action(
                                    &mut app,
                                    SemanticAction::SetInteractionMode(InteractionMode::Adjust),
                                    false,
                                )
                                .unwrap();
                        } else {
                            input
                                .press(&mut app, 1, ControllerButton::South, false)
                                .unwrap();
                        }
                    }
                    assert_eq!(app.state().interaction().mode(), InteractionMode::Adjust);
                    assert_eq!(
                        app.event_log_ref().records().front().unwrap().source(),
                        if keyboard_first {
                            EventSource::Keyboard
                        } else {
                            EventSource::Controller
                        }
                    );
                    // Make Edit a chord so release cannot also activate a control.
                    input
                        .press(&mut app, 1, ControllerButton::DPadRight, false)
                        .unwrap();
                    input
                        .release(&mut app, 1, ControllerButton::DPadRight, false)
                        .unwrap();
                    let generation = app.state().generation();
                    for (index, keyboard) in [keyboard_release_first, !keyboard_release_first]
                        .into_iter()
                        .enumerate()
                    {
                        if keyboard {
                            input
                                .keyboard_action(
                                    &mut app,
                                    SemanticAction::SetInteractionMode(InteractionMode::Navigate),
                                    false,
                                )
                                .unwrap();
                        } else if disconnect {
                            input.disconnect(&mut app, 1).unwrap();
                        } else {
                            input
                                .release(&mut app, 1, ControllerButton::South, false)
                                .unwrap();
                        }
                        if index == 0 {
                            assert_eq!(app.state().interaction().mode(), InteractionMode::Adjust);
                            assert_eq!(
                                app.state().generation(),
                                generation,
                                "first release must not reset or redundantly rewrite mode"
                            );
                        } else {
                            assert_eq!(app.state().interaction().mode(), InteractionMode::Navigate);
                            assert_eq!(
                                app.event_log_ref().records().back().unwrap().source(),
                                if keyboard {
                                    EventSource::Keyboard
                                } else {
                                    EventSource::Controller
                                }
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn controller_navigation_preserves_keyboard_edit_and_both_releases_preserve_modal() {
        let (mut app, mut input, _) = inputs(mixer_state());
        input
            .keyboard_action(
                &mut app,
                SemanticAction::SetInteractionMode(InteractionMode::Adjust),
                false,
            )
            .unwrap();
        let generation = app.state().generation();
        input
            .keyboard_action(&mut app, SemanticAction::OpenMidiSettings, false)
            .unwrap();
        assert_eq!(
            app.state().generation(),
            generation,
            "keyboard Settings admission while K is held remains unchanged"
        );
        assert_eq!(app.state().interaction().mode(), InteractionMode::Adjust);
        input
            .press(&mut app, 1, ControllerButton::DPadDown, false)
            .unwrap();
        assert_eq!(
            app.current_semantic_model().focus_path(),
            &FocusPath::mixer_track(MixerTrackId::default(), MixerTrackParameter::Pan)
        );
        assert_eq!(app.state().interaction().mode(), InteractionMode::Adjust);
        input.quarantine(&mut app).unwrap();
        assert_eq!(app.state().interaction().mode(), InteractionMode::Adjust);
        input
            .keyboard_action(
                &mut app,
                SemanticAction::SetInteractionMode(InteractionMode::Navigate),
                false,
            )
            .unwrap();
        assert_eq!(app.state().interaction().mode(), InteractionMode::Navigate);

        let (mut app, mut input, _) = inputs(sample_state());
        input
            .keyboard_action(
                &mut app,
                SemanticAction::SetInteractionMode(InteractionMode::Adjust),
                false,
            )
            .unwrap();
        input
            .press(&mut app, 1, ControllerButton::South, false)
            .unwrap();
        input
            .press(&mut app, 1, ControllerButton::West, false)
            .unwrap();
        assert_eq!(app.state().interaction().mode(), InteractionMode::Modal);
        input
            .keyboard_action(
                &mut app,
                SemanticAction::SetInteractionMode(InteractionMode::Navigate),
                false,
            )
            .unwrap();
        input
            .release(&mut app, 1, ControllerButton::South, false)
            .unwrap();
        assert_eq!(
            app.state().interaction().active_surface(),
            SurfaceId::PatchChoice
        );
        assert_eq!(app.state().interaction().mode(), InteractionMode::Modal);
    }

    #[test]
    fn rejected_keyboard_preview_release_or_focus_loss_cannot_stop_controller_preview() {
        use crate::shell::{KeyboardInputTranslator, WindowInput, WindowKey};
        for focus_loss in [false, true] {
            let mut state = sample_state();
            state
                .apply_semantic_action(SemanticAction::OpenRelated)
                .unwrap();
            state
                .apply_semantic_action(SemanticAction::OpenRelated)
                .unwrap();
            let (mut app, mut input, _) = inputs(state);
            input
                .press(&mut app, 1, ControllerButton::North, false)
                .unwrap();
            let owned = app.state().file_browser().preview_request_id();
            assert!(owned.is_some());
            let mut keyboard = KeyboardInputTranslator::new();
            let start = keyboard
                .translate(WindowInput::key_down(WindowKey::Space))
                .unwrap();
            input.keyboard_action(&mut app, start, false).unwrap();
            assert_eq!(input.keyboard_preview_request, None);
            assert_eq!(
                app.event_log_ref().records().back().unwrap().source(),
                EventSource::Keyboard
            );
            let release = keyboard
                .translate(if focus_loss {
                    WindowInput::focus_lost()
                } else {
                    WindowInput::key_up(WindowKey::Space)
                })
                .unwrap();
            input.keyboard_action(&mut app, release, false).unwrap();
            assert!(app.state().file_browser().preview_is_held());
            assert_eq!(app.state().file_browser().preview_request_id(), owned);
            input.disconnect(&mut app, 1).unwrap();
            assert!(!app.state().file_browser().preview_is_held());
        }
    }

    fn press(input: &mut DeviceInput, button: ControllerButton) -> Option<SemanticAction> {
        match input.press(button, &ControllerBindings::default(), false, true) {
            Some(InputOutcome::Action(action)) => Some(action),
            None => None,
            other => panic!("unexpected input {other:?}"),
        }
    }

    #[test]
    fn held_directions_repeat_from_elapsed_events_and_stop_on_release_or_disconnect() {
        let mut input = DeviceInput::default();
        assert_eq!(
            press(&mut input, ControllerButton::DPadRight),
            Some(SemanticAction::Navigate(Direction::Right))
        );
        assert_eq!(input.repeat(Duration::from_millis(279)), None);
        assert_eq!(
            input.repeat(Duration::from_millis(1)),
            Some(SemanticAction::Navigate(Direction::Right))
        );
        press(&mut input, ControllerButton::South);
        assert_eq!(
            input.repeat(Duration::from_millis(45)),
            Some(SemanticAction::Adjust(Direction::Right))
        );
        input.release(ControllerButton::DPadRight);
        assert_eq!(input.repeat(Duration::from_secs(1)), None);
        assert_eq!(
            input.release(ControllerButton::South),
            None,
            "repeat consumes Edit tap"
        );
        press(&mut input, ControllerButton::East);
        assert_eq!(
            press(&mut input, ControllerButton::DPadUp),
            Some(SemanticAction::NavigatePage(Direction::Up))
        );
        assert_eq!(
            input.repeat(Duration::from_secs(1)),
            None,
            "page transitions never repeat"
        );
        input.disconnect();
        press(&mut input, ControllerButton::DPadLeft);
        input.quarantine();
        assert_eq!(input.repeat(Duration::from_secs(1)), None);
        input.disconnect();
        assert_eq!(input.repeat(Duration::from_secs(1)), None);
    }

    #[test]
    fn edit_taps_confirm_on_release_and_direction_chords_never_confirm() {
        let mut input = DeviceInput::default();
        assert_eq!(press(&mut input, ControllerButton::South), None);
        assert_eq!(press(&mut input, ControllerButton::South), None);
        assert_eq!(
            input.release(ControllerButton::South),
            Some(SemanticAction::Activate)
        );
        assert_eq!(input.release(ControllerButton::South), None);
        assert_eq!(press(&mut input, ControllerButton::South), None);
        assert_eq!(
            press(&mut input, ControllerButton::DPadRight),
            Some(SemanticAction::Adjust(Direction::Right))
        );
        assert_eq!(input.release(ControllerButton::South), None);
        assert_eq!(press(&mut input, ControllerButton::DPadRight), None);
        assert_eq!(input.release(ControllerButton::DPadRight), None);
        assert_eq!(
            press(&mut input, ControllerButton::DPadRight),
            Some(SemanticAction::Navigate(Direction::Right))
        );
    }

    #[test]
    fn shift_page_edge_keeps_original_gesture_until_direction_release() {
        let mut input = DeviceInput::default();
        press(&mut input, ControllerButton::East);
        assert_eq!(
            press(&mut input, ControllerButton::DPadUp),
            Some(SemanticAction::NavigatePage(Direction::Up))
        );
        input.release(ControllerButton::East);
        assert_eq!(press(&mut input, ControllerButton::DPadUp), None);
        input.release(ControllerButton::DPadUp);
        press(&mut input, ControllerButton::East);
        assert_eq!(
            press(&mut input, ControllerButton::DPadUp),
            Some(SemanticAction::NavigatePage(Direction::Up))
        );
    }

    #[test]
    fn hosted_start_stays_transport_with_shift_held() {
        for hosted in [false, true] {
            let mut input = DeviceInput {
                host_navigation: hosted,
                ..DeviceInput::default()
            };
            press(&mut input, ControllerButton::East);
            assert_eq!(
                press(&mut input, ControllerButton::North),
                Some(if hosted {
                    SemanticAction::PreviewStart
                } else {
                    SemanticAction::OpenMidiSettings
                })
            );
            assert_eq!(press(&mut input, ControllerButton::North), None);
            assert_eq!(
                input.release(ControllerButton::North),
                hosted.then_some(SemanticAction::PreviewStop)
            );
        }
    }

    #[test]
    fn modifiers_are_device_local_and_disconnect_clears_preview_and_holds() {
        let mut first = DeviceInput::default();
        let mut second = DeviceInput::default();
        press(&mut first, ControllerButton::East);
        assert_eq!(
            press(&mut second, ControllerButton::DPadLeft),
            Some(SemanticAction::Navigate(Direction::Left))
        );
        assert_eq!(
            press(&mut second, ControllerButton::North),
            Some(SemanticAction::PreviewStart)
        );
        assert_eq!(second.disconnect(), Some(SemanticAction::PreviewStop));
        assert_eq!(second.disconnect(), None);
        assert_eq!(
            press(&mut second, ControllerButton::DPadLeft),
            Some(SemanticAction::Navigate(Direction::Left))
        );
        assert_eq!(
            press(&mut second, ControllerButton::North),
            Some(SemanticAction::PreviewStart)
        );
    }

    #[test]
    fn shift_preview_opens_settings_without_a_preview_stop_when_shift_releases_first() {
        let mut input = DeviceInput::default();
        press(&mut input, ControllerButton::East);
        assert_eq!(
            press(&mut input, ControllerButton::North),
            Some(SemanticAction::OpenMidiSettings)
        );
        input.release(ControllerButton::East);
        assert_eq!(input.release(ControllerButton::North), None);
        assert_eq!(
            press(&mut input, ControllerButton::North),
            Some(SemanticAction::PreviewStart)
        );
        assert_eq!(
            input.release(ControllerButton::North),
            Some(SemanticAction::PreviewStop)
        );
    }

    #[test]
    fn capture_quarantines_held_controls_and_captured_button_until_release() {
        let mut input = DeviceInput::default();
        press(&mut input, ControllerButton::North);
        press(&mut input, ControllerButton::East);
        assert_eq!(input.quarantine(), Some(SemanticAction::PreviewStop));
        let bindings = ControllerBindings::default();
        assert_eq!(
            input.press(ControllerButton::East, &bindings, true, true),
            None
        );
        assert_eq!(
            input.press(ControllerButton::West, &bindings, true, true),
            Some(InputOutcome::Capture(ControllerButton::West))
        );
        assert_eq!(
            input.press(ControllerButton::West, &bindings, true, true),
            None
        );
        input.quarantine();
        assert_eq!(press(&mut input, ControllerButton::West), None);
        assert_eq!(input.release(ControllerButton::West), None);
        assert_eq!(
            press(&mut input, ControllerButton::DPadUp),
            Some(SemanticAction::Navigate(Direction::Up))
        );
        assert_eq!(input.release(ControllerButton::North), None);
    }

    #[test]
    fn bindings_change_cannot_turn_an_old_hold_into_an_activation() {
        let mut input = DeviceInput::default();
        press(&mut input, ControllerButton::South);
        input.quarantine();
        assert_eq!(input.release(ControllerButton::South), None);
        assert_eq!(press(&mut input, ControllerButton::South), None);
        assert_eq!(
            input.release(ControllerButton::South),
            Some(SemanticAction::Activate)
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn sdl_gamepad_startup_preserves_terminal_signals() {
        use std::os::unix::process::ExitStatusExt;
        const CHILD_SIGNAL: &str = "CREST_GAMEPAD_SIGNAL_TEST";
        if let Ok(signal) = std::env::var(CHILD_SIGNAL) {
            let signal = signal.parse::<i32>().unwrap();
            assert!(matches!(signal, libc::SIGINT | libc::SIGTERM));
            // This subprocess owns its signal dispositions and SDL lifetime.
            unsafe { libc::signal(signal, libc::SIG_DFL) };
            let _backend = SdlGamepads::new().expect("SDL3 Gamepad API must initialize");
            unsafe { libc::raise(signal) };
            panic!("SDL swallowed terminal signal {signal}");
        }
        for signal in [libc::SIGINT, libc::SIGTERM] {
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "shell::gamepad::tests::sdl_gamepad_startup_preserves_terminal_signals",
                    "--nocapture",
                ])
                .env(CHILD_SIGNAL, signal.to_string())
                .env_remove("SDL_NO_SIGNAL_HANDLERS")
                .output()
                .unwrap();
            assert_eq!(
                child.status.signal(),
                Some(signal),
                "signal {signal}: {}",
                String::from_utf8_lossy(&child.stderr)
            );
        }
    }

    // One native SDL witness owns the process-wide event pump. Keep all virtual
    // scenarios together so libtest cannot initialize SDL on competing threads.
    #[test]
    fn sdl_gamepads_reach_reducer_and_preserve_input_lifetimes() {
        use sdl3::joystick::{JoystickType, VirtualJoystickDescription};
        use std::time::{Duration, Instant};

        fn tick(runtime: &mut GamepadRuntime, app: &mut AppLoop<Probe>) {
            // SDL's poll sentinel can end one batch before the next pump. This
            // also verifies duplicate discovery events cannot reset held input.
            for _ in 0..4 {
                runtime.advance(app, false).unwrap();
            }
        }
        fn capture(runtime: &mut GamepadRuntime, app: &mut AppLoop<Probe>) {
            for action in [
                SemanticAction::OpenMidiSettings,
                SemanticAction::Navigate(Direction::Right),
                SemanticAction::Activate,
            ] {
                app.dispatch_action_from(action, EventSource::Keyboard)
                    .unwrap();
            }
            tick(runtime, app);
            assert_eq!(app.state().controller().capture(), Some(ControllerRole::Up));
        }
        let directory = std::env::temp_dir().join(format!(
            "crest-sdl-gamepad-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("controller-buttons.json");
        let original = serde_json::to_vec_pretty(&ControllerBindings::default()).unwrap();
        std::fs::write(&path, &original).unwrap();
        let backend = SdlGamepads::new().expect("SDL3 Gamepad API must initialize");
        let joysticks = backend._sdl.joystick().unwrap();
        let events = backend._sdl.event().unwrap();
        let attach = || {
            joysticks
                .attach_virtual_joystick(
                    VirtualJoystickDescription::new()
                        .name("Crest SDL3 virtual gamepad")
                        .joystick_type(JoystickType::Gamepad)
                        // SDL packs selected controls. Declare the complete contiguous
                        // set so virtual raw indices match the standard enum indices.
                        .with_buttons(STANDARD_BUTTONS.map(|(native, _)| native))
                        .with_axes([
                            Axis::LeftX,
                            Axis::LeftY,
                            Axis::RightX,
                            Axis::RightY,
                            Axis::TriggerLeft,
                            Axis::TriggerRight,
                        ]),
                )
                .unwrap()
        };
        let mut runtime = GamepadRuntime {
            backend: Some(backend),
            backend_failure: None,
            preferences: ControllerPreferenceWorker::with_directory(Some(directory.clone())),
            inputs: GamepadInputs::default(),
            published_devices: Vec::new(),
            input_context: None,
            host_has_input: false,
        };
        let snapshots = Arc::new(AtomicUsize::new(0));
        let mut app = AppLoop::new(
            mixer_state(),
            StateProjector::new(),
            Probe(snapshots.clone()),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !app.state().controller().ready() {
            tick(&mut runtime, &mut app);
            assert!(Instant::now() < deadline, "preference load timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            std::fs::read(&path).unwrap(),
            original,
            "migration must not rewrite preferences"
        );
        let connection = attach();
        let id = connection.id();
        let key = id.raw() as usize;
        let pad = joysticks.open(id).unwrap();
        tick(&mut runtime, &mut app);
        assert!(app
            .state()
            .controller()
            .devices()
            .iter()
            .any(|device| device.id == key));
        let initial_snapshots = snapshots.load(Ordering::Relaxed);
        for (native, track, parameter) in [
            (Button::DPadDown, 0, MixerTrackParameter::Pan),
            (Button::DPadUp, 0, MixerTrackParameter::Level),
            (Button::DPadRight, 1, MixerTrackParameter::Level),
            (Button::DPadLeft, 0, MixerTrackParameter::Level),
        ] {
            pad.set_virtual_button(native as u32, true).unwrap();
            joysticks.update();
            tick(&mut runtime, &mut app);
            assert_eq!(
                app.current_semantic_model().focus_path(),
                &FocusPath::mixer_track(MixerTrackId::new(track).unwrap(), parameter)
            );
            assert_eq!(
                app.event_log_ref().records().back().unwrap().source(),
                EventSource::Controller
            );
            let generation = app.state().generation();
            for event in [
                Event::GamepadAdded {
                    timestamp: 0,
                    which: id,
                },
                Event::GamepadButtonDown {
                    timestamp: 0,
                    which: id,
                    button: native,
                },
            ] {
                events.push_event(event).unwrap();
            }
            tick(&mut runtime, &mut app);
            assert_eq!(
                app.state().generation(),
                generation,
                "held input cannot repeat"
            );
            pad.set_virtual_button(native as u32, false).unwrap();
            tick(&mut runtime, &mut app);
            assert!(!runtime.inputs.devices[&key]
                .held
                .contains_key(&button(native).unwrap()));
            assert_eq!(
                app.state().generation(),
                generation,
                "release cannot navigate"
            );
        }
        // A complete tap queued between UI ticks must not disappear into a
        // current-state snapshot. Analog stick motion must not move focus.
        pad.set_virtual_button(Button::DPadRight as u32, true)
            .unwrap();
        joysticks.update();
        pad.set_virtual_button(Button::DPadRight as u32, false)
            .unwrap();
        joysticks.update();
        tick(&mut runtime, &mut app);
        assert_eq!(
            app.current_semantic_model().focus_path(),
            &FocusPath::mixer_track(MixerTrackId::new(1).unwrap(), MixerTrackParameter::Level)
        );
        let generation = app.state().generation();
        pad.set_virtual_axis(Axis::LeftX as u32, i16::MAX).unwrap();
        tick(&mut runtime, &mut app);
        assert_eq!(app.state().generation(), generation);
        assert_eq!(snapshots.load(Ordering::Relaxed), initial_snapshots);

        // Hosts receive canonical stick axes through the same SDL pump. Their
        // consumption never changes standalone reducer state or audio scalars.
        let mut axes = Vec::new();
        for (native, canonical) in [
            (Axis::LeftX, HostControllerAxis::LeftX),
            (Axis::LeftY, HostControllerAxis::LeftY),
            (Axis::RightX, HostControllerAxis::RightX),
            (Axis::RightY, HostControllerAxis::RightY),
        ] {
            pad.set_virtual_axis(native as u32, -20_000).unwrap();
            joysticks.update();
            for _ in 0..4 {
                runtime
                    .advance_hosted(&mut app, false, true, Duration::ZERO, &mut |input, _| {
                        if let HostControllerInput::Axis { axis, value, .. } = input {
                            axes.push((axis, value));
                        }
                        true
                    })
                    .unwrap();
            }
            assert!(axes.contains(&(canonical, -20_000)));
            pad.set_virtual_axis(native as u32, 0).unwrap();
            joysticks.update();
        }
        tick(&mut runtime, &mut app);
        assert_eq!(snapshots.load(Ordering::Relaxed), initial_snapshots);

        // Exercise the hosted production pump with physical face-button edges,
        // including a workspace change while the same Shift remains held.
        {
            use std::cell::RefCell;
            use std::rc::Rc;
            let actions = Rc::new(RefCell::new(Vec::new()));
            let captured = actions.clone();
            runtime.set_host_action_handler(Some(Rc::new(move |action| {
                captured.borrow_mut().push(action);
                true
            })));
            let hosted_tick = |runtime: &mut GamepadRuntime, app: &mut AppLoop<Probe>, owns| {
                for _ in 0..4 {
                    runtime
                        .advance_hosted(app, false, owns, Duration::ZERO, &mut |_, _| false)
                        .unwrap();
                }
            };
            hosted_tick(&mut runtime, &mut app, true);
            for (native, down, owns) in [
                (Button::South, true, true),
                (Button::DPadRight, true, true),
                (Button::DPadRight, false, true),
                (Button::South, false, true),
                (Button::East, true, true),
                (Button::DPadRight, true, true),
                (Button::DPadRight, false, true),
                // Tracker has yielded to Synth; no second Shift press.
                (Button::DPadUp, true, false),
                (Button::DPadUp, false, false),
                // Back to Tracker with the same physical hold.
                (Button::DPadRight, true, true),
                (Button::DPadRight, false, true),
                (Button::East, false, true),
                (Button::North, true, true),
                (Button::North, false, true),
                (Button::DPadDown, true, true),
                (Button::DPadDown, false, true),
            ] {
                pad.set_virtual_button(native as u32, down).unwrap();
                joysticks.update();
                hosted_tick(&mut runtime, &mut app, owns);
            }
            assert_eq!(
                &*actions.borrow(),
                &[
                    SemanticAction::SetInteractionMode(InteractionMode::Adjust),
                    SemanticAction::Adjust(Direction::Right),
                    SemanticAction::SetInteractionMode(InteractionMode::Navigate),
                    SemanticAction::NavigatePage(Direction::Right),
                    SemanticAction::NavigatePage(Direction::Up),
                    SemanticAction::NavigatePage(Direction::Right),
                    SemanticAction::PreviewStart,
                    SemanticAction::PreviewStop,
                    SemanticAction::Navigate(Direction::Down),
                ]
            );
            runtime.set_host_action_handler(None);
            tick(&mut runtime, &mut app);
            assert_eq!(snapshots.load(Ordering::Relaxed), initial_snapshots);
        }

        // Trigger thresholds are evaluated after SDL normalizes raw axes.
        // Capture maps the trigger through the same production reducer path.
        capture(&mut runtime, &mut app);
        for (axis, expected) in TRIGGERS {
            pad.set_virtual_axis(axis as u32, i16::MIN).unwrap();
            tick(&mut runtime, &mut app);
            pad.set_virtual_axis(axis as u32, i16::MAX).unwrap();
            tick(&mut runtime, &mut app);
            assert!(runtime.inputs.devices[&key].held.contains_key(&expected));
            // Raw 0.4 maps to normalized 0.7, inside the hysteresis band.
            pad.set_virtual_axis(axis as u32, (i16::MAX as f32 * 0.4) as i16)
                .unwrap();
            tick(&mut runtime, &mut app);
            assert!(runtime.inputs.devices[&key].held.contains_key(&expected));
            pad.set_virtual_axis(axis as u32, i16::MIN).unwrap();
            tick(&mut runtime, &mut app);
            assert!(!runtime.inputs.devices[&key].held.contains_key(&expected));
        }
        assert_eq!(
            app.state()
                .controller()
                .bindings()
                .button(ControllerRole::Up),
            ControllerButton::LeftTrigger
        );
        assert_eq!(app.state().controller().capture(), None);
        let saved = app.state().controller().bindings().clone();

        // Remapping cancels capture and suppresses the device's current holds.
        app.dispatch_action_from(SemanticAction::Activate, EventSource::Keyboard)
            .unwrap();
        tick(&mut runtime, &mut app);
        assert_eq!(app.state().controller().capture(), Some(ControllerRole::Up));
        events
            .push_event(Event::GamepadRemapped {
                timestamp: 0,
                which: id,
            })
            .unwrap();
        tick(&mut runtime, &mut app);
        assert_eq!(app.state().controller().capture(), None);
        assert_eq!(app.state().controller().bindings(), &saved);

        // A hotplug with Edit already held cannot activate on first release.
        let second = attach();
        let second_id = second.id();
        let second_pad = joysticks.open(second_id).unwrap();
        second_pad
            .set_virtual_button(Button::South as u32, true)
            .unwrap();
        joysticks.update();
        tick(&mut runtime, &mut app);
        assert!(matches!(
            runtime.inputs.devices[&(second_id.raw() as usize)]
                .held
                .get(&ControllerButton::South),
            Some(Hold::Suppressed)
        ));
        second_pad
            .set_virtual_button(Button::South as u32, false)
            .unwrap();
        tick(&mut runtime, &mut app);
        // A save acknowledgement can advance generation; surface/capture cannot change.
        assert_eq!(app.state().controller().capture(), None);
        assert_eq!(
            app.state().interaction().active_surface(),
            SurfaceId::ControllerSettings
        );
        app.dispatch_action_from(SemanticAction::Activate, EventSource::Keyboard)
            .unwrap();
        tick(&mut runtime, &mut app);
        assert!(app.state().controller().capture().is_some());
        drop(second);
        tick(&mut runtime, &mut app);
        assert_eq!(app.state().controller().capture(), None);
        assert!(!runtime
            .inputs
            .devices
            .contains_key(&(second_id.raw() as usize)));
        assert!(runtime.inputs.devices.contains_key(&key));

        // SDL synthesizes releases on unplug. Edit cleanup must not confirm
        // the selected instrument as an ordinary tap release would.
        let edit_connection = attach();
        let edit_pad = joysticks.open(edit_connection.id()).unwrap();
        let (mut edit_app, _, _) = inputs(sample_state());
        tick(&mut runtime, &mut edit_app);
        let surface = edit_app.state().interaction().active_surface();
        edit_pad
            .set_virtual_button(Button::South as u32, true)
            .unwrap();
        tick(&mut runtime, &mut edit_app);
        assert_eq!(
            edit_app.state().interaction().mode(),
            InteractionMode::Adjust
        );
        drop(edit_connection);
        tick(&mut runtime, &mut edit_app);
        assert_eq!(
            edit_app.state().interaction().mode(),
            InteractionMode::Navigate
        );
        assert_eq!(edit_app.state().interaction().active_surface(), surface);

        // Physical disconnect releases the accepted preview through AppState.
        let mut state = sample_state();
        state
            .apply_semantic_action(SemanticAction::OpenRelated)
            .unwrap();
        state
            .apply_semantic_action(SemanticAction::OpenRelated)
            .unwrap();
        let (mut preview_app, _, _) = inputs(state);
        tick(&mut runtime, &mut preview_app);
        pad.set_virtual_button(Button::North as u32, true).unwrap();
        tick(&mut runtime, &mut preview_app);
        assert!(preview_app.state().file_browser().preview_is_held());
        drop(connection);
        tick(&mut runtime, &mut preview_app);
        assert!(!preview_app.state().file_browser().preview_is_held());
        assert!(!runtime.inputs.devices.contains_key(&key));

        runtime.backend_failure = Some(ControllerFailure::DeviceOpenFailed);
        tick(&mut runtime, &mut preview_app);
        assert_eq!(
            preview_app.state().controller().backend_failure(),
            Some(ControllerFailure::DeviceOpenFailed)
        );
        assert!(preview_app.state().controller().devices().is_empty());
        assert!(runtime.backend.is_none());
        // Owned shutdown flushes the accepted mapping to the isolated file.
        runtime.observe_preferences(app.state().controller());
        drop(runtime);
        assert_eq!(
            serde_json::from_slice::<ControllerBindings>(&std::fs::read(&path).unwrap()).unwrap(),
            saved
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn legacy_c_and_z_bindings_stay_readable_without_aliasing_sdl_buttons() {
        let json = serde_json::to_string(&ControllerBindings::default())
            .unwrap()
            .replace("dPadUp", "c")
            .replace("dPadDown", "z");
        let bindings: ControllerBindings = serde_json::from_str(&json).unwrap();
        assert_eq!(bindings.button(ControllerRole::Up), ControllerButton::C);
        assert_eq!(bindings.button(ControllerRole::Down), ControllerButton::Z);
        assert_eq!(serde_json::to_string(&bindings).unwrap(), json);
        assert!(STANDARD_BUTTONS
            .iter()
            .all(|(_, canonical)| !matches!(canonical, ControllerButton::C | ControllerButton::Z)));
        assert_eq!(button(Button::Misc1), None);
    }

    #[test]
    fn native_names_cannot_invalidate_connected_device_discovery() {
        assert_eq!(device_name(""), "Unnamed controller");
        assert_eq!(device_name("Pad\0Name"), "Pad\u{fffd}Name");
        assert_eq!(device_name("8BitDo Pro 3"), "8BitDo Pro 3");
    }
}
