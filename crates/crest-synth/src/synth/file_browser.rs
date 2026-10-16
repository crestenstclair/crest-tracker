use crate::synth::{SampleAssetError, SampleMetadata};
use core::fmt;
use serde::{Deserialize, Serialize};

/// Stable library-root-relative file identity shared by asset browsers and decoders.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct AssetFileId(String);

impl AssetFileId {
    pub fn new(value: impl Into<String>) -> Result<Self, SampleAssetError> {
        let value = normalize_relative_id(value.into())?;
        Ok(Self(value))
    }

    /// Transient picker references are imported before becoming saved assets.
    pub fn is_external(&self) -> bool {
        self.0.starts_with("@home/") || self.0.starts_with("@volumes/")
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AssetFileId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Stable relative folder identity used only by the transient browser.
#[derive(Clone, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct FileBrowserFolderId(String);

impl FileBrowserFolderId {
    pub fn new(value: impl Into<String>) -> Result<Self, SampleAssetError> {
        let value = value.into();
        if value.is_empty() {
            return Ok(Self::default());
        }
        Ok(Self(normalize_relative_id(value)?))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn parent(&self) -> Option<Self> {
        if self.0.is_empty() {
            None
        } else {
            Some(Self(
                self.0
                    .rsplit_once('/')
                    .map_or_else(String::new, |(parent, _)| parent.to_owned()),
            ))
        }
    }
}

fn normalize_relative_id(value: String) -> Result<String, SampleAssetError> {
    if value.is_empty()
        || value.starts_with('/')
        || value.starts_with('\\')
        || value.contains('\\')
        || value.contains('\0')
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(SampleAssetError::InvalidRelativeId);
    }
    Ok(value)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "target", rename_all = "camelCase")]
pub enum FileBrowserRowKind {
    Parent(FileBrowserFolderId),
    Folder(FileBrowserFolderId),
    File(AssetFileId),
    Cancel,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileBrowserRow {
    id: String,
    label: String,
    kind: FileBrowserRowKind,
    source_bytes: Option<u64>,
    metadata: Option<Result<SampleMetadata, SampleAssetError>>,
}

impl FileBrowserRow {
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        kind: FileBrowserRowKind,
        source_bytes: Option<u64>,
    ) -> Result<Self, SampleAssetError> {
        let id = id.into();
        let label = label.into();
        if id.is_empty() || label.is_empty() {
            return Err(SampleAssetError::InvalidRelativeId);
        }
        Ok(Self {
            id,
            label,
            kind,
            source_bytes,
            metadata: None,
        })
    }

    /// Attaches the catalog adapter's typed metadata result to an eligible
    /// file row. Folders, parent navigation, and Cancel can never masquerade
    /// as files carrying metadata, and successful metadata must name the same
    /// stable library-relative asset as the row.
    pub fn with_metadata(
        mut self,
        metadata: Result<SampleMetadata, SampleAssetError>,
    ) -> Result<Self, SampleAssetError> {
        let FileBrowserRowKind::File(asset_id) = &self.kind else {
            return Err(SampleAssetError::MalformedCatalog);
        };
        if let Ok(value) = &metadata {
            if value.asset_id() != asset_id {
                return Err(SampleAssetError::MalformedCatalog);
            }
            self.source_bytes = Some(value.source_bytes());
        }
        self.metadata = Some(metadata);
        Ok(self)
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn label(&self) -> &str {
        &self.label
    }
    pub const fn kind(&self) -> &FileBrowserRowKind {
        &self.kind
    }
    pub const fn source_bytes(&self) -> Option<u64> {
        self.source_bytes
    }
    pub const fn metadata(&self) -> Option<&Result<SampleMetadata, SampleAssetError>> {
        self.metadata.as_ref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileBrowserListing {
    folder: FileBrowserFolderId,
    rows: Vec<FileBrowserRow>,
}

impl FileBrowserListing {
    pub fn new(
        folder: FileBrowserFolderId,
        rows: Vec<FileBrowserRow>,
    ) -> Result<Self, SampleAssetError> {
        if rows.is_empty()
            || rows
                .iter()
                .enumerate()
                .any(|(index, row)| rows[..index].iter().any(|prior| prior.id() == row.id()))
        {
            return Err(SampleAssetError::MalformedCatalog);
        }
        Ok(Self { folder, rows })
    }
    pub const fn folder(&self) -> &FileBrowserFolderId {
        &self.folder
    }
    pub fn rows(&self) -> &[FileBrowserRow] {
        &self.rows
    }
}
