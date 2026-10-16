use super::*;
use crest_tracker::playback::{Message, Playback, PlaybackBudget, Snapshot};
use std::sync::Arc;

#[test]
fn complete_score_preserves_audible_notes_and_exact_times_through_playback() {
    let imported = import().unwrap();
    let song = &imported.song;
    assert_eq!(imported.parts.len(), 21);
    assert_eq!(song.tracks.len(), 16);
    assert_eq!(song.tempo.milli_bpm(), 110_000);
    let mut expected_releases = Vec::new();
    for track in &song.tracks {
        assert_eq!(
            track.slots.len(),
            29,
            "the Song grid must expose the arrangement"
        );
        for (row, slot) in track.slots.iter().enumerate() {
            let chain = &song.chains[&slot.as_ref().unwrap().chain];
            assert_eq!(chain.phrases.len(), 1);
            let phrase = &song.phrases[&chain.phrases[0].phrase];
            assert_eq!(
                phrase.length().unwrap(),
                Beat::integer(if row == 28 { 4 } else { 16 })
            );
            for event in &phrase.events {
                if let Some(note) = &event.note {
                    expected_releases.push((
                        Beat::integer(row as u64 * 16)
                            .checked_add(event.at)
                            .unwrap()
                            .checked_add(note.duration)
                            .unwrap(),
                        event.target.channel().get(),
                        note.pitch,
                        note.release_velocity,
                    ));
                }
            }
        }
    }
    let source = Smf::parse(MIDI).unwrap();
    let Timing::Metrical(ppq) = source.header.timing else {
        panic!("metrical fixture")
    };
    let mut expected = Vec::new();
    for track in &source.tracks {
        let mut tick = 0_u64;
        for event in track {
            tick += u64::from(event.delta.as_int());
            if let TrackEventKind::Midi {
                channel,
                message: MidiMessage::NoteOn { key, vel },
            } = event.kind
            {
                if vel.as_int() > 0 {
                    expected.push((
                        Beat::new(tick, u64::from(ppq.as_int())).unwrap(),
                        channel.as_int() + 1,
                        key.as_int(),
                        vel.as_int(),
                    ));
                }
            }
        }
    }
    let source_attacks = expected.len();
    let imported_attacks = song
        .phrases
        .values()
        .flat_map(|phrase| &phrase.events)
        .filter(|event| event.note.is_some())
        .count();
    assert_eq!(imported_attacks, source_attacks);
    let decoded: Song = serde_json::from_slice(&serde_json::to_vec(song).unwrap()).unwrap();
    assert_eq!(&decoded, song);
    let snapshot = Snapshot::compile(song, 0, PlaybackBudget::default()).unwrap();
    let mut playback = Playback::new(Arc::new(snapshot));
    let mut actual = Vec::new();
    let mut releases = Vec::new();
    for beat in 1..=460 {
        for event in playback.window(Beat::integer(beat)).unwrap() {
            match event.message {
                Message::NoteOn { pitch, velocity } => {
                    actual.push((event.at, event.target.channel().get(), pitch, velocity))
                }
                Message::NoteOff { pitch, velocity } => {
                    releases.push((event.at, event.target.channel().get(), pitch, velocity))
                }
                _ => {}
            }
        }
    }
    assert!(playback.finished());
    assert_eq!(releases.len(), source_attacks);
    releases.sort();
    expected_releases.sort();
    assert_eq!(
        releases, expected_releases,
        "Song-row boundaries must not cut note tails"
    );
    actual.sort();
    expected.sort();
    assert_eq!(
        actual, expected,
        "Song-row boundaries must preserve source attacks exactly"
    );
    eprintln!(
        "Imported {source_attacks} notes, {} parts, {} phrases",
        imported.parts.len(),
        song.phrases.len()
    );
}

#[test]
fn invalid_midi_and_unknown_assignments_are_rejected() {
    assert!(parse(b"not MIDI").is_err());
    assert!(preset("Unknown", 0, None).is_err());
    assert!(preset("Unknown kit", 9, Some(118)).is_err());
}

#[test]
fn overlapping_notes_silent_placeholders_and_same_tick_gates_preserve_ownership() {
    use midly::{Format, Header, TrackEvent};
    let midi = |delta: u32, message| TrackEvent {
        delta: delta.into(),
        kind: TrackEventKind::Midi {
            channel: 0.into(),
            message,
        },
    };
    let on = |velocity: u8| MidiMessage::NoteOn {
        key: 60.into(),
        vel: velocity.into(),
    };
    let off = |velocity: u8| MidiMessage::NoteOff {
        key: 60.into(),
        vel: velocity.into(),
    };
    let smf = Smf {
        header: Header::new(Format::SingleTrack, Timing::Metrical(384.into())),
        tracks: vec![vec![
            TrackEvent {
                delta: 0.into(),
                kind: TrackEventKind::Meta(MetaMessage::TrackName(b"Test")),
            },
            TrackEvent {
                delta: 0.into(),
                kind: TrackEventKind::Meta(MetaMessage::Tempo(500000.into())),
            },
            midi(0, MidiMessage::ProgramChange { program: 13.into() }),
            midi(0, on(0)), // silent placeholder
            midi(1, on(100)),
            midi(1, on(90)), // overlap: the first release belongs to the first attack
            midi(1, off(20)),
            midi(1, on(0)), // velocity-zero release belongs to the second attack
            midi(1, on(80)),
            midi(0, off(30)), // same-tick gate normalized to one source tick
        ]],
    };
    let mut bytes = Vec::new();
    smf.write_std(&mut bytes).unwrap();
    let imported = parse(&bytes).unwrap();
    let notes: Vec<_> = imported
        .song
        .phrases
        .values()
        .flat_map(|p| &p.events)
        .filter_map(|event| {
            event.note.as_ref().map(|note| {
                (
                    event.at,
                    note.duration,
                    note.velocity,
                    note.release_velocity,
                )
            })
        })
        .collect();
    assert_eq!(
        notes,
        vec![
            (
                Beat::new(1, 384).unwrap(),
                Beat::new(2, 384).unwrap(),
                100,
                20
            ),
            (
                Beat::new(2, 384).unwrap(),
                Beat::new(2, 384).unwrap(),
                90,
                0
            ),
            (
                Beat::new(5, 384).unwrap(),
                Beat::new(1, 384).unwrap(),
                80,
                30
            ),
        ]
    );
    let mut truncated = smf;
    truncated.tracks[0].pop();
    let mut bytes = Vec::new();
    truncated.write_std(&mut bytes).unwrap();
    let ended = parse(&bytes).unwrap();
    assert_eq!(
        ended
            .song
            .phrases
            .values()
            .flat_map(|phrase| &phrase.events)
            .filter(|event| event.note.is_some())
            .count(),
        3
    );
}
