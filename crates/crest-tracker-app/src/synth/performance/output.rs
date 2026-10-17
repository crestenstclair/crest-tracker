//! One native destination adapter for released library playback and live events.
//! Each source owns native voices; these allocations are not an external MIDI zone.
use super::*;
use crest_linnstrument::{MidiMessage as DeviceMidi, MpeOutput, MpeZone};
use crest_performance_core::{DestinationId, Gesture, OutputEvent, VoiceId};
use crest_synth::real_time::scheduled_event::ScheduledLane;

struct OwnedVoice {
    id: MidiVoiceId,
    state: expression::VoiceState,
}
struct Destination {
    lane: ScheduledLane,
    patch: PatchId,
    midi: MpeOutput,
    voices: BTreeMap<VoiceId, OwnedVoice>,
}

pub(super) struct Outputs(BTreeMap<DestinationId, Destination>);
impl Outputs {
    pub fn new(
        document: &crest_tracker::performance::Document,
        app: &App,
        sound: &Sound<'_>,
        lane: ScheduledLane,
    ) -> Result<Self, String> {
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
                    lane,
                    patch,
                    midi: MpeOutput::new(route.destination, MpeZone::lower()),
                    voices: BTreeMap::new(),
                },
            );
        }
        Ok(Self(destinations))
    }
    pub fn schedule(
        &mut self,
        sound: &mut Sound<'_>,
        ids: &mut ScheduledVoices,
        frame: u64,
        event: OutputEvent,
        group: Option<crest_synth::real_time::VoiceGroupId>,
    ) -> Result<(), String> {
        self.0
            .get_mut(&event.destination)
            .ok_or("Performance destination is unbound")?
            .schedule(sound, ids, frame, event, group)
    }
}

impl Destination {
    fn send_midi(
        &self,
        sound: &mut Sound<'_>,
        frame: u64,
        voice: MidiVoiceId,
        message: MidiMessage,
    ) -> Result<(), String> {
        match self.lane {
            ScheduledLane::Playback => {
                sound.schedule_voice_midi(frame, self.patch, message, Some(voice))
            }
            ScheduledLane::Monitor => sound.schedule_owned_monitor_midi(
                frame,
                self.patch,
                voice,
                crest_synth::synth::MidiExpressionScope::Member,
                message,
            ),
        }
        .map_err(|e| e.to_string())
    }

    fn schedule(
        &mut self,
        sound: &mut Sound<'_>,
        ids: &mut ScheduledVoices,
        frame: u64,
        event: OutputEvent,
        group: Option<crest_synth::real_time::VoiceGroupId>,
    ) -> Result<(), String> {
        let batch = self.midi.accept(event).map_err(|e| e.to_string())?;
        match event.gesture {
            Gesture::On { .. } => {
                self.attack(sound, ids, frame, event.voice, batch.messages(), group)
            }
            Gesture::Expression { .. } => self.express(sound, frame, event.voice, batch.messages()),
            Gesture::Off { .. } => {
                let voice = self
                    .voices
                    .get(&event.voice)
                    .ok_or("Performance release has no native voice")?;
                // Wire-channel resets are unnecessary for owned native voices;
                // applying them after Note Off would silence the release tail.
                self.send_midi(sound, frame, voice.id, native_midi(batch.messages()[0])?)?;
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
        group: Option<crest_synth::real_time::VoiceGroupId>,
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
        let id = group.map_or(id, |group| id.with_group(group));
        let initial = state.initial(midi.data2())?;
        match self.lane {
            ScheduledLane::Playback => {
                sound.schedule_initialized_voice(frame, self.patch, id, initial)
            }
            ScheduledLane::Monitor => {
                sound.schedule_initialized_monitor_voice(frame, self.patch, id, initial)
            }
        }
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
            .get(&owner)
            .ok_or("Performance expression has no native voice")?;
        let mut state = voice.state.clone();
        for message in messages {
            state.apply(control(*message)?, false)?;
        }
        state.validate(sound, self.patch)?;
        let id = voice.id;
        for message in messages {
            self.send_midi(sound, frame, id, native_midi(*message)?)?;
        }
        self.voices
            .get_mut(&owner)
            .ok_or("Performance voice disappeared")?
            .state = state;
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
