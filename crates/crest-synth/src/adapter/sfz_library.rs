//! SFZ package import uses sfizz's parser and produces a self-contained library.
use crate::synth::*;
use std::ffi::{c_char, CStr, CString};
pub const CAPABILITY: &str = "instrument.sfizz.sampler";
pub const FILE: &str = "sfizz.file";
pub const DEFAULT: &str = "@bundled/Crest Test Tone.sfz";
extern "C" {
    fn crest_sfz_bundle(path: *const c_char, budget: usize) -> *mut c_char;
    fn crest_sfz_bundle_free(data: *mut c_char);
}
pub fn bundle(path: &std::path::Path) -> Result<Vec<u8>, SampleAssetError> {
    let path = CString::new(path.to_str().ok_or(SampleAssetError::InvalidRelativeId)?)
        .map_err(|_| SampleAssetError::InvalidRelativeId)?;
    let raw = unsafe { crest_sfz_bundle(path.as_ptr(), MAX_SAMPLE_SOURCE_BYTES as usize) };
    if raw.is_null() {
        return Err(SampleAssetError::MalformedModel);
    }
    let bytes = unsafe { CStr::from_ptr(raw) }.to_bytes().to_vec();
    unsafe {
        crest_sfz_bundle_free(raw);
    }
    Ok(bytes)
}
pub fn load(reference: &AssetReference) -> Result<Vec<u8>, SampleAssetError> {
    if reference.kind() != AssetKind::Sfz {
        return Err(SampleAssetError::UnsupportedContainer);
    }
    if reference.locator() == DEFAULT {
        return Ok(include_bytes!("../../assets/sfizz-default.sfz").to_vec());
    }
    let id = AssetFileId::new(reference.locator())?;
    if id.is_external() {
        return Err(SampleAssetError::InvalidRelativeId);
    }
    bundle(&super::model_assets::browser(AssetKind::Sfz)?.resolve(id.as_str())?)
}
pub fn import(id: &AssetFileId) -> Result<AssetFileId, SampleAssetError> {
    let browser = super::model_assets::browser(AssetKind::Sfz)?;
    let path = browser.resolve(id.as_str())?;
    let bytes = bundle(&path)?;
    super::upstream_audio::validate_sfz(&bytes)?;
    browser.store_validated_file(&path, &bytes, "sfz")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "crest-sfz-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(path.join("samples")).unwrap();
            std::fs::write(
                path.join("samples/kick one.wav"),
                include_bytes!("../../assets/sample-test.wav"),
            )
            .unwrap();
            Self(path)
        }
        fn bundle(&self, text: &str) -> Result<Vec<u8>, SampleAssetError> {
            std::fs::write(self.0.join("instrument.sfz"), text).unwrap();
            super::bundle(&self.0.join("instrument.sfz"))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn sfz_packages_includes_definitions_and_samples_with_spaces() {
        let fixture = Fixture::new();
        std::fs::write(
            fixture.0.join("zones.sfz"),
            "<group> ampeg_release=0.2\n<region> sample=kick one.wav key=60\n",
        )
        .unwrap();
        let bundled=fixture.bundle("#define $SAMPLES samples/\n<control> default_path=$SAMPLES\n#include \"zones.sfz\"\n").unwrap();
        let text = std::str::from_utf8(&bundled).unwrap();
        assert!(text.contains("base64data="));
        assert!(!text.contains("#include"));
        assert!(!text.contains("default_path"));
        super::super::upstream_audio::validate_sfz(&bundled).unwrap();
        std::fs::remove_dir_all(fixture.0.join("samples")).unwrap();
        std::fs::write(fixture.0.join("packed.sfz"), &bundled).unwrap();
        let restored = super::bundle(&fixture.0.join("packed.sfz")).unwrap();
        super::super::upstream_audio::validate_sfz(&restored).unwrap();
    }
    #[test]
    fn sfz_rejects_missing_samples_unsupported_opcodes_and_partial_libraries() {
        let fixture = Fixture::new();
        assert!(fixture.bundle("<region> sample=missing.wav").is_err());
        let unsupported = fixture
            .bundle("<region> sample=samples/kick one.wav crest_unknown=1")
            .unwrap();
        assert!(super::super::upstream_audio::validate_sfz(&unsupported).is_err());
        std::fs::write(fixture.0.join("broken.wav"), b"invalid wave").unwrap();
        let partial = fixture
            .bundle("<region> sample=samples/kick one.wav\n<region> sample=broken.wav")
            .unwrap();
        assert!(super::super::upstream_audio::validate_sfz(&partial).is_err());
    }
    #[test]
    fn sfz_rejects_includes_and_samples_outside_the_selected_folder() {
        let fixture = Fixture::new();
        std::fs::create_dir_all(fixture.0.join("child")).unwrap();
        std::fs::write(
            fixture.0.join("child/instrument.sfz"),
            "#include \"../zones.sfz\"\n",
        )
        .unwrap();
        std::fs::write(fixture.0.join("zones.sfz"), "<region> sample=*sine").unwrap();
        assert!(super::bundle(&fixture.0.join("child/instrument.sfz")).is_err());
        std::fs::write(
            fixture.0.join("child/instrument.sfz"),
            "<region> sample=../samples/kick one.wav",
        )
        .unwrap();
        assert!(super::bundle(&fixture.0.join("child/instrument.sfz")).is_err());
    }
}
