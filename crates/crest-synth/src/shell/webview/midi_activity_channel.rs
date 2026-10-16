//! Latest-only presentation transport for physical MIDI activity.
//!
//! Product projection remains generation-gated and observation-free. This
//! channel carries only the newest control-side snapshot at no more than
//! 30 Hz, so activity and the 500 ms Receiving/Waiting transition can repaint
//! without inventing an `AppState` event or entering `StateTree`.

use crate::control::MidiActivityObservation;
use std::time::{Duration, Instant};

pub const MIDI_ACTIVITY_EVENT: &str = "crest://midi-activity";
pub const MIDI_ACTIVITY_RATE_HZ: u32 = 30;
pub const MIDI_ACTIVITY_INTERVAL: Duration =
    Duration::from_nanos(1_000_000_000 / MIDI_ACTIVITY_RATE_HZ as u64);

#[derive(Debug)]
pub enum MidiActivityEmit {
    Emitted,
    Coalescing,
    Idle,
    FrameLost(tauri::Error),
}

#[derive(Debug, Default)]
pub struct MidiActivityChannel {
    pending: Option<MidiActivityObservation>,
    last_emit: Option<Instant>,
}

impl MidiActivityChannel {
    pub const fn new() -> Self {
        Self {
            pending: None,
            last_emit: None,
        }
    }

    pub fn observe(&mut self, observation: MidiActivityObservation) {
        self.pending = Some(observation);
    }

    pub fn emit_if_due<E>(&mut self, now: Instant, emit: E) -> MidiActivityEmit
    where
        E: FnOnce(MidiActivityObservation) -> tauri::Result<()>,
    {
        if self
            .last_emit
            .is_some_and(|last| now.duration_since(last) < MIDI_ACTIVITY_INTERVAL)
        {
            return MidiActivityEmit::Coalescing;
        }
        let Some(observation) = self.pending.take() else {
            return MidiActivityEmit::Idle;
        };
        self.last_emit = Some(now);
        match emit(observation) {
            Ok(()) => MidiActivityEmit::Emitted,
            Err(error) => MidiActivityEmit::FrameLost(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{MidiActivitySnapshot, MidiConnectionRevision};

    fn observation(count: u64) -> MidiActivityObservation {
        MidiActivityObservation::new(
            Some(MidiActivitySnapshot::new(
                MidiConnectionRevision::FIRST,
                count,
                None,
                0,
                Default::default(),
                0,
            )),
            false,
        )
    }

    #[test]
    fn coalesces_to_one_latest_frame_at_the_declared_rate() {
        let start = Instant::now();
        let mut channel = MidiActivityChannel::new();
        let mut counts = Vec::new();
        channel.observe(observation(1));
        assert!(matches!(
            channel.emit_if_due(start, |value| {
                counts.push(value.snapshot().unwrap().accepted_count());
                Ok(())
            }),
            MidiActivityEmit::Emitted
        ));
        channel.observe(observation(2));
        channel.observe(observation(3));
        assert!(matches!(
            channel.emit_if_due(start + MIDI_ACTIVITY_INTERVAL / 2, |_| Ok(())),
            MidiActivityEmit::Coalescing
        ));
        assert!(matches!(
            channel.emit_if_due(start + MIDI_ACTIVITY_INTERVAL, |value| {
                counts.push(value.snapshot().unwrap().accepted_count());
                Ok(())
            }),
            MidiActivityEmit::Emitted
        ));
        assert_eq!(counts, [1, 3]);
    }
}
