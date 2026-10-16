use crate::adapter::braids_capability::BraidsCapability;
use crate::adapter::braids_preparer::BraidsPreparer;
use crate::adapter::hidef_soundfont_asset::HiDefSoundFontAsset;
use crate::adapter::hidef_soundfont_capability::HiDefSoundFontCapability;
use crate::adapter::hidef_soundfont_preparer::HiDefSoundFontPreparer;
use crate::adapter::sample_capability::SampleCapability;
use crate::adapter::sample_preparer::SamplePreparer;
use crate::adapter::{
    filesystem_sample_catalog::FilesystemSampleCatalog, wav_sample_decoder::WavSampleDecoder,
};
use crate::synth::instrument_capability::{CapabilityError, CapabilityRegistry};
use crate::synth::instrument_capability_provider::InstrumentCapabilityProvider;
use crate::synth::instrument_preparer::{InstrumentPreparationError, InstrumentPreparer};
use crate::synth::{AssetFileId, SampleAssetCatalogPort, SampleAssetError, SampleDecoderPort};
use std::sync::{Arc, OnceLock};

pub const SAMPLE_LIBRARY_ROOT_ENV: &str = "CREST_SAMPLE_LIBRARY_ROOT";
pub const SAMPLE_DEFAULT_ASSET_ENV: &str = "CREST_SAMPLE_DEFAULT_ASSET";

pub type ProductionSampleRootListing = Option<(
    crate::synth::FileBrowserFolderId,
    Result<crate::synth::FileBrowserListing, SampleAssetError>,
)>;

static SHARED_TEST_COMPOSITION_ASSET: OnceLock<
    Result<HiDefSoundFontAsset, crate::adapter::hidef_soundfont_asset::HiDefSoundFontAssetError>,
> = OnceLock::new();
static OPTIONAL_PRODUCTION_SAMPLE: OnceLock<
    Result<Option<ProductionSamplePorts>, ProductionInstrumentCompositionError>,
> = OnceLock::new();

#[derive(Clone)]
struct ProductionSamplePorts {
    capability: SampleCapability,
    catalog: Arc<FilesystemSampleCatalog>,
    decoder: Arc<dyn SampleDecoderPort>,
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum ProductionInstrumentCompositionError {
    #[error(transparent)]
    Upstream(#[from] super::upstream_audio::CatalogError),
    #[error("failed to load the production SoundFont asset: {0}")]
    Asset(crate::adapter::hidef_soundfont_asset::HiDefSoundFontAssetError),
    #[error("failed to construct the production capability registry: {0}")]
    Capability(CapabilityError),
    #[error("failed to construct the production instrument preparers: {0}")]
    Preparation(InstrumentPreparationError),
    #[error("invalid production Sample configuration: {0}")]
    Sample(SampleAssetError),
    #[error(
        "{SAMPLE_LIBRARY_ROOT_ENV} and {SAMPLE_DEFAULT_ASSET_ENV} must be configured together"
    )]
    IncompleteSampleConfiguration,
    #[error("SoundFont library is unavailable: {0}")]
    SoundFontLibrary(SampleAssetError),
}

/// Shared process-local asset used by test composition helpers. The standalone
/// composition root loads and owns its explicit asset instead.
pub fn production_soundfont_asset(
) -> Result<&'static HiDefSoundFontAsset, ProductionInstrumentCompositionError> {
    SHARED_TEST_COMPOSITION_ASSET
        .get_or_init(HiDefSoundFontAsset::load)
        .as_ref()
        .map_err(|error| ProductionInstrumentCompositionError::Asset(error.clone()))
}

pub fn production_soundfont_capability(
) -> Result<HiDefSoundFontCapability, ProductionInstrumentCompositionError> {
    HiDefSoundFontCapability::new(production_soundfont_asset()?.catalog())
        .map_err(ProductionInstrumentCompositionError::Capability)
}

fn optional_production_sample(
) -> Result<Option<ProductionSamplePorts>, ProductionInstrumentCompositionError> {
    OPTIONAL_PRODUCTION_SAMPLE
        .get_or_init(load_optional_production_sample)
        .clone()
}

fn load_optional_production_sample(
) -> Result<Option<ProductionSamplePorts>, ProductionInstrumentCompositionError> {
    let (root, asset) = match (
        std::env::var_os(SAMPLE_LIBRARY_ROOT_ENV),
        std::env::var_os(SAMPLE_DEFAULT_ASSET_ENV),
    ) {
        (None, None) => default_sample_library()?,
        (Some(root), Some(asset)) => (root, asset),
        _ => return Err(ProductionInstrumentCompositionError::IncompleteSampleConfiguration),
    };
    let asset = asset.into_string().map_err(|_| {
        ProductionInstrumentCompositionError::Sample(SampleAssetError::InvalidRelativeId)
    })?;
    let asset = AssetFileId::new(asset).map_err(ProductionInstrumentCompositionError::Sample)?;
    let catalog = Arc::new(
        FilesystemSampleCatalog::new(root)
            .map_err(ProductionInstrumentCompositionError::Sample)?
            .with_user_locations(),
    );
    let decoder = Arc::new(WavSampleDecoder);
    // Registration is all-or-nothing: prove the configured default through
    // the real filesystem and WAV adapters before exposing the descriptor.
    let bytes = catalog
        .read(&asset)
        .map_err(ProductionInstrumentCompositionError::Sample)?;
    decoder
        .decode(&asset, &bytes)
        .map_err(ProductionInstrumentCompositionError::Sample)?;
    let capability =
        SampleCapability::new(asset).map_err(ProductionInstrumentCompositionError::Capability)?;
    Ok(Some(ProductionSamplePorts {
        capability,
        catalog,
        decoder,
    }))
}

fn default_sample_library(
) -> Result<(std::ffi::OsString, std::ffi::OsString), ProductionInstrumentCompositionError> {
    use std::io::Write;
    let home = std::env::var_os("HOME").ok_or(ProductionInstrumentCompositionError::Sample(
        SampleAssetError::Unavailable,
    ))?;
    let root = std::path::PathBuf::from(home).join("Music/Crest Synth/Samples");
    std::fs::create_dir_all(&root)
        .map_err(|_| ProductionInstrumentCompositionError::Sample(SampleAssetError::Unavailable))?;
    let path = root.join("Test Tone.wav");
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            if file
                .write_all(include_bytes!("../../assets/sample-test.wav"))
                .and_then(|_| file.sync_all())
                .is_err()
            {
                let _ = std::fs::remove_file(path);
                return Err(ProductionInstrumentCompositionError::Sample(
                    SampleAssetError::Unavailable,
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => {
            return Err(ProductionInstrumentCompositionError::Sample(
                SampleAssetError::Unavailable,
            ))
        }
    }
    Ok((root.into_os_string(), "Test Tone.wav".into()))
}

/// The same library used by graph preparation, browser enumeration, and import.
pub fn production_sample_catalog(
) -> Result<Option<Arc<FilesystemSampleCatalog>>, ProductionInstrumentCompositionError> {
    Ok(optional_production_sample()?.map(|sample| sample.catalog))
}

/// Shared worker-side SoundFont library used by import, graph preparation and Open.
pub fn production_soundfont_catalog() -> Result<
    Arc<super::filesystem_soundfont_catalog::FilesystemSoundFontCatalog>,
    ProductionInstrumentCompositionError,
> {
    static LIBRARY: OnceLock<
        Result<
            Arc<super::filesystem_soundfont_catalog::FilesystemSoundFontCatalog>,
            ProductionInstrumentCompositionError,
        >,
    > = OnceLock::new();
    LIBRARY
        .get_or_init(|| {
            let home = std::env::var_os("HOME").ok_or(
                ProductionInstrumentCompositionError::SoundFontLibrary(
                    SampleAssetError::Unavailable,
                ),
            )?;
            let root = std::path::PathBuf::from(home).join("Music/Crest Synth/SoundFonts");
            std::fs::create_dir_all(&root).map_err(|_| {
                ProductionInstrumentCompositionError::SoundFontLibrary(
                    SampleAssetError::Unavailable,
                )
            })?;
            let catalog = super::filesystem_soundfont_catalog::FilesystemSoundFontCatalog::new(
                root,
                production_soundfont_asset()?.clone(),
            )
            .map_err(ProductionInstrumentCompositionError::SoundFontLibrary)?
            .with_user_locations();
            Ok(Arc::new(catalog))
        })
        .clone()
}

/// Builds the production providers in stable fixture/discovery order.
pub fn production_instrument_providers(
) -> Result<Vec<Box<dyn InstrumentCapabilityProvider>>, ProductionInstrumentCompositionError> {
    let mut providers: Vec<Box<dyn InstrumentCapabilityProvider>> = vec![
        Box::new(production_soundfont_capability()?),
        Box::new(
            BraidsCapability::new().map_err(ProductionInstrumentCompositionError::Capability)?,
        ),
    ];
    if let Some(sample) = optional_production_sample()? {
        let initial_asset = sample
            .capability
            .default_config()
            .map_err(ProductionInstrumentCompositionError::Capability)?;
        let asset = sample
            .capability
            .playback_config(&initial_asset)
            .map_err(ProductionInstrumentCompositionError::Sample)?
            .asset_id;
        providers.push(Box::new(sample.capability));
        providers.push(Box::new(
            super::drum_rack_capability::DrumRackCapability::new(asset)
                .map_err(ProductionInstrumentCompositionError::Capability)?,
        ));
    }
    providers.extend(
        super::upstream_audio::instrument_ports()?
            .into_iter()
            .map(|p| Box::new(p) as Box<dyn InstrumentCapabilityProvider>),
    );
    Ok(providers)
}

/// Builds the immutable production registry in stable fixture/discovery order.
pub fn production_capability_registry(
) -> Result<CapabilityRegistry, ProductionInstrumentCompositionError> {
    let providers = production_instrument_providers()?;
    CapabilityRegistry::new(
        providers
            .iter()
            .map(|provider| provider.descriptor())
            .collect(),
    )
    .map_err(ProductionInstrumentCompositionError::Capability)
}

/// Prepares the installed production factories in the same exact capability order.
pub fn production_instrument_preparers(
) -> Result<Vec<Box<dyn InstrumentPreparer>>, ProductionInstrumentCompositionError> {
    let mut preparers: Vec<Box<dyn InstrumentPreparer>> = vec![
        Box::new(
            HiDefSoundFontPreparer::new(production_soundfont_asset()?)
                .map_err(ProductionInstrumentCompositionError::Preparation)?
                .with_library(production_soundfont_catalog()?),
        ),
        Box::new(BraidsPreparer::new().map_err(ProductionInstrumentCompositionError::Preparation)?),
    ];
    if let Some(sample) = optional_production_sample()? {
        let sample_preparer = SamplePreparer::new(sample.catalog, sample.decoder)
            .map_err(ProductionInstrumentCompositionError::Preparation)?;
        preparers.push(Box::new(sample_preparer.clone()));
        let initial = sample
            .capability
            .default_config()
            .map_err(ProductionInstrumentCompositionError::Capability)?;
        let asset = sample
            .capability
            .playback_config(&initial)
            .map_err(ProductionInstrumentCompositionError::Sample)?
            .asset_id;
        preparers.push(Box::new(
            super::drum_rack_preparer::DrumRackPreparer::with_sample(asset, sample_preparer)
                .map_err(ProductionInstrumentCompositionError::Preparation)?,
        ));
    }
    preparers.extend(
        super::upstream_audio::instrument_ports()?
            .into_iter()
            .map(|p| Box::new(p) as Box<dyn InstrumentPreparer>),
    );
    Ok(preparers)
}

/// Returns the configured production library's root listing for reducer-owned
/// browser composition. Absence means Sample is not installed; a configured
/// adapter failure remains typed and never fabricates browser rows.
pub fn production_sample_root_listing(
) -> Result<ProductionSampleRootListing, ProductionInstrumentCompositionError> {
    let Some(sample) = optional_production_sample()? else {
        return Ok(None);
    };
    let folder = crate::synth::FileBrowserFolderId::default();
    Ok(Some((folder.clone(), sample.catalog.list(&folder))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::braids_capability::BRAIDS_CAPABILITY_ID;
    use crate::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID;

    #[test]
    fn production_composition_installs_all_matching_engine_ports() {
        let providers = production_instrument_providers().unwrap();
        let registry = production_capability_registry().unwrap();
        let preparers = production_instrument_preparers().unwrap();
        assert_eq!(providers.len(), registry.descriptors().len());
        let mut expected = vec![
            HIDEF_CAPABILITY_ID.to_owned(),
            BRAIDS_CAPABILITY_ID.to_owned(),
            crate::adapter::sample_capability::SAMPLE_CAPABILITY_ID.to_owned(),
            crate::adapter::drum_rack_capability::DRUM_RACK_CAPABILITY_ID.to_owned(),
        ];
        expected.extend(
            crate::adapter::upstream_audio::instrument_ports()
                .unwrap()
                .iter()
                .map(|port| port.descriptor().id().as_str().to_owned()),
        );
        assert_eq!(
            registry
                .descriptors()
                .iter()
                .map(|d| d.id().as_str())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            preparers
                .iter()
                .map(|p| p.capability_id().as_str())
                .collect::<Vec<_>>(),
            expected
        );
    }
}
