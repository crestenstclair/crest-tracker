//! Selection, note values and owned expression use the same atomic edit path.
use super::*;
use crate::app::SelectionAction;
use crest_tracker::composition::Control;

fn all_rows(app: &App) -> Vec<WorkspaceRow> {
    let mut rows = vec![
        row(
            "notes_all",
            "Select all events",
            app.selected_event_ids().len().to_string(),
        ),
        row(
            "notes_range",
            "Range selection",
            if app.range_held { "On" } else { "Off" },
        ),
        row(
            "notes_grid",
            "Edit / quantize grid (beats)",
            generator::beat_label(app.roll_grid),
        ),
    ];
    if let Some(event) = effects::selected_event(app) {
        if let Some(note) = event.note {
            rows.extend([
                row("notes_pitch", "Pitch", note.pitch.to_string()),
                row(
                    "notes_velocity",
                    "Attack velocity",
                    note.velocity.to_string(),
                ),
                row(
                    "notes_release",
                    "Release velocity",
                    note.release_velocity.to_string(),
                ),
                row(
                    "notes_length",
                    "Duration (beats)",
                    generator::beat_label(note.duration),
                ),
                row("notes_target", "Target", app.target_label(&event.target)),
                row(
                    "notes_channel",
                    "MIDI channel",
                    event.target.channel().get().to_string(),
                ),
            ]);
        }
    }
    rows.extend(commands(&[
        ("notes_off", "Note OFF at cursor"),
        ("notes_resize", "Resize selected notes (+ / − grid)"),
        ("notes_nudge", "Move selected notes (+ / − grid)"),
        ("notes_quantize", "Quantize starts"),
        ("notes_quantize_lengths", "Quantize starts and durations"),
        ("notes_copy", "Copy"),
        ("notes_cut", "Cut"),
        ("notes_paste", "Paste at cursor"),
        ("notes_duplicate", "Duplicate at cursor"),
        ("notes_delete", "Delete selection"),
        ("expression", "Expression points"),
        ("channel_expression", "Channel automation"),
        ("back", "Back"),
    ]));
    rows
}
pub(super) fn rows(app: &App) -> Vec<WorkspaceRow> {
    let event = effects::selected_event(app);
    let timing = event
        .as_ref()
        .and_then(|event| {
            event.note.as_ref().map(|note| {
                format!(
                    "{} · {} beats · {} beats",
                    pitch_name(note.pitch),
                    generator::beat_label(event.at),
                    generator::beat_label(note.duration)
                )
            })
        })
        .unwrap_or("Select or insert a note".into());
    let dynamics = event
        .as_ref()
        .and_then(|event| {
            event.note.as_ref().map(|note| {
                format!(
                    "{} / {} · {}",
                    note.velocity,
                    note.release_velocity,
                    app.target_label(&event.target)
                )
            })
        })
        .unwrap_or_default();
    let mut rows = vec![
        row("notes_values", "Pitch / start / length", timing),
        row("notes_dynamics", "Velocity / release / target", dynamics),
        row(
            "notes_quantize",
            "Quantize selected",
            format!(
                "{} beat grid · starts only",
                generator::beat_label(app.roll_grid)
            ),
        ),
        row(
            "expression",
            "Expression",
            "Bend · pressure · timbre · sustain",
        ),
        row(
            "channel_expression",
            "Channel automation",
            "Independent phrase-time controls",
        ),
        row(
            "notes_selection",
            "Selection / clipboard",
            "Range · all · copy · paste · duplicate · delete",
        ),
        row("notes_history", "Undo / redo", "Undo last edit · redo"),
        row(
            "notes_view",
            "Grid / zoom / scroll",
            "Entry grid · visible beats · follow playback",
        ),
        row("back", "Back", "Piano Roll / Phrase"),
    ];
    if !app.is_piano_roll() {
        rows.retain(|row| row.id != "notes_view");
    }
    rows
}

pub(super) fn view_rows(app: &App) -> Vec<WorkspaceRow> {
    let mut rows = vec![
        row(
            "notes_zoom",
            "Visible beats",
            format!(
                "{} · from beat {}",
                generator::beat_label(app.roll_view.width),
                generator::beat_label(app.roll_view.start)
            ),
        ),
        row(
            "notes_follow",
            "Follow playback",
            if app.roll_view.follow { "On" } else { "Off" },
        ),
        row(
            "piano_scale_guide",
            "Scale guide",
            if app.roll_view.scale_guide {
                "On"
            } else {
                "Off"
            },
        ),
        row(
            "notes_stick_value",
            "Right stick vertical",
            match app.roll_value {
                crate::app::PianoValue::Pitch => "Pitch",
                crate::app::PianoValue::Velocity => "Velocity",
                crate::app::PianoValue::ReleaseVelocity => "Release velocity",
            },
        ),
        row("notes_previous_page", "Previous beats", "Half window"),
        row("notes_next_page", "Next beats", "Half window"),
        row("piano_octave_down", "Octave down", "View only"),
        row("piano_octave_up", "Octave up", "View only"),
        row("back", "Done", ""),
    ];
    if !app.is_piano_roll() {
        rows.retain(|row| !row.id.starts_with("piano_"));
        rows.insert(
            0,
            row(
                "notes_grid",
                "Step grid (beats)",
                generator::beat_label(app.roll_grid),
            ),
        );
    }
    rows
}

fn pitch_name(pitch: u8) -> String {
    crest_tracker::harmony::Pitch::new(pitch)
        .map(|pitch| pitch.name(crest_tracker::harmony::Spelling::Sharps))
        .unwrap_or_default()
}
pub(super) fn value_rows(app: &App) -> Vec<WorkspaceRow> {
    let mut rows: Vec<_> = all_rows(app)
        .into_iter()
        .filter(|row| {
            matches!(
                row.id.as_str(),
                "notes_pitch"
                    | "notes_velocity"
                    | "notes_release"
                    | "notes_length"
                    | "notes_target"
                    | "notes_channel"
                    | "notes_resize"
                    | "notes_nudge"
                    | "back"
            )
        })
        .collect();
    if let Some(event) = effects::selected_event(app) {
        if let Some(row) = rows.iter_mut().find(|row| row.id == "notes_pitch") {
            if let Some(note) = &event.note {
                row.value = pitch_name(note.pitch);
            }
        }
        rows.insert(
            1.min(rows.len()),
            row(
                "notes_start",
                "Start (beats)",
                generator::beat_label(event.at),
            ),
        );
    }
    rows
}
pub(super) fn selection_rows(app: &App) -> Vec<WorkspaceRow> {
    let mut rows: Vec<_> = all_rows(app)
        .into_iter()
        .filter(|row| {
            matches!(
                row.id.as_str(),
                "notes_all"
                    | "notes_range"
                    | "notes_grid"
                    | "notes_quantize"
                    | "notes_quantize_lengths"
                    | "notes_copy"
                    | "notes_cut"
                    | "notes_paste"
                    | "notes_duplicate"
                    | "notes_delete"
                    | "notes_off"
                    | "back"
            )
        })
        .collect();
    if app.is_piano_roll() {
        rows.splice(
            0..0,
            commands(&[
                ("notes_previous", "Previous note"),
                ("notes_next", "Next note"),
                ("notes_insert", "Select / insert at cursor"),
            ]),
        );
    }
    rows
}

fn selection_action(app: &App, id: &str) -> Option<SelectionAction> {
    let at = if app.is_piano_roll() {
        app.piano_cursor().0
    } else {
        app.selected_phrase()?
            .1
            .division
            .beat_length()
            .checked_scale(app.row as u64, 1)
            .ok()?
    };
    Some(match id {
        "notes_copy" => SelectionAction::Copy,
        "notes_cut" => SelectionAction::Cut,
        "notes_paste" => SelectionAction::Paste { at },
        "notes_duplicate" => SelectionAction::Duplicate { at },
        "notes_delete" => SelectionAction::Delete,
        "notes_quantize" | "notes_quantize_lengths" => SelectionAction::Quantize {
            grid: app.roll_grid,
            durations: id == "notes_quantize_lengths",
        },
        _ => return None,
    })
}
pub(super) fn activate(app: &mut App, id: &str) -> bool {
    if app.is_piano_roll() {
        let mapped = match id {
            "notes_values" | "notes_dynamics" => Some("piano_inspect"),
            "notes_quantize" | "notes_quantize_lengths" => Some("piano_quantize"),
            "notes_grid" => Some("piano_grid"),
            "notes_off" => Some("piano_off"),
            _ => None,
        };
        if let Some(mapped) = mapped {
            return piano::activate(app, mapped);
        }
    }
    if !id.starts_with("notes_") {
        return false;
    }
    if app.is_piano_roll() {
        let control = match id {
            "notes_previous" => Some(PianoControl::NextNote { delta: -1 }),
            "notes_next" => Some(PianoControl::NextNote { delta: 1 }),
            "notes_previous_page" => Some(PianoControl::Scroll { delta: -1 }),
            "notes_next_page" => Some(PianoControl::Scroll { delta: 1 }),
            "notes_follow" => Some(PianoControl::Follow {
                enabled: !app.roll_view.follow,
            }),
            _ => None,
        };
        if let Some(control) = control {
            app.requests.push(Request::PianoControl { control });
            return true;
        }
    }
    if id == "notes_view" {
        open(app, Some(Panel::PianoView));
        return true;
    }
    if id == "notes_insert" {
        app.requests.push(Request::Confirm);
        return true;
    }
    if matches!(
        id,
        "notes_values" | "notes_dynamics" | "notes_selection" | "notes_history"
    ) {
        open(
            app,
            Some(match id {
                "notes_selection" => Panel::NoteSelection,
                "notes_history" => Panel::NoteHistory,
                _ => Panel::NoteValues,
            }),
        );
        if id == "notes_dynamics" {
            app.workspace.focus = Some("notes_velocity".into());
        }
    } else if id == "notes_start" {
        if let Some(event) = effects::selected_event(app) {
            text_entry(
                app,
                id,
                "Start in beats, e.g. 1/4",
                &generator::beat_label(event.at),
            );
        }
    } else if id == "notes_off" {
        note_off(app);
    } else if id == "notes_all" {
        if let (Some(occurrence), Some((_, p))) = (app.occurrence(), app.selected_phrase()) {
            app.requests.push(Request::SelectEvents {
                revision: app.editor.revision(),
                occurrence,
                events: p
                    .events
                    .iter()
                    .filter(|e| !app.is_piano_roll() || e.note.is_some())
                    .map(|e| e.id)
                    .collect(),
                toggle: false,
            });
        }
    } else if id == "notes_range" {
        app.range_held = !app.range_held;
        open(app, None);
    } else if let (Some(action), Some(occurrence)) = (selection_action(app, id), app.occurrence()) {
        app.requests.push(Request::SelectionEdit {
            revision: app.editor.revision(),
            occurrence,
            action,
        });
    } else if id == "notes_grid" {
        text_entry(
            app,
            id,
            "Grid in beats, e.g. 1/3",
            &generator::beat_label(app.roll_grid),
        );
    } else if id == "notes_length" {
        if let Some(e) = effects::selected_event(app) {
            if let Some(n) = e.note {
                text_entry(
                    app,
                    id,
                    "Duration in beats, e.g. 1/4",
                    &generator::beat_label(n.duration),
                );
            }
        }
    } else {
        adjust(app, id, 1);
    }
    true
}
pub(super) fn adjust(app: &mut App, id: &str, delta: i32) {
    if id == "notes_zoom" && app.is_piano_roll() {
        app.requests.push(Request::PianoControl {
            control: PianoControl::Zoom { delta },
        });
        return;
    }
    if id == "notes_stick_value" {
        use crate::app::PianoValue::*;
        let values = [Pitch, Velocity, ReleaseVelocity];
        let index = match app.roll_value {
            Pitch => 0_i32,
            Velocity => 1,
            ReleaseVelocity => 2,
        };
        app.requests.push(Request::PianoControl {
            control: PianoControl::Value {
                value: values[(index + delta.signum()).rem_euclid(3) as usize],
            },
        });
        return;
    }
    if id == "notes_grid" {
        let grid = if delta > 0 {
            app.roll_grid.checked_scale(2, 1)
        } else {
            app.roll_grid.checked_scale(1, 2)
        };
        if let Ok(grid) = grid {
            app.requests.push(Request::PianoControl {
                control: PianoControl::Grid { grid },
            });
        }
        return;
    }
    if matches!(id, "notes_resize" | "notes_nudge") {
        if let Some(occurrence) = app.occurrence() {
            let action = if id == "notes_nudge" {
                SelectionAction::Move {
                    amount: app.roll_grid,
                    backward: delta < 0,
                }
            } else {
                let Some(note) = effects::selected_event(app).and_then(|e| e.note) else {
                    return;
                };
                let duration = if delta < 0 {
                    note.duration.checked_sub(app.roll_grid)
                } else {
                    note.duration.checked_add(app.roll_grid)
                };
                let Ok(duration) = duration else {
                    app.status = "Duration must stay positive".into();
                    return;
                };
                SelectionAction::Duration { duration }
            };
            app.requests.push(Request::SelectionEdit {
                revision: app.editor.revision(),
                occurrence,
                action,
            });
        }
        return;
    }
    let Some((phrase, _)) = app.selected_phrase() else {
        return;
    };
    let Some(mut event) = effects::selected_event(app) else {
        return;
    };
    let Some(note) = &mut event.note else {
        return;
    };
    match id {
        "notes_start" => {
            let amount = app.roll_grid;
            event.at = if delta < 0 {
                event.at.checked_sub(amount).unwrap_or(Beat::ZERO)
            } else {
                event.at.checked_add(amount).unwrap_or(event.at)
            };
        }
        "notes_pitch" => {
            let old = note.pitch;
            note.pitch = (old as i32 + delta).clamp(0, 127) as u8;
            for p in &mut note.expression {
                if let Control::PolyPressure { pitch, .. } = &mut p.control {
                    *pitch = note.pitch;
                }
            }
        }
        "notes_velocity" => note.velocity = (note.velocity as i32 + delta).clamp(1, 127) as u8,
        "notes_release" => {
            note.release_velocity = (note.release_velocity as i32 + delta).clamp(0, 127) as u8
        }
        "notes_length" => {
            app.requests.push(Request::Edit {
                revision: app.editor.revision(),
                command: Box::new(Command::ResizeNotes {
                    phrase,
                    events: vec![event.id],
                    duration: generator::step_beat(note.duration, delta),
                }),
            });
            return;
        }
        "notes_target" => event.target = generator::next_target(app, &event.target, delta),
        "notes_channel" => {
            let channel = crest_tracker::composition::Channel::new(
                (event.target.channel().get() as i32 + delta).clamp(1, 16) as u8,
            )
            .unwrap();
            match &mut event.target {
                crest_tracker::composition::Target::Instrument { channel: c, .. }
                | crest_tracker::composition::Target::Channel { channel: c, .. } => *c = channel,
            }
        }
        _ => return,
    }
    app.requests.push(Request::Edit {
        revision: app.editor.revision(),
        command: Box::new(Command::PutEvent { phrase, event }),
    });
}
pub(super) fn parse_beat(value: &str) -> Result<Beat, String> {
    let parts = value
        .split('/')
        .map(str::trim)
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Use a beat fraction, e.g. 1/4".to_owned())?;
    if parts.is_empty() || parts.len() > 2 {
        return Err("Use a beat fraction, e.g. 1/4".into());
    }
    Beat::new(parts[0], *parts.get(1).unwrap_or(&1)).map_err(|e| e.to_string())
}

fn note_off(app: &mut App) {
    use crest_tracker::composition::{Event, EventId};
    let Some((phrase_id, phrase)) = app.selected_phrase() else {
        return;
    };
    let at = if app.is_piano_roll() {
        app.piano_cursor().0
    } else {
        let Ok(at) = phrase
            .division
            .beat_length()
            .checked_scale(app.row as u64, 1)
        else {
            return;
        };
        at
    };
    let previous = phrase
        .events
        .iter()
        .filter(|e| e.at <= at)
        .max_by_key(|e| e.at);
    let target = previous
        .map(|e| e.target.clone())
        .or_else(|| app.default_target().ok());
    let Some(target) = target else {
        app.status = "Choose an instrument first".into();
        return;
    };
    let id = phrase
        .events
        .iter()
        .find(|e| e.at == at && e.target == target)
        .map(|e| e.id)
        .or_else(|| {
            app.editor
                .song()
                .next_identity()
                .ok()
                .and_then(|id| EventId::new(id).ok())
        });
    let Some(id) = id else {
        return;
    };
    app.requests.push(Request::Edit {
        revision: app.editor.revision(),
        command: Box::new(Command::PutEvent {
            phrase: phrase_id,
            event: Event::note_off(id, at, target),
        }),
    });
    open(app, None);
}
