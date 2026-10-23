//! The native timeline paints library notes directly. Grid divisions are visual;
//! edits add an explicit interval or accept an exact beat fraction.
use super::*;
use crate::performance_editor::{note_id, ClipEditor, ClipView};
use crest_performance_core::{BeatTime, Note, NoteCondition, TICKS_PER_BEAT};
use crest_synth::shell::webview::host_presentation::{WorkspaceSpan, WorkspaceTimeline};

pub(super) fn rows(app: &App) -> Vec<WorkspaceRow> {
    let Some(editor) = &app.performance_editor else {
        return commands(&[("back", "Reopen a clip to edit")]);
    };
    let mut rows = vec![row("performance_view", "View", editor.view.label())];
    rows.extend(commands(&[
        ("performance_previous_note", "Previous note"),
        ("performance_next_note", "Next note"),
    ]));
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
            row(
                "performance_condition_status",
                "Note playback rule",
                condition_label(editor.condition(note.id)),
            ),
            row(
                "performance_condition",
                "Set playback · loop N of P",
                "1/1 plays every loop",
            ),
        ]);
    }
    rows.extend(commands(&[
        (
            "performance_quantize_quarter",
            "Quantize clip attacks · nearest 1/4 beat",
        ),
        (
            "performance_quantize_half",
            "Quantize clip attacks · nearest 1/2 beat",
        ),
        (
            "performance_quantize_beat",
            "Quantize clip attacks · nearest beat",
        ),
    ]));
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
        "performance_view" => editor.view.toggle(),
        "performance_previous_note" => editor.select_next(-1),
        "performance_next_note" => editor.select_next(1),
        "performance_undo_edit" => app.requests.push(Request::PerformanceUndo {
            token: editor.token,
        }),
        "performance_quantize_quarter"
        | "performance_quantize_half"
        | "performance_quantize_beat" => {
            let divisor = match id {
                "performance_quantize_quarter" => 4,
                "performance_quantize_half" => 2,
                _ => 1,
            };
            app.requests.push(Request::PerformanceQuantize {
                token: editor.token,
                quantum: BeatTime(TICKS_PER_BEAT / divisor),
            });
        }
        "performance_editor_launch" => app
            .requests
            .push(Request::PerformanceAudition { slot: editor.slot }),
        "performance_condition" => {
            let Some(note) = editor.selected_note() else {
                return true;
            };
            let value = match editor.condition(note.id) {
                NoteCondition::Every { period, offset } => format!("{}/{period}", offset + 1),
                _ => "1/1".into(),
            };
            let field = format!("performance_value:{}:performance_condition", editor.token);
            text_entry(
                app,
                &field,
                "Loop / period · 4/4 plays on loops 4, 8, 12; 1/1 always",
                &value,
            );
        }
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
    let (identity, field) = id.split_once('@').unwrap_or((id, "pitch"));
    if let Some(note) = editor
        .notes
        .iter()
        .find(|note| note_id(note.id) == identity)
    {
        editor.selected = Some(note.id);
        app.workspace.focus = Some(
            match field {
                "start" => "performance_start",
                "duration" => "performance_duration",
                _ => "performance_pitch",
            }
            .into(),
        );
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
    if field == "performance_condition" {
        app.requests.push(Request::PerformanceNoteCondition {
            token: editor.token,
            note: note.id,
            condition: parse_condition(value)?,
        });
        return Ok(());
    }
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
fn condition_label(condition: NoteCondition) -> String {
    match condition {
        NoteCondition::Always {} => "Every loop".into(),
        NoteCondition::Every { period, offset } => format!("Loop {} of {period}", offset + 1),
        NoteCondition::Fill {} => "Fill · held Fill control unavailable".into(),
        NoteCondition::NotFill {} => "Not Fill · held Fill control unavailable".into(),
    }
}

fn parse_condition(value: &str) -> Result<NoteCondition, String> {
    let (iteration, period) = value
        .trim()
        .split_once('/')
        .ok_or("Enter loop/period, such as 4/4")?;
    let iteration: u16 = iteration
        .trim()
        .parse()
        .map_err(|_| "Loop must be 1–65535")?;
    let period: u16 = period
        .trim()
        .parse()
        .map_err(|_| "Period must be 1–65535")?;
    let offset = iteration.checked_sub(1).ok_or("Loop must start at 1")?;
    let condition = NoteCondition::Every { period, offset };
    condition
        .validate()
        .map_err(|_| "Loop must be within a nonzero period")?;
    Ok(if period == 1 {
        NoteCondition::Always {}
    } else {
        condition
    })
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
    view.title = editor.view.label().to_uppercase();
    view.subtitle = format!(
        "{} notes · {} beats · Free timing",
        editor.notes.len(),
        beat_label(editor.length)
    );
    view.hints = "Select a note or Previous/Next · Edit+direction: pitch / 1/16-beat nudge · Return: exact value · Shortening trims expression; Undo restores".into();
    if editor.view == ClipView::Tracker {
        project_tracker(app, editor, view);
        return;
    }
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
/// Every attack has its own row, even simultaneous or repeated pitches. This
/// is a view of the same clip, not a conversion to a quantized Song phrase.
fn project_tracker(app: &App, editor: &ClipEditor, view: &mut WorkspaceProjection) {
    view.columns = [
        "Start · beats",
        "Pitch",
        "Duration · beats",
        "Velocity",
        "Expression",
    ]
    .map(str::to_owned)
    .to_vec();
    let selected = editor
        .notes
        .iter()
        .position(|note| Some(note.id) == editor.selected)
        .unwrap_or(0);
    let start = selected / 12 * 12;
    view.grid = editor
        .notes
        .iter()
        .enumerate()
        .skip(start)
        .take(12)
        .map(|(index, note)| tracker_row(app, note, index, editor.selected == Some(note.id)))
        .collect();
    view.hints = "One row per note · Exact beats retain off-grid timing and polyphony · Select a cell or Previous/Next · Return: exact value · View changes preserve expression".into();
}
fn tracker_row(app: &App, note: &Note, index: usize, selected: bool) -> WorkspaceGridRow {
    let identity = note_id(note.id);
    let fields = [
        ("start", beat_label(note.start)),
        ("pitch", pitch_name(app, note.key)),
        ("duration", beat_label(note.duration)),
        ("velocity", note.velocity.to_string()),
        (
            "expression",
            format!("{} points", note.expression.points.len()),
        ),
    ];
    WorkspaceGridRow {
        id: identity.clone(),
        label: format!("{index:02}"),
        cells: fields
            .into_iter()
            .map(|(field, value)| {
                let mut cell = row(format!("{identity}@{field}"), "", value);
                cell.selected = selected;
                cell
            })
            .collect(),
        playing: false,
    }
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
            view: ClipView::PianoRoll,
            slot: 3,
            length: BeatTime(TICKS_PER_BEAT),
            selected: Some(note.id),
            notes: vec![note],
            conditions: Vec::new(),
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
    fn periodic_rule_text_uses_one_based_loops_and_validates_before_request() {
        let mut app = fixture();
        app.requests.clear();
        assert!(activate(&mut app, "performance_condition"));
        assert!(!app
            .requests
            .iter()
            .any(|request| matches!(request, Request::PerformanceNoteCondition { .. })));
        app.requests.clear(); // Entering a text form ends any previous edit gesture.
        submit(&mut app, "performance_value:9:performance_condition", "4/4").unwrap();
        assert!(matches!(app.requests.as_slice(),
            [Request::PerformanceNoteCondition { token: 9, note, condition: NoteCondition::Every { period: 4, offset: 3 } }]
            if note.note == NoteId(u64::MAX)));
        assert_eq!(parse_condition("1/1").unwrap(), NoteCondition::Always {});
        assert_eq!(
            parse_condition(" 2 / 7 ").unwrap(),
            NoteCondition::Every {
                period: 7,
                offset: 1
            }
        );
        for value in [
            "0/4",
            "1/0",
            "5/4",
            "65536/65536",
            "4",
            "1/2/3",
            "fill",
            "-1/4",
        ] {
            assert!(parse_condition(value).is_err(), "{value}");
        }
        app.requests.clear();
        assert!(submit(&mut app, "performance_value:8:performance_condition", "4/4").is_err());
        assert!(app.requests.is_empty());
    }

    #[test]
    fn quantization_requires_explicit_activation_and_keeps_editor_token() {
        let mut app = fixture();
        let original = app.performance_editor.as_ref().unwrap().notes.clone();
        for (id, ticks) in [
            ("performance_quantize_quarter", 240_000),
            ("performance_quantize_half", 480_000),
            ("performance_quantize_beat", 960_000),
        ] {
            app.requests.clear();
            assert!(rows(&app).iter().any(|row| row.id == id));
            assert!(app.requests.is_empty());
            event(&mut app, WorkspaceEvent::Activate { id: id.into() });
            assert!(matches!(app.requests.as_slice(),
                [Request::PerformanceQuantize { token: 9, quantum }] if quantum.0 == ticks));
            assert_eq!(app.performance_editor.as_ref().unwrap().notes, original);
        }
    }

    #[test]
    fn tracker_view_preserves_polyphony_exact_times_and_selection_without_song_mutation() {
        let mut app = fixture();
        app.requests.clear(); // The fixture's initial screen change ends any prior gesture.
        let editor = app.performance_editor.as_mut().unwrap();
        let mut second = editor.notes[0].clone();
        second.id.note = NoteId(7);
        editor.notes.push(second.clone());
        let mut third = second;
        third.id.note = NoteId(8);
        third.start.0 += 17;
        editor.notes.push(third);
        let original = editor.notes.clone();
        let song_revision = app.editor.revision();
        event(
            &mut app,
            WorkspaceEvent::Activate {
                id: "performance_view".into(),
            },
        );
        let view = super::super::project(&app).workspace.unwrap();
        assert_eq!(view.title, "TRACKER");
        assert!(view.timeline.is_none());
        assert_eq!(view.grid.len(), 3);
        assert_eq!(view.grid[0].cells[0].value, view.grid[1].cells[0].value);
        assert_eq!(
            parse_time(&view.grid[0].cells[0].value).unwrap(),
            BeatTime(12_345)
        );
        assert_eq!(
            parse_time(&view.grid[2].cells[0].value).unwrap(),
            BeatTime(12_362)
        );
        assert!(view.grid[0].id.ends_with("18446744073709551615"));
        event(
            &mut app,
            WorkspaceEvent::Activate {
                id: view.grid[1].cells[2].id.clone(),
            },
        );
        assert_eq!(
            app.performance_editor.as_ref().unwrap().selected,
            Some(original[1].id)
        );
        assert_eq!(app.workspace.focus.as_deref(), Some("performance_duration"));
        event(
            &mut app,
            WorkspaceEvent::Activate {
                id: "performance_view".into(),
            },
        );
        let view = super::super::project(&app).workspace.unwrap();
        assert!(view
            .timeline
            .as_ref()
            .unwrap()
            .spans
            .iter()
            .any(|span| span.id == note_id(original[1].id) && span.selected));
        action(&mut app, SemanticAction::SelectPatch(Direction::Right));
        assert!(
            app.requests.is_empty(),
            "View and selection must not issue Song edits"
        );
        assert_eq!(app.editor.revision(), song_revision);
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
