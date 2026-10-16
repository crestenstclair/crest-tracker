use crate::kernel::PatchId;
use crate::synth::{EffectCapabilityId, PostEffectConfig, PreparedPostEffect};
use core::fmt;

/// Worker/control-side factory for one installed effect capability.
///
/// The same preparer serves Patch effect slots and bus returns. Each
/// preparation yields an independent instance with independent internal
/// state, and no preparer branches on which role requested it.
pub trait EffectPreparer: Send + Sync {
    fn capability_id(&self) -> &EffectCapabilityId;

    fn prepare(
        &self,
        patch_id: PatchId,
        config: &PostEffectConfig,
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<Box<dyn PreparedPostEffect>, EffectPreparationError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectPreparationError {
    UnsupportedCapability { patch_id: PatchId },
    InvalidConfiguration { patch_id: PatchId },
    InvalidSampleRate,
    InvalidFrameCapacity,
    StorageAllocationFailed { patch_id: PatchId },
    PreparationFailed { patch_id: PatchId },
}

impl fmt::Display for EffectPreparationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::UnsupportedCapability { patch_id } => {
                write!(
                    formatter,
                    "Patch {patch_id} uses an unsupported effect capability"
                )
            }
            Self::InvalidConfiguration { patch_id } => {
                write!(
                    formatter,
                    "Patch {patch_id} has an invalid effect configuration"
                )
            }
            Self::InvalidSampleRate => formatter.write_str("effect sample rate is unsupported"),
            Self::InvalidFrameCapacity => {
                formatter.write_str("effect frame capacity is unsupported")
            }
            Self::StorageAllocationFailed { patch_id } => {
                write!(
                    formatter,
                    "Patch {patch_id} effect storage could not be allocated"
                )
            }
            Self::PreparationFailed { patch_id } => {
                write!(formatter, "Patch {patch_id} effect preparation failed")
            }
        }
    }
}

impl std::error::Error for EffectPreparationError {}
