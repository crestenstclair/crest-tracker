//! Input time follows accepted device packets, never the playback lookahead.
//! Monitoring has a fixed 50 ms lead; missed deadlines fail without moving notes.
use super::*;
use crest_performance_core::{BeatTime, Command, OutputEvent, SlotState, TICKS_PER_BEAT};
use crest_synth::control::PhysicalMidiEvent;
use crest_synth::real_time::scheduled_event::ScheduledLane;
use std::collections::VecDeque;

pub(in crate::synth) struct Live {
    pub engine: Performance,
    outputs: output::Outputs,
    pending: VecDeque<(u64, OutputEvent)>,
    anchor: u64,
    last_input: u64,
    rate: u32,
    tempo: TempoMap,
    tempo_at_start: crest_tracker::time::Tempo,
    graph: u64,
    revision: u64,
    overflow: u64,
    pub fault: Option<String>,
}
impl Live {
    pub fn new(
        document: crest_tracker::performance::Document,
        app: &App,
        sound: &Sound<'_>,
        clock: HostClock,
        revision: u64,
        overflow: u64,
    ) -> Result<Self, String> {
        if clock.sample_rate == 0
            || clock.audio_clock.is_none_or(|c| {
                c.sample_rate() != clock.sample_rate
                    || c.frame_at_midi_timestamp(c.callback_micros()).is_none()
            })
        {
            return Err("LinnStrument input requires correlated MIDI/audio clocks".into());
        }
        let outputs = output::Outputs::new(&document, app, sound, ScheduledLane::Monitor)?;
        let engine = Performance::new(document, Geometry::Pads200, Limits::default())
            .map_err(|e| e.to_string())?;
        let anchor = clock.audio.rendered_frames();
        Ok(Self {
            engine,
            outputs,
            pending: VecDeque::with_capacity(Limits::default().output_events),
            anchor,
            last_input: anchor,
            rate: clock.sample_rate,
            tempo: TempoMap::new(app.editor.song().tempo),
            tempo_at_start: app.editor.song().tempo,
            graph: sound.active_graph_revision().value(),
            revision,
            overflow,
            fault: None,
        })
    }

    pub fn accept(&mut self, event: PhysicalMidiEvent, clock: HostClock) -> Result<(), String> {
        if self.fault.is_some() {
            return Ok(());
        }
        if event.revision().value() != self.revision {
            return Err("LinnStrument connection changed; input stopped".into());
        }
        let frame = clock
            .audio_clock
            .and_then(|c| c.frame_at_midi_timestamp(event.timestamp_micros()))
            .ok_or("MIDI/audio clock correlation unavailable")?;
        let bytes = event.raw_bytes();
        if bytes.is_empty() {
            let (bytes, len) = normalized_bytes(event.message());
            self.accept_at(frame, &bytes[..len])
        } else {
            self.accept_at(frame, bytes)
        }
    }

    /// Device frames precede monitoring lead. Pre-activation packets are excluded;
    /// subsequent backwards timestamps reject even if they round to the same tick.
    fn accept_at(&mut self, frame: u64, bytes: &[u8]) -> Result<(), String> {
        if frame < self.anchor {
            return Ok(());
        }
        if frame < self.last_input {
            return Err("LinnStrument timestamp moved backwards".into());
        }
        let at = self.beat_at(frame)?;
        self.engine
            .accept_midi(at, bytes)
            .map_err(|e| e.to_string())?;
        self.last_input = frame;
        let delivery = frame
            .checked_add(u64::from(self.rate) / 20)
            .ok_or("Monitor clock overflow")?;
        for event in self.engine.drain_output() {
            if self.pending.len() == Limits::default().output_events {
                return Err("LinnStrument monitor queue full; take retained".into());
            }
            self.pending.push_back((delivery, event));
        }
        Ok(())
    }

    /// Conversion rounds down less than one library tick; source ordering and
    /// free timing survive. Monitoring retains the original source frame exactly.
    fn beat_at(&self, frame: u64) -> Result<Beat, String> {
        let relative = frame
            .checked_sub(self.anchor)
            .ok_or("Input predates activation")?;
        let beat = self
            .tempo
            .beat_at_frame(relative, self.rate)
            .map_err(|e| e.to_string())?;
        let ticks = u128::from(beat.numerator()) * u128::from(TICKS_PER_BEAT)
            / u128::from(beat.denominator());
        crest_tracker::performance::tracker_time(BeatTime(
            u64::try_from(ticks).map_err(|_| "Input time overflow")?,
        ))
        .map_err(|e| e.to_string())
    }

    pub fn advance(
        &mut self,
        sound: &mut Sound<'_>,
        clock: HostClock,
        tempo: crest_tracker::time::Tempo,
        ids: &mut ScheduledVoices,
    ) -> Result<(), String> {
        if self.fault.is_some() {
            return Ok(());
        }
        self.validate_clock(sound, clock, tempo)?;
        self.flush(sound, clock, ids)
    }

    fn validate_clock(
        &self,
        sound: &Sound<'_>,
        clock: HostClock,
        tempo: crest_tracker::time::Tempo,
    ) -> Result<(), String> {
        if clock.sample_rate != self.rate
            || sound.active_graph_revision().value() != self.graph
            || tempo != self.tempo_at_start
        {
            return Err(
                "Tempo, sound graph or sample rate changed; LinnStrument input stopped".into(),
            );
        }
        Ok(())
    }

    /// Capture at the click's audio boundary without moving input time. Packets
    /// still in flight retain their original timestamps and live ownership.
    pub fn capture_recent(
        &mut self,
        slot: usize,
        sound: &Sound<'_>,
        clock: HostClock,
        tempo: crest_tracker::time::Tempo,
    ) -> Result<(), String> {
        if self.fault.is_some() {
            return Err("Finish or discard the interrupted input before capturing".into());
        }
        self.validate_clock(sound, clock, tempo)?;
        let cutoff = self.close_boundary(Some(clock.audio.rendered_frames()))?;
        self.engine
            .capture_recent_at(slot, BeatTime((4 * TICKS_PER_BEAT).min(cutoff.0)), cutoff)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn validate_source(&self, sound: &Sound<'_>) -> Result<(), String> {
        if sound
            .active_midi_input_identity()
            .map(|input| input.revision().value())
            != Some(self.revision)
            || sound
                .midi_activity()
                .is_some_and(|input| input.overflow_epoch() != self.overflow)
        {
            return Err("LinnStrument disconnected or input overflowed; take retained".into());
        }
        Ok(())
    }

    fn flush(
        &mut self,
        sound: &mut Sound<'_>,
        clock: HostClock,
        ids: &mut ScheduledVoices,
    ) -> Result<(), String> {
        while let Some((frame, event)) = self.pending.pop_front() {
            if frame < clock.audio.rendered_frames() {
                return Err("LinnStrument monitor missed its deadline; take retained".into());
            }
            self.outputs.schedule(sound, ids, frame, event)?;
        }
        Ok(())
    }

    pub fn recording(&self) -> bool {
        self.engine.snapshot().slots.contains(&SlotState::Recording)
    }

    fn close_boundary(&self, frame: Option<u64>) -> Result<BeatTime, String> {
        let after_last = self
            .engine
            .snapshot()
            .at
            .0
            .checked_add(1)
            .ok_or("Input time overflow")?;
        let at = match frame.filter(|_| self.fault.is_none()) {
            Some(frame) => crest_tracker::performance::performance_time(self.beat_at(frame)?)
                .map_err(|e| e.to_string())?
                .0
                .max(after_last),
            None => after_last,
        };
        Ok(BeatTime(at))
    }

    pub fn checkpoint(
        &self,
        frame: Option<u64>,
    ) -> Result<crest_tracker::performance::Document, String> {
        let cutoff = crest_tracker::performance::tracker_time(self.close_boundary(frame)?)
            .map_err(|e| e.to_string())?;
        self.engine
            .checkpoint_document_at(cutoff)
            .map_err(|e| e.to_string())
    }

    /// Explicitly close the take before releasing input ownership. On fault the
    /// close boundary is one tick after the last accepted gesture. No raw gesture
    /// is retimed; held recorded notes receive the library's loop-end release.
    pub fn finish(&mut self, frame: Option<u64>, discard: bool) -> Result<(), String> {
        self.pending.clear();
        self.engine.drain_output().for_each(drop);
        if discard {
            self.engine
                .command(Command::CancelRecord)
                .map_err(|e| e.to_string())?;
        } else if self.recording() {
            let cutoff = self.close_boundary(frame)?;
            self.engine
                .advance(
                    crest_tracker::performance::tracker_time(cutoff).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
            self.engine
                .command(Command::FinishRecord)
                .map_err(|e| e.to_string())?;
        }
        self.engine
            .command(Command::Stop)
            .map_err(|e| e.to_string())?;
        self.engine.drain_output().for_each(drop);
        Ok(())
    }
}

pub(super) fn normalized_bytes(message: MidiMessage) -> ([u8; 3], usize) {
    let (status, length) = match message.kind() {
        MidiMessageKind::NoteOff => (0x80, 3),
        MidiMessageKind::NoteOn => (0x90, 3),
        MidiMessageKind::PolyPressure => (0xa0, 3),
        MidiMessageKind::ControlChange | MidiMessageKind::AllNotesOff => (0xb0, 3),
        MidiMessageKind::ProgramChange => (0xc0, 2),
        MidiMessageKind::ChannelPressure => (0xd0, 2),
        MidiMessageKind::PitchBend => (0xe0, 3),
    };
    (
        [
            status | message.channel().value(),
            message.data1(),
            message.data2(),
        ],
        length,
    )
}
