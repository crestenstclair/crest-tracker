//! Note-owned expression editing and its time-aligned display.
use super::*;
use crest_tracker::composition::{Control, Expression};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Lane {
    Bend,
    Pressure,
    PolyPressure(u8),
    Cc(u8),
    Program,
}
impl Lane {
    pub(super) fn of(control: Control) -> Self {
        match control {
            Control::Bend { .. } => Self::Bend,
            Control::Pressure { .. } => Self::Pressure,
            Control::PolyPressure { pitch, .. } => Self::PolyPressure(pitch),
            Control::Cc { number, .. } => Self::Cc(number),
            Control::Program { .. } => Self::Program,
        }
    }
    pub(super) fn label(self) -> String {
        match self {
            Self::Bend => "Pitch bend".into(),
            Self::Pressure => "Pressure".into(),
            Self::PolyPressure(pitch) => format!("Key pressure {pitch}"),
            Self::Cc(74) => "Timbre · CC74".into(),
            Self::Cc(64) => "Sustain · CC64".into(),
            Self::Cc(number) => format!("CC{number}"),
            Self::Program => "Program".into(),
        }
    }
    pub(super) fn maximum(self) -> u32 {
        if self == Self::Bend {
            16383
        } else {
            127
        }
    }
}
pub(super) fn raw(control: Control) -> u32 {
    match control {
        Control::Bend { value } => u32::from(value),
        Control::Cc { value, .. }
        | Control::Program { value }
        | Control::Pressure { value }
        | Control::PolyPressure { value, .. } => u32::from(value),
    }
}
fn points<'a>(app: &App, note: &'a crest_tracker::composition::Note) -> &'a [Expression] {
    if app.workspace.master_expression {
        &note.master_expression
    } else {
        &note.expression
    }
}
fn lane(app: &App, points: &[Expression]) -> Lane {
    points
        .get(app.workspace.expression_index)
        .or_else(|| points.first())
        .map_or(Lane::Pressure, |point| Lane::of(point.control))
}

pub(super) fn begin(app: &mut App) {
    app.workspace.expression_index = 0;
    app.workspace.master_expression = false;
    open(app, Some(Panel::Expression));
    app.workspace.focus = Some("expr_lane".into());
}

pub(super) fn rows(app: &App) -> Vec<WorkspaceRow> {
    let Some(note) = effects::selected_event(app).and_then(|e| e.note) else {
        return commands(&[("back", "Select a note first")]);
    };
    let points = points(app, &note);
    let lane = lane(app, points);
    let mut rows = vec![
        row(
            "expr_scope",
            "Scope",
            if app.workspace.master_expression {
                "Master"
            } else {
                "Note"
            },
        ),
        row("expr_lane", "Displayed lane", lane.label()),
        row(
            "channel_expression",
            "Channel automation",
            "Independent events",
        ),
        row("expr_previous", "Previous point", "Current lane"),
        row("expr_next", "Next point", "Current lane"),
    ];
    rows[0].enabled = note.mpe.is_some();
    if note.mpe.is_none() {
        rows[0].value = "Note · no MPE master".into();
    }
    if let Some(point) = points.get(app.workspace.expression_index) {
        rows.extend([
            row(
                "expr_offset",
                "Time · beats from attack",
                generator::beat_label(point.offset),
            ),
            row(
                "expr_value",
                "Raw value",
                format!("{} / {}", raw(point.control), lane.maximum()),
            ),
            row("expr_kind", "Control type", lane.label()),
        ]);
        if let Control::Cc { number, .. } = point.control {
            rows.push(row("expr_number", "CC number", number.to_string()));
        } else if let Control::PolyPressure { pitch, .. } = point.control {
            let mut item = row("expr_number", "Key pitch", pitch.to_string());
            item.enabled = app.workspace.master_expression;
            rows.push(item);
        }
    }
    rows.push(row("expr_new", "Add point", "At cursor"));
    if !points.is_empty() {
        rows.push(row("expr_delete", "Delete point", "Current point only"));
    }
    rows.extend([
        row("undo", "Undo", "Last edit"),
        row("redo", "Redo", "Last undo"),
        row("effects", "FX commands", "Selected note"),
        row("back", "Done", "Changes kept"),
    ]);
    rows.extend(
        points
            .iter()
            .enumerate()
            .filter(|(_, p)| Lane::of(p.control) == lane)
            .map(|(i, p)| {
                let mut item = row(
                    format!("expr_select:{i}"),
                    format!(
                        "Point {} · {} beats",
                        i + 1,
                        generator::beat_label(p.offset)
                    ),
                    raw(p.control).to_string(),
                );
                item.selected = i == app.workspace.expression_index;
                item
            }),
    );
    rows
}
pub(super) fn activate(app: &mut App, id: &str) -> bool {
    if app.workspace.panel == Some(Panel::Expression) && matches!(id, "undo" | "redo") {
        app.workspace.expression_index = 0;
        return false;
    }
    if !matches!(id, "expr_offset" | "expr_value" | "expr_number") {
        return adjust(app, id, 1);
    }
    let Some(note) = effects::selected_event(app).and_then(|event| event.note) else {
        return true;
    };
    let points = if app.workspace.master_expression {
        &note.master_expression
    } else {
        &note.expression
    };
    if let Some(point) = points.get(app.workspace.expression_index) {
        let (label, value) = match id {
            "expr_offset" => (
                "Point time in beats, e.g. 1/3",
                generator::beat_label(point.offset),
            ),
            "expr_value" => (
                "Raw value (bend 0–16383; others 0–127)",
                match point.control {
                    Control::Bend { value } => value.to_string(),
                    Control::Cc { value, .. }
                    | Control::Program { value }
                    | Control::Pressure { value }
                    | Control::PolyPressure { value, .. } => value.to_string(),
                },
            ),
            _ => (
                "CC number / pitch (0–127)",
                match point.control {
                    Control::Cc { number, .. } => number.to_string(),
                    Control::PolyPressure { pitch, .. } => pitch.to_string(),
                    _ => return true,
                },
            ),
        };
        text_entry(app, id, label, &value);
    }
    true
}

pub(super) fn submit(app: &mut App, id: &str, value: &str) -> Result<(), String> {
    let (phrase, _) = app.selected_phrase().ok_or("Select a phrase first")?;
    let mut event = effects::selected_event(app).ok_or("Select a note first")?;
    let note = event.note.as_mut().ok_or("Select a note first")?;
    let lifetime = note.expression_duration();
    let points = if app.workspace.master_expression {
        &mut note.master_expression
    } else {
        &mut note.expression
    };
    let point = points
        .get_mut(app.workspace.expression_index)
        .ok_or("Select an expression point")?;
    if id == "expr_offset" {
        let offset = notes::parse_beat(value)?;
        if offset > lifetime {
            return Err("Point must be within the note lifetime".into());
        }
        point.offset = offset;
    } else {
        let value = value
            .trim()
            .parse::<u16>()
            .map_err(|_| "Enter a nonnegative integer")?;
        let maximum = if id == "expr_value" && matches!(point.control, Control::Bend { .. }) {
            16383
        } else {
            127
        };
        if value > maximum {
            return Err(format!("Value must be 0–{maximum}"));
        }
        match (&mut point.control, id) {
            (Control::Bend { value: v }, "expr_value") => *v = value,
            (
                Control::Cc { value: v, .. }
                | Control::Program { value: v }
                | Control::Pressure { value: v }
                | Control::PolyPressure { value: v, .. },
                "expr_value",
            ) => *v = value as u8,
            (Control::Cc { number, .. }, "expr_number") => *number = value as u8,
            (Control::PolyPressure { pitch, .. }, "expr_number")
                if app.workspace.master_expression =>
            {
                *pitch = value as u8
            }
            _ => return Err("Note pressure retains the note's pitch".into()),
        }
    }
    app.workspace.expression_index = sort_points(points, app.workspace.expression_index);
    app.requests.push(Request::Edit {
        revision: app.editor.revision(),
        command: Box::new(Command::PutEvent { phrase, event }),
    });
    Ok(())
}

pub(super) fn adjust(app: &mut App, id: &str, delta: i32) -> bool {
    if let Some(index) = id.strip_prefix("expr_select:").and_then(|v| v.parse().ok()) {
        app.workspace.expression_index = index;
        return true;
    }
    if !id.starts_with("expr_") {
        return false;
    }
    if id == "expr_scope" {
        if effects::selected_event(app)
            .and_then(|event| event.note)
            .is_none_or(|note| note.mpe.is_none())
        {
            app.status = "Master expression requires MPE provenance".into();
            return true;
        }
        app.workspace.master_expression = !app.workspace.master_expression;
        app.workspace.expression_index = 0;
        return true;
    }
    let Some((phrase, _)) = app.selected_phrase() else {
        return true;
    };
    let Some(mut event) = effects::selected_event(app) else {
        return true;
    };
    let Some(note) = &mut event.note else {
        return true;
    };
    let lifetime = note.expression_duration();
    let pitch = note.pitch;
    let points = if app.workspace.master_expression {
        &mut note.master_expression
    } else {
        &mut note.expression
    };
    if matches!(id, "expr_lane" | "expr_previous" | "expr_next") {
        select_point(app, id, delta, points);
        return true;
    }
    if id == "expr_new" {
        let control = points
            .get(app.workspace.expression_index)
            .map_or(Control::Pressure { value: 0 }, |point| point.control);
        let at = if app.is_piano_roll() {
            app.piano_cursor().0
        } else {
            event.at
        };
        points.push(Expression {
            offset: at.checked_sub(event.at).unwrap_or(Beat::ZERO).min(lifetime),
            control,
            sequence: None,
        });
        app.workspace.expression_index = points.len() - 1;
    } else if id == "expr_delete" && app.workspace.expression_index < points.len() {
        points.remove(app.workspace.expression_index);
        app.workspace.expression_index = app
            .workspace
            .expression_index
            .min(points.len().saturating_sub(1));
    } else if let Some(point) = points.get_mut(app.workspace.expression_index) {
        if !adjust_point(point, id, delta, lifetime) {
            return true;
        }
    } else {
        return true;
    }
    // Poly-pressure on a note must retain that note's pitch ownership.
    if !app.workspace.master_expression {
        for point in points.iter_mut() {
            if let Control::PolyPressure { pitch: p, .. } = &mut point.control {
                *p = pitch;
            }
        }
    }
    app.workspace.expression_index = sort_points(points, app.workspace.expression_index);
    app.requests.push(Request::Edit {
        revision: app.editor.revision(),
        command: Box::new(Command::PutEvent { phrase, event }),
    });
    true
}

fn adjust_point(point: &mut Expression, id: &str, delta: i32, lifetime: Beat) -> bool {
    match id {
        "expr_offset" => {
            let amount = Beat::new(u64::from(delta.unsigned_abs()), 16).unwrap();
            point.offset = if delta < 0 {
                point.offset.checked_sub(amount).unwrap_or(Beat::ZERO)
            } else {
                point
                    .offset
                    .checked_add(amount)
                    .unwrap_or(lifetime)
                    .min(lifetime)
            };
        }
        "expr_kind" => effects::adjust_control(&mut point.control, "fx_control_kind", delta),
        "expr_value" => effects::adjust_control(&mut point.control, "fx_value", delta),
        "expr_number" => effects::adjust_control(&mut point.control, "fx_second", delta),
        _ => return false,
    }
    true
}

// Preserve point identity through sorting, including duplicate values/timestamps.
fn sort_points(points: &mut Vec<Expression>, selected: usize) -> usize {
    let mut indexed: Vec<_> = points.drain(..).enumerate().collect();
    indexed.sort_by_key(|(_, point)| (point.offset, point.sequence));
    let index = indexed
        .iter()
        .position(|(index, _)| *index == selected)
        .unwrap_or(0);
    points.extend(indexed.into_iter().map(|(_, point)| point));
    index
}
fn select_point(app: &mut App, id: &str, delta: i32, points: &[Expression]) {
    let current_lane = lane(app, points);
    let candidates: Vec<_> = if id == "expr_lane" {
        let mut lanes = Vec::new();
        points
            .iter()
            .enumerate()
            .filter_map(|(i, point)| {
                let lane = Lane::of(point.control);
                if lanes.contains(&lane) {
                    None
                } else {
                    lanes.push(lane);
                    Some(i)
                }
            })
            .collect()
    } else {
        points
            .iter()
            .enumerate()
            .filter_map(|(i, point)| (Lane::of(point.control) == current_lane).then_some(i))
            .collect()
    };
    if candidates.is_empty() {
        return;
    }
    let current = candidates
        .iter()
        .position(|index| {
            if id == "expr_lane" {
                Lane::of(points[*index].control) == current_lane
            } else {
                *index == app.workspace.expression_index
            }
        })
        .unwrap_or(0);
    let backwards = id == "expr_previous" || (id == "expr_lane" && delta < 0);
    let index = (current as isize + if backwards { -1 } else { 1 })
        .rem_euclid(candidates.len() as isize) as usize;
    app.workspace.expression_index = candidates[index];
}

pub(super) fn curve(
    app: &App,
    window: &projection::RollWindow,
) -> Option<crest_synth::shell::webview::host_presentation::WorkspaceCurve> {
    let event = effects::selected_event(app)?;
    let note = event.note.as_ref()?;
    let source = points(app, note);
    Some(curve_for_points(
        window,
        source,
        app.workspace.expression_index,
        event.at..event.at.checked_add(note.expression_duration()).ok()?,
        format!(
            "{} · {}",
            if app.workspace.master_expression {
                "MASTER"
            } else {
                "NOTE"
            },
            app.target_label(&event.target),
        ),
        "beats from attack",
    ))
}

/// Shared held-value projection; callers supply the exact scope and lifetime.
pub(super) fn curve_for_points(
    window: &projection::RollWindow,
    source: &[Expression],
    selected: usize,
    lifetime: std::ops::Range<Beat>,
    caption: String,
    time_label: &str,
) -> crest_synth::shell::webview::host_presentation::WorkspaceCurve {
    use crest_synth::shell::webview::host_presentation::{WorkspaceCurve, WorkspaceCurvePoint};
    let lane = source
        .get(selected)
        .or_else(|| source.first())
        .map_or(Lane::Pressure, |point| Lane::of(point.control));
    let mut samples: Vec<_> = source
        .iter()
        .enumerate()
        .filter(|(_, point)| Lane::of(point.control) == lane)
        .filter_map(|(index, point)| {
            Some((index, lifetime.start.checked_add(point.offset).ok()?, point))
        })
        .collect();
    samples.sort_by_key(|(_, at, point)| (*at, point.sequence));
    let end = lifetime.end.min(window.until);
    let mut curve = WorkspaceCurve {
        label: lane.label(),
        caption: format!(
            "{} · {} point{}",
            caption,
            samples.len(),
            if samples.len() == 1 { "" } else { "s" }
        ),
        maximum: lane.maximum(),
        path: Vec::new(),
        points: Vec::new(),
    };
    for (position, (index, at, point)) in samples.iter().enumerate() {
        let value = raw(point.control) * 10000 / lane.maximum();
        // Draw held MIDI values, not interpolated automation ramps. A value before
        // the viewport remains visible until the next point or owned lifetime ends.
        let next = samples
            .get(position + 1)
            .map_or(end, |(_, at, _)| (*at).min(end));
        if *at < window.until && next >= window.from && end >= window.from {
            curve.path.push([window.coordinate(*at), value]);
            curve.path.push([window.coordinate(next), value]);
        }
        if *at >= window.from && *at <= window.until {
            curve.points.push(WorkspaceCurvePoint {
                label: format!(
                    "Point {} · {} {} · {}",
                    index + 1,
                    generator::beat_label(point.offset),
                    time_label,
                    raw(point.control)
                ),
                position: window.coordinate(*at),
                value,
                selected: *index == selected,
            });
        }
    }
    curve
}
