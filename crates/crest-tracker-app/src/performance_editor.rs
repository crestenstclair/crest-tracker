//! Transient selection and immutable note projection, separate from Song phrases.
use crest_performance_core::{BeatTime, Note, NoteKey};

#[derive(Debug, Clone)]
pub(crate) struct ClipEditor {
    pub token: u64,
    pub slot: usize,
    pub length: BeatTime,
    pub notes: Vec<Note>,
    pub selected: Option<NoteKey>,
    pub can_undo: bool,
}
impl ClipEditor {
    pub fn selected_note(&self) -> Option<&Note> {
        self.notes
            .iter()
            .find(|note| Some(note.id) == self.selected)
    }
    pub fn select_next(&mut self, delta: i32) {
        if self.notes.is_empty() {
            return;
        }
        let index = self
            .notes
            .iter()
            .position(|note| Some(note.id) == self.selected)
            .unwrap_or(0);
        let index =
            (index as i64 + i64::from(delta)).clamp(0, self.notes.len() as i64 - 1) as usize;
        self.selected = Some(self.notes[index].id);
    }
}

pub(crate) fn note_id(note: NoteKey) -> String {
    format!(
        "performance_note:{}:{}:{}",
        note.source.0, note.track.0, note.note.0
    )
}
