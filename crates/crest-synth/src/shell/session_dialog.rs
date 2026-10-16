use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionDialogRequestId(u64);

impl SessionDialogRequestId {
    pub fn new(value: u64) -> Result<Self, SessionDialogRequestIdError> {
        (value != 0)
            .then_some(Self(value))
            .ok_or(SessionDialogRequestIdError)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("session dialog request identity must be non-zero")]
pub struct SessionDialogRequestIdError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionFileDialogKind {
    Open,
    SaveAs,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsavedChoice {
    Save,
    Discard,
    Cancel,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionDialogRequest {
    File {
        request_id: SessionDialogRequestId,
        kind: SessionFileDialogKind,
        suggested_name: Option<String>,
    },
    UnsavedChanges {
        request_id: SessionDialogRequestId,
        document_name: String,
    },
}

impl SessionDialogRequest {
    pub const fn request_id(&self) -> SessionDialogRequestId {
        match self {
            Self::File { request_id, .. } | Self::UnsavedChanges { request_id, .. } => *request_id,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionDialogResult {
    FileSelected {
        request_id: SessionDialogRequestId,
        path: PathBuf,
    },
    Cancelled {
        request_id: SessionDialogRequestId,
    },
    UnsavedChoice {
        request_id: SessionDialogRequestId,
        choice: UnsavedChoice,
    },
    Failed {
        request_id: SessionDialogRequestId,
        failure: SessionDialogFailure,
    },
}

impl SessionDialogResult {
    pub const fn request_id(&self) -> SessionDialogRequestId {
        match self {
            Self::FileSelected { request_id, .. }
            | Self::Cancelled { request_id }
            | Self::UnsavedChoice { request_id, .. }
            | Self::Failed { request_id, .. } => *request_id,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SessionDialogFailure {
    #[error("a session dialog is already outstanding")]
    Busy,
    #[error("the native session dialog is unavailable")]
    Unavailable,
}

pub trait SessionDialogPort: Send {
    fn try_request(&mut self, request: SessionDialogRequest) -> Result<(), SessionDialogFailure>;
    fn try_poll(&mut self) -> Option<SessionDialogResult>;
}

#[derive(Debug, Default)]
struct NativeSessionDialogState {
    request: Option<SessionDialogRequest>,
    in_flight: Option<SessionDialogRequestId>,
    result: Option<SessionDialogResult>,
}

/// Bounded handoff between the lifecycle coordinator and a native window.
/// The window takes one request and returns one correlated result; paths stay
/// at this shell boundary.
#[derive(Clone, Debug, Default)]
pub struct NativeSessionDialogBridge {
    state: Arc<Mutex<NativeSessionDialogState>>,
}

impl NativeSessionDialogBridge {
    pub fn try_take_request(&self) -> Option<SessionDialogRequest> {
        let mut state = self.state.lock().ok()?;
        let request = state.request.take()?;
        state.in_flight = Some(request.request_id());
        Some(request)
    }

    pub fn complete(&self, result: SessionDialogResult) -> Result<(), SessionDialogFailure> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| SessionDialogFailure::Unavailable)?;
        if state.in_flight != Some(result.request_id()) || state.result.is_some() {
            return Err(SessionDialogFailure::Unavailable);
        }
        state.result = Some(result);
        Ok(())
    }
}

impl SessionDialogPort for NativeSessionDialogBridge {
    fn try_request(&mut self, request: SessionDialogRequest) -> Result<(), SessionDialogFailure> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| SessionDialogFailure::Unavailable)?;
        if state.request.is_some() || state.in_flight.is_some() || state.result.is_some() {
            return Err(SessionDialogFailure::Busy);
        }
        state.request = Some(request);
        Ok(())
    }

    fn try_poll(&mut self) -> Option<SessionDialogResult> {
        let mut state = self.state.lock().ok()?;
        let result = state.result.take()?;
        state.in_flight = None;
        Some(result)
    }
}

/// Explicit adapter for windows that do not provide native file UI.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableSessionDialog;

impl SessionDialogPort for UnavailableSessionDialog {
    fn try_request(&mut self, _request: SessionDialogRequest) -> Result<(), SessionDialogFailure> {
        Err(SessionDialogFailure::Unavailable)
    }

    fn try_poll(&mut self) -> Option<SessionDialogResult> {
        None
    }
}

/// Deterministic host-neutral fake used at production workflow boundaries.
/// It enforces the same single-outstanding-dialog rule as a native adapter.
#[derive(Default)]
pub struct DeterministicSessionDialog {
    outstanding: Option<SessionDialogRequest>,
    scripted: VecDeque<SessionDialogResult>,
}

impl DeterministicSessionDialog {
    pub fn new(scripted: impl IntoIterator<Item = SessionDialogResult>) -> Self {
        Self {
            outstanding: None,
            scripted: scripted.into_iter().collect(),
        }
    }

    pub const fn outstanding(&self) -> Option<&SessionDialogRequest> {
        self.outstanding.as_ref()
    }
}

impl SessionDialogPort for DeterministicSessionDialog {
    fn try_request(&mut self, request: SessionDialogRequest) -> Result<(), SessionDialogFailure> {
        if self.outstanding.is_some() {
            return Err(SessionDialogFailure::Busy);
        }
        self.outstanding = Some(request);
        Ok(())
    }

    fn try_poll(&mut self) -> Option<SessionDialogResult> {
        let request = self.outstanding.as_ref()?;
        let index = self
            .scripted
            .iter()
            .position(|result| result.request_id() == request.request_id())?;
        let result = self.scripted.remove(index)?;
        self.outstanding = None;
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_is_a_typed_non_error_and_duplicate_request_is_refused() {
        let request_id = SessionDialogRequestId::new(1).unwrap();
        let mut dialogs =
            DeterministicSessionDialog::new([SessionDialogResult::Cancelled { request_id }]);
        dialogs
            .try_request(SessionDialogRequest::File {
                request_id,
                kind: SessionFileDialogKind::Open,
                suggested_name: None,
            })
            .unwrap();
        assert_eq!(
            dialogs.try_request(SessionDialogRequest::File {
                request_id: SessionDialogRequestId::new(2).unwrap(),
                kind: SessionFileDialogKind::SaveAs,
                suggested_name: Some("Untitled.crest".to_owned()),
            }),
            Err(SessionDialogFailure::Busy)
        );
        assert_eq!(
            dialogs.try_poll(),
            Some(SessionDialogResult::Cancelled { request_id })
        );
        assert!(dialogs.outstanding().is_none());
    }

    #[test]
    fn file_and_unsaved_results_retain_request_correlation() {
        let open_id = SessionDialogRequestId::new(1).unwrap();
        let choice_id = SessionDialogRequestId::new(2).unwrap();
        let path = PathBuf::from("session.crest");
        let mut dialogs = DeterministicSessionDialog::new([
            SessionDialogResult::FileSelected {
                request_id: open_id,
                path: path.clone(),
            },
            SessionDialogResult::UnsavedChoice {
                request_id: choice_id,
                choice: UnsavedChoice::Discard,
            },
        ]);
        dialogs
            .try_request(SessionDialogRequest::File {
                request_id: open_id,
                kind: SessionFileDialogKind::Open,
                suggested_name: None,
            })
            .unwrap();
        assert_eq!(
            dialogs.try_poll(),
            Some(SessionDialogResult::FileSelected {
                request_id: open_id,
                path,
            })
        );
        dialogs
            .try_request(SessionDialogRequest::UnsavedChanges {
                request_id: choice_id,
                document_name: "Untitled".to_owned(),
            })
            .unwrap();
        assert_eq!(
            dialogs.try_poll(),
            Some(SessionDialogResult::UnsavedChoice {
                request_id: choice_id,
                choice: UnsavedChoice::Discard,
            })
        );
    }

    #[test]
    fn native_bridge_allows_exactly_one_correlated_round_trip() {
        let bridge = NativeSessionDialogBridge::default();
        let mut control = bridge.clone();
        let request_id = SessionDialogRequestId::new(7).unwrap();
        control
            .try_request(SessionDialogRequest::File {
                request_id,
                kind: SessionFileDialogKind::Open,
                suggested_name: None,
            })
            .unwrap();
        assert_eq!(
            control.try_request(SessionDialogRequest::UnsavedChanges {
                request_id: SessionDialogRequestId::new(8).unwrap(),
                document_name: "Untitled".to_owned(),
            }),
            Err(SessionDialogFailure::Busy)
        );
        assert_eq!(bridge.try_take_request().unwrap().request_id(), request_id);
        assert_eq!(
            bridge.complete(SessionDialogResult::Cancelled {
                request_id: SessionDialogRequestId::new(8).unwrap(),
            }),
            Err(SessionDialogFailure::Unavailable)
        );
        bridge
            .complete(SessionDialogResult::Cancelled { request_id })
            .unwrap();
        assert_eq!(
            control.try_poll(),
            Some(SessionDialogResult::Cancelled { request_id })
        );
    }
}
