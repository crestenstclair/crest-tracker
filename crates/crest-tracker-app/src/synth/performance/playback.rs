//! A disposable library playback snapshot uses Tracker's native audio clock.
//! It never advances the editable performance session or accepts live input.
use super::*;
use crest_linnstrument::{MidiMessage as DeviceMidi, MpeOutput, MpeZone};
use crest_performance_core::{
    BeatTime, Command, DestinationId, Gesture, OutputEvent, VoiceId, TICKS_PER_BEAT,
};

struct OwnedVoice {
    id: MidiVoiceId,
    state: expression::VoiceState,
}
struct Destination {
    patch: PatchId,
    midi: MpeOutput,
    voices: BTreeMap<VoiceId, OwnedVoice>,
}

pub(in crate::synth) struct Playback {
    engine: Performance,
    destinations: BTreeMap<DestinationId, Destination>,
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
        document
            .validate_instruments(app.editor.song())
            .map_err(|e| e.to_string())?;
        let mut destinations = BTreeMap::new();
        for route in &document.routes {
            let patch = PatchId::new(
                *app.mappings
                    .get(&route.instrument)
                    .ok_or("Performance instrument is not installed")?,
            )
            .map_err(|e| e.to_string())?;
            sound
                .owned_expression_profile(patch)
                .map_err(|e| e.to_string())?;
            destinations.insert(
                route.destination,
                Destination {
                    patch,
                    midi: MpeOutput::new(route.destination, MpeZone::lower()),
                    voices: BTreeMap::new(),
                },
            );
        }
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
            self.destinations
                .get_mut(&event.destination)
                .ok_or("Performance destination is unbound")?
                .schedule(sound, ids, frame, event)?;
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

impl Destination {
    fn schedule(
        &mut self,
        sound: &mut Sound<'_>,
        ids: &mut ScheduledVoices,
        frame: u64,
        event: OutputEvent,
    ) -> Result<(), String> {
        let batch = self.midi.accept(event).map_err(|e| e.to_string())?;
        match event.gesture {
            Gesture::On { .. } => self.attack(sound, ids, frame, event.voice, batch.messages()),
            Gesture::Expression { .. } => self.express(sound, frame, event.voice, batch.messages()),
            Gesture::Off { .. } => {
                let voice = self
                    .voices
                    .get(&event.voice)
                    .ok_or("Performance release has no native voice")?;
                // Wire-channel resets are unnecessary for owned native voices;
                // applying them after Note Off would silence the release tail.
                sound
                    .schedule_voice_midi(
                        frame,
                        self.patch,
                        native_midi(batch.messages()[0])?,
                        Some(voice.id),
                    )
                    .map_err(|e| e.to_string())?;
                self.voices.remove(&event.voice);
                Ok(())
            }
        }
    }

    fn attack(
        &mut self,
        sound: &mut Sound<'_>,
        ids: &mut ScheduledVoices,
        frame: u64,
        owner: VoiceId,
        messages: &[DeviceMidi],
    ) -> Result<(), String> {
        let note = messages.last().ok_or("Performance attack is empty")?;
        let midi = native_midi(*note)?;
        if midi.kind() != MidiMessageKind::NoteOn {
            return Err("Performance attack lacks Note On".into());
        }
        let mut state = expression::VoiceState::new(
            midi.data1(),
            Channel::new(midi.channel().value() + 1).map_err(|e| e.to_string())?,
        );
        // Reuse the released output allocator and its exact MIDI resolution.
        // This destination explicitly negotiates the lower 15-member, ±48 profile.
        for (number, value) in [
            (101, 0),
            (100, 0),
            (6, MpeZone::lower().bend_semitones()),
            (38, 0),
            (101, 127),
            (100, 127),
        ] {
            state.apply(Control::Cc { number, value }, false)?;
        }
        for message in &messages[..messages.len() - 1] {
            state.apply(control(*message)?, false)?;
        }
        state.validate(sound, self.patch)?;
        let id = ids.allocate()?;
        sound
            .schedule_initialized_voice(frame, self.patch, id, state.initial(midi.data2())?)
            .map_err(|e| e.to_string())?;
        self.voices.insert(owner, OwnedVoice { id, state });
        Ok(())
    }

    fn express(
        &mut self,
        sound: &mut Sound<'_>,
        frame: u64,
        owner: VoiceId,
        messages: &[DeviceMidi],
    ) -> Result<(), String> {
        let voice = self
            .voices
            .get_mut(&owner)
            .ok_or("Performance expression has no native voice")?;
        let mut state = voice.state.clone();
        for message in messages {
            state.apply(control(*message)?, false)?;
        }
        state.validate(sound, self.patch)?;
        for message in messages {
            sound
                .schedule_voice_midi(frame, self.patch, native_midi(*message)?, Some(voice.id))
                .map_err(|e| e.to_string())?;
        }
        voice.state = state;
        Ok(())
    }
}

fn native_midi(message: DeviceMidi) -> Result<MidiMessage, String> {
    let bytes = message.bytes();
    let kind = match bytes[0] & 0xf0 {
        0x80 => MidiMessageKind::NoteOff,
        0x90 => MidiMessageKind::NoteOn,
        0xb0 => MidiMessageKind::ControlChange,
        0xd0 => MidiMessageKind::ChannelPressure,
        0xe0 => MidiMessageKind::PitchBend,
        _ => return Err("Unsupported performance MIDI message".into()),
    };
    MidiMessage::try_new(
        MidiChannel::new(bytes[0] & 15).map_err(|e| e.to_string())?,
        kind,
        bytes[1],
        *bytes.get(2).unwrap_or(&0),
    )
    .map_err(|e| e.to_string())
}
fn control(message: DeviceMidi) -> Result<Control, String> {
    let midi = native_midi(message)?;
    match midi.kind() {
        MidiMessageKind::PitchBend => Ok(Control::Bend {
            value: u16::from(midi.data1()) | u16::from(midi.data2()) << 7,
        }),
        MidiMessageKind::ChannelPressure => Ok(Control::Pressure {
            value: midi.data1(),
        }),
        MidiMessageKind::ControlChange => Ok(Control::Cc {
            number: midi.data1(),
            value: midi.data2(),
        }),
        _ => Err("Expected owned expression".into()),
    }
}
