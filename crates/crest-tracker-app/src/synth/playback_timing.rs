//! Playback publication covers whole native buffers, then the control budget.
//! These frame counts need no MIDI/device host-time correlation.
use super::HostClock;

pub(super) fn launch_frame(clock: HostClock) -> Result<u64, String> {
    frame_after(clock, 20) // Existing 50 ms launch/control margin.
}

pub(super) fn horizon_frame(clock: HostClock) -> Result<u64, String> {
    frame_after(clock, 10) // Existing 100 ms publication/control margin.
}

fn frame_after(clock: HostClock, rate_divisor: u32) -> Result<u64, String> {
    let device = clock.audio_clock.ok_or("Audio buffer clock is not ready")?;
    if device.sample_rate() != clock.sample_rate {
        return Err("Audio buffer clock sample rate changed".into());
    }
    // The clock is published before rendering. Never schedule inside that
    // reserved buffer, even if the completed-frame observation is older. Cover
    // the next whole callback as well: it may start before publication finishes.
    device
        .frame_end()
        .max(clock.audio.rendered_frames())
        .checked_add(device.buffer_frames())
        .and_then(|frame| frame.checked_add(u64::from(clock.sample_rate / rate_divisor)))
        .ok_or_else(|| "Audio clock overflow".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crest_synth::shell::audio_output::{AudioClockDomain, AudioFrameClock};

    fn clock(frames: u64) -> HostClock {
        let mut clock = super::super::tests::clock();
        // Unknown host epoch is sufficient for frame-based playback, but still
        // cannot correlate physical MIDI timestamps for live monitoring.
        clock.audio_clock =
            AudioFrameClock::new(10_000, frames, 1, 1, 48_000, AudioClockDomain::Unknown);
        clock
    }

    #[test]
    fn publication_covers_reserved_and_next_buffers_plus_control_margin() {
        for frames in [256, 8192] {
            let clock = clock(frames);
            assert_eq!(launch_frame(clock).unwrap(), 10_000 + frames * 2 + 2400);
            assert_eq!(horizon_frame(clock).unwrap(), 10_000 + frames * 2 + 4800);
        }
    }

    #[test]
    fn absent_incompatible_or_overflowing_clock_cannot_promise_a_window() {
        let mut clock = clock(8192);
        clock.sample_rate = 44_100;
        assert!(launch_frame(clock).unwrap_err().contains("sample rate"));
        clock.sample_rate = 48_000;
        clock.audio_clock = None;
        assert!(horizon_frame(clock).unwrap_err().contains("not ready"));
        clock.audio_clock = AudioFrameClock::new(
            u64::MAX - 8192,
            8192,
            1,
            1,
            48_000,
            AudioClockDomain::Unknown,
        );
        assert_eq!(launch_frame(clock).unwrap_err(), "Audio clock overflow");
    }
}
