//! Transactional allocation for explicitly declared external MPE destinations.
//! Channels are zero-based only inside this module and on the MIDI wire.
use super::{ChannelOwnership, Packet};
use crest_tracker::composition::{Channel, Control};
use crest_tracker::mpe::MpeZone;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The receiver must implement MPE zone configuration, RPN 0 bend ranges,
/// channel pressure, member CC74, sustain, and member All Sound Off for recovery.
/// It must preserve released voices' expression across member and master reuse;
/// this is a declared receiver contract, not a certification of any endpoint.
/// Empty initial controls use standard member/master ranges of 48/2 semitones,
/// centered bend, zero pressure/pedal and centered member timbre.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MpeOutputProfile {
    pub endpoint: String,
    pub zones: Vec<MpeZone>,
}

impl MpeOutputProfile {
    pub fn validate(&self) -> Result<(), String> {
        if self.endpoint.trim().is_empty() || self.endpoint.chars().any(char::is_control) {
            return Err("MPE output requires an exact nonempty endpoint identity".into());
        }
        if self.zones.is_empty() || self.zones.len() > 2 {
            return Err("MPE output requires one or two nonoverlapping zones".into());
        }
        let mut used = 0_u16;
        for zone in &self.zones {
            let mask = zone_mask(*zone);
            if used & mask != 0 {
                return Err("MPE output zones overlap".into());
            }
            used |= mask;
        }
        Ok(())
    }

    fn reserves(&self, channel: u8) -> bool {
        self.zones
            .iter()
            .any(|zone| zone_mask(*zone) & (1 << channel) != 0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum MpeSource {
    Playback,
    Monitor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct MpeOwner {
    pub source: MpeSource,
    pub voice: u64,
    pub group: u64,
}

#[derive(Clone, Debug)]
pub enum MpeOutputAction {
    Attack {
        owner: MpeOwner,
        pitch: u8,
        velocity: u8,
        member: Vec<Control>,
        master: Vec<Control>,
    },
    Release {
        owner: MpeOwner,
        velocity: u8,
    },
    Expression {
        owner: MpeOwner,
        master: bool,
        control: Control,
    },
    End {
        owner: MpeOwner,
    },
}

impl MpeOutputAction {
    pub(super) fn owner(&self) -> MpeOwner {
        match self {
            Self::Attack { owner, .. }
            | Self::Release { owner, .. }
            | Self::Expression { owner, .. }
            | Self::End { owner } => *owner,
        }
    }

    /// Conservative expanded wire work plus input traversal. Counting controls
    /// also bounds a batch whose many input updates collapse into one snapshot.
    pub(super) fn cost(&self) -> Option<usize> {
        match self {
            Self::Attack { member, master, .. } => 33_usize
                .checked_add(member.len())?
                .checked_add(master.len()),
            Self::End { .. } => Some(24),
            Self::Release { .. } | Self::Expression { .. } => Some(2),
        }
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        if self.owner().voice == 0 || self.owner().group == 0 {
            return Err("MPE voice and performance group identities must be nonzero".into());
        }
        match self {
            Self::Attack {
                pitch,
                velocity,
                member,
                master,
                ..
            } => {
                if *pitch > 127 || *velocity == 0 || *velocity > 127 {
                    return Err("MPE attack requires a MIDI pitch and positive velocity".into());
                }
                State::initial(member, false)?;
                State::initial(master, true)?;
            }
            Self::Release { velocity, .. } if *velocity > 127 => {
                return Err("MPE release velocity exceeds MIDI range".into());
            }
            Self::Expression {
                master, control, ..
            } => validate_control(*control, *master)?,
            _ => {}
        }
        Ok(())
    }
}

pub(super) fn batch_cost(actions: &[MpeOutputAction]) -> Option<usize> {
    actions
        .iter()
        .try_fold(0_usize, |sum, action| sum.checked_add(action.cost()?))
}

fn zone_mask(zone: MpeZone) -> u16 {
    (1..=16).fold(0, |mask, channel| {
        if zone.master().get() == channel || zone.contains_member(Channel::new(channel).unwrap()) {
            mask | (1 << (channel - 1))
        } else {
            mask
        }
    })
}

fn validate_control(control: Control, master: bool) -> Result<(), String> {
    control.validate().map_err(|error| error.to_string())?;
    match control {
        Control::Bend { .. } | Control::Pressure { .. } => Ok(()),
        Control::Cc { number: 64, .. } => Ok(()),
        Control::Cc { number: 74, .. } if !master => Ok(()),
        Control::Cc { number: 6 | 38, .. } => Ok(()),
        Control::Cc {
            number: 100 | 101,
            value: 0 | 127,
        } => Ok(()),
        _ => Err("control is outside the declared MPE output contract".into()),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct State {
    bend: u16,
    semitones: u8,
    cents: u8,
    pressure: u8,
    timbre: u8,
    pedal: u8,
    rpn: (u8, u8),
}

impl State {
    fn neutral(master: bool) -> Self {
        Self {
            bend: 8192,
            semitones: if master { 2 } else { 48 },
            cents: 0,
            pressure: 0,
            timbre: 64,
            pedal: 0,
            rpn: (127, 127),
        }
    }

    fn initial(controls: &[Control], master: bool) -> Result<Self, String> {
        let mut state = Self::neutral(master);
        for control in controls {
            state.apply(*control, master)?;
        }
        Ok(state)
    }

    fn apply(&mut self, control: Control, master: bool) -> Result<(), String> {
        validate_control(control, master)?;
        match control {
            Control::Bend { value } => self.bend = value,
            Control::Pressure { value } => self.pressure = value,
            Control::Cc { number: 64, value } => self.pedal = value,
            Control::Cc { number: 74, value } => self.timbre = value,
            Control::Cc { number: 101, value } => self.rpn.0 = value,
            Control::Cc { number: 100, value } => self.rpn.1 = value,
            Control::Cc { number: 6 | 38, .. } if self.rpn != (0, 0) => {
                return Err("MPE data entry requires an explicit RPN 0 selection".into());
            }
            Control::Cc { number: 6, value } => self.semitones = value,
            Control::Cc { number: 38, value } if value <= 99 => self.cents = value,
            Control::Cc { number: 38, .. } => return Err("MPE bend-range cents exceed 99".into()),
            _ => unreachable!("validated control"),
        }
        Ok(())
    }

    fn packets(self, channel: u8, master: bool) -> Vec<Packet> {
        let mut packets = vec![
            cc(channel, 101, 0),
            cc(channel, 100, 0),
            cc(channel, 6, self.semitones),
            cc(channel, 38, self.cents),
            cc(channel, 101, self.rpn.0),
            cc(channel, 100, self.rpn.1),
            control_packet(channel, Control::Bend { value: self.bend }),
            control_packet(
                channel,
                Control::Pressure {
                    value: self.pressure,
                },
            ),
            cc(channel, 64, self.pedal),
        ];
        if !master {
            packets.push(cc(channel, 74, self.timbre));
        }
        packets
    }
}

fn cc(channel: u8, number: u8, value: u8) -> Packet {
    Packet {
        bytes: [0xb0 | channel, number, value],
        len: 3,
    }
}

fn control_packet(channel: u8, control: Control) -> Packet {
    match control {
        Control::Bend { value } => Packet {
            bytes: [0xe0 | channel, (value & 127) as u8, (value >> 7) as u8],
            len: 3,
        },
        Control::Pressure { value } => Packet {
            bytes: [0xd0 | channel, value, 0],
            len: 2,
        },
        Control::Cc { number, value } => cc(channel, number, value),
        _ => unreachable!("validated MPE control"),
    }
}

#[derive(Clone)]
struct Voice {
    zone: MpeZone,
    channel: u8,
    pitch: u8,
    held: bool,
    member: State,
    master: State,
}

#[derive(Clone, Default)]
pub(super) struct Allocator {
    profile: Option<MpeOutputProfile>,
    voices: BTreeMap<MpeOwner, Voice>,
    // Release tails can outlive End. Global cleanup still owns these channels,
    // even after their member lease has been returned for normal reuse.
    touched_channels: BTreeSet<u8>,
}

pub(super) struct Plan {
    pub next: Allocator,
    pub packets: Vec<Packet>,
    // Union of old and staged ownership, including voices ending within this
    // batch. Retained before the first backend call until every call succeeds.
    pub notes: BTreeSet<(u8, u8)>,
    pub channels: BTreeSet<u8>,
}

impl Allocator {
    pub(super) fn reserves(&self, channel: u8) -> bool {
        self.profile
            .as_ref()
            .is_some_and(|profile| profile.reserves(channel))
    }

    pub(super) fn plan(
        &self,
        profile: &MpeOutputProfile,
        actions: &[MpeOutputAction],
        ordinary: &[ChannelOwnership; 16],
    ) -> Result<Plan, String> {
        profile.validate()?;
        if self
            .profile
            .as_ref()
            .is_some_and(|current| current != profile)
            && !self.voices.is_empty()
        {
            return Err("MPE output profile cannot change while voices retain leases".into());
        }
        if ordinary
            .iter()
            .enumerate()
            .any(|(channel, state)| state.touched && profile.reserves(channel as u8))
        {
            return Err(
                "MPE zone collides with an ordinary MIDI channel owned by this connection".into(),
            );
        }
        for action in actions {
            action.validate()?;
        }
        let mut plan = Plan {
            next: self.clone(),
            packets: Vec::new(),
            notes: BTreeSet::new(),
            channels: BTreeSet::new(),
        };
        plan.next.profile = Some(profile.clone());
        for voice in self.voices.values() {
            plan.retain(voice);
        }
        // A compiler may place a new attack before an old expression end at
        // the same timestamp. Complete all existing owners first, keeping each
        // owner's source order, so cleanup cannot kill its successor.
        for existing in [true, false] {
            for action in actions {
                if self.voices.contains_key(&action.owner()) == existing {
                    plan.apply(action)?;
                }
            }
        }
        let mut masters = BTreeMap::new();
        for voice in plan.next.voices.values() {
            let channel = voice.zone.master().get() - 1;
            if masters
                .insert(channel, voice.master)
                .is_some_and(|old| old != voice.master)
            {
                return Err(
                    "MPE voices sharing a performance group require identical final master state"
                        .into(),
                );
            }
        }
        Ok(plan)
    }

    pub(super) fn cancel_plan(
        &self,
        source: MpeSource,
        ordinary: &[ChannelOwnership; 16],
    ) -> Result<Option<Plan>, String> {
        let owners: Vec<_> = self
            .voices
            .keys()
            .filter(|owner| owner.source == source)
            .copied()
            .collect();
        if owners.is_empty() {
            return Ok(None);
        }
        let mut plan = self.plan(
            self.profile.as_ref().expect("owned voices have a profile"),
            &[],
            ordinary,
        )?;
        for owner in owners {
            plan.end(owner, true)?;
        }
        Ok(Some(plan))
    }

    pub(super) fn cleanup_ownership(&self) -> (BTreeSet<(u8, u8)>, BTreeSet<u8>) {
        let mut notes = BTreeSet::new();
        let mut channels = self.touched_channels.clone();
        for voice in self.voices.values() {
            notes.insert((voice.channel, voice.pitch));
            channels.insert(voice.channel);
            channels.insert(voice.zone.master().get() - 1);
        }
        (notes, channels)
    }
}

impl Plan {
    fn retain(&mut self, voice: &Voice) {
        self.notes.insert((voice.channel, voice.pitch));
        self.channels.insert(voice.channel);
        self.channels.insert(voice.zone.master().get() - 1);
        self.next.touched_channels.insert(voice.channel);
        self.next
            .touched_channels
            .insert(voice.zone.master().get() - 1);
    }

    fn apply(&mut self, action: &MpeOutputAction) -> Result<(), String> {
        let owner = action.owner();
        match action {
            MpeOutputAction::Attack {
                pitch,
                velocity,
                member,
                master,
                ..
            } => {
                if self.next.voices.contains_key(&owner) {
                    return Err("MPE voice identity already retains a member lease".into());
                }
                let zone = self
                    .next
                    .voices
                    .iter()
                    .find(|(other, _)| other.source == owner.source && other.group == owner.group)
                    .map(|(_, voice)| voice.zone);
                let new_group = zone.is_none();
                let zone = zone
                    .or_else(|| {
                        self.next
                            .profile
                            .as_ref()
                            .unwrap()
                            .zones
                            .iter()
                            .find(|zone| {
                                !self.next.voices.values().any(|voice| voice.zone == **zone)
                            })
                            .copied()
                    })
                    .ok_or("MPE output zones exhausted by independent performance groups")?;
                let channel = (1..=16)
                    .find(|channel| {
                        zone.contains_member(Channel::new(*channel).unwrap())
                            && !self
                                .next
                                .voices
                                .values()
                                .any(|voice| voice.channel == *channel - 1)
                    })
                    .map(|channel| channel - 1)
                    .ok_or("MPE output member channels exhausted; no voice stealing")?;
                let voice = Voice {
                    zone,
                    channel,
                    pitch: *pitch,
                    held: true,
                    member: State::initial(member, false)?,
                    master: State::initial(master, true)?,
                };
                self.retain(&voice);
                let master_channel = zone.master().get() - 1;
                if new_group {
                    self.packets.extend([
                        cc(master_channel, 101, 0),
                        cc(master_channel, 100, 6),
                        cc(master_channel, 6, zone.members()),
                        cc(master_channel, 101, 127),
                        cc(master_channel, 100, 127),
                    ]);
                }
                self.packets
                    .extend(voice.master.packets(master_channel, true));
                self.packets.extend(voice.member.packets(channel, false));
                self.packets.push(Packet {
                    bytes: [0x90 | channel, *pitch, *velocity],
                    len: 3,
                });
                self.next.voices.insert(owner, voice);
            }
            MpeOutputAction::Release { velocity, .. } => {
                let voice = self
                    .next
                    .voices
                    .get_mut(&owner)
                    .ok_or("MPE release has no retained voice owner")?;
                if !voice.held {
                    return Err("MPE voice key was already released".into());
                }
                voice.held = false;
                self.packets.push(Packet {
                    bytes: [0x80 | voice.channel, voice.pitch, *velocity],
                    len: 3,
                });
            }
            MpeOutputAction::Expression {
                master, control, ..
            } => {
                let voice = self
                    .next
                    .voices
                    .get_mut(&owner)
                    .ok_or("MPE expression has no retained voice owner")?;
                let state = if *master {
                    &mut voice.master
                } else {
                    &mut voice.member
                };
                state.apply(*control, *master)?;
                let channel = if *master {
                    voice.zone.master().get() - 1
                } else {
                    voice.channel
                };
                self.packets.push(control_packet(channel, *control));
            }
            MpeOutputAction::End { .. } => {
                self.end(owner, false)?;
            }
        }
        Ok(())
    }

    fn end(&mut self, owner: MpeOwner, hard: bool) -> Result<(), String> {
        let voice = self
            .next
            .voices
            .remove(&owner)
            .ok_or("MPE expression end has no retained voice owner")?;
        let last = !self
            .next
            .voices
            .values()
            .any(|other| other.zone == voice.zone);
        if !hard && !last && voice.master.pedal >= 64 {
            return Err(
                "MPE expression cannot end while its shared master pedal retains it".into(),
            );
        }
        if voice.held || hard {
            self.packets.push(Packet {
                bytes: [0x80 | voice.channel, voice.pitch, 0],
                len: 3,
            });
        }
        self.packets.push(cc(voice.channel, 64, 0));
        if hard {
            self.packets.push(cc(voice.channel, 120, 0));
            self.packets
                .extend(State::neutral(false).packets(voice.channel, false));
        }
        if last {
            let channel = voice.zone.master().get() - 1;
            if hard {
                self.packets
                    .extend(State::neutral(true).packets(channel, true));
            } else {
                self.packets.push(cc(channel, 64, 0));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        deliver, discard_stale_epochs, discard_stale_monitor, Command, Destination,
        MidiOutputError, MidiOutputService, MidiOutputSnapshot, Scheduled, ScheduledMpe, Shared,
        Sink,
    };
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    };
    use std::time::Instant;

    #[derive(Default)]
    struct Wire {
        packets: Vec<Vec<u8>>,
        fail: bool,
    }
    impl Sink for Wire {
        fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
            self.packets.push(bytes.to_vec());
            if self.fail {
                Err("wire failed after possible delivery".into())
            } else {
                Ok(())
            }
        }
    }
    fn profile(zones: Vec<MpeZone>) -> MpeOutputProfile {
        MpeOutputProfile {
            endpoint: "exact-port".into(),
            zones,
        }
    }
    fn lower(members: u8) -> MpeOutputProfile {
        profile(vec![MpeZone::lower(members).unwrap()])
    }
    fn owner(source: MpeSource, voice: u64, group: u64) -> MpeOwner {
        MpeOwner {
            source,
            voice,
            group,
        }
    }
    fn playback(voice: u64) -> MpeOwner {
        owner(MpeSource::Playback, voice, 1)
    }
    fn attack(owner: MpeOwner) -> MpeOutputAction {
        MpeOutputAction::Attack {
            owner,
            pitch: 60,
            velocity: 100,
            member: vec![],
            master: vec![],
        }
    }
    fn expression(owner: MpeOwner, master: bool, control: Control) -> MpeOutputAction {
        MpeOutputAction::Expression {
            owner,
            master,
            control,
        }
    }
    fn bend(owner: MpeOwner, master: bool, value: u16) -> MpeOutputAction {
        expression(owner, master, Control::Bend { value })
    }
    fn pedal(owner: MpeOwner, value: u8) -> MpeOutputAction {
        expression(owner, true, Control::Cc { number: 64, value })
    }

    #[test]
    fn profiles_require_exact_identity_and_disjoint_declared_zones() {
        lower(15).validate().unwrap();
        profile(vec![MpeZone::lower(7).unwrap(), MpeZone::upper(7).unwrap()])
            .validate()
            .unwrap();
        assert!(
            profile(vec![MpeZone::lower(8).unwrap(), MpeZone::upper(7).unwrap()])
                .validate()
                .is_err()
        );
        assert!(
            profile(vec![MpeZone::lower(2).unwrap(), MpeZone::lower(2).unwrap()])
                .validate()
                .is_err()
        );
        assert!(profile(vec![]).validate().is_err());
        let mut invalid = lower(1);
        invalid.endpoint = " \0 ".into();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn same_pitch_voices_get_distinct_members_with_initial_controls_before_attack() {
        let mut destination = Destination::new(Wire::default(), 1);
        let mut first = attack(playback(1));
        let mut second = attack(playback(2));
        if let MpeOutputAction::Attack { member, .. } = &mut first {
            member.push(Control::Bend { value: 1025 });
        }
        if let MpeOutputAction::Attack { member, .. } = &mut second {
            member.push(Control::Bend { value: 15000 });
        }
        destination.send_mpe(&lower(2), &[first, second]).unwrap();
        let wire = &destination.sink.packets;
        let attacks: Vec<_> = wire
            .iter()
            .enumerate()
            .filter(|(_, bytes)| bytes[0] >> 4 == 9)
            .collect();
        assert_eq!(attacks.len(), 2);
        assert_eq!(attacks[0].1, &[0x91, 60, 100]);
        assert_eq!(attacks[1].1, &[0x92, 60, 100]);
        assert!(wire[..attacks[0].0].contains(&vec![0xe1, 1, 8]));
        assert!(wire[attacks[0].0 + 1..attacks[1].0].contains(&vec![0xe2, 24, 117]));
        assert!(wire[..attacks[0].0].contains(&vec![0xb1, 6, 48]));
        assert!(wire[..attacks[0].0].contains(&vec![0xb0, 6, 2]));
        assert_eq!(
            &wire[..5],
            &[
                vec![0xb0, 101, 0],
                vec![0xb0, 100, 6],
                vec![0xb0, 6, 2],
                vec![0xb0, 101, 127],
                vec![0xb0, 100, 127]
            ]
        );
        destination
            .send_mpe(
                &lower(2),
                &[
                    bend(playback(1), false, 22),
                    bend(playback(2), false, 14000),
                ],
            )
            .unwrap();
        assert_eq!(destination.mpe.voices[&playback(1)].member.bend, 22);
        assert_eq!(destination.mpe.voices[&playback(2)].member.bend, 14000);
    }

    #[test]
    fn ordinary_channels_remain_compatible_and_reserved_collisions_are_transactional() {
        let mut destination = Destination::new(Wire::default(), 1);
        destination
            .send(Packet::parse(&[0x98, 62, 90]).unwrap())
            .unwrap();
        destination
            .send_mpe(&lower(2), &[attack(playback(1))])
            .unwrap();
        let before = destination.sink.packets.len();
        for bytes in [[0x90, 60, 100], [0x91, 61, 100], [0xb2, 64, 127]] {
            assert!(destination
                .send(Packet::parse(&bytes).unwrap())
                .unwrap_err()
                .contains("reserved"));
        }
        assert_eq!(destination.sink.packets.len(), before);
        destination
            .send(Packet::parse(&[0x88, 62, 0]).unwrap())
            .unwrap();
        let mut occupied = Destination::new(Wire::default(), 1);
        occupied
            .send(Packet::parse(&[0x91, 60, 100]).unwrap())
            .unwrap();
        assert!(occupied
            .send_mpe(&lower(2), &[attack(playback(1))])
            .is_err());
        assert_eq!(occupied.sink.packets.len(), 1);
    }

    #[test]
    fn sustain_retains_member_until_end_and_equal_time_end_precedes_reuse_without_hard_kill() {
        let mut destination = Destination::new(Wire::default(), 1);
        let profile = lower(1);
        destination
            .send_mpe(
                &profile,
                &[
                    attack(playback(1)),
                    pedal(playback(1), 127),
                    MpeOutputAction::Release {
                        owner: playback(1),
                        velocity: 42,
                    },
                ],
            )
            .unwrap();
        let before = destination.sink.packets.len();
        assert!(destination
            .send_mpe(&profile, &[attack(playback(2))])
            .unwrap_err()
            .contains("exhausted"));
        assert_eq!(destination.sink.packets.len(), before);
        assert!(!destination.mpe.voices[&playback(1)].held);
        destination
            .send_mpe(
                &profile,
                &[
                    attack(playback(2)),
                    pedal(playback(1), 0),
                    MpeOutputAction::End { owner: playback(1) },
                ],
            )
            .unwrap();
        assert!(!destination.mpe.voices.contains_key(&playback(1)));
        assert_eq!(destination.mpe.voices[&playback(2)].channel, 1);
        assert!(destination.sink.packets[before..]
            .iter()
            .all(|bytes| !(bytes[0] >> 4 == 11 && bytes[1] == 120)));
        assert_eq!(destination.sink.packets[before], vec![0xb0, 64, 0]);
    }

    #[test]
    fn final_master_conflict_rejects_whole_batch_but_complete_fanout_succeeds() {
        let mut destination = Destination::new(Wire::default(), 1);
        let profile = lower(3);
        destination
            .send_mpe(&profile, &[attack(playback(1)), attack(playback(2))])
            .unwrap();
        let before = destination.sink.packets.len();
        assert!(destination
            .send_mpe(
                &profile,
                &[bend(playback(1), true, 12345), bend(playback(1), false, 2)]
            )
            .unwrap_err()
            .contains("final master state"));
        assert_eq!(destination.sink.packets.len(), before);
        assert_eq!(destination.mpe.voices[&playback(1)].member.bend, 8192);
        destination
            .send_mpe(
                &profile,
                &[
                    bend(playback(1), true, 12345),
                    bend(playback(2), true, 12345),
                ],
            )
            .unwrap();
        let before = destination.sink.packets.len();
        assert!(destination
            .send_mpe(&profile, &[attack(playback(3))])
            .is_err());
        assert_eq!(destination.sink.packets.len(), before);
        destination
            .send_mpe(
                &profile,
                &[attack(playback(3)), bend(playback(3), true, 12345)],
            )
            .unwrap();
        assert_eq!(destination.mpe.voices.len(), 3);
    }

    #[test]
    fn independent_groups_lease_disjoint_zones_and_exhaust_visibly() {
        let profile = profile(vec![MpeZone::lower(3).unwrap(), MpeZone::upper(3).unwrap()]);
        let first = playback(1);
        let second = owner(MpeSource::Playback, 2, 2);
        let third = owner(MpeSource::Monitor, 1, 1);
        let mut destination = Destination::new(Wire::default(), 1);
        destination
            .send_mpe(
                &profile,
                &[
                    attack(first),
                    attack(second),
                    bend(first, true, 1000),
                    bend(second, true, 15000),
                ],
            )
            .unwrap();
        assert_ne!(
            destination.mpe.voices[&first].zone,
            destination.mpe.voices[&second].zone
        );
        let before = destination.sink.packets.len();
        assert!(destination
            .send_mpe(&profile, &[attack(third)])
            .unwrap_err()
            .contains("independent performance groups"));
        assert_eq!(destination.sink.packets.len(), before);
        destination
            .send_mpe(
                &profile,
                &[attack(third), MpeOutputAction::End { owner: second }],
            )
            .unwrap();
        assert_eq!(
            destination.mpe.voices[&third].zone,
            MpeZone::upper(3).unwrap()
        );
    }

    #[test]
    fn cancellation_is_source_isolated_in_both_directions_and_global_cleans_both() {
        let profile = profile(vec![MpeZone::lower(2).unwrap(), MpeZone::upper(2).unwrap()]);
        let playback = playback(1);
        let monitor = owner(MpeSource::Monitor, 1, 1);
        for canceled in [MpeSource::Playback, MpeSource::Monitor] {
            let mut destination = Destination::new(Wire::default(), 1);
            destination
                .send_mpe(
                    &profile,
                    &[
                        attack(playback),
                        attack(monitor),
                        pedal(playback, 127),
                        pedal(monitor, 127),
                    ],
                )
                .unwrap();
            destination.sink.packets.clear();
            if canceled == MpeSource::Monitor {
                destination.cancel_monitor_mpe().unwrap();
            } else {
                let plan = destination
                    .mpe
                    .cancel_plan(canceled, &destination.channels)
                    .unwrap()
                    .unwrap();
                destination.send_mpe_plan(plan).unwrap();
            }
            let kept = if canceled == MpeSource::Playback {
                monitor
            } else {
                playback
            };
            let kept_voice = &destination.mpe.voices[&kept];
            assert!(destination.sink.packets.iter().all(|bytes| {
                let channel = bytes[0] & 15;
                channel != kept_voice.channel && channel != kept_voice.zone.master().get() - 1
            }));
            destination
                .send_mpe(&profile, &[bend(kept, false, 100)])
                .unwrap();
            destination.cleanup().unwrap();
            assert!(destination.mpe.voices.is_empty());
            assert!(destination.mpe.profile.is_none());
        }
        let mut destination = Destination::new(Wire::default(), 1);
        destination
            .send_mpe(&profile, &[attack(playback), attack(monitor)])
            .unwrap();
        destination.cleanup().unwrap();
        let before = destination.sink.packets.len();
        destination.cancel_monitor_mpe().unwrap();
        assert_eq!(destination.sink.packets.len(), before);
    }

    #[test]
    fn unsupported_controls_and_incomplete_ranges_reject_without_wire_or_leases() {
        let mut destination = Destination::new(Wire::default(), 1);
        for controls in [
            vec![Control::Cc {
                number: 38,
                value: 50,
            }],
            vec![Control::Program { value: 3 }],
            vec![Control::PolyPressure {
                pitch: 60,
                value: 100,
            }],
            vec![
                Control::Cc {
                    number: 101,
                    value: 0,
                },
                Control::Cc {
                    number: 100,
                    value: 0,
                },
                Control::Cc {
                    number: 38,
                    value: 100,
                },
            ],
        ] {
            let mut action = attack(playback(1));
            if let MpeOutputAction::Attack { member, .. } = &mut action {
                *member = controls;
            }
            assert!(destination.send_mpe(&lower(1), &[action]).is_err());
            assert!(destination.sink.packets.is_empty());
            assert!(destination.mpe.voices.is_empty());
        }
        assert!(destination
            .send_mpe(
                &lower(1),
                &[
                    attack(playback(1)),
                    expression(
                        playback(1),
                        true,
                        Control::Cc {
                            number: 74,
                            value: 99
                        }
                    )
                ]
            )
            .is_err());
        assert!(destination.sink.packets.is_empty());
    }

    fn service(capacity: usize) -> (MidiOutputService, mpsc::Receiver<Command>) {
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let shared = Arc::new(Shared {
            epoch: AtomicU64::new(1),
            monitor_generation: AtomicU64::new(1),
            pending: AtomicUsize::new(0),
            shutdown: AtomicBool::new(false),
            snapshot: Mutex::new(MidiOutputSnapshot {
                connection_versions: BTreeMap::from([("exact-port".into(), 7)]),
                ..Default::default()
            }),
        });
        (
            MidiOutputService {
                sender,
                shared,
                snapshot: MidiOutputSnapshot::default(),
                capacity,
                worker: None,
            },
            receiver,
        )
    }

    #[test]
    fn expanded_queue_budget_and_monitor_generation_preserve_playback_and_new_epochs() {
        let (service, receiver) = service(66);
        let actions = vec![attack(playback(1)), attack(owner(MpeSource::Monitor, 1, 1))];
        let cost = batch_cost(&actions).unwrap();
        assert_eq!(cost, 66);
        service
            .enqueue_mpe_batch(&lower(3), actions, Instant::now(), 1)
            .unwrap();
        assert_eq!(service.shared.pending.load(Ordering::Acquire), cost);
        assert!(matches!(
            service.enqueue("exact-port", &[0x98, 60, 100], Instant::now(), 1),
            Err(MidiOutputError::QueueFull)
        ));
        service.cancel_monitor_mpe().unwrap();
        let Command::Enqueue(event) = receiver.try_recv().unwrap() else {
            panic!("batch")
        };
        let mut queue = vec![event];
        discard_stale_monitor(&mut queue, 2, &service.shared);
        assert_eq!(queue[0].mpe.as_ref().unwrap().actions.len(), 1);
        assert_eq!(
            queue[0].mpe.as_ref().unwrap().actions[0].owner().source,
            MpeSource::Playback
        );
        assert_eq!(service.shared.pending.load(Ordering::Acquire), 33);
        let epoch = service.cancel().unwrap();
        discard_stale_epochs(&mut queue, epoch, &service.shared);
        assert_eq!(service.shared.pending.load(Ordering::Acquire), 0);
        service
            .enqueue_mpe_batch(&lower(1), vec![attack(playback(2))], Instant::now(), epoch)
            .unwrap();
        let Command::Enqueue(event) = receiver.try_recv().unwrap() else {
            panic!("batch")
        };
        queue.push(event);
        discard_stale_epochs(&mut queue, epoch, &service.shared);
        assert_eq!(queue.len(), 1);
        assert_eq!(service.shared.pending.load(Ordering::Acquire), 33);
        let mut too_large = attack(playback(3));
        if let MpeOutputAction::Attack { member, .. } = &mut too_large {
            *member = vec![Control::Bend { value: 8192 }; 67];
        }
        assert!(matches!(
            service.enqueue_mpe_batch(&lower(1), vec![too_large], Instant::now(), epoch),
            Err(MidiOutputError::QueueFull)
        ));
    }

    #[test]
    fn mpe_batches_are_connection_fenced_and_never_use_another_endpoint() {
        let actions = vec![attack(playback(1))];
        let mut event = Scheduled {
            endpoint: "exact-port".into(),
            packet: Packet {
                bytes: [0; 3],
                len: 0,
            },
            deadline: Instant::now(),
            epoch: 1,
            connection_version: 1,
            voice: None,
            mpe: Some(ScheduledMpe {
                profile: lower(1),
                cost: batch_cost(&actions).unwrap(),
                actions,
                monitor_generation: 1,
            }),
        };
        let mut destinations = BTreeMap::from([
            ("exact-port".into(), Destination::new(Wire::default(), 2)),
            ("other-port".into(), Destination::new(Wire::default(), 1)),
        ]);
        assert!(deliver(&mut destinations, &event)
            .unwrap_err()
            .contains("stale"));
        event.endpoint = "missing-port".into();
        assert!(deliver(&mut destinations, &event).is_err());
        event.endpoint = "other-port".into();
        assert!(deliver(&mut destinations, &event)
            .unwrap_err()
            .contains("does not match"));
        assert!(destinations
            .values()
            .all(|destination| destination.sink.packets.is_empty()));
    }

    #[test]
    fn backend_failure_retains_old_and_new_cleanup_ownership_until_success() {
        let mut destination = Destination::new(Wire::default(), 1);
        destination
            .send_mpe(&lower(1), &[attack(playback(1))])
            .unwrap();
        destination.sink.fail = true;
        let next = MpeOutputAction::Attack {
            owner: playback(2),
            pitch: 62,
            velocity: 100,
            member: vec![],
            master: vec![],
        };
        assert!(destination
            .send_mpe(
                &lower(1),
                &[next, MpeOutputAction::End { owner: playback(1) }]
            )
            .is_err());
        assert!(destination.mpe.voices.contains_key(&playback(1)));
        assert!(destination.mpe_recovery_notes.contains(&(1, 60)));
        assert!(destination.mpe_recovery_notes.contains(&(1, 62)));
        assert!(destination.cleanup().is_err());
        assert!(!destination.mpe_recovery_notes.is_empty());
        destination.sink.fail = false;
        destination.cleanup().unwrap();
        assert!(destination.mpe_recovery_notes.is_empty());
        assert!(destination.mpe.voices.is_empty());
        assert!(
            destination.failed,
            "only explicit reconnection clears a failed destination"
        );
        assert!(destination.sink.packets.contains(&vec![0x81, 62, 0]));
    }

    #[test]
    fn normal_end_preserves_release_tail_but_global_cleanup_remembers_used_channels() {
        let mut destination = Destination::new(Wire::default(), 1);
        destination
            .send_mpe(
                &lower(1),
                &[
                    attack(playback(1)),
                    MpeOutputAction::Release {
                        owner: playback(1),
                        velocity: 0,
                    },
                    MpeOutputAction::End { owner: playback(1) },
                ],
            )
            .unwrap();
        assert!(destination.mpe.voices.is_empty());
        assert!(!destination.sink.packets.contains(&vec![0xb1, 120, 0]));
        destination.cleanup().unwrap();
        assert!(destination.sink.packets.contains(&vec![0xb1, 120, 0]));
    }
}
