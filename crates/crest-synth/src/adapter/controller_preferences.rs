//! Controller preferences are process-local orchestration and worker-side I/O.
//! Complete mappings cross the reducer boundary, so stale saves cannot acknowledge
//! a newer assignment. No filesystem work runs on the UI or audio callback.
use crate::adapter::filesystem_midi_input_preference::per_user_midi_config_directory;
use crate::control::{
    ControllerBindings, ControllerEvent, ControllerFailure, ControllerPreferenceStatus,
};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::thread::{self, JoinHandle};

const FILENAME: &str = "controller-buttons.json";
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn load(path: &Path) -> Result<Option<ControllerBindings>, ControllerFailure> {
    match fs::read(path) {
        Ok(bytes) => {
            let bindings: ControllerBindings =
                serde_json::from_slice(&bytes).map_err(|_| ControllerFailure::PreferenceDecode)?;
            if !bindings.is_legacy_default() {
                return Ok(Some(bindings));
            }
            // Repair only the former generated defaults. Customized layouts
            // remain byte-for-byte unchanged; retain an exact rollback copy.
            let backup = path.with_file_name("controller-buttons.before-face-layout.json");
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&backup)
            {
                Ok(mut file) => file.write_all(&bytes).and_then(|()| file.sync_all()),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    fs::read(&backup).and_then(|saved| {
                        if saved == bytes {
                            Ok(())
                        } else {
                            Err(std::io::Error::new(
                                std::io::ErrorKind::AlreadyExists,
                                "controller backup contains different preferences",
                            ))
                        }
                    })
                }
                Err(error) => Err(error),
            }
            .map_err(|_| ControllerFailure::PreferenceWrite)?;
            let corrected = ControllerBindings::default();
            save(path, &corrected)?;
            Ok(Some(corrected))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(ControllerFailure::PreferenceRead),
    }
}

fn save(path: &Path, bindings: &ControllerBindings) -> Result<(), ControllerFailure> {
    let operation = || -> std::io::Result<()> {
        let parent = path.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing parent directory")
        })?;
        fs::create_dir_all(parent)?;
        let bytes = serde_json::to_vec_pretty(bindings)?;
        // Never share a temporary file between workers or processes. create_new
        // also protects against a stale file from a previous process with this PID.
        let (temporary, mut file) = loop {
            let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let temporary = parent.join(format!(
                ".controller-buttons-{}-{sequence}.tmp",
                std::process::id()
            ));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => break (temporary, file),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        };
        let result = (|| {
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, path)?;
            // Persist the rename where directory syncing is supported. Windows
            // std::fs::rename uses replacement semantics but cannot open a folder
            // as a normal File, so its completed rename is the final operation.
            #[cfg(unix)]
            fs::File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    };
    operation().map_err(|_| ControllerFailure::PreferenceWrite)
}

pub(crate) struct ControllerPreferenceWorker {
    commands: Option<SyncSender<ControllerBindings>>,
    results: Option<Receiver<ControllerEvent>>,
    thread: Option<JoinHandle<()>>,
    pending_failure: Option<ControllerEvent>,
    loaded: bool,
    in_flight: Option<ControllerBindings>,
    desired: Option<ControllerBindings>,
}

impl ControllerPreferenceWorker {
    pub(crate) fn new() -> Self {
        Self::with_directory(per_user_midi_config_directory())
    }

    pub(crate) fn with_directory(directory: Option<PathBuf>) -> Self {
        let (commands, receiver) = mpsc::sync_channel::<ControllerBindings>(1);
        // The protocol admits one save at a time. Results therefore contain at
        // most the initial load and one completion, without worker-side blocking.
        let (sender, results) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("controller-preferences".to_owned())
            .spawn(move || {
                let path = directory.map(|directory| directory.join(FILENAME));
                let result = path
                    .as_deref()
                    .ok_or(ControllerFailure::PreferenceRead)
                    .and_then(load);
                let _ = sender.send(ControllerEvent::PreferencesLoaded { result });
                while let Ok(bindings) = receiver.recv() {
                    let result = path
                        .as_deref()
                        .ok_or(ControllerFailure::PreferenceWrite)
                        .and_then(|path| save(path, &bindings));
                    let _ = sender.send(ControllerEvent::PreferencesSaved { bindings, result });
                }
            });
        match thread {
            Ok(thread) => Self {
                commands: Some(commands),
                results: Some(results),
                thread: Some(thread),
                pending_failure: None,
                loaded: false,
                in_flight: None,
                desired: None,
            },
            Err(_) => Self {
                commands: None,
                results: None,
                thread: None,
                pending_failure: Some(ControllerEvent::PreferencesLoaded {
                    result: Err(ControllerFailure::WorkerUnavailable),
                }),
                loaded: false,
                in_flight: None,
                desired: None,
            },
        }
    }

    pub(crate) fn poll(&mut self) -> Option<ControllerEvent> {
        if let Some(failure) = self.pending_failure.take() {
            return Some(failure);
        }
        let result = self.results.as_ref()?.try_recv();
        match result {
            Ok(event) => {
                match &event {
                    ControllerEvent::PreferencesLoaded { .. } => self.loaded = true,
                    ControllerEvent::PreferencesSaved { .. } => self.in_flight = None,
                    _ => unreachable!("worker only emits preference completions"),
                }
                Some(event)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.commands = None;
                self.results = None;
                if let Some(bindings) = self.in_flight.take() {
                    Some(ControllerEvent::PreferencesSaved {
                        bindings,
                        result: Err(ControllerFailure::WorkerUnavailable),
                    })
                } else if !self.loaded {
                    self.loaded = true;
                    Some(ControllerEvent::PreferencesLoaded {
                        result: Err(ControllerFailure::WorkerUnavailable),
                    })
                } else {
                    None
                }
            }
        }
    }

    pub(crate) fn observe(
        &mut self,
        status: ControllerPreferenceStatus,
        bindings: &ControllerBindings,
    ) {
        self.desired = (status == ControllerPreferenceStatus::Saving).then(|| bindings.clone());
        if self.in_flight.is_some() || self.pending_failure.is_some() {
            return;
        }
        let Some(desired) = self.desired.clone() else {
            return;
        };
        let submitted = self
            .commands
            .as_ref()
            .is_some_and(|commands| commands.try_send(desired.clone()).is_ok());
        if submitted {
            self.in_flight = Some(desired);
        } else {
            self.pending_failure = Some(ControllerEvent::PreferencesSaved {
                bindings: desired,
                result: Err(ControllerFailure::WorkerUnavailable),
            });
        }
    }
}

impl Drop for ControllerPreferenceWorker {
    fn drop(&mut self) {
        // The reducer may have accepted a newer binding while an older write was
        // in flight. Flush that complete mapping before closing the command queue.
        // Shutdown may wait; neither normal UI ticks nor the callback ever wait.
        self.results.take();
        if let Some(commands) = self.commands.take() {
            if let Some(desired) = self.desired.take() {
                if self.in_flight.as_ref() != Some(&desired) {
                    let _ = commands.send(desired);
                }
            }
            drop(commands);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory(case: &str) -> PathBuf {
        let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "crest-controller-{}-{sequence}-{case}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn changed_bindings() -> ControllerBindings {
        let mut value = serde_json::to_value(ControllerBindings::default()).unwrap();
        value["buttons"][4] = serde_json::json!("west");
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn round_trip_replaces_complete_mapping_without_temporary_files() {
        let directory = directory("round-trip");
        let path = directory.join(FILENAME);
        assert_eq!(load(&path), Ok(None));
        save(&path, &ControllerBindings::default()).unwrap();
        save(&path, &changed_bindings()).unwrap();
        assert_eq!(load(&path), Ok(Some(changed_bindings())));
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn former_generated_defaults_are_backed_up_and_repaired_but_custom_layouts_survive() {
        use crate::control::{ControllerButton, ControllerRole};
        let directory = directory("face-layout");
        let path = directory.join(FILENAME);
        let legacy = br#"{"version":1,"buttons":["dPadUp","dPadDown","dPadLeft","dPadRight","south","leftShoulder","start","leftTrigger","rightTrigger","north"]}"#;
        fs::write(&path, legacy).unwrap();
        let repaired = load(&path).unwrap().unwrap();
        assert_eq!(
            repaired.button(ControllerRole::Edit),
            ControllerButton::South
        );
        assert_eq!(
            repaired.button(ControllerRole::Shift),
            ControllerButton::East
        );
        assert_eq!(
            repaired.button(ControllerRole::Preview),
            ControllerButton::North
        );
        assert_eq!(
            repaired.button(ControllerRole::Settings),
            ControllerButton::Start
        );
        assert_eq!(
            fs::read(directory.join("controller-buttons.before-face-layout.json")).unwrap(),
            legacy
        );
        assert_eq!(load(&path), Ok(Some(repaired)));
        let custom = String::from_utf8(legacy.to_vec())
            .unwrap()
            .replace("south", "west");
        fs::write(&path, &custom).unwrap();
        assert!(!load(&path).unwrap().unwrap().is_legacy_default());
        assert_eq!(fs::read(&path).unwrap(), custom.as_bytes());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn corrupt_unknown_version_and_duplicate_bindings_are_rejected_unchanged() {
        let directory = directory("invalid");
        let path = directory.join(FILENAME);
        let mut duplicate = serde_json::to_value(ControllerBindings::default()).unwrap();
        duplicate["buttons"][1] = duplicate["buttons"][0].clone();
        let mut version = serde_json::to_value(ControllerBindings::default()).unwrap();
        version["version"] = serde_json::json!(2);
        for bytes in [
            b"not json".to_vec(),
            serde_json::to_vec(&duplicate).unwrap(),
            serde_json::to_vec(&version).unwrap(),
        ] {
            fs::write(&path, &bytes).unwrap();
            assert_eq!(load(&path), Err(ControllerFailure::PreferenceDecode));
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn shutdown_flushes_latest_accepted_mapping_after_in_flight_write() {
        let directory = directory("shutdown");
        let mut worker = ControllerPreferenceWorker::with_directory(Some(directory.clone()));
        worker.observe(
            ControllerPreferenceStatus::Saving,
            &ControllerBindings::default(),
        );
        worker.observe(ControllerPreferenceStatus::Saving, &changed_bindings());
        drop(worker);
        assert_eq!(
            load(&directory.join(FILENAME)),
            Ok(Some(changed_bindings()))
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn loading_or_failed_preferences_never_write_defaults_on_shutdown() {
        let directory = directory("preserve");
        let path = directory.join(FILENAME);
        fs::write(&path, b"unrecognized configuration").unwrap();
        let mut worker = ControllerPreferenceWorker::with_directory(Some(directory.clone()));
        worker.observe(
            ControllerPreferenceStatus::Loading,
            &ControllerBindings::default(),
        );
        worker.observe(
            ControllerPreferenceStatus::Failed(ControllerFailure::PreferenceDecode),
            &ControllerBindings::default(),
        );
        drop(worker);
        assert_eq!(fs::read(path).unwrap(), b"unrecognized configuration");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn io_failure_preserves_existing_file() {
        let directory = directory("io");
        let path = directory.join(FILENAME);
        fs::write(&path, b"occupied").unwrap();
        assert_eq!(
            save(&path.join("child"), &ControllerBindings::default()),
            Err(ControllerFailure::PreferenceWrite)
        );
        assert_eq!(fs::read(path).unwrap(), b"occupied");
        assert_eq!(load(&directory), Err(ControllerFailure::PreferenceRead));
        fs::remove_dir_all(directory).unwrap();
    }
}
