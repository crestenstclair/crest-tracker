use crate::control::{Direction, SemanticAction};

/// A normalized controller gesture at the platform-adapter boundary.
///
/// Chord detection belongs to Crest's physical controller adapter, which reads
/// SDL3's standard buttons. The domain sees neither button codes nor a particular
/// device layout: it sees the same directional, Edit, Shift, and Start
/// vocabulary the keyboard adapter already normalizes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerGesture {
    Direction(Direction),
    EditDirection(Direction),
    ShiftDirection(Direction),
    Edit,
    Select,
    Start,
    ShiftStart,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerInputKind {
    Pressed,
    Released,
    Disconnected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControllerInput {
    gesture: ControllerGesture,
    kind: ControllerInputKind,
}

impl ControllerInput {
    pub const fn pressed(gesture: ControllerGesture) -> Self {
        Self {
            gesture,
            kind: ControllerInputKind::Pressed,
        }
    }

    pub const fn released(gesture: ControllerGesture) -> Self {
        Self {
            gesture,
            kind: ControllerInputKind::Released,
        }
    }

    pub const fn disconnected() -> Self {
        Self {
            gesture: ControllerGesture::Start,
            kind: ControllerInputKind::Disconnected,
        }
    }

    pub const fn gesture(self) -> ControllerGesture {
        self.gesture
    }

    pub const fn kind(self) -> ControllerInputKind {
        self.kind
    }
}

/// Converts normalized controller edges to the closed semantic vocabulary.
///
/// Page gestures and Start holds retain only transient edge state. Canonical focus,
/// mode, parameters, and preview state remain reducer-owned.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ControllerInputTranslator {
    page_held: [bool; 4],
    start_held: bool,
    shift_start_held: bool,
}

impl ControllerInputTranslator {
    pub const fn new() -> Self {
        Self {
            page_held: [false; 4],
            start_held: false,
            shift_start_held: false,
        }
    }

    pub fn translate(&mut self, input: ControllerInput) -> Option<SemanticAction> {
        match input.kind() {
            ControllerInputKind::Disconnected => {
                self.page_held = [false; 4];
                self.shift_start_held = false;
                self.start_held.then(|| {
                    self.start_held = false;
                    SemanticAction::PreviewStop
                })
            }
            ControllerInputKind::Released => {
                if let ControllerGesture::ShiftDirection(direction) = input.gesture() {
                    self.page_held[direction as usize] = false;
                }
                if input.gesture() == ControllerGesture::Start && self.start_held {
                    self.start_held = false;
                    Some(SemanticAction::PreviewStop)
                } else if input.gesture() == ControllerGesture::ShiftStart && self.shift_start_held
                {
                    self.shift_start_held = false;
                    None
                } else {
                    None
                }
            }
            ControllerInputKind::Pressed => match input.gesture() {
                ControllerGesture::Direction(direction) => {
                    Some(SemanticAction::Navigate(direction))
                }
                ControllerGesture::EditDirection(direction) => {
                    Some(SemanticAction::Adjust(direction))
                }
                ControllerGesture::ShiftDirection(direction) => {
                    (!core::mem::replace(&mut self.page_held[direction as usize], true))
                        .then_some(SemanticAction::NavigatePage(direction))
                }
                ControllerGesture::Edit => Some(SemanticAction::Activate),
                // Multi-select is reserved until its canonical action exists.
                ControllerGesture::Select => None,
                ControllerGesture::Start if self.start_held => None,
                ControllerGesture::Start => {
                    self.start_held = true;
                    Some(SemanticAction::PreviewStart)
                }
                ControllerGesture::ShiftStart if self.shift_start_held => None,
                ControllerGesture::ShiftStart => {
                    self.shift_start_held = true;
                    Some(SemanticAction::OpenMidiSettings)
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_page_gesture_requires_release_or_disconnect_before_another_edge() {
        let mut translator = ControllerInputTranslator::new();
        for direction in Direction::ALL {
            let gesture = ControllerGesture::ShiftDirection(direction);
            let press = ControllerInput::pressed(gesture);
            assert_eq!(
                translator.translate(press),
                Some(SemanticAction::NavigatePage(direction))
            );
            assert_eq!(translator.translate(press), None);
            assert_eq!(
                translator.translate(ControllerInput::released(gesture)),
                None
            );
            assert_eq!(
                translator.translate(press),
                Some(SemanticAction::NavigatePage(direction))
            );
            translator.translate(ControllerInput::disconnected());
            assert_eq!(
                translator.translate(press),
                Some(SemanticAction::NavigatePage(direction))
            );
            translator.translate(ControllerInput::released(gesture));
        }
    }

    #[test]
    fn controller_gestures_emit_the_same_semantic_vocabulary_as_keyboard_chords() {
        let mut translator = ControllerInputTranslator::new();
        for direction in Direction::ALL {
            assert_eq!(
                translator.translate(ControllerInput::pressed(ControllerGesture::Direction(
                    direction
                ))),
                Some(SemanticAction::Navigate(direction))
            );
            assert_eq!(
                translator.translate(ControllerInput::pressed(ControllerGesture::EditDirection(
                    direction
                ))),
                Some(SemanticAction::Adjust(direction))
            );
        }
        assert_eq!(
            translator.translate(ControllerInput::pressed(ControllerGesture::ShiftDirection(
                Direction::Up
            ))),
            Some(SemanticAction::NavigatePage(Direction::Up))
        );
        assert_eq!(
            translator.translate(ControllerInput::pressed(ControllerGesture::ShiftDirection(
                Direction::Down
            ))),
            Some(SemanticAction::NavigatePage(Direction::Down))
        );
        assert_eq!(
            translator.translate(ControllerInput::pressed(ControllerGesture::Edit)),
            Some(SemanticAction::Activate)
        );
    }

    #[test]
    fn option_state_controller_grammar_uses_adjust_navigate_activate_and_return() {
        let mut translator = ControllerInputTranslator::new();
        assert_eq!(
            translator.translate(ControllerInput::pressed(ControllerGesture::EditDirection(
                Direction::Up
            ))),
            Some(SemanticAction::Adjust(Direction::Up))
        );
        for direction in [Direction::Up, Direction::Down] {
            assert_eq!(
                translator.translate(ControllerInput::pressed(ControllerGesture::Direction(
                    direction
                ))),
                Some(SemanticAction::Navigate(direction))
            );
        }
        assert_eq!(
            translator.translate(ControllerInput::pressed(ControllerGesture::Edit)),
            Some(SemanticAction::Activate)
        );
        assert_eq!(
            translator.translate(ControllerInput::pressed(ControllerGesture::ShiftDirection(
                Direction::Down
            ))),
            Some(SemanticAction::NavigatePage(Direction::Down))
        );
    }

    #[test]
    fn start_is_one_press_and_one_release_even_with_repeat_or_disconnect() {
        let mut translator = ControllerInputTranslator::new();
        let start = ControllerInput::pressed(ControllerGesture::Start);
        assert_eq!(
            translator.translate(start),
            Some(SemanticAction::PreviewStart)
        );
        assert_eq!(translator.translate(start), None);
        assert_eq!(
            translator.translate(ControllerInput::released(ControllerGesture::Start)),
            Some(SemanticAction::PreviewStop)
        );
        assert_eq!(
            translator.translate(ControllerInput::released(ControllerGesture::Start)),
            None
        );
        assert_eq!(
            translator.translate(start),
            Some(SemanticAction::PreviewStart)
        );
        assert_eq!(
            translator.translate(ControllerInput::disconnected()),
            Some(SemanticAction::PreviewStop)
        );
    }

    #[test]
    fn shift_start_is_one_non_repeating_settings_action_and_has_no_preview_release() {
        let mut translator = ControllerInputTranslator::new();
        let shift_start = ControllerInput::pressed(ControllerGesture::ShiftStart);
        assert_eq!(
            translator.translate(shift_start),
            Some(SemanticAction::OpenMidiSettings)
        );
        assert_eq!(translator.translate(shift_start), None);
        assert_eq!(
            translator.translate(ControllerInput::released(ControllerGesture::ShiftStart)),
            None
        );
        assert_eq!(
            translator.translate(ControllerInput::pressed(ControllerGesture::ShiftDirection(
                Direction::Down
            ))),
            Some(SemanticAction::NavigatePage(Direction::Down))
        );
        assert_eq!(
            translator.translate(ControllerInput::pressed(ControllerGesture::Start)),
            Some(SemanticAction::PreviewStart)
        );
    }
}
