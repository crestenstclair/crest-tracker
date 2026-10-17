//! Semantic controls shared by mouse, keyboard and controller navigation.
use super::*;
use crate::app::{PerformanceChordEdit, PerformanceHarmonyEdit as Edit, PerformanceInputMode};
use crest_performance_core::{
    Articulation, BeatTime, ChordExtension, ExpressionPolicy, HarmonyPlaying, HarmonySettings,
    Scale, StrumDirection, Suspension, TICKS_PER_BEAT,
};

pub(super) fn rows(app: &App) -> Vec<WorkspaceRow> {
    let mut rows = commands(&[
        ("harmony_play", "Play · ordinary MPE"),
        ("harmony_chords", "Chords · use library harmony"),
    ]);
    rows.push(row(
        "harmony_mode",
        "Input mode",
        app.performance["session"]["mode"]
            .as_str()
            .unwrap_or("unavailable")
            .to_uppercase(),
    ));
    let Ok(settings) =
        serde_json::from_value::<HarmonySettings>(app.performance["session"]["harmony"].clone())
    else {
        rows.push(row(
            "harmony_unavailable",
            "Open a performance session first",
            "",
        ));
        rows.extend(commands(&[("back", "Back")]));
        return rows;
    };
    if let Some(notes) = app.performance["harmony_notes"].as_array() {
        rows.push(row(
            "harmony_last_voicing",
            "Last played chord · MIDI notes",
            serde_json::Value::Array(notes.clone()).to_string(),
        ));
    }
    rows.extend(chord_controls(settings));
    rows.extend(learned_controls(settings));
    rows.extend(playing_controls(settings));
    rows.extend(commands(&[
        ("harmony_input", "Enable LinnStrument MPE input"),
        ("back", "Back"),
    ]));
    rows
}

fn learned_controls(settings: HarmonySettings) -> Vec<WorkspaceRow> {
    let value = match settings.learned.get(usize::from(settings.degree)) {
        Some(Some(chord)) => format!("{:?} · ROOT {}", chord.notes(), chord.root()),
        Some(None) => "DIATONIC".into(),
        None => "UNAVAILABLE".into(),
    };
    let mut rows = vec![row(
        "harmony_learned",
        "Selected degree · stored chord",
        value,
    )];
    rows.extend(commands(&[
        ("harmony_learn", "Learn held notes into selected degree"),
        ("harmony_restore", "Restore selected degree to diatonic"),
        ("harmony_undo", "Undo last clip or chord bank edit"),
    ]));
    rows.push(row(
        "harmony_learn_help",
        "Chord capture",
        "Play mode · Enable MPE input · Hold 1–5 notes · Lowest note becomes root; bends are not stored",
    ));
    rows
}

fn chord_controls(settings: HarmonySettings) -> Vec<WorkspaceRow> {
    let mut rows = Vec::new();
    rows.extend(adjust("tonic", "Tonic MIDI note", settings.tonic));
    rows.extend(adjust(
        "anchor",
        "Input MIDI note for degree I",
        settings.input_anchor,
    ));
    rows.push(row(
        "harmony_scale",
        "Scale · cycle",
        match settings.scale {
            Scale::Major => "Major",
            Scale::NaturalMinor => "Natural minor",
            Scale::Dorian => "Dorian",
            Scale::Mixolydian => "Mixolydian",
        },
    ));
    rows.push(row(
        "harmony_extension",
        "Chord size · cycle",
        match settings.extension {
            ChordExtension::Triad => "Triad",
            ChordExtension::Seventh => "Seventh",
            ChordExtension::Ninth => "Ninth",
        },
    ));
    rows.push(row(
        "harmony_suspension",
        "Suspension · cycle",
        match settings.suspension {
            Suspension::None => "None",
            Suspension::Second => "Second",
            Suspension::Fourth => "Fourth",
        },
    ));
    rows.push(row(
        "harmony_borrowed",
        "Borrow parallel scale",
        on_off(settings.borrowed),
    ));
    rows.extend(voicing_controls(settings));
    rows
}

fn voicing_controls(settings: HarmonySettings) -> Vec<WorkspaceRow> {
    let mut rows = Vec::new();
    rows.extend(adjust("inversion", "Inversion", settings.inversion));
    rows.extend(adjust("register", "Register · octaves", settings.register));
    rows.push(row(
        "harmony_spread",
        "Spread voicing",
        on_off(settings.spread),
    ));
    rows.push(row(
        "harmony_voice_leading",
        "Minimize movement between chords",
        on_off(settings.voice_leading),
    ));
    rows.push(row(
        "harmony_expression",
        "Gesture expression · all / top",
        match settings.expression {
            ExpressionPolicy::AllVoices => "All voices",
            ExpressionPolicy::TopVoice => "Top voice",
        },
    ));
    rows.push(row(
        "harmony_playing",
        "Playing · chords / manual strumplate",
        match settings.playing {
            HarmonyPlaying::ChordPads => "Chord pads",
            HarmonyPlaying::Strumplate => "Manual strumplate",
        },
    ));
    rows
}

fn playing_controls(settings: HarmonySettings) -> Vec<WorkspaceRow> {
    let mut rows = Vec::new();
    rows.extend(
        ["I", "II", "III", "IV", "V", "VI", "VII"]
            .into_iter()
            .enumerate()
            .map(|(degree, label)| {
                let mut item = row(
                    format!("harmony_degree:{degree}"),
                    format!("Select degree {label} · chord bank / strumplate"),
                    if usize::from(settings.degree) == degree {
                        "SELECTED"
                    } else {
                        ""
                    },
                );
                item.selected = usize::from(settings.degree) == degree;
                item
            }),
    );
    let pitches = settings
        .voicing(settings.degree, None)
        .map(|v| format!("{:?}", v.notes()))
        .unwrap_or_else(|e| e.to_string());
    rows.push(row(
        "harmony_voicing",
        "Selected voicing · before voice leading",
        pitches,
    ));
    rows.push(row(
        "harmony_mapping",
        "Input mapping",
        "Adjacent chromatic input notes select I–VII; strumplate picks successive chord tones",
    ));
    rows.push(row(
        "harmony_articulation",
        "Articulation",
        match settings.articulation {
            Articulation::Block => "Block",
            Articulation::Strum { .. } => "Delayed strum",
            Articulation::Arpeggio { .. } => "Repeating arpeggio",
            Articulation::Humanized { .. } => "Humanized",
        },
    ));
    rows.extend(commands(&[
        ("harmony_block", "Together · no attack delay"),
        ("harmony_strum_up", "Strum up · 1/8 beat between tones"),
        ("harmony_strum_down", "Strum down · 1/8 beat between tones"),
        (
            "harmony_arp_up",
            "Arpeggio up · 1/8 beat step, 1/16 beat gate",
        ),
        (
            "harmony_arp_down",
            "Arpeggio down · 1/8 beat step, 1/16 beat gate",
        ),
        (
            "harmony_humanized",
            "Humanized · up to 1/16 beat delay, velocity ±8, seed 42",
        ),
    ]));
    rows.push(row(
        "harmony_scope",
        "Setting changes",
        "Future attacks only · Existing loops update on relaunch",
    ));
    rows
}
fn on_off(enabled: bool) -> &'static str {
    if enabled {
        "ON"
    } else {
        "OFF"
    }
}
fn adjust(control: &str, label: &str, value: impl std::fmt::Display) -> Vec<WorkspaceRow> {
    vec![
        row(
            format!("harmony_{control}_down"),
            format!("{label} · decrease"),
            value.to_string(),
        ),
        row(
            format!("harmony_{control}_up"),
            format!("{label} · increase"),
            value.to_string(),
        ),
    ]
}

pub(super) fn activate(app: &mut App, id: &str) -> bool {
    if app.workspace.panel != Some(Panel::PerformanceHarmony) {
        return false;
    }
    let request = match id {
        "harmony_play" => Request::PerformanceInputMode {
            mode: PerformanceInputMode::Play,
        },
        "harmony_chords" => Request::PerformanceInputMode {
            mode: PerformanceInputMode::Chords,
        },
        "harmony_input" => Request::PerformanceInput,
        "harmony_learn" => Request::PerformanceChord {
            edit: PerformanceChordEdit::LearnHeld,
        },
        "harmony_restore" => Request::PerformanceChord {
            edit: PerformanceChordEdit::RestoreDiatonic,
        },
        "harmony_undo" => Request::PerformanceUndoClipChange,
        _ => {
            let Some(edit) = edit(id) else {
                return false;
            };
            Request::PerformanceHarmony { edit }
        }
    };
    app.requests.push(request);
    true
}
fn edit(id: &str) -> Option<Edit> {
    Some(match id {
        "harmony_tonic_down" => Edit::Tonic { delta: -1 },
        "harmony_tonic_up" => Edit::Tonic { delta: 1 },
        "harmony_anchor_down" => Edit::Anchor { delta: -1 },
        "harmony_anchor_up" => Edit::Anchor { delta: 1 },
        "harmony_scale" => Edit::Scale,
        "harmony_extension" => Edit::Extension,
        "harmony_suspension" => Edit::Suspension,
        "harmony_borrowed" => Edit::Borrowed,
        "harmony_inversion_down" => Edit::Inversion { delta: -1 },
        "harmony_inversion_up" => Edit::Inversion { delta: 1 },
        "harmony_register_down" => Edit::Register { delta: -1 },
        "harmony_register_up" => Edit::Register { delta: 1 },
        "harmony_spread" => Edit::Spread,
        "harmony_voice_leading" => Edit::VoiceLeading,
        "harmony_expression" => Edit::Expression,
        "harmony_playing" => Edit::Playing,
        "harmony_block" => Edit::BlockArticulation,
        "harmony_strum_up" | "harmony_strum_down" => Edit::Articulation {
            value: Articulation::Strum {
                direction: direction(id),
                interval: BeatTime(TICKS_PER_BEAT / 8),
            },
        },
        "harmony_arp_up" | "harmony_arp_down" => Edit::Articulation {
            value: Articulation::Arpeggio {
                direction: direction(id),
                interval: BeatTime(TICKS_PER_BEAT / 8),
                gate: BeatTime(TICKS_PER_BEAT / 16),
            },
        },
        "harmony_humanized" => Edit::Articulation {
            value: Articulation::Humanized {
                max_delay: BeatTime(TICKS_PER_BEAT / 16),
                velocity_variation: 8,
                seed: 42,
            },
        },
        _ => Edit::Degree {
            degree: id
                .strip_prefix("harmony_degree:")?
                .parse::<u8>()
                .ok()
                .filter(|d| *d < 7)?,
        },
    })
}

fn direction(id: &str) -> StrumDirection {
    if id.ends_with("_down") {
        StrumDirection::Descending
    } else {
        StrumDirection::Ascending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn learned_bank_controls_show_fixed_pitches_and_dispatch_without_stale_settings() {
        let mut app = App::new();
        app.workspace.panel = Some(Panel::PerformanceHarmony);
        let mut settings = HarmonySettings {
            degree: 4,
            ..Default::default()
        };
        settings.learned[4] =
            Some(crest_performance_core::LearnedChord::new(&[60, 63, 70], 60).unwrap());
        app.performance = serde_json::json!({"session":{"mode":"play","harmony":settings}});
        let projected = rows(&app);
        assert!(projected
            .iter()
            .any(|row| row.id == "harmony_learned" && row.value == "[60, 63, 70] · ROOT 60"));
        assert!(projected
            .iter()
            .any(|row| row.id == "harmony_voicing" && row.value == "[60, 63, 70]"));
        for (id, expected) in [
            ("harmony_learn", PerformanceChordEdit::LearnHeld),
            ("harmony_restore", PerformanceChordEdit::RestoreDiatonic),
        ] {
            assert!(activate(&mut app, id));
            assert!(
                matches!(app.requests.pop(), Some(Request::PerformanceChord { edit }) if edit == expected)
            );
        }
        assert!(activate(&mut app, "harmony_undo"));
        assert!(matches!(
            app.requests.pop(),
            Some(Request::PerformanceUndoClipChange)
        ));
        app.workspace.panel = Some(Panel::Performance);
        assert!(!activate(&mut app, "harmony_learn"));
    }

    #[test]
    fn controls_preserve_navigation_and_dispatch_typed_edits() {
        let mut app = App::new();
        commands::activate(&mut app, "menu");
        commands::activate(&mut app, "performance");
        app.workspace.focus = Some("performance_harmony".into());
        commands::activate(&mut app, "performance_harmony");
        app.performance =
            serde_json::json!({"session":{"mode":"chords","harmony":HarmonySettings::default()}});
        let projected = rows(&app);
        assert!(projected
            .iter()
            .any(|row| row.id == "harmony_voicing" && row.value == "[60, 64, 67]"));
        assert!(activate(&mut app, "harmony_degree:6"));
        assert!(matches!(
            app.requests.pop(),
            Some(Request::PerformanceHarmony {
                edit: Edit::Degree { degree: 6 }
            })
        ));
        assert!(!activate(&mut app, "harmony_degree:7"));
        assert!(activate(&mut app, "harmony_inversion_up"));
        assert!(matches!(
            app.requests.pop(),
            Some(Request::PerformanceHarmony {
                edit: Edit::Inversion { delta: 1 }
            })
        ));
        assert!(activate(&mut app, "harmony_chords"));
        assert!(matches!(
            app.requests.pop(),
            Some(Request::PerformanceInputMode {
                mode: PerformanceInputMode::Chords
            })
        ));
        for id in [
            "harmony_strum_up",
            "harmony_strum_down",
            "harmony_arp_up",
            "harmony_arp_down",
            "harmony_humanized",
        ] {
            assert!(projected.iter().any(|row| row.id == id));
            assert!(activate(&mut app, id));
            let Some(Request::PerformanceHarmony {
                edit: Edit::Articulation { value },
            }) = app.requests.pop()
            else {
                panic!("typed articulation command");
            };
            assert_ne!(value, Articulation::Block);
            HarmonySettings {
                articulation: value,
                ..HarmonySettings::default()
            }
            .validate()
            .unwrap();
        }
        app.playing = true;
        action(&mut app, SemanticAction::PreviewStart);
        assert!(matches!(app.requests.pop(), Some(Request::Stop)));
        commands::activate(&mut app, "back");
        assert_eq!(app.workspace.panel, Some(Panel::Performance));
        assert_eq!(app.workspace.focus.as_deref(), Some("performance_harmony"));
    }
}
