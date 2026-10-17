//! A disposable library playback snapshot uses Tracker's native audio clock.
//! It never advances the editable performance session or accepts live input.
use super::output::Outputs;
use super::*;
use crest_performance_core::{BeatTime, Command, TICKS_PER_BEAT};
use crest_synth::real_time::scheduled_event::ScheduledLane;

pub(in crate::synth) struct Playback {
    engine: Performance,
    destinations: Outputs,
    anchor: u64,
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
    pub fn new(
        document: crest_tracker::performance::Document,
        slot: usize,
        app: &App,
        sound: &Sound<'_>,
        clock: HostClock,
    ) -> Result<Self, String> {
        if clock.sample_rate == 0 {
            return Err("Audio clock is not ready".into());
        }
        let destinations = Outputs::new(&document, app, sound, ScheduledLane::Playback)?;
        let mut engine = Performance::new(document, Geometry::Pads200, Limits::default())
            .map_err(|e| e.to_string())?;
        engine
            .command(Command::Launch {
                slot,
                quantum: BeatTime(0),
            })
            .map_err(|e| e.to_string())?;
        let anchor = clock
            .audio
            .rendered_frames()
            .checked_add(u64::from(clock.sample_rate) / 20)
            .ok_or("Audio clock overflow")?;
        Ok(Self {
            engine,
            destinations,
            anchor,
            horizon: anchor,
            rate: clock.sample_rate,
            graph: sound.active_graph_revision().value(),
            tempo: TempoMap::new(app.editor.song().tempo),
            tempo_at_start: app.editor.song().tempo,
            slot,
        })
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
        Ok(())
    }
}
