use crest_tracker::composition::*;
use crest_tracker::harmony::{Scale, ScaleKind};
use crest_tracker::playback::{Message, Playback, PlaybackBudget, Snapshot, TimedEvent};
use crest_tracker::time::{Beat, Division, Tempo};
use std::sync::Arc;

fn target() -> Target {
    Target::Channel {
        channel: Channel::FIRST,
        endpoint: None,
    }
}

fn phrase(pitch: u8, steps: u32) -> Phrase {
    Phrase {
        steps,
        events: (0..steps)
            .map(|row| {
                Event::note(
                    EventId::new(u64::from(row) + 1).unwrap(),
                    Beat::new(u64::from(row), 4).unwrap(),
                    target(),
                    Note::new(pitch, 100, Beat::new(1, 8).unwrap()).unwrap(),
                )
            })
            .collect(),
        ..Phrase::default()
    }
}

fn song() -> Song {
    let mut song = Song::default();
    song.tracks.truncate(1);
    append_lane(&mut song, 0, phrase(60, 4));
    song
}

fn append_lane(song: &mut Song, lane: usize, phrase: Phrase) {
    let base = 100 + lane as u64 * 10;
    let phrase_id = PhraseId::new(base).unwrap();
    let chain = ChainId::new(base + 1).unwrap();
    song.phrases.insert(phrase_id, phrase);
    song.chains.insert(
        chain,
        Chain {
            name: "A".into(),
            phrases: vec![PhraseUse {
                id: UseId::new(base + 2).unwrap(),
                phrase: phrase_id,
                repeats: 1,
            }],
        },
    );
    if lane >= song.tracks.len() {
        song.tracks.push(Track {
            id: TrackId::new(lane as u64 + 1).unwrap(),
            name: "Added".into(),
            slots: Vec::new(),
            looped: true,
            muted: false,
        });
    }
    song.tracks[lane].slots = vec![Some(ChainUse {
        id: UseId::new(base + 3).unwrap(),
        chain: chain.into(),
        repeats: 1,
    })];
    song.tracks[lane].looped = true;
}

fn snapshot(song: &Song, revision: u64) -> Arc<Snapshot> {
    Arc::new(Snapshot::compile(song, revision, PlaybackBudget::default()).unwrap())
}

fn notes(events: &[TimedEvent], track: u64) -> Vec<(Beat, u8)> {
    events
        .iter()
        .filter_map(|event| match event.message {
            Message::NoteOn { pitch, .. } if event.owner.track.get() == track => {
                Some((event.at, pitch))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn ordinary_edits_start_at_the_next_unscheduled_step_and_keep_old_releases() {
    let mut song = song();
    song.phrases
        .get_mut(&PhraseId::new(100).unwrap())
        .unwrap()
        .events[0]
        .note
        .as_mut()
        .unwrap()
        .duration = Beat::integer(2);
    let mut playback = Playback::new(snapshot(&song, 0));
    let first = playback.window(Beat::new(1, 8).unwrap()).unwrap();
    let old_owner = first[0].owner.clone();
    for event in &mut song
        .phrases
        .get_mut(&PhraseId::new(100).unwrap())
        .unwrap()
        .events
    {
        event.note.as_mut().unwrap().pitch = 67;
    }
    playback.adopt(snapshot(&song, 1));
    assert!(playback.pending_revision());
    let output = playback.window(Beat::new(9, 4).unwrap()).unwrap();
    assert_eq!(notes(&output, 1)[0], (Beat::new(1, 4).unwrap(), 67));
    assert!(output.iter().any(|event| event.owner == old_owner
        && event.at == Beat::integer(2)
        && matches!(event.message, Message::NoteOff { pitch: 60, .. })));
    assert!(!playback.pending_revision());
}

#[test]
fn recorded_revision_stays_whole_after_lookahead_and_subsequent_ordinary_edits() {
    let mut song = song();
    let phrase_id = PhraseId::new(100).unwrap();
    song.phrases.get_mut(&phrase_id).unwrap().events[0]
        .note
        .as_mut()
        .unwrap()
        .duration = Beat::integer(2);
    let mut playback = Playback::new(snapshot(&song, 0));
    // The next loop's first step has already been scheduled before capture ends.
    let scheduled = playback.window(Beat::new(9, 8).unwrap()).unwrap();
    let original_owner = scheduled[0].owner.clone();
    for (id, at, pitch) in [(10, Beat::ZERO, 70), (11, Beat::new(1, 2).unwrap(), 72)] {
        song.phrases
            .get_mut(&phrase_id)
            .unwrap()
            .events
            .push(Event::note(
                EventId::new(id).unwrap(),
                at,
                target(),
                Note::new(pitch, 100, Beat::new(1, 8).unwrap()).unwrap(),
            ));
    }
    playback.adopt_at_phrase_boundary(snapshot(&song, 1));
    song.phrases.get_mut(&phrase_id).unwrap().events[5]
        .note
        .as_mut()
        .unwrap()
        .pitch = 73;
    playback.adopt(snapshot(&song, 2));
    assert!(playback.pending_revision());
    let remainder = playback.window(Beat::integer(2)).unwrap();
    assert_eq!(
        notes(&remainder, 1),
        vec![
            (Beat::new(5, 4).unwrap(), 60),
            (Beat::new(3, 2).unwrap(), 60),
            (Beat::new(7, 4).unwrap(), 60),
        ]
    );
    let replay = playback.window(Beat::new(25, 8).unwrap()).unwrap();
    let new_notes: Vec<_> = notes(&replay, 1)
        .into_iter()
        .filter(|(at, pitch)| *at < Beat::integer(3) && *pitch != 60)
        .collect();
    assert_eq!(
        new_notes,
        vec![(Beat::integer(2), 70), (Beat::new(5, 2).unwrap(), 73)]
    );
    assert!(replay.iter().any(|event| event.owner == original_owner
        && event.at == Beat::integer(2)
        && matches!(event.message, Message::NoteOff { pitch: 60, .. })));
    assert!(!playback.pending_revision());
    // Once this lane has adopted, later ordinary edits regain step-level timing.
    song.phrases.get_mut(&phrase_id).unwrap().events[5]
        .note
        .as_mut()
        .unwrap()
        .pitch = 74;
    playback.adopt(snapshot(&song, 3));
    let ordinary = playback.window(Beat::new(15, 4).unwrap()).unwrap();
    assert!(notes(&ordinary, 1).contains(&(Beat::new(7, 2).unwrap(), 74)));
}

#[test]
fn forced_adoption_uses_each_running_lanes_own_phrase_boundary() {
    let mut song = song();
    append_lane(&mut song, 1, phrase(72, 8));
    let mut playback = Playback::new(snapshot(&song, 0));
    playback.window(Beat::new(1, 8).unwrap()).unwrap();
    song.phrases
        .insert(PhraseId::new(100).unwrap(), phrase(65, 4));
    song.phrases
        .insert(PhraseId::new(110).unwrap(), phrase(77, 8));
    playback.adopt_at_phrase_boundary(snapshot(&song, 1));
    let first = playback.window(Beat::new(3, 2).unwrap()).unwrap();
    assert_eq!(
        notes(&first, 1),
        vec![
            (Beat::new(1, 4).unwrap(), 60),
            (Beat::new(1, 2).unwrap(), 60),
            (Beat::new(3, 4).unwrap(), 60),
            (Beat::ONE, 65),
            (Beat::new(5, 4).unwrap(), 65),
        ]
    );
    assert!(notes(&first, 2).iter().all(|(_, pitch)| *pitch == 72));
    assert!(playback.pending_revision());
    let second = playback.window(Beat::new(5, 2).unwrap()).unwrap();
    assert_eq!(
        notes(&second, 2),
        vec![
            (Beat::new(3, 2).unwrap(), 72),
            (Beat::new(7, 4).unwrap(), 72),
            (Beat::integer(2), 77),
            (Beat::new(9, 4).unwrap(), 77),
        ]
    );
    assert!(!playback.pending_revision());
}

#[test]
fn pending_phrase_boundary_never_delays_invalidation_or_drops_owned_releases() {
    for change in ["delete occurrence", "mute lane", "delete lane"] {
        assert_boundary_adoption_preserves_invalidation(change);
    }
}

fn assert_boundary_adoption_preserves_invalidation(change: &str) {
    let mut song = song();
    append_lane(&mut song, 1, phrase(72, 8));
    song.phrases
        .get_mut(&PhraseId::new(100).unwrap())
        .unwrap()
        .events[0]
        .note
        .as_mut()
        .unwrap()
        .duration = Beat::integer(2);
    let mut playback = Playback::new(snapshot(&song, 0));
    let scheduled = playback.window(Beat::new(1, 8).unwrap()).unwrap();
    let owner = scheduled
        .iter()
        .find(|event| event.owner.track.get() == 1)
        .unwrap()
        .owner
        .clone();
    song.name = "Recorded revision".into();
    playback.adopt_at_phrase_boundary(snapshot(&song, 1));
    match change {
        "delete occurrence" => song.tracks[0].slots[0] = None,
        "mute lane" => song.tracks[0].muted = true,
        "delete lane" => {
            song.tracks.remove(0);
        }
        _ => unreachable!(),
    }
    playback.adopt(snapshot(&song, 2));
    let output = playback.window(Beat::new(9, 4).unwrap()).unwrap();
    assert!(notes(&output, 1).is_empty(), "{change}");
    assert_eq!(
        output
            .iter()
            .filter(|event| event.owner == owner
                && event.at == Beat::integer(2)
                && matches!(event.message, Message::NoteOff { pitch: 60, .. }))
            .count(),
        1,
        "{change}"
    );
    assert!(!playback.pending_revision(), "{change}");
}

#[test]
fn structural_edits_wait_for_each_lanes_own_phrase_boundary() {
    let mut song = song();
    append_lane(&mut song, 1, phrase(72, 8));
    let mut playback = Playback::new(snapshot(&song, 0));
    playback.window(Beat::new(1, 8).unwrap()).unwrap();
    let first = song.phrases.get_mut(&PhraseId::new(100).unwrap()).unwrap();
    *first = phrase(65, 8);
    let second = song.phrases.get_mut(&PhraseId::new(110).unwrap()).unwrap();
    *second = phrase(77, 4);
    playback.adopt(snapshot(&song, 1));
    let first_window = playback.window(Beat::new(3, 2).unwrap()).unwrap();
    assert!(notes(&first_window, 1)
        .iter()
        .filter(|(at, _)| *at < Beat::ONE)
        .all(|(_, pitch)| *pitch == 60));
    assert!(notes(&first_window, 1)
        .iter()
        .filter(|(at, _)| *at >= Beat::ONE)
        .all(|(_, pitch)| *pitch == 65));
    assert!(notes(&first_window, 2)
        .iter()
        .all(|(_, pitch)| *pitch == 72));
    assert!(playback.pending_revision());
    let second_window = playback.window(Beat::new(5, 2).unwrap()).unwrap();
    assert_eq!(
        notes(&second_window, 2),
        vec![
            (Beat::new(3, 2).unwrap(), 72),
            (Beat::new(7, 4).unwrap(), 72),
            (Beat::integer(2), 77),
            (Beat::new(9, 4).unwrap(), 77)
        ]
    );
    assert!(!playback.pending_revision());
}

#[test]
fn division_edits_keep_old_clock_until_the_natural_phrase_boundary() {
    let mut song = song();
    let mut playback = Playback::new(snapshot(&song, 0));
    playback.window(Beat::new(1, 8).unwrap()).unwrap();
    let phrase = song.phrases.get_mut(&PhraseId::new(100).unwrap()).unwrap();
    phrase.division = Division::new(8, 1, 1).unwrap();
    phrase.events.truncate(1);
    phrase.events[0].note.as_mut().unwrap().pitch = 69;
    playback.adopt(snapshot(&song, 1));
    let output = playback.window(Beat::integer(4)).unwrap();
    assert_eq!(
        notes(&output, 1),
        vec![
            (Beat::new(1, 4).unwrap(), 60),
            (Beat::new(1, 2).unwrap(), 60),
            (Beat::new(3, 4).unwrap(), 60),
            (Beat::ONE, 69),
            (Beat::integer(3), 69)
        ]
    );
}

#[test]
fn jumping_to_row_zero_does_not_pretend_a_phrase_boundary_occurred() {
    let mut song = song();
    let phrase = song.phrases.get_mut(&PhraseId::new(100).unwrap()).unwrap();
    phrase.events.truncate(1);
    phrase.events[0].fx[0] = Some(Fx::Jump { step: 0 });
    let mut playback = Playback::new(snapshot(&song, 0));
    playback.window(Beat::new(1, 8).unwrap()).unwrap();
    let phrase = song.phrases.get_mut(&PhraseId::new(100).unwrap()).unwrap();
    phrase.steps = 8;
    phrase.events[0].note.as_mut().unwrap().pitch = 70;
    playback.adopt(snapshot(&song, 1));
    let output = playback.window(Beat::ONE).unwrap();
    assert!(notes(&output, 1).iter().all(|(_, pitch)| *pitch == 60));
    assert!(playback.pending_revision());
}

#[test]
fn random_scale_edits_wait_for_the_generator_phrase_boundary() {
    let mut song = song();
    let phrase = song.phrases.get_mut(&PhraseId::new(100).unwrap()).unwrap();
    phrase.events.clear();
    phrase.form = PhraseForm::Random {
        seed: 7,
        vary: true,
        density: 100,
        low: 60,
        high: 62,
        velocity_low: 100,
        velocity_high: 100,
        gate: Beat::ONE,
        gate_high: None,
        target: target(),
    };
    song.key = Scale::custom(0, vec![0]).unwrap();
    let mut playback = Playback::new(snapshot(&song, 0));
    playback.window(Beat::new(1, 8).unwrap()).unwrap();
    song.key = Scale::custom(2, vec![2]).unwrap();
    playback.adopt(snapshot(&song, 1));
    let output = playback.window(Beat::new(5, 4).unwrap()).unwrap();
    assert_eq!(
        notes(&output, 1),
        vec![
            (Beat::new(1, 4).unwrap(), 60),
            (Beat::new(1, 2).unwrap(), 60),
            (Beat::new(3, 4).unwrap(), 60),
            (Beat::ONE, 62)
        ]
    );
}

#[test]
fn deleted_occurrence_stops_new_steps_without_dropping_pending_releases() {
    let mut song = song();
    song.phrases
        .get_mut(&PhraseId::new(100).unwrap())
        .unwrap()
        .events[0]
        .note
        .as_mut()
        .unwrap()
        .duration = Beat::integer(2);
    let mut playback = Playback::new(snapshot(&song, 0));
    playback.window(Beat::new(1, 8).unwrap()).unwrap();
    song.tracks[0].slots[0] = None;
    playback.adopt(snapshot(&song, 1));
    let output = playback.window(Beat::new(9, 4).unwrap()).unwrap();
    assert!(notes(&output, 1).is_empty());
    assert_eq!(
        output
            .iter()
            .filter(|event| matches!(event.message, Message::NoteOff { pitch: 60, .. }))
            .count(),
        1
    );
    assert!(playback.finished());
    assert!(!playback.pending_revision());
}

#[test]
fn stopped_lane_resumes_appended_material_at_horizon_without_replaying_prior_slots() {
    let mut song = song();
    song.tracks[0].looped = false;
    let mut playback = Playback::new(snapshot(&song, 0));
    playback.window(Beat::integer(2)).unwrap();
    assert!(playback.finished());
    let mut appended = song.tracks[0].slots[0].as_ref().unwrap().clone();
    appended.id = UseId::new(200).unwrap();
    song.tracks[0].slots.push(Some(appended));
    playback.adopt(snapshot(&song, 1));
    let output = playback.window(Beat::integer(3)).unwrap();
    assert_eq!(notes(&output, 1)[0], (Beat::integer(2), 60));
    assert!(output
        .iter()
        .all(|event| event.owner.chain_use == UseId::new(200).unwrap()));
}

#[test]
fn newly_added_lanes_start_at_horizon_and_empty_slot_gaps_stay_stopped() {
    let mut song = song();
    let mut playback = Playback::new(snapshot(&song, 0));
    playback.window(Beat::new(3, 8).unwrap()).unwrap();
    append_lane(&mut song, 1, phrase(76, 4));
    playback.adopt(snapshot(&song, 1));
    let output = playback.window(Beat::new(7, 8).unwrap()).unwrap();
    assert_eq!(notes(&output, 2)[0], (Beat::new(3, 8).unwrap(), 76));
    let mut stopped = song.clone();
    stopped.tracks[1].slots.insert(0, None);
    let mut playback = Playback::new(snapshot(&stopped, 0));
    playback.window(Beat::ONE).unwrap();
    stopped.name = "Rename only".into();
    stopped.key = Scale::new(0, ScaleKind::Major).unwrap();
    playback.adopt(snapshot(&stopped, 1));
    assert!(notes(&playback.window(Beat::integer(2)).unwrap(), 2).is_empty());
}

#[test]
fn finite_lane_adopts_appended_slots_at_its_boundary_inside_a_large_window() {
    let mut song = song();
    song.tracks[0].looped = false;
    let mut playback = Playback::new(snapshot(&song, 0));
    playback.window(Beat::new(1, 8).unwrap()).unwrap();
    let mut appended = song.tracks[0].slots[0].as_ref().unwrap().clone();
    appended.id = UseId::new(200).unwrap();
    song.tracks[0].slots.push(Some(appended));
    playback.adopt(snapshot(&song, 1));
    let output = playback.window(Beat::integer(3)).unwrap();
    let appended: Vec<_> = output
        .iter()
        .filter(|event| event.owner.chain_use == UseId::new(200).unwrap())
        .cloned()
        .collect();
    assert_eq!(notes(&appended, 1)[0], (Beat::ONE, 60));
    assert!(playback.finished());
}

#[test]
fn disabling_a_loop_at_its_boundary_does_not_play_an_extra_cycle() {
    let mut song = song();
    let mut playback = Playback::new(snapshot(&song, 0));
    playback.window(Beat::new(1, 8).unwrap()).unwrap();
    song.tracks[0].looped = false;
    playback.adopt(snapshot(&song, 1));
    let output = playback.window(Beat::integer(3)).unwrap();
    assert!(notes(&output, 1).iter().all(|(at, _)| *at < Beat::ONE));
    assert!(playback.finished());
}

#[test]
fn tempo_conflicts_use_song_track_order_then_last_fx_slot_and_report_losers() {
    let mut song = song();
    append_lane(&mut song, 1, phrase(70, 4));
    song.tracks[0].id = TrackId::new(20).unwrap();
    song.tracks[1].id = TrackId::new(10).unwrap();
    let first = &mut song
        .phrases
        .get_mut(&PhraseId::new(100).unwrap())
        .unwrap()
        .events[0];
    first.fx = [
        Some(Fx::Tempo {
            tempo: Tempo::new(80_000).unwrap(),
        }),
        None,
        Some(Fx::Tempo {
            tempo: Tempo::new(90_000).unwrap(),
        }),
    ];
    song.phrases
        .get_mut(&PhraseId::new(110).unwrap())
        .unwrap()
        .events[0]
        .fx[0] = Some(Fx::Tempo {
        tempo: Tempo::new(150_000).unwrap(),
    });
    let mut playback = Playback::new(snapshot(&song, 0));
    let output = playback.window(Beat::new(1, 8).unwrap()).unwrap();
    let tempos: Vec<_> = output
        .iter()
        .filter_map(|event| match event.message {
            Message::Tempo { tempo } => Some(tempo),
            _ => None,
        })
        .collect();
    assert_eq!(tempos, vec![Tempo::new(90_000).unwrap()]);
    assert_eq!(playback.tempo_conflicts().len(), 1);
    assert_eq!(
        playback.tempo_conflicts()[0].winner.track,
        TrackId::new(20).unwrap()
    );
    assert_eq!(playback.tempo_conflicts()[0].ignored.len(), 2);
}
