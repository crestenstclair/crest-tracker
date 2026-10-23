use crest_tracker::composition::*;
use crest_tracker::playback::{Message, Playback, PlaybackBudget, Snapshot, TimedEvent};
use crest_tracker::time::{Beat, Meter};
use std::sync::Arc;

fn slot(id: u64, kind: SpecialChain, measures: u32) -> Option<ChainUse> {
    Some(ChainUse {
        id: UseId::new(id).unwrap(),
        chain: ChainRef::Special(kind),
        repeats: measures,
    })
}
fn fixture(kind: SpecialChain) -> Song {
    let mut song = Song::default();
    song.tracks.truncate(2);
    let phrase = PhraseId::new(10).unwrap();
    let chain = ChainId::new(11).unwrap();
    song.phrases.insert(
        phrase,
        Phrase {
            events: vec![Event::note(
                EventId::new(12).unwrap(),
                Beat::ZERO,
                Target::Channel {
                    channel: Channel::FIRST,
                    endpoint: None,
                },
                Note::new(60, 100, Beat::integer(20)).unwrap(),
            )],
            ..Phrase::default()
        },
    );
    song.chains.insert(
        chain,
        Chain {
            name: "00".into(),
            phrases: vec![PhraseUse {
                id: UseId::new(13).unwrap(),
                phrase,
                repeats: 1,
            }],
        },
    );
    song.tracks[0].slots = vec![
        Some(ChainUse {
            id: UseId::new(14).unwrap(),
            chain: chain.into(),
            repeats: 1,
        }),
        slot(15, kind, 2),
        Some(ChainUse {
            id: UseId::new(16).unwrap(),
            chain: chain.into(),
            repeats: 1,
        }),
    ];
    song.tracks[1].slots = vec![Some(ChainUse {
        id: UseId::new(17).unwrap(),
        chain: chain.into(),
        repeats: 1,
    })];
    song
}
fn playback(song: &Song) -> Playback {
    Playback::new(Arc::new(
        Snapshot::compile(song, 1, PlaybackBudget::default()).unwrap(),
    ))
}
fn collect(song: &Song, window: u64) -> Vec<TimedEvent> {
    let mut playback = playback(song);
    let mut events = vec![];
    for end in (window..=48).step_by(window as usize) {
        events.extend(playback.window(Beat::integer(end)).unwrap());
    }
    events
}

#[test]
fn builtins_round_trip_without_catalog_entries_and_old_numbers_stay_numbers() {
    let song = fixture(SpecialChain::Rest);
    let encoded = serde_json::to_value(&song).unwrap();
    assert_eq!(encoded["tracks"][0]["slots"][0]["chain"], 11);
    assert_eq!(encoded["tracks"][0]["slots"][1]["chain"], "rest");
    let decoded: Song = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(decoded, song);
    decoded.validate().unwrap();
    let mut bad = encoded;
    bad["tracks"][0]["slots"][1]["repeats"] = 0.into();
    assert!(serde_json::from_value::<Song>(bad)
        .unwrap()
        .validate()
        .is_err());
    let mut empty = Song::default();
    empty.tracks[0].slots = vec![slot(50, SpecialChain::Kill, 1)];
    empty.validate().unwrap();
    assert!(empty.chains.is_empty());
    let mut playback = playback(&empty);
    assert!(playback.window(Beat::ONE).unwrap().is_empty());
    assert_eq!(
        playback
            .song_playheads_at(Beat::ZERO)
            .unwrap()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn rest_keeps_existing_releases_and_resumes_after_meter_aware_padding() {
    let mut song = fixture(SpecialChain::Rest);
    song.meters.push(MeterChange {
        at: Beat::integer(8),
        meter: Meter::new(3, 4).unwrap(),
    });
    let events = collect(&song, 1);
    let track = song.tracks[0].id;
    let attacks: Vec<_> = events
        .iter()
        .filter(|e| e.owner.track == track && matches!(e.message, Message::NoteOn { .. }))
        .map(|e| e.at)
        .collect();
    assert_eq!(attacks, vec![Beat::ZERO, Beat::integer(11)]);
    assert!(events.iter().any(|e| e.owner.track == track
        && e.at == Beat::integer(20)
        && matches!(e.message, Message::NoteOff { .. })));
    assert!(!events
        .iter()
        .any(|e| matches!(e.message, Message::KillNote { .. })));
    let mut player = playback(&song);
    player.window(Beat::integer(13)).unwrap();
    assert_eq!(
        player
            .song_playheads_at(Beat::integer(10))
            .unwrap()
            .unwrap()[0]
            .chain_use,
        UseId::new(15).unwrap()
    );
    assert_eq!(
        player
            .song_playheads_at(Beat::integer(11))
            .unwrap()
            .unwrap()[0]
            .chain_use,
        UseId::new(16).unwrap()
    );
}

#[test]
fn kill_cancels_delayed_attacks_retriggers_and_sends_only_on_its_track() {
    let mut song = fixture(SpecialChain::Kill);
    let phrase = song.phrases.values_mut().next().unwrap();
    phrase.events[0].fx[0] = Some(Fx::Send {
        bus: 0,
        amount: 60,
        duration: Beat::integer(30),
    });
    let mut delayed = phrase.events[0].clone();
    delayed.id = EventId::new(22).unwrap();
    delayed.note.as_mut().unwrap().pitch = 67;
    delayed.fx = [
        Some(Fx::Delay {
            beats: Beat::integer(6),
        }),
        Some(Fx::Retrigger {
            count: 2,
            spacing: Beat::integer(2),
        }),
        None,
    ];
    phrase.events.push(delayed);
    let one = collect(&song, 1);
    assert_eq!(
        one,
        collect(&song, 48),
        "window partition cannot change cleanup"
    );
    let track = song.tracks[0].id;
    assert!(one.iter().any(|e| e.owner.track == track
        && e.at == Beat::integer(4)
        && matches!(e.message, Message::KillNote { pitch: 60 })));
    assert!(one.iter().any(|e| e.owner.track == track
        && e.at == Beat::integer(4)
        && matches!(e.message, Message::EndSend { .. })));
    assert!(!one.iter().any(|e| e.owner.track == track
        && e.at >= Beat::integer(4)
        && e.at < Beat::integer(12)
        && matches!(e.message, Message::NoteOn { .. })));
    assert!(one.iter().any(|e| e.owner.track == song.tracks[1].id
        && e.at == Beat::integer(6)
        && matches!(e.message, Message::NoteOn { pitch: 67, .. })));
    assert!(one.iter().any(|e| e.owner.track == song.tracks[1].id
        && e.at == Beat::integer(20)
        && matches!(e.message, Message::NoteOff { pitch: 60, .. })));
    assert!(one.iter().any(|e| e.owner.track == track
        && e.at == Beat::integer(12)
        && matches!(e.message, Message::NoteOn { .. })));
}

#[test]
fn kill_releases_pedal_held_notes_after_their_key_off_was_delivered() {
    let mut song = fixture(SpecialChain::Kill);
    let event = &mut song.phrases.values_mut().next().unwrap().events[0];
    event.note.as_mut().unwrap().duration = Beat::ONE;
    event.controls.push(Control::Cc {
        number: 64,
        value: 127,
    });
    let events = collect(&song, 1);
    assert_eq!(events, collect(&song, 48));
    let kills: Vec<_> = events
        .iter()
        .filter(|e| matches!(e.message, Message::KillNote { .. }))
        .collect();
    assert_eq!(kills.len(), 1);
    assert_eq!(kills[0].owner.track, song.tracks[0].id);
    assert_eq!(kills[0].at, Beat::integer(4));
}

#[test]
fn all_padding_loops_are_bounded_and_independent() {
    let mut song = Song::default();
    song.tracks.truncate(2);
    song.tracks[0].slots = vec![
        slot(30, SpecialChain::Rest, 1),
        slot(31, SpecialChain::Kill, 1),
    ];
    song.tracks[0].looped = true;
    song.tracks[1].slots = vec![slot(32, SpecialChain::Rest, 100)];
    let mut player = playback(&song);
    player.window(Beat::integer(15)).unwrap();
    let heads = player
        .song_playheads_at(Beat::integer(13))
        .unwrap()
        .unwrap();
    assert_eq!(heads[0].chain_use, UseId::new(31).unwrap());
    assert_eq!(heads[1].chain_use, UseId::new(32).unwrap());
    player.discard_playheads_before(Beat::integer(14));
    assert_eq!(
        player
            .song_playheads_at(Beat::integer(14))
            .unwrap()
            .unwrap()
            .len(),
        2
    );
    assert!(!player.finished());
    assert_eq!(
        song.after_measures(Beat::ZERO, u32::MAX).unwrap(),
        Beat::integer(u64::from(u32::MAX) * 4)
    );
}

#[test]
fn seek_does_not_kill_unsounded_voices_and_off_precedes_later_kill() {
    let mut song = fixture(SpecialChain::Kill);
    let mut player = playback(&song);
    player.seek_forward(Beat::integer(2)).unwrap();
    assert!(!player
        .window(Beat::integer(6))
        .unwrap()
        .iter()
        .any(|e| matches!(e.message, Message::KillNote { .. })));
    let phrase = song.phrases.values_mut().next().unwrap();
    let target = phrase.events[0].target.clone();
    phrase.events.push(Event::note_off(
        EventId::new(44).unwrap(),
        Beat::ONE,
        target,
    ));
    let events = collect(&song, 48);
    assert!(!events
        .iter()
        .any(|e| matches!(e.message, Message::KillNote { .. })));
    assert_eq!(events, collect(&song, 1));
}

#[test]
fn append_after_padding_waits_for_its_end_instead_of_resuming_at_lookahead() {
    let mut song = fixture(SpecialChain::Rest);
    song.tracks[0].slots = vec![slot(15, SpecialChain::Rest, 2)];
    let mut player = playback(&song);
    player.window(Beat::ONE).unwrap();
    song.tracks[0].slots.push(Some(ChainUse {
        id: UseId::new(16).unwrap(),
        chain: ChainId::new(11).unwrap().into(),
        repeats: 1,
    }));
    player.adopt(Arc::new(
        Snapshot::compile(&song, 2, PlaybackBudget::default()).unwrap(),
    ));
    let events = player.window(Beat::integer(9)).unwrap();
    let attack = events
        .iter()
        .find(|e| e.owner.track == song.tracks[0].id && matches!(e.message, Message::NoteOn { .. }))
        .unwrap();
    assert_eq!(attack.at, Beat::integer(8));
}

#[test]
fn kill_ends_owned_expression_and_drops_later_modulation() {
    let mut song = fixture(SpecialChain::Kill);
    let note = song.phrases.values_mut().next().unwrap().events[0]
        .note
        .as_mut()
        .unwrap();
    note.duration = Beat::ONE;
    note.expression_duration = Some(Beat::integer(20));
    note.expression.push(Expression {
        offset: Beat::integer(6),
        control: Control::Pressure { value: 10 },
        sequence: None,
    });
    for window in [1, 48] {
        let events = collect(&song, window);
        let first = UseId::new(14).unwrap();
        assert!(events.iter().any(|e| e.owner.chain_use == first
            && e.at == Beat::integer(4)
            && matches!(e.message, Message::KillNote { .. })));
        assert!(events.iter().any(|e| e.owner.chain_use == first
            && e.at == Beat::integer(4)
            && e.message == Message::EndExpression));
        assert!(!events
            .iter()
            .any(|e| e.owner.chain_use == first && e.at > Beat::integer(4)));
    }
}
