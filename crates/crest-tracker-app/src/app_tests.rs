use super::*;
use crest_tracker::harmony::{Scale, Spelling};

/// Headless cost breakdown only; native feedback remains the release gate.
#[test]
#[ignore = "manual release-mode projection cost witness"]
fn dense_projection_cost() {
    use std::{hint::black_box, time::Instant};
    fn measure<T>(label: &str, mut operation: impl FnMut() -> T) {
        let started = Instant::now();
        for _ in 0..50 {
            black_box(operation());
        }
        eprintln!(
            "{label}: {:.3} ms/call",
            started.elapsed().as_secs_f64() * 20.0
        );
    }
    for count in [130, 8193] {
        let mut app = with_phrase();
        let (id, source) = app.selected_phrase().unwrap();
        let mut phrase = source.clone();
        phrase.form = PhraseForm::PianoRoll;
        phrase.steps = count;
        phrase.events = (0..count)
            .map(|index| {
                Event::note(
                    EventId::new(1000 + u64::from(index)).unwrap(),
                    Beat::new(u64::from(index), 4).unwrap(),
                    Target::instrument(InstrumentId::new(17).unwrap()),
                    Note::new(48 + (index % 24) as u8, 88, Beat::new(1, 8).unwrap()).unwrap(),
                )
            })
            .collect();
        app.command(Command::PutPhrase { id, phrase }).unwrap();
        app.editor.mark_saved(app.editor.song().clone());
        eprintln!("{count} selected Piano notes");
        measure("grid", || app.grid());
        measure("identity", || app.editor.song().next_identity());
        measure("dirty", || app.dirty());
        measure("projection", || app.projection());
        let projection = app.projection();
        measure("wire/clone", || {
            crate::webview_wire::encode(projection.clone())
        });
        let wire = crate::webview_wire::encode(projection);
        measure("serialize", || serde_json::to_vec(&wire).unwrap());
        eprintln!("wire bytes: {}", serde_json::to_vec(&wire).unwrap().len());
        let initial = app.browser_projection();
        app.queue(
            json!({"kind":"phrase_projection_ack", "token":initial["phraseSource"]["token"]}),
        );
        measure("acknowledged browser projection", || {
            app.browser_projection()
        });
        let compact = app.browser_projection();
        measure("acknowledged wire/clone", || {
            crate::webview_wire::encode(compact.clone())
        });
        eprintln!(
            "acknowledged wire bytes: {}",
            serde_json::to_vec(&crate::webview_wire::encode(compact))
                .unwrap()
                .len()
        );
    }
}

fn with_instrument() -> App {
    let mut app = App::new();
    app.command(Command::PutInstrument {
        id: InstrumentId::new(17).unwrap(),
        instrument: InstrumentUse {
            name: "Lead".into(),
            resource: None,
        },
    })
    .unwrap();
    app
}

fn with_phrase() -> App {
    let mut app = with_instrument();
    app.apply(Request::Confirm).unwrap();
    app.apply(Request::Confirm).unwrap();
    assert_eq!(app.view, View::Phrase);
    app
}

#[test]
fn delayed_form_edits_cannot_mutate_a_replacement_song() {
    let mut app = App::new();
    let old_revision = app.editor.revision();
    let replacement = Song {
        name: "Replacement".into(),
        ..Song::default()
    };
    app.replace_song(replacement.clone()).unwrap();
    assert!(matches!(
        app.apply(Request::Edit {
            revision: old_revision,
            command: Box::new(Command::RenameSong {
                name: "Delayed edit".into()
            }),
        }),
        Err(DocumentError::Stale { .. })
    ));
    assert_eq!(app.editor.song(), &replacement);
    assert!(!app.editor.dirty());
    assert!(!app.editor.can_undo());
    let next_revision = app.editor.revision();
    app.replace_song(replacement.clone()).unwrap();
    assert!(app.editor.revision() > next_revision);
    assert_eq!(app.projection()["revision"], app.editor.revision());
}

#[test]
fn playback_fault_survives_navigation_and_invalid_replacement() {
    let mut app = with_phrase();
    let failure = "Sound rejected a note/control or reached its voice budget; transport stopped";
    app.playback_fault = Some(failure.into());
    app.apply(Request::Move {
        horizontal: 0,
        vertical: 1,
    })
    .unwrap();
    app.apply(Request::View { view: View::Song }).unwrap();
    assert_eq!(app.projection()["playbackFault"], failure);
    let mut invalid = app.editor.song().clone();
    invalid.name.clear();
    assert!(app.replace_song(invalid).is_err());
    assert_eq!(app.playback_fault.as_deref(), Some(failure));
    app.replace_song(Song::default()).unwrap();
    assert!(app.playback_fault.is_none());
}

#[test]
fn catalog_payload_does_not_grow_with_unselected_phrase_content() {
    let mut app = with_phrase();
    let selected = app.selected_phrase().unwrap().0;
    let other = PhraseId::new(500).unwrap();
    let mut phrase = Phrase {
        name: "Unselected performance".into(),
        form: PhraseForm::PianoRoll,
        ..Phrase::default()
    };
    app.command(Command::PutPhrase {
        id: other,
        phrase: phrase.clone(),
    })
    .unwrap();
    let before = app.projection();
    // Dense overlapping notes are valid. They must remain editable when chosen,
    // without being copied into the catalog for every unrelated UI update.
    phrase.events = (1_000..11_000)
        .map(|id| {
            Event::note(
                EventId::new(id).unwrap(),
                Beat::new(1, 3).unwrap(),
                Target::instrument(InstrumentId::new(17).unwrap()),
                Note::new(60, 88, Beat::new(1, 8).unwrap()).unwrap(),
            )
        })
        .collect();
    app.command(Command::PutPhrase {
        id: other,
        phrase: phrase.clone(),
    })
    .unwrap();
    let after = app.projection();
    assert_eq!(before["phrases"], after["phrases"]);
    assert_eq!(
        after["phrases"][other.get().to_string()],
        json!({"name":phrase.name,"steps":16})
    );
    assert_eq!(before["phrase"], after["phrase"]);
    assert_eq!(after["phrase"]["id"], json!(selected));
    assert!(
        serde_json::to_vec(&after).unwrap().len()
            <= serde_json::to_vec(&before).unwrap().len() + 16
    );

    let (chain_id, source) = app.selected_chain().unwrap();
    let mut chain = source.clone();
    let chain_name = chain.name.clone();
    chain.phrases[0].phrase = other;
    app.command(Command::PutChain {
        id: chain_id,
        chain,
    })
    .unwrap();
    let chosen = app.projection();
    assert_eq!(chosen["phrase"]["value"], json!(phrase));
    assert_eq!(
        chosen["chains"][chain_id.get().to_string()],
        json!({"name":chain_name})
    );
    assert_eq!(
        chosen["selectedChain"]["value"],
        json!(app.selected_chain().unwrap().1)
    );
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.selected_phrase().unwrap().0, selected);
    assert_eq!(app.editor.song().phrases[&other], phrase);
}

#[test]
fn browser_delivery_retains_all_dense_notes_without_resending_them_on_navigation() {
    let mut app = with_phrase();
    let (id, source) = app.selected_phrase().unwrap();
    let mut phrase = source.clone();
    phrase.form = PhraseForm::PianoRoll;
    phrase.events = (1000..9193)
        .map(|id| {
            Event::note(
                EventId::new(id).unwrap(),
                Beat::new(1, 3).unwrap(),
                Target::instrument(InstrumentId::new(17).unwrap()),
                Note::new(60, 88, Beat::new(1, 8).unwrap()).unwrap(),
            )
        })
        .collect();
    app.command(Command::PutPhrase {
        id,
        phrase: phrase.clone(),
    })
    .unwrap();
    let full = app.browser_projection();
    assert_eq!(full["phrase"]["value"], json!(phrase));
    let full_bytes = serde_json::to_vec(&full).unwrap().len();
    app.queue(json!({"kind":"phrase_projection_ack", "token":full["phraseSource"]["token"]}));
    app.apply(Request::PianoControl {
        control: PianoControl::Navigate {
            horizontal: 1,
            vertical: 0,
        },
    })
    .unwrap();
    let mut next = app.browser_projection();
    assert!(next.get("phrase").is_none());
    assert!(serde_json::to_vec(&next).unwrap().len() * 10 < full_bytes);
    next["phrase"] = full["phrase"].clone();
    next.as_object_mut().unwrap().remove("phraseSource");
    assert_eq!(next, app.projection());
}

#[test]
fn fractional_tempo_survives_native_projection_edit_and_undo() {
    let mut app = App::new();
    let request = serde_json::from_value(json!({"kind":"tempo","tempo":123456})).unwrap();
    app.apply(request).unwrap();
    let saved = app.editor.song().clone();
    assert_eq!(saved.tempo.milli_bpm(), 123_456);
    let projection = app.projection();
    assert_eq!(projection["tempo"], 123.456);
    assert!(projection["subtitle"]
        .as_str()
        .unwrap()
        .starts_with("123.456 BPM"));

    // Reapplying the displayed value must not create a rounded edit.
    let request = serde_json::from_value(json!({"kind":"tempo","tempo":123456})).unwrap();
    app.apply(request).unwrap();
    assert_eq!(app.editor.song(), &saved);
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song().tempo, Tempo::DEFAULT);
    app.apply(Request::Redo).unwrap();
    assert_eq!(app.editor.song(), &saved);

    for value in [0.0, -1.0, 123.456, 4_294_967_296.0] {
        assert!(serde_json::from_value::<Request>(json!({"kind":"tempo","tempo":value})).is_err());
    }
    let preferences = serde_json::from_value::<Request>(json!({
        "kind":"preferences", "default_tempo":123456, "monitor":false,
        "input_clock":false, "output_clock":null, "delay_ms":100
    }))
    .unwrap();
    assert!(
        matches!(preferences, Request::Preferences { default_tempo, .. } if default_tempo.milli_bpm() == 123_456)
    );
}

#[test]
fn long_phrase_grid_is_bounded_and_edits_absolute_source_rows() {
    let mut app = with_phrase();
    let (id, source) = app.selected_phrase().unwrap();
    let mut phrase = source.clone();
    phrase.steps = 1_000_003;
    app.command(Command::PutPhrase { id, phrase }).unwrap();
    let baseline = app.editor.song().clone();
    let revision = app.editor.revision();
    for row in [0, 63, 64, 127, 128, 1_000_002] {
        app.apply(Request::Focus { row, column: 0 }).unwrap();
        let projection = app.projection();
        let start = projection["rowStart"].as_u64().unwrap() as usize;
        assert_eq!(projection["rowCount"], 1_000_003);
        assert_eq!(start, row / grid::PAGE_ROWS * grid::PAGE_ROWS);
        let rows = projection["rows"].as_array().unwrap();
        assert!(rows.len() <= grid::PAGE_ROWS);
        assert_eq!(rows[row - start][0], "—");
        assert!(
            matches!(app.feedback_state().unwrap().focus, crate::feedback::Focus::Grid { row: actual, .. } if actual == row)
        );
    }
    assert_eq!(app.editor.song(), &baseline);
    assert_eq!(app.editor.revision(), revision);
    app.apply(Request::Confirm).unwrap();
    let note = &app.selected_phrase().unwrap().1.events[0];
    assert_eq!(note.at, Beat::new(1_000_002, 4).unwrap());
    let projection = app.projection();
    let local_row = app.row - projection["rowStart"].as_u64().unwrap() as usize;
    assert_eq!(projection["rows"][local_row][0], "C4");
    assert!(matches!(app.feedback_state().unwrap().focus,
        crate::feedback::Focus::Grid { label, .. } if label == "NOTE, row 1000003: C4"));
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &baseline);
}

#[test]
fn arrangement_grid_pages_preserve_occurrence_and_append_rows() {
    let mut app = with_phrase();
    let phrase = app.selected_phrase().unwrap().0;
    let chain = app.selected_chain().unwrap().0;
    let mut song = app.editor.song().clone();
    let first = song.next_identity().unwrap();
    song.chains.get_mut(&chain).unwrap().phrases = (0..130)
        .map(|index| PhraseUse {
            id: UseId::new(first + index).unwrap(),
            phrase,
            repeats: 1,
        })
        .collect();
    song.tracks[0].slots = (0..130)
        .map(|index| {
            Some(ChainUse {
                id: UseId::new(first + 130 + index).unwrap(),
                chain,
                repeats: 1,
            })
        })
        .collect();
    app.editor = Editor::new(song.clone()).unwrap();
    app.apply(Request::View { view: View::Song }).unwrap();
    app.apply(Request::Focus {
        row: 129,
        column: 0,
    })
    .unwrap();
    assert_eq!(app.projection()["rowStart"], 128);
    assert_eq!(app.projection()["rows"].as_array().unwrap().len(), 3);
    assert_eq!(
        app.projection()["rows"][1][0],
        format!("{:02}", chain.get())
    );
    let selected_use = song.tracks[0].slots[129].as_ref().unwrap().id;
    app.apply(Request::Confirm).unwrap();
    assert_eq!(app.occurrence().unwrap().chain_use, selected_use);
    app.apply(Request::Focus {
        row: 130,
        column: 0,
    })
    .unwrap();
    assert_eq!(app.projection()["rowStart"], 128);
    assert_eq!(app.projection()["rows"][2][0], "+ NEW PHRASE");
    assert_eq!(app.editor.song(), &song, "paging is navigation only");
    app.apply(Request::Focus {
        row: 129,
        column: 0,
    })
    .unwrap();
    app.apply(Request::Confirm).unwrap();
    assert_eq!(
        app.occurrence().unwrap().phrase_use,
        song.chains[&chain].phrases[129].id
    );
    assert_eq!(
        app.projection()["rowStart"],
        0,
        "opening a source resets its row window"
    );
}

#[test]
fn malformed_native_requests_do_not_escape_view_specific_focus_bounds() {
    let mut app = App::new();
    for view in [View::Song, View::Expanded, View::Minimap] {
        app.apply(Request::View { view }).unwrap();
        app.apply(Request::Focus {
            row: usize::MAX,
            column: usize::MAX,
        })
        .unwrap();
        assert_eq!((app.track, app.column), (15, 15));
        assert_eq!((app.slot, app.row), (0, 0));
        app.apply(Request::Move {
            horizontal: i32::MIN,
            vertical: i32::MIN,
        })
        .unwrap();
        assert_eq!((app.track, app.column, app.slot, app.row), (0, 0, 0, 0));
    }
    let before = app.editor.song().clone();
    app.queue(json!({"kind":"focus","row":-1,"column":0}));
    app.queue(json!({"kind":"confirm","unknown":true}));
    app.queue(json!({"kind":"reset_shared_choice","unknown":true}));
    app.queue(json!({"kind":"synth_settings","unknown":true}));
    assert!(app.requests.is_empty());
    assert_eq!(app.editor.song(), &before);
    assert!(app.status.starts_with("Invalid action:"));
    app.queue(json!({"kind":"confirm"}));
    assert!(matches!(app.requests.as_slice(), [Request::Confirm]));
}

#[test]
fn chain_append_row_creates_one_undoable_phrase_and_back_preserves_focus() {
    let mut app = with_phrase();
    app.apply(Request::Back).unwrap();
    let before = app.editor.song().clone();
    app.apply(Request::Focus {
        row: usize::MAX,
        column: 0,
    })
    .unwrap();
    assert_eq!(app.row, 1);
    assert_eq!(app.projection()["rows"].as_array().unwrap().len(), 2);
    app.apply(Request::Confirm).unwrap();
    assert_eq!(app.view, View::Phrase);
    assert_eq!(app.chain_row, 1);
    assert_eq!(app.selected_chain().unwrap().1.phrases.len(), 2);
    app.apply(Request::Back).unwrap();
    assert_eq!((app.view, app.row), (View::Chain, 1));
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &before);
    assert_eq!(app.chain_row, 0);
    assert_eq!(app.row, 0);
}

#[test]
fn native_reuse_keeps_distinct_uses_and_rejects_sparse_assignment_atomically() {
    let mut app = with_phrase();
    let phrase = app.selected_phrase().unwrap().0;
    app.apply(Request::AppendPhrase {
        phrase: Some(phrase),
    })
    .unwrap();
    let (chain, source) = app.selected_chain().unwrap();
    assert_eq!(source.phrases.len(), 2);
    assert_eq!(source.phrases[0].phrase, source.phrases[1].phrase);
    assert_ne!(source.phrases[0].id, source.phrases[1].id);
    let original_use = app.editor.song().tracks[0].slots[0].as_ref().unwrap().id;
    let track = app.editor.song().tracks[1].id;
    app.apply(Request::AssignChain {
        track,
        slot: 0,
        chain,
    })
    .unwrap();
    let second_use = app.editor.song().tracks[1].slots[0].as_ref().unwrap();
    assert_eq!(second_use.chain, chain);
    assert_ne!(second_use.id, original_use);
    let before = app.editor.song().clone();
    let revision = app.editor.revision();
    assert!(app
        .apply(Request::AssignChain {
            track,
            slot: usize::MAX,
            chain
        })
        .is_err());
    assert!(app
        .apply(Request::AssignPhrase {
            row: usize::MAX,
            phrase
        })
        .is_err());
    assert_eq!(app.editor.song(), &before);
    assert_eq!(app.editor.revision(), revision);
    app.apply(Request::Undo).unwrap();
    assert!(app.editor.song().tracks[1].slots.is_empty());
}

#[test]
fn stable_occurrence_follows_chain_and_slot_reordering_through_undo() {
    let mut app = with_phrase();
    app.apply(Request::AppendPhrase { phrase: None }).unwrap();
    let chain = app.selected_chain().unwrap().0;
    let track_id = app.editor.song().tracks[0].id;
    app.apply(Request::AssignChain {
        track: track_id,
        slot: 1,
        chain,
    })
    .unwrap();
    app.apply(Request::Focus { row: 0, column: 0 }).unwrap();
    app.apply(Request::Confirm).unwrap();
    app.apply(Request::Focus { row: 1, column: 0 }).unwrap();
    app.apply(Request::Confirm).unwrap();
    let selected = app.occurrence().unwrap();
    let mut track = app.editor.song().tracks[0].clone();
    track.slots.swap(0, 1);
    let mut value = app.editor.song().chains[&chain].clone();
    value.phrases.swap(0, 1);
    assert!(app
        .apply(Request::Edit {
            revision: app.editor.revision(),
            command: Box::new(Command::Batch {
                commands: vec![
                    Command::PutTrack { track },
                    Command::PutChain {
                        id: chain,
                        chain: value,
                    },
                ],
            }),
        })
        .is_err());
    let token = app.projection()["sharedEdit"]["token"].as_u64().unwrap();
    app.apply(Request::SharedDecision {
        token,
        choice: Some(SharedChoice::Shared),
        remember: false,
    })
    .unwrap();
    assert_eq!(app.occurrence(), Some(selected));
    assert_eq!((app.slot, app.chain_row), (1, 0));
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.occurrence(), Some(selected));
    assert_eq!((app.slot, app.chain_row), (0, 1));
    app.apply(Request::Back).unwrap();
    app.apply(Request::Back).unwrap();
    assert_eq!((app.view, app.row, app.column), (View::Song, 0, 0));
}

#[test]
fn phrase_clear_only_removes_the_focused_cell_and_preserves_overlapping_notes() {
    let mut app = with_phrase();
    app.apply(Request::Confirm).unwrap();
    let (id, phrase) = app.selected_phrase().unwrap();
    let mut phrase = phrase.clone();
    phrase.events[0].fx[0] = Some(Fx::Offset { semitones: 7 });
    phrase.events[0].controls.push(Control::Cc {
        number: 1,
        value: 77,
    });
    let mut overlap = phrase.events[0].clone();
    overlap.id = EventId::new(app.editor.song().next_identity().unwrap()).unwrap();
    overlap.note.as_mut().unwrap().pitch = 72;
    phrase.events.push(overlap.clone());
    app.command(Command::PutPhrase { id, phrase }).unwrap();
    app.apply(Request::Focus { row: 0, column: 3 }).unwrap();
    app.apply(Request::Clear).unwrap();
    let phrase = app.selected_phrase().unwrap().1;
    assert!(phrase.events[0].fx[0].is_none());
    assert!(phrase.events[0].note.is_some());
    assert_eq!(phrase.events[1], overlap);
    app.apply(Request::Focus { row: 0, column: 0 }).unwrap();
    app.apply(Request::Clear).unwrap();
    let phrase = app.selected_phrase().unwrap().1;
    assert_eq!(phrase.events.len(), 2);
    assert!(phrase.events[0].note.is_none());
    assert_eq!(
        phrase.events[0].controls,
        vec![Control::Cc {
            number: 1,
            value: 77
        }]
    );
    assert_eq!(phrase.events[1], overlap);
    app.apply(Request::Focus { row: 0, column: 4 }).unwrap();
    app.apply(Request::Adjust { delta: 3 }).unwrap();
    assert_eq!(
        app.selected_phrase().unwrap().1.events[0].fx[1],
        Some(Fx::Offset { semitones: 3 })
    );
    app.apply(Request::Focus { row: 0, column: 0 }).unwrap();
    app.apply(Request::Confirm).unwrap();
    let phrase = app.selected_phrase().unwrap().1;
    assert_eq!(phrase.events[0].note.as_ref().unwrap().pitch, 60);
    assert_eq!(phrase.events[0].controls.len(), 1);
    assert_eq!(phrase.events[1], overlap);
}

#[test]
fn midi_output_requests_pass_to_the_host_and_output_status_projects_without_editing() {
    let mut app = App::new();
    let before = app.editor.song().clone();
    app.queue(json!({"kind":"midi_output","action":"discover"}));
    assert_eq!(app.requests.len(), 1);
    let request = app.requests.pop().unwrap();
    assert!(
        matches!(&request, Request::MidiOutput { action, endpoint: None } if action == "discover")
    );
    app.apply(request).unwrap();
    app.outputs = json!({"endpoints":[{"id":"test","name":"Device"}],"connected":"test"});
    assert_eq!(app.projection()["outputs"], app.outputs);
    assert_eq!(app.editor.song(), &before);
    assert!(!app.dirty());
}

#[test]
fn chain_and_song_clear_remove_uses_without_destroying_reusable_content() {
    let mut app = with_phrase();
    app.apply(Request::AppendPhrase { phrase: None }).unwrap();
    let before = app.editor.song().clone();
    app.apply(Request::Clear).unwrap();
    assert_eq!(app.selected_chain().unwrap().1.phrases.len(), 1);
    assert_eq!(app.editor.song().phrases, before.phrases);
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &before);
    app.apply(Request::Back).unwrap();
    app.apply(Request::Clear).unwrap();
    assert!(app.editor.song().tracks[0].slots[0].is_none());
    assert_eq!(app.editor.song().chains, before.chains);
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &before);
}

#[test]
fn phrase_entry_uses_local_scale_and_extreme_adjustments_cannot_overflow() {
    let mut app = with_phrase();
    let (id, phrase) = app.selected_phrase().unwrap();
    let mut phrase = phrase.clone();
    phrase.key = Some(
        Scale::custom(1, vec![1])
            .unwrap()
            .with_spelling(Spelling::Flats),
    );
    app.command(Command::PutPhrase { id, phrase }).unwrap();
    app.apply(Request::Confirm).unwrap();
    assert_eq!(
        app.selected_phrase().unwrap().1.events[0]
            .note
            .as_ref()
            .unwrap()
            .pitch,
        61
    );
    assert_eq!(app.projection()["rows"][0][0], "Db4");
    app.apply(Request::Adjust { delta: i32::MAX }).unwrap();
    assert_eq!(
        app.selected_phrase().unwrap().1.events[0]
            .note
            .as_ref()
            .unwrap()
            .pitch,
        121
    );
    app.apply(Request::Adjust { delta: i32::MIN }).unwrap();
    assert_eq!(
        app.selected_phrase().unwrap().1.events[0]
            .note
            .as_ref()
            .unwrap()
            .pitch,
        1
    );
    app.apply(Request::Focus { row: 0, column: 2 }).unwrap();
    app.apply(Request::Adjust { delta: i32::MAX }).unwrap();
    assert_eq!(
        app.selected_phrase().unwrap().1.events[0]
            .note
            .as_ref()
            .unwrap()
            .velocity,
        127
    );
    app.apply(Request::Focus { row: 0, column: 3 }).unwrap();
    app.apply(Request::Adjust { delta: i32::MIN }).unwrap();
    assert_eq!(
        app.selected_phrase().unwrap().1.events[0].fx[0],
        Some(Fx::Offset { semitones: -24 })
    );
}

#[test]
fn generator_preview_is_read_only_and_invalidates_after_same_revision_replacement() {
    let mut app = with_phrase();
    let phrase = app.selected_phrase().unwrap().0;
    let mut song = app.editor.song().clone();
    song.phrases.get_mut(&phrase).unwrap().form = PhraseForm::Random {
        seed: 7,
        density: 100,
        low: 60,
        high: 60,
        velocity_low: 100,
        velocity_high: 100,
        gate: Beat::ONE,
        gate_high: None,
        target: Target::instrument(InstrumentId::new(17).unwrap()),
    };
    app.editor = Editor::new(song.clone()).unwrap();
    let first = app.projection();
    assert_eq!(first["rows"][0][0], "C4");
    assert!(first["generatorPreview"]["events"]
        .as_array()
        .is_some_and(|events| !events.is_empty()));
    assert_eq!(app.editor.song(), &song);
    assert_eq!(app.editor.revision(), 0);
    if let PhraseForm::Random { low, high, .. } = &mut song.phrases.get_mut(&phrase).unwrap().form {
        *low = 62;
        *high = 62;
    }
    app.editor = Editor::new(song.clone()).unwrap();
    let changed = app.projection();
    assert_eq!(changed["rows"][0][0], "D4");
    assert_ne!(changed["generatorPreview"], first["generatorPreview"]);
    assert_eq!(app.editor.song(), &song);
    assert!(app.selected_phrase().unwrap().1.events.is_empty());
}

fn gesture_fixture() -> App {
    let mut app = with_phrase();
    app.apply(Request::Confirm).unwrap();
    app.apply(Request::Focus { row: 0, column: 2 }).unwrap();
    app.editor = Editor::new(app.editor.song().clone()).unwrap();
    app
}

#[test]
fn held_adjustments_share_one_undo_across_ui_acknowledgements() {
    let mut app = gesture_fixture();
    let original = app.editor.song().clone();
    app.apply(Request::AdjustGesture {
        delta: 1,
        gesture: 7,
    })
    .unwrap();
    assert!(app.adjustment_gesture.is_none());
    assert!(app.ui_input("next"));
    app.apply(Request::UiAck { sequence: 1 }).unwrap();
    app.apply(Request::AdjustGesture {
        delta: 1,
        gesture: 7,
    })
    .unwrap();
    assert_eq!(app.editor.revision(), 2);
    assert_eq!(
        app.selected_phrase().unwrap().1.events[0]
            .note
            .as_ref()
            .unwrap()
            .velocity,
        102
    );
    let adjusted = app.editor.song().clone();
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &original);
    assert!(!app.editor.can_undo());
    app.apply(Request::Redo).unwrap();
    assert_eq!(app.editor.song(), &adjusted);
}

#[test]
fn new_gesture_identity_release_and_ordinary_adjustments_create_separate_undo_steps() {
    let mut app = gesture_fixture();
    let original = app.editor.song().clone();
    app.apply(Request::AdjustGesture {
        delta: 1,
        gesture: 8,
    })
    .unwrap();
    let first = app.editor.song().clone();
    app.apply(Request::AdjustGesture {
        delta: 1,
        gesture: 9,
    })
    .unwrap();
    let second = app.editor.song().clone();
    app.apply(Request::EndGesture).unwrap();
    assert_eq!(app.editor.revision(), 2);
    app.apply(Request::AdjustGesture {
        delta: 1,
        gesture: 9,
    })
    .unwrap();
    let third = app.editor.song().clone();
    app.apply(Request::Adjust { delta: 1 }).unwrap();
    let fourth = app.editor.song().clone();
    app.apply(Request::AdjustGesture {
        delta: 1,
        gesture: 9,
    })
    .unwrap();
    for expected in [fourth, third, second, first, original] {
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &expected);
    }
    assert!(!app.editor.can_undo());
}

#[test]
fn navigation_and_modal_actions_end_gestures_even_when_focus_does_not_move() {
    for action in [
        Request::Focus { row: 0, column: 2 },
        Request::Move {
            horizontal: 0,
            vertical: 0,
        },
        Request::View { view: View::Phrase },
        Request::TextEntry { active: true },
    ] {
        let mut app = gesture_fixture();
        let original = app.editor.song().clone();
        app.apply(Request::AdjustGesture {
            delta: 1,
            gesture: 10,
        })
        .unwrap();
        let first = app.editor.song().clone();
        app.apply(action).unwrap();
        app.apply(Request::AdjustGesture {
            delta: 1,
            gesture: 10,
        })
        .unwrap();
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &first);
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &original);
        assert!(!app.editor.can_undo());
    }
}

#[test]
fn failed_adjustments_clear_temporary_context_without_poisoning_ordinary_commands() {
    let mut app = gesture_fixture();
    app.apply(Request::AdjustGesture {
        delta: 1,
        gesture: 11,
    })
    .unwrap();
    let before_failure = app.editor.song().clone();
    let revision = app.editor.revision();
    assert!(app
        .apply(Request::AdjustGesture {
            delta: 1,
            gesture: 0
        })
        .is_err());
    // Exercise command validation after gesture context has been installed.
    app.row = app.selected_phrase().unwrap().1.steps as usize;
    assert!(app
        .apply(Request::AdjustGesture {
            delta: 1,
            gesture: 11
        })
        .is_err());
    assert_eq!(app.editor.song(), &before_failure);
    assert_eq!(app.editor.revision(), revision);
    assert!(app.adjustment_gesture.is_none());
    app.row = 0;
    app.command(Command::RenameSong {
        name: "Ordinary edit after failure".into(),
    })
    .unwrap();
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &before_failure);
    assert!(app.editor.can_undo());

    let mut no_instrument = App::new();
    no_instrument.apply(Request::Confirm).unwrap();
    no_instrument.apply(Request::Confirm).unwrap();
    assert!(no_instrument
        .apply(Request::AdjustGesture {
            delta: 1,
            gesture: 1
        })
        .is_err());
    assert!(no_instrument.adjustment_gesture.is_none());
}

#[test]
fn held_chain_repeat_adjustments_use_the_same_gesture_owner() {
    let mut app = gesture_fixture();
    app.apply(Request::Back).unwrap();
    app.apply(Request::Focus { row: 0, column: 1 }).unwrap();
    let original = app.editor.song().clone();
    app.apply(Request::AdjustGesture {
        delta: 1,
        gesture: 12,
    })
    .unwrap();
    app.apply(Request::AdjustGesture {
        delta: 1,
        gesture: 12,
    })
    .unwrap();
    assert_eq!(app.selected_chain().unwrap().1.phrases[0].repeats, 3);
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &original);
    assert!(!app.editor.can_undo());
}

#[test]
fn webview_cannot_deserialize_or_queue_internal_gesture_requests() {
    let mut app = gesture_fixture();
    let original = app.editor.song().clone();
    for value in [
        json!({"kind":"adjust_gesture","delta":1,"gesture":1}),
        json!({"kind":"end_gesture"}),
    ] {
        assert!(serde_json::from_value::<Request>(value.clone()).is_err());
        app.queue(value);
        assert!(app.requests.is_empty());
        assert!(app.status.starts_with("Invalid action:"));
    }
    assert_eq!(app.editor.song(), &original);
    assert_eq!(app.editor.revision(), 0);
    app.queue(json!({"kind":"adjust","delta":1}));
    assert!(matches!(
        app.requests.as_slice(),
        [Request::Adjust { delta: 1 }]
    ));
}

#[test]
fn full_scale_catalog_phrase_override_and_display_preserve_stored_notes() {
    let mut app = with_phrase();
    app.apply(Request::Adjust { delta: 1 }).unwrap();
    for scale in ScaleKind::CATALOG {
        app.apply(Request::KeySettings {
            revision: app.editor.revision(),
            phrase: false,
            root: 2,
            scale: Some(scale),
            pitch_classes: vec![],
            spelling: Spelling::Sharps,
        })
        .unwrap();
        assert_eq!(app.editor.song().key.kind(), scale);
    }
    app.apply(Request::KeySettings {
        revision: app.editor.revision(),
        phrase: true,
        root: 3,
        scale: Some(ScaleKind::Custom),
        pitch_classes: vec![3, 6, 10],
        spelling: Spelling::Flats,
    })
    .unwrap();
    let before = app.editor.song().clone();
    app.apply(Request::PitchDisplay {
        intervals: true,
        chromatic: true,
    })
    .unwrap();
    assert_eq!(app.editor.song(), &before);
    assert!(app.projection()["rows"][0][0]
        .as_str()
        .unwrap()
        .contains("oct"));
    let pitch = app.selected_phrase().unwrap().1.events[0]
        .note
        .as_ref()
        .unwrap()
        .pitch;
    app.apply(Request::Adjust { delta: 1 }).unwrap();
    assert_eq!(
        app.selected_phrase().unwrap().1.events[0]
            .note
            .as_ref()
            .unwrap()
            .pitch,
        pitch + 1
    );
    let before = app.editor.song().clone();
    assert!(app
        .apply(Request::KeySettings {
            revision: app.editor.revision(),
            phrase: true,
            root: 3,
            scale: Some(ScaleKind::Custom),
            pitch_classes: vec![0, 2],
            spelling: Spelling::Flats
        })
        .is_err());
    assert_eq!(app.editor.song(), &before);
}

#[test]
fn range_filters_are_exact_and_duplicate_preserves_expression_without_touching_clipboard() {
    let mut app = with_phrase();
    app.apply(Request::Adjust { delta: 1 }).unwrap();
    let (id, source) = app.selected_phrase().unwrap();
    let mut phrase = source.clone();
    let mut note = phrase.events[0].clone();
    note.at = Beat::new(1, 3).unwrap();
    note.note.as_mut().unwrap().expression.push(Expression {
        offset: Beat::ZERO,
        sequence: None,
        control: Control::Pressure { value: 39 },
    });
    phrase.events[0] = note.clone();
    let mut second = note.clone();
    second.id = EventId::new(app.editor.song().next_identity().unwrap()).unwrap();
    second.at = Beat::ONE;
    phrase.events.push(second);
    app.command(Command::PutPhrase { id, phrase }).unwrap();
    let occurrence = app.occurrence().unwrap();
    app.apply(Request::SelectRange {
        revision: app.editor.revision(),
        occurrence,
        range: EventRange {
            start: Beat::ZERO,
            end: Beat::ONE,
            low: 0,
            high: 127,
            target: Some(note.target.clone()),
            include_controls: false,
        },
    })
    .unwrap();
    assert_eq!(app.projection()["selectedEvents"], json!([note.id]));
    let before = app.editor.song().clone();
    let clipboard = app.clipboard.clone();
    app.apply(Request::SelectionEdit {
        revision: app.editor.revision(),
        occurrence,
        action: SelectionAction::Duplicate {
            at: Beat::integer(2),
        },
    })
    .unwrap();
    let copied = app
        .selected_phrase()
        .unwrap()
        .1
        .events
        .iter()
        .find(|event| event.at == Beat::integer(2))
        .unwrap();
    assert_ne!(copied.id, note.id);
    assert_eq!(copied.note, note.note);
    assert_eq!(app.clipboard, clipboard);
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &before);
}
