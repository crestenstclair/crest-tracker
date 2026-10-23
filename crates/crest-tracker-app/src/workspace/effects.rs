//! Three typed effect slots; drafts never alter the Song until Apply.
use super::*;
use crest_tracker::composition::{Control, Event, EventId, Fx};
use generator::{beat_label, step_beat};

pub(super) fn selected_event(app: &App) -> Option<Event> {
    let (_, phrase) = app.selected_phrase()?;
    let selected = if app.is_piano_roll() {
        app.selected_event_ids()
    } else {
        vec![]
    };
    let at = if app.is_piano_roll() {
        app.piano_cursor().0
    } else {
        phrase
            .division
            .beat_length()
            .checked_scale(app.row as u64, 1)
            .ok()?
    };
    phrase
        .events
        .iter()
        .find(|e| selected.contains(&e.id))
        .or_else(|| {
            if app.is_piano_roll() {
                let occupied = app.piano_notes_at_cursor();
                phrase.events.iter().find(|e| occupied.contains(&e.id))
            } else {
                phrase.events.iter().find(|e| e.at == at)
            }
        })
        .cloned()
}
pub(super) fn begin(app: &mut App) {
    if app.selected_phrase().is_none() {
        app.status = "Open a phrase before editing FX".into();
        return;
    }
    app.workspace.fx_slot = app.column.saturating_sub(3).min(2);
    app.workspace.fx =
        selected_event(app).and_then(|event| event.fx[app.workspace.fx_slot].clone());
    open(app, Some(Panel::FxCommands));
}

pub(super) fn command_rows() -> Vec<WorkspaceRow> {
    let descriptions = [
        "Clear · Remove command",
        "Pitch · Transpose note in semitones",
        "Dynamics · Set note velocity",
        "Timing · Scale note length",
        "Timing · End sounding notes after a delay",
        "Timing · Delay note onset",
        "Timing · Repeat note at fixed intervals",
        "Sequence · Probability of playing this event",
        "Control · MIDI CC, bend, pressure or program",
        "Control · Set temporary send amount",
        "Sequence · Jump to a phrase step",
        "Sequence · Set transport tempo",
        "Pitch · Choose intervals from chord bank",
        "Pitch · Add intervals to the root",
        "Pitch · Invert or open chord voicing",
        "Pitch · Spread voices across instruments",
    ];
    let choices = catalog();
    let mut rows: Vec<_> = [0, 1, 12, 13, 14, 15, 2, 3, 4, 5, 6, 7, 10, 11, 8, 9]
        .into_iter()
        .map(|i| {
            row(
                format!("fx_command:{i}"),
                name(&choices[i]),
                descriptions[i],
            )
        })
        .collect();
    rows.push(row("fx_cancel", "Cancel", ""));
    rows
}

fn catalog() -> Vec<Option<Fx>> {
    vec![
        None,
        Some(Fx::Offset { semitones: 0 }),
        Some(Fx::Velocity { value: 100 }),
        Some(Fx::Gate {
            fraction: Beat::ONE,
        }),
        Some(Fx::Cut {
            after: Beat::new(1, 4).unwrap(),
        }),
        Some(Fx::Delay {
            beats: Beat::new(1, 4).unwrap(),
        }),
        Some(Fx::Retrigger {
            count: 2,
            spacing: Beat::new(1, 8).unwrap(),
        }),
        Some(Fx::Chance { percent: 50 }),
        Some(Fx::Control {
            control: Control::Cc {
                number: 1,
                value: 64,
            },
        }),
        Some(Fx::Send {
            bus: 0,
            amount: 50,
            duration: Beat::ONE,
        }),
        Some(Fx::Jump { step: 0 }),
        Some(Fx::Tempo {
            tempo: Tempo::DEFAULT,
        }),
        Some(Fx::Chord {
            intervals: vec![0, 4, 7],
        }),
        Some(Fx::Add {
            intervals: vec![12],
        }),
        Some(Fx::Voicing {
            inversion: 0,
            open: false,
        }),
        Some(Fx::Spread { voices: 1 }),
    ]
}
fn name(fx: &Option<Fx>) -> &'static str {
    match fx {
        None => "None",
        Some(f) => match f {
            Fx::Offset { .. } => "OFS · Offset",
            Fx::Velocity { .. } => "VEL · Velocity",
            Fx::Gate { .. } => "GAT · Gate",
            Fx::Cut { .. } => "CUT · Cut",
            Fx::Delay { .. } => "DLY · Delay",
            Fx::Retrigger { .. } => "RET · Retrigger",
            Fx::Chance { .. } => "CHN · Chance",
            Fx::Control { .. } => "CTL · MIDI control",
            Fx::Send { .. } => "SND · Temporary send",
            Fx::Jump { .. } => "JMP · Jump",
            Fx::Tempo { .. } => "TMP · Tempo",
            Fx::Chord { .. } | Fx::ChordRevision { .. } => "CRD · Chord",
            Fx::Add { .. } => "ADD · Add intervals",
            Fx::Voicing { .. } => "VOI · Voicing",
            Fx::Spread { .. } => "SPR · Spread",
        },
    }
}
pub(super) fn rows(app: &App) -> Vec<WorkspaceRow> {
    let mut rows = vec![row("fx_kind", "Command", name(&app.workspace.fx))];
    if let Some(f) = &app.workspace.fx {
        if let Fx::ChordRevision { revision } = f {
            if let Some(chord) = app.editor.song().chords.get(revision) {
                rows.push(row(
                    "fx_value",
                    chords::display_name(chord),
                    chords::interval_label(chord.definition().shape().intervals()),
                ));
            }
        } else {
            rows.extend(fields(f));
        }
    }
    rows.extend(commands(&[
        ("chords", "Chord picker"),
        ("fx_apply", "Apply FX"),
        ("fx_cancel", "Cancel"),
    ]));
    rows
}
fn fields(f: &Fx) -> Vec<WorkspaceRow> {
    let (label, value) = match f {
        Fx::Offset { semitones } => ("Semitones", semitones.to_string()),
        Fx::Velocity { value } => ("Velocity", value.to_string()),
        Fx::Gate { fraction } => ("Gate fraction", beat_label(*fraction)),
        Fx::Cut { after } => ("Cut after beats", beat_label(*after)),
        Fx::Delay { beats } => ("Delay beats", beat_label(*beats)),
        Fx::Retrigger { count, .. } => ("Count", count.to_string()),
        Fx::Chance { percent } => ("Chance %", percent.to_string()),
        Fx::Control { control } => ("Control value", format!("{control:?}")),
        Fx::Send { bus, .. } => ("Send bus", bus.to_string()),
        Fx::Jump { step } => ("Step", step.to_string()),
        Fx::Tempo { tempo } => ("BPM", tempo.bpm().to_string()),
        Fx::Chord { intervals } | Fx::Add { intervals } => (
            "Intervals",
            intervals
                .iter()
                .map(i8::to_string)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        Fx::ChordRevision { revision } => ("Saved revision", revision.to_string()),
        Fx::Voicing { inversion, .. } => ("Inversion", inversion.to_string()),
        Fx::Spread { voices } => ("Voices", voices.to_string()),
    };
    let mut rows = vec![row("fx_value", label, value)];
    match f {
        Fx::Retrigger { spacing, .. } => {
            rows.push(row("fx_second", "Spacing (beats)", beat_label(*spacing)))
        }
        Fx::Send {
            amount, duration, ..
        } => rows.extend([
            row("fx_second", "Amount %", amount.to_string()),
            row("fx_third", "Duration (beats)", beat_label(*duration)),
        ]),
        Fx::Voicing { open, .. } => rows.push(row("fx_second", "Open voicing", open.to_string())),
        Fx::Control { control } => {
            rows.push(row(
                "fx_control_kind",
                "Control type",
                format!("{control:?}"),
            ));
            if let Control::Cc { number, .. } = control {
                rows.push(row("fx_second", "CC number", number.to_string()));
            }
            if let Control::PolyPressure { pitch, .. } = control {
                rows.push(row("fx_second", "Pitch", pitch.to_string()));
            }
        }
        _ => {}
    }
    rows
}
pub(super) fn adjust(app: &mut App, id: &str, delta: i32) {
    if id == "fx_kind" {
        let choices = catalog();
        let index = choices
            .iter()
            .position(|choice| name(choice) == name(&app.workspace.fx))
            .unwrap_or(0);
        app.workspace.fx = choices[index
            .saturating_add_signed(delta.signum() as isize)
            .min(choices.len() - 1)]
        .clone();
        return;
    }
    let steps = app.selected_phrase().map_or(1, |(_, p)| p.steps);
    if let Some(fx) = &mut app.workspace.fx {
        adjust_fx(fx, id, delta, steps);
    }
}
fn adjust_fx(fx: &mut Fx, id: &str, d: i32, steps: u32) {
    let first = id == "fx_value";
    match fx {
        Fx::Offset { semitones } if first => *semitones = semitones.saturating_add(d as i8),
        Fx::Velocity { value } if first => *value = (*value as i32 + d).clamp(1, 127) as u8,
        Fx::Gate { fraction } if first => *fraction = step_beat(*fraction, d),
        Fx::Cut { after } if first => *after = step_beat(*after, d),
        Fx::Delay { beats } if first => *beats = step_beat(*beats, d),
        Fx::Retrigger { count, spacing } => {
            if first {
                *count = count.saturating_add_signed(d as i16).max(1)
            } else {
                *spacing = step_beat(*spacing, d)
            }
        }
        Fx::Chance { percent } if first => *percent = (*percent as i32 + d).clamp(0, 100) as u8,
        Fx::Control { control } => adjust_control(control, id, d),
        Fx::Send {
            bus,
            amount,
            duration,
        } => match id {
            "fx_value" => *bus = bus.saturating_add_signed(d),
            "fx_second" => *amount = (*amount as i32 + d).clamp(0, 100) as u8,
            _ => *duration = step_beat(*duration, d),
        },
        Fx::Jump { step } if first => *step = step.saturating_add_signed(d).min(steps - 1),
        Fx::Tempo { tempo } if first => {
            if let Ok(t) = Tempo::new(((tempo.bpm() + d as f64).max(1.0) * 1000.0) as u32) {
                *tempo = t;
            }
        }
        Fx::Voicing { inversion, open } => {
            if first {
                *inversion = inversion.saturating_add_signed(d as i8)
            } else {
                *open = !*open
            }
        }
        Fx::Spread { voices } if first => *voices = voices.saturating_add_signed(d as i8),
        _ => {}
    }
}
pub(super) fn adjust_control(control: &mut Control, id: &str, d: i32) {
    if id == "fx_control_kind" {
        let all = [
            Control::Bend { value: 8192 },
            Control::Pressure { value: 64 },
            Control::PolyPressure {
                pitch: 60,
                value: 64,
            },
            Control::Cc {
                number: 1,
                value: 64,
            },
            Control::Program { value: 0 },
        ];
        let i = all
            .iter()
            .position(|v| std::mem::discriminant(v) == std::mem::discriminant(control))
            .unwrap();
        *control = all[(i as i32 + d).rem_euclid(5) as usize];
        return;
    }
    match control {
        Control::Bend { value } => *value = (*value as i32 + d).clamp(0, 16383) as u16,
        Control::Pressure { value } | Control::Program { value } => {
            *value = (*value as i32 + d).clamp(0, 127) as u8
        }
        Control::Cc { number, value } => {
            let v = if id == "fx_second" { number } else { value };
            *v = (*v as i32 + d).clamp(0, 127) as u8;
        }
        Control::PolyPressure { pitch, value } => {
            let v = if id == "fx_second" { pitch } else { value };
            *v = (*v as i32 + d).clamp(0, 127) as u8;
        }
    }
}
pub(super) fn activate(app: &mut App, id: &str) -> bool {
    if !id.starts_with("fx_") {
        return false;
    }
    if let Some(index) = id
        .strip_prefix("fx_command:")
        .and_then(|v| v.parse::<usize>().ok())
    {
        let Some(choice) = catalog().get(index).cloned() else {
            return true;
        };
        app.workspace.fx = choice;
        // Replacing the command list keeps Back from cycling through two dialogs.
        app.workspace.panel = Some(Panel::Effects);
        app.workspace.focus = Some("fx_value".into());
        cancel_edit(app);
        if matches!(app.workspace.fx, Some(Fx::Chord { .. })) {
            commands::activate(app, "chords");
        }
    } else if id == "fx_kind" {
        open(app, Some(Panel::FxCommands));
    } else if id == "fx_cancel" {
        open(app, None);
    } else if id == "fx_apply" {
        let result = apply(app);
        match result {
            Ok(()) => open(app, None),
            Err(error) => app.status = error.to_string(),
        }
    } else if id == "fx_value" && matches!(app.workspace.fx, Some(Fx::ChordRevision { .. })) {
        commands::activate(app, "chords");
    } else if id == "fx_value"
        && matches!(app.workspace.fx, Some(Fx::Chord { .. } | Fx::Add { .. }))
    {
        let values = match &app.workspace.fx {
            Some(Fx::Chord { intervals } | Fx::Add { intervals }) => intervals
                .iter()
                .map(i8::to_string)
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        };
        text_entry(
            app,
            "fx_intervals",
            "Intervals separated by spaces",
            &values,
        );
    } else {
        adjust(app, id, 1);
    }
    true
}

fn apply(app: &mut App) -> Result<(), crest_tracker::composition::DocumentError> {
    let Some((phrase, source)) = app.selected_phrase() else {
        return Ok(());
    };
    let mut event = if let Some(event) = selected_event(app) {
        event
    } else {
        if app.workspace.fx.is_none() {
            return Ok(());
        }
        Event {
            id: EventId::new(app.editor.song().next_identity()?)?,
            at: if app.is_piano_roll() {
                app.piano_cursor().0
            } else {
                source
                    .division
                    .beat_length()
                    .checked_scale(app.row as u64, 1)?
            },
            target: app.default_target()?,
            note: None,
            controls: vec![],
            fx: [None, None, None],
        }
    };
    event.fx[app.workspace.fx_slot] = app.workspace.fx.clone();
    app.requests.push(Request::Edit {
        revision: app.editor.revision(),
        command: Box::new(Command::PutEvent { phrase, event }),
    });
    Ok(())
}

pub(super) fn numeric_field(app: &App, id: &str) -> bool {
    use Fx::*;
    match (&app.workspace.fx, id) {
        (Some(Chord { .. } | ChordRevision { .. } | Add { .. }), _) => false,
        (Some(Voicing { .. }), "fx_second") => false,
        (Some(_), "fx_value" | "fx_second" | "fx_third") => true,
        _ => false,
    }
}
