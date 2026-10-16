//! Tracker interaction reducer; physical input and rendering remain Synth-owned.
mod chords;
mod commands;
mod files;
mod projection;
#[cfg(test)]
mod tests;
use crate::app::{App, CaptureAction, FileAction, PianoControl, Request, SynthPage, View};
use chords::{chord_action, chord_rows};
use commands::{activate, change_phrase, submit, text_entry};
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
    SongSettings,
    PhraseSettings,
    File,
    Tools,
    Chords,
    Text,
}

#[derive(Default)]
pub struct Navigation {
    panel: Option<Panel>,
    focus: Option<String>,
    text: Option<WorkspaceTextEntry>,
    gesture: u64,
}

fn row(id: impl Into<String>, label: impl Into<String>, value: impl Into<String>) -> WorkspaceRow {
    WorkspaceRow {
        id: id.into(),
        label: label.into(),
        value: value.into(),
    }
}
fn commands(values: &[(&str, &str)]) -> Vec<WorkspaceRow> {
    values
        .iter()
        .map(|(id, label)| row(*id, *label, ""))
        .collect()
}
fn open(app: &mut App, panel: Option<Panel>) {
    app.workspace.panel = panel;
    app.workspace.focus = None;
    app.workspace.text = None;
    cancel_edit(app);
}
pub fn text_entry_active(app: &App) -> bool {
    app.workspace.text.is_some()
}

pub fn owns_input(app: &App) -> bool {
    app.view != View::Instruments || app.workspace.panel.is_some() || !app.file_dialog.is_null()
}
pub fn cancel_edit(app: &mut App) {
    app.edit_held = false;
    app.requests.push(Request::EndGesture);
}

/// Both keyboard and SDL call this same semantic boundary. Native Synth actions
/// fall through while its workspace is active, preserving all original pages.
pub fn action(app: &mut App, action: SemanticAction) -> bool {
    if action == SemanticAction::Return {
        if app.workspace.panel.is_some() {
            open(app, None);
        } else if matches!(app.view, View::Song | View::Instruments) {
            open(app, Some(Panel::Menu));
        } else {
            app.requests.push(Request::Back);
        }
        return true;
    }
    if app.view == View::Instruments && app.workspace.panel.is_none() && app.file_dialog.is_null() {
        return false;
    }
    match action {
        SemanticAction::SetInteractionMode(mode) => {
            if mode == InteractionMode::Adjust && !app.edit_held {
                app.workspace.gesture = app.workspace.gesture.checked_add(1).unwrap_or(1);
            }
            app.edit_held = mode == InteractionMode::Adjust;
            if !app.edit_held {
                app.requests.push(Request::EndGesture);
            }
        }
        SemanticAction::Navigate(direction) => navigate(app, direction),
        SemanticAction::NavigatePage(direction) => navigate_page(app, direction),
        SemanticAction::Adjust(direction) => adjust(app, direction),
        SemanticAction::Activate => activate_focused(app),
        SemanticAction::PreviewStart => app.requests.push(Request::Play),
        SemanticAction::PreviewStop => {}
        SemanticAction::ToggleTestMidi => open(app, Some(Panel::Menu)),
        SemanticAction::OpenMidiSettings => open(app, Some(Panel::SongSettings)),
        SemanticAction::SelectContext(TopLevelContext::Patch) => synth(app, SynthPage::Instruments),
        SemanticAction::SelectContext(TopLevelContext::Mixer) => synth(app, SynthPage::Mixer),
        SemanticAction::Send(_) => synth(app, SynthPage::Sends),
        SemanticAction::SelectPatch(Direction::Left) => {
            if !app.file_dialog.is_null() {
                file_cancel(app);
            } else if app.workspace.panel.is_some() {
                open(app, None);
            } else {
                app.requests.push(Request::Back);
            }
        }
        SemanticAction::SelectPatch(Direction::Right) => app.requests.push(Request::Clear),
        _ => {}
    }
    true
}

fn navigate_page(app: &mut App, direction: Direction) {
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
        (_, View::Song, Direction::Up) => open(app, Some(Panel::SongSettings)),
        (_, View::Phrase, Direction::Up) => open(app, Some(Panel::PhraseSettings)),
        (None, View::Song | View::Chain, Direction::Right) => app.requests.push(Request::Confirm),
        (_, _, Direction::Left) => {
            open(app, None);
            app.requests.push(Request::Back);
        }
        _ => {}
    }
}

fn navigate(app: &mut App, direction: Direction) {
    let view = project(app).workspace.expect("Tracker owns input");
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
    let delta = if matches!(direction, Direction::Left | Direction::Down) {
        -1
    } else {
        1
    };
    match app.workspace.panel {
        Some(Panel::SongSettings) => {
            let focus = focused_id(app);
            if focus == "tempo" {
                let bpm = (app.editor.song().tempo.bpm() + f64::from(delta)).max(1.0);
                if let Ok(tempo) = Tempo::new((bpm * 1000.0) as u32) {
                    app.requests.push(Request::Tempo { tempo });
                }
            }
        }
        Some(Panel::PhraseSettings) => change_phrase(app, &focused_id(app), delta),
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
        None if app.is_piano_roll() => {
            let control = if matches!(direction, Direction::Left | Direction::Right) {
                PianoControl::Nudge { delta }
            } else {
                PianoControl::Adjust { delta }
            };
            app.requests.push(Request::PianoGesture {
                control,
                gesture: app.workspace.gesture.max(1),
            });
        }
        None if app.is_drum() => app.requests.push(Request::AdjustGesture {
            delta,
            gesture: app.workspace.gesture.max(1),
        }),
        None if direction == Direction::Up && app.view == View::Phrase && app.column >= 3 => {
            let held = app.edit_held;
            open(app, Some(Panel::Chords));
            app.edit_held = held;
        }
        None if app.view == View::Phrase
            && app.column == 0
            && matches!(direction, Direction::Left | Direction::Right) =>
        {
            app.requests.push(Request::AdjustOctave {
                delta,
                gesture: Some(app.workspace.gesture.max(1)),
            });
        }
        None => app.requests.push(Request::AdjustGesture {
            delta,
            gesture: app.workspace.gesture.max(1),
        }),
    }
}
fn synth(app: &mut App, page: SynthPage) {
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
    if id.starts_with("cell:") || id.starts_with("piano:") {
        app.requests.push(Request::Confirm);
    } else {
        activate(app, &id);
    }
}

pub fn event(app: &mut App, event: WorkspaceEvent) {
    if let WorkspaceEvent::Submit { id, value } = event {
        submit(app, &id, value);
        return;
    }
    let (id, activate_row) = match event {
        WorkspaceEvent::Focus { id } => (id, false),
        WorkspaceEvent::Activate { id } => (id, true),
        WorkspaceEvent::Submit { .. } => unreachable!(),
    };
    if id == "menu" {
        open(app, Some(Panel::Menu));
        return;
    }
    let Some(view) = project(app).workspace else {
        return;
    };
    if !view
        .rows
        .iter()
        .chain(&view.inspector)
        .any(|row| row.id == id)
    {
        if let Some(cell) = view
            .grid
            .iter()
            .flat_map(|row| &row.cells)
            .find(|cell| cell.id == id)
        {
            select_cell(app, &cell.id);
        }
        return;
    }
    app.workspace.focus = Some(id.clone());
    if activate_row {
        activate(app, &id);
    }
}

fn select_cell(app: &mut App, id: &str) {
    let parts: Vec<_> = id.split(':').collect();
    if parts.len() != 3 {
        return;
    }
    let (Ok(row), Ok(column)) = (parts[1].parse::<usize>(), parts[2].parse::<usize>()) else {
        return;
    };
    if parts[0] == "cell" {
        app.requests.push(Request::Focus { row, column });
    } else if parts[0] == "piano" {
        if let Some((_, phrase)) = app.selected_phrase() {
            if let Ok(at) = phrase.division.beat_length().checked_scale(row as u64, 1) {
                app.requests.push(Request::PianoControl {
                    control: PianoControl::Cursor {
                        at,
                        pitch: column as u8,
                    },
                });
            }
        }
    }
}
