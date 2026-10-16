//! Ordered decoding for an explicitly declared MPE input source. Ordinary MIDI
//! capture remains separate: this decoder never infers a zone from musical data.
use crate::composition::{Channel, Control, Target};
use crate::recording::CaptureMessage;
use crate::time::Beat;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MpeError {
    #[error("pitch-bend range requires 0–127 semitones and 0–99 cents")]
    BendRange,
    #[error("an MPE zone requires 1–15 member channels")]
    Zone,
    #[error("an MPE source identity must be nonempty and contain no NUL")]
    Source,
    #[error("MPE provenance channel is not a member of its declared zone")]
    Provenance,
    #[error("MPE capture capacity must be positive")]
    InvalidCapacity,
    #[error("MPE capture packet capacity exceeded")]
    PacketCapacity,
    #[error("MPE retained voice capacity exceeded")]
    VoiceCapacity,
    #[error("MPE voice identity range exhausted")]
    IdentityExhausted,
    #[error("MPE input timestamps must not move backwards")]
    Backwards,
    #[error("MPE input value exceeds its MIDI range")]
    MidiRange,
    #[error("notes on the MPE master channel are unsupported")]
    MasterNote,
    #[error("MPE member {member:?} already holds pitch {held_pitch}; cannot attack {pitch}")]
    RepeatedAttack {
        member: Channel,
        held_pitch: u8,
        pitch: u8,
    },
    #[error("MPE member {member:?} has no held key matching pitch {pitch}")]
    UnmatchedRelease { member: Channel, pitch: u8 },
    #[error("unsupported MPE control on channel {channel:?}: {control:?}")]
    UnsupportedControl { channel: Channel, control: Control },
    #[error("unsupported or incomplete MPE RPN selection on channel {0:?}")]
    Rpn(Channel),
    #[error("MPE zone configuration must restate {expected} members, received {actual}")]
    ZoneChange { expected: u8, actual: u8 },
    #[error("invalid MPE decoder checkpoint: {0}")]
    Checkpoint(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "BendRangeData")]
pub struct BendRange {
    semitones: u8,
    cents: u8,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BendRangeData {
    semitones: u8,
    cents: u8,
}
impl BendRange {
    pub fn new(semitones: u8, cents: u8) -> Result<Self, MpeError> {
        if semitones > 127 || cents > 99 {
            return Err(MpeError::BendRange);
        }
        Ok(Self { semitones, cents })
    }
    pub const fn semitones(self) -> u8 {
        self.semitones
    }
    pub const fn cents(self) -> u8 {
        self.cents
    }
}
impl TryFrom<BendRangeData> for BendRange {
    type Error = MpeError;
    fn try_from(value: BendRangeData) -> Result<Self, Self::Error> {
        Self::new(value.semitones, value.cents)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ZoneKind {
    Lower,
    Upper,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ZoneData")]
pub struct MpeZone {
    kind: ZoneKind,
    members: u8,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ZoneData {
    kind: ZoneKind,
    members: u8,
}
impl MpeZone {
    pub fn lower(members: u8) -> Result<Self, MpeError> {
        Self::new(ZoneKind::Lower, members)
    }
    pub fn upper(members: u8) -> Result<Self, MpeError> {
        Self::new(ZoneKind::Upper, members)
    }
    fn new(kind: ZoneKind, members: u8) -> Result<Self, MpeError> {
        if !(1..=15).contains(&members) {
            return Err(MpeError::Zone);
        }
        Ok(Self { kind, members })
    }
    pub const fn members(self) -> u8 {
        self.members
    }
    pub fn master(self) -> Channel {
        Channel::new(match self.kind {
            ZoneKind::Lower => 1,
            ZoneKind::Upper => 16,
        })
        .expect("zone master is valid")
    }
    pub fn contains_member(self, channel: Channel) -> bool {
        match self.kind {
            ZoneKind::Lower => channel.get() >= 2 && channel.get() <= self.members + 1,
            ZoneKind::Upper => channel.get() >= 16 - self.members && channel.get() <= 15,
        }
    }
}
impl TryFrom<ZoneData> for MpeZone {
    type Error = MpeError;
    fn try_from(value: ZoneData) -> Result<Self, Self::Error> {
        Self::new(value.kind, value.members)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ProfileData")]
pub struct MpeProfile {
    source: String,
    zone: MpeZone,
    member_range: BendRange,
    master_range: BendRange,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileData {
    source: String,
    zone: MpeZone,
    member_range: BendRange,
    master_range: BendRange,
}
impl MpeProfile {
    pub fn new(
        source: String,
        zone: MpeZone,
        member_range: BendRange,
        master_range: BendRange,
    ) -> Result<Self, MpeError> {
        if source.trim().is_empty() || source.contains('\0') {
            return Err(MpeError::Source);
        }
        Ok(Self {
            source,
            zone,
            member_range,
            master_range,
        })
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub const fn zone(&self) -> MpeZone {
        self.zone
    }
    pub const fn member_range(&self) -> BendRange {
        self.member_range
    }
    pub const fn master_range(&self) -> BendRange {
        self.master_range
    }
}
impl TryFrom<ProfileData> for MpeProfile {
    type Error = MpeError;
    fn try_from(value: ProfileData) -> Result<Self, Self::Error> {
        Self::new(
            value.source,
            value.zone,
            value.member_range,
            value.master_range,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ProvenanceData")]
pub struct MpeProvenance {
    profile: MpeProfile,
    member: Channel,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProvenanceData {
    profile: MpeProfile,
    member: Channel,
}
impl MpeProvenance {
    pub fn new(profile: MpeProfile, member: Channel) -> Result<Self, MpeError> {
        if !profile.zone.contains_member(member) {
            return Err(MpeError::Provenance);
        }
        Ok(Self { profile, member })
    }
    pub const fn profile(&self) -> &MpeProfile {
        &self.profile
    }
    pub const fn member(&self) -> Channel {
        self.member
    }
}
impl TryFrom<ProvenanceData> for MpeProvenance {
    type Error = MpeError;
    fn try_from(value: ProvenanceData) -> Result<Self, Self::Error> {
        Self::new(value.profile, value.member)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MpeEvent {
    Attack {
        id: u64,
        at: Beat,
        target: Target,
        pitch: u8,
        velocity: u8,
        provenance: MpeProvenance,
        member: Vec<Control>,
        master: Vec<Control>,
    },
    Release {
        id: u64,
        at: Beat,
        velocity: u8,
    },
    Expression {
        id: u64,
        at: Beat,
        master: bool,
        control: Control,
    },
    End {
        id: u64,
        at: Beat,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RpnSelection {
    msb: u8,
    lsb: u8,
}
impl RpnSelection {
    const NULL: Self = Self { msb: 127, lsb: 127 };
    fn valid(self, master: bool) -> bool {
        matches!(self.msb, 0 | 127) && (matches!(self.lsb, 0 | 127) || master && self.lsb == 6)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControllerCache {
    range: BendRange,
    bend: u16,
    // Absence preserves the synth's initial pressure behavior. Inventing zero
    // would silence profiles that map pressure directly to gain.
    pressure: Option<u8>,
    extra: u8,
    rpn: RpnSelection,
}
impl ControllerCache {
    fn new(range: BendRange, master: bool) -> Self {
        Self {
            range,
            bend: 8192,
            pressure: None,
            extra: if master { 0 } else { 64 },
            rpn: RpnSelection::NULL,
        }
    }
    fn range_controls(self) -> Vec<Control> {
        vec![
            cc(101, 0),
            cc(100, 0),
            cc(6, self.range.semitones),
            cc(38, self.range.cents),
            cc(101, 127),
            cc(100, 127),
        ]
    }
    fn snapshot(self, master: bool) -> Vec<Control> {
        let mut values = self.range_controls();
        values.push(Control::Bend { value: self.bend });
        if let Some(value) = self.pressure {
            values.push(Control::Pressure { value });
        }
        values.push(cc(if master { 64 } else { 74 }, self.extra));
        values
    }
    fn validate(self, master: bool) -> Result<(), MpeError> {
        if self.bend > 16383
            || self.pressure.is_some_and(|value| value > 127)
            || self.extra > 127
            || !self.rpn.valid(master)
        {
            return Err(MpeError::Checkpoint(
                "invalid controller cache or RPN selection",
            ));
        }
        Ok(())
    }
    /// Candidate-only update: no live cache changes until the complete incoming
    /// packet is admitted. RPN data emits a complete transaction so an attack's
    /// null selector snapshot cannot disconnect a later source data entry.
    fn update(
        &mut self,
        channel: Channel,
        control: Control,
        master: bool,
        members: u8,
    ) -> Result<Vec<Control>, MpeError> {
        match control {
            Control::Bend { value } => self.bend = value,
            Control::Pressure { value } => self.pressure = Some(value),
            Control::Cc { number: 74, value } if !master => self.extra = value,
            Control::Cc { number: 64, value } if master => self.extra = value,
            Control::Cc { number, value } if number == 101 || number == 100 => {
                let next = if number == 101 {
                    RpnSelection {
                        msb: value,
                        ..self.rpn
                    }
                } else {
                    RpnSelection {
                        lsb: value,
                        ..self.rpn
                    }
                };
                if !next.valid(master) {
                    return Err(MpeError::Rpn(channel));
                }
                self.rpn = next;
                return Ok(Vec::new());
            }
            Control::Cc { number, value } if number == 6 || number == 38 => {
                return self.data_entry(channel, number, value, master, members);
            }
            _ => return Err(MpeError::UnsupportedControl { channel, control }),
        }
        Ok(vec![control])
    }
    fn data_entry(
        &mut self,
        channel: Channel,
        number: u8,
        value: u8,
        master: bool,
        members: u8,
    ) -> Result<Vec<Control>, MpeError> {
        match self.rpn {
            RpnSelection { msb: 0, lsb: 0 } => {
                self.range = if number == 6 {
                    BendRange::new(value, self.range.cents)?
                } else {
                    BendRange::new(self.range.semitones, value)?
                };
                Ok(self.range_controls())
            }
            RpnSelection { msb: 0, lsb: 6 } if master => {
                if number == 6 && value != members {
                    return Err(MpeError::ZoneChange {
                        expected: members,
                        actual: value,
                    });
                }
                if number == 38 && value != 0 {
                    return Err(MpeError::Rpn(channel));
                }
                Ok(Vec::new())
            }
            _ => Err(MpeError::Rpn(channel)),
        }
    }
}
fn cc(number: u8, value: u8) -> Control {
    Control::Cc { number, value }
}
fn index(channel: Channel) -> usize {
    usize::from(channel.get() - 1)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Voice {
    id: u64,
    member: Channel,
    pitch: u8,
    attack_at: Beat,
    released_at: Option<Beat>,
}

/// Immutable ownership evidence for validating a recorder's pending note
/// builders against its decoder. It cannot be used to mutate decoder state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MpeVoiceState {
    id: u64,
    member: Channel,
    pitch: u8,
    attack_at: Beat,
    released_at: Option<Beat>,
}
impl MpeVoiceState {
    pub const fn id(self) -> u64 {
        self.id
    }
    pub const fn member(self) -> Channel {
        self.member
    }
    pub const fn pitch(self) -> u8 {
        self.pitch
    }
    pub const fn attack_at(self) -> Beat {
        self.attack_at
    }
    pub const fn released_at(self) -> Option<Beat> {
        self.released_at
    }
}

/// Portable data is not a live decoder. `recover` checks all cross-field
/// relationships before this checkpoint can accept input or emit events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MpeDecoderCheckpoint {
    version: u32,
    profile: MpeProfile,
    capacity: usize,
    packets: usize,
    next_id: u64,
    last_input: Option<Beat>,
    master: ControllerCache,
    members: [ControllerCache; 16],
    current: [Option<u64>; 16],
    voices: Vec<Voice>,
}

#[derive(Debug, Clone)]
pub struct MpeDecoder {
    profile: MpeProfile,
    capacity: usize,
    packets: usize,
    next_id: u64,
    last_input: Option<Beat>,
    master: ControllerCache,
    members: [ControllerCache; 16],
    current: [Option<u64>; 16],
    voices: BTreeMap<u64, Voice>,
}
impl MpeDecoder {
    pub fn new(profile: MpeProfile, capacity: usize) -> Result<Self, MpeError> {
        if capacity == 0 {
            return Err(MpeError::InvalidCapacity);
        }
        Ok(Self {
            master: ControllerCache::new(profile.master_range, true),
            members: [ControllerCache::new(profile.member_range, false); 16],
            profile,
            capacity,
            packets: 0,
            next_id: 1,
            last_input: None,
            current: [None; 16],
            voices: BTreeMap::new(),
        })
    }
    pub const fn profile(&self) -> &MpeProfile {
        &self.profile
    }
    pub fn active_count(&self) -> usize {
        self.voices.len()
    }
    pub const fn last_input(&self) -> Option<Beat> {
        self.last_input
    }
    pub const fn packets(&self) -> usize {
        self.packets
    }
    pub const fn capacity(&self) -> usize {
        self.capacity
    }
    pub fn voices(&self) -> impl Iterator<Item = MpeVoiceState> + '_ {
        self.voices.values().map(|voice| MpeVoiceState {
            id: voice.id,
            member: voice.member,
            pitch: voice.pitch,
            attack_at: voice.attack_at,
            released_at: voice.released_at,
        })
    }

    /// Filters channels outside the declared zone without touching timestamps or
    /// budgets. Every admitted in-zone packet consumes one capacity unit. All
    /// errors preserve caches, voices, IDs, timestamp, and remaining capacity.
    pub fn input(
        &mut self,
        at: Beat,
        channel: Channel,
        message: CaptureMessage,
        target: Target,
    ) -> Result<Vec<MpeEvent>, MpeError> {
        let master = channel == self.profile.zone.master();
        if !master && !self.profile.zone.contains_member(channel) {
            return Ok(Vec::new());
        }
        self.check_time(at)?;
        let packets = self
            .packets
            .checked_add(1)
            .filter(|count| *count <= self.capacity)
            .ok_or(MpeError::PacketCapacity)?;
        let events = match message {
            CaptureMessage::NoteOn { pitch, velocity: 0 }
            | CaptureMessage::NoteOff { pitch, velocity: 0 } => {
                self.release(at, channel, pitch, 0, master)?
            }
            CaptureMessage::NoteOn { pitch, velocity } => {
                self.attack(at, channel, pitch, velocity, target, master)?
            }
            CaptureMessage::NoteOff { pitch, velocity } => {
                self.release(at, channel, pitch, velocity, master)?
            }
            CaptureMessage::Control(control) => self.control(at, channel, control, master)?,
        };
        self.packets = packets;
        self.last_input = Some(at);
        Ok(events)
    }
    fn check_time(&self, at: Beat) -> Result<(), MpeError> {
        if self.last_input.is_some_and(|last| at < last) {
            return Err(MpeError::Backwards);
        }
        Ok(())
    }
    fn attack(
        &mut self,
        at: Beat,
        member: Channel,
        pitch: u8,
        velocity: u8,
        target: Target,
        master: bool,
    ) -> Result<Vec<MpeEvent>, MpeError> {
        if master {
            return Err(MpeError::MasterNote);
        }
        if pitch > 127 || velocity > 127 {
            return Err(MpeError::MidiRange);
        }
        if let Some(voice) = self.current[index(member)]
            .and_then(|id| self.voices.get(&id))
            .filter(|voice| voice.released_at.is_none())
        {
            return Err(MpeError::RepeatedAttack {
                member,
                held_pitch: voice.pitch,
                pitch,
            });
        }
        if self.voices.len() >= self.capacity {
            return Err(MpeError::VoiceCapacity);
        }
        let id = self.next_id;
        let next_id = id.checked_add(1).ok_or(MpeError::IdentityExhausted)?;
        let provenance = MpeProvenance::new(self.profile.clone(), member)?;
        let event = MpeEvent::Attack {
            id,
            at,
            target,
            pitch,
            velocity,
            provenance,
            member: self.members[index(member)].snapshot(false),
            master: self.master.snapshot(true),
        };
        self.voices.insert(
            id,
            Voice {
                id,
                member,
                pitch,
                attack_at: at,
                released_at: None,
            },
        );
        self.current[index(member)] = Some(id);
        self.next_id = next_id;
        Ok(vec![event])
    }
    fn release(
        &mut self,
        at: Beat,
        member: Channel,
        pitch: u8,
        velocity: u8,
        master: bool,
    ) -> Result<Vec<MpeEvent>, MpeError> {
        if master {
            return Err(MpeError::MasterNote);
        }
        if pitch > 127 || velocity > 127 {
            return Err(MpeError::MidiRange);
        }
        let id = self.current[index(member)]
            .filter(|id| {
                self.voices
                    .get(id)
                    .is_some_and(|voice| voice.pitch == pitch && voice.released_at.is_none())
            })
            .ok_or(MpeError::UnmatchedRelease { member, pitch })?;
        let mut events = vec![MpeEvent::Release { id, at, velocity }];
        if self.master.extra >= 64 {
            self.voices
                .get_mut(&id)
                .expect("validated voice")
                .released_at = Some(at);
        } else {
            self.remove_voice(id);
            events.push(MpeEvent::End { id, at });
        }
        Ok(events)
    }
    fn control(
        &mut self,
        at: Beat,
        channel: Channel,
        control: Control,
        master: bool,
    ) -> Result<Vec<MpeEvent>, MpeError> {
        control.validate().map_err(|_| MpeError::MidiRange)?;
        let mut candidate = if master {
            self.master
        } else {
            self.members[index(channel)]
        };
        let controls = candidate.update(channel, control, master, self.profile.zone.members)?;
        let mut events = Vec::new();
        if master {
            for id in self.voices.keys().copied() {
                append_expression(&mut events, id, at, true, &controls);
            }
            self.master = candidate;
            if matches!(
                control,
                Control::Cc {
                    number: 64,
                    value: 0..=63
                }
            ) {
                self.end_released(at, &mut events);
            }
        } else {
            if let Some(id) = self.current[index(channel)] {
                append_expression(&mut events, id, at, false, &controls);
            }
            self.members[index(channel)] = candidate;
        }
        Ok(events)
    }
    fn end_released(&mut self, at: Beat, events: &mut Vec<MpeEvent>) {
        let released: Vec<_> = self
            .voices
            .values()
            .filter(|voice| voice.released_at.is_some())
            .map(|voice| voice.id)
            .collect();
        for id in released {
            self.remove_voice(id);
            events.push(MpeEvent::End { id, at });
        }
    }
    fn remove_voice(&mut self, id: u64) {
        let voice = self.voices.remove(&id).expect("voice belongs to decoder");
        if self.current[index(voice.member)] == Some(id) {
            self.current[index(voice.member)] = None;
        }
    }
    /// Finalize every retained voice, even after packet capacity is exhausted.
    /// Missing physical releases use velocity zero; already released voices are
    /// never released twice. All releases precede pedal-off expressions and Ends.
    pub fn flush(&mut self, at: Beat) -> Result<Vec<MpeEvent>, MpeError> {
        self.check_time(at)?;
        let mut events = Vec::new();
        for voice in self
            .voices
            .values()
            .filter(|voice| voice.released_at.is_none())
        {
            events.push(MpeEvent::Release {
                id: voice.id,
                at,
                velocity: 0,
            });
        }
        for id in self.voices.keys().copied() {
            events.push(MpeEvent::Expression {
                id,
                at,
                master: true,
                control: cc(64, 0),
            });
        }
        for id in self.voices.keys().copied() {
            events.push(MpeEvent::End { id, at });
        }
        self.voices.clear();
        self.current = [None; 16];
        self.master.extra = 0;
        self.last_input = Some(at);
        Ok(events)
    }
    pub fn checkpoint(&self) -> MpeDecoderCheckpoint {
        MpeDecoderCheckpoint {
            version: 1,
            profile: self.profile.clone(),
            capacity: self.capacity,
            packets: self.packets,
            next_id: self.next_id,
            last_input: self.last_input,
            master: self.master,
            members: self.members,
            current: self.current,
            voices: self.voices.values().cloned().collect(),
        }
    }
    pub fn recover(checkpoint: MpeDecoderCheckpoint) -> Result<Self, MpeError> {
        validate_checkpoint(&checkpoint)?;
        Ok(Self {
            profile: checkpoint.profile,
            capacity: checkpoint.capacity,
            packets: checkpoint.packets,
            next_id: checkpoint.next_id,
            last_input: checkpoint.last_input,
            master: checkpoint.master,
            members: checkpoint.members,
            current: checkpoint.current,
            voices: checkpoint
                .voices
                .into_iter()
                .map(|voice| (voice.id, voice))
                .collect(),
        })
    }
}
fn append_expression(
    events: &mut Vec<MpeEvent>,
    id: u64,
    at: Beat,
    master: bool,
    controls: &[Control],
) {
    events.extend(
        controls
            .iter()
            .copied()
            .map(|control| MpeEvent::Expression {
                id,
                at,
                master,
                control,
            }),
    );
}

fn validate_checkpoint(checkpoint: &MpeDecoderCheckpoint) -> Result<(), MpeError> {
    if checkpoint.version != 1 {
        return Err(MpeError::Checkpoint("unsupported version"));
    }
    if checkpoint.capacity == 0
        || checkpoint.packets > checkpoint.capacity
        || checkpoint.voices.len() > checkpoint.capacity
    {
        return Err(MpeError::Checkpoint("invalid packet or voice capacity"));
    }
    if checkpoint.next_id == 0 || u128::from(checkpoint.next_id - 1) > checkpoint.packets as u128 {
        return Err(MpeError::Checkpoint(
            "voice identity exceeds admitted attacks",
        ));
    }
    if checkpoint.packets > 0 && checkpoint.last_input.is_none() {
        return Err(MpeError::Checkpoint("admitted packets require a timestamp"));
    }
    checkpoint.master.validate(true)?;
    validate_member_caches(checkpoint)?;
    if checkpoint.packets == 0
        && (checkpoint.master != ControllerCache::new(checkpoint.profile.master_range, true)
            || checkpoint.members.iter().any(|cache| {
                *cache != ControllerCache::new(checkpoint.profile.member_range, false)
            }))
    {
        return Err(MpeError::Checkpoint(
            "unobserved controllers require initial cache values",
        ));
    }
    validate_voices(checkpoint)
}
fn validate_member_caches(checkpoint: &MpeDecoderCheckpoint) -> Result<(), MpeError> {
    let initial = ControllerCache::new(checkpoint.profile.member_range, false);
    for (index, cache) in checkpoint.members.iter().copied().enumerate() {
        cache.validate(false)?;
        let channel = Channel::new(index as u8 + 1).expect("fixed channel array");
        if !checkpoint.profile.zone.contains_member(channel) && cache != initial {
            return Err(MpeError::Checkpoint(
                "controller cache outside declared zone",
            ));
        }
    }
    Ok(())
}
fn validate_voices(checkpoint: &MpeDecoderCheckpoint) -> Result<(), MpeError> {
    let mut ids = BTreeMap::new();
    let mut current = [None; 16];
    let mut held = [None; 16];
    let mut released = 0_usize;
    for voice in &checkpoint.voices {
        if voice.id == 0
            || voice.id >= checkpoint.next_id
            || ids.insert(voice.id, voice).is_some()
            || voice.pitch > 127
            || !checkpoint.profile.zone.contains_member(voice.member)
        {
            return Err(MpeError::Checkpoint(
                "invalid voice identity, pitch, or member",
            ));
        }
        if checkpoint
            .last_input
            .is_none_or(|last| voice.attack_at > last)
        {
            return Err(MpeError::Checkpoint("voice attack is after the last input"));
        }
        if let Some(at) = voice.released_at {
            if at < voice.attack_at
                || checkpoint.last_input.is_some_and(|last| at > last)
                || checkpoint.master.extra < 64
            {
                return Err(MpeError::Checkpoint(
                    "released voice has invalid timestamp or no sustaining pedal",
                ));
            }
            released += 1;
        } else if held[index(voice.member)].replace(voice.id).is_some() {
            return Err(MpeError::Checkpoint("multiple physical keys on one member"));
        }
        current[index(voice.member)] =
            Some(current[index(voice.member)].map_or(voice.id, |id: u64| id.max(voice.id)));
    }
    if checkpoint
        .voices
        .len()
        .checked_add(released)
        .is_none_or(|minimum| minimum > checkpoint.packets)
    {
        return Err(MpeError::Checkpoint(
            "voices require more packets than admitted",
        ));
    }
    if current != checkpoint.current {
        return Err(MpeError::Checkpoint(
            "member ownership does not match the newest retained voice",
        ));
    }
    for (index, id) in held.into_iter().enumerate() {
        if id.is_some() && id != current[index] {
            return Err(MpeError::Checkpoint(
                "a held key is not the current member voice",
            ));
        }
    }
    validate_voice_order(&ids)?;
    Ok(())
}
fn validate_voice_order(voices: &BTreeMap<u64, &Voice>) -> Result<(), MpeError> {
    let mut previous_attack = None;
    let mut previous_member: [Option<&Voice>; 16] = [None; 16];
    for voice in voices.values().copied() {
        if previous_attack.is_some_and(|at| voice.attack_at < at) {
            return Err(MpeError::Checkpoint(
                "voice identities disagree with attack order",
            ));
        }
        if previous_member[index(voice.member)]
            .is_some_and(|previous| previous.released_at.is_none_or(|at| at > voice.attack_at))
        {
            return Err(MpeError::Checkpoint(
                "member was reused before its physical release",
            ));
        }
        previous_attack = Some(voice.attack_at);
        previous_member[index(voice.member)] = Some(voice);
    }
    Ok(())
}
