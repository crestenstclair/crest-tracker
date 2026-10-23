use super::*;
use crest_synth::shell::webview::host_presentation::{
    WorkspacePreview, WorkspaceSpan, WorkspaceTimeline,
};
use crest_tracker::composition::Event;

pub fn project(app: &App) -> HostView {
    let _timing = crest_synth::shell::performance::span(
        crest_synth::shell::performance::Stage::HostProjection,
    );
    let mut host = HostView {
        name: "CREST TRACKER".into(),
        entry: row("menu", "MENU", ""),
        action_overrides: vec![
            crest_synth::control::ValidAction::new(
                SemanticAction::PreviewStart,
                if app.playing {
                    "Stop playback"
                } else {
                    "Play song"
                },
                Some(format!(
                    "Start / {} / Space (also with Shift)",
                    app.workspace
                        .controller_bindings
                        .button(crest_synth::control::ControllerRole::Preview)
                        .label()
                )),
            ),
            crest_synth::control::ValidAction::new(
                SemanticAction::OpenMidiSettings,
                "Global settings",
                Some("Menu > Global settings"),
            ),
            crest_synth::control::ValidAction::new(
                SemanticAction::Return,
                "Main menu",
                Some("Shift+Q"),
            ),
        ],
        workspace: None,
    };
    if let Some(surface) = app.workspace.synth_surface {
        use crest_synth::control::{SurfaceId, ValidAction};
        let root = matches!(
            surface,
            SurfaceId::PatchMain | SurfaceId::MixerMain | SurfaceId::Sends
        );
        let label = if root { "Main menu" } else { "Back" };
        host.action_overrides.extend([
            ValidAction::new(
                SemanticAction::NavigatePage(Direction::Down),
                label,
                Some("Shift+Down"),
            ),
            ValidAction::new(
                SemanticAction::NavigatePage(Direction::Left),
                label,
                Some("Shift+Left"),
            ),
        ]);
        host.action_overrides[2] = ValidAction::new(SemanticAction::Return, label, Some("Shift+Q"));
        if surface == SurfaceId::MixerMain {
            host.action_overrides.push(ValidAction::new(
                SemanticAction::NavigatePage(Direction::Up),
                "Track sends",
                app.workspace.synth_detail_available.then_some("Shift+Up"),
            ));
        } else if surface == SurfaceId::MixerInspector {
            host.action_overrides.push(ValidAction::new(
                SemanticAction::NavigatePage(Direction::Up),
                "",
                None::<&str>,
            ));
        }
    }
    let dialog = !app.file_dialog.is_null();
    let shared = app.shared_edit_projection();
    if app.view == View::Instruments
        && app.workspace.panel.is_none()
        && !dialog
        && shared.is_null()
        && app.resource_repair.is_null()
        && app.load_progress.is_none()
    {
        return host;
    }
    let mut view = WorkspaceProjection {
        overlay: false,
        busy: false,
        panel_columns: 1,
        panel_caption: String::new(),
        summary: String::new(),
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
        hints: String::new(),
        text_entry: None,
    };
    if let Some(progress) = app.load_progress {
        project_grid(app, &mut view);
        view.overlay = true;
        view.busy = true;
        view.timeline = None;
        view.context = "LOADING SONG".into();
        view.panel_caption = progress.into();
        view.rows.clear();
        view.inspector.clear();
        view.hints = "Please wait · Controls resume when loading finishes".into();
        host.entry.enabled = false;
        host.action_overrides.clear();
        host.workspace = Some(view);
        return host;
    }
    if app.workspace.panel == Some(Panel::Text) {
        if let Some(entry) = &app.workspace.text {
            view.context = "TEXT ENTRY".into();
            view.title = entry.label.clone();
            view.text_entry = Some(entry.clone());
            view.rows = commands::text_rows(app);
        }
    } else if dialog {
        project_dialog(app, &mut view);
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
    if app.is_piano_roll() && piano::panel(app.workspace.panel) {
        piano::layout(&mut view.rows, app.workspace.panel.unwrap());
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
    view.hints = hints::project(app, &view);
    host.workspace = Some(view);
    host
}

fn project_panel(app: &App, panel: Panel, view: &mut WorkspaceProjection) {
    if panel == Panel::PianoVelocity && app.is_piano_roll() {
        project_piano(app, view);
        return;
    }
    if (matches!(panel, Panel::Chords | Panel::Effects | Panel::FxCommands)
        && app.view == View::Phrase)
        || panel == Panel::SongSlot
    {
        project_grid(app, view);
        view.overlay = true;
        view.inspector.clear();
    }
    if app.is_piano_roll() && piano::panel(Some(panel)) {
        project_piano(app, view);
        view.overlay = true;
        view.context = piano::title(app);
        view.panel_columns = 6;
        view.panel_caption = piano::caption(app);
    }
    view.context = match panel {
        Panel::Settings => "GLOBAL SETTINGS".into(),
        Panel::SongSettings => "SONG SETTINGS".into(),
        Panel::ChainSettings => "CHAIN SETTINGS".into(),
        Panel::Library => "SAVED INSTRUMENTS".into(),
        Panel::PhraseSettings => "PHRASE SETTINGS".into(),
        Panel::ChordEditor => "CHORD EDITOR".into(),
        Panel::Notes if app.is_piano_roll() => piano::title(app),
        Panel::Notes => "EDIT NOTES".into(),
        Panel::FxCommands => "FX COMMANDS".into(),
        Panel::SongSlot => "SONG SLOT".into(),
        Panel::NoteValues if app.is_piano_roll() => piano::title(app),
        Panel::NoteValues => "NOTE VALUES".into(),
        Panel::NoteSelection => "SELECTION / CLIPBOARD".into(),
        Panel::NoteHistory => "UNDO / REDO".into(),
        Panel::PianoView => "VIEW".into(),
        Panel::ChannelExpression => "CHANNEL AUTOMATION".into(),
        Panel::PianoTiming
        | Panel::PianoDefaults
        | Panel::PianoTarget
        | Panel::PianoQuantize
        | Panel::PianoStack
        | Panel::PianoVelocity => piano::title(app),
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
    if panel == Panel::Settings {
        view.tabs = settings::tabs(app);
    }
    view.rows = match panel {
        Panel::Menu => commands(&[
            ("home", "Song"),
            ("instruments", "Instruments"),
            ("chord_library", "Chord Bank"),
            ("mixer", "Mixer"),
            ("sends", "Sends"),
            ("settings", "Global settings"),
            ("files", "Song File"),
            ("library", "Saved instruments"),
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
        Panel::Repair => library::repair_rows(app),
        Panel::Midi => midi::rows(app),
        Panel::MidiBinding => midi::binding_rows(app),
        Panel::Arrangement => arrangement::rows(app),
        Panel::SongSlot => arrangement::slot_rows(),
        Panel::Generator => generator::rows(app),
        Panel::Effects => effects::rows(app),
        Panel::FxCommands => effects::command_rows(),
        Panel::Notes | Panel::NoteValues if app.is_piano_roll() => piano::rows(app, panel),
        Panel::PianoTiming
        | Panel::PianoDefaults
        | Panel::PianoTarget
        | Panel::PianoQuantize
        | Panel::PianoStack
        | Panel::PianoVelocity => piano::rows(app, panel),
        Panel::Notes => notes::rows(app),
        Panel::NoteValues => notes::value_rows(app),
        Panel::NoteSelection => notes::selection_rows(app),
        Panel::NoteHistory => commands(&[("undo", "Undo"), ("redo", "Redo"), ("back", "Back")]),
        Panel::PianoView => notes::view_rows(app),
        Panel::Expression => expression::rows(app),
        Panel::ChannelExpression => channel_expression::rows(app),
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
                "patch_capacity",
                "Patch capacity",
                app.workspace.patch_capacity.map_or_else(
                    || "Loading…".into(),
                    |budget| {
                        format!(
                            "{} · {} installed · max {}",
                            budget.limit, budget.installed, budget.maximum
                        )
                    },
                ),
            ),
            row(
                "synth_settings",
                "MIDI devices / controller / Synth settings",
                "",
            ),
            row("back", "Back", ""),
        ],
        Panel::ChainSettings => app.selected_chain().map_or_else(
            || commands(&[("back", "Back")]),
            |(_, chain)| {
                vec![
                    row("arr_chain_name", "Name", &chain.name),
                    row("back", "Back", ""),
                ]
            },
        ),
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
            ("library", "Saved instruments"),
            ("recover", "Recover interrupted work"),
            ("back", "Back"),
        ]),
        Panel::Chords => chord_rows(app),
        Panel::Text => vec![],
    };
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
        let start = app.column / 8 * 8;
        view.columns = view.columns.drain(..).skip(start).take(8).collect();
        for row in &mut view.grid {
            row.cells = row.cells.drain(..).skip(start).take(8).collect();
        }
    }
    if app.view == View::Phrase {
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
            app.song_play_heads
                .iter()
                .any(|head| head.track == track.id && head.chain_use == usage.id)
                || app.play_heads.iter().any(|head| {
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

/// One time interval owns cells, spans, velocity bars, and both cursors.
pub(super) struct RollWindow {
    pub from: Beat,
    pub until: Beat,
    length: Beat,
    step: Beat,
    start: usize,
    end: usize,
    low: u8,
}
impl RollWindow {
    fn new(app: &App) -> Self {
        let length = app
            .selected_phrase()
            .expect("Piano Roll")
            .1
            .length()
            .expect("validated phrase");
        let step = app.piano_paint_step();
        let window_start = app.roll_view.start.min(
            length
                .checked_sub(app.roll_view.width)
                .unwrap_or(Beat::ZERO),
        );
        let start = window_start.units(step).unwrap() as usize;
        let end_time = window_start
            .checked_add(app.roll_view.width)
            .unwrap_or(length)
            .min(length);
        let end = (u128::from(end_time.numerator()) * u128::from(step.denominator()))
            .div_ceil(u128::from(end_time.denominator()) * u128::from(step.numerator()))
            as usize;
        Self {
            from: step.checked_scale(start as u64, 1).unwrap(),
            until: step.checked_scale(end as u64, 1).unwrap(),
            length,
            step,
            start,
            end,
            low: app.roll_view.low_pitch,
        }
    }
    pub fn coordinate(&self, at: Beat) -> u32 {
        let local = at
            .max(self.from)
            .min(self.until)
            .checked_sub(self.from)
            .unwrap();
        let width = self.until.checked_sub(self.from).unwrap();
        ((local.numerator() as f64 / local.denominator() as f64)
            / (width.numerator() as f64 / width.denominator() as f64)
            * 10000.0)
            .round() as u32
    }
    fn pitches(&self) -> std::ops::RangeInclusive<u8> {
        self.low..=self.low + 12
    }
    fn contains(&self, at: Beat) -> bool {
        at >= self.from && at < self.until
    }
}

fn project_piano(app: &App, view: &mut WorkspaceProjection) {
    let (_, phrase) = app.selected_phrase().expect("Piano Roll");
    let window = RollWindow::new(app);
    let (at, pitch) = app.piano_cursor();
    view.context = format!("PHRASE {}", phrase.name);
    view.title = "PIANO ROLL".into();
    view.compact_grid = true;
    view.columns = piano_columns(app, &window);
    let selected_ids: std::collections::BTreeSet<_> =
        app.selected_event_ids().into_iter().collect();
    let mut lanes: [Vec<(&Event, bool)>; 128] = std::array::from_fn(|_| Vec::new());
    for event in &phrase.events {
        if let Some(note) = &event.note {
            lanes[note.pitch as usize].push((event, selected_ids.contains(&event.id)));
        }
    }
    view.grid = window
        .pitches()
        .rev()
        .map(|pitch| {
            piano_row(
                app,
                &lanes[pitch as usize],
                pitch,
                window.start,
                window.end,
                window.step,
            )
        })
        .collect();
    view.focus = format!("piano:{}:{pitch}", at.units(window.step).unwrap());
    view.timeline = Some(piano_timeline(app, &window, &view.context, &selected_ids));
    view.inspector = vec![
        row("piano_grid", "Step grid", piano::notation(app.roll_grid)),
        row(
            "piano_new_length",
            "New length",
            piano::notation(app.roll_new_length),
        ),
        row(
            "piano_defaults",
            "New velocity",
            format!("{} / 127", app.roll_new_velocity),
        ),
        row("piano_quantize", "Quantize", "Selected notes"),
        row("notes", "Tools", "Popover · West / ["),
        row("notes_view", "View", "Zoom / follow"),
        row("notes_history", "History", "Undo / redo"),
    ];
    view.summary = piano::summary(app);
    let meter = app.editor.song().meters[0].meter;
    view.subtitle = format!(
        "TRACK {:02} · {} BEATS · {}/{}",
        app.track,
        generator::beat_label(window.length),
        meter.numerator(),
        meter.denominator()
    );
}
fn piano_columns(app: &App, window: &RollWindow) -> Vec<String> {
    let meter = app.editor.song().meters[0].meter;
    let pulse = Beat::new(4, u64::from(meter.denominator())).unwrap();
    (window.start..window.end)
        .map(|index| {
            let at = window.step.checked_scale(index as u64, 1).unwrap();
            let pulses = at.units(pulse).unwrap();
            if pulse.checked_scale(pulses, 1).unwrap() == at {
                format!(
                    "{}.{}",
                    pulses / u64::from(meter.numerator()) + 1,
                    pulses % u64::from(meter.numerator()) + 1
                )
            } else if app.roll_view.width < Beat::integer(2) {
                generator::beat_label(at)
            } else {
                String::new()
            }
        })
        .collect()
}
fn piano_timeline(
    app: &App,
    window: &RollWindow,
    label: &str,
    selected: &std::collections::BTreeSet<crest_tracker::composition::EventId>,
) -> WorkspaceTimeline {
    let (_, phrase) = app.selected_phrase().unwrap();
    let (at, pitch) = app.piano_cursor();
    let preview = piano::preview(app).unwrap_or_default();
    let preview_ids: std::collections::BTreeSet<_> = preview.iter().map(|event| event.id).collect();
    let span = |event: &Event| {
        piano_span(
            app,
            event,
            window,
            selected.contains(&event.id) || preview_ids.contains(&event.id),
        )
    };
    let spans = phrase
        .events
        .iter()
        .filter(|event| !preview_ids.contains(&event.id))
        .chain(preview.iter())
        .filter_map(span)
        .collect();
    let originals = phrase
        .events
        .iter()
        .filter(|event| preview_ids.contains(&event.id))
        .filter_map(span)
        .collect();
    let key = phrase.key.as_ref().unwrap_or(&app.editor.song().key);
    WorkspaceTimeline {
        label: label.into(),
        caption: format!(
            "{} · {}",
            key.name(),
            app.roll_new_target
                .as_ref()
                .or_else(|| phrase.events.first().map(|event| &event.target))
                .map_or("Choose instrument".into(), |target| app
                    .target_label(target))
        ),
        view_label: format!(
            "GRID {} · VIEW {}–{} · FOLLOW {}",
            piano::notation(app.roll_grid),
            generator::beat_label(window.from),
            generator::beat_label(window.until.min(window.length)),
            if app.roll_view.follow { "ON" } else { "OFF" }
        ),
        muted_lanes: window
            .pitches()
            .rev()
            .enumerate()
            .filter_map(|(lane, pitch)| {
                (app.roll_view.scale_guide
                    && !key.contains(crest_tracker::harmony::Pitch::new(pitch).unwrap()))
                .then_some(lane)
            })
            .collect(),
        originals,
        beat_columns: (window.start..window.end)
            .enumerate()
            .filter_map(|(column, index)| {
                (window
                    .step
                    .checked_scale(index as u64, 1)
                    .unwrap()
                    .denominator()
                    == 1)
                    .then_some(column)
            })
            .collect(),
        cursor_lane: window
            .pitches()
            .contains(&pitch)
            .then(|| (window.low + 12 - pitch) as usize),
        level_action: row("piano_velocity", "VELOCITY", "1–127"),
        level_focused: app.workspace.panel == Some(Panel::PianoVelocity),
        curve: match app.workspace.panel {
            Some(Panel::Expression) => expression::curve(app, window),
            Some(Panel::ChannelExpression) => channel_expression::curve(app, window),
            _ => None,
        },
        cursor_width: window.coordinate(
            window
                .from
                .checked_add(app.roll_grid)
                .unwrap_or(window.until),
        ),
        cursor: window.contains(at).then(|| window.coordinate(at)),
        secondary_lanes: window
            .pitches()
            .rev()
            .enumerate()
            .filter_map(|(lane, pitch)| matches!(pitch % 12, 1 | 3 | 6 | 8 | 10).then_some(lane))
            .collect(),
        spans,
        playhead: app
            .play_phrase_beat
            .filter(|beat| app.playing && window.contains(*beat))
            .map(|beat| window.coordinate(beat)),
    }
}
fn piano_span(
    app: &App,
    event: &Event,
    window: &RollWindow,
    selected: bool,
) -> Option<WorkspaceSpan> {
    let note = event.note.as_ref()?;
    let end = event.at.checked_add(note.duration).ok()?;
    if !window.pitches().contains(&note.pitch) || event.at >= window.until || end <= window.from {
        return None;
    }
    Some(WorkspaceSpan {
        id: format!("note:{}", event.id.get()),
        level_id: format!("velocity:{}", event.id.get()),
        label: format!(
            "{}{}{}",
            if event.at < window.from { "‹ " } else { "" },
            piano::pitch_label(app, note.pitch),
            if end > window.until || end > window.length {
                " ›"
            } else {
                ""
            }
        ),
        lane: (window.low + 12 - note.pitch) as usize,
        start: window.coordinate(event.at),
        end: window.coordinate(end),
        level: note.velocity,
        selected,
    })
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
        label: piano::pitch_label(app, pitch),
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
    let mut selected = false;
    for (event, is_selected) in events {
        let note = event.note.as_ref().expect("pitched event");
        if event.at < until
            && event
                .at
                .checked_add(note.duration)
                .is_ok_and(|end| end > at)
        {
            selected |= is_selected;
        }
    }
    let mut cell = row(
        format!("piano:{index}:{pitch}"),
        format!("Beat {}", generator::beat_label(at)),
        "",
    );
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
        controls: vec![],
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
}

fn editor_preview(app: &App, view: &mut WorkspaceProjection) {
    let Some(draft) = &app.workspace.chord else {
        return;
    };
    view.preview = Some(WorkspacePreview {
        title: draft.name.clone(),
        caption: "INTERVALS / FROM ROOT".into(),
        controls: view
            .rows
            .iter()
            .filter(|row| chord_editor::is_detail_control(&row.id))
            .cloned()
            .collect(),
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
