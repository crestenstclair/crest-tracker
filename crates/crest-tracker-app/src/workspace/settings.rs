//! Global musical settings; device ownership stays in the hosted Synth runtime.
use super::*;
use crest_tracker::composition::Key;
use crest_tracker::harmony::{ScaleKind, Spelling};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Tab {
    #[default]
    Music,
    Clock,
    Devices,
    Files,
    Defaults,
}
impl Tab {
    const ALL: [Self; 5] = [
        Self::Music,
        Self::Clock,
        Self::Devices,
        Self::Files,
        Self::Defaults,
    ];
    fn id(self) -> &'static str {
        match self {
            Self::Music => "settings_music",
            Self::Clock => "settings_clock",
            Self::Devices => "settings_devices",
            Self::Files => "settings_files",
            Self::Defaults => "settings_defaults",
        }
    }
}
pub(super) fn tabs(app: &App) -> Vec<WorkspaceRow> {
    Tab::ALL
        .into_iter()
        .zip([
            "Music",
            "Clock / MIDI",
            "Devices",
            "Files",
            "Global / defaults",
        ])
        .map(|(tab, name)| {
            let mut item = row(tab.id(), name, "");
            item.selected = app.workspace.settings_tab == tab;
            item
        })
        .collect()
}
pub(super) fn select(app: &mut App, id: &str) -> bool {
    let Some(tab) = Tab::ALL.into_iter().find(|tab| tab.id() == id) else {
        return false;
    };
    app.workspace.settings_tab = tab;
    app.workspace.focus = Some(tab.id().into());
    true
}
pub(super) fn navigate(app: &mut App, direction: Direction, focus: &str) -> bool {
    let tab = app.workspace.settings_tab;
    if focus == tab.id() {
        match direction {
            Direction::Left | Direction::Right => {
                let index = Tab::ALL.iter().position(|item| *item == tab).unwrap();
                let delta = if direction == Direction::Left { -1 } else { 1 };
                select(
                    app,
                    Tab::ALL[(index as i32 + delta).rem_euclid(Tab::ALL.len() as i32) as usize]
                        .id(),
                );
            }
            Direction::Down => app.workspace.focus = rows(app).first().map(|row| row.id.clone()),
            Direction::Up => {}
        }
        return true;
    }
    if direction == Direction::Up && rows(app).first().is_some_and(|row| row.id == focus) {
        app.workspace.focus = Some(tab.id().into());
        return true;
    }
    false
}

pub(super) fn rows(app: &App) -> Vec<WorkspaceRow> {
    let p = &app.preferences;
    let mut rows = vec![
        row(
            "tempo",
            "Current tempo",
            format!("{} BPM", app.editor.song().tempo.bpm()),
        ),
        row(
            "song_settings",
            "Song name / meter",
            &app.editor.song().name,
        ),
    ];
    rows.extend(key_rows(&app.editor.song().key));
    rows.extend([
        row(
            "pitch_display",
            "Pitch display",
            if app.pitch_intervals {
                "Intervals"
            } else {
                "Notes"
            },
        ),
        row(
            "pitch_entry",
            "Note entry",
            if app.chromatic_entry {
                "Chromatic"
            } else {
                "Scale only"
            },
        ),
        row(
            "default_tempo",
            "Default tempo for new Songs",
            p["default_bpm"].as_f64().unwrap_or(120.0).to_string(),
        ),
        row(
            "clock_input",
            "Clock input (selected MIDI device)",
            if p["clock"]["input"].is_string() {
                "External"
            } else {
                "Internal"
            },
        ),
        row(
            "clock_output",
            "Clock output",
            p["clock"]["output"].as_str().unwrap_or("Off"),
        ),
        row(
            "clock_delay",
            "External clock delay (ms)",
            p["clock"]["delay_ms"].as_u64().unwrap_or(100).to_string(),
        ),
        row("midi_outputs", "MIDI destinations / control surfaces", ""),
        row(
            "synth_settings",
            "Audio / MIDI devices / controller buttons",
            "Synth settings",
        ),
        row(
            "instruments",
            "Master volume / instrument settings",
            "Synth workspace",
        ),
        row(
            "sample_folder",
            "Sample starting directory",
            p["sample_folder"].as_str().unwrap_or("Library default"),
        ),
        row(
            "soundfont_folder",
            "SoundFont starting directory",
            p["soundfont_folder"].as_str().unwrap_or("Library default"),
        ),
        row("back", "Back", ""),
    ]);
    rows.retain(|row| {
        row.id == "back"
            || match app.workspace.settings_tab {
                Tab::Music => {
                    matches!(
                        row.id.as_str(),
                        "tempo" | "song_settings" | "pitch_display" | "pitch_entry"
                    ) || row.id.starts_with("key_")
                }
                Tab::Clock => row.id.starts_with("clock_") || row.id == "midi_outputs",
                Tab::Devices => matches!(row.id.as_str(), "synth_settings" | "instruments"),
                Tab::Files => matches!(row.id.as_str(), "sample_folder" | "soundfont_folder"),
                Tab::Defaults => row.id == "default_tempo",
            }
    });
    rows
}

pub(super) fn key_rows(key: &Key) -> Vec<WorkspaceRow> {
    vec![
        row(
            "key_root",
            "Key root",
            [
                "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B",
            ][key.root() as usize],
        ),
        row("key_scale", "Scale", key.kind().name()),
        row(
            "key_custom",
            "Custom scale pitch classes",
            key.pitch_classes()
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        row("key_spelling", "Spelling", format!("{:?}", key.spelling())),
    ]
}

pub(super) fn adjust(app: &mut App, id: &str, delta: i32) {
    if id.starts_with("key_") {
        change_key(app, id, delta, false);
        return;
    }
    match id {
        "patch_capacity" => {
            if let Some(budget) = app.workspace.patch_capacity {
                app.requests.push(Request::PatchCapacity {
                    capacity: budget
                        .limit
                        .saturating_add_signed(delta as isize)
                        .clamp(budget.installed.max(1), budget.maximum),
                });
            }
        }
        "tempo" => {
            let bpm = (app.editor.song().tempo.bpm() + f64::from(delta)).max(1.0);
            if let Ok(tempo) = Tempo::new((bpm * 1000.0) as u32) {
                app.requests.push(Request::Tempo { tempo });
            }
        }
        "pitch_display" | "pitch_entry" => app.requests.push(Request::PitchDisplay {
            intervals: app.pitch_intervals ^ (id == "pitch_display"),
            chromatic: app.chromatic_entry ^ (id == "pitch_entry"),
        }),
        "default_tempo" | "clock_input" | "clock_output" | "clock_delay" => {
            change_preferences(app, id, delta)
        }
        _ => {}
    }
}

pub(super) fn change_key(app: &mut App, id: &str, delta: i32, phrase: bool) {
    let key = if phrase {
        app.selected_phrase()
            .and_then(|(_, p)| p.key.as_ref())
            .unwrap_or(&app.editor.song().key)
    } else {
        &app.editor.song().key
    };
    let mut root = key.root();
    let mut scale = Some(key.kind());
    let mut spelling = key.spelling();
    match id {
        "key_root" => root = (i32::from(root) + delta).rem_euclid(12) as u8,
        "key_scale" => {
            let index = ScaleKind::CATALOG
                .iter()
                .position(|v| *v == key.kind())
                .unwrap_or(0);
            scale = Some(
                ScaleKind::CATALOG
                    [(index as i32 + delta).rem_euclid(ScaleKind::CATALOG.len() as i32) as usize],
            );
        }
        "key_spelling" => {
            spelling = if spelling == Spelling::Sharps {
                Spelling::Flats
            } else {
                Spelling::Sharps
            }
        }
        "key_inherit" => scale = None,
        _ => return,
    }
    app.requests.push(Request::KeySettings {
        revision: app.editor.revision(),
        phrase,
        root,
        scale,
        pitch_classes: key.pitch_classes().to_vec(),
        spelling,
    });
}

fn change_preferences(app: &mut App, id: &str, delta: i32) {
    let p = &app.preferences;
    let mut bpm = p["default_bpm"].as_f64().unwrap_or(120.0);
    let mut input_clock = p["clock"]["input"].is_string();
    let mut output_clock = p["clock"]["output"].as_str().map(str::to_owned);
    let mut delay_ms = p["clock"]["delay_ms"].as_u64().unwrap_or(100) as u32;
    match id {
        "default_tempo" => bpm = (bpm + f64::from(delta)).max(1.0),
        "clock_input" => {
            input_clock = !input_clock;
            if input_clock {
                output_clock = None;
            }
        }
        "clock_output" => {
            let mut outputs = vec![None];
            if let Some(endpoints) = app.outputs["endpoints"].as_array() {
                outputs.extend(
                    endpoints
                        .iter()
                        .filter_map(|v| v["id"].as_str().map(|id| Some(id.to_owned()))),
                );
            }
            let index = outputs.iter().position(|v| v == &output_clock).unwrap_or(0);
            output_clock =
                outputs[(index as i32 + delta).rem_euclid(outputs.len() as i32) as usize].clone();
            if output_clock.is_some() {
                input_clock = false;
            }
        }
        "clock_delay" => delay_ms = delay_ms.saturating_add_signed(delta).clamp(20, 2000),
        _ => return,
    }
    let Ok(default_tempo) = Tempo::new((bpm * 1000.0).round() as u32) else {
        return;
    };
    app.requests.push(Request::Preferences {
        default_tempo,
        monitor: p["monitor"].as_bool().unwrap_or(true),
        input_clock,
        output_clock,
        delay_ms,
        mpe_input: serde_json::from_value(p["mpe_input"].clone())
            .ok()
            .flatten(),
    });
}
