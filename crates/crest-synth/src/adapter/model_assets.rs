//! Worker-side model/impulse assets. Imported files are validated before copying.
use super::filesystem_file_browser::FilesystemFileBrowser;
use super::filesystem_sample_catalog::read_sample_path;
use crate::synth::*;
pub const NAM_FILE: &str = "nam.file";
pub const IR_FILE: &str = "convolution.file";
pub const NAM_DEFAULT: &str = "@bundled/NAM Test LSTM.nam";
pub const IR_DEFAULT: &str = "@bundled/Unit impulse (transparent).wav";
pub fn spec(id: &str, kind: AssetKind, reference: &str) -> Result<ParameterSpec, CapabilityError> {
    ParameterSpec::new_with_patch_interaction(
        ParameterId::new(id).unwrap(),
        if kind == AssetKind::NeuralModel {
            "NAM Model"
        } else {
            "Impulse Response"
        },
        ParameterKind::Asset,
        ParameterUpdate::Structural,
        PatchInteraction::ReadOnly,
        ParameterDefault::Asset(AssetReference::new(kind, reference)?),
        None,
        Vec::new(),
        None,
        None,
        None,
        "asset",
        None,
        None,
    )
}
pub fn browser(kind: AssetKind) -> Result<FilesystemFileBrowser, SampleAssetError> {
    let folder = match kind {
        AssetKind::Sfz => "SFZ Libraries",
        AssetKind::NeuralModel => "NAM Models",
        AssetKind::ImpulseResponse => "Impulse Responses",
        _ => return Err(SampleAssetError::UnsupportedContainer),
    };
    let root =
        std::path::PathBuf::from(std::env::var_os("HOME").ok_or(SampleAssetError::Unavailable)?)
            .join("Music/Crest Synth")
            .join(folder);
    std::fs::create_dir_all(&root).map_err(|_| SampleAssetError::Unavailable)?;
    Ok(FilesystemFileBrowser::new(root)?.with_user_locations())
}
pub fn read(reference: &AssetReference) -> Result<Vec<u8>, SampleAssetError> {
    let id = AssetFileId::new(reference.locator())?;
    if id.is_external() {
        return Err(SampleAssetError::InvalidRelativeId);
    }
    read_sample_path(&browser(reference.kind())?.resolve(id.as_str())?)
}
pub fn import(kind: AssetKind, id: &AssetFileId) -> Result<AssetFileId, SampleAssetError> {
    let browser = browser(kind)?;
    let path = browser.resolve(id.as_str())?;
    let bytes = read_sample_path(&path)?;
    super::upstream_audio::validate_model_asset(kind, &bytes)?;
    browser.store_validated_file(
        &path,
        &bytes,
        if kind == AssetKind::NeuralModel {
            "nam"
        } else {
            "wav"
        },
    )
}
