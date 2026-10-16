//! One durable recording checkpoint, with its exact composite Song baseline.
//! Control may serialize snapshots; the single worker owns all mutation of disk.
use crate::persistence::{atomic_write, Document, MAX_DOCUMENT_BYTES};
use crest_tracker::editing::Editor;
use crest_tracker::recording::{Recorder, RecordingCheckpoint};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

const RECOVERY_FILE: &str = "recording-recovery.crest";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoverySnapshot {
    format: String,
    version: u32,
    baseline: Document,
    recording: RecordingCheckpoint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkpoint_unix_millis: Option<u64>,
}

impl RecoverySnapshot {
    pub fn new(baseline: Document, recording: RecordingCheckpoint) -> Result<Self, String> {
        let snapshot = Self {
            format: "crest-tracker-recording-recovery".into(),
            version: 1,
            baseline,
            recording,
            checkpoint_unix_millis: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|error| format!("Cannot timestamp recovery checkpoint: {error}"))?
                    .as_millis()
                    .try_into()
                    .map_err(|_| "Recovery timestamp overflow")?,
            ),
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn baseline(&self) -> &Document {
        &self.baseline
    }

    pub fn checkpoint_unix_millis(&self) -> Option<u64> {
        self.checkpoint_unix_millis
    }
    pub fn take_events(&self) -> usize {
        self.recording.draft_event_count()
    }

    /// The host must explicitly adopt the saved composite baseline first. This
    /// compares Synth-native content, mappings and pins as well as musical data.
    pub fn restore(&self, current: &Document, editor: &Editor) -> Result<Recorder, String> {
        self.validate()?;
        current.validate()?;
        if serde_json::to_value(current).map_err(|e| e.to_string())?
            != serde_json::to_value(&self.baseline).map_err(|e| e.to_string())?
            || current.song != *editor.song()
        {
            return Err("Recording recovery belongs to a different composite Song baseline".into());
        }
        Recorder::recover(self.recording.clone(), editor).map_err(|e| e.to_string())
    }

    fn validate(&self) -> Result<(), String> {
        if self.format != "crest-tracker-recording-recovery" || self.version != 1 {
            return Err("Unsupported recording recovery format or version".into());
        }
        self.baseline.validate()?;
        let editor = Editor::new(self.baseline.song.clone()).map_err(|e| e.to_string())?;
        Recorder::recover(self.recording.clone(), &editor).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Verify and normalize native asset identities before any recovery graph
    /// can activate. Composition and the take checkpoint remain byte-for-byte.
    pub fn resolve_resources(mut self) -> Result<Self, String> {
        self.validate()?;
        for resource in &mut self.baseline.resources {
            resource.file = None;
        }
        crate::synth::resources::prepare_open(
            &mut self.baseline,
            Path::new("recording-recovery.crest"),
        )?;
        self.validate()?;
        Ok(self)
    }

    fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
            return Err("Recording recovery exceeds the 64 MiB admission budget".into());
        }
        Ok(bytes)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryAction {
    Written,
    Deleted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryCompletion {
    pub generation: u64,
    pub action: RecoveryAction,
}

enum Task {
    Write { generation: u64, bytes: Vec<u8> },
    Delete { generation: u64 },
}

impl Task {
    fn completion(&self) -> RecoveryCompletion {
        match self {
            Self::Write { generation, .. } => RecoveryCompletion {
                generation: *generation,
                action: RecoveryAction::Written,
            },
            Self::Delete { generation } => RecoveryCompletion {
                generation: *generation,
                action: RecoveryAction::Deleted,
            },
        }
    }

    fn execute(self, path: &Path) -> Result<(), String> {
        match self {
            Self::Write { bytes, .. } => {
                let parent = path
                    .parent()
                    .ok_or("Recording recovery path has no parent")?;
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                atomic_write(path, &bytes)
            }
            Self::Delete { .. } => match fs::remove_file(path) {
                Ok(()) => File::open(
                    path.parent()
                        .ok_or("Recording recovery path has no parent")?,
                )
                .and_then(|directory| directory.sync_all())
                .map_err(|e| e.to_string()),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.to_string()),
            },
        }
    }
}

#[derive(Default)]
struct State {
    pending: Option<Task>,
    next_generation: u64,
    completed: Option<RecoveryCompletion>,
    // A later success cannot erase an unobserved I/O error. Both result slots
    // remain bounded when the window stops polling.
    failure: Option<String>,
    closing: bool,
    disconnected_reported: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    ready: Condvar,
}

pub struct RecoveryWorker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl RecoveryWorker {
    /// No directory I/O occurs here; creation and writes happen on the worker.
    pub fn new(data_directory: PathBuf) -> Result<Self, String> {
        let shared = Arc::new(Shared::default());
        let worker_shared = Arc::clone(&shared);
        let path = data_directory.join(RECOVERY_FILE);
        let thread = thread::Builder::new()
            .name("crest-recording-recovery".into())
            .spawn(move || run_worker(worker_shared, path))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    /// At most one write is in progress and one latest task is pending. A newer
    /// submission replaces the pending task; generations identify acknowledgments.
    pub fn submit(&self, snapshot: RecoverySnapshot) -> Result<u64, String> {
        let bytes = snapshot.encode()?;
        self.enqueue(|generation| Task::Write { generation, bytes })
    }

    /// Explicit commit/discard cleanup. Replaces any pending older snapshot;
    /// the one worker executes this after an already-running write finishes.
    pub fn delete(&self) -> Result<u64, String> {
        self.enqueue(|generation| Task::Delete { generation })
    }

    fn enqueue(&self, task: impl FnOnce(u64) -> Task) -> Result<u64, String> {
        if self.thread.as_ref().is_none_or(JoinHandle::is_finished) {
            return Err("Recording recovery worker disconnected".into());
        }
        let mut state = self
            .shared
            .state
            .lock()
            .map_err(|_| "Recording recovery worker state failed")?;
        if state.closing {
            return Err("Recording recovery worker is closing".into());
        }
        let generation = state
            .next_generation
            .checked_add(1)
            .ok_or("Recording recovery generation exhausted")?;
        state.next_generation = generation;
        state.pending = Some(task(generation));
        self.shared.ready.notify_one();
        Ok(generation)
    }

    pub fn poll(&self) -> Option<Result<RecoveryCompletion, String>> {
        let mut state = match self.shared.state.lock() {
            Ok(state) => state,
            Err(_) => return Some(Err("Recording recovery worker state failed".into())),
        };
        if let Some(error) = state.failure.take() {
            return Some(Err(error));
        }
        if let Some(completed) = state.completed.take() {
            return Some(Ok(completed));
        }
        if self.thread.as_ref().is_none_or(JoinHandle::is_finished) && !state.disconnected_reported
        {
            state.disconnected_reported = true;
            return Some(Err(
                "Recording recovery worker disconnected before completion".into(),
            ));
        }
        None
    }

    /// Startup/worker-side read. Absence is normal; malformed, oversized,
    /// version-mismatched or internally inconsistent files are explicit errors.
    pub fn load(data_directory: &Path) -> Result<Option<RecoverySnapshot>, String> {
        let file = match File::open(data_directory.join(RECOVERY_FILE)) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        let mut bytes = Vec::new();
        file.take(MAX_DOCUMENT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
            return Err("Recording recovery exceeds the 64 MiB admission budget".into());
        }
        let snapshot: RecoverySnapshot =
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        snapshot.validate()?;
        Ok(Some(snapshot))
    }
}

impl Drop for RecoveryWorker {
    fn drop(&mut self) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.closing = true;
            self.shared.ready.notify_one();
        }
        // Shutdown drains the latest accepted task before releasing ownership.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_worker(shared: Arc<Shared>, path: PathBuf) {
    loop {
        let task = {
            let Ok(mut state) = shared.state.lock() else {
                return;
            };
            while state.pending.is_none() && !state.closing {
                let Ok(next) = shared.ready.wait(state) else {
                    return;
                };
                state = next;
            }
            match state.pending.take() {
                Some(task) => task,
                None => return,
            }
        };
        let completion = task.completion();
        let result = task.execute(&path);
        let Ok(mut state) = shared.state.lock() else {
            return;
        };
        match result {
            Ok(()) => state.completed = Some(completion),
            Err(error) if state.failure.is_none() => {
                state.failure = Some(format!(
                    "Recording recovery generation {} failed: {error}",
                    completion.generation
                ));
            }
            Err(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crest_tracker::composition::*;
    use crest_tracker::editing::{Command, Occurrence};
    use crest_tracker::recording::{CaptureInput, CaptureMessage, CaptureState};
    use crest_tracker::time::Beat;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!("crest-recovery-{}", uuid::Uuid::new_v4()))
    }

    fn fixture() -> (Document, Recorder) {
        let definition = json!({"engine":"braids","native":{"timbre":0.25}});
        let pinned = crate::library::PinnedSound {
            resource: uuid::Uuid::new_v4().to_string(),
            revision: format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&definition).unwrap())
            ),
            definition,
        };
        let instrument = InstrumentId::new(100).unwrap();
        let mut song = Song::default();
        song.instruments.insert(
            instrument,
            InstrumentUse {
                name: "Sound".into(),
                resource: Some(ResourceRevision {
                    resource: pinned.resource.clone(),
                    revision: pinned.revision.clone(),
                }),
            },
        );
        let mut editor = Editor::new(song).unwrap();
        let track = editor.song().tracks[0].id;
        editor
            .apply(0, Command::AddPhrase { track, slot: 0 })
            .unwrap();
        let usage = editor.song().tracks[0].slots[0].as_ref().unwrap();
        let phrase_use = &editor.song().chains[&usage.chain].phrases[0];
        let occurrence = Occurrence {
            track,
            chain_use: usage.id,
            phrase_use: phrase_use.id,
        };
        let mut recorder = Recorder::default();
        recorder.arm(&editor, occurrence).unwrap();
        recorder.request_record(Beat::ZERO).unwrap();
        recorder
            .input(CaptureInput {
                at: Beat::ONE,
                source_channel: Channel::FIRST,
                target: Target::instrument(instrument),
                message: CaptureMessage::NoteOn {
                    pitch: 60,
                    velocity: 100,
                },
            })
            .unwrap();
        let mut document = Document::new(
            editor.song().clone(),
            json!({"version":1,"patches":[{"id":42,"native":{"modulation":[1,2,3]}}]}),
            BTreeMap::from([(instrument, 42)]),
        );
        document.pinned_sounds.push(pinned);
        (document, recorder)
    }

    fn until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "recovery worker did not progress"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn paused_worker(root: &Path) -> (RecoveryWorker, std::sync::mpsc::Sender<()>) {
        let shared = Arc::new(Shared::default());
        let worker_shared = Arc::clone(&shared);
        let path = root.join(RECOVERY_FILE);
        let (release, wait) = std::sync::mpsc::channel();
        let thread = thread::spawn(move || {
            // Timeout also prevents an assertion failure stranding test teardown.
            let _ = wait.recv_timeout(Duration::from_secs(2));
            run_worker(worker_shared, path);
        });
        (
            RecoveryWorker {
                shared,
                thread: Some(thread),
            },
            release,
        )
    }

    #[test]
    fn recovery_review_keeps_timestamp_and_older_checkpoints_have_no_fabricated_time() {
        let (baseline, recorder) = fixture();
        let snapshot = RecoverySnapshot::new(baseline.clone(), recorder.checkpoint()).unwrap();
        let timestamp = snapshot.checkpoint_unix_millis().unwrap();
        assert!(timestamp > 0);
        let mut encoded = serde_json::to_value(&snapshot).unwrap();
        let decoded: RecoverySnapshot = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(decoded.checkpoint_unix_millis(), Some(timestamp));
        assert_eq!(decoded.take_events(), snapshot.take_events());
        encoded
            .as_object_mut()
            .unwrap()
            .remove("checkpoint_unix_millis");
        let legacy: RecoverySnapshot = serde_json::from_value(encoded).unwrap();
        assert_eq!(legacy.checkpoint_unix_millis(), None);
        assert!(legacy
            .restore(&baseline, &Editor::new(baseline.song.clone()).unwrap())
            .is_ok());
    }

    #[test]
    fn durable_roundtrip_preserves_full_native_baseline_and_restores_only_explicitly() {
        let root = root();
        assert!(RecoveryWorker::load(&root).unwrap().is_none());
        let (baseline, recorder) = fixture();
        let snapshot = RecoverySnapshot::new(baseline.clone(), recorder.checkpoint()).unwrap();
        let worker = RecoveryWorker::new(root.clone()).unwrap();
        let generation = worker.submit(snapshot).unwrap();
        let mut completion = None;
        until(|| {
            completion = worker.poll();
            completion.is_some()
        });
        assert_eq!(
            completion.unwrap().unwrap(),
            RecoveryCompletion {
                generation,
                action: RecoveryAction::Written
            }
        );
        let loaded = RecoveryWorker::load(&root).unwrap().unwrap();
        assert_eq!(
            serde_json::to_value(loaded.baseline()).unwrap(),
            serde_json::to_value(&baseline).unwrap()
        );
        let editor = Editor::new(baseline.song.clone()).unwrap();
        let recovered = loaded.restore(&baseline, &editor).unwrap();
        assert!(matches!(
            recovered.state(),
            CaptureState::Interrupted { .. }
        ));
        assert_eq!(recovered.held_count(), 1);
        let mut changed = baseline.clone();
        changed.synth["patches"][0]["native"]["modulation"] = json!([9]);
        assert!(loaded.restore(&changed, &editor).is_err());
        changed = baseline.clone();
        changed
            .installed_patches
            .insert(InstrumentId::new(100).unwrap(), 43);
        assert!(loaded.restore(&changed, &editor).is_err());
        changed = baseline.clone();
        changed.pinned_sounds.push(changed.pinned_sounds[0].clone());
        assert!(loaded.restore(&changed, &editor).is_err());
        let generation = worker.delete().unwrap();
        let mut completion = None;
        until(|| {
            completion = worker.poll();
            completion.is_some()
        });
        assert_eq!(
            completion.unwrap().unwrap(),
            RecoveryCompletion {
                generation,
                action: RecoveryAction::Deleted
            }
        );
        assert!(RecoveryWorker::load(&root).unwrap().is_none());
        drop(worker);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn latest_pending_snapshot_replaces_older_write_and_shutdown_drains_it() {
        let root = root();
        let (baseline, mut recorder) = fixture();
        let first = RecoverySnapshot::new(baseline.clone(), recorder.checkpoint()).unwrap();
        recorder
            .input(CaptureInput {
                at: Beat::integer(2),
                source_channel: Channel::FIRST,
                target: Target::instrument(InstrumentId::new(100).unwrap()),
                message: CaptureMessage::Control(Control::Bend { value: 12345 }),
            })
            .unwrap();
        let latest = RecoverySnapshot::new(baseline, recorder.checkpoint()).unwrap();
        let expected = latest.encode().unwrap();
        let (worker, release) = paused_worker(&root);
        let first_generation = worker.submit(first).unwrap();
        let generation = worker.submit(latest).unwrap();
        let only_latest_pending = matches!(&worker.shared.state.lock().unwrap().pending,
            Some(Task::Write { generation: pending, bytes }) if *pending == generation && bytes == &expected);
        release.send(()).unwrap();
        drop(worker);
        assert!(only_latest_pending);
        assert!(generation > first_generation);
        assert_eq!(
            RecoveryWorker::load(&root)
                .unwrap()
                .unwrap()
                .encode()
                .unwrap(),
            expected
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_delete_supersedes_pending_write_and_cannot_be_undone_by_it() {
        let root = root();
        fs::create_dir_all(&root).unwrap();
        let (baseline, recorder) = fixture();
        let snapshot = RecoverySnapshot::new(baseline, recorder.checkpoint()).unwrap();
        atomic_write(&root.join(RECOVERY_FILE), &snapshot.encode().unwrap()).unwrap();
        let (worker, release) = paused_worker(&root);
        worker.submit(snapshot).unwrap();
        let generation = worker.delete().unwrap();
        let delete_pending = matches!(worker.shared.state.lock().unwrap().pending, Some(Task::Delete { generation: pending }) if pending == generation);
        release.send(()).unwrap();
        drop(worker);
        assert!(delete_pending);
        assert!(RecoveryWorker::load(&root).unwrap().is_none());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn load_rejects_malformed_version_baseline_and_held_data_without_removing_file() {
        let root = root();
        fs::create_dir_all(&root).unwrap();
        let (baseline, recorder) = fixture();
        let snapshot = RecoverySnapshot::new(baseline, recorder.checkpoint()).unwrap();
        let original = serde_json::to_value(snapshot).unwrap();
        let reject = |mutate: fn(&mut serde_json::Value)| {
            let mut value = original.clone();
            mutate(&mut value);
            let bytes = serde_json::to_vec(&value).unwrap();
            atomic_write(&root.join(RECOVERY_FILE), &bytes).unwrap();
            assert!(RecoveryWorker::load(&root).is_err());
            assert_eq!(fs::read(root.join(RECOVERY_FILE)).unwrap(), bytes);
        };
        reject(|value| value["version"] = json!(2));
        reject(|value| value["baseline"]["song"]["name"] = json!("Different Song"));
        reject(|value| value["baseline"]["synth"] = json!([]));
        reject(|value| value["baseline"]["installed_patches"]["100"] = json!(0));
        reject(|value| {
            value["baseline"]["pinned_sounds"][0]["definition"] = json!({"changed":true})
        });
        reject(|value| {
            value["recording"]["held"][0]["entries"][0]["active"]["velocity"] = json!(0)
        });
        atomic_write(&root.join(RECOVERY_FILE), b"truncated JSON {").unwrap();
        assert!(RecoveryWorker::load(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn io_failure_remains_visible_after_later_success_and_worker_can_retry() {
        let root = root();
        fs::write(&root, b"directory obstructed by file").unwrap();
        let (baseline, recorder) = fixture();
        let snapshot = RecoverySnapshot::new(baseline, recorder.checkpoint()).unwrap();
        let worker = RecoveryWorker::new(root.clone()).unwrap();
        worker.submit(snapshot.clone()).unwrap();
        until(|| worker.shared.state.lock().unwrap().failure.is_some());
        fs::remove_file(&root).unwrap();
        let generation = worker.submit(snapshot).unwrap();
        until(|| worker.shared.state.lock().unwrap().completed.is_some());
        assert!(matches!(worker.poll(), Some(Err(error)) if error.contains("failed")));
        assert_eq!(worker.poll().unwrap().unwrap().generation, generation);
        assert!(RecoveryWorker::load(&root).unwrap().is_some());
        drop(worker);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unexpectedly_finished_worker_reports_disconnection_once() {
        let worker = RecoveryWorker {
            shared: Arc::new(Shared::default()),
            thread: None,
        };
        assert!(worker.delete().unwrap_err().contains("disconnected"));
        assert!(matches!(worker.poll(), Some(Err(error)) if error.contains("disconnected")));
        assert!(worker.poll().is_none());
    }
}
