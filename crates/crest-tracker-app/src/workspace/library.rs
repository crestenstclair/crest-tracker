use super::*;
use crate::app::ChordLibraryAction;
use crest_tracker::composition::InstrumentId;

pub(super) fn sound_rows(app: &App) -> Vec<WorkspaceRow> {
    let mut rows = Vec::new();
    for (index, (id, instrument)) in app.editor.song().instruments.iter().enumerate() {
        rows.push(row(
            format!("sound:publish:{}", id.get()),
            format!("Publish {index:02X} · {}", instrument.name),
            "New immutable revision",
        ));
        rows.push(row(
            format!("sound:adopt:{}", id.get()),
            format!("Adopt / update {}", instrument.name),
            instrument
                .resource
                .as_ref()
                .map_or("Local sound".into(), |r| r.revision.clone()),
        ));
    }
    rows.push(row("repair", "Missing resource repair", ""));
    rows.push(row("back", "Back", ""));
    rows
}
pub(super) fn chord_rows(app: &App) -> Vec<WorkspaceRow> {
    let mut rows = commands(&[("chord_new", "Create chord")]);
    rows.extend(chords::choices(app).into_iter().map(|(id, _, chord)| {
        row(
            format!("library_edit:{id}"),
            chords::display_name(&chord),
            chords::interval_label(chord.definition().shape().intervals()),
        )
    }));
    rows.push(row("back", "Back", ""));
    rows
}
pub(super) fn repair_rows(app: &App) -> Vec<WorkspaceRow> {
    let mut rows = Vec::new();
    if let Some(resources) = app.resource_repair["resources"].as_array() {
        for item in resources {
            let Some(index) = item["index"].as_u64() else {
                continue;
            };
            let name = item["name"].as_str().unwrap_or("Resource");
            rows.push(row(
                format!("repair:locate:{index}"),
                format!("Locate original {name}"),
                "Verify saved digest",
            ));
            rows.push(row(
                format!("repair:replace:{index}"),
                format!("Replace {name}"),
                "Accept different content explicitly",
            ));
        }
        rows.push(row(
            "repair:cancel:0",
            "Cancel opening Song",
            "Keep current Song",
        ));
    } else {
        rows.push(row("back", "No missing resources", ""));
    }
    rows
}
pub(super) fn activate(app: &mut App, id: &str) -> bool {
    if let Some(choice) = id.strip_prefix("library_edit:") {
        if let Some((_, _, chord)) = chords::choices(app)
            .into_iter()
            .find(|(id, _, _)| id == choice)
        {
            chord_editor::begin(app, Some(chord));
        }
        return true;
    }
    if id == "library_refresh" {
        app.requests.push(Request::ChordLibrary {
            action: ChordLibraryAction::Refresh,
        });
        return true;
    }
    if let Some(digest) = id.strip_prefix("library_adopt:") {
        app.requests.push(Request::ChordLibrary {
            action: ChordLibraryAction::Adopt {
                revision: app.editor.revision(),
                digest: digest.into(),
            },
        });
        return true;
    }
    let parts: Vec<_> = id.split(':').collect();
    if parts.len() != 3 {
        return false;
    }
    if parts[0] == "sound" {
        if let Some(instrument) = parts[2]
            .parse()
            .ok()
            .and_then(|id| InstrumentId::new(id).ok())
        {
            app.requests.push(Request::SoundLibrary {
                action: parts[1].into(),
                instrument,
            });
        }
        return true;
    }
    if parts[0] == "repair" {
        if let Ok(index) = parts[2].parse() {
            app.requests.push(Request::RepairResource {
                index,
                action: parts[1].into(),
            });
        }
        return true;
    }
    false
}
