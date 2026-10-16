//! Control/worker-side MIDI clock. Backend timestamps, not UI update frequency,
//! determine tempo. Input never produces output packets or enables clock echo.
use crest_tracker::playback::TempoMap;
use crest_tracker::time::{Beat, Tempo};
use serde::Serialize;
use std::collections::VecDeque;

const CLOCKS_PER_QUARTER: u64 = 24;
const MINIMUM_LOSS_MICROS: u64 = 500_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ClockSource {
    Internal,
    Input { endpoint: String, revision: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ClockMessage {
    Start,
    Continue,
    Stop,
    Tick,
    /// MIDI Song Position Pointer units are six clocks, or one sixteenth note.
    SongPosition(u16),
}

impl ClockMessage {
    pub fn parse(bytes: &[u8]) -> Result<Self, ClockError> {
        match bytes {
            [0xfa] => Ok(Self::Start),
            [0xfb] => Ok(Self::Continue),
            [0xfc] => Ok(Self::Stop),
            [0xf8] => Ok(Self::Tick),
            [0xf2, lsb, msb] if *lsb < 128 && *msb < 128 => {
                Ok(Self::SongPosition(u16::from(*lsb) | (u16::from(*msb) << 7)))
            }
            _ => Err(ClockError::InvalidMessage),
        }
    }

    pub fn bytes(self) -> Result<Vec<u8>, ClockError> {
        self.validate()?;
        Ok(match self {
            Self::Start => vec![0xfa],
            Self::Continue => vec![0xfb],
            Self::Stop => vec![0xfc],
            Self::Tick => vec![0xf8],
            Self::SongPosition(value) => vec![0xf2, (value & 127) as u8, (value >> 7) as u8],
        })
    }

    /// Explicit seeking must be representable without silently rounding SPP.
    #[cfg(test)]
    pub fn song_position(position: Beat) -> Result<Self, ClockError> {
        let sixteenths = position.units(Beat::new(1, 4)?)?;
        if sixteenths > 16_383 || Beat::new(sixteenths, 4)? != position {
            return Err(ClockError::InvalidSongPosition);
        }
        Ok(Self::SongPosition(sixteenths as u16))
    }

    fn validate(self) -> Result<(), ClockError> {
        if matches!(self, Self::SongPosition(value) if value > 16_383) {
            return Err(ClockError::InvalidMessage);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ClockStopReason {
    LocalStop,
    TransportStop,
    SourceChanged,
    Disconnected,
    ClockLost,
    InvalidTimestamp,
    PositionWhileRunning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ClockAction {
    Begin {
        position: Beat,
        timestamp_micros: u64,
    },
    Stop {
        reason: ClockStopReason,
    },
    Position {
        position: Beat,
    },
    /// The first pulse after Start/Continue marks the starting position. Later
    /// pulses advance by exactly 1/24. Stopped clocks measure tempo only.
    Tick {
        position: Option<Beat>,
        timestamp_micros: u64,
        tempo: Option<Tempo>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ClockError {
    #[error(
        "expected Start, Continue, Stop, Timing Clock or a complete 14-bit Song Position Pointer"
    )]
    InvalidMessage,
    #[error("clock input requires an exact endpoint and nonzero connection revision")]
    InvalidSource,
    #[error("reconnecting a clock input requires a newer source revision")]
    StaleSource,
    #[cfg(test)]
    #[error("Song Position Pointer requires a sixteenth-note position from 0 through 16383")]
    InvalidSongPosition,
    #[error("clock pulse capacity must be positive")]
    InvalidCapacity,
    #[error("clock window exceeds its pulse capacity")]
    Capacity,
    #[error("clock windows must advance monotonically")]
    Backwards,
    #[error("clock position exhausted")]
    PositionExhausted,
    #[error("clock requires a positive sample rate")]
    InvalidSampleRate,
    #[error("clock interval cannot be represented as a tempo")]
    InvalidInterval,
    #[error("clock timeline requires at least two retained pulses")]
    InvalidHistoryCapacity,
    #[error("clock timeline requires consecutive pulse positions and increasing timestamps")]
    InvalidTimelineOrder,
    #[error("position lies outside the retained completed clock intervals")]
    OutsideTimeline,
    #[error(transparent)]
    Time(#[from] crest_tracker::time::TimeError),
    #[error(transparent)]
    Playback(#[from] crest_tracker::playback::PlaybackError),
}

/// One authority at a time. Source changes and reconnections always leave this
/// transport stopped; only a fresh Start or Continue permits incoming playback.
#[derive(Debug, Clone)]
pub struct ClockTransport {
    source: ClockSource,
    connected: bool,
    running: bool,
    next_clock: u64,
    last_packet: Option<u64>,
    last_tick: Option<u64>,
    started_at: Option<u64>,
    interval_micros: Option<u64>,
}

impl Default for ClockTransport {
    fn default() -> Self {
        Self {
            source: ClockSource::Internal,
            connected: true,
            running: false,
            next_clock: 0,
            last_packet: None,
            last_tick: None,
            started_at: None,
            interval_micros: None,
        }
    }
}

impl ClockTransport {
    pub fn source(&self) -> &ClockSource {
        &self.source
    }
    pub fn running(&self) -> bool {
        self.running
    }
    /// The next pulse's exact position; Stop/Continue preserves partial quarters.
    pub fn position(&self) -> Beat {
        Beat::new(self.next_clock, CLOCKS_PER_QUARTER).expect("positive clock denominator")
    }

    pub fn set_source(&mut self, source: ClockSource) -> Result<Option<ClockAction>, ClockError> {
        if matches!(&source, ClockSource::Input { endpoint, revision } if endpoint.is_empty() || *revision == 0)
        {
            return Err(ClockError::InvalidSource);
        }
        if self.source == source && self.connected {
            return Ok(None);
        }
        if let (
            ClockSource::Input {
                endpoint: previous,
                revision: old,
            },
            ClockSource::Input {
                endpoint: next,
                revision: new,
            },
        ) = (&self.source, &source)
        {
            if previous == next && (*new < *old || (!self.connected && new == old)) {
                return Err(ClockError::StaleSource);
            }
        }
        let action = self.stop(ClockStopReason::SourceChanged);
        self.source = source;
        self.connected = true;
        self.last_packet = None;
        self.last_tick = None;
        self.started_at = None;
        self.interval_micros = None;
        Ok(action)
    }

    /// Stale or unselected sessions are ignored before timestamp validation.
    /// Selected malformed timing stops visibly; arithmetic errors publish no
    /// partial state. No returned action requests MIDI output/echo.
    pub fn receive(
        &mut self,
        endpoint: &str,
        revision: u64,
        timestamp_micros: u64,
        message: ClockMessage,
    ) -> Result<Option<ClockAction>, ClockError> {
        if !self.matches(endpoint, revision) {
            return Ok(None);
        }
        message.validate()?;
        let mut candidate = self.clone();
        let action = candidate.accept(timestamp_micros, message)?;
        *self = candidate;
        Ok(action)
    }

    pub fn disconnect(&mut self, endpoint: &str, revision: u64) -> Option<ClockAction> {
        if !self.matches(endpoint, revision) {
            return None;
        }
        self.connected = false;
        self.stop(ClockStopReason::Disconnected)
    }

    pub fn stop_local(&mut self) -> Option<ClockAction> {
        let action = self.stop(ClockStopReason::LocalStop);
        self.last_tick = None;
        self.started_at = None;
        self.interval_micros = None;
        action
    }

    /// `now_micros` uses the same monotonic backend clock as receive. Polling
    /// never advances musical position or synthesizes missing timing pulses.
    pub fn poll_loss(&mut self, now_micros: u64) -> Option<ClockAction> {
        if !self.running {
            return None;
        }
        let reference = self.last_tick.or(self.started_at)?;
        if now_micros.saturating_sub(reference) >= self.loss_timeout_micros() {
            return self.stop(ClockStopReason::ClockLost);
        }
        None
    }

    pub fn loss_timeout_micros(&self) -> u64 {
        self.interval_micros
            .unwrap_or(0)
            .saturating_mul(4)
            .max(MINIMUM_LOSS_MICROS)
    }

    fn matches(&self, endpoint: &str, revision: u64) -> bool {
        self.connected
            && matches!(&self.source,
            ClockSource::Input { endpoint: selected, revision: active }
            if selected == endpoint && *active == revision)
    }

    fn stop(&mut self, reason: ClockStopReason) -> Option<ClockAction> {
        let was_running = self.running;
        self.running = false;
        was_running.then_some(ClockAction::Stop { reason })
    }

    fn accept(
        &mut self,
        timestamp: u64,
        message: ClockMessage,
    ) -> Result<Option<ClockAction>, ClockError> {
        if self.last_packet.is_some_and(|last| timestamp < last)
            || (message == ClockMessage::Tick
                && self.last_tick.is_some_and(|last| timestamp <= last))
        {
            self.running = false;
            return Ok(Some(ClockAction::Stop {
                reason: ClockStopReason::InvalidTimestamp,
            }));
        }
        // A fresh explicit transport command can reestablish the position after
        // a long pause. Only an unannounced late pulse is refused as clock loss.
        if message == ClockMessage::Tick {
            if let Some(action) = self.poll_loss(timestamp) {
                self.last_packet = Some(timestamp);
                self.last_tick = None;
                self.interval_micros = None;
                return Ok(Some(action));
            }
        }
        self.last_packet = Some(timestamp);
        match message {
            ClockMessage::Start | ClockMessage::Continue => {
                if message == ClockMessage::Start {
                    self.next_clock = 0;
                }
                self.running = true;
                self.started_at = Some(timestamp);
                self.last_tick = None;
                Ok(Some(ClockAction::Begin {
                    position: self.position(),
                    timestamp_micros: timestamp,
                }))
            }
            ClockMessage::Stop => {
                self.running = false;
                Ok(Some(ClockAction::Stop {
                    reason: ClockStopReason::TransportStop,
                }))
            }
            ClockMessage::SongPosition(value) => {
                self.next_clock = u64::from(value) * 6;
                if self.running {
                    return Ok(self.stop(ClockStopReason::PositionWhileRunning));
                }
                Ok(Some(ClockAction::Position {
                    position: self.position(),
                }))
            }
            ClockMessage::Tick => self.tick(timestamp).map(Some),
        }
    }

    fn tick(&mut self, timestamp: u64) -> Result<ClockAction, ClockError> {
        let tempo = match self.last_tick {
            Some(last) => {
                let interval = timestamp - last;
                if !self.running && interval >= self.loss_timeout_micros() {
                    self.interval_micros = None;
                    None
                } else {
                    let denominator = u128::from(interval) * u128::from(CLOCKS_PER_QUARTER);
                    let milli_bpm = (60_000_000_000_u128 + denominator / 2) / denominator;
                    let tempo = Tempo::new(
                        u32::try_from(milli_bpm).map_err(|_| ClockError::InvalidInterval)?,
                    )?;
                    self.interval_micros = Some(interval);
                    Some(tempo)
                }
            }
            None => None,
        };
        self.last_tick = Some(timestamp);
        let position = self.running.then(|| self.position());
        if self.running {
            self.next_clock = self
                .next_clock
                .checked_add(1)
                .ok_or(ClockError::PositionExhausted)?;
        }
        Ok(ClockAction::Tick {
            position,
            timestamp_micros: timestamp,
            tempo,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClockPulse {
    pub at: Beat,
    /// Frame from beat zero in TempoMap. The host must rebase after a seek
    /// before adding its audio anchor, exactly as it does for note events.
    pub frame: u64,
}

/// Internal-master output only. Callers must select this instead of input
/// following; there is deliberately no automatic input-to-output bridge.
#[derive(Debug, Clone)]
pub struct ClockOutput {
    next_clock: u64,
    horizon: Beat,
    capacity: usize,
}

impl ClockOutput {
    pub fn new(start: Beat, capacity: usize) -> Result<Self, ClockError> {
        if capacity == 0 {
            return Err(ClockError::InvalidCapacity);
        }
        Ok(Self {
            next_clock: clock_at_or_after(start)?,
            horizon: start,
            capacity,
        })
    }

    /// Emits [previous horizon, end), including beat zero exactly once. Every
    /// deadline is independently mapped from k/24, never accumulated rounded
    /// frame intervals. A failed or oversized window leaves the cursor intact.
    pub fn window(
        &mut self,
        end: Beat,
        tempo: &TempoMap,
        sample_rate: u32,
    ) -> Result<Vec<ClockPulse>, ClockError> {
        if end < self.horizon {
            return Err(ClockError::Backwards);
        }
        if sample_rate == 0 {
            return Err(ClockError::InvalidSampleRate);
        }
        let until = clock_at_or_after(end)?;
        let count = usize::try_from(until - self.next_clock).map_err(|_| ClockError::Capacity)?;
        if count > self.capacity {
            return Err(ClockError::Capacity);
        }
        let mut pulses = Vec::with_capacity(count);
        for clock in self.next_clock..until {
            let at = Beat::new(clock, CLOCKS_PER_QUARTER)?;
            pulses.push(ClockPulse {
                at,
                frame: tempo.frame_at(at, sample_rate)?,
            });
        }
        self.next_clock = until;
        self.horizon = end;
        Ok(pulses)
    }
}

fn clock_at_or_after(at: Beat) -> Result<u64, ClockError> {
    let clock = at.units(Beat::new(1, CLOCKS_PER_QUARTER)?)?;
    if Beat::new(clock, CLOCKS_PER_QUARTER)? < at {
        return clock.checked_add(1).ok_or(ClockError::PositionExhausted);
    }
    Ok(clock)
}

/// Exact accepted pulse segments for external follow and input recording.
/// Capacity is explicit; once an old anchor is evicted, queries fail rather than
/// extrapolate from tempo or quietly use a newer clock segment.
#[derive(Debug, Clone)]
pub struct PulseTimeline {
    anchors: VecDeque<(Beat, u64)>,
    capacity: usize,
}

impl PulseTimeline {
    pub fn new(capacity: usize) -> Result<Self, ClockError> {
        if capacity < 2 {
            return Err(ClockError::InvalidHistoryCapacity);
        }
        Ok(Self {
            anchors: VecDeque::new(),
            capacity,
        })
    }

    pub fn clear(&mut self) {
        self.anchors.clear();
    }
    pub fn latest(&self) -> Option<(Beat, u64)> {
        self.anchors.back().copied()
    }
    pub fn earliest(&self) -> Option<(Beat, u64)> {
        self.anchors.front().copied()
    }

    pub fn push(&mut self, position: Beat, timestamp_micros: u64) -> Result<(), ClockError> {
        if let Some((previous, timestamp)) = self.latest() {
            if timestamp_micros <= timestamp
                || position != previous.checked_add(Beat::new(1, CLOCKS_PER_QUARTER)?)?
            {
                return Err(ClockError::InvalidTimelineOrder);
            }
        }
        if self.anchors.len() == self.capacity {
            self.anchors.pop_front();
        }
        self.anchors.push_back((position, timestamp_micros));
        Ok(())
    }

    /// Rounds only the final timestamp, independently for each exact beat.
    pub fn timestamp_at(&self, position: Beat) -> Result<u64, ClockError> {
        if let Some((_, timestamp)) = self.anchors.iter().find(|(beat, _)| *beat == position) {
            return Ok(*timestamp);
        }
        let (left, right) = self
            .anchors
            .iter()
            .zip(self.anchors.iter().skip(1))
            .find(|(left, right)| left.0 < position && position < right.0)
            .ok_or(ClockError::OutsideTimeline)?;
        let offset = position
            .checked_sub(left.0)?
            .checked_scale(right.1 - left.1, 1)?
            .checked_scale(CLOCKS_PER_QUARTER, 1)?;
        let rounded = (u128::from(offset.numerator()) + u128::from(offset.denominator()) / 2)
            / u128::from(offset.denominator());
        left.1
            .checked_add(u64::try_from(rounded).map_err(|_| ClockError::PositionExhausted)?)
            .ok_or(ClockError::PositionExhausted)
    }

    pub fn beat_at(&self, timestamp_micros: u64) -> Result<Beat, ClockError> {
        if let Some((beat, _)) = self
            .anchors
            .iter()
            .find(|(_, timestamp)| *timestamp == timestamp_micros)
        {
            return Ok(*beat);
        }
        let (left, right) = self
            .anchors
            .iter()
            .zip(self.anchors.iter().skip(1))
            .find(|(left, right)| left.1 < timestamp_micros && timestamp_micros < right.1)
            .ok_or(ClockError::OutsideTimeline)?;
        let offset = Beat::new(timestamp_micros - left.1, right.1 - left.1)?
            .checked_scale(1, CLOCKS_PER_QUARTER)?;
        Ok(left.0.checked_add(offset)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(revision: u64) -> ClockSource {
        ClockSource::Input {
            endpoint: "controller".into(),
            revision,
        }
    }
    fn follower() -> ClockTransport {
        let mut clock = ClockTransport::default();
        clock.set_source(source(1)).unwrap();
        clock
    }
    fn receive(clock: &mut ClockTransport, at: u64, message: ClockMessage) -> ClockAction {
        clock
            .receive("controller", 1, at, message)
            .unwrap()
            .unwrap()
    }
    fn beat(n: u64, d: u64) -> Beat {
        Beat::new(n, d).unwrap()
    }

    #[test]
    fn exact_transport_codec_rejects_partial_and_unrelated_system_packets() {
        for bytes in [
            vec![0xfa],
            vec![0xfb],
            vec![0xfc],
            vec![0xf8],
            vec![0xf2, 127, 127],
        ] {
            assert_eq!(ClockMessage::parse(&bytes).unwrap().bytes().unwrap(), bytes);
        }
        for bytes in [
            vec![],
            vec![0xf2, 1],
            vec![0xf2, 128, 0],
            vec![0xf8, 0],
            vec![0xf9],
            vec![0xfe],
            vec![0x90, 60, 100],
        ] {
            assert!(ClockMessage::parse(&bytes).is_err());
        }
        assert!(ClockMessage::SongPosition(16_384).bytes().is_err());
        assert_eq!(
            ClockMessage::song_position(beat(7, 4)).unwrap(),
            ClockMessage::SongPosition(7)
        );
        assert!(ClockMessage::song_position(beat(1, 24)).is_err());
        assert!(ClockMessage::song_position(Beat::integer(4096)).is_err());
    }

    #[test]
    fn only_selected_source_advances_exactly_twenty_four_pulses_per_quarter() {
        let mut clock = ClockTransport::default();
        assert!(clock
            .receive("controller", 1, 0, ClockMessage::Start)
            .unwrap()
            .is_none());
        clock.set_source(source(1)).unwrap();
        assert!(clock
            .receive("other", 1, 0, ClockMessage::Start)
            .unwrap()
            .is_none());
        assert!(clock
            .receive("controller", 2, 0, ClockMessage::Start)
            .unwrap()
            .is_none());
        assert!(matches!(
            receive(&mut clock, 0, ClockMessage::Start),
            ClockAction::Begin {
                position: Beat::ZERO,
                ..
            }
        ));
        for tick in 0..24 {
            let action = receive(&mut clock, 1 + tick * 20_000, ClockMessage::Tick);
            let ClockAction::Tick {
                position, tempo, ..
            } = action
            else {
                panic!("tick expected")
            };
            assert_eq!(position, Some(beat(tick, 24)));
            assert_eq!(tempo, (tick > 0).then(|| Tempo::new(125_000).unwrap()));
        }
        assert_eq!(clock.position(), Beat::ONE);
        assert!(matches!(
            receive(&mut clock, 480_001, ClockMessage::Tick),
            ClockAction::Tick {
                position: Some(Beat::ONE),
                ..
            }
        ));
    }

    #[test]
    fn spp_and_continue_preserve_six_clock_units_and_partial_quarter_position() {
        let mut clock = follower();
        assert_eq!(
            receive(&mut clock, 0, ClockMessage::SongPosition(7)),
            ClockAction::Position {
                position: beat(7, 4)
            }
        );
        assert_eq!(
            receive(&mut clock, 1, ClockMessage::Continue),
            ClockAction::Begin {
                position: beat(7, 4),
                timestamp_micros: 1
            }
        );
        for tick in 0..6 {
            receive(&mut clock, 2 + tick * 20_000, ClockMessage::Tick);
        }
        assert_eq!(clock.position(), Beat::integer(2));
        receive(&mut clock, 120_002, ClockMessage::Tick);
        receive(&mut clock, 121_000, ClockMessage::Stop);
        let position = beat(49, 24);
        assert_eq!(clock.position(), position);
        assert!(matches!(
            receive(&mut clock, 122_000, ClockMessage::Tick),
            ClockAction::Tick { position: None, .. }
        ));
        assert_eq!(clock.position(), position);
        assert_eq!(
            receive(&mut clock, 123_000, ClockMessage::Continue),
            ClockAction::Begin {
                position,
                timestamp_micros: 123_000
            }
        );
        assert!(
            matches!(receive(&mut clock, 124_000, ClockMessage::Tick), ClockAction::Tick { position: Some(at), .. } if at == position)
        );
        receive(&mut clock, 125_000, ClockMessage::Start);
        assert_eq!(clock.position(), Beat::ZERO);
    }

    #[test]
    fn loss_and_reconnect_never_resume_without_a_new_transport_command() {
        let mut clock = follower();
        receive(&mut clock, 0, ClockMessage::Start);
        receive(&mut clock, 1, ClockMessage::Tick);
        receive(&mut clock, 20_001, ClockMessage::Tick);
        assert_eq!(clock.loss_timeout_micros(), 500_000);
        assert_eq!(clock.poll_loss(520_000), None);
        assert_eq!(
            clock.poll_loss(520_001),
            Some(ClockAction::Stop {
                reason: ClockStopReason::ClockLost
            })
        );
        let position = clock.position();
        assert!(matches!(
            receive(&mut clock, 530_000, ClockMessage::Tick),
            ClockAction::Tick { position: None, .. }
        ));
        assert_eq!(clock.position(), position);
        receive(&mut clock, 540_000, ClockMessage::Continue);
        assert_eq!(
            clock.disconnect("controller", 1),
            Some(ClockAction::Stop {
                reason: ClockStopReason::Disconnected
            })
        );
        assert!(clock
            .receive("controller", 1, 550_000, ClockMessage::Start)
            .unwrap()
            .is_none());
        assert!(matches!(
            clock.set_source(source(1)),
            Err(ClockError::StaleSource)
        ));
        clock.set_source(source(2)).unwrap();
        assert!(!clock.running());
        assert!(clock
            .receive("controller", 1, 560_000, ClockMessage::Start)
            .unwrap()
            .is_none());
        assert!(matches!(
            clock
                .receive("controller", 2, 1, ClockMessage::Tick)
                .unwrap(),
            Some(ClockAction::Tick { position: None, .. })
        ));
        assert_eq!(clock.position(), position);
        assert!(
            matches!(clock.receive("controller", 2, 2, ClockMessage::Continue).unwrap(), Some(ClockAction::Begin { position: at, .. }) if at == position)
        );
    }

    #[test]
    fn slow_measured_clock_uses_four_intervals_and_polling_never_moves_position() {
        let mut clock = follower();
        receive(&mut clock, 0, ClockMessage::Tick);
        receive(&mut clock, 300_000, ClockMessage::Tick);
        assert_eq!(clock.loss_timeout_micros(), 1_200_000);
        receive(&mut clock, 300_001, ClockMessage::Start);
        receive(&mut clock, 300_002, ClockMessage::Tick);
        let position = clock.position();
        for now in [300_003, 700_000, 1_500_001] {
            assert_eq!(clock.poll_loss(now), None);
        }
        assert_eq!(clock.position(), position);
        assert_eq!(
            clock.poll_loss(1_500_002),
            Some(ClockAction::Stop {
                reason: ClockStopReason::ClockLost
            })
        );
    }

    #[test]
    fn invalid_order_gaps_and_local_stop_cannot_advance_or_restart_transport() {
        let mut clock = follower();
        receive(&mut clock, 100, ClockMessage::Start);
        receive(&mut clock, 101, ClockMessage::Tick);
        let position = clock.position();
        assert_eq!(
            receive(&mut clock, 101, ClockMessage::Tick),
            ClockAction::Stop {
                reason: ClockStopReason::InvalidTimestamp
            }
        );
        assert_eq!(clock.position(), position);
        receive(&mut clock, 102, ClockMessage::Continue);
        assert_eq!(
            receive(&mut clock, 100, ClockMessage::Stop),
            ClockAction::Stop {
                reason: ClockStopReason::InvalidTimestamp
            }
        );
        receive(&mut clock, 103, ClockMessage::Continue);
        assert_eq!(
            receive(&mut clock, 500_103, ClockMessage::Tick),
            ClockAction::Stop {
                reason: ClockStopReason::ClockLost
            }
        );
        assert_eq!(clock.position(), position);
        receive(&mut clock, 500_104, ClockMessage::Continue);
        assert_eq!(
            clock.stop_local(),
            Some(ClockAction::Stop {
                reason: ClockStopReason::LocalStop
            })
        );
        assert!(matches!(
            receive(&mut clock, 500_105, ClockMessage::Tick),
            ClockAction::Tick { position: None, .. }
        ));
        assert_eq!(clock.position(), position);
        receive(&mut clock, 500_106, ClockMessage::Continue);
        assert_eq!(
            receive(&mut clock, 500_107, ClockMessage::SongPosition(8)),
            ClockAction::Stop {
                reason: ClockStopReason::PositionWhileRunning
            }
        );
        assert_eq!(clock.position(), Beat::integer(2));
    }

    #[test]
    fn fresh_explicit_start_reestablishes_transport_after_an_unpolled_gap() {
        let mut clock = follower();
        receive(&mut clock, 0, ClockMessage::Start);
        receive(&mut clock, 1, ClockMessage::Tick);
        assert!(matches!(
            receive(&mut clock, 5_000_000, ClockMessage::Start),
            ClockAction::Begin {
                position: Beat::ZERO,
                ..
            }
        ));
        assert!(matches!(
            receive(&mut clock, 5_000_001, ClockMessage::Tick),
            ClockAction::Tick {
                position: Some(Beat::ZERO),
                ..
            }
        ));
    }

    #[test]
    fn internal_pulses_use_exact_beats_across_tempo_changes_and_partitioned_windows() {
        let mut tempo = TempoMap::new(Tempo::new(123_456).unwrap());
        tempo.insert(beat(1, 2), Tempo::new(60_000).unwrap());
        let whole = ClockOutput::new(Beat::ZERO, 240)
            .unwrap()
            .window(Beat::integer(10), &tempo, 44_100)
            .unwrap();
        assert_eq!(whole.len(), 240);
        for (index, pulse) in whole.iter().enumerate() {
            assert_eq!(pulse.at, beat(index as u64, 24));
            assert_eq!(pulse.frame, tempo.frame_at(pulse.at, 44_100).unwrap());
        }
        let mut output = ClockOutput::new(Beat::ZERO, 240).unwrap();
        let mut parts = Vec::new();
        for end in [beat(1, 10), beat(1, 3), beat(7, 8), Beat::integer(10)] {
            parts.extend(output.window(end, &tempo, 44_100).unwrap());
        }
        assert_eq!(parts, whole);
        assert!(output
            .window(Beat::integer(10), &tempo, 44_100)
            .unwrap()
            .is_empty());
        let shifted = ClockOutput::new(beat(1, 96), 24)
            .unwrap()
            .window(beat(1, 4), &tempo, 44_100)
            .unwrap();
        assert_eq!(
            shifted.iter().map(|pulse| pulse.at).collect::<Vec<_>>(),
            (1..6).map(|k| beat(k, 24)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn oversized_clock_output_is_atomic_and_retries_at_the_original_pulse() {
        let tempo = TempoMap::new(Tempo::DEFAULT);
        let mut output = ClockOutput::new(Beat::ZERO, 6).unwrap();
        assert!(matches!(
            output.window(Beat::ONE, &tempo, 48_000),
            Err(ClockError::Capacity)
        ));
        assert!(matches!(
            output.window(beat(1, 4), &tempo, 0),
            Err(ClockError::InvalidSampleRate)
        ));
        let pulses = output.window(beat(1, 4), &tempo, 48_000).unwrap();
        assert_eq!(pulses.len(), 6);
        assert_eq!(pulses[0].at, Beat::ZERO);
        assert!(matches!(
            output.window(Beat::ZERO, &tempo, 48_000),
            Err(ClockError::Backwards)
        ));
        assert_eq!(
            output.window(beat(1, 2), &tempo, 48_000).unwrap()[0].at,
            beat(1, 4)
        );
    }

    #[test]
    fn pulse_timeline_interpolates_irregular_backend_intervals_without_bpm_drift() {
        let mut timeline = PulseTimeline::new(4).unwrap();
        let base = beat(7, 4);
        timeline.push(base, 1_000_000).unwrap();
        timeline
            .push(base.checked_add(beat(1, 24)).unwrap(), 1_020_001)
            .unwrap();
        timeline
            .push(base.checked_add(beat(2, 24)).unwrap(), 1_049_007)
            .unwrap();
        assert_eq!(
            timeline
                .timestamp_at(base.checked_add(beat(1, 48)).unwrap())
                .unwrap(),
            1_010_001
        );
        let expected = base.checked_add(beat(10_000, 20_001 * 24)).unwrap();
        assert_eq!(timeline.beat_at(1_010_000).unwrap(), expected);
        assert_eq!(timeline.timestamp_at(expected).unwrap(), 1_010_000);
        assert!(matches!(
            timeline.timestamp_at(base.checked_add(beat(3, 24)).unwrap()),
            Err(ClockError::OutsideTimeline)
        ));
        let mut stamp = 1_049_007;
        for index in 3..1000 {
            stamp += 19_001 + index % 7;
            timeline
                .push(base.checked_add(beat(index, 24)).unwrap(), stamp)
                .unwrap();
        }
        let latest = base.checked_add(beat(999, 24)).unwrap();
        assert_eq!(timeline.latest(), Some((latest, stamp)));
        assert_eq!(timeline.timestamp_at(latest).unwrap(), stamp);
        assert_eq!(timeline.beat_at(stamp).unwrap(), latest);
        assert_eq!(
            timeline.earliest().unwrap().0,
            base.checked_add(beat(996, 24)).unwrap()
        );
        assert!(matches!(
            timeline.beat_at(1_049_007),
            Err(ClockError::OutsideTimeline)
        ));
    }

    #[test]
    fn timeline_rejects_discontinuities_without_eviction_and_clear_allows_repositioning() {
        let mut timeline = PulseTimeline::new(2).unwrap();
        timeline.push(Beat::ONE, 10).unwrap();
        timeline.push(beat(25, 24), 20).unwrap();
        for (at, timestamp) in [(beat(26, 24), 20), (beat(27, 24), 30)] {
            assert!(matches!(
                timeline.push(at, timestamp),
                Err(ClockError::InvalidTimelineOrder)
            ));
        }
        assert_eq!(timeline.earliest(), Some((Beat::ONE, 10)));
        timeline.clear();
        assert_eq!(timeline.latest(), None);
        timeline.push(beat(7, 4), 30).unwrap();
        assert_eq!(timeline.beat_at(30).unwrap(), beat(7, 4));
        assert!(matches!(
            timeline.beat_at(31),
            Err(ClockError::OutsideTimeline)
        ));
    }
}
