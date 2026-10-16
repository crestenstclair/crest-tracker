use super::*;
use crest_synth::shell::{KeyboardInputTranslator, WindowInput, WindowKey};
use crest_tracker::composition::{InstrumentId, InstrumentUse};

fn apply_pending(app: &mut App) {
    for request in std::mem::take(&mut app.requests) {
        if matches!(
            request,
            Request::ChordLibrary {
                action: crate::app::ChordLibraryAction::Refresh
            }
        ) {
            continue;
        }
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
fn blank_song_slots_scroll_and_edit_without_filling_gaps_on_navigation() {
    let mut app = fixture();
    let before = app.editor.song().clone();
    for _ in 0..75 {
        action(&mut app, SemanticAction::Navigate(Direction::Down));
        apply_pending(&mut app);
    }
    action(&mut app, SemanticAction::Navigate(Direction::Right));
    apply_pending(&mut app);
    assert_eq!((app.row, app.slot, app.track), (75, 75, 1));
    assert_eq!(app.editor.song(), &before);
    let view = project(&app).workspace.unwrap();
    assert!(view
        .grid
        .iter()
        .flat_map(|row| &row.cells)
        .any(|cell| cell.id == "cell:75:1"));
    action(&mut app, SemanticAction::Adjust(Direction::Right));
    apply_pending(&mut app);
    assert!(app.editor.song().tracks[1].slots[..75]
        .iter()
        .all(Option::is_none));
    assert!(app.editor.song().tracks[1].slots[75].is_some());
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &before);
    assert_eq!(app.row, 75);
    app.apply(Request::Confirm).unwrap();
    assert_eq!(app.view, View::Chain);
    assert!(app.editor.song().tracks[1].slots[75].is_some());
}

#[test]
fn settings_tabs_have_reachable_fields_and_restore_tab_focus() {
    let mut app = fixture();
    action(&mut app, SemanticAction::OpenMidiSettings);
    assert_eq!(focused_id(&app), "settings_music");
    action(&mut app, SemanticAction::Navigate(Direction::Right));
    assert_eq!(focused_id(&app), "settings_clock");
    let view = project(&app).workspace.unwrap();
    assert!(view.rows.iter().any(|row| row.id == "clock_delay"));
    assert!(!view.rows.iter().any(|row| row.id == "sample_folder"));
    action(&mut app, SemanticAction::Navigate(Direction::Down));
    assert_eq!(focused_id(&app), "clock_input");
    action(&mut app, SemanticAction::Navigate(Direction::Up));
    assert_eq!(focused_id(&app), "settings_clock");
    click(&mut app, "settings_files");
    action(&mut app, SemanticAction::Navigate(Direction::Down));
    assert_eq!(focused_id(&app), "sample_folder");
    for _ in 0..30 {
        action(&mut app, SemanticAction::Navigate(Direction::Down));
    }
    assert_eq!(focused_id(&app), "back");
}

#[test]
fn command_list_creates_control_event_in_selected_blank_slot() {
    let mut app = note_fixture();
    app.apply(Request::Focus { row: 5, column: 5 }).unwrap();
    let before = app.editor.song().clone();
    action(&mut app, SemanticAction::Adjust(Direction::Up));
    assert_eq!(app.workspace.panel, Some(Panel::FxCommands));
    let view = project(&app).workspace.unwrap();
    assert_eq!(view.rows[1].id, "fx_command:1");
    assert_eq!(view.rows[2].id, "fx_command:12");
    click(&mut app, "fx_command:11");
    action(
        &mut app,
        SemanticAction::SetInteractionMode(InteractionMode::Navigate),
    );
    focus_adjust(&mut app, "fx_value", Direction::Up);
    click(&mut app, "fx_apply");
    let event = app
        .selected_phrase()
        .unwrap()
        .1
        .events
        .iter()
        .find(|event| event.at == Beat::new(5, 4).unwrap())
        .unwrap();
    assert!(event.note.is_none());
    assert!(
        matches!(event.fx[2], Some(crest_tracker::composition::Fx::Tempo { tempo }) if tempo.bpm() == 130.0)
    );
    assert!(event.fx[0].is_none() && event.fx[1].is_none());
    assert!(app.workspace.panel.is_none());
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &before);
}

#[test]
fn generator_numeric_fields_are_coarse_and_individual_notes_use_octaves() {
    let mut app = note_fixture();
    click(&mut app, "phrase_settings");
    for _ in 0..3 {
        focus_adjust(&mut app, "phrase_form", Direction::Right);
    }
    click(&mut app, "back");
    click(&mut app, "gen_notes");
    focus_adjust(&mut app, "gen_note:1", Direction::Up);
    assert!(
        matches!(&app.selected_phrase().unwrap().1.form, PhraseForm::Euclidean { pitches, .. } if pitches == &vec![None, Some(72)])
    );
    focus_adjust(&mut app, "gen_note:1", Direction::Left);
    assert!(
        matches!(&app.selected_phrase().unwrap().1.form, PhraseForm::Euclidean { pitches, .. } if pitches[1] == Some(71))
    );
    click(&mut app, "back");
    action(&mut app, SemanticAction::NavigatePage(Direction::Up));
    focus_adjust(&mut app, "phrase_form", Direction::Right);
    click(&mut app, "back");
    focus_adjust(&mut app, "gen_density", Direction::Up);
    assert!(matches!(
        app.selected_phrase().unwrap().1.form,
        PhraseForm::Random { density: 60, .. }
    ));
    focus_adjust(&mut app, "gen_density", Direction::Left);
    assert!(matches!(
        app.selected_phrase().unwrap().1.form,
        PhraseForm::Random { density: 59, .. }
    ));
}

#[test]
fn piano_controller_toggles_notes_and_resizes_without_moving_them() {
    let mut app = note_fixture();
    click(&mut app, "phrase_settings");
    focus_adjust(&mut app, "phrase_form", Direction::Right);
    click(&mut app, "back");
    let note = app.selected_phrase().unwrap().1.events[0].clone();
    app.apply(Request::PianoControl {
        control: PianoControl::Cursor {
            at: note.at,
            pitch: note.note.as_ref().unwrap().pitch,
        },
    })
    .unwrap();
    action(&mut app, SemanticAction::Adjust(Direction::Right));
    apply_pending(&mut app);
    let resized = &app.selected_phrase().unwrap().1.events[0];
    assert_eq!(resized.at, note.at);
    assert_eq!(
        resized.note.as_ref().unwrap().duration,
        note.note
            .as_ref()
            .unwrap()
            .duration
            .checked_add(app.roll_grid)
            .unwrap()
    );
    action(&mut app, SemanticAction::Activate);
    apply_pending(&mut app);
    assert!(app.selected_phrase().unwrap().1.events.is_empty());
    action(&mut app, SemanticAction::Activate);
    apply_pending(&mut app);
    assert_eq!(app.selected_phrase().unwrap().1.events.len(), 1);
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
    app.view = View::Song;
    let projection = project(&app).workspace.unwrap();
    assert!(projection.grid[0].cells[0].playing);
    assert!(projection.grid.iter().all(|row| !row.playing));
    app.view = View::Chain;
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
fn song_playheads_mark_independent_track_cells_across_column_pages() {
    use crest_tracker::composition::{Chain, ChainId, ChainUse, PhraseId, PhraseUse, UseId};
    use crest_tracker::playback::{Playback, PlaybackBudget, Snapshot};
    use std::sync::Arc;

    let mut app = fixture();
    app.apply(Request::Confirm).unwrap();
    app.apply(Request::Confirm).unwrap();
    let short_phrase = app.selected_phrase().unwrap().0;
    let short_chain = app.selected_chain().unwrap().0;
    let mut song = app.editor.song().clone();
    song.phrases.get_mut(&short_phrase).unwrap().steps = 10;
    let mut long = song.phrases[&short_phrase].clone();
    long.steps = 100;
    let long_phrase = PhraseId::new(10_000).unwrap();
    let long_chain = ChainId::new(10_000).unwrap();
    song.phrases.insert(long_phrase, long);
    song.chains.insert(
        long_chain,
        Chain {
            name: "100 rows".into(),
            phrases: vec![PhraseUse {
                id: UseId::new(10_001).unwrap(),
                phrase: long_phrase,
                repeats: 1,
            }],
        },
    );
    let slot = |id, chain| {
        Some(ChainUse {
            id: UseId::new(id).unwrap(),
            chain,
            repeats: 1,
        })
    };
    song.tracks[0].slots = (20_000..20_010).map(|id| slot(id, short_chain)).collect();
    song.tracks[1].slots = vec![slot(20_010, long_chain)];
    song.tracks[8].slots = vec![slot(20_011, short_chain)];
    song.tracks[8].looped = true;
    app.editor = crest_tracker::editing::Editor::new(song).unwrap();
    let snapshot = Snapshot::compile(
        app.editor.song(),
        app.editor.revision(),
        PlaybackBudget::default(),
    )
    .unwrap();
    let mut playback = Playback::new(Arc::new(snapshot));
    playback.window(Beat::integer(14)).unwrap();
    app.play_heads = playback
        .playheads_at(Beat::new(55, 4).unwrap())
        .unwrap()
        .unwrap();
    app.playing = true;
    app.view = View::Song;
    let playing_cells = |app: &App| {
        let view = project(app).workspace.unwrap();
        assert!(
            view.grid.iter().all(|row| !row.playing),
            "Song has no shared horizontal playhead"
        );
        view.grid
            .into_iter()
            .flat_map(|row| row.cells)
            .filter(|cell| cell.playing)
            .map(|cell| cell.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(playing_cells(&app), ["cell:0:1", "cell:5:0"]);
    app.apply(Request::Focus { row: 0, column: 8 }).unwrap();
    assert_eq!(playing_cells(&app), ["cell:0:8"]);
    app.playing = false;
    assert!(playing_cells(&app).is_empty());
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
    assert_eq!(app.selected_phrase().unwrap().1.name, "00");
    assert!(project(&app).workspace.unwrap().grid[0].cells[0]
        .value
        .starts_with("00 / "));
    assert_eq!(app.view, View::Chain);
    action(&mut app, SemanticAction::Adjust(Direction::Right));
    apply_pending(&mut app);
    assert_ne!(app.selected_phrase().unwrap().0, first_phrase);
    assert_eq!(app.selected_phrase().unwrap().1.name, "01");
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
    assert!(view.rows.is_empty());
    assert_eq!(app.workspace.panel, None);
}
#[test]
fn native_synth_retains_local_actions_and_exposes_global_navigation() {
    let mut app = fixture();
    app.view = View::Instruments;
    for action_value in [
        SemanticAction::Activate,
        SemanticAction::Adjust(Direction::Right),
        SemanticAction::PreviewStart,
    ] {
        assert!(!action(&mut app, action_value));
    }
    assert!(project(&app).workspace.is_none());
    action(&mut app, SemanticAction::Return);
    assert!(owns_input(&app));
    let menu = project(&app).workspace.unwrap();
    for id in [
        "home",
        "instruments",
        "chord_library",
        "mixer",
        "sends",
        "settings",
    ] {
        assert!(menu.rows.iter().any(|r| r.id == id));
    }
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
        if matches!(form, PhraseForm::PianoRoll) {
            action(&mut app, SemanticAction::Activate);
            apply_pending(&mut app);
            action(&mut app, SemanticAction::Adjust(Direction::Up));
            assert_eq!(app.workspace.panel, Some(Panel::Notes));
        } else {
            action(&mut app, SemanticAction::Adjust(Direction::Up));
            apply_pending(&mut app);
            assert!(app.workspace.panel.is_none());
        }
        assert_eq!(app.selected_phrase().unwrap().1.events.len(), 1);
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
    assert_eq!(app.workspace.panel, Some(Panel::FxCommands));
    event(
        &mut app,
        WorkspaceEvent::Activate {
            id: "fx_command:12".into(),
        },
    );
    action(
        &mut app,
        SemanticAction::SetInteractionMode(InteractionMode::Adjust),
    );
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
    assert!(focused_id(&app).starts_with("chord:"));
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

fn click(app: &mut App, id: &str) {
    let view = project(app);
    assert!(
        view.entry.id == id
            || view.workspace.as_ref().is_some_and(|v| v
                .rows
                .iter()
                .chain(&v.inspector)
                .chain(&v.tabs)
                .any(|r| r.id == id)),
        "Invisible action {id}"
    );
    event(app, WorkspaceEvent::Activate { id: id.into() });
    apply_pending(app);
}
fn focus_adjust(app: &mut App, id: &str, direction: Direction) {
    event(app, WorkspaceEvent::Focus { id: id.into() });
    action(app, SemanticAction::Adjust(direction));
    apply_pending(app);
}
fn note_fixture() -> App {
    let mut app = fixture();
    for _ in 0..3 {
        action(&mut app, SemanticAction::Activate);
        apply_pending(&mut app);
    }
    app
}

#[test]
fn native_navigation_falls_through_and_global_settings_restores_focus() {
    let mut app = fixture();
    app.view = View::Instruments;
    action(&mut app, SemanticAction::NavigatePage(Direction::Down));
    assert_eq!(app.workspace.panel, None);
    assert!(matches!(
        app.requests.pop(),
        Some(Request::SynthAction {
            action: SemanticAction::NavigatePage(Direction::Down)
        })
    ));
    click(&mut app, "home");
    assert_eq!(app.view, View::Song);
    action(&mut app, SemanticAction::NavigatePage(Direction::Up));
    assert_eq!(app.workspace.panel, Some(Panel::Settings));
    focus_adjust(&mut app, "key_root", Direction::Right);
    assert_eq!(app.editor.song().key.root(), 1);
    focus_adjust(&mut app, "key_scale", Direction::Right);
    assert_eq!(
        app.editor.song().key.kind(),
        crest_tracker::harmony::ScaleKind::HarmonicMinor
    );
    click(&mut app, "song_settings");
    click(&mut app, "name");
    event(
        &mut app,
        WorkspaceEvent::Submit {
            id: "name".into(),
            value: "Controller Song".into(),
        },
    );
    apply_pending(&mut app);
    assert_eq!(app.editor.song().name, "Controller Song");
    assert_eq!(app.workspace.panel, Some(Panel::SongSettings));
    assert_eq!(focused_id(&app), "name");
    click(&mut app, "back");
    assert_eq!(focused_id(&app), "song_settings");
    app.view = View::Instruments;
    open(&mut app, None);
    click(&mut app, "home");
    assert_eq!(app.view, View::Song);
}
#[test]
fn controller_text_entry_updates_draft_and_cancel_preserves_name() {
    let mut app = fixture();
    action(&mut app, SemanticAction::NavigatePage(Direction::Up));
    click(&mut app, "song_settings");
    click(&mut app, "name");
    click(&mut app, "text_clear");
    focus_adjust(&mut app, "text_character", Direction::Right);
    click(&mut app, "text_append");
    click(&mut app, "text_submit");
    assert_eq!(app.editor.song().name, "A");
    click(&mut app, "name");
    click(&mut app, "text_delete");
    click(&mut app, "back");
    assert_eq!(app.editor.song().name, "A");
}
#[test]
fn chord_editor_uses_interval_keys_and_saves_only_to_shared_library() {
    let mut app = note_fixture();
    let original = app.editor.song().clone();
    click(&mut app, "menu");
    // Opening the library queues an external refresh, checked at the host boundary.
    event(
        &mut app,
        WorkspaceEvent::Activate {
            id: "chord_library".into(),
        },
    );
    app.requests.clear();
    click(&mut app, "chord_new");
    let view = project(&app).workspace.unwrap();
    assert_eq!(view.preview.as_ref().unwrap().keys.len(), 13);
    click(&mut app, "chord_key:4");
    click(&mut app, "chord_key:3");
    assert_eq!(app.workspace.chord.as_ref().unwrap().intervals, [0, 3, 7]);
    focus_adjust(&mut app, "chord_key:3", Direction::Up);
    assert_eq!(app.workspace.chord_octave, 1);
    assert_eq!(focused_id(&app), "chord_key:15");
    focus_adjust(&mut app, "chord_key:15", Direction::Down);
    assert_eq!(app.workspace.chord_octave, 0);
    assert_eq!(focused_id(&app), "chord_key:3");
    event(
        &mut app,
        WorkspaceEvent::Activate {
            id: "chord_publish".into(),
        },
    );
    assert!(app.requests.iter().any(|request| matches!(
        request,
        Request::ChordLibrary {
            action: crate::app::ChordLibraryAction::Publish { .. }
        }
    )));
    assert_eq!(app.editor.song(), &original);
}
#[test]
fn phrase_retime_generator_bake_and_minimap_share_the_same_source() {
    let mut app = note_fixture();
    let before = app.selected_phrase().unwrap().1.events[0]
        .note
        .as_ref()
        .unwrap()
        .duration;
    action(&mut app, SemanticAction::NavigatePage(Direction::Up));
    focus_adjust(&mut app, "division", Direction::Right);
    assert_eq!(
        app.selected_phrase().unwrap().1.events[0]
            .note
            .as_ref()
            .unwrap()
            .duration,
        before.checked_scale(1, 2).unwrap()
    );
    for _ in 0..3 {
        focus_adjust(&mut app, "phrase_form", Direction::Right);
    }
    click(&mut app, "generator");
    focus_adjust(&mut app, "gen_pulses", Direction::Right);
    click(&mut app, "gen_bake_piano");
    assert!(app.is_piano_roll());
    assert!(!app.selected_phrase().unwrap().1.events.is_empty());
    open(&mut app, None);
    action(&mut app, SemanticAction::Return);
    apply_pending(&mut app);
    action(&mut app, SemanticAction::Return);
    apply_pending(&mut app);
    action(&mut app, SemanticAction::Return);
    assert!(!project(&app)
        .workspace
        .unwrap()
        .rows
        .iter()
        .any(|row| row.id == "minimap"));
    open(&mut app, None);
    app.apply(Request::View {
        view: View::Minimap,
    })
    .unwrap();
    let view = project(&app).workspace.unwrap();
    assert!(!view.grid.is_empty());
    let id = view
        .grid
        .iter()
        .flat_map(|r| &r.cells)
        .find(|c| c.id.starts_with("map:"))
        .unwrap()
        .id
        .clone();
    event(&mut app, WorkspaceEvent::Activate { id });
    apply_pending(&mut app);
    let occurrence = app.occurrence();
    action(&mut app, SemanticAction::Activate);
    apply_pending(&mut app);
    assert_eq!(app.view, View::Phrase);
    assert_eq!(app.occurrence(), occurrence);
}
#[test]
fn piano_duration_selection_expression_and_quantize_survive_view_switches() {
    let mut app = note_fixture();
    action(&mut app, SemanticAction::NavigatePage(Direction::Up));
    focus_adjust(&mut app, "phrase_form", Direction::Right);
    click(&mut app, "back");
    click(&mut app, "menu");
    click(&mut app, "tools");
    click(&mut app, "notes");
    click(&mut app, "notes_selection");
    click(&mut app, "notes_all");
    click(&mut app, "back");
    click(&mut app, "notes_values");
    focus_adjust(&mut app, "notes_resize", Direction::Right);
    click(&mut app, "back");
    click(&mut app, "expression");
    click(&mut app, "expr_new");
    focus_adjust(&mut app, "expr_value", Direction::Right);
    assert_eq!(
        app.selected_phrase().unwrap().1.events[0]
            .note
            .as_ref()
            .unwrap()
            .expression
            .len(),
        1
    );
    click(&mut app, "back");
    click(&mut app, "notes_quantize");
    open(&mut app, None);
    let view = project(&app).workspace.unwrap();
    assert!(view
        .timeline
        .unwrap()
        .spans
        .iter()
        .any(|span| span.end > span.start && span.selected));
    let source = app.editor.song().clone();
    action(&mut app, SemanticAction::NavigatePage(Direction::Down));
    click(&mut app, "home");
    assert_eq!(app.editor.song(), &source);
}
#[test]
fn typed_fx_editor_replaces_only_selected_slot_and_undo_restores_it() {
    let mut app = note_fixture();
    app.column = 4;
    action(&mut app, SemanticAction::Adjust(Direction::Up));
    assert_eq!(app.workspace.panel, Some(Panel::FxCommands));
    click(&mut app, "fx_command:6");
    assert!(!project(&app)
        .workspace
        .unwrap()
        .rows
        .iter()
        .any(|row| row.id == "fx_slot"));
    focus_adjust(&mut app, "fx_value", Direction::Right);
    click(&mut app, "fx_apply");
    let event = &app.selected_phrase().unwrap().1.events[0];
    assert!(event.fx[0].is_none() && event.fx[2].is_none());
    assert!(matches!(
        event.fx[1],
        Some(crest_tracker::composition::Fx::Retrigger { count: 3, .. })
    ));
    app.apply(Request::Undo).unwrap();
    assert!(app.selected_phrase().unwrap().1.events[0]
        .fx
        .iter()
        .all(Option::is_none));
}

#[test]
fn semantic_back_preserves_panel_target_and_cancels_nested_edits() {
    let mut app = note_fixture();
    let song = app.editor.song().clone();
    click(&mut app, "menu");
    click(&mut app, "tools");
    event(&mut app, WorkspaceEvent::Focus { id: "notes".into() });
    action(&mut app, SemanticAction::Activate);
    apply_pending(&mut app);
    assert_eq!(app.workspace.panel, Some(Panel::Notes));
    action(&mut app, SemanticAction::Return);
    apply_pending(&mut app);
    assert_eq!(app.workspace.panel, Some(Panel::Tools));
    assert_eq!(focused_id(&app), "notes");
    action(&mut app, SemanticAction::Return);
    apply_pending(&mut app);
    assert_eq!(app.workspace.panel, Some(Panel::Menu));
    action(&mut app, SemanticAction::Return);
    apply_pending(&mut app);
    assert_eq!(app.view, View::Phrase);
    assert_eq!(app.editor.song(), &song);
    action(&mut app, SemanticAction::Return);
    apply_pending(&mut app);
    assert_eq!(app.view, View::Chain);
}

#[test]
fn chord_picker_retains_effect_slot_selected_in_editor() {
    let mut app = note_fixture();
    app.column = 4;
    action(&mut app, SemanticAction::Adjust(Direction::Up));
    click(&mut app, "fx_command:12");
    event(
        &mut app,
        WorkspaceEvent::Activate {
            id: "chord:default:0".into(),
        },
    );
    app.requests
        .retain(|request| !matches!(request, Request::ChordLibrary { .. }));
    apply_pending(&mut app);
    let event = &app.selected_phrase().unwrap().1.events[0];
    assert!(event.fx[0].is_none() && event.fx[2].is_none());
    assert!(matches!(
        event.fx[1],
        Some(crest_tracker::composition::Fx::ChordRevision { .. })
    ));
}

#[test]
fn triplet_grid_entry_preserves_notes_and_returns_to_selection_tools() {
    let mut app = note_fixture();
    let song = app.editor.song().clone();
    click(&mut app, "menu");
    click(&mut app, "tools");
    click(&mut app, "notes");
    click(&mut app, "notes_selection");
    click(&mut app, "notes_grid");
    event(
        &mut app,
        WorkspaceEvent::Submit {
            id: "notes_grid".into(),
            value: "1/3".into(),
        },
    );
    apply_pending(&mut app);
    assert_eq!(app.roll_grid, Beat::new(1, 3).unwrap());
    assert_eq!(app.workspace.panel, Some(Panel::NoteSelection));
    assert_eq!(app.editor.song(), &song);
}

#[test]
fn piano_stick_resize_repeats_as_one_undo_and_disconnect_ends_the_gesture() {
    use crest_synth::control::ControllerBindings;
    use crest_synth::shell::hosted_application::{HostControllerAxis, HostControllerInput};
    use std::time::Duration;
    let mut app = note_fixture();
    action(&mut app, SemanticAction::NavigatePage(Direction::Up));
    focus_adjust(&mut app, "phrase_form", Direction::Right);
    click(&mut app, "back");
    let original = app.editor.song().clone();
    let mut sticks = sticks::Sticks::default();
    let bindings = ControllerBindings::default();
    let edge = |value| HostControllerInput::Axis {
        device: 1,
        axis: HostControllerAxis::RightX,
        value,
    };
    sticks.event(
        &mut app,
        edge(25_000),
        &bindings,
        &crate::preferences::RollBindings::default(),
    );
    apply_pending(&mut app);
    sticks.advance(&mut app, Duration::from_millis(280));
    apply_pending(&mut app);
    let resized = app.editor.song().clone();
    assert_ne!(resized, original);
    sticks.event(
        &mut app,
        HostControllerInput::Disconnected { device: 1 },
        &bindings,
        &crate::preferences::RollBindings::default(),
    );
    apply_pending(&mut app);
    sticks.advance(&mut app, Duration::from_secs(1));
    assert!(app.requests.is_empty());
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &original);
    app.apply(Request::Redo).unwrap();
    assert_eq!(app.editor.song(), &resized);
    sticks.event(
        &mut app,
        edge(25_000),
        &bindings,
        &crate::preferences::RollBindings::default(),
    );
    apply_pending(&mut app);
    sticks.event(
        &mut app,
        edge(0),
        &bindings,
        &crate::preferences::RollBindings::default(),
    );
    apply_pending(&mut app);
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &resized);
}

#[test]
fn exact_note_start_edit_preserves_selection_and_rejects_out_of_phrase_time() {
    let mut app = note_fixture();
    action(&mut app, SemanticAction::NavigatePage(Direction::Up));
    focus_adjust(&mut app, "phrase_form", Direction::Right);
    click(&mut app, "back");
    click(&mut app, "menu");
    click(&mut app, "tools");
    click(&mut app, "notes");
    click(&mut app, "notes_selection");
    click(&mut app, "notes_all");
    click(&mut app, "back");
    click(&mut app, "notes_values");
    let selected = app.selected_event_ids();
    click(&mut app, "notes_start");
    event(
        &mut app,
        WorkspaceEvent::Submit {
            id: "notes_start".into(),
            value: "1/3".into(),
        },
    );
    apply_pending(&mut app);
    assert_eq!(
        app.selected_phrase().unwrap().1.events[0].at,
        Beat::new(1, 3).unwrap()
    );
    assert_eq!(app.selected_event_ids(), selected);
    let original = app.editor.song().clone();
    click(&mut app, "notes_start");
    event(
        &mut app,
        WorkspaceEvent::Submit {
            id: "notes_start".into(),
            value: "10000".into(),
        },
    );
    let request = app.requests.pop().unwrap();
    assert!(app.apply(request).is_err());
    assert_eq!(app.editor.song(), &original);
}

#[test]
fn minimap_follows_meter_changes_and_manual_scroll_can_pause_and_resume() {
    use crest_tracker::composition::MeterChange;
    use crest_tracker::time::Meter;
    let mut app = note_fixture();
    app.apply(Request::Edit {
        revision: app.editor.revision(),
        command: Box::new(Command::SetMeters {
            meters: vec![
                MeterChange {
                    at: Beat::ZERO,
                    meter: Meter::new(4, 4).unwrap(),
                },
                MeterChange {
                    at: Beat::integer(8),
                    meter: Meter::new(3, 4).unwrap(),
                },
            ],
        }),
    })
    .unwrap();
    app.view = View::Minimap;
    app.preview_bars = 2;
    follow_playback(&mut app, Beat::integer(8));
    assert_eq!(app.preview_start, Beat::integer(8));
    follow_playback(&mut app, Beat::integer(13));
    assert_eq!(app.preview_start, Beat::integer(8));
    follow_playback(&mut app, Beat::integer(14));
    assert_eq!(app.preview_start, Beat::integer(14));
    action(&mut app, SemanticAction::Adjust(Direction::Left));
    apply_pending(&mut app);
    let manual = app.preview_start;
    follow_playback(&mut app, Beat::integer(32));
    assert_eq!(app.preview_start, manual);
    app.play_beat = Some(Beat::integer(32));
    click(&mut app, "minimap_follow");
    assert_eq!(app.preview_start, Beat::integer(32));
}

#[test]
fn expression_text_edits_preserve_exact_raw_values_and_reject_outside_note() {
    let mut app = note_fixture();
    click(&mut app, "menu");
    click(&mut app, "tools");
    click(&mut app, "notes");
    click(&mut app, "expression");
    click(&mut app, "expr_new");
    for (id, value) in [("expr_offset", "1/12"), ("expr_value", "117")] {
        click(&mut app, id);
        event(
            &mut app,
            WorkspaceEvent::Submit {
                id: id.into(),
                value: value.into(),
            },
        );
        apply_pending(&mut app);
    }
    let point = &app.selected_phrase().unwrap().1.events[0]
        .note
        .as_ref()
        .unwrap()
        .expression[0];
    assert_eq!(point.offset, Beat::new(1, 12).unwrap());
    assert_eq!(
        point.control,
        crest_tracker::composition::Control::Pressure { value: 117 }
    );
    let song = app.editor.song().clone();
    click(&mut app, "expr_offset");
    event(
        &mut app,
        WorkspaceEvent::Submit {
            id: "expr_offset".into(),
            value: "9".into(),
        },
    );
    apply_pending(&mut app);
    assert_eq!(app.editor.song(), &song);
    assert_eq!(app.workspace.panel, Some(Panel::Text));
    assert!(app.status.contains("lifetime"));
}

#[test]
fn coarse_song_gesture_and_octave_pitch_follow_reviewed_layout() {
    let mut app = fixture();
    action(&mut app, SemanticAction::Adjust(Direction::Right));
    apply_pending(&mut app);
    action(&mut app, SemanticAction::Adjust(Direction::Up));
    apply_pending(&mut app);
    assert_eq!(
        project(&app).workspace.unwrap().grid[0].cells[0].value,
        "10"
    );
    action(&mut app, SemanticAction::Adjust(Direction::Down));
    apply_pending(&mut app);
    assert_eq!(
        project(&app).workspace.unwrap().grid[0].cells[0].value,
        "00"
    );
    for _ in 0..3 {
        app.apply(Request::Confirm).unwrap();
    }
    let pitch = app.selected_phrase().unwrap().1.events[0]
        .note
        .as_ref()
        .unwrap()
        .pitch;
    action(&mut app, SemanticAction::Adjust(Direction::Up));
    apply_pending(&mut app);
    assert_eq!(
        app.selected_phrase().unwrap().1.events[0]
            .note
            .as_ref()
            .unwrap()
            .pitch,
        pitch + 12
    );
    action(&mut app, SemanticAction::Adjust(Direction::Down));
    apply_pending(&mut app);
    action(&mut app, SemanticAction::Adjust(Direction::Right));
    apply_pending(&mut app);
    let next = app.selected_phrase().unwrap().1.events[0]
        .note
        .as_ref()
        .unwrap()
        .pitch;
    assert!(next > pitch && next < pitch + 12);
    assert_eq!(app.selected_phrase().unwrap().1.name, "00");
}

#[test]
fn picker_is_shared_shapes_and_cancel_only_and_applies_without_changing_root() {
    let mut app = note_fixture();
    let root = app.selected_phrase().unwrap().1.events[0]
        .note
        .as_ref()
        .unwrap()
        .pitch;
    click(&mut app, "chords");
    let picker = project(&app).workspace.unwrap();
    assert!(picker.overlay);
    assert!(picker
        .rows
        .iter()
        .all(|row| row.id.starts_with("chord:") || row.id == "chords_cancel"));
    assert_eq!(picker.rows[0].label, "Major");
    assert_eq!(picker.rows[0].value, "1 – 3 – 5");
    click(&mut app, "chord:default:0");
    assert!(app.workspace.panel.is_none());
    let note = &app.selected_phrase().unwrap().1.events[0];
    assert_eq!(note.note.as_ref().unwrap().pitch, root);
    let crest_tracker::composition::Fx::ChordRevision { revision } = note.fx[0].as_ref().unwrap()
    else {
        panic!("chord not applied");
    };
    assert_eq!(
        app.editor
            .song()
            .chords
            .get(revision)
            .unwrap()
            .definition()
            .shape()
            .intervals(),
        [0, 4, 7]
    );
    let before = app.editor.song().clone();
    click(&mut app, "chords");
    click(&mut app, "chords_cancel");
    assert!(app.workspace.panel.is_none());
    assert_eq!(app.editor.song(), &before);
}

#[test]
fn fx_apply_and_cancel_return_to_phrase_and_hold_does_not_edit_new_popup() {
    let mut app = note_fixture();
    app.column = 3;
    action(&mut app, SemanticAction::Adjust(Direction::Up));
    assert_eq!(app.workspace.fx_slot, 0);
    action(&mut app, SemanticAction::Adjust(Direction::Up));
    assert_eq!(
        app.workspace.fx_slot, 0,
        "opening direction must not repeat into a new popup"
    );
    click(&mut app, "fx_cancel");
    assert!(app.workspace.panel.is_none());
    click(&mut app, "effects");
    click(&mut app, "fx_command:1");
    click(&mut app, "fx_apply");
    assert!(app.workspace.panel.is_none());
    assert_eq!(app.view, View::Phrase);
}

#[test]
fn native_detail_navigation_is_preserved_and_mixer_returns_to_song_origin() {
    use crest_synth::control::SurfaceId;
    let mut app = fixture();
    app.row = 3;
    app.column = 2;
    action(&mut app, SemanticAction::NavigatePage(Direction::Down));
    assert!(app.requests.iter().any(|request| matches!(
        request,
        Request::SynthPage {
            page: SynthPage::Mixer
        }
    )));
    app.requests.clear();
    app.view = View::Instruments;
    assert!(!return_from_synth(
        &mut app,
        &SemanticAction::NavigatePage(Direction::Down),
        SurfaceId::PatchDetail
    ));
    assert!(return_from_synth(
        &mut app,
        &SemanticAction::NavigatePage(Direction::Up),
        SurfaceId::MixerMain
    ));
    assert_eq!((app.view, app.row, app.column), (View::Song, 3, 2));
    action(&mut app, SemanticAction::NavigatePage(Direction::Up));
    assert_eq!(app.workspace.panel, Some(Panel::Settings));
    action(&mut app, SemanticAction::NavigatePage(Direction::Down));
    assert!(app.workspace.panel.is_none());
}

#[test]
fn off_is_visible_and_drum_time_moves_horizontally() {
    let mut app = note_fixture();
    action(&mut app, SemanticAction::Navigate(Direction::Down));
    apply_pending(&mut app);
    click(&mut app, "notes_off");
    let phrase = app.selected_phrase().unwrap().1;
    assert!(phrase.events.iter().any(|event| event.is_note_off()));
    assert_eq!(
        project(&app).workspace.unwrap().grid[1].cells[0].value,
        "OFF"
    );
    let (id, phrase) = app.selected_phrase().unwrap();
    let mut phrase = phrase.clone();
    phrase.form = PhraseForm::Drum;
    app.command(Command::PutPhrase { id, phrase }).unwrap();
    app.row = 0;
    app.column = 0;
    action(&mut app, SemanticAction::Navigate(Direction::Right));
    apply_pending(&mut app);
    assert_eq!((app.row, app.column), (1, 0));
    action(&mut app, SemanticAction::Navigate(Direction::Down));
    apply_pending(&mut app);
    assert_eq!((app.row, app.column), (1, 1));
    assert_eq!(
        project(&app).workspace.unwrap().grid[1].cells[1].id,
        "cell:1:1"
    );
}

#[test]
fn generator_forms_have_primary_controls_and_compiled_preview() {
    let mut app = note_fixture();
    click(&mut app, "phrase_settings");
    for _ in 0..3 {
        focus_adjust(&mut app, "phrase_form", Direction::Right);
    }
    click(&mut app, "back");
    let view = project(&app).workspace.unwrap();
    assert_eq!(view.title, "EUCLIDEAN");
    assert_eq!(view.rows[0].id, "gen_pulses");
    assert!(view.grid[0].cells.iter().any(|cell| cell.selected));
    focus_adjust(&mut app, "gen_pulses", Direction::Right);
    assert!(matches!(
        app.selected_phrase().unwrap().1.form,
        PhraseForm::Euclidean { pulses: 5, .. }
    ));
    action(&mut app, SemanticAction::NavigatePage(Direction::Up));
    focus_adjust(&mut app, "phrase_form", Direction::Right);
    click(&mut app, "back");
    assert_eq!(project(&app).workspace.unwrap().title, "RANDOMIZER");
    let before = app.selected_phrase().unwrap().1.form.clone();
    click(&mut app, "gen_regenerate");
    assert_ne!(app.selected_phrase().unwrap().1.form, before);
    focus_adjust(&mut app, "gen_repeat", Direction::Right);
    assert!(matches!(
        app.selected_phrase().unwrap().1.form,
        PhraseForm::Random { vary: true, .. }
    ));
}

#[test]
fn piano_right_stick_velocity_uses_coarse_and_fine_events_without_transposing() {
    use crest_synth::control::ControllerBindings;
    use crest_synth::shell::hosted_application::{HostControllerAxis, HostControllerInput};
    let mut app = note_fixture();
    click(&mut app, "phrase_settings");
    focus_adjust(&mut app, "phrase_form", Direction::Right);
    click(&mut app, "back");
    let before = app.selected_phrase().unwrap().1.events[0].clone();
    let mut sticks = sticks::Sticks::default();
    let bindings = ControllerBindings::default();
    let roll = crate::preferences::RollBindings::default();
    let axis = |value| HostControllerInput::Axis {
        device: 1,
        axis: HostControllerAxis::RightY,
        value,
    };
    sticks.event(&mut app, axis(-25_000), &bindings, &roll);
    apply_pending(&mut app);
    assert_eq!(
        app.selected_phrase().unwrap().1.events[0]
            .note
            .as_ref()
            .unwrap()
            .velocity,
        110
    );
    sticks.event(&mut app, axis(0), &bindings, &roll);
    sticks.event(
        &mut app,
        HostControllerInput::Pressed {
            device: 1,
            button: roll.fine,
        },
        &bindings,
        &roll,
    );
    assert!(app.fine_held);
    sticks.event(&mut app, axis(-25_000), &bindings, &roll);
    apply_pending(&mut app);
    let note = app.selected_phrase().unwrap().1.events[0]
        .note
        .as_ref()
        .unwrap();
    assert_eq!(note.velocity, 111);
    assert_eq!(note.pitch, before.note.unwrap().pitch);
    sticks.event(
        &mut app,
        HostControllerInput::Disconnected { device: 1 },
        &bindings,
        &roll,
    );
    assert!(!app.fine_held);
}
