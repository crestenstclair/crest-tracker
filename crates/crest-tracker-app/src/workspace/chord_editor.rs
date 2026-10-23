//! Immutable chord revisions: edits make a draft; Save/Apply publishes it once.
use super::*;
use crate::app::{ChordDraft, ChordSource};
use crest_tracker::chord_bank::ChordRevision;
use crest_tracker::harmony::{Chord, ChordKind};

pub(super) fn is_detail_control(id: &str) -> bool {
    matches!(
        id,
        "chord_inversion" | "chord_open" | "chord_spread" | "chord_publish" | "back"
    )
}

/// Follow the rendered columns, not the flattened projection's hidden key rows.
pub(super) fn navigate(app: &mut App, direction: Direction, view: &WorkspaceProjection) {
    let setup: Vec<_> = view
        .rows
        .iter()
        .filter(|row| !is_detail_control(&row.id) && !row.id.starts_with("chord_key:"))
        .collect();
    let controls: Vec<_> = view
        .rows
        .iter()
        .filter(|row| is_detail_control(&row.id))
        .collect();
    let keys: Vec<_> = view
        .rows
        .iter()
        .filter(|row| row.id.starts_with("chord_key:"))
        .collect();
    let focus = view.focus.as_str();
    let next = if focus.starts_with("chord_key:") {
        match direction {
            Direction::Down => controls.first().copied(),
            Direction::Up => setup.last().copied(),
            Direction::Left | Direction::Right => adjacent(&keys, focus, direction),
        }
    } else if is_detail_control(focus) {
        match direction {
            Direction::Left => setup.last().copied(),
            Direction::Up if controls.first().is_some_and(|row| row.id == focus) => {
                keys.first().copied()
            }
            _ => adjacent(&controls, focus, direction),
        }
    } else {
        match direction {
            Direction::Right => controls.first().copied(),
            Direction::Left => None,
            _ => adjacent(&setup, focus, direction),
        }
    };
    if let Some(next) = next {
        app.workspace.focus = Some(next.id.clone());
    }
}

fn adjacent<'a>(
    rows: &[&'a WorkspaceRow],
    focus: &str,
    direction: Direction,
) -> Option<&'a WorkspaceRow> {
    let index = rows.iter().position(|row| row.id == focus)?;
    let delta = if matches!(direction, Direction::Up | Direction::Left) {
        -1
    } else {
        1
    };
    rows.get(
        index
            .saturating_add_signed(delta)
            .min(rows.len().saturating_sub(1)),
    )
    .copied()
}

pub(super) fn begin(app: &mut App, chord: Option<ChordRevision>) {
    let mut draft = chord.map_or(
        ChordDraft {
            name: "Custom chord".into(),
            intervals: vec![0, 4, 7],
            root: 60,
            inversion: 0,
            open: false,
            spread: 0,
        },
        |chord| {
            let d = chord.definition();
            ChordDraft {
                name: chords::display_name(&chord),
                intervals: d.shape().intervals().to_vec(),
                root: chord.verification().root().midi(),
                inversion: d.inversion(),
                open: d.open(),
                spread: d.spread(),
            }
        },
    );
    if app.view == View::Phrase {
        if let Some(note) = effects::selected_event(app).and_then(|e| e.note) {
            draft.root = note.pitch;
        }
    }
    app.workspace.chord_octave = 0;
    open(app, Some(Panel::ChordEditor));
    app.workspace.chord = Some(draft);
}

pub(super) fn rows(app: &App) -> Vec<WorkspaceRow> {
    let Some(d) = &app.workspace.chord else {
        return commands(&[("back", "Back")]);
    };
    let mut rows = vec![
        row("chord_name", "Name", &d.name),
        row(
            "chord_quality",
            "Quality",
            Chord::new(d.intervals.clone()).map_or("Custom".into(), |c| c.name()),
        ),
        row(
            "chord_octave",
            "Interval octave",
            format!("{:+}", app.workspace.chord_octave),
        ),
    ];
    rows.extend((0..=12).map(|key| {
        let interval = key + app.workspace.chord_octave * 12;
        let mut row = row(
            format!("chord_key:{interval}"),
            chords::interval_label(&[interval]),
            if d.intervals.contains(&interval) {
                "On"
            } else {
                "Off"
            },
        );
        row.selected = d.intervals.contains(&interval);
        row
    }));
    rows.extend([
        row("chord_inversion", "Inversion", d.inversion.to_string()),
        row(
            "chord_open",
            "Open voicing",
            if d.open { "On" } else { "Off" },
        ),
        row("chord_spread", "Spread", d.spread.to_string()),
    ]);
    rows.extend(commands(&[
        ("chord_publish", "Save chord"),
        ("back", "Cancel"),
    ]));
    rows
}

pub(super) fn adjust(app: &mut App, id: &str, delta: i32) {
    if id == "chord_octave" {
        app.workspace.chord_octave = (app.workspace.chord_octave as i32 + delta).clamp(-1, 1) as i8;
        return;
    }
    let Some(d) = &mut app.workspace.chord else {
        return;
    };
    match id {
        "chord_quality" => {
            let index = ChordKind::CATALOG
                .iter()
                .position(|k| Chord::from_kind(*k).intervals() == d.intervals)
                .unwrap_or(0);
            let kind = ChordKind::CATALOG
                [(index as i32 + delta).rem_euclid(ChordKind::CATALOG.len() as i32) as usize];
            d.intervals = Chord::from_kind(kind).intervals().to_vec();
            d.name = kind.name().into();
            d.inversion = 0;
        }
        "chord_root" => d.root = (i32::from(d.root) + delta).clamp(0, 127) as u8,
        "chord_inversion" => {
            d.inversion = d
                .inversion
                .saturating_add_signed(delta as i8)
                .min(d.intervals.len().saturating_sub(1) as u8)
        }
        "chord_open" => d.open = !d.open,
        "chord_spread" => d.spread = d.spread.saturating_add_signed(delta as i8),
        _ => {
            if let Some(i) = id
                .strip_prefix("chord_interval:")
                .and_then(|i| i.parse::<usize>().ok())
            {
                if let Some(v) = d.intervals.get_mut(i) {
                    *v = v.saturating_add(delta as i8);
                }
            }
        }
    }
}

pub(super) fn activate(app: &mut App, id: &str) -> bool {
    if let Some(interval) = id
        .strip_prefix("chord_key:")
        .and_then(|v| v.parse::<i8>().ok())
    {
        if let Some(draft) = &mut app.workspace.chord {
            if draft.intervals.contains(&interval) {
                if draft.intervals.len() > 1 {
                    draft.intervals.retain(|v| *v != interval);
                }
            } else {
                draft.intervals.push(interval);
                draft.intervals.sort_unstable();
            }
            draft.inversion = draft
                .inversion
                .min(draft.intervals.len().saturating_sub(1) as u8);
        }
        return true;
    }
    if id == "chord_new" {
        begin(app, None);
        return true;
    }
    if !id.starts_with("chord_") || id == "chord_library" {
        return false;
    }
    let Some(draft) = app.workspace.chord.clone() else {
        return false;
    };
    match id {
        "chord_name" => text_entry(app, id, "Chord name", &draft.name),
        "chord_intervals" => text_entry(
            app,
            id,
            "Intervals separated by spaces",
            &draft
                .intervals
                .iter()
                .map(i8::to_string)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        "chord_add" => app.workspace.chord.as_mut().unwrap().intervals.push(
            draft
                .intervals
                .last()
                .copied()
                .unwrap_or(0)
                .saturating_add(2),
        ),
        "chord_remove" => {
            if draft.intervals.len() > 1 {
                app.workspace.chord.as_mut().unwrap().intervals.pop();
            }
        }
        "chord_save" => app.requests.push(Request::SaveChord {
            revision: app.editor.revision(),
            draft,
        }),
        "chord_apply" => {
            if apply(app, ChordSource::Draft { draft }) {
                open(app, None);
            }
        }
        "chord_publish" => app.requests.push(Request::ChordLibrary {
            action: crate::app::ChordLibraryAction::Publish {
                revision: app.editor.revision(),
                source: ChordSource::Draft { draft },
            },
        }),
        "chord_preview" => {
            app.status = draft
                .verified()
                .map_or_else(|e| e.to_string(), |_| "Chord voicing valid".into());
        }
        _ => adjust(app, id, 1),
    }
    true
}

pub(super) fn apply(app: &mut App, source: ChordSource) -> bool {
    let Some((_, phrase)) = app.selected_phrase() else {
        app.status = "Open a phrase and select a note first".into();
        return false;
    };
    let at = if app.is_piano_roll() {
        Some(app.piano_cursor().0)
    } else {
        phrase
            .division
            .beat_length()
            .checked_scale(app.row as u64, 1)
            .ok()
    };
    let selected = if app.is_piano_roll() {
        app.selected_event_ids()
    } else {
        vec![]
    };
    let event = phrase.events.iter().find(|event| {
        event.note.is_some() && (selected.contains(&event.id) || Some(event.at) == at)
    });
    let Some(event) = event else {
        app.status = "Select a note before applying a chord".into();
        return false;
    };
    let Some(occurrence) = app.occurrence() else {
        return false;
    };
    app.requests.push(Request::ApplyChord {
        revision: app.editor.revision(),
        occurrence,
        event: event.id,
        slot: app.workspace.chord_slot,
        root: event.note.as_ref().unwrap().pitch,
        source,
    });
    true
}
