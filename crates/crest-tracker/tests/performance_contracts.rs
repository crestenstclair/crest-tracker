use crest_linnstrument::{Geometry, MpeOutput, MpeZone};
use crest_performance_core::{
    self as library, BeatTime, Command, DestinationId, Engine, Gesture, Limits, Session, SlotState,
    SurfaceMode,
};
use crest_performance_testkit::{expressive_phrase, CaptureSink};
use crest_tracker::{
    composition::InstrumentId,
    performance::{self, Document, Performance, Route},
    time::Beat,
};

fn document() -> Document {
    Document {
        version: 1,
        session: Session::default(),
        routes: vec![Route {
            destination: DestinationId(1),
            instrument: InstrumentId::new(1).unwrap(),
        }],
    }
}
fn adapter() -> Performance {
    Performance::new(document(), Geometry::Pads200, Limits::default()).unwrap()
}

#[test]
fn schema2_fixture_migrates_losslessly_and_keeps_accepted_playback_trace() {
    // This fixture and its playback came from the previous independent API-2
    // Git consumer, not the implementation performing this migration.
    let legacy: Session =
        serde_json::from_str(include_str!("fixtures/performance-schema2.json")).unwrap();
    let expected: Vec<library::OutputEvent> =
        serde_json::from_str(include_str!("fixtures/performance-schema2-playback.json")).unwrap();
    assert_eq!(legacy.version, 2);
    let mut document = document();
    document.session = legacy.clone();
    let mut performance = Performance::new(document, Geometry::Pads200, Limits::default()).unwrap();
    let migrated = performance.document().unwrap();
    assert_eq!(migrated.session.version, library::SESSION_VERSION);
    assert_eq!(migrated.session.tracks, legacy.tracks);
    assert_eq!(migrated.session.slots, legacy.slots);
    assert_eq!(migrated.session.harmony, legacy.harmony);
    assert_eq!(migrated.clone().migrate().unwrap(), migrated);
    let length = migrated.session.slots[0].clip.as_ref().unwrap().length;
    performance
        .command(Command::Launch {
            slot: 0,
            quantum: BeatTime(0),
        })
        .unwrap();
    performance
        .advance(performance::tracker_time(BeatTime(length.0 * 2 - 1)).unwrap())
        .unwrap();
    performance.command(Command::Stop).unwrap();
    let actual: Vec<_> = performance.drain_output().collect();
    assert_eq!(actual, expected);
    assert_eq!(performance.snapshot().active_voices, 0);
}

#[test]
fn playback_copy_keeps_unfinished_recording_and_live_ownership_untouched() {
    let mut performance = adapter();
    performance
        .command(Command::BeginRecord { slot: 0 })
        .unwrap();
    performance
        .accept_midi(Beat::ZERO, &[0x91, 60, 100])
        .unwrap();
    performance
        .accept_midi(Beat::new(1, 2).unwrap(), &[0x81, 60, 0])
        .unwrap();
    performance.advance(Beat::ONE).unwrap();
    performance.command(Command::FinishRecord).unwrap();
    let committed = performance.document().unwrap();
    performance.drain_output().for_each(drop);
    performance.arm_recording(1).unwrap();
    assert!(performance.playback_document(1).is_err());
    assert_eq!(performance.playback_document(0).unwrap(), committed);
    performance
        .accept_midi(Beat::integer(2), &[0x91, 64, 90])
        .unwrap();
    performance
        .accept_midi(Beat::new(5, 2).unwrap(), &[0xe1, 0, 65])
        .unwrap();
    let before = performance.snapshot();
    assert_eq!(performance.playback_document(0).unwrap(), committed);
    assert!(performance.playback_document(1).is_err());
    assert!(performance.playback_document(usize::MAX).is_err());
    assert!(
        performance.document().is_err(),
        "unfinished take cannot silently be saved"
    );
    assert_eq!(
        serde_json::to_value(performance.snapshot()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    performance
        .accept_midi(Beat::integer(3), &[0x81, 64, 0])
        .unwrap();
    performance.command(Command::FinishRecord).unwrap();
    let saved = performance.document().unwrap();
    assert_eq!(saved.session.slots[0], committed.session.slots[0]);
    let recorded = saved.session.slots[1].clip.as_ref().unwrap();
    assert_eq!(recorded.events.len(), 3);
    assert_eq!(recorded.events[1].at, BeatTime(library::TICKS_PER_BEAT / 2));
    assert!(
        matches!(recorded.events[1].gesture, Gesture::Expression { expression }
        if expression.pitch == 750_000)
    );
}

#[test]
fn first_note_arming_preserves_packet_time_and_initial_expression() {
    let mut tracker = adapter();
    tracker.arm_recording(0).unwrap();
    for bytes in [
        &[0x90, 48, 100][..],
        &[0xe1, 0, 65],
        &[0xd1, 96],
        &[0x91, 60],
    ] {
        tracker.accept_midi(Beat::integer(3), bytes).unwrap();
    }
    assert_eq!(tracker.armed_slot(), Some(0));
    // Realtime may interleave with the split attack. Only its completion starts recording.
    tracker.accept_midi(Beat::integer(4), &[0xf8, 100]).unwrap();
    assert_eq!(tracker.armed_slot(), None);
    assert_eq!(tracker.snapshot().slots[0], SlotState::Recording);
    assert!(tracker.arm_recording(1).is_err());
    tracker
        .accept_midi(Beat::new(9, 2).unwrap(), &[0xe1, 0, 66, 0x81, 60, 0])
        .unwrap();
    tracker.advance(Beat::integer(5)).unwrap();
    tracker.command(Command::FinishRecord).unwrap();
    let document = tracker.document().unwrap();
    let clip = document.session.slots[0].clip.as_ref().unwrap();
    assert_eq!(clip.length, BeatTime(library::TICKS_PER_BEAT));
    assert_eq!(clip.events[0].at, BeatTime(0));
    assert!(
        matches!(clip.events[0].gesture, Gesture::On { expression, .. }
        if expression.pitch == 750_000 && expression.pressure == 49_538)
    );
    assert_eq!(clip.events[1].at, BeatTime(library::TICKS_PER_BEAT / 2));
    assert!(
        matches!(clip.events[1].gesture, Gesture::Expression { expression }
        if expression.pitch == 1_500_000)
    );
    assert!(matches!(clip.events[2].gesture, Gesture::Off { .. }));
    tracker.arm_recording(1).unwrap();
    tracker.command(Command::Stop).unwrap();
    assert_eq!(tracker.armed_slot(), None);
    assert_eq!(tracker.document().unwrap(), document);
}

#[test]
fn tracker_time_conversion_is_exact_and_rejects_unrepresentable_edits() {
    for ticks in [0, 1, 240_001, 730_011, library::TICKS_PER_BEAT, u64::MAX] {
        let beat = performance::tracker_time(BeatTime(ticks)).unwrap();
        assert_eq!(
            performance::performance_time(beat).unwrap(),
            BeatTime(ticks)
        );
    }
    assert!(matches!(
        performance::performance_time(Beat::new(1, 7).unwrap()),
        Err(performance::Error::TimeResolution)
    ));
    let mut adapter = adapter();
    assert!(adapter
        .accept_midi(Beat::new(1, 7).unwrap(), &[0x91, 60, 90])
        .is_err());
    assert_eq!(adapter.snapshot().at, BeatTime(0));
    assert_eq!(adapter.snapshot().active_voices, 0);
}

#[test]
fn same_public_fixture_records_identical_notes_and_expression_in_tracker() {
    let mut standalone =
        Engine::new(library::API_VERSION, Session::default(), Limits::default()).unwrap();
    let mut expected = CaptureSink::default();
    standalone
        .command(Command::BeginRecord { slot: 0 })
        .unwrap();
    expressive_phrase(
        &mut standalone,
        &mut expected,
        &mut MpeOutput::new(DestinationId(1), MpeZone::lower()),
    )
    .unwrap();
    standalone
        .advance(BeatTime(library::TICKS_PER_BEAT))
        .unwrap();
    standalone.command(Command::FinishRecord).unwrap();

    let mut tracker = adapter();
    tracker.command(Command::BeginRecord { slot: 0 }).unwrap();
    let mut actual = vec![];
    // The released shared fixture's inputs, including off-grid times and realtime
    // between channel-pressure bytes. This enters through Tracker's public adapter.
    for (tick, bytes) in [
        (0, [0x91, 60, 100]),
        (0, [0x92, 64, 90]),
        (0, [0x93, 67, 80]),
        (240_001, [0xe1, 0, 65]),
        (250_003, [0xb2, 74, 110]),
        (260_007, [0xd3, 90, 0xf8]),
        (720_009, [0x81, 60, 0]),
        (730_011, [0x82, 64, 0]),
        (740_013, [0x83, 67, 0]),
    ] {
        tracker
            .accept_midi(performance::tracker_time(BeatTime(tick)).unwrap(), &bytes)
            .unwrap();
        actual.extend(tracker.drain_output());
    }
    assert_eq!(actual, expected.events);
    assert_eq!(actual.len(), 9);
    assert!(
        matches!(actual[3].gesture, Gesture::Expression { expression } if expression.pitch == 750_000)
    );
    assert!(
        matches!(actual[4].gesture, Gesture::Expression { expression } if expression.timbre == 56_762)
    );
    assert!(
        matches!(actual[5].gesture, Gesture::Expression { expression } if expression.pressure == 46_442)
    );
    tracker.advance(Beat::ONE).unwrap();
    tracker.command(Command::FinishRecord).unwrap();
    let saved = tracker.document().unwrap();
    assert_eq!(saved.session, standalone.save_session().unwrap());
    let restored: Document = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
    assert_eq!(saved, restored);
    verify_replay_with_live_note(restored, &actual);
}

fn verify_replay_with_live_note(document: Document, recorded: &[library::OutputEvent]) {
    let mut tracker = Performance::new(document, Geometry::Pads200, Limits::default()).unwrap();
    tracker.accept_midi(Beat::ZERO, &[0x91, 72, 90]).unwrap();
    let live = tracker.drain_output().next().unwrap();
    tracker
        .command(Command::Launch {
            slot: 0,
            quantum: BeatTime(0),
        })
        .unwrap();
    tracker
        .advance(performance::tracker_time(BeatTime(740_014)).unwrap())
        .unwrap();
    let played: Vec<_> = tracker.drain_output().collect();
    assert_eq!(played.len(), recorded.len());
    for (actual, expected) in played.iter().zip(recorded) {
        assert_eq!((actual.at, actual.gesture), (expected.at, expected.gesture));
        assert_ne!(actual.voice, live.voice);
    }
    assert_eq!(tracker.snapshot().active_voices, 1);
    tracker.command(Command::Stop).unwrap();
    assert_eq!(tracker.snapshot().active_voices, 0);
    let stopped: Vec<_> = tracker.drain_output().collect();
    assert_eq!(stopped.len(), 1);
    assert_eq!(stopped[0].voice, live.voice);
}

#[test]
fn controller_modes_use_library_coordinates_and_keep_feedback_separate() {
    let mut tracker = adapter();
    tracker
        .command(Command::Generate {
            slot: 0,
            settings: library::PatternSettings::default(),
        })
        .unwrap();
    let configuration = tracker
        .command(Command::SetMode(SurfaceMode::Clips))
        .unwrap();
    assert!(!configuration.is_empty());
    assert_eq!(tracker.drain_output().count(), 0);
    tracker.launch_quantum = BeatTime(library::TICKS_PER_BEAT);
    tracker
        .accept_midi(Beat::new(1, 4).unwrap(), &[0x90, 1, 127])
        .unwrap();
    assert_eq!(tracker.snapshot().slots[0], SlotState::Queued);
    assert_eq!(tracker.drain_output().count(), 0);
    let leds = tracker.led_commands(16).unwrap();
    assert!(leds.iter().any(|m| m.bytes() == [0xb0, 22, 2]));
    tracker.advance(Beat::ONE).unwrap();
    let attack = tracker.drain_output().next().unwrap();
    assert_eq!(attack.at, BeatTime(library::TICKS_PER_BEAT));
    assert!(matches!(attack.gesture, Gesture::On { .. }));
    assert_eq!(
        tracker.instrument(attack.destination).unwrap(),
        InstrumentId::new(1).unwrap()
    );
    assert_eq!(tracker.snapshot().slots[0], SlotState::Playing);
    let leds = tracker.led_commands(16).unwrap();
    assert!(leds.iter().any(|m| m.bytes() == [0xb0, 22, 3]));
    tracker
        .accept_midi(Beat::ONE, &[0x90, 1, 0, 1, 127])
        .unwrap();
    assert_eq!(tracker.snapshot().slots[0], SlotState::Recorded);
    assert_eq!(tracker.snapshot().active_voices, 0);
}

#[test]
fn unknown_versions_and_ambiguous_bindings_fail_before_engine_creation() {
    let mut candidate = document();
    candidate.version = 2;
    assert!(Performance::new(candidate, Geometry::Pads200, Limits::default()).is_err());
    let mut candidate = document();
    candidate.routes.push(candidate.routes[0].clone());
    assert!(Performance::new(candidate, Geometry::Pads200, Limits::default()).is_err());
    let mut candidate = document();
    candidate.routes.clear();
    assert!(Performance::new(candidate, Geometry::Pads200, Limits::default()).is_err());
    let mut legacy = document();
    legacy.session.version = 1;
    assert_eq!(
        Performance::new(legacy, Geometry::Pads200, Limits::default())
            .unwrap()
            .document()
            .unwrap()
            .session
            .version,
        library::SESSION_VERSION
    );
}

#[test]
fn retrospective_capture_keeps_live_input_and_rejects_armed_take_changes() {
    let mut tracker = adapter();
    tracker.arm_recording(0).unwrap();
    assert!(tracker
        .capture_recent_at(0, BeatTime(10), BeatTime(10))
        .is_err());
    assert!(tracker.command(Command::Undo).is_err());
    assert_eq!(tracker.armed_slot(), Some(0));
    tracker.command(Command::CancelRecord).unwrap();
    tracker.command(Command::BeginRecord { slot: 1 }).unwrap();
    assert!(tracker
        .capture_recent_at(0, BeatTime(10), BeatTime(10))
        .is_err());
    tracker.command(Command::CancelRecord).unwrap();
    tracker
        .accept_midi(Beat::integer(1), &[0x91, 60, 100])
        .unwrap();
    tracker
        .accept_midi(Beat::new(3, 2).unwrap(), &[0xe1, 0, 65])
        .unwrap();
    let before = tracker.snapshot();
    tracker
        .capture_recent_at(
            0,
            BeatTime(4 * library::TICKS_PER_BEAT),
            BeatTime(4 * library::TICKS_PER_BEAT),
        )
        .unwrap();
    assert_eq!(tracker.snapshot().at, before.at);
    assert_eq!(tracker.snapshot().active_voices, 1);
    assert_eq!(tracker.drain_output().count(), 2);
    let captured = tracker.document().unwrap();
    let notes = captured.session.slots[0]
        .clip
        .as_ref()
        .unwrap()
        .notes()
        .unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(
        (notes[0].start, notes[0].duration),
        (
            BeatTime(library::TICKS_PER_BEAT),
            BeatTime(3 * library::TICKS_PER_BEAT)
        )
    );
    assert_eq!(notes[0].expression.points[1].value.pitch, 750_000);
    tracker
        .accept_midi(Beat::integer(2), &[0x81, 60, 17])
        .unwrap();
    assert_eq!(tracker.document().unwrap(), captured);
    tracker.command(Command::Undo).unwrap();
    assert!(tracker.clip(0).unwrap().is_none());
    assert_eq!(tracker.snapshot().active_voices, 0);
}

#[test]
fn overdub_retains_closing_expression_and_zero_duration_notes_across_project_reopen() {
    let mut tracker = adapter();
    tracker.command(Command::BeginRecord { slot: 0 }).unwrap();
    for (tick, bytes) in [
        (0, &[0x91, 60, 96][..]),
        (20, &[0xe1, 0, 65, 0x81, 60, 7]),
        (30, &[0x92, 64, 90, 0xd2, 80, 0x82, 64, 8]),
    ] {
        tracker
            .accept_midi(performance::tracker_time(BeatTime(tick)).unwrap(), bytes)
            .unwrap();
        tracker.drain_output().for_each(drop);
    }
    tracker
        .advance(performance::tracker_time(BeatTime(40)).unwrap())
        .unwrap();
    tracker.command(Command::FinishRecord).unwrap();
    let original = tracker.document().unwrap().session.slots[0]
        .clip
        .clone()
        .unwrap();
    assert_eq!(original.events.len(), 6);
    tracker.command(Command::BeginOverdub { slot: 0 }).unwrap();
    for (tick, bytes) in [
        (45, &[0x93, 72, 90][..]),
        (55, &[0xb3, 74, 110, 0x83, 72, 9]),
    ] {
        tracker
            .accept_midi(performance::tracker_time(BeatTime(tick)).unwrap(), bytes)
            .unwrap();
        tracker.drain_output().for_each(drop);
    }
    tracker
        .advance(performance::tracker_time(BeatTime(61)).unwrap())
        .unwrap();
    tracker.command(Command::FinishRecord).unwrap();
    tracker.command(Command::Stop).unwrap();
    let saved = tracker.document().unwrap();
    let merged = saved.session.slots[0].clip.as_ref().unwrap();
    assert_eq!(merged.length, BeatTime(40));
    assert_eq!(merged.events.len(), 9);
    let old: Vec<_> = merged
        .events
        .iter()
        .filter(|event| original.events.iter().any(|prior| prior.note == event.note))
        .copied()
        .collect();
    assert_eq!(old, original.events);
    let added = merged
        .notes()
        .unwrap()
        .into_iter()
        .find(|note| note.key == 72)
        .unwrap();
    assert_eq!(
        (added.key, added.start, added.duration),
        (72, BeatTime(5), BeatTime(10))
    );
    assert_eq!(added.expression.points[1].offset, BeatTime(10));
    assert_eq!(added.expression.points[1].value.timbre, 56_762);
    let reopened: Document = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
    let restored = Performance::new(reopened, Geometry::Pads200, Limits::default()).unwrap();
    assert_eq!(restored.document().unwrap(), saved);
    tracker.command(Command::Undo).unwrap();
    assert_eq!(
        tracker.document().unwrap().session.slots[0].clip,
        Some(original)
    );
}

#[test]
fn shortened_note_releases_before_zero_duration_attack_after_project_reopen() {
    let mut tracker = adapter();
    tracker.command(Command::BeginRecord { slot: 0 }).unwrap();
    for (tick, bytes) in [
        (0, &[0x91, 60, 96][..]),
        (100, &[0xd1, 50]),
        (200, &[0x92, 64, 90, 0xd2, 80, 0x82, 64, 8]),
        (300, &[0x81, 60, 7]),
    ] {
        tracker
            .accept_midi(performance::tracker_time(BeatTime(tick)).unwrap(), bytes)
            .unwrap();
        tracker.drain_output().for_each(drop);
    }
    tracker
        .advance(performance::tracker_time(BeatTime(400)).unwrap())
        .unwrap();
    tracker.command(Command::FinishRecord).unwrap();
    let original = tracker.document().unwrap().session.slots[0]
        .clip
        .clone()
        .unwrap();
    let notes = original.notes().unwrap();
    let held = notes.iter().find(|note| note.key == 60).unwrap();
    tracker
        .command(Command::EditNote {
            slot: 0,
            note: held.id,
            edit: library::NoteEdit {
                start: BeatTime(0),
                duration: BeatTime(200),
                key: 60,
            },
        })
        .unwrap();
    let saved = tracker.document().unwrap();
    let edited = saved.session.slots[0].clip.as_ref().unwrap();
    assert_eq!(edited.events[2].gesture, Gesture::Off { velocity: 7 });
    assert!(matches!(
        edited.events[3].gesture,
        Gesture::On { key: 64, .. }
    ));
    assert_eq!(edited.notes().unwrap()[1], notes[1]);
    tracker.command(Command::Undo).unwrap();
    assert_eq!(
        tracker.document().unwrap().session.slots[0].clip.as_ref(),
        Some(&original)
    );
    let reopened: Document = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
    let mut playback = Performance::new(
        reopened,
        Geometry::Pads200,
        Limits {
            voices: 1,
            output_events: 16,
            recorded_events: 16,
        },
    )
    .unwrap();
    playback
        .command(Command::Launch {
            slot: 0,
            quantum: BeatTime(0),
        })
        .unwrap();
    playback
        .advance(performance::tracker_time(BeatTime(799)).unwrap())
        .unwrap();
    let events: Vec<_> = playback.drain_output().collect();
    assert_eq!(events.len(), 12);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.gesture, Gesture::On { .. }))
            .count(),
        4
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.gesture, Gesture::Off { .. }))
            .count(),
        4
    );
    assert_eq!(playback.snapshot().active_voices, 0);
}
