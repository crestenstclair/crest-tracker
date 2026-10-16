//! Immutable named chord revisions. Musical transforms remain owned by `harmony`.
use crate::harmony::{Chord, HarmonyError, Pitch, Scale};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroU64;
use thiserror::Error;

pub const CHORD_REVISION_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ChordBankError {
    #[error("chord revision identity must be nonzero")]
    InvalidIdentity,
    #[error("chord revision name must be nonempty and contain no NUL")]
    InvalidName,
    #[error("unsupported chord revision version {0}")]
    UnsupportedVersion(u32),
    #[error("chord verification pitches do not match the definition at its declared root")]
    VerificationMismatch,
    #[error("chord revision {0} is immutable; publish a new identity")]
    ImmutableRevision(ChordRevisionId),
    #[error("duplicate chord revision identity {0}")]
    DuplicateIdentity(ChordRevisionId),
    #[error(transparent)]
    Harmony(#[from] HarmonyError),
}

/// A Song-allocated revision identity, never a content digest or mutable bank slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct ChordRevisionId(NonZeroU64);
impl ChordRevisionId {
    pub fn new(value: u64) -> Result<Self, ChordBankError> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(ChordBankError::InvalidIdentity)
    }
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}
impl TryFrom<u64> for ChordRevisionId {
    type Error = ChordBankError;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<ChordRevisionId> for u64 {
    fn from(value: ChordRevisionId) -> Self {
        value.get()
    }
}
impl fmt::Display for ChordRevisionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(formatter)
    }
}

/// Shape and transform settings, never already voiced offsets. The settings must
/// resolve at some legal MIDI root; a revision validates its actual saved root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ChordDefinitionData")]
pub struct ChordDefinition {
    shape: Chord,
    inversion: u8,
    open: bool,
    spread: u8,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChordDefinitionData {
    shape: Chord,
    inversion: u8,
    open: bool,
    spread: u8,
}
impl ChordDefinition {
    pub fn new(
        shape: Chord,
        inversion: u8,
        open: bool,
        spread: u8,
    ) -> Result<Self, ChordBankError> {
        // The lowest legal root offers the full remaining MIDI range. Let the
        // existing resolver validate inversion, open voicing, and spread bounds.
        let lowest_root = shape.intervals()[0].min(0).unsigned_abs();
        shape.resolve(
            Pitch::new(lowest_root)?,
            usize::from(inversion),
            open,
            usize::from(spread),
        )?;
        Ok(Self {
            shape,
            inversion,
            open,
            spread,
        })
    }
    pub fn shape(&self) -> &Chord {
        &self.shape
    }
    pub const fn inversion(&self) -> u8 {
        self.inversion
    }
    pub const fn open(&self) -> bool {
        self.open
    }
    pub const fn spread(&self) -> u8 {
        self.spread
    }
    pub fn resolve(&self, root: Pitch) -> Result<Vec<Pitch>, HarmonyError> {
        self.shape.resolve(
            root,
            usize::from(self.inversion),
            self.open,
            usize::from(self.spread),
        )
    }
}
impl TryFrom<ChordDefinitionData> for ChordDefinition {
    type Error = ChordBankError;
    fn try_from(value: ChordDefinitionData) -> Result<Self, Self::Error> {
        Self::new(value.shape, value.inversion, value.open, value.spread)
    }
}

/// Evidence for one definition at one root. Recall never treats these pitches as
/// a fresh shape, which would apply the saved voicing and spread twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChordVerification {
    root: Pitch,
    pitches: Vec<Pitch>,
}
impl ChordVerification {
    pub const fn root(&self) -> Pitch {
        self.root
    }
    pub fn pitches(&self) -> &[Pitch] {
        &self.pitches
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChordVerificationData {
    root: Pitch,
    pitches: Vec<Pitch>,
}

/// An immutable named definition plus an exact verification snapshot. A new name
/// or any changed setting requires a new Song-allocated revision identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ChordRevisionData")]
pub struct ChordRevision {
    version: u32,
    name: String,
    definition: ChordDefinition,
    verification: ChordVerification,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChordRevisionData {
    version: u32,
    name: String,
    definition: ChordDefinition,
    verification: ChordVerificationData,
}
impl ChordRevision {
    pub fn new(
        name: String,
        definition: ChordDefinition,
        root: Pitch,
    ) -> Result<Self, ChordBankError> {
        let pitches = definition.resolve(root)?;
        let revision = Self {
            version: CHORD_REVISION_VERSION,
            name,
            definition,
            verification: ChordVerification { root, pitches },
        };
        revision.validate()?;
        Ok(revision)
    }
    pub const fn version(&self) -> u32 {
        self.version
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn definition(&self) -> &ChordDefinition {
        &self.definition
    }
    pub fn verification(&self) -> &ChordVerification {
        &self.verification
    }
    /// Resolve the original shape with its saved settings exactly once. A changed
    /// root can fail MIDI bounds; recall never clips, wraps, or substitutes notes.
    pub fn recall(&self, root: Pitch) -> Result<Vec<Pitch>, HarmonyError> {
        self.definition.resolve(root)
    }
    pub fn validate(&self) -> Result<(), ChordBankError> {
        if self.version != CHORD_REVISION_VERSION {
            return Err(ChordBankError::UnsupportedVersion(self.version));
        }
        if self.name.trim().is_empty() || self.name.contains('\0') {
            return Err(ChordBankError::InvalidName);
        }
        if self.definition.resolve(self.verification.root)? != self.verification.pitches {
            return Err(ChordBankError::VerificationMismatch);
        }
        Ok(())
    }
}
impl TryFrom<ChordRevisionData> for ChordRevision {
    type Error = ChordBankError;
    fn try_from(value: ChordRevisionData) -> Result<Self, Self::Error> {
        let revision = Self {
            version: value.version,
            name: value.name,
            definition: value.definition,
            verification: ChordVerification {
                root: value.verification.root,
                pitches: value.verification.pitches,
            },
        };
        revision.validate()?;
        Ok(revision)
    }
}

/// Revisions are retained without a bank-size cap. Equal insertions are no-ops;
/// replacing an existing identity with different content is always rejected.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ChordBank {
    revisions: BTreeMap<ChordRevisionId, ChordRevision>,
}
impl ChordBank {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.revisions.len()
    }
    pub fn is_empty(&self) -> bool {
        self.revisions.is_empty()
    }
    pub fn get(&self, id: &ChordRevisionId) -> Option<&ChordRevision> {
        self.revisions.get(id)
    }
    pub fn entries(&self) -> impl Iterator<Item = (&ChordRevisionId, &ChordRevision)> {
        self.revisions.iter()
    }
    pub fn keys(&self) -> impl DoubleEndedIterator<Item = &ChordRevisionId> {
        self.revisions.keys()
    }
    pub fn with_revision(
        &self,
        id: ChordRevisionId,
        revision: ChordRevision,
    ) -> Result<Self, ChordBankError> {
        if self.get(&id).is_some_and(|existing| existing != &revision) {
            return Err(ChordBankError::ImmutableRevision(id));
        }
        let mut bank = self.clone();
        bank.revisions.insert(id, revision);
        Ok(bank)
    }
}
impl<'de> Deserialize<'de> for ChordBank {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct BankVisitor;
        impl<'de> Visitor<'de> for BankVisitor {
            type Value = ChordBank;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .write_str("a map of distinct chord revision identities to validated revisions")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut revisions = BTreeMap::new();
                while let Some((id, revision)) =
                    map.next_entry::<ChordRevisionId, ChordRevision>()?
                {
                    if revisions.insert(id, revision).is_some() {
                        return Err(de::Error::custom(ChordBankError::DuplicateIdentity(id)));
                    }
                }
                Ok(ChordBank { revisions })
            }
        }
        deserializer.deserialize_map(BankVisitor)
    }
}

/// Derive read-only candidates without allocating Song identities. Copying one
/// into Custom is a separate publication. MIDI 60 + pitch class is the declared
/// verification root; names retain the scale's existing spelling policy.
pub fn default_bank(scale: &Scale) -> Result<Vec<ChordRevision>, ChordBankError> {
    scale
        .default_chords()
        .into_iter()
        .map(|entry| {
            let root = Pitch::new(60 + entry.root)?;
            let name = format!("{} {}", root.name(scale.spelling()), entry.chord.name());
            ChordRevision::new(name, ChordDefinition::new(entry.chord, 0, false, 0)?, root)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harmony::{ChordKind, ScaleKind};
    use serde_json::json;

    fn saved_minor(inversion: u8, open: bool, spread: u8) -> ChordRevision {
        ChordRevision::new(
            "My minor".into(),
            ChordDefinition::new(Chord::from_kind(ChordKind::Minor), inversion, open, spread)
                .unwrap(),
            Pitch::new(60).unwrap(),
        )
        .unwrap()
    }
    fn midi(notes: &[Pitch]) -> Vec<u8> {
        notes.iter().map(|pitch| pitch.midi()).collect()
    }

    #[test]
    fn saved_voicing_fixtures_recall_original_shape_exactly_once() {
        for (inversion, open, spread, expected) in [
            (0, false, 0, vec![60, 63, 67]),
            (1, false, 0, vec![63, 67, 72]),
            (2, false, 0, vec![67, 72, 75]),
            (0, true, 0, vec![60, 67, 75]),
            (1, true, 0, vec![63, 72, 79]),
            (2, true, 0, vec![67, 75, 84]),
            (0, false, 3, vec![60, 63, 67, 72, 75, 79]),
            (1, true, 2, vec![63, 72, 79, 84, 87]),
        ] {
            let original = saved_minor(inversion, open, spread);
            let encoded = serde_json::to_string(&original).unwrap();
            let recalled: ChordRevision = serde_json::from_str(&encoded).unwrap();
            assert_eq!(midi(recalled.verification().pitches()), expected);
            assert_eq!(
                midi(&recalled.recall(Pitch::new(60).unwrap()).unwrap()),
                expected
            );
            assert_eq!(
                midi(&recalled.recall(Pitch::new(62).unwrap()).unwrap()),
                expected.iter().map(|pitch| pitch + 2).collect::<Vec<_>>()
            );
            assert_eq!(recalled.definition().shape().intervals(), &[0, 3, 7]);
            assert_eq!(serde_json::to_string(&recalled).unwrap(), encoded);
            assert_eq!(original, recalled);
        }
    }

    #[test]
    fn version_one_serialization_keeps_shape_separate_from_verified_output() {
        let revision = saved_minor(1, true, 2);
        let fixture = r#"{"version":1,"name":"My minor","definition":{"shape":[0,3,7],"inversion":1,"open":true,"spread":2},"verification":{"root":60,"pitches":[63,72,79,84,87]}}"#;
        assert_eq!(serde_json::to_string(&revision).unwrap(), fixture);
        assert_eq!(
            serde_json::from_str::<ChordRevision>(fixture).unwrap(),
            revision
        );
        let unsorted = fixture.replace("[0,3,7]", "[7,0,3]");
        let canonical: ChordRevision = serde_json::from_str(&unsorted).unwrap();
        assert_eq!(serde_json::to_string(&canonical).unwrap(), fixture);
    }

    #[test]
    fn invalid_version_snapshot_shape_and_settings_reject_on_deserialization() {
        let valid = serde_json::to_value(saved_minor(1, true, 2)).unwrap();
        for (path, bad) in [
            (vec!["version"], json!(2)),
            (vec!["name"], json!(" ")),
            (vec!["name"], json!("bad\0name")),
            (vec!["definition", "shape"], json!([3, 12, 19, 24, 27])),
            (vec!["definition", "inversion"], json!(3)),
            (vec!["definition", "spread"], json!(255)),
            (vec!["verification", "root"], json!(61)),
            (vec!["verification", "pitches"], json!([63, 72, 79, 84, 88])),
            (vec!["verification", "pitches"], json!([72, 63, 79, 84, 87])),
            (vec!["verification", "pitches"], json!([63, 72, 79, 84, 84])),
        ] {
            let mut changed = valid.clone();
            let mut field = &mut changed;
            for key in path {
                field = &mut field[key];
            }
            *field = bad;
            assert!(serde_json::from_value::<ChordRevision>(changed).is_err());
        }
        let mut unknown = valid;
        unknown["verification"]["unexpected"] = json!(1);
        assert!(serde_json::from_value::<ChordRevision>(unknown).is_err());
        assert!(serde_json::from_str::<ChordRevisionId>("0").is_err());
    }

    #[test]
    fn recall_bounds_never_clip_or_mutate_a_saved_revision() {
        let revision = saved_minor(1, true, 2);
        let before = revision.clone();
        assert!(matches!(
            revision.recall(Pitch::new(125).unwrap()),
            Err(HarmonyError::PitchOutOfRange(_))
        ));
        assert_eq!(revision, before);
        let negative =
            ChordDefinition::new(Chord::new(vec![-12, 0, 7]).unwrap(), 0, false, 0).unwrap();
        assert!(ChordRevision::new("Low chord".into(), negative, Pitch::new(0).unwrap()).is_err());
        assert!(ChordDefinition::new(Chord::new(vec![0, 7]).unwrap(), 0, true, 0).is_err());
        assert!(ChordDefinition::new(Chord::new(vec![-12, 0, 12]).unwrap(), 0, true, 0).is_err());
    }

    #[test]
    fn bank_preserves_pinned_revisions_and_has_no_arbitrary_size_cap() {
        let id = ChordRevisionId::new(7).unwrap();
        let original = saved_minor(1, true, 2);
        let bank = ChordBank::new()
            .with_revision(id, original.clone())
            .unwrap();
        assert_eq!(bank.with_revision(id, original.clone()).unwrap(), bank);
        assert_eq!(
            bank.with_revision(id, saved_minor(0, false, 0)),
            Err(ChordBankError::ImmutableRevision(id))
        );
        let mut expanded = bank.clone();
        for number in 8..=308 {
            expanded = expanded
                .with_revision(
                    ChordRevisionId::new(number).unwrap(),
                    saved_minor(0, false, 0),
                )
                .unwrap();
        }
        assert_eq!(bank.len(), 1);
        assert_eq!(expanded.len(), 302);
        assert_eq!(expanded.get(&id), Some(&original));
        assert_eq!(expanded.keys().next_back().unwrap().get(), 308);
        assert_eq!(expanded.entries().count(), 302);
        let encoded = serde_json::to_string(&expanded).unwrap();
        assert_eq!(
            serde_json::from_str::<ChordBank>(&encoded).unwrap(),
            expanded
        );
        let revision = serde_json::to_string(&original).unwrap();
        assert!(serde_json::from_str::<ChordBank>(&format!(
            "{{\"7\":{revision},\"7\":{revision}}}"
        ))
        .is_err());
        assert_eq!(serde_json::to_string(&ChordBank::default()).unwrap(), "{}");
    }

    #[test]
    fn default_candidates_reuse_scale_catalog_and_keep_honest_spelled_names() {
        for scale in [
            Scale::new(0, ScaleKind::NaturalMinor).unwrap(),
            Scale::new(5, ScaleKind::Dorian).unwrap(),
            Scale::custom(0, vec![0, 1, 6]).unwrap(),
        ] {
            let candidates = default_bank(&scale).unwrap();
            let defaults = scale.default_chords();
            assert_eq!(candidates.len(), defaults.len());
            for (candidate, default) in candidates.iter().zip(defaults) {
                assert_eq!(candidate.definition().shape(), &default.chord);
                let root = Pitch::new(60 + default.root).unwrap();
                assert_eq!(candidate.verification().root(), root);
                assert_eq!(
                    candidate.name(),
                    format!("{} {}", root.name(scale.spelling()), default.chord.name())
                );
                assert!(candidate
                    .verification()
                    .pitches()
                    .iter()
                    .all(|pitch| scale.contains(*pitch)));
            }
        }
        let minor = default_bank(&Scale::default()).unwrap();
        assert!(minor
            .iter()
            .any(|revision| revision.name().starts_with("Eb4 ")));
    }
}
