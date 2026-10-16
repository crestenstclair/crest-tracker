use crate::adapter::delay_capability::{
    DelayCapability, DELAY_CAPABILITY_ID, DELAY_MAX_DELAY_MILLISECONDS, DELAY_MILLISECONDS_MAXIMUM,
    DELAY_MILLISECONDS_MINIMUM,
};
use crate::kernel::PatchId;
use crate::real_time::RtPostEffectParameters;
use crate::synth::{
    EffectCapabilityId, EffectCapabilityProvider, EffectPreparationError, EffectPreparer,
    EffectSlotId, PostEffectConfig, PreparedEffectError, PreparedPostEffect,
};

/// Registry preparer for the shared stereo feedback Delay.
///
/// The DSP is the retired `GlobalReverbDelay` delay moved unchanged behind the
/// generic prepared-effect boundary: algorithm and delay-line sizing contract
/// are identical. Preparation sizes every delay line for the entry's declared
/// maximum-delay requirement, so an in-range delay time is never silently
/// truncated. The same preparer serves Patch effect slots and bus returns;
/// each preparation yields an independent instance.
pub struct DelayPreparer {
    capability_id: EffectCapabilityId,
}

impl DelayPreparer {
    pub fn new() -> Result<Self, EffectPreparationError> {
        let capability_id = EffectCapabilityId::new(DELAY_CAPABILITY_ID).map_err(|_| {
            EffectPreparationError::PreparationFailed {
                patch_id: PatchId::new(1).expect("static error Patch id is valid"),
            }
        })?;
        Ok(Self { capability_id })
    }
}

impl EffectPreparer for DelayPreparer {
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
        let capability = DelayCapability::new()
            .map_err(|_| EffectPreparationError::InvalidConfiguration { patch_id })?;
        let canonical = capability
            .create_config(config.slot_id(), config.values(), config.asset_references())
            .map_err(|_| EffectPreparationError::InvalidConfiguration { patch_id })?;
        if &canonical != config {
            return Err(EffectPreparationError::InvalidConfiguration { patch_id });
        }
        let delay = StereoFeedbackDelay::prepared(sample_rate, DELAY_MAX_DELAY_MILLISECONDS)
            .ok_or(EffectPreparationError::StorageAllocationFailed { patch_id })?;
        Ok(Box::new(PreparedDelay {
            patch_id,
            slot_id: config.slot_id(),
            delay,
            max_frames,
        }))
    }
}

struct PreparedDelay {
    patch_id: PatchId,
    slot_id: EffectSlotId,
    delay: StereoFeedbackDelay,
    max_frames: usize,
}

impl PreparedPostEffect for PreparedDelay {
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
        let milliseconds = parameters
            .scalar(0)
            .filter(|value| {
                value.is_finite()
                    && (DELAY_MILLISECONDS_MINIMUM as f32..=DELAY_MILLISECONDS_MAXIMUM as f32)
                        .contains(value)
            })
            .ok_or(PreparedEffectError::ScalarLayoutMismatch)?;
        let feedback = parameters
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
            let (wet_left, wet_right) = self.delay.process(
                interleaved_stereo[left],
                interleaved_stereo[right],
                milliseconds,
                feedback,
            );
            interleaved_stereo[left] = wet_left;
            interleaved_stereo[right] = wet_right;
            frame += 1;
        }
        Ok(())
    }
}

/// The retired `GlobalReverbDelay` delay DSP, moved verbatim: one shared
/// write index over two equal-length feedback delay lines.
#[derive(Debug, Default)]
struct StereoFeedbackDelay {
    left_buffer: Vec<f32>,
    right_buffer: Vec<f32>,
    write_index: usize,
    sample_rate: f32,
}

impl StereoFeedbackDelay {
    fn prepared(sample_rate: f32, max_delay_milliseconds: f32) -> Option<Self> {
        let max_delay_samples = milliseconds_to_samples(sample_rate, max_delay_milliseconds)?;
        let buffer_length = max_delay_samples.checked_add(1)?;

        Some(Self {
            left_buffer: allocate_zeros(buffer_length)?,
            right_buffer: allocate_zeros(buffer_length)?,
            write_index: 0,
            sample_rate,
        })
    }

    fn process(
        &mut self,
        left_input: f32,
        right_input: f32,
        delay_milliseconds: f32,
        feedback: f32,
    ) -> (f32, f32) {
        let requested_delay = (delay_milliseconds * self.sample_rate / 1000.0).round() as usize;
        let delay_samples = requested_delay.clamp(1, self.left_buffer.len() - 1);
        let read_index =
            (self.write_index + self.left_buffer.len() - delay_samples) % self.left_buffer.len();

        let left_delayed = self.left_buffer[read_index];
        let right_delayed = self.right_buffer[read_index];

        self.left_buffer[self.write_index] = left_input + left_delayed * feedback;
        self.right_buffer[self.write_index] = right_input + right_delayed * feedback;

        self.write_index += 1;
        if self.write_index == self.left_buffer.len() {
            self.write_index = 0;
        }

        (left_delayed, right_delayed)
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
    use super::DelayPreparer;
    use crate::adapter::delay_capability::DelayCapability;
    use crate::kernel::PatchId;
    use crate::real_time::RtPostEffectParameters;
    use crate::synth::{
        EffectPreparationError, EffectPreparer, EffectSlotId, ParameterAssignment, ParameterId,
        ParameterValue, PostEffectConfig, PreparedEffectError, PreparedPostEffect,
    };

    const SAMPLE_RATE: f32 = 1_000.0;
    const MAX_FRAMES: usize = 64;

    fn default_config(slot: u16) -> PostEffectConfig {
        DelayCapability::new()
            .unwrap()
            .default_config(EffectSlotId::new(slot).unwrap())
            .unwrap()
    }

    fn prepared(patch: u32, slot: u16) -> Box<dyn PreparedPostEffect> {
        DelayPreparer::new()
            .unwrap()
            .prepare(
                PatchId::new(patch).unwrap(),
                &default_config(slot),
                SAMPLE_RATE,
                MAX_FRAMES,
            )
            .expect("valid delay preparation succeeds")
    }

    fn parameters(slot: u16, milliseconds: f32, feedback: f32) -> RtPostEffectParameters {
        RtPostEffectParameters::new(EffectSlotId::new(slot).unwrap(), &[milliseconds, feedback])
            .unwrap()
    }

    fn render(
        instance: &mut Box<dyn PreparedPostEffect>,
        slot: u16,
        input: &[f32],
        milliseconds: f32,
        feedback: f32,
    ) -> Vec<f32> {
        let mut buffer = input.to_vec();
        instance
            .process(
                &mut buffer,
                input.len() / 2,
                &parameters(slot, milliseconds, feedback),
            )
            .unwrap();
        buffer
    }

    #[test]
    fn delayed_stereo_feedback_matches_the_retired_behavior() {
        let mut instance = prepared(1, 1);
        let mut input = vec![0.0; MAX_FRAMES * 2];
        input[0] = 1.0;
        input[1] = 0.5;

        let output = render(&mut instance, 1, &input, 10.0, 0.5);

        // 10 ms at 1 kHz is 10 frames; feedback halves each recurrence.
        assert!((output[20] - 1.0).abs() < f32::EPSILON);
        assert!((output[21] - 0.5).abs() < f32::EPSILON);
        assert!((output[40] - 0.5).abs() < f32::EPSILON);
        assert!((output[41] - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn one_entry_prepares_into_both_roles_with_independent_instances() {
        // C-ER-1: the same registry entry serves a Patch-slot caller and a
        // return caller with no role hint; the instances are independent.
        let mut patch_slot_instance = prepared(1, 1);
        let mut return_instance = prepared(u32::MAX, 2);

        let mut impulse = vec![0.0; MAX_FRAMES * 2];
        impulse[0] = 1.0;
        let excited = render(&mut patch_slot_instance, 1, &impulse, 10.0, 0.5);
        let silent = render(
            &mut return_instance,
            2,
            &vec![0.0; MAX_FRAMES * 2],
            10.0,
            0.5,
        );

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

        let output = render(&mut instance, 1, &vec![0.0; MAX_FRAMES * 2], 10.0, 1.0);

        assert!(output.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn the_complete_declared_delay_range_is_prepared_without_truncation() {
        // The declared maximum-delay requirement sizes the delay lines, so the
        // full 2000 ms range is honored at any prepared sample rate.
        let mut instance = prepared(1, 1);
        let mut input = vec![0.0; MAX_FRAMES * 2];
        input[0] = 1.0;

        let output = render(&mut instance, 1, &input, 2000.0, 0.0);
        assert!(
            output.iter().all(|sample| *sample == 0.0),
            "a 2000 ms delay at 1 kHz cannot recur inside 64 frames"
        );

        // The echo arrives exactly 2000 frames after the impulse: frame 16 of
        // block 31, which is interleaved sample 32.
        let mut remaining = 2000 / MAX_FRAMES;
        let mut echo = Vec::new();
        while remaining > 0 {
            echo = render(&mut instance, 1, &vec![0.0; MAX_FRAMES * 2], 2000.0, 0.0);
            remaining -= 1;
        }
        assert!(echo[..32].iter().all(|sample| *sample == 0.0));
        assert!((echo[32] - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn preparation_refusals_are_typed_and_publish_no_instance() {
        // C-ER-4: every refusal is a typed error, never a partial instance.
        let preparer = DelayPreparer::new().unwrap();
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

        let out_of_range = PostEffectConfig::from_parts(
            config.slot_id(),
            config.capability_id().clone(),
            vec![
                ParameterAssignment::new(
                    ParameterId::new("delay.milliseconds").unwrap(),
                    ParameterValue::continuous(2000.1).unwrap(),
                ),
                ParameterAssignment::new(
                    ParameterId::new("delay.feedback").unwrap(),
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
            instance.process(&mut buffer, MAX_FRAMES + 1, &parameters(1, 250.0, 0.5)),
            Err(PreparedEffectError::FrameCapacityExceeded)
        );
        assert_eq!(
            instance.process(&mut buffer, MAX_FRAMES, &parameters(2, 250.0, 0.5)),
            Err(PreparedEffectError::ScalarLayoutMismatch)
        );
        assert_eq!(
            instance.process(&mut buffer, MAX_FRAMES, &parameters(1, 0.5, 0.5)),
            Err(PreparedEffectError::ScalarLayoutMismatch),
            "a delay time below the declared minimum is rejected, not clamped"
        );
        assert_eq!(
            instance.process(&mut buffer, MAX_FRAMES, &parameters(1, 2000.5, 0.5)),
            Err(PreparedEffectError::ScalarLayoutMismatch),
            "a delay time above the declared maximum is rejected, not truncated"
        );
        assert!(buffer.iter().all(|sample| *sample == 0.25));
    }
}
