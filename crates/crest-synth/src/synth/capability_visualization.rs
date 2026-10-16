use crate::synth::{CapabilityError, ParameterId};
use serde::{Deserialize, Serialize};

/// Semantic role of one normalized landmark in an informative waveform.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WaveformLandmarkRole {
    PlaybackStart,
    PlaybackEnd,
    LoopStart,
    LoopEnd,
}

/// Descriptor-owned link from a waveform landmark role to canonical state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaveformLandmarkSpec {
    role: WaveformLandmarkRole,
    parameter_id: ParameterId,
}

impl WaveformLandmarkSpec {
    pub const fn new(role: WaveformLandmarkRole, parameter_id: ParameterId) -> Self {
        Self { role, parameter_id }
    }

    pub const fn role(&self) -> WaveformLandmarkRole {
        self.role
    }

    pub const fn parameter_id(&self) -> &ParameterId {
        &self.parameter_id
    }
}

/// Generic, non-focusable presentation declared by a capability.
///
/// It names canonical sources only. Geometry, pixels, decoded PCM, playhead
/// state, and widget identities are deliberately absent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CapabilityVisualization {
    Envelope {
        id: String,
        label: String,
    },
    Waveform {
        id: String,
        label: String,
        asset_parameter_id: ParameterId,
        landmarks: Vec<WaveformLandmarkSpec>,
    },
    Status {
        id: String,
        label: String,
    },
}

impl CapabilityVisualization {
    pub fn envelope(
        id: impl Into<String>,
        label: impl Into<String>,
    ) -> Result<Self, CapabilityError> {
        Self::validate_identity(id.into(), label.into())
            .map(|(id, label)| Self::Envelope { id, label })
    }

    pub fn waveform(
        id: impl Into<String>,
        label: impl Into<String>,
        asset_parameter_id: ParameterId,
        landmarks: Vec<WaveformLandmarkSpec>,
    ) -> Result<Self, CapabilityError> {
        let (id, label) = Self::validate_identity(id.into(), label.into())?;
        if landmarks.is_empty() {
            return Err(CapabilityError::InvalidVisualization(id));
        }
        for (index, landmark) in landmarks.iter().enumerate() {
            if landmarks[..index]
                .iter()
                .any(|prior| prior.role() == landmark.role())
            {
                return Err(CapabilityError::InvalidVisualization(id));
            }
        }
        Ok(Self::Waveform {
            id,
            label,
            asset_parameter_id,
            landmarks,
        })
    }

    pub fn status(
        id: impl Into<String>,
        label: impl Into<String>,
    ) -> Result<Self, CapabilityError> {
        Self::validate_identity(id.into(), label.into())
            .map(|(id, label)| Self::Status { id, label })
    }

    fn validate_identity(id: String, label: String) -> Result<(String, String), CapabilityError> {
        if id.is_empty()
            || !id.split('.').all(|segment| {
                !segment.is_empty()
                    && segment
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || character == '-')
            })
        {
            return Err(CapabilityError::InvalidMetadataIdentifier(id));
        }
        if label.is_empty() {
            return Err(CapabilityError::EmptyLabel);
        }
        Ok((id, label))
    }

    pub fn id(&self) -> &str {
        match self {
            Self::Envelope { id, .. } | Self::Waveform { id, .. } | Self::Status { id, .. } => id,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Envelope { label, .. }
            | Self::Waveform { label, .. }
            | Self::Status { label, .. } => label,
        }
    }
}
