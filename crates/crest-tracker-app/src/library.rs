//! Immutable host-owned library revisions. Sound contents remain Synth-native JSON.
pub mod chords;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinnedSound {
    pub resource: String,
    pub revision: String,
    pub definition: Value,
}
impl PinnedSound {
    pub fn new(resource: String, definition: Value) -> Result<Self, String> {
        let pinned = Self {
            resource,
            revision: digest(&definition)?,
            definition,
        };
        pinned.validate()?;
        Ok(pinned)
    }
    /// Creates a new immutable revision of this resource without publishing it.
    pub fn with_definition(&self, definition: Value) -> Result<Self, String> {
        let next = Self {
            resource: self.resource.clone(),
            revision: digest(&definition)?,
            definition,
        };
        next.validate()?;
        Ok(next)
    }
    pub fn validate(&self) -> Result<(), String> {
        uuid::Uuid::parse_str(&self.resource).map_err(|_| "Invalid sound resource identity")?;
        if self.revision != digest(&self.definition)? {
            return Err("Sound revision digest mismatch".into());
        }
        if !self.definition.is_object() {
            return Err("Missing sound definition".into());
        }
        Ok(())
    }
}
fn digest(value: &Value) -> Result<String, String> {
    // serde_json's default object map orders keys; array order is musical state.
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(|v| v.to_string())?)
    ))
}
pub fn data_directory() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    #[cfg(target_os = "macos")]
    let root = home.join("Library/Application Support");
    #[cfg(not(target_os = "macos"))]
    let root = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    root.join("crest-tracker")
}
pub fn publish(
    root: &Path,
    resource: Option<&str>,
    definition: Value,
) -> Result<PinnedSound, String> {
    let resource = resource
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let pinned = PinnedSound::new(resource, definition)?;
    let folder = root.join(&pinned.resource);
    fs::create_dir_all(&folder).map_err(|v| v.to_string())?;
    let path = folder.join(format!("{}.crest-sound", pinned.revision));
    let bytes = serde_json::to_vec_pretty(&pinned).map_err(|v| v.to_string())?;
    // A fully flushed temporary inode becomes visible with one no-overwrite link.
    let temporary = folder.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|v| v.to_string())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|v| v.to_string())?;
        drop(file);
        match fs::hard_link(&temporary, &path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let old = read(&path)?;
                if old.resource != pinned.resource
                    || old.revision != pinned.revision
                    || old.definition != pinned.definition
                {
                    return Err("Existing sound revision differs".into());
                }
            }
            Err(error) => return Err(error.to_string()),
        }
        fs::File::open(&folder)
            .and_then(|f| f.sync_all())
            .map_err(|v| v.to_string())?;
        Ok(())
    })();
    let _ = fs::remove_file(&temporary);
    result?;
    Ok(pinned)
}
pub fn read(path: &Path) -> Result<PinnedSound, String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|v| v.to_string())?
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|v| v.to_string())?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("Sound definition exceeds library admission budget".into());
    }
    let pinned: PinnedSound = serde_json::from_slice(&bytes).map_err(|v| v.to_string())?;
    pinned.validate()?;
    Ok(pinned)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn publication_is_immutable_and_content_checked() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let first = publish(
            &root,
            None,
            serde_json::json!({"version":1,"name":"A","data":[1,2]}),
        )
        .unwrap();
        let next = publish(
            &root,
            Some(&first.resource),
            serde_json::json!({"version":1,"name":"B","data":[1,2]}),
        )
        .unwrap();
        assert_ne!(first.revision, next.revision);
        assert_eq!(first.resource, next.resource);
        let path = root
            .join(&first.resource)
            .join(format!("{}.crest-sound", first.revision));
        assert_eq!(read(&path).unwrap().definition["name"], "A");
        let mut corrupt = first;
        corrupt.definition["name"] = Value::String("changed".into());
        assert!(corrupt.validate().is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
