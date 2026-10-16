use crest_tracker::composition::*;
use crest_tracker::editing::{Command, Editor, Occurrence};
use crest_tracker::time::{Beat, Division, Tempo};

fn fixture() -> (Song, PhraseId) {
    let mut song = Song::default();
    let phrase = PhraseId::new(17).unwrap();
    let mut note = Note::new(64, 99, Beat::new(5, 6).unwrap()).unwrap();
    note.release_velocity = 23;
    note.expression = vec![Expression {
        sequence: None,
        offset: Beat::new(2, 3).unwrap(),
        control: Control::Bend { value: 8193 },
    }];
    let target = Target::Channel {
        channel: Channel::new(3).unwrap(),
        endpoint: Some("device-stable-id".into()),
    };
    let first = Event::note(
        EventId::new(18).unwrap(),
        Beat::new(9, 8).unwrap(),
        target.clone(),
        note.clone(),
    );
    let second = Event::note(
        EventId::new(19).unwrap(),
        Beat::new(15, 4).unwrap(),
        target,
        note,
    );
    song.phrases.insert(
        phrase,
        Phrase {
            events: vec![first, second],
            ..Phrase::default()
        },
    );
    song.validate().unwrap();
    (song, phrase)
}

fn apply(editor: &mut Editor, command: Command) {
    assert!(editor.apply(editor.revision(), command).unwrap());
}

#[test]
fn stale_and_failed_batches_preserve_document_revision_and_redo_history() {
    let (song, _) = fixture();
    let mut editor = Editor::new(song.clone()).unwrap();
    apply(
        &mut editor,
        Command::RenameSong {
            name: "Edited".into(),
        },
    );
    editor.undo().unwrap();
    let revision = editor.revision();
    assert!(matches!(
        editor.apply(
            0,
            Command::RenameSong {
                name: "Stale".into()
            }
        ),
        Err(DocumentError::Stale { .. })
    ));
    assert!(editor
        .apply(
            revision,
            Command::Batch {
                commands: vec![
                    Command::SetTempo {
                        tempo: Tempo::new(135_000).unwrap()
                    },
                    Command::RenameSong {
                        name: "\0invalid".into()
                    },
                ]
            }
        )
        .is_err());
    assert_eq!(editor.song(), &song);
    assert_eq!(editor.revision(), revision);
    assert!(editor.can_redo());
    assert!(!editor.can_undo());
    editor.redo().unwrap();
    assert_eq!(editor.song().name, "Edited");
    assert_eq!(editor.song().tempo, song.tempo);
}

#[test]
fn replacing_song_fences_old_commands_and_resets_history_only_after_validation() {
    let (song, phrase) = fixture();
    let mut editor = Editor::new(song.clone()).unwrap();
    let old_command = Command::PutPhrase {
        id: phrase,
        phrase: Phrase {
            name: "Delayed old phrase".into(),
            ..Phrase::default()
        },
    };
    apply(
        &mut editor,
        Command::RenameSong {
            name: "Old edit".into(),
        },
    );
    editor.undo().unwrap();
    let revision = editor.revision();
    let mut invalid = song.clone();
    invalid.name = "\0invalid".into();
    assert!(editor.replace_song(invalid).is_err());
    assert_eq!(editor.song(), &song);
    assert_eq!(editor.revision(), revision);
    assert!(editor.can_redo());

    let mut replacement = song.clone();
    replacement.name = "New document, same IDs".into();
    editor.replace_song(replacement.clone()).unwrap();
    assert_eq!(editor.revision(), revision + 1);
    assert!(matches!(
        editor.apply(revision, old_command),
        Err(DocumentError::Stale { .. })
    ));
    assert_eq!(editor.song(), &replacement);
    assert!(!editor.dirty());
    assert!(!editor.can_undo());
    assert!(!editor.can_redo());
    assert!(!editor.undo().unwrap());
    apply(
        &mut editor,
        Command::RenameSong {
            name: "New edit".into(),
        },
    );
    editor.undo().unwrap();
    assert_eq!(editor.song(), &replacement);
    assert!(!editor.dirty());
    let revision = editor.revision();
    editor.replace_song(replacement).unwrap();
    assert_eq!(
        editor.revision(),
        revision + 1,
        "reopening identical content still retires old commands"
    );
}

#[test]
fn shared_parent_make_unique_isolates_one_occurrence_and_undo_restores_graph() {
    let (mut song, phrase) = fixture();
    let chain = ChainId::new(20).unwrap();
    song.chains.insert(
        chain,
        Chain {
            name: "Shared".into(),
            phrases: vec![
                PhraseUse {
                    id: UseId::new(21).unwrap(),
                    phrase,
                    repeats: 2,
                },
                PhraseUse {
                    id: UseId::new(22).unwrap(),
                    phrase,
                    repeats: 1,
                },
            ],
        },
    );
    song.tracks[0].slots.push(Some(ChainUse {
        id: UseId::new(23).unwrap(),
        chain,
        repeats: 1,
    }));
    song.tracks[1].slots.push(Some(ChainUse {
        id: UseId::new(24).unwrap(),
        chain,
        repeats: 3,
    }));
    let occurrence = Occurrence {
        track: song.tracks[0].id,
        chain_use: UseId::new(23).unwrap(),
        phrase_use: UseId::new(21).unwrap(),
    };
    let mut editor = Editor::new(song.clone()).unwrap();
    apply(&mut editor, Command::MakeUnique { occurrence });
    let isolated = editor.song().clone();
    let new_chain = isolated.tracks[0].slots[0].as_ref().unwrap().chain;
    assert_ne!(new_chain, chain);
    assert_eq!(isolated.tracks[1], song.tracks[1]);
    assert_eq!(isolated.chains[&chain], song.chains[&chain]);
    assert_eq!(isolated.phrases[&phrase], song.phrases[&phrase]);
    let new_phrase = isolated.chains[&new_chain].phrases[0].phrase;
    assert_ne!(new_phrase, phrase);
    assert_eq!(isolated.chains[&new_chain].phrases[1].phrase, phrase);
    assert_ne!(
        isolated.chains[&new_chain].phrases[1].id,
        UseId::new(22).unwrap()
    );
    let mut event = isolated.phrases[&new_phrase].events[0].clone();
    assert_ne!(event.id, isolated.phrases[&phrase].events[0].id);
    event.note.as_mut().unwrap().pitch = 72;
    apply(
        &mut editor,
        Command::PutEvent {
            phrase: new_phrase,
            event,
        },
    );
    assert_eq!(editor.song().phrases[&phrase], song.phrases[&phrase]);
    editor.undo().unwrap();
    assert_eq!(editor.song(), &isolated);
    editor.undo().unwrap();
    assert_eq!(editor.song(), &song);
    editor.redo().unwrap();
    assert_eq!(editor.song(), &isolated);
}

#[test]
fn quantize_moves_selected_starts_only_with_exact_midpoint_later_policy() {
    let (song, phrase) = fixture();
    let original = song.phrases[&phrase].clone();
    let mut editor = Editor::new(song.clone()).unwrap();
    apply(
        &mut editor,
        Command::Quantize {
            phrase,
            events: vec![EventId::new(18).unwrap()],
            grid: Beat::new(1, 4).unwrap(),
            durations: false,
        },
    );
    let quantized = &editor.song().phrases[&phrase];
    assert_eq!(quantized.events[0].at, Beat::new(5, 4).unwrap());
    assert_eq!(quantized.events[0].note, original.events[0].note);
    assert_eq!(quantized.events[0].target, original.events[0].target);
    assert_eq!(quantized.events[1], original.events[1]);
    editor.undo().unwrap();
    assert_eq!(editor.song(), &song);
}

#[test]
fn a_quantize_failure_after_an_earlier_candidate_edit_is_atomic() {
    let (song, phrase) = fixture();
    let mut editor = Editor::new(song.clone()).unwrap();
    assert!(editor
        .apply(
            0,
            Command::Quantize {
                phrase,
                events: vec![EventId::new(18).unwrap(), EventId::new(19).unwrap()],
                grid: Beat::ONE,
                durations: false,
            }
        )
        .is_err());
    assert_eq!(editor.song(), &song);
    assert_eq!(editor.revision(), 0);
    assert!(!editor.can_undo());
    assert!(!editor.dirty());
}

#[test]
fn explicit_duration_quantize_rejects_loss_of_recorded_expression() {
    let (mut song, phrase) = fixture();
    let note = song.phrases.get_mut(&phrase).unwrap().events[0]
        .note
        .as_mut()
        .unwrap();
    note.duration = Beat::new(7, 10).unwrap();
    note.expression[0].offset = Beat::new(13, 20).unwrap();
    let mut editor = Editor::new(song.clone()).unwrap();
    assert!(editor
        .apply(
            0,
            Command::Quantize {
                phrase,
                events: vec![EventId::new(18).unwrap()],
                grid: Beat::new(1, 2).unwrap(),
                durations: true
            }
        )
        .is_err());
    assert_eq!(editor.song(), &song);
    assert!(!editor.can_undo());
}

#[test]
fn division_retime_scales_notes_and_relative_expression_by_exact_ratio() {
    let (song, phrase) = fixture();
    let mut editor = Editor::new(song.clone()).unwrap();
    apply(
        &mut editor,
        Command::RetimeDivision {
            phrase,
            division: Division::new(16, 3, 2).unwrap(),
        },
    );
    let retimed = &editor.song().phrases[&phrase];
    let note = retimed.events[0].note.as_ref().unwrap();
    assert_eq!(retimed.events[0].at, Beat::new(3, 4).unwrap());
    assert_eq!(note.duration, Beat::new(5, 9).unwrap());
    assert_eq!(note.expression[0].offset, Beat::new(4, 9).unwrap());
    assert_eq!(note.expression[0].control, Control::Bend { value: 8193 });
    assert_eq!(note.pitch, 64);
    assert_eq!(retimed.length().unwrap(), Beat::new(8, 3).unwrap());
    editor.undo().unwrap();
    assert_eq!(editor.song(), &song);
}

#[test]
fn explicit_trim_removes_overhanging_expression_and_events_in_one_undo() {
    let (song, phrase) = fixture();
    let mut editor = Editor::new(song.clone()).unwrap();
    assert!(editor
        .apply(
            0,
            Command::ResizePhrase {
                phrase,
                steps: 6,
                trim: false
            }
        )
        .is_err());
    assert_eq!(editor.song(), &song);
    apply(
        &mut editor,
        Command::ResizePhrase {
            phrase,
            steps: 6,
            trim: true,
        },
    );
    let trimmed = &editor.song().phrases[&phrase];
    assert_eq!(trimmed.events.len(), 1);
    let note = trimmed.events[0].note.as_ref().unwrap();
    assert_eq!(note.duration, Beat::new(3, 8).unwrap());
    assert!(note.expression.is_empty());
    editor.undo().unwrap();
    assert_eq!(editor.song(), &song);
    assert!(!editor.can_undo());
}

#[test]
fn completed_save_marks_its_snapshot_without_erasing_newer_unsaved_edits() {
    let (song, _) = fixture();
    let mut editor = Editor::new(song).unwrap();
    apply(
        &mut editor,
        Command::RenameSong {
            name: "Saved snapshot".into(),
        },
    );
    let captured = editor.song().clone();
    apply(
        &mut editor,
        Command::SetTempo {
            tempo: Tempo::new(140_000).unwrap(),
        },
    );
    let current_revision = editor.revision();
    editor.mark_saved(captured.clone());
    assert!(editor.dirty());
    assert_eq!(editor.revision(), current_revision);
    assert_eq!(editor.song().tempo, Tempo::new(140_000).unwrap());
    editor.undo().unwrap();
    assert_eq!(editor.song(), &captured);
    assert!(!editor.dirty());
    editor.redo().unwrap();
    assert!(editor.dirty());
}

fn gesture_tempo(editor: &mut Editor, gesture: u64, bpm: u32) {
    assert!(editor
        .apply_gesture(
            editor.revision(),
            Command::SetTempo {
                tempo: Tempo::new(bpm * 1000).unwrap()
            },
            gesture,
        )
        .unwrap());
}

#[test]
fn ten_held_adjustments_advance_revisions_but_have_one_undo_and_redo() {
    let (song, _) = fixture();
    let mut editor = Editor::new(song.clone()).unwrap();
    for adjustment in 1..=10 {
        gesture_tempo(&mut editor, 17, 120 + adjustment);
        assert_eq!(editor.revision(), u64::from(adjustment));
    }
    let adjusted = editor.song().clone();
    assert!(editor.dirty());
    assert!(editor.undo().unwrap());
    assert_eq!(editor.song(), &song);
    assert!(!editor.dirty());
    assert!(!editor.can_undo());
    assert!(editor.redo().unwrap());
    assert_eq!(editor.song(), &adjusted);
    assert_eq!(editor.revision(), 12);
    assert!(!editor.can_redo());
    assert!(editor.undo().unwrap());
    assert_eq!(editor.song(), &song);
    assert!(!editor.can_undo());
}

#[test]
fn gesture_stale_invalid_and_zero_ids_preserve_content_history_and_active_group() {
    let (song, _) = fixture();
    let mut editor = Editor::new(song.clone()).unwrap();
    gesture_tempo(&mut editor, 1, 121);
    let first = editor.song().clone();
    let revision = editor.revision();
    assert!(matches!(
        editor.apply_gesture(
            0,
            Command::RenameSong {
                name: "Stale".into()
            },
            1
        ),
        Err(DocumentError::Stale { .. })
    ));
    assert!(editor
        .apply_gesture(
            revision,
            Command::Batch {
                commands: vec![
                    Command::SetTempo {
                        tempo: Tempo::new(140_000).unwrap()
                    },
                    Command::RenameSong {
                        name: "\0invalid".into()
                    }
                ],
            },
            2
        )
        .is_err());
    assert!(editor
        .apply_gesture(
            revision,
            Command::RenameSong {
                name: "Zero".into()
            },
            0
        )
        .is_err());
    assert!(editor
        .apply(
            revision,
            Command::RenameSong {
                name: String::new()
            }
        )
        .is_err());
    assert_eq!(editor.song(), &first);
    assert_eq!(editor.revision(), revision);
    assert!(!editor.can_redo());
    gesture_tempo(&mut editor, 1, 122);
    editor.undo().unwrap();
    assert_eq!(editor.song(), &song);
    assert!(!editor.can_undo());
    let revision = editor.revision();
    assert!(editor
        .apply_gesture(
            revision,
            Command::RenameSong {
                name: String::new()
            },
            3
        )
        .is_err());
    assert_eq!(editor.revision(), revision);
    assert!(editor.can_redo());
    editor.redo().unwrap();
    assert_eq!(editor.song().tempo, Tempo::new(122_000).unwrap());
}

#[test]
fn ordinary_edits_and_changed_gestures_cannot_be_absorbed_by_an_older_id() {
    let (song, _) = fixture();
    let mut editor = Editor::new(song.clone()).unwrap();
    gesture_tempo(&mut editor, 4, 121);
    let first = editor.song().clone();
    apply(
        &mut editor,
        Command::RenameSong {
            name: "Separate edit".into(),
        },
    );
    let unrelated = editor.song().clone();
    gesture_tempo(&mut editor, 4, 122);
    let second = editor.song().clone();
    gesture_tempo(&mut editor, 5, 123);
    let third = editor.song().clone();
    gesture_tempo(&mut editor, 4, 124);
    for expected in [third, second, unrelated, first, song] {
        editor.undo().unwrap();
        assert_eq!(editor.song(), &expected);
    }
    assert!(!editor.can_undo());
}

#[test]
fn no_ops_create_no_history_and_unrelated_no_ops_end_the_current_gesture() {
    let (song, _) = fixture();
    let mut editor = Editor::new(song.clone()).unwrap();
    let same = Command::SetTempo { tempo: song.tempo };
    assert!(!editor.apply_gesture(0, same.clone(), 8).unwrap());
    assert!(!editor.apply(0, same).unwrap());
    assert_eq!(editor.revision(), 0);
    assert!(!editor.can_undo());
    gesture_tempo(&mut editor, 8, 121);
    assert!(!editor
        .apply_gesture(
            1,
            Command::SetTempo {
                tempo: editor.song().tempo
            },
            8
        )
        .unwrap());
    gesture_tempo(&mut editor, 8, 122);
    let first = editor.song().clone();
    assert!(!editor
        .apply(
            editor.revision(),
            Command::SetTempo {
                tempo: editor.song().tempo
            }
        )
        .unwrap());
    gesture_tempo(&mut editor, 8, 123);
    let second = editor.song().clone();
    assert!(!editor
        .apply_gesture(
            editor.revision(),
            Command::SetTempo {
                tempo: editor.song().tempo
            },
            9
        )
        .unwrap());
    gesture_tempo(&mut editor, 8, 124);
    editor.undo().unwrap();
    assert_eq!(editor.song(), &second);
    editor.undo().unwrap();
    assert_eq!(editor.song(), &first);
    editor.undo().unwrap();
    assert_eq!(editor.song(), &song);
    assert!(!editor.can_undo());
    assert!(!editor
        .apply_gesture(
            editor.revision(),
            Command::SetTempo { tempo: song.tempo },
            8
        )
        .unwrap());
    assert!(editor.can_redo());
}

#[test]
fn explicit_end_and_history_navigation_each_start_a_fresh_gesture_baseline() {
    let (song, _) = fixture();
    let mut editor = Editor::new(song.clone()).unwrap();
    gesture_tempo(&mut editor, 3, 121);
    let first = editor.song().clone();
    editor.end_gesture();
    assert_eq!(editor.revision(), 1);
    gesture_tempo(&mut editor, 3, 122);
    editor.undo().unwrap();
    assert_eq!(editor.song(), &first);
    gesture_tempo(&mut editor, 3, 123);
    assert!(!editor.can_redo());
    editor.undo().unwrap();
    assert_eq!(editor.song(), &first);
    editor.redo().unwrap();
    let third = editor.song().clone();
    gesture_tempo(&mut editor, 3, 124);
    editor.undo().unwrap();
    assert_eq!(editor.song(), &third);
    editor.undo().unwrap();
    assert_eq!(editor.song(), &first);
    editor.undo().unwrap();
    assert_eq!(editor.song(), &song);
    assert!(!editor.can_undo());
}

#[test]
fn save_boundary_keeps_saved_snapshot_reachable_and_dirty_state_exact() {
    let (song, _) = fixture();
    let mut editor = Editor::new(song.clone()).unwrap();
    gesture_tempo(&mut editor, 11, 121);
    let captured = editor.song().clone();
    editor.mark_saved(captured.clone());
    assert!(!editor.dirty());
    assert_eq!(editor.revision(), 1);
    gesture_tempo(&mut editor, 11, 122);
    gesture_tempo(&mut editor, 11, 123);
    assert!(editor.dirty());
    editor.undo().unwrap();
    assert_eq!(editor.song(), &captured);
    assert!(!editor.dirty());
    editor.redo().unwrap();
    assert!(editor.dirty());
    let newer = editor.song().clone();
    editor.mark_saved(captured.clone());
    assert!(editor.dirty());
    gesture_tempo(&mut editor, 11, 124);
    editor.undo().unwrap();
    assert_eq!(editor.song(), &newer);
    assert!(editor.dirty());
    editor.undo().unwrap();
    assert_eq!(editor.song(), &captured);
    assert!(!editor.dirty());
    editor.undo().unwrap();
    assert_eq!(editor.song(), &song);
    assert!(editor.dirty());
}
