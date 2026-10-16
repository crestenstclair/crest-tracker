//! The meter transport: decimated latest-value push of
//! [`AudioObservationSnapshot`] frames to the page.
//!
//! The window's event loop feeds the channel the latest snapshot each tick —
//! read through the same `AudioObservationCallback` accessor the retired native
//! window paints its meters from — and the channel emits at most one frame
//! per [`METER_INTERVAL`] on the [`METER_EVENT`] named event. This realizes
//! DESIGN.md's meter transport semantics ("latest-value snapshots,
//! decimated") as equivalent push traffic: same decimation, same
//! loss-tolerance, no new state category.
//!
//! # Latest-value only — no queue
//!
//! The channel owns exactly one pending slot. Every observation overwrites
//! it; nothing is ever queued behind it, so queue depth is structurally
//! bounded at one frame regardless of how fast observations arrive: 30 Hz
//! without queue growth over a five-minute render.
//!
//! # Loss semantics: display-only degradation
//!
//! A frame superseded before the next due emit is simply never sent, and a
//! frame whose emit fails is dropped — the next observation replaces it.
//! Meters are render evidence, not control state: a lost frame costs one
//! meter repaint and nothing else, and the next due frame carries the
//! current values. Their loss degrades display only, so emit failures never take
//! the window's fatal error path.
//!
//! # Nothing here touches the real-time callback
//!
//! The channel runs on the window's event thread and reads the control side's
//! copy of the last completed render block's evidence — the same
//! atomically-published snapshot the retired native window read every frame. It
//! installs nothing into the audio callback, allocates nothing inside it,
//! and shares no lock the callback could contend on; the render path's
//! measured bounds are unchanged from the pre-cutover shell baseline.

use crate::real_time::AudioObservationSnapshot;
use std::time::{Duration, Instant};

/// The named tauri event carrying each decimated meter frame to the page.
pub const METER_EVENT: &str = "crest://meters";

/// The declared meter decimation rate.
///
/// NFR-002: meter animation sustains a 30 Hz update rate without
/// accumulating queue depth over a five-minute render. The interval below is
/// derived from this named rate; no independent millisecond literal exists.
pub const METER_RATE_HZ: u32 = 30;

/// The minimum spacing between meter emits, derived from [`METER_RATE_HZ`].
pub const METER_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / METER_RATE_HZ as u64);

/// What one [`MeterChannel::emit_if_due`] did.
#[derive(Debug)]
pub enum MeterEmit {
    /// The interval had elapsed and the latest pending frame was emitted.
    Emitted,
    /// The interval had not yet elapsed; the pending frame (if any) keeps
    /// coalescing — later observations overwrite it until the next due emit.
    Coalescing,
    /// The interval had elapsed but no frame was pending since the last
    /// emit; nothing was sent.
    Idle,
    /// The due frame's emit failed and the frame was dropped — display-only
    /// degradation, superseded by the next observation. The cause is carried
    /// for callers that want to note it; it is never fatal.
    FrameLost(tauri::Error),
}

/// Latest-value meter channel with a single pending slot and a derived
/// 30 Hz emit pace.
#[derive(Debug, Default)]
pub struct MeterChannel {
    pending: Option<AudioObservationSnapshot>,
    last_emit: Option<Instant>,
}

impl MeterChannel {
    /// Creates a channel with an empty slot whose first due frame emits
    /// immediately.
    pub const fn new() -> Self {
        Self {
            pending: None,
            last_emit: None,
        }
    }

    /// Records the latest snapshot, overwriting any pending frame.
    ///
    /// This is the whole coalescing rule: the slot holds at most one frame,
    /// and the newest observation always wins.
    pub fn observe(&mut self, snapshot: AudioObservationSnapshot) {
        self.pending = Some(snapshot);
    }

    /// Emits the pending frame through `emit` if at least [`METER_INTERVAL`]
    /// has passed since the last emit.
    ///
    /// `emit` performs the actual tauri emit on [`METER_EVENT`]; injecting it
    /// keeps the pacing and coalescing logic provable without a tauri
    /// runtime (the `Emitter` trait is sealed). `now` is injected for the
    /// same reason.
    pub fn emit_if_due<E>(&mut self, now: Instant, emit: E) -> MeterEmit
    where
        E: FnOnce(AudioObservationSnapshot) -> tauri::Result<()>,
    {
        let due = match self.last_emit {
            None => true,
            Some(previous) => now.duration_since(previous) >= METER_INTERVAL,
        };
        if !due {
            return MeterEmit::Coalescing;
        }
        let Some(frame) = self.pending.take() else {
            return MeterEmit::Idle;
        };
        self.last_emit = Some(now);
        match emit(frame) {
            Ok(()) => MeterEmit::Emitted,
            // The frame is already dropped; the next observation supersedes
            // it. Display-only degradation, documented above.
            Err(error) => MeterEmit::FrameLost(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MeterChannel, MeterEmit, METER_EVENT, METER_INTERVAL, METER_RATE_HZ};
    use crate::real_time::AudioObservationSnapshot;
    use std::time::{Duration, Instant};

    fn snapshot(sequence: u64) -> AudioObservationSnapshot {
        AudioObservationSnapshot::from_parts(
            sequence, 0, 0, 0, 0, 0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0, 0,
        )
    }

    #[test]
    fn meter_event_name_satisfies_the_tauri_event_charset() {
        assert!(METER_EVENT.chars().all(|c| c.is_alphanumeric()
            || c == '-'
            || c == '/'
            || c == ':'
            || c == '_'));
    }

    #[test]
    fn the_interval_realizes_the_declared_rate() {
        assert_eq!(
            Duration::from_secs(1).as_nanos() / METER_INTERVAL.as_nanos(),
            u128::from(METER_RATE_HZ)
        );
    }

    #[test]
    fn rapid_reads_coalesce_and_the_latest_frame_wins() {
        let mut channel = MeterChannel::new();
        let start = Instant::now();
        let mut emitted: Vec<u64> = Vec::new();
        let mut record = |frame: AudioObservationSnapshot| {
            emitted.push(frame.sequence());
            Ok(())
        };

        // First due frame emits immediately.
        channel.observe(snapshot(1));
        assert!(matches!(
            channel.emit_if_due(start, &mut record),
            MeterEmit::Emitted
        ));

        // N rapid reads inside the interval: nothing emits, the slot holds
        // only the newest frame.
        for sequence in 2..=5 {
            channel.observe(snapshot(sequence));
            assert!(matches!(
                channel.emit_if_due(start + METER_INTERVAL / 2, &mut record),
                MeterEmit::Coalescing
            ));
        }

        // The next due emit carries the latest frame; the superseded ones
        // are the documented display-only loss.
        assert!(matches!(
            channel.emit_if_due(start + METER_INTERVAL, &mut record),
            MeterEmit::Emitted
        ));
        assert_eq!(emitted, [1, 5]);
    }

    #[test]
    fn at_most_one_frame_is_ever_pending() {
        let mut channel = MeterChannel::new();
        let start = Instant::now();
        let mut emitted: Vec<u64> = Vec::new();

        channel.observe(snapshot(1));
        channel.observe(snapshot(2));
        let mut record = |frame: AudioObservationSnapshot| {
            emitted.push(frame.sequence());
            Ok(())
        };
        assert!(matches!(
            channel.emit_if_due(start, &mut record),
            MeterEmit::Emitted
        ));
        // The slot emptied on emit: a due tick with no new observation sends
        // nothing rather than repeating a stale frame.
        assert!(matches!(
            channel.emit_if_due(start + METER_INTERVAL, &mut record),
            MeterEmit::Idle
        ));
        assert_eq!(emitted, [2], "only the latest of the two rapid reads");
    }

    #[test]
    fn a_failed_emit_loses_only_that_frame() {
        let mut channel = MeterChannel::new();
        let start = Instant::now();

        channel.observe(snapshot(1));
        let outcome = channel.emit_if_due(start, |_| {
            Err(tauri::Error::from(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "window gone",
            )))
        });
        assert!(matches!(outcome, MeterEmit::FrameLost(_)));

        // The lost frame is superseded, not retried: the next observation
        // flows normally at the paced interval.
        channel.observe(snapshot(2));
        let mut emitted: Vec<u64> = Vec::new();
        assert!(matches!(
            channel.emit_if_due(start + METER_INTERVAL, |frame| {
                emitted.push(frame.sequence());
                Ok(())
            }),
            MeterEmit::Emitted
        ));
        assert_eq!(emitted, [2]);
    }
}
