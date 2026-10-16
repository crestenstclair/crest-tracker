use crest_tracker::composition::*;
use crest_tracker::editing::Occurrence;
use crest_tracker::playback::{
    AudiblePlayhead, Message, Playback, PlaybackBudget, PlaybackError, Snapshot,
};
use crest_tracker::time::{Beat, Division};
use std::sync::Arc;

fn beat(numerator: u64, denominator: u64) -> Beat {
    Beat::new(numerator, denominator).unwrap()
}

fn phrase(steps: u32, division: u8) -> Phrase {
    Phrase {
        steps,
        division: Division::new(u16::from(division), 1, 1).unwrap(),
        ..Phrase::default()
    }
}

fn song() -> Song {
    let mut song = Song::default();
    song.tracks.truncate(1);
    song.phrases
        .insert(PhraseId::new(1).unwrap(), phrase(2, 16));
    song.chains.insert(
        ChainId::new(1).unwrap(),
        Chain {
            name: "First".into(),
            phrases: vec![usage(11, 1, 1)],
        },
    );
    song.tracks[0].slots = vec![Some(slot(21, 1, 1))];
    song
}

fn usage(id: u64, phrase: u64, repeats: u32) -> PhraseUse {
    PhraseUse {
        id: UseId::new(id).unwrap(),
        phrase: PhraseId::new(phrase).unwrap(),
        repeats,
    }
}

fn slot(id: u64, chain: u64, repeats: u32) -> ChainUse {
    ChainUse {
        id: UseId::new(id).unwrap(),
        chain: ChainId::new(chain).unwrap(),
        repeats,
    }
}

fn snapshot(song: &Song, revision: u64, steps: usize) -> Arc<Snapshot> {
    Arc::new(
        Snapshot::compile(
            song,
            revision,
            PlaybackBudget {
                steps_per_window: steps,
                events_per_window: 128,
            },
        )
        .unwrap(),
    )
}

fn heads(playback: &Playback, at: Beat) -> Vec<AudiblePlayhead> {
    playback.playheads_at(at).unwrap().unwrap()
}

fn arrangement() -> Song {
    let mut song = song();
    song.phrases
        .insert(PhraseId::new(1).unwrap(), phrase(1, 16));
    song.phrases
        .insert(PhraseId::new(2).unwrap(), phrase(2, 16));
    song.phrases
        .insert(PhraseId::new(3).unwrap(), phrase(1, 16));
    song.chains.insert(
        ChainId::new(2).unwrap(),
        Chain {
            name: "Repeated".into(),
            phrases: vec![usage(12, 2, 2), usage(13, 3, 1)],
        },
    );
    song.tracks[0].slots.push(Some(slot(22, 2, 2)));
    song
}

#[test]
fn ten_short_chains_and_one_long_chain_keep_independent_shared_phrase_cursors_and_notes() {
    let mut song = Song::default();
    song.tracks.truncate(2);
    for (id, steps, pitch) in [(1, 10, 60), (2, 5, 72)] {
        let mut phrase = phrase(steps, 16);
        phrase.events = (0..steps)
            .map(|row| {
                Event::note(
                    EventId::new(id * 100 + u64::from(row)).unwrap(),
                    beat(u64::from(row), 4),
                    Target::Channel {
                        channel: Channel::FIRST,
                        endpoint: None,
                    },
                    Note::new(pitch + row as u8, 100, beat(1, 8)).unwrap(),
                )
            })
            .collect();
        song.phrases.insert(PhraseId::new(id).unwrap(), phrase);
    }
    song.chains.insert(
        ChainId::new(1).unwrap(),
        Chain {
            name: "10 rows".into(),
            phrases: vec![usage(11, 1, 1)],
        },
    );
    song.chains.insert(
        ChainId::new(2).unwrap(),
        Chain {
            name: "100 rows, shared phrase offset by 5".into(),
            phrases: vec![usage(12, 2, 1), usage(13, 1, 9), usage(14, 2, 1)],
        },
    );
    song.tracks[0].slots = (21..31).map(|id| Some(slot(id, 1, 1))).collect();
    song.tracks[1].slots = vec![Some(slot(31, 2, 1))];
    for track in &mut song.tracks {
        track.looped = false;
    }
    let mut playback = Playback::new(snapshot(&song, 0, 32));
    for row in 0..100_u64 {
        let at = beat(row, 4);
        let events = playback.window(beat(row + 1, 4)).unwrap();
        let heads = heads(&playback, at.checked_add(beat(1, 8)).unwrap());
        assert_eq!(heads.len(), 2);
        let (long_use, long_phrase, long_row, repeat) = match row {
            0..5 => (12, 2, row, 0),
            5..95 => (13, 1, (row - 5) % 10, (row - 5) / 10),
            _ => (14, 2, row - 95, 0),
        };
        for (track, chain_use, phrase_use, phrase, local_row, phrase_repeat) in [
            (0, 21 + row / 10, 11, 1, row % 10, 0),
            (1, 31, long_use, long_phrase, long_row, repeat),
        ] {
            let head = &heads[track];
            assert_eq!(
                head.occurrence,
                Occurrence {
                    track: song.tracks[track].id,
                    chain_use: UseId::new(chain_use).unwrap(),
                    phrase_use: UseId::new(phrase_use).unwrap(),
                }
            );
            assert_eq!(head.phrase.get(), phrase);
            assert_eq!(u64::from(head.row), local_row);
            assert_eq!(u64::from(head.phrase_repeat), phrase_repeat);
            assert_eq!(head.local_beat, beat(local_row * 2 + 1, 8));
            let lane: Vec<_> = events
                .iter()
                .filter(|event| event.owner.track == head.occurrence.track)
                .collect();
            assert_eq!(lane.len(), 2, "one attack and release per track per row");
            let pitch = if phrase == 1 { 60 } else { 72 } + local_row as u8;
            assert_eq!(lane[0].at, at);
            assert_eq!(
                lane[0].message,
                Message::NoteOn {
                    pitch,
                    velocity: 100
                }
            );
            assert_eq!(lane[1].at, at.checked_add(beat(1, 8)).unwrap());
            assert!(matches!(lane[1].message, Message::NoteOff { pitch: p, .. } if p == pitch));
            assert_eq!(lane[0].owner, lane[1].owner);
            assert_eq!(lane[0].owner.chain_use, head.occurrence.chain_use);
            assert_eq!(lane[0].owner.phrase_use, head.occurrence.phrase_use);
            assert_eq!(lane[0].owner.cycle, head.cycle);
            assert_eq!(lane[0].owner.event.get(), phrase * 100 + local_row);
        }
    }
    assert!(playback.finished());
    assert!(playback.window(beat(101, 4)).unwrap().is_empty());
    assert!(heads(&playback, Beat::integer(25)).is_empty());
}

#[test]
fn rendered_position_is_independent_of_lookahead_and_finished_cursors() {
    let mut playback = Playback::new(snapshot(&song(), 0, 32));
    assert_eq!(playback.playheads_at(Beat::ZERO).unwrap(), None);
    playback.window(beat(1, 8)).unwrap();
    assert_eq!(playback.playheads()[0].row, 1);
    let head = &heads(&playback, beat(1, 16))[0];
    assert_eq!(
        (head.row, head.local_beat, head.at),
        (0, beat(1, 16), beat(1, 16))
    );
    assert_eq!(playback.playheads_at(beat(1, 8)).unwrap(), None);

    playback.window(beat(3, 4)).unwrap();
    assert!(playback.finished());
    assert!(playback.playheads().is_empty());
    assert_eq!(heads(&playback, beat(1, 4))[0].row, 1);
    assert_eq!(heads(&playback, beat(3, 8))[0].local_beat, beat(3, 8));
    assert!(heads(&playback, beat(1, 2)).is_empty());
    assert_eq!(playback.playheads_at(beat(3, 4)).unwrap(), None);
}

#[test]
fn slot_phrase_and_chain_repeats_reset_local_time_at_exact_boundaries() {
    let mut playback = Playback::new(snapshot(&arrangement(), 9, 32));
    playback.window(Beat::integer(3)).unwrap();
    for (at, chain_use, phrase_use, row, local, cycle) in [
        (Beat::ZERO, 21, 11, 0, Beat::ZERO, 0),
        (beat(1, 4), 22, 12, 0, Beat::ZERO, 1),
        (beat(5, 8), 22, 12, 1, beat(3, 8), 1),
        (beat(3, 4), 22, 12, 0, Beat::ZERO, 2),
        (beat(5, 4), 22, 13, 0, Beat::ZERO, 3),
        (beat(3, 2), 22, 12, 0, Beat::ZERO, 4),
        (beat(5, 2), 22, 13, 0, Beat::ZERO, 6),
    ] {
        let head = &heads(&playback, at)[0];
        assert_eq!(head.occurrence.chain_use.get(), chain_use);
        assert_eq!(head.occurrence.phrase_use.get(), phrase_use);
        assert_eq!((head.row, head.local_beat, head.cycle), (row, local, cycle));
        assert_eq!(head.revision, 9);
    }
    assert!(heads(&playback, beat(11, 4)).is_empty());
    for (at, phrase_repeat, chain_repeat) in [
        (Beat::ZERO, 0, 0),
        (beat(1, 4), 0, 0),
        (beat(3, 4), 1, 0),
        (beat(5, 4), 0, 0),
        (beat(3, 2), 0, 1),
        (Beat::integer(2), 1, 1),
        (beat(5, 2), 0, 1),
    ] {
        let head = &heads(&playback, at)[0];
        assert_eq!(
            (head.phrase_repeat, head.chain_repeat),
            (phrase_repeat, chain_repeat)
        );
    }
}

#[test]
fn looping_nonzero_slot_and_phrase_keeps_independent_lane_positions() {
    let mut song = arrangement();
    song.phrases.insert(PhraseId::new(4).unwrap(), phrase(3, 8));
    song.chains.insert(
        ChainId::new(3).unwrap(),
        Chain {
            name: "Independent".into(),
            phrases: vec![usage(14, 4, 1)],
        },
    );
    song.tracks.push(Track {
        id: TrackId::new(2).unwrap(),
        name: "Second".into(),
        slots: vec![Some(slot(23, 3, 1))],
        looped: true,
        muted: false,
    });
    let path = Occurrence {
        track: song.tracks[0].id,
        chain_use: UseId::new(22).unwrap(),
        phrase_use: UseId::new(13).unwrap(),
    };
    let mut playback = Playback::new_looping(snapshot(&song, 0, 32), path).unwrap();
    playback.window(beat(7, 4)).unwrap();
    let at = heads(&playback, beat(5, 8));
    assert_eq!(at[0].occurrence, path);
    assert_eq!(
        (at[0].row, at[0].local_beat, at[0].cycle),
        (0, beat(1, 8), 2)
    );
    assert_eq!((at[1].row, at[1].local_beat), (1, beat(5, 8)));
    let boundary = heads(&playback, beat(3, 2));
    assert_eq!(boundary[0].cycle, 6);
    assert_eq!(
        (boundary[1].row, boundary[1].local_beat, boundary[1].cycle),
        (0, Beat::ZERO, 1)
    );
}

#[test]
fn jumps_report_executed_rows_instead_of_elapsed_time_modulo_phrase_length() {
    let mut song = song();
    let phrase = song.phrases.get_mut(&PhraseId::new(1).unwrap()).unwrap();
    phrase.steps = 4;
    for (id, row, destination) in [(1, 0, 2), (2, 2, 1)] {
        let mut event = Event::note(
            EventId::new(id).unwrap(),
            beat(row, 4),
            Target::Channel {
                channel: Channel::FIRST,
                endpoint: None,
            },
            Note::new(60, 100, beat(1, 8)).unwrap(),
        );
        event.fx[0] = Some(Fx::Jump { step: destination });
        phrase.events.push(event);
    }
    let mut playback = Playback::new(snapshot(&song, 0, 32));
    playback.window(Beat::ONE).unwrap();
    for (at, row, local, cycle) in [
        (beat(1, 8), 0, beat(1, 8), 0),
        (beat(1, 4), 2, beat(1, 2), 1),
        (beat(3, 8), 2, beat(5, 8), 1),
        (beat(1, 2), 1, beat(1, 4), 2),
        (beat(3, 4), 2, beat(1, 2), 2),
    ] {
        let head = &heads(&playback, at)[0];
        assert_eq!((head.row, head.local_beat, head.cycle), (row, local, cycle));
    }
}

#[test]
fn adoption_and_mute_leave_already_delivered_positions_on_their_original_revision() {
    let mut song = song();
    song.tracks[0].looped = true;
    let mut playback = Playback::new(snapshot(&song, 0, 32));
    playback.window(beat(1, 8)).unwrap();
    song.phrases.insert(PhraseId::new(1).unwrap(), phrase(4, 8));
    playback.adopt_at_phrase_boundary(snapshot(&song, 1, 32));
    playback.window(beat(3, 4)).unwrap();
    let before = &heads(&playback, beat(3, 8))[0];
    assert_eq!(
        (before.row, before.local_beat, before.revision),
        (1, beat(3, 8), 0)
    );
    let after = &heads(&playback, beat(5, 8))[0];
    assert_eq!(
        (after.row, after.local_beat, after.revision),
        (0, beat(1, 8), 1)
    );
    song.tracks[0].muted = true;
    playback.adopt(snapshot(&song, 2, 32));
    playback.window(beat(5, 4)).unwrap();
    assert_eq!(heads(&playback, beat(7, 8))[0].revision, 1);
    assert!(heads(&playback, Beat::ONE).is_empty());
    assert_eq!(heads(&playback, beat(3, 8))[0], *before);
}

#[test]
fn resumed_stopped_lane_does_not_resurrect_the_previous_final_row() {
    let mut song = song();
    song.phrases
        .insert(PhraseId::new(1).unwrap(), phrase(1, 16));
    let mut playback = Playback::new(snapshot(&song, 0, 32));
    playback.window(beat(1, 8)).unwrap();
    assert!(playback.finished());
    song.phrases
        .insert(PhraseId::new(2).unwrap(), phrase(1, 16));
    song.chains.insert(
        ChainId::new(2).unwrap(),
        Chain {
            name: "Appended".into(),
            phrases: vec![usage(12, 2, 1)],
        },
    );
    song.tracks[0].slots.push(Some(slot(22, 2, 1)));
    playback.adopt(snapshot(&song, 1, 32));
    playback.window(Beat::ONE).unwrap();
    assert_eq!(heads(&playback, beat(1, 16))[0].phrase.get(), 1);
    assert_eq!(heads(&playback, beat(1, 8))[0].phrase.get(), 2);
    assert!(heads(&playback, beat(3, 8)).is_empty());
}

#[test]
fn failed_windows_publish_neither_partial_history_nor_a_new_horizon() {
    let mut song = song();
    song.tracks[0].looped = true;
    let mut playback = Playback::new(snapshot(&song, 0, 2));
    playback.window(beat(1, 4)).unwrap();
    let before = heads(&playback, beat(1, 8));
    assert!(matches!(
        playback.window(beat(5, 4)),
        Err(PlaybackError::Budget(_))
    ));
    assert_eq!(playback.horizon(), beat(1, 4));
    assert_eq!(heads(&playback, beat(1, 8)), before);
    assert_eq!(playback.playheads_at(beat(1, 2)).unwrap(), None);
    playback.window(beat(3, 4)).unwrap();
    assert_eq!(heads(&playback, beat(3, 8))[0].row, 1);
}

#[test]
fn bounded_history_keeps_slow_crossing_lanes_and_survives_empty_windows() {
    let mut song = song();
    song.phrases.insert(PhraseId::new(1).unwrap(), phrase(1, 4));
    song.tracks[0].looped = true;
    song.phrases
        .insert(PhraseId::new(2).unwrap(), phrase(1, 64));
    song.chains.insert(
        ChainId::new(2).unwrap(),
        Chain {
            name: "Fast".into(),
            phrases: vec![usage(12, 2, 1)],
        },
    );
    song.tracks.push(Track {
        id: TrackId::new(2).unwrap(),
        name: "Fast".into(),
        slots: vec![Some(slot(22, 2, 1))],
        looped: true,
        muted: false,
    });
    let mut playback = Playback::new(snapshot(&song, 0, 2));
    for step in 1..=11 {
        playback.window(beat(step, 16)).unwrap();
    }
    assert_eq!(playback.playheads_at(Beat::ZERO).unwrap(), None);
    let at = beat(19, 32);
    let before = heads(&playback, at);
    assert_eq!(before.len(), 2);
    assert_eq!((before[0].row, before[0].local_beat), (0, at));
    assert_eq!((before[1].row, before[1].local_beat), (0, beat(1, 32)));
    for _ in 0..100 {
        playback.window(beat(11, 16)).unwrap();
    }
    assert_eq!(heads(&playback, at), before);
    assert_eq!(heads(&playback, beat(7, 16)).len(), 2);
}

#[test]
fn discarding_consumed_history_preserves_crossing_lanes_events_and_future_queries() {
    let mut song = arrangement();
    song.tracks[0].looped = true;
    let mut slow = phrase(4, 4);
    slow.events.push(Event::note(
        EventId::new(80).unwrap(),
        Beat::ZERO,
        Target::Channel {
            channel: Channel::FIRST,
            endpoint: None,
        },
        Note::new(60, 100, Beat::integer(3)).unwrap(),
    ));
    song.phrases.insert(PhraseId::new(4).unwrap(), slow);
    song.chains.insert(
        ChainId::new(4).unwrap(),
        Chain {
            name: "Slow lane".into(),
            phrases: vec![usage(14, 4, 1)],
        },
    );
    song.tracks.push(Track {
        id: TrackId::new(2).unwrap(),
        name: "Slow".into(),
        slots: vec![Some(slot(24, 4, 1))],
        looped: true,
        muted: false,
    });
    let mut complete = Playback::new(snapshot(&song, 0, 1024));
    let mut compact = complete.clone();
    for frame in 1..=512 {
        let end = beat(frame, 16);
        assert_eq!(compact.window(end).unwrap(), complete.window(end).unwrap());
        let audible = beat(frame - 1, 16);
        compact.discard_playheads_before(audible);
        assert_eq!(heads(&compact, audible), heads(&complete, audible));
        let within = audible.checked_add(beat(1, 32)).unwrap();
        assert_eq!(heads(&compact, within), heads(&complete, within));
        if audible > Beat::ZERO {
            assert_eq!(compact.playheads_at(Beat::ZERO).unwrap(), None);
        }
    }
    // A future cutoff is clamped, and an older acknowledgement cannot restore
    // discarded history or damage the next loop's delivery.
    compact.discard_playheads_before(Beat::integer(10_000));
    compact.discard_playheads_before(Beat::ZERO);
    assert_eq!(
        compact.window(Beat::integer(33)).unwrap(),
        complete.window(Beat::integer(33)).unwrap()
    );
    assert_eq!(
        heads(&compact, Beat::integer(32)),
        heads(&complete, Beat::integer(32))
    );
}
