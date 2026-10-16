use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crate::synth::sound_font_instrument::SoundFontInstrument;
use crate::testing::instrument_part::InstrumentPart;
use crate::testing::midi_event_source::{FixedEventBatch, MidiEventSource, MidiSourceError};
use midly::{
    Format, Fps, MetaMessage, MidiMessage as MidlyMidiMessage, Smf, Timing, TrackEventKind,
};
use std::fs;
use std::time::Duration;

/// The only MIDI file used by the automatic input path.
pub const FIXED_MIDI_PATH: &str =
    "./midi/Radiohead - Everything In Its Right Place - HiDef Compatible.mid";

const DEFAULT_MICROSECONDS_PER_BEAT: u64 = 500_000;
const BANK_SELECT_MSB: u8 = 0;
const BANK_SELECT_LSB: u8 = 32;
const ALL_SOUND_OFF: u8 = 120;
const ALL_NOTES_OFF: u8 = 123;
const PERCUSSION_CHANNEL: u8 = 9;
const MIDI_CHANNEL_COUNT: usize = 16;

/// Automatic, run-once Standard MIDI File input for the fixed production fixture.
#[derive(Debug, Default)]
pub struct FixedMidiEventSource {
    events: Vec<ScheduledMidiEvent>,
    cursor: usize,
    elapsed: Duration,
    prepared: bool,
    started: bool,
}

impl FixedMidiEventSource {
    /// Creates an unprepared source bound to the fixed fixture path.
    pub const fn new() -> Self {
        Self {
            events: Vec::new(),
            cursor: 0,
            elapsed: Duration::ZERO,
            prepared: false,
            started: false,
        }
    }
}

impl MidiEventSource for FixedMidiEventSource {
    fn prepare(&mut self) -> Result<Vec<InstrumentPart>, MidiSourceError> {
        self.events.clear();
        self.cursor = 0;
        self.elapsed = Duration::ZERO;
        self.prepared = false;
        self.started = false;

        let bytes = super::bundled_resource::path(FIXED_MIDI_PATH)
            .and_then(fs::read)
            .map_err(|error| {
                MidiSourceError::new(format!(
                    "failed to read fixed MIDI fixture {FIXED_MIDI_PATH}: {error}"
                ))
            })?;
        let smf = Smf::parse(&bytes).map_err(|error| {
            MidiSourceError::new(format!(
                "fixed MIDI fixture {FIXED_MIDI_PATH} is malformed: {error}"
            ))
        })?;
        let (parts, events) = prepare_smf(&smf)?;

        self.events = events;
        self.prepared = true;
        Ok(parts)
    }

    fn start(&mut self) {
        self.cursor = 0;
        self.elapsed = Duration::ZERO;
        self.started = self.prepared;
    }

    fn poll(
        &mut self,
        elapsed: Duration,
        output: &mut FixedEventBatch,
    ) -> Result<(), MidiSourceError> {
        if !self.started {
            return Err(MidiSourceError::new(
                "fixed MIDI fixture must be prepared and started before polling",
            ));
        }

        self.elapsed = self.elapsed.saturating_add(elapsed);
        while let Some(event) = self.events.get(self.cursor) {
            if event.due > self.elapsed || output.is_full() {
                break;
            }

            output.try_push(event.event)?;
            self.cursor += 1;
        }
        Ok(())
    }

    fn finished(&self) -> bool {
        self.started && self.cursor == self.events.len()
    }
}

#[derive(Clone, Copy, Debug)]
struct ScheduledMidiEvent {
    due: Duration,
    event: MidiMessage,
}

#[derive(Clone, Copy, Debug)]
struct TimedMidiEvent {
    due: Duration,
    track_index: usize,
    source_channel: u8,
    message: MidlyMidiMessage,
}

#[derive(Clone, Copy, Debug)]
struct RawEvent {
    tick: u64,
    order: u64,
    kind: RawEventKind,
}

#[derive(Clone, Copy, Debug)]
enum RawEventKind {
    Tempo(u32),
    Midi {
        track_index: usize,
        source_channel: u8,
        message: MidlyMidiMessage,
    },
}

#[derive(Clone, Copy, Debug, Default)]
struct ChannelProgram {
    bank_msb: u8,
    bank_lsb: u8,
    program: u8,
}

impl ChannelProgram {
    const fn identity(self, source_channel: u8) -> InstrumentIdentity {
        InstrumentIdentity {
            bank: (self.bank_msb as u16) << 7 | self.bank_lsb as u16,
            program: self.program,
            percussion: source_channel == PERCUSSION_CHANNEL,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InstrumentIdentity {
    bank: u16,
    program: u8,
    percussion: bool,
}

fn prepare_smf(
    smf: &Smf<'_>,
) -> Result<(Vec<InstrumentPart>, Vec<ScheduledMidiEvent>), MidiSourceError> {
    let (raw_events, track_names) = collect_raw_events(smf)?;
    let timed_events = apply_timing(&raw_events, smf.header.timing)?;
    build_targeted_events(&timed_events, &track_names)
}

fn collect_raw_events(smf: &Smf<'_>) -> Result<(Vec<RawEvent>, Vec<String>), MidiSourceError> {
    let mut events = Vec::new();
    let mut track_names = vec![String::new(); smf.tracks.len()];
    let mut sequential_base = 0_u64;
    let mut order = 0_u64;

    for (track_index, track) in smf.tracks.iter().enumerate() {
        let mut tick = if smf.header.format == Format::Sequential {
            sequential_base
        } else {
            0
        };

        for event in track {
            tick = tick
                .checked_add(u64::from(event.delta.as_int()))
                .ok_or_else(|| MidiSourceError::new("MIDI fixture tick position overflowed"))?;

            match event.kind {
                TrackEventKind::Midi { channel, message } => {
                    events.push(RawEvent {
                        tick,
                        order,
                        kind: RawEventKind::Midi {
                            track_index,
                            source_channel: channel.as_int(),
                            message,
                        },
                    });
                }
                TrackEventKind::Meta(MetaMessage::Tempo(microseconds_per_beat)) => {
                    events.push(RawEvent {
                        tick,
                        order,
                        kind: RawEventKind::Tempo(microseconds_per_beat.as_int()),
                    });
                }
                TrackEventKind::Meta(MetaMessage::TrackName(name))
                    if track_names[track_index].is_empty() =>
                {
                    track_names[track_index] = String::from_utf8_lossy(name).trim().to_owned();
                }
                _ => {}
            }
            order = order
                .checked_add(1)
                .ok_or_else(|| MidiSourceError::new("MIDI fixture event ordering overflowed"))?;
        }

        if smf.header.format == Format::Sequential {
            sequential_base = tick;
        }
    }

    events.sort_by_key(|event| (event.tick, event.order));
    Ok((events, track_names))
}

fn apply_timing(
    raw_events: &[RawEvent],
    timing: Timing,
) -> Result<Vec<TimedMidiEvent>, MidiSourceError> {
    match timing {
        Timing::Metrical(ticks_per_beat) => {
            apply_metrical_timing(raw_events, u64::from(ticks_per_beat.as_int()))
        }
        Timing::Timecode(frames_per_second, ticks_per_frame) => {
            apply_timecode_timing(raw_events, frames_per_second, ticks_per_frame)
        }
    }
}

fn apply_metrical_timing(
    raw_events: &[RawEvent],
    ticks_per_beat: u64,
) -> Result<Vec<TimedMidiEvent>, MidiSourceError> {
    if ticks_per_beat == 0 {
        return Err(MidiSourceError::new(
            "fixed MIDI fixture declares zero ticks per beat",
        ));
    }

    let mut events = Vec::new();
    let mut last_tick = 0_u64;
    let mut microseconds_numerator = 0_u128;
    let mut microseconds_per_beat = DEFAULT_MICROSECONDS_PER_BEAT;

    for raw_event in raw_events {
        let delta_ticks = raw_event.tick.checked_sub(last_tick).ok_or_else(|| {
            MidiSourceError::new("fixed MIDI fixture events are not time ordered")
        })?;
        let delta_numerator = u128::from(delta_ticks)
            .checked_mul(u128::from(microseconds_per_beat))
            .ok_or_else(|| MidiSourceError::new("MIDI fixture duration overflowed"))?;
        microseconds_numerator = microseconds_numerator
            .checked_add(delta_numerator)
            .ok_or_else(|| MidiSourceError::new("MIDI fixture duration overflowed"))?;
        last_tick = raw_event.tick;

        match raw_event.kind {
            RawEventKind::Tempo(tempo) => {
                if tempo == 0 {
                    return Err(MidiSourceError::new(
                        "fixed MIDI fixture declares a zero microsecond tempo",
                    ));
                }
                microseconds_per_beat = u64::from(tempo);
            }
            RawEventKind::Midi {
                track_index,
                source_channel,
                message,
            } => events.push(TimedMidiEvent {
                due: duration_from_micros(microseconds_numerator / u128::from(ticks_per_beat))?,
                track_index,
                source_channel,
                message,
            }),
        }
    }

    Ok(events)
}

fn apply_timecode_timing(
    raw_events: &[RawEvent],
    frames_per_second: Fps,
    ticks_per_frame: u8,
) -> Result<Vec<TimedMidiEvent>, MidiSourceError> {
    if ticks_per_frame == 0 {
        return Err(MidiSourceError::new(
            "fixed MIDI fixture declares zero ticks per timecode frame",
        ));
    }

    let (frames_numerator, frames_denominator) = match frames_per_second {
        Fps::Fps24 => (24_u128, 1_u128),
        Fps::Fps25 => (25_u128, 1_u128),
        Fps::Fps29 => (30_000_u128, 1_001_u128),
        Fps::Fps30 => (30_u128, 1_u128),
    };
    let denominator = frames_numerator
        .checked_mul(u128::from(ticks_per_frame))
        .ok_or_else(|| MidiSourceError::new("MIDI timecode denominator overflowed"))?;
    let mut events = Vec::new();

    for raw_event in raw_events {
        if let RawEventKind::Midi {
            track_index,
            source_channel,
            message,
        } = raw_event.kind
        {
            let nanoseconds = u128::from(raw_event.tick)
                .checked_mul(frames_denominator)
                .and_then(|value| value.checked_mul(1_000_000_000))
                .ok_or_else(|| MidiSourceError::new("MIDI fixture duration overflowed"))?
                / denominator;
            events.push(TimedMidiEvent {
                due: duration_from_nanos(nanoseconds)?,
                track_index,
                source_channel,
                message,
            });
        }
    }

    Ok(events)
}

fn duration_from_micros(microseconds: u128) -> Result<Duration, MidiSourceError> {
    let microseconds = u64::try_from(microseconds)
        .map_err(|_| MidiSourceError::new("MIDI fixture duration exceeds supported range"))?;
    Ok(Duration::from_micros(microseconds))
}

fn duration_from_nanos(nanoseconds: u128) -> Result<Duration, MidiSourceError> {
    let nanoseconds = u64::try_from(nanoseconds)
        .map_err(|_| MidiSourceError::new("MIDI fixture duration exceeds supported range"))?;
    Ok(Duration::from_nanos(nanoseconds))
}

fn build_targeted_events(
    timed_events: &[TimedMidiEvent],
    track_names: &[String],
) -> Result<(Vec<InstrumentPart>, Vec<ScheduledMidiEvent>), MidiSourceError> {
    let mut channel_programs = [ChannelProgram::default(); 16];
    let mut active_notes = [[None; 128]; 16];
    let mut identities = Vec::new();
    let mut parts = Vec::new();
    let mut events = Vec::new();

    for timed_event in timed_events {
        let channel_index = usize::from(timed_event.source_channel);
        let state = channel_programs[channel_index];

        match timed_event.message {
            MidlyMidiMessage::NoteOn { key, vel } if vel.as_int() != 0 => {
                let identity = state.identity(timed_event.source_channel);
                let part_index = find_or_create_part(
                    identity,
                    timed_event.track_index,
                    track_names,
                    &mut identities,
                    &mut parts,
                )?;
                active_notes[channel_index][usize::from(key.as_int())] = Some(part_index);
                push_message(
                    &mut events,
                    timed_event.due,
                    &parts,
                    part_index,
                    MidiMessageKind::NoteOn,
                    key.as_int(),
                    vel.as_int(),
                )?;
            }
            MidlyMidiMessage::NoteOn { key, .. } => {
                push_note_off(
                    &mut events,
                    timed_event.due,
                    channel_index,
                    key.as_int(),
                    0,
                    state,
                    &identities,
                    &parts,
                    &mut active_notes,
                )?;
            }
            MidlyMidiMessage::NoteOff { key, vel } => {
                push_note_off(
                    &mut events,
                    timed_event.due,
                    channel_index,
                    key.as_int(),
                    vel.as_int(),
                    state,
                    &identities,
                    &parts,
                    &mut active_notes,
                )?;
            }
            MidlyMidiMessage::Controller { controller, value } => {
                let controller = controller.as_int();
                if controller == BANK_SELECT_MSB {
                    channel_programs[channel_index].bank_msb = value.as_int();
                } else if controller == BANK_SELECT_LSB {
                    channel_programs[channel_index].bank_lsb = value.as_int();
                } else if controller == ALL_SOUND_OFF || controller == ALL_NOTES_OFF {
                    push_all_notes_off(
                        &mut events,
                        timed_event.due,
                        channel_index,
                        &parts,
                        &mut active_notes,
                    );
                } else if let Some(part_index) =
                    find_part(state.identity(timed_event.source_channel), &identities)
                {
                    push_message(
                        &mut events,
                        timed_event.due,
                        &parts,
                        part_index,
                        MidiMessageKind::ControlChange,
                        controller,
                        value.as_int(),
                    )?;
                }
            }
            MidlyMidiMessage::ProgramChange { program } => {
                channel_programs[channel_index].program = program.as_int();
            }
            MidlyMidiMessage::ChannelAftertouch { vel } => {
                if let Some(part_index) =
                    find_part(state.identity(timed_event.source_channel), &identities)
                {
                    push_message(
                        &mut events,
                        timed_event.due,
                        &parts,
                        part_index,
                        MidiMessageKind::ChannelPressure,
                        vel.as_int(),
                        0,
                    )?;
                }
            }
            MidlyMidiMessage::PitchBend { bend } => {
                if let Some(part_index) =
                    find_part(state.identity(timed_event.source_channel), &identities)
                {
                    let raw = bend.0.as_int();
                    push_message(
                        &mut events,
                        timed_event.due,
                        &parts,
                        part_index,
                        MidiMessageKind::PitchBend,
                        (raw & 0x7f) as u8,
                        (raw >> 7) as u8,
                    )?;
                }
            }
            MidlyMidiMessage::Aftertouch { .. } => {}
        }
    }

    Ok((parts, events))
}

fn find_or_create_part(
    identity: InstrumentIdentity,
    track_index: usize,
    track_names: &[String],
    identities: &mut Vec<InstrumentIdentity>,
    parts: &mut Vec<InstrumentPart>,
) -> Result<usize, MidiSourceError> {
    if let Some(part_index) = find_part(identity, identities) {
        return Ok(part_index);
    }

    let part_index = parts.len();
    if part_index >= MIDI_CHANNEL_COUNT {
        return Err(MidiSourceError::new(format!(
            "fixed MIDI fixture has more than {MIDI_CHANNEL_COUNT} sounding instrument identities; cannot assign a unique MIDI channel"
        )));
    }
    let instrument = SoundFontInstrument::new(identity.bank, identity.program, identity.percussion)
        .map_err(|error| {
            MidiSourceError::new(format!(
                "fixed MIDI fixture contains an invalid instrument: {error}"
            ))
        })?;
    let base_name = track_names
        .get(track_index)
        .map(String::as_str)
        .filter(|name| !name.is_empty())
        .unwrap_or("Instrument");
    let name = format!(
        "{base_name} {} (bank {}, program {}, percussion {})",
        part_index + 1,
        identity.bank,
        identity.program,
        identity.percussion
    );

    identities.push(identity);
    parts.push(InstrumentPart::new(part_index, name, instrument));
    Ok(part_index)
}

fn find_part(identity: InstrumentIdentity, identities: &[InstrumentIdentity]) -> Option<usize> {
    identities
        .iter()
        .position(|candidate| *candidate == identity)
}

#[allow(clippy::too_many_arguments)]
fn push_note_off(
    events: &mut Vec<ScheduledMidiEvent>,
    due: Duration,
    channel_index: usize,
    key: u8,
    velocity: u8,
    state: ChannelProgram,
    identities: &[InstrumentIdentity],
    parts: &[InstrumentPart],
    active_notes: &mut [[Option<usize>; 128]; 16],
) -> Result<(), MidiSourceError> {
    let part_index = active_notes[channel_index][usize::from(key)]
        .take()
        .or_else(|| find_part(state.identity(channel_index as u8), identities));

    if let Some(part_index) = part_index {
        push_message(
            events,
            due,
            parts,
            part_index,
            MidiMessageKind::NoteOff,
            key,
            velocity,
        )?;
    }
    Ok(())
}

fn push_all_notes_off(
    events: &mut Vec<ScheduledMidiEvent>,
    due: Duration,
    channel_index: usize,
    parts: &[InstrumentPart],
    active_notes: &mut [[Option<usize>; 128]; 16],
) {
    let mut targeted_parts = Vec::new();
    for active_part in &mut active_notes[channel_index] {
        if let Some(part_index) = active_part.take() {
            if !targeted_parts.contains(&part_index) {
                targeted_parts.push(part_index);
            }
        }
    }

    for part_index in targeted_parts {
        events.push(ScheduledMidiEvent {
            due,
            event: MidiMessage::all_notes_off(parts[part_index].assigned_channel()),
        });
    }
}

fn push_message(
    events: &mut Vec<ScheduledMidiEvent>,
    due: Duration,
    parts: &[InstrumentPart],
    part_index: usize,
    kind: MidiMessageKind,
    data1: u8,
    data2: u8,
) -> Result<(), MidiSourceError> {
    let message = MidiMessage::try_new(parts[part_index].assigned_channel(), kind, data1, data2)
        .map_err(|error| {
            MidiSourceError::new(format!(
                "fixed MIDI fixture produced an invalid normalized message: {error}"
            ))
        })?;
    events.push(ScheduledMidiEvent {
        due,
        event: message,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{find_or_create_part, FixedMidiEventSource, InstrumentIdentity, FIXED_MIDI_PATH};
    use crate::kernel::midi_message::MidiMessageKind;
    use crate::testing::midi_event_source::{FixedEventBatch, MidiEventSource};
    use std::collections::BTreeSet;
    use std::time::Duration;

    #[test]
    fn fixed_midi_event_source_prepares_multiple_stable_parts() {
        let mut source = FixedMidiEventSource::new();
        let parts = source.prepare().expect("real fixture should prepare");

        assert!(parts.len() > 1);
        for (index, part) in parts.iter().enumerate() {
            assert_eq!(part.index(), index);
            assert_eq!(usize::from(part.assigned_channel().value()), index);
        }
    }

    #[test]
    fn fixed_midi_event_source_rejects_channel_exhaustion() {
        let mut identities = Vec::new();
        let mut parts = Vec::new();
        let track_names = Vec::<String>::new();

        for program in 0..16_u8 {
            let part_index = find_or_create_part(
                InstrumentIdentity {
                    bank: 0,
                    program,
                    percussion: false,
                },
                0,
                &track_names,
                &mut identities,
                &mut parts,
            )
            .expect("the first sixteen identities should receive unique channels");
            assert_eq!(part_index, usize::from(program));
        }

        let error = find_or_create_part(
            InstrumentIdentity {
                bank: 0,
                program: 16,
                percussion: false,
            },
            0,
            &track_names,
            &mut identities,
            &mut parts,
        )
        .expect_err("the seventeenth identity must not reuse a MIDI channel");

        assert!(error.message().contains("more than 16"));
        assert!(error.message().contains("unique MIDI channel"));
        assert_eq!(parts.len(), 16);
    }

    #[test]
    fn fixed_midi_event_source_runs_once_and_rewrites_target_channels() {
        let mut source = FixedMidiEventSource::new();
        let parts = source.prepare().expect("real fixture should prepare");
        source.start();

        let mut output = FixedEventBatch::new();
        let mut targeted_parts = BTreeSet::new();
        let mut emitted_messages = 0_usize;
        let mut note_on_messages = 0_usize;

        for _ in 0..30_000 {
            if source.finished() {
                break;
            }

            output.clear();
            source
                .poll(Duration::from_millis(20), &mut output)
                .expect("bounded fixture polling should succeed");
            for message in output.iter() {
                let part_index = usize::from(message.channel().value());
                let part = &parts[part_index];
                assert_eq!(message.channel(), part.assigned_channel());
                targeted_parts.insert(part_index);
                emitted_messages += 1;
                if message.kind() == MidiMessageKind::NoteOn {
                    note_on_messages += 1;
                }
            }
        }

        assert!(source.finished(), "fixture should stop at its end");
        assert!(emitted_messages > 0);
        assert!(note_on_messages > 0);
        assert!(targeted_parts.len() > 1);
    }

    #[test]
    fn fixed_midi_event_source_drains_delayed_input_in_bounded_ordered_batches() {
        let mut source = FixedMidiEventSource::new();
        source.prepare().expect("real fixture should prepare");
        let expected = source
            .events
            .iter()
            .map(|event| event.event)
            .collect::<Vec<_>>();
        assert!(expected.len() > FixedEventBatch::CAPACITY);
        source.start();

        let mut output = FixedEventBatch::new();
        let mut actual = Vec::with_capacity(expected.len());
        let mut elapsed = Duration::MAX;
        let mut full_batches = 0_usize;
        while !source.finished() {
            output.clear();
            source
                .poll(elapsed, &mut output)
                .expect("elapsed-time catch-up must retain rather than overflow");
            elapsed = Duration::ZERO;
            assert!(!output.is_empty());
            assert!(output.len() <= FixedEventBatch::CAPACITY);
            if output.is_full() {
                full_batches += 1;
            }
            actual.extend(output.iter().copied());
        }

        assert!(full_batches > 0);
        assert_eq!(actual, expected);
    }

    #[test]
    fn fixed_midi_event_source_reports_malformed_data_clearly() {
        let error = midly::Smf::parse(b"not a MIDI file").unwrap_err();

        assert!(!error.to_string().is_empty());
        assert!(FIXED_MIDI_PATH
            .ends_with("Radiohead - Everything In Its Right Place - HiDef Compatible.mid"));
    }
}
