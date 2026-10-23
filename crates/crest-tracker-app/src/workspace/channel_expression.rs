//! Phrase-time MIDI controls stay independent from note-owned expression.
use super::*;
use crest_tracker::composition::{Control, Event, EventId, Expression, Target};
use expression::{raw, Lane};

#[derive(Clone, Copy, PartialEq, Eq)]
struct PointKey {
    event: EventId,
    control_index: usize,
}
#[derive(Clone, Copy)]
struct Point {
    key: PointKey,
    at: Beat,
    control: Control,
}
#[derive(Default)]
pub(super) struct Editor {
    target: Option<Target>,
    selected: Option<PointKey>,
}

pub(super) fn begin(app: &mut App) {
    app.workspace.channel_expression = Editor {
        target: effects::selected_event(app)
            .map(|event| event.target)
            .or_else(|| targets(app).into_iter().next()),
        selected: None,
    };
    open(app, Some(Panel::ChannelExpression));
}

fn targets(app: &App) -> Vec<Target> {
    let mut targets = Vec::new();
    if let Some((_, phrase)) = app.selected_phrase() {
        for event in &phrase.events {
            if !targets.contains(&event.target) {
                targets.push(event.target.clone());
            }
        }
    }
    if let Some(target) = app
        .roll_new_target
        .clone()
        .or_else(|| app.default_target().ok())
    {
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
    targets
}

fn points(app: &App) -> Vec<Point> {
    let Some((_, phrase)) = app.selected_phrase() else {
        return Vec::new();
    };
    let mut points: Vec<_> = phrase
        .events
        .iter()
        .filter(|event| Some(&event.target) == app.workspace.channel_expression.target.as_ref())
        .flat_map(event_points)
        .collect();
    points.sort_by_key(|point| (point.at, point.key.event, point.key.control_index));
    points
}
fn event_points(event: &Event) -> impl Iterator<Item = Point> + '_ {
    event
        .controls
        .iter()
        .enumerate()
        .map(move |(control_index, control)| Point {
            key: PointKey {
                event: event.id,
                control_index,
            },
            at: event.at,
            control: *control,
        })
}
fn selected(app: &App, points: &[Point]) -> Option<Point> {
    points
        .iter()
        .find(|point| Some(point.key) == app.workspace.channel_expression.selected)
        .or_else(|| points.first())
        .copied()
}

pub(super) fn rows(app: &App) -> Vec<WorkspaceRow> {
    let points = points(app);
    let selected = selected(app, &points);
    let lane = selected.map_or(Lane::Pressure, |point| Lane::of(point.control));
    let mut rows = vec![
        row(
            "channel_target",
            "Channel scope",
            app.workspace
                .channel_expression
                .target
                .as_ref()
                .map_or("No destination".into(), |target| app.target_label(target)),
        ),
        row("channel_lane", "Displayed lane", lane.label()),
        row("channel_previous", "Previous point", "Current lane"),
        row("channel_next", "Next point", "Current lane"),
    ];
    if let Some(point) = selected {
        rows.extend([
            row(
                "channel_time",
                "Time · phrase beats",
                generator::beat_label(point.at),
            ),
            row(
                "channel_value",
                "Raw value",
                format!("{} / {}", raw(point.control), lane.maximum()),
            ),
            row("channel_kind", "Control type", lane.label()),
        ]);
        match point.control {
            Control::Cc { number, .. } => {
                rows.push(row("channel_number", "CC number", number.to_string()))
            }
            Control::PolyPressure { pitch, .. } => {
                rows.push(row("channel_number", "Key pitch", pitch.to_string()))
            }
            _ => {}
        }
        rows.push(row(
            "channel_delete",
            "Delete point",
            "Keep notes and other controls",
        ));
    }
    let mut add = row("channel_new", "Add point", "At cursor");
    add.enabled = app.workspace.channel_expression.target.is_some();
    let mut note = row("expression", "Note / master expression", "Separate scope");
    note.enabled = effects::selected_event(app).is_some_and(|event| event.note.is_some());
    rows.extend([
        add,
        note,
        row("undo", "Undo", "Last edit"),
        row("redo", "Redo", "Last undo"),
        row("back", "Done", "Changes kept"),
    ]);
    rows.extend(
        points
            .iter()
            .enumerate()
            .filter(|(_, point)| Lane::of(point.control) == lane)
            .map(|(index, point)| {
                let mut row = row(
                    format!("channel_select:{index}"),
                    format!(
                        "Point {} · {} beats",
                        index + 1,
                        generator::beat_label(point.at)
                    ),
                    raw(point.control).to_string(),
                );
                row.selected = selected.is_some_and(|selected| selected.key == point.key);
                row
            }),
    );
    rows
}

pub(super) fn activate(app: &mut App, id: &str) -> bool {
    if id == "channel_expression" {
        begin(app);
        return true;
    }
    if !id.starts_with("channel_") {
        return false;
    }
    if matches!(id, "channel_time" | "channel_value" | "channel_number") {
        if let Some(point) = selected(app, &points(app)) {
            let (label, value) = match id {
                "channel_time" => (
                    "Time in phrase beats, e.g. 1/3",
                    generator::beat_label(point.at),
                ),
                "channel_value" => (
                    "Raw value (bend 0–16383; others 0–127)",
                    raw(point.control).to_string(),
                ),
                _ => (
                    "CC number / key pitch (0–127)",
                    match point.control {
                        Control::Cc { number, .. } => number.to_string(),
                        Control::PolyPressure { pitch, .. } => pitch.to_string(),
                        _ => return true,
                    },
                ),
            };
            app.workspace.channel_expression.selected = Some(point.key);
            text_entry(app, id, label, &value);
        }
    } else {
        adjust(app, id, 1);
    }
    true
}

pub(super) fn adjust(app: &mut App, id: &str, delta: i32) {
    if let Err(error) = adjust_value(app, id, delta) {
        app.status = error;
    }
}
fn adjust_value(app: &mut App, id: &str, delta: i32) -> Result<(), String> {
    let points = points(app);
    if let Some(index) = id
        .strip_prefix("channel_select:")
        .and_then(|value| value.parse::<usize>().ok())
    {
        app.workspace.channel_expression.selected = points.get(index).map(|point| point.key);
        return Ok(());
    }
    if id == "channel_target" {
        let choices = targets(app);
        if !choices.is_empty() {
            let index = choices
                .iter()
                .position(|target| Some(target) == app.workspace.channel_expression.target.as_ref())
                .unwrap_or(0);
            let next = (index as i64 + i64::from(delta)).rem_euclid(choices.len() as i64) as usize;
            app.workspace.channel_expression.target = Some(choices[next].clone());
            app.workspace.channel_expression.selected = None;
        }
        return Ok(());
    }
    if id == "channel_new" {
        return add(app);
    }
    let Some(point) = selected(app, &points) else {
        return Ok(());
    };
    if matches!(id, "channel_lane" | "channel_previous" | "channel_next") {
        browse(app, &points, point, id, delta);
        return Ok(());
    }
    if id == "channel_delete" {
        return edit(app, point, point.at, None);
    }
    let mut control = point.control;
    let mut at = point.at;
    match id {
        "channel_time" => {
            let amount = Beat::new(u64::from(delta.unsigned_abs()), 16)
                .map_err(|error| error.to_string())?;
            at = if delta < 0 {
                at.checked_sub(amount).unwrap_or(Beat::ZERO)
            } else {
                at.checked_add(amount).map_err(|error| error.to_string())?
            };
        }
        "channel_value" => effects::adjust_control(&mut control, "fx_value", delta),
        "channel_number" => effects::adjust_control(&mut control, "fx_second", delta),
        "channel_kind" => effects::adjust_control(&mut control, "fx_control_kind", delta),
        _ => return Ok(()),
    }
    edit(app, point, at, Some(control))
}

fn browse(app: &mut App, points: &[Point], point: Point, id: &str, delta: i32) {
    let mut lanes = Vec::new();
    let lane = Lane::of(point.control);
    let candidates: Vec<_> = points
        .iter()
        .filter(|candidate| {
            let candidate_lane = Lane::of(candidate.control);
            if id == "channel_lane" {
                if lanes.contains(&candidate_lane) {
                    false
                } else {
                    lanes.push(candidate_lane);
                    true
                }
            } else {
                candidate_lane == lane
            }
        })
        .collect();
    let index = candidates
        .iter()
        .position(|candidate| {
            if id == "channel_lane" {
                Lane::of(candidate.control) == lane
            } else {
                candidate.key == point.key
            }
        })
        .unwrap_or(0);
    let direction = if id == "channel_previous" || (id == "channel_lane" && delta < 0) {
        -1
    } else {
        1
    };
    let next = (index as isize + direction).rem_euclid(candidates.len() as isize) as usize;
    app.workspace.channel_expression.selected = Some(candidates[next].key);
}

pub(super) fn submit(app: &mut App, id: &str, value: &str) -> Result<(), String> {
    let point = selected(app, &points(app)).ok_or("Select a channel point first")?;
    if id == "channel_time" {
        return edit(app, point, notes::parse_beat(value)?, Some(point.control));
    }
    let value = value
        .trim()
        .parse::<u16>()
        .map_err(|_| "Enter a nonnegative integer")?;
    let maximum = if id == "channel_value" && matches!(point.control, Control::Bend { .. }) {
        16383
    } else {
        127
    };
    if value > maximum {
        return Err(format!("Value must be 0–{maximum}"));
    }
    let mut control = point.control;
    match (&mut control, id) {
        (Control::Bend { value: current }, "channel_value") => *current = value,
        (
            Control::Pressure { value: current }
            | Control::PolyPressure { value: current, .. }
            | Control::Cc { value: current, .. }
            | Control::Program { value: current },
            "channel_value",
        ) => *current = value as u8,
        (Control::Cc { number, .. }, "channel_number") => *number = value as u8,
        (Control::PolyPressure { pitch, .. }, "channel_number") => *pitch = value as u8,
        _ => return Err("This control has no editable number".into()),
    }
    edit(app, point, point.at, Some(control))
}

fn add(app: &mut App) -> Result<(), String> {
    let (phrase, source) = app.selected_phrase().ok_or("Select a phrase first")?;
    let target = app
        .workspace
        .channel_expression
        .target
        .clone()
        .ok_or("Choose a channel scope")?;
    let at = if app.is_piano_roll() {
        app.piano_cursor().0
    } else {
        source
            .division
            .beat_length()
            .checked_scale(app.row as u64, 1)
            .map_err(|error| error.to_string())?
    };
    let control =
        selected(app, &points(app)).map_or(Control::Pressure { value: 0 }, |point| point.control);
    let id = EventId::new(
        app.editor
            .song()
            .next_identity()
            .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let event = Event {
        id,
        at,
        target,
        note: None,
        controls: vec![control],
        fx: [None, None, None],
    };
    app.requests.push(Request::Edit {
        revision: app.editor.revision(),
        command: Box::new(Command::PutEvent { phrase, event }),
    });
    app.workspace.channel_expression.selected = Some(PointKey {
        event: id,
        control_index: 0,
    });
    Ok(())
}

/// Move/delete only the chosen control, splitting mixed note/control events when
/// necessary. The batch and ordinary shared-reference guard own atomicity/Undo.
fn edit(app: &mut App, point: Point, at: Beat, control: Option<Control>) -> Result<(), String> {
    let (phrase, source) = app.selected_phrase().ok_or("Select a phrase first")?;
    if at >= source.length().map_err(|error| error.to_string())? {
        return Err("Channel point must be inside the phrase".into());
    }
    let mut event = source
        .events
        .iter()
        .find(|event| event.id == point.key.event)
        .cloned()
        .ok_or("Channel point no longer exists")?;
    let current = event
        .controls
        .get_mut(point.key.control_index)
        .ok_or("Channel point no longer exists")?;
    if let Some(control) = control.filter(|_| at == point.at) {
        *current = control;
        app.requests.push(Request::Edit {
            revision: app.editor.revision(),
            command: Box::new(Command::PutEvent { phrase, event }),
        });
        app.workspace.channel_expression.selected = Some(point.key);
        return Ok(());
    }
    event.controls.remove(point.key.control_index);
    let mut commands = vec![if event.note.is_none()
        && event.controls.is_empty()
        && event.fx.iter().all(Option::is_none)
    {
        Command::DeleteEvents {
            phrase,
            events: vec![event.id],
        }
    } else {
        Command::PutEvent {
            phrase,
            event: event.clone(),
        }
    }];
    let key = if let Some(control) = control {
        let id = EventId::new(
            app.editor
                .song()
                .next_identity()
                .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        commands.push(Command::PutEvent {
            phrase,
            event: Event {
                id,
                at,
                target: event.target,
                note: None,
                controls: vec![control],
                fx: [None, None, None],
            },
        });
        Some(PointKey {
            event: id,
            control_index: 0,
        })
    } else {
        None
    };
    app.requests.push(Request::Edit {
        revision: app.editor.revision(),
        command: Box::new(Command::Batch { commands }),
    });
    app.workspace.channel_expression.selected = key;
    Ok(())
}

pub(super) fn curve(
    app: &App,
    window: &projection::RollWindow,
) -> Option<crest_synth::shell::webview::host_presentation::WorkspaceCurve> {
    let (_, phrase) = app.selected_phrase()?;
    let target = app.workspace.channel_expression.target.as_ref()?;
    let points = points(app);
    let selected = selected(app, &points)
        .and_then(|selected| points.iter().position(|point| point.key == selected.key))
        .unwrap_or(0);
    let source: Vec<_> = points
        .iter()
        .map(|point| Expression {
            offset: point.at,
            control: point.control,
            sequence: None,
        })
        .collect();
    Some(expression::curve_for_points(
        window,
        &source,
        selected,
        Beat::ZERO..phrase.length().ok()?,
        format!("CHANNEL · {}", app.target_label(target)),
        "phrase beats",
    ))
}
