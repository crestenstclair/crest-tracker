use super::*;
use crate::app::ChordLibraryAction;
use crest_tracker::chord_bank::ChordRevisionId;

impl Runtime {
    pub(super) fn chord_library_action(
        &mut self,
        action: ChordLibraryAction,
    ) -> Result<(), String> {
        if self.chord_library_worker.is_some() {
            return Err("Chord library operation is still running".into());
        }
        let chord = match action {
            ChordLibraryAction::Refresh => None,
            ChordLibraryAction::Publish { revision, source } => {
                let app = self.app.borrow();
                app.checked_revision(revision)
                    .map_err(|error| error.to_string())?;
                let (id, derived) = app
                    .chosen_chord(source)
                    .map_err(|error| error.to_string())?;
                Some(
                    derived
                        .or_else(|| app.editor.song().chords.get(&id).cloned())
                        .ok_or("Chord revision missing")?,
                )
            }
            ChordLibraryAction::Adopt { revision, digest } => {
                let entry = self
                    .library_chords
                    .iter()
                    .find(|entry| entry.digest == digest)
                    .ok_or("Refresh the library and choose an existing revision")?;
                let mut app = self.app.borrow_mut();
                app.checked_revision(revision)
                    .map_err(|error| error.to_string())?;
                let id = ChordRevisionId::new(
                    app.editor
                        .song()
                        .next_identity()
                        .map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
                app.command(Command::PutChord {
                    id,
                    revision: entry.chord.clone(),
                })
                .map_err(|error| error.to_string())?;
                app.status = format!("CHORD ADOPTED · {} · revision {id}", entry.chord.name());
                return Ok(());
            }
        };
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let root = library::data_directory().join("chords");
        std::thread::spawn(move || {
            let result = (|| {
                if let Some(chord) = chord {
                    library::chords::publish(&root, chord)?;
                }
                library::chords::list(&root)
            })();
            let _ = send.send(result);
        });
        self.chord_library_worker = Some(receive);
        self.app.borrow_mut().chord_library =
            serde_json::json!({"entries":self.library_chords,"loading":true});
        Ok(())
    }

    pub(super) fn poll_chord_library(&mut self) {
        let Some(result) = self.chord_library_worker.as_ref().and_then(poll_once) else {
            return;
        };
        self.chord_library_worker = None;
        let error = match result {
            Ok(entries) => {
                self.library_chords = entries;
                None
            }
            Err(error) => Some(error),
        };
        let mut app = self.app.borrow_mut();
        app.chord_library =
            serde_json::json!({"entries":self.library_chords,"loading":false,"error":error});
        app.status = error.unwrap_or_else(|| {
            format!(
                "CHORD LIBRARY · {} revisions available across Songs",
                self.library_chords.len()
            )
        });
    }
}
