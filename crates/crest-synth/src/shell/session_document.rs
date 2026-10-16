use crate::control::SavedSession;
use std::path::{Path, PathBuf};

/// Shell-owned identity of the active document. Filesystem paths never enter
/// `AppState` or `SavedSession`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DocumentIdentity {
    Untitled,
    Path(PathBuf),
}

impl DocumentIdentity {
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Untitled => None,
            Self::Path(path) => Some(path.as_path()),
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            Self::Untitled => "Untitled".to_owned(),
            Self::Path(path) => path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
                .unwrap_or_else(|| path.display().to_string()),
        }
    }
}

/// Active identity plus the exact last successfully established file/new
/// content. Dirty state is derived rather than toggled, so restoring values to
/// the baseline becomes clean automatically.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionDocument {
    identity: DocumentIdentity,
    clean_baseline: SavedSession,
}

impl SessionDocument {
    pub const fn new_untitled(clean_baseline: SavedSession) -> Self {
        Self {
            identity: DocumentIdentity::Untitled,
            clean_baseline,
        }
    }

    pub const fn identity(&self) -> &DocumentIdentity {
        &self.identity
    }

    pub const fn clean_baseline(&self) -> &SavedSession {
        &self.clean_baseline
    }

    pub fn is_dirty(&self, current: &SavedSession) -> bool {
        current != &self.clean_baseline
    }

    /// Commits identity and baseline together after New/Open/Save/Save As has
    /// succeeded. Tentative destinations never call this method.
    pub fn establish(&mut self, identity: DocumentIdentity, baseline: SavedSession) {
        self.identity = identity;
        self.clean_baseline = baseline;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::braids_capability::BraidsCapability;
    use crate::control::{AppEvent, AppState, Direction, InteractionMode, TopLevelContext};
    use crate::kernel::{MidiChannel, PatchId};
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::patch_output::PatchOutput;
    use crate::synth::{CapabilityRegistry, InstrumentCapabilityProvider, Patch};

    fn state() -> AppState {
        let provider = BraidsCapability::new().unwrap();
        let registry = CapabilityRegistry::new(vec![provider.descriptor()]).unwrap();
        let patch = Patch::new(
            PatchId::new(1).unwrap(),
            "Document".to_owned(),
            provider.default_config().unwrap(),
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        );
        let mut state = AppState::new(registry, GlobalParameters::new(0.0).unwrap());
        state.apply(AppEvent::InstallPatches(vec![patch])).unwrap();
        state
    }

    #[test]
    fn typed_content_equality_drives_dirty_and_exact_restoration_becomes_clean() {
        let mut state = state();
        let baseline = SavedSession::capture(&state);
        let document = SessionDocument::new_untitled(baseline.clone());
        assert!(!document.is_dirty(&SavedSession::capture(&state)));

        state
            .apply(AppEvent::SetInteractionMode(InteractionMode::Adjust))
            .unwrap();
        state.apply(AppEvent::Adjust(Direction::Down)).unwrap();
        assert!(document.is_dirty(&SavedSession::capture(&state)));

        state.apply(AppEvent::Adjust(Direction::Up)).unwrap();
        assert_eq!(SavedSession::capture(&state), baseline);
        assert!(!document.is_dirty(&SavedSession::capture(&state)));
    }

    #[test]
    fn non_session_activity_preserves_the_existing_dirty_answer() {
        let mut state = state();
        let document = SessionDocument::new_untitled(SavedSession::capture(&state));
        state
            .apply(AppEvent::SetInteractionMode(InteractionMode::Adjust))
            .unwrap();
        state.apply(AppEvent::Adjust(Direction::Down)).unwrap();
        assert!(document.is_dirty(&SavedSession::capture(&state)));

        state
            .apply(AppEvent::SetInteractionMode(InteractionMode::Navigate))
            .unwrap();
        state
            .apply(AppEvent::SelectContext(TopLevelContext::Patch))
            .unwrap();
        assert!(document.is_dirty(&SavedSession::capture(&state)));
    }

    #[test]
    fn path_identity_is_shell_only_and_exposes_only_a_document_name() {
        let state = state();
        let saved = SavedSession::capture(&state);
        let mut document = SessionDocument::new_untitled(saved.clone());
        assert_eq!(document.identity().display_name(), "Untitled");
        let path = PathBuf::from("/tmp/private/session.crest");
        document.establish(
            DocumentIdentity::Path(path.clone()),
            document.clean_baseline().clone(),
        );
        assert_eq!(document.identity().path(), Some(path.as_path()));
        assert_eq!(document.identity().display_name(), "session.crest");
        let saved_json = saved.to_json().unwrap();
        let state_json =
            crate::control::StateProjector::for_graph(crate::real_time::GraphRevision::INITIAL)
                .project_with_shell_tree(&state)
                .unwrap()
                .5
                .json()
                .to_owned();
        for forbidden in [
            "/tmp/private",
            "session.crest",
            "dirty",
            "dialog",
            "pendingContinuation",
        ] {
            assert!(!saved_json.contains(forbidden));
            assert!(!state_json.contains(forbidden));
        }
    }
}
