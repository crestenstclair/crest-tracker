use super::*;
use crest_synth::shell::webview::host_presentation::{
    WorkspacePreview, WorkspaceSpan, WorkspaceTimeline,
};
use crest_tracker::composition::Event;

pub fn project(app: &App) -> HostView {
    let mut host = HostView {
        name: "CREST TRACKER".into(),
        entry: if app.view == View::Instruments {
            row("home", "SONG", "")
        } else {
            row("menu", "MENU", "")
        },
        action_overrides: vec![
            crest_synth::control::ValidAction::new(
                SemanticAction::OpenMidiSettings,
                "Global settings",
                Some("Options"),
            ),
            crest_synth::control::ValidAction::new(
                SemanticAction::Return,
                "Main menu",
                Some("Shift+Q"),
            ),
        ],
        workspace: None,
    };
    let dialog = !app.file_dialog.is_null();
    let shared = app.shared_edit_projection();
    if app.view == View::Instruments
        && app.workspace.panel.is_none()
        && !dialog
        && shared.is_null()
        && app.resource_repair.is_null()
    {
        return host;
    }
    let mut view = WorkspaceProjection {
        overlay: false,
        timeline: None,
        preview: None,
        playing: app.playing,
        context: format!("{:?}", app.view).to_uppercase(),
        title: app.editor.song().name.clone(),
        subtitle: format!(
            "{} BPM · {} · {}",
            app.editor.song().tempo.bpm(),
            app.editor.song().key.name(),
            if app.playing { "PLAYING" } else { "STOPPED" }
        ),
        status: format!(
            "{}{}",
            if app.dirty() { "UNSAVED · " } else { "" },
            app.status
        ),
        focus: String::new(),
        adjusting: app.edit_held,
        columns: vec![],
        compact_grid: false,
        grid: vec![],
        rows: vec![],
        tabs: vec![],
        inspector_title: "ACTIONS".into(),
        inspector: vec![],
        hints: "D-pad / arrows: choose · Edit / Return: select · Shift+Left: back".into(),
        text_entry: None,
    };
    if app.workspace.panel == Some(Panel::Text) {
        if let Some(entry) = &app.workspace.text {
            view.context = "TEXT ENTRY".into();
            view.title = entry.label.clone();
            view.text_entry = Some(entry.clone());
            view.rows = commands::text_rows(app);
            view.hints="Keyboard: type, Enter accepts · Controller: choose character with Edit+direction, select to append; Accept saves".into();
        }
    } else if dialog {
        project_dialog(app, &mut view);
        view.hints =
            "D-pad / arrows: file or action · Edit / Return: choose · Shift+Left: cancel".into();
    } else if !shared.is_null() {
        project_shared(&shared, &mut view);
    } else if !app.resource_repair.is_null() {
        view.context = "RESOURCE REPAIR".into();
        view.status = app.resource_repair["error"].as_str().unwrap_or("").into();
        view.rows = library::repair_rows(app);
    } else if let Some(panel) = app.workspace.panel {
        project_panel(app, panel, &mut view);
    } else {
        project_grid(app, &mut view);
    }
    if !view.rows.is_empty() {
        view.focus = app
            .workspace
            .focus
            .as_ref()
            .filter(|id| view.tabs.iter().chain(&view.rows).any(|row| &row.id == *id))
            .cloned()
            .unwrap_or_else(|| {
                view.tabs
                    .iter()
                    .find(|row| row.selected)
                    .unwrap_or(&view.rows[0])
                    .id
                    .clone()
            });
    }
    if app.workspace.panel == Some(Panel::Chords) {
        chord_preview(app, &mut view);
    }
    if app.workspace.panel == Some(Panel::ChordEditor) {
        editor_preview(app, &mut view);
    }
    host.workspace = Some(view);
    host
}

fn project_panel(app: &App, panel: Panel, view: &mut WorkspaceProjection) {
    if matches!(panel, Panel::Chords | Panel::Effects | Panel::FxCommands)
        && app.view == View::Phrase
    {
        project_grid(app, view);
        view.overlay = true;
        view.inspector.clear();
    }
    view.context = match panel {
        Panel::QuickJump => "QUICK JUMP".into(),
        Panel::Settings => "GLOBAL SETTINGS".into(),
        Panel::SongSettings => "SONG SETTINGS".into(),
        Panel::PhraseSettings => "PHRASE SETTINGS".into(),
        Panel::ChordEditor => "CHORD EDITOR".into(),
        Panel::Notes => "EDIT NOTES".into(),
        Panel::FxCommands => "FX COMMANDS".into(),
        Panel::NoteValues => "NOTE VALUES".into(),
        Panel::NoteSelection => "SELECTION / CLIPBOARD".into(),
        Panel::NoteHistory => "UNDO / REDO".into(),
        Panel::ChordLibrary => "CHORD LIBRARY".into(),
        Panel::MidiBinding => "MIDI BINDING".into(),
        _ => format!("{panel:?}").to_uppercase(),
    };
    if !view.overlay {
        view.title = view.context.clone();
        view.context = if matches!(panel, Panel::ChordEditor | Panel::ChordLibrary) {
            "CHORD BANK".into()
        } else if let Some((_, phrase)) = app.selected_phrase().filter(|_| app.view == View::Phrase)
        {
            format!("PHRASE {}", phrase.name)
        } else {
            format!("{:?}", app.view).to_uppercase()
        };
    }
    view.hints = "D-pad: choose · Edit / Return: select · Shift+Left: back".into();
    if panel == Panel::Settings {
        view.tabs = settings::tabs(app);
        view.hints =
            "Top tabs: Left/Right · Down: fields · Edit+←/→: ±1 · Edit+↑/↓: ±10 · Shift+Left: back"
                .into();
    } else if matches!(
        panel,
        Panel::Effects | Panel::Generator | Panel::PhraseSettings | Panel::NoteValues
    ) {
        view.hints =
            "D-pad: field · Edit+←/→: fine · Edit+↑/↓: coarse · Edit: choose · Shift+Left: back"
                .into();
    } else if panel == Panel::Chords {
        view.hints = "D-pad: chord · Edit / Return: use chord · Cancel: Phrase".into();
    }
    view.rows = match panel {
        Panel::Menu => commands(&[
            ("home", "Song"),
            ("instruments", "Instruments"),
            ("chord_library", "Chord Bank"),
            ("performance", "Performance clips"),
            ("mixer", "Mixer"),
            ("sends", "Sends"),
            ("settings", "Global settings"),
            ("files", "Song File"),
            ("library", "Sound library"),
            ("tools", "Editing tools"),
        ]),
        Panel::QuickJump => commands(&[
            ("home", "Song"),
            ("chords", "Chord picker"),
            ("instruments", "Instruments"),
            ("files", "Song File"),
            ("mixer", "Mixer"),
            ("sends", "Sends"),
            ("settings", "Global settings"),
            ("tools", "Editing tools"),
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
            ("import_midi", "Import MIDI file (.mid / .midi)"),
            ("back", "Back"),
        ]),
        Panel::Library => library::sound_rows(app),
        Panel::ChordLibrary => library::chord_rows(app),
        Panel::Performance => performance::rows(app),
        Panel::PerformanceEdit => {
            performance_edit::project(app, view);
            performance_edit::rows(app)
        }
        Panel::Repair => library::repair_rows(app),
        Panel::Midi => midi::rows(app),
        Panel::MidiBinding => midi::binding_rows(app),
        Panel::Arrangement => arrangement::rows(app),
        Panel::Generator => generator::rows(app),
        Panel::Effects => effects::rows(app),
        Panel::FxCommands => effects::command_rows(),
        Panel::Notes => notes::rows(app),
        Panel::NoteValues => notes::value_rows(app),
        Panel::NoteSelection => notes::selection_rows(app),
        Panel::NoteHistory => commands(&[("undo", "Undo"), ("redo", "Redo"), ("back", "Back")]),
        Panel::Expression => notes::expression_rows(app),
        Panel::Settings => settings::rows(app),
        Panel::ChordEditor => chord_editor::rows(app),
        Panel::SongSettings => vec![
            row("name", "Name", &app.editor.song().name),
            row(
                "meter",
                "Meter (numerator/denominator)",
                format!(
                    "{}/{}",
                    app.editor.song().meters[0].meter.numerator(),
                    app.editor.song().meters[0].meter.denominator()
                ),
            ),
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
        Panel::PhraseSettings => phrase::rows(app),
        Panel::Tools => commands(&[
            ("undo", "Undo"),
            ("redo", "Redo"),
            ("clear", "Clear selected value"),
            ("notes_off", "Note OFF at cursor"),
            ("phrase_settings", "Phrase settings"),
            ("arrangement", "Track / Chain / occurrence tools"),
            ("chords", "Chord picker"),
            ("effects", "FX command editor"),
            ("notes", "Selection / Piano Roll tools"),
            ("generator", "Generator settings"),
            ("library", "Sound library"),
            ("recover", "Recover interrupted work"),
            ("back", "Back"),
        ]),
        Panel::Chords => chord_rows(app),
        Panel::Text => vec![],
    };
    if panel == Panel::QuickJump && app.view == View::Minimap {
        view.rows.insert(
            1,
            row(
                "minimap_follow",
                "Follow playback",
                if app.workspace.minimap_follow_paused {
                    "Off"
                } else {
                    "On"
                },
            ),
        );
    }
}

fn project_grid(app: &App, view: &mut WorkspaceProjection) {
    if app.view == View::Minimap {
        minimap::project(app, view);
        return;
    }
    if generator::active(app) {
        generator::project(app, view);
        return;
    }
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
            let mut row = grid_row(index, cells, playing_row(app, index));
            if app.view == View::Song {
                for (column, cell) in row.cells.iter_mut().enumerate() {
                    cell.playing = playing_song_cell(app, index, column);
                }
            }
            row
        })
        .collect();
    view.focus = format!("cell:{}:{}", app.row, app.column);
    if app.view == View::Song {
        view.hints = "D-pad: any slot · Edit+←/→: chain ±1 · Edit+↑/↓: ±10 · Shift+Right: open · Shift+Up: settings · Start / Space: play".into();
        let start = app.column / 8 * 8;
        view.columns = view.columns.drain(..).skip(start).take(8).collect();
        for row in &mut view.grid {
            row.cells = row.cells.drain(..).skip(start).take(8).collect();
        }
    }
    if app.view == View::Chain {
        view.hints = "D-pad: phrase / repeats · Edit+←/→: fine · Edit+↑/↓: coarse repeats · Shift+Right: open phrase · Shift+Left: Song · Shift+Down: Quick Jump · Start / Space: play".into();
    }
    if app.view == View::Phrase {
        let edit = match app.column {
            0 => "Edit+←/→: note · Edit+↑/↓: octave",
            1 => "Edit+direction: instrument",
            2 => "Edit+←/→: velocity ±1 · Edit+↑/↓: ±10",
            _ => "Edit+↑: command list",
        };
        view.hints = format!("D-pad: cell · {edit} · Shift+Left: Chain · Shift+Up: phrase settings · Shift+Down: Quick Jump · Start / Space: play");
        view.inspector = commands(&[
            ("notes", "Note actions"),
            ("effects", "FX"),
            ("chords", "Chords"),
            ("notes_off", "OFF"),
            ("phrase_settings", "Phrase settings"),
        ]);
    }
    if app.is_piano_roll() {
        project_piano(app, view);
    }
    if app.is_drum() {
        project_drum(app, view);
    }
}

fn playing_song_cell(app: &App, index: usize, column: usize) -> bool {
    if !app.playing {
        return false;
    }
    let track = &app.editor.song().tracks[column];
    track
        .slots
        .get(index)
        .and_then(Option::as_ref)
        .is_some_and(|usage| {
            app.play_heads.iter().any(|head| {
                head.occurrence.track == track.id && head.occurrence.chain_use == usage.id
            })
        })
}

fn playing_row(app: &App, index: usize) -> bool {
    if !app.playing {
        return false;
    }
    let song = app.editor.song();
    match app.view {
        // Song rows are arrangement slots, not shared musical time. Each track
        // marks its own cell; only single-track views have a row playhead.
        View::Song => false,
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
        label: format!("{index:02}"),
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
    view.context = format!("PHRASE {}", phrase.name);
    view.title = "PIANO ROLL".into();
    let (at, pitch) = app.piano_cursor();
    let step = app.roll_grid;
    view.compact_grid = true;
    let selected = (u128::from(at.numerator()) * u128::from(step.denominator())
        / (u128::from(at.denominator()) * u128::from(step.numerator())))
        as usize;
    let start = selected / 32 * 32;
    let length = phrase.length().expect("validated phrase");
    let total = (length.numerator() as u128 * step.denominator() as u128)
        .div_ceil(length.denominator() as u128 * step.numerator() as u128) as usize;
    let end = (start + 32).min(total);
    let low = (pitch / 12 * 12).min(115);
    view.columns = (start..end)
        .map(|index| {
            let at = step.checked_scale(index as u64, 1).unwrap();
            let end = at.checked_add(step).unwrap();
            format!(
                "{}{:02}",
                if app.playing && app.play_phrase_beat.is_some_and(|b| at <= b && b < end) {
                    "▶"
                } else {
                    ""
                },
                index
            )
        })
        .collect();
    let selected_ids: std::collections::BTreeSet<_> =
        app.selected_event_ids().into_iter().collect();
    let mut lanes: [Vec<(&Event, bool)>; 128] = std::array::from_fn(|_| Vec::new());
    for event in &phrase.events {
        if let Some(note) = &event.note {
            lanes[note.pitch as usize].push((event, selected_ids.contains(&event.id)));
        }
    }
    view.grid = (low..=low + 12)
        .rev()
        .map(|pitch| piano_row(app, &lanes[pitch as usize], pitch, start, end, step))
        .collect();
    view.focus = format!("piano:{selected}:{pitch}");
    let from = step.checked_scale(start as u64, 1).unwrap();
    let until = step.checked_scale(end as u64, 1).unwrap();
    let width = until.checked_sub(from).unwrap();
    let coordinate = |at: Beat| -> u32 {
        let local = at.max(from).min(until).checked_sub(from).unwrap();
        ((local.numerator() as f64 / local.denominator() as f64)
            / (width.numerator() as f64 / width.denominator() as f64)
            * 10000.0)
            .round() as u32
    };
    let spans = phrase
        .events
        .iter()
        .filter_map(|event| {
            let note = event.note.as_ref()?;
            let end = event.at.checked_add(note.duration).ok()?;
            if !(low..=low + 12).contains(&note.pitch) || event.at >= until || end <= from {
                return None;
            }
            Some(WorkspaceSpan {
                id: format!("note:{}", event.id.get()),
                label: format!(
                    "{} · {}",
                    crest_tracker::harmony::Pitch::new(note.pitch)
                        .ok()?
                        .name(crest_tracker::harmony::Spelling::Sharps),
                    note.velocity
                ),
                lane: (low + 12 - note.pitch) as usize,
                start: coordinate(event.at),
                end: coordinate(end),
                level: note.velocity,
                selected: selected_ids.contains(&event.id),
            })
        })
        .collect();
    view.timeline = Some(WorkspaceTimeline {
        secondary_lanes: (low..=low + 12)
            .rev()
            .enumerate()
            .filter_map(|(lane, pitch)| matches!(pitch % 12, 1 | 3 | 6 | 8 | 10).then_some(lane))
            .collect(),
        spans,
        playhead: app
            .play_phrase_beat
            .filter(|beat| app.playing && *beat >= from && *beat < until)
            .map(coordinate),
    });
    view.inspector.push(row(
        "notes_grid",
        "Grid",
        format!("{} beats", generator::beat_label(step)),
    ));
    view.subtitle = format!(
        "GRID {} beats · CURSOR {} beats · {} · {} selected",
        generator::beat_label(step),
        generator::beat_label(at),
        app.editor
            .song()
            .key
            .pitch_name(crest_tracker::harmony::Pitch::new(pitch).unwrap()),
        selected_ids.len()
    );
    view.hints = "D-pad / left stick: cursor · Edit: add/remove · Edit+←/→: length · Right stick: length / velocity · Edit+↑: tools · Select: range · R1: fine · Start: play".into();
}

fn piano_row(
    app: &App,
    events: &[(&Event, bool)],
    pitch: u8,
    start: usize,
    end: usize,
    step: Beat,
) -> WorkspaceGridRow {
    let cells = (start..end)
        .map(|index| piano_cell(app, events, pitch, index, step))
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

fn piano_cell(
    app: &App,
    events: &[(&Event, bool)],
    pitch: u8,
    index: usize,
    step: Beat,
) -> WorkspaceRow {
    let at = step
        .checked_scale(index as u64, 1)
        .expect("validated phrase");
    let until = at.checked_add(step).expect("validated phrase");
    let mut velocity = None;
    let mut held = false;
    let mut selected = false;
    for (event, is_selected) in events {
        let note = event.note.as_ref().expect("pitched event");
        if event.at < until
            && event
                .at
                .checked_add(note.duration)
                .is_ok_and(|end| end > at)
        {
            held = true;
            selected |= is_selected;
            if event.at >= at && velocity.is_none() {
                velocity = Some(note.velocity);
            }
        }
    }
    let value = String::new();
    let _ = (velocity, held);
    let mut cell = row(format!("piano:{index}:{pitch}"), "", value);
    cell.selected = selected;
    cell.playing = app.playing && app.play_phrase_beat.is_some_and(|b| at <= b && b < until);
    cell
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
    let start = app.row / 16 * 16;
    let end = (start + 16).min(phrase.steps as usize);
    view.compact_grid = true;
    view.columns = (start..end).map(|step| format!("{step:02}")).collect();
    view.grid = lanes
        .iter()
        .enumerate()
        .map(|(lane, drum)| WorkspaceGridRow {
            id: format!("drum:{lane}"),
            label: format!("{} · {}", drum.pitch, app.target_label(&drum.target)),
            playing: false,
            cells: (start..end)
                .map(|step| {
                    let at = phrase
                        .division
                        .beat_length()
                        .checked_scale(step as u64, 1)
                        .unwrap();
                    let hit = phrase.events.iter().find(|event| {
                        event.at == at
                            && event.target == drum.target
                            && event
                                .note
                                .as_ref()
                                .is_some_and(|note| note.pitch == drum.pitch)
                    });
                    let mut cell = row(
                        format!("cell:{step}:{lane}"),
                        "",
                        hit.and_then(|event| event.note.as_ref())
                            .map_or("·".into(), |note| note.velocity.to_string()),
                    );
                    cell.selected = hit.is_some();
                    cell.playing = app.playing && app.play_row == Some(step as u32);
                    cell
                })
                .collect(),
        })
        .collect();
    view.hints = "Left/Right: step · Up/Down: drum lane · Edit: toggle hit · Edit+direction: velocity · Shift+Left: Chain".into();
}

fn chord_preview(app: &App, view: &mut WorkspaceProjection) {
    let choices = chords::choices(app);
    let Some((_, _, chord)) = choices
        .iter()
        .find(|(id, _, _)| *id == view.focus)
        .or_else(|| choices.first())
    else {
        return;
    };
    let root = effects::selected_event(app)
        .and_then(|event| event.note)
        .map_or(60, |note| note.pitch);
    view.preview = Some(WorkspacePreview {
        title: chords::display_name(chord),
        caption: "INTERVALS / FROM ROOT".into(),
        values: chord
            .definition()
            .shape()
            .intervals()
            .iter()
            .map(|interval| chords::interval_label(&[*interval]))
            .collect(),
        keys: vec![],
        footer: chord
            .definition()
            .resolve(crest_tracker::harmony::Pitch::new(root).unwrap())
            .map(|pitches| {
                pitches
                    .iter()
                    .map(|pitch| pitch.name(crest_tracker::harmony::Spelling::Sharps))
                    .collect::<Vec<_>>()
                    .join("   ")
            })
            .unwrap_or_else(|error| error.to_string()),
    });
    view.hints =
        "D-pad: choose chord · Edit: apply · Back: cancel · Chords use the selected note's root"
            .into();
}

fn editor_preview(app: &App, view: &mut WorkspaceProjection) {
    let Some(draft) = &app.workspace.chord else {
        return;
    };
    view.hints = "D-pad: interval / field · Edit: toggle · Edit+Up/Down on interval: octave · Save chord: publish to library · Back: cancel".into();
    view.preview = Some(WorkspacePreview {
        title: draft.name.clone(),
        caption: "INTERVALS / FROM ROOT".into(),
        values: draft
            .intervals
            .iter()
            .map(|interval| chords::interval_label(&[*interval]))
            .collect(),
        keys: (0..=12)
            .map(|key| {
                let interval = key + app.workspace.chord_octave * 12;
                let mut key = row(
                    format!("chord_key:{interval}"),
                    chords::interval_label(&[interval]),
                    interval.to_string(),
                );
                key.selected = draft.intervals.contains(&interval);
                key
            })
            .collect(),
        footer: draft
            .verified()
            .map(|chord| {
                chord
                    .verification()
                    .pitches()
                    .iter()
                    .map(|pitch| pitch.name(crest_tracker::harmony::Spelling::Sharps))
                    .collect::<Vec<_>>()
                    .join("   ")
            })
            .unwrap_or_else(|error| error.to_string()),
    });
}
