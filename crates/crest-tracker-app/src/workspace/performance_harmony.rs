//! Semantic controls shared by mouse, keyboard and controller navigation.
use super::*;
use crate::app::{PerformanceHarmonyEdit as Edit, PerformanceInputMode};
use crest_performance_core::{
    Articulation, ChordExtension, ExpressionPolicy, HarmonyPlaying, HarmonySettings, Scale,
    Suspension,
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
    rows.extend(playing_controls(settings));
    rows.extend(commands(&[
        ("harmony_input", "Enable LinnStrument MPE input"),
        ("back", "Back"),
    ]));
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
                    format!("Select degree {label} · strumplate harmony"),
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
        "Stored articulation",
        match settings.articulation {
            Articulation::Block => "Block",
            Articulation::Strum { .. } => "Delayed strum",
            Articulation::Arpeggio { .. } => "Repeating arpeggio",
            Articulation::Humanized { .. } => "Humanized",
        },
    ));
    if settings.articulation != Articulation::Block {
        rows.extend(commands(&[(
            "harmony_block",
            "Use Block articulation for live chord pads",
        )]));
    }
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
        _ => Edit::Degree {
            degree: id
                .strip_prefix("harmony_degree:")?
                .parse::<u8>()
                .ok()
                .filter(|d| *d < 7)?,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
        app.playing = true;
        action(&mut app, SemanticAction::PreviewStart);
        assert!(matches!(app.requests.pop(), Some(Request::Stop)));
        commands::activate(&mut app, "back");
        assert_eq!(app.workspace.panel, Some(Panel::Performance));
        assert_eq!(app.workspace.focus.as_deref(), Some("performance_harmony"));
    }
}
