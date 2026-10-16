use crate::adapter::chorus_capability::{
    ChorusCapability, CHORUS_AMOUNT_PARAMETER_ID, CHORUS_CAPABILITY_ID, CHORUS_DEPTH_PARAMETER_ID,
};
use crate::adapter::chorus_native::ChorusProcessor;
use crate::kernel::PatchId;
use crate::real_time::RtPostEffectParameters;
use crate::synth::{
    EffectCapabilityId, EffectCapabilityProvider, EffectPreparationError, EffectPreparer,
    EffectSlotId, ParameterId, PostEffectConfig, PreparedEffectError, PreparedPostEffect,
};

pub const CHORUS_SAMPLE_RATE: f32 = 48_000.0;
pub const CHORUS_DELAY_SAMPLES: usize = 2_048;

pub struct ChorusPreparer {
    capability_id: EffectCapabilityId,
}

impl ChorusPreparer {
    pub fn new() -> Result<Self, EffectPreparationError> {
        let capability_id = EffectCapabilityId::new(CHORUS_CAPABILITY_ID).map_err(|_| {
            EffectPreparationError::PreparationFailed {
                patch_id: PatchId::new(1).expect("static error Patch id is valid"),
            }
        })?;
        Ok(Self { capability_id })
    }
}

impl EffectPreparer for ChorusPreparer {
    fn capability_id(&self) -> &EffectCapabilityId {
        &self.capability_id
    }

    fn prepare(
        &self,
        patch_id: PatchId,
        config: &PostEffectConfig,
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<Box<dyn PreparedPostEffect>, EffectPreparationError> {
        if sample_rate != CHORUS_SAMPLE_RATE {
            return Err(EffectPreparationError::InvalidSampleRate);
        }
        if max_frames == 0 {
            return Err(EffectPreparationError::InvalidFrameCapacity);
        }
        if config.capability_id() != &self.capability_id {
            return Err(EffectPreparationError::UnsupportedCapability { patch_id });
        }
        let capability = ChorusCapability::new()
            .map_err(|_| EffectPreparationError::InvalidConfiguration { patch_id })?;
        let canonical = capability
            .create_config(config.slot_id(), config.values(), config.asset_references())
            .map_err(|_| EffectPreparationError::InvalidConfiguration { patch_id })?;
        if &canonical != config {
            return Err(EffectPreparationError::InvalidConfiguration { patch_id });
        }
        let processor = ChorusProcessor::new(max_frames)
            .map_err(|_| EffectPreparationError::StorageAllocationFailed { patch_id })?;
        Ok(Box::new(PreparedChorus {
            patch_id,
            slot_id: config.slot_id(),
            processor,
            max_frames,
        }))
    }
}

struct PreparedChorus {
    patch_id: PatchId,
    slot_id: EffectSlotId,
    processor: ChorusProcessor,
    max_frames: usize,
}

impl PreparedPostEffect for PreparedChorus {
    fn patch_id(&self) -> PatchId {
        self.patch_id
    }

    fn slot_id(&self) -> EffectSlotId {
        self.slot_id
    }

    fn process(
        &mut self,
        interleaved_stereo: &mut [f32],
        frame_count: usize,
        parameters: &RtPostEffectParameters,
    ) -> Result<(), PreparedEffectError> {
        if frame_count > self.max_frames || interleaved_stereo.len() != frame_count * 2 {
            return Err(PreparedEffectError::FrameCapacityExceeded);
        }
        if parameters.slot_id() != Some(self.slot_id) || parameters.scalar_count() != 2 {
            return Err(PreparedEffectError::ScalarLayoutMismatch);
        }
        let amount = parameters
            .scalar(0)
            .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
            .ok_or(PreparedEffectError::ScalarLayoutMismatch)?;
        let depth = parameters
            .scalar(1)
            .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
            .ok_or(PreparedEffectError::ScalarLayoutMismatch)?;
        self.processor
            .process(interleaved_stereo, frame_count, amount, depth)
            .map_err(|_| PreparedEffectError::ProcessRejected)
    }
}

#[allow(dead_code)]
fn parameter_ids() -> (ParameterId, ParameterId) {
    (
        ParameterId::new(CHORUS_AMOUNT_PARAMETER_ID).expect("static Amount id is valid"),
        ParameterId::new(CHORUS_DEPTH_PARAMETER_ID).expect("static Depth id is valid"),
    )
}
