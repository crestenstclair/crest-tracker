use super::*;

pub(super) fn activate(app: &mut App, id: &str) {
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
    if navigate_command(app, id) || edit_command(app, id) {
        return;
    }
    if let Some(request) = request_command(app, id) {
        app.requests.push(request);
    }
}

fn navigate_command(app: &mut App, id: &str) -> bool {
    let panel = match id {
        "menu" => Some(Panel::Menu),
        "jump" => Some(Panel::QuickJump),
        "settings" => Some(Panel::SongSettings),
        "phrase_settings" => Some(Panel::PhraseSettings),
        "files" => Some(Panel::File),
        "tools" => Some(Panel::Tools),
        "chords" => Some(Panel::Chords),
        _ => None,
    };
    if let Some(panel) = panel {
        open(app, Some(panel));
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
        open(app, None);
        if id == "home" {
            app.requests.push(Request::View { view: View::Song });
        }
        return true;
    }
    false
}

fn edit_command(app: &mut App, id: &str) -> bool {
    match id {
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
        "phrase_steps" | "phrase_form" => change_phrase(app, id, 1),
        _ => return false,
    }
    true
}

fn request_command(app: &App, id: &str) -> Option<Request> {
    let file = match id {
        "open" => Some(FileAction::Open),
        "new" => Some(FileAction::New),
        "corridors" => Some(FileAction::Corridors),
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
        "enter" => Request::Confirm,
        "parent" => Request::Back,
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
    let request = match id {
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
    open(app, None);
    if let Some(request) = request {
        app.requests.push(request);
    }
}

pub(super) fn change_phrase(app: &mut App, field: &str, delta: i32) {
    let Some((id, phrase)) = app.selected_phrase() else {
        return;
    };
    let mut phrase = phrase.clone();
    match field {
        "phrase_steps" => phrase.steps = phrase.steps.saturating_add_signed(delta).max(1),
        "phrase_form" => {
            phrase.form = match phrase.form {
                PhraseForm::Tracker => PhraseForm::PianoRoll,
                PhraseForm::PianoRoll => PhraseForm::Drum,
                _ => PhraseForm::Tracker,
            }
        }
        _ => return,
    }
    app.requests.push(Request::Edit {
        revision: app.editor.revision(),
        command: Box::new(Command::PutPhrase { id, phrase }),
    });
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
