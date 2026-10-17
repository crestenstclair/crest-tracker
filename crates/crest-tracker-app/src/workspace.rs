//! Tracker interaction reducer; physical input and rendering remain Synth-owned.
mod arrangement;
mod chord_editor;
pub(crate) mod chords;
mod commands;
mod effects;
mod files;
mod generator;
mod library;
mod midi;
mod minimap;
mod notes;
mod performance;
mod performance_edit;
mod phrase;
mod projection;
mod settings;
pub(crate) mod sticks;
#[cfg(test)]
mod tests;
use crate::app::{App, CaptureAction, FileAction, PianoControl, Request, SynthPage, View};
use chords::{chord_action, chord_rows};
use commands::{activate, submit, text_entry};
use crest_synth::control::{Direction, InteractionMode, SemanticAction, TopLevelContext};
use crest_synth::shell::webview::host_presentation::{
    HostView, WorkspaceEvent, WorkspaceGridRow, WorkspaceProjection, WorkspaceRow,
    WorkspaceTextEntry,
};
use crest_tracker::composition::PhraseForm;
use crest_tracker::editing::Command;
use crest_tracker::time::{Beat, Tempo};
use files::{dialog_action, file_cancel, project_dialog};
pub use projection::project;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Panel {
    Menu,
    QuickJump,
    Settings,
    SongSettings,
    PhraseSettings,
    File,
    Tools,
    Arrangement,
    Chords,
    ChordEditor,
    Generator,
    Effects,
    FxCommands,
    Notes,
    NoteValues,
    NoteSelection,
    NoteHistory,
    Expression,
    Library,
    ChordLibrary,
    Performance,
    PerformanceEdit,
    Repair,
    Midi,
    MidiBinding,
    Text,
}

#[derive(Default)]
pub struct Navigation {
    panel: Option<Panel>,
    synth_return: Option<(View, usize, usize, SynthPage)>,
    focus: Option<String>,
    text: Option<WorkspaceTextEntry>,
    gesture: u64,
    opening_direction: Option<Direction>,
    parents: Vec<(Option<Panel>, Option<String>)>,
    chord: Option<crate::app::ChordDraft>,
    chord_octave: i8,
    character: usize,
    fx: Option<crest_tracker::composition::Fx>,
    fx_slot: usize,
    chord_slot: u8,
    minimap_follow_paused: bool,
    expression_index: usize,
    master_expression: bool,
    midi_binding: Option<crate::preferences::MidiControlBinding>,
    midi_binding_index: Option<usize>,
    settings_tab: settings::Tab,
}

fn row(id: impl Into<String>, label: impl Into<String>, value: impl Into<String>) -> WorkspaceRow {
    WorkspaceRow {
        id: id.into(),
        label: label.into(),
        value: value.into(),
        selected: false,
        playing: false,
    }
}
fn commands(values: &[(&str, &str)]) -> Vec<WorkspaceRow> {
    values
        .iter()
        .map(|(id, label)| row(*id, *label, ""))
        .collect()
}
fn open(app: &mut App, panel: Option<Panel>) {
    if panel.is_some() && panel != app.workspace.panel {
        app.workspace
            .parents
            .push((app.workspace.panel, app.workspace.focus.clone()));
    } else if panel.is_none() {
        app.workspace.parents.clear();
    }
    app.workspace.panel = panel;
    app.workspace.focus = None;
    app.workspace.text = None;
    cancel_edit(app);
}
pub(crate) fn open_performance_editor(app: &mut App) {
    open(app, Some(Panel::PerformanceEdit));
    app.workspace.focus = Some("performance_pitch".into());
}
fn back(app: &mut App) {
    let (panel, focus) = app.workspace.parents.pop().unwrap_or((None, None));
    app.workspace.panel = panel;
    app.workspace.focus = focus;
    app.workspace.text = None;
    cancel_edit(app);
}
pub fn text_entry_active(app: &App) -> bool {
    app.workspace.text.is_some()
}

pub fn owns_input(app: &App) -> bool {
    app.view != View::Instruments
        || app.workspace.panel.is_some()
        || !app.file_dialog.is_null()
        || !app.shared_edit_projection().is_null()
        || !app.resource_repair.is_null()
}
pub fn cancel_edit(app: &mut App) {
    app.edit_held = false;
    app.requests.push(Request::EndGesture);
}

pub(crate) fn follow_playback(app: &mut App, at: Beat) {
    if app.view == View::Minimap
        && !app.workspace.minimap_follow_paused
        && app.workspace.panel.is_none()
    {
        if let Err(error) = app.follow_minimap_window(at) {
            app.status = error.to_string();
        }
    }
}

/// Both keyboard and SDL call this same semantic boundary. Native Synth actions
/// fall through while its workspace is active, preserving all original pages.
pub fn action(app: &mut App, action: SemanticAction) -> bool {
    if action == SemanticAction::Return {
        return_to_parent(app);
        return true;
    }
    if action == SemanticAction::OpenMidiSettings {
        open(app, Some(Panel::Settings));
        return true;
    }
    if !owns_input(app) {
        if matches!(action, SemanticAction::NavigatePage(_)) {
            app.requests.push(Request::SynthAction { action });
            return true;
        }
        return false;
    }
    match action {
        SemanticAction::SetInteractionMode(mode) => {
            if mode == InteractionMode::Adjust && !app.edit_held {
                app.workspace.gesture = app.workspace.gesture.checked_add(1).unwrap_or(1);
            }
            app.edit_held = mode == InteractionMode::Adjust;
            if !app.edit_held {
                app.workspace.opening_direction = None;
                app.requests.push(Request::EndGesture);
            }
        }
        SemanticAction::Navigate(direction) => navigate(app, direction),
        SemanticAction::NavigatePage(direction) => navigate_page(app, direction),
        SemanticAction::Adjust(direction) => adjust(app, direction),
        SemanticAction::Activate => activate_focused(app),
        SemanticAction::PreviewStart
            if matches!(
                app.workspace.panel,
                Some(Panel::Performance | Panel::PerformanceEdit)
            ) =>
        {
            app.requests.push(performance::transport(app));
        }
        SemanticAction::PreviewStart => app.requests.push(Request::Play),
        SemanticAction::PreviewStop => {}
        SemanticAction::ToggleTestMidi => open(app, Some(Panel::Menu)),
        SemanticAction::OpenMidiSettings => unreachable!(),
        SemanticAction::SelectContext(TopLevelContext::Patch) => synth(app, SynthPage::Instruments),
        SemanticAction::SelectContext(TopLevelContext::Mixer) => synth(app, SynthPage::Mixer),
        SemanticAction::Send(_) => synth(app, SynthPage::Sends),
        SemanticAction::SelectPatch(Direction::Left) => {
            return_to_parent(app);
        }
        SemanticAction::SelectPatch(Direction::Right) => app.requests.push(Request::Clear),
        _ => {}
    }
    true
}

fn return_to_parent(app: &mut App) {
    if app.workspace.panel == Some(Panel::Text) {
        back(app);
    } else if !app.file_dialog.is_null() {
        file_cancel(app);
    } else if let Some(token) = app.shared_edit_projection()["token"].as_u64() {
        app.requests.push(Request::SharedDecision {
            token,
            choice: None,
            remember: false,
        });
    } else if !app.resource_repair.is_null() {
        app.requests.push(Request::RepairResource {
            index: 0,
            action: "cancel".into(),
        });
    } else if app.workspace.panel.is_some() {
        back(app);
    } else if matches!(app.view, View::Song | View::Instruments) {
        open(app, Some(Panel::Menu));
    } else {
        app.requests.push(Request::Back);
    }
}

fn navigate_page(app: &mut App, direction: Direction) {
    if app.workspace.panel.is_some() && matches!(direction, Direction::Up | Direction::Down) {
        back(app);
        return;
    }
    if direction == Direction::Down {
        if app.view == View::Song && app.workspace.panel.is_none() {
            synth(app, SynthPage::Mixer);
        } else {
            open(app, Some(Panel::QuickJump));
        }
        return;
    }
    match (app.workspace.panel, app.view, direction) {
        (Some(Panel::File), _, Direction::Up) => open(app, Some(Panel::SongSettings)),
        (_, View::Song, Direction::Up) => open(app, Some(Panel::Settings)),
        (_, View::Phrase, Direction::Up) => open(app, Some(Panel::PhraseSettings)),
        (None, View::Song | View::Chain, Direction::Right) => app.requests.push(Request::Confirm),
        (_, _, Direction::Left) => {
            return_to_parent(app);
        }
        _ => {}
    }
}

fn navigate(app: &mut App, direction: Direction) {
    let view = project(app).workspace.expect("Tracker owns input");
    if app.workspace.panel == Some(Panel::Settings)
        && settings::navigate(app, direction, &view.focus)
    {
        return;
    }
    if !view.rows.is_empty() {
        let index = view
            .rows
            .iter()
            .position(|row| row.id == view.focus)
            .unwrap_or(0);
        let delta = if matches!(direction, Direction::Up | Direction::Left) {
            -1
        } else {
            1
        };
        let next = (index as isize + delta).clamp(0, view.rows.len() as isize - 1) as usize;
        app.workspace.focus = Some(view.rows[next].id.clone());
        return;
    }
    let (horizontal, vertical) = direction_delta(direction);
    if app.view == View::Minimap {
        app.workspace.minimap_follow_paused = true;
    }
    app.requests.push(Request::Move {
        horizontal,
        vertical,
    });
}
fn direction_delta(direction: Direction) -> (i32, i32) {
    match direction {
        Direction::Up => (0, -1),
        Direction::Down => (0, 1),
        Direction::Left => (-1, 0),
        Direction::Right => (1, 0),
    }
}
fn adjust(app: &mut App, direction: Direction) {
    if app.workspace.opening_direction == Some(direction) {
        return;
    }
    app.workspace.opening_direction = None;
    let delta = if matches!(direction, Direction::Left | Direction::Down) {
        -1
    } else {
        1
    };
    let field_delta = field_delta(app, &focused_id(app), direction, delta);
    match app.workspace.panel {
        Some(Panel::FxCommands) => navigate(app, direction),
        Some(Panel::Midi | Panel::MidiBinding) => midi::adjust(app, &focused_id(app), field_delta),
        Some(Panel::PerformanceEdit) => {
            performance_edit::adjust(app, &focused_id(app), field_delta)
        }
        Some(Panel::Generator) => generator::adjust(app, &focused_id(app), field_delta),
        Some(Panel::Effects) => effects::adjust(app, &focused_id(app), field_delta),
        Some(Panel::Notes | Panel::NoteValues | Panel::NoteSelection) => {
            notes::adjust(app, &focused_id(app), field_delta)
        }
        Some(Panel::Expression) => {
            notes::expression_action(app, &focused_id(app), field_delta);
        }
        Some(Panel::Arrangement) => arrangement::adjust(app, &focused_id(app), field_delta),
        Some(Panel::Settings) => settings::adjust(app, &focused_id(app), field_delta),
        Some(Panel::SongSettings) => {
            let focus = focused_id(app);
            if focus == "tempo" {
                let bpm = (app.editor.song().tempo.bpm() + f64::from(field_delta)).max(1.0);
                if let Ok(tempo) = Tempo::new((bpm * 1000.0) as u32) {
                    app.requests.push(Request::Tempo { tempo });
                }
            }
        }
        Some(Panel::PhraseSettings) => phrase::adjust(app, &focused_id(app), field_delta),
        Some(Panel::ChordEditor) => {
            let focus = focused_id(app);
            if matches!(direction, Direction::Up | Direction::Down) {
                if let Some(interval) = focus
                    .strip_prefix("chord_key:")
                    .and_then(|v| v.parse::<i8>().ok())
                {
                    let old_octave = app.workspace.chord_octave;
                    chord_editor::adjust(app, "chord_octave", delta);
                    app.workspace.focus = Some(format!(
                        "chord_key:{}",
                        interval + (app.workspace.chord_octave - old_octave) * 12
                    ));
                    return;
                }
            }
            chord_editor::adjust(app, &focus, field_delta);
        }
        Some(Panel::Text) => commands::adjust_text(app, &focused_id(app), delta),
        Some(Panel::Chords) if matches!(direction, Direction::Left | Direction::Right) => {
            let choices: Vec<_> = chord_rows(app)
                .into_iter()
                .filter(|row| row.id.starts_with("chord:"))
                .collect();
            let current = focused_id(app);
            let index = choices
                .iter()
                .position(|row| row.id == current)
                .map_or(0, |index| index.saturating_add_signed(delta as isize))
                .min(choices.len().saturating_sub(1));
            if let Some(choice) = choices.get(index) {
                app.workspace.focus = Some(choice.id.clone());
            }
        }
        Some(_) => {}
        None if app.view == View::Minimap => minimap::adjust(app, direction),
        None if generator::active(app) => generator::adjust(app, &focused_id(app), field_delta),
        None if app.is_piano_roll() && direction == Direction::Up => {
            open(app, Some(Panel::Notes));
            app.workspace.opening_direction = Some(direction);
        }
        None if app.is_piano_roll() => {
            let control = if matches!(direction, Direction::Left | Direction::Right) {
                PianoControl::Resize { delta }
            } else {
                PianoControl::Velocity { delta: delta * 10 }
            };
            app.requests.push(Request::PianoGesture {
                control,
                gesture: app.workspace.gesture.max(1),
            });
        }
        None if app.is_drum() => app.requests.push(Request::AdjustGesture {
            delta: if matches!(direction, Direction::Up | Direction::Down) {
                delta * 10
            } else {
                delta
            },
            gesture: app.workspace.gesture.max(1),
        }),
        None if direction == Direction::Up && app.view == View::Phrase && app.column >= 3 => {
            let held = app.edit_held;
            effects::begin(app);
            app.edit_held = held;
            app.workspace.opening_direction = Some(direction);
        }
        None if app.view == View::Phrase
            && app.column == 0
            && matches!(direction, Direction::Up | Direction::Down) =>
        {
            app.requests.push(Request::AdjustOctave {
                delta,
                gesture: Some(app.workspace.gesture.max(1)),
            });
        }
        None => app.requests.push(Request::AdjustGesture {
            delta: if (app.view == View::Song
                || (app.view == View::Chain && app.column == 1)
                || (app.view == View::Phrase && app.column == 2))
                && matches!(direction, Direction::Up | Direction::Down)
            {
                delta * 10
            } else {
                delta
            },
            gesture: app.workspace.gesture.max(1),
        }),
    }
}
fn synth(app: &mut App, page: SynthPage) {
    if app.view != View::Instruments {
        app.workspace.synth_return = Some((app.view, app.row, app.column, page));
    }
    open(app, None);
    app.requests.push(Request::SynthPage { page });
}
fn focused_id(app: &App) -> String {
    project(app)
        .workspace
        .map_or(String::new(), |view| view.focus)
}
fn activate_focused(app: &mut App) {
    let id = focused_id(app);
    if id.starts_with("piano:") {
        app.requests.push(Request::PianoControl {
            control: PianoControl::Toggle,
        });
    } else if id.starts_with("cell:") || id.starts_with("map:") {
        app.requests.push(Request::Confirm);
    } else {
        activate(app, &id);
    }
}

pub fn event(app: &mut App, event: WorkspaceEvent) {
    if let WorkspaceEvent::TextChanged { id, value } = event {
        if let Some(entry) = &mut app.workspace.text {
            if entry.id == id {
                entry.value = value;
            }
        }
        return;
    }
    if let WorkspaceEvent::Submit { id, value } = event {
        submit(app, &id, value);
        return;
    }
    let (id, activate_row) = match event {
        WorkspaceEvent::Focus { id } => (id, false),
        WorkspaceEvent::Activate { id } => (id, true),
        WorkspaceEvent::Submit { .. } | WorkspaceEvent::TextChanged { .. } => unreachable!(),
    };
    if id == "transport" && activate_row {
        app.requests.push(
            if matches!(
                app.workspace.panel,
                Some(Panel::Performance | Panel::PerformanceEdit)
            ) {
                performance::transport(app)
            } else {
                Request::Play
            },
        );
        return;
    }
    if id == "instruments" {
        synth(app, SynthPage::Instruments);
        return;
    }
    if id == "menu" {
        open(app, Some(Panel::Menu));
        return;
    }
    if id == "home" {
        activate(app, "home");
        return;
    }
    let Some(view) = project(app).workspace else {
        return;
    };
    if let Some(preview) = &view.preview {
        if preview.keys.iter().any(|key| key.id == id) {
            app.workspace.focus = Some(id.clone());
            if activate_row {
                activate(app, &id);
            }
            return;
        }
    }
    if let Some(timeline) = &view.timeline {
        if timeline.spans.iter().any(|span| span.id == id) {
            if app.workspace.panel == Some(Panel::PerformanceEdit) {
                performance_edit::select(app, &id);
                return;
            }
            if let (Some(event), Some(occurrence)) = (
                id.strip_prefix("note:")
                    .and_then(|id| id.parse().ok())
                    .and_then(|id| crest_tracker::composition::EventId::new(id).ok()),
                app.occurrence(),
            ) {
                app.requests.push(Request::SelectEvents {
                    revision: app.editor.revision(),
                    occurrence,
                    events: vec![event],
                    toggle: false,
                });
            }
            return;
        }
    }
    if !view
        .rows
        .iter()
        .chain(&view.inspector)
        .chain(&view.tabs)
        .any(|row| row.id == id)
    {
        if let Some(cell) = view
            .grid
            .iter()
            .flat_map(|row| &row.cells)
            .find(|cell| cell.id == id)
        {
            select_cell(app, &cell.id);
            if activate_row && (cell.id.starts_with("piano:") || app.is_drum()) {
                app.requests.push(Request::Confirm);
            }
        }
        return;
    }
    // Sidebar actions operate on the workspace's current target; clicking one
    // must not replace the row identity that Edit/Open needs to activate.
    if view.tabs.iter().any(|row| row.id == id) {
        settings::select(app, &id);
        return;
    }
    if view.rows.iter().any(|row| row.id == id) {
        app.workspace.focus = Some(id.clone());
    }
    if activate_row {
        activate(app, &id);
    }
}

fn select_cell(app: &mut App, id: &str) {
    if id.starts_with("map:") {
        minimap::select(app, id);
        return;
    }
    let parts: Vec<_> = id.split(':').collect();
    if parts.len() != 3 {
        return;
    }
    let (Ok(row), Ok(column)) = (parts[1].parse::<usize>(), parts[2].parse::<usize>()) else {
        return;
    };
    if parts[0] == "cell" {
        app.requests.push(Request::Focus { row, column });
    } else if parts[0] == "piano" && app.selected_phrase().is_some() {
        if let Ok(at) = app.roll_grid.checked_scale(row as u64, 1) {
            app.requests.push(Request::PianoControl {
                control: PianoControl::Cursor {
                    at,
                    pitch: column as u8,
                },
            });
        }
    }
}

/// Called with the current Synth surface at dispatch time, never a cached view.
/// Local Detail/Choice transitions remain owned by the native Synth reducer.
pub(crate) fn return_from_synth(
    app: &mut App,
    action: &SemanticAction,
    surface: crest_synth::control::SurfaceId,
) -> bool {
    use crest_synth::control::SurfaceId;
    let Some((view, row, column, page)) = app.workspace.synth_return else {
        return false;
    };
    let returns = matches!(
        (page, surface, action),
        (
            SynthPage::Mixer,
            SurfaceId::MixerMain | SurfaceId::MixerInspector,
            SemanticAction::NavigatePage(Direction::Up)
        ) | (
            _,
            SurfaceId::PatchMain,
            SemanticAction::NavigatePage(Direction::Left)
        )
    );
    if !returns {
        return false;
    }
    app.workspace.synth_return = None;
    open(app, None);
    app.view = view;
    app.row = row;
    app.column = column;
    true
}

pub(crate) fn chord_saved(app: &mut App) {
    if app.workspace.panel == Some(Panel::ChordEditor) {
        back(app);
    }
}

/// Vertical numeric editing follows the shared coarse/fine contract. Choices
/// remain adjacent, and pitches move by an octave rather than ten semitones.
fn field_delta(app: &App, id: &str, direction: Direction, delta: i32) -> i32 {
    if !matches!(direction, Direction::Up | Direction::Down) {
        return delta;
    }
    if matches!(
        id,
        "gen_pitch" | "gen_low" | "gen_high" | "notes_pitch" | "chord_root" | "performance_pitch"
    ) || id.starts_with("gen_note:")
        || id.starts_with("chord_interval:")
    {
        return delta * 12;
    }
    let numeric = matches!(
        id,
        "tempo"
            | "default_tempo"
            | "clock_delay"
            | "phrase_steps"
            | "tuplet_notes"
            | "tuplet_time"
            | "gen_pulses"
            | "gen_rotation"
            | "gen_seed"
            | "gen_density"
            | "gen_velocity"
            | "gen_velocity_low"
            | "gen_velocity_high"
            | "gen_gate"
            | "gen_gate_high"
            | "binding_channel"
            | "binding_number"
            | "notes_start"
            | "notes_velocity"
            | "notes_release"
            | "notes_length"
            | "notes_channel"
            | "expr_offset"
            | "expr_value"
            | "expr_number"
            | "arr_chain_repeat"
            | "arr_phrase_repeat"
    );
    let fx_numeric = id.starts_with("fx_") && effects::numeric_field(app, id);
    delta * if numeric || fx_numeric { 10 } else { 1 }
}
