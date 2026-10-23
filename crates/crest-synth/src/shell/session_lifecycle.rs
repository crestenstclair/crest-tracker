use crate::control::{
    AppLoop, SavedSession, SessionCandidateFailure, SessionCandidateRequest,
    SessionCandidateResult, SessionCandidateSource, SessionCandidateToken, SessionCandidateWorker,
};
use crate::kernel::PatchId;
use crate::real_time::{ControlAudioBoundary, WorkerShutdownError};
use crate::shell::{
    DocumentIdentity, SessionContentToken, SessionDialogFailure, SessionDialogPort,
    SessionDialogRequest, SessionDialogRequestId, SessionDialogResult, SessionDocument,
    SessionFileDialogKind, SessionFileFailure, SessionFilePort, SessionSaveFailure,
    SessionSaveRequest, SessionSaveResult, SessionSaveWorker, UnsavedChoice,
};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionDocumentMarker {
    Ready,
    Busy,
    Error,
}

/// Path-free document facts merged with the immutable product shell at the
/// host boundary. Status is explicit text plus shape/marker, never color-only.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDocumentProjection {
    name: String,
    dirty: bool,
    marker: SessionDocumentMarker,
    operation: Option<String>,
    status: String,
    failure: Option<String>,
}

impl SessionDocumentProjection {
    pub fn new(
        name: impl Into<String>,
        dirty: bool,
        marker: SessionDocumentMarker,
        operation: Option<String>,
        status: impl Into<String>,
        failure: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            dirty,
            marker,
            operation,
            status: status.into(),
            failure,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn dirty(&self) -> bool {
        self.dirty
    }

    pub const fn marker(&self) -> SessionDocumentMarker {
        self.marker
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionShellProjection {
    shell: crate::control::GraphicalShellProjection,
    document: SessionDocumentProjection,
}

impl SessionShellProjection {
    pub const fn shell(&self) -> &crate::control::GraphicalShellProjection {
        &self.shell
    }

    pub const fn document(&self) -> &SessionDocumentProjection {
        &self.document
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionOperation {
    New,
    Open,
    Save,
    SaveAs,
    Close,
}

impl SessionOperation {
    pub const fn label(self) -> &'static str {
        match self {
            Self::New => "NEW",
            Self::Open => "OPEN",
            Self::Save => "SAVE",
            Self::SaveAs => "SAVE AS",
            Self::Close => "CLOSE",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionLifecycleStage {
    Ready,
    AwaitingUnsavedChoice,
    SelectingOpenFile,
    ReadingFile,
    PreparingCandidate,
    StagingGraph,
    ActivatingGraph,
    SelectingSaveDestination,
    WritingFile,
    Failed,
}

impl SessionLifecycleStage {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ready => "READY",
            Self::AwaitingUnsavedChoice => "WAITING FOR SAVE / DISCARD / CANCEL",
            Self::SelectingOpenFile => "SELECTING FILE TO OPEN",
            Self::ReadingFile => "READING SESSION FILE",
            Self::PreparingCandidate => "VALIDATING AND PREPARING SESSION",
            Self::StagingGraph => "STAGING PREPARED AUDIO GRAPH",
            Self::ActivatingGraph => "ACTIVATING PREPARED AUDIO GRAPH",
            Self::SelectingSaveDestination => "SELECTING SAVE DESTINATION",
            Self::WritingFile => "WRITING SESSION FILE",
            Self::Failed => "FAILED — PRIOR SESSION UNCHANGED",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingSessionContinuation {
    New,
    Open,
    Close,
}

impl PendingSessionContinuation {
    const fn operation(self) -> SessionOperation {
        match self {
            Self::New => SessionOperation::New,
            Self::Open => SessionOperation::Open,
            Self::Close => SessionOperation::Close,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SessionLifecycleCause {
    #[error("another session lifecycle operation is already active")]
    Busy,
    #[error("session dialog failed: {0}")]
    Dialog(SessionDialogFailure),
    #[error("session file operation failed: {0}")]
    File(SessionFileFailure),
    #[error("session candidate failed: {0}")]
    Candidate(SessionCandidateFailure),
    #[error("restored sound is missing required Patch {0}")]
    MissingRequiredPatch(PatchId),
    #[error("prepared graph handoff failed: {0}")]
    Structural(crate::control::StructuralAdvanceError),
    #[error("session save failed: {0}")]
    Save(SessionSaveFailure),
    #[error("session worker is busy or unavailable")]
    WorkerBusy,
    #[error("a stale lifecycle completion was received")]
    StaleCompletion,
    #[error("session worker shutdown failed: {0}")]
    Shutdown(WorkerShutdownError),
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{operation:?} failed during {stage:?}: {cause}")]
pub struct SessionLifecycleError {
    operation: SessionOperation,
    stage: SessionLifecycleStage,
    cause: SessionLifecycleCause,
}

impl SessionLifecycleError {
    const fn new(
        operation: SessionOperation,
        stage: SessionLifecycleStage,
        cause: SessionLifecycleCause,
    ) -> Self {
        Self {
            operation,
            stage,
            cause,
        }
    }

    pub const fn operation(&self) -> SessionOperation {
        self.operation
    }

    pub const fn stage(&self) -> SessionLifecycleStage {
        self.stage
    }

    pub const fn cause(&self) -> &SessionLifecycleCause {
        &self.cause
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SessionLifecycleProgress {
    dialog_completed: bool,
    candidate_completed: bool,
    graph_committed: bool,
    save_completed: bool,
    close_approved: bool,
}

impl SessionLifecycleProgress {
    pub const fn graph_committed(self) -> bool {
        self.graph_committed
    }

    pub const fn save_completed(self) -> bool {
        self.save_completed
    }

    pub const fn close_approved(self) -> bool {
        self.close_approved
    }
}

#[derive(Clone, Debug)]
enum DialogPurpose {
    Open,
    SaveAs,
    Unsaved(PendingSessionContinuation),
}

#[derive(Clone, Debug)]
struct OutstandingDialog {
    request_id: SessionDialogRequestId,
    purpose: DialogPurpose,
}

#[derive(Clone, Debug)]
struct CandidateContext {
    token: SessionCandidateToken,
    identity: DocumentIdentity,
    required_patches: Vec<PatchId>,
}

#[derive(Clone, Debug)]
struct ActivationContext {
    saved: SavedSession,
    identity: DocumentIdentity,
}

#[derive(Clone, Debug)]
struct SaveContext {
    token: SessionContentToken,
    destination: PathBuf,
}

/// Shell-owned orchestration for one active document. It owns paths, dialog
/// correlations, clean baseline, pending continuations, and worker state, but
/// every product-state commit still crosses `AppState::apply` via `AppLoop`.
pub struct SessionLifecycleCoordinator {
    document: SessionDocument,
    default_session: SavedSession,
    dialogs: Box<dyn SessionDialogPort>,
    files: Arc<dyn SessionFilePort>,
    candidate_worker: Box<dyn SessionCandidateWorker>,
    save_worker: Box<dyn SessionSaveWorker>,
    operation: Option<SessionOperation>,
    stage: SessionLifecycleStage,
    pending_continuation: Option<PendingSessionContinuation>,
    dialog: Option<OutstandingDialog>,
    candidate: Option<CandidateContext>,
    activation: Option<ActivationContext>,
    save: Option<SaveContext>,
    failure: Option<SessionLifecycleError>,
    next_correlation: u64,
    close_approved: bool,
}

impl SessionLifecycleCoordinator {
    pub fn new(
        default_session: SavedSession,
        dialogs: Box<dyn SessionDialogPort>,
        files: Arc<dyn SessionFilePort>,
        candidate_worker: Box<dyn SessionCandidateWorker>,
        save_worker: Box<dyn SessionSaveWorker>,
    ) -> Self {
        Self {
            document: SessionDocument::new_untitled(default_session.clone()),
            default_session,
            dialogs,
            files,
            candidate_worker,
            save_worker,
            operation: None,
            stage: SessionLifecycleStage::Ready,
            pending_continuation: None,
            dialog: None,
            candidate: None,
            activation: None,
            save: None,
            failure: None,
            next_correlation: 1,
            close_approved: false,
        }
    }

    pub const fn document(&self) -> &SessionDocument {
        &self.document
    }

    pub const fn operation(&self) -> Option<SessionOperation> {
        self.operation
    }

    pub const fn stage(&self) -> SessionLifecycleStage {
        self.stage
    }

    pub const fn pending_continuation(&self) -> Option<PendingSessionContinuation> {
        self.pending_continuation
    }

    pub const fn failure(&self) -> Option<&SessionLifecycleError> {
        self.failure.as_ref()
    }

    /// Saved-field edits are unavailable only after a destructive continuation
    /// has been authorized. Navigation and performance remain live, and direct
    /// Save/Save As deliberately continue to admit edits so a later edit stays
    /// dirty against the exact in-flight capture.
    pub const fn persisted_edits_blocked(&self) -> bool {
        if matches!(
            self.stage,
            SessionLifecycleStage::Ready
                | SessionLifecycleStage::AwaitingUnsavedChoice
                | SessionLifecycleStage::Failed
        ) {
            return false;
        }
        matches!(
            self.operation,
            Some(SessionOperation::New | SessionOperation::Open)
        ) || self.pending_continuation.is_some()
    }

    pub fn is_dirty<Boundary>(&self, app: &AppLoop<Boundary>) -> bool
    where
        Boundary: ControlAudioBoundary,
    {
        self.document.is_dirty(&app.capture_saved_session())
    }

    pub fn project_shell<Boundary>(&self, app: &AppLoop<Boundary>) -> SessionShellProjection
    where
        Boundary: ControlAudioBoundary,
    {
        let dirty = self.is_dirty(app);
        let marker = if self.failure.is_some() {
            SessionDocumentMarker::Error
        } else if self.operation.is_some() {
            SessionDocumentMarker::Busy
        } else {
            SessionDocumentMarker::Ready
        };
        SessionShellProjection {
            shell: app.current_graphical_shell(),
            document: SessionDocumentProjection::new(
                self.document.identity().display_name(),
                dirty,
                marker,
                self.operation.map(|operation| operation.label().to_owned()),
                self.stage.label(),
                self.failure.as_ref().map(ToString::to_string),
            ),
        }
    }

    pub fn dismiss_failure(&mut self) {
        self.failure = None;
        if self.operation.is_none() {
            self.stage = SessionLifecycleStage::Ready;
        }
    }

    /// Routes a path-free command from native chrome or an accepted Settings row.
    pub fn request<Boundary>(
        &mut self,
        command: crate::control::SessionCommand,
        app: &mut AppLoop<Boundary>,
    ) -> Result<(), SessionLifecycleError>
    where
        Boundary: ControlAudioBoundary,
    {
        use crate::control::SessionCommand;
        match command {
            SessionCommand::New => self.request_new(app),
            SessionCommand::Open => self.request_open(app),
            SessionCommand::Save => self.request_save(app),
            SessionCommand::SaveAs => self.request_save_as(app),
            SessionCommand::Close => self.request_close(app),
        }
    }

    pub fn request_new<Boundary>(
        &mut self,
        app: &mut AppLoop<Boundary>,
    ) -> Result<(), SessionLifecycleError>
    where
        Boundary: ControlAudioBoundary,
    {
        self.request_destructive(PendingSessionContinuation::New, app)
    }

    pub fn request_open<Boundary>(
        &mut self,
        app: &mut AppLoop<Boundary>,
    ) -> Result<(), SessionLifecycleError>
    where
        Boundary: ControlAudioBoundary,
    {
        self.request_destructive(PendingSessionContinuation::Open, app)
    }

    pub fn request_close<Boundary>(
        &mut self,
        app: &mut AppLoop<Boundary>,
    ) -> Result<(), SessionLifecycleError>
    where
        Boundary: ControlAudioBoundary,
    {
        self.close_approved = false;
        self.request_destructive(PendingSessionContinuation::Close, app)
    }

    pub fn request_save<Boundary>(
        &mut self,
        app: &AppLoop<Boundary>,
    ) -> Result<(), SessionLifecycleError>
    where
        Boundary: ControlAudioBoundary,
    {
        self.ensure_idle(SessionOperation::Save)?;
        match self.document.identity() {
            DocumentIdentity::Untitled => self.request_save_as_dialog(SessionOperation::SaveAs),
            DocumentIdentity::Path(path) => self.submit_save(
                SessionOperation::Save,
                path.clone(),
                app.capture_saved_session(),
            ),
        }
    }

    pub fn request_save_as<Boundary>(
        &mut self,
        _app: &AppLoop<Boundary>,
    ) -> Result<(), SessionLifecycleError>
    where
        Boundary: ControlAudioBoundary,
    {
        self.ensure_idle(SessionOperation::SaveAs)?;
        self.request_save_as_dialog(SessionOperation::SaveAs)
    }

    fn request_destructive<Boundary>(
        &mut self,
        continuation: PendingSessionContinuation,
        app: &mut AppLoop<Boundary>,
    ) -> Result<(), SessionLifecycleError>
    where
        Boundary: ControlAudioBoundary,
    {
        self.ensure_idle(continuation.operation())?;
        self.failure = None;
        if self.document.is_dirty(&app.capture_saved_session()) {
            self.operation = Some(continuation.operation());
            self.pending_continuation = Some(continuation);
            self.stage = SessionLifecycleStage::AwaitingUnsavedChoice;
            let request_id = self.next_dialog_id(continuation.operation())?;
            let request = SessionDialogRequest::UnsavedChanges {
                request_id,
                document_name: self.document.identity().display_name(),
            };
            self.dialogs.try_request(request).map_err(|failure| {
                self.fail(
                    continuation.operation(),
                    SessionLifecycleStage::AwaitingUnsavedChoice,
                    SessionLifecycleCause::Dialog(failure),
                )
            })?;
            self.dialog = Some(OutstandingDialog {
                request_id,
                purpose: DialogPurpose::Unsaved(continuation),
            });
            return Ok(());
        }
        self.continue_destructive(continuation, app)
    }

    fn continue_destructive<Boundary>(
        &mut self,
        continuation: PendingSessionContinuation,
        app: &mut AppLoop<Boundary>,
    ) -> Result<(), SessionLifecycleError>
    where
        Boundary: ControlAudioBoundary,
    {
        self.pending_continuation = None;
        match continuation {
            PendingSessionContinuation::New => self.submit_default_candidate(app),
            PendingSessionContinuation::Open => self.request_open_dialog(),
            PendingSessionContinuation::Close => {
                self.operation = None;
                self.stage = SessionLifecycleStage::Ready;
                self.close_approved = true;
                Ok(())
            }
        }
    }

    fn submit_default_candidate<Boundary>(
        &mut self,
        app: &AppLoop<Boundary>,
    ) -> Result<(), SessionLifecycleError>
    where
        Boundary: ControlAudioBoundary,
    {
        self.submit_candidate(
            SessionOperation::New,
            DocumentIdentity::Untitled,
            SessionCandidateSource::Default(Box::new(self.default_session.clone())),
            app,
        )
    }

    fn request_open_dialog(&mut self) -> Result<(), SessionLifecycleError> {
        let operation = SessionOperation::Open;
        self.operation = Some(operation);
        self.stage = SessionLifecycleStage::SelectingOpenFile;
        let request_id = self.next_dialog_id(operation)?;
        self.dialogs
            .try_request(SessionDialogRequest::File {
                request_id,
                kind: SessionFileDialogKind::Open,
                suggested_name: None,
            })
            .map_err(|failure| {
                self.fail(
                    operation,
                    SessionLifecycleStage::SelectingOpenFile,
                    SessionLifecycleCause::Dialog(failure),
                )
            })?;
        self.dialog = Some(OutstandingDialog {
            request_id,
            purpose: DialogPurpose::Open,
        });
        Ok(())
    }

    fn request_save_as_dialog(
        &mut self,
        operation: SessionOperation,
    ) -> Result<(), SessionLifecycleError> {
        self.operation = Some(operation);
        self.stage = SessionLifecycleStage::SelectingSaveDestination;
        let request_id = self.next_dialog_id(operation)?;
        self.dialogs
            .try_request(SessionDialogRequest::File {
                request_id,
                kind: SessionFileDialogKind::SaveAs,
                suggested_name: Some(format!("{}.crest", self.document.identity().display_name())),
            })
            .map_err(|failure| {
                self.fail(
                    operation,
                    SessionLifecycleStage::SelectingSaveDestination,
                    SessionLifecycleCause::Dialog(failure),
                )
            })?;
        self.dialog = Some(OutstandingDialog {
            request_id,
            purpose: DialogPurpose::SaveAs,
        });
        Ok(())
    }

    fn submit_candidate<Boundary>(
        &mut self,
        operation: SessionOperation,
        identity: DocumentIdentity,
        source: SessionCandidateSource,
        app: &AppLoop<Boundary>,
    ) -> Result<(), SessionLifecycleError>
    where
        Boundary: ControlAudioBoundary,
    {
        self.submit_candidate_requiring_patches(operation, identity, source, Vec::new(), app)
    }

    fn submit_candidate_requiring_patches<Boundary>(
        &mut self,
        operation: SessionOperation,
        identity: DocumentIdentity,
        source: SessionCandidateSource,
        required_patches: Vec<PatchId>,
        app: &AppLoop<Boundary>,
    ) -> Result<(), SessionLifecycleError>
    where
        Boundary: ControlAudioBoundary,
    {
        self.operation = Some(operation);
        self.stage = SessionLifecycleStage::PreparingCandidate;
        let token = self.next_candidate_token(operation)?;
        let revision = app.next_structural_graph_revision().map_err(|error| {
            self.fail(
                operation,
                SessionLifecycleStage::PreparingCandidate,
                SessionLifecycleCause::Structural(error),
            )
        })?;
        self.candidate_worker
            .try_submit(SessionCandidateRequest::new(token, source, revision))
            .map_err(|_| {
                self.fail(
                    operation,
                    SessionLifecycleStage::PreparingCandidate,
                    SessionLifecycleCause::WorkerBusy,
                )
            })?;
        self.candidate = Some(CandidateContext {
            token,
            identity,
            required_patches,
        });
        Ok(())
    }

    fn submit_save(
        &mut self,
        operation: SessionOperation,
        destination: PathBuf,
        capture: SavedSession,
    ) -> Result<(), SessionLifecycleError> {
        self.operation = Some(operation);
        self.stage = SessionLifecycleStage::WritingFile;
        let token = self.next_content_token(operation)?;
        self.save_worker
            .try_submit(SessionSaveRequest::new(token, destination.clone(), capture))
            .map_err(|_| {
                self.fail(
                    operation,
                    SessionLifecycleStage::WritingFile,
                    SessionLifecycleCause::WorkerBusy,
                )
            })?;
        self.save = Some(SaveContext { token, destination });
        Ok(())
    }

    /// Restore an already authorized composite document through the existing worker.
    /// The caller publishes its corresponding document only after Ready is observed.
    pub fn request_host_restore<Boundary: ControlAudioBoundary>(
        &mut self,
        saved: SavedSession,
        app: &AppLoop<Boundary>,
    ) -> Result<(), SessionLifecycleError> {
        self.ensure_idle(SessionOperation::Open)?;
        self.failure = None;
        self.submit_candidate(
            SessionOperation::Open,
            DocumentIdentity::Untitled,
            SessionCandidateSource::Default(Box::new(saved)),
            app,
        )
    }

    /// Restores encoded host sound content using the existing worker's decoder
    /// and registries. No capability or preparer ownership escapes to the host.
    pub fn request_host_restore_json<Boundary: ControlAudioBoundary>(
        &mut self,
        bytes: Vec<u8>,
        app: &AppLoop<Boundary>,
    ) -> Result<(), SessionLifecycleError> {
        self.request_host_restore_json_requiring_patches(bytes, Vec::new(), app)
    }

    /// Validates composite-document Patch references against the worker's
    /// prepared candidate before staging either audio or product replacement.
    pub fn request_host_restore_json_requiring_patches<Boundary: ControlAudioBoundary>(
        &mut self,
        bytes: Vec<u8>,
        required_patches: Vec<PatchId>,
        app: &AppLoop<Boundary>,
    ) -> Result<(), SessionLifecycleError> {
        self.ensure_idle(SessionOperation::Open)?;
        self.failure = None;
        self.submit_candidate_requiring_patches(
            SessionOperation::Open,
            DocumentIdentity::Untitled,
            SessionCandidateSource::OpenedBytes(bytes),
            required_patches,
            app,
        )
    }

    pub fn advance<Boundary>(
        &mut self,
        app: &mut AppLoop<Boundary>,
    ) -> Result<SessionLifecycleProgress, SessionLifecycleError>
    where
        Boundary: ControlAudioBoundary,
    {
        // Consume every intent now so a busy click is never replayed later.
        // Request methods retain typed failures and enforce the same admission
        // and unsaved-change protection as native menu commands.
        while let Some(action) = app.take_session_file_action() {
            let _ = self.request(action, app);
        }
        self.advance_hosted(app)
    }

    /// Advances worker and graph ownership; composite hosts route file intents themselves.
    pub fn advance_hosted<Boundary: ControlAudioBoundary>(
        &mut self,
        app: &mut AppLoop<Boundary>,
    ) -> Result<SessionLifecycleProgress, SessionLifecycleError> {
        let mut progress = SessionLifecycleProgress::default();
        if let Some(result) = self.dialogs.try_poll() {
            progress.dialog_completed = true;
            self.handle_dialog(result, app)?;
        }
        if let Some(result) = self.candidate_worker.try_poll() {
            progress.candidate_completed = true;
            self.handle_candidate(result, app)?;
        }
        if self.activation.is_some() {
            let structural = app.advance_structural().map_err(|error| {
                let operation = self.operation.unwrap_or(SessionOperation::Open);
                self.fail(
                    operation,
                    SessionLifecycleStage::ActivatingGraph,
                    SessionLifecycleCause::Structural(error),
                )
            })?;
            if structural.session_replacement_committed() {
                let activation = self.activation.take().ok_or_else(|| {
                    self.fail(
                        self.operation.unwrap_or(SessionOperation::Open),
                        SessionLifecycleStage::ActivatingGraph,
                        SessionLifecycleCause::StaleCompletion,
                    )
                })?;
                self.document
                    .establish(activation.identity, activation.saved);
                self.operation = None;
                self.stage = SessionLifecycleStage::Ready;
                progress.graph_committed = true;
            }
        }
        if let Some(result) = self.save_worker.try_poll() {
            self.handle_save(result, app)?;
            progress.save_completed = true;
        }
        if self.close_approved {
            progress.close_approved = true;
            self.close_approved = false;
        }
        Ok(progress)
    }

    fn handle_dialog<Boundary>(
        &mut self,
        result: SessionDialogResult,
        app: &mut AppLoop<Boundary>,
    ) -> Result<(), SessionLifecycleError>
    where
        Boundary: ControlAudioBoundary,
    {
        let outstanding = self.dialog.take().ok_or_else(|| {
            self.fail(
                self.operation.unwrap_or(SessionOperation::Open),
                self.stage,
                SessionLifecycleCause::StaleCompletion,
            )
        })?;
        if result.request_id() != outstanding.request_id {
            return Err(self.fail(
                self.operation.unwrap_or(SessionOperation::Open),
                self.stage,
                SessionLifecycleCause::StaleCompletion,
            ));
        }
        match (outstanding.purpose, result) {
            (_, SessionDialogResult::Cancelled { .. }) => {
                self.cancel_active();
                Ok(())
            }
            (DialogPurpose::Open, SessionDialogResult::FileSelected { path, .. }) => {
                self.stage = SessionLifecycleStage::ReadingFile;
                let bytes = self.files.read(&path).map_err(|failure| {
                    self.fail(
                        SessionOperation::Open,
                        SessionLifecycleStage::ReadingFile,
                        SessionLifecycleCause::File(failure),
                    )
                })?;
                self.submit_candidate(
                    SessionOperation::Open,
                    DocumentIdentity::Path(path),
                    SessionCandidateSource::OpenedBytes(bytes),
                    app,
                )
            }
            (DialogPurpose::SaveAs, SessionDialogResult::FileSelected { path, .. }) => {
                self.submit_save(SessionOperation::SaveAs, path, app.capture_saved_session())
            }
            (
                DialogPurpose::Unsaved(continuation),
                SessionDialogResult::UnsavedChoice { choice, .. },
            ) => match choice {
                UnsavedChoice::Cancel => {
                    self.cancel_active();
                    Ok(())
                }
                UnsavedChoice::Discard => self.continue_destructive(continuation, app),
                UnsavedChoice::Save => {
                    self.pending_continuation = Some(continuation);
                    match self.document.identity() {
                        DocumentIdentity::Untitled => {
                            self.request_save_as_dialog(SessionOperation::SaveAs)
                        }
                        DocumentIdentity::Path(path) => self.submit_save(
                            SessionOperation::Save,
                            path.clone(),
                            app.capture_saved_session(),
                        ),
                    }
                }
            },
            (_, SessionDialogResult::Failed { failure, .. }) => Err(self.fail(
                self.operation.unwrap_or(SessionOperation::Open),
                self.stage,
                SessionLifecycleCause::Dialog(failure),
            )),
            _ => Err(self.fail(
                self.operation.unwrap_or(SessionOperation::Open),
                self.stage,
                SessionLifecycleCause::StaleCompletion,
            )),
        }
    }

    fn handle_candidate<Boundary>(
        &mut self,
        result: SessionCandidateResult,
        app: &mut AppLoop<Boundary>,
    ) -> Result<(), SessionLifecycleError>
    where
        Boundary: ControlAudioBoundary,
    {
        let context = self.candidate.take().ok_or_else(|| {
            self.fail(
                self.operation.unwrap_or(SessionOperation::Open),
                SessionLifecycleStage::PreparingCandidate,
                SessionLifecycleCause::StaleCompletion,
            )
        })?;
        if result.token() != context.token {
            return Err(self.fail(
                self.operation.unwrap_or(SessionOperation::Open),
                SessionLifecycleStage::PreparingCandidate,
                SessionLifecycleCause::StaleCompletion,
            ));
        }
        match result {
            SessionCandidateResult::Prepared {
                saved, prepared, ..
            } => {
                if let Some(missing) = context.required_patches.iter().copied().find(|required| {
                    !prepared
                        .state()
                        .patches()
                        .iter()
                        .any(|patch| patch.id() == *required)
                }) {
                    return Err(self.fail(
                        self.operation.unwrap_or(SessionOperation::Open),
                        SessionLifecycleStage::PreparingCandidate,
                        SessionLifecycleCause::MissingRequiredPatch(missing),
                    ));
                }
                self.stage = SessionLifecycleStage::StagingGraph;
                let (replacement, graph) = (*prepared).into_replacement();
                app.stage_session_replacement(replacement, graph)
                    .map_err(|error| {
                        self.fail(
                            self.operation.unwrap_or(SessionOperation::Open),
                            SessionLifecycleStage::StagingGraph,
                            SessionLifecycleCause::Structural(error),
                        )
                    })?;
                self.activation = Some(ActivationContext {
                    saved: *saved,
                    identity: context.identity,
                });
                self.stage = SessionLifecycleStage::ActivatingGraph;
                Ok(())
            }
            SessionCandidateResult::Failed { failure, .. } => Err(self.fail(
                self.operation.unwrap_or(SessionOperation::Open),
                SessionLifecycleStage::PreparingCandidate,
                SessionLifecycleCause::Candidate(failure),
            )),
        }
    }

    fn handle_save<Boundary>(
        &mut self,
        result: SessionSaveResult,
        app: &mut AppLoop<Boundary>,
    ) -> Result<(), SessionLifecycleError>
    where
        Boundary: ControlAudioBoundary,
    {
        let context = self.save.take().ok_or_else(|| {
            self.fail(
                self.operation.unwrap_or(SessionOperation::Save),
                SessionLifecycleStage::WritingFile,
                SessionLifecycleCause::StaleCompletion,
            )
        })?;
        if result.token() != context.token {
            return Err(self.fail(
                self.operation.unwrap_or(SessionOperation::Save),
                SessionLifecycleStage::WritingFile,
                SessionLifecycleCause::StaleCompletion,
            ));
        }
        match result {
            SessionSaveResult::Saved {
                destination,
                written,
                ..
            } => {
                if destination != context.destination {
                    return Err(self.fail(
                        self.operation.unwrap_or(SessionOperation::Save),
                        SessionLifecycleStage::WritingFile,
                        SessionLifecycleCause::StaleCompletion,
                    ));
                }
                self.document
                    .establish(DocumentIdentity::Path(destination), *written);
                self.operation = None;
                self.stage = SessionLifecycleStage::Ready;
                if let Some(continuation) = self.pending_continuation.take() {
                    self.continue_destructive(continuation, app)?;
                }
                Ok(())
            }
            SessionSaveResult::Failed { failure, .. } => {
                self.pending_continuation = None;
                Err(self.fail(
                    self.operation.unwrap_or(SessionOperation::Save),
                    SessionLifecycleStage::WritingFile,
                    SessionLifecycleCause::Save(failure),
                ))
            }
        }
    }

    pub fn shutdown_on_control(&mut self) -> Result<(), SessionLifecycleError> {
        let candidate = self.candidate_worker.shutdown_on_control();
        let save = self.save_worker.shutdown_on_control();
        match candidate.and(save) {
            Ok(()) => Ok(()),
            Err(error) => Err(self.fail(
                self.operation.unwrap_or(SessionOperation::Close),
                self.stage,
                SessionLifecycleCause::Shutdown(error),
            )),
        }
    }

    fn ensure_idle(&mut self, operation: SessionOperation) -> Result<(), SessionLifecycleError> {
        if self.operation.is_some()
            || self.dialog.is_some()
            || self.candidate.is_some()
            || self.activation.is_some()
            || self.save.is_some()
        {
            return Err(SessionLifecycleError::new(
                operation,
                self.stage,
                SessionLifecycleCause::Busy,
            ));
        }
        Ok(())
    }

    fn cancel_active(&mut self) {
        self.operation = None;
        self.stage = SessionLifecycleStage::Ready;
        self.pending_continuation = None;
        self.dialog = None;
        self.failure = None;
    }

    fn fail(
        &mut self,
        operation: SessionOperation,
        stage: SessionLifecycleStage,
        cause: SessionLifecycleCause,
    ) -> SessionLifecycleError {
        let error = SessionLifecycleError::new(operation, stage, cause);
        self.operation = None;
        self.stage = SessionLifecycleStage::Failed;
        self.pending_continuation = None;
        self.dialog = None;
        self.candidate = None;
        self.activation = None;
        self.save = None;
        self.failure = Some(error.clone());
        error
    }

    fn take_correlation(
        &mut self,
        operation: SessionOperation,
    ) -> Result<u64, SessionLifecycleError> {
        let value = self.next_correlation;
        self.next_correlation = self.next_correlation.checked_add(1).ok_or_else(|| {
            self.fail(
                operation,
                self.stage,
                SessionLifecycleCause::StaleCompletion,
            )
        })?;
        Ok(value)
    }

    fn next_dialog_id(
        &mut self,
        operation: SessionOperation,
    ) -> Result<SessionDialogRequestId, SessionLifecycleError> {
        SessionDialogRequestId::new(self.take_correlation(operation)?).map_err(|_| {
            self.fail(
                operation,
                self.stage,
                SessionLifecycleCause::StaleCompletion,
            )
        })
    }

    fn next_candidate_token(
        &mut self,
        operation: SessionOperation,
    ) -> Result<SessionCandidateToken, SessionLifecycleError> {
        SessionCandidateToken::new(self.take_correlation(operation)?).map_err(|_| {
            self.fail(
                operation,
                self.stage,
                SessionLifecycleCause::StaleCompletion,
            )
        })
    }

    fn next_content_token(
        &mut self,
        operation: SessionOperation,
    ) -> Result<SessionContentToken, SessionLifecycleError> {
        SessionContentToken::new(self.take_correlation(operation)?).map_err(|_| {
            self.fail(
                operation,
                self.stage,
                SessionLifecycleCause::StaleCompletion,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::braids_capability::BraidsCapability;
    use crate::adapter::braids_preparer::BraidsPreparer;
    use crate::adapter::lock_free_audio_boundary::{
        LockFreeAudioBoundary, LockFreeAudioHandle, LockFreeControlHandle,
    };
    use crate::adapter::lock_free_structural_graph_boundary::{
        LockFreeStructuralAudioHandle, LockFreeStructuralGraphBoundary,
    };
    use crate::adapter::threaded_session_candidate_worker::ThreadedSessionCandidateWorker;
    use crate::control::{
        AppEvent, AppState, Direction, InteractionMode, PatchControlId, SessionCandidateWorkerBusy,
        SessionCandidateWorkerBusyReason,
    };
    use crate::kernel::{MidiChannel, PatchId};
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_state::MixerState;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::mixer_track_parameters::{MixerTrackParameter, MixerTrackParameters};
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::{
        AudioBoundary, AudioRenderer, GraphHandoffStatus, GraphRevision, PreparedGraphBuilder,
        StructuralGraphBoundary,
    };
    use crate::shell::audio_output::{AudioDeviceConfig, AudioSampleFormat};
    use crate::shell::{
        NativeSessionDialogBridge, SessionFileFailureKind, SessionSaveWorkerBusy,
        SessionSaveWorkerBusyReason,
    };
    use crate::synth::{
        CapabilityRegistry, DescriptorDefaultConfigFactory, EffectCapabilityRegistry,
        InstrumentCapabilityProvider, InstrumentPreparer, Patch,
    };
    use crate::testing::{
        DeterministicGraphPreparationHandle, DeterministicGraphPreparationWorker,
        NativeSessionLifecycleHandoffEvidence, NativeSessionLifecycleHandoffReport,
    };
    use std::collections::{BTreeMap, VecDeque};
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    #[derive(Default)]
    struct MemoryFiles {
        files: Mutex<BTreeMap<PathBuf, Vec<u8>>>,
        fail_write: std::sync::atomic::AtomicBool,
    }

    impl MemoryFiles {
        fn put(&self, path: PathBuf, bytes: Vec<u8>) {
            self.files.lock().unwrap().insert(path, bytes);
        }

        fn set_fail_write(&self, fail: bool) {
            self.fail_write.store(fail, Ordering::SeqCst);
        }
    }

    impl SessionFilePort for MemoryFiles {
        fn read(&self, path: &Path) -> Result<Vec<u8>, SessionFileFailure> {
            self.files
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .ok_or_else(|| SessionFileFailure::new_for_test(SessionFileFailureKind::Read))
        }

        fn write_atomic(&self, path: &Path, bytes: &[u8]) -> Result<(), SessionFileFailure> {
            if self.fail_write.load(Ordering::SeqCst) {
                return Err(SessionFileFailure::new_for_test(
                    SessionFileFailureKind::WriteTemporary,
                ));
            }
            self.files
                .lock()
                .unwrap()
                .insert(path.to_path_buf(), bytes.to_vec());
            Ok(())
        }
    }

    enum AutoResponse {
        File(PathBuf),
        Choice(UnsavedChoice),
        Cancel,
    }

    struct AutoDialog {
        scripted: VecDeque<AutoResponse>,
        ready: Option<SessionDialogResult>,
    }

    impl AutoDialog {
        fn new(scripted: impl IntoIterator<Item = AutoResponse>) -> Self {
            Self {
                scripted: scripted.into_iter().collect(),
                ready: None,
            }
        }
    }

    impl SessionDialogPort for AutoDialog {
        fn try_request(
            &mut self,
            request: SessionDialogRequest,
        ) -> Result<(), SessionDialogFailure> {
            if self.ready.is_some() {
                return Err(SessionDialogFailure::Busy);
            }
            let request_id = request.request_id();
            self.ready = Some(
                match self.scripted.pop_front().unwrap_or(AutoResponse::Cancel) {
                    AutoResponse::File(path) => {
                        SessionDialogResult::FileSelected { request_id, path }
                    }
                    AutoResponse::Choice(choice) => {
                        SessionDialogResult::UnsavedChoice { request_id, choice }
                    }
                    AutoResponse::Cancel => SessionDialogResult::Cancelled { request_id },
                },
            );
            Ok(())
        }

        fn try_poll(&mut self) -> Option<SessionDialogResult> {
            self.ready.take()
        }
    }

    struct ImmediateCandidateWorker {
        capabilities: CapabilityRegistry,
        preparers: Vec<Box<dyn InstrumentPreparer>>,
        audio_config: AudioDeviceConfig,
        pending: Option<SessionCandidateResult>,
        submissions: Arc<AtomicUsize>,
    }

    impl SessionCandidateWorker for ImmediateCandidateWorker {
        fn try_submit(
            &mut self,
            request: SessionCandidateRequest,
        ) -> Result<(), SessionCandidateWorkerBusy> {
            if self.pending.is_some() {
                return Err(SessionCandidateWorkerBusy::new(
                    SessionCandidateWorkerBusyReason::OutstandingRequest,
                    request,
                ));
            }
            self.submissions.fetch_add(1, Ordering::SeqCst);
            let (token, source, revision) = request.into_parts();
            let saved = match source {
                SessionCandidateSource::Default(saved) => Ok(saved),
                SessionCandidateSource::OpenedBytes(bytes) => String::from_utf8(bytes)
                    .map_err(|_| SessionCandidateFailure::DecodeEncoding)
                    .and_then(|json| {
                        SavedSession::from_json(&json, &self.capabilities)
                            .map(Box::new)
                            .map_err(SessionCandidateFailure::Saved)
                    }),
            };
            self.pending = Some(match saved {
                Ok(saved) => match saved.prepare_restore(
                    self.capabilities.clone(),
                    EffectCapabilityRegistry::default(),
                    &self.preparers,
                    &[],
                    revision,
                    self.audio_config.sample_rate(),
                    self.audio_config.render_capacity_frames(),
                ) {
                    Ok(prepared) => SessionCandidateResult::Prepared {
                        token,
                        saved,
                        prepared: Box::new(prepared),
                    },
                    Err(error) => SessionCandidateResult::Failed {
                        token,
                        failure: SessionCandidateFailure::Restore(error),
                    },
                },
                Err(failure) => SessionCandidateResult::Failed { token, failure },
            });
            Ok(())
        }

        fn try_poll(&mut self) -> Option<SessionCandidateResult> {
            self.pending.take()
        }

        fn shutdown_on_control(&mut self) -> Result<(), WorkerShutdownError> {
            self.pending = None;
            Ok(())
        }
    }

    struct DelayedSaveWorker {
        files: Arc<dyn SessionFilePort>,
        pending: Option<SessionSaveResult>,
        defer_once: bool,
    }

    struct ForcedEncodeSaveWorker {
        pending: Option<SessionSaveResult>,
    }

    impl SessionSaveWorker for ForcedEncodeSaveWorker {
        fn try_submit(&mut self, request: SessionSaveRequest) -> Result<(), SessionSaveWorkerBusy> {
            let token = request.token();
            let destination = request.destination().to_path_buf();
            self.pending = Some(SessionSaveResult::Failed {
                token,
                destination,
                failure: SessionSaveFailure::Encode(crate::control::SavedSessionError::Encode),
            });
            Ok(())
        }

        fn try_poll(&mut self) -> Option<SessionSaveResult> {
            self.pending.take()
        }

        fn shutdown_on_control(&mut self) -> Result<(), WorkerShutdownError> {
            self.pending = None;
            Ok(())
        }
    }

    impl SessionSaveWorker for DelayedSaveWorker {
        fn try_submit(&mut self, request: SessionSaveRequest) -> Result<(), SessionSaveWorkerBusy> {
            if self.pending.is_some() {
                return Err(SessionSaveWorkerBusy::new(
                    SessionSaveWorkerBusyReason::OutstandingRequest,
                    request,
                ));
            }
            let token = request.token();
            let destination = request.destination().to_path_buf();
            let written = request.capture().clone();
            self.pending = Some(match written.to_json() {
                Ok(json) => match self.files.write_atomic(&destination, json.as_bytes()) {
                    Ok(()) => SessionSaveResult::Saved {
                        token,
                        destination,
                        written: Box::new(written),
                    },
                    Err(failure) => SessionSaveResult::Failed {
                        token,
                        destination,
                        failure: SessionSaveFailure::File(failure),
                    },
                },
                Err(failure) => SessionSaveResult::Failed {
                    token,
                    destination,
                    failure: SessionSaveFailure::Encode(failure),
                },
            });
            self.defer_once = true;
            Ok(())
        }

        fn try_poll(&mut self) -> Option<SessionSaveResult> {
            if self.pending.is_some() && self.defer_once {
                self.defer_once = false;
                None
            } else {
                self.pending.take()
            }
        }

        fn shutdown_on_control(&mut self) -> Result<(), WorkerShutdownError> {
            self.pending = None;
            Ok(())
        }
    }

    type TestApp = AppLoop<LockFreeControlHandle>;
    type TestRenderer = AudioRenderer<LockFreeAudioHandle, LockFreeStructuralAudioHandle>;

    fn braids_state(mixer_gain: f32) -> (CapabilityRegistry, AppState) {
        let provider = BraidsCapability::new().unwrap();
        let descriptor = provider.descriptor();
        let default_id = descriptor.id().clone();
        let registry = CapabilityRegistry::new(vec![descriptor]).unwrap();
        let creation_blueprint = crate::control::PatchCreationBlueprint::resolve(
            &default_id,
            &DescriptorDefaultConfigFactory::new(
                registry.clone(),
                vec![Box::new(BraidsCapability::new().unwrap())],
            ),
        )
        .unwrap();
        let mut mixer = MixerState::default();
        mixer.set_track(
            MixerTrackId::default(),
            MixerTrackParameters::default()
                .with_scalar_value(MixerTrackParameter::Level, mixer_gain)
                .unwrap(),
        );
        let mut state = AppState::new(registry.clone(), GlobalParameters::new(0.0).unwrap())
            .with_initial_mixer(mixer)
            .with_patch_creation_blueprint(creation_blueprint);
        state
            .apply(AppEvent::InstallPatches(vec![Patch::new(
                PatchId::new(1).unwrap(),
                "INIT".to_owned(),
                provider.default_config().unwrap(),
                MidiChannel::new(0).unwrap(),
                PatchOutput::default(),
            )]))
            .unwrap();
        (registry, state)
    }

    fn harness(
        responses: impl IntoIterator<Item = AutoResponse>,
    ) -> (
        TestApp,
        TestRenderer,
        SessionLifecycleCoordinator,
        Arc<MemoryFiles>,
        Arc<AtomicUsize>,
        SavedSession,
        DeterministicGraphPreparationHandle,
    ) {
        let (registry, state) = braids_state(0.0);
        let default = SavedSession::capture(&state);
        let audio_config = AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 64).unwrap();
        let parameters = crate::control::StateProjector::for_graph(GraphRevision::INITIAL)
            .project(&state)
            .unwrap()
            .2;
        let initial_graph = PreparedGraphBuilder::new(
            &registry,
            &[Box::new(BraidsPreparer::new().unwrap()) as Box<dyn InstrumentPreparer>],
        )
        .build(
            GraphRevision::INITIAL,
            state.patches(),
            parameters.clone(),
            audio_config.sample_rate(),
            audio_config.render_capacity_frames(),
        )
        .unwrap();
        let (audio_control, audio_handle) =
            LockFreeAudioBoundary::new(16, parameters).into_handles();
        let (structural_control, structural_audio) = LockFreeStructuralGraphBoundary::new(
            1,
            1,
            GraphHandoffStatus::with_active(GraphRevision::INITIAL),
        )
        .unwrap()
        .into_handles();
        let graph_worker = DeterministicGraphPreparationWorker::new(
            registry.clone(),
            vec![Box::new(BraidsPreparer::new().unwrap())],
            audio_config,
        );
        let graph_worker_handle = graph_worker.advance_handle();
        let mut app = AppLoop::new(
            state,
            crate::control::StateProjector::for_graph(GraphRevision::INITIAL),
            audio_control,
        )
        .unwrap();
        app.configure_engine_selection(
            DescriptorDefaultConfigFactory::new(
                registry.clone(),
                vec![Box::new(BraidsCapability::new().unwrap())],
            ),
            graph_worker,
            structural_control,
            &initial_graph,
            audio_config,
        )
        .unwrap();
        let renderer = AudioRenderer::new(audio_handle, structural_audio, initial_graph);
        let files = Arc::new(MemoryFiles::default());
        let submissions = Arc::new(AtomicUsize::new(0));
        let candidate = ImmediateCandidateWorker {
            capabilities: registry,
            preparers: vec![Box::new(BraidsPreparer::new().unwrap())],
            audio_config,
            pending: None,
            submissions: Arc::clone(&submissions),
        };
        let save = DelayedSaveWorker {
            files: files.clone(),
            pending: None,
            defer_once: false,
        };
        let lifecycle = SessionLifecycleCoordinator::new(
            default.clone(),
            Box::new(AutoDialog::new(responses)),
            files.clone(),
            Box::new(candidate),
            Box::new(save),
        );
        (
            app,
            renderer,
            lifecycle,
            files,
            submissions,
            default,
            graph_worker_handle,
        )
    }

    fn render(renderer: &mut TestRenderer) {
        let mut output = [0.0; 128];
        renderer.render(&mut output);
        assert!(output.iter().all(|sample| sample.is_finite()));
    }

    fn drive_to_idle(
        lifecycle: &mut SessionLifecycleCoordinator,
        app: &mut TestApp,
        renderer: &mut TestRenderer,
        native_dialogs: &NativeSessionDialogBridge,
        responses: &mut VecDeque<AutoResponse>,
    ) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            service_native_dialog(native_dialogs, responses);
            lifecycle.advance(app).unwrap();
            if lifecycle.stage() == SessionLifecycleStage::ActivatingGraph {
                render(renderer);
            }
            if lifecycle.operation().is_none() && lifecycle.stage() == SessionLifecycleStage::Ready
            {
                return;
            }
            assert!(Instant::now() < deadline, "session lifecycle timed out");
            std::thread::yield_now();
        }
    }

    fn service_native_dialog(
        dialogs: &NativeSessionDialogBridge,
        responses: &mut VecDeque<AutoResponse>,
    ) {
        let Some(request) = dialogs.try_take_request() else {
            return;
        };
        let request_id = request.request_id();
        let result = match responses.pop_front().expect("native response is scripted") {
            AutoResponse::File(path) => SessionDialogResult::FileSelected { request_id, path },
            AutoResponse::Choice(choice) => {
                SessionDialogResult::UnsavedChoice { request_id, choice }
            }
            AutoResponse::Cancel => SessionDialogResult::Cancelled { request_id },
        };
        dialogs.complete(result).unwrap();
    }

    fn request_native_menu(
        id: &str,
        lifecycle: &mut SessionLifecycleCoordinator,
        app: &mut TestApp,
    ) -> Result<(), SessionLifecycleError> {
        match crate::shell::webview::window::session_command_for_menu_id(id)
            .expect("the native File menu id is installed")
        {
            crate::shell::app_window::SessionCommand::New => lifecycle.request_new(app),
            crate::shell::app_window::SessionCommand::Open => lifecycle.request_open(app),
            crate::shell::app_window::SessionCommand::Save => lifecycle.request_save(app),
            crate::shell::app_window::SessionCommand::SaveAs => lifecycle.request_save_as(app),
            crate::shell::app_window::SessionCommand::Close => lifecycle.request_close(app),
        }
    }

    fn drive_to_close_approval(
        lifecycle: &mut SessionLifecycleCoordinator,
        app: &mut TestApp,
        renderer: &mut TestRenderer,
        native_dialogs: &NativeSessionDialogBridge,
        responses: &mut VecDeque<AutoResponse>,
    ) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            service_native_dialog(native_dialogs, responses);
            let progress = lifecycle.advance(app).unwrap();
            if lifecycle.stage() == SessionLifecycleStage::ActivatingGraph {
                render(renderer);
            }
            if progress.close_approved() {
                return;
            }
            assert!(Instant::now() < deadline, "close continuation timed out");
            std::thread::yield_now();
        }
    }

    fn drive_settings_to_idle(
        lifecycle: &mut SessionLifecycleCoordinator,
        app: &mut TestApp,
        renderer: &mut TestRenderer,
    ) {
        for _ in 0..10 {
            lifecycle.advance(app).unwrap();
            render(renderer);
            if lifecycle.operation().is_none() {
                return;
            }
        }
        panic!("Settings session operation did not complete");
    }

    fn settings_file_action(app: &mut TestApp, action: crate::control::SessionCommand) {
        use crate::control::{SemanticAction, SurfaceId};
        if !app.state().interaction().active_surface().is_system() {
            app.dispatch_action(SemanticAction::SetInteractionMode(
                InteractionMode::Navigate,
            ))
            .unwrap();
            app.dispatch_action(SemanticAction::OpenMidiSettings)
                .unwrap();
        }
        while app.state().interaction().active_surface() != SurfaceId::SaveLoadSettings {
            app.dispatch_action(SemanticAction::Navigate(Direction::Left))
                .unwrap();
        }
        while app
            .dispatch_action(SemanticAction::Navigate(Direction::Up))
            .is_ok()
        {}
        for candidate in crate::control::SessionCommand::SETTINGS_ACTIONS {
            if candidate == action {
                break;
            }
            app.dispatch_action(SemanticAction::Navigate(Direction::Down))
                .unwrap();
        }
        app.dispatch_action(SemanticAction::Activate).unwrap();
    }

    #[test]
    fn settings_save_load_uses_existing_file_workers_and_block_boundary_activation() {
        use crate::control::{SemanticAction, SessionCommand};
        let destination = PathBuf::from("settings-save.crest");
        let copy = PathBuf::from("settings-copy.crest");
        let (mut app, mut renderer, mut lifecycle, files, _, initial, _) = harness([
            AutoResponse::File(destination.clone()),
            AutoResponse::File(copy.clone()),
            AutoResponse::File(destination.clone()),
        ]);
        settings_file_action(&mut app, SessionCommand::Save);
        // Two inputs before a shell tick must not queue a second dialog for later.
        app.dispatch_action(SemanticAction::Activate).unwrap();
        lifecycle.advance(&mut app).unwrap();
        assert!(app.take_session_file_action().is_none());
        assert_eq!(
            lifecycle.project_shell(&app).document().marker(),
            SessionDocumentMarker::Busy
        );
        assert_eq!(lifecycle.document().identity(), &DocumentIdentity::Untitled);
        drive_settings_to_idle(&mut lifecycle, &mut app, &mut renderer);
        assert_eq!(
            lifecycle.document().identity(),
            &DocumentIdentity::Path(destination.clone())
        );
        assert!(!lifecycle.is_dirty(&app));
        assert!(files.read(&destination).is_ok());

        settings_file_action(&mut app, SessionCommand::SaveAs);
        drive_settings_to_idle(&mut lifecycle, &mut app, &mut renderer);
        assert_eq!(
            lifecycle.document().identity(),
            &DocumentIdentity::Path(copy.clone())
        );
        // A subsequent Save reuses the established path and needs no dialog.
        app.dispatch_action(SemanticAction::Return).unwrap();
        app.dispatch(AppEvent::SetInteractionMode(InteractionMode::Adjust))
            .unwrap();
        app.dispatch(AppEvent::Adjust(Direction::Down)).unwrap();
        let changed = app.capture_saved_session();
        assert_ne!(changed, initial);
        settings_file_action(&mut app, SessionCommand::Save);
        drive_settings_to_idle(&mut lifecycle, &mut app, &mut renderer);
        let written = SavedSession::from_json(
            &String::from_utf8(files.read(&copy).unwrap()).unwrap(),
            app.capabilities(),
        )
        .unwrap();
        assert_eq!(written, changed);
        settings_file_action(&mut app, SessionCommand::Open);
        lifecycle.advance(&mut app).unwrap();
        assert_eq!(
            app.capture_saved_session(),
            changed,
            "load waits for audio activation"
        );
        drive_settings_to_idle(&mut lifecycle, &mut app, &mut renderer);
        assert_eq!(app.capture_saved_session(), initial);
        assert_eq!(
            lifecycle.document().identity(),
            &DocumentIdentity::Path(destination)
        );
        assert!(!lifecycle.is_dirty(&app));
        render(&mut renderer);
    }

    #[test]
    fn settings_load_cancel_and_failure_preserve_dirty_content_focus_and_sound() {
        use crate::control::SessionCommand;
        let bad = PathBuf::from("broken.crest");
        let (mut app, mut renderer, mut lifecycle, files, _, _, _) = harness([
            AutoResponse::Choice(UnsavedChoice::Cancel),
            AutoResponse::Choice(UnsavedChoice::Discard),
            AutoResponse::File(bad.clone()),
        ]);
        app.dispatch(AppEvent::SetInteractionMode(InteractionMode::Adjust))
            .unwrap();
        app.dispatch(AppEvent::Adjust(Direction::Down)).unwrap();
        files.put(bad, b"not a session".to_vec());
        settings_file_action(&mut app, SessionCommand::Open);
        let saved = app.capture_saved_session();
        let focus = app.state().interaction().clone();
        lifecycle.advance(&mut app).unwrap();
        assert_eq!(app.capture_saved_session(), saved);
        assert_eq!(app.state().interaction(), &focus);
        assert!(lifecycle.is_dirty(&app));
        assert_eq!(
            lifecycle.project_shell(&app).document().marker(),
            SessionDocumentMarker::Ready
        );
        settings_file_action(&mut app, SessionCommand::Open);
        lifecycle.advance(&mut app).unwrap();
        assert!(lifecycle.advance(&mut app).is_err());
        assert_eq!(app.capture_saved_session(), saved);
        assert_eq!(app.state().interaction(), &focus);
        assert_eq!(renderer.active_revision(), GraphRevision::INITIAL);
        assert_eq!(
            lifecycle.project_shell(&app).document().marker(),
            SessionDocumentMarker::Error
        );
        render(&mut renderer);
    }

    #[test]
    fn host_restore_busy_preserves_existing_candidate_and_activation() {
        let (mut app, mut renderer, mut lifecycle, _, submissions, baseline, _) = harness([]);
        let (_, target_state) = braids_state(-12.0);
        let target = SavedSession::capture(&target_state);
        lifecycle
            .request_host_restore(target.clone(), &app)
            .unwrap();
        let token = lifecycle.candidate.as_ref().unwrap().token;

        let error = lifecycle
            .request_host_restore(baseline.clone(), &app)
            .unwrap_err();
        assert_eq!(error.cause(), &SessionLifecycleCause::Busy);
        assert_eq!(lifecycle.stage(), SessionLifecycleStage::PreparingCandidate);
        assert_eq!(lifecycle.operation(), Some(SessionOperation::Open));
        assert_eq!(lifecycle.candidate.as_ref().unwrap().token, token);
        assert_eq!(lifecycle.failure(), None);
        assert_eq!(submissions.load(Ordering::SeqCst), 1);
        assert_eq!(app.capture_saved_session(), baseline);

        lifecycle.advance_hosted(&mut app).unwrap();
        assert_eq!(lifecycle.stage(), SessionLifecycleStage::ActivatingGraph);
        let error = lifecycle.request_host_restore(baseline, &app).unwrap_err();
        assert_eq!(error.cause(), &SessionLifecycleCause::Busy);
        assert_eq!(lifecycle.stage(), SessionLifecycleStage::ActivatingGraph);
        assert_eq!(lifecycle.activation.as_ref().unwrap().saved, target);
        assert_eq!(lifecycle.failure(), None);
        assert_eq!(submissions.load(Ordering::SeqCst), 1);

        render(&mut renderer);
        assert!(lifecycle
            .advance_hosted(&mut app)
            .unwrap()
            .graph_committed());
        assert_eq!(app.capture_saved_session(), target);
    }

    #[test]
    fn host_restore_reports_ready_only_after_render_graph_acknowledgement() {
        use crate::shell::hosted_synth::{HostedSynthControl, HostedSynthError};

        let (mut app, mut renderer, mut lifecycle, _, _, baseline, _) = harness([]);
        let (_, target_state) = braids_state(-9.0);
        let target = SavedSession::capture(&target_state);
        {
            let mut synth = HostedSynthControl::with_lifecycle(&mut app, &mut lifecycle);
            synth.request_restore(target.clone()).unwrap();
            assert!(!synth.replacement_status().unwrap());
            assert!(matches!(
                synth.capture_saved_session(),
                Err(HostedSynthError::CaptureBusy)
            ));
            assert!(matches!(
                synth.dispatch_action(
                    crate::control::SemanticAction::Adjust(Direction::Down),
                    crate::control::EventSource::Keyboard
                ),
                Err(HostedSynthError::EditsBlocked)
            ));
        }
        lifecycle.advance_hosted(&mut app).unwrap();
        assert_eq!(lifecycle.stage(), SessionLifecycleStage::ActivatingGraph);
        assert_eq!(renderer.active_revision(), GraphRevision::INITIAL);
        assert_eq!(app.capture_saved_session(), baseline);
        assert!(!lifecycle
            .advance_hosted(&mut app)
            .unwrap()
            .graph_committed());
        assert!(
            !HostedSynthControl::with_lifecycle(&mut app, &mut lifecycle)
                .replacement_status()
                .unwrap()
        );

        render(&mut renderer);
        assert_ne!(renderer.active_revision(), GraphRevision::INITIAL);
        assert_eq!(
            app.capture_saved_session(),
            baseline,
            "audio activation still needs control acknowledgement"
        );
        assert!(
            !HostedSynthControl::with_lifecycle(&mut app, &mut lifecycle)
                .replacement_status()
                .unwrap()
        );
        assert!(lifecycle
            .advance_hosted(&mut app)
            .unwrap()
            .graph_committed());
        assert_eq!(lifecycle.stage(), SessionLifecycleStage::Ready);
        assert_eq!(lifecycle.document().clean_baseline(), &target);
        let synth = HostedSynthControl::with_lifecycle(&mut app, &mut lifecycle);
        assert!(synth.replacement_status().unwrap());
        assert_eq!(synth.capture_saved_session().unwrap(), target);
    }

    #[test]
    fn host_restore_candidate_failure_preserves_prior_sound_and_accepts_retry() {
        use crate::adapter::sample_capability::SampleCapability;
        use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
        use crate::shell::hosted_synth::{HostedSynthControl, HostedSynthError};
        use crate::synth::AssetFileId;

        let (mut app, mut renderer, mut lifecycle, _, _, baseline, _) = harness([]);
        let prior_document = lifecycle.document().clone();
        let provider = SampleCapability::new(AssetFileId::new("unavailable.wav").unwrap()).unwrap();
        let registry = CapabilityRegistry::new(vec![provider.descriptor()]).unwrap();
        let mut candidate = AppState::new(registry, GlobalParameters::new(0.0).unwrap());
        candidate
            .apply(AppEvent::InstallPatches(vec![Patch::new(
                PatchId::new(1).unwrap(),
                "Unavailable".into(),
                provider.default_config().unwrap(),
                MidiChannel::new(0).unwrap(),
                PatchOutput::default(),
            )]))
            .unwrap();
        let note = MidiMessage::try_new(
            MidiChannel::new(0).unwrap(),
            MidiMessageKind::NoteOn,
            60,
            100,
        )
        .unwrap();
        app.dispatch(AppEvent::Midi {
            patch_id: PatchId::new(1).unwrap(),
            message: note,
        })
        .unwrap();
        render(&mut renderer);
        let prior_focus = app.state().interaction().clone();

        HostedSynthControl::with_lifecycle(&mut app, &mut lifecycle)
            .request_restore_json(
                SavedSession::capture(&candidate)
                    .to_json()
                    .unwrap()
                    .into_bytes(),
            )
            .unwrap();
        let error = lifecycle.advance_hosted(&mut app).unwrap_err();
        assert!(matches!(error.cause(), SessionLifecycleCause::Candidate(_)));
        assert_eq!(lifecycle.stage(), SessionLifecycleStage::Failed);
        assert_eq!(lifecycle.document(), &prior_document);
        assert_eq!(app.capture_saved_session(), baseline);
        assert_eq!(app.state().interaction(), &prior_focus);
        assert_eq!(renderer.active_revision(), GraphRevision::INITIAL);
        render(&mut renderer);
        assert!(renderer.active_patch_audio().stems()[0]
            .samples()
            .iter()
            .any(|sample| sample.abs() > f32::EPSILON));
        assert!(matches!(
            HostedSynthControl::with_lifecycle(&mut app, &mut lifecycle).replacement_status(),
            Err(HostedSynthError::Lifecycle(_))
        ));

        let (_, target_state) = braids_state(-6.0);
        let target = SavedSession::capture(&target_state);
        HostedSynthControl::with_lifecycle(&mut app, &mut lifecycle)
            .request_restore_json(target.to_json().unwrap().into_bytes())
            .unwrap();
        assert_eq!(lifecycle.failure(), None);
        lifecycle.advance_hosted(&mut app).unwrap();
        render(&mut renderer);
        assert!(lifecycle
            .advance_hosted(&mut app)
            .unwrap()
            .graph_committed());
        assert_eq!(app.capture_saved_session(), target);
    }

    #[test]
    fn host_restore_required_patches_validate_candidate_before_staging_and_allow_retry() {
        use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
        use crate::shell::hosted_synth::{HostedSynthControl, HostedSynthError};

        let (mut app, mut renderer, mut lifecycle, _, submissions, baseline, _) = harness([]);
        let prior_document = lifecycle.document().clone();
        let current_id = PatchId::new(1).unwrap();
        let candidate_id = PatchId::new(7).unwrap();
        let provider = BraidsCapability::new().unwrap();
        let registry = CapabilityRegistry::new(vec![provider.descriptor()]).unwrap();
        let mut target_state = AppState::new(registry, GlobalParameters::new(-9.0).unwrap());
        target_state
            .apply(AppEvent::InstallPatches(vec![Patch::new(
                candidate_id,
                "Replacement".into(),
                provider.default_config().unwrap(),
                MidiChannel::new(0).unwrap(),
                PatchOutput::default(),
            )]))
            .unwrap();
        let target = SavedSession::capture(&target_state);
        let bytes = target.to_json().unwrap().into_bytes();
        app.dispatch(AppEvent::Midi {
            patch_id: current_id,
            message: MidiMessage::try_new(
                MidiChannel::new(0).unwrap(),
                MidiMessageKind::NoteOn,
                60,
                100,
            )
            .unwrap(),
        })
        .unwrap();
        render(&mut renderer);
        let prior_focus = app.state().interaction().clone();

        // This ID exists in the active graph, but not in the prepared candidate.
        HostedSynthControl::with_lifecycle(&mut app, &mut lifecycle)
            .request_restore_json_requiring_patches(bytes.clone(), vec![current_id])
            .unwrap();
        assert_eq!(submissions.load(Ordering::SeqCst), 1);
        let error = lifecycle.advance_hosted(&mut app).unwrap_err();
        assert_eq!(
            error.cause(),
            &SessionLifecycleCause::MissingRequiredPatch(current_id)
        );
        assert_eq!(error.stage(), SessionLifecycleStage::PreparingCandidate);
        assert_eq!(lifecycle.stage(), SessionLifecycleStage::Failed);
        assert_eq!(lifecycle.document(), &prior_document);
        assert_eq!(app.capture_saved_session(), baseline);
        assert_eq!(app.state().interaction(), &prior_focus);
        assert!(!app.session_replacement_pending());
        assert!(matches!(
            HostedSynthControl::with_lifecycle(&mut app, &mut lifecycle).replacement_status(),
            Err(HostedSynthError::Lifecycle(_))
        ));
        render(&mut renderer);
        assert_eq!(renderer.active_revision(), GraphRevision::INITIAL);
        assert!(renderer.active_patch_audio().stems()[0]
            .samples()
            .iter()
            .any(|sample| sample.abs() > f32::EPSILON));

        // Required IDs may belong only to the candidate; commit still waits for audio.
        HostedSynthControl::with_lifecycle(&mut app, &mut lifecycle)
            .request_restore_json_requiring_patches(bytes, vec![candidate_id])
            .unwrap();
        assert_eq!(lifecycle.failure(), None);
        lifecycle.advance_hosted(&mut app).unwrap();
        assert_eq!(lifecycle.stage(), SessionLifecycleStage::ActivatingGraph);
        assert_eq!(app.capture_saved_session(), baseline);
        assert_eq!(renderer.active_revision(), GraphRevision::INITIAL);
        render(&mut renderer);
        assert!(lifecycle
            .advance_hosted(&mut app)
            .unwrap()
            .graph_committed());
        assert_eq!(app.capture_saved_session(), target);
        assert_eq!(lifecycle.document().clean_baseline(), &target);
    }

    #[test]
    fn host_restore_busy_cannot_replace_candidate_patch_requirements() {
        let (mut app, mut renderer, mut lifecycle, _, submissions, baseline, _) = harness([]);
        let required = PatchId::new(1).unwrap();
        let bytes = baseline.to_json().unwrap().into_bytes();
        lifecycle
            .request_host_restore_json_requiring_patches(bytes.clone(), vec![required], &app)
            .unwrap();
        let token = lifecycle.candidate.as_ref().unwrap().token;
        let error = lifecycle
            .request_host_restore_json_requiring_patches(
                bytes,
                vec![PatchId::new(7).unwrap()],
                &app,
            )
            .unwrap_err();
        assert_eq!(error.cause(), &SessionLifecycleCause::Busy);
        let context = lifecycle.candidate.as_ref().unwrap();
        assert_eq!(context.token, token);
        assert_eq!(context.required_patches, [required]);
        assert_eq!(submissions.load(Ordering::SeqCst), 1);
        assert_eq!(lifecycle.failure(), None);
        lifecycle.advance_hosted(&mut app).unwrap();
        render(&mut renderer);
        assert!(lifecycle
            .advance_hosted(&mut app)
            .unwrap()
            .graph_committed());
    }

    #[test]
    fn host_restore_json_decode_failure_is_worker_owned_and_preserves_sound() {
        use crate::shell::hosted_synth::HostedSynthControl;

        let (mut app, renderer, mut lifecycle, _, submissions, baseline, _) = harness([]);
        HostedSynthControl::with_lifecycle(&mut app, &mut lifecycle)
            .request_restore_json(vec![0xff])
            .unwrap();
        assert_eq!(lifecycle.stage(), SessionLifecycleStage::PreparingCandidate);
        assert_eq!(submissions.load(Ordering::SeqCst), 1);
        let error = lifecycle.advance_hosted(&mut app).unwrap_err();
        assert_eq!(
            error.cause(),
            &SessionLifecycleCause::Candidate(SessionCandidateFailure::DecodeEncoding)
        );
        assert_eq!(app.capture_saved_session(), baseline);
        assert_eq!(renderer.active_revision(), GraphRevision::INITIAL);
        assert!(!app.session_replacement_pending());
    }

    #[test]
    fn dirty_new_discard_uses_the_prepared_path_and_duplicate_request_does_not_interrupt_it() {
        let (mut app, mut renderer, mut lifecycle, _, submissions, default, _) =
            harness([AutoResponse::Choice(UnsavedChoice::Discard)]);
        app.dispatch(AppEvent::SelectContext(
            crate::control::TopLevelContext::Mixer,
        ))
        .unwrap();
        app.dispatch(AppEvent::SetInteractionMode(InteractionMode::Adjust))
            .unwrap();
        app.dispatch(AppEvent::Adjust(Direction::Down)).unwrap();
        let dirty = app.capture_saved_session();
        assert_ne!(dirty, default);
        lifecycle.request_new(&mut app).unwrap();
        assert_eq!(
            lifecycle.stage(),
            SessionLifecycleStage::AwaitingUnsavedChoice
        );
        assert!(!lifecycle.persisted_edits_blocked());
        assert!(matches!(
            lifecycle.request_open(&mut app),
            Err(SessionLifecycleError {
                cause: SessionLifecycleCause::Busy,
                ..
            })
        ));
        assert_eq!(
            lifecycle.stage(),
            SessionLifecycleStage::AwaitingUnsavedChoice
        );

        lifecycle.advance(&mut app).unwrap();
        assert_eq!(lifecycle.stage(), SessionLifecycleStage::ActivatingGraph);
        assert!(lifecycle.persisted_edits_blocked());
        assert_eq!(submissions.load(Ordering::SeqCst), 1);
        render(&mut renderer);
        assert!(lifecycle.advance(&mut app).unwrap().graph_committed());
        assert_eq!(app.capture_saved_session(), default);
        assert_eq!(lifecycle.document().identity(), &DocumentIdentity::Untitled);
        assert!(!lifecycle.is_dirty(&app));
        assert!(!lifecycle.persisted_edits_blocked());
        assert_eq!(
            app.state().interaction().patch_control_focus(),
            Some(PatchControlId::Engine)
        );
    }

    #[test]
    fn failed_open_preserves_prior_document_then_success_establishes_path_after_activation() {
        let bad = PathBuf::from("bad.crest");
        let good = PathBuf::from("good.crest");
        let (mut app, mut renderer, mut lifecycle, files, _, default, _) = harness([
            AutoResponse::File(bad.clone()),
            AutoResponse::File(good.clone()),
        ]);
        files.put(bad, vec![0xff]);
        let (_, target_state) = braids_state(-12.0);
        let target = SavedSession::capture(&target_state);
        files.put(good.clone(), target.to_json().unwrap().into_bytes());

        lifecycle.request_open(&mut app).unwrap();
        let error = lifecycle.advance(&mut app).unwrap_err();
        assert!(matches!(
            error.cause(),
            SessionLifecycleCause::Candidate(failure)
                if failure.kind() == crate::control::SessionCandidateFailureKind::Decode
        ));
        assert_eq!(app.capture_saved_session(), default);
        assert_eq!(renderer.active_revision(), GraphRevision::INITIAL);
        assert_eq!(lifecycle.document().identity(), &DocumentIdentity::Untitled);
        assert_eq!(error.operation(), SessionOperation::Open);
        assert_eq!(error.stage(), SessionLifecycleStage::PreparingCandidate);
        let failed = lifecycle.project_shell(&app);
        assert_eq!(failed.document().marker(), SessionDocumentMarker::Error);
        assert_eq!(
            failed.document().status(),
            "FAILED — PRIOR SESSION UNCHANGED"
        );
        assert!(failed
            .document()
            .failure()
            .unwrap()
            .contains("not UTF-8 JSON"));
        lifecycle.advance(&mut app).unwrap();
        assert_eq!(lifecycle.failure(), Some(&error));

        lifecycle.dismiss_failure();
        lifecycle.request_open(&mut app).unwrap();
        lifecycle.advance(&mut app).unwrap();
        assert_eq!(lifecycle.document().identity(), &DocumentIdentity::Untitled);
        render(&mut renderer);
        assert!(lifecycle.advance(&mut app).unwrap().graph_committed());
        assert_eq!(app.capture_saved_session(), target);
        assert_eq!(
            lifecycle.document().identity(),
            &DocumentIdentity::Path(good)
        );
        assert!(!lifecycle.is_dirty(&app));
    }

    #[test]
    fn save_as_identity_waits_for_success_and_post_capture_edit_remains_dirty() {
        let destination = PathBuf::from("saved.crest");
        let (mut app, _renderer, mut lifecycle, files, _, _, _) =
            harness([AutoResponse::File(destination.clone())]);
        app.dispatch(AppEvent::SelectContext(
            crate::control::TopLevelContext::Mixer,
        ))
        .unwrap();
        app.dispatch(AppEvent::SetInteractionMode(InteractionMode::Adjust))
            .unwrap();
        app.dispatch(AppEvent::Adjust(Direction::Down)).unwrap();
        let written = app.capture_saved_session();
        lifecycle.request_save(&app).unwrap();
        lifecycle.advance(&mut app).unwrap();
        assert_eq!(lifecycle.document().identity(), &DocumentIdentity::Untitled);
        app.dispatch(AppEvent::Adjust(Direction::Down)).unwrap();
        assert!(lifecycle.advance(&mut app).unwrap().save_completed());
        assert_eq!(
            lifecycle.document().identity(),
            &DocumentIdentity::Path(destination.clone())
        );
        let bytes = files.read(&destination).unwrap();
        let decoded =
            SavedSession::from_json(&String::from_utf8(bytes).unwrap(), app.capabilities())
                .unwrap();
        assert_eq!(decoded, written);
        assert!(lifecycle.is_dirty(&app));
    }

    #[test]
    fn implicit_creation_save_close_and_fresh_open_persist_only_acknowledged_patches() {
        let destination = PathBuf::from("implicit-created.crest");
        let (mut app, mut renderer, mut lifecycle, files, _, baseline, graph_worker) =
            harness([AutoResponse::File(destination.clone())]);

        app.dispatch(AppEvent::SelectContext(
            crate::control::TopLevelContext::Patch,
        ))
        .unwrap();
        app.dispatch(AppEvent::SelectPatch(Direction::Right))
            .unwrap();
        app.dispatch(AppEvent::SetInteractionMode(InteractionMode::Adjust))
            .unwrap();
        app.dispatch(AppEvent::OpenPopover).unwrap();
        app.dispatch(AppEvent::Activate).unwrap();
        assert_eq!(app.patches().len(), 1);
        assert_eq!(app.capture_saved_session(), baseline);

        // Save As captures the acknowledged set while append preparation is
        // pending. The worker and graph may advance independently, but the
        // written baseline cannot contain the candidate.
        lifecycle.request_save(&app).unwrap();
        lifecycle.advance(&mut app).unwrap();
        assert_eq!(lifecycle.stage(), SessionLifecycleStage::WritingFile);
        assert_eq!(
            app.advance_structural()
                .unwrap()
                .engine_selection_lifecycle_advanced(),
            Some(crate::control::EngineSelectionStatusKind::Validating)
        );
        assert_eq!(
            app.advance_structural()
                .unwrap()
                .engine_selection_lifecycle_advanced(),
            Some(crate::control::EngineSelectionStatusKind::Preparing)
        );
        assert!(graph_worker.advance());
        let prepared = app.advance_structural().unwrap();
        assert_eq!(
            prepared.graph_stage(),
            Some(crate::real_time::GraphStageOutcome::Staged)
        );
        assert_eq!(app.patches().len(), 1);
        assert!(lifecycle.advance(&mut app).unwrap().save_completed());
        let first_written = files.read(&destination).unwrap();
        let first_saved = SavedSession::from_json(
            &String::from_utf8(first_written).unwrap(),
            app.capabilities(),
        )
        .unwrap();
        assert_eq!(first_saved, baseline);
        assert!(!lifecycle.is_dirty(&app));

        render(&mut renderer);
        assert!(app
            .advance_structural()
            .unwrap()
            .activation_acknowledged()
            .is_some());
        let created = app.capture_saved_session();
        assert_eq!(app.patches().len(), 2);
        assert_ne!(created, baseline);
        assert!(lifecycle.is_dirty(&app));

        // A later Save writes the committed Patch and establishes that exact
        // capture as the clean baseline.
        lifecycle.request_save(&app).unwrap();
        assert!(!lifecycle.advance(&mut app).unwrap().save_completed());
        assert!(lifecycle.advance(&mut app).unwrap().save_completed());
        assert!(!lifecycle.is_dirty(&app));
        let created_bytes = files.read(&destination).unwrap();
        let created_json = String::from_utf8(created_bytes.clone()).unwrap();
        assert!(!created_json.contains("trailingEmpty"));
        assert_eq!(
            SavedSession::from_json(&created_json, app.capabilities()).unwrap(),
            created
        );

        lifecycle.request_close(&mut app).unwrap();
        assert!(lifecycle.advance(&mut app).unwrap().close_approved());

        // A fresh application composition opens the saved bytes, activates
        // the complete graph, resets focus per Phase 01, and derives a new
        // trailing empty position after the restored created set.
        let (
            mut reopened_app,
            mut reopened_renderer,
            mut reopened_lifecycle,
            reopened_files,
            _,
            _,
            _,
        ) = harness([AutoResponse::File(destination.clone())]);
        reopened_files.put(destination.clone(), created_bytes);
        reopened_lifecycle.request_open(&mut reopened_app).unwrap();
        reopened_lifecycle.advance(&mut reopened_app).unwrap();
        assert_eq!(
            reopened_lifecycle.stage(),
            SessionLifecycleStage::ActivatingGraph
        );
        render(&mut reopened_renderer);
        assert!(reopened_lifecycle
            .advance(&mut reopened_app)
            .unwrap()
            .graph_committed());
        assert_eq!(reopened_app.capture_saved_session(), created);
        assert_eq!(reopened_app.patches().len(), 2);
        assert_eq!(
            reopened_app.state().interaction().patch_control_focus(),
            Some(PatchControlId::Engine)
        );
        reopened_app
            .dispatch(AppEvent::SelectContext(
                crate::control::TopLevelContext::Patch,
            ))
            .unwrap();
        reopened_app
            .dispatch(AppEvent::SelectPatch(Direction::Right))
            .unwrap();
        reopened_app
            .dispatch(AppEvent::SelectPatch(Direction::Right))
            .unwrap();
        assert_eq!(
            reopened_app.state().interaction().patch_position_focus(),
            Some(crate::control::PatchPositionId::TrailingEmpty)
        );
    }

    #[test]
    fn exact_capacity_open_activates_and_renders_created_patches_only() {
        let source = PathBuf::from("capacity.crest");
        let provider = BraidsCapability::new().unwrap();
        let registry = CapabilityRegistry::new(vec![provider.descriptor()]).unwrap();
        let patches = (0..crate::kernel::MAX_ACTIVE_PATCHES)
            .map(|index| {
                Patch::new(
                    PatchId::new(index as u32 + 1).unwrap(),
                    format!("Capacity {}", index + 1),
                    provider.default_config().unwrap(),
                    MidiChannel::new((index % 16) as u8).unwrap(),
                    PatchOutput::to_track(MixerTrackId::new((index % 16) as u8).unwrap()),
                )
            })
            .collect();
        let mut target_state = AppState::new(registry, GlobalParameters::new(0.0).unwrap());
        target_state
            .apply(AppEvent::InstallPatches(patches))
            .unwrap();
        let target = SavedSession::capture(&target_state);
        let target_json = target.to_json().unwrap();
        assert!(!target_json.contains("trailingEmpty"));

        let (mut app, mut renderer, mut lifecycle, files, _, _, _) =
            harness([AutoResponse::File(source.clone())]);
        files.put(source, target_json.into_bytes());
        lifecycle.request_open(&mut app).unwrap();
        lifecycle.advance(&mut app).unwrap();
        assert_eq!(lifecycle.stage(), SessionLifecycleStage::ActivatingGraph);
        render(&mut renderer);
        assert!(lifecycle.advance(&mut app).unwrap().graph_committed());
        assert_eq!(app.capture_saved_session(), target);
        assert_eq!(app.patches().len(), crate::kernel::MAX_ACTIVE_PATCHES);
        assert_eq!(
            renderer.parameters().patch_count(),
            crate::kernel::MAX_ACTIVE_PATCHES
        );

        let patch_ids = app.patches().iter().map(Patch::id).collect::<Vec<_>>();
        for (index, patch_id) in patch_ids.into_iter().enumerate() {
            let note = crate::kernel::midi_message::MidiMessage::try_new(
                MidiChannel::new((index % 16) as u8).unwrap(),
                crate::kernel::midi_message::MidiMessageKind::NoteOn,
                48 + index as u8,
                100,
            )
            .unwrap();
            app.dispatch(AppEvent::Midi {
                patch_id,
                message: note,
            })
            .unwrap();
            render(&mut renderer);
        }
        render(&mut renderer);
        assert_eq!(
            renderer.active_patch_audio().stems().len(),
            crate::kernel::MAX_ACTIVE_PATCHES
        );
        assert!(renderer.active_patch_audio().stems().iter().all(|stem| stem
            .samples()
            .iter()
            .any(|sample| sample.abs() > f32::EPSILON)));

        app.dispatch(AppEvent::SelectContext(
            crate::control::TopLevelContext::Patch,
        ))
        .unwrap();
        for _ in 0..crate::kernel::MAX_ACTIVE_PATCHES {
            app.dispatch(AppEvent::SelectPatch(Direction::Right))
                .unwrap();
        }
        let page = app.current_patch_page().unwrap();
        assert!(page.patch().is_empty());
        assert!(!page.patch().creation_available());
        assert_eq!(app.patches().len(), crate::kernel::MAX_ACTIVE_PATCHES);
    }

    #[test]
    fn pending_creation_visibly_refuses_session_replacement_and_remains_recoverable() {
        let (mut app, mut renderer, mut lifecycle, _, submissions, baseline, graph_worker) =
            harness([]);
        app.dispatch(AppEvent::SelectContext(
            crate::control::TopLevelContext::Patch,
        ))
        .unwrap();
        app.dispatch(AppEvent::SelectPatch(Direction::Right))
            .unwrap();
        app.dispatch(AppEvent::SetInteractionMode(InteractionMode::Adjust))
            .unwrap();
        app.dispatch(AppEvent::OpenPopover).unwrap();
        app.dispatch(AppEvent::Activate).unwrap();

        let error = lifecycle.request_new(&mut app).unwrap_err();
        assert!(matches!(
            error.cause(),
            SessionLifecycleCause::Structural(crate::control::StructuralAdvanceError::SessionBusy)
        ));
        assert_eq!(error.operation(), SessionOperation::New);
        assert_eq!(submissions.load(Ordering::SeqCst), 0);
        assert_eq!(app.capture_saved_session(), baseline);
        assert_eq!(app.patches().len(), 1);
        assert_eq!(
            app.state().interaction().patch_position_focus(),
            Some(crate::control::PatchPositionId::TrailingEmpty)
        );
        assert!(app.state().pending_patch_creation().is_some());

        lifecycle.dismiss_failure();
        for expected in [
            crate::control::EngineSelectionStatusKind::Validating,
            crate::control::EngineSelectionStatusKind::Preparing,
        ] {
            assert_eq!(
                app.advance_structural()
                    .unwrap()
                    .engine_selection_lifecycle_advanced(),
                Some(expected)
            );
        }
        assert!(graph_worker.advance());
        assert_eq!(
            app.advance_structural().unwrap().graph_stage(),
            Some(crate::real_time::GraphStageOutcome::Staged)
        );
        render(&mut renderer);
        assert!(app
            .advance_structural()
            .unwrap()
            .activation_acknowledged()
            .is_some());
        assert_eq!(app.patches().len(), 2);
    }

    #[test]
    fn dirty_guard_cancel_aborts_and_save_resumes_new_exactly_once_after_success() {
        let destination = PathBuf::from("guard-save.crest");
        let (mut app, mut renderer, mut lifecycle, _, submissions, default, _) = harness([
            AutoResponse::Choice(UnsavedChoice::Cancel),
            AutoResponse::Choice(UnsavedChoice::Save),
            AutoResponse::File(destination),
        ]);
        app.dispatch(AppEvent::SetInteractionMode(InteractionMode::Adjust))
            .unwrap();
        app.dispatch(AppEvent::Adjust(Direction::Down)).unwrap();
        let dirty = app.capture_saved_session();
        lifecycle.request_new(&mut app).unwrap();
        lifecycle.advance(&mut app).unwrap();
        assert_eq!(app.capture_saved_session(), dirty);
        assert!(lifecycle.is_dirty(&app));
        assert_eq!(submissions.load(Ordering::SeqCst), 0);

        lifecycle.request_new(&mut app).unwrap();
        lifecycle.advance(&mut app).unwrap();
        assert_eq!(
            lifecycle.stage(),
            SessionLifecycleStage::SelectingSaveDestination
        );
        lifecycle.advance(&mut app).unwrap();
        assert_eq!(lifecycle.stage(), SessionLifecycleStage::WritingFile);
        lifecycle.advance(&mut app).unwrap();
        assert_eq!(lifecycle.stage(), SessionLifecycleStage::PreparingCandidate);
        lifecycle.advance(&mut app).unwrap();
        assert_eq!(lifecycle.stage(), SessionLifecycleStage::ActivatingGraph);
        assert_eq!(submissions.load(Ordering::SeqCst), 1);
        render(&mut renderer);
        assert!(lifecycle.advance(&mut app).unwrap().graph_committed());
        assert_eq!(submissions.load(Ordering::SeqCst), 1);
        assert_eq!(app.capture_saved_session(), default);
        assert_eq!(lifecycle.document().identity(), &DocumentIdentity::Untitled);
    }

    #[test]
    fn dialog_cancellation_is_a_non_error_noop_for_open_save_as_and_dirty_close() {
        let (mut app, _renderer, mut lifecycle, _, submissions, baseline, _) = harness([
            AutoResponse::Cancel,
            AutoResponse::Cancel,
            AutoResponse::Choice(UnsavedChoice::Cancel),
        ]);
        let tree = app.current_state_tree();
        lifecycle.request_open(&mut app).unwrap();
        assert!(lifecycle.persisted_edits_blocked());
        lifecycle.advance(&mut app).unwrap();
        assert_eq!(lifecycle.stage(), SessionLifecycleStage::Ready);
        assert!(!lifecycle.persisted_edits_blocked());
        assert!(lifecycle.failure().is_none());
        assert_eq!(app.current_state_tree(), tree);

        lifecycle.request_save_as(&app).unwrap();
        assert!(!lifecycle.persisted_edits_blocked());
        lifecycle.advance(&mut app).unwrap();
        assert_eq!(lifecycle.stage(), SessionLifecycleStage::Ready);
        assert!(lifecycle.failure().is_none());
        assert_eq!(lifecycle.document().identity(), &DocumentIdentity::Untitled);

        app.dispatch(AppEvent::SetInteractionMode(InteractionMode::Adjust))
            .unwrap();
        app.dispatch(AppEvent::Adjust(Direction::Down)).unwrap();
        let dirty = app.capture_saved_session();
        assert_ne!(dirty, baseline);
        lifecycle.request_close(&mut app).unwrap();
        let progress = lifecycle.advance(&mut app).unwrap();
        assert!(!progress.close_approved());
        assert!(lifecycle.failure().is_none());
        assert_eq!(app.capture_saved_session(), dirty);
        assert!(lifecycle.is_dirty(&app));
        assert_eq!(submissions.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn save_failure_keeps_identity_baseline_dirty_work_and_aborts_guard_continuation() {
        let destination = PathBuf::from("failed-save.crest");
        let (mut app, _renderer, mut lifecycle, files, submissions, baseline, _) = harness([
            AutoResponse::Choice(UnsavedChoice::Save),
            AutoResponse::File(destination.clone()),
        ]);
        app.dispatch(AppEvent::SetInteractionMode(InteractionMode::Adjust))
            .unwrap();
        app.dispatch(AppEvent::Adjust(Direction::Down)).unwrap();
        let dirty = app.capture_saved_session();
        files.set_fail_write(true);

        lifecycle.request_new(&mut app).unwrap();
        lifecycle.advance(&mut app).unwrap();
        assert_eq!(
            lifecycle.stage(),
            SessionLifecycleStage::SelectingSaveDestination
        );
        lifecycle.advance(&mut app).unwrap();
        assert_eq!(lifecycle.stage(), SessionLifecycleStage::WritingFile);
        let error = lifecycle.advance(&mut app).unwrap_err();
        assert_eq!(error.operation(), SessionOperation::SaveAs);
        assert_eq!(error.stage(), SessionLifecycleStage::WritingFile);
        assert!(matches!(
            error.cause(),
            SessionLifecycleCause::Save(SessionSaveFailure::File(failure))
                if failure.kind() == SessionFileFailureKind::WriteTemporary
        ));
        assert_eq!(lifecycle.document().identity(), &DocumentIdentity::Untitled);
        assert_eq!(lifecycle.document().clean_baseline(), &baseline);
        assert_eq!(app.capture_saved_session(), dirty);
        assert!(lifecycle.is_dirty(&app));
        assert!(files.read(&destination).is_err());
        assert_eq!(submissions.load(Ordering::SeqCst), 0);
        assert!(lifecycle.pending_continuation().is_none());
        let projection = lifecycle.project_shell(&app);
        assert_eq!(projection.document().marker(), SessionDocumentMarker::Error);
        assert!(projection
            .document()
            .failure()
            .unwrap()
            .contains("WriteTemporary"));
    }

    #[test]
    fn read_and_encode_failures_keep_exact_operation_stage_and_prior_document() {
        let missing = PathBuf::from("missing.crest");
        let destination = PathBuf::from("encode-failed.crest");
        let (mut app, _renderer, mut lifecycle, files, _, baseline, _) = harness([
            AutoResponse::File(missing),
            AutoResponse::File(destination.clone()),
        ]);
        let tree = app.current_state_tree();
        lifecycle.request_open(&mut app).unwrap();
        let read = lifecycle.advance(&mut app).unwrap_err();
        assert_eq!(read.operation(), SessionOperation::Open);
        assert_eq!(read.stage(), SessionLifecycleStage::ReadingFile);
        assert!(matches!(
            read.cause(),
            SessionLifecycleCause::File(failure)
                if failure.kind() == SessionFileFailureKind::Read
        ));
        assert_eq!(app.current_state_tree(), tree);
        assert_eq!(lifecycle.document().clean_baseline(), &baseline);

        lifecycle.dismiss_failure();
        lifecycle.save_worker = Box::new(ForcedEncodeSaveWorker { pending: None });
        lifecycle.request_save_as(&app).unwrap();
        let encode = lifecycle.advance(&mut app).unwrap_err();
        assert_eq!(encode.operation(), SessionOperation::SaveAs);
        assert_eq!(encode.stage(), SessionLifecycleStage::WritingFile);
        assert!(matches!(
            encode.cause(),
            SessionLifecycleCause::Save(SessionSaveFailure::Encode(
                crate::control::SavedSessionError::Encode
            ))
        ));
        assert_eq!(lifecycle.document().identity(), &DocumentIdentity::Untitled);
        assert_eq!(lifecycle.document().clean_baseline(), &baseline);
        assert_eq!(app.current_state_tree(), tree);
        assert!(files.read(&destination).is_err());
    }

    #[test]
    fn document_projection_is_path_free_explicit_and_focus_neutral() {
        let (app, _renderer, lifecycle, _, _, baseline, _) = harness([]);
        let focus = app.state().interaction().focus_path().clone();
        let projection = lifecycle.project_shell(&app);
        assert_eq!(projection.document().name(), "Untitled");
        assert!(!projection.document().dirty());
        assert_eq!(projection.document().marker(), SessionDocumentMarker::Ready);
        assert_eq!(projection.document().status(), "READY");
        assert_eq!(app.state().interaction().focus_path(), &focus);
        assert_eq!(app.capture_saved_session(), baseline);
        let json = serde_json::to_string(projection.document()).unwrap();
        assert!(!json.contains("/"));
        assert!(!json.contains("path"));
    }

    #[test]
    fn threaded_production_boundary_matrix_runs_new_open_save_save_as_and_close() {
        crate::real_time::callback_safety::reset_callback_safety_counts();
        let (registry, mut state) = braids_state(0.0);
        state
            .apply(AppEvent::SelectContext(
                crate::control::TopLevelContext::Patch,
            ))
            .unwrap();
        let default = SavedSession::capture(&state);
        let audio_config = AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 64).unwrap();
        let parameters = crate::control::StateProjector::for_graph(GraphRevision::INITIAL)
            .project(&state)
            .unwrap()
            .2;
        let initial_preparers: Vec<Box<dyn InstrumentPreparer>> =
            vec![Box::new(BraidsPreparer::new().unwrap())];
        let initial_graph = PreparedGraphBuilder::new(&registry, &initial_preparers)
            .build(
                GraphRevision::INITIAL,
                state.patches(),
                parameters.clone(),
                audio_config.sample_rate(),
                audio_config.render_capacity_frames(),
            )
            .unwrap();
        let (audio_control, audio_handle) =
            LockFreeAudioBoundary::new(16, parameters).into_handles();
        let (structural_control, structural_audio) = LockFreeStructuralGraphBoundary::new(
            1,
            1,
            GraphHandoffStatus::with_active(GraphRevision::INITIAL),
        )
        .unwrap()
        .into_handles();
        let graph_worker = DeterministicGraphPreparationWorker::new(
            registry.clone(),
            vec![Box::new(BraidsPreparer::new().unwrap())],
            audio_config,
        );
        let mut app = AppLoop::new(
            state,
            crate::control::StateProjector::for_graph(GraphRevision::INITIAL),
            audio_control,
        )
        .unwrap();
        app.configure_engine_selection(
            DescriptorDefaultConfigFactory::new(
                registry.clone(),
                vec![Box::new(BraidsCapability::new().unwrap())],
            ),
            graph_worker,
            structural_control,
            &initial_graph,
            audio_config,
        )
        .unwrap();
        let mut renderer = AudioRenderer::new(audio_handle, structural_audio, initial_graph);

        let missing_path = PathBuf::from("threaded-missing.crest");
        let open_path = PathBuf::from("threaded-open.crest");
        let save_as_path = PathBuf::from("threaded-save-as.crest");
        let files = Arc::new(MemoryFiles::default());
        let (_, target_state) = braids_state(-9.0);
        let target = SavedSession::capture(&target_state);
        files.put(open_path.clone(), target.to_json().unwrap().into_bytes());
        let native_dialogs = NativeSessionDialogBridge::default();
        let mut responses = VecDeque::from([
            AutoResponse::Choice(UnsavedChoice::Cancel),
            AutoResponse::Choice(UnsavedChoice::Discard),
            AutoResponse::File(missing_path),
            AutoResponse::File(open_path.clone()),
            AutoResponse::File(save_as_path.clone()),
            AutoResponse::Choice(UnsavedChoice::Cancel),
            AutoResponse::Choice(UnsavedChoice::Save),
            AutoResponse::Choice(UnsavedChoice::Discard),
        ]);
        let candidate = ThreadedSessionCandidateWorker::new(
            registry,
            vec![Box::new(BraidsPreparer::new().unwrap())],
            EffectCapabilityRegistry::default(),
            vec![],
            audio_config,
        )
        .unwrap();
        let file_port: Arc<dyn SessionFilePort> = files.clone();
        let save = crate::shell::ThreadedSessionSaveWorker::new(Arc::clone(&file_port)).unwrap();
        let mut lifecycle = SessionLifecycleCoordinator::new(
            default.clone(),
            Box::new(native_dialogs.clone()),
            file_port,
            Box::new(candidate),
            Box::new(save),
        );

        let menu = crate::shell::webview::window::native_file_menu_items();
        assert_eq!(menu.len(), 5);
        assert_eq!(
            menu.iter()
                .map(|item| (item.id(), item.accelerator(), item.command()))
                .collect::<Vec<_>>(),
            vec![
                (
                    "session.new",
                    "CmdOrCtrl+N",
                    crate::shell::app_window::SessionCommand::New,
                ),
                (
                    "session.open",
                    "CmdOrCtrl+O",
                    crate::shell::app_window::SessionCommand::Open,
                ),
                (
                    "session.save",
                    "CmdOrCtrl+S",
                    crate::shell::app_window::SessionCommand::Save,
                ),
                (
                    "session.save_as",
                    "CmdOrCtrl+Shift+S",
                    crate::shell::app_window::SessionCommand::SaveAs,
                ),
                (
                    "session.close",
                    "CmdOrCtrl+W",
                    crate::shell::app_window::SessionCommand::Close,
                ),
            ]
        );

        app.dispatch(AppEvent::SelectContext(
            crate::control::TopLevelContext::Mixer,
        ))
        .unwrap();
        app.dispatch(AppEvent::SetInteractionMode(InteractionMode::Adjust))
            .unwrap();
        app.dispatch(AppEvent::Adjust(Direction::Down)).unwrap();
        let dirty_before_cancel = app.capture_saved_session();
        request_native_menu("session.new", &mut lifecycle, &mut app).unwrap();
        service_native_dialog(&native_dialogs, &mut responses);
        assert!(!lifecycle.advance(&mut app).unwrap().close_approved());
        assert_eq!(app.capture_saved_session(), dirty_before_cancel);
        assert!(lifecycle.is_dirty(&app));

        request_native_menu("session.new", &mut lifecycle, &mut app).unwrap();
        drive_to_idle(
            &mut lifecycle,
            &mut app,
            &mut renderer,
            &native_dialogs,
            &mut responses,
        );
        assert_eq!(app.capture_saved_session(), default);
        assert_eq!(lifecycle.document().identity(), &DocumentIdentity::Untitled);
        assert!(!lifecycle.is_dirty(&app));
        assert_eq!(
            app.state().interaction().patch_control_focus(),
            Some(PatchControlId::Engine)
        );

        let before_failed_open = app.capture_saved_session();
        let before_failed_open_revision = renderer.active_revision();
        request_native_menu("session.open", &mut lifecycle, &mut app).unwrap();
        service_native_dialog(&native_dialogs, &mut responses);
        let failure = lifecycle.advance(&mut app).unwrap_err();
        assert_eq!(failure.operation(), SessionOperation::Open);
        assert_eq!(failure.stage(), SessionLifecycleStage::ReadingFile);
        assert_eq!(app.capture_saved_session(), before_failed_open);
        assert_eq!(renderer.active_revision(), before_failed_open_revision);
        let failure_projection = lifecycle.project_shell(&app);
        assert_eq!(
            failure_projection.document().marker(),
            SessionDocumentMarker::Error
        );
        assert!(failure_projection.document().failure().is_some());
        lifecycle.dismiss_failure();

        request_native_menu("session.open", &mut lifecycle, &mut app).unwrap();
        drive_to_idle(
            &mut lifecycle,
            &mut app,
            &mut renderer,
            &native_dialogs,
            &mut responses,
        );
        assert_eq!(app.capture_saved_session(), target);
        assert_eq!(
            lifecycle.document().identity(),
            &DocumentIdentity::Path(open_path.clone())
        );
        assert_eq!(renderer.active_revision(), app.graph_revision());

        app.dispatch(AppEvent::SelectContext(
            crate::control::TopLevelContext::Mixer,
        ))
        .unwrap();
        app.dispatch(AppEvent::SetInteractionMode(InteractionMode::Adjust))
            .unwrap();
        app.dispatch(AppEvent::Adjust(Direction::Down)).unwrap();
        let saved_to_existing = app.capture_saved_session();
        request_native_menu("session.save", &mut lifecycle, &mut app).unwrap();
        drive_to_idle(
            &mut lifecycle,
            &mut app,
            &mut renderer,
            &native_dialogs,
            &mut responses,
        );
        assert_eq!(
            SavedSession::from_json(
                &String::from_utf8(files.read(&open_path).unwrap()).unwrap(),
                app.capabilities(),
            )
            .unwrap(),
            saved_to_existing
        );
        assert!(!lifecycle.is_dirty(&app));

        request_native_menu("session.save_as", &mut lifecycle, &mut app).unwrap();
        drive_to_idle(
            &mut lifecycle,
            &mut app,
            &mut renderer,
            &native_dialogs,
            &mut responses,
        );
        assert_eq!(
            lifecycle.document().identity(),
            &DocumentIdentity::Path(save_as_path.clone())
        );
        assert_eq!(
            files.read(&save_as_path).unwrap(),
            files.read(&open_path).unwrap()
        );
        assert!(files.read(&open_path).is_ok());

        app.dispatch(AppEvent::SelectContext(
            crate::control::TopLevelContext::Mixer,
        ))
        .unwrap();
        app.dispatch(AppEvent::SetInteractionMode(InteractionMode::Adjust))
            .unwrap();
        app.dispatch(AppEvent::Adjust(Direction::Down)).unwrap();
        request_native_menu("session.close", &mut lifecycle, &mut app).unwrap();
        service_native_dialog(&native_dialogs, &mut responses);
        assert!(!lifecycle.advance(&mut app).unwrap().close_approved());
        assert!(lifecycle.is_dirty(&app));

        request_native_menu("session.close", &mut lifecycle, &mut app).unwrap();
        drive_to_close_approval(
            &mut lifecycle,
            &mut app,
            &mut renderer,
            &native_dialogs,
            &mut responses,
        );
        assert!(!lifecycle.is_dirty(&app));
        assert_eq!(
            SavedSession::from_json(
                &String::from_utf8(files.read(&save_as_path).unwrap()).unwrap(),
                app.capabilities(),
            )
            .unwrap(),
            app.capture_saved_session()
        );

        app.dispatch(AppEvent::Adjust(Direction::Down)).unwrap();
        assert!(lifecycle.is_dirty(&app));
        request_native_menu("session.close", &mut lifecycle, &mut app).unwrap();
        drive_to_close_approval(
            &mut lifecycle,
            &mut app,
            &mut renderer,
            &native_dialogs,
            &mut responses,
        );
        assert!(lifecycle.is_dirty(&app));
        assert!(responses.is_empty());

        let callback_safety = crate::real_time::callback_safety::callback_safety_snapshot();
        assert_eq!(callback_safety.allocations(), 0);
        assert_eq!(callback_safety.destructions(), 0);
        lifecycle.shutdown_on_control().unwrap();
        drop(renderer);
        app.shutdown_engine_selection_on_control().unwrap();
        assert_eq!(app.owned_structural_graphs_on_control(), 0);

        let report = NativeSessionLifecycleHandoffReport::from_evidence(
            NativeSessionLifecycleHandoffEvidence {
                menu_shortcuts_available: true,
                new_cancelled: true,
                save_as_completed: true,
                open_round_trip_completed: true,
                dirty_close_cancelled: true,
                dirty_close_saved: true,
                dirty_close_discarded: true,
                controlled_failure_visible: true,
                clean_teardown: true,
            },
        );
        assert!(report.complete());
        assert!(serde_json::to_string(&report)
            .unwrap()
            .contains("completed"));
    }
}
