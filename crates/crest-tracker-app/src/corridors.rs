//! Import the bundled arrangement as editable musical data, independent of Synth.
//! This is a named fixture import, not a general-purpose MIDI file importer.
use crest_tracker::composition::*;
use crest_tracker::time::{Beat, Division, Tempo};
use midly::{MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};
use std::collections::{BTreeMap, VecDeque};

pub const NAME: &str = "Corridors of Time";
const MIDI: &[u8] = include_bytes!("../../crest-synth/midi/Corridors of Time - Chrono Trigger.mid");

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
}

pub fn import() -> Result<Import, String> {
    parse(MIDI).map_err(|error| format!("Corridors of Time import: {error}"))
}

fn parse(bytes: &[u8]) -> Result<Import, String> {
    let smf = Smf::parse(bytes).map_err(|e| e.to_string())?;
    let Timing::Metrical(ppq) = smf.header.timing else {
        return Err("Expected metrical MIDI timing".into());
    };
    if smf.header.format == midly::Format::Sequential || ppq.as_int() == 0 {
        return Err("Expected simultaneous tracks with positive PPQ".into());
    }
    let beat = |tick| Beat::new(tick, u64::from(ppq.as_int())).map_err(|e| e.to_string());
    let mut song = Song {
        name: NAME.into(),
        ..Song::default()
    };
    let mut parts = Vec::new();
    let mut lanes: [Vec<Event>; 16] = std::array::from_fn(|_| Vec::new());
    let mut tempo = None;
    let mut identity = 17_u64;
    let mut end = Beat::ZERO;
    for track in &smf.tracks {
        let mut tick = 0_u64;
        let mut name = String::new();
        let mut channel = None;
        let mut program = None;
        let mut active: BTreeMap<u8, VecDeque<(u64, u8)>> = BTreeMap::new();
        let mut events = Vec::new();
        let instrument = InstrumentId::new(identity).map_err(|e| e.to_string())?;
        identity += 1;
        for event in track {
            tick = tick
                .checked_add(u64::from(event.delta.as_int()))
                .ok_or("Tick overflow")?;
            match event.kind {
                TrackEventKind::Meta(MetaMessage::TrackName(value)) => {
                    name = String::from_utf8_lossy(value).trim().to_owned()
                }
                TrackEventKind::Meta(MetaMessage::Tempo(value)) => {
                    if tick != 0
                        || value.as_int() == 0
                        || tempo.is_some_and(|old| old != value.as_int())
                    {
                        return Err("Bundled arrangement must have one initial tempo".into());
                    }
                    tempo = Some(value.as_int());
                }
                TrackEventKind::Meta(MetaMessage::TimeSignature(n, d, _, _))
                    if tick != 0 || (n, d) != (4, 2) =>
                {
                    return Err("Bundled arrangement must use 4/4".into());
                }
                TrackEventKind::Midi {
                    channel: source,
                    message,
                } => {
                    let source = source.as_int();
                    if channel.replace(source).is_some_and(|old| old != source) {
                        return Err("Expected one MIDI channel per source track".into());
                    }
                    let target = Target::Instrument {
                        id: instrument,
                        channel: Channel::new(source + 1).map_err(|e| e.to_string())?,
                    };
                    match message {
                        MidiMessage::ProgramChange { program: value } if tick == 0 => {
                            program = Some(value.as_int())
                        }
                        MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => {
                            active
                                .entry(key.as_int())
                                .or_default()
                                .push_back((tick, vel.as_int()));
                        }
                        MidiMessage::NoteOff { key, vel } | MidiMessage::NoteOn { key, vel } => {
                            // Zero-velocity NoteOn is a release, never an attack. This export
                            // includes silent placeholders and their unmatched release edges.
                            if let Some((at, velocity)) =
                                active.get_mut(&key.as_int()).and_then(VecDeque::pop_front)
                            {
                                // The source contains same-tick gates. Tracker requires a
                                // positive duration: retain the attack with one source tick.
                                let finish = tick.max(at + 1);
                                let mut note =
                                    Note::new(key.as_int(), velocity, beat(finish - at)?)
                                        .map_err(|e| e.to_string())?;
                                note.release_velocity = vel.as_int();
                                events.push(Event::note(
                                    EventId::new(identity).map_err(|e| e.to_string())?,
                                    beat(at)?,
                                    target,
                                    note,
                                ));
                                identity += 1;
                                end = end.max(beat(finish)?);
                            }
                        }
                        MidiMessage::ChannelAftertouch { vel } => {
                            events.push(Event {
                                id: EventId::new(identity).map_err(|e| e.to_string())?,
                                at: beat(tick)?,
                                target,
                                note: None,
                                controls: vec![Control::Pressure {
                                    value: vel.as_int(),
                                }],
                                fx: [None, None, None],
                            });
                            identity += 1;
                        }
                        _ => {
                            return Err(format!(
                                "Unsupported event in bundled part {name}: {message:?}"
                            ))
                        }
                    }
                }
                TrackEventKind::SysEx(_) | TrackEventKind::Escape(_) => {
                    return Err("Unsupported system data in bundled arrangement".into())
                }
                _ => {}
            }
        }
        // Two notes in the bundled export lack release edges. End-of-track
        // terminates those source voices; never let them become stuck notes.
        for (pitch, notes) in active {
            for (at, velocity) in notes {
                let finish = tick.max(at + 1);
                let note =
                    Note::new(pitch, velocity, beat(finish - at)?).map_err(|e| e.to_string())?;
                events.push(Event::note(
                    EventId::new(identity).map_err(|e| e.to_string())?,
                    beat(at)?,
                    Target::Instrument {
                        id: instrument,
                        channel: Channel::new(channel.ok_or("Missing source channel")? + 1)
                            .map_err(|e| e.to_string())?,
                    },
                    note,
                ));
                identity += 1;
                end = end.max(beat(finish)?);
            }
        }
        if !events.iter().any(|event| event.note.is_some()) {
            continue;
        }
        let channel = channel.ok_or("Sounding part has no channel")?;
        let (bank, program) = preset(&name, channel, program)?;
        song.instruments.insert(
            instrument,
            InstrumentUse {
                name: name.clone(),
                resource: None,
            },
        );
        lanes[usize::from(channel)].extend(events);
        parts.push(Part {
            instrument,
            name,
            channel,
            bank,
            program,
        });
    }
    if parts.is_empty() {
        return Err("No audible notes".into());
    }
    let micros = u64::from(tempo.ok_or("Missing tempo")?);
    song.tempo = Tempo::new(((60_000_000_000_u64 + micros / 2) / micros) as u32)
        .map_err(|e| e.to_string())?;
    arrange(song, parts, lanes, identity, end)
}

pub(crate) fn arrange(
    mut song: Song,
    parts: Vec<Part>,
    lanes: [Vec<Event>; 16],
    mut identity: u64,
    end: Beat,
) -> Result<Import, String> {
    // Each MIDI channel is one arrangement lane. Source parts still have distinct
    // instrument identities, including tracks reusing a channel in this export.
    // Four-bar Song rows keep the imported arrangement visible. Each placement
    // owns a one-phrase chain; splitting references never quantizes note timing.
    let bars = end.units(Beat::integer(4)).map_err(|e| e.to_string())?
        + u64::from(!end.numerator().is_multiple_of(end.denominator() * 4));
    for (lane, mut events) in lanes.into_iter().enumerate() {
        if events.is_empty() {
            continue;
        }
        events.sort_by_key(|event| (event.at, event.id));
        let names: Vec<_> = parts
            .iter()
            .filter(|part| usize::from(part.channel) == lane)
            .map(|part| part.name.as_str())
            .collect();
        song.tracks[lane].name = names.join(" + ");
        let mut events = events.into_iter().peekable();
        for bar in (0..bars).step_by(4) {
            let at = Beat::integer(bar * 4);
            let length = (bars - bar).min(4) * 4;
            let finish = Beat::integer(bar * 4 + length);
            let mut phrase = Phrase {
                name: format!(
                    "Channel {} · bars {}–{}",
                    lane + 1,
                    bar + 1,
                    bar + length / 4
                ),
                steps: (length * 4) as u32,
                division: Division::SIXTEENTH,
                form: if lane == 9 {
                    PhraseForm::Drum
                } else {
                    PhraseForm::PianoRoll
                },
                ..Phrase::default()
            };
            while events.peek().is_some_and(|event| event.at < finish) {
                let mut event = events.next().expect("peeked event");
                event.at = event.at.checked_sub(at).map_err(|e| e.to_string())?;
                phrase.events.push(event);
            }
            let id = PhraseId::new(identity).map_err(|e| e.to_string())?;
            identity += 1;
            let name = phrase.name.clone();
            song.phrases.insert(id, phrase);
            let usage = PhraseUse {
                id: UseId::new(identity).map_err(|e| e.to_string())?,
                phrase: id,
                repeats: 1,
            };
            identity += 1;
            let chain_id = ChainId::new(identity).map_err(|e| e.to_string())?;
            identity += 1;
            song.chains.insert(
                chain_id,
                Chain {
                    name,
                    phrases: vec![usage],
                },
            );
            song.tracks[lane].slots.push(Some(ChainUse {
                id: UseId::new(identity).map_err(|e| e.to_string())?,
                chain: chain_id,
                repeats: 1,
            }));
            identity += 1;
        }
        if events.next().is_some() {
            return Err("Event exceeds imported arrangement".into());
        }
    }
    song.validate().map_err(|e| e.to_string())?;
    Ok(Import { song, parts })
}

fn preset(name: &str, channel: u8, program: Option<u8>) -> Result<(u16, u8), String> {
    // Explicit arrangement choices for non-GM/missing assignments in this file.
    // These are authored mappings, never a missing-preset fallback at playback.
    match (name, channel, program) {
        ("Electric Drum Kit", 9, Some(118)) => Ok((128, 24)),
        (_, 9, Some(0)) => Ok((128, 0)),
        ("Smooth Synth (Classic)", _, None) => Ok((0, 89)),
        ("8-Bit Sawtooth", _, None) => Ok((0, 81)),
        ("Koto", _, None) => Ok((0, 107)),
        (_, channel, Some(program)) if channel != 9 => Ok((0, program)),
        _ => Err(format!("No authored SoundFont assignment for {name}")),
    }
}

#[cfg(test)]
mod tests;
