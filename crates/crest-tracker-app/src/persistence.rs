//! Versioned composite documents and crash-safe local writes.
use crest_tracker::composition::{InstrumentId, Song};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};

pub(crate) const MAX_DOCUMENT_BYTES: u64 = 64 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub format: String,
    pub version: u32,
    pub song: Song,
    pub synth: Value,
    pub installed_patches: BTreeMap<InstrumentId, u32>,
    #[serde(default)]
    pub pinned_sounds: Vec<crate::library::PinnedSound>,
    #[serde(default)]
    pub resources: Vec<crate::synth::resources::Resource>,
}
impl Document {
    pub fn new(song: Song, synth: Value, installed_patches: BTreeMap<InstrumentId, u32>) -> Self {
        Self {
            format: "crest-tracker-song".into(),
            version: 1,
            song,
            synth,
            installed_patches,
            pinned_sounds: Vec::new(),
            resources: Vec::new(),
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.format != "crest-tracker-song" || self.version != 1 {
            return Err("Unsupported Song format or version".into());
        }
        crate::synth::resources::validate(&self.resources)?;
        for pinned in &self.pinned_sounds {
            pinned.validate()?;
        }
        self.song.validate().map_err(|v| v.to_string())?;
        if !self.synth.is_object() {
            return Err("Missing Synth-native sound document".into());
        }
        for instrument in self.song.instruments.values() {
            if let Some(reference) = &instrument.resource {
                if !self
                    .pinned_sounds
                    .iter()
                    .any(|p| p.resource == reference.resource && p.revision == reference.revision)
                {
                    return Err("A pinned sound revision is missing from the Song".into());
                }
            }
        }
        for id in self.song.instruments.keys() {
            if self
                .installed_patches
                .get(id)
                .is_none_or(|patch| *patch == 0)
            {
                return Err(format!(
                    "Instrument {} has no installed Patch mapping",
                    id.get()
                ));
            }
        }
        Ok(())
    }
    pub fn read(path: &Path) -> Result<Self, String> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|v| v.to_string())?
            .take(MAX_DOCUMENT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|v| v.to_string())?;
        if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
            return Err("Song exceeds the 64 MiB document admission budget".into());
        }
        let value: Self = serde_json::from_slice(&bytes).map_err(|v| v.to_string())?;
        value.validate()?;
        Ok(value)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SaveMode {
    Normal,
    Collect,
    UpdateCollection,
}

pub struct SaveResult {
    pub path: PathBuf,
    pub document: Document,
    pub result: Result<(), String>,
}
pub struct SaveWorker {
    requests: Option<SyncSender<(PathBuf, Document, SaveMode)>>,
    results: Receiver<SaveResult>,
    thread: Option<std::thread::JoinHandle<()>>,
    disconnected_reported: Cell<bool>,
}
impl SaveWorker {
    pub fn new() -> Self {
        let (requests, incoming) = mpsc::sync_channel::<(PathBuf, Document, SaveMode)>(1);
        let (results, outgoing) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            while let Ok((path, mut document, mode)) = incoming.recv() {
                let result = (|| {
                    document.validate()?;
                    let root = path.parent().ok_or("Save path has no parent")?;
                    if mode == SaveMode::Collect {
                        fs::create_dir(root)
                            .map_err(|e| format!("Cannot create collection folder: {e}"))?;
                        File::open(root.parent().unwrap_or(Path::new(".")))
                            .and_then(|directory| directory.sync_all())
                            .map_err(|e| e.to_string())?;
                    }
                    crate::synth::resources::prepare_save(
                        &mut document,
                        (mode != SaveMode::Normal).then_some(root),
                    )?;
                    let bytes = serde_json::to_vec_pretty(&document).map_err(|v| v.to_string())?;
                    atomic_write(&path, &bytes)
                })();
                if results
                    .send(SaveResult {
                        path,
                        document,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            requests: Some(requests),
            results: outgoing,
            thread: Some(thread),
            disconnected_reported: Cell::new(false),
        }
    }
    pub fn submit(&self, path: PathBuf, document: Document, mode: SaveMode) -> Result<(), String> {
        self.requests
            .as_ref()
            .ok_or("save worker closed")?
            .try_send((path, document, mode))
            .map_err(|error| match error {
                TrySendError::Full(_) => "Save already in progress".into(),
                TrySendError::Disconnected(_) => {
                    "Save worker disconnected before accepting the document".into()
                }
            })
    }
    pub fn poll(&self) -> Option<Result<SaveResult, String>> {
        match self.results.try_recv() {
            Ok(result) => Some(Ok(result)),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) if !self.disconnected_reported.replace(true) => {
                Some(Err("Save worker disconnected before completion".into()))
            }
            Err(TryRecvError::Disconnected) => None,
        }
    }
}
impl Drop for SaveWorker {
    fn drop(&mut self) {
        self.requests.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err("Song exceeds the document admission budget".into());
    }
    let parent = path
        .parent()
        .filter(|v| !v.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|v| v.to_string())?
        .as_nanos();
    let temporary = parent.join(format!(".crest-save-{}-{stamp}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|v| v.to_string())?;
    let result = (|| -> std::io::Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|v| v.to_string())
}

/// Candidate I/O stays off the window/control thread. A worker result never publishes itself.
pub enum Loaded {
    Song(Document),
    Imported(Document),
    Resources { document: Document, error: String },
    Synth(Value),
}
pub struct LoadWorker {
    result: Receiver<Result<Loaded, String>>,
    completed: Cell<bool>,
}
impl LoadWorker {
    pub fn midi(path: PathBuf) -> Self {
        let (send, result) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let _ = send.send(crate::synth::corridors::midi_document(&path).map(Loaded::Imported));
        });
        Self {
            result,
            completed: Cell::new(false),
        }
    }
    pub fn corridors() -> Self {
        let (send, result) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let _ = send.send(crate::synth::corridors::document().map(Loaded::Imported));
        });
        Self {
            result,
            completed: Cell::new(false),
        }
    }
    pub fn prepared(document: Document, path: PathBuf) -> Self {
        let (send, result) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut candidate = document.clone();
            let loaded = match crate::synth::resources::prepare_open(&mut candidate, &path) {
                Ok(()) => Loaded::Song(candidate),
                Err(error) => Loaded::Resources { document, error },
            };
            let _ = send.send(Ok(loaded));
        });
        Self {
            result,
            completed: Cell::new(false),
        }
    }
    pub fn start(path: PathBuf, synth: bool) -> Self {
        let (send, result) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let loaded = if synth {
                read_json(&path).map(Loaded::Synth)
            } else {
                Document::read(&path).map(|document| {
                    let mut candidate = document.clone();
                    match crate::synth::resources::prepare_open(&mut candidate, &path) {
                        Ok(()) => Loaded::Song(candidate),
                        Err(error) => Loaded::Resources { document, error },
                    }
                })
            };
            let _ = send.send(loaded);
        });
        Self {
            result,
            completed: Cell::new(false),
        }
    }
    pub fn poll(&self) -> Option<Result<Loaded, String>> {
        if self.completed.get() {
            return None;
        }
        match self.result.try_recv() {
            Ok(result) => {
                self.completed.set(true);
                Some(result)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.completed.set(true);
                Some(Err("Load worker disconnected before completion".into()))
            }
        }
    }
}
fn read_json(path: &Path) -> Result<Value, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|v| v.to_string())?
        .take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|v| v.to_string())?;
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err("Sound document exceeds admission budget".into());
    }
    serde_json::from_slice(&bytes).map_err(|v| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disconnected_save_and_load_workers_report_failure_once() {
        let (send, results) = mpsc::channel();
        drop(send);
        let saver = SaveWorker {
            requests: None,
            results,
            thread: None,
            disconnected_reported: Cell::new(false),
        };
        assert!(matches!(saver.poll(), Some(Err(error)) if error.contains("disconnected")));
        assert!(saver.poll().is_none());

        let (send, result) = mpsc::channel();
        let loader = LoadWorker {
            result,
            completed: Cell::new(false),
        };
        assert!(loader.poll().is_none());
        drop(send);
        assert!(matches!(loader.poll(), Some(Err(error)) if error.contains("disconnected")));
        assert!(loader.poll().is_none());
    }

    #[test]
    fn successful_one_shot_load_does_not_report_expected_sender_exit_as_failure() {
        let (send, result) = mpsc::channel();
        assert!(send
            .send(Ok(Loaded::Synth(serde_json::json!({"native":true}))))
            .is_ok());
        drop(send);
        let loader = LoadWorker {
            result,
            completed: Cell::new(false),
        };
        assert!(matches!(loader.poll(), Some(Ok(Loaded::Synth(value))) if value["native"] == true));
        assert!(loader.poll().is_none());
    }
    #[test]
    fn failed_write_preserves_previous_file_and_success_replaces_atomically() {
        let root = std::env::temp_dir().join(format!("crest-save-test-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("song.crest");
        atomic_write(&path, b"previous").unwrap();
        assert!(atomic_write(&root.join("missing/song.crest"), b"invalid").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"previous");
        atomic_write(&path, b"next").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"next");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
