use crate::synth::{
    AssetFileId, AssetKind, FileBrowserFolderId, FileBrowserListing, FileBrowserRow,
    FileBrowserRowKind, SampleAssetError,
};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Shared directory navigation and file filtering. Engine adapters own decoding
/// and import; this adapter never opens a platform dialog or mutates a Patch.
#[derive(Clone, Debug)]
pub struct FilesystemFileBrowser {
    library: PathBuf,
    locations: Vec<(&'static str, &'static str, PathBuf)>,
}

impl FilesystemFileBrowser {
    pub fn new(library: impl AsRef<Path>) -> Result<Self, SampleAssetError> {
        let library = library
            .as_ref()
            .canonicalize()
            .map_err(|_| SampleAssetError::Unavailable)?;
        if !library.is_dir() {
            return Err(SampleAssetError::Unavailable);
        }
        Ok(Self {
            library,
            locations: Vec::new(),
        })
    }

    /// Stores bytes already validated by the owning engine adapter. Never overwrites an asset.
    pub(crate) fn store_validated_file(
        &self,
        source: &Path,
        bytes: &[u8],
        extension: &str,
    ) -> Result<AssetFileId, SampleAssetError> {
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(SampleAssetError::InvalidRelativeId)?;
        if let Ok(relative) = source.strip_prefix(&self.library) {
            return AssetFileId::new(
                relative
                    .to_str()
                    .ok_or(SampleAssetError::InvalidRelativeId)?
                    .replace('\\', "/"),
            );
        }
        let import_root = self.library.join("Imported");
        std::fs::create_dir_all(&import_root).map_err(|_| SampleAssetError::Unavailable)?;
        self.resolve("Imported")?;
        std::fs::File::open(&self.library)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| SampleAssetError::Unavailable)?;
        let stem = source
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or(SampleAssetError::InvalidRelativeId)?;
        for suffix in 0..10_000 {
            let name = if suffix == 0 {
                name.to_owned()
            } else {
                format!("{stem}-{suffix}.{extension}")
            };
            let id = AssetFileId::new(format!("Imported/{name}"))?;
            let path = import_root.join(name);
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    if file.write_all(bytes).and_then(|_| file.sync_all()).is_err() {
                        let _ = std::fs::remove_file(path);
                        return Err(SampleAssetError::Unavailable);
                    }
                    std::fs::File::open(&import_root)
                        .and_then(|directory| directory.sync_all())
                        .map_err(|_| SampleAssetError::Unavailable)?;
                    return Ok(id);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if self
                        .resolve(id.as_str())
                        .and_then(|path| super::filesystem_sample_catalog::read_sample_path(&path))
                        .is_ok_and(|existing| existing == bytes)
                    {
                        return Ok(id);
                    }
                }
                Err(_) => return Err(SampleAssetError::Unavailable),
            }
        }
        Err(SampleAssetError::Unavailable)
    }

    pub fn with_user_locations(mut self) -> Self {
        if let Some(home) =
            std::env::var_os("HOME").and_then(|home| PathBuf::from(home).canonicalize().ok())
        {
            self.locations.push(("@home", "HOME", home));
        }
        #[cfg(target_os = "macos")]
        if let Ok(volumes) = PathBuf::from("/Volumes").canonicalize() {
            self.locations.push(("@volumes", "VOLUMES", volumes));
        }
        self
    }

    pub fn is_external(&self, id: &str) -> bool {
        self.locations.iter().any(|(prefix, _, _)| {
            id == *prefix
                || id
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
    }

    pub fn resolve(&self, id: &str) -> Result<PathBuf, SampleAssetError> {
        FileBrowserFolderId::new(id)?;
        let (root, relative) = self
            .locations
            .iter()
            .find_map(|(prefix, _, root)| {
                if id == *prefix {
                    Some((root, ""))
                } else {
                    id.strip_prefix(prefix)
                        .and_then(|rest| rest.strip_prefix('/'))
                        .map(|relative| (root, relative))
                }
            })
            .unwrap_or((&self.library, id));
        let path = root
            .join(relative)
            .canonicalize()
            .map_err(|_| SampleAssetError::Unavailable)?;
        if !path.starts_with(root) {
            return Err(SampleAssetError::PathEscape);
        }
        Ok(path)
    }

    pub fn list(
        &self,
        folder: &FileBrowserFolderId,
        kind: AssetKind,
    ) -> Result<FileBrowserListing, SampleAssetError> {
        let path = self.resolve(folder.as_str())?;
        let extension = match kind {
            AssetKind::Sample => "wav",
            AssetKind::SoundFont => "sf2",
            AssetKind::SysEx => "syx",
            AssetKind::Sfz => "sfz",
            AssetKind::NeuralModel => "nam",
            AssetKind::ImpulseResponse => "wav",
            AssetKind::Other => return Err(SampleAssetError::UnsupportedContainer),
        };
        let mut directories = Vec::new();
        let mut files = Vec::new();
        for entry in std::fs::read_dir(path).map_err(|_| SampleAssetError::Unavailable)? {
            let entry = entry.map_err(|_| SampleAssetError::Unavailable)?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            let id = if folder.as_str().is_empty() {
                name.clone()
            } else {
                format!("{}/{name}", folder.as_str())
            };
            // Broken or escaping symlinks are not selectable files. One such
            // entry must not make every sibling in Home inaccessible.
            let Ok(canonical) = self.resolve(&id) else {
                continue;
            };
            if canonical.is_dir() {
                directories.push(FileBrowserRow::new(
                    format!("folder:{id}"),
                    name,
                    FileBrowserRowKind::Folder(FileBrowserFolderId::new(id)?),
                    None,
                )?);
            } else if canonical.is_file()
                && entry
                    .path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
            {
                let bytes = canonical
                    .metadata()
                    .map_err(|_| SampleAssetError::Unavailable)?
                    .len();
                files.push(FileBrowserRow::new(
                    format!("file:{id}"),
                    name,
                    FileBrowserRowKind::File(AssetFileId::new(id)?),
                    Some(bytes),
                )?);
            }
        }
        let order = |row: &FileBrowserRow| (row.label().to_lowercase(), row.label().to_owned());
        directories.sort_by_key(&order);
        files.sort_by_key(&order);
        let mut rows = Vec::new();
        if let Some(parent) = folder.parent() {
            rows.push(FileBrowserRow::new(
                format!("parent:{}", folder.as_str()),
                "..",
                FileBrowserRowKind::Parent(parent),
                None,
            )?);
        }
        rows.extend(directories);
        rows.extend(files);
        if folder.as_str().is_empty() {
            for (id, label, _) in &self.locations {
                rows.push(FileBrowserRow::new(
                    format!("location:{id}"),
                    *label,
                    FileBrowserRowKind::Folder(FileBrowserFolderId::new(*id)?),
                    None,
                )?);
            }
        }
        rows.push(FileBrowserRow::new(
            format!("cancel:{}", folder.as_str()),
            "CANCEL — UNCHANGED",
            FileBrowserRowKind::Cancel,
            None,
        )?);
        FileBrowserListing::new(folder.clone(), rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_browser_lists_wav_and_soundfont_files_with_the_same_folder_navigation() {
        let root = std::env::temp_dir().join(format!("crest-file-browser-{}", std::process::id()));
        std::fs::create_dir_all(root.join("Library/Folder")).unwrap();
        std::fs::create_dir_all(root.join("Home/Music")).unwrap();
        std::fs::write(root.join("Library/Folder/Kick.WAV"), b"WAV fixture").unwrap();
        std::fs::write(root.join("Library/Folder/Bank.sf2"), b"SF2 fixture").unwrap();
        std::fs::write(root.join("Home/Music/External.wav"), b"WAV fixture").unwrap();
        let mut browser = FilesystemFileBrowser::new(root.join("Library")).unwrap();
        browser
            .locations
            .push(("@home", "HOME", root.join("Home").canonicalize().unwrap()));
        for (kind, expected) in [
            (AssetKind::Sample, "Kick.WAV"),
            (AssetKind::SoundFont, "Bank.sf2"),
        ] {
            let listing = browser
                .list(&FileBrowserFolderId::new("Folder").unwrap(), kind)
                .unwrap();
            assert_eq!(
                listing
                    .rows()
                    .iter()
                    .map(FileBrowserRow::label)
                    .collect::<Vec<_>>(),
                ["..", expected, "CANCEL — UNCHANGED"]
            );
            assert!(
                matches!(listing.rows()[0].kind(), FileBrowserRowKind::Parent(parent) if parent.as_str().is_empty())
            );
        }
        let home = browser
            .list(
                &FileBrowserFolderId::new("@home/Music").unwrap(),
                AssetKind::Sample,
            )
            .unwrap();
        assert!(
            matches!(home.rows()[1].kind(), FileBrowserRowKind::File(id) if id.as_str() == "@home/Music/External.wav" && id.is_external())
        );
        assert!(browser.resolve("@home/../Library/Folder/Kick.WAV").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
