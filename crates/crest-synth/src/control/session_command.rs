use serde::{Deserialize, Serialize};

/// Host-neutral document intent shared by native menus and Settings. The shell
/// owns paths, dialogs, dirty-state protection and session lifecycle execution.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionCommand {
    New,
    Open,
    Save,
    SaveAs,
    Close,
}

impl SessionCommand {
    pub const SETTINGS_ACTIONS: [Self; 3] = [Self::Save, Self::SaveAs, Self::Open];

    pub const fn label(self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Open => "Load…",
            Self::Save => "Save",
            Self::SaveAs => "Save As…",
            Self::Close => "Close",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::New => "Create a new session.",
            Self::Open => "Open a saved session. You can save unsaved changes before replacing it.",
            Self::Save => "Save the current session. Choose a file when saving for the first time.",
            Self::SaveAs => "Save the current session to a different file.",
            Self::Close => "Close the current session.",
        }
    }
}
