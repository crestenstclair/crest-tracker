//! Machine-local settings. Device identities are opaque strings; connections,
//! graph revisions and other runtime state never enter the preference file.
use crest_tracker::time::Tempo;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

const FILE_NAME: &str = "preferences.json";
const MAX_PREFERENCE_BYTES: usize = 64 * 1024;
pub const MIN_CLOCK_DELAY_MS: u32 = 20;
pub const MAX_CLOCK_DELAY_MS: u32 = 2_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachinePreferences {
    pub version: u32,
    pub default_tempo: Tempo,
    #[serde(default)]
    pub monitor: bool,
    #[serde(default)]
    pub mpe_input: Option<crest_tracker::mpe::MpeProfile>,
    #[serde(default)]
    pub mpe_outputs: Vec<crate::midi_output::MpeOutputProfile>,
    #[serde(default)]
    pub clock: ClockSettings,
    #[serde(default)]
    pub midi_controls: Vec<MidiControlBinding>,
    #[serde(default)]
    pub roll_bindings: RollBindings,
    #[serde(default)]
    pub sample_folder: Option<String>,
    #[serde(default)]
    pub soundfont_folder: Option<String>,
}

impl Default for MachinePreferences {
    fn default() -> Self {
        Self {
            version: 1,
            default_tempo: Tempo::DEFAULT,
            monitor: false,
            mpe_input: None,
            mpe_outputs: Vec::new(),
            clock: ClockSettings::default(),
            midi_controls: Vec::new(),
            roll_bindings: RollBindings::default(),
            sample_folder: None,
            soundfont_folder: None,
        }
    }
}

impl MachinePreferences {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err(format!("Unsupported preferences version: {}", self.version));
        }
        let mut endpoints = std::collections::BTreeSet::new();
        for profile in &self.mpe_outputs {
            profile.validate()?;
            if !endpoints.insert(&profile.endpoint) {
                return Err("Duplicate MPE destination profile".into());
            }
        }
        validate_midi_controls(&self.midi_controls)?;
        self.roll_bindings.validate()?;
        validate_identity(self.sample_folder.as_deref(), "sample folder")?;
        validate_identity(self.soundfont_folder.as_deref(), "SoundFont folder")?;
        self.clock.validate()
    }
}

/// Exact physical packets mapped to button actions. These packets are consumed
/// before performance recording; all other packets keep their normal routing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MidiControlBinding {
    pub source: String,
    pub channel: u8,
    pub trigger: MidiControlTrigger,
    pub number: u8,
    pub action: MidiControlAction,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiControlTrigger {
    Note,
    Cc,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiControlAction {
    Up,
    Down,
    Left,
    Right,
    Confirm,
    Back,
    Play,
    Stop,
    Undo,
    Redo,
    Menu,
    Song,
    Instruments,
    Record,
    EndTake,
    Increase,
    Decrease,
}
pub fn validate_midi_controls(bindings: &[MidiControlBinding]) -> Result<(), String> {
    let mut keys = std::collections::BTreeSet::new();
    for binding in bindings {
        if binding.source.trim().is_empty() || binding.source.chars().any(char::is_control) {
            return Err("MIDI control source must be an exact nonblank device identity".into());
        }
        if !(1..=16).contains(&binding.channel) || binding.number > 127 {
            return Err("MIDI control channel must be 1–16 and note/CC number 0–127".into());
        }
        if !keys.insert((
            &binding.source,
            binding.channel,
            binding.trigger,
            binding.number,
        )) {
            return Err(
                "Conflicting MIDI controls: one source/channel/packet cannot have two actions"
                    .into(),
            );
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockSettings {
    /// Exact Synth MidiInputDeviceId display value, retained without normalization.
    pub input: Option<String>,
    /// Exact midir output identity, retained without normalization.
    pub output: Option<String>,
    #[serde(default = "default_clock_delay_ms")]
    pub delay_ms: u32,
}

fn default_clock_delay_ms() -> u32 {
    100
}

impl Default for ClockSettings {
    fn default() -> Self {
        Self {
            input: None,
            output: None,
            delay_ms: default_clock_delay_ms(),
        }
    }
}

impl ClockSettings {
    pub fn validate(&self) -> Result<(), String> {
        validate_identity(self.input.as_deref(), "input")?;
        validate_identity(self.output.as_deref(), "output")?;
        if self.input.is_some() && self.output.is_some() {
            return Err(
                "Clock input and output cannot both be enabled; clock echo is disabled".into(),
            );
        }
        if !(MIN_CLOCK_DELAY_MS..=MAX_CLOCK_DELAY_MS).contains(&self.delay_ms) {
            return Err(format!(
                "Clock delay must be between {MIN_CLOCK_DELAY_MS} and {MAX_CLOCK_DELAY_MS} ms"
            ));
        }
        Ok(())
    }
}

fn validate_identity(identity: Option<&str>, direction: &str) -> Result<(), String> {
    if identity.is_some_and(|value| value.trim().is_empty() || value.chars().any(char::is_control))
    {
        return Err(format!(
            "Clock {direction} identity must be nonblank and contain no control characters"
        ));
    }
    Ok(())
}

/// Reads `preferences.json` inside the supplied directory. A missing file uses
/// defaults without creating it; malformed or inaccessible files remain errors.
pub fn load(directory: &Path) -> Result<MachinePreferences, String> {
    let path = directory.join(FILE_NAME);
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(MachinePreferences::default());
        }
        Err(error) => {
            return Err(format!(
                "Cannot open preferences at {}: {error}",
                path.display()
            ))
        }
    };
    let mut bytes = Vec::new();
    file.take(MAX_PREFERENCE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Cannot read preferences at {}: {error}", path.display()))?;
    if bytes.len() > MAX_PREFERENCE_BYTES {
        return Err(format!(
            "Preferences at {} exceed the 64 KiB limit",
            path.display()
        ));
    }
    let preferences: MachinePreferences = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Invalid preferences at {}: {error}", path.display()))?;
    preferences
        .validate()
        .map_err(|error| format!("Invalid preferences at {}: {error}", path.display()))?;
    Ok(preferences)
}

/// Validates and atomically replaces this directory's preference file. Callers
/// apply settings only after successful completion of their persistence worker.
pub fn save(directory: &Path, preferences: &MachinePreferences) -> Result<(), String> {
    let path = directory.join(FILE_NAME);
    preferences
        .validate()
        .map_err(|error| format!("Invalid preferences for {}: {error}", path.display()))?;
    let bytes = serde_json::to_vec_pretty(preferences).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_PREFERENCE_BYTES {
        return Err(format!(
            "Preferences for {} exceed the 64 KiB limit",
            path.display()
        ));
    }
    fs::create_dir_all(directory).map_err(|error| {
        format!(
            "Cannot create preferences directory {}: {error}",
            directory.display()
        )
    })?;
    crate::persistence::atomic_write(&path, &bytes)
        .map_err(|error| format!("Cannot save preferences at {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("crest-preferences-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn midi_mapping_conflicts_preserve_saved_profile_and_reset_is_explicit() {
        let root = Fixture::new();
        let binding = MidiControlBinding {
            source: "opaque:controls/1".into(),
            channel: 16,
            trigger: MidiControlTrigger::Cc,
            number: 74,
            action: MidiControlAction::Play,
        };
        let preferences = MachinePreferences {
            midi_controls: vec![binding.clone()],
            ..MachinePreferences::default()
        };
        save(&root.0, &preferences).unwrap();
        assert_eq!(load(&root.0).unwrap(), preferences);
        let original = fs::read(root.0.join(FILE_NAME)).unwrap();
        let mut invalid = preferences.clone();
        invalid.midi_controls.push(MidiControlBinding {
            action: MidiControlAction::Stop,
            ..binding
        });
        assert!(save(&root.0, &invalid).unwrap_err().contains("Conflicting"));
        invalid.midi_controls.pop();
        invalid.midi_controls[0].channel = 0;
        assert!(save(&root.0, &invalid).is_err());
        assert_eq!(fs::read(root.0.join(FILE_NAME)).unwrap(), original);
        save(&root.0, &MachinePreferences::default()).unwrap();
        assert!(load(&root.0).unwrap().midi_controls.is_empty());
    }

    #[test]
    fn missing_preferences_use_defaults_without_creating_files() {
        let root = Fixture::new();
        let directory = root.0.join("missing");
        let preferences = load(&directory).unwrap();
        assert_eq!(preferences, MachinePreferences::default());
        assert_eq!(preferences.default_tempo.milli_bpm(), 120_000);
        assert!(!preferences.monitor);
        assert_eq!(preferences.clock.delay_ms, 100);
        preferences.validate().unwrap();
        assert!(!directory.exists());
    }

    #[test]
    fn atomic_reopen_preserves_exact_identities_and_uses_only_supplied_directory() {
        let root = Fixture::new();
        let directory = root.0.join("machine-a");
        let other = root.0.join("machine-b");
        let mut preferences = MachinePreferences {
            default_tempo: Tempo::new(123_456).unwrap(),
            monitor: true,
            clock: ClockSettings {
                input: Some("  CoreMIDI:鍵盤 #17  ".into()),
                delay_ms: MIN_CLOCK_DELAY_MS,
                ..ClockSettings::default()
            },
            ..MachinePreferences::default()
        };
        save(&directory, &preferences).unwrap();
        assert_eq!(load(&directory).unwrap(), preferences);
        assert_eq!(load(&other).unwrap(), MachinePreferences::default());
        preferences.clock.input = None;
        preferences.clock.output = Some("  output:coremidi:908172  ".into());
        preferences.clock.delay_ms = MAX_CLOCK_DELAY_MS;
        save(&directory, &preferences).unwrap();
        assert_eq!(load(&directory).unwrap(), preferences);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        assert!(!other.exists());
    }

    #[test]
    fn validation_rejects_unsupported_versions_echo_and_unadmitted_clock_delay() {
        let mut preferences = MachinePreferences {
            version: 2,
            ..MachinePreferences::default()
        };
        assert!(preferences.validate().unwrap_err().contains("version"));
        preferences.version = 1;
        preferences.clock.input = Some("source".into());
        preferences.clock.output = Some("different destination".into());
        assert!(preferences
            .validate()
            .unwrap_err()
            .contains("echo is disabled"));
        preferences.clock.output = None;
        for delay in [0, MIN_CLOCK_DELAY_MS - 1, MAX_CLOCK_DELAY_MS + 1, u32::MAX] {
            preferences.clock.delay_ms = delay;
            assert!(preferences
                .validate()
                .unwrap_err()
                .contains("20 and 2000 ms"));
        }
        for delay in [MIN_CLOCK_DELAY_MS, 100, MAX_CLOCK_DELAY_MS] {
            preferences.clock.delay_ms = delay;
            preferences.validate().unwrap();
        }
    }

    #[test]
    fn clock_identities_reject_blank_or_control_characters_in_either_direction() {
        for identity in [
            "",
            "   ",
            "\n",
            "device\0tail",
            "device\u{7f}",
            "device\u{85}",
        ] {
            let input = ClockSettings {
                input: Some(identity.into()),
                ..ClockSettings::default()
            };
            assert!(input.validate().unwrap_err().contains("input identity"));
            let output = ClockSettings {
                output: Some(identity.into()),
                ..ClockSettings::default()
            };
            assert!(output.validate().unwrap_err().contains("output identity"));
        }
    }

    #[test]
    fn malformed_or_oversized_files_report_their_path_without_falling_back() {
        let root = Fixture::new();
        let path = root.0.join(FILE_NAME);
        let valid = serde_json::to_value(MachinePreferences::default()).unwrap();
        for (field, value) in [
            ("version", serde_json::json!(2)),
            ("default_tempo", serde_json::json!(0)),
            ("monitor", serde_json::json!("yes")),
            ("unexpected", serde_json::json!(true)),
            (
                "clock",
                serde_json::json!({"input":"in", "output":"out", "delay_ms":100}),
            ),
            ("clock", serde_json::json!({"delay_ms":19})),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
            assert!(load(&root.0)
                .unwrap_err()
                .contains(&path.display().to_string()));
        }
        fs::write(&path, b"{").unwrap();
        assert!(load(&root.0).unwrap_err().contains("Invalid preferences"));
        fs::write(&path, b"{\"default_tempo\":120000}").unwrap();
        assert!(load(&root.0).unwrap_err().contains("version"));
        fs::write(&path, vec![b' '; MAX_PREFERENCE_BYTES + 1]).unwrap();
        assert!(load(&root.0).unwrap_err().contains("64 KiB"));
    }

    #[test]
    fn optional_monitor_and_clock_fields_default_without_weakening_required_version() {
        let root = Fixture::new();
        let path = root.0.join(FILE_NAME);
        fs::write(&path, br#"{"version":1,"default_tempo":120000}"#).unwrap();
        assert_eq!(load(&root.0).unwrap(), MachinePreferences::default());
        fs::write(
            &path,
            br#"{"version":1,"default_tempo":120000,"clock":{"input":"source"}}"#,
        )
        .unwrap();
        let loaded = load(&root.0).unwrap();
        assert_eq!(loaded.clock.input.as_deref(), Some("source"));
        assert_eq!(loaded.clock.delay_ms, 100);
        assert_eq!(loaded.clock.output, None);
    }

    #[test]
    fn failed_saves_preserve_existing_preferences_and_do_not_leave_temporary_files() {
        let root = Fixture::new();
        let directory = root.0.join("settings");
        let preferences = MachinePreferences::default();
        save(&directory, &preferences).unwrap();
        let path = directory.join(FILE_NAME);
        let original = fs::read(&path).unwrap();
        let mut invalid = preferences.clone();
        invalid.version = 0;
        assert!(save(&directory, &invalid).is_err());
        invalid.version = 1;
        invalid.clock.input = Some("x".repeat(MAX_PREFERENCE_BYTES));
        assert!(save(&directory, &invalid).unwrap_err().contains("64 KiB"));
        let blocked = root.0.join("ordinary-file");
        fs::write(&blocked, b"keep").unwrap();
        assert!(save(&blocked.join("settings"), &preferences).is_err());
        assert_eq!(fs::read(&blocked).unwrap(), b"keep");
        let failed_atomic_write = root.0.join("blocked-destination");
        fs::create_dir_all(failed_atomic_write.join(FILE_NAME)).unwrap();
        assert!(save(&failed_atomic_write, &preferences).is_err());
        assert_eq!(fs::read_dir(&failed_atomic_write).unwrap().count(), 1);
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(load(&directory).unwrap(), preferences);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
    }
}

// Retained for lossless reading of existing machine preferences.
use crest_synth::control::ControllerButton;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollBindings {
    pub fine: ControllerButton,
    pub range: ControllerButton,
}
impl Default for RollBindings {
    fn default() -> Self {
        Self {
            fine: ControllerButton::RightShoulder,
            range: ControllerButton::Select,
        }
    }
}
impl RollBindings {
    pub fn validate(&self) -> Result<(), String> {
        if self.fine == self.range {
            return Err("Fine and Range need different buttons".into());
        }
        Ok(())
    }
    pub fn button_options() -> serde_json::Value {
        use ControllerButton::*;
        serde_json::json!([
            South,
            East,
            North,
            West,
            LeftShoulder,
            RightShoulder,
            LeftTrigger,
            RightTrigger,
            Select,
            Start,
            Mode,
            LeftThumb,
            RightThumb,
            DPadUp,
            DPadDown,
            DPadLeft,
            DPadRight
        ]
        .map(|button| serde_json::json!({"value":button,"label":button.label()})))
    }
}
