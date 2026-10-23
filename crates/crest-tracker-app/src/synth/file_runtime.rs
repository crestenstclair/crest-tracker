//! Correlated file interaction. The active Song is retained until the existing
//! worker/graph replacement path succeeds; closing a chooser never discards it.
use super::file_dialog::{FileDialog, Mode};
use super::*;
use serde_json::json;

pub(super) enum Purpose {
    File {
        action: FileAction,
        approved: bool,
    },
    Repair {
        index: usize,
        replace: bool,
    },
    Adopt {
        instrument: InstrumentId,
        revision: u64,
    },
    Replace {
        action: FileAction,
    },
    Recover,
}
pub(super) struct ActiveDialog {
    token: u64,
    purpose: Purpose,
    browser: Option<FileDialog>,
    prompt: Value,
}

impl Runtime {
    pub(super) fn load_progress(&self) -> Option<&'static str> {
        if self.loading.is_some() {
            Some("Reading song and checking assets…")
        } else if self.pending_open.is_some() {
            Some("Preparing instruments and effects…")
        } else if self.recovery_preparing.is_some() || self.repairing_resource.is_some() {
            Some("Checking song assets…")
        } else {
            None
        }
    }

    pub(super) fn project_loading(&mut self) {
        let progress = self.load_progress();
        let mut app = self.app.borrow_mut();
        if progress.is_some() && app.load_progress.is_none() {
            app.edit_held = false;
            app.shift_held = false;
            app.fine_held = false;
            app.range_held = false;
            crate::workspace::cancel_edit(&mut app);
        }
        app.load_progress = progress;
    }

    fn next_dialog_token(&mut self) -> Result<u64, String> {
        if self.dialog_token >= 9_007_199_254_740_991 {
            return Err("File dialog identity exhausted; restart the application".into());
        }
        self.dialog_token += 1;
        Ok(self.dialog_token)
    }

    pub(super) fn show_file_browser(
        &mut self,
        purpose: Purpose,
        title: String,
        mode: Mode,
        initial: PathBuf,
        extensions: &[&str],
    ) -> Result<(), String> {
        if self.dialog.is_some() {
            return Err("Finish or cancel the current file dialog".into());
        }
        let token = self.next_dialog_token()?;
        let browser = FileDialog::new(
            token,
            title,
            mode,
            initial,
            extensions.iter().map(|v| (*v).into()).collect(),
        );
        self.dialog = Some(ActiveDialog {
            token,
            purpose,
            browser: Some(browser),
            prompt: Value::Null,
        });
        crate::workspace::cancel_edit(&mut self.app.borrow_mut());
        self.project_file_dialog();
        Ok(())
    }

    pub(super) fn show_replace_prompt(&mut self, action: FileAction) -> Result<(), String> {
        self.show_prompt(
            Purpose::Replace { action },
            "Unsaved Song",
            "Save this Song before continuing?",
            json!([
                {"value":"save","label":"Save Song, then continue"},
                {"value":"discard","label":"Discard unsaved Song changes"},
                {"value":"cancel","label":"Cancel"}
            ]),
        )
    }

    pub(super) fn show_recovery_prompt(&mut self) -> Result<(), String> {
        self.show_prompt(
            Purpose::Recover,
            "Recover interrupted take",
            "Replace the current unsaved Song with its recovery checkpoint? Cancel to save first.",
            json!([
                {"value":"cancel","label":"Cancel and keep current Song"},
                {"value":"discard","label":"Discard current changes and recover checkpoint"}
            ]),
        )
    }

    fn show_prompt(
        &mut self,
        purpose: Purpose,
        title: &str,
        message: &str,
        choices: Value,
    ) -> Result<(), String> {
        if self.dialog.is_some() {
            return Err("Finish or cancel the current file dialog".into());
        }
        let token = self.next_dialog_token()?;
        let prompt = json!({"token":token,"version":1,"title":title,"mode":"confirm","message":message,"choices":choices});
        self.dialog = Some(ActiveDialog {
            token,
            purpose,
            browser: None,
            prompt,
        });
        crate::workspace::cancel_edit(&mut self.app.borrow_mut());
        self.project_file_dialog();
        Ok(())
    }

    pub(super) fn project_file_dialog(&mut self) {
        self.app.borrow_mut().file_dialog = self.dialog.as_ref().map_or(Value::Null, |dialog| {
            dialog
                .browser
                .as_ref()
                .map_or_else(|| dialog.prompt.clone(), FileDialog::projection)
        });
    }

    pub(super) fn file_browse(&mut self, token: u64, directory: String) -> Result<(), String> {
        self.dialog
            .as_mut()
            .and_then(|dialog| dialog.browser.as_mut())
            .ok_or("No file browser is open")?
            .navigate(token, directory)
    }

    pub(super) fn file_choose(
        &mut self,
        token: u64,
        path: String,
        overwrite: bool,
    ) -> Result<(), String> {
        self.dialog
            .as_mut()
            .and_then(|dialog| dialog.browser.as_mut())
            .ok_or("No file browser is open")?
            .choose(token, path, overwrite)
    }

    pub(super) fn file_cancel(&mut self, token: u64) -> Result<(), String> {
        if self
            .dialog
            .as_ref()
            .is_none_or(|dialog| dialog.token != token)
        {
            return Err("File dialog is stale".into());
        }
        self.dialog = None;
        self.after_save = None;
        self.app.borrow_mut().status = "FILE ACTION CANCELED · Current Song retained".into();
        self.project_file_dialog();
        Ok(())
    }

    pub(super) fn file_decision(
        &mut self,
        sound: &mut Sound<'_>,
        token: u64,
        decision: &str,
    ) -> Result<(), String> {
        let dialog = self.dialog.as_ref().ok_or("No file confirmation is open")?;
        if dialog.token != token || dialog.browser.is_some() {
            return Err("File confirmation is stale".into());
        }
        if !dialog.prompt["choices"]
            .as_array()
            .is_some_and(|choices| choices.iter().any(|choice| choice["value"] == decision))
        {
            return Err("Unknown file confirmation choice".into());
        }
        if decision == "cancel" {
            return self.file_cancel(token);
        }
        let dialog = self.dialog.take().expect("validated dialog");
        self.project_file_dialog();
        match dialog.purpose {
            Purpose::Replace { action } if decision == "save" => {
                self.after_save = Some(action);
                if let Err(error) = self.file(sound, FileAction::Save) {
                    self.after_save = None;
                    return Err(error);
                }
                Ok(())
            }
            Purpose::Replace { action } if decision == "discard" => {
                self.file_at(sound, action, None, true).map(|_| ())
            }
            Purpose::Recover if decision == "discard" => {
                self.recovery_action_confirmed(sound, "recover", true)
            }
            _ => Err("Unknown file confirmation choice".into()),
        }
    }

    pub(super) fn poll_file_dialog(&mut self, sound: &mut Sound<'_>) -> Result<(), String> {
        let selected = self
            .dialog
            .as_mut()
            .and_then(|dialog| dialog.browser.as_mut())
            .and_then(FileDialog::poll);
        self.project_file_dialog();
        let Some(path) = selected else { return Ok(()) };
        let dialog = self.dialog.take().expect("selection has an active dialog");
        self.project_file_dialog();
        let result = match dialog.purpose {
            Purpose::File { action, approved } => self
                .file_at(sound, action, Some(path), approved)
                .map(|_| ()),
            Purpose::Repair { index, replace } => self.start_resource_repair(index, replace, path),
            Purpose::Adopt {
                instrument,
                revision,
            } => self.start_sound_adoption(instrument, revision, path),
            _ => Err("File selection has no matching operation".into()),
        };
        if result.is_err() {
            self.after_save = None;
        }
        result
    }

    pub(super) fn file_initial_path(&self, filename: &str) -> PathBuf {
        self.app
            .borrow()
            .path
            .as_ref()
            .and_then(|path| path.parent())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(filename)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn loading_projection_spans_reader_and_sound_activation_then_unlocks() {
        let mut runtime = super::super::tests::capture_fixture();
        {
            let mut app = runtime.app.borrow_mut();
            app.edit_held = true;
            app.shift_held = true;
            app.fine_held = true;
            app.range_held = true;
        }
        let missing = runtime.data_directory.join("missing.crest-song");
        runtime.loading = Some((LoadWorker::start(missing.clone(), false), missing));
        runtime.project_loading();
        assert_eq!(
            runtime.app.borrow().load_progress,
            Some("Reading song and checking assets…")
        );
        {
            let app = runtime.app.borrow();
            assert!(!app.edit_held && !app.shift_held && !app.fine_held && !app.range_held);
        }
        runtime.loading = None;
        runtime.pending_open = Some((demo::document().unwrap(), None));
        runtime.project_loading();
        assert_eq!(
            runtime.app.borrow().load_progress,
            Some("Preparing instruments and effects…")
        );
        runtime.pending_open = None;
        runtime.project_loading();
        assert!(runtime.app.borrow().load_progress.is_none());
    }

    #[test]
    fn cancel_is_correlated_and_retains_song_take_and_path() {
        let mut runtime = super::super::tests::capture_fixture();
        let before = runtime.app.borrow().editor.song().clone();
        let state = runtime.recorder.checkpoint();
        runtime.show_replace_prompt(FileAction::Open).unwrap();
        let token = runtime.dialog_token;
        assert_eq!(runtime.app.borrow().file_dialog["mode"], "confirm");
        assert!(runtime.file_cancel(token + 1).is_err());
        assert!(runtime.dialog.is_some());
        runtime.after_save = Some(FileAction::Close);
        runtime.file_cancel(token).unwrap();
        assert!(runtime.dialog.is_none());
        assert!(runtime.after_save.is_none());
        assert!(runtime.app.borrow().file_dialog.is_null());
        assert_eq!(runtime.app.borrow().editor.song(), &before);
        assert_eq!(
            serde_json::to_value(runtime.recorder.checkpoint()).unwrap(),
            serde_json::to_value(state).unwrap()
        );
        runtime.show_recovery_prompt().unwrap();
        assert!(runtime.dialog_token > token);
        assert!(runtime.file_cancel(token).is_err());
    }
    #[test]
    fn one_modal_owns_file_input_even_in_synth_view() {
        let mut runtime = super::super::tests::capture_fixture();
        runtime.app.borrow_mut().view = View::Instruments;
        assert!(!runtime.owns_input());
        runtime.show_replace_prompt(FileAction::New).unwrap();
        assert!(runtime.owns_input());
        let projected = runtime.app.borrow().file_dialog.clone();
        assert!(runtime.show_recovery_prompt().is_err());
        assert_eq!(runtime.app.borrow().file_dialog, projected);
        let token = runtime.dialog_token;
        runtime.file_cancel(token).unwrap();
        assert!(!runtime.owns_input());
    }
}
