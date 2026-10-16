use super::filesystem_file_browser::FilesystemFileBrowser;
use super::filesystem_sample_catalog::read_sample_path;
use super::hidef_soundfont_asset::{HiDefSoundFontAsset, HiDefSoundFontAssetError};
use super::hidef_soundfont_capability::{HiDefSoundFontCapability, HIDEF_SOUNDFONT_PATH};
use super::soundfont_voice_engine::PreparedSoundFontBank;
use crate::synth::{
    AssetFileId, AssetKind, AssetReference, CapabilityDescriptor, FileBrowserFolderId,
    FileBrowserListing, InstrumentCapabilityProvider, SampleAssetError, SoundFontPresetCatalog,
};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, Weak};

type CachedBank = (Arc<SoundFontPresetCatalog>, Weak<PreparedSoundFontBank>);

/// Worker-owned bank resources. Weak numeric ownership never keeps retired banks alive.
pub struct FilesystemSoundFontCatalog {
    browser: FilesystemFileBrowser,
    bundled: HiDefSoundFontAsset,
    banks: Mutex<BTreeMap<String, CachedBank>>,
}

impl FilesystemSoundFontCatalog {
    pub fn new(
        root: impl AsRef<Path>,
        bundled: HiDefSoundFontAsset,
    ) -> Result<Self, SampleAssetError> {
        Ok(Self {
            browser: FilesystemFileBrowser::new(root)?,
            bundled,
            banks: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn with_user_locations(mut self) -> Self {
        self.browser = self.browser.with_user_locations();
        self
    }

    pub fn list(
        &self,
        folder: &FileBrowserFolderId,
    ) -> Result<FileBrowserListing, SampleAssetError> {
        self.browser.list(folder, AssetKind::SoundFont)
    }

    /// Original bank bytes for hashing and collection, outside the callback.
    pub fn read_source(&self, reference: &AssetReference) -> Result<Vec<u8>, SampleAssetError> {
        if reference.kind() != AssetKind::SoundFont {
            return Err(SampleAssetError::MalformedSoundFont);
        }
        if reference.locator() == HIDEF_SOUNDFONT_PATH {
            return read_sample_path(
                &super::bundled_resource::path(HIDEF_SOUNDFONT_PATH)
                    .map_err(|_| SampleAssetError::Unavailable)?,
            );
        }
        let id = AssetFileId::new(reference.locator())?;
        if id.is_external() {
            return Err(SampleAssetError::InvalidRelativeId);
        }
        read_sample_path(&self.browser.resolve(id.as_str())?)
    }

    /// Admit a complete bank under its content identity. Exact preset choice
    /// remains in the caller's configuration and is checked during restore.
    pub fn store_source(&self, bytes: &[u8]) -> Result<AssetReference, SampleAssetError> {
        use sha2::{Digest, Sha256};
        HiDefSoundFontAsset::from_bytes(bytes).map_err(asset_error)?;
        let name = format!("{:x}.sf2", Sha256::digest(bytes));
        let id = self
            .browser
            .store_validated_file(Path::new(&name), bytes, "sf2")?;
        AssetReference::new(AssetKind::SoundFont, id.as_str())
            .map_err(|_| SampleAssetError::InvalidRelativeId)
    }

    pub fn load(
        &self,
        reference: &AssetReference,
    ) -> Result<HiDefSoundFontAsset, SampleAssetError> {
        if reference.kind() != AssetKind::SoundFont {
            return Err(SampleAssetError::MalformedSoundFont);
        }
        if reference.locator() == HIDEF_SOUNDFONT_PATH {
            return Ok(self.bundled.clone());
        }
        let id = AssetFileId::new(reference.locator())?;
        if id.is_external() {
            return Err(SampleAssetError::InvalidRelativeId);
        }
        let mut banks = self
            .banks
            .lock()
            .map_err(|_| SampleAssetError::Unavailable)?;
        banks.retain(|_, (_, bank)| bank.strong_count() > 0);
        if let Some((catalog, bank)) = banks.get(id.as_str()) {
            if let Some(bank) = bank.upgrade() {
                return Ok(HiDefSoundFontAsset::from_projections(catalog.clone(), bank));
            }
        }
        let asset = HiDefSoundFontAsset::load_from_path(&self.browser.resolve(id.as_str())?)
            .map_err(asset_error)?;
        banks.insert(
            id.as_str().to_owned(),
            (asset.catalog(), Arc::downgrade(&asset.prepared_bank())),
        );
        Ok(asset)
    }

    pub fn import_browser_asset(
        &self,
        id: &AssetFileId,
    ) -> Result<(AssetFileId, CapabilityDescriptor), SampleAssetError> {
        self.import_file(&self.browser.resolve(id.as_str())?)
    }

    pub fn import_file(
        &self,
        source: &Path,
    ) -> Result<(AssetFileId, CapabilityDescriptor), SampleAssetError> {
        let source = source
            .canonicalize()
            .map_err(|_| SampleAssetError::Unavailable)?;
        let bytes = read_sample_path(&source)?;
        let asset = HiDefSoundFontAsset::from_bytes(&bytes).map_err(asset_error)?;
        let id = self.browser.store_validated_file(&source, &bytes, "sf2")?;
        let reference = AssetReference::new(AssetKind::SoundFont, id.as_str())
            .map_err(|_| SampleAssetError::InvalidRelativeId)?;
        let descriptor = HiDefSoundFontCapability::for_asset(asset.catalog(), reference)
            .map_err(|_| SampleAssetError::MalformedSoundFont)?
            .descriptor();
        Ok((id, descriptor))
    }
}

fn asset_error(error: HiDefSoundFontAssetError) -> SampleAssetError {
    match error {
        HiDefSoundFontAssetError::File(error) => error,
        _ => SampleAssetError::MalformedSoundFont,
    }
}
