//! Shared, machine-local browser roots. Sessions never contain these paths.
use crate::synth::{AssetKind, FileBrowserFolderId};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

const FILE: &str = "browser-starts.json";
const MAX_BYTES: u64 = 64 * 1024;
static NEXT_WRITE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserPreferences {
    pub sample_folder: Option<String>,
    pub soundfont_folder: Option<String>,
}

impl BrowserPreferences {
    pub fn validate(&self) -> Result<(), String> {
        for path in [&self.sample_folder, &self.soundfont_folder]
            .into_iter()
            .flatten()
        {
            if !Path::new(path).is_absolute() || path.chars().any(char::is_control) {
                return Err(format!(
                    "Browser starting directory must be an absolute path: {path}"
                ));
            }
        }
        Ok(())
    }

    pub fn folders(&self) -> Result<[(AssetKind, FileBrowserFolderId); 2], String> {
        self.validate()?;
        let folder = |path: &Option<String>| {
            FileBrowserFolderId::new(
                path.as_ref()
                    .map(|path| {
                        format!("@root/{}", path.trim_matches('/'))
                            .trim_end_matches('/')
                            .to_owned()
                    })
                    .unwrap_or_default(),
            )
            .map_err(|error| error.to_string())
        };
        Ok([
            (AssetKind::Sample, folder(&self.sample_folder)?),
            (AssetKind::SoundFont, folder(&self.soundfont_folder)?),
        ])
    }
}

pub fn load(directory: &Path) -> Result<Option<BrowserPreferences>, String> {
    let path = directory.join(FILE);
    let file = match fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Cannot read {}: {error}", path.display())),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Browser preferences exceed 64 KiB".into());
    }
    let value: BrowserPreferences = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Invalid {}: {error}", path.display()))?;
    value.validate()?;
    Ok(Some(value))
}

/// Atomic replacement, or create-only import of legacy host settings. A
/// concurrent creator wins over migration; it is never overwritten.
pub fn save(directory: &Path, value: &BrowserPreferences, replace: bool) -> Result<(), String> {
    value.validate()?;
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Browser preferences exceed 64 KiB".into());
    }
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let path = directory.join(FILE);
    let temporary = directory.join(format!(
        ".{FILE}.{}-{}",
        std::process::id(),
        NEXT_WRITE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        if replace {
            fs::rename(&temporary, &path)
        } else {
            match fs::hard_link(&temporary, &path) {
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                result => result,
            }
        }
    })();
    let _ = fs::remove_file(temporary);
    result.map_err(|error| format!("Cannot save {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_store_preserves_existing_roots_and_rejects_malformed_data() {
        let directory = std::env::temp_dir().join(format!("crest-browser-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let _ = fs::remove_file(directory.join(FILE));
        assert_eq!(load(&directory).unwrap(), None);
        let first = BrowserPreferences {
            sample_folder: Some(directory.display().to_string()),
            soundfont_folder: None,
        };
        save(&directory, &first, false).unwrap();
        save(&directory, &BrowserPreferences::default(), false).unwrap();
        assert_eq!(load(&directory).unwrap(), Some(first));
        save(&directory, &BrowserPreferences::default(), true).unwrap();
        assert_eq!(
            load(&directory).unwrap(),
            Some(BrowserPreferences::default())
        );
        fs::write(directory.join(FILE), b"{broken").unwrap();
        assert!(load(&directory).is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
