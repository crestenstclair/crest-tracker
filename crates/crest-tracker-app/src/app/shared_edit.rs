use super::*;
use crest_tracker::editing::shared::{self, Definition, Location};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedChoice {
    Shared,
    Unique,
}

pub(super) struct PendingEdit {
    token: u64,
    revision: u64,
    location: Location,
    definition: Definition,
    can_make_unique: bool,
    command: Command,
}

impl App {
    pub(super) fn guard_shared_edit(&mut self, command: Command) -> Result<Command, DocumentError> {
        if self.pending_edit.is_some() {
            return Err(DocumentError::Invalid(
                "resolve the pending shared edit first",
            ));
        }
        let definitions = shared::affected_definitions(self.editor.song(), &command);
        let shared: Vec<_> = definitions
            .into_iter()
            .filter(|definition| shared::references(self.editor.song(), *definition).len() > 1)
            .collect();
        let Some(definition) = shared.first().copied() else {
            return Ok(command);
        };
        if shared.len() != 1 {
            return Err(DocumentError::Invalid(
                "edit shared definitions one at a time",
            ));
        }
        let location = self.edit_location(definition)?;
        let can_make_unique = !self.editor.has_recording_guard()
            && shared::edited_definitions(self.editor.song(), &command)
                == Some([definition].into_iter().collect());
        if let Some(choice) = self
            .shared_choice
            .filter(|choice| matches!(choice, SharedChoice::Shared) || can_make_unique)
        {
            return Ok(local_command(command, location, choice));
        }
        self.shared_token = self
            .shared_token
            .checked_add(1)
            .ok_or(DocumentError::Invalid("shared edit identity exhausted"))?;
        self.pending_edit = Some(PendingEdit {
            token: self.shared_token,
            revision: self.editor.revision(),
            location,
            definition,
            can_make_unique,
            command,
        });
        Err(DocumentError::Invalid("choose Edit Shared or Make Unique"))
    }

    fn edit_location(&self, definition: Definition) -> Result<Location, DocumentError> {
        let selection = self.selection();
        let location = Location {
            track: selection.track,
            chain_use: selection
                .chain_use
                .ok_or(DocumentError::Invalid("select the shared use first"))?,
            phrase_use: match definition {
                Definition::Phrase(_) => selection.phrase_use,
                Definition::Chain(_) => None,
            },
        };
        if shared::definition_at(self.editor.song(), location)? != definition {
            return Err(DocumentError::Invalid(
                "select the shared definition before editing",
            ));
        }
        Ok(location)
    }

    pub(super) fn resolve_shared_edit(
        &mut self,
        token: u64,
        choice: Option<SharedChoice>,
        remember: bool,
    ) -> Result<(), DocumentError> {
        let pending = self
            .pending_edit
            .as_ref()
            .ok_or(DocumentError::Invalid("no shared edit is pending"))?;
        if token != pending.token {
            return Err(DocumentError::Invalid("shared edit token is stale"));
        }
        if matches!(choice, Some(SharedChoice::Unique)) && !pending.can_make_unique {
            if self.editor.has_recording_guard() {
                return Err(DocumentError::Invalid(
                    "finish the retained take before making a phrase unique",
                ));
            }
            return Err(DocumentError::Invalid(
                "this edit also changes the arrangement; choose Edit Shared or Cancel",
            ));
        }
        let pending = self.pending_edit.take().expect("checked pending edit");
        let Some(choice) = choice else {
            self.status = "EDIT CANCELED".into();
            return Ok(());
        };
        self.checked_revision(pending.revision)?;
        if self.edit_location(pending.definition)? != pending.location {
            return Err(DocumentError::Invalid(
                "shared edit selection changed; reopen the editor",
            ));
        }
        self.commit_command(local_command(pending.command, pending.location, choice))?;
        if matches!(choice, SharedChoice::Unique) {
            self.clear_event_selection();
        }
        if remember {
            self.shared_choice = Some(choice);
        }
        Ok(())
    }

    pub(crate) fn shared_edit_projection(&self) -> Value {
        self.pending_edit.as_ref().map_or(Value::Null, |pending| {
            json!({
                "token":pending.token, "revision":pending.revision, "definition":pending.definition,
                "canMakeUnique":pending.can_make_unique,
                "recordingRestricted":self.editor.has_recording_guard(),
                "references":shared::references(self.editor.song(), pending.definition)
            })
        })
    }

    #[cfg(test)]
    pub(super) fn shared_references_projection(&self) -> Value {
        let definition = if self.view == View::Chain {
            self.selected_chain().map(|(id, _)| Definition::Chain(id))
        } else {
            self.selected_phrase().map(|(id, _)| Definition::Phrase(id))
        };
        definition.map_or(Value::Null, |definition| {
            json!({ "definition":definition,
            "references":shared::references(self.editor.song(), definition) })
        })
    }
}

fn local_command(command: Command, location: Location, choice: SharedChoice) -> Command {
    match choice {
        SharedChoice::Shared => command,
        SharedChoice::Unique => Command::EditUnique {
            location,
            command: Box::new(command),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared_phrase() -> App {
        let mut app = App::new();
        app.command(Command::PutInstrument {
            id: InstrumentId::new(17).unwrap(),
            instrument: InstrumentUse {
                name: "Test".into(),
                resource: None,
            },
        })
        .unwrap();
        app.apply(Request::Confirm).unwrap();
        app.apply(Request::Confirm).unwrap();
        app.apply(Request::Adjust { delta: 1 }).unwrap();
        let mut track = app.editor.song().tracks[0].clone();
        let mut usage = track.slots[0].clone().unwrap();
        usage.id = UseId::new(app.editor.song().next_identity().unwrap()).unwrap();
        track.slots.push(Some(usage));
        app.command(Command::PutTrack { track }).unwrap();
        app
    }

    #[test]
    fn retained_capture_suspends_remembered_unique_choice_without_erasing_it() {
        let mut app = shared_phrase();
        app.shared_choice = Some(SharedChoice::Unique);
        let mut recorder = crest_tracker::recording::Recorder::default();
        recorder
            .arm(&app.editor, app.occurrence().unwrap())
            .unwrap();
        app.editor.set_recording_guard(recorder.edit_guard());
        let before = app.editor.song().clone();
        assert!(app.apply(Request::Adjust { delta: 1 }).is_err());
        let pending = app.shared_edit_projection();
        assert_eq!(pending["canMakeUnique"], false);
        assert_eq!(pending["recordingRestricted"], true);
        let token = pending["token"].as_u64().unwrap();
        assert!(app
            .resolve_shared_edit(token, Some(SharedChoice::Unique), false)
            .is_err());
        assert!(app
            .resolve_shared_edit(token, Some(SharedChoice::Shared), false)
            .is_err());
        assert_eq!(app.editor.song(), &before);
        assert!(matches!(app.shared_choice, Some(SharedChoice::Unique)));
        recorder.discard();
        app.editor.set_recording_guard(None);
        app.apply(Request::Adjust { delta: 1 }).unwrap();
        assert_ne!(app.editor.song(), &before);
        app.editor.undo().unwrap();
        assert_eq!(app.editor.song(), &before);
    }

    #[test]
    fn local_phrase_edit_clones_shared_parent_and_undo_restores_whole_transaction() {
        let mut app = shared_phrase();
        let before = app.editor.song().clone();
        let original = app.selected_phrase().unwrap().0;
        assert!(app.apply(Request::Adjust { delta: 1 }).is_err());
        assert_eq!(app.editor.song(), &before);
        assert_eq!(
            app.shared_edit_projection()["references"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let token = app.pending_edit.as_ref().unwrap().token;
        app.resolve_shared_edit(token, Some(SharedChoice::Unique), true)
            .unwrap();
        let (private, phrase) = app.selected_phrase().unwrap();
        assert_ne!(private, original);
        assert_ne!(
            phrase.events[0].note,
            before.phrases[&original].events[0].note
        );
        assert_ne!(phrase.events[0].id, before.phrases[&original].events[0].id);
        assert_eq!(
            app.editor.song().phrases[&original],
            before.phrases[&original]
        );
        assert_eq!(
            app.editor.song().tracks[0].slots[1],
            before.tracks[0].slots[1]
        );
        app.editor.song().validate().unwrap();
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &before);
        // Remembered choice also guards later edits after undo restores sharing.
        app.apply(Request::Adjust { delta: 1 }).unwrap();
        assert!(app.pending_edit.is_none());
        assert_ne!(app.selected_phrase().unwrap().0, original);
    }

    #[test]
    fn shared_choice_updates_all_uses_and_stale_or_canceled_decisions_do_not_edit() {
        let mut app = shared_phrase();
        let before = app.editor.song().clone();
        assert!(app.apply(Request::Adjust { delta: 1 }).is_err());
        let token = app.pending_edit.as_ref().unwrap().token;
        assert!(app
            .resolve_shared_edit(token + 1, Some(SharedChoice::Shared), false)
            .is_err());
        assert_eq!(app.editor.song(), &before);
        app.resolve_shared_edit(token, None, false).unwrap();
        assert_eq!(app.editor.song(), &before);
        assert!(app.apply(Request::Adjust { delta: 1 }).is_err());
        let token = app.pending_edit.as_ref().unwrap().token;
        app.resolve_shared_edit(token, Some(SharedChoice::Shared), false)
            .unwrap();
        assert_eq!(app.editor.song().phrases.len(), before.phrases.len());
        assert_eq!(app.editor.song().chains, before.chains);
        assert_ne!(app.editor.song().phrases, before.phrases);
        assert!(app.apply(Request::Adjust { delta: 1 }).is_err());
        let token = app.pending_edit.as_ref().unwrap().token;
        app.apply(Request::Undo).unwrap();
        assert!(app
            .resolve_shared_edit(token, Some(SharedChoice::Shared), false)
            .is_err());
        assert_eq!(app.editor.song(), &before);
    }

    #[test]
    fn mixed_edit_requires_shared_consent_even_with_remembered_unique_choice() {
        let mut app = shared_phrase();
        let before = app.editor.song().clone();
        let (id, phrase) = app.selected_phrase().unwrap();
        let mut phrase = phrase.clone();
        phrase.name = "Updated".into();
        let mut track = before.tracks[0].clone();
        track.name = "Updated track".into();
        app.shared_choice = Some(SharedChoice::Unique);
        assert!(app
            .command(Command::Batch {
                commands: vec![
                    Command::PutPhrase { id, phrase },
                    Command::PutTrack { track }
                ],
            })
            .is_err());
        assert_eq!(app.editor.song(), &before);
        assert_eq!(app.shared_edit_projection()["canMakeUnique"], false);
        let token = app.pending_edit.as_ref().unwrap().token;
        assert!(app
            .resolve_shared_edit(token, Some(SharedChoice::Unique), false)
            .is_err());
        assert!(app.pending_edit.is_some());
        assert_eq!(app.editor.song(), &before);
        app.resolve_shared_edit(token, Some(SharedChoice::Shared), false)
            .unwrap();
        assert_eq!(app.editor.song().phrases[&id].name, "Updated");
        assert_eq!(app.editor.song().tracks[0].name, "Updated track");
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &before);
    }

    #[test]
    fn local_chain_append_preserves_other_uses_and_allocates_valid_ids() {
        let mut app = shared_phrase();
        app.open_view(View::Chain);
        let before = app.editor.song().clone();
        let original = app.selected_chain().unwrap().0;
        assert!(app.apply(Request::AppendPhrase { phrase: None }).is_err());
        let token = app.pending_edit.as_ref().unwrap().token;
        app.resolve_shared_edit(token, Some(SharedChoice::Unique), false)
            .unwrap();
        assert_eq!(app.selected_chain().unwrap().1.phrases.len(), 2);
        assert_eq!(
            app.editor.song().chains[&original],
            before.chains[&original]
        );
        app.editor.song().validate().unwrap();
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &before);
    }
}
