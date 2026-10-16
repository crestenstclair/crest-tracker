use crate::adapter::hidef_soundfont_capability::HIDEF_SOUNDFONT_PATH;
use crate::adapter::soundfont_voice_engine::{PreparedSoundFontBank, PreparedSoundFontBankError};
use crate::synth::{SoundFontPresetCatalog, SoundFontPresetCatalogError, SoundFontPresetSource};
use core::fmt;
use rustysynth::SoundFont;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

/// The two immutable projections produced by the one production SF2 parse.
#[derive(Clone)]
pub struct HiDefSoundFontAsset {
    catalog: Arc<SoundFontPresetCatalog>,
    prepared_bank: Arc<PreparedSoundFontBank>,
    parse_count: usize,
}

impl HiDefSoundFontAsset {
    /// Opens the fixed production asset and parses it exactly once.
    pub fn load() -> Result<Self, HiDefSoundFontAssetError> {
        let path = super::bundled_resource::path(HIDEF_SOUNDFONT_PATH)
            .map_err(|_| HiDefSoundFontAssetError::FileOpen)?;
        Self::load_from_path(&path)
    }

    pub fn load_from_path(path: &Path) -> Result<Self, HiDefSoundFontAssetError> {
        let bytes = crate::adapter::filesystem_sample_catalog::read_sample_path(path)
            .map_err(HiDefSoundFontAssetError::File)?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, HiDefSoundFontAssetError> {
        if bytes.len() as u64 > crate::synth::MAX_SAMPLE_SOURCE_BYTES {
            return Err(HiDefSoundFontAssetError::File(
                crate::synth::SampleAssetError::SourceTooLarge,
            ));
        }
        validate_riff(bytes)?;
        let sound_font = std::panic::catch_unwind(|| SoundFont::new(&mut Cursor::new(bytes)))
            .map_err(|_| HiDefSoundFontAssetError::Parse)?
            .map_err(|_| HiDefSoundFontAssetError::Parse)?;
        let sound_font = Arc::new(sound_font);
        let (prepared_bank, playable_source_ordinals) =
            PreparedSoundFontBank::from_sound_font(sound_font.clone())
                .map_err(HiDefSoundFontAssetError::from_prepared_bank)?;

        let mut playable = vec![false; sound_font.get_presets().len()];
        for source_ordinal in playable_source_ordinals {
            let slot = playable
                .get_mut(source_ordinal)
                .ok_or(HiDefSoundFontAssetError::Metadata)?;
            *slot = true;
        }
        let sources =
            sound_font
                .get_presets()
                .iter()
                .enumerate()
                .map(|(source_ordinal, preset)| {
                    SoundFontPresetSource::new(
                        source_ordinal,
                        preset.get_bank_number(),
                        preset.get_patch_number(),
                        preset.get_name(),
                        playable[source_ordinal],
                    )
                });
        let catalog = SoundFontPresetCatalog::from_sources(sources)
            .map_err(HiDefSoundFontAssetError::Catalog)?;

        // Retain the immutable upstream SoundFont for the complete renderer.
        // No parsing, metadata edits, or destruction occurs on the callback.
        Ok(Self {
            catalog: Arc::new(catalog),
            prepared_bank: Arc::new(prepared_bank),
            parse_count: 1,
        })
    }

    pub fn catalog(&self) -> Arc<SoundFontPresetCatalog> {
        Arc::clone(&self.catalog)
    }

    pub(crate) fn prepared_bank(&self) -> Arc<PreparedSoundFontBank> {
        Arc::clone(&self.prepared_bank)
    }

    pub(crate) fn from_projections(
        catalog: Arc<SoundFontPresetCatalog>,
        prepared_bank: Arc<PreparedSoundFontBank>,
    ) -> Self {
        Self {
            catalog,
            prepared_bank,
            parse_count: 0,
        }
    }

    pub const fn parse_count(&self) -> usize {
        self.parse_count
    }
}

// Validate chunk lengths before the parser uses untrusted sizes for allocation or seeking.
fn validate_riff(bytes: &[u8]) -> Result<(), HiDefSoundFontAssetError> {
    let invalid = HiDefSoundFontAssetError::Parse;
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"sfbk" {
        return Err(invalid);
    }
    let size = u32::from_le_bytes(bytes[4..8].try_into().map_err(|_| invalid.clone())?) as usize;
    if size.checked_add(8) != Some(bytes.len()) {
        return Err(invalid);
    }
    let mut pending = vec![(12usize, bytes.len(), 0usize)];
    while let Some((mut offset, end, depth)) = pending.pop() {
        while offset < end {
            let header = bytes
                .get(offset..offset.checked_add(8).ok_or_else(|| invalid.clone())?)
                .filter(|_| offset + 8 <= end)
                .ok_or_else(|| invalid.clone())?;
            let length =
                u32::from_le_bytes(header[4..8].try_into().map_err(|_| invalid.clone())?) as usize;
            let finish = offset
                .checked_add(8)
                .and_then(|n| n.checked_add(length))
                .filter(|n| *n <= end)
                .ok_or_else(|| invalid.clone())?;
            if &header[..4] == b"LIST" {
                if depth >= 2 || length < 4 {
                    return Err(invalid);
                }
                pending.push((offset + 12, finish, depth + 1));
            }
            offset = finish
                .checked_add(length % 2)
                .filter(|n| *n <= end)
                .ok_or_else(|| invalid.clone())?;
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HiDefSoundFontAssetError {
    File(crate::synth::SampleAssetError),
    UnexpectedPath,
    FileOpen,
    Parse,
    Metadata,
    SampleStorage,
    InvalidPresetAddress { source_ordinal: usize },
    InvalidInstrumentReference { source_ordinal: usize },
    InvalidSampleReference { source_ordinal: usize },
    InvalidRegion { source_ordinal: usize },
    Catalog(SoundFontPresetCatalogError),
}

impl HiDefSoundFontAssetError {
    fn from_prepared_bank(error: PreparedSoundFontBankError) -> Self {
        match error {
            PreparedSoundFontBankError::SampleStorage => Self::SampleStorage,
            PreparedSoundFontBankError::InvalidPresetAddress { source_ordinal } => {
                Self::InvalidPresetAddress { source_ordinal }
            }
            PreparedSoundFontBankError::InvalidInstrumentReference { source_ordinal } => {
                Self::InvalidInstrumentReference { source_ordinal }
            }
            PreparedSoundFontBankError::InvalidSampleReference { source_ordinal } => {
                Self::InvalidSampleReference { source_ordinal }
            }
            PreparedSoundFontBankError::InvalidRegion { source_ordinal } => {
                Self::InvalidRegion { source_ordinal }
            }
        }
    }
}

impl fmt::Display for HiDefSoundFontAssetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(error) => error.fmt(formatter),
            Self::UnexpectedPath => formatter.write_str("HiDef asset path is not the fixed asset"),
            Self::FileOpen => formatter.write_str("failed to open the SoundFont"),
            Self::Parse => formatter.write_str("failed to parse the SoundFont"),
            Self::Metadata => formatter.write_str("SoundFont metadata is inconsistent"),
            Self::SampleStorage => {
                formatter.write_str("failed to allocate numeric SoundFont sample storage")
            }
            Self::InvalidPresetAddress { source_ordinal } => write!(
                formatter,
                "SoundFont preset {source_ordinal} has an invalid numeric address"
            ),
            Self::InvalidInstrumentReference { source_ordinal } => write!(
                formatter,
                "SoundFont preset {source_ordinal} references an invalid instrument"
            ),
            Self::InvalidSampleReference { source_ordinal } => write!(
                formatter,
                "SoundFont preset {source_ordinal} references an invalid sample"
            ),
            Self::InvalidRegion { source_ordinal } => {
                write!(
                    formatter,
                    "SoundFont preset {source_ordinal} has an invalid region"
                )
            }
            Self::Catalog(error) => write!(formatter, "invalid SoundFont preset catalog: {error}"),
        }
    }
}

impl std::error::Error for HiDefSoundFontAssetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Catalog(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_asset_is_parsed_once_into_sorted_control_and_numeric_projections() {
        let asset = HiDefSoundFontAsset::load().unwrap();
        assert_eq!(asset.parse_count(), 1);
        assert!(!asset.catalog().entries().is_empty());
        assert!(asset
            .catalog()
            .entries()
            .windows(2)
            .all(|pair| pair[0].id() < pair[1].id()));
        assert!(asset
            .catalog()
            .entries()
            .iter()
            .all(|entry| asset.prepared_bank().has_preset(entry.id())));
    }
}
