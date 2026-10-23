use super::*;
use crest_synth::shell::{KeyboardInputTranslator, WindowInput, WindowKey};
use crest_tracker::composition::{InstrumentId, InstrumentUse};

#[test]
fn loading_modal_owns_tracker_and_native_input_without_buffering() {
    for view in [View::Song, View::Instruments] {
        let mut app = App::new();
        app.view = view;
        app.load_progress = Some("Preparing instruments and effects…");
        let before = app.editor.song().clone();
        let projection = project(&app);
        let modal = projection.workspace.unwrap();
        assert!(modal.busy && modal.overlay);
        assert_eq!(modal.context, "LOADING SONG");
        assert!(!projection.entry.enabled);
        assert!(modal.rows.is_empty() && modal.text_entry.is_none());
        assert!(modal.hints.contains("Please wait"));
        assert!(owns_input(&app));
        for action in [
            SemanticAction::Navigate(Direction::Down),
            SemanticAction::NavigatePage(Direction::Up),
            SemanticAction::Adjust(Direction::Right),
            SemanticAction::Return,
            SemanticAction::OpenMidiSettings,
            SemanticAction::OpenPopover,
            SemanticAction::PreviewStart,
            SemanticAction::SelectContext(TopLevelContext::Mixer),
            SemanticAction::SetInteractionMode(InteractionMode::Adjust),
        ] {
            assert!(input::action(&mut app, action.clone()));
            assert!(input::apply(&mut app, input::Input::Action(action)).is_none());
        }
        for id in ["home", "instruments", "menu", "transport", "cell:1:0"] {
            let event = WorkspaceEvent::Activate { id: id.into() };
            input::enqueue(&mut app, input::Input::Event(event.clone()));
            super::event(&mut app, event);
        }
        assert!(app.requests.is_empty());
        assert_eq!(app.view, view);
        assert!(app.workspace.panel.is_none());
        assert!(!app.edit_held);
        assert_eq!(app.editor.song(), &before);
        app.load_progress = None;
        assert!(input::action(&mut app, SemanticAction::OpenMidiSettings));
        assert_eq!(app.requests.len(), 1);
    }
}

pub(super) fn apply_pending(app: &mut App) {
    let mut pending: std::collections::VecDeque<_> = std::mem::take(&mut app.requests).into();
    while let Some(request) = pending.pop_front() {
        match request {
            Request::WorkspaceInput { input } => {
                assert!(input::apply(app, input).is_none());
            }
            Request::ChordLibrary {
                action: crate::app::ChordLibraryAction::Refresh,
            } => {}
            request => {
                app.apply(request).unwrap();
            }
        }
        for child in std::mem::take(&mut app.requests).into_iter().rev() {
            pending.push_front(child);
        }
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
fn playing_native_synth_queues_input_without_changing_its_owner() {
    let mut app = App::new();
    app.view = View::Instruments;
    let adjust = SemanticAction::Adjust(Direction::Right);
    assert!(!owns_input(&app));
    assert!(
        !input::action(&mut app, adjust.clone()),
        "idle Synth keeps its direct route"
    );
    app.playing = true;
    assert!(input::action(&mut app, adjust.clone()));
    assert_eq!(app.requests.len(), 1);
    let Request::WorkspaceInput { input: queued } = app.requests.pop().unwrap() else {
        panic!("expected ordered semantic input");
    };
    assert_eq!(
        input::apply(&mut app, queued),
        Some(adjust),
        "Synth still owns the edit"
    );
    assert!(app.playing);
}

#[test]
fn start_is_transport_in_every_workspace_and_overlay_even_with_shift_held() {
    use crest_synth::control::{ControllerBindings, ControllerButton};
    use crest_synth::shell::hosted_application::HostControllerInput;
    for view in [View::Song, View::Chain, View::Phrase, View::Instruments] {
        for panel in [
            None,
            Some(Panel::Menu),
            Some(Panel::Settings),
            Some(Panel::Text),
            Some(Panel::Effects),
        ] {
            for shifted in [false, true] {
                let mut app = fixture();
                app.view = view;
                app.workspace.panel = panel;
                app.workspace.focus = Some("unchanged".into());
                let mut keyboard = KeyboardInputTranslator::new().with_host_navigation(true);
                if shifted {
                    keyboard.translate(WindowInput::key_down(WindowKey::Shift));
                }
                for event in [
                    WindowInput::key_down(WindowKey::Space),
                    WindowInput::key_down(WindowKey::Space),
                    WindowInput::key_up(WindowKey::Space),
                ] {
                    if let Some(action) = keyboard.translate(event) {
                        assert!(input::action(&mut app, action));
                    }
                }
                for request in std::mem::take(&mut app.requests) {
                    let Request::WorkspaceInput { input } = request else {
                        panic!("expected input")
                    };
                    assert!(input::apply(&mut app, input).is_none());
                }
                assert_eq!(app.requests.len(), 1);
                assert!(matches!(app.requests.pop(), Some(Request::Play)));
                let mut sticks = sticks::Sticks::default();
                let bindings = ControllerBindings::default();
                let roll = crate::preferences::RollBindings::default();
                if shifted {
                    sticks.event(
                        &mut app,
                        HostControllerInput::Pressed {
                            device: 1,
                            button: bindings.button(crest_synth::control::ControllerRole::Shift),
                        },
                        &bindings,
                        &roll,
                    );
                }
                for event in [
                    HostControllerInput::Pressed {
                        device: 1,
                        button: ControllerButton::Start,
                    },
                    HostControllerInput::Pressed {
                        device: 1,
                        button: ControllerButton::Start,
                    },
                    HostControllerInput::Released {
                        device: 1,
                        button: ControllerButton::Start,
                    },
                ] {
                    assert!(sticks.event(&mut app, event, &bindings, &roll));
                }
                for request in std::mem::take(&mut app.requests) {
                    let Request::WorkspaceInput { input } = request else {
                        panic!("expected input")
                    };
                    assert!(input::apply(&mut app, input).is_none());
                }
                assert_eq!(app.requests.len(), 1);
                assert!(matches!(app.requests.pop(), Some(Request::Play)));
                assert_eq!(app.view, view);
                assert_eq!(app.workspace.panel, panel);
                assert_eq!(app.workspace.focus.as_deref(), Some("unchanged"));
            }
        }
    }
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
    action(&mut app, SemanticAction::OpenPopover);
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
fn piano_controller_selects_without_deleting_and_nudges_without_resizing() {
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
    let moved = &app.selected_phrase().unwrap().1.events[0];
    assert_eq!(moved.at, note.at.checked_add(app.roll_grid).unwrap());
    assert_eq!(
        moved.note.as_ref().unwrap().duration,
        note.note.as_ref().unwrap().duration
    );
    let before = app.editor.song().clone();
    action(&mut app, SemanticAction::Activate);
    apply_pending(&mut app);
    assert_eq!(app.editor.song(), &before);
    app.apply(Request::Clear).unwrap();
    assert!(app.selected_phrase().unwrap().1.events.is_empty());
    action(&mut app, SemanticAction::Activate);
    apply_pending(&mut app);
    assert_eq!(app.selected_phrase().unwrap().1.events.len(), 1);
}

#[test]
fn piano_projection_bounds_extreme_grids_and_keeps_exact_separate_cursors() {
    let mut app = note_fixture();
    click(&mut app, "phrase_settings");
    focus_adjust(&mut app, "phrase_form", Direction::Right);
    click(&mut app, "back");
    let note = app.selected_phrase().unwrap().1.events[0].clone();
    let (phrase, _) = app.selected_phrase().unwrap();
    app.command(Command::ResizePhrase {
        phrase,
        steps: 400,
        trim: false,
    })
    .unwrap();
    let mut sustained = note.clone();
    sustained.note.as_mut().unwrap().duration = Beat::integer(40);
    app.command(Command::PutEvent {
        phrase,
        event: sustained,
    })
    .unwrap();
    let before = app.editor.song().clone();
    app.apply(Request::PianoControl {
        control: PianoControl::Cursor {
            at: Beat::new(5, 3).unwrap(),
            pitch: note.note.unwrap().pitch,
        },
    })
    .unwrap();
    app.playing = true;
    app.play_phrase_beat = Some(Beat::integer(3));
    for grid in [
        Beat::new(1, u64::MAX).unwrap(),
        Beat::integer(u64::MAX),
        Beat::new(1, 3).unwrap(),
    ] {
        app.apply(Request::PianoControl {
            control: PianoControl::Grid { grid },
        })
        .unwrap();
        let view = project(&app).workspace.unwrap();
        assert!(view.columns.len() <= 64);
        let timeline = view.timeline.unwrap();
        assert!(timeline.cursor.is_some() && timeline.playhead.is_some());
        assert_ne!(timeline.cursor, timeline.playhead);
        assert!(timeline.spans[0].label.ends_with('›'));
        assert_eq!(app.editor.song(), &before);
    }
    app.apply(Request::PianoControl {
        control: PianoControl::Grid {
            grid: Beat::new(1, 4).unwrap(),
        },
    })
    .unwrap();
    app.apply(Request::PianoControl {
        control: PianoControl::Scroll { delta: 2 },
    })
    .unwrap();
    let timeline = project(&app).workspace.unwrap().timeline.unwrap();
    assert!(timeline.cursor.is_none() && timeline.playhead.is_none());
    assert!(timeline.spans[0].label.starts_with('‹'));
    assert_eq!(timeline.spans[0].start, 0);
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
    let slot = |id, chain: ChainId| {
        Some(ChainUse {
            id: UseId::new(id).unwrap(),
            chain: chain.into(),
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
    assert_eq!(
        project(&app).workspace.unwrap().grid[0].cells[0].value,
        "00"
    );
    assert_eq!(app.view, View::Chain);
    action(&mut app, SemanticAction::Adjust(Direction::Right));
    apply_pending(&mut app);
    assert_ne!(app.selected_phrase().unwrap().0, first_phrase);
    assert_eq!(app.selected_phrase().unwrap().1.name, "01");
    assert_eq!(
        project(&app).workspace.unwrap().grid[0].cells[0].value,
        "01"
    );
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
    ] {
        assert!(!action(&mut app, action_value));
    }
    assert!(project(&app).workspace.is_none());
    action(&mut app, SemanticAction::Return);
    assert!(matches!(
        app.requests.last(),
        Some(Request::SynthAction {
            action: SemanticAction::Return
        })
    ));
    assert!(return_from_synth(
        &mut app,
        &SemanticAction::Return,
        crest_synth::control::SurfaceId::PatchMain
    ));
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
            action(&mut app, SemanticAction::OpenPopover);
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
    action(&mut app, SemanticAction::OpenPopover);
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
fn file_picker_pages_by_visible_rows_and_rejects_stale_layout_without_editing() {
    let mut app = fixture();
    let before = app.editor.song().clone();
    // A file dialog owns navigation even over a page with spatial controls.
    app.workspace.panel = Some(Panel::ChordEditor);
    let entries: Vec<_> = (0..30)
        .map(|index| {
            serde_json::json!({
                "name": format!("Song {index:02}.crest-song"), "directory": false
            })
        })
        .collect();
    app.file_dialog = serde_json::json!({
        "token": 9, "version": 2, "title": "Open Song", "mode": "open",
        "entries": entries, "parent": null
    });
    let rows = project(&app).workspace.unwrap().rows;
    let first = rows[0].id.clone();
    event(
        &mut app,
        WorkspaceEvent::ListPageSize {
            id: first.clone(),
            rows: 7,
        },
    );
    assert!(control_hints(&app).contains("←/→: page up/down"));
    action(&mut app, SemanticAction::Navigate(Direction::Right));
    assert_eq!(focused_id(&app), rows[7].id);
    action(&mut app, SemanticAction::Navigate(Direction::Down));
    assert_eq!(focused_id(&app), rows[8].id);
    action(&mut app, SemanticAction::Navigate(Direction::Up));
    assert_eq!(focused_id(&app), rows[7].id);
    action(&mut app, SemanticAction::Navigate(Direction::Left));
    assert_eq!(focused_id(&app), first);
    for (id, size) in [
        ("stale-list".to_owned(), 1),
        (first.clone(), 0),
        (first.clone(), usize::MAX),
    ] {
        event(&mut app, WorkspaceEvent::ListPageSize { id, rows: size });
    }
    action(&mut app, SemanticAction::Navigate(Direction::Right));
    assert_eq!(focused_id(&app), rows[7].id);
    // Resizing the viewport changes the next page jump.
    event(
        &mut app,
        WorkspaceEvent::ListPageSize {
            id: first.clone(),
            rows: 4,
        },
    );
    action(&mut app, SemanticAction::Navigate(Direction::Right));
    assert_eq!(focused_id(&app), rows[11].id);
    for _ in 0..10 {
        action(&mut app, SemanticAction::Navigate(Direction::Right));
    }
    assert_eq!(focused_id(&app), rows.last().unwrap().id);
    for _ in 0..10 {
        action(&mut app, SemanticAction::Navigate(Direction::Left));
    }
    assert_eq!(focused_id(&app), first);
    assert!(
        app.requests.is_empty(),
        "paging never activates a file or edits the Song"
    );
    assert_eq!(app.editor.song(), &before);
}

#[test]
fn ordinary_menus_page_and_settings_tabs_keep_horizontal_navigation() {
    let mut app = fixture();
    action(&mut app, SemanticAction::NavigatePage(Direction::Down));
    let rows = project(&app).workspace.unwrap().rows;
    action(&mut app, SemanticAction::Navigate(Direction::Right));
    assert_eq!(focused_id(&app), rows[10.min(rows.len() - 1)].id);
    action(&mut app, SemanticAction::Navigate(Direction::Left));
    assert_eq!(focused_id(&app), rows[0].id);
    action(&mut app, SemanticAction::OpenMidiSettings);
    action(&mut app, SemanticAction::Navigate(Direction::Right));
    assert_eq!(focused_id(&app), "settings_clock");
    action(&mut app, SemanticAction::Navigate(Direction::Down));
    let rows = project(&app).workspace.unwrap().rows;
    event(
        &mut app,
        WorkspaceEvent::ListPageSize {
            id: rows[0].id.clone(),
            rows: 3,
        },
    );
    action(&mut app, SemanticAction::Navigate(Direction::Right));
    assert_eq!(focused_id(&app), rows[3].id);
    action(&mut app, SemanticAction::Navigate(Direction::Left));
    assert_eq!(focused_id(&app), rows[0].id);
    action(&mut app, SemanticAction::Navigate(Direction::Up));
    assert_eq!(focused_id(&app), "settings_clock");
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

fn control_hints(app: &App) -> String {
    project(app).workspace.unwrap().hints
}

#[test]
fn hints_follow_song_slots_chain_columns_and_playback() {
    let mut app = fixture();
    assert!(control_hints(&app).contains("create chain and open"));
    action(&mut app, SemanticAction::Activate);
    apply_pending(&mut app);
    assert!(control_hints(&app).contains("phrase number"));
    action(&mut app, SemanticAction::Navigate(Direction::Right));
    apply_pending(&mut app);
    assert!(control_hints(&app).contains("repeats — ←/→: −1/+1 · ↑/↓: +10/−10"));
    action(&mut app, SemanticAction::NavigatePage(Direction::Left));
    apply_pending(&mut app);
    assert!(control_hints(&app).contains("Enter: open chain"));
    assert!(!control_hints(&app).contains("Enter: create"));
    app.playing = true;
    assert!(control_hints(&app).contains("Space: stop playback"));
}

#[test]
fn hints_follow_phrase_columns_and_fx_command_availability() {
    let mut app = note_fixture();
    assert!(control_hints(&app).contains("octave"));
    app.apply(Request::Focus { row: 0, column: 2 }).unwrap();
    assert!(control_hints(&app).contains("velocity — ←/→: −1/+1 · ↑/↓: +10/−10"));
    assert!(!control_hints(&app).contains("octave"));
    action(
        &mut app,
        SemanticAction::SetInteractionMode(InteractionMode::Adjust),
    );
    assert!(control_hints(&app).starts_with("EDIT HELD"));
    action(&mut app, SemanticAction::Adjust(Direction::Up));
    apply_pending(&mut app);
    assert_eq!(
        effects::selected_event(&app)
            .unwrap()
            .note
            .unwrap()
            .velocity,
        110
    );
    app.apply(Request::Focus { row: 0, column: 3 }).unwrap();
    assert!(control_hints(&app).contains("West / [: FX command list"));
    action(&mut app, SemanticAction::OpenPopover);
    assert_eq!(app.workspace.panel, Some(Panel::FxCommands));
    click(&mut app, "fx_command:4");
    assert!(control_hints(&app).contains("Cut after beats"));
    assert!(control_hints(&app).contains("+10/−10 × 1/16 beat"));
    event(
        &mut app,
        WorkspaceEvent::Focus {
            id: "fx_apply".into(),
        },
    );
    assert!(control_hints(&app).contains("apply FX; return to Phrase"));
    assert!(!control_hints(&app).contains("Edit+direction"));
}

#[test]
fn hints_distinguish_settings_tabs_numbers_choices_and_actions() {
    let mut app = fixture();
    action(&mut app, SemanticAction::OpenMidiSettings);
    assert!(control_hints(&app).contains("previous/next tab"));
    assert!(!control_hints(&app).contains("Edit+direction"));
    action(&mut app, SemanticAction::Navigate(Direction::Down));
    assert!(control_hints(&app).contains("↑: tabs"));
    assert!(control_hints(&app).contains("Current tempo: ←/→: −1/+1 · ↑/↓: +10/−10"));
    event(
        &mut app,
        WorkspaceEvent::Focus {
            id: "key_scale".into(),
        },
    );
    assert!(control_hints(&app).contains("previous choice"));
    assert!(!control_hints(&app).contains("+10/−10"));
    event(&mut app, WorkspaceEvent::Focus { id: "back".into() });
    assert!(control_hints(&app).contains("Enter: Back"));
    assert!(!control_hints(&app).contains("Edit+direction"));
}

#[test]
fn hints_follow_piano_stick_value_modifiers_and_note_occupancy() {
    let mut app = note_fixture();
    click(&mut app, "phrase_settings");
    focus_adjust(&mut app, "phrase_form", Direction::Right);
    click(&mut app, "back");
    assert!(app.is_piano_roll());
    assert!(control_hints(&app).contains("Enter: select note"));
    assert!(control_hints(&app).contains("↑/↓: velocity +10/−10"));
    app.fine_held = true;
    app.range_held = true;
    app.roll_value = crate::app::PianoValue::ReleaseVelocity;
    let hints = control_hints(&app);
    assert!(hints.contains("extend selection"));
    assert!(hints.contains("nudge by 1/16 grid"));
    assert!(hints.contains("↑/↓: release +1/−1"));
    app.apply(Request::PianoControl {
        control: PianoControl::Cursor {
            at: Beat::ONE,
            pitch: 80,
        },
    })
    .unwrap();
    assert!(control_hints(&app).contains("Enter: insert note"));
    app.clear_event_selection();
    assert!(!control_hints(&app).contains("shorten/lengthen"));
    assert!(control_hints(&app).contains("Insert/select a note"));
}

#[test]
fn hints_follow_chord_interval_focus_and_text_entry_ownership() {
    let mut app = fixture();
    chord_editor::begin(&mut app, None);
    event(
        &mut app,
        WorkspaceEvent::Focus {
            id: "chord_key:4".into(),
        },
    );
    assert!(control_hints(&app).contains("toggle interval"));
    assert!(control_hints(&app).contains("interval octave +1/−1"));
    event(
        &mut app,
        WorkspaceEvent::Focus {
            id: "chord_publish".into(),
        },
    );
    assert!(control_hints(&app).contains("Save chord"));
    assert!(!control_hints(&app).contains("Edit+direction"));
    text_entry(&mut app, "chord_name", "Name", "Test");
    assert!(control_hints(&app).contains("previous character"));
    assert!(control_hints(&app).contains("Keyboard: type text · Enter: accept"));
    assert!(!control_hints(&app).contains(" / K"));
    assert!(!control_hints(&app).contains("Space:"));
    event(
        &mut app,
        WorkspaceEvent::Focus {
            id: "text_submit".into(),
        },
    );
    assert!(!control_hints(&app).contains("previous character"));
}

#[test]
fn hints_cover_generator_actions_without_inventing_adjustments() {
    let mut app = note_fixture();
    click(&mut app, "phrase_settings");
    for _ in 0..4 {
        focus_adjust(&mut app, "phrase_form", Direction::Right);
    }
    click(&mut app, "back");
    event(
        &mut app,
        WorkspaceEvent::Focus {
            id: "gen_density".into(),
        },
    );
    assert!(control_hints(&app).contains("Density %: ←/→: −1/+1 · ↑/↓: +10/−10"));
    event(
        &mut app,
        WorkspaceEvent::Focus {
            id: "gen_regenerate".into(),
        },
    );
    assert!(control_hints(&app).contains("generate a new preview"));
    assert!(control_hints(&app).contains("previous seed"));
    app.file_dialog = serde_json::json!({"token":9,"version":2,"title":"Open Song","mode":"open","entries":[],"parent":null});
    assert!(!control_hints(&app).contains("Density"));
    assert!(!control_hints(&app).contains("Edit+direction"));
    assert!(control_hints(&app).contains("back / cancel"));
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
    click(&mut app, "menu");
    click(&mut app, "home");
    assert_eq!(app.view, View::Song);
    action(&mut app, SemanticAction::OpenMidiSettings);
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
    click(&mut app, "menu");
    click(&mut app, "home");
    assert_eq!(app.view, View::Song);
}
#[test]
fn controller_text_entry_updates_draft_and_cancel_preserves_name() {
    let mut app = fixture();
    action(&mut app, SemanticAction::NavigatePage(Direction::Up));
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
    click(&mut app, "notes_all");
    click(&mut app, "piano_inspect");
    choose_piano_time(&mut app, "piano_length", 8, false);
    click(&mut app, "piano_apply");
    click(&mut app, "notes");
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
    click(&mut app, "piano_quantize");
    click(&mut app, "piano_apply");
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
    action(&mut app, SemanticAction::OpenPopover);
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
    action(&mut app, SemanticAction::OpenPopover);
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
    click(&mut app, "notes_all");
    click(&mut app, "piano_inspect");
    let selected = app.selected_event_ids();
    click(&mut app, "piano_start");
    event(
        &mut app,
        WorkspaceEvent::Submit {
            id: "piano_start".into(),
            value: "1/3".into(),
        },
    );
    apply_pending(&mut app);
    click(&mut app, "piano_apply");
    assert_eq!(
        app.selected_phrase().unwrap().1.events[0].at,
        Beat::new(1, 3).unwrap()
    );
    assert_eq!(app.selected_event_ids(), selected);
    let original = app.editor.song().clone();
    click(&mut app, "notes");
    click(&mut app, "piano_inspect");
    click(&mut app, "piano_start");
    event(
        &mut app,
        WorkspaceEvent::Submit {
            id: "piano_start".into(),
            value: "10000".into(),
        },
    );
    apply_pending(&mut app);
    assert!(app.status.contains("inside phrase"));
    assert_eq!(app.workspace.panel, Some(Panel::Text));
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
    action(&mut app, SemanticAction::OpenPopover);
    assert_eq!(app.workspace.fx_slot, 0);
    action(&mut app, SemanticAction::OpenPopover);
    assert_eq!(
        app.workspace.fx_slot, 0,
        "opening another popover must not change the FX slot"
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
fn native_roots_open_menu_and_return_without_changing_underlying_page() {
    use crest_synth::control::SurfaceId;
    let mut app = fixture();
    app.view = View::Instruments;
    app.row = 3;
    app.column = 2;
    assert!(!return_from_synth(
        &mut app,
        &SemanticAction::NavigatePage(Direction::Down),
        SurfaceId::PatchDetail
    ));
    for surface in [SurfaceId::MixerMain, SurfaceId::PatchMain, SurfaceId::Sends] {
        assert!(return_from_synth(
            &mut app,
            &SemanticAction::NavigatePage(Direction::Down),
            surface
        ));
        assert_eq!(app.workspace.panel, Some(Panel::Menu));
        action(&mut app, SemanticAction::NavigatePage(Direction::Up));
        assert_eq!(app.workspace.panel, None);
        assert_eq!((app.view, app.row, app.column), (View::Instruments, 3, 2));
        assert!(!app
            .requests
            .iter()
            .any(|r| matches!(r, Request::View { .. } | Request::SynthPage { .. })));
    }
}

#[test]
fn shift_detail_and_menu_navigation_restore_tracker_page_and_focus() {
    let mut app = note_fixture();
    for (view, panel) in [
        (View::Song, Panel::SongSettings),
        (View::Chain, Panel::ChainSettings),
        (View::Phrase, Panel::PhraseSettings),
    ] {
        app.view = view;
        app.workspace.focus = None;
        let focus = focused_id(&app);
        action(&mut app, SemanticAction::NavigatePage(Direction::Up));
        assert_eq!(app.workspace.panel, Some(panel));
        action(&mut app, SemanticAction::NavigatePage(Direction::Down));
        assert_eq!(app.workspace.panel, None);
        assert_eq!(focused_id(&app), focus);
        action(&mut app, SemanticAction::NavigatePage(Direction::Down));
        assert_eq!(app.workspace.panel, Some(Panel::Menu));
        assert!(!project(&app)
            .workspace
            .unwrap()
            .rows
            .iter()
            .any(|row| row.id == "jump"));
        action(&mut app, SemanticAction::NavigatePage(Direction::Up));
        assert_eq!(app.workspace.panel, None);
        assert_eq!(app.view, view);
        assert_eq!(focused_id(&app), focus);
    }
    app.view = View::Chain;
    action(&mut app, SemanticAction::NavigatePage(Direction::Up));
    click(&mut app, "arr_chain_name");
    event(
        &mut app,
        WorkspaceEvent::Submit {
            id: "arr_chain_name".into(),
            value: "Verse".into(),
        },
    );
    apply_pending(&mut app);
    assert_eq!(app.selected_chain().unwrap().1.name, "Verse");
}

#[test]
fn chord_editor_follows_visible_columns_and_reaches_all_controls() {
    let mut app = fixture();
    chord_editor::begin(&mut app, None);
    action(&mut app, SemanticAction::Navigate(Direction::Down));
    action(&mut app, SemanticAction::Navigate(Direction::Down));
    assert_eq!(focused_id(&app), "chord_octave");
    action(&mut app, SemanticAction::Navigate(Direction::Down));
    assert_eq!(focused_id(&app), "chord_octave");
    action(&mut app, SemanticAction::Navigate(Direction::Right));
    assert_eq!(focused_id(&app), "chord_inversion");
    assert!(control_hints(&app).contains("interval keyboard"));
    action(&mut app, SemanticAction::Navigate(Direction::Up));
    assert_eq!(focused_id(&app), "chord_key:0");
    action(&mut app, SemanticAction::Navigate(Direction::Right));
    assert_eq!(focused_id(&app), "chord_key:1");
    action(&mut app, SemanticAction::Activate);
    assert!(app.workspace.chord.as_ref().unwrap().intervals.contains(&1));
    action(&mut app, SemanticAction::Navigate(Direction::Down));
    for id in [
        "chord_inversion",
        "chord_open",
        "chord_spread",
        "chord_publish",
        "back",
    ] {
        assert_eq!(focused_id(&app), id);
        assert!(project(&app)
            .workspace
            .unwrap()
            .preview
            .unwrap()
            .controls
            .iter()
            .any(|r| r.id == id));
        action(&mut app, SemanticAction::Navigate(Direction::Down));
    }
    action(&mut app, SemanticAction::Activate);
    assert_eq!(app.workspace.panel, None);
}

#[test]
fn global_defaults_and_song_patch_capacity_have_distinct_owners() {
    let mut app = fixture();
    action(&mut app, SemanticAction::OpenMidiSettings);
    assert!(!project(&app)
        .workspace
        .unwrap()
        .rows
        .iter()
        .any(|r| r.id == "default_tempo"));
    click(&mut app, "settings_defaults");
    assert!(project(&app)
        .workspace
        .unwrap()
        .rows
        .iter()
        .any(|r| r.id == "default_tempo"));
    action(&mut app, SemanticAction::Navigate(Direction::Right));
    assert_eq!(focused_id(&app), "settings_music");
    action(&mut app, SemanticAction::NavigatePage(Direction::Down));
    app.workspace.patch_capacity = Some(crest_synth::shell::hosted_synth::HostedPatchCapacity {
        limit: 32,
        installed: 2,
        maximum: 32,
    });
    action(&mut app, SemanticAction::NavigatePage(Direction::Up));
    event(
        &mut app,
        WorkspaceEvent::Focus {
            id: "patch_capacity".into(),
        },
    );
    action(&mut app, SemanticAction::Adjust(Direction::Down));
    assert!(matches!(
        app.requests.last(),
        Some(Request::PatchCapacity { capacity: 22 })
    ));
    assert!(control_hints(&app).contains("+10/−10"));
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
    apply_pending(&mut app);
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
    apply_pending(&mut app);
    assert!(!app.fine_held);
}

fn gadget_fixture() -> App {
    let mut app = note_fixture();
    phrase::adjust(&mut app, "phrase_form", 1);
    apply_pending(&mut app);
    assert!(app.is_piano_roll());
    app.apply(Request::PianoControl {
        control: PianoControl::Cursor {
            at: Beat::ZERO,
            pitch: 60,
        },
    })
    .unwrap();
    app.apply(Request::Confirm).unwrap();
    app
}
fn choose_piano_time(app: &mut App, opener: &str, denominator: u64, triplet: bool) {
    click(app, opener);
    click(
        app,
        if triplet {
            "piano_triplet"
        } else {
            "piano_straight"
        },
    );
    click(app, &format!("piano_time:{denominator}"));
    click(app, "piano_time_apply");
}

#[test]
fn gadget_grid_defaults_and_selected_lengths_are_independent_and_cancelable() {
    let mut app = gadget_fixture();
    let original = app.editor.song().clone();
    click(&mut app, "piano_grid");
    click(&mut app, "piano_triplet");
    click(&mut app, "piano_time:8");
    assert_eq!(app.roll_grid, Beat::new(1, 4).unwrap());
    click(&mut app, "back");
    assert_eq!(app.roll_grid, Beat::new(1, 4).unwrap());
    choose_piano_time(&mut app, "piano_grid", 8, true);
    assert_eq!(app.roll_grid, Beat::new(1, 3).unwrap());
    assert_eq!(app.editor.song(), &original);
    click(&mut app, "piano_defaults");
    choose_piano_time(&mut app, "piano_new_length", 8, false);
    focus_adjust(&mut app, "piano_new_velocity", Direction::Down);
    assert_eq!(app.roll_new_velocity, 100);
    click(&mut app, "piano_close");
    assert_eq!(app.roll_new_length, Beat::new(1, 4).unwrap());
    click(&mut app, "piano_defaults");
    choose_piano_time(&mut app, "piano_new_length", 8, false);
    focus_adjust(&mut app, "piano_new_velocity", Direction::Down);
    click(&mut app, "piano_defaults_apply");
    assert_eq!(app.roll_new_length, Beat::new(1, 2).unwrap());
    assert_eq!(app.roll_new_velocity, 90);
    assert_eq!(app.editor.song(), &original);
    app.apply(Request::PianoControl {
        control: PianoControl::Cursor {
            at: Beat::ONE,
            pitch: 67,
        },
    })
    .unwrap();
    app.apply(Request::Confirm).unwrap();
    let note = app
        .selected_phrase()
        .unwrap()
        .1
        .events
        .last()
        .unwrap()
        .note
        .as_ref()
        .unwrap();
    assert_eq!(
        (note.velocity, note.duration),
        (90, Beat::new(1, 2).unwrap())
    );
    click(&mut app, "notes");
    click(&mut app, "piano_inspect");
    choose_piano_time(&mut app, "piano_length", 16, true);
    click(&mut app, "piano_apply");
    let note = app
        .selected_phrase()
        .unwrap()
        .1
        .events
        .last()
        .unwrap()
        .note
        .as_ref()
        .unwrap();
    assert_eq!(note.duration, Beat::new(1, 6).unwrap());
    assert_eq!(app.roll_new_length, Beat::new(1, 2).unwrap());
    assert_eq!(app.roll_grid, Beat::new(1, 3).unwrap());
}

#[test]
fn gadget_inspector_previews_coarse_edits_and_cancel_restores_without_history() {
    let mut app = gadget_fixture();
    let original = app.editor.song().clone();
    let revision = app.editor.revision();
    action(&mut app, SemanticAction::OpenPopover);
    apply_pending(&mut app);
    click(&mut app, "piano_inspect");
    focus_adjust(&mut app, "piano_pitch", Direction::Up);
    focus_adjust(&mut app, "piano_velocity_value", Direction::Down);
    assert_eq!(app.editor.song(), &original);
    let pending = piano::preview(&app).unwrap();
    assert_eq!(pending[0].note.as_ref().unwrap().pitch, 72);
    assert_eq!(pending[0].note.as_ref().unwrap().velocity, 90);
    let projected = project(&app).workspace.unwrap();
    assert!(projected.overlay && projected.timeline.is_some());
    assert_eq!(projected.panel_columns, 6);
    assert!(projected.hints.contains("+10/−10"));
    action(&mut app, SemanticAction::NavigatePage(Direction::Down));
    apply_pending(&mut app);
    assert!(app.workspace.panel.is_none());
    assert_eq!(app.editor.revision(), revision);
    assert_eq!(app.editor.song(), &original);
    click(&mut app, "notes");
    click(&mut app, "piano_inspect");
    focus_adjust(&mut app, "piano_pitch", Direction::Right);
    click(&mut app, "piano_apply");
    assert_eq!(app.editor.revision(), revision + 1);
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &original);
}

#[test]
fn gadget_quantize_preview_cancel_apply_and_undo_preserve_expression() {
    use crest_tracker::composition::{Control, Expression};
    let mut app = gadget_fixture();
    let (phrase, source) = app.selected_phrase().unwrap();
    let mut event = source.events[0].clone();
    event.at = Beat::new(7, 20).unwrap();
    event.note.as_mut().unwrap().expression.push(Expression {
        offset: Beat::new(1, 16).unwrap(),
        control: Control::Pressure { value: 37 },
        sequence: Some(1),
    });
    app.command(Command::PutEvent { phrase, event }).unwrap();
    let original = app.editor.song().clone();
    let cursor = app.piano_cursor();
    click(&mut app, "piano_quantize");
    choose_piano_time(&mut app, "piano_quantize_grid", 8, true);
    assert_eq!(app.roll_grid, Beat::new(1, 4).unwrap());
    let preview = piano::preview(&app).unwrap();
    assert_eq!(preview[0].at, Beat::new(1, 3).unwrap());
    assert_eq!(preview[0].note, original.phrases[&phrase].events[0].note);
    let timeline = project(&app).workspace.unwrap().timeline.unwrap();
    assert_eq!(timeline.originals.len(), 1);
    assert_ne!(timeline.originals[0].start, timeline.spans[0].start);
    assert_eq!(app.editor.song(), &original);
    click(&mut app, "piano_close");
    assert_eq!(app.piano_cursor(), cursor);
    assert_eq!(app.editor.song(), &original);
    click(&mut app, "piano_quantize");
    click(&mut app, "piano_apply");
    assert!(app.workspace.panel.is_none());
    assert_eq!(
        app.selected_phrase().unwrap().1.events[0].at,
        Beat::new(1, 3).unwrap()
    );
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &original);
}

#[test]
fn gadget_quantize_requires_explicit_all_and_rejects_stale_drafts() {
    let mut app = gadget_fixture();
    click(&mut app, "notes");
    click(&mut app, "piano_clear_selection");
    let original = app.editor.song().clone();
    click(&mut app, "piano_quantize");
    assert!(
        !project(&app)
            .workspace
            .unwrap()
            .rows
            .iter()
            .find(|row| row.id == "piano_apply")
            .unwrap()
            .enabled
    );
    click(&mut app, "piano_apply");
    assert_eq!(app.editor.song(), &original);
    click(&mut app, "piano_scope");
    assert_eq!(piano::preview(&app).unwrap().len(), 1);
    app.command(Command::RenameSong {
        name: "Changed while previewing".into(),
    })
    .unwrap();
    assert!(piano::preview(&app).is_err());
    assert!(
        !project(&app)
            .workspace
            .unwrap()
            .rows
            .iter()
            .find(|row| row.id == "piano_apply")
            .unwrap()
            .enabled
    );
}

#[test]
fn gadget_tools_directions_reach_all_controls_and_modal_blocks_roll_clicks() {
    let mut app = gadget_fixture();
    click(&mut app, "notes");
    assert_eq!(focused_id(&app), "piano_grid");
    action(&mut app, SemanticAction::Navigate(Direction::Right));
    assert_eq!(focused_id(&app), "piano_defaults");
    action(&mut app, SemanticAction::Navigate(Direction::Down));
    assert_eq!(focused_id(&app), "piano_inspect");
    let mut reached = std::collections::BTreeSet::new();
    for _ in 0..30 {
        reached.insert(focused_id(&app));
        action(&mut app, SemanticAction::Navigate(Direction::Right));
    }
    assert!(reached.contains("piano_close"));
    let original = app.editor.song().clone();
    let cursor = app.piano_cursor();
    event(
        &mut app,
        WorkspaceEvent::Activate {
            id: "piano:5:61".into(),
        },
    );
    apply_pending(&mut app);
    assert_eq!(app.piano_cursor(), cursor);
    assert_eq!(app.editor.song(), &original);
}

#[test]
fn gadget_note_off_closes_sounding_pitch_at_cursor_without_erasing_attack() {
    let mut app = gadget_fixture();
    let (phrase, source) = app.selected_phrase().unwrap();
    let id = source.events[0].id;
    app.command(Command::ResizeNotes {
        phrase,
        events: vec![id],
        duration: Beat::integer(2),
    })
    .unwrap();
    app.apply(Request::PianoControl {
        control: PianoControl::Cursor {
            at: Beat::ONE,
            pitch: 60,
        },
    })
    .unwrap();
    let original = app.editor.song().clone();
    action(&mut app, SemanticAction::OpenPopover);
    apply_pending(&mut app);
    assert_eq!(app.piano_cursor().0, Beat::ONE);
    assert!(
        project(&app)
            .workspace
            .unwrap()
            .rows
            .iter()
            .find(|row| row.id == "piano_off")
            .unwrap()
            .enabled
    );
    click(&mut app, "piano_off");
    let event = &app.selected_phrase().unwrap().1.events[0];
    assert_eq!(event.id, id);
    assert_eq!(event.at, Beat::ZERO);
    assert_eq!(event.note.as_ref().unwrap().duration, Beat::ONE);
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &original);
}

#[test]
#[ignore = "exports production projections for browser visual QA"]
fn gadget_export_render_states() {
    use crest_tracker::composition::{Event, EventId, Note};
    let root = std::env::var("CREST_PIANO_RENDER_DIR").expect("set a scratch output directory");
    std::fs::create_dir_all(&root).unwrap();
    let mut app = gadget_fixture();
    let phrase = app.selected_phrase().unwrap().0;
    app.command(Command::ResizePhrase {
        phrase,
        steps: 32,
        trim: false,
    })
    .unwrap();
    let target = app.default_target().unwrap();
    let notes = [
        (0, 60, 88),
        (115, 64, 96),
        (185, 67, 104),
        (316, 69, 87),
        (400, 67, 95),
        (475, 64, 96),
        (550, 62, 88),
        (650, 60, 102),
    ];
    for (index, (at, pitch, velocity)) in notes.into_iter().enumerate() {
        let event = Event::note(
            EventId::new(500 + index as u64).unwrap(),
            Beat::new(at, 100).unwrap(),
            target.clone(),
            Note::new(pitch, velocity, Beat::new(1, 2).unwrap()).unwrap(),
        );
        app.command(Command::PutEvent { phrase, event }).unwrap();
    }
    let mut states = Vec::new();
    app.apply(Request::SelectEvents {
        revision: app.editor.revision(),
        occurrence: app.occurrence().unwrap(),
        events: vec![EventId::new(501).unwrap()],
        toggle: false,
    })
    .unwrap();
    states.push(("compose", project(&app)));
    click(&mut app, "notes");
    states.push(("tools", project(&app)));
    click(&mut app, "piano_inspect");
    states.push(("inspector", project(&app)));
    click(&mut app, "piano_length");
    states.push(("length", project(&app)));
    click(&mut app, "back");
    click(&mut app, "piano_close");
    click(&mut app, "piano_quantize");
    choose_piano_time(&mut app, "piano_quantize_grid", 8, true);
    states.push(("quantize", project(&app)));
    click(&mut app, "piano_quantize_grid");
    states.push(("quantize-grid", project(&app)));
    click(&mut app, "back");
    click(&mut app, "piano_close");
    click(&mut app, "piano_grid");
    states.push(("grid", project(&app)));
    click(&mut app, "back");
    click(&mut app, "piano_defaults");
    states.push(("defaults", project(&app)));
    click(&mut app, "piano_close");
    click(&mut app, "notes_view");
    states.push(("view", project(&app)));
    click(&mut app, "back");
    click(&mut app, "notes");
    click(&mut app, "piano_velocity");
    states.push(("velocity", project(&app)));
    states.push(("expression", project(&expression_fixture())));
    states.push(("channel-expression", project(&channel_expression_fixture())));
    states.push(("overlapping-notes", project(&stacked_notes_fixture())));
    std::fs::write(
        std::path::Path::new(&root).join("states.json"),
        serde_json::to_vec_pretty(&states).unwrap(),
    )
    .unwrap();
}

#[test]
fn gadget_velocity_lane_uses_semantic_fine_coarse_edits_and_returns_to_roll() {
    let mut app = gadget_fixture();
    click(&mut app, "notes");
    click(&mut app, "piano_velocity");
    let view = project(&app).workspace.unwrap();
    assert!(!view.overlay);
    assert!(view.timeline.unwrap().level_focused);
    let original = app.editor.song().clone();
    action(
        &mut app,
        SemanticAction::SetInteractionMode(InteractionMode::Adjust),
    );
    action(&mut app, SemanticAction::Adjust(Direction::Down));
    apply_pending(&mut app);
    action(&mut app, SemanticAction::Adjust(Direction::Right));
    apply_pending(&mut app);
    assert_eq!(
        app.selected_phrase().unwrap().1.events[0]
            .note
            .as_ref()
            .unwrap()
            .velocity,
        91
    );
    app.fine_held = true;
    action(&mut app, SemanticAction::Adjust(Direction::Up));
    apply_pending(&mut app);
    assert_eq!(
        app.selected_phrase().unwrap().1.events[0]
            .note
            .as_ref()
            .unwrap()
            .velocity,
        92
    );
    action(
        &mut app,
        SemanticAction::SetInteractionMode(InteractionMode::Navigate),
    );
    apply_pending(&mut app);
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &original);
    action(&mut app, SemanticAction::Navigate(Direction::Up));
    apply_pending(&mut app);
    assert!(app.workspace.panel.is_none());
}

#[test]
fn gadget_view_octaves_and_scale_guide_leave_notes_and_edit_cursor_unchanged() {
    let mut app = gadget_fixture();
    let original = app.editor.song().clone();
    let cursor = app.piano_cursor();
    let low = app.roll_view.low_pitch;
    click(&mut app, "notes_view");
    click(&mut app, "piano_octave_up");
    assert_eq!(app.roll_view.low_pitch, low + 12);
    assert!(project(&app)
        .workspace
        .unwrap()
        .timeline
        .unwrap()
        .cursor_lane
        .is_none());
    click(&mut app, "piano_scale_guide");
    assert!(!app.roll_view.scale_guide);
    assert_eq!(app.piano_cursor(), cursor);
    assert_eq!(app.editor.song(), &original);
    click(&mut app, "back");
    action(&mut app, SemanticAction::Navigate(Direction::Right));
    apply_pending(&mut app);
    assert!(project(&app)
        .workspace
        .unwrap()
        .timeline
        .unwrap()
        .cursor_lane
        .is_some());
}

#[test]
fn gadget_inspector_target_keeps_channel_and_expression_owned_until_apply() {
    let mut app = gadget_fixture();
    let original = app.editor.song().clone();
    click(&mut app, "notes");
    click(&mut app, "piano_inspect");
    click(&mut app, "piano_target");
    focus_adjust(&mut app, "piano_channel", Direction::Up);
    assert_eq!(piano::preview(&app).unwrap()[0].target.channel().get(), 11);
    assert_eq!(app.editor.song(), &original);
    click(&mut app, "back");
    click(&mut app, "piano_apply");
    assert_eq!(
        app.selected_phrase().unwrap().1.events[0]
            .target
            .channel()
            .get(),
        11
    );
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &original);
}

#[test]
fn gadget_quantize_apply_retains_shared_phrase_guard_and_atomic_undo() {
    use crest_tracker::composition::UseId;
    let mut app = gadget_fixture();
    let (phrase, source) = app.selected_phrase().unwrap();
    let mut note = source.events[0].clone();
    note.at = Beat::new(1, 10).unwrap();
    app.command(Command::PutEvent {
        phrase,
        event: note,
    })
    .unwrap();
    let mut track = app.editor.song().tracks[0].clone();
    let mut usage = track.slots[0].clone().unwrap();
    usage.id = UseId::new(app.editor.song().next_identity().unwrap()).unwrap();
    track.slots.push(Some(usage));
    app.command(Command::PutTrack { track }).unwrap();
    let original = app.editor.song().clone();
    click(&mut app, "piano_quantize");
    event(
        &mut app,
        WorkspaceEvent::Activate {
            id: "piano_apply".into(),
        },
    );
    let results: Vec<_> = std::mem::take(&mut app.requests)
        .into_iter()
        .map(|request| app.apply(request))
        .collect();
    assert!(results.iter().any(Result::is_err));
    assert_eq!(app.editor.song(), &original);
    let token = app.shared_edit_projection()["token"].as_u64().unwrap();
    app.apply(Request::SharedDecision {
        token,
        choice: Some(crate::app::SharedChoice::Shared),
        remember: false,
    })
    .unwrap();
    assert_eq!(app.selected_phrase().unwrap().1.events[0].at, Beat::ZERO);
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &original);
}

fn expression_fixture() -> App {
    use crest_tracker::composition::{Control, Expression};
    let mut app = gadget_fixture();
    let (phrase, source) = app.selected_phrase().unwrap();
    let mut event = source.events[0].clone();
    event.at = Beat::ONE;
    let note = event.note.as_mut().unwrap();
    note.duration = Beat::integer(2);
    // A valid document can retain source order; projection must not mutate it.
    note.expression = vec![
        Expression {
            offset: Beat::ZERO,
            control: Control::Pressure { value: 40 },
            sequence: None,
        },
        Expression {
            offset: Beat::new(1, 4).unwrap(),
            control: Control::Pressure { value: 100 },
            sequence: None,
        },
        Expression {
            offset: Beat::new(1, 8).unwrap(),
            control: Control::Bend { value: 12345 },
            sequence: None,
        },
        Expression {
            offset: Beat::new(1, 2).unwrap(),
            control: Control::Cc {
                number: 74,
                value: 67,
            },
            sequence: None,
        },
    ];
    let id = event.id;
    app.command(Command::PutEvent { phrase, event }).unwrap();
    app.apply(Request::SelectEvents {
        revision: app.editor.revision(),
        occurrence: app.occurrence().unwrap(),
        events: vec![id],
        toggle: false,
    })
    .unwrap();
    click(&mut app, "notes");
    click(&mut app, "expression");
    app
}

#[test]
fn expression_lane_projects_exact_onsets_and_browses_without_changing_music() {
    let mut app = expression_fixture();
    let original = app.editor.song().clone();
    let view = project(&app).workspace.unwrap();
    assert!(view.overlay);
    let curve = view.timeline.unwrap().curve.unwrap();
    assert_eq!(curve.label, "Pressure");
    assert_eq!(curve.maximum, 127);
    assert_eq!(
        curve
            .points
            .iter()
            .map(|point| point.position)
            .collect::<Vec<_>>(),
        vec![2500, 3125]
    );
    assert_eq!(
        curve.path.last().unwrap()[0],
        7500,
        "owned lifetime ends at beat 3"
    );
    click(&mut app, "expr_next");
    assert_eq!(app.workspace.expression_index, 1);
    click(&mut app, "expr_lane");
    let curve = project(&app)
        .workspace
        .unwrap()
        .timeline
        .unwrap()
        .curve
        .unwrap();
    assert_eq!(curve.label, "Pitch bend");
    assert_eq!(curve.maximum, 16383);
    assert!(curve.points[0].label.ends_with("12345"));
    assert_eq!(curve.points[0].position, 2813);
    click(&mut app, "expr_lane");
    assert_eq!(
        project(&app)
            .workspace
            .unwrap()
            .timeline
            .unwrap()
            .curve
            .unwrap()
            .label,
        "Timbre · CC74"
    );
    assert_eq!(app.editor.song(), &original);
    assert!(
        !project(&app)
            .workspace
            .unwrap()
            .rows
            .iter()
            .find(|row| row.id == "expr_scope")
            .unwrap()
            .enabled
    );
}

#[test]
fn expression_reordering_and_duplicate_values_keep_the_edited_point_selected() {
    use crest_tracker::composition::Control;
    let mut app = expression_fixture();
    let original = app.editor.song().clone();
    focus_adjust(&mut app, "expr_offset", Direction::Up);
    focus_adjust(&mut app, "expr_value", Direction::Up);
    let note = app.selected_phrase().unwrap().1.events[0]
        .note
        .as_ref()
        .unwrap();
    let point = &note.expression[app.workspace.expression_index];
    assert_eq!(point.offset, Beat::new(10, 16).unwrap());
    assert_eq!(point.control, Control::Pressure { value: 50 });
    click(&mut app, "undo");
    click(&mut app, "undo");
    assert_eq!(app.editor.song(), &original);
    click(&mut app, "expr_select:0");
    click(&mut app, "expr_new");
    focus_adjust(&mut app, "expr_value", Direction::Right);
    let note = app.selected_phrase().unwrap().1.events[0]
        .note
        .as_ref()
        .unwrap();
    assert_eq!(note.expression[0].control, Control::Pressure { value: 40 });
    assert_eq!(note.expression[1].control, Control::Pressure { value: 41 });
    assert_eq!(app.workspace.expression_index, 1);
    click(&mut app, "expr_delete");
    click(&mut app, "undo");
    assert_eq!(
        app.selected_phrase().unwrap().1.events[0]
            .note
            .as_ref()
            .unwrap()
            .expression[1]
            .control,
        Control::Pressure { value: 41 }
    );
}

#[test]
fn expression_viewport_retains_held_values_without_inventing_points() {
    let mut app = expression_fixture();
    let original = app.editor.song().clone();
    app.apply(Request::PianoControl {
        control: PianoControl::Zoom { delta: 1 },
    })
    .unwrap();
    app.apply(Request::PianoControl {
        control: PianoControl::Zoom { delta: 1 },
    })
    .unwrap();
    app.apply(Request::PianoControl {
        control: PianoControl::Scroll { delta: 1 },
    })
    .unwrap();
    app.apply(Request::PianoControl {
        control: PianoControl::Scroll { delta: 1 },
    })
    .unwrap();
    let curve = project(&app)
        .workspace
        .unwrap()
        .timeline
        .unwrap()
        .curve
        .unwrap();
    assert!(curve.points.is_empty());
    assert_eq!(
        curve.path,
        vec![[0, 100 * 10000 / 127], [5000, 100 * 10000 / 127]]
    );
    assert_eq!(app.editor.song(), &original);
}

#[test]
fn expression_master_scope_stays_separate_and_edits_restore_with_undo() {
    use crest_tracker::composition::{Channel, Control, Expression};
    use crest_tracker::mpe::{BendRange, MpeProfile, MpeProvenance, MpeZone};
    let mut app = expression_fixture();
    let (phrase, source) = app.selected_phrase().unwrap();
    let mut event = source.events[0].clone();
    let note = event.note.as_mut().unwrap();
    let profile = MpeProfile::new(
        "test-source".into(),
        MpeZone::lower(4).unwrap(),
        BendRange::new(48, 0).unwrap(),
        BendRange::new(2, 0).unwrap(),
    )
    .unwrap();
    note.mpe = Some(MpeProvenance::new(profile, Channel::new(2).unwrap()).unwrap());
    note.expression_duration = Some(note.duration);
    note.master_expression = vec![Expression {
        offset: Beat::ZERO,
        control: Control::Cc {
            number: 64,
            value: 127,
        },
        sequence: None,
    }];
    app.command(Command::PutEvent { phrase, event }).unwrap();
    let original = app.editor.song().clone();
    click(&mut app, "expr_scope");
    let curve = project(&app)
        .workspace
        .unwrap()
        .timeline
        .unwrap()
        .curve
        .unwrap();
    assert!(curve.caption.starts_with("MASTER"));
    assert_eq!(curve.label, "Sustain · CC64");
    focus_adjust(&mut app, "expr_value", Direction::Down);
    let note = app.selected_phrase().unwrap().1.events[0]
        .note
        .as_ref()
        .unwrap();
    assert_eq!(
        note.expression,
        original.phrases[&phrase].events[0]
            .note
            .as_ref()
            .unwrap()
            .expression
    );
    assert_eq!(
        note.master_expression[0].control,
        Control::Cc {
            number: 64,
            value: 117
        }
    );
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &original);
}

#[test]
fn dense_piano_grids_keep_a_musical_ruler_when_zoomed_out() {
    let mut app = gadget_fixture();
    let phrase = app.selected_phrase().unwrap().0;
    app.command(Command::ResizePhrase {
        phrase,
        steps: 256,
        trim: false,
    })
    .unwrap();
    for _ in 0..3 {
        app.apply(Request::PianoControl {
            control: PianoControl::Zoom { delta: -1 },
        })
        .unwrap();
    }
    let original = app.editor.song().clone();
    for grid in [Beat::new(1, 16).unwrap(), Beat::new(1, 24).unwrap()] {
        app.apply(Request::PianoControl {
            control: PianoControl::Grid { grid },
        })
        .unwrap();
        let view = project(&app).workspace.unwrap();
        assert!(view.columns.len() <= 64);
        assert!(
            view.columns.iter().any(|label| label == "2.1"),
            "bar 2 disappeared at grid {grid:?}: {:?}",
            view.columns
        );
        assert!(view.timeline.unwrap().beat_columns.len() > 1);
        assert_eq!(
            app.roll_grid, grid,
            "paint density must not change entry spacing"
        );
        assert_eq!(app.editor.song(), &original);
    }
}

#[test]
fn queued_controls_preserve_held_edit_fine_release_and_one_gesture_undo() {
    use crest_synth::shell::hosted_application::HostControllerAxis;
    use input::Input;
    let mut app = gadget_fixture();
    let original = app.editor.song().clone();
    input::enqueue(
        &mut app,
        Input::Modifiers {
            fine: true,
            range: false,
        },
    );
    input::action(
        &mut app,
        SemanticAction::SetInteractionMode(InteractionMode::Adjust),
    );
    for _ in 0..2 {
        input::enqueue(
            &mut app,
            Input::Stick {
                axis: HostControllerAxis::LeftX,
                sign: 1,
                shift: false,
                gesture: 42,
            },
        );
    }
    input::action(
        &mut app,
        SemanticAction::SetInteractionMode(InteractionMode::Navigate),
    );
    input::enqueue(
        &mut app,
        Input::Modifiers {
            fine: false,
            range: false,
        },
    );
    apply_pending(&mut app);
    assert_eq!(
        app.selected_phrase().unwrap().1.events[0].at,
        Beat::new(1, 32).unwrap()
    );
    assert!(!app.edit_held && !app.fine_held);
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &original);
}

#[test]
fn queued_pointer_actions_resolve_panels_and_text_before_apply() {
    use input::Input;
    let mut app = gadget_fixture();
    for id in ["notes", "piano_inspect", "piano_start"] {
        input::enqueue(
            &mut app,
            Input::Event(WorkspaceEvent::Activate { id: id.into() }),
        );
    }
    input::enqueue(
        &mut app,
        Input::Event(WorkspaceEvent::Submit {
            id: "piano_start".into(),
            value: "7/20".into(),
        }),
    );
    input::enqueue(
        &mut app,
        Input::Event(WorkspaceEvent::Activate {
            id: "piano_apply".into(),
        }),
    );
    apply_pending(&mut app);
    assert_eq!(
        app.selected_phrase().unwrap().1.events[0].at,
        Beat::new(7, 20).unwrap()
    );
    assert!(app.workspace.panel.is_none());
}

fn channel_expression_fixture() -> App {
    use crest_tracker::composition::{Channel, Control, Event, EventId, Target};
    let mut app = expression_fixture();
    let (phrase, source) = app.selected_phrase().unwrap();
    let mut event = source.events[0].clone();
    event.controls = vec![
        Control::Cc {
            number: 64,
            value: 127,
        },
        Control::Bend { value: 9001 },
    ];
    let target = event.target.clone();
    app.command(Command::PutEvent { phrase, event }).unwrap();
    for (target, value) in [
        (target.clone(), 0),
        (
            match target {
                Target::Instrument { id, .. } => Target::Instrument {
                    id,
                    channel: Channel::new(2).unwrap(),
                },
                Target::Channel { endpoint, .. } => Target::Channel {
                    endpoint,
                    channel: Channel::new(2).unwrap(),
                },
            },
            64,
        ),
    ] {
        app.command(Command::PutEvent {
            phrase,
            event: Event {
                id: EventId::new(app.editor.song().next_identity().unwrap()).unwrap(),
                at: Beat::integer(2),
                target,
                note: None,
                controls: vec![Control::Cc { number: 64, value }],
                fx: [None, None, None],
            },
        })
        .unwrap();
    }
    click(&mut app, "channel_expression");
    app
}

#[test]
fn channel_automation_browses_destinations_and_lanes_without_rerouting_notes() {
    let mut app = channel_expression_fixture();
    let original = app.editor.song().clone();
    let view = project(&app).workspace.unwrap();
    assert_eq!(view.context, "CHANNEL AUTOMATION");
    let curve = view.timeline.unwrap().curve.unwrap();
    assert!(curve.caption.starts_with("CHANNEL"));
    assert_eq!(curve.label, "Sustain · CC64");
    assert_eq!(
        curve
            .points
            .iter()
            .map(|point| point.position)
            .collect::<Vec<_>>(),
        vec![2500, 5000]
    );
    assert_eq!(curve.path.last().unwrap()[0], 10000);
    click(&mut app, "channel_next");
    assert!(
        project(&app)
            .workspace
            .unwrap()
            .timeline
            .unwrap()
            .curve
            .unwrap()
            .points[1]
            .selected
    );
    click(&mut app, "channel_lane");
    assert_eq!(
        project(&app)
            .workspace
            .unwrap()
            .timeline
            .unwrap()
            .curve
            .unwrap()
            .label,
        "Pitch bend"
    );
    click(&mut app, "channel_target");
    let curve = project(&app)
        .workspace
        .unwrap()
        .timeline
        .unwrap()
        .curve
        .unwrap();
    assert_eq!(curve.points.len(), 1);
    assert!(curve.points[0].label.ends_with("64"));
    assert_eq!(app.editor.song(), &original);
}

#[test]
fn channel_automation_moves_one_mixed_event_control_and_undo_restores_everything() {
    use crest_tracker::composition::Control;
    let mut app = channel_expression_fixture();
    let original = app.editor.song().clone();
    let phrase = app.selected_phrase().unwrap().0;
    click(&mut app, "channel_time");
    event(
        &mut app,
        WorkspaceEvent::Submit {
            id: "channel_time".into(),
            value: "3/2".into(),
        },
    );
    apply_pending(&mut app);
    let source = &app.editor.song().phrases[&phrase];
    let note = source
        .events
        .iter()
        .find(|event| event.note.is_some())
        .unwrap();
    assert_eq!(note.at, Beat::ONE);
    assert_eq!(note.note, original.phrases[&phrase].events[0].note);
    assert_eq!(note.controls, vec![Control::Bend { value: 9001 }]);
    let moved = source
        .events
        .iter()
        .find(|event| event.at == Beat::new(3, 2).unwrap())
        .unwrap();
    assert_eq!(
        moved.controls,
        vec![Control::Cc {
            number: 64,
            value: 127
        }]
    );
    assert!(moved.note.is_none());
    focus_adjust(&mut app, "channel_value", Direction::Down);
    let moved = app.editor.song().phrases[&phrase]
        .events
        .iter()
        .find(|event| event.at == Beat::new(3, 2).unwrap())
        .unwrap();
    assert_eq!(
        moved.controls[0],
        Control::Cc {
            number: 64,
            value: 117
        }
    );
    click(&mut app, "undo");
    click(&mut app, "undo");
    assert_eq!(app.editor.song(), &original);
    click(&mut app, "channel_delete");
    let note = app.editor.song().phrases[&phrase]
        .events
        .iter()
        .find(|event| event.note.is_some())
        .unwrap();
    assert_eq!(note.note, original.phrases[&phrase].events[0].note);
    assert_eq!(note.controls, vec![Control::Bend { value: 9001 }]);
    click(&mut app, "undo");
    assert_eq!(app.editor.song(), &original);
}

#[test]
fn channel_automation_exact_value_bounds_and_phrase_end_reject_without_mutation() {
    let mut app = channel_expression_fixture();
    let original = app.editor.song().clone();
    click(&mut app, "channel_time");
    event(
        &mut app,
        WorkspaceEvent::Submit {
            id: "channel_time".into(),
            value: "4".into(),
        },
    );
    apply_pending(&mut app);
    assert_eq!(app.workspace.panel, Some(Panel::Text));
    assert!(app.status.contains("inside the phrase"));
    assert_eq!(app.editor.song(), &original);
    action(&mut app, SemanticAction::Return);
    click(&mut app, "channel_value");
    event(
        &mut app,
        WorkspaceEvent::Submit {
            id: "channel_value".into(),
            value: "128".into(),
        },
    );
    apply_pending(&mut app);
    assert!(app.status.contains("0–127"));
    assert_eq!(app.editor.song(), &original);
}

#[test]
fn channel_automation_adds_without_a_note_and_keeps_shared_edit_guard() {
    use crest_tracker::composition::UseId;
    let mut app = gadget_fixture();
    let (phrase, source) = app.selected_phrase().unwrap();
    app.command(Command::DeleteEvents {
        phrase,
        events: source.events.iter().map(|event| event.id).collect(),
    })
    .unwrap();
    click(&mut app, "notes");
    click(&mut app, "channel_expression");
    let original = app.editor.song().clone();
    click(&mut app, "channel_new");
    let point = &app.editor.song().phrases[&phrase].events[0];
    assert!(point.note.is_none());
    assert_eq!(point.controls.len(), 1);
    click(&mut app, "undo");
    assert_eq!(app.editor.song(), &original);
    let mut track = app.editor.song().tracks[0].clone();
    let mut usage = track.slots[0].clone().unwrap();
    usage.id = UseId::new(app.editor.song().next_identity().unwrap()).unwrap();
    track.slots.push(Some(usage));
    app.command(Command::PutTrack { track }).unwrap();
    let original = app.editor.song().clone();
    event(
        &mut app,
        WorkspaceEvent::Activate {
            id: "channel_new".into(),
        },
    );
    let request = app.requests.pop().unwrap();
    assert!(app.apply(request).is_err());
    assert_eq!(app.editor.song(), &original);
    let token = app.shared_edit_projection()["token"].as_u64().unwrap();
    app.apply(Request::SharedDecision {
        token,
        choice: None,
        remember: false,
    })
    .unwrap();
    assert_eq!(app.editor.song(), &original);
}

fn stacked_notes_fixture() -> App {
    use crest_tracker::composition::{Channel, Event, EventId, Note, Target};
    let mut app = gadget_fixture();
    let (phrase, source) = app.selected_phrase().unwrap();
    let first = source.events[0].id;
    app.command(Command::ResizeNotes {
        phrase,
        events: vec![first],
        duration: Beat::integer(2),
    })
    .unwrap();
    let target = app.default_target().unwrap();
    for (at, pitch, channel, velocity) in [
        (Beat::new(1, 2).unwrap(), 60, 2, 74),
        (Beat::ZERO, 67, 1, 88),
        (Beat::integer(3), 60, 1, 100),
    ] {
        let target = match &target {
            Target::Instrument { id, .. } => Target::Instrument {
                id: *id,
                channel: Channel::new(channel).unwrap(),
            },
            Target::Channel { endpoint, .. } => Target::Channel {
                endpoint: endpoint.clone(),
                channel: Channel::new(channel).unwrap(),
            },
        };
        let event = Event::note(
            EventId::new(app.editor.song().next_identity().unwrap()).unwrap(),
            at,
            target,
            Note::new(pitch, velocity, Beat::ONE).unwrap(),
        );
        app.command(Command::PutEvent { phrase, event }).unwrap();
    }
    app.apply(Request::PianoControl {
        control: PianoControl::Cursor {
            at: Beat::ONE,
            pitch: 60,
        },
    })
    .unwrap();
    click(&mut app, "notes");
    click(&mut app, "piano_stack");
    app
}

#[test]
fn overlapping_note_picker_distinguishes_targets_selects_one_and_does_not_edit_music() {
    let mut app = stacked_notes_fixture();
    let original = app.editor.song().clone();
    let revision = app.editor.revision();
    let view = project(&app).workspace.unwrap();
    let candidates: Vec<_> = view
        .rows
        .iter()
        .filter(|row| row.id.starts_with("piano_pick:"))
        .collect();
    assert_eq!(
        candidates.len(),
        2,
        "exclude other pitches and notes outside cursor time"
    );
    assert!(candidates.iter().all(|row| row.span == 6));
    assert!(candidates[0].label.contains("CH 01"));
    assert!(candidates[1].label.contains("CH 02"));
    assert!(candidates[1].value.contains("Velocity 74"));
    let chosen = candidates[1].id.clone();
    action(&mut app, SemanticAction::Navigate(Direction::Down));
    assert_eq!(focused_id(&app), chosen);
    assert!(project(&app)
        .workspace
        .unwrap()
        .hints
        .contains("select this note"));
    action(&mut app, SemanticAction::Activate);
    apply_pending(&mut app);
    assert!(app.workspace.panel.is_none());
    assert_eq!(app.selected_event_ids().len(), 1);
    assert_eq!(app.piano_cursor(), (Beat::new(1, 2).unwrap(), 60));
    assert_eq!(app.editor.revision(), revision);
    assert_eq!(app.editor.song(), &original);
}

#[test]
fn overlapping_note_picker_cancel_and_stale_choice_preserve_selection() {
    let mut app = stacked_notes_fixture();
    let original = app.editor.song().clone();
    let selected = app.selected_event_ids();
    let cursor = app.piano_cursor();
    click(&mut app, "back");
    assert_eq!(app.workspace.panel, Some(Panel::Notes));
    assert_eq!(app.selected_event_ids(), selected);
    assert_eq!(app.piano_cursor(), cursor);
    click(&mut app, "piano_stack");
    event(
        &mut app,
        WorkspaceEvent::Activate {
            id: "piano_pick:9999999".into(),
        },
    );
    apply_pending(&mut app);
    assert_eq!(app.workspace.panel, Some(Panel::PianoStack));
    assert_eq!(app.editor.song(), &original);
    assert_eq!(app.selected_event_ids(), selected);
}

#[test]
fn piano_roll_inspector_cursor_and_keys_share_phrase_spelling_override() {
    use crest_tracker::harmony::Spelling;
    let mut app = gadget_fixture();
    let (phrase, source) = app.selected_phrase().unwrap();
    let mut changed = source.clone();
    changed.key = Some(app.editor.song().key.clone().with_spelling(Spelling::Flats));
    changed.events[0].note.as_mut().unwrap().pitch = 61;
    app.command(Command::PutPhrase {
        id: phrase,
        phrase: changed,
    })
    .unwrap();
    app.apply(Request::PianoControl {
        control: PianoControl::Cursor {
            at: Beat::ZERO,
            pitch: 61,
        },
    })
    .unwrap();
    let view = project(&app).workspace.unwrap();
    assert!(view.summary.contains("Db4"));
    assert!(view.grid.iter().any(|row| row.label == "Db4"));
    app.apply(Request::Confirm).unwrap();
    click(&mut app, "notes");
    click(&mut app, "piano_inspect");
    assert_eq!(
        project(&app)
            .workspace
            .unwrap()
            .rows
            .iter()
            .find(|row| row.id == "piano_pitch")
            .unwrap()
            .value,
        "Db4"
    );
}

#[test]
fn song_popover_assigns_builtins_edits_measures_and_undo_restores_slot() {
    use crest_tracker::composition::{ChainRef, SpecialChain};
    let mut app = fixture();
    action(&mut app, SemanticAction::OpenPopover);
    let view = project(&app).workspace.unwrap();
    assert!(view.overlay);
    assert_eq!(
        view.rows
            .iter()
            .map(|row| (row.id.as_str(), row.label.as_str()))
            .collect::<Vec<_>>(),
        [
            ("slot_kill", "Kill"),
            ("slot_rest", "Rest"),
            ("slot_chain", "Chain")
        ]
    );
    click(&mut app, "slot_rest");
    assert_eq!(app.view, View::Song);
    assert_eq!(app.selected_special(), Some(SpecialChain::Rest));
    action(&mut app, SemanticAction::Adjust(Direction::Up));
    apply_pending(&mut app);
    assert!(
        app.workspace.panel.is_none(),
        "coarse edit must not open popover"
    );
    assert_eq!(
        app.editor.song().tracks[0].slots[0]
            .as_ref()
            .unwrap()
            .repeats,
        11
    );
    action(&mut app, SemanticAction::Adjust(Direction::Left));
    apply_pending(&mut app);
    assert_eq!(
        app.editor.song().tracks[0].slots[0]
            .as_ref()
            .unwrap()
            .repeats,
        10
    );
    assert_eq!(app.grid().3.values[0][0], "Rest 10");
    let saved = serde_json::to_vec(app.editor.song()).unwrap();
    let restored: crest_tracker::composition::Song = serde_json::from_slice(&saved).unwrap();
    restored.validate().unwrap();
    assert_eq!(restored, *app.editor.song());
    action(&mut app, SemanticAction::Activate);
    apply_pending(&mut app);
    assert_eq!(app.view, View::Song, "builtins have no phrase editor");
    action(&mut app, SemanticAction::OpenPopover);
    click(&mut app, "slot_kill");
    assert_eq!(
        app.editor.song().tracks[0].slots[0].as_ref().unwrap().chain,
        ChainRef::Special(SpecialChain::Kill)
    );
    assert_eq!(app.grid().3.values[0][0], "Kill 10");
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.selected_special(), Some(SpecialChain::Rest));
    let before_chain = app.editor.song().clone();
    action(&mut app, SemanticAction::OpenPopover);
    click(&mut app, "slot_chain");
    assert!(app.selected_chain().is_some());
    assert_eq!(app.view, View::Song);
    assert!(app.workspace.panel.is_none());
    assert_eq!(app.grid().3.values[0][0], "00");
    assert_eq!(app.editor.song().chains.len(), 1);
    assert_eq!(
        app.editor.song().tracks[0].slots[0]
            .as_ref()
            .unwrap()
            .repeats,
        1
    );
    app.apply(Request::Undo).unwrap();
    assert_eq!(
        app.editor.song(),
        &before_chain,
        "creating Chain is one undo"
    );
}

#[test]
fn song_popover_stays_three_types_with_full_catalog_and_configures_chain_in_song() {
    let mut app = fixture();
    action(&mut app, SemanticAction::Adjust(Direction::Right));
    apply_pending(&mut app);
    let first = app.selected_chain().unwrap().0;
    action(&mut app, SemanticAction::Adjust(Direction::Up));
    apply_pending(&mut app);
    assert_eq!(app.editor.song().chains.len(), 11);
    assert_eq!(app.grid().3.values[0][0], "10");
    let original = app.editor.song().clone();
    let revision = app.editor.revision();
    action(&mut app, SemanticAction::OpenPopover);
    let view = project(&app).workspace.unwrap();
    assert_eq!(
        view.rows
            .iter()
            .map(|row| row.label.as_str())
            .collect::<Vec<_>>(),
        ["Kill", "Rest", "Chain"]
    );
    action(&mut app, SemanticAction::Adjust(Direction::Up));
    apply_pending(&mut app);
    click(&mut app, "slot_chain");
    assert!(app.workspace.panel.is_none());
    assert_eq!(
        app.editor.song(),
        &original,
        "reselecting Chain keeps its number"
    );
    assert_eq!(app.editor.revision(), revision);

    action(&mut app, SemanticAction::OpenPopover);
    click(&mut app, "slot_rest");
    action(&mut app, SemanticAction::Adjust(Direction::Up));
    apply_pending(&mut app);
    let rest = app.editor.song().clone();
    action(&mut app, SemanticAction::OpenPopover);
    click(&mut app, "slot_chain");
    assert_eq!(app.selected_chain().unwrap().0, first);
    assert_eq!(app.editor.song().chains.len(), 11);
    assert_eq!(
        app.editor.song().tracks[0].slots[0]
            .as_ref()
            .unwrap()
            .repeats,
        1
    );
    action(&mut app, SemanticAction::Adjust(Direction::Right));
    apply_pending(&mut app);
    assert_eq!(app.grid().3.values[0][0], "01");
    app.apply(Request::Undo).unwrap();
    app.apply(Request::Undo).unwrap();
    assert_eq!(app.editor.song(), &rest);

    action(&mut app, SemanticAction::Navigate(Direction::Down));
    apply_pending(&mut app);
    action(&mut app, SemanticAction::OpenPopover);
    click(&mut app, "slot_chain");
    assert_eq!(app.selected_chain().unwrap().0, first);
    assert_eq!(app.editor.song().chains.len(), 11);
    let before_close = app.editor.song().clone();
    action(&mut app, SemanticAction::OpenPopover);
    action(&mut app, SemanticAction::OpenPopover);
    apply_pending(&mut app);
    assert!(app.workspace.panel.is_none());
    assert_eq!(app.editor.song(), &before_close);
}

#[test]
fn keyboard_popover_is_one_shot_and_fx_up_remains_coarse() {
    let mut app = note_fixture();
    app.column = 3;
    let mut keyboard = KeyboardInputTranslator::new();
    let key = WindowInput::key_down(WindowKey::BracketLeft);
    action(&mut app, keyboard.translate(key).unwrap());
    assert_eq!(app.workspace.panel, Some(Panel::FxCommands));
    assert!(keyboard.translate(key).is_none());
    click(&mut app, "fx_command:1");
    focus_adjust(&mut app, "fx_value", Direction::Up);
    click(&mut app, "fx_apply");
    assert!(app.workspace.panel.is_none());
    let before = effects::selected_event(&app).unwrap();
    action(&mut app, SemanticAction::Adjust(Direction::Up));
    apply_pending(&mut app);
    assert!(app.workspace.panel.is_none());
    let after = effects::selected_event(&app).unwrap();
    assert!(
        matches!((before.fx[0].clone(), after.fx[0].clone()), (Some(crest_tracker::composition::Fx::Offset { semitones: a }), Some(crest_tracker::composition::Fx::Offset { semitones: b })) if b-a == 10)
    );
}
