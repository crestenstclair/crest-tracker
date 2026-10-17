//! The native timeline paints library notes directly. Grid divisions are visual;
//! edits add an explicit interval or accept an exact beat fraction.
use super::*;
use crate::performance_editor::{note_id, ClipEditor};
use crest_performance_core::{BeatTime, Note, TICKS_PER_BEAT};
use crest_synth::shell::webview::host_presentation::{WorkspaceSpan, WorkspaceTimeline};

pub(super) fn rows(app: &App) -> Vec<WorkspaceRow> {
    let Some(editor) = &app.performance_editor else {
        return commands(&[("back", "Reopen a clip to edit")]);
    };
    let mut rows = commands(&[
        ("performance_previous_note", "Previous note"),
        ("performance_next_note", "Next note"),
    ]);
    if let Some(note) = editor.selected_note() {
        rows.extend([
            row("performance_pitch", "Pitch", pitch_name(app, note.key)),
            row("performance_start", "Start · beats", beat_label(note.start)),
            row(
                "performance_duration",
                "Duration · beats",
                beat_label(note.duration),
            ),
            row(
                "performance_expression",
                "Per-note expression",
                format!(
                    "{} points · Pitch / timbre / pressure retained",
                    note.expression.points.len()
                ),
            ),
        ]);
    }
    rows.extend([
        row(
            "performance_undo_edit",
            "Undo clip edit",
            if editor.can_undo {
                "AVAILABLE"
            } else {
                "NO EDIT"
            },
        ),
        row("performance_editor_launch", "Launch this clip", ""),
        row("stop", "Stop", ""),
        row("back", "Back to clips", ""),
    ]);
    rows
}

pub(super) fn activate(app: &mut App, id: &str) -> bool {
    if app.workspace.panel != Some(Panel::PerformanceEdit) {
        return false;
    }
    let Some(editor) = &mut app.performance_editor else {
        return false;
    };
    match id {
        "performance_previous_note" => editor.select_next(-1),
        "performance_next_note" => editor.select_next(1),
        "performance_undo_edit" => app.requests.push(Request::PerformanceUndo {
            token: editor.token,
        }),
        "performance_editor_launch" => app
            .requests
            .push(Request::PerformanceAudition { slot: editor.slot }),
        "performance_pitch" | "performance_start" | "performance_duration" => {
            let Some(note) = editor.selected_note() else {
                return true;
            };
            let (label, value) = match id {
                "performance_pitch" => ("MIDI pitch · 0–127", note.key.to_string()),
                "performance_start" => ("Start · exact beat fraction", beat_label(note.start)),
                _ => (
                    "Duration · shortening trims expression; Undo restores",
                    beat_label(note.duration),
                ),
            };
            let field = format!("performance_value:{}:{id}", editor.token);
            text_entry(app, &field, label, &value);
        }
        _ => return false,
    }
    true
}

pub(super) fn select(app: &mut App, id: &str) {
    let Some(editor) = &mut app.performance_editor else {
        return;
    };
    if let Some(note) = editor.notes.iter().find(|note| note_id(note.id) == id) {
        editor.selected = Some(note.id);
        app.workspace.focus = Some("performance_pitch".into());
    }
}

pub(super) fn submit(app: &mut App, id: &str, value: &str) -> Result<(), String> {
    let (token, field) = id
        .strip_prefix("performance_value:")
        .and_then(|s| s.split_once(':'))
        .ok_or("Unknown clip edit field")?;
    let editor = app
        .performance_editor
        .as_ref()
        .filter(|view| token.parse::<u64>().ok() == Some(view.token))
        .ok_or("Clip editor changed; reopen the note field")?;
    let mut note = editor.selected_note().ok_or("Select a note first")?.clone();
    match field {
        "performance_pitch" => {
            note.key = value.trim().parse().map_err(|_| "Pitch must be 0–127")?
        }
        "performance_start" => note.start = parse_time(value)?,
        "performance_duration" => note.duration = parse_time(value)?,
        _ => return Err("Unknown clip edit field".into()),
    }
    app.requests.push(edit_request(editor.token, &note));
    Ok(())
}

pub(super) fn adjust(app: &mut App, id: &str, delta: i32) {
    let Some(editor) = &app.performance_editor else {
        return;
    };
    if !matches!(
        id,
        "performance_pitch" | "performance_start" | "performance_duration"
    ) {
        return;
    }
    let Some(mut note) = editor.selected_note().cloned() else {
        return;
    };
    let result = match id {
        "performance_pitch" => {
            note.key = (i32::from(note.key) + delta).clamp(0, 127) as u8;
            Ok(())
        }
        "performance_start" => nudge(note.start, delta).map(|at| note.start = at),
        _ => nudge(note.duration, delta).map(|at| note.duration = at),
    };
    match result {
        Ok(()) => app.requests.push(edit_request(editor.token, &note)),
        Err(error) => app.status = error,
    }
}

fn nudge(at: BeatTime, delta: i32) -> Result<BeatTime, String> {
    let amount = u64::from(delta.unsigned_abs()) * (TICKS_PER_BEAT / 16);
    if delta < 0 {
        at.0.checked_sub(amount)
    } else {
        at.0.checked_add(amount)
    }
    .map(BeatTime)
    .ok_or_else(|| "Note timing cannot move beyond its bounds".into())
}
fn parse_time(value: &str) -> Result<BeatTime, String> {
    crest_tracker::performance::performance_time(notes::parse_beat(value)?)
        .map_err(|e| e.to_string())
}
fn beat_label(at: BeatTime) -> String {
    generator::beat_label(
        crest_tracker::performance::tracker_time(at).expect("fixed-point beats fit rational time"),
    )
}
fn pitch_name(app: &App, key: u8) -> String {
    app.editor
        .song()
        .key
        .pitch_name(crest_tracker::harmony::Pitch::new(key).expect("validated library pitch"))
}
fn edit_request(token: u64, note: &Note) -> Request {
    Request::PerformanceNote {
        token,
        note: note.id,
        key: note.key,
        start: note.start,
        duration: note.duration,
    }
}

pub(super) fn project(app: &App, view: &mut WorkspaceProjection) {
    let Some(editor) = &app.performance_editor else {
        return;
    };
    view.context = format!("CLIP {}", editor.slot + 1);
    view.title = "PIANO ROLL".into();
    view.subtitle = format!(
        "{} notes · {} beats · Free timing",
        editor.notes.len(),
        beat_label(editor.length)
    );
    view.hints = "Select a note or Previous/Next · Edit+direction: pitch / 1/16-beat nudge · Return: exact value · Shortening trims expression; Undo restores".into();
    let Some(note) = editor.selected_note() else {
        return;
    };
    let window = 8 * TICKS_PER_BEAT;
    let from = note.start.0 / window * window;
    let until = from.saturating_add(window);
    let low = note.key.saturating_sub(6).min(115);
    view.columns = (0..32)
        .map(|column| beat_label(BeatTime(from.saturating_add(column * TICKS_PER_BEAT / 4))))
        .collect();
    view.grid = (low..=low + 12)
        .rev()
        .map(|pitch| lane(app, pitch))
        .collect();
    view.timeline = Some(WorkspaceTimeline {
        secondary_lanes: (low..=low + 12)
            .rev()
            .enumerate()
            .filter_map(|(lane, pitch)| matches!(pitch % 12, 1 | 3 | 6 | 8 | 10).then_some(lane))
            .collect(),
        spans: spans(app, editor, low, from, until),
        playhead: None,
    });
    view.compact_grid = true;
}
fn lane(app: &App, pitch: u8) -> WorkspaceGridRow {
    WorkspaceGridRow {
        id: format!("performance_pitch:{pitch}"),
        label: pitch_name(app, pitch),
        cells: (0..32)
            .map(|column| row(format!("performance_cell:{column}:{pitch}"), "", ""))
            .collect(),
        playing: false,
    }
}
fn spans(app: &App, editor: &ClipEditor, low: u8, from: u64, until: u64) -> Vec<WorkspaceSpan> {
    editor
        .notes
        .iter()
        .filter_map(|note| {
            let end = note.start.0.checked_add(note.duration.0)?;
            if !(low..=low + 12).contains(&note.key) || note.start.0 >= until || end <= from {
                return None;
            }
            Some(WorkspaceSpan {
                id: note_id(note.id),
                label: pitch_name(app, note.key),
                lane: usize::from(low + 12 - note.key),
                start: coordinate(note.start.0, from, until),
                end: coordinate(end, from, until),
                level: note.velocity,
                selected: Some(note.id) == editor.selected,
            })
        })
        .collect()
}
fn coordinate(at: u64, from: u64, until: u64) -> u32 {
    (u128::from(at.clamp(from, until) - from) * 10_000 / u128::from(until - from)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crest_performance_core::{
        Expression, ExpressionCurve, ExpressionPoint, NoteId, NoteKey, SourceId, TrackId,
    };

    fn fixture() -> App {
        let mut app = App::new();
        let note = Note {
            id: NoteKey {
                source: SourceId(2),
                track: TrackId(1),
                note: NoteId(u64::MAX),
            },
            key: 60,
            velocity: 100,
            release_velocity: 30,
            start: BeatTime(12_345),
            duration: BeatTime(200_001),
            expression: ExpressionCurve {
                points: vec![ExpressionPoint {
                    offset: BeatTime(0),
                    value: Expression::default(),
                }],
            },
        };
        app.performance_editor = Some(ClipEditor {
            token: 9,
            slot: 3,
            length: BeatTime(TICKS_PER_BEAT),
            selected: Some(note.id),
            notes: vec![note],
            can_undo: false,
        });
        open_performance_editor(&mut app);
        app
    }
    #[test]
    fn native_timeline_and_exact_text_edit_preserve_free_timing_and_large_note_identity() {
        let mut app = fixture();
        let original = app.performance_editor.as_ref().unwrap().notes.clone();
        let projection = super::super::project(&app).workspace.unwrap();
        let span = &projection.timeline.as_ref().unwrap().spans[0];
        assert_eq!(
            span.start,
            (12_345_u64 * 10_000 / (8 * TICKS_PER_BEAT)) as u32
        );
        assert!(span.id.ends_with("18446744073709551615"));
        event(
            &mut app,
            WorkspaceEvent::Activate {
                id: span.id.clone(),
            },
        );
        assert_eq!(app.performance_editor.as_ref().unwrap().notes, original);
        assert!(activate(&mut app, "performance_start"));
        submit(
            &mut app,
            "performance_value:9:performance_start",
            "333/960000",
        )
        .unwrap();
        assert!(
            matches!(app.requests.last(), Some(Request::PerformanceNote { token: 9, note, start: BeatTime(333), duration: BeatTime(200_001), .. }) if note.note == NoteId(u64::MAX))
        );
        assert!(submit(&mut app, "performance_value:8:performance_start", "1/4").is_err());
        assert!(submit(&mut app, "performance_value:9:performance_start", "1/7").is_err());
        assert_eq!(app.performance_editor.as_ref().unwrap().notes, original);
    }
    #[test]
    fn file_dialog_replaces_clip_projection_without_mutating_notes() {
        let mut app = fixture();
        let original = app.performance_editor.as_ref().unwrap().notes.clone();
        app.file_dialog = serde_json::json!({
            "token": 3, "version": 1, "mode": "save", "title": "Save Song As",
            "directory": "/tmp", "filename": "edited.crest-song", "entries": []
        });
        let view = super::super::project(&app).workspace.unwrap();
        assert_eq!(view.context, "SONG FILE");
        assert_eq!(view.title, "Save Song As");
        assert!(view.timeline.is_none() && view.grid.is_empty());
        assert!(view.rows.iter().any(|row| row.id == "dialog:3:1:filename"));
        assert_eq!(app.performance_editor.as_ref().unwrap().notes, original);
        event(
            &mut app,
            WorkspaceEvent::Activate {
                id: "dialog:3:1:filename".into(),
            },
        );
        let view = super::super::project(&app).workspace.unwrap();
        assert_eq!(view.context, "TEXT ENTRY");
        assert_eq!(view.text_entry.as_ref().unwrap().id, "filename:3");
        assert!(view.timeline.is_none());
    }
    #[test]
    fn nudge_adds_interval_without_snapping_and_transport_targets_edited_clip() {
        let mut app = fixture();
        adjust(&mut app, "performance_start", 1);
        assert!(matches!(
            app.requests.last(),
            Some(Request::PerformanceNote {
                start: BeatTime(72_345),
                ..
            })
        ));
        assert!(nudge(BeatTime(12_345), -1).is_err());
        event(
            &mut app,
            WorkspaceEvent::Activate {
                id: "transport".into(),
            },
        );
        assert!(matches!(
            app.requests.last(),
            Some(Request::PerformanceAudition { slot: 3 })
        ));
        app.playing = true;
        action(&mut app, SemanticAction::PreviewStart);
        assert!(matches!(app.requests.last(), Some(Request::Stop)));
    }
}
