use crate::kernel::PatchId;
use crate::real_time::RtPostEffectParameters;
use crate::synth::EffectSlotId;
use core::fmt;

/// One fully prepared post-effect callback instance.
///
/// An instance is bound to the identities it was prepared with; it carries no
/// knowledge of whether a Patch effect slot or a bus return requested it.
pub trait PreparedPostEffect: Send {
    fn patch_id(&self) -> PatchId;
    fn slot_id(&self) -> EffectSlotId;

    fn process(
        &mut self,
        interleaved_stereo: &mut [f32],
        frame_count: usize,
        parameters: &RtPostEffectParameters,
    ) -> Result<(), PreparedEffectError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreparedEffectError {
    PatchMismatch,
    SlotMismatch,
    ScalarLayoutMismatch,
    FrameCapacityExceeded,
    ProcessRejected,
}

impl fmt::Display for PreparedEffectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::PatchMismatch => "prepared effect Patch identity does not match",
            Self::SlotMismatch => "prepared effect slot identity does not match",
            Self::ScalarLayoutMismatch => "prepared effect Scalar layout does not match",
            Self::FrameCapacityExceeded => "prepared effect frame capacity was exceeded",
            Self::ProcessRejected => "prepared effect rejected processing",
        })
    }
}

impl std::error::Error for PreparedEffectError {}
