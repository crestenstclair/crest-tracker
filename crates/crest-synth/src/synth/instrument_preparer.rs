use crate::kernel::patch_id::PatchId;
use crate::synth::capability_id::CapabilityId;
use crate::synth::patch::Patch;
use crate::synth::prepared_instrument::PreparedInstrument;
use crate::synth::{InstrumentConfig, PreparedAudition, SampleAssetError};
use core::fmt;

/// Control/worker-side factory for one installed instrument capability.
///
/// Preparation may validate and read assets, parse immutable data, allocate
/// voices and scratch, and warm the runtime. None of those operations may be
/// deferred into the returned callback-owned instrument.
pub trait InstrumentPreparer: Send + Sync {
    /// Returns the one stable capability identity accepted by this preparer.
    fn capability_id(&self) -> &CapabilityId;

    /// Reports immutable shared assets already prepared by this adapter.
    /// This control-side witness is capability-neutral; adapters with no
    /// shared asset return zero.
    fn prepared_shared_asset_count(&self) -> usize {
        0
    }

    /// Resolves asset-dependent metadata on worker ownership before candidate validation.
    fn asset_descriptor(
        &self,
        _config: &InstrumentConfig,
    ) -> Result<Option<crate::synth::CapabilityDescriptor>, InstrumentPreparationError> {
        Ok(None)
    }

    /// Completely prepares one Patch for the declared device limits.
    fn prepare(
        &self,
        patch: &Patch,
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError>;

    /// Prepares the capability's optional one-voice asset audition.
    ///
    /// The default is an explicit refusal; callers never substitute the live
    /// instrument or another capability as a preview fallback.
    fn prepare_audition(
        &self,
        patch_id: PatchId,
        _candidate: &InstrumentConfig,
        _sample_rate: f32,
        _max_frames: usize,
    ) -> Result<Box<dyn PreparedAudition>, InstrumentPreparationError> {
        Err(InstrumentPreparationError::UnsupportedCapability { patch_id })
    }
}

/// A typed control-side instrument preparation failure. No variant permits a
/// fallback capability, asset, preset, or instrument.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstrumentPreparationError {
    AssetLoadFailed,
    AssetParseFailed,
    UnsupportedCapability {
        patch_id: PatchId,
    },
    InvalidConfiguration {
        patch_id: PatchId,
    },
    InvalidSampleRate,
    InvalidFrameCapacity,
    AssetUnavailable {
        patch_id: PatchId,
    },
    InvalidAsset {
        patch_id: PatchId,
    },
    PresetUnavailable {
        patch_id: PatchId,
    },
    VoiceCapacityExceeded {
        patch_id: PatchId,
    },
    StorageAllocationFailed {
        patch_id: PatchId,
    },
    SampleAsset {
        patch_id: PatchId,
        cause: SampleAssetError,
    },
    PreparationFailed {
        patch_id: PatchId,
    },
}

impl fmt::Display for InstrumentPreparationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::AssetLoadFailed => formatter.write_str("instrument asset could not be loaded"),
            Self::AssetParseFailed => formatter.write_str("instrument asset could not be parsed"),
            Self::UnsupportedCapability { patch_id } => {
                write!(formatter, "Patch {patch_id} uses an unsupported capability")
            }
            Self::InvalidConfiguration { patch_id } => {
                write!(
                    formatter,
                    "Patch {patch_id} has an invalid instrument configuration"
                )
            }
            Self::InvalidSampleRate => formatter.write_str("sample rate is unsupported"),
            Self::InvalidFrameCapacity => formatter.write_str("frame capacity is unsupported"),
            Self::AssetUnavailable { patch_id } => {
                write!(
                    formatter,
                    "Patch {patch_id} instrument asset is unavailable"
                )
            }
            Self::InvalidAsset { patch_id } => {
                write!(formatter, "Patch {patch_id} instrument asset is invalid")
            }
            Self::PresetUnavailable { patch_id } => {
                write!(
                    formatter,
                    "Patch {patch_id} instrument preset is unavailable"
                )
            }
            Self::VoiceCapacityExceeded { patch_id } => {
                write!(
                    formatter,
                    "Patch {patch_id} voice capacity could not be prepared"
                )
            }
            Self::StorageAllocationFailed { patch_id } => {
                write!(
                    formatter,
                    "Patch {patch_id} instrument storage could not be allocated"
                )
            }
            Self::SampleAsset { patch_id, cause } => {
                write!(
                    formatter,
                    "Patch {patch_id} Sample preparation failed: {cause}"
                )
            }
            Self::PreparationFailed { patch_id } => {
                write!(formatter, "Patch {patch_id} instrument preparation failed")
            }
        }
    }
}

impl std::error::Error for InstrumentPreparationError {}
