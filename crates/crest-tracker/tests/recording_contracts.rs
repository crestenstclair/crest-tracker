use crest_tracker::composition::*;
use crest_tracker::editing::{Command, Editor, Occurrence};
use crest_tracker::recording::{
    CaptureDiagnostic, CaptureInput, CaptureMessage, CaptureState, Recorder, RecordingError,
};
use crest_tracker::time::Beat;
use std::collections::BTreeSet;

fn setup() -> (Editor, Occurrence, PhraseId) {
    let mut song = Song::default();
    let phrase = PhraseId::new(101).unwrap();
    let chain = ChainId::new(102).unwrap();
    let original = Event::note(
        EventId::new(100).unwrap(),
        Beat::ZERO,
        destination(1),
        Note::new(60, 90, Beat::ONE).unwrap(),
    );
    song.phrases.insert(
        phrase,
        Phrase {
            events: vec![original],
            form: PhraseForm::PianoRoll,
            ..Phrase::default()
        },
    );
    song.chains.insert(
        chain,
        Chain {
            name: "A".into(),
            phrases: vec![PhraseUse {
                id: UseId::new(103).unwrap(),
                phrase,
                repeats: 1,
            }],
        },
    );
    song.tracks[0].slots.push(Some(ChainUse {
        id: UseId::new(104).unwrap(),
        chain,
        repeats: 1,
    }));
    let path = Occurrence {
        track: song.tracks[0].id,
        chain_use: UseId::new(104).unwrap(),
        phrase_use: UseId::new(103).unwrap(),
    };
    (Editor::new(song).unwrap(), path, phrase)
}

fn destination(channel: u8) -> Target {
    Target::Channel {
        channel: Channel::new(channel).unwrap(),
        endpoint: Some("stable-output".into()),
    }
}

fn setup_other_phrase() -> (Editor, Occurrence, PhraseId, PhraseId) {
    let (mut editor, path, recorded) = setup();
    let other = PhraseId::new(200).unwrap();
    editor
        .apply(
            editor.revision(),
            Command::PutPhrase {
                id: other,
                phrase: Phrase::default(),
            },
        )
        .unwrap();
    (editor, path, recorded, other)
}

fn insert_other_note(editor: &mut Editor, other: PhraseId) {
    let event = Event::note(
        EventId::new(editor.song().next_identity().unwrap()).unwrap(),
        Beat::ZERO,
        destination(1),
        Note::new(67, 81, Beat::ONE).unwrap(),
    );
    editor
        .apply(
            editor.revision(),
            Command::PutEvent {
                phrase: other,
                event,
            },
        )
        .unwrap();
}

#[test]
fn other_phrase_edit_rebases_completed_held_and_retired_capture_ids_and_recovery() {
    let (mut editor, path, recorded, other) = setup_other_phrase();
    let mut recorder = running(&editor, path);
    send(
        &mut recorder,
        4,
        1,
        1,
        CaptureMessage::NoteOn {
            pitch: 60,
            velocity: 101,
        },
    );
    send(
        &mut recorder,
        9,
        2,
        1,
        CaptureMessage::NoteOn {
            pitch: 60,
            velocity: 87,
        },
    );
    editor.set_recording_guard(recorder.edit_guard());
    insert_other_note(&mut editor, other);
    recorder.rebase_other_phrase_edits(&editor).unwrap();
    let unrelated = editor.song().phrases[&other].clone();
    let mut recovered = Recorder::recover(recorder.checkpoint(), &editor).unwrap();
    recovered.finish(Beat::integer(8)).unwrap();
    recovered.prepare_commit(&editor).unwrap();
    send(
        &mut recorder,
        5,
        1,
        1,
        CaptureMessage::NoteOff {
            pitch: 60,
            velocity: 45,
        },
    );
    send(
        &mut recorder,
        11,
        2,
        1,
        CaptureMessage::NoteOff {
            pitch: 60,
            velocity: 33,
        },
    );
    recorder.finish(Beat::integer(8)).unwrap();
    recorder.commit(&mut editor).unwrap();
    let phrase = &editor.song().phrases[&recorded];
    assert_eq!(phrase.events.len(), 3);
    assert_eq!(phrase.events[1].note.as_ref().unwrap().velocity, 101);
    assert_eq!(phrase.events[2].note.as_ref().unwrap().release_velocity, 33);
    assert_eq!(editor.song().phrases[&other], unrelated);
    editor.song().validate().unwrap();
    editor.undo().unwrap();
    assert_eq!(editor.song().phrases[&recorded].events.len(), 1);
    assert_eq!(editor.song().phrases[&other], unrelated);
}

#[test]
fn recording_guard_rejects_protected_and_structural_changes_without_history_loss() {
    let (mut editor, path, recorded, other) = setup_other_phrase();
    let recorder = running(&editor, path);
    editor.set_recording_guard(recorder.edit_guard());
    insert_other_note(&mut editor, other);
    editor.undo().unwrap();
    let before = editor.song().clone();
    let revision = editor.revision();
    assert!(
        editor.undo().is_err(),
        "cannot remove an existing phrase definition during capture"
    );
    let mut protected = editor.song().phrases[&recorded].clone();
    protected.events.clear();
    assert!(editor
        .apply(
            revision,
            Command::PutPhrase {
                id: recorded,
                phrase: protected
            }
        )
        .is_err());
    assert!(editor
        .apply(
            revision,
            Command::RenameSong {
                name: "Changed".into()
            }
        )
        .is_err());
    assert_eq!(editor.song(), &before);
    assert_eq!(editor.revision(), revision);
    assert!(
        editor.redo().unwrap(),
        "rejected Undo and edits retain Redo history"
    );
    assert_eq!(editor.song().phrases[&other].events.len(), 1);
}

#[test]
fn independent_undo_redo_rebases_capture_ids_in_both_directions() {
    let (mut editor, path, recorded, other) = setup_other_phrase();
    let mut recorder = finished_raw_take(&editor, path);
    let original_take = recorder.draft().unwrap().events.clone();
    editor.set_recording_guard(recorder.edit_guard());
    insert_other_note(&mut editor, other);
    recorder.rebase_other_phrase_edits(&editor).unwrap();
    let shifted_take = recorder.draft().unwrap().events.clone();
    assert_ne!(shifted_take[0].id, original_take[0].id);

    editor.set_recording_guard(recorder.edit_guard());
    editor.undo().unwrap();
    recorder.rebase_other_phrase_edits(&editor).unwrap();
    assert_eq!(recorder.draft().unwrap().events, original_take);
    Recorder::recover(recorder.checkpoint(), &editor).unwrap();

    editor.set_recording_guard(recorder.edit_guard());
    editor.redo().unwrap();
    recorder.rebase_other_phrase_edits(&editor).unwrap();
    assert_eq!(recorder.draft().unwrap().events, shifted_take);
    Recorder::recover(recorder.checkpoint(), &editor).unwrap();
    let independent = editor.song().clone();
    recorder.commit(&mut editor).unwrap();
    assert_eq!(editor.song().phrases[&recorded].events.len(), 4);
    editor.undo().unwrap();
    assert_eq!(editor.song(), &independent);
    editor.undo().unwrap();
    assert!(editor.song().phrases[&other].events.is_empty());
}

#[test]
fn capture_id_exhaustion_and_failed_rebase_preserve_both_owners() {
    let (mut editor, path, recorded, other) = setup_other_phrase();
    let mut recorder = finished_raw_take(&editor, path);
    editor.set_recording_guard(recorder.edit_guard());
    let before = editor.song().clone();
    let revision = editor.revision();
    let checkpoint = serde_json::to_value(recorder.checkpoint()).unwrap();
    let exhausted_event = Event::note(
        EventId::new(u64::MAX - 2).unwrap(),
        Beat::ZERO,
        destination(1),
        Note::new(67, 81, Beat::ONE).unwrap(),
    );
    let edit = Command::PutEvent {
        phrase: other,
        event: exhausted_event,
    };
    let error = editor.apply(revision, edit.clone()).unwrap_err();
    assert_eq!(
        error,
        DocumentError::Invalid("capture identity range exhausted")
    );
    assert_eq!(editor.song(), &before);
    assert_eq!(editor.revision(), revision);

    // Even a caller without the transaction guard cannot partially rebase a take.
    let mut unguarded = Editor::new(before.clone()).unwrap();
    unguarded.apply(unguarded.revision(), edit).unwrap();
    assert!(recorder.rebase_other_phrase_edits(&unguarded).is_err());
    assert_eq!(
        serde_json::to_value(recorder.checkpoint()).unwrap(),
        checkpoint
    );
    let mut protected = Editor::new(before.clone()).unwrap();
    let mut phrase = before.phrases[&recorded].clone();
    phrase.events.clear();
    protected
        .apply(
            0,
            Command::PutPhrase {
                id: recorded,
                phrase,
            },
        )
        .unwrap();
    assert!(recorder.rebase_other_phrase_edits(&protected).is_err());
    assert_eq!(
        serde_json::to_value(recorder.checkpoint()).unwrap(),
        checkpoint
    );

    insert_other_note(&mut editor, other);
    recorder.rebase_other_phrase_edits(&editor).unwrap();
    recorder.commit(&mut editor).unwrap();
    editor.song().validate().unwrap();
}

#[test]
fn mpe_pending_voice_rebase_preserves_curves_and_recoverable_sustain() {
    use crest_tracker::mpe::{BendRange, MpeProfile, MpeZone};
    let (mut editor, path, recorded, other) = setup_other_phrase();
    let mut recorder = Recorder::default();
    recorder
        .arm_mpe(
            &editor,
            path,
            MpeProfile::new(
                "exact-source".into(),
                MpeZone::lower(2).unwrap(),
                BendRange::new(48, 0).unwrap(),
                BendRange::new(2, 0).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    recorder.request_record(Beat::integer(4)).unwrap();
    recorder.advance(Beat::integer(4));
    send(
        &mut recorder,
        4,
        1,
        2,
        CaptureMessage::NoteOn {
            pitch: 60,
            velocity: 101,
        },
    );
    send(
        &mut recorder,
        17,
        4,
        2,
        CaptureMessage::Control(Control::Bend { value: 8193 }),
    );
    send(
        &mut recorder,
        9,
        2,
        1,
        CaptureMessage::Control(Control::Cc {
            number: 64,
            value: 127,
        }),
    );
    send(
        &mut recorder,
        5,
        1,
        2,
        CaptureMessage::NoteOff {
            pitch: 60,
            velocity: 45,
        },
    );
    editor.set_recording_guard(recorder.edit_guard());
    insert_other_note(&mut editor, other);
    recorder.rebase_other_phrase_edits(&editor).unwrap();
    let mut recovered = Recorder::recover(recorder.checkpoint(), &editor).unwrap();
    recovered.finish(Beat::integer(8)).unwrap();
    recovered.prepare_commit(&editor).unwrap();
    send(
        &mut recorder,
        6,
        1,
        1,
        CaptureMessage::Control(Control::Cc {
            number: 64,
            value: 0,
        }),
    );
    recorder.finish(Beat::integer(8)).unwrap();
    recorder.commit(&mut editor).unwrap();
    let note = editor.song().phrases[&recorded].events[1]
        .note
        .as_ref()
        .unwrap();
    assert_eq!(
        (note.velocity, note.release_velocity, note.duration),
        (101, 45, Beat::ONE)
    );
    assert_eq!(note.expression_duration, Some(Beat::integer(2)));
    assert!(note
        .expression
        .iter()
        .any(|point| point.control == Control::Bend { value: 8193 }));
    assert_eq!(editor.song().phrases[&other].events.len(), 1);
    editor.song().validate().unwrap();
}

fn packet(at: Beat, channel: u8, message: CaptureMessage) -> CaptureInput {
    CaptureInput {
        at,
        source_channel: Channel::new(channel).unwrap(),
        target: destination(channel),
        message,
    }
}

fn running(editor: &Editor, path: Occurrence) -> Recorder {
    let mut recorder = Recorder::default();
    recorder.arm(editor, path).unwrap();
    recorder.request_record(Beat::integer(4)).unwrap();
    recorder.advance(Beat::integer(4));
    recorder
}

fn send(
    recorder: &mut Recorder,
    numerator: u64,
    denominator: u64,
    channel: u8,
    message: CaptureMessage,
) {
    assert!(recorder
        .input(packet(
            Beat::new(numerator, denominator).unwrap(),
            channel,
            message
        ))
        .unwrap());
}

#[test]
fn arm_does_not_record_and_waiting_starts_only_at_the_eligible_boundary() {
    let (editor, path, _) = setup();
    let mut recorder = Recorder::default();
    recorder.arm(&editor, path).unwrap();
    assert_eq!(recorder.state(), &CaptureState::Armed);
    assert!(!recorder
        .input(packet(
            Beat::ONE,
            1,
            CaptureMessage::NoteOn {
                pitch: 60,
                velocity: 101
            }
        ))
        .unwrap());
    assert!(recorder.draft().is_none());
    recorder.request_record(Beat::integer(4)).unwrap();
    recorder.advance(Beat::integer(3));
    assert_eq!(
        recorder.state(),
        &CaptureState::Waiting {
            boundary: Beat::integer(4)
        }
    );
    assert!(!recorder
        .input(packet(
            Beat::integer(3),
            1,
            CaptureMessage::NoteOff {
                pitch: 60,
                velocity: 20
            }
        ))
        .unwrap());
    recorder.advance(Beat::integer(4));
    assert_eq!(
        recorder.state(),
        &CaptureState::Recording {
            started_at: Beat::integer(4)
        }
    );
    assert_eq!(recorder.held_count(), 0);
    assert!(recorder.draft().unwrap().events.is_empty());
}

#[test]
fn external_start_boundaries_use_frozen_armed_length_and_exact_positions() {
    let (mut editor, path, id) = setup();
    let mut phrase = editor.song().phrases[&id].clone();
    phrase.steps = 7;
    editor
        .apply(editor.revision(), Command::PutPhrase { id, phrase })
        .unwrap();
    let length = Beat::new(7, 4).unwrap();
    let mut recorder = Recorder::default();
    assert!(recorder.next_start_boundary(Beat::ZERO).is_err());
    recorder.arm(&editor, path).unwrap();
    let mut changed = editor.song().phrases[&id].clone();
    changed.steps = 32;
    editor
        .apply(
            editor.revision(),
            Command::PutPhrase {
                id,
                phrase: changed,
            },
        )
        .unwrap();
    assert_eq!(
        recorder.next_start_boundary(Beat::ZERO).unwrap(),
        Beat::ZERO
    );
    assert_eq!(recorder.next_start_boundary(Beat::ONE).unwrap(), length);
    assert_eq!(recorder.next_start_boundary(length).unwrap(), length);
    recorder.request_record(length).unwrap();
    let resumed = recorder.next_start_boundary(Beat::integer(2)).unwrap();
    assert_eq!(resumed, Beat::new(7, 2).unwrap());
    recorder.reschedule_waiting(resumed).unwrap();
    recorder.advance(resumed);
    let draft = recorder.draft().unwrap();
    assert_eq!(draft.started_at, resumed);
    assert_eq!(draft.length, length);
    assert_eq!(draft.occurrence, path);
    assert!(recorder.next_start_boundary(resumed).is_err());
}

#[test]
fn raw_controls_and_key_release_survive_sustain_without_quantization() {
    let (mut editor, path, phrase) = setup();
    let original = editor.song().clone();
    let mut recorder = running(&editor, path);
    send(
        &mut recorder,
        17,
        4,
        1,
        CaptureMessage::NoteOn {
            pitch: 61,
            velocity: 101,
        },
    );
    let controls = [
        Control::Bend { value: 8193 },
        Control::Pressure { value: 113 },
        Control::Cc {
            number: 74,
            value: 73,
        },
        Control::PolyPressure {
            pitch: 61,
            value: 117,
        },
        Control::Program { value: 9 },
        Control::Cc {
            number: 64,
            value: 127,
        },
    ];
    for control in controls {
        send(&mut recorder, 9, 2, 1, CaptureMessage::Control(control));
    }
    send(
        &mut recorder,
        5,
        1,
        1,
        CaptureMessage::NoteOff {
            pitch: 61,
            velocity: 37,
        },
    );
    send(
        &mut recorder,
        6,
        1,
        1,
        CaptureMessage::Control(Control::Cc {
            number: 64,
            value: 0,
        }),
    );
    recorder.finish(Beat::integer(8)).unwrap();
    let draft = recorder.draft().unwrap();
    let note_event = draft
        .events
        .iter()
        .find(|event| event.note.is_some())
        .unwrap();
    let note = note_event.note.as_ref().unwrap();
    assert_eq!(note_event.at, Beat::new(1, 4).unwrap());
    assert_eq!(
        (note.pitch, note.velocity, note.release_velocity),
        (61, 101, 37)
    );
    assert_eq!(note.duration, Beat::new(3, 4).unwrap());
    assert!(
        note.expression.is_empty(),
        "ordinary channel controls must remain channel-wide"
    );
    let raw: Vec<_> = draft
        .events
        .iter()
        .flat_map(|event| event.controls.iter().copied())
        .collect();
    assert_eq!(&raw[..6], &controls);
    assert_eq!(
        raw[6],
        Control::Cc {
            number: 64,
            value: 0
        }
    );
    assert_eq!(editor.song(), &original);
    recorder.commit(&mut editor).unwrap();
    assert_eq!(editor.song().phrases[&phrase].events.len(), 9);
    assert!(recorder.draft().is_none());
    assert_eq!(editor.revision(), 1);
    editor.undo().unwrap();
    assert_eq!(editor.song(), &original);
}

#[test]
fn loop_crossings_preserve_unwrapped_durations_and_distinct_overdub_events() {
    let (mut editor, path, phrase) = setup();
    let mut recorder = running(&editor, path);
    send(
        &mut recorder,
        15,
        2,
        1,
        CaptureMessage::NoteOn {
            pitch: 64,
            velocity: 100,
        },
    );
    send(
        &mut recorder,
        10,
        1,
        1,
        CaptureMessage::NoteOff {
            pitch: 64,
            velocity: 11,
        },
    );
    send(
        &mut recorder,
        23,
        2,
        1,
        CaptureMessage::NoteOn {
            pitch: 64,
            velocity: 110,
        },
    );
    recorder.finish(Beat::integer(12)).unwrap();
    let draft = recorder.draft().unwrap();
    assert_eq!(draft.events.len(), 2);
    assert_eq!(draft.events[0].at, Beat::new(7, 2).unwrap());
    assert_eq!(draft.events[1].at, Beat::new(7, 2).unwrap());
    assert_eq!(
        draft.events[0].note.as_ref().unwrap().duration,
        Beat::new(5, 2).unwrap()
    );
    assert_eq!(
        draft.events[1].note.as_ref().unwrap().duration,
        Beat::new(1, 2).unwrap()
    );
    assert_ne!(draft.events[0].id, draft.events[1].id);
    recorder.commit(&mut editor).unwrap();
    let events = &editor.song().phrases[&phrase].events;
    assert_eq!(
        events
            .iter()
            .map(|event| event.id)
            .collect::<BTreeSet<_>>()
            .len(),
        3
    );
}

#[test]
fn repeated_tuple_fifo_absorbs_old_release_without_cutting_replacement() {
    let (editor, path, _) = setup();
    let mut recorder = running(&editor, path);
    send(
        &mut recorder,
        4,
        1,
        1,
        CaptureMessage::NoteOn {
            pitch: 60,
            velocity: 80,
        },
    );
    send(
        &mut recorder,
        5,
        1,
        1,
        CaptureMessage::NoteOn {
            pitch: 60,
            velocity: 120,
        },
    );
    send(
        &mut recorder,
        6,
        1,
        1,
        CaptureMessage::NoteOff {
            pitch: 60,
            velocity: 13,
        },
    );
    assert_eq!(recorder.held_count(), 1);
    send(
        &mut recorder,
        7,
        1,
        1,
        CaptureMessage::NoteOff {
            pitch: 60,
            velocity: 29,
        },
    );
    recorder.finish(Beat::integer(8)).unwrap();
    let draft = recorder.draft().unwrap();
    assert!(matches!(
        draft.diagnostics.as_slice(),
        [CaptureDiagnostic::RepeatedAttack { .. }]
    ));
    let first = draft.events[0].note.as_ref().unwrap();
    let second = draft.events[1].note.as_ref().unwrap();
    assert_eq!((first.duration, first.release_velocity), (Beat::ONE, 13));
    assert_eq!(
        (second.duration, second.release_velocity),
        (Beat::integer(2), 29)
    );
}

#[test]
fn equal_pitch_on_different_channels_keeps_targets_and_durations_separate() {
    let (editor, path, _) = setup();
    let mut recorder = running(&editor, path);
    send(
        &mut recorder,
        4,
        1,
        1,
        CaptureMessage::NoteOn {
            pitch: 60,
            velocity: 91,
        },
    );
    send(
        &mut recorder,
        4,
        1,
        2,
        CaptureMessage::NoteOn {
            pitch: 60,
            velocity: 92,
        },
    );
    send(
        &mut recorder,
        5,
        1,
        1,
        CaptureMessage::NoteOn {
            pitch: 60,
            velocity: 0,
        },
    );
    send(
        &mut recorder,
        6,
        1,
        2,
        CaptureMessage::NoteOff {
            pitch: 60,
            velocity: 42,
        },
    );
    recorder.finish(Beat::integer(8)).unwrap();
    let notes = &recorder.draft().unwrap().events;
    assert_eq!(notes[0].target, destination(1));
    assert_eq!(notes[1].target, destination(2));
    assert_eq!(notes[0].note.as_ref().unwrap().duration, Beat::ONE);
    assert_eq!(notes[1].note.as_ref().unwrap().duration, Beat::integer(2));
}

#[test]
fn stale_commit_preserves_take_and_concurrent_edits() {
    let (mut editor, path, _) = setup();
    let mut recorder = running(&editor, path);
    send(
        &mut recorder,
        4,
        1,
        1,
        CaptureMessage::NoteOn {
            pitch: 60,
            velocity: 100,
        },
    );
    recorder.finish(Beat::integer(8)).unwrap();
    let captured = serde_json::to_value(recorder.draft().unwrap()).unwrap();
    editor
        .apply(
            0,
            Command::RenameSong {
                name: "Concurrent edit".into(),
            },
        )
        .unwrap();
    let changed = editor.song().clone();
    assert!(matches!(
        recorder.commit(&mut editor),
        Err(RecordingError::Document(DocumentError::Stale {
            expected: 0,
            actual: 1
        }))
    ));
    assert_eq!(
        serde_json::to_value(recorder.draft().unwrap()).unwrap(),
        captured
    );
    assert_eq!(editor.song(), &changed);
    assert!(recorder.arm(&editor, path).is_err());
}

#[test]
fn interruption_closes_known_attacks_and_never_rearms_on_more_input() {
    let (mut editor, path, _) = setup();
    let mut recorder = running(&editor, path);
    send(
        &mut recorder,
        4,
        1,
        1,
        CaptureMessage::NoteOn {
            pitch: 60,
            velocity: 100,
        },
    );
    recorder
        .interrupt(Beat::new(9, 2).unwrap(), "input disconnected")
        .unwrap();
    assert!(matches!(recorder.state(), CaptureState::Interrupted { .. }));
    assert_eq!(
        recorder.draft().unwrap().events[0]
            .note
            .as_ref()
            .unwrap()
            .duration,
        Beat::new(1, 2).unwrap()
    );
    assert!(!recorder
        .input(packet(
            Beat::integer(5),
            1,
            CaptureMessage::NoteOn {
                pitch: 67,
                velocity: 100
            }
        ))
        .unwrap());
    assert!(recorder.request_record(Beat::integer(8)).is_err());
    recorder.commit(&mut editor).unwrap();
    assert_eq!(recorder.state(), &CaptureState::Idle);
}

#[test]
fn malformed_input_and_nonboundary_finish_preserve_the_live_take() {
    let (editor, path, _) = setup();
    let mut recorder = running(&editor, path);
    send(
        &mut recorder,
        5,
        1,
        1,
        CaptureMessage::NoteOn {
            pitch: 60,
            velocity: 100,
        },
    );
    assert_eq!(
        recorder.input(packet(
            Beat::integer(4),
            1,
            CaptureMessage::NoteOff {
                pitch: 60,
                velocity: 0
            }
        )),
        Err(RecordingError::Backwards)
    );
    assert!(recorder
        .input(packet(
            Beat::integer(6),
            1,
            CaptureMessage::Control(Control::Bend { value: 16384 })
        ))
        .is_err());
    assert_eq!(
        recorder.finish(Beat::integer(7)),
        Err(RecordingError::NotLoopBoundary)
    );
    assert_eq!(recorder.held_count(), 1);
    recorder.finish(Beat::integer(8)).unwrap();
    assert_eq!(
        recorder.draft().unwrap().events[0]
            .note
            .as_ref()
            .unwrap()
            .duration,
        Beat::integer(3)
    );
}

#[test]
fn capacity_interruption_retains_accepted_data_and_rejects_new_packets() {
    let (editor, path, _) = setup();
    let mut recorder = Recorder::with_capacity(2).unwrap();
    recorder.arm(&editor, path).unwrap();
    recorder.request_record(Beat::integer(4)).unwrap();
    send(
        &mut recorder,
        4,
        1,
        1,
        CaptureMessage::NoteOn {
            pitch: 60,
            velocity: 100,
        },
    );
    send(
        &mut recorder,
        5,
        1,
        1,
        CaptureMessage::Control(Control::Pressure { value: 75 }),
    );
    assert_eq!(
        recorder.input(packet(
            Beat::integer(6),
            1,
            CaptureMessage::NoteOff {
                pitch: 60,
                velocity: 5
            }
        )),
        Err(RecordingError::Capacity)
    );
    assert!(matches!(recorder.state(), CaptureState::Interrupted { .. }));
    let draft = recorder.draft().unwrap();
    assert_eq!(draft.events.len(), 2);
    assert_eq!(
        draft
            .events
            .iter()
            .find_map(|event| event.note.as_ref())
            .unwrap()
            .duration,
        Beat::ONE
    );
    assert_eq!(recorder.held_count(), 0);
}

fn finished_raw_take(editor: &Editor, path: Occurrence) -> Recorder {
    let mut recorder = running(editor, path);
    send(
        &mut recorder,
        17,
        4,
        2,
        CaptureMessage::NoteOn {
            pitch: 61,
            velocity: 101,
        },
    );
    send(
        &mut recorder,
        9,
        2,
        2,
        CaptureMessage::Control(Control::Bend { value: 8193 }),
    );
    send(
        &mut recorder,
        9,
        2,
        2,
        CaptureMessage::Control(Control::Pressure { value: 113 }),
    );
    send(
        &mut recorder,
        19,
        4,
        2,
        CaptureMessage::NoteOff {
            pitch: 61,
            velocity: 37,
        },
    );
    recorder.finish(Beat::integer(8)).unwrap();
    recorder
}

fn phrase_at(song: &Song, occurrence: Occurrence) -> (PhraseId, &Phrase) {
    let track = song
        .tracks
        .iter()
        .find(|track| track.id == occurrence.track)
        .unwrap();
    let chain_use = track
        .slots
        .iter()
        .flatten()
        .find(|usage| usage.id == occurrence.chain_use)
        .unwrap();
    let phrase_use = song.chains[&chain_use.chain]
        .phrases
        .iter()
        .find(|usage| usage.id == occurrence.phrase_use)
        .unwrap();
    (phrase_use.phrase, &song.phrases[&phrase_use.phrase])
}

fn assert_recovered_content(actual: &Phrase, mut baseline: Phrase, draft: &[Event]) {
    baseline.events.extend_from_slice(draft);
    baseline.events.sort_by_key(|event| (event.at, event.id));
    assert_eq!(actual.events.len(), baseline.events.len());
    let old_ids: BTreeSet<_> = baseline.events.iter().map(|event| event.id).collect();
    let new_ids: BTreeSet<_> = actual.events.iter().map(|event| event.id).collect();
    assert!(new_ids.is_disjoint(&old_ids));
    assert_eq!(new_ids.len(), actual.events.len());
    for (expected, copied) in baseline.events.iter_mut().zip(&actual.events) {
        expected.id = copied.id;
    }
    assert_eq!(*actual, baseline, "only event identities may change");
}

#[test]
fn save_as_new_phrase_recovers_stale_take_without_overwriting_edits_and_undoes_once() {
    let (mut editor, path, original_id) = setup();
    let mut baseline = editor.song().phrases[&original_id].clone();
    baseline.name = "Baseline performance".into();
    baseline.seed = 712;
    baseline.events[0]
        .note
        .as_mut()
        .unwrap()
        .expression
        .push(Expression {
            sequence: None,
            offset: Beat::new(1, 3).unwrap(),
            control: Control::Bend { value: 8191 },
        });
    baseline.events[0].fx[2] = Some(Fx::Offset { semitones: -7 });
    editor
        .apply(
            editor.revision(),
            Command::PutPhrase {
                id: original_id,
                phrase: baseline.clone(),
            },
        )
        .unwrap();
    let mut recorder = finished_raw_take(&editor, path);
    let draft = recorder.draft().unwrap().events.clone();
    let mut concurrent = baseline.clone();
    concurrent.name = "Current edit".into();
    concurrent.events[0].note.as_mut().unwrap().pitch = 73;
    concurrent.events.push(Event::note(
        EventId::new(1000).unwrap(),
        Beat::ONE,
        destination(4),
        Note::new(74, 99, Beat::ONE).unwrap(),
    ));
    editor
        .apply(
            editor.revision(),
            Command::PutPhrase {
                id: original_id,
                phrase: concurrent,
            },
        )
        .unwrap();
    let before = editor.song().clone();
    let revision = editor.revision();
    assert!(matches!(
        recorder.commit(&mut editor),
        Err(RecordingError::Document(DocumentError::Stale { .. }))
    ));
    let track = editor.song().tracks[1].id;
    let recovered = recorder.save_as_new_phrase(&mut editor, track, 0).unwrap();
    let (new_id, new_phrase) = phrase_at(editor.song(), recovered);
    assert_ne!(new_id, original_id);
    assert_recovered_content(new_phrase, baseline, &draft);
    assert!(new_phrase.events.iter().all(|event| event.id.get() > 1000));
    assert_eq!(
        editor.song().phrases[&original_id],
        before.phrases[&original_id]
    );
    assert_eq!(editor.revision(), revision + 1);
    assert_eq!(recorder.state(), &CaptureState::Idle);
    assert!(recorder.draft().is_none());
    let after = editor.song().clone();
    editor.undo().unwrap();
    assert_eq!(*editor.song(), before);
    editor.redo().unwrap();
    assert_eq!(*editor.song(), after);
}

#[test]
fn save_as_new_phrase_uses_armed_baseline_when_original_occurrence_moved() {
    let (editor, path, original_id) = setup();
    let baseline = editor.song().phrases[&original_id].clone();
    let mut recorder = finished_raw_take(&editor, path);
    let draft = recorder.draft().unwrap().events.clone();
    let mut relocated = editor.song().clone();
    let usage = relocated.tracks[0].slots[0].take();
    relocated.tracks[1].slots.push(usage);
    relocated.phrases.get_mut(&original_id).unwrap().name = "Moved and edited".into();
    let mut editor = Editor::new(relocated.clone()).unwrap();
    assert_eq!(
        editor.revision(),
        recorder.draft().unwrap().expected_revision
    );
    assert!(recorder.commit(&mut editor).is_err());
    let recovered = recorder
        .save_as_new_phrase(&mut editor, path.track, 0)
        .unwrap();
    assert_ne!(recovered.chain_use, path.chain_use);
    assert_ne!(recovered.phrase_use, path.phrase_use);
    assert_recovered_content(phrase_at(editor.song(), recovered).1, baseline, &draft);
    assert_eq!(editor.song().tracks[1], relocated.tracks[1]);
    assert_eq!(
        editor.song().phrases[&original_id],
        relocated.phrases[&original_id]
    );
    editor.undo().unwrap();
    assert_eq!(*editor.song(), relocated);
}

#[test]
fn save_as_new_phrase_rejects_active_held_and_occupied_destinations_without_consuming_take() {
    let (mut editor, path, _) = setup();
    let mut recorder = running(&editor, path);
    send(
        &mut recorder,
        4,
        1,
        1,
        CaptureMessage::NoteOn {
            pitch: 65,
            velocity: 91,
        },
    );
    let before = editor.song().clone();
    let active = serde_json::to_value(recorder.checkpoint()).unwrap();
    assert!(recorder
        .save_as_new_phrase(&mut editor, path.track, 1)
        .is_err());
    assert_eq!(serde_json::to_value(recorder.checkpoint()).unwrap(), active);
    assert!(recorder
        .interrupt(Beat::integer(4), "zero duration; retain attack")
        .is_err());
    let held = serde_json::to_value(recorder.checkpoint()).unwrap();
    assert!(recorder
        .save_as_new_phrase(&mut editor, path.track, 1)
        .is_err());
    assert_eq!(serde_json::to_value(recorder.checkpoint()).unwrap(), held);
    recorder.finish_at(Beat::new(9, 2).unwrap()).unwrap();
    let finished = serde_json::to_value(recorder.checkpoint()).unwrap();
    for (track, slot) in [
        (path.track, 0),
        (path.track, 2),
        (TrackId::new(999).unwrap(), 0),
    ] {
        assert!(recorder
            .save_as_new_phrase(&mut editor, track, slot)
            .is_err());
        assert_eq!(
            serde_json::to_value(recorder.checkpoint()).unwrap(),
            finished
        );
        assert_eq!(*editor.song(), before);
        assert_eq!(editor.revision(), 0);
        assert!(!editor.can_undo());
    }
    let recovered = recorder
        .save_as_new_phrase(&mut editor, path.track, 1)
        .unwrap();
    assert_eq!(phrase_at(editor.song(), recovered).1.events.len(), 2);
}

#[test]
fn save_as_new_phrase_accepts_closed_interrupted_take_and_retains_identity_exhaustion() {
    let (mut editor, path, _) = setup();
    let mut recorder = running(&editor, path);
    send(
        &mut recorder,
        4,
        1,
        1,
        CaptureMessage::NoteOn {
            pitch: 65,
            velocity: 91,
        },
    );
    recorder
        .interrupt(Beat::new(9, 2).unwrap(), "disconnected")
        .unwrap();
    let checkpoint = serde_json::to_value(recorder.checkpoint()).unwrap();
    let mut exhausted = editor.song().clone();
    exhausted
        .phrases
        .insert(PhraseId::new(u64::MAX - 2).unwrap(), Phrase::default());
    let mut exhausted_editor = Editor::new(exhausted.clone()).unwrap();
    assert!(recorder
        .save_as_new_phrase(&mut exhausted_editor, path.track, 1)
        .is_err());
    assert_eq!(*exhausted_editor.song(), exhausted);
    assert!(!exhausted_editor.can_undo());
    assert_eq!(
        serde_json::to_value(recorder.checkpoint()).unwrap(),
        checkpoint
    );
    assert!(matches!(recorder.state(), CaptureState::Interrupted { .. }));
    recorder
        .save_as_new_phrase(&mut editor, path.track, 1)
        .unwrap();
    assert!(recorder.draft().is_none());
}

#[test]
fn finish_at_preserves_exact_off_grid_cutoff_and_raw_controls() {
    let (mut editor, path, phrase) = setup();
    let mut recorder = running(&editor, path);
    send(
        &mut recorder,
        17,
        4,
        2,
        CaptureMessage::NoteOn {
            pitch: 66,
            velocity: 103,
        },
    );
    send(
        &mut recorder,
        9,
        2,
        2,
        CaptureMessage::Control(Control::Bend { value: 16382 }),
    );
    assert_eq!(
        recorder.finish(Beat::new(19, 4).unwrap()),
        Err(RecordingError::NotLoopBoundary)
    );
    recorder.finish_at(Beat::new(19, 4).unwrap()).unwrap();
    assert_eq!(recorder.state(), &CaptureState::Idle);
    assert_eq!(recorder.held_count(), 0);
    let events = &recorder.draft().unwrap().events;
    let note = events.iter().find(|event| event.note.is_some()).unwrap();
    assert_eq!(note.at, Beat::new(1, 4).unwrap());
    assert_eq!(
        note.note.as_ref().unwrap().duration,
        Beat::new(1, 2).unwrap()
    );
    assert_eq!(note.note.as_ref().unwrap().velocity, 103);
    assert_eq!(events[0].controls, vec![Control::Bend { value: 16382 }]);
    assert_eq!(events[0].at, Beat::new(1, 2).unwrap());
    recorder.commit(&mut editor).unwrap();
    assert_eq!(editor.song().phrases[&phrase].events.len(), 3);
}

#[test]
fn finish_at_failure_retains_all_held_notes_and_state() {
    let (editor, path, _) = setup();
    let mut recorder = running(&editor, path);
    send(
        &mut recorder,
        4,
        1,
        1,
        CaptureMessage::NoteOn {
            pitch: 60,
            velocity: 91,
        },
    );
    send(
        &mut recorder,
        9,
        2,
        1,
        CaptureMessage::NoteOn {
            pitch: 64,
            velocity: 92,
        },
    );
    let original = serde_json::to_value(recorder.checkpoint()).unwrap();
    assert_eq!(
        recorder.finish_at(Beat::integer(4)),
        Err(RecordingError::Backwards)
    );
    assert!(recorder.finish_at(Beat::new(9, 2).unwrap()).is_err());
    assert_eq!(
        serde_json::to_value(recorder.checkpoint()).unwrap(),
        original
    );
    assert_eq!(recorder.held_count(), 2);
    recorder.finish_at(Beat::new(19, 4).unwrap()).unwrap();
    let events = &recorder.draft().unwrap().events;
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].note.as_ref().unwrap().duration,
        Beat::new(3, 4).unwrap()
    );
    assert_eq!(
        events[1].note.as_ref().unwrap().duration,
        Beat::new(1, 4).unwrap()
    );
}
