use crate::adapter::reverb_capability::{ReverbCapability, REVERB_CAPABILITY_ID};
use crate::kernel::PatchId;
use crate::real_time::RtPostEffectParameters;
use crate::synth::{
    EffectCapabilityId, EffectCapabilityProvider, EffectPreparationError, EffectPreparer,
    EffectSlotId, PostEffectConfig, PreparedEffectError, PreparedPostEffect,
};

const REVERB_LEFT_MILLISECONDS: f32 = 29.7;
const REVERB_RIGHT_MILLISECONDS: f32 = 37.1;

/// Registry preparer for the shared algorithmic stereo Reverb.
///
/// The DSP is the retired `GlobalReverbDelay` reverb moved unchanged behind
/// the generic prepared-effect boundary: algorithm, coefficients, and internal
/// state layout are identical. The same preparer serves Patch effect slots and
/// bus returns; each preparation yields an independent instance.
pub struct ReverbPreparer {
    capability_id: EffectCapabilityId,
}

impl ReverbPreparer {
    pub fn new() -> Result<Self, EffectPreparationError> {
        let capability_id = EffectCapabilityId::new(REVERB_CAPABILITY_ID).map_err(|_| {
            EffectPreparationError::PreparationFailed {
                patch_id: PatchId::new(1).expect("static error Patch id is valid"),
            }
        })?;
        Ok(Self { capability_id })
    }
}

impl EffectPreparer for ReverbPreparer {
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
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(EffectPreparationError::InvalidSampleRate);
        }
        if max_frames == 0 {
            return Err(EffectPreparationError::InvalidFrameCapacity);
        }
        if config.capability_id() != &self.capability_id {
            return Err(EffectPreparationError::UnsupportedCapability { patch_id });
        }
        let capability = ReverbCapability::new()
            .map_err(|_| EffectPreparationError::InvalidConfiguration { patch_id })?;
        let canonical = capability
            .create_config(config.slot_id(), config.values(), config.asset_references())
            .map_err(|_| EffectPreparationError::InvalidConfiguration { patch_id })?;
        if &canonical != config {
            return Err(EffectPreparationError::InvalidConfiguration { patch_id });
        }
        let reverb = StereoReverb::prepared(sample_rate)
            .ok_or(EffectPreparationError::StorageAllocationFailed { patch_id })?;
        Ok(Box::new(PreparedReverb {
            patch_id,
            slot_id: config.slot_id(),
            reverb,
            max_frames,
        }))
    }
}

struct PreparedReverb {
    patch_id: PatchId,
    slot_id: EffectSlotId,
    reverb: StereoReverb,
    max_frames: usize,
}

impl PreparedPostEffect for PreparedReverb {
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
        let room_size = parameters
            .scalar(0)
            .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
            .ok_or(PreparedEffectError::ScalarLayoutMismatch)?;
        let damping = parameters
            .scalar(1)
            .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
            .ok_or(PreparedEffectError::ScalarLayoutMismatch)?;
        // Wet excitation derives exclusively from the samples handed in; the
        // buffer is replaced with the wet return so zero input from cleared
        // state cannot create a wet return.
        let mut frame = 0;
        while frame < frame_count {
            let left = frame * 2;
            let right = left + 1;
            let (wet_left, wet_right) = self.reverb.process(
                interleaved_stereo[left],
                interleaved_stereo[right],
                room_size,
                damping,
            );
            interleaved_stereo[left] = wet_left;
            interleaved_stereo[right] = wet_right;
            frame += 1;
        }
        Ok(())
    }
}

/// The retired `GlobalReverbDelay` reverb DSP, moved verbatim: two unequal
/// feedback comb lines with one one-pole damping filter per channel.
#[derive(Debug, Default)]
struct StereoReverb {
    left_buffer: Vec<f32>,
    right_buffer: Vec<f32>,
    left_index: usize,
    right_index: usize,
    left_damping_state: f32,
    right_damping_state: f32,
}

impl StereoReverb {
    fn prepared(sample_rate: f32) -> Option<Self> {
        let left_length = milliseconds_to_samples(sample_rate, REVERB_LEFT_MILLISECONDS)?;
        let right_length = milliseconds_to_samples(sample_rate, REVERB_RIGHT_MILLISECONDS)?;

        Some(Self {
            left_buffer: allocate_zeros(left_length)?,
            right_buffer: allocate_zeros(right_length)?,
            left_index: 0,
            right_index: 0,
            left_damping_state: 0.0,
            right_damping_state: 0.0,
        })
    }

    fn process(
        &mut self,
        left_input: f32,
        right_input: f32,
        room_size: f32,
        damping: f32,
    ) -> (f32, f32) {
        let left_delayed = self.left_buffer[self.left_index];
        let right_delayed = self.right_buffer[self.right_index];
        let undamped = 1.0 - damping;

        let left_filtered = left_delayed * undamped + self.left_damping_state * damping;
        let right_filtered = right_delayed * undamped + self.right_damping_state * damping;
        self.left_damping_state = left_filtered;
        self.right_damping_state = right_filtered;

        let feedback = 0.2 + room_size * 0.75;
        self.left_buffer[self.left_index] = left_input + left_filtered * feedback;
        self.right_buffer[self.right_index] = right_input + right_filtered * feedback;

        self.left_index += 1;
        if self.left_index == self.left_buffer.len() {
            self.left_index = 0;
        }
        self.right_index += 1;
        if self.right_index == self.right_buffer.len() {
            self.right_index = 0;
        }

        (left_filtered, right_filtered)
    }
}

fn milliseconds_to_samples(sample_rate: f32, milliseconds: f32) -> Option<usize> {
    let sample_count = (f64::from(sample_rate) * f64::from(milliseconds) / 1000.0).ceil();
    if !sample_count.is_finite() || sample_count > (usize::MAX - 1) as f64 {
        return None;
    }
    Some((sample_count as usize).max(1))
}

fn allocate_zeros(length: usize) -> Option<Vec<f32>> {
    let mut storage = Vec::new();
    storage.try_reserve_exact(length).ok()?;
    storage.resize(length, 0.0);
    Some(storage)
}

#[cfg(test)]
mod tests {
    use super::ReverbPreparer;
    use crate::adapter::reverb_capability::ReverbCapability;
    use crate::kernel::PatchId;
    use crate::real_time::RtPostEffectParameters;
    use crate::synth::{
        EffectPreparationError, EffectPreparer, EffectSlotId, ParameterAssignment, ParameterId,
        ParameterValue, PostEffectConfig, PreparedEffectError, PreparedPostEffect,
    };

    const SAMPLE_RATE: f32 = 1_000.0;
    const MAX_FRAMES: usize = 64;

    fn default_config(slot: u16) -> PostEffectConfig {
        ReverbCapability::new()
            .unwrap()
            .default_config(EffectSlotId::new(slot).unwrap())
            .unwrap()
    }

    fn prepared(patch: u32, slot: u16) -> Box<dyn PreparedPostEffect> {
        ReverbPreparer::new()
            .unwrap()
            .prepare(
                PatchId::new(patch).unwrap(),
                &default_config(slot),
                SAMPLE_RATE,
                MAX_FRAMES,
            )
            .expect("valid reverb preparation succeeds")
    }

    fn parameters(slot: u16, room_size: f32, damping: f32) -> RtPostEffectParameters {
        RtPostEffectParameters::new(EffectSlotId::new(slot).unwrap(), &[room_size, damping])
            .unwrap()
    }

    fn render(instance: &mut Box<dyn PreparedPostEffect>, slot: u16, input: &[f32]) -> Vec<f32> {
        let mut buffer = input.to_vec();
        instance
            .process(&mut buffer, input.len() / 2, &parameters(slot, 0.6, 0.25))
            .unwrap();
        buffer
    }

    #[test]
    fn one_entry_prepares_into_both_roles_with_independent_instances() {
        // C-ER-1: the same registry entry serves a Patch-slot caller and a
        // return caller with no role hint; the instances are independent.
        let mut patch_slot_instance = prepared(1, 1);
        let mut return_instance = prepared(u32::MAX, 2);

        let mut impulse = vec![0.0; MAX_FRAMES * 2];
        impulse[0] = 1.0;
        impulse[1] = 1.0;
        let excited = render(&mut patch_slot_instance, 1, &impulse);
        let silent = render(&mut return_instance, 2, &vec![0.0; MAX_FRAMES * 2]);

        assert!(
            excited.iter().any(|sample| sample.abs() > 0.0),
            "the excited instance produces a wet return"
        );
        assert!(
            silent.iter().all(|sample| *sample == 0.0),
            "C-ER-2: the second instance shares no delay lines or tails"
        );
    }

    #[test]
    fn zero_input_cannot_create_a_wet_return() {
        let mut instance = prepared(1, 1);

        let output = render(&mut instance, 1, &vec![0.0; MAX_FRAMES * 2]);

        assert!(output.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn wet_excitation_derives_only_from_the_declared_input() {
        // Two identical instances fed identical input must produce identical
        // output regardless of what the caller previously left in other
        // buffers: process has no channel other than the handed-in samples.
        let mut first = prepared(1, 1);
        let mut second = prepared(2, 1);
        let mut input = vec![0.0; MAX_FRAMES * 2];
        input[0] = 0.8;
        input[3] = -0.4;

        assert_eq!(
            render(&mut first, 1, &input),
            render(&mut second, 1, &input)
        );
    }

    #[test]
    fn preparation_refusals_are_typed_and_publish_no_instance() {
        // C-ER-4: every refusal is a typed error, never a partial instance.
        let preparer = ReverbPreparer::new().unwrap();
        let patch_id = PatchId::new(1).unwrap();
        let config = default_config(1);

        assert!(matches!(
            preparer.prepare(patch_id, &config, f32::NAN, MAX_FRAMES),
            Err(EffectPreparationError::InvalidSampleRate)
        ));
        assert!(matches!(
            preparer.prepare(patch_id, &config, SAMPLE_RATE, 0),
            Err(EffectPreparationError::InvalidFrameCapacity)
        ));

        let foreign = PostEffectConfig::from_parts(
            EffectSlotId::new(1).unwrap(),
            crate::synth::EffectCapabilityId::new("effect.other").unwrap(),
            Vec::new(),
            Vec::new(),
        );
        assert!(matches!(
            preparer.prepare(patch_id, &foreign, SAMPLE_RATE, MAX_FRAMES),
            Err(EffectPreparationError::UnsupportedCapability { .. })
        ));

        let out_of_range = PostEffectConfig::from_parts(
            config.slot_id(),
            config.capability_id().clone(),
            vec![
                ParameterAssignment::new(
                    ParameterId::new("reverb.room-size").unwrap(),
                    ParameterValue::continuous(2.0).unwrap(),
                ),
                ParameterAssignment::new(
                    ParameterId::new("reverb.damping").unwrap(),
                    ParameterValue::continuous(0.5).unwrap(),
                ),
            ],
            Vec::new(),
        );
        assert!(matches!(
            preparer.prepare(patch_id, &out_of_range, SAMPLE_RATE, MAX_FRAMES),
            Err(EffectPreparationError::InvalidConfiguration { .. })
        ));
    }

    #[test]
    fn process_rejects_mismatched_layouts_without_touching_audio() {
        let mut instance = prepared(1, 1);
        let mut buffer = vec![0.25; MAX_FRAMES * 2];

        assert_eq!(
            instance.process(&mut buffer, MAX_FRAMES + 1, &parameters(1, 0.5, 0.5)),
            Err(PreparedEffectError::FrameCapacityExceeded)
        );
        assert_eq!(
            instance.process(&mut buffer, MAX_FRAMES, &parameters(2, 0.5, 0.5)),
            Err(PreparedEffectError::ScalarLayoutMismatch)
        );
        assert_eq!(
            instance.process(
                &mut buffer,
                MAX_FRAMES,
                &RtPostEffectParameters::new(EffectSlotId::new(1).unwrap(), &[0.5]).unwrap()
            ),
            Err(PreparedEffectError::ScalarLayoutMismatch)
        );
        assert_eq!(
            instance.process(&mut buffer, MAX_FRAMES, &parameters(1, 1.5, 0.5)),
            Err(PreparedEffectError::ScalarLayoutMismatch)
        );
        assert!(buffer.iter().all(|sample| *sample == 0.25));
    }
}
