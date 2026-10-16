use super::*;

impl App {
    pub(super) fn preview_edit(
        &self,
        revision: u64,
        command: Command,
    ) -> Result<Value, DocumentError> {
        self.checked_revision(revision)?;
        let (id, before) = self
            .selected_phrase()
            .ok_or(DocumentError::Invalid("select a phrase to preview edits"))?;
        let mut candidate = Editor::new(self.editor.song().clone())?;
        candidate.apply(0, command)?;
        let after = candidate
            .song()
            .phrases
            .get(&id)
            .ok_or(DocumentError::Invalid("preview source disappeared"))?;
        let changes: Vec<_> = before.events.iter().filter_map(|old| {
            let next = after.events.iter().find(|event| event.id == old.id);
            (next != Some(old)).then(|| json!({"event":old.id,"before":event_summary(old),"after":next.map(event_summary)}))
        }).collect();
        let added = after
            .events
            .iter()
            .filter(|event| !before.events.iter().any(|old| old.id == event.id))
            .count();
        Ok(
            json!({"changed":changes.len(),"added":added,"removed":before.events.iter().filter(|old| !after.events.iter().any(|event| event.id==old.id)).count(),
            "beforeLength":before.length()?,"afterLength":after.length()?,"events":changes.into_iter().take(32).collect::<Vec<_>>()}),
        )
    }
}

fn event_summary(event: &Event) -> Value {
    json!({"at":event.at,"pitch":event.note.as_ref().map(|note| note.pitch),
        "duration":event.note.as_ref().map(|note| note.duration),
        "expressionPoints":event.note.as_ref().map_or(0, |note| note.expression.len()+note.master_expression.len()),
        "controls":event.controls.len()})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_reports_trim_without_mutating_and_rejects_stale_or_invalid_edits() {
        let mut app = App::new();
        app.apply(Request::Confirm).unwrap();
        app.apply(Request::Confirm).unwrap();
        let (id, source) = app.selected_phrase().unwrap();
        let mut phrase = source.clone();
        phrase.events.push(Event::note(
            EventId::new(app.editor.song().next_identity().unwrap()).unwrap(),
            Beat::integer(3),
            Target::Channel {
                channel: Channel::FIRST,
                endpoint: None,
            },
            Note::new(60, 100, Beat::ONE).unwrap(),
        ));
        app.command(Command::PutPhrase { id, phrase }).unwrap();
        let before = app.editor.song().clone();
        let revision = app.editor.revision();
        let command = Command::ResizePhrase {
            phrase: id,
            steps: 8,
            trim: true,
        };
        let preview = app.preview_edit(revision, command.clone()).unwrap();
        assert_eq!(preview["removed"], 1);
        assert_eq!(app.editor.song(), &before);
        assert_eq!(app.editor.revision(), revision);
        assert!(app.preview_edit(revision - 1, command).is_err());
        assert!(app
            .preview_edit(
                revision,
                Command::ResizePhrase {
                    phrase: id,
                    steps: 8,
                    trim: false
                }
            )
            .is_err());
        assert_eq!(app.editor.song(), &before);
    }
}
