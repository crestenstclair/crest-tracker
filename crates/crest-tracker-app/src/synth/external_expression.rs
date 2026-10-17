//! Maps musical occurrence ownership to one worker-owned MPE zone lease.
use super::*;
use crate::midi_output::{MpeOutputAction, MpeOwner, MpeSource};
use crest_tracker::mpe::MpeEvent;

type Group = (TrackId, UseId, UseId, u64);
pub(super) struct Ledger {
    groups: BTreeMap<Group, u64>,
    voices: BTreeMap<Owner, MpeOwner>,
    next: u64,
}
impl Default for Ledger {
    fn default() -> Self {
        Self {
            groups: BTreeMap::new(),
            voices: BTreeMap::new(),
            next: 1,
        }
    }
}
impl Ledger {
    pub fn clear(&mut self) {
        self.groups.clear();
        self.voices.clear();
    }
    pub fn contains(&self, owner: &Owner) -> bool {
        self.voices.contains_key(owner)
    }
    pub fn event(
        &mut self,
        event: &TimedEvent,
        voice: Option<MidiVoiceId>,
        initial: Option<&(Vec<Control>, Vec<Control>)>,
    ) -> Result<Option<MpeOutputAction>, String> {
        if let Some((member, master)) = initial {
            let Message::NoteOn { pitch, velocity } = event.message else {
                return Err("MPE initialization requires attack".into());
            };
            let key = (
                event.owner.track,
                event.owner.chain_use,
                event.owner.phrase_use,
                event.owner.cycle,
            );
            let group = if let Some(group) = self.groups.get(&key) {
                *group
            } else {
                let group = self.next;
                self.next = self
                    .next
                    .checked_add(1)
                    .ok_or("MPE group identity exhausted")?;
                self.groups.insert(key, group);
                group
            };
            let owner = MpeOwner {
                source: MpeSource::Playback,
                voice: voice.ok_or("MPE attack lacks identity")?.get(),
                group,
            };
            if self.voices.insert(event.owner.clone(), owner).is_some() {
                return Err("Duplicate MPE attack owner".into());
            }
            return Ok(Some(MpeOutputAction::Attack {
                owner,
                pitch,
                velocity,
                member: member.clone(),
                master: master.clone(),
            }));
        }
        let owner = *self
            .voices
            .get(&event.owner)
            .ok_or("MPE delivery lacks original attack ownership")?;
        Ok(match event.message {
            Message::NoteOff { velocity, .. } => Some(MpeOutputAction::Release { owner, velocity }),
            Message::Expression { control } => Some(MpeOutputAction::Expression {
                owner,
                master: false,
                control,
            }),
            Message::MasterExpression { control } => Some(MpeOutputAction::Expression {
                owner,
                master: true,
                control,
            }),
            Message::ExpressionLifetime { .. } => None,
            Message::EndExpression => {
                self.voices.remove(&event.owner);
                if !self.voices.values().any(|voice| voice.group == owner.group) {
                    self.groups.retain(|_, group| *group != owner.group);
                }
                Some(MpeOutputAction::End { owner })
            }
            _ => return Err("Unexpected event in owned MPE delivery".into()),
        })
    }
}
pub(super) fn monitor(events: Vec<MpeEvent>) -> BTreeMap<Beat, Vec<MpeOutputAction>> {
    let mut batches: BTreeMap<Beat, Vec<MpeOutputAction>> = BTreeMap::new();
    let owner = |voice| MpeOwner {
        source: MpeSource::Monitor,
        voice,
        group: 1,
    };
    for event in events {
        let (at, action) = match event {
            MpeEvent::Attack {
                id,
                at,
                pitch,
                velocity,
                member,
                master,
                ..
            } => (
                at,
                MpeOutputAction::Attack {
                    owner: owner(id),
                    pitch,
                    velocity,
                    member,
                    master,
                },
            ),
            MpeEvent::Release { id, at, velocity } => (
                at,
                MpeOutputAction::Release {
                    owner: owner(id),
                    velocity,
                },
            ),
            MpeEvent::Expression {
                id,
                at,
                master,
                control,
            } => (
                at,
                MpeOutputAction::Expression {
                    owner: owner(id),
                    master,
                    control,
                },
            ),
            MpeEvent::End { id, at } => (at, MpeOutputAction::End { owner: owner(id) }),
        };
        batches.entry(at).or_default().push(action);
    }
    batches
}
impl Runtime {
    pub(super) fn flush_external_expression(
        &mut self,
        at: Beat,
        batches: &mut BTreeMap<String, Vec<MpeOutputAction>>,
        clock: HostClock,
    ) -> Result<(), String> {
        // Internal-only event windows have no external clock dependency.
        if batches.is_empty() {
            return Ok(());
        }
        let deadline = deadline_for_frame(clock, self.frame_at_beat(at, clock)?)?;
        self.enqueue_external_expression(batches, deadline)
    }

    /// The scheduler supplies a correlated deadline; output ownership and
    /// channel allocation do not depend on how the host establishes that clock.
    pub(super) fn enqueue_external_expression(
        &mut self,
        batches: &mut BTreeMap<String, Vec<MpeOutputAction>>,
        deadline: std::time::Instant,
    ) -> Result<(), String> {
        for (endpoint, actions) in std::mem::take(batches) {
            let profile = self
                .sync
                .preferences
                .mpe_outputs
                .iter()
                .find(|profile| profile.endpoint == endpoint)
                .ok_or("MPE output profile disappeared")?;
            self.outputs
                .enqueue_mpe_batch(profile, actions, deadline, self.outputs.epoch())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub(super) fn flush_external_monitor(&mut self, endpoint: &str) -> Result<(), String> {
        if self.monitor_cleanup {
            self.monitor_pending.clear();
            self.monitor_owned_pending.clear();
            self.monitor_owned.clear();
            self.outputs
                .cancel_monitor_mpe()
                .map_err(|e| e.to_string())?;
            self.monitor_cleanup = false;
            return Ok(());
        }
        let profile = self
            .sync
            .preferences
            .mpe_outputs
            .iter()
            .find(|profile| profile.endpoint == endpoint)
            .ok_or("MPE output profile disappeared")?;
        let now = std::time::Instant::now();
        for (_, actions) in monitor(std::mem::take(&mut self.monitor_owned_pending)) {
            self.outputs
                .enqueue_mpe_batch(profile, actions, now, self.outputs.epoch())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
