//! Display measurements retained before lossy UI decimation. Raw block evidence
//! stays separate; these envelopes never affect rendered audio.
use crate::mixer::{mix_observation::MixObservation, mixer_track_id::MixerTrackId};
use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct MeterLevel {
    pub rms: f32,
    pub peak: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeterLevels {
    pub tracks: [MeterLevel; MixerTrackId::COUNT],
    pub output: MeterLevel,
    /// Cumulative pre-device overloads, including blocks the UI never observes.
    pub clipped_samples: u64,
}

#[derive(Clone, Copy, Default)]
struct Envelope {
    energy: f64,
    peak: f32,
    hold: f32,
}

impl Envelope {
    fn advance(&mut self, rms: f32, peak: f32, seconds: f32, blend: f64) -> MeterLevel {
        self.energy += (f64::from(rms).powi(2) - self.energy) * blend;
        // A one-second peak hold catches transients between UI observations.
        // Release is 12 dB/second; attack is immediate.
        if peak >= self.peak {
            self.peak = peak;
            self.hold = 1.0;
        } else {
            let release = (seconds - self.hold).max(0.0);
            self.hold = (self.hold - seconds).max(0.0);
            self.peak = peak.max(self.peak * 10.0_f32.powf(-12.0 * release / 20.0));
        }
        MeterLevel {
            rms: self.energy.sqrt() as f32,
            peak: self.peak,
        }
    }
}

#[derive(Default)]
pub(crate) struct MeterBallistics {
    tracks: [Envelope; MixerTrackId::COUNT],
    output: Envelope,
    clipped_samples: u64,
}

impl MeterBallistics {
    pub(crate) fn advance(&mut self, mix: MixObservation, frames: usize, rate: f32) -> MeterLevels {
        let seconds = frames as f32 / rate;
        // 200 ms energy averaging keeps sustained levels readable.
        let blend = 1.0 - (-f64::from(seconds) / 0.2).exp();
        let tracks = std::array::from_fn(|index| {
            let raw = mix.tracks()[index];
            self.tracks[index].advance(
                raw.rms(),
                raw.left_peak().max(raw.right_peak()),
                seconds,
                blend,
            )
        });
        self.clipped_samples = self.clipped_samples.saturating_add(mix.clipped_samples());
        MeterLevels {
            tracks,
            output: self.output.advance(
                mix.output_rms(),
                mix.left_peak().max(mix.right_peak()),
                seconds,
                blend,
            ),
            clipped_samples: self.clipped_samples,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixer::track_meter::TrackMeter;

    fn signal(rms: f32, peak: f32, clips: u64) -> MixObservation {
        MixObservation::new(
            [TrackMeter::ZERO; MixerTrackId::COUNT],
            peak,
            peak,
            rms,
            0.0,
            0.0,
            0.0,
            0,
            clips,
        )
    }

    #[test]
    fn missed_transient_remains_visible_and_clipping_is_cumulative() {
        let mut meter = MeterBallistics::default();
        meter.advance(signal(0.1, 1.4, 2), 256, 48_000.0);
        let mut latest = MeterLevels::default();
        for _ in 0..100 {
            latest = meter.advance(signal(0.0, 0.0, 0), 256, 48_000.0);
        }
        assert_eq!(latest.output.peak, 1.4);
        assert_eq!(latest.clipped_samples, 2);
        for _ in 0..300 {
            latest = meter.advance(signal(0.0, 0.0, 0), 256, 48_000.0);
        }
        assert!(latest.output.peak < 0.4);
        assert_eq!(latest.clipped_samples, 2);
    }

    #[test]
    fn energy_average_is_independent_of_callback_size_and_smooths_silence() {
        let mut small = MeterBallistics::default();
        let mut large = MeterBallistics::default();
        let mut result = MeterLevels::default();
        for _ in 0..100 {
            result = small.advance(signal(0.5, 0.7, 0), 256, 48_000.0);
        }
        let other = large.advance(signal(0.5, 0.7, 0), 25_600, 48_000.0);
        assert!((result.output.rms - other.output.rms).abs() < 0.00001);
        let falling = small.advance(signal(0.0, 0.0, 0), 256, 48_000.0);
        assert!(
            falling.output.rms < result.output.rms && falling.output.rms > result.output.rms * 0.98
        );
    }
}
