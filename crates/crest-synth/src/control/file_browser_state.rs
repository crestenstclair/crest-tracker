use crate::control::{EngineSelectionFailure, EngineSelectionRequestId};
use crate::kernel::PatchId;
use crate::synth::{
    AssetFileId, FileBrowserFolderId, FileBrowserListing, FileBrowserRow, FileBrowserRowKind,
    ParameterId, SampleAssetError,
};
use serde::Serialize;
use std::collections::BTreeMap;

/// Canonical correlation for an in-app file selection awaiting import.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetImportRequest {
    pub(crate) generation: u64,
    pub(crate) asset_kind: crate::synth::AssetKind,
    pub(crate) asset_id: AssetFileId,
    pub(crate) origin: crate::control::FocusPath,
    pub(crate) graph_revision: crate::real_time::GraphRevision,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetImportResult {
    pub request: AssetImportRequest,
    #[serde(default)]
    pub descriptor: Option<crate::synth::CapabilityDescriptor>,
    pub result: Result<AssetFileId, SampleAssetError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SampleAssetLifecycle {
    Loading,
    Validating,
    Preparing,
    Activating,
    Ready,
    Unavailable,
    Invalid,
    Cancelled,
    Failed(EngineSelectionFailure),
}

impl SampleAssetLifecycle {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Loading => "LOADING",
            Self::Validating => "VALIDATING",
            Self::Preparing => "PREPARING",
            Self::Activating => "ACTIVATING",
            Self::Ready => "READY",
            Self::Unavailable => "UNAVAILABLE",
            Self::Invalid => "INVALID",
            Self::Cancelled => "CANCELLED — UNCHANGED",
            Self::Failed(_) => "FAILED",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SamplePreviewState {
    Idle,
    Held {
        asset_id: AssetFileId,
    },
    Preparing {
        asset_id: AssetFileId,
        /// Whether Start is still physically held when preparation completes.
        held: bool,
    },
    Playing {
        asset_id: AssetFileId,
    },
    Stopping {
        asset_id: AssetFileId,
    },
    Failed {
        asset_id: AssetFileId,
        cause: SampleAssetError,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileBrowserState {
    origin: Option<crate::control::FocusPath>,
    import_request: Option<AssetImportRequest>,
    file_selection_failure: Option<SampleAssetError>,
    asset_kind: crate::synth::AssetKind,
    catalog: BTreeMap<
        (crate::synth::AssetKind, FileBrowserFolderId),
        Result<FileBrowserListing, SampleAssetError>,
    >,
    patch_id: Option<PatchId>,
    asset_parameter_id: Option<ParameterId>,
    folder: FileBrowserFolderId,
    rows: Vec<FileBrowserRow>,
    lifecycle: SampleAssetLifecycle,
    requested_asset: Option<AssetFileId>,
    request_id: Option<EngineSelectionRequestId>,
    preview_request_id: Option<EngineSelectionRequestId>,
    preview: SamplePreviewState,
}

impl Default for FileBrowserState {
    fn default() -> Self {
        Self {
            origin: None,
            import_request: None,
            file_selection_failure: None,
            catalog: BTreeMap::new(),
            asset_kind: crate::synth::AssetKind::Sample,
            patch_id: None,
            asset_parameter_id: None,
            folder: FileBrowserFolderId::default(),
            rows: fallback_rows(&FileBrowserFolderId::default()),
            lifecycle: SampleAssetLifecycle::Unavailable,
            requested_asset: None,
            request_id: None,
            preview_request_id: None,
            preview: SamplePreviewState::Idle,
        }
    }
}

impl FileBrowserState {
    pub fn origin(&self) -> Option<&crate::control::FocusPath> {
        self.origin.as_ref()
    }
    pub(crate) fn begin_from(
        &mut self,
        origin: crate::control::FocusPath,
        parameter: ParameterId,
        kind: crate::synth::AssetKind,
    ) {
        self.begin(origin.patch_id(), parameter, kind);
        self.origin = Some(origin);
    }
    pub fn import_request(&self) -> Option<&AssetImportRequest> {
        self.import_request.as_ref()
    }

    pub const fn file_selection_failure(&self) -> Option<SampleAssetError> {
        self.file_selection_failure
    }

    pub(crate) fn request_file_import(
        &mut self,
        request: AssetImportRequest,
        patch_id: Option<PatchId>,
        parameter: ParameterId,
    ) {
        self.begin(patch_id, parameter, request.asset_kind);
        self.origin = Some(request.origin.clone());
        self.import_request = Some(request);
        self.lifecycle = SampleAssetLifecycle::Loading;
    }

    pub(crate) fn finish_file_import(&mut self, result: &Result<AssetFileId, SampleAssetError>) {
        self.import_request = None;
        self.file_selection_failure = result.as_ref().err().copied();
        self.lifecycle = match result {
            Ok(_) => SampleAssetLifecycle::Ready,
            Err(SampleAssetError::Cancelled) => SampleAssetLifecycle::Cancelled,
            Err(SampleAssetError::Unavailable | SampleAssetError::DownloadRequired) => {
                SampleAssetLifecycle::Unavailable
            }
            Err(_) => SampleAssetLifecycle::Invalid,
        };
    }
    /// Clears every document-specific browser/preview correlation while
    /// retaining the adapter-discovered catalog owned by the application.
    pub(crate) fn reset_for_session(&mut self) {
        let catalog = std::mem::take(&mut self.catalog);
        *self = Self::default();
        self.catalog = catalog;
    }

    pub fn with_catalog(
        mut self,
        listings: impl IntoIterator<
            Item = (
                FileBrowserFolderId,
                Result<FileBrowserListing, SampleAssetError>,
            ),
        >,
    ) -> Self {
        self.catalog = listings
            .into_iter()
            .map(|(folder, listing)| ((crate::synth::AssetKind::Sample, folder), listing))
            .collect();
        self
    }

    pub const fn asset_kind(&self) -> crate::synth::AssetKind {
        self.asset_kind
    }

    pub const fn patch_id(&self) -> Option<PatchId> {
        self.patch_id
    }

    pub const fn asset_parameter_id(&self) -> Option<&ParameterId> {
        self.asset_parameter_id.as_ref()
    }

    pub const fn folder(&self) -> &FileBrowserFolderId {
        &self.folder
    }

    pub fn rows(&self) -> &[FileBrowserRow] {
        &self.rows
    }

    pub const fn lifecycle(&self) -> SampleAssetLifecycle {
        self.lifecycle
    }

    pub const fn requested_asset(&self) -> Option<&AssetFileId> {
        self.requested_asset.as_ref()
    }

    pub const fn request_id(&self) -> Option<EngineSelectionRequestId> {
        self.request_id
    }

    pub const fn preview(&self) -> &SamplePreviewState {
        &self.preview
    }

    pub const fn preview_request_id(&self) -> Option<EngineSelectionRequestId> {
        self.preview_request_id
    }

    pub const fn preview_is_held(&self) -> bool {
        matches!(
            self.preview,
            SamplePreviewState::Held { .. }
                | SamplePreviewState::Preparing { held: true, .. }
                | SamplePreviewState::Playing { .. }
        )
    }

    pub fn row(&self, id: &str) -> Option<&FileBrowserRow> {
        self.rows.iter().find(|row| row.id() == id)
    }

    pub(crate) fn begin(
        &mut self,
        patch_id: impl Into<Option<PatchId>>,
        asset_parameter_id: ParameterId,
        kind: crate::synth::AssetKind,
    ) {
        self.file_selection_failure = None;
        self.origin = None;
        self.asset_kind = kind;
        self.patch_id = patch_id.into();
        self.asset_parameter_id = Some(asset_parameter_id);
        self.requested_asset = None;
        self.request_id = None;
        self.preview_request_id = None;
        self.preview = SamplePreviewState::Idle;
        self.load_folder(FileBrowserFolderId::default());
    }

    pub(crate) fn load_folder(&mut self, folder: FileBrowserFolderId) {
        self.preview = SamplePreviewState::Idle;
        self.preview_request_id = None;
        self.folder = folder.clone();
        match self.catalog.get(&(self.asset_kind, folder.clone())) {
            Some(Ok(listing)) if listing.folder() == &folder => {
                self.rows = listing.rows().to_vec();
                self.lifecycle = SampleAssetLifecycle::Ready;
            }
            None => {
                self.rows = fallback_rows(&folder);
                self.lifecycle = SampleAssetLifecycle::Loading;
            }
            Some(Err(SampleAssetError::Unavailable | SampleAssetError::DownloadRequired)) => {
                self.rows = fallback_rows(&folder);
                self.lifecycle = SampleAssetLifecycle::Unavailable;
            }
            Some(Err(error)) => {
                self.rows = fallback_rows(&folder);
                self.lifecycle = if matches!(error, SampleAssetError::Cancelled) {
                    SampleAssetLifecycle::Cancelled
                } else {
                    SampleAssetLifecycle::Invalid
                };
            }
            Some(Ok(_)) => {
                self.rows = fallback_rows(&folder);
                self.lifecycle = SampleAssetLifecycle::Invalid;
            }
        }
    }

    /// Replaces one adapter-owned catalog result and reloads the visible
    /// folder when it is the one currently open. Focus repair remains an
    /// AppState responsibility because only the reducer owns FocusPath.
    pub(crate) fn refresh_listing(
        &mut self,
        asset_kind: crate::synth::AssetKind,
        folder: FileBrowserFolderId,
        listing: Result<FileBrowserListing, SampleAssetError>,
    ) -> bool {
        let reload = self.asset_parameter_id.is_some()
            && self.asset_kind == asset_kind
            && self.folder == folder;
        self.catalog.insert((asset_kind, folder.clone()), listing);
        if reload {
            self.load_folder(folder);
        }
        reload
    }

    pub(crate) fn preview_requested(
        &mut self,
        asset_id: AssetFileId,
        request_id: EngineSelectionRequestId,
    ) {
        self.preview_request_id = Some(request_id);
        self.preview = SamplePreviewState::Preparing {
            asset_id,
            held: true,
        };
    }

    /// Releases the hold. Returns `true` only when a compatible active
    /// audition voice needs a fixed-size stop command.
    pub(crate) fn preview_stop(&mut self) -> bool {
        let command_required = matches!(self.preview, SamplePreviewState::Playing { .. });
        self.preview = match &self.preview {
            SamplePreviewState::Idle => SamplePreviewState::Idle,
            SamplePreviewState::Held { asset_id } => SamplePreviewState::Preparing {
                asset_id: asset_id.clone(),
                held: false,
            },
            SamplePreviewState::Preparing { asset_id, .. } => SamplePreviewState::Preparing {
                asset_id: asset_id.clone(),
                held: false,
            },
            SamplePreviewState::Playing { .. } | SamplePreviewState::Stopping { .. } => {
                SamplePreviewState::Idle
            }
            SamplePreviewState::Failed { .. } => SamplePreviewState::Idle,
        };
        if command_required {
            self.preview_request_id = None;
        }
        command_required
    }

    pub(crate) fn preview_activating(&mut self, request_id: EngineSelectionRequestId) {
        debug_assert_eq!(self.preview_request_id, Some(request_id));
    }

    /// Completes activation and reports whether Start is still held, which is
    /// the only point at which the reducer may emit `PreviewStart`.
    pub(crate) fn preview_ready(&mut self, request_id: EngineSelectionRequestId) -> bool {
        if self.preview_request_id != Some(request_id) {
            return false;
        }
        let next = match &self.preview {
            SamplePreviewState::Preparing {
                asset_id,
                held: true,
            } => Some(SamplePreviewState::Playing {
                asset_id: asset_id.clone(),
            }),
            SamplePreviewState::Preparing { held: false, .. } => {
                self.preview_request_id = None;
                self.preview = SamplePreviewState::Idle;
                return false;
            }
            _ => return false,
        };
        self.preview = next.expect("held preparation produces Playing");
        true
    }

    pub(crate) fn preview_failed(
        &mut self,
        request_id: EngineSelectionRequestId,
        failure: EngineSelectionFailure,
    ) {
        if self.preview_request_id != Some(request_id) {
            return;
        }
        let asset_id = match &self.preview {
            SamplePreviewState::Held { asset_id }
            | SamplePreviewState::Preparing { asset_id, .. }
            | SamplePreviewState::Playing { asset_id }
            | SamplePreviewState::Stopping { asset_id }
            | SamplePreviewState::Failed { asset_id, .. } => asset_id.clone(),
            SamplePreviewState::Idle => return,
        };
        let cause = match failure {
            EngineSelectionFailure::AssetUnavailable => SampleAssetError::Unavailable,
            EngineSelectionFailure::UnsupportedAssetFormat => SampleAssetError::UnsupportedEncoding,
            EngineSelectionFailure::AssetCapacityExceeded => {
                SampleAssetError::AssetScalarCapacityExceeded
            }
            EngineSelectionFailure::GraphCapacityExceeded => {
                SampleAssetError::GraphPcmCapacityExceeded
            }
            EngineSelectionFailure::Cancelled => SampleAssetError::Cancelled,
            EngineSelectionFailure::AllocationFailed => SampleAssetError::AllocationFailed,
            _ => SampleAssetError::MalformedPcm,
        };
        self.preview = SamplePreviewState::Failed { asset_id, cause };
        self.preview_request_id = None;
    }

    pub(crate) fn assignment_requested(
        &mut self,
        asset_id: AssetFileId,
        request_id: EngineSelectionRequestId,
    ) {
        self.file_selection_failure = None;
        self.requested_asset = Some(asset_id);
        self.request_id = Some(request_id);
        self.preview_request_id = None;
        self.lifecycle = SampleAssetLifecycle::Loading;
        self.preview = SamplePreviewState::Idle;
    }

    pub(crate) fn assignment_unchanged(&mut self) {
        self.requested_asset = None;
        self.request_id = None;
        self.preview_request_id = None;
        self.preview = SamplePreviewState::Idle;
        self.lifecycle = SampleAssetLifecycle::Ready;
    }

    pub(crate) fn assignment_lifecycle_advanced(
        &mut self,
        request_id: EngineSelectionRequestId,
        lifecycle: SampleAssetLifecycle,
    ) -> bool {
        if self.request_id != Some(request_id) {
            return false;
        }
        let valid = matches!(
            (self.lifecycle, lifecycle),
            (
                SampleAssetLifecycle::Loading,
                SampleAssetLifecycle::Validating
            ) | (
                SampleAssetLifecycle::Validating,
                SampleAssetLifecycle::Preparing
            )
        );
        if valid {
            self.lifecycle = lifecycle;
        }
        valid
    }

    pub(crate) fn assignment_activating(&mut self, request_id: EngineSelectionRequestId) {
        if self.request_id == Some(request_id) && self.lifecycle == SampleAssetLifecycle::Preparing
        {
            self.lifecycle = SampleAssetLifecycle::Activating;
        }
    }

    pub(crate) fn assignment_ready(&mut self, request_id: EngineSelectionRequestId) {
        if self.request_id == Some(request_id) {
            self.lifecycle = SampleAssetLifecycle::Ready;
            self.requested_asset = None;
            self.request_id = None;
        }
    }

    pub(crate) fn assignment_failed(
        &mut self,
        request_id: EngineSelectionRequestId,
        failure: EngineSelectionFailure,
    ) {
        if self.request_id != Some(request_id) {
            return;
        }
        self.lifecycle = match failure {
            EngineSelectionFailure::AssetUnavailable => SampleAssetLifecycle::Unavailable,
            EngineSelectionFailure::InvalidAsset
            | EngineSelectionFailure::UnsupportedAssetFormat
            | EngineSelectionFailure::AssetCapacityExceeded
            | EngineSelectionFailure::GraphCapacityExceeded => SampleAssetLifecycle::Invalid,
            EngineSelectionFailure::Cancelled => SampleAssetLifecycle::Cancelled,
            other => SampleAssetLifecycle::Failed(other),
        };
        self.requested_asset = None;
        self.request_id = None;
        self.preview_request_id = None;
        self.preview = SamplePreviewState::Idle;
    }

    pub(crate) fn cancelled(&mut self) {
        self.preview = SamplePreviewState::Idle;
        self.requested_asset = None;
        self.request_id = None;
        self.preview_request_id = None;
        self.lifecycle = SampleAssetLifecycle::Cancelled;
    }
}

fn fallback_rows(folder: &FileBrowserFolderId) -> Vec<FileBrowserRow> {
    vec![FileBrowserRow::new(
        format!("cancel:{}", folder.as_str()),
        "CANCEL — UNCHANGED",
        FileBrowserRowKind::Cancel,
        None,
    )
    .expect("static fallback browser row is valid")]
}
