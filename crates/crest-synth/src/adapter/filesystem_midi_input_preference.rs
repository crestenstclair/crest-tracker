use crate::control::{
    MidiDeviceFailure, MidiInputPreference, MidiInputPreferencePort, MIDI_INPUT_PREFERENCE_VERSION,
};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const MIDI_INPUT_PREFERENCE_FILENAME: &str = "midi-input.json";

/// Resolves the platform's per-user Crest Synth configuration directory.
/// No current-directory or shared-system fallback is permitted: absence is a
/// typed preference-capability failure rather than a write to the wrong scope.
pub fn per_user_midi_config_directory() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        return std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("crest-synth")
        });
    }
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|directory| directory.join("crest-synth"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        return std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".config"))
            })
            .map(|directory| directory.join("crest-synth"));
    }
    #[allow(unreachable_code)]
    None
}

/// Produces the production preference port. A shell with no resolvable user
/// configuration directory still starts; load/store then report their exact
/// typed failures through the worker and reducer.
pub fn per_user_midi_input_preference_store() -> Box<dyn MidiInputPreferencePort> {
    match per_user_midi_config_directory() {
        Some(directory) => Box::new(FilesystemMidiInputPreferenceStore::new(directory)),
        None => Box::new(UnavailableMidiInputPreferenceStore),
    }
}

struct UnavailableMidiInputPreferenceStore;

impl MidiInputPreferencePort for UnavailableMidiInputPreferenceStore {
    fn load(&mut self) -> Result<Option<MidiInputPreference>, MidiDeviceFailure> {
        Err(MidiDeviceFailure::PreferenceReadFailed)
    }

    fn store(&mut self, _value: &MidiInputPreference) -> Result<(), MidiDeviceFailure> {
        Err(MidiDeviceFailure::PreferenceWriteFailed)
    }
}

/// Worker-side JSON storage for the separately versioned selected-input
/// preference. The shell supplies the exact per-user configuration directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemMidiInputPreferenceStore {
    path: PathBuf,
}

impl FilesystemMidiInputPreferenceStore {
    pub fn new(config_directory: impl Into<PathBuf>) -> Self {
        Self {
            path: config_directory.into().join(MIDI_INPUT_PREFERENCE_FILENAME),
        }
    }

    pub fn from_file_path(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn temporary_path(&self) -> PathBuf {
        self.path.with_extension("json.tmp")
    }
}

impl MidiInputPreferencePort for FilesystemMidiInputPreferenceStore {
    fn load(&mut self) -> Result<Option<MidiInputPreference>, MidiDeviceFailure> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(MidiDeviceFailure::PreferenceReadFailed),
        };
        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_| MidiDeviceFailure::PreferenceDecodeFailed)?;
        if let Some(version) = value.get("version").and_then(serde_json::Value::as_u64) {
            if version != u64::from(MIDI_INPUT_PREFERENCE_VERSION) {
                return Err(MidiDeviceFailure::PreferenceVersionUnsupported);
            }
        }
        serde_json::from_value(value)
            .map(Some)
            .map_err(|_| MidiDeviceFailure::PreferenceDecodeFailed)
    }

    fn store(&mut self, value: &MidiInputPreference) -> Result<(), MidiDeviceFailure> {
        let parent = self
            .path
            .parent()
            .ok_or(MidiDeviceFailure::PreferenceWriteFailed)?;
        fs::create_dir_all(parent).map_err(|_| MidiDeviceFailure::PreferenceWriteFailed)?;
        let bytes = serde_json::to_vec_pretty(value)
            .map_err(|_| MidiDeviceFailure::PreferenceWriteFailed)?;
        let temporary = self.temporary_path();
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)
            .map_err(|_| MidiDeviceFailure::PreferenceWriteFailed)?;
        file.write_all(&bytes)
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.sync_all())
            .map_err(|_| MidiDeviceFailure::PreferenceWriteFailed)?;
        drop(file);
        fs::rename(&temporary, &self.path).map_err(|_| MidiDeviceFailure::PreferenceWriteFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{MidiInputDeviceId, MidiPreferredInput};

    fn preference() -> MidiInputPreference {
        MidiInputPreference::new(
            MidiPreferredInput::new(
                MidiInputDeviceId::new("midir-v1", "opaque-a").unwrap(),
                "Controller A",
            )
            .unwrap(),
        )
    }

    fn temp_dir(case: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "crest-midi-preference-{}-{case}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn absent_and_round_trip_preference_are_exact() {
        let directory = temp_dir("round-trip");
        let mut store = FilesystemMidiInputPreferenceStore::new(&directory);
        assert_eq!(store.load().unwrap(), None);
        store.store(&preference()).unwrap();
        assert_eq!(store.load().unwrap(), Some(preference()));
        assert!(!store.temporary_path().exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalid_and_unsupported_documents_are_typed() {
        let directory = temp_dir("invalid");
        let mut store = FilesystemMidiInputPreferenceStore::new(&directory);
        fs::write(store.path(), b"not json").unwrap();
        assert_eq!(store.load(), Err(MidiDeviceFailure::PreferenceDecodeFailed));
        fs::write(
            store.path(),
            br#"{"version":2,"selectedInput":{"identitySchema":"midir-v1","identity":"a","lastKnownDisplayName":"A"}}"#,
        )
        .unwrap();
        assert_eq!(
            store.load(),
            Err(MidiDeviceFailure::PreferenceVersionUnsupported)
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn read_and_write_io_failures_are_typed() {
        let directory = temp_dir("io");
        let mut read_failure = FilesystemMidiInputPreferenceStore::from_file_path(&directory);
        assert_eq!(
            read_failure.load(),
            Err(MidiDeviceFailure::PreferenceReadFailed)
        );

        let parent_file = directory.join("not-a-directory");
        fs::write(&parent_file, b"occupied").unwrap();
        let mut write_failure = FilesystemMidiInputPreferenceStore::new(&parent_file);
        assert_eq!(
            write_failure.store(&preference()),
            Err(MidiDeviceFailure::PreferenceWriteFailed)
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
