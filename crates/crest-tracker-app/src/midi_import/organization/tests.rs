use super::*;
use crest_tracker::playback::{Playback, PlaybackBudget, Snapshot};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};
use std::sync::Arc;

fn fixture() -> Vec<u8> {
    let midi = |channel: u8, message| TrackEventKind::Midi {
        channel: channel.into(),
        message,
    };
    let mut events = vec![
        (
            0,
            midi(
                0,
                MidiMessage::Controller {
                    controller: 7.into(),
                    value: 99.into(),
                },
            ),
        ),
        (0, midi(0, MidiMessage::ProgramChange { program: 5.into() })),
        (
            0,
            TrackEventKind::Meta(MetaMessage::TimeSignature(3, 2, 24, 8)),
        ),
        // Tempo target is initially the phantom program-0 patch.
        (48, TrackEventKind::Meta(MetaMessage::Tempo(600000.into()))),
        (
            48 * 3 * 8,
            TrackEventKind::Meta(MetaMessage::TimeSignature(7, 3, 24, 8)),
        ),
    ];
    for bar in 0..24 {
        let tick = if bar < 8 {
            bar * 144
        } else {
            8 * 144 + (bar - 8) * 168
        };
        // Four-bar motif with two bars of genuine silence, repeated later.
        if bar % 4 >= 2 {
            continue;
        }
        events.push((
            tick + 3,
            midi(
                0,
                MidiMessage::NoteOn {
                    key: 64.into(),
                    vel: 87.into(),
                },
            ),
        ));
        // Crosses a phrase boundary, with an exact non-grid release.
        events.push((
            tick + 169,
            midi(
                0,
                MidiMessage::NoteOff {
                    key: 64.into(),
                    vel: 31.into(),
                },
            ),
        ));
        events.push((
            tick + 18,
            midi(
                0,
                MidiMessage::Controller {
                    controller: 11.into(),
                    value: 80.into(),
                },
            ),
        ));
        events.push((
            tick + 18,
            midi(
                0,
                MidiMessage::Controller {
                    controller: 11.into(),
                    value: 100.into(),
                },
            ),
        ));
    }
    events.push((
        8 * 144 + 16 * 168,
        TrackEventKind::Meta(MetaMessage::EndOfTrack),
    ));
    events.sort_by_key(|e| e.0);
    let mut previous = 0;
    let track = events
        .into_iter()
        .map(|(tick, kind)| {
            let delta = tick - previous;
            previous = tick;
            TrackEvent {
                delta: delta.into(),
                kind,
            }
        })
        .collect();
    let smf = Smf {
        header: Header::new(Format::SingleTrack, Timing::Metrical(48.into())),
        tracks: vec![track],
    };
    let mut bytes = vec![];
    smf.write_std(&mut bytes).unwrap();
    bytes
}

fn scheduled(song: &Song) -> Vec<(Beat, Target, crest_tracker::playback::Message)> {
    let snapshot = Snapshot::compile(song, 0, PlaybackBudget::default()).unwrap();
    let mut playback = Playback::new(Arc::new(snapshot));
    let mut events = vec![];
    // Compare production playback, including tails crossing shared boundaries.
    for beat in 1..100 {
        events.extend(playback.window(Beat::integer(beat)).unwrap());
    }
    events
        .into_iter()
        .map(|e| (e.at, e.target, e.message))
        .collect()
}

#[test]
fn shared_patterns_rests_and_chains_preserve_meter_and_performance() {
    let bytes = fixture();
    let before = crate::midi_import::parse(&bytes, "Fixture").unwrap();
    let mut plan = default_plan(&before).unwrap();
    plan.tracks[0].phrase_bars = 1;
    plan.tracks[0].name = "Keys".into();
    plan.sections = vec![SectionPlan {
        start_bar: 1,
        name: "Theme".into(),
        reason: "Repeated motif".into(),
    }];
    let after = organize(crate::midi_import::parse(&bytes, "Fixture").unwrap(), &plan).unwrap();
    assert_eq!(before.parts.len(), 2);
    assert_eq!(after.parts.len(), 1);
    assert_eq!(after.parts[0].program, 5);
    assert_eq!(
        crate::midi_plan::performance_digest(&before.song).unwrap(),
        crate::midi_plan::performance_digest(&after.song).unwrap()
    );
    let source = Timeline::read(&before.song).unwrap();
    // Playback of the canonical source removes only never-sounding targets.
    let mut cleaned = before.song.clone();
    let fallback = source
        .lanes
        .iter()
        .flatten()
        .find(|e| e.note.is_some())
        .unwrap()
        .target
        .clone();
    for phrase in cleaned.phrases.values_mut() {
        phrase.events.retain_mut(|e| {
            super::super::timeline::retain_sounding_event(e, &source.sounding, &fallback)
        });
    }
    assert_eq!(scheduled(&cleaned), scheduled(&after.song));
    let stats = statistics(&after.song).unwrap();
    assert!(stats["reused_phrase_placements"].as_u64().unwrap() >= 8);
    assert!(stats["multi_phrase_chains"].as_u64().unwrap() >= 2);
    assert!(stats["reused_chain_placements"].as_u64().unwrap() >= 1);
    assert!(stats["rest_phrases"].as_u64().unwrap() <= 2);
    assert!(after
        .song
        .phrases
        .values()
        .all(|p| [Beat::integer(3), Beat::new(7, 2).unwrap()].contains(&p.length().unwrap())));
    assert_eq!(Timeline::read(&after.song).unwrap().note_count(), 12);
    for chain in after.song.chains.values() {
        if chain
            .phrases
            .iter()
            .all(|usage| after.song.phrases[&usage.phrase].events.is_empty())
        {
            assert_eq!(chain.name, "Rest");
        }
    }
    assert!(after
        .song
        .phrases
        .values()
        .all(|p| p.name.starts_with("Keys") || p.name.starts_with("Rest")));
}

#[test]
fn analysis_reports_meters_repeated_bar_shapes_and_unused_patches() {
    let import = crate::midi_import::parse(&fixture(), "Fixture").unwrap();
    let analysis = inspect(&import).unwrap();
    assert_eq!(analysis["total_bars"], 24);
    assert_eq!(analysis["meters"][1]["start_bar"], 9);
    assert_eq!(analysis["unused_source_patches"], 1);
    let sequence = analysis["tracks"][0]["bar_patterns"].as_array().unwrap();
    assert_eq!(sequence.len(), 24);
    assert_eq!(sequence[0], sequence[4]);
    assert_eq!(sequence[2], 0);
}

#[test]
fn untrusted_arrangements_cannot_omit_duplicate_or_invent_channels_and_bars() {
    let bytes = fixture();
    let source = crate::midi_import::parse(&bytes, "Fixture").unwrap();
    let baseline = default_plan(&source).unwrap();
    let bad: [fn(&mut ArrangementPlan); 9] = [
        |p| p.tracks.clear(),
        |p| p.tracks.push(p.tracks[0].clone()),
        |p| p.tracks[0].channel = 17,
        |p| p.tracks[0].phrase_bars = 3,
        |p| p.chain_bars = u32::MAX,
        |p| p.sections[0].start_bar = 2,
        |p| p.sections[1].start_bar = 1,
        |p| p.sections[1].start_bar = 25,
        |p| p.sections[0].reason.clear(),
    ];
    for mutate in bad {
        let mut plan = baseline.clone();
        mutate(&mut plan);
        assert!(organize(crate::midi_import::parse(&bytes, "Fixture").unwrap(), &plan).is_err());
    }
}

#[test]
fn digest_detects_note_control_meter_and_timing_changes_but_ignores_layout_names() {
    let mut import = crate::midi_import::parse(&fixture(), "Fixture").unwrap();
    let plan = default_plan(&import).unwrap();
    import = organize(import, &plan).unwrap();
    let baseline = crate::midi_plan::performance_digest(&import.song).unwrap();
    let mut renamed = import.song.clone();
    renamed.phrases.values_mut().next().unwrap().name = "Renamed".into();
    assert_eq!(
        baseline,
        crate::midi_plan::performance_digest(&renamed).unwrap()
    );
    let mut changed = import.song.clone();
    changed
        .phrases
        .values_mut()
        .flat_map(|p| &mut p.events)
        .find_map(|e| e.note.as_mut())
        .unwrap()
        .velocity -= 1;
    assert_ne!(
        baseline,
        crate::midi_plan::performance_digest(&changed).unwrap()
    );
    let mut changed = import.song.clone();
    let event = changed
        .phrases
        .values_mut()
        .flat_map(|p| &mut p.events)
        .find(|e| !e.controls.is_empty())
        .unwrap();
    event.controls.clear();
    assert_ne!(
        baseline,
        crate::midi_plan::performance_digest(&changed).unwrap()
    );
}

#[test]
fn independent_channels_use_different_phrase_sizes_without_shifting_notes() {
    let bytes = fixture();
    let mut source = crate::midi_import::parse(&bytes, "Fixture").unwrap();
    let original = &source.song.tracks[0];
    let mut second = original.clone();
    second.id = TrackId::new(2).unwrap();
    for (index, usage) in second.slots.iter_mut().flatten().enumerate() {
        usage.id = UseId::new(100_000 + index as u64).unwrap();
    }
    source.song.tracks[1] = second;
    let before = crate::midi_plan::performance_digest(&source.song).unwrap();
    let mut plan = default_plan(&source).unwrap();
    plan.tracks[0].phrase_bars = 1;
    plan.tracks[1].phrase_bars = 4;
    let after = organize(source, &plan).unwrap();
    assert_eq!(
        before,
        crate::midi_plan::performance_digest(&after.song).unwrap()
    );
    let lanes = Timeline::read(&after.song).unwrap().lanes;
    assert_eq!(lanes[0], lanes[1]);
    let lengths = |lane: usize| {
        let chain = after.song.tracks[lane].slots[0]
            .as_ref()
            .unwrap()
            .chain
            .pattern()
            .unwrap();
        after.song.chains[&chain].phrases.len()
    };
    assert_eq!((lengths(0), lengths(1)), (8, 2));
}

#[test]
fn repeated_bar_with_different_velocity_remains_a_separate_phrase() {
    let bytes = fixture();
    let mut source = crate::midi_import::parse(&bytes, "Fixture").unwrap();
    let first_id = *source.song.phrases.keys().next().unwrap();
    let second_id = *source.song.phrases.keys().nth(1).unwrap();
    let mut repeated = source.song.phrases[&first_id].clone();
    repeated
        .events
        .iter_mut()
        .find_map(|e| e.note.as_mut())
        .unwrap()
        .velocity = 50;
    source.song.phrases.insert(second_id, repeated);
    let plan = default_plan(&source).unwrap();
    let after = organize(source, &plan).unwrap();
    let velocities: BTreeSet<_> = after
        .song
        .phrases
        .values()
        .flat_map(|p| &p.events)
        .filter_map(|e| e.note.as_ref().map(|n| n.velocity))
        .collect();
    assert!(velocities.contains(&50));
    assert!(velocities.contains(&87));
}

#[test]
fn enormous_repeat_counts_fail_before_unbounded_expansion() {
    let mut source = crate::midi_import::parse(&fixture(), "Fixture").unwrap();
    source.song.tracks[0].slots[0].as_mut().unwrap().repeats = u32::MAX;
    assert!(Timeline::read(&source.song)
        .err()
        .unwrap()
        .contains("budget"));
}
