//! The visible Piano Roll tour and headless DSP witness share semantic events.
use super::*;

pub(super) enum Checkpoint {
    Roll,
    Grid,
    Note {
        at: Beat,
        duration: Beat,
        velocity: u8,
    },
    Preview,
    Expression,
    Channel(usize),
}

pub(super) fn check(app: &App, checkpoint: &Checkpoint) -> bool {
    let view = crate::workspace::project(app).workspace;
    match checkpoint {
        Checkpoint::Roll => {
            app.is_piano_roll() && view.is_some_and(|view| !view.overlay && view.timeline.is_some())
        }
        Checkpoint::Grid => app.roll_grid == beat(1, 3) && app.roll_new_length == Beat::ONE,
        Checkpoint::Note {
            at,
            duration,
            velocity,
        } => app.selected_phrase().is_some_and(|(_, phrase)| {
            phrase.events.len() == 1
                && phrase.events[0].at == *at
                && phrase.events[0]
                    .note
                    .as_ref()
                    .is_some_and(|note| note.duration == *duration && note.velocity == *velocity)
        }),
        Checkpoint::Preview => view.and_then(|view| view.timeline).is_some_and(|timeline| {
            timeline.originals.len() == 1
                && timeline.spans.len() == 1
                && timeline.originals[0].start != timeline.spans[0].start
        }),
        Checkpoint::Expression => view
            .and_then(|view| view.timeline)
            .and_then(|timeline| timeline.curve)
            .is_some_and(|curve| {
                curve.label == "Pressure"
                    && curve.points.len() == 2
                    && curve
                        .points
                        .iter()
                        .all(|point| point.label.ends_with("117"))
            }),
        Checkpoint::Channel(count) => app.selected_phrase().is_some_and(|(_, phrase)| {
            let controls: Vec<_> = phrase
                .events
                .iter()
                .flat_map(|event| &event.controls)
                .collect();
            let note = phrase.events.iter().find_map(|event| event.note.as_ref());
            controls.len() == *count
                && controls.iter().all(|control| {
                    **control == crest_tracker::composition::Control::Pressure { value: 53 }
                })
                && note.is_some_and(|note| {
                    note.expression.len() == 2
                        && note.expression.iter().all(|point| {
                            point.control
                                == crest_tracker::composition::Control::Pressure { value: 117 }
                        })
                })
        }),
    }
}
fn beat(numerator: u64, denominator: u64) -> Beat {
    Beat::new(numerator, denominator).unwrap()
}

pub(super) fn scene() -> Scene {
    use Check::*;
    use Checkpoint::{Channel, Expression, Grid, Preview, Roll};
    use Direction::*;
    use Event::{Action, Activate, Focus, Submit, Wait};
    use SemanticAction::{Adjust, Navigate, NavigatePage, PreviewStart, SetInteractionMode};
    let edit = || Action(SetInteractionMode(InteractionMode::Adjust));
    let release = || Action(SetInteractionMode(InteractionMode::Navigate));
    let note = |at, duration, velocity| {
        Piano(Checkpoint::Note {
            at,
            duration,
            velocity,
        })
    };
    let mut scene = Scene::new();
    scene.report.scene = "piano-roll";
    scene.steps = vec![
        ("Application ready", Wait, Ready),
        ("Hold Edit", edit(), Held(true)),
        ("Create chain", Action(Adjust(Right)), Chain),
        ("Release Edit", release(), Held(false)),
        (
            "Enter chain",
            Action(NavigatePage(Right)),
            View(crate::app::View::Chain),
        ),
        ("Hold Edit", edit(), Held(true)),
        ("Create phrase", Action(Adjust(Right)), Phrase),
        ("Release Edit", release(), Held(false)),
        (
            "Enter phrase",
            Action(NavigatePage(Right)),
            View(crate::app::View::Phrase),
        ),
        (
            "Phrase settings",
            Action(NavigatePage(Up)),
            Panel("phrase_form"),
        ),
        (
            "Choose Piano Roll",
            Activate("phrase_form"),
            Panel("phrase_form"),
        ),
        ("Return to roll", Activate("back"), Piano(Roll)),
        ("Step Grid", Activate("piano_grid"), Panel("piano_triplet")),
        (
            "Triplet grid",
            Activate("piano_triplet"),
            Panel("piano_time:8"),
        ),
        (
            "Choose 1/8T",
            Activate("piano_time:8"),
            Panel("piano_time_apply"),
        ),
        ("Apply Step Grid", Activate("piano_time_apply"), Piano(Roll)),
        (
            "Independent New Length",
            Activate("piano_new_length"),
            Panel("piano_time:4"),
        ),
        (
            "Quarter-note default",
            Activate("piano_time:4"),
            Panel("piano_time_apply"),
        ),
        (
            "Apply New Length",
            Activate("piano_time_apply"),
            Piano(Grid),
        ),
        (
            "Insert with controller Edit",
            Action(SemanticAction::Activate),
            note(Beat::ZERO, Beat::ONE, 100),
        ),
        ("Move by triplet grid", Action(Navigate(Right)), Piano(Roll)),
        (
            "Popover opens Tools",
            Action(SemanticAction::OpenPopover),
            Panel("piano_inspect"),
        ),
        (
            "Inspect selected note",
            Activate("piano_inspect"),
            Panel("piano_start"),
        ),
        ("Exact start", Activate("piano_start"), Panel("text_submit")),
        (
            "Preview off-grid start",
            Submit("piano_start", "7/20"),
            Panel("piano_length"),
        ),
        (
            "Selected length",
            Activate("piano_length"),
            Panel("piano_triplet"),
        ),
        (
            "Triplet duration",
            Activate("piano_triplet"),
            Panel("piano_time:8"),
        ),
        (
            "Choose 1/8T duration",
            Activate("piano_time:8"),
            Panel("piano_time_apply"),
        ),
        (
            "Preview selected duration",
            Activate("piano_time_apply"),
            Panel("piano_velocity_value"),
        ),
        (
            "Exact raw velocity",
            Activate("piano_velocity_value"),
            Panel("text_submit"),
        ),
        (
            "Preview velocity 101",
            Submit("piano_velocity_value", "101"),
            note(Beat::ZERO, Beat::ONE, 100),
        ),
        (
            "Apply note edit once",
            Activate("piano_apply"),
            note(beat(7, 20), beat(1, 3), 101),
        ),
        (
            "Quantize preview",
            Activate("piano_quantize"),
            Piano(Preview),
        ),
        (
            "Independent Quantize Grid",
            Activate("piano_quantize_grid"),
            Panel("piano_triplet"),
        ),
        (
            "Quantize triplets",
            Activate("piano_triplet"),
            Panel("piano_time:8"),
        ),
        (
            "Choose 1/8T quantize",
            Activate("piano_time:8"),
            Panel("piano_time_apply"),
        ),
        (
            "Return to quantize preview",
            Activate("piano_time_apply"),
            Piano(Preview),
        ),
        (
            "Cancel keeps exact original",
            Activate("piano_close"),
            note(beat(7, 20), beat(1, 3), 101),
        ),
        (
            "Reopen quantize",
            Activate("piano_quantize"),
            Piano(Preview),
        ),
        (
            "Apply quantize",
            Activate("piano_apply"),
            note(beat(1, 3), beat(1, 3), 101),
        ),
        ("History", Activate("notes_history"), Panel("undo")),
        (
            "Undo restores exact onset",
            Activate("undo"),
            note(beat(7, 20), beat(1, 3), 101),
        ),
        (
            "Redo restores quantize",
            Activate("redo"),
            note(beat(1, 3), beat(1, 3), 101),
        ),
        ("Back to roll", Activate("back"), Piano(Roll)),
        ("Tools", Activate("notes"), Panel("expression")),
        ("Expression lane", Activate("expression"), Panel("expr_new")),
        (
            "Add owned pressure point",
            Activate("expr_new"),
            Panel("expr_value"),
        ),
        (
            "Exact pressure",
            Activate("expr_value"),
            Panel("text_submit"),
        ),
        (
            "Raw pressure 117",
            Submit("expr_value", "117"),
            Panel("expr_new"),
        ),
        (
            "Add next pressure point",
            Activate("expr_new"),
            Panel("expr_offset"),
        ),
        (
            "Exact relative time",
            Activate("expr_offset"),
            Panel("text_submit"),
        ),
        (
            "Pressure at 1/12 beat",
            Submit("expr_offset", "1/12"),
            Piano(Expression),
        ),
        (
            "Focus expression time",
            Focus("expr_offset"),
            Piano(Expression),
        ),
        ("Coarse time edit", Action(Adjust(Up)), Piano(Expression)),
        ("Undo expression edit", Activate("undo"), Piano(Expression)),
        (
            "Separate channel scope",
            Activate("channel_expression"),
            Panel("channel_new"),
        ),
        (
            "Add channel point",
            Activate("channel_new"),
            Panel("channel_value"),
        ),
        (
            "Exact channel pressure",
            Activate("channel_value"),
            Panel("text_submit"),
        ),
        (
            "Channel value leaves note expression intact",
            Submit("channel_value", "53"),
            Piano(Channel(1)),
        ),
        (
            "Delete only channel point",
            Activate("channel_delete"),
            Piano(Channel(0)),
        ),
        (
            "Undo restores channel point",
            Activate("undo"),
            Piano(Channel(1)),
        ),
        (
            "Return to note expression",
            Activate("back"),
            Piano(Expression),
        ),
        ("Back to Tools", Activate("back"), Panel("piano_inspect")),
        ("Back to roll", Activate("piano_close"), Piano(Roll)),
        (
            "Play through production Synth",
            Action(PreviewStart),
            FullPlayback,
        ),
        ("Audible phrase advances", Wait, Progress(2)),
        ("Stop releases owned voices", Action(PreviewStart), Stopped),
    ]
    .into_iter()
    .map(|(label, event, check)| Step {
        label,
        event,
        check,
    })
    .collect();
    scene
}
