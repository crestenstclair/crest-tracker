use super::*;

pub(super) fn activate(app: &mut App, id: &str) {
    if app.workspace.panel == Some(Panel::Settings) && settings::select(app, id) {
        return;
    }
    if id == "chords_cancel" {
        open(app, None);
        return;
    }
    if id == "parent" {
        return_to_parent(app);
        return;
    }
    if id == "enter" {
        activate_focused(app);
        return;
    }
    if id == "minimap_follow" {
        app.workspace.minimap_follow_paused = !app.workspace.minimap_follow_paused;
        if let Some(at) = app.play_beat {
            super::follow_playback(app, at);
        }
        return;
    }
    if id.starts_with("dialog:") {
        dialog_action(app, id);
        return;
    }
    if id.starts_with("shared:") {
        shared_action(app, id);
        return;
    }
    if id.starts_with("chord:") {
        chord_action(app, id);
        return;
    }
    if arrangement::activate(app, id)
        || library::activate(app, id)
        || midi::activate(app, id)
        || generator::activate(app, id)
        || effects::activate(app, id)
        || notes::activate(app, id)
        || notes::expression_activate(app, id)
        || chord_editor::activate(app, id)
        || navigate_command(app, id)
        || edit_command(app, id)
    {
        return;
    }
    if let Some(request) = request_command(app, id) {
        app.requests.push(request);
    }
}

fn navigate_command(app: &mut App, id: &str) -> bool {
    if id == "effects" {
        effects::begin(app);
        return true;
    }
    if id == "synth_back" {
        open(app, None);
        app.requests.push(Request::SynthAction {
            action: SemanticAction::NavigatePage(Direction::Down),
        });
        return true;
    }
    let panel = match id {
        "menu" => Some(Panel::Menu),
        "jump" => Some(Panel::QuickJump),
        "settings" => Some(Panel::Settings),
        "song_settings" => Some(Panel::SongSettings),
        "phrase_settings" => Some(Panel::PhraseSettings),
        "files" => Some(Panel::File),
        "tools" => Some(Panel::Tools),
        "arrangement" => Some(Panel::Arrangement),
        "generator" => Some(Panel::Generator),
        "notes" => Some(Panel::Notes),
        "expression" => Some(Panel::Expression),
        "library" => Some(Panel::Library),
        "chord_library" => Some(Panel::ChordLibrary),
        "repair" => Some(Panel::Repair),
        "midi_outputs" => Some(Panel::Midi),
        "chords" => Some(Panel::Chords),
        _ => None,
    };
    if let Some(panel) = panel {
        if panel == Panel::Chords {
            app.workspace.chord_slot = if app.workspace.panel == Some(Panel::Effects) {
                app.workspace.fx_slot as u8
            } else {
                app.column.saturating_sub(3).min(2) as u8
            };
        }
        open(app, Some(panel));
        if matches!(panel, Panel::ChordLibrary | Panel::Chords) {
            app.requests.push(Request::ChordLibrary {
                action: crate::app::ChordLibraryAction::Refresh,
            });
        }
        if panel == Panel::Midi {
            app.requests.push(Request::MidiOutput {
                action: "discover".into(),
                endpoint: None,
            });
        }
        return true;
    }
    let page = match id {
        "instruments" => Some(SynthPage::Instruments),
        "mixer" => Some(SynthPage::Mixer),
        "sends" => Some(SynthPage::Sends),
        "synth_settings" => Some(SynthPage::Settings),
        _ => None,
    };
    if let Some(page) = page {
        synth(app, page);
        return true;
    }
    if matches!(id, "home" | "back") {
        if id == "back" {
            back(app);
        } else {
            open(app, None);
        }
        if id == "home" {
            app.requests.push(Request::View { view: View::Song });
        }
        return true;
    }
    if id == "minimap" {
        open(app, None);
        app.requests.push(Request::View {
            view: View::Minimap,
        });
        return true;
    }
    false
}

fn edit_command(app: &mut App, id: &str) -> bool {
    if app.workspace.panel == Some(Panel::Text) && id.starts_with("text_") {
        text_action(app, id);
        return true;
    }
    if id.starts_with("key_") && id != "key_custom" {
        settings::change_key(
            app,
            id,
            1,
            app.workspace.panel == Some(Panel::PhraseSettings),
        );
        return true;
    }
    if matches!(
        id,
        "pitch_display"
            | "pitch_entry"
            | "clock_input"
            | "clock_output"
            | "clock_delay"
            | "default_tempo"
    ) {
        settings::adjust(app, id, 1);
        return true;
    }
    match id {
        "sample_folder" | "soundfont_folder" => {
            let path = app.preferences[id].as_str().unwrap_or("").to_owned();
            text_entry(
                app,
                id,
                "Starting directory (empty restores default)",
                &path,
            );
        }
        "key_custom" => {
            let key = if app.workspace.panel == Some(Panel::PhraseSettings) {
                app.selected_phrase()
                    .and_then(|(_, p)| p.key.as_ref())
                    .unwrap_or(&app.editor.song().key)
            } else {
                &app.editor.song().key
            };
            let value = key
                .pitch_classes()
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(" ");
            let id = if app.workspace.panel == Some(Panel::PhraseSettings) {
                "phrase_custom"
            } else {
                "song_custom"
            };
            text_entry(app, id, "Pitch classes 0–11, separated by spaces", &value);
        }
        "meter" => text_entry(
            app,
            id,
            "Meter, e.g. 4/4",
            &format!(
                "{}/{}",
                app.editor.song().meters[0].meter.numerator(),
                app.editor.song().meters[0].meter.denominator()
            ),
        ),
        "name" => text_entry(app, "name", "Song name", &app.editor.song().name.clone()),
        "tempo" => text_entry(
            app,
            "tempo",
            "Tempo (BPM)",
            &app.editor.song().tempo.bpm().to_string(),
        ),
        "phrase_name" => {
            if let Some((_, phrase)) = app.selected_phrase() {
                let name = phrase.name.clone();
                text_entry(app, "phrase_name", "Phrase name", &name);
            }
        }
        "phrase_steps" | "phrase_form" | "division" | "tuplet_notes" | "tuplet_time" => {
            phrase::adjust(app, id, 1)
        }
        _ => return false,
    }
    true
}

fn request_command(app: &App, id: &str) -> Option<Request> {
    let file = match id {
        "open" => Some(FileAction::Open),
        "new" => Some(FileAction::New),
        "corridors" => Some(FileAction::Corridors),
        "import_midi" => Some(FileAction::ImportMidi),
        "save" => Some(FileAction::Save),
        "save_as" => Some(FileAction::SaveAs),
        "import" => Some(FileAction::ImportSynth),
        "export" => Some(FileAction::ExportSynth),
        "collect" => Some(FileAction::CollectAssets),
        _ => None,
    };
    if let Some(action) = file {
        return Some(Request::File { action });
    }
    let capture = match id {
        "record" => Some(CaptureAction::Record),
        "end_take" => Some(CaptureAction::End),
        "commit_take" => Some(CaptureAction::Commit),
        "discard_take" => Some(CaptureAction::Discard),
        _ => None,
    };
    if let Some(action) = capture {
        return Some(Request::Capture { action });
    }
    Some(match id {
        "example" => Request::Example,
        "play" => Request::Play,
        "stop" => Request::Stop,
        "clear" => Request::Clear,
        "undo" => Request::Undo,
        "redo" => Request::Redo,
        "recover" => Request::Recovery {
            action: "recover".into(),
        },
        "arm" => Request::Arm {
            mpe: false,
            instrument: app.editor.song().instruments.keys().next().copied(),
            endpoint: None,
            monitor: true,
            unique: false,
        },
        _ => return None,
    })
}

pub(super) fn text_entry(app: &mut App, id: &str, label: &str, value: &str) {
    open(app, Some(Panel::Text));
    app.workspace.character = 0;
    app.workspace.text = Some(WorkspaceTextEntry {
        id: id.into(),
        label: label.into(),
        value: value.into(),
        submit_label: if id.starts_with("filename:") && app.file_dialog["mode"] == "open" {
            "Open"
        } else {
            "Save"
        }
        .into(),
    });
}

pub(super) fn submit(app: &mut App, id: &str, value: String) {
    if app
        .workspace
        .text
        .as_ref()
        .is_none_or(|entry| entry.id != id)
    {
        return;
    }
    if matches!(id, "chord_name" | "chord_intervals") {
        let Some(draft) = &mut app.workspace.chord else {
            return;
        };
        if id == "chord_name" {
            draft.name = value;
        } else {
            match parse_numbers::<i8>(&value) {
                Ok(v) => draft.intervals = v,
                Err(e) => {
                    app.status = e;
                    return;
                }
            }
        }
        back(app);
        return;
    }
    if id == "fx_intervals" {
        let values = match parse_numbers::<i8>(&value) {
            Ok(v) => v,
            Err(e) => {
                app.status = e;
                return;
            }
        };
        if let Some(
            crest_tracker::composition::Fx::Chord { intervals }
            | crest_tracker::composition::Fx::Add { intervals },
        ) = &mut app.workspace.fx
        {
            *intervals = values;
        }
        back(app);
        return;
    }
    if matches!(id, "expr_offset" | "expr_value" | "expr_number") {
        match notes::submit_expression(app, id, &value) {
            Ok(()) => back(app),
            Err(error) => app.status = error,
        }
        return;
    }
    let request = match id {
        "notes_grid" => match notes::parse_beat(&value) {
            Ok(grid) if !grid.is_zero() => Some(Request::PianoControl {
                control: PianoControl::Grid { grid },
            }),
            _ => {
                app.status = "Grid must be positive; use e.g. 1/3".into();
                return;
            }
        },
        "sample_folder" | "soundfont_folder" => {
            let mut sample = app.preferences["sample_folder"].as_str().map(str::to_owned);
            let mut soundfont = app.preferences["soundfont_folder"]
                .as_str()
                .map(str::to_owned);
            let path = if value.trim().is_empty() {
                None
            } else {
                Some(value)
            };
            if id == "sample_folder" {
                sample = path;
            } else {
                soundfont = path;
            }
            Some(Request::BrowserFolders { sample, soundfont })
        }

        "arr_track_name" => {
            let mut track = app.editor.song().tracks[app.track].clone();
            track.name = value;
            Some(Request::Edit {
                revision: app.editor.revision(),
                command: Box::new(Command::PutTrack { track }),
            })
        }
        "arr_chain_name" => app.selected_chain().map(|(id, chain)| {
            let mut chain = chain.clone();
            chain.name = value;
            Request::Edit {
                revision: app.editor.revision(),
                command: Box::new(Command::PutChain { id, chain }),
            }
        }),
        "notes_start" => match notes::parse_beat(&value) {
            Ok(at) => app.selected_phrase().zip(effects::selected_event(app)).map(
                |((phrase, _), mut event)| {
                    event.at = at;
                    Request::Edit {
                        revision: app.editor.revision(),
                        command: Box::new(Command::PutEvent { phrase, event }),
                    }
                },
            ),
            Err(error) => {
                app.status = error;
                return;
            }
        },
        "notes_length" => {
            let duration = match notes::parse_beat(&value) {
                Ok(v) if !v.is_zero() => v,
                _ => {
                    app.status = "Duration must be positive".into();
                    return;
                }
            };
            app.selected_phrase()
                .zip(effects::selected_event(app))
                .map(|((phrase, _), event)| Request::Edit {
                    revision: app.editor.revision(),
                    command: Box::new(Command::ResizeNotes {
                        phrase,
                        events: vec![event.id],
                        duration,
                    }),
                })
        }
        "name" => Some(Request::Rename { name: value }),
        "tempo" => match value
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && *v > 0.0 && *v <= f64::from(u32::MAX) / 1000.0)
            .and_then(|bpm| Tempo::new((bpm * 1000.0).round() as u32).ok())
        {
            Some(tempo) => Some(Request::Tempo { tempo }),
            None => {
                app.status = "Invalid tempo".into();
                return;
            }
        },
        "meter" => {
            let parts = value
                .split('/')
                .map(str::parse::<u16>)
                .collect::<Result<Vec<_>, _>>();
            let meter = parts
                .ok()
                .filter(|v| v.len() == 2)
                .and_then(|v| crest_tracker::time::Meter::new(v[0], v[1]).ok());
            let Some(meter) = meter else {
                app.status = "Invalid meter; use e.g. 4/4".into();
                return;
            };
            let mut meters = app.editor.song().meters.clone();
            meters[0].meter = meter;
            Some(Request::Edit {
                revision: app.editor.revision(),
                command: Box::new(Command::SetMeters { meters }),
            })
        }
        "phrase_custom" | "song_custom" => {
            let values = match parse_numbers::<u8>(&value) {
                Ok(v) => v,
                Err(e) => {
                    app.status = e;
                    return;
                }
            };
            let phrase = id == "phrase_custom";
            let key = if phrase {
                app.selected_phrase()
                    .and_then(|(_, p)| p.key.as_ref())
                    .unwrap_or(&app.editor.song().key)
            } else {
                &app.editor.song().key
            };
            if let Err(e) = crest_tracker::composition::Key::custom(key.root(), values.clone()) {
                app.status = e.to_string();
                return;
            }
            Some(Request::KeySettings {
                revision: app.editor.revision(),
                phrase,
                root: key.root(),
                scale: Some(crest_tracker::harmony::ScaleKind::Custom),
                pitch_classes: values,
                spelling: key.spelling(),
            })
        }
        "phrase_name" => app.selected_phrase().map(|(id, phrase)| {
            let mut phrase = phrase.clone();
            phrase.name = value;
            Request::Edit {
                revision: app.editor.revision(),
                command: Box::new(Command::PutPhrase { id, phrase }),
            }
        }),
        _ if id.starts_with("filename:") => {
            let token = id.trim_start_matches("filename:").parse::<u64>().ok();
            if token != app.file_dialog["token"].as_u64() {
                return;
            }
            Some(Request::FileChoose {
                token: token.unwrap(),
                path: value,
                overwrite: false,
            })
        }
        _ => None,
    };
    back(app);
    if let Some(request) = request {
        app.requests.push(request);
    }
}

fn shared_action(app: &mut App, id: &str) {
    let parts: Vec<_> = id.split(':').collect();
    if parts.len() != 3 {
        return;
    }
    let Ok(token) = parts[1].parse() else {
        return;
    };
    let choice = match parts[2] {
        "shared" => Some(crate::app::SharedChoice::Shared),
        "unique" => Some(crate::app::SharedChoice::Unique),
        _ => None,
    };
    app.requests.push(Request::SharedDecision {
        token,
        choice,
        remember: false,
    });
}

const CHARACTERS: &str = " ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_. /#:,+";
pub(super) fn text_rows(app: &App) -> Vec<WorkspaceRow> {
    let character = CHARACTERS
        .chars()
        .nth(app.workspace.character)
        .unwrap_or(' ');
    vec![
        row(
            "text_character",
            "Character (Edit + direction)",
            format!("[{character}]"),
        ),
        row("text_append", "Append character", ""),
        row("text_delete", "Delete last character", ""),
        row("text_clear", "Clear text", ""),
        row("text_submit", "Accept", ""),
        row("back", "Cancel", ""),
    ]
}
pub(super) fn adjust_text(app: &mut App, id: &str, delta: i32) {
    if id == "text_character" {
        app.workspace.character =
            (app.workspace.character as i32 + delta).rem_euclid(CHARACTERS.len() as i32) as usize;
    }
}
fn text_action(app: &mut App, id: &str) {
    if id == "text_submit" {
        if let Some(e) = app.workspace.text.clone() {
            submit(app, &e.id, e.value);
        }
        return;
    }
    let Some(entry) = &mut app.workspace.text else {
        return;
    };
    match id {
        "text_append" | "text_character" => entry.value.push(
            CHARACTERS
                .chars()
                .nth(app.workspace.character)
                .unwrap_or(' '),
        ),
        "text_delete" => {
            entry.value.pop();
        }
        "text_clear" => entry.value.clear(),
        _ => {}
    }
}
pub(super) fn parse_numbers<T: std::str::FromStr>(value: &str) -> Result<Vec<T>, String> {
    let values = value
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|v| !v.is_empty())
        .map(str::parse)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Enter numbers separated by spaces".to_owned())?;
    if values.is_empty() {
        return Err("Enter at least one number".into());
    }
    Ok(values)
}
