use super::*;
use crest_synth::shell::{KeyboardInputTranslator, WindowInput, WindowKey};
use crest_tracker::composition::{InstrumentId, InstrumentUse};

fn apply_pending(app: &mut App) {
    for request in std::mem::take(&mut app.requests) {
        app.apply(request).unwrap();
    }
}
fn fixture() -> App {
    let mut app = App::new();
    app.command(Command::PutInstrument {
        id: InstrumentId::new(1).unwrap(),
        instrument: InstrumentUse {
            name: "Sample".into(),
            resource: None,
        },
    })
    .unwrap();
    app
}

#[test]
fn arrangement_playheads_use_occurrences_instead_of_phrase_note_rows() {
    let mut app = fixture();
    app.apply(Request::Confirm).unwrap();
    app.apply(Request::Confirm).unwrap();
    let occurrence = app.occurrence().unwrap();
    app.playing = true;
    app.play_row = Some(7);
    app.play_heads
        .push(crest_tracker::playback::AudiblePlayhead {
            occurrence,
            phrase: app.selected_phrase().unwrap().0,
            row: 7,
            local_beat: Beat::new(7, 4).unwrap(),
            phrase_repeat: 0,
            chain_repeat: 0,
            at: Beat::new(7, 4).unwrap(),
            cycle: 0,
            revision: app.editor.revision(),
        });
    for view in [View::Song, View::Chain] {
        app.view = view;
        let projection = project(&app).workspace.unwrap();
        assert_eq!(
            projection
                .grid
                .iter()
                .filter(|row| row.playing)
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            ["row:0"]
        );
    }
    app.view = View::Phrase;
    assert!(project(&app).workspace.unwrap().grid[7].playing);
    app.playing = false;
    app.view = View::Song;
    assert!(project(&app)
        .workspace
        .unwrap()
        .grid
        .iter()
        .all(|row| !row.playing));
}

#[test]
fn held_edit_creates_and_cycles_references_without_changing_layers() {
    let mut app = fixture();
    let empty = app.editor.song().clone();
    action(
        &mut app,
        SemanticAction::SetInteractionMode(InteractionMode::Adjust),
    );
    action(&mut app, SemanticAction::Adjust(Direction::Right));
    apply_pending(&mut app);
    assert_eq!(app.view, View::Song);
    let first_chain = app.selected_chain().expect("Edit creates a chain").0;
    assert_eq!(app.selected_chain().unwrap().1.name, "Chain 00");
    assert_eq!(
        project(&app).workspace.unwrap().grid[0].cells[0].value,
        "00"
    );
    action(&mut app, SemanticAction::Adjust(Direction::Right));
    apply_pending(&mut app);
    assert_ne!(app.selected_chain().unwrap().0, first_chain);
    assert_eq!(app.selected_chain().unwrap().1.name, "Chain 01");
    assert_eq!(
        project(&app).workspace.unwrap().grid[0].cells[0].value,
        "01"
    );
    action(&mut app, SemanticAction::Adjust(Direction::Left));
    apply_pending(&mut app);
    assert_eq!(app.selected_chain().unwrap().0, first_chain);
    action(
        &mut app,
        SemanticAction::SetInteractionMode(InteractionMode::Navigate),
    );
    apply_pending(&mut app);
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &empty, "one held gesture is one undo");
    app.apply(Request::Redo).unwrap();
    action(&mut app, SemanticAction::NavigatePage(Direction::Right));
    apply_pending(&mut app);
    assert_eq!(app.view, View::Chain);
    action(
        &mut app,
        SemanticAction::SetInteractionMode(InteractionMode::Adjust),
    );
    action(&mut app, SemanticAction::Adjust(Direction::Up));
    apply_pending(&mut app);
    let first_phrase = app.selected_phrase().expect("Edit creates a phrase").0;
    assert_eq!(app.selected_phrase().unwrap().1.name, "Phrase 00");
    assert!(project(&app).workspace.unwrap().grid[0].cells[0]
        .value
        .starts_with("00 / "));
    assert_eq!(app.view, View::Chain);
    action(&mut app, SemanticAction::Adjust(Direction::Right));
    apply_pending(&mut app);
    assert_ne!(app.selected_phrase().unwrap().0, first_phrase);
    assert_eq!(app.selected_phrase().unwrap().1.name, "Phrase 01");
    assert!(project(&app).workspace.unwrap().grid[0].cells[0]
        .value
        .starts_with("01 / "));
    action(&mut app, SemanticAction::Adjust(Direction::Left));
    apply_pending(&mut app);
    assert_eq!(app.selected_phrase().unwrap().0, first_phrase);
    action(
        &mut app,
        SemanticAction::SetInteractionMode(InteractionMode::Navigate),
    );
    apply_pending(&mut app);
    action(&mut app, SemanticAction::NavigatePage(Direction::Right));
    apply_pending(&mut app);
    assert_eq!(app.view, View::Phrase);
    action(
        &mut app,
        SemanticAction::SetInteractionMode(InteractionMode::Adjust),
    );
    action(&mut app, SemanticAction::Adjust(Direction::Up));
    apply_pending(&mut app);
    assert_eq!(app.selected_phrase().unwrap().1.events.len(), 1);
    action(
        &mut app,
        SemanticAction::SetInteractionMode(InteractionMode::Navigate),
    );
    apply_pending(&mut app);
    action(&mut app, SemanticAction::Navigate(Direction::Down));
    apply_pending(&mut app);
    assert_eq!(app.row, 1);
    assert!(!app.edit_held);
}
#[test]
fn documented_keyboard_page_chords_reach_chain_phrase_and_settings() {
    let mut app = fixture();
    let mut keyboard = KeyboardInputTranslator::new();
    keyboard.translate(WindowInput::key_down(WindowKey::Shift));
    for expected in [View::Chain, View::Phrase] {
        let event = keyboard
            .translate(WindowInput::key_down(WindowKey::D))
            .unwrap();
        assert!(action(&mut app, event));
        apply_pending(&mut app);
        assert_eq!(app.view, expected);
        keyboard.translate(WindowInput::key_up(WindowKey::D));
    }
    let before = app.editor.song().clone();
    action(
        &mut app,
        keyboard
            .translate(WindowInput::key_down(WindowKey::W))
            .unwrap(),
    );
    assert_eq!(app.workspace.panel, Some(Panel::PhraseSettings));
    assert_eq!(app.editor.song(), &before);
    action(&mut app, SemanticAction::NavigatePage(Direction::Down));
    let view = project(&app).workspace.unwrap();
    assert!(view.rows.iter().any(|row| row.id == "files"));
}
#[test]
fn native_synth_keeps_all_actions_except_explicit_global_menu() {
    let mut app = fixture();
    app.view = View::Instruments;
    for action_value in [
        SemanticAction::NavigatePage(Direction::Down),
        SemanticAction::Activate,
        SemanticAction::Adjust(Direction::Right),
        SemanticAction::PreviewStart,
        SemanticAction::OpenMidiSettings,
    ] {
        assert!(!action(&mut app, action_value));
    }
    assert!(project(&app).workspace.is_none());
    action(&mut app, SemanticAction::Return);
    assert!(owns_input(&app));
    assert_eq!(
        project(&app)
            .workspace
            .unwrap()
            .rows
            .iter()
            .map(|row| row.label.as_str())
            .collect::<Vec<_>>(),
        [
            "Tracker",
            "Instruments",
            "Chord Bank",
            "Mixer",
            "Sends",
            "Settings"
        ]
    );
    action(&mut app, SemanticAction::Return);
    assert!(project(&app).synth_visible());
}
#[test]
fn pointer_ids_are_validated_and_reprojection_keeps_focus_and_song_unchanged() {
    let mut app = fixture();
    let before = app.editor.song().clone();
    event(
        &mut app,
        WorkspaceEvent::Activate {
            id: "cell:99999:99999".into(),
        },
    );
    assert!(app.requests.is_empty());
    action(&mut app, SemanticAction::Return);
    action(&mut app, SemanticAction::Navigate(Direction::Down));
    let first = project(&app);
    for _ in 0..10 {
        assert_eq!(project(&app), first);
    }
    assert_eq!(first.workspace.unwrap().focus, "instruments");
    assert_eq!(app.editor.song(), &before);
}
#[test]
fn edit_gesture_is_one_undo_and_release_restores_navigation() {
    let mut app = fixture();
    for _ in 0..2 {
        app.apply(Request::Confirm).unwrap();
    }
    app.apply(Request::Confirm).unwrap();
    let before = app.editor.song().clone();
    action(
        &mut app,
        SemanticAction::SetInteractionMode(InteractionMode::Adjust),
    );
    for _ in 0..3 {
        action(&mut app, SemanticAction::Adjust(Direction::Up));
        apply_pending(&mut app);
    }
    action(
        &mut app,
        SemanticAction::SetInteractionMode(InteractionMode::Navigate),
    );
    apply_pending(&mut app);
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &before);
    assert!(!project(&app).workspace.unwrap().adjusting);
}

#[test]
fn piano_and_drum_use_the_same_navigation_and_implicit_edit_grammar() {
    for form in [PhraseForm::PianoRoll, PhraseForm::Drum] {
        let mut app = fixture();
        app.apply(Request::Confirm).unwrap();
        app.apply(Request::Confirm).unwrap();
        let (id, phrase) = app.selected_phrase().unwrap();
        let mut phrase = phrase.clone();
        phrase.form = form.clone();
        app.command(Command::PutPhrase { id, phrase }).unwrap();
        let before = app.editor.song().clone();
        if matches!(form, PhraseForm::PianoRoll) {
            let (at, pitch) = app.piano_cursor();
            action(&mut app, SemanticAction::Navigate(Direction::Right));
            action(&mut app, SemanticAction::Navigate(Direction::Up));
            apply_pending(&mut app);
            let (next, higher) = app.piano_cursor();
            assert!(next > at);
            assert_eq!(higher, pitch + 1);
        } else {
            // Fourth drum lane must not be mistaken for Tracker's first FX.
            for _ in 0..3 {
                action(&mut app, SemanticAction::Navigate(Direction::Down));
            }
            apply_pending(&mut app);
            assert_eq!(app.column, 3);
        }
        action(
            &mut app,
            SemanticAction::SetInteractionMode(InteractionMode::Adjust),
        );
        for _ in 0..2 {
            action(&mut app, SemanticAction::Adjust(Direction::Up));
            apply_pending(&mut app);
        }
        assert!(app.workspace.panel.is_none());
        assert_eq!(app.selected_phrase().unwrap().1.events.len(), 1);
        action(
            &mut app,
            SemanticAction::SetInteractionMode(InteractionMode::Navigate),
        );
        apply_pending(&mut app);
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &before);
    }
}

#[test]
fn chord_picker_cycles_while_edit_is_held_and_navigates_after_release() {
    let mut app = fixture();
    for _ in 0..3 {
        app.apply(Request::Confirm).unwrap();
    }
    app.apply(Request::Focus { row: 0, column: 3 }).unwrap();
    action(
        &mut app,
        SemanticAction::SetInteractionMode(InteractionMode::Adjust),
    );
    action(&mut app, SemanticAction::Adjust(Direction::Up));
    assert_eq!(app.workspace.panel, Some(Panel::Chords));
    assert!(app.edit_held);
    action(&mut app, SemanticAction::Adjust(Direction::Right));
    let first = focused_id(&app);
    assert!(first.starts_with("chord:"));
    action(&mut app, SemanticAction::Adjust(Direction::Right));
    assert_ne!(focused_id(&app), first);
    action(&mut app, SemanticAction::Adjust(Direction::Left));
    assert_eq!(focused_id(&app), first);
    action(
        &mut app,
        SemanticAction::SetInteractionMode(InteractionMode::Navigate),
    );
    apply_pending(&mut app);
    action(&mut app, SemanticAction::Navigate(Direction::Up));
    assert_eq!(focused_id(&app), "back");
    assert!(!app.edit_held);
}
#[test]
fn native_file_projection_rejects_stale_click_and_preserves_original_document() {
    let mut app = fixture();
    let before = app.editor.song().clone();
    app.file_dialog = serde_json::json!({"token":9,"version":2,"title":"Open Song","mode":"open","entries":[],"parent":null});
    event(
        &mut app,
        WorkspaceEvent::Activate {
            id: "dialog:9:1:cancel".into(),
        },
    );
    assert!(app.requests.is_empty());
    event(
        &mut app,
        WorkspaceEvent::Activate {
            id: "dialog:9:2:filename".into(),
        },
    );
    event(
        &mut app,
        WorkspaceEvent::Submit {
            id: "filename:9".into(),
            value: "/tmp/test.crest-song".into(),
        },
    );
    assert!(app.requests.iter().any(|request|matches!(request,Request::FileChoose{token:9,path,overwrite:false} if path=="/tmp/test.crest-song")));
    assert_eq!(app.editor.song(), &before);
}
#[test]
fn pre_restart_example_remains_valid_and_all_sound_mappings_survive() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/window-light.crest-song");
    let document = crate::persistence::Document::read(&path).unwrap();
    document.validate().unwrap();
    let mut app = App::new();
    app.replace_song(document.song.clone()).unwrap();
    assert_eq!(app.editor.song(), &document.song);
    assert_eq!(
        document.installed_patches.len(),
        document.song.instruments.len()
    );
    let serialized = serde_json::to_vec(&document).unwrap();
    let reopened: crate::persistence::Document = serde_json::from_slice(&serialized).unwrap();
    assert_eq!(reopened.synth, document.synth);
    assert_eq!(reopened.song, document.song);
}
