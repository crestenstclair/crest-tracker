//! Literal pitch, scale and chord rules. Display and scale selection never retune music.
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HarmonyError {
    #[error("MIDI pitch {0} is outside 0..=127")]
    PitchOutOfRange(i32),
    #[error("a scale root and its pitch classes must be in 0..=11")]
    InvalidPitchClass,
    #[error("a scale must contain its root and distinct pitch classes")]
    InvalidScale,
    #[error("custom scales need explicit pitch classes")]
    MissingCustomScale,
    #[error("saved pitch classes do not match the named scale")]
    ScaleCatalogMismatch,
    #[error("a chord needs distinct intervals in -12..=12, including zero")]
    InvalidChord,
    #[error("inversion {inversion} requires more than {voices} voices")]
    InvalidInversion { inversion: usize, voices: usize },
    #[error("open voicing requires at least three voices")]
    OpenNeedsThreeVoices,
    #[error("voicing would place two voices on MIDI pitch {0}")]
    DuplicateVoice(u8),
    #[error("spread requests {requested} additional voices, but only {available} fit MIDI range")]
    SpreadExhausted { requested: usize, available: usize },
}

/// A validated MIDI note number. Middle C is MIDI 60, displayed as C4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct Pitch(u8);

impl Pitch {
    pub fn new(value: u8) -> Result<Self, HarmonyError> {
        if value > 127 {
            return Err(HarmonyError::PitchOutOfRange(i32::from(value)));
        }
        Ok(Self(value))
    }
    pub const fn midi(self) -> u8 {
        self.0
    }
    pub const fn pitch_class(self) -> u8 {
        self.0 % 12
    }
    pub fn transpose(self, semitones: i16) -> Result<Self, HarmonyError> {
        let value = i32::from(self.0) + i32::from(semitones);
        if !(0..=127).contains(&value) {
            return Err(HarmonyError::PitchOutOfRange(value));
        }
        Ok(Self(value as u8))
    }
    pub fn name(self, spelling: Spelling) -> String {
        format!(
            "{}{}",
            spelling.pitch_class_name(self.pitch_class()),
            i16::from(self.0) / 12 - 1
        )
    }
}
impl TryFrom<u8> for Pitch {
    type Error = HarmonyError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<Pitch> for u8 {
    fn from(value: Pitch) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Spelling {
    Sharps,
    Flats,
}
impl Spelling {
    fn pitch_class_name(self, pitch_class: u8) -> &'static str {
        const SHARPS: [&str; 12] = [
            "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
        ];
        const FLATS: [&str; 12] = [
            "C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B",
        ];
        match self {
            Self::Sharps => SHARPS[pitch_class as usize],
            Self::Flats => FLATS[pitch_class as usize],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
}

/// Serialized names are stable catalog identifiers; melodic minor is ascending jazz minor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScaleKind {
    Major,
    NaturalMinor,
    HarmonicMinor,
    MelodicMinor,
    Ionian,
    Dorian,
    Phrygian,
    Lydian,
    Mixolydian,
    Aeolian,
    Locrian,
    MajorPentatonic,
    MinorPentatonic,
    Blues,
    WholeTone,
    Chromatic,
    Custom,
}
impl ScaleKind {
    pub const CATALOG: [Self; 16] = [
        Self::Major,
        Self::NaturalMinor,
        Self::HarmonicMinor,
        Self::MelodicMinor,
        Self::Ionian,
        Self::Dorian,
        Self::Phrygian,
        Self::Lydian,
        Self::Mixolydian,
        Self::Aeolian,
        Self::Locrian,
        Self::MajorPentatonic,
        Self::MinorPentatonic,
        Self::Blues,
        Self::WholeTone,
        Self::Chromatic,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Major => "Major",
            Self::NaturalMinor => "Natural minor",
            Self::HarmonicMinor => "Harmonic minor",
            Self::MelodicMinor => "Melodic minor",
            Self::Ionian => "Ionian",
            Self::Dorian => "Dorian",
            Self::Phrygian => "Phrygian",
            Self::Lydian => "Lydian",
            Self::Mixolydian => "Mixolydian",
            Self::Aeolian => "Aeolian",
            Self::Locrian => "Locrian",
            Self::MajorPentatonic => "Major pentatonic",
            Self::MinorPentatonic => "Minor pentatonic",
            Self::Blues => "Blues",
            Self::WholeTone => "Whole tone",
            Self::Chromatic => "Chromatic",
            Self::Custom => "Custom",
        }
    }

    fn intervals(self) -> &'static [u8] {
        match self {
            Self::Major | Self::Ionian => &[0, 2, 4, 5, 7, 9, 11],
            Self::NaturalMinor | Self::Aeolian => &[0, 2, 3, 5, 7, 8, 10],
            Self::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            Self::MelodicMinor => &[0, 2, 3, 5, 7, 9, 11],
            Self::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Self::Phrygian => &[0, 1, 3, 5, 7, 8, 10],
            Self::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            Self::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            Self::Locrian => &[0, 1, 3, 5, 6, 8, 10],
            Self::MajorPentatonic => &[0, 2, 4, 7, 9],
            Self::MinorPentatonic => &[0, 3, 5, 7, 10],
            Self::Blues => &[0, 3, 5, 6, 7, 10],
            Self::WholeTone => &[0, 2, 4, 6, 8, 10],
            Self::Chromatic => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            Self::Custom => &[],
        }
    }
}

/// Pitch classes are absolute (C=0), sorted and unique. Root must be present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ScaleData", into = "ScaleData")]
pub struct Scale {
    root: u8,
    kind: ScaleKind,
    pitch_classes: Vec<u8>,
    spelling: Spelling,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScaleData {
    root: u8,
    kind: ScaleKind,
    pitch_classes: Vec<u8>,
    spelling: Spelling,
}

impl Scale {
    pub fn new(root: u8, kind: ScaleKind) -> Result<Self, HarmonyError> {
        if root > 11 {
            return Err(HarmonyError::InvalidPitchClass);
        }
        if kind == ScaleKind::Custom {
            return Err(HarmonyError::MissingCustomScale);
        }
        let pitch_classes = kind
            .intervals()
            .iter()
            .map(|interval| (root + interval) % 12)
            .collect();
        let mut scale = Self::custom(root, pitch_classes)?;
        scale.kind = kind;
        Ok(scale)
    }

    pub fn custom(root: u8, mut pitch_classes: Vec<u8>) -> Result<Self, HarmonyError> {
        if root > 11 || pitch_classes.iter().any(|pitch| *pitch > 11) {
            return Err(HarmonyError::InvalidPitchClass);
        }
        pitch_classes.sort_unstable();
        if !pitch_classes.contains(&root) || pitch_classes.windows(2).any(|pair| pair[0] == pair[1])
        {
            return Err(HarmonyError::InvalidScale);
        }
        let minor = pitch_classes.contains(&((root + 3) % 12))
            && !pitch_classes.contains(&((root + 4) % 12));
        let major_root = if minor { (root + 3) % 12 } else { root };
        let spelling = if [1, 3, 5, 8, 10].contains(&major_root) {
            Spelling::Flats
        } else {
            Spelling::Sharps
        };
        Ok(Self {
            root,
            kind: ScaleKind::Custom,
            pitch_classes,
            spelling,
        })
    }

    pub const fn root(&self) -> u8 {
        self.root
    }
    pub const fn kind(&self) -> ScaleKind {
        self.kind
    }
    pub fn pitch_classes(&self) -> &[u8] {
        &self.pitch_classes
    }
    pub const fn spelling(&self) -> Spelling {
        self.spelling
    }
    pub fn with_spelling(mut self, spelling: Spelling) -> Self {
        self.spelling = spelling;
        self
    }
    pub fn contains(&self, pitch: Pitch) -> bool {
        self.pitch_classes.contains(&pitch.pitch_class())
    }
    pub fn name(&self) -> String {
        format!(
            "{} {}",
            self.spelling.pitch_class_name(self.root),
            self.kind.name()
        )
    }
    pub fn pitch_name(&self, pitch: Pitch) -> String {
        pitch.name(self.spelling)
    }
    pub fn interval_name(&self, pitch: Pitch) -> &'static str {
        const NAMES: [&str; 12] = [
            "1", "b2", "2", "b3", "3", "4", "#4", "5", "b6", "6", "b7", "7",
        ];
        NAMES[((pitch.pitch_class() + 12 - self.root) % 12) as usize]
    }

    /// Strictly next in-scale pitch. No wrapping or clipping at the MIDI boundaries.
    pub fn next_pitch(&self, pitch: Pitch, direction: Direction) -> Option<Pitch> {
        let mut value = i16::from(pitch.midi());
        let delta = match direction {
            Direction::Up => 1,
            Direction::Down => -1,
        };
        loop {
            value += delta;
            if !(0..=127).contains(&value) {
                return None;
            }
            let next = Pitch(value as u8);
            if self.contains(next) {
                return Some(next);
            }
        }
    }

    pub fn default_chords(&self) -> Vec<DefaultChord> {
        self.pitch_classes
            .iter()
            .flat_map(|root| self.chords_for_root(*root))
            .collect()
    }

    fn chords_for_root(&self, root: u8) -> Vec<DefaultChord> {
        let mut chords: Vec<_> = ChordKind::CATALOG
            .iter()
            .copied()
            .map(|kind| DefaultChord {
                root,
                chord: Chord::from_kind(kind),
            })
            .filter(|entry| entry.chord.fits_scale(root, self))
            .collect();
        if chords.is_empty() {
            let mut steps: Vec<_> = self
                .pitch_classes
                .iter()
                .map(|pitch| (pitch + 12 - root) % 12)
                .collect();
            steps.sort_unstable();
            let intervals = if steps.len() < 3 {
                steps.iter().map(|step| *step as i8).collect()
            } else {
                vec![
                    0,
                    steps[2] as i8,
                    steps[if steps.len() > 4 { 4 } else { 1 }] as i8,
                ]
            };
            chords.push(DefaultChord {
                root,
                chord: Chord::new(intervals)
                    .expect("derived scale intervals are unique and rooted"),
            });
        }
        chords
    }
}
impl Default for Scale {
    fn default() -> Self {
        Self::new(0, ScaleKind::NaturalMinor).expect("the built-in C minor scale is valid")
    }
}
impl TryFrom<ScaleData> for Scale {
    type Error = HarmonyError;
    fn try_from(value: ScaleData) -> Result<Self, Self::Error> {
        let mut scale = Self::custom(value.root, value.pitch_classes)?;
        if value.kind != ScaleKind::Custom
            && Self::new(value.root, value.kind)?.pitch_classes != scale.pitch_classes
        {
            return Err(HarmonyError::ScaleCatalogMismatch);
        }
        scale.kind = value.kind;
        scale.spelling = value.spelling;
        Ok(scale)
    }
}
impl From<Scale> for ScaleData {
    fn from(value: Scale) -> Self {
        Self {
            root: value.root,
            kind: value.kind,
            pitch_classes: value.pitch_classes,
            spelling: value.spelling,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChordKind {
    Major,
    Minor,
    Diminished,
    Augmented,
    Sus2,
    Sus4,
    MajorSixth,
    MinorSixth,
    DominantSeventh,
    MajorSeventh,
    MinorSeventh,
    HalfDiminished,
    DiminishedSeventh,
}
impl ChordKind {
    pub const CATALOG: [Self; 13] = [
        Self::Major,
        Self::Minor,
        Self::Diminished,
        Self::Augmented,
        Self::Sus2,
        Self::Sus4,
        Self::MajorSixth,
        Self::MinorSixth,
        Self::DominantSeventh,
        Self::MajorSeventh,
        Self::MinorSeventh,
        Self::HalfDiminished,
        Self::DiminishedSeventh,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Major => "Major",
            Self::Minor => "Minor",
            Self::Diminished => "Diminished",
            Self::Augmented => "Augmented",
            Self::Sus2 => "Sus2",
            Self::Sus4 => "Sus4",
            Self::MajorSixth => "Major sixth",
            Self::MinorSixth => "Minor sixth",
            Self::DominantSeventh => "Dominant seventh",
            Self::MajorSeventh => "Major seventh",
            Self::MinorSeventh => "Minor seventh",
            Self::HalfDiminished => "Half-diminished",
            Self::DiminishedSeventh => "Diminished seventh",
        }
    }
    fn intervals(self) -> &'static [i8] {
        match self {
            Self::Major => &[0, 4, 7],
            Self::Minor => &[0, 3, 7],
            Self::Diminished => &[0, 3, 6],
            Self::Augmented => &[0, 4, 8],
            Self::Sus2 => &[0, 2, 7],
            Self::Sus4 => &[0, 5, 7],
            Self::MajorSixth => &[0, 4, 7, 9],
            Self::MinorSixth => &[0, 3, 7, 9],
            Self::DominantSeventh => &[0, 4, 7, 10],
            Self::MajorSeventh => &[0, 4, 7, 11],
            Self::MinorSeventh => &[0, 3, 7, 10],
            Self::HalfDiminished => &[0, 3, 6, 10],
            Self::DiminishedSeventh => &[0, 3, 6, 9],
        }
    }
}

/// Immutable literal semitone offsets, not scale degrees or already voiced pitches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<i8>", into = "Vec<i8>")]
pub struct Chord {
    intervals: Vec<i8>,
}
impl Chord {
    pub fn new(mut intervals: Vec<i8>) -> Result<Self, HarmonyError> {
        intervals.sort_unstable();
        if !intervals.contains(&0)
            || intervals.iter().any(|step| !(-12..=12).contains(step))
            || intervals.windows(2).any(|pair| pair[0] == pair[1])
        {
            return Err(HarmonyError::InvalidChord);
        }
        Ok(Self { intervals })
    }
    pub fn from_kind(kind: ChordKind) -> Self {
        Self {
            intervals: kind.intervals().to_vec(),
        }
    }
    pub fn intervals(&self) -> &[i8] {
        &self.intervals
    }
    pub fn name(&self) -> String {
        if let Some(kind) = ChordKind::CATALOG
            .iter()
            .find(|kind| kind.intervals() == self.intervals)
        {
            return kind.name().into();
        }
        format!(
            "Intervals {}",
            self.intervals
                .iter()
                .map(|step| format!("{step:+}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
    fn fits_scale(&self, root: u8, scale: &Scale) -> bool {
        self.intervals.iter().all(|interval| {
            scale
                .pitch_classes
                .contains(&((i16::from(root) + i16::from(*interval)).rem_euclid(12) as u8))
        })
    }

    /// Resolve shape, inversion, open voicing, then added octave voices exactly once.
    /// A bass inversion raises the current lowest voice above all remaining voices.
    pub fn resolve(
        &self,
        root: Pitch,
        inversion: usize,
        open: bool,
        spread: usize,
    ) -> Result<Vec<Pitch>, HarmonyError> {
        if inversion >= self.intervals.len() {
            return Err(HarmonyError::InvalidInversion {
                inversion,
                voices: self.intervals.len(),
            });
        }
        if open && self.intervals.len() < 3 {
            return Err(HarmonyError::OpenNeedsThreeVoices);
        }
        let mut voices = self
            .intervals
            .iter()
            .map(|step| root.transpose(i16::from(*step)))
            .collect::<Result<Vec<_>, _>>()?;
        for _ in 0..inversion {
            let lowest = voices.remove(0);
            let highest = voices
                .last()
                .copied()
                .expect("an inversion leaves another voice");
            let octaves = (i16::from(highest.midi()) - i16::from(lowest.midi())) / 12 + 1;
            voices.push(lowest.transpose(octaves * 12)?);
        }
        if open {
            voices[1] = voices[1].transpose(12)?;
            voices.sort_unstable();
        }
        if let Some(pair) = voices.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(HarmonyError::DuplicateVoice(pair[0].midi()));
        }
        let highest = voices.last().expect("a chord always has its root").midi();
        let candidates: Vec<_> = ((u16::from(highest) + 1)..=127)
            .map(|value| Pitch(value as u8))
            .filter(|pitch| self.has_pitch_class(root, pitch.pitch_class()))
            .collect();
        if spread > candidates.len() {
            return Err(HarmonyError::SpreadExhausted {
                requested: spread,
                available: candidates.len(),
            });
        }
        voices.extend(candidates.into_iter().take(spread));
        Ok(voices)
    }

    fn has_pitch_class(&self, root: Pitch, pitch_class: u8) -> bool {
        self.intervals.iter().any(|interval| {
            (i16::from(root.midi()) + i16::from(*interval)).rem_euclid(12) as u8 == pitch_class
        })
    }
}
impl TryFrom<Vec<i8>> for Chord {
    type Error = HarmonyError;
    fn try_from(value: Vec<i8>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<Chord> for Vec<i8> {
    fn from(value: Chord) -> Self {
        value.intervals
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultChord {
    pub root: u8,
    pub chord: Chord,
}
