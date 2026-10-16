use crate::synth::voice_envelope::VoiceEnvelope;

/// One bounded sample-domain stage for an engine-owned note voice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VoiceEnvelopeStage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// Destructor-free ADSR state embedded in one prepared note voice.
///
/// Attack, Decay, and Sustain are latched at note-on. Release is latched at
/// note-off. Stage transitions contain no loops and zero-time stages collapse
/// immediately in bounded work.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceEnvelopeState {
    stage: VoiceEnvelopeStage,
    level: f32,
    remaining_samples: u32,
    increment: f32,
    latched_decay_milliseconds: f32,
    latched_sustain: f32,
}

impl VoiceEnvelopeState {
    pub const IDLE: Self = Self {
        stage: VoiceEnvelopeStage::Idle,
        level: 0.0,
        remaining_samples: 0,
        increment: 0.0,
        latched_decay_milliseconds: 0.0,
        latched_sustain: 0.0,
    };

    pub const fn new() -> Self {
        Self::IDLE
    }

    pub const fn stage(&self) -> VoiceEnvelopeStage {
        self.stage
    }

    pub const fn level(&self) -> f32 {
        self.level
    }

    pub const fn is_idle(&self) -> bool {
        matches!(self.stage, VoiceEnvelopeStage::Idle)
    }

    pub const fn is_releasing(&self) -> bool {
        matches!(self.stage, VoiceEnvelopeStage::Release)
    }

    /// Restarts the voice and latches Attack/Decay/Sustain from this snapshot.
    pub fn note_on(&mut self, envelope: VoiceEnvelope, sample_rate: f32) {
        self.level = 0.0;
        self.latched_decay_milliseconds = envelope.decay_milliseconds();
        self.latched_sustain = envelope.sustain();
        let attack_samples = milliseconds_to_samples(envelope.attack_milliseconds(), sample_rate);
        if attack_samples == 0 {
            self.begin_decay(sample_rate);
        } else {
            self.stage = VoiceEnvelopeStage::Attack;
            self.remaining_samples = attack_samples;
            self.increment = 1.0 / attack_samples as f32;
        }
    }

    /// Begins release from the exact current level and latches this duration.
    /// Repeated note-offs cannot restart an older voice's latched release.
    pub fn note_off(&mut self, release_milliseconds: f32, sample_rate: f32) {
        if self.is_idle() || self.is_releasing() {
            return;
        }
        let release_samples = milliseconds_to_samples(release_milliseconds, sample_rate);
        if release_samples == 0 || self.level <= 0.0 {
            self.clear();
        } else {
            self.stage = VoiceEnvelopeStage::Release;
            self.remaining_samples = release_samples;
            self.increment = -self.level / release_samples as f32;
        }
    }

    /// Advances exactly one output sample and returns a finite bounded gain.
    #[inline]
    pub fn next_gain(&mut self, sample_rate: f32) -> f32 {
        match self.stage {
            VoiceEnvelopeStage::Idle => 0.0,
            VoiceEnvelopeStage::Sustain => self.level,
            VoiceEnvelopeStage::Attack
            | VoiceEnvelopeStage::Decay
            | VoiceEnvelopeStage::Release => {
                self.level = (self.level + self.increment).clamp(0.0, 1.0);
                self.remaining_samples = self.remaining_samples.saturating_sub(1);
                if self.remaining_samples == 0 {
                    match self.stage {
                        VoiceEnvelopeStage::Attack => self.begin_decay(sample_rate),
                        VoiceEnvelopeStage::Decay => self.begin_sustain(),
                        VoiceEnvelopeStage::Release => self.clear(),
                        VoiceEnvelopeStage::Idle | VoiceEnvelopeStage::Sustain => {}
                    }
                }
                self.level
            }
        }
    }

    /// Fills a prepared block with the same gains and final state as repeated
    /// `next_gain` calls. Stage dispatch occurs at boundaries, not each sample.
    pub fn fill_gains(&mut self, mut output: &mut [f32], sample_rate: f32) {
        while !output.is_empty() {
            match self.stage {
                VoiceEnvelopeStage::Idle | VoiceEnvelopeStage::Sustain => {
                    output.fill(self.level);
                    return;
                }
                VoiceEnvelopeStage::Attack
                | VoiceEnvelopeStage::Decay
                | VoiceEnvelopeStage::Release => {
                    // Leave the transition sample to the scalar state machine:
                    // it may latch decay or return the sustain/idle level.
                    let count = output
                        .len()
                        .min(self.remaining_samples.saturating_sub(1) as usize);
                    let (within_stage, remainder) = output.split_at_mut(count);
                    let mut level = self.level;
                    // Private stage state is finite and bounded. Attack only
                    // rises; decay/release only fall. Keep each scalar addition
                    // and its reachable clamp, without testing the other bound
                    // on every sample. The transition still uses next_gain.
                    if self.stage == VoiceEnvelopeStage::Attack {
                        for gain in within_stage {
                            level = (level + self.increment).min(1.0);
                            *gain = level;
                        }
                    } else {
                        for gain in within_stage {
                            level = (level + self.increment).max(0.0);
                            *gain = level;
                        }
                    }
                    self.level = level;
                    self.remaining_samples -= count as u32;
                    output = remainder;
                    let Some((transition, remainder)) = output.split_first_mut() else {
                        return;
                    };
                    *transition = self.next_gain(sample_rate);
                    output = remainder;
                }
            }
        }
    }

    /// Immediately returns the slot to its reusable idle state.
    pub fn clear(&mut self) {
        *self = Self::IDLE;
    }

    fn begin_decay(&mut self, sample_rate: f32) {
        self.level = 1.0;
        let decay_samples = milliseconds_to_samples(self.latched_decay_milliseconds, sample_rate);
        if decay_samples == 0 {
            self.begin_sustain();
        } else {
            self.stage = VoiceEnvelopeStage::Decay;
            self.remaining_samples = decay_samples;
            self.increment = (self.latched_sustain - 1.0) / decay_samples as f32;
        }
    }

    fn begin_sustain(&mut self) {
        self.stage = VoiceEnvelopeStage::Sustain;
        self.level = self.latched_sustain;
        self.remaining_samples = 0;
        self.increment = 0.0;
    }
}

impl Default for VoiceEnvelopeState {
    fn default() -> Self {
        Self::IDLE
    }
}

fn milliseconds_to_samples(milliseconds: f32, sample_rate: f32) -> u32 {
    if !milliseconds.is_finite()
        || !sample_rate.is_finite()
        || milliseconds <= 0.0
        || sample_rate <= 0.0
    {
        return 0;
    }
    let samples = f64::from(milliseconds) * f64::from(sample_rate) / 1_000.0;
    samples.round().clamp(1.0, f64::from(u32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use super::{VoiceEnvelopeStage, VoiceEnvelopeState};
    use crate::synth::voice_envelope::VoiceEnvelope;

    #[test]
    fn zero_time_stages_collapse_without_a_loop() {
        let mut state = VoiceEnvelopeState::new();
        state.note_on(VoiceEnvelope::DEFAULT, 48_000.0);
        assert_eq!(state.stage(), VoiceEnvelopeStage::Sustain);
        assert_eq!(state.next_gain(48_000.0), 1.0);

        state.note_off(0.0, 48_000.0);
        assert!(state.is_idle());
        assert_eq!(state.next_gain(48_000.0), 0.0);
    }

    #[test]
    fn attack_decay_and_release_advance_exactly_one_sample_at_a_time() {
        let envelope = VoiceEnvelope::new(2.0, 2.0, 0.5, 4.0).unwrap();
        let mut state = VoiceEnvelopeState::new();
        state.note_on(envelope, 1_000.0);

        assert_eq!(state.next_gain(1_000.0), 0.5);
        assert_eq!(state.next_gain(1_000.0), 1.0);
        assert_eq!(state.next_gain(1_000.0), 0.75);
        assert_eq!(state.next_gain(1_000.0), 0.5);
        assert_eq!(state.stage(), VoiceEnvelopeStage::Sustain);

        state.note_off(envelope.release_milliseconds(), 1_000.0);
        assert_eq!(state.next_gain(1_000.0), 0.375);
        assert_eq!(state.next_gain(1_000.0), 0.25);
        assert_eq!(state.next_gain(1_000.0), 0.125);
        assert_eq!(state.next_gain(1_000.0), 0.0);
        assert!(state.is_idle());
    }

    #[test]
    fn note_on_and_note_off_latch_independent_snapshot_fields() {
        let note_on = VoiceEnvelope::new(2.0, 2.0, 0.5, 1.0).unwrap();
        let note_off = VoiceEnvelope::new(0.0, 0.0, 1.0, 4.0).unwrap();
        let mut first = VoiceEnvelopeState::new();
        let mut second = VoiceEnvelopeState::new();
        first.note_on(note_on, 1_000.0);
        second.note_on(VoiceEnvelope::DEFAULT, 1_000.0);

        assert_eq!(first.next_gain(1_000.0), 0.5);
        assert_eq!(second.next_gain(1_000.0), 1.0);
        first.note_off(note_off.release_milliseconds(), 1_000.0);
        assert_eq!(first.next_gain(1_000.0), 0.375);
        assert_eq!(second.next_gain(1_000.0), 1.0);
    }

    #[test]
    fn repeated_note_off_preserves_the_original_release_and_reclaims_its_voice() {
        let mut state = VoiceEnvelopeState::new();
        state.note_on(VoiceEnvelope::DEFAULT, 1_000.0);
        state.note_off(40.0, 1_000.0);
        let mut reference = state;
        for sample in 0..40 {
            if sample % 8 == 0 {
                // A later note on the same key can end while this older voice
                // is still releasing. It must not relatch or extend this tail.
                state.note_off(10_000.0, 1_000.0);
            }
            assert_eq!(state.next_gain(1_000.0), reference.next_gain(1_000.0));
        }
        assert!(state.is_idle());
        state.note_on(VoiceEnvelope::DEFAULT, 1_000.0);
        assert_eq!(state.next_gain(1_000.0), 1.0);
    }

    #[test]
    fn block_gains_match_scalar_clocks_across_stages_and_retriggers() {
        let envelopes = [
            VoiceEnvelope::DEFAULT,
            VoiceEnvelope::new(1.3, 2.7, 0.37, 9.7).unwrap(),
            VoiceEnvelope::new(0.0, 0.2, 0.0, 0.0).unwrap(),
            VoiceEnvelope::new(0.2, 0.0, 1.0, 0.3).unwrap(),
            VoiceEnvelope::new(0.0001, 0.0001, -0.0, 0.0001).unwrap(),
            VoiceEnvelope::new(10_000.0, 10_000.0, 0.37, 10_000.0).unwrap(),
        ];
        for envelope in envelopes {
            for rate in [1.0, 1_000.0, 44_100.0, 48_000.0, 96_000.0, f32::MAX] {
                let mut scalar = VoiceEnvelopeState::new();
                let mut block = scalar;
                for step in 0..96 {
                    if step % 24 == 0 {
                        scalar.note_on(envelope, rate);
                        block.note_on(envelope, rate);
                    }
                    if step % 24 == 5 || step % 24 == 7 {
                        scalar.note_off(envelope.release_milliseconds(), rate);
                        block.note_off(envelope.release_milliseconds(), rate);
                    }
                    let frames = [0, 1, 7, 64, 127, 512][step % 6];
                    let mut actual = [f32::NAN; 512];
                    block.fill_gains(&mut actual[..frames], rate);
                    for gain in &actual[..frames] {
                        assert_eq!(gain.to_bits(), scalar.next_gain(rate).to_bits());
                    }
                    assert_eq!(block, scalar);
                }
            }
        }
    }

    #[test]
    fn extreme_finite_times_and_rates_remain_finite_and_destructor_free() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<VoiceEnvelopeState>();
        assert!(!core::mem::needs_drop::<VoiceEnvelopeState>());

        let mut state = VoiceEnvelopeState::new();
        state.note_on(
            VoiceEnvelope::new(10_000.0, 10_000.0, 0.0, 10_000.0).unwrap(),
            f32::MAX,
        );
        for _ in 0..32 {
            assert!(state.next_gain(f32::MAX).is_finite());
        }
    }
}
