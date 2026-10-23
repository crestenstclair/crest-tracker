//! Feature tours use the same semantic events, visible controls and DSP as users.
use super::*;
use crest_tracker::composition::{Fx, PhraseForm};

type Script = Vec<(&'static str, Event, Check)>;
pub(super) enum Checkpoint {
    Form(&'static str),
    Cursor(usize, usize),
    Notes(usize),
    Hit { at: Beat, pitch: u8, velocity: u8 },
    Euclidean,
    Random { density: u8, vary: bool },
    Baked { form: &'static str, notes: usize },
    Chord(bool),
    Draft,
    Published,
    Fx(Option<Fx>),
    Off,
    Focus(&'static str),
    DefaultTempo,
    ChainName,
    Padding(crest_tracker::composition::SpecialChain, u32),
}

pub(super) fn check(app: &App, check: &Checkpoint) -> bool {
    let phrase = app.selected_phrase().map(|(_, p)| p);
    let notes = || phrase.map_or(0, |p| p.events.iter().filter(|e| e.note.is_some()).count());
    match check {
        Checkpoint::Padding(kind, measures) => {
            app.selected_special() == Some(*kind)
                && app.editor.song().tracks[app.track].slots[app.slot]
                    .as_ref()
                    .is_some_and(|slot| slot.repeats == *measures)
        }
        Checkpoint::Form(form) => phrase.is_some_and(|p| form_name(&p.form) == *form),
        Checkpoint::Cursor(row, column) => app.row == *row && app.column == *column,
        Checkpoint::Notes(count) => notes() == *count,
        Checkpoint::Hit {
            at,
            pitch,
            velocity,
        } => phrase.is_some_and(|p| {
            p.events.iter().any(|e| {
                e.at == *at
                    && e.note
                        .as_ref()
                        .is_some_and(|n| n.pitch == *pitch && n.velocity == *velocity)
            })
        }),
        Checkpoint::Euclidean => phrase.is_some_and(|p| {
            matches!(&p.form,
            PhraseForm::Euclidean { pulses: 5, pitches, .. } if pitches.first() == Some(&Some(72)))
        }),
        Checkpoint::Random { density, vary } => phrase.is_some_and(|p| {
            matches!(&p.form,
            PhraseForm::Random { density: d, vary: v, .. } if d == density && v == vary)
        }),
        Checkpoint::Baked { form, notes: count } => {
            phrase.is_some_and(|p| form_name(&p.form) == *form)
                && notes() == *count
                && app.editor.song().phrases.len() == 2
                && app.editor.song().phrases.values().any(|p| {
                    matches!(
                        p.form,
                        PhraseForm::Euclidean { .. } | PhraseForm::Random { .. }
                    )
                })
        }
        Checkpoint::Chord(present) => phrase.is_some_and(|p| {
            p.events.first().is_some_and(|e| {
                let chord = e.fx.iter().flatten().find_map(|fx| {
                    if let Fx::ChordRevision { revision } = fx {
                        app.editor.song().chords.get(revision)
                    } else {
                        None
                    }
                });
                chord.is_some() == *present
                    && chord
                        .is_none_or(|chord| chord.definition().shape().intervals() == [0, 4, 7, 11])
            })
        }),
        Checkpoint::Draft => crate::workspace::project(app)
            .workspace
            .is_some_and(|view| {
                [
                    ("chord_key:3", "On"),
                    ("chord_key:4", "Off"),
                    ("chord_inversion", "1"),
                    ("chord_open", "On"),
                    ("chord_spread", "1"),
                ]
                .into_iter()
                .all(|(id, value)| {
                    view.rows
                        .iter()
                        .any(|row| row.id == id && row.value == value)
                })
            }),
        Checkpoint::Published => {
            app.chord_library["loading"] == false
                && app.chord_library["entries"]
                    .as_array()
                    .is_some_and(|entries| {
                        entries.len() == 1 && entries[0]["chord"]["name"] == "Scene minor"
                    })
        }
        Checkpoint::Fx(expected) => {
            phrase.is_some_and(|p| p.events.first().is_some_and(|e| e.fx[0] == *expected))
        }
        Checkpoint::Off => phrase.is_some_and(|p| {
            p.events
                .iter()
                .any(|e| e.is_note_off() && e.at == Beat::new(1, 4).unwrap())
        }),
        Checkpoint::Focus(id) => crate::workspace::project(app)
            .workspace
            .is_some_and(|v| v.focus == *id),
        Checkpoint::DefaultTempo => {
            app.preferences["default_bpm"] == 130.0 && app.editor.song().tempo.bpm() == 120.0
        }
        Checkpoint::ChainName => app
            .selected_chain()
            .is_some_and(|(_, c)| c.name == "Scene chain"),
    }
}
fn form_name(form: &PhraseForm) -> &'static str {
    match form {
        PhraseForm::Tracker => "Tracker",
        PhraseForm::PianoRoll => "Piano Roll",
        PhraseForm::Drum => "Drum",
        PhraseForm::Euclidean { .. } => "Euclidean",
        PhraseForm::Random { .. } => "Random",
    }
}
fn feature(check: Checkpoint) -> Check {
    Check::Feature(check)
}

fn phrase() -> Script {
    use Check::*;
    use Event::*;
    use SemanticAction::*;
    vec![
        ("Application ready", Wait, Ready),
        (
            "Create chain with Edit+Right",
            Action(Adjust(Direction::Right)),
            Chain,
        ),
        (
            "Enter chain",
            Action(NavigatePage(Direction::Right)),
            View(crate::app::View::Chain),
        ),
        (
            "Create phrase with Edit+Right",
            Action(Adjust(Direction::Right)),
            Phrase,
        ),
        (
            "Enter phrase",
            Action(NavigatePage(Direction::Right)),
            View(crate::app::View::Phrase),
        ),
    ]
}
fn choose_form(steps: &mut Script, index: usize, name: &'static str) {
    steps.push((
        "Open phrase settings",
        Event::Action(SemanticAction::NavigatePage(Direction::Up)),
        Check::Panel("phrase_form"),
    ));
    for _ in 0..index {
        steps.push((
            "Choose phrase form",
            Event::Activate("phrase_form"),
            Check::Panel("phrase_form"),
        ));
    }
    steps.push((
        "Return to phrase",
        Event::Activate("back"),
        feature(Checkpoint::Form(name)),
    ));
}
fn playback(steps: &mut Script) {
    steps.extend([
        (
            "Play through production Synth",
            Event::Action(SemanticAction::PreviewStart),
            Check::FullPlayback,
        ),
        (
            "Audible transport advances",
            Event::Wait,
            Check::Progress(2),
        ),
        (
            "Stop releases every voice",
            Event::Action(SemanticAction::PreviewStart),
            Check::Stopped,
        ),
    ]);
}
fn history(steps: &mut Script) {
    steps.extend([
        (
            "Open note tools",
            Event::Activate("notes"),
            Check::Panel("notes_history"),
        ),
        (
            "Open history",
            Event::Activate("notes_history"),
            Check::Panel("undo"),
        ),
    ]);
}

pub(super) fn scene(name: &str) -> Option<Scene> {
    Some(match name {
        "drums" => drums(),
        "euclidean" => euclidean(),
        "random" => random(),
        "chords" => chords(),
        "fx" => effects(),
        "navigation" => navigation(),
        "rest-kill" => rest_kill(),
        _ => return None,
    })
}

fn drums() -> Scene {
    use Check::*;
    use Checkpoint::*;
    use Direction::*;
    use Event::{Action, Activate};
    use SemanticAction::{Adjust, Navigate};
    let mut steps = phrase();
    choose_form(&mut steps, 2, "Drum");
    steps.extend([
        (
            "Toggle kick step 00",
            Action(SemanticAction::Activate),
            feature(Notes(1)),
        ),
        (
            "Right moves time",
            Action(Navigate(Right)),
            feature(Cursor(1, 0)),
        ),
        (
            "Down moves lane",
            Action(Navigate(Down)),
            feature(Cursor(1, 1)),
        ),
        (
            "Toggle snare step 01",
            Action(SemanticAction::Activate),
            feature(Notes(2)),
        ),
        (
            "Coarse velocity edit",
            Action(Adjust(Up)),
            feature(Hit {
                at: Beat::new(1, 4).unwrap(),
                pitch: 38,
                velocity: 110,
            }),
        ),
    ]);
    history(&mut steps);
    steps.extend([
        (
            "Undo velocity only",
            Activate("undo"),
            feature(Hit {
                at: Beat::new(1, 4).unwrap(),
                pitch: 38,
                velocity: 100,
            }),
        ),
        (
            "Redo velocity",
            Activate("redo"),
            feature(Hit {
                at: Beat::new(1, 4).unwrap(),
                pitch: 38,
                velocity: 110,
            }),
        ),
        ("Back to tools", Activate("back"), Panel("notes_history")),
        ("Back to drum grid", Activate("back"), feature(Form("Drum"))),
        (
            "Toggle only selected hit off",
            Action(SemanticAction::Activate),
            feature(Notes(1)),
        ),
        (
            "Restore selected hit",
            Action(SemanticAction::Activate),
            feature(Notes(2)),
        ),
    ]);
    playback(&mut steps);
    Scene::script("drums", steps)
}

fn euclidean() -> Scene {
    use Check::*;
    use Checkpoint::*;
    use Event::{Action, Activate, Focus};
    let mut steps = phrase();
    choose_form(&mut steps, 3, "Euclidean");
    steps.extend([
        (
            "Set five pulses",
            Activate("gen_pulses"),
            Panel("gen_notes"),
        ),
        (
            "Individual pulse assignment",
            Activate("gen_notes"),
            Panel("gen_note:0"),
        ),
        (
            "Select first pulse",
            Focus("gen_note:0"),
            Panel("gen_note:0"),
        ),
        (
            "Up transposes assigned pulse by octave",
            Action(SemanticAction::Adjust(Direction::Up)),
            feature(Euclidean),
        ),
        ("Back to rhythm", Activate("back"), feature(Euclidean)),
    ]);
    playback(&mut steps);
    steps.extend([
        (
            "Open settings for bake",
            Action(SemanticAction::NavigatePage(Direction::Up)),
            Panel("generator"),
        ),
        (
            "Generator settings",
            Activate("generator"),
            Panel("gen_bake_piano"),
        ),
        (
            "Bake as separate Piano Roll",
            Activate("gen_bake_piano"),
            feature(Baked {
                form: "Piano Roll",
                notes: 5,
            }),
        ),
    ]);
    steps.extend([
        ("Open Piano Roll tools", Activate("notes"), Panel("undo")),
        (
            "Undo restores generator occurrence",
            Activate("undo"),
            feature(Euclidean),
        ),
        (
            "Open history in generator context",
            Activate("notes_history"),
            Panel("redo"),
        ),
        (
            "Redo restores baked notes",
            Activate("redo"),
            feature(Baked {
                form: "Piano Roll",
                notes: 5,
            }),
        ),
        ("Back to tools", Activate("back"), Panel("piano_close")),
        (
            "Back to roll",
            Activate("piano_close"),
            feature(Form("Piano Roll")),
        ),
    ]);
    playback(&mut steps);
    Scene::script("euclidean", steps)
}

fn random() -> Scene {
    use Check::*;
    use Checkpoint::*;
    use Event::{Action, Activate, Focus};
    let mut steps = phrase();
    choose_form(&mut steps, 4, "Random");
    steps.push(("Focus density", Focus("gen_density"), Panel("gen_density")));
    for density in [60, 70, 80, 90, 100] {
        steps.push((
            "Up changes density by ten",
            Action(SemanticAction::Adjust(Direction::Up)),
            feature(Random {
                density,
                vary: false,
            }),
        ));
    }
    steps.extend([
        (
            "Generate fresh deterministic seed",
            Activate("gen_regenerate"),
            feature(Random {
                density: 100,
                vary: false,
            }),
        ),
        (
            "Vary generated notes each loop",
            Activate("gen_repeat"),
            feature(Random {
                density: 100,
                vary: true,
            }),
        ),
        (
            "Keep generated notes for reproducible bake",
            Activate("gen_repeat"),
            feature(Random {
                density: 100,
                vary: false,
            }),
        ),
    ]);
    playback(&mut steps);
    steps.extend([
        (
            "Open phrase settings",
            Action(SemanticAction::NavigatePage(Direction::Up)),
            Panel("generator"),
        ),
        (
            "Generator settings",
            Activate("generator"),
            Panel("gen_bake_tracker"),
        ),
        (
            "Bake sixteen notes into new Tracker phrase",
            Activate("gen_bake_tracker"),
            feature(Baked {
                form: "Tracker",
                notes: 16,
            }),
        ),
    ]);
    playback(&mut steps);
    Scene::script("random", steps)
}

fn chords() -> Scene {
    use Check::*;
    use Checkpoint::*;
    use Event::{Action, Activate, Focus, Submit};
    let mut steps = phrase();
    steps.extend([
        (
            "Insert root note",
            Action(SemanticAction::Activate),
            feature(Notes(1)),
        ),
        (
            "Open shared chord picker",
            Activate("chords"),
            Panel("chords_cancel"),
        ),
        (
            "Cancel leaves root unchanged",
            Activate("chords_cancel"),
            feature(Chord(false)),
        ),
        (
            "Reopen chord picker",
            Activate("chords"),
            Panel("chord:default:9"),
        ),
        (
            "Apply major seventh intervals",
            Activate("chord:default:9"),
            feature(Chord(true)),
        ),
    ]);
    playback(&mut steps);
    steps.push((
        "Chord rendered four simultaneous voices",
        Event::Wait,
        Check::PeakVoices(4),
    ));
    history(&mut steps);
    steps.extend([
        (
            "Undo chord assignment",
            Activate("undo"),
            feature(Chord(false)),
        ),
        (
            "Redo chord assignment",
            Activate("redo"),
            feature(Chord(true)),
        ),
        ("Back to tools", Activate("back"), Panel("notes_history")),
        ("Back to Phrase", Activate("back"), feature(Chord(true))),
        (
            "Main menu",
            Action(SemanticAction::NavigatePage(Direction::Down)),
            Panel("chord_library"),
        ),
        (
            "Separate chord editor library",
            Activate("chord_library"),
            Panel("chord_new"),
        ),
        ("Create draft", Activate("chord_new"), Panel("chord_key:4")),
        (
            "Remove major third",
            Activate("chord_key:4"),
            Panel("chord_key:3"),
        ),
        (
            "Add minor third",
            Activate("chord_key:3"),
            Panel("chord_octave"),
        ),
        (
            "Focus interval octave",
            Focus("chord_octave"),
            Panel("chord_octave"),
        ),
        (
            "Right reaches voicing column",
            Action(SemanticAction::Navigate(Direction::Right)),
            feature(Checkpoint::Focus("chord_inversion")),
        ),
        (
            "Set first inversion",
            Activate("chord_inversion"),
            Panel("chord_open"),
        ),
        (
            "Open voicing",
            Activate("chord_open"),
            Panel("chord_spread"),
        ),
        ("Spread voices", Activate("chord_spread"), feature(Draft)),
        ("Name draft", Activate("chord_name"), Panel("text_submit")),
        (
            "Set library name",
            Submit("chord_name", "Scene minor"),
            feature(Draft),
        ),
        (
            "Publish only to isolated scene library",
            Activate("chord_publish"),
            feature(Published),
        ),
        ("Back to main menu", Activate("back"), Panel("home")),
        (
            "Shift Up restores previous Phrase",
            Action(SemanticAction::NavigatePage(Direction::Up)),
            View(crate::app::View::Phrase),
        ),
    ]);
    Scene::script("chords", steps)
}

fn effects() -> Scene {
    use Check::*;
    use Checkpoint::*;
    use Event::{Action, Activate};
    let mut steps = phrase();
    steps.extend([
        (
            "Insert root note",
            Action(SemanticAction::Activate),
            feature(Notes(1)),
        ),
        (
            "Move to instrument column",
            Action(SemanticAction::Navigate(Direction::Right)),
            feature(Cursor(0, 1)),
        ),
        (
            "Move to velocity column",
            Action(SemanticAction::Navigate(Direction::Right)),
            feature(Cursor(0, 2)),
        ),
        (
            "Select first FX column",
            Action(SemanticAction::Navigate(Direction::Right)),
            feature(Cursor(0, 3)),
        ),
        (
            "Popover opens typed command list",
            Action(SemanticAction::OpenPopover),
            Panel("fx_command:4"),
        ),
        (
            "Choose Gate Cut",
            Activate("fx_command:4"),
            Panel("fx_apply"),
        ),
        (
            "Cancel draft without mutation",
            Activate("fx_cancel"),
            feature(Fx(None)),
        ),
        (
            "Reopen commands",
            Activate("effects"),
            Panel("fx_command:4"),
        ),
        ("Choose Cut", Activate("fx_command:4"), Panel("fx_value")),
        (
            "Apply Cut and return to Phrase",
            Activate("fx_apply"),
            feature(Fx(Some(crest_tracker::composition::Fx::Cut {
                after: Beat::new(1, 4).unwrap(),
            }))),
        ),
        (
            "Return past velocity",
            Action(SemanticAction::Navigate(Direction::Left)),
            feature(Cursor(0, 2)),
        ),
        (
            "Return past instrument",
            Action(SemanticAction::Navigate(Direction::Left)),
            feature(Cursor(0, 1)),
        ),
        (
            "Return to note column",
            Action(SemanticAction::Navigate(Direction::Left)),
            feature(Cursor(0, 0)),
        ),
        (
            "Next row",
            Action(SemanticAction::Navigate(Direction::Down)),
            feature(Cursor(1, 0)),
        ),
        (
            "Insert explicit Note OFF",
            Activate("notes_off"),
            feature(Off),
        ),
    ]);
    steps.extend([
        (
            "Return to root row",
            Action(SemanticAction::Navigate(Direction::Up)),
            feature(Cursor(0, 0)),
        ),
        (
            "Clear Cut before testing standalone OFF",
            Activate("effects"),
            Panel("fx_command:0"),
        ),
        ("Choose no FX", Activate("fx_command:0"), Panel("fx_apply")),
        ("Apply cleared FX", Activate("fx_apply"), feature(Fx(None))),
        ("Open note tools", Activate("notes"), Panel("notes_values")),
        (
            "Note values",
            Activate("notes_values"),
            Panel("notes_length"),
        ),
        (
            "Set a long gate",
            Activate("notes_length"),
            Panel("text_submit"),
        ),
        (
            "Two-beat note duration",
            Event::Submit("notes_length", "2"),
            Panel("notes_length"),
        ),
        ("Back to tools", Activate("back"), Panel("notes_history")),
        ("Return to Phrase", Activate("back"), feature(Off)),
        (
            "Play note and OFF marker",
            Action(SemanticAction::PreviewStart),
            FullPlayback,
        ),
        (
            "OFF releases voice before its two-beat duration",
            Event::Wait,
            ReleasedBefore(Beat::integer(2)),
        ),
        (
            "Stop transport",
            Action(SemanticAction::PreviewStart),
            Stopped,
        ),
    ]);
    Scene::script("fx", steps)
}

fn navigation() -> Scene {
    use Check::*;
    use Checkpoint::*;
    use Direction::*;
    use Event::{Action, Activate, Focus, Submit};
    let mut steps = phrase();
    steps.extend([
        (
            "Main menu from Phrase",
            Action(SemanticAction::NavigatePage(Down)),
            Panel("settings"),
        ),
        (
            "Shift Up returns to previous Phrase",
            Action(SemanticAction::NavigatePage(Up)),
            View(crate::app::View::Phrase),
        ),
        (
            "Return to Chain",
            Action(SemanticAction::NavigatePage(Left)),
            View(crate::app::View::Chain),
        ),
        (
            "Chain settings",
            Action(SemanticAction::NavigatePage(Up)),
            Panel("arr_chain_name"),
        ),
        (
            "Rename chain",
            Activate("arr_chain_name"),
            Panel("text_submit"),
        ),
        (
            "Apply chain name",
            Submit("arr_chain_name", "Scene chain"),
            feature(ChainName),
        ),
        (
            "Back to Chain",
            Activate("back"),
            View(crate::app::View::Chain),
        ),
        (
            "Return to Song",
            Action(SemanticAction::NavigatePage(Left)),
            View(crate::app::View::Song),
        ),
        (
            "Global settings",
            Action(SemanticAction::OpenMidiSettings),
            Panel("settings_music"),
        ),
        (
            "Focus Music tab",
            Focus("settings_music"),
            feature(Checkpoint::Focus("settings_music")),
        ),
        (
            "Left reaches Global defaults",
            Action(SemanticAction::Navigate(Left)),
            Panel("default_tempo"),
        ),
        (
            "Focus new-song default tempo",
            Focus("default_tempo"),
            feature(Checkpoint::Focus("default_tempo")),
        ),
        (
            "Coarse default tempo keeps current Song unchanged",
            Action(SemanticAction::Adjust(Up)),
            feature(DefaultTempo),
        ),
        (
            "Return to Song",
            Activate("back"),
            View(crate::app::View::Song),
        ),
        (
            "Main menu",
            Action(SemanticAction::NavigatePage(Down)),
            Panel("instruments"),
        ),
        ("Synth Instruments", Activate("instruments"), Synth),
        (
            "Menu from Synth",
            Action(SemanticAction::NavigatePage(Down)),
            Panel("mixer"),
        ),
        (
            "Previous Synth page restored",
            Action(SemanticAction::NavigatePage(Up)),
            Synth,
        ),
        (
            "Menu again",
            Action(SemanticAction::NavigatePage(Down)),
            Panel("home"),
        ),
        (
            "Song remains reachable",
            Activate("home"),
            View(crate::app::View::Song),
        ),
    ]);
    Scene::script("navigation", steps)
}

fn rest_kill() -> Scene {
    use crest_tracker::composition::SpecialChain;
    use Check::*;
    use Event::{Action, Activate, Submit, Wait};
    use SemanticAction::*;
    let mut steps = phrase();
    steps.extend([
        (
            "Insert held note",
            Action(Adjust(Direction::Right)),
            Note(false),
        ),
        (
            "Popover opens note tools",
            Action(OpenPopover),
            Panel("notes_values"),
        ),
        (
            "Open note values",
            Activate("notes_values"),
            Panel("notes_length"),
        ),
        (
            "Set long duration",
            Activate("notes_length"),
            Panel("text_submit"),
        ),
        (
            "Hold note for twenty beats",
            Submit("notes_length", "20"),
            Panel("notes_length"),
        ),
        (
            "Return to note tools",
            Activate("back"),
            Panel("notes_values"),
        ),
        (
            "Return to phrase",
            Activate("back"),
            View(crate::app::View::Phrase),
        ),
        (
            "Open main menu",
            Action(NavigatePage(Direction::Down)),
            Panel("home"),
        ),
        (
            "Return to Song",
            Activate("home"),
            View(crate::app::View::Song),
        ),
        (
            "Select padding slot",
            Action(Navigate(Direction::Down)),
            feature(Checkpoint::Cursor(1, 0)),
        ),
        (
            "Open Song slot picker",
            Action(OpenPopover),
            Panel("slot_rest"),
        ),
        (
            "Insert Rest",
            Activate("slot_rest"),
            feature(Checkpoint::Padding(SpecialChain::Rest, 1)),
        ),
        (
            "Coarse rest count increases by ten",
            Action(Adjust(Direction::Up)),
            feature(Checkpoint::Padding(SpecialChain::Rest, 11)),
        ),
        (
            "Restore one measure",
            Action(Adjust(Direction::Down)),
            feature(Checkpoint::Padding(SpecialChain::Rest, 1)),
        ),
        (
            "Select force-release slot",
            Action(Navigate(Direction::Down)),
            feature(Checkpoint::Cursor(2, 0)),
        ),
        ("Open Kill picker", Action(OpenPopover), Panel("slot_kill")),
        (
            "Insert Kill",
            Activate("slot_kill"),
            feature(Checkpoint::Padding(SpecialChain::Kill, 1)),
        ),
        (
            "Select return slot",
            Action(Navigate(Direction::Down)),
            feature(Checkpoint::Cursor(3, 0)),
        ),
        (
            "Open Chain picker",
            Action(OpenPopover),
            Panel("slot_chain"),
        ),
        ("Reuse chain after padding", Activate("slot_chain"), Chain),
        ("Play arrangement", Action(PreviewStart), FullPlayback),
        (
            "Rest cursor advances while previous note rings",
            Wait,
            PaddingPlayback {
                slot: 1,
                sounding: true,
            },
        ),
        (
            "Kill releases note while its cursor waits",
            Wait,
            PaddingPlayback {
                slot: 2,
                sounding: false,
            },
        ),
        (
            "Track resumes after Kill",
            Wait,
            PaddingPlayback {
                slot: 3,
                sounding: true,
            },
        ),
        ("Stop demo", Action(PreviewStart), Stopped),
    ]);
    Scene::script("rest-kill", steps)
}
