//! The portable musical document. Validate candidates before publishing them.
use crate::time::{Beat, Division, Meter, Tempo, TimeError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;
use thiserror::Error;

macro_rules! identity {
    ($($name:ident),+) => {$(
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(NonZeroU64);
        impl $name {
            pub fn new(value: u64) -> Result<Self, DocumentError> {
                NonZeroU64::new(value).map(Self).ok_or(DocumentError::Invalid("identity must be nonzero"))
            }
            pub const fn get(self) -> u64 { self.0.get() }
        }
    )+};
}
identity!(PhraseId, ChainId, UseId, EventId, InstrumentId, TrackId);

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DocumentError {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("missing {kind} {id}")]
    Missing { kind: &'static str, id: u64 },
    #[error("stale edit: expected revision {expected}, current revision {actual}")]
    Stale { expected: u64, actual: u64 },
    #[error("{0}")]
    Time(#[from] TimeError),
    #[error(transparent)]
    ChordBank(#[from] crate::chord_bank::ChordBankError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct Channel(u8);
impl Channel {
    pub const FIRST: Self = Self(1);
    pub fn new(value: u8) -> Result<Self, DocumentError> {
        if (1..=16).contains(&value) {
            Ok(Self(value))
        } else {
            Err(DocumentError::Invalid("MIDI channel must be 1–16"))
        }
    }
    pub const fn get(self) -> u8 {
        self.0
    }
}
impl TryFrom<u8> for Channel {
    type Error = DocumentError;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        Self::new(v)
    }
}
impl From<Channel> for u8 {
    fn from(v: Channel) -> Self {
        v.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    Instrument {
        id: InstrumentId,
        channel: Channel,
    },
    Channel {
        channel: Channel,
        endpoint: Option<String>,
    },
}
impl Target {
    pub fn instrument(id: InstrumentId) -> Self {
        Self::Instrument {
            id,
            channel: Channel::FIRST,
        }
    }
    pub const fn channel(&self) -> Channel {
        match self {
            Self::Instrument { channel, .. } | Self::Channel { channel, .. } => *channel,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstrumentUse {
    pub name: String,
    pub resource: Option<ResourceRevision>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceRevision {
    pub resource: String,
    pub revision: String,
}

/// Raw performance values survive serialization without percentage conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Control {
    Bend { value: u16 },
    Pressure { value: u8 },
    PolyPressure { pitch: u8, value: u8 },
    Cc { number: u8, value: u8 },
    Program { value: u8 },
}
impl Control {
    pub fn validate(self) -> Result<(), DocumentError> {
        let valid = match self {
            Self::Bend { value } => value <= 16383,
            Self::Pressure { value } | Self::Program { value } => value <= 127,
            Self::PolyPressure { pitch, value } => pitch <= 127 && value <= 127,
            Self::Cc { number, value } => number <= 127 && value <= 127,
        };
        if valid {
            Ok(())
        } else {
            Err(DocumentError::Invalid(
                "performance value exceeds MIDI range",
            ))
        }
    }
}
pub const MAX_EXPRESSION_SEQUENCE: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expression {
    pub offset: Beat,
    pub control: Control,
    /// Source order across master and member scopes at equal timestamps.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_expression_sequence"
    )]
    pub sequence: Option<u64>,
}

fn deserialize_expression_sequence<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u64>, D::Error> {
    let sequence = Option::<u64>::deserialize(deserializer)?;
    if sequence.is_some_and(|value| value > MAX_EXPRESSION_SEQUENCE) {
        return Err(serde::de::Error::custom(
            "expression sequence exceeds exact JSON integer range",
        ));
    }
    Ok(sequence)
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Note {
    pub pitch: u8,
    pub velocity: u8,
    pub release_velocity: u8,
    pub duration: Beat,
    pub expression: Vec<Expression>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub master_expression: Vec<Expression>,
    /// Owned control lifetime; absent legacy values follow physical key duration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression_duration: Option<Beat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mpe: Option<crate::mpe::MpeProvenance>,
}
impl Note {
    pub fn new(pitch: u8, velocity: u8, duration: Beat) -> Result<Self, DocumentError> {
        let note = Self {
            pitch,
            velocity,
            release_velocity: 0,
            duration,
            expression: Vec::new(),
            master_expression: Vec::new(),
            expression_duration: None,
            mpe: None,
        };
        note.validate()?;
        Ok(note)
    }
    pub fn expression_duration(&self) -> Beat {
        self.expression_duration.unwrap_or(self.duration)
    }
    pub fn validate(&self) -> Result<(), DocumentError> {
        if self.pitch > 127
            || !(1..=127).contains(&self.velocity)
            || self.release_velocity > 127
            || self.duration.is_zero()
        {
            return Err(DocumentError::Invalid(
                "note requires MIDI pitch, positive velocity and duration",
            ));
        }
        if self.expression_duration() < self.duration {
            return Err(DocumentError::Invalid(
                "expression lifetime cannot end before physical key release",
            ));
        }
        if !self.master_expression.is_empty() && self.mpe.is_none() {
            return Err(DocumentError::Invalid(
                "master expression requires explicit MPE provenance",
            ));
        }
        if self.mpe.is_some() && self.expression_duration.is_none() {
            return Err(DocumentError::Invalid(
                "MPE notes require an explicit expression lifetime",
            ));
        }
        for point in self.expression.iter().chain(&self.master_expression) {
            point.control.validate()?;
            if point
                .sequence
                .is_some_and(|value| value > MAX_EXPRESSION_SEQUENCE)
            {
                return Err(DocumentError::Invalid(
                    "expression sequence exceeds exact JSON integer range",
                ));
            }
            if point.offset > self.expression_duration() {
                return Err(DocumentError::Invalid(
                    "expression lies after its owned control lifetime",
                ));
            }
        }
        Ok(())
    }

    /// Explicit key-length edits preserve relative control offsets. Shortening
    /// also trims the owned tail; lengthening never stretches recorded curves.
    /// Invalid edits leave this note unchanged.
    pub fn resize(&mut self, duration: Beat) -> Result<(), DocumentError> {
        self.validate()?;
        let mut candidate = self.clone();
        candidate.duration = duration;
        if duration < self.duration {
            candidate.trim_owned_lifetime(duration)?;
        } else if let Some(lifetime) = &mut candidate.expression_duration {
            *lifetime = (*lifetime).max(duration);
        }
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    /// Phrase truncation can shorten a sustained tail without lengthening its
    /// already released key. A trimmed MPE tail closes both pedal scopes.
    pub fn trim_lifetime(&mut self, maximum: Beat) -> Result<(), DocumentError> {
        self.validate()?;
        if maximum >= self.expression_duration() {
            return Ok(());
        }
        let mut candidate = self.clone();
        candidate.duration = candidate.duration.min(maximum);
        candidate.trim_owned_lifetime(maximum)?;
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    /// Division retiming scales both lifetimes and both curves as one edit.
    /// Raw MIDI values, provenance and curve order remain unchanged.
    pub fn retime(&mut self, numerator: u64, denominator: u64) -> Result<(), DocumentError> {
        self.validate()?;
        let mut candidate = self.clone();
        candidate.duration = self.duration.checked_scale(numerator, denominator)?;
        candidate.expression_duration = self
            .expression_duration
            .map(|lifetime| lifetime.checked_scale(numerator, denominator))
            .transpose()?;
        for point in candidate
            .expression
            .iter_mut()
            .chain(&mut candidate.master_expression)
        {
            point.offset = point.offset.checked_scale(numerator, denominator)?;
        }
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    /// Quantizes physical key duration only. MPE controls retain their separate
    /// lifetime; ordinary notes refuse any edit that would cut their expression.
    pub fn quantize_duration(&mut self, grid: Beat) -> Result<(), DocumentError> {
        self.validate()?;
        let duration = self.duration.quantized(grid)?;
        if duration.is_zero()
            || (self.mpe.is_none()
                && self
                    .expression
                    .iter()
                    .chain(&self.master_expression)
                    .any(|point| point.offset > duration))
        {
            return Err(DocumentError::Invalid(
                "duration quantize would remove note or expression",
            ));
        }
        let mut candidate = self.clone();
        candidate.duration = duration;
        if let Some(lifetime) = &mut candidate.expression_duration {
            *lifetime = (*lifetime).max(duration);
        }
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    /// Called only on an unpublished candidate whose new physical duration has
    /// already been assigned. Preserve legacy absence of an explicit lifetime.
    fn trim_owned_lifetime(&mut self, lifetime: Beat) -> Result<(), DocumentError> {
        if self.expression_duration.is_some() {
            self.expression_duration = Some(lifetime);
        }
        trim_expression_curve(&mut self.expression, lifetime);
        trim_expression_curve(&mut self.master_expression, lifetime);
        if self.mpe.is_some() {
            let maximum = self
                .expression
                .iter()
                .chain(&self.master_expression)
                .filter_map(|point| point.sequence)
                .max();
            let member = maximum
                .map_or(Some(0), |sequence| sequence.checked_add(1))
                .filter(|sequence| *sequence <= MAX_EXPRESSION_SEQUENCE)
                .ok_or(DocumentError::Invalid("expression sequence exhausted"))?;
            let master = member
                .checked_add(1)
                .filter(|sequence| *sequence <= MAX_EXPRESSION_SEQUENCE)
                .ok_or(DocumentError::Invalid("expression sequence exhausted"))?;
            self.expression.push(Expression {
                offset: lifetime,
                control: Control::Cc {
                    number: 64,
                    value: 0,
                },
                sequence: Some(member),
            });
            self.master_expression.push(Expression {
                offset: lifetime,
                control: Control::Cc {
                    number: 64,
                    value: 0,
                },
                sequence: Some(master),
            });
        }
        Ok(())
    }
}

fn trim_expression_curve(curve: &mut Vec<Expression>, lifetime: Beat) {
    curve.retain(|point| point.offset <= lifetime);
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Fx {
    Offset {
        semitones: i8,
    },
    Velocity {
        value: u8,
    },
    Gate {
        fraction: Beat,
    },
    Cut {
        after: Beat,
    },
    Delay {
        beats: Beat,
    },
    Retrigger {
        count: u16,
        spacing: Beat,
    },
    Chance {
        percent: u8,
    },
    Control {
        control: Control,
    },
    Send {
        bus: u32,
        amount: u8,
        duration: Beat,
    },
    Jump {
        step: u32,
    },
    Tempo {
        tempo: Tempo,
    },
    Chord {
        intervals: Vec<i8>,
    },
    ChordRevision {
        revision: crate::chord_bank::ChordRevisionId,
    },
    Add {
        intervals: Vec<i8>,
    },
    Voicing {
        inversion: u8,
        open: bool,
    },
    Spread {
        voices: u8,
    },
}
impl Fx {
    fn validate(&self, steps: u32) -> Result<(), DocumentError> {
        let valid = match self {
            Self::Velocity { value } => (1..=127).contains(value),
            Self::Gate { fraction } => !fraction.is_zero(),
            Self::Delay { beats } => !beats.is_zero(),
            Self::Retrigger { count, spacing } => *count > 0 && !spacing.is_zero(),
            Self::Chance { percent } => *percent <= 100,
            Self::Control { control } => return control.validate(),
            Self::Send {
                amount, duration, ..
            } => *amount <= 100 && !duration.is_zero(),
            Self::Jump { step } => *step < steps,
            Self::Chord { intervals } => crate::harmony::Chord::new(intervals.clone()).is_ok(),
            Self::Add { intervals } => {
                !intervals.is_empty() && intervals.iter().all(|v| (-12..=12).contains(v))
            }
            _ => true,
        };
        if valid {
            Ok(())
        } else {
            Err(DocumentError::Invalid(
                "FX value is outside its supported range",
            ))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub id: EventId,
    pub at: Beat,
    pub target: Target,
    pub note: Option<Note>,
    pub controls: Vec<Control>,
    pub fx: [Option<Fx>; 3],
}
impl Event {
    /// A note-less zero Cut is the tracker OFF marker. It releases earlier
    /// voices owned by this arrangement track and target, including chords.
    pub fn note_off(id: EventId, at: Beat, target: Target) -> Self {
        Self {
            id,
            at,
            target,
            note: None,
            controls: vec![],
            fx: [Some(Fx::Cut { after: Beat::ZERO }), None, None],
        }
    }
    pub fn is_note_off(&self) -> bool {
        self.note.is_none()
            && self
                .fx
                .iter()
                .flatten()
                .any(|fx| matches!(fx, Fx::Cut { after } if after.is_zero()))
    }
    pub fn note(id: EventId, at: Beat, target: Target, note: Note) -> Self {
        Self {
            id,
            at,
            target,
            note: Some(note),
            controls: Vec::new(),
            fx: [None, None, None],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PhraseForm {
    Tracker,
    PianoRoll,
    Drum,
    Euclidean {
        /// Optional pitches in unrotated pulse order. Missing assignments use
        /// `pitch`; assignments survive reducing and restoring pulse count.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pitches: Vec<Option<u8>>,
        pulses: u32,
        rotation: u32,
        pitch: u8,
        velocity: u8,
        gate: Beat,
        target: Target,
    },
    Random {
        seed: u64,
        #[serde(default = "random_varies")]
        vary: bool,
        density: u8,
        low: u8,
        high: u8,
        velocity_low: u8,
        velocity_high: u8,
        gate: Beat,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gate_high: Option<Beat>,
        target: Target,
    },
}
fn random_varies() -> bool {
    true
}
pub use crate::harmony::Scale as Key;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Phrase {
    pub name: String,
    pub steps: u32,
    pub division: Division,
    pub key: Option<Key>,
    pub form: PhraseForm,
    pub events: Vec<Event>,
    pub seed: u64,
}
impl Default for Phrase {
    fn default() -> Self {
        Self {
            name: "Phrase".into(),
            steps: 16,
            division: Division::new(16, 1, 1).expect("valid default division"),
            key: None,
            form: PhraseForm::Tracker,
            events: Vec::new(),
            seed: 1,
        }
    }
}
impl Phrase {
    pub fn length(&self) -> Result<Beat, TimeError> {
        self.division
            .beat_length()
            .checked_scale(u64::from(self.steps), 1)
    }
    fn validate(&self, song: &Song) -> Result<(), DocumentError> {
        validate_name(&self.name)?;
        if self.steps == 0 {
            return Err(DocumentError::Invalid("phrase length must be positive"));
        }
        let length = self.length()?;
        let mut ids = BTreeSet::new();
        for event in &self.events {
            if !ids.insert(event.id) || event.at >= length {
                return Err(DocumentError::Invalid(
                    "duplicate event identity or event outside phrase",
                ));
            }
            song.validate_target(&event.target)?;
            if let Some(note) = &event.note {
                note.validate()?;
            }
            for control in &event.controls {
                control.validate()?;
            }
            for fx in event.fx.iter().flatten() {
                fx.validate(self.steps)?;
                if let Fx::ChordRevision { revision } = fx {
                    require(
                        song.chords.get(revision).is_some(),
                        "chord revision",
                        revision.get(),
                    )?;
                }
            }
        }
        self.validate_generator(song)
    }
    fn validate_generator(&self, song: &Song) -> Result<(), DocumentError> {
        let valid = match &self.form {
            PhraseForm::Euclidean {
                pitches,
                pulses,
                rotation,
                pitch,
                velocity,
                gate,
                target,
            } => {
                song.validate_target(target)?;
                *pulses <= self.steps
                    && *rotation < self.steps
                    && *pitch <= 127
                    && pitches.iter().flatten().all(|pitch| *pitch <= 127)
                    && (1..=127).contains(velocity)
                    && !gate.is_zero()
            }
            PhraseForm::Random {
                density,
                low,
                high,
                velocity_low,
                velocity_high,
                gate,
                gate_high,
                target,
                ..
            } => {
                song.validate_target(target)?;
                *density <= 100
                    && low <= high
                    && *high <= 127
                    && (*low..=*high).any(|pitch| {
                        crate::harmony::Pitch::new(pitch).is_ok_and(|pitch| {
                            self.key.as_ref().unwrap_or(&song.key).contains(pitch)
                        })
                    })
                    && *velocity_low > 0
                    && velocity_low <= velocity_high
                    && *velocity_high <= 127
                    && !gate.is_zero()
                    && gate_high.is_none_or(|high| high >= *gate)
            }
            _ => true,
        };
        if valid {
            Ok(())
        } else {
            Err(DocumentError::Invalid("invalid generator parameters"))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhraseUse {
    pub id: UseId,
    pub phrase: PhraseId,
    pub repeats: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chain {
    pub name: String,
    pub phrases: Vec<PhraseUse>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainUse {
    pub id: UseId,
    pub chain: ChainId,
    pub repeats: u32,
}

/// Stable arrangement occurrence selected by an Expanded view or saved group.
/// A saved selection may outlive its source; consumers must show missing sources
/// rather than silently substituting another track or chain occurrence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainSelection {
    pub track: TrackId,
    pub chain_use: UseId,
}

/// The same finite selection budget applies to live Expanded views and groups.
pub const MAX_CHAIN_SELECTIONS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Track {
    pub id: TrackId,
    pub name: String,
    pub slots: Vec<Option<ChainUse>>,
    pub looped: bool,
    pub muted: bool,
}
impl Track {
    /// Materialize an edited arrangement cell; navigation alone adds no data.
    pub fn ensure_slot(&mut self, slot: usize) -> Result<(), DocumentError> {
        let length = slot
            .checked_add(1)
            .ok_or(DocumentError::Invalid("slot overflow"))?;
        if length > self.slots.len() {
            self.slots
                .try_reserve(length - self.slots.len())
                .map_err(|_| DocumentError::Invalid("cannot allocate arrangement slots"))?;
            self.slots.resize(length, None);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeterChange {
    pub at: Beat,
    pub meter: Meter,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Song {
    pub name: String,
    pub tempo: Tempo,
    pub key: Key,
    pub meters: Vec<MeterChange>,
    pub tracks: Vec<Track>,
    pub chains: BTreeMap<ChainId, Chain>,
    pub phrases: BTreeMap<PhraseId, Phrase>,
    pub instruments: BTreeMap<InstrumentId, InstrumentUse>,
    #[serde(default)]
    pub chords: crate::chord_bank::ChordBank,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub view_groups: BTreeMap<String, Vec<ChainSelection>>,
}
impl Default for Song {
    fn default() -> Self {
        Self {
            name: "Untitled".into(),
            tempo: Tempo::DEFAULT,
            key: Key::default(),
            meters: vec![MeterChange {
                at: Beat::ZERO,
                meter: Meter::COMMON,
            }],
            tracks: (1..=16)
                .map(|id| Track {
                    id: TrackId::new(id).expect("nonzero default id"),
                    name: format!("Track {id:02}"),
                    slots: Vec::new(),
                    looped: false,
                    muted: false,
                })
                .collect(),
            chains: BTreeMap::new(),
            phrases: BTreeMap::new(),
            instruments: BTreeMap::new(),
            chords: Default::default(),
            view_groups: BTreeMap::new(),
        }
    }
}
impl Song {
    pub fn validate(&self) -> Result<(), DocumentError> {
        validate_name(&self.name)?;
        if self.tracks.is_empty() || self.tracks.len() > 16 {
            return Err(DocumentError::Invalid("Song requires 1–16 tracks"));
        }
        if self.meters.first().map(|m| m.at) != Some(Beat::ZERO)
            || self.meters.windows(2).any(|v| v[0].at >= v[1].at)
        {
            return Err(DocumentError::Invalid(
                "meter map must start at zero and increase",
            ));
        }
        for pair in self.meters.windows(2) {
            let elapsed = pair[1].at.checked_sub(pair[0].at)?;
            let bar = pair[0].meter.bar_length();
            if bar.checked_scale(elapsed.units(bar)?, 1)? != elapsed {
                return Err(DocumentError::Invalid(
                    "meter change must fall on a preceding-meter bar boundary",
                ));
            }
        }
        let mut track_ids = BTreeSet::new();
        let mut uses = BTreeSet::new();
        for track in &self.tracks {
            validate_name(&track.name)?;
            if !track_ids.insert(track.id) {
                return Err(DocumentError::Invalid("duplicate track identity"));
            }
            for slot in track.slots.iter().flatten() {
                validate_use(slot.id, slot.repeats, &mut uses)?;
                require(
                    self.chains.contains_key(&slot.chain),
                    "chain",
                    slot.chain.get(),
                )?;
            }
        }
        for chain in self.chains.values() {
            validate_name(&chain.name)?;
            for usage in &chain.phrases {
                validate_use(usage.id, usage.repeats, &mut uses)?;
                require(
                    self.phrases.contains_key(&usage.phrase),
                    "phrase",
                    usage.phrase.get(),
                )?;
            }
        }
        for phrase in self.phrases.values() {
            phrase.validate(self)?;
        }
        for instrument in self.instruments.values() {
            validate_name(&instrument.name)?;
        }
        for (name, selections) in &self.view_groups {
            validate_name(name)?;
            if selections.is_empty() || selections.len() > MAX_CHAIN_SELECTIONS {
                return Err(DocumentError::Invalid(
                    "view group requires 1–16 selected chains",
                ));
            }
            let mut unique = BTreeSet::new();
            if selections
                .iter()
                .any(|selection| !unique.insert(*selection))
            {
                return Err(DocumentError::Invalid("duplicate view group selection"));
            }
            // Missing references are intentional saved state. The view exposes
            // them so structural edits do not silently rewrite a named group.
        }
        Ok(())
    }
    pub fn validate_target(&self, target: &Target) -> Result<(), DocumentError> {
        match target {
            Target::Instrument { id, .. } => {
                require(self.instruments.contains_key(id), "instrument", id.get())
            }
            Target::Channel {
                endpoint: Some(endpoint),
                ..
            } if endpoint.is_empty() => {
                Err(DocumentError::Invalid("endpoint identity cannot be empty"))
            }
            _ => Ok(()),
        }
    }
    pub fn phrase_references(&self, phrase: PhraseId) -> Vec<(ChainId, UseId)> {
        self.chains
            .iter()
            .flat_map(|(id, chain)| {
                chain
                    .phrases
                    .iter()
                    .filter(move |usage| usage.phrase == phrase)
                    .map(move |usage| (*id, usage.id))
            })
            .collect()
    }
    pub fn next_identity(&self) -> Result<u64, DocumentError> {
        let maximum = self
            .phrases
            .keys()
            .map(|id| id.get())
            .chain(self.chords.keys().map(|id| id.get()))
            .chain(self.chains.keys().map(|id| id.get()))
            .chain(self.instruments.keys().map(|id| id.get()))
            .chain(self.tracks.iter().map(|track| track.id.get()))
            .chain(
                self.tracks
                    .iter()
                    .flat_map(|track| track.slots.iter().flatten().map(|slot| slot.id.get())),
            )
            .chain(
                self.chains
                    .values()
                    .flat_map(|chain| chain.phrases.iter().map(|usage| usage.id.get())),
            )
            .chain(
                self.phrases
                    .values()
                    .flat_map(|phrase| phrase.events.iter().map(|event| event.id.get())),
            )
            .chain(self.view_groups.values().flatten().flat_map(|selection| {
                // Do not recycle deleted occurrence identities still retained
                // by groups and make those selections point to new content.
                [selection.track.get(), selection.chain_use.get()]
            }))
            .max()
            .unwrap_or(0);
        maximum
            .checked_add(1)
            .ok_or(DocumentError::Invalid("identity range exhausted"))
    }
}
pub(crate) fn validate_name(name: &str) -> Result<(), DocumentError> {
    if name.trim().is_empty() || name.contains('\0') {
        Err(DocumentError::Invalid(
            "name must be nonempty and contain no NUL",
        ))
    } else {
        Ok(())
    }
}
fn validate_use(id: UseId, repeats: u32, uses: &mut BTreeSet<UseId>) -> Result<(), DocumentError> {
    if repeats == 0 || !uses.insert(id) {
        Err(DocumentError::Invalid(
            "duplicate use identity or zero repeats",
        ))
    } else {
        Ok(())
    }
}
fn require(exists: bool, kind: &'static str, id: u64) -> Result<(), DocumentError> {
    if exists {
        Ok(())
    } else {
        Err(DocumentError::Missing { kind, id })
    }
}

#[cfg(test)]
mod view_group_contract_tests {
    use super::*;

    fn selection(track: u64, chain_use: u64) -> ChainSelection {
        ChainSelection {
            track: TrackId::new(track).unwrap(),
            chain_use: UseId::new(chain_use).unwrap(),
        }
    }

    #[test]
    fn legacy_json_omits_empty_groups_and_named_selections_round_trip_in_order() {
        let original = Song::default();
        let legacy = serde_json::to_value(&original).unwrap();
        assert!(legacy.get("view_groups").is_none());
        let restored: Song = serde_json::from_value(legacy).unwrap();
        assert_eq!(restored, original);
        restored.validate().unwrap();

        let mut song = original;
        let selections = vec![selection(99, 102), selection(1, 101)];
        song.view_groups
            .insert("Verse focus".into(), selections.clone());
        song.validate().unwrap();
        let encoded = serde_json::to_value(&song).unwrap();
        assert_eq!(
            encoded["view_groups"]["Verse focus"][0],
            serde_json::json!({"track":99,"chain_use":102})
        );
        let decoded: Song = serde_json::from_value(encoded).unwrap();
        decoded.validate().unwrap();
        assert_eq!(decoded, song);
        assert_eq!(decoded.view_groups["Verse focus"], selections);
        assert!(decoded.tracks.iter().all(|track| track.id.get() != 99));
        assert_eq!(decoded.next_identity().unwrap(), 103);
    }

    #[test]
    fn view_group_validation_bounds_each_selection_without_capping_group_count() {
        let mut song = Song::default();
        for index in 0..65 {
            song.view_groups
                .insert(format!("Group {index}"), vec![selection(99, 100)]);
        }
        song.validate().unwrap();
        let full: Vec<_> = (1..=MAX_CHAIN_SELECTIONS as u64)
            .map(|id| selection(99, id))
            .collect();
        song.view_groups.insert("Full".into(), full.clone());
        song.validate().unwrap();

        for (name, selections) in [
            (" ".to_owned(), vec![selection(1, 2)]),
            ("bad\0name".to_owned(), vec![selection(1, 2)]),
            ("Empty".to_owned(), vec![]),
            (
                "Duplicate".to_owned(),
                vec![selection(1, 2), selection(1, 2)],
            ),
            (
                "Oversize".to_owned(),
                full.into_iter().chain([selection(99, 100)]).collect(),
            ),
        ] {
            let mut candidate = song.clone();
            candidate.view_groups.insert(name, selections);
            assert!(candidate.validate().is_err());
        }
    }

    #[test]
    fn selection_serialization_rejects_zero_identities_and_unknown_fields() {
        for value in [
            serde_json::json!({"track":0,"chain_use":1}),
            serde_json::json!({"track":1,"chain_use":0}),
            serde_json::json!({"track":1,"chain_use":2,"slot":0}),
        ] {
            assert!(serde_json::from_value::<ChainSelection>(value).is_err());
        }
    }
}

#[cfg(test)]
mod note_lifetime_tests {
    use super::*;
    use crate::mpe::{BendRange, MpeProfile, MpeProvenance, MpeZone};

    fn beat(n: u64, d: u64) -> Beat {
        Beat::new(n, d).unwrap()
    }
    fn mpe_note() -> Note {
        let profile = MpeProfile::new(
            "physical-input-17".into(),
            MpeZone::lower(4).unwrap(),
            BendRange::new(48, 25).unwrap(),
            BendRange::new(2, 0).unwrap(),
        )
        .unwrap();
        let mut note = Note::new(64, 99, beat(1, 2)).unwrap();
        note.release_velocity = 37;
        note.mpe = Some(MpeProvenance::new(profile, Channel::new(2).unwrap()).unwrap());
        note.expression_duration = Some(Beat::integer(2));
        note.expression = vec![
            Expression {
                offset: Beat::ZERO,
                control: Control::Bend { value: 9001 },
                sequence: Some(0),
            },
            Expression {
                offset: beat(3, 4),
                control: Control::Cc {
                    number: 74,
                    value: 91,
                },
                sequence: Some(3),
            },
            Expression {
                offset: beat(3, 2),
                control: Control::Cc {
                    number: 64,
                    value: 127,
                },
                sequence: Some(5),
            },
        ];
        note.master_expression = vec![
            Expression {
                offset: Beat::ZERO,
                control: Control::Bend { value: 7901 },
                sequence: Some(1),
            },
            Expression {
                offset: beat(3, 4),
                control: Control::Pressure { value: 19 },
                sequence: Some(2),
            },
            Expression {
                offset: beat(7, 4),
                control: Control::Cc {
                    number: 64,
                    value: 0,
                },
                sequence: Some(6),
            },
        ];
        note.validate().unwrap();
        note
    }

    #[test]
    fn legacy_notes_and_unsequenced_points_serialize_byte_for_byte_unchanged() {
        let legacy = r#"{"pitch":60,"velocity":99,"release_velocity":17,"duration":{"numerator":1,"denominator":1},"expression":[{"offset":{"numerator":1,"denominator":3},"control":{"kind":"bend","value":8291}}]}"#;
        let note: Note = serde_json::from_str(legacy).unwrap();
        note.validate().unwrap();
        assert_eq!(note.expression_duration(), Beat::ONE);
        assert_eq!(note.expression_duration, None);
        assert!(note.master_expression.is_empty());
        assert_eq!(note.mpe, None);
        assert_eq!(note.expression[0].sequence, None);
        assert_eq!(serde_json::to_string(&note).unwrap(), legacy);
    }

    #[test]
    fn sustained_master_and_member_curves_roundtrip_beyond_physical_release() {
        let note = mpe_note();
        let encoded = serde_json::to_vec(&note).unwrap();
        let decoded: Note = serde_json::from_slice(&encoded).unwrap();
        decoded.validate().unwrap();
        assert_eq!(decoded, note);
        assert!(decoded.expression[1].offset > decoded.duration);
        assert!(decoded.master_expression[2].offset > decoded.duration);
        assert_eq!(decoded.expression_duration(), Beat::integer(2));
    }

    #[test]
    fn invalid_control_lifetimes_and_unowned_master_curves_are_rejected() {
        let valid = mpe_note();
        let mut invalid = valid.clone();
        invalid.expression_duration = None;
        assert!(invalid
            .validate()
            .unwrap_err()
            .to_string()
            .contains("explicit expression lifetime"));
        invalid = valid.clone();
        invalid.expression_duration = Some(beat(1, 4));
        assert!(invalid
            .validate()
            .unwrap_err()
            .to_string()
            .contains("physical key release"));
        invalid = valid.clone();
        invalid.mpe = None;
        assert!(invalid
            .validate()
            .unwrap_err()
            .to_string()
            .contains("provenance"));
        invalid = valid.clone();
        invalid.expression[0].offset = Beat::integer(3);
        assert!(invalid.validate().is_err());
        invalid = valid.clone();
        invalid.master_expression[0].offset = Beat::integer(3);
        assert!(invalid.validate().is_err());
        invalid = valid;
        invalid.master_expression[0].control = Control::Pressure { value: 128 };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn explicit_key_shortening_trims_both_scopes_and_closes_pedals_after_retained_points() {
        let original = mpe_note();
        let mut note = original.clone();
        note.resize(beat(1, 4)).unwrap();
        assert_eq!(note.duration, beat(1, 4));
        assert_eq!(note.expression_duration(), beat(1, 4));
        assert_eq!(note.expression[0], original.expression[0]);
        assert_eq!(note.master_expression[0], original.master_expression[0]);
        assert_eq!(
            note.expression.last().unwrap(),
            &Expression {
                offset: beat(1, 4),
                control: Control::Cc {
                    number: 64,
                    value: 0
                },
                sequence: Some(2)
            }
        );
        assert_eq!(
            note.master_expression.last().unwrap(),
            &Expression {
                offset: beat(1, 4),
                control: Control::Cc {
                    number: 64,
                    value: 0
                },
                sequence: Some(3)
            }
        );
        assert_eq!(note.expression.len(), 2);
        assert_eq!(note.master_expression.len(), 2);
        assert_eq!(note.release_velocity, original.release_velocity);
        assert_eq!(note.mpe, original.mpe);
    }

    #[test]
    fn phrase_tail_trim_keeps_physical_release_and_orders_terminal_pedals() {
        let original = mpe_note();
        let mut note = original.clone();
        note.trim_lifetime(Beat::ONE).unwrap();
        assert_eq!(note.duration, original.duration);
        assert_eq!(note.expression_duration(), Beat::ONE);
        assert_eq!(&note.expression[..2], &original.expression[..2]);
        assert_eq!(
            &note.master_expression[..2],
            &original.master_expression[..2]
        );
        assert_eq!(note.expression.last().unwrap().sequence, Some(4));
        assert_eq!(note.master_expression.last().unwrap().sequence, Some(5));
        assert_eq!(note.expression.last().unwrap().offset, Beat::ONE);
        assert_eq!(
            note.master_expression.last().unwrap().control,
            Control::Cc {
                number: 64,
                value: 0
            }
        );
    }

    #[test]
    fn lengthening_keeps_raw_curve_times_and_grows_only_necessary_lifetime() {
        let original = mpe_note();
        let mut note = original.clone();
        note.resize(Beat::ONE).unwrap();
        assert_eq!(note.expression_duration(), Beat::integer(2));
        note.resize(Beat::integer(3)).unwrap();
        assert_eq!(note.expression_duration(), Beat::integer(3));
        assert_eq!(note.expression, original.expression);
        assert_eq!(note.master_expression, original.master_expression);
        let mut legacy = Note::new(60, 90, Beat::ONE).unwrap();
        legacy.resize(Beat::integer(2)).unwrap();
        assert_eq!(legacy.expression_duration, None);
        assert_eq!(legacy.expression_duration(), Beat::integer(2));
    }

    #[test]
    fn retime_scales_both_lifetimes_and_curves_without_changing_raw_values_or_order() {
        let original = mpe_note();
        let mut note = original.clone();
        note.retime(3, 2).unwrap();
        assert_eq!(note.duration, beat(3, 4));
        assert_eq!(note.expression_duration(), Beat::integer(3));
        for (before, after) in original
            .expression
            .iter()
            .chain(&original.master_expression)
            .zip(note.expression.iter().chain(&note.master_expression))
        {
            assert_eq!(after.offset, before.offset.checked_scale(3, 2).unwrap());
            assert_eq!(after.control, before.control);
            assert_eq!(after.sequence, before.sequence);
        }
        assert_eq!(note.mpe, original.mpe);
        assert_eq!(note.release_velocity, original.release_velocity);
    }

    #[test]
    fn duration_quantize_keeps_mpe_tail_but_rejects_ordinary_expression_loss() {
        let mut note = mpe_note();
        note.duration = beat(3, 5);
        let raw = note.clone();
        note.quantize_duration(beat(1, 2)).unwrap();
        assert_eq!(note.duration, beat(1, 2));
        assert_eq!(note.expression_duration, raw.expression_duration);
        assert_eq!(note.expression, raw.expression);
        assert_eq!(note.master_expression, raw.master_expression);
        let mut short = mpe_note();
        short.expression.clear();
        short.master_expression.clear();
        short.duration = beat(3, 4);
        short.expression_duration = Some(beat(3, 4));
        short.quantize_duration(Beat::ONE).unwrap();
        assert_eq!(short.duration, Beat::ONE);
        assert_eq!(short.expression_duration(), Beat::ONE);
        let mut ordinary = Note::new(60, 100, beat(3, 5)).unwrap();
        ordinary.expression.push(Expression {
            offset: beat(11, 20),
            control: Control::Bend { value: 8193 },
            sequence: None,
        });
        let previous = ordinary.clone();
        assert!(ordinary.quantize_duration(beat(1, 2)).is_err());
        assert_eq!(ordinary, previous);
    }

    #[test]
    fn failed_note_edits_never_publish_partial_curves_or_lifetimes() {
        let original = mpe_note();
        let mut note = original.clone();
        assert!(note.resize(Beat::ZERO).is_err());
        assert_eq!(note, original);
        assert!(note.trim_lifetime(Beat::ZERO).is_err());
        assert_eq!(note, original);
        assert!(note.retime(1, 0).is_err());
        assert_eq!(note, original);
        assert!(note.retime(u64::MAX, 1).is_err());
        assert_eq!(note, original);
        assert!(note.quantize_duration(Beat::integer(3)).is_err());
        assert_eq!(note, original);
        note.expression[0].sequence = Some(MAX_EXPRESSION_SEQUENCE);
        let exhausted = note.clone();
        assert!(note
            .trim_lifetime(Beat::ONE)
            .unwrap_err()
            .to_string()
            .contains("sequence exhausted"));
        assert_eq!(note, exhausted);
    }
    #[test]
    fn expression_sequence_is_exact_through_json_and_rejects_unsafe_integers() {
        let point = Expression {
            offset: Beat::ZERO,
            control: Control::Pressure { value: 73 },
            sequence: Some(MAX_EXPRESSION_SEQUENCE),
        };
        let encoded = serde_json::to_value(&point).unwrap();
        assert_eq!(
            serde_json::from_value::<Expression>(encoded.clone()).unwrap(),
            point
        );
        let mut invalid = encoded;
        invalid["sequence"] = serde_json::json!(MAX_EXPRESSION_SEQUENCE + 1);
        assert!(serde_json::from_value::<Expression>(invalid).is_err());
        let mut note = Note::new(60, 100, Beat::ONE).unwrap();
        note.expression = vec![Expression {
            sequence: Some(MAX_EXPRESSION_SEQUENCE + 1),
            ..point
        }];
        assert!(note
            .validate()
            .unwrap_err()
            .to_string()
            .contains("exact JSON integer range"));
    }
}
