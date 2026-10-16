//! Machine-local MIDI buttons enter the same Tracker request queue as the UI.
//! Performance packets not explicitly bound here retain their original routing.
use super::*;
use crate::preferences::{
    MidiControlAction as Action, MidiControlBinding, MidiControlTrigger as Trigger,
};
use crest_synth::control::PhysicalMidiEvent;
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct MidiControls {
    source: Option<(String, u64)>,
    overflow: u64,
    held: BTreeMap<(u8, Trigger, u8), HeldRoute>,
}
/// Preserve the original route until every matching key release arrives, even
/// when preferences change. CC buttons retain their route until falling below 64.
struct HeldRoute {
    mapped: bool,
    presses: u32,
}
#[derive(Debug, PartialEq, Eq)]
enum InputMatch {
    Unmapped,
    Consumed,
    Pressed(Action),
}
impl MidiControls {
    fn reset(&mut self) {
        self.held.clear();
    }
    /// A new connection never inherits a held button from an earlier device.
    pub fn observe_source(&mut self, source: Option<(String, u64)>, overflow: u64) {
        if self.source != source || self.overflow != overflow {
            self.reset();
            self.source = source;
            self.overflow = overflow;
        }
    }
    fn receive(&mut self, bindings: &[MidiControlBinding], event: PhysicalMidiEvent) -> InputMatch {
        let Some((source, revision)) = &self.source else {
            return InputMatch::Unmapped;
        };
        if *revision != event.revision().value() {
            return InputMatch::Consumed;
        }
        let message = event.message();
        let (trigger, down) = match message.kind() {
            MidiMessageKind::NoteOn => (Trigger::Note, message.data2() > 0),
            MidiMessageKind::NoteOff => (Trigger::Note, false),
            MidiMessageKind::ControlChange => (Trigger::Cc, message.data2() >= 64),
            _ => return InputMatch::Unmapped,
        };
        let channel = message.channel().value() + 1;
        let binding = bindings.iter().find(|binding| {
            binding.source == *source
                && binding.channel == channel
                && binding.trigger == trigger
                && binding.number == message.data1()
        });
        let key = (channel, trigger, message.data1());
        if let Some(held) = self.held.get_mut(&key) {
            let result = if held.mapped {
                InputMatch::Consumed
            } else {
                InputMatch::Unmapped
            };
            if down {
                if trigger == Trigger::Note && !held.mapped {
                    held.presses = held.presses.saturating_add(1);
                }
            } else {
                held.presses = held.presses.saturating_sub(1);
                if held.presses == 0 {
                    self.held.remove(&key);
                }
            }
            return result;
        }
        if down {
            self.held.insert(
                key,
                HeldRoute {
                    mapped: binding.is_some(),
                    presses: 1,
                },
            );
            return binding.map_or(InputMatch::Unmapped, |binding| {
                InputMatch::Pressed(binding.action)
            });
        }
        if binding.is_some() {
            InputMatch::Consumed
        } else {
            InputMatch::Unmapped
        }
    }
}
impl Runtime {
    pub(super) fn control_midi_input(&mut self, event: PhysicalMidiEvent) -> bool {
        match self
            .midi_controls
            .receive(&self.sync.preferences.midi_controls, event)
        {
            InputMatch::Unmapped => false,
            InputMatch::Consumed => true,
            InputMatch::Pressed(action) => {
                dispatch(&mut self.app.borrow_mut(), action);
                true
            }
        }
    }
}
fn dispatch(app: &mut App, action: Action) {
    if app.text_entry {
        let ui = match action {
            Action::Up => "previous",
            Action::Down => "next",
            Action::Left | Action::Decrease => "decrease",
            Action::Right | Action::Increase => "increase",
            Action::Confirm => "confirm",
            Action::Back => "back",
            Action::Stop => {
                app.requests.push(Request::Stop);
                return;
            }
            _ => return,
        };
        app.ui_input(ui);
        return;
    }
    let request = match action {
        Action::Menu => {
            app.ui_input("menu");
            return;
        }
        Action::Song => Request::View { view: View::Song },
        Action::Instruments => Request::View {
            view: View::Instruments,
        },
        Action::Play => Request::Play,
        Action::Stop => Request::Stop,
        Action::Undo => Request::Undo,
        Action::Redo => Request::Redo,
        _ if app.view == View::Instruments => {
            app.status = "MIDI navigation controls target Tracker; switch to Song first".into();
            return;
        }
        Action::Up => Request::Move {
            horizontal: 0,
            vertical: -1,
        },
        Action::Down => Request::Move {
            horizontal: 0,
            vertical: 1,
        },
        Action::Left => Request::Move {
            horizontal: -1,
            vertical: 0,
        },
        Action::Right => Request::Move {
            horizontal: 1,
            vertical: 0,
        },
        Action::Confirm => Request::Confirm,
        Action::Back => Request::Back,
        Action::Increase => Request::Adjust { delta: 1 },
        Action::Decrease => Request::Adjust { delta: -1 },
        Action::Record => Request::Capture {
            action: CaptureAction::Record,
        },
        Action::EndTake => Request::Capture {
            action: CaptureAction::End,
        },
    };
    app.requests.push(request);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crest_synth::control::MidiConnectionRevision;
    fn packet(revision: u64, kind: MidiMessageKind, number: u8, value: u8) -> PhysicalMidiEvent {
        PhysicalMidiEvent::new(
            MidiConnectionRevision::new(revision).unwrap(),
            1,
            MidiMessage::try_new(MidiChannel::new(0).unwrap(), kind, number, value).unwrap(),
        )
    }
    fn binding(trigger: Trigger) -> MidiControlBinding {
        MidiControlBinding {
            source: "controls".into(),
            channel: 1,
            trigger,
            number: 60,
            action: Action::Play,
        }
    }
    #[test]
    fn exact_source_and_revision_latch_presses_and_consume_releases_without_note_leakage() {
        let mut controls = MidiControls::default();
        controls.observe_source(Some(("controls".into(), 1)), 0);
        let bindings = [binding(Trigger::Note)];
        let on = packet(1, MidiMessageKind::NoteOn, 60, 100);
        assert_eq!(
            controls.receive(&bindings, on),
            InputMatch::Pressed(Action::Play)
        );
        assert_eq!(controls.receive(&bindings, on), InputMatch::Consumed);
        assert_eq!(
            controls.receive(&bindings, packet(1, MidiMessageKind::NoteOff, 60, 64)),
            InputMatch::Consumed
        );
        assert_eq!(
            controls.receive(&bindings, on),
            InputMatch::Pressed(Action::Play)
        );
        controls.observe_source(Some(("controls".into(), 2)), 0);
        assert_eq!(controls.receive(&bindings, on), InputMatch::Consumed);
        assert_eq!(
            controls.receive(&bindings, packet(2, MidiMessageKind::NoteOn, 60, 100)),
            InputMatch::Pressed(Action::Play)
        );
        controls.observe_source(Some(("other".into(), 3)), 0);
        assert_eq!(
            controls.receive(&bindings, packet(3, MidiMessageKind::NoteOn, 60, 100)),
            InputMatch::Unmapped
        );
    }
    #[test]
    fn binding_changes_preserve_passed_notes_and_suppress_mapped_release_leakage() {
        let mut controls = MidiControls::default();
        controls.observe_source(Some(("controls".into(), 1)), 0);
        let on = packet(1, MidiMessageKind::NoteOn, 60, 100);
        let off = packet(1, MidiMessageKind::NoteOff, 60, 0);
        let bindings = [binding(Trigger::Note)];
        assert_eq!(controls.receive(&[], on), InputMatch::Unmapped);
        assert_eq!(controls.receive(&[], on), InputMatch::Unmapped);
        assert_eq!(controls.receive(&bindings, off), InputMatch::Unmapped);
        assert_eq!(controls.receive(&bindings, off), InputMatch::Unmapped);
        assert_eq!(
            controls.receive(&bindings, on),
            InputMatch::Pressed(Action::Play)
        );
        assert_eq!(controls.receive(&[], on), InputMatch::Consumed);
        assert_eq!(controls.receive(&[], off), InputMatch::Consumed);
        assert_eq!(controls.receive(&[], on), InputMatch::Unmapped);
    }

    #[test]
    fn cc_threshold_and_dialog_actions_use_authoritative_queue() {
        let mut controls = MidiControls::default();
        controls.observe_source(Some(("controls".into(), 1)), 0);
        let bindings = [binding(Trigger::Cc)];
        for (value, expected) in [
            (63, InputMatch::Consumed),
            (64, InputMatch::Pressed(Action::Play)),
            (127, InputMatch::Consumed),
            (0, InputMatch::Consumed),
            (100, InputMatch::Pressed(Action::Play)),
        ] {
            assert_eq!(
                controls.receive(
                    &bindings,
                    packet(1, MidiMessageKind::ControlChange, 60, value)
                ),
                expected
            );
        }
        let mut app = App::new();
        dispatch(&mut app, Action::Play);
        assert!(matches!(app.requests.as_slice(), [Request::Play]));
        app.text_entry = true;
        dispatch(&mut app, Action::Play);
        assert_eq!(app.requests.len(), 1);
        dispatch(&mut app, Action::Stop);
        assert!(matches!(app.requests.last(), Some(Request::Stop)));
    }
}
