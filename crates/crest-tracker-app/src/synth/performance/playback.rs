//! A disposable library playback snapshot uses Tracker's native audio clock.
//! It never advances the editable performance session or accepts live input.
use super::output::Outputs;
use super::*;
use crest_performance_core::{BeatTime, Command, SlotState, TICKS_PER_BEAT};
use crest_synth::real_time::scheduled_event::ScheduledLane;

pub(in crate::synth) struct Playback {
    engine: Performance,
    destinations: Outputs,
    anchor: u64,
    launch_frame: u64,
    rendered_frame: u64,
    horizon: u64,
    rate: u32,
    graph: u64,
    tempo: TempoMap,
    pub tempo_at_start: crest_tracker::time::Tempo,
    pub slot: usize,
}
impl Playback {
    #[cfg(test)]
    pub fn anchor(&self) -> u64 {
        self.anchor
    }
    #[cfg(test)]
    pub fn launch_frame(&self) -> u64 {
        self.launch_frame
    }
    #[cfg(test)]
    pub fn scheduler_slot_state(&self) -> SlotState {
        self.engine.snapshot().slots[self.slot]
    }
    pub fn new(
        document: crest_tracker::performance::Document,
        slot: usize,
        app: &App,
        sound: &Sound<'_>,
        clock: HostClock,
        grid: crate::app::PerformanceLaunchGrid,
    ) -> Result<Self, String> {
        if clock.sample_rate == 0 {
            return Err("Audio clock is not ready".into());
        }
        let destinations = Outputs::new(&document, app, sound, ScheduledLane::Playback)?;
        let mut engine = Performance::new(document, Geometry::Pads200, Limits::default())
            .map_err(|e| e.to_string())?;
        let earliest = clock
            .audio
            .rendered_frames()
            .checked_add(u64::from(clock.sample_rate) / 20)
            .ok_or("Audio clock overflow")?;
        let tempo = TempoMap::new(app.editor.song().tempo);
        // Quantized launches share audio-frame zero as a stable local grid.
        // Immediate audition retains its existing 50 ms scheduling lead.
        let anchor = if grid == crate::app::PerformanceLaunchGrid::Immediate {
            earliest
        } else {
            0
        };
        let launch_beat = launch_engine(
            &mut engine,
            slot,
            grid.quantum(),
            &tempo,
            earliest - anchor,
            clock.sample_rate,
        )?;
        let launch_frame = anchor
            .checked_add(
                tempo
                    .frame_at(launch_beat, clock.sample_rate)
                    .map_err(|e| e.to_string())?,
            )
            .ok_or("Audio clock overflow")?;
        Ok(Self {
            engine,
            destinations,
            anchor,
            launch_frame,
            rendered_frame: clock.audio.rendered_frames(),
            horizon: earliest,
            rate: clock.sample_rate,
            graph: sound.active_graph_revision().value(),
            tempo,
            tempo_at_start: app.editor.song().tempo,
            slot,
        })
    }

    pub fn slot_state(&self) -> (usize, SlotState) {
        (
            self.slot,
            if self.rendered_frame < self.launch_frame {
                SlotState::Queued
            } else {
                SlotState::Playing
            },
        )
    }

    pub fn advance(
        &mut self,
        sound: &mut Sound<'_>,
        clock: HostClock,
        ids: &mut ScheduledVoices,
    ) -> Result<(), String> {
        if clock.sample_rate != self.rate || sound.active_graph_revision().value() != self.graph {
            return Err("Sound graph or sample rate changed; performance audition stopped".into());
        }
        let now = clock.audio.rendered_frames();
        if now > self.horizon {
            return Err("Performance audition missed its audio deadline; stopped safely".into());
        }
        let horizon = now
            .checked_add(u64::from(self.rate) / 10)
            .ok_or("Audio clock overflow")?;
        let beat = self
            .tempo
            .beat_at_frame(horizon.saturating_sub(self.anchor), self.rate)
            .map_err(|e| e.to_string())?;
        // Scheduling horizon rounds down by less than one library tick. Event
        // positions themselves remain exact; only the final sample conversion rounds.
        let ticks = u128::from(beat.numerator()) * u128::from(TICKS_PER_BEAT)
            / u128::from(beat.denominator());
        let at = crest_tracker::performance::tracker_time(BeatTime(
            u64::try_from(ticks).map_err(|_| "Performance time overflow")?,
        ))
        .map_err(|e| e.to_string())?;
        self.engine.advance(at).map_err(|e| e.to_string())?;
        for event in self.engine.drain_output() {
            let beat =
                crest_tracker::performance::tracker_time(event.at).map_err(|e| e.to_string())?;
            let frame = self
                .anchor
                .checked_add(
                    self.tempo
                        .frame_at(beat, self.rate)
                        .map_err(|e| e.to_string())?,
                )
                .ok_or("Audio event time overflow")?;
            if frame < now {
                return Err("Performance event arrived after its audio deadline".into());
            }
            self.destinations.schedule(sound, ids, frame, event)?;
        }
        // The exact completed horizon is important: do not promise time that
        // rounding deferred to the next window.
        self.horizon = self
            .anchor
            .checked_add(
                self.tempo
                    .frame_at(at, self.rate)
                    .map_err(|e| e.to_string())?,
            )
            .ok_or("Audio clock overflow")?;
        self.rendered_frame = now;
        Ok(())
    }
}

/// Advance to the first representable tick at/after the safe scheduling frame,
/// then let the library choose the grid boundary. The query avoids duplicating
/// its quantization policy in the host or deriving state from future events.
fn launch_engine(
    engine: &mut Performance,
    slot: usize,
    quantum: BeatTime,
    tempo: &TempoMap,
    earliest_frame: u64,
    rate: u32,
) -> Result<Beat, String> {
    let earliest = tempo
        .beat_at_frame(earliest_frame, rate)
        .map_err(|e| e.to_string())?;
    let ticks = (u128::from(earliest.numerator()) * u128::from(TICKS_PER_BEAT))
        .div_ceil(u128::from(earliest.denominator()));
    let at = BeatTime(u64::try_from(ticks).map_err(|_| "Performance time overflow")?);
    engine
        .advance(crest_tracker::performance::tracker_time(at).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    engine
        .command(Command::Launch { slot, quantum })
        .map_err(|e| e.to_string())?;
    let launch = engine
        .slot_launch_time(slot)
        .map_err(|e| e.to_string())?
        .ok_or("Library did not retain launch boundary")?;
    crest_tracker::performance::tracker_time(launch).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crest_performance_core::{DestinationId, Gesture, PatternSettings, Session, TrackId};
    use crest_tracker::performance::{Document, Route};

    fn engine() -> Performance {
        let mut session = Session::default();
        session.slots[0].clip = Some(
            PatternSettings {
                steps: 1,
                pulses: 1,
                tone_min: 0,
                tone_max: 0,
                step: BeatTime(TICKS_PER_BEAT),
                gate: BeatTime(TICKS_PER_BEAT / 2),
                ..PatternSettings::default()
            }
            .render(session.harmony, TrackId(1))
            .unwrap(),
        );
        Performance::new(
            Document {
                version: 1,
                session,
                routes: vec![Route {
                    destination: DestinationId(1),
                    instrument: InstrumentId::new(1).unwrap(),
                }],
            },
            Geometry::Pads200,
            Limits::default(),
        )
        .unwrap()
    }

    #[test]
    fn library_launch_rounds_up_from_safe_audio_frame_and_keeps_exact_boundary() {
        let tempo = TempoMap::new(crest_tracker::time::Tempo::new(120_000).unwrap());
        for (frame, beats, expected_frame) in [
            (23_999, 1, 24_000),
            (24_000, 1, 24_000),
            (24_001, 1, 48_000),
            (24_001, 4, 96_000),
        ] {
            let mut engine = engine();
            let launch = launch_engine(
                &mut engine,
                0,
                BeatTime(beats * TICKS_PER_BEAT),
                &tempo,
                frame,
                48_000,
            )
            .unwrap();
            assert_eq!(tempo.frame_at(launch, 48_000).unwrap(), expected_frame);
            assert_eq!(engine.drain_output().count(), 0);
            engine.advance(launch).unwrap();
            let events: Vec<_> = engine.drain_output().collect();
            assert_eq!(events.len(), 1);
            assert!(matches!(events[0].gesture, Gesture::On { key: 60, .. }));
            assert_eq!(
                tempo
                    .frame_at(
                        crest_tracker::performance::tracker_time(events[0].at).unwrap(),
                        48_000
                    )
                    .unwrap(),
                expected_frame
            );
        }
    }

    #[test]
    fn canceled_queue_never_emits_and_nonintegral_sample_conversion_never_launches_early() {
        let tempo = TempoMap::new(crest_tracker::time::Tempo::new(123_456).unwrap());
        let mut engine = engine();
        let at = launch_engine(
            &mut engine,
            0,
            BeatTime(TICKS_PER_BEAT),
            &tempo,
            44_101,
            44_100,
        )
        .unwrap();
        assert_eq!(at, Beat::integer(3));
        assert!(tempo.frame_at(at, 44_100).unwrap() >= 44_101);
        engine.command(Command::Stop).unwrap();
        engine.advance(Beat::integer(10)).unwrap();
        assert_eq!(engine.drain_output().count(), 0);
        assert_eq!(engine.slot_launch_time(0).unwrap(), None);
    }
}
