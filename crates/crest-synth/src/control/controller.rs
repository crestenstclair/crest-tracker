//! Canonical controller bindings and Settings state. Only AppState mutates it.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ControllerButton {
    South,
    East,
    North,
    West,
    C,
    Z,
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
    DPadRight,
}

impl ControllerButton {
    pub const fn label(self) -> &'static str {
        match self {
            Self::South => "South (A / Cross)",
            Self::East => "East (B / Circle)",
            Self::North => "North (Y / Triangle)",
            Self::West => "West (X / Square)",
            Self::C => "C (legacy; reassign)",
            Self::Z => "Z (legacy; reassign)",
            Self::LeftShoulder => "Left shoulder (LB / L1)",
            Self::RightShoulder => "Right shoulder (RB / R1)",
            Self::LeftTrigger => "Left trigger (LT / L2)",
            Self::RightTrigger => "Right trigger (RT / R2)",
            Self::Select => "Select / Back",
            Self::Start => "Start / Options",
            Self::Mode => "Home / Guide",
            Self::LeftThumb => "Left stick press",
            Self::RightThumb => "Right stick press",
            Self::DPadUp => "D-pad up",
            Self::DPadDown => "D-pad down",
            Self::DPadLeft => "D-pad left",
            Self::DPadRight => "D-pad right",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ControllerRole {
    Up,
    Down,
    Left,
    Right,
    Edit,
    Shift,
    Preview,
    PreviousPatch,
    NextPatch,
    Settings,
    Popover,
}

impl ControllerRole {
    pub const ALL: [Self; 11] = [
        Self::Up,
        Self::Down,
        Self::Left,
        Self::Right,
        Self::Edit,
        Self::Shift,
        Self::Preview,
        Self::PreviousPatch,
        Self::NextPatch,
        Self::Settings,
        Self::Popover,
    ];
    pub const fn label(self) -> &'static str {
        match self {
            Self::Up => "Move up",
            Self::Down => "Move down",
            Self::Left => "Move left",
            Self::Right => "Move right",
            Self::Edit => "Edit / confirm",
            Self::Shift => "Shift / page modifier",
            Self::Preview => "Hold to preview",
            Self::PreviousPatch => "Previous Patch",
            Self::NextPatch => "Next Patch",
            Self::Settings => "Open Settings",
            Self::Popover => "Open edit popover",
        }
    }
    pub const fn description(self) -> &'static str {
        match self {
            Self::Edit => "Tap to confirm. Hold with a direction to adjust a value.",
            Self::Shift => {
                "Hold with a direction to change pages. Hold with Preview to open Settings."
            }
            Self::Preview => "Hold on a Sample Browser file to hear it. Release to stop.",
            Self::Settings => "Open Settings from a performance page.",
            Self::Popover => "Open the focused control’s options or detailed picker.",
            Self::PreviousPatch | Self::NextPatch => "Select the adjacent Patch without wrapping.",
            _ => "Move focus. Combine with Edit to adjust or Shift to change pages.",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControllerBindings {
    version: u32,
    buttons: [ControllerButton; 11],
}

impl Default for ControllerBindings {
    fn default() -> Self {
        use ControllerButton::*;
        Self {
            version: 2,
            buttons: [
                DPadUp,
                DPadDown,
                DPadLeft,
                DPadRight,
                South,
                East,
                North,
                LeftTrigger,
                RightTrigger,
                Start,
                West,
            ],
        }
    }
}
impl<'de> Deserialize<'de> for ControllerBindings {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            version: u32,
            buttons: Vec<ControllerButton>,
        }
        let mut wire = Wire::deserialize(deserializer)?;
        if !matches!((wire.version, wire.buttons.len()), (1, 10) | (2, 11)) {
            return Err(serde::de::Error::custom(
                "unsupported controller bindings version",
            ));
        }
        if wire
            .buttons
            .iter()
            .enumerate()
            .any(|(i, button)| wire.buttons[..i].contains(button))
        {
            return Err(serde::de::Error::custom(
                "duplicate controller button assignment",
            ));
        }
        if wire.version == 1 {
            let button = std::iter::once(ControllerButton::West)
                .chain([
                    ControllerButton::North,
                    ControllerButton::LeftShoulder,
                    ControllerButton::RightShoulder,
                    ControllerButton::Select,
                    ControllerButton::Mode,
                    ControllerButton::LeftThumb,
                    ControllerButton::RightThumb,
                    ControllerButton::South,
                    ControllerButton::East,
                    ControllerButton::Start,
                    ControllerButton::LeftTrigger,
                    ControllerButton::RightTrigger,
                ])
                .find(|button| !wire.buttons.contains(button))
                .ok_or_else(|| serde::de::Error::custom("no free popover button"))?;
            wire.buttons.push(button);
        }
        Ok(Self {
            version: 2,
            buttons: wire
                .buttons
                .try_into()
                .map_err(|_| serde::de::Error::custom("invalid controller role count"))?,
        })
    }
}
impl ControllerBindings {
    pub(crate) fn is_legacy_default(&self) -> bool {
        use ControllerButton::*;
        self.buttons[..10]
            == [
                DPadUp,
                DPadDown,
                DPadLeft,
                DPadRight,
                South,
                LeftShoulder,
                Start,
                LeftTrigger,
                RightTrigger,
                North,
            ]
    }

    pub fn button(&self, role: ControllerRole) -> ControllerButton {
        self.buttons[role as usize]
    }
    pub fn role(&self, button: ControllerButton) -> Option<ControllerRole> {
        self.buttons
            .iter()
            .position(|value| *value == button)
            .map(|index| ControllerRole::ALL[index])
    }
    pub(super) fn assign(&mut self, role: ControllerRole, button: ControllerButton) {
        let previous = self.button(role);
        if let Some(other) = self.role(button) {
            self.buttons[other as usize] = previous;
        }
        self.buttons[role as usize] = button;
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(tag = "kind", content = "role", rename_all = "camelCase")]
pub enum ControllerSettingId {
    Binding(ControllerRole),
    ResetDefaults,
}
impl ControllerSettingId {
    pub fn all() -> impl Iterator<Item = Self> {
        ControllerRole::ALL
            .into_iter()
            .map(Self::Binding)
            .chain(std::iter::once(Self::ResetDefaults))
    }
    pub const fn label(self) -> &'static str {
        match self {
            Self::Binding(role) => role.label(),
            Self::ResetDefaults => "Restore default buttons",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControllerDevice {
    pub id: usize,
    pub name: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ControllerFailure {
    BackendUnavailable,
    DeviceOpenFailed,
    PreferenceRead,
    PreferenceDecode,
    PreferenceWrite,
    WorkerUnavailable,
}
impl ControllerFailure {
    pub const fn label(self) -> &'static str {
        match self {
            Self::BackendUnavailable => "Controller backend unavailable",
            Self::DeviceOpenFailed => "Could not open controller; reconnect and restart the app",
            Self::PreferenceRead => {
                "Could not read controller buttons; restore defaults to recover"
            }
            Self::PreferenceDecode => "Invalid controller button file; restore defaults to recover",
            Self::PreferenceWrite => {
                "Buttons active, but saving failed; confirm a mapping to retry"
            }
            Self::WorkerUnavailable => "Controller preference worker unavailable",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "failure", rename_all = "camelCase")]
pub enum ControllerPreferenceStatus {
    #[default]
    Loading,
    Saved,
    Saving,
    Failed(ControllerFailure),
}
impl ControllerPreferenceStatus {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Loading => "Loading buttons",
            Self::Saved => "Buttons saved",
            Self::Saving => "Saving buttons",
            Self::Failed(failure) => failure.label(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControllerState {
    pub(crate) devices: Vec<ControllerDevice>,
    pub(crate) bindings: ControllerBindings,
    pub(crate) capture: Option<ControllerRole>,
    pub(crate) ready: bool,
    pub(crate) preference_status: ControllerPreferenceStatus,
    pub(crate) backend_failure: Option<ControllerFailure>,
}
impl ControllerState {
    pub fn devices(&self) -> &[ControllerDevice] {
        &self.devices
    }
    pub fn bindings(&self) -> &ControllerBindings {
        &self.bindings
    }
    pub const fn capture(&self) -> Option<ControllerRole> {
        self.capture
    }
    pub const fn ready(&self) -> bool {
        self.ready
    }
    pub const fn preference_status(&self) -> ControllerPreferenceStatus {
        self.preference_status
    }
    pub const fn backend_failure(&self) -> Option<ControllerFailure> {
        self.backend_failure
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ControllerEvent {
    DevicesChanged {
        devices: Vec<ControllerDevice>,
    },
    BackendFailed {
        failure: ControllerFailure,
    },
    PreferencesLoaded {
        result: Result<Option<ControllerBindings>, ControllerFailure>,
    },
    PreferencesSaved {
        bindings: ControllerBindings,
        result: Result<(), ControllerFailure>,
    },
    ButtonCaptured {
        device_id: usize,
        button: ControllerButton,
    },
}

#[cfg(test)]
mod popover_tests {
    use super::*;
    #[test]
    fn legacy_bindings_gain_popover_without_overwriting_custom_roles() {
        let mut encoded = serde_json::to_value(ControllerBindings::default()).unwrap();
        encoded["version"] = 1.into();
        encoded["buttons"].as_array_mut().unwrap().pop();
        let standard: ControllerBindings = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(
            standard.button(ControllerRole::Popover),
            ControllerButton::West
        );
        encoded["buttons"][6] = "west".into();
        let custom: ControllerBindings = serde_json::from_value(encoded).unwrap();
        assert_eq!(
            custom.button(ControllerRole::Preview),
            ControllerButton::West
        );
        assert_ne!(
            custom.button(ControllerRole::Popover),
            ControllerButton::West
        );
        assert_eq!(
            serde_json::from_value::<ControllerBindings>(serde_json::to_value(&custom).unwrap())
                .unwrap(),
            custom
        );
    }
}
