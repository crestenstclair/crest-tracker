//! Transient selection and immutable note projection, separate from Song phrases.
use crest_performance_core::{BeatTime, ConditionalNote, Note, NoteCondition, NoteKey};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ClipView {
    #[default]
    PianoRoll,
    Tracker,
}
impl ClipView {
    pub fn label(self) -> &'static str {
        match self {
            Self::PianoRoll => "Piano roll",
            Self::Tracker => "Tracker",
        }
    }
    pub fn toggle(&mut self) {
        *self = match self {
            Self::PianoRoll => Self::Tracker,
            Self::Tracker => Self::PianoRoll,
        };
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ClipEditor {
    pub token: u64,
    pub view: ClipView,
    pub slot: usize,
    pub length: BeatTime,
    pub notes: Vec<Note>,
    pub conditions: Vec<ConditionalNote>,
    pub selected: Option<NoteKey>,
    pub can_undo: bool,
}
impl ClipEditor {
    pub fn condition(&self, note: NoteKey) -> NoteCondition {
        self.conditions
            .iter()
            .find(|entry| entry.note == note)
            .map_or(NoteCondition::default(), |entry| entry.condition)
    }
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
