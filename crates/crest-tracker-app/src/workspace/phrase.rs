//! Phrase timing and key edits use the domain's lossless retiming commands.
use super::*;
use crest_tracker::time::Division;

pub(super) fn rows(app: &App) -> Vec<WorkspaceRow> {
    let Some((_, p)) = app.selected_phrase() else {
        return commands(&[("back", "Choose a phrase first")]);
    };
    let (notes, time) = p.division.tuplet();
    let mut rows = vec![
        row("phrase_name", "Name", &p.name),
        row(
            "phrase_steps",
            "Steps (preserve notes)",
            p.steps.to_string(),
        ),
        row("phrase_form", "Form", form_name(&p.form)),
        row(
            "division",
            "Division (retime notes and expression)",
            format!("1/{}", p.division.denominator()),
        ),
        row("tuplet_notes", "Tuplet notes", notes.to_string()),
        row("tuplet_time", "Tuplet in time of", time.to_string()),
        row(
            "key_inherit",
            "Phrase key",
            if p.key.is_none() {
                "Inherit Song"
            } else {
                "Local override; select to inherit"
            },
        ),
    ];
    rows.extend(settings::key_rows(
        p.key.as_ref().unwrap_or(&app.editor.song().key),
    ));
    if matches!(
        p.form,
        PhraseForm::Euclidean { .. } | PhraseForm::Random { .. }
    ) {
        rows.push(row("generator", "Generator options", ""));
    }
    rows.push(row("back", "Back", ""));
    rows
}

pub(super) fn form_name(form: &PhraseForm) -> &'static str {
    match form {
        PhraseForm::Tracker => "Tracker",
        PhraseForm::PianoRoll => "Piano Roll",
        PhraseForm::Drum => "Drum",
        PhraseForm::Euclidean { .. } => "Euclidean",
        PhraseForm::Random { .. } => "Random",
    }
}

pub(super) fn adjust(app: &mut App, field: &str, delta: i32) {
    if field.starts_with("key_") {
        settings::change_key(app, field, delta, true);
        return;
    }
    let Some((id, source)) = app.selected_phrase() else {
        return;
    };
    let mut phrase = source.clone();
    let command = match field {
        "phrase_steps" => Command::ResizePhrase {
            phrase: id,
            steps: phrase.steps.saturating_add_signed(delta).max(1),
            trim: false,
        },
        "division" | "tuplet_notes" | "tuplet_time" => {
            let (mut notes, mut time) = phrase.division.tuplet();
            let mut denominator = phrase.division.denominator();
            match field {
                "division" => {
                    denominator = if delta > 0 {
                        (denominator * 2).min(64)
                    } else {
                        (denominator / 2).max(4)
                    }
                }
                "tuplet_notes" => notes = notes.saturating_add_signed(delta as i16).max(1),
                _ => time = time.saturating_add_signed(delta as i16).max(1),
            }
            Command::RetimeDivision {
                phrase: id,
                division: Division::new(denominator, notes, time)
                    .expect("bounded positive division"),
            }
        }
        "phrase_form" => {
            let index = match phrase.form {
                PhraseForm::Tracker => 0,
                PhraseForm::PianoRoll => 1,
                PhraseForm::Drum => 2,
                PhraseForm::Euclidean { .. } => 3,
                PhraseForm::Random { .. } => 4,
            };
            let target = app
                .editor
                .song()
                .instruments
                .keys()
                .next()
                .map(|id| crest_tracker::composition::Target::Instrument {
                    id: *id,
                    channel: crest_tracker::composition::Channel::new(1).unwrap(),
                })
                .unwrap_or(crest_tracker::composition::Target::Channel {
                    endpoint: None,
                    channel: crest_tracker::composition::Channel::new(1).unwrap(),
                });
            phrase.form = match (index + delta).rem_euclid(5) {
                0 => PhraseForm::Tracker,
                1 => PhraseForm::PianoRoll,
                2 => PhraseForm::Drum,
                3 => PhraseForm::Euclidean {
                    pitches: vec![],
                    pulses: 4.min(phrase.steps),
                    rotation: 0,
                    pitch: 60,
                    velocity: 100,
                    gate: Beat::new(9, 10).unwrap(),
                    target,
                },
                _ => PhraseForm::Random {
                    seed: phrase.seed,
                    vary: false,
                    density: 50,
                    low: 48,
                    high: 72,
                    velocity_low: 70,
                    velocity_high: 110,
                    gate: Beat::new(1, 2).unwrap(),
                    gate_high: None,
                    target,
                },
            };
            Command::PutPhrase { id, phrase }
        }
        _ => return,
    };
    app.requests.push(Request::Edit {
        revision: app.editor.revision(),
        command: Box::new(command),
    });
}
