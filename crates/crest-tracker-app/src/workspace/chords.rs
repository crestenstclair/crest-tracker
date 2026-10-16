use super::*;

pub(super) fn chord_rows(app: &App) -> Vec<WorkspaceRow> {
    let key = app
        .selected_phrase()
        .and_then(|(_, phrase)| phrase.key.as_ref())
        .unwrap_or(&app.editor.song().key);
    let mut rows = vec![row("back", "Back", "")];
    if let Ok(defaults) = crest_tracker::chord_bank::default_bank(key) {
        rows.extend(defaults.iter().enumerate().map(|(index, chord)| {
            row(
                format!("chord:default:{index}"),
                chord.name(),
                if app.view == View::Phrase {
                    "APPLY"
                } else {
                    "COPY TO CUSTOM"
                },
            )
        }));
    }
    rows.extend(
        app.editor
            .song()
            .chords
            .entries()
            .map(|(id, chord)| row(format!("chord:saved:{}", id.get()), chord.name(), "CUSTOM")),
    );
    rows
}

pub(super) fn chord_action(app: &mut App, id: &str) {
    use crate::app::ChordSource;
    use crest_tracker::chord_bank::ChordRevisionId;
    let parts: Vec<_> = id.split(':').collect();
    if parts.len() != 3 {
        return;
    }
    let source = match parts[1] {
        "default" => parts[2]
            .parse()
            .ok()
            .map(|index| ChordSource::Default { index }),
        "saved" => parts[2]
            .parse()
            .ok()
            .and_then(|id| ChordRevisionId::new(id).ok())
            .map(|revision| ChordSource::Saved { revision }),
        _ => None,
    };
    let Some(source) = source else {
        return;
    };
    if app.view != View::Phrase {
        match app.chosen_chord(source) {
            Ok((id, Some(chord))) => app.requests.push(Request::Edit {
                revision: app.editor.revision(),
                command: Box::new(Command::PutChord {
                    id,
                    revision: chord,
                }),
            }),
            Ok(_) => app.status = "Chord already saved in Custom bank".into(),
            Err(error) => app.status = error.to_string(),
        }
        return;
    }
    let Some((_, phrase)) = app.selected_phrase() else {
        return;
    };
    let at = phrase
        .division
        .beat_length()
        .checked_scale(app.row as u64, 1)
        .ok();
    let event = phrase
        .events
        .iter()
        .find(|event| Some(event.at) == at && event.note.is_some());
    let Some(event) = event else {
        app.status = "Select a note before applying a chord".into();
        return;
    };
    let Some(occurrence) = app.occurrence() else {
        return;
    };
    app.requests.push(Request::ApplyChord {
        revision: app.editor.revision(),
        occurrence,
        event: event.id,
        slot: app.column.saturating_sub(3).min(2) as u8,
        root: event.note.as_ref().unwrap().pitch,
        source,
    });
    open(app, None);
}
