use crate::adapter::wav_sample_decoder::WavSampleDecoder;
use crate::synth::{
    AssetFileId, FileBrowserFolderId, FileBrowserListing, FileBrowserRowKind,
    SampleAssetCatalogPort, SampleAssetError, SampleDecoderPort, MAX_SAMPLE_SOURCE_BYTES,
};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Library-relative Sample assets, using the shared in-app file browser.
#[derive(Clone, Debug)]
pub struct FilesystemSampleCatalog {
    root: PathBuf,
    browser: crate::adapter::filesystem_file_browser::FilesystemFileBrowser,
}

impl FilesystemSampleCatalog {
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Validates one explicitly selected file before publishing a stable copy.
    /// Existing library files retain their identity; imports never overwrite.
    pub fn import_file(&self, source: &Path) -> Result<AssetFileId, SampleAssetError> {
        let source = source
            .canonicalize()
            .map_err(|_| SampleAssetError::Unavailable)?;
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(SampleAssetError::InvalidRelativeId)?;
        let candidate = AssetFileId::new(name)?;
        let bytes = read_sample_path(&source)?;
        WavSampleDecoder.decode(&candidate, &bytes)?;
        self.browser.store_validated_file(&source, &bytes, "wav")
    }

    pub fn new(root: impl AsRef<Path>) -> Result<Self, SampleAssetError> {
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|_| SampleAssetError::Unavailable)?;
        if !root.is_dir() {
            return Err(SampleAssetError::Unavailable);
        }
        let browser = crate::adapter::filesystem_file_browser::FilesystemFileBrowser::new(&root)?;
        Ok(Self { root, browser })
    }

    pub fn with_user_locations(mut self) -> Self {
        self.browser = self.browser.with_user_locations();
        self
    }

    pub fn import_browser_asset(
        &self,
        asset: &AssetFileId,
    ) -> Result<AssetFileId, SampleAssetError> {
        self.import_file(&self.browser.resolve(asset.as_str())?)
    }

    fn resolve(&self, relative: &str) -> Result<PathBuf, SampleAssetError> {
        self.browser.resolve(relative)
    }
}

impl SampleAssetCatalogPort for FilesystemSampleCatalog {
    fn list(&self, folder: &FileBrowserFolderId) -> Result<FileBrowserListing, SampleAssetError> {
        let listing = self.browser.list(folder, crate::synth::AssetKind::Sample)?;
        let rows = listing
            .rows()
            .iter()
            .cloned()
            .map(|row| {
                let FileBrowserRowKind::File(id) = row.kind() else {
                    return Ok(row);
                };
                let metadata = self
                    .read(id)
                    .and_then(|bytes| WavSampleDecoder::metadata(id, &bytes));
                row.with_metadata(metadata)
            })
            .collect::<Result<Vec<_>, SampleAssetError>>()?;
        FileBrowserListing::new(folder.clone(), rows)
    }

    fn read(&self, asset: &AssetFileId) -> Result<Vec<u8>, SampleAssetError> {
        let path = self.resolve(asset.as_str())?;
        read_sample_path(&path)
    }
}

pub(crate) fn read_sample_path(path: &Path) -> Result<Vec<u8>, SampleAssetError> {
    if !path.is_file() {
        return Err(SampleAssetError::Unavailable);
    }
    let length = path
        .metadata()
        .map_err(|_| SampleAssetError::Unavailable)?
        .len();
    if length == 0 {
        #[cfg(target_os = "macos")]
        if is_dropbox_placeholder(path)? {
            return Err(SampleAssetError::DownloadRequired);
        }
        return Err(SampleAssetError::EmptyFile);
    }
    if length > MAX_SAMPLE_SOURCE_BYTES {
        return Err(SampleAssetError::SourceTooLarge);
    }
    let capacity = usize::try_from(length).map_err(|_| SampleAssetError::SourceTooLarge)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(capacity)
        .map_err(|_| SampleAssetError::AllocationFailed)?;
    File::open(path)
        .map_err(|_| SampleAssetError::Unavailable)?
        .take(MAX_SAMPLE_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| SampleAssetError::Unavailable)?;
    if u64::try_from(bytes.len()).map_err(|_| SampleAssetError::SourceTooLarge)?
        > MAX_SAMPLE_SOURCE_BYTES
    {
        return Err(SampleAssetError::SourceTooLarge);
    }
    Ok(bytes)
}

/// Legacy Dropbox online-only files can read successfully as zero bytes. The
/// provider marker distinguishes them from empty local files; downloaded WAVs
/// bypass this check even if Dropbox retains the attribute.
#[cfg(target_os = "macos")]
fn is_dropbox_placeholder(path: &Path) -> Result<bool, SampleAssetError> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| SampleAssetError::InvalidRelativeId)?;
    // SAFETY: Names are NUL-terminated and live for the call. A null buffer and
    // zero size request only the attribute length; no audio or attribute value
    // is read. Filesystem access remains outside the audio callback.
    let length = unsafe {
        libc::getxattr(
            path.as_ptr(),
            c"com.dropbox.placeholder".as_ptr(),
            std::ptr::null_mut(),
            0,
            0,
            0,
        )
    };
    if length >= 0 {
        return Ok(true);
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::ENOATTR | libc::ENOTSUP) => Ok(false),
        _ => Err(SampleAssetError::Unavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::FilesystemSampleCatalog;
    use crate::synth::{
        AssetFileId, FileBrowserFolderId, FileBrowserRowKind, SampleAssetCatalogPort,
        SampleAssetError,
    };
    use std::path::PathBuf;

    struct FixtureRoot(PathBuf);

    #[cfg(target_os = "macos")]
    #[test]
    fn dropbox_placeholder_reports_download_required_and_import_retries_after_download() {
        use std::os::unix::ffi::OsStrExt;

        let fixture = FixtureRoot::new();
        let library = fixture.0.join("Library");
        std::fs::create_dir(&library).unwrap();
        let catalog = FilesystemSampleCatalog::new(&library).unwrap();
        let source = fixture.0.join("Cloud.wav");
        std::fs::write(&source, []).unwrap();
        let path = std::ffi::CString::new(source.as_os_str().as_bytes()).unwrap();
        let marker = b"placeholder fixture";
        // SAFETY: Both names are NUL-terminated, and marker is readable for its
        // full length. Only this test's temporary file is modified.
        assert_eq!(
            unsafe {
                libc::setxattr(
                    path.as_ptr(),
                    c"com.dropbox.placeholder".as_ptr(),
                    marker.as_ptr().cast(),
                    marker.len(),
                    0,
                    0,
                )
            },
            0
        );

        let error = catalog.import_file(&source).unwrap_err();
        assert_eq!(error, SampleAssetError::DownloadRequired);
        assert_eq!(error.to_string(), "file is not downloaded; choose Make available offline in your cloud storage app, then select it again");
        assert!(!library.join("Imported").exists());
        let source_catalog = FilesystemSampleCatalog::new(&fixture.0).unwrap();
        let listing = source_catalog
            .list(&FileBrowserFolderId::default())
            .unwrap();
        let row = listing
            .rows()
            .iter()
            .find(|row| row.label() == "Cloud.wav")
            .unwrap();
        assert_eq!(row.metadata(), Some(&Err(error)));
        assert_eq!(
            source_catalog.read(&AssetFileId::new("Cloud.wav").unwrap()),
            Err(error)
        );

        // A downloaded file may retain provider metadata. Actual audio must win.
        let wave = mono_pcm16_wave();
        std::fs::write(&source, &wave).unwrap();
        let imported = catalog.import_file(&source).unwrap();
        assert_eq!(imported.as_str(), "Imported/Cloud.wav");
        assert_eq!(catalog.read(&imported).unwrap(), wave);
        let listing = source_catalog
            .list(&FileBrowserFolderId::default())
            .unwrap();
        assert!(listing
            .rows()
            .iter()
            .find(|row| row.label() == "Cloud.wav")
            .unwrap()
            .metadata()
            .unwrap()
            .is_ok());

        let empty = fixture.0.join("Empty.wav");
        std::fs::write(&empty, []).unwrap();
        assert_eq!(
            catalog.import_file(&empty),
            Err(SampleAssetError::EmptyFile)
        );
        assert!(!library.join("Imported/Empty.wav").exists());
    }

    fn mono_pcm16_wave() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&38_u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&96_000_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&2_u32.to_le_bytes());
        bytes.extend_from_slice(&0_i16.to_le_bytes());
        bytes
    }

    impl FixtureRoot {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "crest-sample-catalog-{}-{}",
                std::process::id(),
                std::thread::current().name().unwrap_or("test")
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("Drums")).unwrap();
            std::fs::write(root.join("zeta.wav"), b"wave-z").unwrap();
            std::fs::write(root.join("Alpha.WAV"), mono_pcm16_wave()).unwrap();
            std::fs::write(root.join("ignore.mp3"), b"mp3").unwrap();
            std::fs::write(root.join("Drums/kick.wav"), b"kick").unwrap();
            Self(root)
        }
    }
    impl Drop for FixtureRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn import_validates_before_writing_and_never_overwrites_another_asset() {
        let root = FixtureRoot::new();
        let catalog = FilesystemSampleCatalog::new(&root.0).unwrap();
        let external = root.0.with_extension("external");
        std::fs::create_dir_all(&external).unwrap();
        let path = external.join("Picked.wav");
        std::fs::write(&path, b"not a WAV").unwrap();
        assert_eq!(
            catalog.import_file(&path),
            Err(SampleAssetError::UnsupportedContainer)
        );
        assert!(!root.0.join("Imported").exists());
        std::fs::write(&path, mono_pcm16_wave()).unwrap();
        let first = catalog.import_file(&path).unwrap();
        assert_eq!(first.as_str(), "Imported/Picked.wav");
        assert_eq!(
            catalog.import_file(&path).unwrap(),
            first,
            "same content reuses its stable copy"
        );
        let mut changed = mono_pcm16_wave();
        *changed.last_mut().unwrap() = 1;
        std::fs::write(&path, &changed).unwrap();
        let second = catalog.import_file(&path).unwrap();
        assert_ne!(first, second);
        assert_eq!(catalog.read(&first).unwrap(), mono_pcm16_wave());
        assert_eq!(catalog.read(&second).unwrap(), changed);
        assert_eq!(
            catalog.import_file(&root.0.join(first.as_str())).unwrap(),
            first
        );
        std::fs::remove_dir_all(external).unwrap();
        assert!(
            catalog.read(&first).is_ok(),
            "the original external file is no longer required"
        );
    }

    #[test]
    fn listing_is_stable_folder_first_and_ids_are_library_relative() {
        let root = FixtureRoot::new();
        let catalog = FilesystemSampleCatalog::new(&root.0).unwrap();
        let listing = catalog.list(&FileBrowserFolderId::default()).unwrap();
        let labels = listing
            .rows()
            .iter()
            .map(|row| row.label())
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            ["Drums", "Alpha.WAV", "zeta.wav", "CANCEL — UNCHANGED"]
        );
        assert!(matches!(
            listing.rows()[0].kind(),
            FileBrowserRowKind::Folder(folder) if folder.as_str() == "Drums"
        ));
        let alpha = &listing.rows()[1];
        let metadata = alpha
            .metadata()
            .expect("eligible files carry a typed metadata state")
            .as_ref()
            .expect("the valid WAV metadata is admitted");
        assert_eq!(metadata.sample_rate(), 48_000);
        assert_eq!(metadata.channels(), 1);
        assert_eq!(metadata.bits_per_sample(), 16);
        assert_eq!(metadata.frames(), 1);
        assert!(matches!(
            listing.rows()[2].metadata(),
            Some(Err(SampleAssetError::UnsupportedContainer))
        ));
        let bytes = catalog
            .read(&AssetFileId::new("Drums/kick.wav").unwrap())
            .unwrap();
        assert_eq!(bytes, b"kick");
    }

    #[cfg(unix)]
    #[test]
    fn traversal_and_symlink_escape_are_typed_and_never_read() {
        use std::os::unix::fs::symlink;
        let root = FixtureRoot::new();
        let outside = root.0.parent().unwrap().join("outside.wav");
        std::fs::write(&outside, b"outside").unwrap();
        symlink(&outside, root.0.join("escape.wav")).unwrap();
        let catalog = FilesystemSampleCatalog::new(&root.0).unwrap();
        assert_eq!(
            catalog.read(&AssetFileId::new("escape.wav").unwrap()),
            Err(SampleAssetError::PathEscape)
        );
        assert_eq!(
            AssetFileId::new("../outside.wav"),
            Err(SampleAssetError::InvalidRelativeId)
        );
        let _ = std::fs::remove_file(outside);
    }
}
