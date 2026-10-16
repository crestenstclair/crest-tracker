use core::fmt;
use core::str::FromStr;
use serde::{Deserialize, Serialize};

const CHOICE_PREFIX: &str = "sf2.bank-";
const PROGRAM_SEPARATOR: &str = ".program-";

/// The numeric SF2 preset address used as stable playback identity.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundFontPresetId {
    bank: u16,
    program: u8,
}

impl SoundFontPresetId {
    pub const MIN_PROGRAM: u8 = 0;
    pub const MAX_PROGRAM: u8 = 127;

    pub const fn new(bank: u16, program: u8) -> Result<Self, SoundFontPresetIdError> {
        if program > Self::MAX_PROGRAM {
            return Err(SoundFontPresetIdError::ProgramOutOfRange(program as u16));
        }
        Ok(Self { bank, program })
    }

    pub const fn bank(self) -> u16 {
        self.bank
    }

    pub const fn program(self) -> u8 {
        self.program
    }

    pub const fn is_percussion(self) -> bool {
        self.bank == 128
    }

    pub fn choice_id(self) -> String {
        format!(
            "{CHOICE_PREFIX}{}{PROGRAM_SEPARATOR}{}",
            self.bank, self.program
        )
    }
}

impl fmt::Display for SoundFontPresetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.choice_id())
    }
}

impl FromStr for SoundFontPresetId {
    type Err = SoundFontPresetIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let remainder = value
            .strip_prefix(CHOICE_PREFIX)
            .ok_or(SoundFontPresetIdError::MalformedChoiceId)?;
        let (bank, program) = remainder
            .split_once(PROGRAM_SEPARATOR)
            .ok_or(SoundFontPresetIdError::MalformedChoiceId)?;
        if bank.is_empty()
            || program.is_empty()
            || bank.starts_with('+')
            || program.starts_with('+')
            || (bank.len() > 1 && bank.starts_with('0'))
            || (program.len() > 1 && program.starts_with('0'))
            || !bank.bytes().all(|byte| byte.is_ascii_digit())
            || !program.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(SoundFontPresetIdError::MalformedChoiceId);
        }
        let bank = bank
            .parse::<u32>()
            .map_err(|_| SoundFontPresetIdError::MalformedChoiceId)?;
        let program = program
            .parse::<u16>()
            .map_err(|_| SoundFontPresetIdError::MalformedChoiceId)?;
        let bank = u16::try_from(bank).map_err(|_| SoundFontPresetIdError::BankOutOfRange(bank))?;
        if program > u16::from(Self::MAX_PROGRAM) {
            return Err(SoundFontPresetIdError::ProgramOutOfRange(program));
        }
        Self::new(bank, program as u8)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoundFontPresetIdError {
    MalformedChoiceId,
    BankOutOfRange(u32),
    ProgramOutOfRange(u16),
}

impl fmt::Display for SoundFontPresetIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedChoiceId => formatter.write_str(
                "SoundFont preset choice id must be sf2.bank-<decimal>.program-<decimal>",
            ),
            Self::BankOutOfRange(bank) => {
                write!(
                    formatter,
                    "SoundFont preset bank {bank} is outside 0..=65535"
                )
            }
            Self::ProgramOutOfRange(program) => {
                write!(
                    formatter,
                    "SoundFont preset program {program} is outside 0..=127"
                )
            }
        }
    }
}

impl std::error::Error for SoundFontPresetIdError {}

/// One effective playable source record before catalog precedence and sorting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SoundFontPresetSource {
    source_ordinal: usize,
    bank: i32,
    program: i32,
    name: String,
    playable: bool,
}

impl SoundFontPresetSource {
    pub fn new(
        source_ordinal: usize,
        bank: i32,
        program: i32,
        name: impl Into<String>,
        playable: bool,
    ) -> Self {
        Self {
            source_ordinal,
            bank,
            program,
            name: name.into(),
            playable,
        }
    }

    pub const fn source_ordinal(&self) -> usize {
        self.source_ordinal
    }

    pub const fn playable(&self) -> bool {
        self.playable
    }
}

/// One sorted selectable preset with exact authored presentation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundFontPresetCatalogEntry {
    id: SoundFontPresetId,
    name: String,
    source_ordinal: usize,
}

impl SoundFontPresetCatalogEntry {
    pub const fn id(&self) -> SoundFontPresetId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn source_ordinal(&self) -> usize {
        self.source_ordinal
    }

    pub fn choice_id(&self) -> String {
        self.id.choice_id()
    }
}

/// Diagnostic retained for every playable source record shadowed by SF2
/// first-record coordinate precedence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundFontPresetCollision {
    id: SoundFontPresetId,
    retained_source_ordinal: usize,
    shadowed_source_ordinal: usize,
}

impl SoundFontPresetCollision {
    pub const fn id(self) -> SoundFontPresetId {
        self.id
    }

    pub const fn retained_source_ordinal(self) -> usize {
        self.retained_source_ordinal
    }

    pub const fn shadowed_source_ordinal(self) -> usize {
        self.shadowed_source_ordinal
    }
}

/// Immutable control-side SF2 preset metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundFontPresetCatalog {
    entries: Vec<SoundFontPresetCatalogEntry>,
    coordinate_collisions: Vec<SoundFontPresetCollision>,
}

impl SoundFontPresetCatalog {
    /// Builds the effective catalog in source order for precedence, then sorts
    /// selectable entries by numeric bank and program.
    pub fn from_sources(
        sources: impl IntoIterator<Item = SoundFontPresetSource>,
    ) -> Result<Self, SoundFontPresetCatalogError> {
        let mut entries: Vec<SoundFontPresetCatalogEntry> = Vec::new();
        let mut coordinate_collisions = Vec::new();
        for source in sources {
            if !source.playable {
                continue;
            }
            let bank = u16::try_from(source.bank).map_err(|_| {
                SoundFontPresetCatalogError::BankOutOfRange {
                    source_ordinal: source.source_ordinal,
                    bank: source.bank,
                }
            })?;
            let program = u8::try_from(source.program).map_err(|_| {
                SoundFontPresetCatalogError::ProgramOutOfRange {
                    source_ordinal: source.source_ordinal,
                    program: source.program,
                }
            })?;
            let id = SoundFontPresetId::new(bank, program).map_err(|_| {
                SoundFontPresetCatalogError::ProgramOutOfRange {
                    source_ordinal: source.source_ordinal,
                    program: source.program,
                }
            })?;
            if source.name.is_empty() {
                return Err(SoundFontPresetCatalogError::EmptyName {
                    source_ordinal: source.source_ordinal,
                });
            }
            if let Some(retained) = entries.iter().find(|entry| entry.id == id) {
                coordinate_collisions.push(SoundFontPresetCollision {
                    id,
                    retained_source_ordinal: retained.source_ordinal,
                    shadowed_source_ordinal: source.source_ordinal,
                });
                continue;
            }
            entries.push(SoundFontPresetCatalogEntry {
                id,
                name: source.name,
                source_ordinal: source.source_ordinal,
            });
        }
        if entries.is_empty() {
            return Err(SoundFontPresetCatalogError::EmptyCatalog);
        }
        entries.sort_by_key(|entry| entry.id);
        Ok(Self {
            entries,
            coordinate_collisions,
        })
    }

    pub fn entries(&self) -> &[SoundFontPresetCatalogEntry] {
        &self.entries
    }

    pub fn coordinate_collisions(&self) -> &[SoundFontPresetCollision] {
        &self.coordinate_collisions
    }

    pub fn default_entry(&self) -> &SoundFontPresetCatalogEntry {
        self.entries
            .first()
            .expect("a validated SoundFont catalog is nonempty")
    }

    pub fn entry(&self, id: SoundFontPresetId) -> Option<&SoundFontPresetCatalogEntry> {
        self.entries
            .binary_search_by_key(&id, SoundFontPresetCatalogEntry::id)
            .ok()
            .map(|index| &self.entries[index])
    }

    pub fn resolve_choice_id(
        &self,
        choice_id: &str,
    ) -> Result<SoundFontPresetId, SoundFontPresetCatalogError> {
        let id = SoundFontPresetId::from_str(choice_id)
            .map_err(SoundFontPresetCatalogError::InvalidChoiceId)?;
        self.entry(id)
            .map(SoundFontPresetCatalogEntry::id)
            .ok_or(SoundFontPresetCatalogError::PresetUnavailable(id))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SoundFontPresetCatalogError {
    EmptyCatalog,
    EmptyName { source_ordinal: usize },
    BankOutOfRange { source_ordinal: usize, bank: i32 },
    ProgramOutOfRange { source_ordinal: usize, program: i32 },
    InvalidChoiceId(SoundFontPresetIdError),
    PresetUnavailable(SoundFontPresetId),
}

impl fmt::Display for SoundFontPresetCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyCatalog => formatter.write_str("SoundFont contains no playable presets"),
            Self::EmptyName { source_ordinal } => write!(
                formatter,
                "playable SoundFont preset {source_ordinal} has an empty authored name"
            ),
            Self::BankOutOfRange {
                source_ordinal,
                bank,
            } => write!(
                formatter,
                "SoundFont preset {source_ordinal} has invalid bank {bank}"
            ),
            Self::ProgramOutOfRange {
                source_ordinal,
                program,
            } => write!(
                formatter,
                "SoundFont preset {source_ordinal} has invalid program {program}"
            ),
            Self::InvalidChoiceId(error) => error.fmt(formatter),
            Self::PresetUnavailable(id) => {
                write!(formatter, "SoundFont preset {id} is unavailable")
            }
        }
    }
}

impl std::error::Error for SoundFontPresetCatalogError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(ordinal: usize, bank: i32, program: i32, name: &str) -> SoundFontPresetSource {
        SoundFontPresetSource::new(ordinal, bank, program, name, true)
    }

    #[test]
    fn stable_choice_identity_round_trips_limits_and_percussion() {
        for id in [
            SoundFontPresetId::new(0, 0).unwrap(),
            SoundFontPresetId::new(128, 0).unwrap(),
            SoundFontPresetId::new(u16::MAX, 127).unwrap(),
        ] {
            assert_eq!(id.choice_id().parse::<SoundFontPresetId>().unwrap(), id);
        }
        assert!(SoundFontPresetId::new(128, 0).unwrap().is_percussion());
        assert!(!SoundFontPresetId::new(129, 0).unwrap().is_percussion());
    }

    #[test]
    fn stable_choice_identity_rejects_noncanonical_or_out_of_range_text() {
        for value in [
            "",
            "sf2.bank-0.program-",
            "sf2.bank-.program-0",
            "sf2.bank-00.program-0",
            "sf2.bank-0.program-00",
            "sf2.bank-0.program-128",
            "sf2.bank-65536.program-0",
            "sf2.bank--1.program-0",
            "SF2.bank-0.program-0",
            "sf2.bank-0.program-0.extra",
        ] {
            assert!(
                value.parse::<SoundFontPresetId>().is_err(),
                "accepted {value}"
            );
        }
    }

    #[test]
    fn catalog_uses_first_playable_coordinate_and_numeric_order_without_name_identity() {
        let catalog = SoundFontPresetCatalog::from_sources([
            source(0, 128, 0, "Drums"),
            source(1, 0, 48, "Strings"),
            source(2, 0, 0, "Piano"),
            source(3, 0, 1, "Piano"),
            source(4, 0, 0, "Shadow"),
            SoundFontPresetSource::new(5, 0, 2, "Ignored", false),
        ])
        .unwrap();

        assert_eq!(
            catalog
                .entries()
                .iter()
                .map(|entry| (entry.id(), entry.name(), entry.source_ordinal()))
                .collect::<Vec<_>>(),
            [
                (SoundFontPresetId::new(0, 0).unwrap(), "Piano", 2),
                (SoundFontPresetId::new(0, 1).unwrap(), "Piano", 3),
                (SoundFontPresetId::new(0, 48).unwrap(), "Strings", 1),
                (SoundFontPresetId::new(128, 0).unwrap(), "Drums", 0),
            ]
        );
        assert_eq!(
            catalog.coordinate_collisions(),
            &[SoundFontPresetCollision {
                id: SoundFontPresetId::new(0, 0).unwrap(),
                retained_source_ordinal: 2,
                shadowed_source_ordinal: 4,
            }]
        );
    }

    #[test]
    fn catalog_rejects_invalid_playable_metadata_and_lookup_without_fallback() {
        assert_eq!(
            SoundFontPresetCatalog::from_sources([SoundFontPresetSource::new(0, 0, 0, "", true)]),
            Err(SoundFontPresetCatalogError::EmptyName { source_ordinal: 0 })
        );
        assert!(matches!(
            SoundFontPresetCatalog::from_sources([source(0, -1, 0, "Bad")]),
            Err(SoundFontPresetCatalogError::BankOutOfRange { .. })
        ));
        assert!(matches!(
            SoundFontPresetCatalog::from_sources([source(0, 0, 128, "Bad")]),
            Err(SoundFontPresetCatalogError::ProgramOutOfRange { .. })
        ));
        assert_eq!(
            SoundFontPresetCatalog::from_sources([SoundFontPresetSource::new(
                0, 0, 0, "Ignored", false
            )]),
            Err(SoundFontPresetCatalogError::EmptyCatalog)
        );

        let catalog = SoundFontPresetCatalog::from_sources([source(0, 0, 0, "Piano")]).unwrap();
        assert!(matches!(
            catalog.resolve_choice_id("sf2.bank-0.program-1"),
            Err(SoundFontPresetCatalogError::PresetUnavailable(_))
        ));
        assert!(matches!(
            catalog.resolve_choice_id("Piano"),
            Err(SoundFontPresetCatalogError::InvalidChoiceId(_))
        ));
    }
}
