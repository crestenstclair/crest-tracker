//! Presentation-only roll window. Musical data, edit grid and cursor remain independent.
use super::*;

#[derive(Clone, Copy, Debug)]
pub(crate) struct PianoViewport {
    pub start: Beat,
    pub width: Beat,
    pub low_pitch: u8,
    pub follow: bool,
    pub scale_guide: bool,
}

impl Default for PianoViewport {
    fn default() -> Self {
        Self {
            start: Beat::ZERO,
            width: Beat::integer(8),
            low_pitch: 59,
            follow: false,
            scale_guide: true,
        }
    }
}

impl PianoViewport {
    pub fn reveal(&mut self, at: Beat, length: Beat) {
        let width = self.width.min(length);
        let end = self.start.checked_add(width).unwrap_or(length);
        if at < self.start || at >= end {
            // Half-window overlap keeps nearby musical context visible.
            let stride = width.checked_scale(1, 2).expect("bounded viewport width");
            self.start = stride
                .checked_scale(at.units(stride).unwrap_or(0), 1)
                .unwrap_or(Beat::ZERO);
        }
        self.start = self
            .start
            .min(length.checked_sub(width).unwrap_or(Beat::ZERO));
    }

    pub fn reveal_pitch(&mut self, pitch: u8) {
        if pitch < self.low_pitch {
            self.low_pitch = pitch;
        }
        if pitch > self.low_pitch + 12 {
            self.low_pitch = pitch - 12;
        }
        self.low_pitch = self.low_pitch.min(115);
    }
}

impl App {
    pub(crate) fn reveal_piano_cursor(&mut self) {
        if let Some((_, phrase)) = self.selected_phrase().filter(|_| self.is_piano_roll()) {
            let length = phrase.length().expect("validated phrase");
            self.roll_view.reveal(self.roll_cursor, length);
            self.roll_view.reveal_pitch(self.roll_pitch);
        }
    }

    pub(crate) fn follow_piano_playback(&mut self) {
        if self.is_piano_roll() && self.roll_view.follow && self.playing {
            if let (Some(at), Some((_, phrase))) = (self.play_phrase_beat, self.selected_phrase()) {
                let length = phrase.length().expect("validated phrase");
                self.roll_view.reveal(at, length);
            }
        }
    }

    /// At most 64 visual cells. Tiny edit grids never allocate unbounded UI rows;
    /// the exact edit cursor and notes are still projected at their true positions.
    pub(crate) fn piano_paint_step(&self) -> Beat {
        let minimum = self
            .roll_view
            .width
            .checked_scale(1, 63)
            .expect("bounded viewport");
        let mut step = self.roll_grid.min(self.roll_view.width);
        // Coarsen by musical multiples, preserving straight/triplet ruler marks.
        // Dividing the window into 63 equal cells hides almost every beat label.
        while step < minimum {
            step = step.checked_scale(2, 1).unwrap_or(self.roll_view.width);
        }
        let length = self
            .selected_phrase()
            .and_then(|(_, phrase)| phrase.length().ok())
            .unwrap_or(self.roll_view.width);
        let representable_end = length
            .units(step)
            .ok()
            .and_then(|cells| cells.checked_add(1))
            .and_then(|cells| step.checked_scale(cells, 1).ok());
        if representable_end.is_some() {
            step
        } else {
            // Arbitrary exact entry fractions may overflow when multiplied into
            // absolute cell times. Only drawing falls back to a binary ruler.
            self.roll_view
                .width
                .checked_scale(1, 32)
                .expect("bounded viewport")
        }
    }

    pub(super) fn piano_zoom(&mut self, delta: i32) -> Result<(), DocumentError> {
        let width = if delta > 0 {
            self.roll_view.width.checked_scale(1, 2)?
        } else {
            self.roll_view.width.checked_scale(2, 1)?
        };
        self.roll_view.width = width.max(Beat::new(1, 4)?).min(Beat::integer(64));
        self.roll_view.follow = false;
        self.reveal_piano_cursor();
        Ok(())
    }

    pub(super) fn piano_scroll(&mut self, delta: i32) -> Result<(), DocumentError> {
        let length = self.selected_phrase().expect("piano phrase").1.length()?;
        let amount = self
            .roll_view
            .width
            .checked_scale(u64::from(delta.unsigned_abs()), 2)?;
        let start = if delta < 0 {
            self.roll_view
                .start
                .checked_sub(amount)
                .unwrap_or(Beat::ZERO)
        } else {
            self.roll_view.start.checked_add(amount)?
        };
        self.roll_view.start = start.min(
            length
                .checked_sub(self.roll_view.width)
                .unwrap_or(Beat::ZERO),
        );
        self.roll_view.follow = false;
        Ok(())
    }

    pub(super) fn piano_next_note(&mut self, delta: i32) -> Result<(), DocumentError> {
        let (_, phrase) = self.selected_phrase().expect("piano phrase");
        let mut notes: Vec<_> = phrase.events.iter().filter(|e| e.note.is_some()).collect();
        notes.sort_by_key(|e| (e.at, e.note.as_ref().unwrap().pitch, e.id));
        if notes.is_empty() {
            return Err(DocumentError::Invalid("no notes in this phrase"));
        }
        let selected = self.selected_event_ids();
        let index = match notes.iter().position(|e| selected.contains(&e.id)) {
            Some(index) => {
                (index as i64 + i64::from(delta.signum())).rem_euclid(notes.len() as i64) as usize
            }
            None if delta < 0 => notes
                .iter()
                .rposition(|e| e.at <= self.roll_cursor)
                .unwrap_or(notes.len() - 1),
            None => notes
                .iter()
                .position(|e| e.at >= self.roll_cursor)
                .unwrap_or(0),
        };
        let id = notes[index].id;
        self.select_events(
            self.editor.revision(),
            self.occurrence().expect("piano occurrence"),
            vec![id],
            false,
        )?;
        self.roll_view.follow = false;
        self.reveal_piano_cursor();
        Ok(())
    }
}
