use super::*;

pub(super) fn project_dialog(app: &App, view: &mut WorkspaceProjection) {
    let dialog = &app.file_dialog;
    view.context = "SONG FILE".into();
    view.title = dialog["title"].as_str().unwrap_or("Song File").into();
    view.subtitle = dialog["directory"]
        .as_str()
        .or(dialog["message"].as_str())
        .unwrap_or("")
        .into();
    if let Some(error) = dialog["error"].as_str() {
        view.status = error.into();
    }
    let prefix = format!("dialog:{}:{}", dialog["token"], dialog["version"]);
    if let Some(choices) = dialog["choices"].as_array() {
        view.rows = choices
            .iter()
            .map(|choice| {
                row(
                    format!("{prefix}:{}", choice["value"].as_str().unwrap_or("cancel")),
                    choice["label"].as_str().unwrap_or(""),
                    "",
                )
            })
            .collect();
        return;
    }
    view.rows = vec![row(format!("{prefix}:cancel"), "Cancel", "")];
    if dialog["overwrite"].is_string() {
        view.rows.push(row(
            format!("{prefix}:overwrite"),
            "Replace existing file",
            dialog["overwrite"].as_str().unwrap_or(""),
        ));
        return;
    }
    if let Some(parent) = dialog["parent"].as_str() {
        view.rows
            .push(row(format!("{prefix}:parent"), "Parent folder", parent));
    }
    if dialog["mode"] == "open" {
        view.rows
            .push(row(format!("{prefix}:filename"), "Open path…", ""));
    }
    if dialog["mode"] != "open" {
        view.rows.push(row(
            format!("{prefix}:filename"),
            "Save to filename",
            dialog["filename"].as_str().unwrap_or("Song.crest-song"),
        ));
    }
    if let Some(entries) = dialog["entries"].as_array() {
        view.rows
            .extend(entries.iter().enumerate().map(|(index, entry)| {
                row(
                    format!("{prefix}:entry:{index}"),
                    entry["name"].as_str().unwrap_or(""),
                    if entry["directory"] == true {
                        "FOLDER"
                    } else {
                        "FILE"
                    },
                )
            }));
    }
}

pub(super) fn file_cancel(app: &mut App) {
    if let Some(token) = app.file_dialog["token"].as_u64() {
        app.requests.push(Request::FileCancel { token });
    }
}

pub(super) fn dialog_action(app: &mut App, id: &str) {
    let dialog = &app.file_dialog;
    let prefix = format!("dialog:{}:{}:", dialog["token"], dialog["version"]);
    let Some(choice) = id.strip_prefix(&prefix) else {
        return;
    };
    let Some(token) = dialog["token"].as_u64() else {
        return;
    };
    if choice == "cancel" {
        file_cancel(app);
        return;
    }
    if dialog["mode"] == "confirm" {
        app.requests.push(Request::FileDecision {
            token,
            decision: choice.into(),
        });
        return;
    }
    if choice == "filename" {
        let name = dialog["filename"]
            .as_str()
            .filter(|v| !v.is_empty())
            .unwrap_or("Song.crest-song")
            .to_owned();
        let label = if dialog["mode"] == "open" {
            "Open Song path"
        } else {
            "Save to filename"
        };
        text_entry(app, &format!("filename:{token}"), label, &name);
        return;
    }
    let request = if choice == "parent" {
        Some(Request::FileBrowse {
            token,
            directory: dialog["parent"].as_str().unwrap_or("").into(),
        })
    } else if choice == "overwrite" {
        Some(Request::FileChoose {
            token,
            path: dialog["overwrite"].as_str().unwrap_or("").into(),
            overwrite: true,
        })
    } else if let Some(entry) = choice
        .strip_prefix("entry:")
        .and_then(|index| index.parse::<usize>().ok())
        .and_then(|index| dialog["entries"].get(index))
    {
        let path = entry["path"].as_str().unwrap_or("").to_owned();
        if entry["directory"] == true {
            Some(Request::FileBrowse {
                token,
                directory: path,
            })
        } else {
            Some(Request::FileChoose {
                token,
                path,
                overwrite: false,
            })
        }
    } else {
        None
    };
    if let Some(request) = request {
        app.requests.push(request);
    }
}
