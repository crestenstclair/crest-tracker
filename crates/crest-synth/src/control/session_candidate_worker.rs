use crate::control::{
    PreparedSavedSession, SavedSession, SavedSessionError, SavedSessionRestoreError,
};
use crate::real_time::{GraphPreparationError, GraphRevision, WorkerShutdownError};
use crate::synth::{InstrumentPreparationError, RackPreparationError};
use core::fmt;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionCandidateToken(u64);

impl SessionCandidateToken {
    pub fn new(value: u64) -> Result<Self, SessionCandidateTokenError> {
        (value != 0)
            .then_some(Self(value))
            .ok_or(SessionCandidateTokenError)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("session candidate token must be non-zero")]
pub struct SessionCandidateTokenError;

/// Immutable worker input. Open carries bytes, never a path; New carries the
/// captured default through the exact same preparation branch.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionCandidateSource {
    Default(Box<SavedSession>),
    OpenedBytes(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionCandidateRequest {
    token: SessionCandidateToken,
    source: SessionCandidateSource,
    target_graph_revision: GraphRevision,
}

impl SessionCandidateRequest {
    pub const fn new(
        token: SessionCandidateToken,
        source: SessionCandidateSource,
        target_graph_revision: GraphRevision,
    ) -> Self {
        Self {
            token,
            source,
            target_graph_revision,
        }
    }

    pub const fn token(&self) -> SessionCandidateToken {
        self.token
    }

    pub const fn source(&self) -> &SessionCandidateSource {
        &self.source
    }

    pub const fn target_graph_revision(&self) -> GraphRevision {
        self.target_graph_revision
    }

    pub fn into_parts(self) -> (SessionCandidateToken, SessionCandidateSource, GraphRevision) {
        (self.token, self.source, self.target_graph_revision)
    }
}

pub enum SessionCandidateResult {
    Prepared {
        token: SessionCandidateToken,
        saved: Box<SavedSession>,
        prepared: Box<PreparedSavedSession>,
    },
    Failed {
        token: SessionCandidateToken,
        failure: SessionCandidateFailure,
    },
}

impl SessionCandidateResult {
    pub const fn token(&self) -> SessionCandidateToken {
        match self {
            Self::Prepared { token, .. } | Self::Failed { token, .. } => *token,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionCandidateFailureKind {
    Decode,
    Version,
    Shape,
    Validation,
    Capability,
    Asset,
    Projection,
    Preparation,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SessionCandidateFailure {
    #[error("opened session bytes are not UTF-8 JSON")]
    DecodeEncoding,
    #[error("saved session decode, migration, or validation failed: {0}")]
    Saved(SavedSessionError),
    #[error("saved session candidate preparation failed: {0}")]
    Restore(SavedSessionRestoreError),
}

impl SessionCandidateFailure {
    pub fn kind(&self) -> SessionCandidateFailureKind {
        match self {
            Self::DecodeEncoding | Self::Saved(SavedSessionError::Decode) => {
                SessionCandidateFailureKind::Decode
            }
            Self::Saved(SavedSessionError::UnsupportedVersion(_)) => {
                SessionCandidateFailureKind::Version
            }
            Self::Saved(
                SavedSessionError::InvalidShape
                | SavedSessionError::InvalidPatch
                | SavedSessionError::InvalidPatchSend
                | SavedSessionError::InvalidReturn
                | SavedSessionError::InvalidGlobal
                | SavedSessionError::InvalidVoiceLimit,
            ) => SessionCandidateFailureKind::Shape,
            Self::Saved(
                SavedSessionError::InvalidCapability | SavedSessionError::InvalidEffect,
            ) => SessionCandidateFailureKind::Capability,
            Self::Saved(SavedSessionError::Encode) => SessionCandidateFailureKind::Validation,
            Self::Restore(SavedSessionRestoreError::Session(error)) => Self::Saved(*error).kind(),
            Self::Restore(SavedSessionRestoreError::AssetMetadata(_)) => {
                SessionCandidateFailureKind::Asset
            }
            Self::Restore(SavedSessionRestoreError::Projection(_)) => {
                SessionCandidateFailureKind::Projection
            }
            Self::Restore(SavedSessionRestoreError::Preparation(error)) => {
                if graph_preparation_is_asset_failure(error) {
                    SessionCandidateFailureKind::Asset
                } else {
                    SessionCandidateFailureKind::Preparation
                }
            }
        }
    }
}

fn graph_preparation_is_asset_failure(error: &GraphPreparationError) -> bool {
    matches!(
        error,
        GraphPreparationError::Rack(RackPreparationError::Instrument {
            source: InstrumentPreparationError::AssetLoadFailed
                | InstrumentPreparationError::AssetParseFailed
                | InstrumentPreparationError::AssetUnavailable { .. }
                | InstrumentPreparationError::InvalidAsset { .. }
                | InstrumentPreparationError::SampleAsset { .. },
            ..
        })
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionCandidateWorkerBusyReason {
    OutstandingRequest,
    WorkerUnavailable,
    Shutdown,
}

pub struct SessionCandidateWorkerBusy {
    reason: SessionCandidateWorkerBusyReason,
    request: Box<SessionCandidateRequest>,
}

impl SessionCandidateWorkerBusy {
    pub fn new(reason: SessionCandidateWorkerBusyReason, request: SessionCandidateRequest) -> Self {
        Self {
            reason,
            request: Box::new(request),
        }
    }

    pub const fn reason(&self) -> SessionCandidateWorkerBusyReason {
        self.reason
    }

    pub fn into_request(self) -> SessionCandidateRequest {
        *self.request
    }
}

impl fmt::Debug for SessionCandidateWorkerBusy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionCandidateWorkerBusy")
            .field("reason", &self.reason)
            .field("token", &self.request.token())
            .finish()
    }
}

pub trait SessionCandidateWorker: Send {
    fn try_submit(
        &mut self,
        request: SessionCandidateRequest,
    ) -> Result<(), SessionCandidateWorkerBusy>;

    fn try_poll(&mut self) -> Option<SessionCandidateResult>;

    fn shutdown_on_control(&mut self) -> Result<(), WorkerShutdownError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::StateProjectionError;

    #[test]
    fn projection_failure_has_its_own_public_category() {
        let failure = SessionCandidateFailure::Restore(SavedSessionRestoreError::Projection(
            StateProjectionError::StateSerialization,
        ));
        assert_eq!(failure.kind(), SessionCandidateFailureKind::Projection);
    }
}
