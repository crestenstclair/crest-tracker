//! Edits and undo go through the versioned library. No MIDI reconstruction or
//! conversion into tracker rows is involved in opening a clip.
use super::*;
use crate::performance_editor::ClipEditor;
use crest_performance_core::{BeatTime, Command as PerformanceCommand, NoteEdit, NoteKey};

impl Runtime {
    pub(in crate::synth) fn open_performance_editor(
        &mut self,
        sound: &mut Sound<'_>,
        slot: usize,
    ) -> Result<(), String> {
        if self.pending_open.is_some() || self.loading.is_some() || self.capture_baseline.is_some()
        {
            return Err("Finish the current document or take operation first".into());
        }
        self.finish_performance_input(sound, false)?;
        if self
            .performance_playback
            .as_ref()
            .is_some_and(|p| p.contains(slot))
        {
            self.stop_performance_slot(sound, slot)?;
        }
        self.refresh_performance_editor(slot, None, false)?;
        crate::workspace::open_performance_editor(&mut self.app.borrow_mut());
        Ok(())
    }

    fn refresh_performance_editor(
        &mut self,
        slot: usize,
        selected: Option<NoteKey>,
        can_undo: bool,
    ) -> Result<(), String> {
        let clip = self
            .performance
            .as_ref()
            .ok_or("Performance session unavailable")?
            .clip(slot)
            .map_err(|e| e.to_string())?
            .ok_or("Record a clip first")?;
        let notes = clip.notes().map_err(|e| e.to_string())?;
        let selected = selected
            .filter(|id| notes.iter().any(|note| note.id == *id))
            .or_else(|| notes.first().map(|note| note.id));
        let token = self
            .performance_edit_token
            .checked_add(1)
            .ok_or("Performance editor revision exhausted")?;
        let display = self
            .app
            .borrow()
            .performance_editor
            .as_ref()
            .filter(|editor| editor.slot == slot)
            .map(|editor| editor.view)
            .unwrap_or_default();
        let view = ClipEditor {
            token,
            view: display,
            slot,
            length: clip.length,
            notes,
            selected,
            can_undo,
        };
        self.performance_edit_token = token;
        self.app.borrow_mut().performance_editor = Some(view);
        Ok(())
    }

    fn require_performance_editor(
        &self,
        token: u64,
    ) -> Result<(usize, Option<NoteKey>, bool), String> {
        let app = self.app.borrow();
        let editor = app
            .performance_editor
            .as_ref()
            .filter(|view| view.token == token)
            .ok_or("Clip editor changed; select the note again")?;
        if self.performance_live.is_some() || self.pending_open.is_some() || self.loading.is_some()
        {
            return Err("Finish input and document operations before editing".into());
        }
        if self
            .performance_playback
            .as_ref()
            .is_some_and(|p| p.contains(editor.slot))
        {
            return Err("Stop this clip before editing".into());
        }
        Ok((editor.slot, editor.selected, editor.can_undo))
    }

    pub(in crate::synth) fn edit_performance_note(
        &mut self,
        token: u64,
        note: NoteKey,
        key: u8,
        start: BeatTime,
        duration: BeatTime,
    ) -> Result<(), String> {
        let (slot, _, _) = self.require_performance_editor(token)?;
        let previous = self
            .app
            .borrow()
            .performance_editor
            .as_ref()
            .and_then(|view| view.notes.iter().find(|n| n.id == note))
            .cloned()
            .ok_or("Selected note is no longer in this clip")?;
        if previous.key == key && previous.start == start && previous.duration == duration {
            return Ok(());
        }
        // Check the next publication before mutating, so exhaustion cannot leave
        // a successful edit hidden behind a stale view.
        self.performance_edit_token
            .checked_add(1)
            .ok_or("Performance editor revision exhausted")?;
        self.performance
            .as_mut()
            .ok_or("Performance session unavailable")?
            .command(PerformanceCommand::EditNote {
                slot,
                note,
                edit: NoteEdit {
                    start,
                    duration,
                    key,
                },
            })
            .map_err(|e| e.to_string())?;
        self.refresh_performance_editor(slot, Some(note), true)?;
        self.performance_edit_committed(
            "CLIP EDITED · Free timing and retained expression preserved",
        )
    }

    pub(in crate::synth) fn undo_performance_edit(&mut self, token: u64) -> Result<(), String> {
        let (slot, selected, can_undo) = self.require_performance_editor(token)?;
        if !can_undo {
            return Err("No clip edit to undo".into());
        }
        self.performance_edit_token
            .checked_add(1)
            .ok_or("Performance editor revision exhausted")?;
        self.performance
            .as_mut()
            .ok_or("Performance session unavailable")?
            .command(PerformanceCommand::Undo)
            .map_err(|e| e.to_string())?;
        self.refresh_performance_editor(slot, selected, false)?;
        self.performance_edit_committed("CLIP EDIT UNDONE · Original performance restored")
    }

    pub(in crate::synth) fn quantize_performance_clip(
        &mut self,
        token: u64,
        quantum: BeatTime,
    ) -> Result<(), String> {
        let (slot, selected, _) = self.require_performance_editor(token)?;
        self.performance_edit_token
            .checked_add(1)
            .ok_or("Performance editor revision exhausted")?;
        let engine = self
            .performance
            .as_mut()
            .ok_or("Performance session unavailable")?;
        let original = engine.clip(slot).map_err(|e| e.to_string())?.cloned();
        engine
            .command(PerformanceCommand::QuantizeClip { slot, quantum })
            .map_err(|e| e.to_string())?;
        if engine.clip(slot).map_err(|e| e.to_string())? == original.as_ref() {
            // A no-op must preserve both the editor revision and earlier Undo.
            self.app.borrow_mut().status = "CLIP UNCHANGED · Attacks already on grid".into();
            return Ok(());
        }
        self.refresh_performance_editor(slot, selected, true)?;
        self.performance_edit_committed(
            "ATTACKS QUANTIZED · Durations and expression retained · Undo restores free timing",
        )
    }

    pub(super) fn performance_edit_committed(&mut self, status: &str) -> Result<(), String> {
        self.checkpoint_dirty |= self.performance_recovery.is_some();
        let dirty = self.performance_document()? != self.saved_performance;
        let mut app = self.app.borrow_mut();
        app.performance_dirty = dirty;
        app.status = status.into();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Runtime {
        let app = Rc::new(RefCell::new(App::new()));
        let mut runtime = Runtime::with_data_directory(
            app,
            std::env::temp_dir().join(format!("crest-performance-edit-{}", uuid::Uuid::new_v4())),
        )
        .unwrap();
        let document = example_document().unwrap().performance.unwrap();
        runtime.saved_performance = Some(document.clone());
        runtime.performance =
            Some(Performance::new(document, Geometry::Pads200, Limits::default()).unwrap());
        runtime.refresh_performance_editor(0, None, false).unwrap();
        runtime
    }

    #[test]
    fn clip_edit_retains_exact_curves_reopens_and_undo_restores_authoritative_stream() {
        let mut runtime = fixture();
        let original = runtime.performance_document().unwrap();
        runtime
            .app
            .borrow_mut()
            .performance_editor
            .as_mut()
            .unwrap()
            .view = crate::performance_editor::ClipView::Tracker;
        let view = runtime.app.borrow().performance_editor.clone().unwrap();
        let note = view.notes[0].clone();
        let start = BeatTime(note.start.0 + 37);
        runtime
            .edit_performance_note(view.token, note.id, note.key + 1, start, note.duration)
            .unwrap();
        let edited = runtime.app.borrow().performance_editor.clone().unwrap();
        assert!(edited.can_undo);
        assert_eq!(edited.view, crate::performance_editor::ClipView::Tracker);
        let changed = edited.notes.iter().find(|n| n.id == note.id).unwrap();
        assert_eq!((changed.start, changed.key), (start, note.key + 1));
        assert_eq!(changed.expression, note.expression);
        assert_eq!(
            (changed.velocity, changed.release_velocity, changed.duration),
            (note.velocity, note.release_velocity, note.duration)
        );
        assert!(view
            .notes
            .iter()
            .filter(|n| n.id != note.id)
            .all(|n| edited.notes.contains(n)));
        assert!(runtime.app.borrow().performance_dirty);
        let document = runtime.performance_document().unwrap().unwrap();
        let reopened: crest_tracker::performance::Document =
            serde_json::from_slice(&serde_json::to_vec(&document).unwrap()).unwrap();
        let reopened = Performance::new(reopened, Geometry::Pads200, Limits::default()).unwrap();
        assert_eq!(
            reopened.clip(0).unwrap().unwrap().notes().unwrap(),
            edited.notes
        );
        assert!(runtime
            .edit_performance_note(view.token, note.id, 65, start, note.duration)
            .is_err());
        assert_eq!(runtime.performance_document().unwrap(), Some(document));
        runtime.undo_performance_edit(edited.token).unwrap();
        assert_eq!(runtime.performance_document().unwrap(), original);
        assert!(!runtime.app.borrow().performance_dirty);
    }

    #[test]
    fn quantization_moves_whole_polyphonic_lifetimes_and_undo_survives_noop() {
        let mut runtime = fixture();
        let original = runtime.performance_document().unwrap().unwrap();
        let view = runtime.app.borrow().performance_editor.clone().unwrap();
        assert_eq!(
            view.notes
                .iter()
                .map(|note| note.start.0)
                .collect::<Vec<_>>(),
            [5_970, 15_370, 24_730]
        );
        runtime
            .quantize_performance_clip(view.token, BeatTime(240_000))
            .unwrap();
        let edited = runtime.app.borrow().performance_editor.clone().unwrap();
        assert_eq!(edited.selected, view.selected);
        assert_eq!(edited.view, view.view);
        assert_eq!(edited.length, view.length);
        let mut expected_notes = view.notes.clone();
        for note in &mut expected_notes {
            note.start = BeatTime(0);
        }
        // Exact equality includes identity, attack/release velocities, duration
        // and every expression offset/value. Simultaneous attacks stay polyphonic.
        assert_eq!(edited.notes, expected_notes);
        assert!(edited.can_undo);
        assert!(runtime.app.borrow().performance_dirty);
        let quantized = runtime.performance_document().unwrap().unwrap();
        let mut expected_metadata = original.clone();
        expected_metadata.session.slots[0]
            .clip
            .as_mut()
            .unwrap()
            .events = quantized.session.slots[0]
            .clip
            .as_ref()
            .unwrap()
            .events
            .clone();
        assert_eq!(quantized, expected_metadata);
        let reopened: crest_tracker::performance::Document =
            serde_json::from_slice(&serde_json::to_vec(&quantized).unwrap()).unwrap();
        let reopened = Performance::new(reopened, Geometry::Pads200, Limits::default()).unwrap();
        assert_eq!(
            reopened.clip(0).unwrap().unwrap().notes().unwrap(),
            expected_notes
        );

        assert!(runtime
            .quantize_performance_clip(view.token, BeatTime(480_000))
            .is_err());
        assert!(runtime
            .quantize_performance_clip(edited.token, BeatTime(0))
            .is_err());
        assert_eq!(
            runtime.performance_document().unwrap(),
            Some(quantized.clone())
        );
        runtime
            .quantize_performance_clip(edited.token, BeatTime(240_000))
            .unwrap();
        let unchanged = runtime.app.borrow().performance_editor.clone().unwrap();
        assert_eq!(unchanged.token, edited.token);
        assert!(unchanged.can_undo);
        runtime.undo_performance_edit(unchanged.token).unwrap();
        assert_eq!(runtime.performance_document().unwrap(), Some(original));
        assert!(!runtime.app.borrow().performance_dirty);
    }

    #[test]
    fn shorten_is_explicit_undoable_and_invalid_or_unchanged_edits_do_not_mutate() {
        let mut runtime = fixture();
        let original = runtime.performance_document().unwrap();
        let view = runtime.app.borrow().performance_editor.clone().unwrap();
        let note = view
            .notes
            .iter()
            .find(|n| n.expression.points.len() > 1)
            .unwrap();
        runtime
            .edit_performance_note(view.token, note.id, note.key, note.start, note.duration)
            .unwrap();
        assert_eq!(
            runtime
                .app
                .borrow()
                .performance_editor
                .as_ref()
                .unwrap()
                .token,
            view.token
        );
        assert_eq!(runtime.performance_document().unwrap(), original);
        assert!(runtime
            .edit_performance_note(view.token, note.id, 255, note.start, note.duration)
            .is_err());
        assert!(runtime
            .edit_performance_note(view.token, note.id, note.key, view.length, note.duration)
            .is_err());
        assert_eq!(runtime.performance_document().unwrap(), original);
        let shorter = note.expression.points.last().unwrap().offset;
        assert!(shorter.0 > 0);
        runtime
            .edit_performance_note(view.token, note.id, note.key, note.start, shorter)
            .unwrap();
        let edited = runtime.app.borrow().performance_editor.clone().unwrap();
        let changed = edited.notes.iter().find(|n| n.id == note.id).unwrap();
        assert!(changed.expression.points.len() < note.expression.points.len());
        assert!(changed.expression.points.iter().all(|p| p.offset < shorter));
        runtime.undo_performance_edit(edited.token).unwrap();
        assert_eq!(runtime.performance_document().unwrap(), original);
        runtime.app.borrow_mut().reset_document_editors();
        assert!(runtime
            .edit_performance_note(view.token, note.id, 61, note.start, note.duration)
            .is_err());
        assert_eq!(runtime.performance_document().unwrap(), original);
    }
}
