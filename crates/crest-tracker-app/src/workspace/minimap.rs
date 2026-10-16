//! Bounded activity projection of the existing occurrence timeline, never a note store.
use super::*;

pub(super) fn project(app: &App, view: &mut WorkspaceProjection) {
    view.inspector.insert(
        0,
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
    let timeline = app.timeline_projection();
    if let Some(error) = timeline["error"].as_str() {
        view.status = error.into();
        return;
    }
    let Some(lanes) = timeline["lanes"].as_array() else {
        return;
    };
    let Some(bars) = timeline["bars"].as_array() else {
        return;
    };
    view.columns = lanes
        .iter()
        .map(|l| l["name"].as_str().unwrap_or("Track").to_owned())
        .collect();
    view.grid = bars
        .iter()
        .enumerate()
        .map(|(index, bar)| bar_row(app, lanes, bars, index, bar, &timeline["end"]))
        .collect();
    view.title = "Song Minimap".into();
    view.subtitle = format!(
        "{} bars · Edit+Left/Right: scroll · Edit+Up/Down: zoom · Edit: open phrase",
        app.preview_bars
    );
    view.focus = lanes
        .iter()
        .enumerate()
        .find_map(|(lane, l)| focused_span(app, l).map(|span| format!("map:{lane}:{span}")))
        .unwrap_or_default();
}
fn focused_span(app: &App, lane: &Value) -> Option<usize> {
    lane["spans"].as_array()?.iter().position(|s| {
        serde_json::from_value::<crest_tracker::editing::Occurrence>(s["occurrence"].clone()).ok()
            == app.occurrence()
            && app.timeline_focus.is_none_or(|at| {
                serde_json::from_value::<Beat>(s["start"].clone()).ok() == Some(at)
            })
    })
}
fn bar_row(
    app: &App,
    lanes: &[Value],
    bars: &[Value],
    index: usize,
    bar: &Value,
    last: &Value,
) -> WorkspaceGridRow {
    let start = serde_json::from_value::<Beat>(bar["at"].clone()).unwrap();
    let end =
        serde_json::from_value::<Beat>(bars.get(index + 1).map_or(last, |v| &v["at"]).clone())
            .unwrap();
    WorkspaceGridRow {
        id: format!("bar:{index}"),
        label: format!("{:02X}", bar["number"].as_u64().unwrap_or(0)),
        playing: app.play_beat.is_some_and(|b| start <= b && b < end),
        cells: lanes
            .iter()
            .enumerate()
            .map(|(lane, l)| activity_cell(app, lane, l, start, end))
            .collect(),
    }
}
fn activity_cell(app: &App, lane: usize, value: &Value, start: Beat, end: Beat) -> WorkspaceRow {
    let spans = value["spans"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let active: Vec<_> = spans
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            let a: Beat = serde_json::from_value(s["start"].clone()).unwrap();
            let b: Beat = serde_json::from_value(s["end"].clone()).unwrap();
            a < end && b > start
        })
        .collect();
    let Some((index, span)) = active.first() else {
        return row(format!("empty:{lane}:{}", start.numerator()), "", "·");
    };
    let count: usize = active
        .iter()
        .map(|(_, s)| activity_count(s, start, end))
        .sum();
    let playing = app
        .play_heads
        .iter()
        .any(|h| serde_json::to_value(h.occurrence).ok().as_ref() == Some(&span["occurrence"]));
    row(
        format!("map:{lane}:{index}"),
        "",
        format!(
            "{}{} · {} notes{}",
            if playing { "▶ " } else { "" },
            span["name"].as_str().unwrap_or("Phrase"),
            count,
            if span["conditional"] == true {
                " ?"
            } else {
                ""
            }
        ),
    )
}
fn activity_count(span: &Value, start: Beat, end: Beat) -> usize {
    span["activity"].as_array().map_or(0, |v| {
        v.iter()
            .filter(|v| {
                let a: Beat = serde_json::from_value(v["at"].clone()).unwrap();
                a >= start && a < end
            })
            .count()
    })
}
pub(super) fn select(app: &mut App, id: &str) {
    app.workspace.minimap_follow_paused = true;
    let parts: Vec<_> = id.split(':').collect();
    if parts.len() != 3 {
        return;
    }
    let (Ok(lane), Ok(span)) = (parts[1].parse::<usize>(), parts[2].parse::<usize>()) else {
        return;
    };
    let timeline = app.timeline_projection();
    let span = &timeline["lanes"][lane]["spans"][span];
    if let (Ok(occurrence), Ok(at)) = (
        serde_json::from_value(span["occurrence"].clone()),
        serde_json::from_value(span["start"].clone()),
    ) {
        app.requests.push(Request::Locate {
            occurrence,
            at,
            row: 0,
            view: View::Minimap,
            row_at: None,
        });
    }
}
pub(super) fn adjust(app: &mut App, direction: Direction) {
    if matches!(direction, Direction::Up | Direction::Down) {
        app.requests.push(Request::PreviewBars {
            bars: if direction == Direction::Up {
                (app.preview_bars / 2).max(1)
            } else {
                (app.preview_bars * 2).min(64)
            },
        });
    } else {
        app.workspace.minimap_follow_paused = true;
        let distance = app.editor.song().meters[0]
            .meter
            .bar_length()
            .checked_scale(app.preview_bars as u64, 1)
            .unwrap();
        let at = if direction == Direction::Left {
            app.preview_start
                .checked_sub(distance)
                .unwrap_or(Beat::ZERO)
        } else {
            app.preview_start
                .checked_add(distance)
                .unwrap_or(app.preview_start)
        };
        app.requests.push(Request::PreviewStart { at });
    }
}
