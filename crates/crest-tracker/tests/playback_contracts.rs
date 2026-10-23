use crest_tracker::composition::*;
use crest_tracker::mpe::{BendRange, MpeProfile, MpeProvenance, MpeZone};
use crest_tracker::playback::{
    Message, Playback, PlaybackBudget, PlaybackError, Snapshot, TempoMap, TimedEvent,
};
use crest_tracker::time::{Beat, Division, Tempo};
use std::collections::BTreeSet;
use std::sync::Arc;

fn note(id: u64, at: Beat, pitch: u8, duration: Beat) -> Event {
    Event::note(
        EventId::new(id).unwrap(),
        at,
        Target::Channel {
            channel: Channel::FIRST,
            endpoint: None,
        },
        Note::new(pitch, 100, duration).unwrap(),
    )
}

fn beat(numerator: u64, denominator: u64) -> Beat {
    Beat::new(numerator, denominator).unwrap()
}

fn expression(offset: Beat, sequence: u64, control: Control) -> Expression {
    Expression {
        offset,
        sequence: Some(sequence),
        control,
    }
}

fn mpe_note(id: u64, at: Beat, duration: Beat, lifetime: Beat) -> Event {
    let mut event = note(id, at, 60, duration);
    let note = event.note.as_mut().unwrap();
    note.expression_duration = Some(lifetime);
    note.mpe = Some(
        MpeProvenance::new(
            MpeProfile::new(
                "physical-input-17".into(),
                MpeZone::lower(4).unwrap(),
                BendRange::new(48, 0).unwrap(),
                BendRange::new(2, 0).unwrap(),
            )
            .unwrap(),
            Channel::new(2).unwrap(),
        )
        .unwrap(),
    );
    event
}

fn timeline(events: &[TimedEvent]) -> Vec<(Beat, Message)> {
    events
        .iter()
        .map(|event| (event.at, event.message.clone()))
        .collect()
}

fn single_lane(phrase: Phrase, looped: bool) -> Song {
    let mut song = Song::default();
    song.tracks.truncate(1);
    let phrase_id = PhraseId::new(101).unwrap();
    let chain = ChainId::new(102).unwrap();
    song.phrases.insert(phrase_id, phrase);
    song.chains.insert(
        chain,
        Chain {
            name: "A".into(),
            phrases: vec![PhraseUse {
                id: UseId::new(103).unwrap(),
                phrase: phrase_id,
                repeats: 1,
            }],
        },
    );
    song.tracks[0].slots = vec![Some(ChainUse {
        id: UseId::new(104).unwrap(),
        chain: chain.into(),
        repeats: 1,
    })];
    song.tracks[0].looped = looped;
    song
}

fn player(song: &Song) -> Playback {
    Playback::new(Arc::new(
        Snapshot::compile(song, 7, PlaybackBudget::default()).unwrap(),
    ))
}

fn attacks(events: &[TimedEvent], track: TrackId) -> Vec<&TimedEvent> {
    events
        .iter()
        .filter(|event| {
            event.owner.track == track && matches!(event.message, Message::NoteOn { .. })
        })
        .collect()
}

#[test]
fn voice_expression_is_distinct_from_channel_controls_and_initializes_before_render() {
    let mut event = note(1, Beat::ZERO, 60, Beat::ONE);
    event.controls.push(Control::Bend { value: 9000 });
    event.note.as_mut().unwrap().expression = vec![
        Expression {
            sequence: None,
            offset: Beat::ZERO,
            control: Control::Pressure { value: 20 },
        },
        Expression {
            sequence: None,
            offset: Beat::ZERO,
            control: Control::Pressure { value: 30 },
        },
        Expression {
            sequence: None,
            offset: Beat::ONE,
            control: Control::Pressure { value: 40 },
        },
    ];
    let song = single_lane(
        Phrase {
            events: vec![event],
            ..Phrase::default()
        },
        false,
    );
    let events = player(&song).window(Beat::integer(2)).unwrap();
    assert_eq!(
        events.len(),
        6,
        "ordinary notes keep their existing protocol"
    );
    assert!(matches!(
        events[0].message,
        Message::Control {
            control: Control::Bend { value: 9000 }
        }
    ));
    assert!(matches!(events[1].message, Message::NoteOn { .. }));
    assert!(matches!(
        events[2].message,
        Message::Expression {
            control: Control::Pressure { value: 20 }
        }
    ));
    assert!(matches!(
        events[3].message,
        Message::Expression {
            control: Control::Pressure { value: 30 }
        }
    ));
    assert_eq!(events[1].at, events[3].at);
    assert!(matches!(events[4].message, Message::NoteOff { .. }));
    assert!(matches!(
        events[5].message,
        Message::Expression {
            control: Control::Pressure { value: 40 }
        }
    ));
    assert_eq!(events[4].at, events[5].at);
    assert_eq!(events[1].owner, events[5].owner);
}

#[test]
fn send_end_keeps_its_musical_deadline_and_slot_identity_across_tempo_and_edits() {
    let mut event = note(1, Beat::ZERO, 60, Beat::ONE);
    event.fx = [
        Some(Fx::Send {
            bus: 0,
            amount: 35,
            duration: Beat::integer(2),
        }),
        Some(Fx::Send {
            bus: 0,
            amount: 60,
            duration: Beat::integer(3),
        }),
        None,
    ];
    let mut change = note(2, Beat::ONE, 62, Beat::ONE);
    change.fx[0] = Some(Fx::Tempo {
        tempo: Tempo::new(60_000).unwrap(),
    });
    let mut song = single_lane(
        Phrase {
            events: vec![event, change],
            ..Phrase::default()
        },
        false,
    );
    let mut playback = player(&song);
    let starts = playback.window(Beat::ONE).unwrap();
    assert_eq!(
        starts
            .iter()
            .filter(|e| matches!(e.message, Message::Send { .. }))
            .count(),
        2
    );
    // Deleting the authored send cannot remove the cleanup already owned by playback.
    song.phrases.values_mut().next().unwrap().events.remove(0);
    playback.adopt(Arc::new(
        Snapshot::compile(&song, 8, PlaybackBudget::default()).unwrap(),
    ));
    let events = playback.window(Beat::integer(4)).unwrap();
    let mut tempo = TempoMap::new(Tempo::DEFAULT);
    for event in &events {
        if let Message::Tempo { tempo: next } = event.message {
            tempo.insert(event.at, next);
        }
    }
    let ends: Vec<_> = events
        .iter()
        .filter_map(|e| match e.message {
            Message::EndSend { slot } => Some((slot, e.at, tempo.frame_at(e.at, 48000).unwrap())),
            _ => None,
        })
        .collect();
    assert_eq!(
        ends,
        vec![(0, Beat::integer(2), 72000), (1, Beat::integer(3), 120000)]
    );
}

#[test]
fn capture_loop_keeps_other_tracks_advancing_without_rewriting_shared_chains() {
    let mut song = single_lane(
        Phrase {
            steps: 4,
            events: vec![note(1, Beat::ZERO, 60, Beat::new(1, 4).unwrap())],
            ..Phrase::default()
        },
        false,
    );
    let mut second = song.tracks[0].clone();
    second.id = TrackId::new(200).unwrap();
    second.slots[0].as_mut().unwrap().id = UseId::new(201).unwrap();
    song.tracks.push(second);
    let original = song.clone();
    let path = crest_tracker::editing::Occurrence {
        track: song.tracks[0].id,
        chain_use: UseId::new(104).unwrap(),
        phrase_use: UseId::new(103).unwrap(),
    };
    let snapshot = Arc::new(Snapshot::compile(&song, 1, PlaybackBudget::default()).unwrap());
    let mut playback = Playback::new_looping(snapshot.clone(), path).unwrap();
    let events = playback.window(Beat::integer(3)).unwrap();
    assert_eq!(attacks(&events, song.tracks[0].id).len(), 3);
    assert_eq!(attacks(&events, song.tracks[1].id).len(), 1);
    assert_eq!(snapshot.song(), &original);
    assert!(attacks(&events, path.track)
        .iter()
        .all(|e| e.owner.phrase_use == path.phrase_use));
}

#[test]
fn partitioned_windows_match_one_window_including_delayed_events_and_cleanup() {
    let mut delayed = note(2, Beat::new(1, 3).unwrap(), 67, Beat::new(1, 6).unwrap());
    delayed.fx = [
        Some(Fx::Delay {
            beats: Beat::new(2, 3).unwrap(),
        }),
        Some(Fx::Chance { percent: 63 }),
        None,
    ];
    let phrase = Phrase {
        steps: 4,
        events: vec![
            note(1, Beat::ZERO, 60, Beat::new(1, 2).unwrap()),
            delayed,
            note(3, Beat::new(1, 2).unwrap(), 60, Beat::new(1, 2).unwrap()),
        ],
        ..Phrase::default()
    };
    let song = single_lane(phrase, true);
    let expected = player(&song).window(Beat::integer(8)).unwrap();
    let mut split = player(&song);
    let mut actual = Vec::new();
    for numerator in 1..=56 {
        actual.extend(split.window(Beat::new(numerator, 7).unwrap()).unwrap());
    }
    assert_eq!(actual, expected);
    assert_eq!(split.horizon(), Beat::integer(8));
    assert!(matches!(
        split.window(Beat::integer(7)),
        Err(PlaybackError::Backwards)
    ));
    assert_eq!(split.horizon(), Beat::integer(8));
}

#[test]
fn unequal_straight_and_triplet_lanes_keep_independent_exact_cycles() {
    let straight = Phrase {
        events: vec![note(1, Beat::ZERO, 60, Beat::new(1, 4).unwrap())],
        ..Phrase::default()
    };
    let mut song = single_lane(straight, true);
    let phrase_id = PhraseId::new(201).unwrap();
    let chain = ChainId::new(202).unwrap();
    song.phrases.insert(
        phrase_id,
        Phrase {
            steps: 10,
            division: Division::new(8, 3, 2).unwrap(),
            events: vec![note(2, Beat::ZERO, 67, Beat::new(1, 3).unwrap())],
            ..Phrase::default()
        },
    );
    song.chains.insert(
        chain,
        Chain {
            name: "Triplets".into(),
            phrases: vec![PhraseUse {
                id: UseId::new(203).unwrap(),
                phrase: phrase_id,
                repeats: 1,
            }],
        },
    );
    let mut second = song.tracks[0].clone();
    second.id = TrackId::new(2).unwrap();
    second.slots = vec![Some(ChainUse {
        id: UseId::new(204).unwrap(),
        chain: chain.into(),
        repeats: 1,
    })];
    song.tracks.push(second);
    let events = player(&song).window(Beat::integer(21)).unwrap();
    let straight = attacks(&events, TrackId::new(1).unwrap());
    let triplet = attacks(&events, TrackId::new(2).unwrap());
    assert_eq!(
        straight.iter().map(|event| event.at).collect::<Vec<_>>(),
        [0, 4, 8, 12, 16, 20].map(Beat::integer)
    );
    assert_eq!(
        triplet.iter().map(|event| event.at).collect::<Vec<_>>(),
        [0, 10, 20, 30, 40, 50, 60].map(|value| Beat::new(value, 3).unwrap())
    );
    assert_eq!(straight.last().unwrap().owner.cycle, 5);
    assert_eq!(triplet.last().unwrap().owner.cycle, 6);
}

#[test]
fn shared_chain_occurrences_have_separate_owners_and_repeat_counters() {
    let phrase = Phrase {
        steps: 1,
        events: vec![note(1, Beat::ZERO, 60, Beat::new(1, 8).unwrap())],
        ..Phrase::default()
    };
    let mut song = single_lane(phrase, false);
    let mut second = song.tracks[0].clone();
    second.id = TrackId::new(2).unwrap();
    second.slots[0].as_mut().unwrap().id = UseId::new(105).unwrap();
    second.slots[0].as_mut().unwrap().repeats = 3;
    song.tracks.push(second);
    let before = song.clone();
    let mut playback = player(&song);
    let events = playback.window(Beat::ONE).unwrap();
    assert_eq!(attacks(&events, TrackId::new(1).unwrap()).len(), 1);
    let repeated = attacks(&events, TrackId::new(2).unwrap());
    assert_eq!(
        repeated
            .iter()
            .map(|event| event.owner.cycle)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert!(repeated
        .iter()
        .all(|event| event.owner.chain_use == UseId::new(105).unwrap()));
    assert!(playback.finished());
    assert_eq!(song, before);
}

#[test]
fn equal_beat_release_precedes_retrigger_attack() {
    let phrase = Phrase {
        steps: 2,
        events: vec![
            note(1, Beat::ZERO, 60, Beat::new(1, 4).unwrap()),
            note(2, Beat::new(1, 4).unwrap(), 60, Beat::new(1, 4).unwrap()),
        ],
        ..Phrase::default()
    };
    let events = player(&single_lane(phrase, false))
        .window(Beat::ONE)
        .unwrap();
    let boundary: Vec<_> = events
        .iter()
        .filter(|event| event.at == Beat::new(1, 4).unwrap())
        .collect();
    assert!(matches!(
        boundary[0].message,
        Message::NoteOff { pitch: 60, .. }
    ));
    assert!(matches!(
        boundary[1].message,
        Message::NoteOn { pitch: 60, .. }
    ));
    assert_ne!(boundary[0].owner, boundary[1].owner);
}

#[test]
fn chance_suppression_preserves_global_controls_and_earlier_owned_cleanup() {
    let mut suppressed = note(2, Beat::ONE, 60, Beat::ONE);
    suppressed.fx = [
        Some(Fx::Chance { percent: 0 }),
        Some(Fx::Control {
            control: Control::Cc {
                number: 1,
                value: 96,
            },
        }),
        Some(Fx::Tempo {
            tempo: Tempo::new(90_000).unwrap(),
        }),
    ];
    let mut controls_only = note(3, Beat::new(1, 2).unwrap(), 67, Beat::ONE);
    controls_only.note = None;
    controls_only.controls.push(Control::Program { value: 7 });
    controls_only.fx[0] = Some(Fx::Control {
        control: Control::Pressure { value: 70 },
    });
    let phrase = Phrase {
        events: vec![
            note(1, Beat::ZERO, 60, Beat::integer(2)),
            suppressed,
            controls_only,
        ],
        ..Phrase::default()
    };
    let events = player(&single_lane(phrase, false))
        .window(Beat::integer(3))
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.message, Message::NoteOn { .. }))
            .count(),
        1
    );
    let releases: Vec<_> = events
        .iter()
        .filter(|event| matches!(event.message, Message::NoteOff { .. }))
        .collect();
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0].at, Beat::integer(2));
    assert_eq!(releases[0].owner.event, EventId::new(1).unwrap());
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.message, Message::Control { .. }))
            .count(),
        3
    );
    assert!(events
        .iter()
        .any(|event| event.at == Beat::ONE && matches!(event.message, Message::Tempo { .. })));
}

#[test]
fn tempo_segments_accumulate_exactly_before_final_frame_rounding() {
    let mut map = TempoMap::new(Tempo::DEFAULT);
    map.insert(Beat::new(2, 3).unwrap(), Tempo::new(60_000).unwrap());
    map.insert(Beat::new(1, 3).unwrap(), Tempo::new(90_000).unwrap());
    assert_eq!(map.frame_at(Beat::ONE, 48_000).unwrap(), 34_667);
    assert_eq!(map.frame_at(Beat::ONE, 3).unwrap(), 2);
    assert_eq!(
        map.frame_at(Beat::new(1, 3).unwrap(), 48_000).unwrap(),
        8_000
    );
    map.insert(Beat::new(1, 3).unwrap(), Tempo::new(180_000).unwrap());
    assert_eq!(map.frame_at(Beat::ONE, 48_000).unwrap(), 29_333);
}

#[test]
fn many_unrelated_tempos_keep_sample_accuracy_and_representable_cursors() {
    let values = [
        120001, 119999, 113003, 127007, 109013, 131009, 107021, 137029, 103007, 139021,
    ];
    let mut map = TempoMap::new(Tempo::new(values[0]).unwrap());
    for index in 0..200 {
        map.insert(
            Beat::integer(index),
            Tempo::new(values[index as usize % values.len()]).unwrap(),
        );
    }
    // Independently summed as exact fractions, then rounded once at 48 kHz.
    assert_eq!(map.frame_at(Beat::integer(200), 48_000).unwrap(), 4_823_353);
    for frame in (0..4_823_353).step_by(7919) {
        let at = map.beat_at_frame(frame, 48_000).unwrap();
        assert_eq!(map.frame_at(at, 48_000).unwrap(), frame);
    }
}

#[test]
fn jump_loops_make_positive_progress_and_failed_admission_preserves_state() {
    let mut looping = note(1, Beat::ZERO, 60, Beat::new(1, 8).unwrap());
    looping.fx[0] = Some(Fx::Jump { step: 0 });
    let song = single_lane(
        Phrase {
            steps: 4,
            events: vec![looping],
            ..Phrase::default()
        },
        false,
    );
    let snapshot = Snapshot::compile(
        &song,
        7,
        PlaybackBudget {
            steps_per_window: 2,
            events_per_window: 16,
        },
    )
    .unwrap();
    let mut playback = Playback::new(Arc::new(snapshot));
    let before = serde_json::to_value(playback.playheads()).unwrap();
    assert!(matches!(
        playback.window(Beat::ONE),
        Err(PlaybackError::Budget(_))
    ));
    assert_eq!(playback.horizon(), Beat::ZERO);
    assert_eq!(serde_json::to_value(playback.playheads()).unwrap(), before);
    let mut output = playback.window(Beat::new(1, 2).unwrap()).unwrap();
    output.extend(playback.window(Beat::ONE).unwrap());
    let starts = attacks(&output, TrackId::new(1).unwrap());
    assert_eq!(
        starts.iter().map(|event| event.at).collect::<Vec<_>>(),
        [0, 1, 2, 3].map(|value| Beat::new(value, 4).unwrap())
    );
    assert_eq!(starts.iter().map(|event| &event.owner).collect::<BTreeSet<_>>().len(), 4,
        "each jump traversal needs a distinct attack owner so stale releases cannot address a later attack");
}

#[test]
fn invalid_compiler_candidate_leaves_the_previous_snapshot_playable() {
    let song = single_lane(
        Phrase {
            steps: 1,
            events: vec![note(1, Beat::ZERO, 60, Beat::new(1, 8).unwrap())],
            ..Phrase::default()
        },
        true,
    );
    let mut playback = player(&song);
    let mut invalid = song.clone();
    let event = &mut invalid.phrases.values_mut().next().unwrap().events[0];
    event.note.as_mut().unwrap().expression.push(Expression {
        sequence: None,
        offset: Beat::ZERO,
        control: Control::Pressure { value: 99 },
    });
    event.fx[0] = Some(Fx::Chord {
        intervals: vec![0, 4, 7],
    });
    assert!(matches!(
        Snapshot::compile(&invalid, 8, PlaybackBudget::default()),
        Err(PlaybackError::Expression(_))
    ));
    assert_eq!(playback.revision(), 7);
    assert_eq!(
        playback.window(Beat::ONE).unwrap(),
        player(&song).window(Beat::ONE).unwrap()
    );
}

#[test]
fn seek_retains_future_off_grid_attack_and_discards_old_release() {
    let phrase = Phrase {
        events: vec![
            note(201, Beat::ZERO, 60, Beat::ONE),
            note(202, Beat::new(1, 8).unwrap(), 64, Beat::ONE),
        ],
        ..Phrase::default()
    };
    let mut playback = player(&single_lane(phrase, false));
    playback.seek_forward(Beat::new(1, 16).unwrap()).unwrap();
    let events = playback.window(Beat::new(2, 1).unwrap()).unwrap();
    assert!(!events
        .iter()
        .any(|event| event.owner.event == EventId::new(201).unwrap()));
    assert!(events
        .iter()
        .any(|event| event.at == Beat::new(1, 8).unwrap()
            && matches!(event.message, Message::NoteOn { pitch: 64, .. })));
    assert!(events
        .iter()
        .any(|event| matches!(event.message, Message::NoteOff { pitch: 64, .. })));
    let horizon = playback.horizon();
    assert!(playback.seek_forward(Beat::ZERO).is_err());
    assert_eq!(playback.horizon(), horizon);
}

#[test]
fn mpe_key_release_preserves_both_expression_scopes_until_owned_lifetime_end() {
    let mut event = mpe_note(1, Beat::ZERO, beat(1, 2), Beat::integer(2));
    let note = event.note.as_mut().unwrap();
    note.release_velocity = 73;
    note.expression = vec![
        expression(Beat::ZERO, 2, Control::Bend { value: 8_192 }),
        expression(Beat::ONE, 3, Control::Pressure { value: 77 }),
    ];
    note.master_expression = vec![
        expression(
            Beat::ZERO,
            1,
            Control::Cc {
                number: 64,
                value: 127,
            },
        ),
        expression(beat(3, 2), 4, Control::Bend { value: 10_000 }),
    ];
    let target = event.target.clone();
    let song = single_lane(
        Phrase {
            steps: 4,
            events: vec![event],
            ..Phrase::default()
        },
        false,
    );
    let mut playback = player(&song);
    let mut events = playback.window(Beat::integer(2)).unwrap();
    assert!(
        !playback.finished(),
        "expression ownership outlives the phrase"
    );
    events.extend(playback.window(beat(9, 4)).unwrap());
    assert!(playback.finished());
    assert_eq!(
        timeline(&events),
        vec![
            (
                Beat::ZERO,
                Message::NoteOn {
                    pitch: 60,
                    velocity: 100
                }
            ),
            (
                Beat::ZERO,
                Message::ExpressionLifetime {
                    duration: Beat::integer(2)
                }
            ),
            (
                Beat::ZERO,
                Message::MasterExpression {
                    control: Control::Cc {
                        number: 64,
                        value: 127
                    }
                }
            ),
            (
                Beat::ZERO,
                Message::Expression {
                    control: Control::Bend { value: 8_192 }
                }
            ),
            (
                beat(1, 2),
                Message::NoteOff {
                    pitch: 60,
                    velocity: 73
                }
            ),
            (
                Beat::ONE,
                Message::Expression {
                    control: Control::Pressure { value: 77 }
                }
            ),
            (
                beat(3, 2),
                Message::MasterExpression {
                    control: Control::Bend { value: 10_000 }
                }
            ),
            (
                Beat::integer(2),
                Message::Expression {
                    control: Control::Cc {
                        number: 64,
                        value: 0
                    }
                }
            ),
            (
                Beat::integer(2),
                Message::MasterExpression {
                    control: Control::Cc {
                        number: 64,
                        value: 0
                    }
                }
            ),
            (Beat::integer(2), Message::EndExpression),
        ]
    );
    assert!(events.iter().all(|event| event.owner == events[0].owner));
    assert!(events.iter().all(|event| event.target == target));
}

#[test]
fn reused_mpe_member_and_pitch_keep_independent_sustained_owners() {
    let mut old = mpe_note(1, Beat::ZERO, beat(1, 4), Beat::integer(2));
    old.note.as_mut().unwrap().expression =
        vec![expression(Beat::ONE, 1, Control::Pressure { value: 31 })];
    let mut new = mpe_note(2, beat(1, 2), beat(1, 4), beat(5, 2));
    new.note.as_mut().unwrap().expression = vec![expression(
        Beat::integer(2),
        2,
        Control::Pressure { value: 93 },
    )];
    assert_eq!(
        old.note.as_ref().unwrap().mpe,
        new.note.as_ref().unwrap().mpe
    );
    let song = single_lane(
        Phrase {
            events: vec![old, new],
            ..Phrase::default()
        },
        false,
    );
    let events = player(&song).window(Beat::integer(4)).unwrap();
    let attacks = attacks(&events, song.tracks[0].id);
    assert_eq!(attacks.len(), 2);
    assert_ne!(attacks[0].owner, attacks[1].owner);
    for (attack, release, pressure, value, end) in [
        (attacks[0], beat(1, 4), Beat::ONE, 31, Beat::integer(2)),
        (attacks[1], beat(3, 4), beat(5, 2), 93, Beat::integer(3)),
    ] {
        let owned: Vec<_> = events
            .iter()
            .filter(|event| event.owner == attack.owner)
            .collect();
        assert_eq!(
            owned
                .iter()
                .filter(|event| matches!(event.message, Message::NoteOn { .. }))
                .count(),
            1
        );
        assert_eq!(
            owned
                .iter()
                .filter(|event| matches!(event.message, Message::NoteOff { .. }))
                .map(|event| event.at)
                .collect::<Vec<_>>(),
            vec![release]
        );
        assert!(owned.iter().any(|event| event.at == pressure
            && event.message
                == Message::Expression {
                    control: Control::Pressure { value }
                }));
        assert_eq!(
            owned
                .iter()
                .filter(|event| matches!(event.message, Message::EndExpression))
                .map(|event| event.at)
                .collect::<Vec<_>>(),
            vec![end]
        );
        assert_eq!(owned.last().unwrap().message, Message::EndExpression);
    }
}

#[test]
fn equal_time_mpe_controls_follow_capture_sequence_across_both_scopes() {
    let mut event = mpe_note(1, Beat::ZERO, Beat::ONE, Beat::ONE);
    let note = event.note.as_mut().unwrap();
    note.expression = vec![
        expression(Beat::ZERO, 9, Control::Pressure { value: 90 }),
        expression(Beat::ZERO, 2, Control::Bend { value: 9_000 }),
        expression(Beat::ONE, 12, Control::Pressure { value: 12 }),
    ];
    note.master_expression = vec![
        expression(Beat::ZERO, 8, Control::Pressure { value: 80 }),
        expression(Beat::ZERO, 1, Control::Bend { value: 8_000 }),
        expression(Beat::ONE, 11, Control::Pressure { value: 11 }),
    ];
    let song = single_lane(
        Phrase {
            events: vec![event],
            ..Phrase::default()
        },
        false,
    );
    let events = player(&song).window(Beat::integer(2)).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.at == Beat::ZERO)
            .map(|event| event.message.clone())
            .collect::<Vec<_>>(),
        vec![
            Message::NoteOn {
                pitch: 60,
                velocity: 100
            },
            Message::ExpressionLifetime {
                duration: Beat::ONE
            },
            Message::MasterExpression {
                control: Control::Bend { value: 8_000 }
            },
            Message::Expression {
                control: Control::Bend { value: 9_000 }
            },
            Message::MasterExpression {
                control: Control::Pressure { value: 80 }
            },
            Message::Expression {
                control: Control::Pressure { value: 90 }
            },
        ]
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.at == Beat::ONE)
            .map(|event| event.message.clone())
            .collect::<Vec<_>>(),
        vec![
            Message::NoteOff {
                pitch: 60,
                velocity: 0
            },
            Message::MasterExpression {
                control: Control::Pressure { value: 11 }
            },
            Message::Expression {
                control: Control::Pressure { value: 12 }
            },
            Message::Expression {
                control: Control::Cc {
                    number: 64,
                    value: 0
                }
            },
            Message::MasterExpression {
                control: Control::Cc {
                    number: 64,
                    value: 0
                }
            },
            Message::EndExpression,
        ]
    );
}

#[test]
fn mpe_gate_and_cut_transform_key_and_expression_lifetimes_together() {
    let gate = Fx::Gate {
        fraction: beat(1, 2),
    };
    let cut = Fx::Cut { after: beat(3, 4) };
    for (effects, release, lifetime, pressure_times) in [
        (
            [Some(gate.clone()), None, None],
            beat(1, 2),
            Beat::integer(2),
            vec![beat(3, 4), beat(3, 2), Beat::integer(2)],
        ),
        (
            [
                Some(Fx::Cut {
                    after: Beat::integer(2),
                }),
                None,
                None,
            ],
            Beat::ONE,
            Beat::integer(2),
            vec![beat(3, 2)],
        ),
        (
            [Some(gate.clone()), Some(cut.clone()), None],
            beat(1, 2),
            beat(3, 4),
            vec![beat(3, 4)],
        ),
        (
            [Some(cut), Some(gate), None],
            beat(1, 2),
            beat(3, 4),
            vec![beat(3, 4)],
        ),
        (
            [Some(Fx::Cut { after: beat(1, 4) }), None, None],
            beat(1, 4),
            beat(1, 4),
            vec![],
        ),
    ] {
        let mut event = mpe_note(1, Beat::ZERO, Beat::ONE, Beat::integer(4));
        event.fx = effects;
        let note = event.note.as_mut().unwrap();
        note.expression = vec![
            expression(
                Beat::ZERO,
                1,
                Control::Cc {
                    number: 64,
                    value: 127,
                },
            ),
            expression(beat(3, 2), 3, Control::Pressure { value: 33 }),
            expression(Beat::integer(3), 4, Control::Pressure { value: 66 }),
            expression(Beat::integer(4), 5, Control::Pressure { value: 99 }),
        ];
        note.master_expression = vec![expression(
            Beat::ZERO,
            2,
            Control::Cc {
                number: 64,
                value: 127,
            },
        )];
        let song = single_lane(
            Phrase {
                events: vec![event],
                ..Phrase::default()
            },
            false,
        );
        let events = player(&song).window(Beat::integer(5)).unwrap();
        assert!(events.iter().any(|event| event.at == Beat::ZERO
            && event.message == Message::ExpressionLifetime { duration: lifetime }));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.message, Message::NoteOff { .. }))
                .map(|event| event.at)
                .collect::<Vec<_>>(),
            vec![release]
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event.message,
                    Message::Expression {
                        control: Control::Pressure { .. }
                    }
                ))
                .map(|event| event.at)
                .collect::<Vec<_>>(),
            pressure_times
        );
        assert_eq!(
            timeline(&events[events.len() - 3..]),
            vec![
                (
                    lifetime,
                    Message::Expression {
                        control: Control::Cc {
                            number: 64,
                            value: 0
                        }
                    }
                ),
                (
                    lifetime,
                    Message::MasterExpression {
                        control: Control::Cc {
                            number: 64,
                            value: 0
                        }
                    }
                ),
                (lifetime, Message::EndExpression),
            ]
        );
        assert!(events.iter().all(|event| event.at <= lifetime));
    }
}

#[test]
fn suppressed_mpe_note_emits_no_owned_lifetime_or_cleanup() {
    for suppression in [Fx::Cut { after: Beat::ZERO }, Fx::Chance { percent: 0 }] {
        let mut event = mpe_note(1, Beat::ZERO, Beat::ONE, Beat::integer(2));
        event.note.as_mut().unwrap().expression =
            vec![expression(Beat::ONE, 1, Control::Pressure { value: 50 })];
        event.note.as_mut().unwrap().master_expression = vec![expression(
            Beat::ZERO,
            2,
            Control::Cc {
                number: 64,
                value: 127,
            },
        )];
        event.controls.push(Control::Program { value: 7 });
        event.fx[0] = Some(suppression);
        let song = single_lane(
            Phrase {
                events: vec![event],
                ..Phrase::default()
            },
            false,
        );
        assert_eq!(
            timeline(&player(&song).window(Beat::integer(4)).unwrap()),
            vec![(
                Beat::ZERO,
                Message::Control {
                    control: Control::Program { value: 7 }
                }
            )]
        );
    }
}

#[test]
fn mpe_seek_discards_old_tails_and_preserves_future_off_grid_and_delayed_attacks() {
    let old = mpe_note(1, Beat::ZERO, beat(1, 4), Beat::integer(3));
    let off_grid = mpe_note(2, beat(1, 8), beat(1, 4), Beat::integer(2));
    let mut delayed = mpe_note(3, Beat::ZERO, beat(1, 4), Beat::integer(2));
    delayed.fx[0] = Some(Fx::Delay { beats: beat(1, 2) });
    let mut notes = vec![old, off_grid, delayed];
    for event in &mut notes {
        let note = event.note.as_mut().unwrap();
        note.expression = vec![expression(Beat::ONE, 2, Control::Pressure { value: 55 })];
        note.master_expression = vec![expression(Beat::ONE, 1, Control::Bend { value: 9_500 })];
    }
    let song = single_lane(
        Phrase {
            events: notes,
            ..Phrase::default()
        },
        false,
    );
    let all = player(&song).window(Beat::integer(4)).unwrap();
    for position in [
        beat(1, 16),
        beat(1, 8),
        beat(1, 4),
        beat(1, 2),
        Beat::ONE,
        Beat::integer(3),
    ] {
        let retained: BTreeSet<_> = all
            .iter()
            .filter(|event| event.at >= position && matches!(event.message, Message::NoteOn { .. }))
            .map(|event| event.owner.clone())
            .collect();
        let expected: Vec<_> = all
            .iter()
            .filter(|event| retained.contains(&event.owner))
            .cloned()
            .collect();
        let mut playback = player(&song);
        playback.seek_forward(position).unwrap();
        let mut actual = playback.window(position).unwrap();
        assert!(actual.is_empty(), "seek windows remain half-open");
        for end in [
            beat(1, 8),
            beat(1, 2),
            Beat::ONE,
            Beat::integer(2),
            Beat::integer(4),
        ] {
            if end >= position {
                actual.extend(playback.window(end).unwrap());
            }
        }
        assert_eq!(actual, expected, "seek at {position:?}");
    }
}

#[test]
fn seeking_after_delivered_key_release_removes_its_remaining_mpe_tail() {
    let event = mpe_note(1, Beat::ZERO, beat(1, 4), Beat::integer(3));
    let song = single_lane(
        Phrase {
            events: vec![event],
            ..Phrase::default()
        },
        false,
    );
    let mut playback = player(&song);
    assert!(playback
        .window(beat(1, 2))
        .unwrap()
        .iter()
        .any(|event| matches!(event.message, Message::NoteOff { .. })));
    playback.seek_forward(Beat::ONE).unwrap();
    assert!(playback.window(Beat::integer(4)).unwrap().is_empty());
    assert!(playback.finished());
}

#[test]
fn deleting_authored_mpe_note_cannot_remove_already_owned_tail_cleanup() {
    let mut event = mpe_note(1, Beat::ZERO, beat(1, 4), Beat::integer(3));
    event.note.as_mut().unwrap().master_expression = vec![expression(
        Beat::integer(2),
        1,
        Control::Pressure { value: 91 },
    )];
    let mut song = single_lane(
        Phrase {
            events: vec![event],
            ..Phrase::default()
        },
        false,
    );
    let expected = player(&song).window(Beat::integer(4)).unwrap();
    let mut playback = player(&song);
    let mut actual = playback.window(beat(1, 2)).unwrap();
    song.phrases.values_mut().next().unwrap().events.clear();
    playback.adopt(Arc::new(
        Snapshot::compile(&song, 8, PlaybackBudget::default()).unwrap(),
    ));
    actual.extend(playback.window(Beat::integer(4)).unwrap());
    assert_eq!(actual, expected);
    assert!(playback.finished());
}

#[test]
fn partitioned_mpe_windows_preserve_overlapping_cycles_and_exact_control_order() {
    let mut first = mpe_note(1, Beat::ZERO, beat(1, 4), Beat::integer(2));
    first.note.as_mut().unwrap().expression =
        vec![expression(Beat::ONE, 2, Control::Pressure { value: 11 })];
    first.note.as_mut().unwrap().master_expression =
        vec![expression(Beat::ONE, 1, Control::Pressure { value: 22 })];
    let mut second = mpe_note(2, beat(1, 8), beat(1, 4), beat(3, 2));
    second.fx[0] = Some(Fx::Delay { beats: beat(1, 3) });
    second.note.as_mut().unwrap().expression =
        vec![expression(beat(1, 3), 3, Control::Bend { value: 10_123 })];
    let song = single_lane(
        Phrase {
            steps: 4,
            events: vec![first, second],
            ..Phrase::default()
        },
        true,
    );
    let expected = player(&song).window(Beat::integer(8)).unwrap();
    let mut playback = player(&song);
    let mut actual = Vec::new();
    for numerator in 1..=192 {
        actual.extend(playback.window(beat(numerator, 24)).unwrap());
    }
    assert_eq!(actual, expected);
    let attacks = attacks(&actual, song.tracks[0].id);
    assert_eq!(attacks.len(), 16);
    assert_eq!(
        attacks
            .iter()
            .map(|event| &event.owner)
            .collect::<BTreeSet<_>>()
            .len(),
        16
    );
}

#[test]
fn explicit_expression_lifetime_rejects_polyphonic_expansion_even_without_curves() {
    for effect in [
        Fx::Chord {
            intervals: vec![0, 4, 7],
        },
        Fx::Add { intervals: vec![7] },
        Fx::Retrigger {
            count: 2,
            spacing: beat(1, 4),
        },
    ] {
        for with_provenance in [false, true] {
            let mut event = mpe_note(1, Beat::ZERO, beat(1, 4), Beat::ONE);
            if !with_provenance {
                event.note.as_mut().unwrap().mpe = None;
            }
            event.fx[0] = Some(effect.clone());
            let song = single_lane(
                Phrase {
                    events: vec![event],
                    ..Phrase::default()
                },
                false,
            );
            assert!(matches!(
                Snapshot::compile(&song, 7, PlaybackBudget::default()),
                Err(PlaybackError::Expression(_))
            ));
        }
    }
}

#[test]
fn failed_mpe_window_admission_preserves_existing_tail_and_seek_state() {
    let song = single_lane(
        Phrase {
            events: vec![
                mpe_note(1, Beat::ZERO, beat(1, 4), Beat::integer(3)),
                mpe_note(2, beat(1, 4), beat(1, 4), Beat::integer(3)),
            ],
            ..Phrase::default()
        },
        false,
    );
    let snapshot = Snapshot::compile(
        &song,
        7,
        PlaybackBudget {
            steps_per_window: 64,
            events_per_window: 8,
        },
    )
    .unwrap();
    let mut playback = Playback::new(Arc::new(snapshot));
    let mut actual = playback.window(beat(1, 4)).unwrap();
    let before = serde_json::to_value(playback.playheads()).unwrap();
    assert!(matches!(
        playback.window(Beat::ONE),
        Err(PlaybackError::Budget(_))
    ));
    assert!(matches!(
        playback.seek_forward(Beat::ONE),
        Err(PlaybackError::Budget(_))
    ));
    assert_eq!(playback.horizon(), beat(1, 4));
    assert_eq!(serde_json::to_value(playback.playheads()).unwrap(), before);
    let mut repaired = song;
    repaired
        .phrases
        .values_mut()
        .next()
        .unwrap()
        .events
        .remove(1);
    playback.adopt(Arc::new(
        Snapshot::compile(&repaired, 8, PlaybackBudget::default()).unwrap(),
    ));
    actual.extend(playback.window(Beat::integer(4)).unwrap());
    assert_eq!(actual, player(&repaired).window(Beat::integer(4)).unwrap());
}

#[test]
fn off_cuts_owned_chord_across_windows_without_cutting_other_track_or_future_attack() {
    let mut chord = note(110, Beat::ZERO, 60, Beat::integer(8));
    chord.fx[0] = Some(Fx::Chord {
        intervals: vec![0, 4, 7],
    });
    let off = Event::note_off(EventId::new(111).unwrap(), Beat::ONE, chord.target.clone());
    let later = note(112, Beat::integer(2), 72, Beat::integer(3));
    let mut song = single_lane(
        Phrase {
            events: vec![chord, off, later],
            ..Phrase::default()
        },
        false,
    );
    let other_phrase = PhraseId::new(201).unwrap();
    let other_chain = ChainId::new(202).unwrap();
    song.phrases.insert(
        other_phrase,
        Phrase {
            events: vec![note(210, Beat::ZERO, 60, Beat::integer(8))],
            ..Phrase::default()
        },
    );
    song.chains.insert(
        other_chain,
        Chain {
            name: "Other".into(),
            phrases: vec![PhraseUse {
                id: UseId::new(203).unwrap(),
                phrase: other_phrase,
                repeats: 1,
            }],
        },
    );
    song.tracks.push(Track {
        id: TrackId::new(2).unwrap(),
        name: "Other".into(),
        slots: vec![Some(ChainUse {
            id: UseId::new(204).unwrap(),
            chain: (other_chain).into(),
            repeats: 1,
        })],
        looped: false,
        muted: false,
    });
    let expected = player(&song).window(Beat::integer(10)).unwrap();
    let mut playback = player(&song);
    let mut actual = Vec::new();
    for quarter in 1..=40 {
        actual.extend(playback.window(beat(quarter, 4)).unwrap());
    }
    assert_eq!(
        actual, expected,
        "OFF must not depend on scheduler window size"
    );
    let releases: Vec<_> = actual
        .iter()
        .filter(|event| {
            event.owner.event.get() == 110 && matches!(event.message, Message::NoteOff { .. })
        })
        .collect();
    assert_eq!(releases.len(), 3);
    assert!(releases.iter().all(|event| event.at == Beat::ONE));
    assert!(actual.iter().any(|event| event.owner.event.get() == 210
        && event.at == Beat::integer(8)
        && matches!(event.message, Message::NoteOff { .. })));
    assert!(actual.iter().any(|event| event.owner.event.get() == 112
        && event.at == Beat::integer(5)
        && matches!(event.message, Message::NoteOff { .. })));
}

#[test]
fn off_ends_sustained_expression_and_removes_later_points() {
    let mut event = mpe_note(110, Beat::ZERO, beat(1, 2), Beat::integer(4));
    event.note.as_mut().unwrap().expression = vec![
        expression(
            Beat::ZERO,
            1,
            Control::Cc {
                number: 64,
                value: 127,
            },
        ),
        expression(Beat::integer(2), 2, Control::Pressure { value: 100 }),
    ];
    let off = Event::note_off(EventId::new(111).unwrap(), Beat::ONE, event.target.clone());
    let song = single_lane(
        Phrase {
            events: vec![event, off],
            ..Phrase::default()
        },
        false,
    );
    let mut playback = player(&song);
    playback.window(beat(3, 4)).unwrap();
    let remaining = playback.window(Beat::integer(8)).unwrap();
    assert!(remaining
        .iter()
        .any(|event| event.at == Beat::ONE && matches!(event.message, Message::EndExpression)));
    assert!(remaining.iter().any(|event| event.at == Beat::ONE
        && matches!(
            event.message,
            Message::Expression {
                control: Control::Cc {
                    number: 64,
                    value: 0
                }
            }
        )));
    assert!(!remaining
        .iter()
        .any(|event| event.at > Beat::ONE && event.owner.event.get() == 110));
}
