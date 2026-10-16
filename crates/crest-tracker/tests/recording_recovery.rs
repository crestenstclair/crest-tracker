use crest_tracker::composition::*;
use crest_tracker::editing::{Command, Editor, Occurrence};
use crest_tracker::recording::{
    CaptureInput, CaptureMessage, CaptureState, Recorder, RecordingCheckpoint, RecordingError,
};
use crest_tracker::time::Beat;
use serde_json::{json, Value};

fn setup() -> (Editor, Occurrence, PhraseId) {
    let mut editor = Editor::new(Song::default()).unwrap();
    let track = editor.song().tracks[0].id;
    editor
        .apply(0, Command::AddPhrase { track, slot: 0 })
        .unwrap();
    let chain_use = editor.song().tracks[0].slots[0].as_ref().unwrap();
    let phrase_use = &editor.song().chains[&chain_use.chain].phrases[0];
    let path = Occurrence {
        track,
        chain_use: chain_use.id,
        phrase_use: phrase_use.id,
    };
    let phrase = phrase_use.phrase;
    (editor, path, phrase)
}

fn input(at: Beat, channel: u8, message: CaptureMessage) -> CaptureInput {
    let channel = Channel::new(channel).unwrap();
    CaptureInput {
        at,
        source_channel: channel,
        target: Target::Channel {
            channel,
            endpoint: Some("stable-output".into()),
        },
        message,
    }
}

fn capturing(editor: &Editor, occurrence: Occurrence) -> Recorder {
    let mut recorder = Recorder::with_capacity(64).unwrap();
    recorder.arm(editor, occurrence).unwrap();
    recorder.request_record(Beat::integer(4)).unwrap();
    for (numerator, denominator, channel, message) in [
        (
            17,
            4,
            1,
            CaptureMessage::NoteOn {
                pitch: 60,
                velocity: 101,
            },
        ),
        (
            9,
            2,
            1,
            CaptureMessage::NoteOn {
                pitch: 60,
                velocity: 102,
            },
        ),
        (
            5,
            1,
            1,
            CaptureMessage::NoteOn {
                pitch: 60,
                velocity: 103,
            },
        ),
        (
            51,
            10,
            1,
            CaptureMessage::NoteOff {
                pitch: 60,
                velocity: 27,
            },
        ),
        (
            21,
            4,
            2,
            CaptureMessage::NoteOn {
                pitch: 64,
                velocity: 99,
            },
        ),
        (
            11,
            2,
            1,
            CaptureMessage::Control(Control::Bend { value: 12_345 }),
        ),
        (
            23,
            4,
            2,
            CaptureMessage::Control(Control::Pressure { value: 67 }),
        ),
    ] {
        assert!(recorder
            .input(input(
                Beat::new(numerator, denominator).unwrap(),
                channel,
                message
            ))
            .unwrap());
    }
    recorder
}

fn roundtrip(recorder: &Recorder) -> RecordingCheckpoint {
    let bytes = serde_json::to_vec(&recorder.checkpoint()).unwrap();
    let checkpoint = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(serde_json::to_vec(&checkpoint).unwrap(), bytes);
    checkpoint
}

#[test]
fn held_fifo_and_raw_draft_survive_restart_then_finalize_in_one_undoable_commit() {
    let (editor, occurrence, phrase) = setup();
    let recorder = capturing(&editor, occurrence);
    let baseline = editor.song().clone();
    let original = serde_json::to_value(recorder.checkpoint()).unwrap();
    assert_eq!(recorder.held_count(), 2);
    assert_eq!(original["held"][0]["entries"].as_array().unwrap().len(), 2);
    assert!(original["held"][0]["entries"][0].get("retired").is_some());
    let mut reopened = Editor::new(baseline.clone()).unwrap();
    assert_ne!(editor.revision(), reopened.revision());
    let mut recovered = Recorder::recover(roundtrip(&recorder), &reopened).unwrap();
    assert!(matches!(
        recovered.state(),
        CaptureState::Interrupted { .. }
    ));
    assert_eq!(recovered.held_count(), 2);
    assert_eq!(
        recovered.draft().unwrap().expected_revision,
        reopened.revision()
    );
    let restored = serde_json::to_value(recovered.checkpoint()).unwrap();
    for key in ["held", "next_identity", "last_input", "packets", "capacity"] {
        assert_eq!(restored[key], original[key], "{key}");
    }
    assert_eq!(restored["draft"]["events"], original["draft"]["events"]);
    assert_eq!(
        restored["draft"]["diagnostics"],
        original["draft"]["diagnostics"]
    );
    assert!(recovered.request_record(Beat::integer(8)).is_err());
    assert!(recovered.arm(&reopened, occurrence).is_err());
    recovered.advance(Beat::integer(8));
    assert!(!recovered
        .input(input(
            Beat::integer(6),
            1,
            CaptureMessage::NoteOff {
                pitch: 60,
                velocity: 127
            }
        ))
        .unwrap());
    assert_eq!(recovered.held_count(), 2);
    assert!(recovered.prepare_commit(&reopened).is_err());
    assert_eq!(
        recovered.finish(Beat::integer(7)),
        Err(RecordingError::NotLoopBoundary)
    );
    assert_eq!(recovered.held_count(), 2);
    recovered.finish(Beat::integer(8)).unwrap();
    let draft = recovered.draft().unwrap();
    assert_eq!(draft.events.len(), 6);
    let first = draft
        .events
        .iter()
        .find_map(|event| event.note.as_ref().filter(|note| note.velocity == 101))
        .unwrap();
    assert_eq!(first.release_velocity, 27);
    let held = draft
        .events
        .iter()
        .find_map(|event| event.note.as_ref().filter(|note| note.velocity == 103))
        .unwrap();
    assert_eq!(held.duration, Beat::integer(3));
    assert!(draft
        .events
        .iter()
        .any(|event| event.controls == [Control::Bend { value: 12_345 }]));
    assert!(draft
        .events
        .iter()
        .any(|event| event.controls == [Control::Pressure { value: 67 }]));
    // A finalized checkpoint remains recoverable even when close time is later
    // than the last physical packet, and still never resumes automatically.
    recovered = Recorder::recover(roundtrip(&recovered), &reopened).unwrap();
    assert_eq!(recovered.held_count(), 0);
    assert!(recovered.commit(&mut reopened).unwrap());
    assert_eq!(reopened.song().phrases[&phrase].events.len(), 6);
    assert_eq!(reopened.revision(), 1);
    assert!(reopened.undo().unwrap());
    assert_eq!(reopened.song(), &baseline);
    assert!(reopened.redo().unwrap());
    assert_eq!(reopened.song().phrases[&phrase].events.len(), 6);
}

#[test]
fn changed_song_is_rejected_even_when_reopened_revision_matches() {
    let (editor, occurrence, _) = setup();
    let recorder = capturing(&editor, occurrence);
    let mut changed = editor.song().clone();
    changed.name = "Changed baseline".into();
    let reopened = Editor::new(changed.clone()).unwrap();
    assert!(matches!(
        Recorder::recover(roundtrip(&recorder), &reopened),
        Err(RecordingError::BaselineChanged)
    ));
    assert_eq!(reopened.song(), &changed);
    assert_eq!(reopened.revision(), 0);
    assert!(!reopened.can_undo());
    assert_eq!(recorder.held_count(), 2);
}

#[test]
fn malformed_checkpoints_never_change_live_document_or_recorder() {
    let (editor, occurrence, _) = setup();
    let recorder = capturing(&editor, occurrence);
    let original = serde_json::to_value(recorder.checkpoint()).unwrap();
    let baseline = editor.song().clone();
    let reject = |label: &str, mutate: fn(&mut Value)| {
        let mut value = original.clone();
        mutate(&mut value);
        // Decoding may reject invalid canonical values before recovery validation.
        if let Ok(checkpoint) = serde_json::from_value::<RecordingCheckpoint>(value) {
            assert!(Recorder::recover(checkpoint, &editor).is_err(), "{label}");
        }
        assert_eq!(editor.song(), &baseline);
        assert_eq!(
            serde_json::to_value(recorder.checkpoint()).unwrap(),
            original
        );
    };
    reject("version", |v| v["version"] = json!(2));
    reject("zero capacity", |v| v["capacity"] = json!(0));
    reject("capacity exceeded", |v| v["capacity"] = json!(1));
    reject("lost packets", |v| v["packets"] = json!(0));
    reject("invented packet", |v| v["packets"] = json!(8));
    reject("identity overlaps baseline", |v| {
        v["next_identity"] = json!(1)
    });
    reject("identity gap", |v| {
        v["next_identity"] = json!(v["next_identity"].as_u64().unwrap() + 1)
    });
    reject("missing occurrence", |v| {
        v["setup"]["occurrence"]["phrase_use"] = json!(999_999)
    });
    reject("revision mismatch", |v| {
        v["draft"]["expected_revision"] = json!(999)
    });
    reject("length mismatch", |v| {
        v["draft"]["length"] = json!({"numerator": 1, "denominator": 1})
    });
    reject("missing input time", |v| v["last_input"] = Value::Null);
    reject("capture start mismatch", |v| {
        v["state"]["started_at"] = json!({"numerator": 0, "denominator": 1})
    });
    reject("finished with held notes", |v| {
        v["state"] = json!({"state":"idle"})
    });
    reject("invalid pitch", |v| v["held"][0]["pitch"] = json!(128));
    reject("invalid velocity", |v| {
        v["held"][0]["entries"][1]["active"]["velocity"] = json!(0)
    });
    reject("future attack", |v| {
        v["held"][0]["entries"][1]["active"]["at"] = json!({"numerator": 100, "denominator": 1})
    });
    reject("duplicate attack ID", |v| {
        v["held"][0]["entries"][1]["active"]["id"] = v["draft"]["events"][0]["id"].clone()
    });
    reject("retired missing event", |v| {
        v["held"][0]["entries"][0]["retired"] = json!(999_999)
    });
    reject("retired control reference", |v| {
        v["held"][0]["entries"][0]["retired"] = v["draft"]["events"][2]["id"].clone()
    });
    reject("multiple active FIFO entries", |v| {
        v["held"][0]["entries"][0] = v["held"][0]["entries"][1].clone()
    });
    reject("duplicate queue", |v| {
        let queue = v["held"][0].clone();
        v["held"].as_array_mut().unwrap().push(queue);
    });
    reject("duplicate captured event", |v| {
        let event = v["draft"]["events"][0].clone();
        v["draft"]["events"].as_array_mut().unwrap().push(event);
    });
    reject("bad control value", |v| {
        v["draft"]["events"][2]["controls"][0]["value"] = json!(16_384)
    });
    reject("zero duration", |v| {
        v["draft"]["events"][0]["note"]["duration"] = json!({"numerator":0,"denominator":1})
    });
    reject("missing retirement diagnostic", |v| {
        v["draft"]["diagnostics"].as_array_mut().unwrap().pop();
    });
    reject("changed retirement duration", |v| {
        v["draft"]["events"][1]["note"]["duration"] = json!({"numerator":1,"denominator":1})
    });
}

#[test]
fn armed_waiting_and_interrupted_setups_restore_stopped_without_inventing_a_take() {
    let (editor, occurrence, _) = setup();
    let mut recorder = Recorder::with_capacity(3).unwrap();
    recorder.arm(&editor, occurrence).unwrap();
    for waiting in [false, true] {
        if waiting {
            recorder.request_record(Beat::integer(4)).unwrap();
        }
        let mut recovered = Recorder::recover(roundtrip(&recorder), &editor).unwrap();
        assert!(matches!(
            recovered.state(),
            CaptureState::Interrupted { .. }
        ));
        assert!(recovered.draft().is_none());
        assert!(recovered.request_record(Beat::integer(4)).is_err());
        assert!(recovered.finish(Beat::integer(8)).is_err());
        recovered = Recorder::recover(roundtrip(&recovered), &editor).unwrap();
        recovered.discard();
        recovered.arm(&editor, occurrence).unwrap();
    }
}

#[test]
fn capacity_interrupt_with_unclosable_attack_can_recover_and_finalize_later() {
    let (editor, occurrence, _) = setup();
    let mut recorder = Recorder::with_capacity(1).unwrap();
    recorder.arm(&editor, occurrence).unwrap();
    recorder.request_record(Beat::integer(4)).unwrap();
    recorder
        .input(input(
            Beat::integer(4),
            1,
            CaptureMessage::NoteOn {
                pitch: 60,
                velocity: 100,
            },
        ))
        .unwrap();
    assert_eq!(
        recorder.input(input(
            Beat::integer(5),
            1,
            CaptureMessage::Control(Control::Pressure { value: 90 })
        )),
        Err(RecordingError::Capacity)
    );
    assert!(matches!(recorder.state(), CaptureState::Interrupted { .. }));
    assert_eq!(recorder.held_count(), 1);
    assert!(recorder.draft().unwrap().events.is_empty());
    let mut recovered = Recorder::recover(roundtrip(&recorder), &editor).unwrap();
    assert_eq!(recovered.held_count(), 1);
    let before = serde_json::to_value(recovered.checkpoint()).unwrap();
    assert!(recovered.finish(Beat::integer(4)).is_err());
    assert_eq!(
        serde_json::to_value(recovered.checkpoint()).unwrap(),
        before
    );
    recovered.finish(Beat::integer(8)).unwrap();
    assert_eq!(
        recovered.draft().unwrap().events[0]
            .note
            .as_ref()
            .unwrap()
            .duration,
        Beat::integer(4)
    );
}
