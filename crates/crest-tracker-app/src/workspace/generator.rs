use super::*;
use crest_tracker::composition::{Channel, Target};

pub(super) fn rows(app: &App) -> Vec<WorkspaceRow> {
    let Some((_, p)) = app.selected_phrase() else {
        return commands(&[("back", "Choose a phrase first")]);
    };
    let mut rows = vec![
        row("phrase_form", "Form", phrase::form_name(&p.form)),
        row("phrase_steps", "Steps", p.steps.to_string()),
    ];
    match &p.form {
        PhraseForm::Euclidean {
            pitches,
            pulses,
            rotation,
            pitch,
            velocity,
            gate,
            target,
        } => {
            rows.extend([
                row("gen_pulses", "Pulses", pulses.to_string()),
                row("gen_rotation", "Rotation", rotation.to_string()),
                row("gen_pitch", "Pitch", pitch.to_string()),
                row("gen_velocity", "Velocity", velocity.to_string()),
                row("gen_gate", "Gate (fraction of step)", beat_label(*gate)),
                row("gen_target", "Target", app.target_label(target)),
            ]);
            let key = p.key.as_ref().unwrap_or(&app.editor.song().key);
            rows.extend((0..*pulses as usize).map(|index| {
                let value = pitches.get(index).copied().flatten().unwrap_or(*pitch);
                row(
                    format!("gen_note:{index}"),
                    format!("Pulse {:02}", index + 1),
                    key.pitch_name(crest_tracker::harmony::Pitch::new(value).expect("valid pitch")),
                )
            }));
        }
        PhraseForm::Random {
            seed,
            vary,
            density,
            low,
            high,
            velocity_low,
            velocity_high,
            gate,
            gate_high,
            target,
        } => rows.extend([
            row("gen_seed", "Seed", seed.to_string()),
            row(
                "gen_repeat",
                "Repeat behavior",
                if *vary {
                    "New each loop"
                } else {
                    "Keep generated notes"
                },
            ),
            row("gen_density", "Density %", density.to_string()),
            row("gen_low", "Lowest pitch", low.to_string()),
            row("gen_high", "Highest pitch", high.to_string()),
            row(
                "gen_velocity_low",
                "Minimum velocity",
                velocity_low.to_string(),
            ),
            row(
                "gen_velocity_high",
                "Maximum velocity",
                velocity_high.to_string(),
            ),
            row("gen_gate", "Minimum gate", beat_label(*gate)),
            row(
                "gen_gate_high",
                "Maximum gate",
                beat_label(gate_high.unwrap_or(*gate)),
            ),
            row("gen_target", "Target", app.target_label(target)),
        ]),
        _ => {}
    }
    rows.extend(commands(&[
        ("gen_regenerate", "Generate"),
        ("gen_bake_tracker", "Use notes in new Tracker phrase"),
        ("gen_bake_piano", "Use notes in new Piano Roll phrase"),
        ("back", "Back"),
    ]));
    rows
}
pub(super) fn beat_label(beat: Beat) -> String {
    format!("{}/{}", beat.numerator(), beat.denominator())
}
pub(super) fn step_beat(beat: Beat, delta: i32) -> Beat {
    let amount = Beat::new(delta.unsigned_abs() as u64, 16).unwrap();
    if delta < 0 {
        beat.checked_sub(amount)
            .ok()
            .filter(|b| !b.is_zero())
            .unwrap_or(Beat::new(1, 16).unwrap())
    } else {
        beat.checked_add(amount).unwrap_or(beat)
    }
}
pub(super) fn next_target(app: &App, current: &Target, delta: i32) -> Target {
    let mut choices: Vec<_> = app
        .editor
        .song()
        .instruments
        .keys()
        .map(|id| Target::Instrument {
            id: *id,
            channel: Channel::FIRST,
        })
        .collect();
    if let Some(endpoints) = app.outputs["endpoints"].as_array() {
        choices.extend(endpoints.iter().filter_map(|v| v["id"].as_str()).map(|id| {
            Target::Channel {
                endpoint: Some(id.into()),
                channel: Channel::FIRST,
            }
        }));
    }
    choices.push(Target::Channel {
        endpoint: None,
        channel: Channel::FIRST,
    });
    let index = choices.iter().position(|v| v == current).unwrap_or(0);
    choices[(index as i32 + delta).rem_euclid(choices.len() as i32) as usize].clone()
}
pub(super) fn adjust(app: &mut App, field: &str, delta: i32) {
    if matches!(field, "phrase_form" | "phrase_steps" | "division") {
        phrase::adjust(app, field, delta);
        return;
    }
    let Some((id, p)) = app.selected_phrase() else {
        return;
    };
    let mut phrase = p.clone();
    match &mut phrase.form {
        PhraseForm::Euclidean {
            pitches,
            pulses,
            rotation,
            pitch,
            velocity,
            gate,
            target,
        } => match field {
            "gen_pulses" => *pulses = pulses.saturating_add_signed(delta).min(phrase.steps),
            "gen_rotation" => {
                *rotation = (*rotation as i64 + delta as i64).rem_euclid(phrase.steps as i64) as u32
            }
            "gen_pitch" => *pitch = ((*pitch as i32) + delta).clamp(0, 127) as u8,
            "gen_velocity" => *velocity = ((*velocity as i32) + delta).clamp(1, 127) as u8,
            "gen_gate" => *gate = step_beat(*gate, delta),
            "gen_target" => *target = next_target(app, target, delta),
            _ if field.starts_with("gen_note:") => {
                let Some(index) = field
                    .strip_prefix("gen_note:")
                    .and_then(|v| v.parse::<usize>().ok())
                    .filter(|i| *i < *pulses as usize)
                else {
                    return;
                };
                pitches.resize(pitches.len().max(index + 1), None);
                pitches[index] =
                    Some((i32::from(pitches[index].unwrap_or(*pitch)) + delta).clamp(0, 127) as u8);
            }
            _ => return,
        },
        PhraseForm::Random {
            seed,
            vary,
            density,
            low,
            high,
            velocity_low,
            velocity_high,
            gate,
            gate_high,
            target,
        } => match field {
            "gen_repeat" => *vary = !*vary,
            "gen_seed" | "gen_regenerate" => *seed = seed.wrapping_add_signed(delta as i64),
            "gen_density" => *density = ((*density as i32) + delta).clamp(0, 100) as u8,
            "gen_low" => *low = ((*low as i32) + delta).clamp(0, *high as i32) as u8,
            "gen_high" => *high = ((*high as i32) + delta).clamp(*low as i32, 127) as u8,
            "gen_velocity_low" => {
                *velocity_low =
                    ((*velocity_low as i32) + delta).clamp(1, *velocity_high as i32) as u8
            }
            "gen_velocity_high" => {
                *velocity_high =
                    ((*velocity_high as i32) + delta).clamp(*velocity_low as i32, 127) as u8
            }
            "gen_gate" => {
                *gate = step_beat(*gate, delta);
                if gate_high.is_some_and(|v| v < *gate) {
                    *gate_high = Some(*gate);
                }
            }
            "gen_gate_high" => {
                *gate_high = Some(step_beat(gate_high.unwrap_or(*gate), delta).max(*gate))
            }
            "gen_target" => *target = next_target(app, target, delta),
            _ => return,
        },
        _ => {
            app.status = "Choose Euclidean or Random form first".into();
            return;
        }
    }
    app.requests.push(Request::Edit {
        revision: app.editor.revision(),
        command: Box::new(Command::PutPhrase { id, phrase }),
    });
}
pub(super) fn activate(app: &mut App, id: &str) -> bool {
    if id == "gen_notes" {
        open(app, Some(Panel::Generator));
        app.workspace.focus = Some("gen_note:0".into());
        return true;
    }
    if matches!(id, "gen_range" | "gen_velocity_range") {
        open(app, Some(Panel::Generator));
        app.workspace.focus = Some(
            if id == "gen_range" {
                "gen_low"
            } else {
                "gen_velocity_low"
            }
            .into(),
        );
        return true;
    }
    if id == "gen_scale" {
        open(app, Some(Panel::PhraseSettings));
        app.workspace.focus = Some("key_scale".into());
        return true;
    }
    if id == "gen_apply" {
        app.requests.push(Request::StartAt {
            scope: crate::app::TransportScope::Phrase,
            at: Beat::ZERO,
        });
        return true;
    }
    if !id.starts_with("gen_") {
        return false;
    }
    if matches!(id, "gen_bake_tracker" | "gen_bake_piano") {
        if let Some(occurrence) = app.occurrence() {
            app.requests.push(Request::BakeGenerator {
                revision: app.editor.revision(),
                occurrence,
                form: if id == "gen_bake_tracker" {
                    crate::app::BakeForm::Tracker
                } else {
                    crate::app::BakeForm::PianoRoll
                },
            });
            open(app, None);
        }
    } else {
        adjust(app, id, 1);
    }
    true
}

pub(super) fn active(app: &App) -> bool {
    app.view == View::Phrase
        && app.selected_phrase().is_some_and(|(_, phrase)| {
            matches!(
                phrase.form,
                PhraseForm::Euclidean { .. } | PhraseForm::Random { .. }
            )
        })
}

pub(super) fn project(app: &App, view: &mut WorkspaceProjection) {
    let (_, phrase) = app.selected_phrase().expect("generator phrase");
    let key = phrase.key.as_ref().unwrap_or(&app.editor.song().key);
    let pitch_name = |pitch| {
        crest_tracker::harmony::Pitch::new(pitch)
            .map(|pitch| key.pitch_name(pitch))
            .unwrap_or_default()
    };
    view.context = format!("PHRASE {}", phrase.name);
    view.title = match phrase.form {
        PhraseForm::Euclidean { .. } => "EUCLIDEAN",
        _ => "RANDOMIZER",
    }
    .into();
    view.subtitle = format!(
        "{} · {} STEPS · 1/{}",
        key.name(),
        phrase.steps,
        phrase.division.denominator()
    );
    let all = rows(app);
    view.rows = match &phrase.form {
        PhraseForm::Euclidean { .. } => {
            let mut rows: Vec<_> = [
                "gen_pulses",
                "phrase_steps",
                "gen_rotation",
                "gen_pitch",
                "gen_velocity",
            ]
            .into_iter()
            .filter_map(|id| all.iter().find(|row| row.id == id).cloned())
            .collect();
            if let PhraseForm::Euclidean { pitch, .. } = phrase.form {
                rows[3].value = pitch_name(pitch);
                rows[3].label = "Note".into();
            }
            rows.push(row(
                "division",
                "Division",
                format!("1/{}", phrase.division.denominator()),
            ));
            rows.push(row("gen_notes", "Pulse notes", "Assign individual pitches"));
            rows.push(row("gen_apply", "Apply rhythm", "Preview phrase"));
            rows
        }
        PhraseForm::Random {
            low,
            high,
            velocity_low,
            velocity_high,
            ..
        } => {
            vec![
                row("gen_scale", "Scale", key.name()),
                row(
                    "gen_range",
                    "Note range",
                    format!("{} – {}", pitch_name(*low), pitch_name(*high)),
                ),
                all.iter()
                    .find(|row| row.id == "gen_density")
                    .unwrap()
                    .clone(),
                row(
                    "gen_velocity_range",
                    "Velocity range",
                    format!("{velocity_low} – {velocity_high}"),
                ),
                all.iter().find(|row| row.id == "gen_seed").unwrap().clone(),
                all.iter()
                    .find(|row| row.id == "gen_repeat")
                    .unwrap()
                    .clone(),
                row("gen_regenerate", "Generate", "New preview"),
            ]
        }
        _ => unreachable!(),
    };
    // Preview is compiled by the same occurrence scheduler used for playback.
    let (_, _, _, grid) = app.grid();
    let cells = grid
        .values
        .into_iter()
        .take(32)
        .enumerate()
        .map(|(step, values)| {
            let note = values.first().cloned().unwrap_or_default();
            let mut cell = row(
                format!("preview:{step}"),
                "",
                if note == "—" {
                    "·".into()
                } else {
                    note.clone()
                },
            );
            cell.selected = note != "—";
            cell.playing = app.playing && app.play_row == Some(step as u32);
            cell
        })
        .collect::<Vec<_>>();
    view.columns = (0..cells.len()).map(|step| format!("{step:02}")).collect();
    view.grid = vec![WorkspaceGridRow {
        id: "preview".into(),
        label: "PREVIEW".into(),
        cells,
        playing: false,
    }];
    view.compact_grid = true;
    view.inspector = commands(&[
        ("gen_apply", "Play preview"),
        ("generator", "More options"),
        ("gen_bake_tracker", "Use notes in Tracker"),
    ]);
    view.hints = "D-pad: field · Edit+←/→: fine · Edit+↑/↓: coarse · Edit: choose / preview · Shift+Up: phrase settings · Shift+Left: Chain".into();
}
