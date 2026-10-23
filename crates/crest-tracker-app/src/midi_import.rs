//! Standard MIDI type 0/1 imports into the same exact-time Song used by the demo.
mod arrangement;
pub(crate) mod organization;
pub(crate) mod timeline;
pub(crate) use arrangement::arrange;
use crest_tracker::composition::*;
use crest_tracker::time::{Beat, Meter, Tempo};
use midly::{MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};
use std::collections::{BTreeMap, VecDeque};
use std::io::Read;
use std::path::Path;

pub(crate) fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(crate::persistence::MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > crate::persistence::MAX_DOCUMENT_BYTES {
        return Err("Import input exceeds the 64 MiB admission budget".into());
    }
    Ok(bytes)
}

pub struct Part {
    pub instrument: InstrumentId,
    pub name: String,
    pub channel: u8,
    pub bank: u16,
    pub program: u8,
}

pub struct Import {
    pub song: Song,
    pub parts: Vec<Part>,
    pub diagnostics: Diagnostics,
    pub end: Beat,
}

#[derive(Default, Debug, serde::Serialize)]
pub struct Diagnostics {
    pub one_tick_gates: usize,
    pub notes_closed_at_end: usize,
    pub unmatched_releases: usize,
}

struct Attack {
    at: u64,
    velocity: u8,
    instrument: InstrumentId,
}
struct Importer {
    song: Song,
    parts: Vec<Part>,
    lanes: [Vec<Event>; 16],
    next: u64,
    ppq: u16,
    programs: [u8; 16],
    banks: [u16; 16],
    controls: [Vec<Control>; 16],
    active: BTreeMap<(u8, u8), VecDeque<Attack>>,
    tempos: BTreeMap<u64, Tempo>,
    end: u64,
    diagnostics: Diagnostics,
}
impl Importer {
    fn beat(&self, tick: u64) -> Result<Beat, String> {
        Beat::new(tick, self.ppq as u64).map_err(|e| e.to_string())
    }
    fn identity(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }
    fn target(instrument: InstrumentId, channel: u8) -> Target {
        Target::Instrument {
            id: instrument,
            channel: Channel::new(channel + 1).unwrap(),
        }
    }
    fn instrument(&mut self, channel: u8, name: &str, tick: u64) -> Result<InstrumentId, String> {
        let bank = if channel == 9 {
            128 + self.banks[9]
        } else {
            self.banks[channel as usize]
        };
        let program = self.programs[channel as usize];
        if let Some(part) = self
            .parts
            .iter()
            .find(|p| p.channel == channel && p.bank == bank && p.program == program)
        {
            return Ok(part.instrument);
        }
        let instrument = InstrumentId::new(self.identity()).map_err(|e| e.to_string())?;
        let name = if name.is_empty() {
            format!("Channel {} · program {}", channel + 1, program)
        } else {
            format!("{name} · program {program}")
        };
        self.song.instruments.insert(
            instrument,
            InstrumentUse {
                name: name.clone(),
                resource: None,
            },
        );
        self.parts.push(Part {
            instrument,
            name,
            channel,
            bank,
            program,
        });
        for control in self.controls[channel as usize].clone() {
            self.push_control(tick, channel, instrument, control)?;
        }
        Ok(instrument)
    }
    fn push_control(
        &mut self,
        tick: u64,
        channel: u8,
        instrument: InstrumentId,
        control: Control,
    ) -> Result<(), String> {
        let id = EventId::new(self.identity()).map_err(|e| e.to_string())?;
        let at = self.beat(tick)?;
        self.end = self.end.max(tick + 1);
        self.lanes[channel as usize].push(Event {
            id,
            at,
            target: Self::target(instrument, channel),
            note: None,
            controls: vec![control],
            fx: [None, None, None],
        });
        Ok(())
    }
    fn control(&mut self, tick: u64, channel: u8, control: Control) -> Result<(), String> {
        self.instrument(channel, "", tick)?;
        let controls = &mut self.controls[channel as usize];
        controls.retain(|old| !same_control(*old, control));
        controls.push(control);
        let ids: Vec<_> = self
            .parts
            .iter()
            .filter(|p| p.channel == channel)
            .map(|p| p.instrument)
            .collect();
        for id in ids {
            self.push_control(tick, channel, id, control)?;
        }
        Ok(())
    }
    fn release(&mut self, tick: u64, channel: u8, pitch: u8, velocity: u8) -> Result<(), String> {
        let Some(attack) = self
            .active
            .get_mut(&(channel, pitch))
            .and_then(VecDeque::pop_front)
        else {
            self.diagnostics.unmatched_releases += 1;
            return Ok(());
        };
        self.diagnostics.one_tick_gates += usize::from(tick == attack.at);
        let finish = tick.max(attack.at + 1);
        let mut note = Note::new(pitch, attack.velocity, self.beat(finish - attack.at)?)
            .map_err(|e| e.to_string())?;
        note.release_velocity = velocity;
        let id = EventId::new(self.identity()).map_err(|e| e.to_string())?;
        let at = self.beat(attack.at)?;
        self.lanes[channel as usize].push(Event::note(
            id,
            at,
            Self::target(attack.instrument, channel),
            note,
        ));
        self.end = self.end.max(finish);
        Ok(())
    }
    fn midi(
        &mut self,
        tick: u64,
        channel: u8,
        message: MidiMessage,
        name: &str,
    ) -> Result<(), String> {
        match message {
            MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => {
                let instrument = self.instrument(channel, name, tick)?;
                self.active
                    .entry((channel, key.as_int()))
                    .or_default()
                    .push_back(Attack {
                        at: tick,
                        velocity: vel.as_int(),
                        instrument,
                    });
            }
            MidiMessage::NoteOn { key, .. } => self.release(tick, channel, key.as_int(), 0)?,
            MidiMessage::NoteOff { key, vel } => {
                self.release(tick, channel, key.as_int(), vel.as_int())?
            }
            MidiMessage::ProgramChange { program } => {
                self.programs[channel as usize] = program.as_int()
            }
            MidiMessage::Controller { controller, value } => match controller.as_int() {
                0 => {
                    self.banks[channel as usize] =
                        (value.as_int() as u16) * 128 + (self.banks[channel as usize] % 128)
                }
                32 => {
                    self.banks[channel as usize] =
                        (self.banks[channel as usize] / 128) * 128 + value.as_int() as u16
                }
                number => self.control(
                    tick,
                    channel,
                    Control::Cc {
                        number,
                        value: value.as_int(),
                    },
                )?,
            },
            MidiMessage::PitchBend { bend } => self.control(
                tick,
                channel,
                Control::Bend {
                    value: bend.0.as_int(),
                },
            )?,
            MidiMessage::ChannelAftertouch { vel } => self.control(
                tick,
                channel,
                Control::Pressure {
                    value: vel.as_int(),
                },
            )?,
            MidiMessage::Aftertouch { key, vel } => self.control(
                tick,
                channel,
                Control::PolyPressure {
                    pitch: key.as_int(),
                    value: vel.as_int(),
                },
            )?,
        }
        Ok(())
    }
    fn meta(&mut self, tick: u64, message: MetaMessage<'_>) -> Result<(), String> {
        match message {
            MetaMessage::Tempo(value) => {
                let micros = value.as_int() as u64;
                if micros == 0 {
                    return Err("MIDI tempo must be positive".into());
                }
                let milli_bpm = u32::try_from((60_000_000_000 + micros / 2) / micros)
                    .map_err(|_| "MIDI tempo exceeds Tracker's representable range")?;
                self.tempos
                    .insert(tick, Tempo::new(milli_bpm).map_err(|e| e.to_string())?);
            }
            MetaMessage::TimeSignature(n, d, _, _) => {
                let denominator = 1_u16
                    .checked_shl(d as u32)
                    .ok_or("Unsupported meter denominator")?;
                let at = self.beat(tick)?;
                let meter = Meter::new(n as u16, denominator).map_err(|e| e.to_string())?;
                self.song.meters.retain(|m| m.at != at);
                self.song.meters.push(MeterChange { at, meter });
            }
            _ => {}
        }
        Ok(())
    }
    fn finish(mut self) -> Result<Import, String> {
        let held: Vec<_> = self.active.iter().map(|(key, v)| (*key, v.len())).collect();
        for ((channel, pitch), count) in held {
            self.diagnostics.notes_closed_at_end += count;
            self.release_remaining(channel, pitch, count)?;
        }
        if !self
            .lanes
            .iter()
            .flatten()
            .any(|event| event.note.is_some())
        {
            return Err("MIDI file contains no notes".into());
        }
        let tempos = std::mem::take(&mut self.tempos);
        for (tick, tempo) in tempos {
            self.tempo_event(tick, tempo)?;
        }
        self.song.meters.sort_by_key(|m| m.at);
        let end = self.beat(self.end.max(1))?;
        let mut imported = arrange(self.song, self.parts, self.lanes, self.next, end)?;
        imported.diagnostics = self.diagnostics;
        Ok(imported)
    }
    fn release_remaining(&mut self, channel: u8, pitch: u8, count: usize) -> Result<(), String> {
        for _ in 0..count {
            self.release(self.end, channel, pitch, 0)?;
        }
        Ok(())
    }
    fn tempo_event(&mut self, tick: u64, tempo: Tempo) -> Result<(), String> {
        if tick == 0 {
            self.song.tempo = tempo;
            return Ok(());
        }
        let target = Self::target(self.parts[0].instrument, self.parts[0].channel);
        let id = EventId::new(self.identity()).map_err(|e| e.to_string())?;
        let at = self.beat(tick)?;
        self.end = self.end.max(tick + 1);
        self.lanes[0].push(Event {
            id,
            at,
            target,
            note: None,
            controls: vec![],
            fx: [Some(Fx::Tempo { tempo }), None, None],
        });
        Ok(())
    }
}
fn same_control(a: Control, b: Control) -> bool {
    match (a, b) {
        (Control::Cc { number: a, .. }, Control::Cc { number: b, .. }) => a == b,
        (Control::PolyPressure { pitch: a, .. }, Control::PolyPressure { pitch: b, .. }) => a == b,
        _ => std::mem::discriminant(&a) == std::mem::discriminant(&b),
    }
}

pub(crate) fn parse(bytes: &[u8], name: &str) -> Result<Import, String> {
    if bytes.len() as u64 > crate::persistence::MAX_DOCUMENT_BYTES {
        return Err("MIDI file exceeds the 64 MiB import budget".into());
    }
    let smf = Smf::parse(bytes).map_err(|e| e.to_string())?;
    let Timing::Metrical(ppq) = smf.header.timing else {
        return Err("SMPTE MIDI timing is not supported; export metrical type 0 or 1".into());
    };
    if smf.header.format == midly::Format::Sequential || ppq.as_int() == 0 {
        return Err("Use MIDI type 0 or 1 with positive PPQ".into());
    }
    let mut importer = Importer {
        song: Song {
            name: name.into(),
            ..Song::default()
        },
        parts: vec![],
        lanes: std::array::from_fn(|_| vec![]),
        next: 17,
        ppq: ppq.as_int(),
        programs: [0; 16],
        banks: [0; 16],
        controls: std::array::from_fn(|_| vec![]),
        active: BTreeMap::new(),
        tempos: BTreeMap::new(),
        end: 0,
        diagnostics: Diagnostics::default(),
    };
    let names: Vec<_> = smf
        .tracks
        .iter()
        .map(|t| {
            t.iter()
                .find_map(|e| {
                    if let TrackEventKind::Meta(MetaMessage::TrackName(n)) = e.kind {
                        Some(String::from_utf8_lossy(n).into_owned())
                    } else {
                        None
                    }
                })
                .unwrap_or_default()
        })
        .collect();
    let mut events = Vec::new();
    for (track, source) in smf.tracks.iter().enumerate() {
        events.extend(absolute_events(track, source)?);
    }
    events.sort_by_key(|(tick, track, index, _)| (*tick, *track, *index));
    for (tick, track, _, event) in events {
        importer.end = importer.end.max(tick);
        match event {
            TrackEventKind::Midi { channel, message } => importer.midi(tick, channel.as_int(), message, &names[track])?,
            TrackEventKind::Meta(message) => importer.meta(tick, message)?,
            TrackEventKind::SysEx(_) | TrackEventKind::Escape(_) => return Err("System-exclusive MIDI requires device-specific interpretation; remove SysEx before import".into()),
        }
    }
    importer.finish()
}
fn absolute_events<'a>(
    track: usize,
    events: &[midly::TrackEvent<'a>],
) -> Result<Vec<(u64, usize, usize, TrackEventKind<'a>)>, String> {
    let mut tick = 0_u64;
    let mut result = Vec::new();
    for (index, event) in events.iter().enumerate() {
        tick = tick
            .checked_add(event.delta.as_int() as u64)
            .ok_or("MIDI tick overflow")?;
        result.push((tick, track, index, event.kind));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn file(events: Vec<(u32, TrackEventKind<'static>)>) -> Vec<u8> {
        let smf = Smf {
            header: midly::Header::new(midly::Format::SingleTrack, Timing::Metrical(48.into())),
            tracks: vec![events
                .into_iter()
                .map(|(delta, kind)| midly::TrackEvent {
                    delta: delta.into(),
                    kind,
                })
                .collect()],
        };
        let mut bytes = Vec::new();
        smf.write_std(&mut bytes).unwrap();
        bytes
    }
    fn midi(channel: u8, message: MidiMessage) -> TrackEventKind<'static> {
        TrackEventKind::Midi {
            channel: channel.into(),
            message,
        }
    }
    #[test]
    fn control_only_file_does_not_replace_song_with_empty_import() {
        let bytes = file(vec![(
            0,
            midi(
                0,
                MidiMessage::Controller {
                    controller: 7.into(),
                    value: 100.into(),
                },
            ),
        )]);
        assert_eq!(
            parse(&bytes, "No notes").err().as_deref(),
            Some("MIDI file contains no notes")
        );
    }
    #[test]
    fn type_zero_preserves_channels_program_changes_release_values_and_tempo_meter() {
        let bytes = file(vec![
            (
                0,
                TrackEventKind::Meta(MetaMessage::TimeSignature(3, 2, 24, 8)),
            ),
            (0, TrackEventKind::Meta(MetaMessage::Tempo(500000.into()))),
            (0, midi(0, MidiMessage::ProgramChange { program: 5.into() })),
            (
                0,
                midi(
                    0,
                    MidiMessage::NoteOn {
                        key: 60.into(),
                        vel: 100.into(),
                    },
                ),
            ),
            (
                0,
                midi(
                    9,
                    MidiMessage::NoteOn {
                        key: 36.into(),
                        vel: 90.into(),
                    },
                ),
            ),
            (
                48,
                midi(0, MidiMessage::ProgramChange { program: 10.into() }),
            ),
            (
                0,
                midi(
                    0,
                    MidiMessage::NoteOn {
                        key: 64.into(),
                        vel: 80.into(),
                    },
                ),
            ),
            (
                48,
                midi(
                    0,
                    MidiMessage::NoteOff {
                        key: 60.into(),
                        vel: 37.into(),
                    },
                ),
            ),
            (0, TrackEventKind::Meta(MetaMessage::Tempo(600000.into()))),
            (
                48,
                midi(
                    0,
                    MidiMessage::NoteOff {
                        key: 64.into(),
                        vel: 22.into(),
                    },
                ),
            ),
            (
                0,
                midi(
                    9,
                    MidiMessage::NoteOff {
                        key: 36.into(),
                        vel: 0.into(),
                    },
                ),
            ),
        ]);
        let imported = parse(&bytes, "Type zero").unwrap();
        imported.song.validate().unwrap();
        assert_eq!(imported.parts.len(), 3);
        assert!(imported
            .parts
            .iter()
            .any(|p| p.bank == 128 && p.channel == 9));
        let events: Vec<_> = imported
            .song
            .phrases
            .values()
            .flat_map(|p| &p.events)
            .collect();
        let first = events
            .iter()
            .find(|e| e.note.as_ref().is_some_and(|n| n.pitch == 60))
            .unwrap();
        assert_eq!(first.note.as_ref().unwrap().duration, Beat::integer(2));
        assert_eq!(first.note.as_ref().unwrap().release_velocity, 37);
        assert!(events
            .iter()
            .any(|e| matches!(e.fx[0],Some(Fx::Tempo{tempo}) if tempo.bpm()==100.0)));
        assert_eq!(imported.song.meters[0].meter, Meter::new(3, 4).unwrap());
    }
    #[test]
    fn owned_attacks_close_at_end_of_track_and_zero_note_on_is_release() {
        let bytes = file(vec![
            (
                0,
                midi(
                    0,
                    MidiMessage::NoteOn {
                        key: 60.into(),
                        vel: 90.into(),
                    },
                ),
            ),
            (
                0,
                midi(
                    1,
                    MidiMessage::NoteOn {
                        key: 64.into(),
                        vel: 70.into(),
                    },
                ),
            ),
            (
                48,
                midi(
                    0,
                    MidiMessage::NoteOn {
                        key: 60.into(),
                        vel: 0.into(),
                    },
                ),
            ),
            (48, TrackEventKind::Meta(MetaMessage::EndOfTrack)),
        ]);
        let imported = parse(&bytes, "Hanging note").unwrap();
        let notes: Vec<_> = imported
            .song
            .phrases
            .values()
            .flat_map(|p| &p.events)
            .filter_map(|e| e.note.as_ref())
            .collect();
        assert_eq!(notes.len(), 2);
        assert_eq!(
            notes.iter().find(|n| n.pitch == 64).unwrap().duration,
            Beat::integer(2)
        );
        assert_eq!(
            notes.iter().find(|n| n.pitch == 60).unwrap().duration,
            Beat::ONE
        );
    }
    #[test]
    fn tiny_file_with_extreme_silence_fails_before_arrangement_expansion() {
        let bytes = file(vec![
            (
                0,
                midi(
                    0,
                    MidiMessage::NoteOn {
                        key: 60.into(),
                        vel: 90.into(),
                    },
                ),
            ),
            (
                0x0fff_ffff,
                midi(
                    0,
                    MidiMessage::NoteOff {
                        key: 60.into(),
                        vel: 0.into(),
                    },
                ),
            ),
        ]);
        assert!(bytes.len() < 100);
        assert!(parse(&bytes, "Huge delta")
            .err()
            .unwrap()
            .contains("placement import budget"));
    }
    #[test]
    fn diagnostics_account_for_each_explicit_gate_repair() {
        let bytes = file(vec![
            (
                0,
                midi(
                    0,
                    MidiMessage::NoteOn {
                        key: 60.into(),
                        vel: 90.into(),
                    },
                ),
            ),
            (
                0,
                midi(
                    0,
                    MidiMessage::NoteOff {
                        key: 60.into(),
                        vel: 0.into(),
                    },
                ),
            ),
            (
                0,
                midi(
                    0,
                    MidiMessage::NoteOff {
                        key: 61.into(),
                        vel: 0.into(),
                    },
                ),
            ),
            (
                12,
                midi(
                    0,
                    MidiMessage::NoteOn {
                        key: 64.into(),
                        vel: 90.into(),
                    },
                ),
            ),
            (36, TrackEventKind::Meta(MetaMessage::EndOfTrack)),
        ]);
        let import = parse(&bytes, "Repairs").unwrap();
        assert_eq!(import.diagnostics.one_tick_gates, 1);
        assert_eq!(import.diagnostics.unmatched_releases, 1);
        assert_eq!(import.diagnostics.notes_closed_at_end, 1);
    }
    #[test]
    fn unsupported_system_data_fails_without_partial_import() {
        assert!(
            parse(&file(vec![(0, TrackEventKind::SysEx(&[1, 2, 3]))]), "SysEx")
                .err()
                .unwrap()
                .contains("System-exclusive")
        );
    }

    #[test]
    fn unrepresentable_tempo_is_rejected_instead_of_wrapping() {
        let bytes = file(vec![(
            0,
            TrackEventKind::Meta(MetaMessage::Tempo(1.into())),
        )]);
        assert!(parse(&bytes, "Extreme tempo")
            .err()
            .unwrap()
            .contains("representable range"));
    }
}
