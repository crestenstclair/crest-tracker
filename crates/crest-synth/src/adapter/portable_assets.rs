//! Worker-side transfer of canonical asset sources. This module performs disk
//! I/O, parsing and allocation; it must never run on the audio callback.
use super::filesystem_file_browser::FilesystemFileBrowser;
use super::filesystem_sample_catalog::read_sample_path;
use super::{dx7_library, model_assets, production_instruments, sfz_library};
use crate::synth::{
    AssetFileId, AssetKind, AssetReference, SampleAssetCatalogPort, SampleDecoderPort,
    MAX_SAMPLE_SOURCE_BYTES,
};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::path::{Path, PathBuf};

/// Exact baked resources remain native requirements; other assets carry their
/// source bytes. SFZ sources include their samples and expanded includes.
#[derive(Debug, Eq, PartialEq)]
pub enum PortableAsset {
    Builtin,
    Bytes(Vec<u8>),
}

/// Reads a user-selected source using the native admission rules. SFZ includes
/// and samples are bundled before hashing, exactly as for installed sources.
pub fn read_selected(kind: AssetKind, path: &Path) -> Result<Vec<u8>, String> {
    let bytes = if kind == AssetKind::Sfz {
        sfz_library::bundle(path).map_err(|error| error.to_string())?
    } else {
        read_sample_path(path).map_err(|error| error.to_string())?
    };
    Ok(validated_source(kind, &bytes)?.into_owned())
}

/// Reads an installed asset using its owning catalog, never a caller-supplied
/// filesystem path. Transient browser identities cannot become portable sources.
pub fn read(reference: &AssetReference) -> Result<PortableAsset, String> {
    if is_baked(reference) {
        return Ok(PortableAsset::Builtin);
    }
    validate_reference(reference)?;
    let bytes = match reference.kind() {
        AssetKind::Sample => production_instruments::production_sample_catalog()
            .map_err(|error| error.to_string())?
            .ok_or("Sample library is unavailable")?
            .read(&AssetFileId::new(reference.locator()).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?,
        AssetKind::SoundFont => production_instruments::production_soundfont_catalog()
            .map_err(|error| error.to_string())?
            .read_source(reference)
            .map_err(|error| error.to_string())?,
        AssetKind::SysEx => {
            let browser = dx7_library::browser().map_err(|error| error.to_string())?;
            let path = browser
                .resolve(reference.locator())
                .map_err(|error| error.to_string())?;
            read_sample_path(&path).map_err(|error| error.to_string())?
        }
        AssetKind::Sfz => {
            canonicalize_sfz(&sfz_library::load(reference).map_err(|error| error.to_string())?)?
        }
        AssetKind::NeuralModel | AssetKind::ImpulseResponse => {
            model_assets::read(reference).map_err(|error| error.to_string())?
        }
        AssetKind::Other => return Err("Unsupported portable asset kind".into()),
    };
    validate_size(bytes.len())?;
    verify_content_identity(reference, &bytes)?;
    Ok(PortableAsset::Bytes(bytes))
}

/// Validates source bytes with the native engine and stores an immutable
/// content-hash filename in its existing library. No active graph or library
/// root changes. The returned reference must still pass normal graph admission.
pub fn install(kind: AssetKind, bytes: &[u8]) -> Result<AssetReference, String> {
    validate_size(bytes.len())?;
    if kind == AssetKind::SoundFont {
        return production_instruments::production_soundfont_catalog()
            .map_err(|error| error.to_string())?
            .store_source(bytes)
            .map_err(|error| error.to_string());
    }
    let browser = match kind {
        AssetKind::Sample => {
            let catalog = production_instruments::production_sample_catalog()
                .map_err(|error| error.to_string())?
                .ok_or("Sample library is unavailable")?;
            FilesystemFileBrowser::new(catalog.root())
        }
        AssetKind::SysEx => dx7_library::browser(),
        AssetKind::Sfz | AssetKind::NeuralModel | AssetKind::ImpulseResponse => {
            model_assets::browser(kind)
        }
        AssetKind::Other | AssetKind::SoundFont => {
            return Err("Unsupported portable asset kind".into())
        }
    }
    .map_err(|error| error.to_string())?;
    install_into(&browser, kind, bytes)
}

fn is_baked(reference: &AssetReference) -> bool {
    matches!(
        (reference.kind(), reference.locator()),
        (AssetKind::SysEx, dx7_library::BUNDLED)
            | (AssetKind::NeuralModel, model_assets::NAM_DEFAULT)
            | (AssetKind::ImpulseResponse, model_assets::IR_DEFAULT)
    )
}

fn validate_reference(reference: &AssetReference) -> Result<(), String> {
    if reference.kind() == AssetKind::Other {
        return Err("Unsupported portable asset kind".into());
    }
    if (reference.kind() == AssetKind::SoundFont
        && reference.locator() == super::hidef_soundfont_capability::HIDEF_SOUNDFONT_PATH)
        || (reference.kind() == AssetKind::Sfz && reference.locator() == sfz_library::DEFAULT)
    {
        return Ok(());
    }
    if reference.locator().starts_with("@bundled/") {
        return Err("Unknown builtin asset reference".into());
    }
    let id = AssetFileId::new(reference.locator()).map_err(|error| error.to_string())?;
    if id.is_external() {
        return Err("Portable assets require an installed library-relative identity".into());
    }
    Ok(())
}

fn validate_size(length: usize) -> Result<(), String> {
    if length == 0 {
        return Err("Portable asset source is empty".into());
    }
    if u64::try_from(length).map_err(|_| "Portable asset source is too large")?
        > MAX_SAMPLE_SOURCE_BYTES
    {
        return Err("Portable asset source exceeds the source byte budget".into());
    }
    Ok(())
}

/// Whether this reference declares an immutable content identity. Reading still
/// verifies that identity against the actual bytes.
pub fn is_content_addressed(reference: &AssetReference) -> bool {
    content_digest(reference).is_some()
}

fn content_digest(reference: &AssetReference) -> Option<&str> {
    let name = reference.locator().strip_prefix("Imported/")?;
    let (stem, extension) = name.rsplit_once('.')?;
    if extension.is_empty() || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return None;
    }
    let expected = stem.get(..64)?;
    if !expected
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let suffix = &stem[64..];
    if !suffix.is_empty()
        && !suffix
            .strip_prefix('-')
            .is_some_and(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    Some(expected)
}

fn verify_content_identity(reference: &AssetReference, bytes: &[u8]) -> Result<(), String> {
    if let Some(expected) = content_digest(reference) {
        if format!("{:x}", Sha256::digest(bytes)) != expected {
            return Err(format!(
                "Portable asset content changed for immutable reference {}",
                reference.locator()
            ));
        }
    }
    Ok(())
}

fn install_into(
    browser: &FilesystemFileBrowser,
    kind: AssetKind,
    bytes: &[u8],
) -> Result<AssetReference, String> {
    let source = validated_source(kind, bytes)?;
    let bytes = source.as_ref();
    let extension = match kind {
        AssetKind::Sample | AssetKind::ImpulseResponse => "wav",
        AssetKind::SoundFont => "sf2",
        AssetKind::SysEx => "syx",
        AssetKind::Sfz => "sfz",
        AssetKind::NeuralModel => "nam",
        AssetKind::Other => return Err("Unsupported portable asset kind".into()),
    };
    let name = format!("{:x}.{extension}", Sha256::digest(bytes));
    // A generated relative source name forces the existing import path. Passing
    // an absolute in-library source would return its old ID without storing bytes.
    let id = browser
        .store_validated_file(Path::new(&name), bytes, extension)
        .map_err(|error| error.to_string())?;
    AssetReference::new(kind, id.as_str()).map_err(|error| error.to_string())
}

fn validated_source(kind: AssetKind, bytes: &[u8]) -> Result<Cow<'_, [u8]>, String> {
    validate_size(bytes.len())?;
    let source = if kind == AssetKind::Sfz {
        Cow::Owned(canonicalize_sfz(bytes)?)
    } else {
        Cow::Borrowed(bytes)
    };
    let bytes = source.as_ref();
    match kind {
        AssetKind::Sample => super::wav_sample_decoder::WavSampleDecoder
            .decode(
                &AssetFileId::new("portable.wav").map_err(|error| error.to_string())?,
                bytes,
            )
            .map(|_| ())
            .map_err(|error| error.to_string()),
        AssetKind::SoundFont => {
            super::hidef_soundfont_asset::HiDefSoundFontAsset::from_bytes(bytes)
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
        AssetKind::SysEx => dx7_library::Dx7Library::parse(bytes)
            .map(|_| ())
            .map_err(|error| error.to_string()),
        AssetKind::Sfz => {
            super::upstream_audio::validate_sfz(bytes).map_err(|error| error.to_string())
        }
        AssetKind::NeuralModel | AssetKind::ImpulseResponse => {
            super::upstream_audio::validate_model_asset(kind, bytes)
                .map_err(|error| error.to_string())
        }
        AssetKind::Other => Err("Unsupported portable asset kind".into()),
    }?;
    Ok(source)
}

fn canonicalize_sfz(bytes: &[u8]) -> Result<Vec<u8>, String> {
    validate_size(bytes.len())?;
    let staging = SfzStaging::new(bytes)?;
    // The native parser confines includes and samples to this empty directory.
    // Only self-contained bytes can succeed; arbitrary references cannot reach
    // the string loader, which otherwise permits filesystem dependencies.
    let bundled =
        sfz_library::bundle(&staging.0.join("source.sfz")).map_err(|error| error.to_string())?;
    validate_size(bundled.len())?;
    Ok(bundled)
}

struct SfzStaging(PathBuf);

impl SfzStaging {
    fn new(bytes: &[u8]) -> Result<Self, String> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "crest-portable-sfz-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ));
        std::fs::create_dir(&directory).map_err(|error| error.to_string())?;
        let staging = Self(directory);
        std::fs::write(staging.0.join("source.sfz"), bytes).map_err(|error| error.to_string())?;
        Ok(staging)
    }
}

impl Drop for SfzStaging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let staging = SfzStaging::new(b"fixture").unwrap();
            std::fs::remove_file(staging.0.join("source.sfz")).unwrap();
            let path = staging.0.clone();
            std::mem::forget(staging);
            Self(path)
        }

        fn browser(&self) -> FilesystemFileBrowser {
            FilesystemFileBrowser::new(&self.0).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn content_names_reuse_exact_bytes_without_overwriting_collisions() {
        let fixture = Fixture::new();
        let browser = fixture.browser();
        let bytes = include_bytes!("../../assets/sample-test.wav");
        let name = format!("{:x}.wav", Sha256::digest(bytes));
        std::fs::create_dir(fixture.0.join("Imported")).unwrap();
        let collision = fixture.0.join("Imported").join(&name);
        std::fs::write(&collision, b"existing unrelated bytes").unwrap();
        let installed = install_into(&browser, AssetKind::Sample, bytes).unwrap();
        assert_eq!(
            installed.locator(),
            format!("Imported/{}-1.wav", name.trim_end_matches(".wav"))
        );
        assert_eq!(
            std::fs::read(&collision).unwrap(),
            b"existing unrelated bytes"
        );
        assert_eq!(
            std::fs::read(browser.resolve(installed.locator()).unwrap()).unwrap(),
            bytes
        );
        assert_eq!(
            install_into(&browser, AssetKind::Sample, bytes).unwrap(),
            installed
        );
        let mut changed = bytes.to_vec();
        *changed.last_mut().unwrap() ^= 1;
        assert_ne!(
            install_into(&browser, AssetKind::Sample, &changed).unwrap(),
            installed
        );
        assert_eq!(
            std::fs::read_dir(fixture.0.join("Imported"))
                .unwrap()
                .count(),
            3
        );
    }

    #[test]
    fn malformed_assets_fail_native_validation_before_library_writes() {
        let fixture = Fixture::new();
        for kind in [
            AssetKind::Sample,
            AssetKind::SoundFont,
            AssetKind::SysEx,
            AssetKind::Sfz,
            AssetKind::NeuralModel,
            AssetKind::ImpulseResponse,
            AssetKind::Other,
        ] {
            assert!(
                install_into(&fixture.browser(), kind, b"not an asset").is_err(),
                "{kind:?}"
            );
        }
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 0);
    }

    #[test]
    fn changed_content_addressed_imports_fail_read_integrity_checks() {
        let fixture = Fixture::new();
        let bytes = include_bytes!("../../assets/sample-test.wav");
        let installed = install_into(&fixture.browser(), AssetKind::Sample, bytes).unwrap();
        let path = fixture.browser().resolve(installed.locator()).unwrap();
        verify_content_identity(&installed, &read_sample_path(&path).unwrap()).unwrap();
        let suffixed = AssetReference::new(
            AssetKind::Sample,
            format!("Imported/{:x}-2.wav", Sha256::digest(bytes)),
        )
        .unwrap();
        verify_content_identity(&suffixed, bytes).unwrap();
        let mut changed = bytes.to_vec();
        *changed.last_mut().unwrap() ^= 1;
        std::fs::write(&path, &changed).unwrap();
        assert!(verify_content_identity(&installed, &read_sample_path(&path).unwrap()).is_err());
        assert!(verify_content_identity(&suffixed, &changed).is_err());
        for locator in [
            "Imported/ordinary.wav".to_owned(),
            format!("Imported/{:x}-named.wav", Sha256::digest(bytes)),
            format!("Other/{:x}.wav", Sha256::digest(bytes)),
        ] {
            let legacy = AssetReference::new(AssetKind::Sample, locator).unwrap();
            verify_content_identity(&legacy, &changed).unwrap();
        }
    }

    #[test]
    fn baked_markers_require_exact_kind_and_identity() {
        for (kind, locator) in [
            (AssetKind::SysEx, dx7_library::BUNDLED),
            (AssetKind::NeuralModel, model_assets::NAM_DEFAULT),
            (AssetKind::ImpulseResponse, model_assets::IR_DEFAULT),
        ] {
            assert_eq!(
                read(&AssetReference::new(kind, locator).unwrap()).unwrap(),
                PortableAsset::Builtin
            );
            assert!(read(&AssetReference::new(AssetKind::Sample, locator).unwrap()).is_err());
        }
        let PortableAsset::Bytes(bytes) =
            read(&AssetReference::new(AssetKind::Sfz, sfz_library::DEFAULT).unwrap()).unwrap()
        else {
            panic!("SFZ has a transferable self-contained source");
        };
        validated_source(AssetKind::Sfz, &bytes).unwrap();
    }

    #[test]
    fn source_identities_and_byte_budget_are_validated_before_catalog_access() {
        for locator in [
            "../outside.wav",
            "/outside.wav",
            "@home/outside.wav",
            "@volumes/outside.wav",
            "x\\outside.wav",
        ] {
            assert!(read(&AssetReference::new(AssetKind::Sample, locator).unwrap()).is_err());
        }
        assert!(validate_size(0).is_err());
        assert!(validate_size(MAX_SAMPLE_SOURCE_BYTES as usize).is_ok());
        assert!(validate_size(MAX_SAMPLE_SOURCE_BYTES as usize + 1).is_err());
    }

    #[test]
    fn sysex_sfz_and_impulses_use_their_native_validators_and_exact_sources() {
        let fixture = Fixture::new();
        let voice = dx7_library::Dx7Library::bundled().presets[0].data;
        let mut sysex = vec![0xf0, 0x43, 0, 0, 1, 27];
        sysex.extend_from_slice(&voice);
        sysex.push(
            (128 - (voice.iter().map(|byte| u32::from(*byte)).sum::<u32>() & 127)) as u8 & 127,
        );
        sysex.push(0xf7);
        for (kind, bytes) in [
            (AssetKind::SysEx, sysex.as_slice()),
            (AssetKind::Sfz, &b"<region>\nsample=*sine\nkey=60\n"[..]),
            (
                AssetKind::ImpulseResponse,
                &include_bytes!("../../assets/sample-test.wav")[..],
            ),
        ] {
            let installed = install_into(&fixture.browser(), kind, bytes).unwrap();
            assert_eq!(installed.kind(), kind);
            assert_eq!(
                std::fs::read(fixture.browser().resolve(installed.locator()).unwrap()).unwrap(),
                bytes
            );
        }
    }

    #[test]
    fn portable_sfz_rejects_external_dependencies_before_storage() {
        let fixture = Fixture::new();
        let source = fixture.0.join("outside.wav");
        std::fs::write(&source, include_bytes!("../../assets/sample-test.wav")).unwrap();
        let dependency = format!("<region> sample={}\n", source.display());
        assert!(install_into(&fixture.browser(), AssetKind::Sfz, dependency.as_bytes()).is_err());
        let include = fixture.0.join("outside.sfz");
        std::fs::write(&include, b"<region> sample=*sine").unwrap();
        let dependency = format!("#include \"{}\"\n", include.display());
        assert!(install_into(&fixture.browser(), AssetKind::Sfz, dependency.as_bytes()).is_err());
        assert!(!fixture.0.join("Imported").exists());
    }

    #[test]
    fn collected_sfz_is_canonical_across_install_and_read_with_embedded_samples() {
        let fixture = Fixture::new();
        std::fs::write(
            fixture.0.join("sample.wav"),
            include_bytes!("../../assets/sample-test.wav"),
        )
        .unwrap();
        std::fs::write(
            fixture.0.join("zones.sfz"),
            b"<region> sample=sample.wav key=60\n",
        )
        .unwrap();
        let instrument = fixture.0.join("instrument.sfz");
        std::fs::write(&instrument, b"#include \"zones.sfz\"\n").unwrap();
        let expanded = sfz_library::bundle(&instrument).unwrap();
        let collected = canonicalize_sfz(&expanded).unwrap();
        assert_ne!(
            expanded, collected,
            "first expansion uses different embedded-sample formatting"
        );
        assert_eq!(canonicalize_sfz(&collected).unwrap(), collected);
        let installed = install_into(&fixture.browser(), AssetKind::Sfz, &expanded).unwrap();
        let installed_path = fixture.browser().resolve(installed.locator()).unwrap();
        assert_eq!(std::fs::read(&installed_path).unwrap(), collected);
        // Same operations as production read, against this isolated catalog.
        let reread = canonicalize_sfz(&sfz_library::bundle(&installed_path).unwrap()).unwrap();
        assert_eq!(reread, collected);
        assert_eq!(
            install_into(&fixture.browser(), AssetKind::Sfz, &reread).unwrap(),
            installed
        );
    }

    #[cfg(unix)]
    #[test]
    fn import_directory_symlink_cannot_escape_the_library() {
        let fixture = Fixture::new();
        let outside = Fixture::new();
        std::os::unix::fs::symlink(&outside.0, fixture.0.join("Imported")).unwrap();
        assert!(install_into(
            &fixture.browser(),
            AssetKind::Sample,
            include_bytes!("../../assets/sample-test.wav")
        )
        .is_err());
        assert_eq!(std::fs::read_dir(&outside.0).unwrap().count(), 0);
    }
}
