//! Control-side articulation ownership, including channel-pedal tails.
use super::*;
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
struct Voice {
    target: Target,
    pitch: u8,
    released: bool,
    expression: bool,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Voices {
    active: BTreeMap<Owner, Voice>,
    pedals: BTreeSet<Target>,
}
impl Voices {
    pub fn apply(&mut self, event: &TimedEvent) {
        match event.message {
            Message::NoteOn { pitch, .. } => {
                self.active.insert(
                    event.owner.clone(),
                    Voice {
                        target: event.target.clone(),
                        pitch,
                        released: false,
                        expression: false,
                    },
                );
            }
            Message::ExpressionLifetime { .. } => {
                if let Some(voice) = self.active.get_mut(&event.owner) {
                    voice.expression = true;
                }
            }
            Message::NoteOff { .. } => {
                if let Some(voice) = self.active.get_mut(&event.owner) {
                    voice.released = true;
                }
                self.expire();
            }
            Message::EndExpression | Message::KillNote { .. } => {
                self.active.remove(&event.owner);
            }
            Message::Control {
                control: Control::Cc { number: 64, value },
            } => {
                if value >= 64 {
                    self.pedals.insert(event.target.clone());
                } else {
                    self.pedals.remove(&event.target);
                    self.expire();
                }
            }
            _ => {}
        }
    }
    fn expire(&mut self) {
        self.active.retain(|_, voice| {
            !voice.released || voice.expression || self.pedals.contains(&voice.target)
        });
    }
    pub fn len(&self) -> usize {
        self.active.len()
    }
    pub fn kills(&self, track: TrackId, at: Beat) -> Vec<TimedEvent> {
        self.active
            .iter()
            .filter(|(owner, _)| owner.track == track)
            .map(|(owner, voice)| TimedEvent {
                at,
                owner: owner.clone(),
                target: voice.target.clone(),
                message: Message::KillNote { pitch: voice.pitch },
            })
            .collect()
    }
}
