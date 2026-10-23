//! Tracker interaction reducer; physical input and rendering remain Synth-owned.
mod arrangement;
mod channel_expression;
mod chord_editor;
pub(crate) mod chords;
mod commands;
mod effects;
mod expression;
mod files;
mod generator;
mod hints;
pub(crate) mod input;
mod library;
mod midi;
mod minimap;
mod notes;
mod phrase;
mod piano;
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
    Settings,
    SongSettings,
    ChainSettings,
    PhraseSettings,
    File,
    Tools,
    Arrangement,
    SongSlot,
    Chords,
    ChordEditor,
    Generator,
    Effects,
    FxCommands,
    Notes,
    NoteValues,
    NoteSelection,
    NoteHistory,
    PianoView,
    PianoTiming,
    PianoDefaults,
    PianoTarget,
    PianoQuantize,
    PianoVelocity,
    PianoStack,
    Expression,
    ChannelExpression,
    Library,
    ChordLibrary,
    Repair,
    Midi,
    MidiBinding,
    Text,
}

#[derive(Default)]
pub struct Navigation {
    pub(crate) patch_capacity: Option<crest_synth::shell::hosted_synth::HostedPatchCapacity>,
    pub(crate) synth_detail_available: bool,
    pub(crate) synth_surface: Option<crest_synth::control::SurfaceId>,
    controller_bindings: crest_synth::control::ControllerBindings,
    panel: Option<Panel>,
    focus: Option<String>,
    list_page_size: Option<(String, usize)>,
    text: Option<WorkspaceTextEntry>,
    gesture: u64,
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
    channel_expression: channel_expression::Editor,
    midi_binding: Option<crate::preferences::MidiControlBinding>,
    midi_binding_index: Option<usize>,
    settings_tab: settings::Tab,
    piano: piano::PianoEditor,
}

fn row(id: impl Into<String>, label: impl Into<String>, value: impl Into<String>) -> WorkspaceRow {
    WorkspaceRow {
        id: id.into(),
        label: label.into(),
        value: value.into(),
        enabled: true,
        span: 1,
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
        app.workspace.piano.draft = None;
        app.workspace.piano.defaults = None;
        app.workspace.parents.clear();
    }
    app.workspace.panel = panel;
    app.workspace.focus = None;
    app.workspace.text = None;
    cancel_edit(app);
}
fn back(app: &mut App) {
    if app.is_piano_roll()
        && matches!(
            app.workspace.panel,
            Some(
                Panel::NoteValues
                    | Panel::PianoQuantize
                    | Panel::PianoDefaults
                    | Panel::PianoVelocity
            )
        )
    {
        app.workspace.piano.draft = None;
        open(app, None);
        return;
    }
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
    app.load_progress.is_some()
        || app.view != View::Instruments
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
    if app.workspace.panel.is_none() {
        app.follow_piano_playback();
    }
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
    if app.load_progress.is_some() {
        return true;
    }
    match action {
        SemanticAction::PreviewStart => {
            app.requests.push(Request::Play);
            return true;
        }
        SemanticAction::PreviewStop => return true,
        _ => {}
    }
    if !owns_input(app) && action == SemanticAction::Return {
        app.requests.push(Request::SynthAction { action });
        return true;
    }
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
                app.requests.push(Request::EndGesture);
            }
        }
        SemanticAction::Navigate(direction) => navigate(app, direction),
        SemanticAction::NavigatePage(direction) => navigate_page(app, direction),
        SemanticAction::Adjust(direction) => adjust(app, direction),
        SemanticAction::Activate => activate_focused(app),
        SemanticAction::OpenPopover => popover(app),
        SemanticAction::ToggleTestMidi => open(app, Some(Panel::Menu)),
        SemanticAction::OpenMidiSettings => unreachable!(),
        SemanticAction::SelectContext(TopLevelContext::Patch) => synth(app, SynthPage::Instruments),
        SemanticAction::SelectContext(TopLevelContext::Mixer) => synth(app, SynthPage::Mixer),
        SemanticAction::Send(_) => synth(app, SynthPage::Sends),
        SemanticAction::SelectPatch(Direction::Left) => {
            return_to_parent(app);
        }
        SemanticAction::SelectPatch(Direction::Right)
            if !(app.is_piano_roll() && piano::panel(app.workspace.panel)) =>
        {
            app.requests.push(Request::Clear);
        }
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
    if app.workspace.panel == Some(Panel::Menu) && direction == Direction::Down {
        return;
    }
    if app.workspace.panel == Some(Panel::Menu) && direction == Direction::Up {
        back(app);
        return;
    }
    if app.workspace.panel.is_some() && direction == Direction::Down {
        back(app);
        return;
    }
    if direction == Direction::Down {
        open(app, Some(Panel::Menu));
        return;
    }
    match (app.workspace.panel, app.view, direction) {
        (None, View::Song, Direction::Up) => open(app, Some(Panel::SongSettings)),
        (None, View::Chain, Direction::Up) => open(app, Some(Panel::ChainSettings)),
        (None, View::Phrase, Direction::Up) => open(app, Some(Panel::PhraseSettings)),
        (None, View::Song | View::Chain, Direction::Right) => app.requests.push(Request::Confirm),
        (_, _, Direction::Left) => {
            return_to_parent(app);
        }
        _ => {}
    }
}

fn navigate(app: &mut App, direction: Direction) {
    let modal = modal_list(app);
    if !modal && app.workspace.panel == Some(Panel::PianoVelocity) {
        if matches!(direction, Direction::Up | Direction::Down) {
            open(app, None);
        } else {
            app.requests.push(Request::PianoControl {
                control: PianoControl::NextNote {
                    delta: if direction == Direction::Left { -1 } else { 1 },
                },
            });
        }
        return;
    }
    let view = project(app).workspace.expect("Tracker owns input");
    if !modal && app.is_piano_roll() && piano::panel(app.workspace.panel) {
        piano::navigate(app, direction, &view);
        return;
    }
    if !modal && app.workspace.panel == Some(Panel::ChordEditor) {
        chord_editor::navigate(app, direction, &view);
        return;
    }
    if !modal
        && app.workspace.panel == Some(Panel::Settings)
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
        // Native layout supplies the page capacity. Headless input and the
        // first frame use ten rows until the current list has been measured.
        let page = app
            .workspace
            .list_page_size
            .as_ref()
            .filter(|(id, _)| id == &view.rows[0].id)
            .map_or(10, |(_, rows)| *rows)
            .min(view.rows.len()) as isize;
        let delta = match direction {
            Direction::Up => -1,
            Direction::Down => 1,
            Direction::Left => -page,
            Direction::Right => page,
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

fn modal_list(app: &App) -> bool {
    !app.file_dialog.is_null()
        || !app.shared_edit_projection().is_null()
        || !app.resource_repair.is_null()
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
    if app.workspace.panel == Some(Panel::PianoVelocity) {
        let coarse = matches!(direction, Direction::Up | Direction::Down) && !app.fine_held;
        piano::adjust(app, "piano_lane_value", delta * if coarse { 10 } else { 1 });
        return;
    }
    let Some(view) = project(app).workspace else {
        return;
    };
    let focus = view.focus;
    if view.rows.iter().any(|row| row.id == focus && !row.enabled) {
        return;
    }
    let field_delta = field_delta(app, &focus, direction, delta);
    if app.is_piano_roll() && focus.starts_with("piano_") {
        piano::adjust(app, &focus, field_delta);
        return;
    }
    match app.workspace.panel {
        Some(Panel::FxCommands) => navigate(app, direction),
        Some(Panel::Midi | Panel::MidiBinding) => midi::adjust(app, &focus, field_delta),
        Some(Panel::Generator) => generator::adjust(app, &focus, field_delta),
        Some(Panel::Effects) => effects::adjust(app, &focus, field_delta),
        Some(Panel::Notes | Panel::NoteValues | Panel::NoteSelection | Panel::PianoView) => {
            notes::adjust(app, &focus, field_delta)
        }
        Some(Panel::Expression) => {
            expression::adjust(app, &focus, field_delta);
        }
        Some(Panel::ChannelExpression) => channel_expression::adjust(app, &focus, field_delta),
        Some(Panel::Arrangement) => arrangement::adjust(app, &focus, field_delta),
        Some(Panel::Settings) => settings::adjust(app, &focus, field_delta),
        Some(Panel::SongSettings) => {
            if focus == "patch_capacity" {
                settings::adjust(app, &focus, field_delta);
            } else if focus == "tempo" {
                let bpm = (app.editor.song().tempo.bpm() + f64::from(field_delta)).max(1.0);
                if let Ok(tempo) = Tempo::new((bpm * 1000.0) as u32) {
                    app.requests.push(Request::Tempo { tempo });
                }
            }
        }
        Some(Panel::PhraseSettings) => phrase::adjust(app, &focus, field_delta),
        Some(Panel::ChordEditor) => {
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
        Some(Panel::Text) => commands::adjust_text(app, &focus, delta),
        Some(Panel::Chords) if matches!(direction, Direction::Left | Direction::Right) => {
            let choices: Vec<_> = chord_rows(app)
                .into_iter()
                .filter(|row| row.id.starts_with("chord:"))
                .collect();
            let current = &focus;
            let index = choices
                .iter()
                .position(|row| &row.id == current)
                .map_or(0, |index| index.saturating_add_signed(delta as isize))
                .min(choices.len().saturating_sub(1));
            if let Some(choice) = choices.get(index) {
                app.workspace.focus = Some(choice.id.clone());
            }
        }
        Some(_) => {}
        None if app.view == View::Minimap => minimap::adjust(app, direction),
        None if generator::active(app) => generator::adjust(app, &focus, field_delta),
        None if app.is_piano_roll() => {
            let control = if matches!(direction, Direction::Left | Direction::Right) {
                PianoControl::Nudge { delta }
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
                || (app.view == View::Phrase && app.column >= 2))
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
fn popover(app: &mut App) {
    match app.workspace.panel {
        Some(Panel::SongSlot) => open(app, None),
        Some(Panel::Effects) => {
            let focus = focused_id(app);
            effects::activate(
                app,
                if focus == "fx_value"
                    && matches!(
                        app.workspace.fx,
                        Some(
                            crest_tracker::composition::Fx::ChordRevision { .. }
                                | crest_tracker::composition::Fx::Chord { .. }
                                | crest_tracker::composition::Fx::Add { .. }
                        )
                    )
                {
                    "fx_value"
                } else {
                    "fx_kind"
                },
            );
        }
        Some(_) => {}
        None if app.view == View::Song => open(app, Some(Panel::SongSlot)),
        None if app.view == View::Chain => open(app, Some(Panel::Arrangement)),
        None if app.is_piano_roll() => {
            app.requests.push(Request::PianoControl {
                control: PianoControl::Select,
            });
            open(app, Some(Panel::Notes));
        }
        None if app.view == View::Phrase && app.column >= 3 => effects::begin(app),
        None if app.view == View::Phrase => open(app, Some(Panel::Notes)),
        _ => {}
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
    if app.workspace.panel == Some(Panel::PianoVelocity) {
        piano::activate(app, "piano_inspect");
        app.workspace.focus = Some("piano_velocity_value".into());
        return;
    }
    let id = focused_id(app);
    if id.starts_with("piano:") || id.starts_with("cell:") || id.starts_with("map:") {
        app.requests.push(Request::Confirm);
    } else if project(app)
        .workspace
        .is_some_and(|view| view.rows.iter().any(|row| row.id == id && row.enabled))
    {
        activate(app, &id);
    }
}

pub fn event(app: &mut App, event: WorkspaceEvent) {
    if app.load_progress.is_some() {
        return;
    }
    if let WorkspaceEvent::ListPageSize { id, rows } = event {
        if let Some(view) = project(app).workspace {
            if rows > 0
                && rows <= view.rows.len()
                && view.rows.first().is_some_and(|row| row.id == id)
            {
                app.workspace.list_page_size = Some((id, rows));
            }
        }
        return;
    }
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
        WorkspaceEvent::Submit { .. }
        | WorkspaceEvent::TextChanged { .. }
        | WorkspaceEvent::ListPageSize { .. } => unreachable!(),
    };
    if id == "transport" && activate_row {
        app.requests.push(Request::Play);
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
    if view
        .rows
        .iter()
        .chain(&view.inspector)
        .any(|row| row.id == id && !row.enabled)
    {
        return;
    }
    if !view.overlay {
        if let Some(timeline) = &view.timeline {
            if id == timeline.level_action.id {
                if activate_row {
                    piano::activate(app, &id);
                }
                return;
            }
            if let Some(span) = timeline
                .spans
                .iter()
                .find(|span| span.id == id || span.level_id == id)
            {
                if span.level_id == id {
                    open(app, Some(Panel::PianoVelocity));
                }
                if let (Some(event), Some(occurrence)) = (
                    span.id
                        .strip_prefix("note:")
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
    }
    if !view
        .rows
        .iter()
        .chain(&view.inspector)
        .chain(&view.tabs)
        .any(|row| row.id == id)
    {
        if view.overlay {
            return;
        }
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
        if let Ok(at) = app.piano_paint_step().checked_scale(row as u64, 1) {
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
    let returns = matches!(
        (surface, action),
        (
            SurfaceId::PatchMain | SurfaceId::MixerMain | SurfaceId::Sends,
            SemanticAction::NavigatePage(Direction::Down | Direction::Left)
                | SemanticAction::Return
        )
    );
    if !returns {
        return false;
    }
    open(app, Some(Panel::Menu));
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
        "gen_pitch" | "gen_low" | "gen_high" | "notes_pitch" | "piano_pitch" | "chord_root"
    ) || id.starts_with("gen_note:")
        || id.starts_with("chord_interval:")
    {
        return delta * 12;
    }
    let numeric = matches!(
        id,
        "tempo"
            | "patch_capacity"
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
            | "piano_start"
            | "piano_velocity_value"
            | "piano_new_velocity"
            | "piano_channel"
            | "piano_release"
            | "piano_lane_value"
            | "notes_velocity"
            | "notes_release"
            | "notes_length"
            | "notes_channel"
            | "expr_offset"
            | "expr_value"
            | "expr_number"
            | "channel_time"
            | "channel_value"
            | "channel_number"
            | "arr_chain_repeat"
            | "arr_phrase_repeat"
    );
    let fx_numeric = id.starts_with("fx_") && effects::numeric_field(app, id);
    delta * if numeric || fx_numeric { 10 } else { 1 }
}
