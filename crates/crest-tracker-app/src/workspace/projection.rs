use super::*;
use crate::app::DrumNote;
use crest_tracker::composition::Phrase;

pub fn project(app: &App) -> HostView {
    let mut host = HostView {
        name: "CREST TRACKER".into(),
        entry: row("menu", "TRACKER / MENU", ""),
        workspace: None,
    };
    let dialog = !app.file_dialog.is_null();
    let shared = app.shared_edit_projection();
    if app.view == View::Instruments && app.workspace.panel.is_none() && !dialog && shared.is_null()
    {
        return host;
    }
    let mut view = WorkspaceProjection {
        context: format!("{:?}", app.view).to_uppercase(), title: app.editor.song().name.clone(),
        subtitle: format!("{} BPM · {} · {}", app.editor.song().tempo.bpm(), app.editor.song().key.name(), if app.playing { "PLAYING" } else { "STOPPED" }),
        status: format!("{}{}", if app.dirty() { "UNSAVED · " } else { "" }, app.status),
        focus: String::new(), adjusting: app.edit_held, columns: vec![], grid: vec![], rows: vec![],
        inspector_title: "SONG".into(),
        inspector: commands(&[("play","Play / Stop"),("enter","Edit / Open"),("parent","Back"),("tools","Edit tools"),("files","Song File"),("menu","Main menu")]),
        hints: "D-pad / arrows: move · Edit / Return: confirm · Edit+direction / K+direction: adjust · Start / Space: play · Shift+Back / Shift+Q: menu".into(),
        text_entry: None,
    };
    if app.workspace.panel == Some(Panel::Text) {
        if let Some(entry) = &app.workspace.text {
            view.context = "TEXT ENTRY".into();
            view.title = entry.label.clone();
            view.text_entry = Some(entry.clone());
            view.rows = commands(&[("back", "Cancel")]);
        }
    } else if dialog {
        project_dialog(app, &mut view);
    } else if !shared.is_null() {
        project_shared(&shared, &mut view);
    } else if let Some(panel) = app.workspace.panel {
        project_panel(app, panel, &mut view);
    } else {
        project_grid(app, &mut view);
    }
    if matches!(app.view, View::Chain | View::Phrase) && app.workspace.panel.is_none() {
        view.hints = "D-pad / arrows: move · Edit / Return: confirm · Edit+direction / K+direction: adjust · Shift+Right: open · Shift+Left: parent · Shift+Up: settings · Shift+Down: Quick Jump · Start / Space: play".into();
    }
    if !view.rows.is_empty() {
        view.focus = app
            .workspace
            .focus
            .as_ref()
            .filter(|id| view.rows.iter().any(|row| &row.id == *id))
            .cloned()
            .unwrap_or_else(|| view.rows[0].id.clone());
    }
    host.workspace = Some(view);
    host
}

fn project_panel(app: &App, panel: Panel, view: &mut WorkspaceProjection) {
    view.context = format!("{panel:?}").to_uppercase();
    view.rows = match panel {
        Panel::Menu => commands(&[
            ("home", "Tracker"),
            ("instruments", "Instruments"),
            ("chords", "Chord Bank"),
            ("mixer", "Mixer"),
            ("sends", "Sends"),
            ("settings", "Settings"),
        ]),
        Panel::QuickJump => commands(&[
            ("home", "Song"),
            ("instruments", "Instruments"),
            ("files", "Song File"),
            ("mixer", "Mixer"),
            ("sends", "Sends"),
            ("settings", "Settings"),
        ]),
        Panel::File => commands(&[
            ("new", "New Song"),
            ("open", "Open Song"),
            ("save", "Save Song"),
            ("save_as", "Save As"),
            ("collect", "Collect assets"),
            ("import", "Import Synth session"),
            ("export", "Export Synth session"),
            ("example", "Open example song"),
            ("corridors", "Import Corridors of Time"),
            ("back", "Back"),
        ]),
        Panel::SongSettings => vec![
            row("name", "Name", &app.editor.song().name),
            row(
                "tempo",
                "Tempo",
                format!("{} BPM", app.editor.song().tempo.bpm()),
            ),
            row(
                "synth_settings",
                "MIDI devices / controller / Synth settings",
                "",
            ),
            row("back", "Back", ""),
        ],
        Panel::PhraseSettings => phrase_settings(app),
        Panel::Tools => commands(&[
            ("undo", "Undo"),
            ("redo", "Redo"),
            ("clear", "Clear selected value"),
            ("phrase_settings", "Phrase settings"),
            ("arm", "Arm selected phrase"),
            ("record", "Record"),
            ("end_take", "End take"),
            ("commit_take", "Commit take"),
            ("discard_take", "Discard take"),
            ("recover", "Recover interrupted work"),
            ("back", "Back"),
        ]),
        Panel::Chords => chord_rows(app),
        Panel::Text => vec![],
    };
}

fn phrase_settings(app: &App) -> Vec<WorkspaceRow> {
    let Some((_, phrase)) = app.selected_phrase() else {
        return commands(&[("back", "Choose a phrase first")]);
    };
    vec![
        row("phrase_name", "Name", &phrase.name),
        row("phrase_steps", "Steps", phrase.steps.to_string()),
        row("phrase_form", "Form", format!("{:?}", phrase.form)),
        row("back", "Back", ""),
    ]
}

fn project_grid(app: &App, view: &mut WorkspaceProjection) {
    let (title, subtitle, columns, rows) = app.grid();
    view.title = title;
    view.subtitle = format!("{subtitle} · {} ROWS", rows.total);
    view.columns = columns;
    view.grid = rows
        .values
        .into_iter()
        .enumerate()
        .map(|(offset, cells)| {
            let index = rows.start + offset;
            grid_row(index, cells, playing_row(app, index))
        })
        .collect();
    view.focus = format!("cell:{}:{}", app.row, app.column);
    if app.is_piano_roll() {
        project_piano(app, view);
    }
    if app.is_drum() {
        project_drum(app, view);
    }
}

fn playing_row(app: &App, index: usize) -> bool {
    if !app.playing {
        return false;
    }
    let song = app.editor.song();
    match app.view {
        View::Song => song.tracks.iter().any(|track| {
            track
                .slots
                .get(index)
                .and_then(Option::as_ref)
                .is_some_and(|usage| {
                    app.play_heads.iter().any(|head| {
                        head.occurrence.track == track.id && head.occurrence.chain_use == usage.id
                    })
                })
        }),
        View::Chain => app.selected_chain().is_some_and(|(_, chain)| {
            let selected_use = song.tracks[app.track]
                .slots
                .get(app.slot)
                .and_then(Option::as_ref)
                .map(|usage| usage.id);
            chain.phrases.get(index).is_some_and(|usage| {
                app.play_heads.iter().any(|head| {
                    head.occurrence.track == song.tracks[app.track].id
                        && Some(head.occurrence.chain_use) == selected_use
                        && head.occurrence.phrase_use == usage.id
                })
            })
        }),
        View::Phrase => app.play_row == Some(index as u32),
        _ => false,
    }
}

fn grid_row(index: usize, cells: Vec<String>, playing: bool) -> WorkspaceGridRow {
    WorkspaceGridRow {
        id: format!("row:{index}"),
        label: format!("{index:02X}"),
        playing,
        cells: cells
            .into_iter()
            .enumerate()
            .map(|(column, value)| row(format!("cell:{index}:{column}"), "", value))
            .collect(),
    }
}

fn project_piano(app: &App, view: &mut WorkspaceProjection) {
    let (_, phrase) = app.selected_phrase().expect("Piano Roll");
    let (at, pitch) = app.piano_cursor();
    let step = phrase.division.beat_length();
    let selected = (u128::from(at.numerator()) * u128::from(step.denominator())
        / (u128::from(at.denominator()) * u128::from(step.numerator())))
        as usize;
    let start = selected / 32 * 32;
    let end = (start + 32).min(phrase.steps as usize);
    let low = pitch.saturating_sub(12).min(104);
    view.columns = (start..end).map(|index| format!("{index:02X}")).collect();
    view.grid = (low..=low + 23)
        .rev()
        .map(|pitch| piano_row(app, pitch, start, end, step))
        .collect();
    view.focus = format!("piano:{selected}:{pitch}");
}

fn piano_row(app: &App, pitch: u8, start: usize, end: usize, step: Beat) -> WorkspaceGridRow {
    let (_, phrase) = app.selected_phrase().expect("Piano Roll");
    let cells = (start..end)
        .map(|index| piano_cell(phrase, pitch, index, step))
        .collect();
    WorkspaceGridRow {
        id: format!("pitch:{pitch}"),
        label: app
            .editor
            .song()
            .key
            .pitch_name(crest_tracker::harmony::Pitch::new(pitch).expect("MIDI pitch")),
        cells,
        playing: false,
    }
}

fn piano_cell(phrase: &Phrase, pitch: u8, index: usize, step: Beat) -> WorkspaceRow {
    let at = step
        .checked_scale(index as u64, 1)
        .expect("validated phrase");
    let until = at.checked_add(step).expect("validated phrase");
    let value = phrase
        .events
        .iter()
        .find_map(|event| {
            event
                .note
                .as_ref()
                .filter(|note| note.pitch == pitch && event.at >= at && event.at < until)
                .map(|note| format!("● {}", note.velocity))
        })
        .unwrap_or_else(|| "·".into());
    row(format!("piano:{index}:{pitch}"), "", value)
}

fn project_shared(shared: &Value, view: &mut WorkspaceProjection) {
    view.context = "SHARED DEFINITION".into();
    view.subtitle = "This edit affects more than one occurrence".into();
    let token = shared["token"].as_u64().unwrap_or(0);
    view.rows = vec![
        row(format!("shared:{token}:shared"), "Edit Shared", ""),
        row(format!("shared:{token}:cancel"), "Cancel", ""),
    ];
    if shared["canMakeUnique"] == true {
        view.rows
            .insert(1, row(format!("shared:{token}:unique"), "Make Unique", ""));
    }
}

fn project_drum(app: &App, view: &mut WorkspaceProjection) {
    let (_, phrase) = app.selected_phrase().expect("Drum phrase");
    let lanes = app.drum_lanes();
    view.columns = lanes
        .iter()
        .map(|lane| format!("{} · {}", lane.pitch, app.target_label(&lane.target)))
        .collect();
    let start = app.row / 64 * 64;
    view.grid = (start..(start + 64).min(phrase.steps as usize))
        .map(|step| drum_row(phrase, &lanes, step, app.play_phrase_beat))
        .collect();
}

fn drum_row(
    phrase: &Phrase,
    lanes: &[DrumNote],
    step: usize,
    playing: Option<Beat>,
) -> WorkspaceGridRow {
    let at = phrase
        .division
        .beat_length()
        .checked_scale(step as u64, 1)
        .expect("validated phrase");
    let cells = lanes
        .iter()
        .enumerate()
        .map(|(column, lane)| {
            row(
                format!("cell:{step}:{column}"),
                "",
                drum_hit(phrase, lane, at),
            )
        })
        .collect();
    WorkspaceGridRow {
        id: format!("step:{step}"),
        label: format!("{step:02X}"),
        cells,
        playing: playing == Some(at),
    }
}

fn drum_hit(phrase: &Phrase, lane: &DrumNote, at: Beat) -> String {
    phrase
        .events
        .iter()
        .find(|event| {
            event.at == at
                && event.target == lane.target
                && event
                    .note
                    .as_ref()
                    .is_some_and(|note| note.pitch == lane.pitch)
        })
        .and_then(|event| event.note.as_ref())
        .map_or_else(|| "·".into(), |note| format!("● {}", note.velocity))
}
