use super::*;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PianoValue {
    Pitch,
    Velocity,
    ReleaseVelocity,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PianoControl {
    Select,
    NextNote {
        delta: i32,
    },
    Zoom {
        delta: i32,
    },
    Scroll {
        delta: i32,
    },
    PitchScroll {
        octaves: i32,
    },
    ScaleGuide {
        enabled: bool,
    },
    Follow {
        enabled: bool,
    },
    Velocity {
        delta: i32,
    },
    Grid {
        grid: Beat,
    },
    Value {
        value: PianoValue,
    },
    NewLength {
        length: Beat,
    },
    NewVelocity {
        velocity: u8,
    },
    Defaults {
        length: Beat,
        velocity: u8,
        target: Option<Target>,
    },
    Cursor {
        at: Beat,
        pitch: u8,
    },
    Navigate {
        horizontal: i32,
        vertical: i32,
    },
    Nudge {
        delta: i32,
    },
    Resize {
        delta: i32,
    },
    Adjust {
        delta: i32,
    },
}

impl App {
    pub(crate) fn piano_cursor(&self) -> (Beat, u8) {
        (self.roll_cursor, self.roll_pitch)
    }
    pub(super) fn focus_piano_source(&mut self, at: Beat) {
        if !self.is_piano_roll() {
            return;
        }
        let pitch = self.selected_phrase().and_then(|(_, phrase)| {
            phrase
                .events
                .iter()
                .find(|event| event.at == at && event.note.is_some())
                .and_then(|event| event.note.as_ref())
                .map(|note| note.pitch)
        });
        self.roll_cursor = at;
        if let Some(pitch) = pitch {
            self.roll_pitch = pitch;
        }
        self.roll_range_anchor = None;
        self.reveal_piano_cursor();
    }

    /// A source can become shorter through edits, Undo, or changing phrases.
    /// Return an out-of-bounds cursor to the exact start without retiming notes.
    pub(super) fn reconcile_piano_cursor(&mut self) {
        if !self.is_piano_roll() {
            return;
        }
        let length = self
            .selected_phrase()
            .expect("piano phrase")
            .1
            .length()
            .expect("validated phrase");
        if self.roll_cursor >= length {
            self.roll_cursor = Beat::ZERO;
            self.roll_range_anchor = None;
            self.reveal_piano_cursor();
        }
        if self.roll_range_anchor.is_some_and(|(at, _)| at >= length) {
            self.roll_range_anchor = None;
        }
    }

    pub(crate) fn is_piano_roll(&self) -> bool {
        self.view == View::Phrase
            && self
                .selected_phrase()
                .is_some_and(|(_, phrase)| matches!(phrase.form, PhraseForm::PianoRoll))
    }

    pub(super) fn piano_control(&mut self, control: PianoControl) -> Result<(), DocumentError> {
        if !self.is_piano_roll()
            && !matches!(
                control,
                PianoControl::Grid { .. }
                    | PianoControl::Value { .. }
                    | PianoControl::NewLength { .. }
                    | PianoControl::NewVelocity { .. }
                    | PianoControl::Defaults { .. }
            )
        {
            return Err(DocumentError::Invalid("open a Piano Roll phrase first"));
        }
        match control {
            PianoControl::Select => self.select_cursor_for_edit()?,
            PianoControl::NextNote { delta } => self.piano_next_note(delta)?,
            PianoControl::Zoom { delta } => self.piano_zoom(delta)?,
            PianoControl::Scroll { delta } => self.piano_scroll(delta)?,
            PianoControl::PitchScroll { octaves } => {
                self.roll_view.low_pitch = i32::from(self.roll_view.low_pitch)
                    .saturating_add(octaves.saturating_mul(12))
                    .clamp(0, 115) as u8;
                self.roll_view.follow = false;
            }
            PianoControl::ScaleGuide { enabled } => self.roll_view.scale_guide = enabled,
            PianoControl::Follow { enabled } => {
                self.roll_view.follow = enabled;
                self.follow_piano_playback();
            }
            PianoControl::Velocity { delta } => {
                let previous = self.roll_value;
                self.roll_value = PianoValue::Velocity;
                let result = self.adjust_piano_notes(delta, false);
                self.roll_value = previous;
                result?;
            }
            PianoControl::Grid { grid } => {
                if grid.is_zero() {
                    return Err(DocumentError::Invalid("edit grid must be positive"));
                }
                self.roll_grid = grid;
            }
            PianoControl::Value { value } => self.roll_value = value,
            PianoControl::NewLength { length } => {
                if length.is_zero() {
                    return Err(DocumentError::Invalid("new note length must be positive"));
                }
                self.roll_new_length = length;
            }
            PianoControl::NewVelocity { velocity } => {
                if !(1..=127).contains(&velocity) {
                    return Err(DocumentError::Invalid("new note velocity must be 1–127"));
                }
                self.roll_new_velocity = velocity;
            }
            PianoControl::Defaults {
                length,
                velocity,
                target,
            } => {
                if length.is_zero() || !(1..=127).contains(&velocity) {
                    return Err(DocumentError::Invalid(
                        "new note defaults require positive length and velocity 1–127",
                    ));
                }
                if let Some(target) = &target {
                    self.editor.song().validate_target(target)?;
                }
                self.roll_new_length = length;
                self.roll_new_velocity = velocity;
                self.roll_new_target = target;
            }
            PianoControl::Cursor { at, pitch } => {
                if pitch > 127 || at >= self.selected_phrase().expect("piano phrase").1.length()? {
                    return Err(DocumentError::Invalid(
                        "cursor is outside phrase or MIDI range",
                    ));
                }
                self.roll_cursor = at;
                self.roll_pitch = pitch;
                self.roll_range_anchor = None;
                self.event_selection = None;
                self.roll_view.follow = false;
                self.reveal_piano_cursor();
            }
            PianoControl::Navigate {
                horizontal,
                vertical,
            } => self.move_piano_cursor(horizontal, vertical)?,
            PianoControl::Nudge { delta } => {
                self.select_cursor_for_edit()?;
                let mut amount = self
                    .piano_step()?
                    .checked_scale(u64::from(delta.unsigned_abs()), 1)?;
                let ids = self.selected_event_ids();
                let (_, phrase) = self.selected_phrase().expect("piano phrase");
                let starts: Vec<_> = phrase
                    .events
                    .iter()
                    .filter(|e| ids.contains(&e.id))
                    .map(|e| e.at)
                    .collect();
                let first = starts
                    .iter()
                    .min()
                    .copied()
                    .ok_or(DocumentError::Invalid("select notes to edit"))?;
                let last = starts.iter().max().copied().unwrap();
                let step = self.piano_step()?;
                let length = phrase.length()?;
                let units = length.units(step)?;
                let last_grid = step.checked_scale(units, 1)?;
                let last_grid = if last_grid == length {
                    last_grid.checked_sub(step)?
                } else {
                    last_grid
                };
                amount = amount.min(if delta < 0 {
                    first
                } else {
                    last_grid.checked_sub(last).unwrap_or(Beat::ZERO)
                });
                if amount.is_zero() {
                    return Ok(());
                }
                self.edit_selection(
                    self.editor.revision(),
                    self.occurrence().expect("piano occurrence"),
                    SelectionAction::Move {
                        amount,
                        backward: delta < 0,
                    },
                )?;
                if let Some(event) = self
                    .selected_phrase()
                    .unwrap()
                    .1
                    .events
                    .iter()
                    .find(|e| ids.contains(&e.id))
                {
                    self.roll_cursor = event.at;
                }
                self.roll_view.follow = false;
                self.reveal_piano_cursor();
            }
            PianoControl::Resize { delta } => self.adjust_piano_notes(delta, true)?,
            PianoControl::Adjust { delta } => self.adjust_piano_notes(delta, false)?,
        }
        Ok(())
    }

    fn piano_step(&self) -> Result<Beat, DocumentError> {
        Ok(if self.fine_held {
            self.roll_grid.checked_scale(1, 16)?
        } else {
            self.roll_grid
        })
    }

    fn move_piano_cursor(&mut self, horizontal: i32, vertical: i32) -> Result<(), DocumentError> {
        let (id, phrase) = self.selected_phrase().expect("piano phrase");
        let length = phrase.length()?;
        let anchor = self
            .roll_range_anchor
            .unwrap_or((self.roll_cursor, self.roll_pitch));
        let amount = self
            .piano_step()?
            .checked_scale(u64::from(horizontal.unsigned_abs()), 1)?;
        let at = if horizontal < 0 {
            self.roll_cursor.checked_sub(amount).unwrap_or(Beat::ZERO)
        } else {
            self.roll_cursor.checked_add(amount)?
        };
        self.roll_cursor = if at < length {
            at
        } else {
            self.roll_cursor
                .min(length.checked_sub(self.piano_step()?.min(length))?)
        };
        self.roll_pitch = i32::from(self.roll_pitch)
            .saturating_sub(vertical)
            .clamp(0, 127) as u8;
        self.roll_view.follow = false;
        self.reveal_piano_cursor();
        if self.range_held {
            self.roll_range_anchor = Some(anchor);
            let end = anchor
                .0
                .max(self.roll_cursor)
                .checked_add(self.piano_step()?)?
                .min(length);
            let events = self.editor.song().phrases[&id]
                .events
                .iter()
                .filter(|event| {
                    event.at >= anchor.0.min(self.roll_cursor)
                        && event.at < end
                        && event.note.as_ref().is_some_and(|note| {
                            (anchor.1.min(self.roll_pitch)..=anchor.1.max(self.roll_pitch))
                                .contains(&note.pitch)
                        })
                })
                .map(|event| event.id)
                .collect();
            self.event_selection = Some(EventSelection { phrase: id, events });
        } else {
            self.roll_range_anchor = None;
            self.event_selection = None;
        }
        self.status = format!(
            "CURSOR · {}/{} beats · {}",
            self.roll_cursor.numerator(),
            self.roll_cursor.denominator(),
            Pitch::new(self.roll_pitch)
                .expect("valid pitch")
                .name(Spelling::Sharps)
        );
        Ok(())
    }

    /// Stored notes under the exact cursor, in source order for selection cycling.
    pub(crate) fn piano_notes_at_cursor(&self) -> Vec<EventId> {
        let (_, source) = self.selected_phrase().expect("piano phrase");
        source
            .events
            .iter()
            .filter(|event| {
                event.note.as_ref().is_some_and(|note| {
                    note.pitch == self.roll_pitch
                        && event.at <= self.roll_cursor
                        && event
                            .at
                            .checked_add(note.duration)
                            .is_ok_and(|end| end > self.roll_cursor)
                })
            })
            .map(|event| event.id)
            .collect()
    }

    pub(super) fn piano_confirm(&mut self) -> Result<(), DocumentError> {
        let (phrase, source) = self.selected_phrase().expect("piano phrase");
        let ids = self.selected_event_ids();
        let occupied = self.piano_notes_at_cursor();
        if !occupied.is_empty() {
            let index = occupied
                .iter()
                .position(|id| ids.contains(id))
                .map_or(0, |index| (index + 1) % occupied.len());
            return self.select_events(
                self.editor.revision(),
                self.occurrence().expect("piano occurrence"),
                vec![occupied[index]],
                false,
            );
        }
        if self.roll_cursor >= source.length()? {
            return Err(DocumentError::Invalid(
                "move cursor inside phrase before inserting",
            ));
        }
        let event = Event::note(
            EventId::new(self.editor.song().next_identity()?)?,
            self.roll_cursor,
            self.roll_new_target
                .clone()
                .map_or_else(|| self.default_target(), Ok)?,
            Note::new(
                self.roll_pitch,
                self.roll_new_velocity,
                self.roll_new_length,
            )?,
        );
        let id = event.id;
        self.command(Command::PutEvent { phrase, event })?;
        self.event_selection = Some(EventSelection {
            phrase,
            events: vec![id],
        });
        Ok(())
    }

    fn select_cursor_for_edit(&mut self) -> Result<(), DocumentError> {
        if self.selected_event_ids().is_empty() {
            let ids = self.piano_notes_at_cursor();
            if let Some(id) = ids.first() {
                let cursor = self.piano_cursor();
                let viewport = self.roll_view;
                self.select_events(
                    self.editor.revision(),
                    self.occurrence().expect("piano occurrence"),
                    vec![*id],
                    false,
                )?;
                self.roll_cursor = cursor.0;
                self.roll_pitch = cursor.1;
                self.roll_view = viewport;
            }
        }
        Ok(())
    }

    pub(super) fn clear_piano_notes(&mut self) -> Result<(), DocumentError> {
        self.select_cursor_for_edit()?;
        if !self.selected_event_ids().is_empty() {
            self.edit_selection(
                self.editor.revision(),
                self.occurrence().expect("piano occurrence"),
                SelectionAction::Delete,
            )?;
        }
        Ok(())
    }

    fn adjust_piano_notes(&mut self, delta: i32, resize: bool) -> Result<(), DocumentError> {
        self.select_cursor_for_edit()?;
        let (phrase, source) = self.selected_phrase().expect("piano phrase");
        let ids = self.selected_event_ids();
        if ids.is_empty() {
            return Err(DocumentError::Invalid("select notes to edit"));
        }
        let mut commands = Vec::new();
        for event in source.events.iter().filter(|event| ids.contains(&event.id)) {
            let Some(note) = &event.note else {
                continue;
            };
            if resize {
                let amount = self
                    .piano_step()?
                    .checked_scale(u64::from(delta.unsigned_abs()), 1)?;
                let duration = if delta < 0 {
                    note.duration.checked_sub(amount)?
                } else {
                    note.duration.checked_add(amount)?
                };
                if duration.is_zero() {
                    return Err(DocumentError::Invalid("note length must stay positive"));
                }
                commands.push(Command::ResizeNotes {
                    phrase,
                    events: vec![event.id],
                    duration,
                });
            } else if matches!(self.roll_value, PianoValue::Pitch) {
                // Existing transpose preserves note-relative poly-pressure ownership.
                return self.edit_selection(
                    self.editor.revision(),
                    self.occurrence().expect("piano occurrence"),
                    SelectionAction::Transpose {
                        semitones: i16::try_from(delta)
                            .map_err(|_| DocumentError::Invalid("pitch change is too large"))?,
                    },
                );
            } else {
                let mut event = event.clone();
                let note = event.note.as_mut().expect("selected note");
                match self.roll_value {
                    PianoValue::Velocity => {
                        note.velocity =
                            i32::from(note.velocity).saturating_add(delta).clamp(1, 127) as u8
                    }
                    PianoValue::ReleaseVelocity => {
                        note.release_velocity = i32::from(note.release_velocity)
                            .saturating_add(delta)
                            .clamp(0, 127) as u8
                    }
                    PianoValue::Pitch => unreachable!(),
                }
                commands.push(Command::PutEvent { phrase, event });
            }
        }
        self.command(Command::Batch { commands })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> App {
        let mut app = App::new();
        let instrument = InstrumentId::new(17).unwrap();
        app.command(Command::PutInstrument {
            id: instrument,
            instrument: InstrumentUse {
                name: "Lead".into(),
                resource: None,
            },
        })
        .unwrap();
        app.apply(Request::Confirm).unwrap();
        app.apply(Request::Confirm).unwrap();
        let (id, source) = app.selected_phrase().unwrap();
        let mut phrase = source.clone();
        phrase.form = PhraseForm::PianoRoll;
        let mut note = Note::new(60, 100, Beat::new(1, 2).unwrap()).unwrap();
        note.expression.push(Expression {
            offset: Beat::new(1, 10).unwrap(),
            control: Control::Pressure { value: 37 },
            sequence: None,
        });
        phrase.events.push(Event::note(
            EventId::new(app.editor.song().next_identity().unwrap()).unwrap(),
            Beat::new(103, 100).unwrap(),
            Target::instrument(instrument),
            note,
        ));
        app.command(Command::PutPhrase { id, phrase }).unwrap();
        let event = app.selected_phrase().unwrap().1.events[0].id;
        app.select_events(
            app.editor.revision(),
            app.occurrence().unwrap(),
            vec![event],
            false,
        )
        .unwrap();
        app
    }
    #[test]
    fn zoom_scroll_follow_and_grid_preserve_music_and_edit_cursor() {
        let mut app = fixture();
        let phrase = app.selected_phrase().unwrap().0;
        app.command(Command::ResizePhrase {
            phrase,
            steps: 400,
            trim: false,
        })
        .unwrap();
        app.piano_control(PianoControl::Cursor {
            at: Beat::integer(40),
            pitch: 73,
        })
        .unwrap();
        let before = app.editor.song().clone();
        let cursor = app.piano_cursor();
        app.piano_control(PianoControl::Zoom { delta: 1 }).unwrap();
        app.piano_control(PianoControl::Scroll { delta: 1 })
            .unwrap();
        app.piano_control(PianoControl::Grid {
            grid: Beat::new(1, 3).unwrap(),
        })
        .unwrap();
        assert_eq!(app.roll_view.width, Beat::integer(4));
        assert_eq!(app.roll_view.start, Beat::integer(42));
        app.playing = true;
        app.play_phrase_beat = Some(Beat::integer(75));
        app.piano_control(PianoControl::Follow { enabled: true })
            .unwrap();
        assert!(app.roll_view.start <= Beat::integer(75));
        assert!(
            app.roll_view
                .start
                .checked_add(app.roll_view.width)
                .unwrap()
                > Beat::integer(75)
        );
        assert_eq!(app.piano_cursor(), cursor);
        assert_eq!(app.editor.song(), &before);
        app.piano_control(PianoControl::Navigate {
            horizontal: 1,
            vertical: 0,
        })
        .unwrap();
        assert!(!app.roll_view.follow);
        assert!(app.roll_view.start <= app.piano_cursor().0);
    }

    #[test]
    fn pitch_scroll_moves_one_lane_and_next_note_reaches_identical_overlaps() {
        let mut app = fixture();
        app.piano_control(PianoControl::Cursor {
            at: Beat::ZERO,
            pitch: 72,
        })
        .unwrap();
        assert_eq!(app.roll_view.low_pitch, 60);
        app.piano_control(PianoControl::Navigate {
            horizontal: 0,
            vertical: -1,
        })
        .unwrap();
        assert_eq!(app.roll_view.low_pitch, 61);
        let (phrase, source) = app.selected_phrase().unwrap();
        let mut other = source.events[0].clone();
        other.id = EventId::new(app.editor.song().next_identity().unwrap()).unwrap();
        let second = other.id;
        app.command(Command::PutEvent {
            phrase,
            event: other,
        })
        .unwrap();
        let first = app.selected_phrase().unwrap().1.events[0].id;
        app.piano_control(PianoControl::NextNote { delta: 1 })
            .unwrap();
        assert_eq!(app.selected_event_ids(), vec![first]);
        app.piano_control(PianoControl::NextNote { delta: 1 })
            .unwrap();
        assert_eq!(app.selected_event_ids(), vec![second]);
        app.piano_control(PianoControl::NextNote { delta: -1 })
            .unwrap();
        assert_eq!(app.selected_event_ids(), vec![first]);
    }

    #[test]
    fn boundary_nudge_clamps_without_changing_duration_or_expression() {
        let mut app = fixture();
        let note = app.selected_phrase().unwrap().1.events[0].note.clone();
        app.piano_control(PianoControl::Nudge { delta: -100 })
            .unwrap();
        assert_eq!(app.selected_phrase().unwrap().1.events[0].at, Beat::ZERO);
        app.piano_control(PianoControl::Nudge { delta: -1 })
            .unwrap();
        app.piano_control(PianoControl::Nudge { delta: 100 })
            .unwrap();
        let phrase = app.selected_phrase().unwrap().1;
        assert_eq!(
            phrase.events[0].at,
            phrase.length().unwrap().checked_sub(app.roll_grid).unwrap()
        );
        assert_eq!(phrase.events[0].note, note);
        assert_eq!(app.piano_cursor().0, phrase.events[0].at);
    }

    #[test]
    fn clear_at_empty_piano_cursor_never_deletes_another_pitch() {
        let mut app = fixture();
        let at = app.selected_phrase().unwrap().1.events[0].at;
        let before = app.editor.song().clone();
        app.piano_control(PianoControl::Cursor { at, pitch: 61 })
            .unwrap();
        app.apply(Request::Clear).unwrap();
        assert_eq!(app.editor.song(), &before);
        app.piano_control(PianoControl::Cursor { at, pitch: 60 })
            .unwrap();
        app.apply(Request::Clear).unwrap();
        assert!(app.selected_phrase().unwrap().1.events.is_empty());
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &before);
    }

    #[test]
    fn confirm_admission_distinguishes_existing_sources_and_notes_from_creation() {
        let mut app = fixture();
        let song = app.editor.song().clone();
        let revision = app.editor.revision();
        assert!(!app.confirm_edits_song());
        app.apply(Request::Confirm).unwrap();
        assert_eq!(app.editor.revision(), revision);
        app.roll_cursor = Beat::ZERO;
        assert!(
            app.confirm_edits_song(),
            "empty Piano cell would insert a note"
        );
        app.open_view(View::Chain);
        assert!(!app.confirm_edits_song());
        app.row = app.selected_chain().unwrap().1.phrases.len();
        assert!(app.confirm_edits_song(), "append row would create a phrase");
        app.open_view(View::Song);
        assert!(!app.confirm_edits_song());
        app.slot = app.editor.song().tracks[app.track].slots.len();
        assert!(
            app.confirm_edits_song(),
            "empty Song slot would create a chain"
        );
        assert_eq!(app.editor.song(), &song);
    }

    #[test]
    fn document_replacement_resets_piano_cursor_and_range_anchor_even_with_reused_ids() {
        let mut app = fixture();
        let song = app.editor.song().clone();
        app.roll_range_anchor = Some((app.roll_cursor, app.roll_pitch));
        assert!(!app.roll_cursor.is_zero());
        app.replace_song(song).unwrap();
        assert_eq!(app.roll_cursor, Beat::ZERO);
        assert_eq!(app.roll_pitch, 60);
        assert!(app.roll_range_anchor.is_none());
        assert!(app.selected_event_ids().is_empty());
    }

    #[test]
    fn shrinking_phrase_keeps_piano_cursor_reachable_and_confirm_usable() {
        let mut app = fixture();
        let (phrase, source) = app.selected_phrase().unwrap();
        app.roll_cursor = source
            .length()
            .unwrap()
            .checked_sub(Beat::new(1, 4).unwrap())
            .unwrap();
        app.roll_range_anchor = Some((app.roll_cursor, app.roll_pitch));
        app.command(Command::ResizePhrase {
            phrase,
            steps: 4,
            trim: true,
        })
        .unwrap();
        assert!(app.roll_cursor < app.selected_phrase().unwrap().1.length().unwrap());
        assert!(app.roll_range_anchor.is_none());
        app.apply(Request::Confirm).unwrap();
        assert_eq!(app.selected_event_ids().len(), 1);
        assert_eq!(
            app.selected_phrase().unwrap().1.events[0].at,
            app.roll_cursor
        );
    }

    #[test]
    fn fine_grid_nudges_raw_notes_exactly_and_coalesces_one_undo() {
        let mut app = fixture();
        let before = app.editor.song().clone();
        app.piano_control(PianoControl::Grid {
            grid: Beat::new(1, 4).unwrap(),
        })
        .unwrap();
        assert_eq!(app.editor.song(), &before);
        app.fine_held = true;
        for _ in 0..2 {
            app.apply(Request::PianoGesture {
                control: PianoControl::Nudge { delta: 1 },
                gesture: 1,
            })
            .unwrap();
        }
        let event = &app.selected_phrase().unwrap().1.events[0];
        assert_eq!(
            event.at,
            Beat::new(103, 100)
                .unwrap()
                .checked_add(Beat::new(1, 32).unwrap())
                .unwrap()
        );
        assert_eq!(
            event.note.as_ref().unwrap().expression[0].offset,
            Beat::new(1, 10).unwrap()
        );
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &before);
    }
    #[test]
    fn occupied_confirm_selects_without_mutating_and_invalid_resize_is_atomic() {
        let mut app = fixture();
        let before = app.editor.song().clone();
        app.piano_confirm().unwrap();
        assert_eq!(app.editor.song(), &before);
        assert!(app
            .piano_control(PianoControl::Resize { delta: -3 })
            .is_err());
        assert_eq!(app.editor.song(), &before);
        app.piano_control(PianoControl::Resize { delta: 1 })
            .unwrap();
        assert_eq!(
            app.selected_phrase().unwrap().1.events[0]
                .note
                .as_ref()
                .unwrap()
                .duration,
            Beat::new(3, 4).unwrap()
        );
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &before);
    }
    #[test]
    fn range_navigation_preserves_exact_pitch_and_time_rectangle() {
        let mut app = fixture();
        app.piano_control(PianoControl::Cursor {
            at: Beat::ONE,
            pitch: 59,
        })
        .unwrap();
        app.range_held = true;
        app.piano_control(PianoControl::Navigate {
            horizontal: 1,
            vertical: -2,
        })
        .unwrap();
        assert_eq!(app.selected_event_ids().len(), 1);
        assert_eq!(app.roll_pitch, 61);
        assert_eq!(app.roll_cursor, Beat::new(5, 4).unwrap());
        app.piano_control(PianoControl::Navigate {
            horizontal: -1,
            vertical: 2,
        })
        .unwrap();
        assert!(app.selected_event_ids().is_empty());
    }
}
