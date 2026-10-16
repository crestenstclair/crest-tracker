//! Drum lane identity and controller editing. Draft lanes are editor state;
//! their notes enter the ordinary composition transaction and undo history.
use super::*;

impl App {
    pub(super) fn drum_feedback_focus(&self) -> Option<crate::feedback::Focus> {
        let (phrase, lane, at) = self.focused_drum().ok()?;
        if self.feedback.surface() == crate::feedback::Surface::DrumEdit {
            let hits = self.editor.song().phrases[&phrase]
                .events
                .iter()
                .filter(|event| lane.matches(event, at))
                .map(|event| event.id)
                .collect();
            Some(crate::feedback::Focus::DrumEdit {
                phrase,
                step: self.row,
                lane: self.column,
                pitch: lane.pitch,
                target: lane.target,
                hits,
            })
        } else {
            Some(crate::feedback::Focus::Drum {
                phrase,
                step: self.row,
                lane: self.column,
                pitch: lane.pitch,
                target: lane.target,
            })
        }
    }

    pub(crate) fn is_drum(&self) -> bool {
        self.view == View::Phrase
            && self
                .selected_phrase()
                .is_some_and(|(_, phrase)| matches!(phrase.form, PhraseForm::Drum))
    }

    pub(super) fn drum_lanes(&self) -> Vec<DrumNote> {
        let Some((id, phrase)) = self
            .selected_phrase()
            .filter(|(_, phrase)| matches!(phrase.form, PhraseForm::Drum))
        else {
            return vec![];
        };
        let defaults = self.drum_drafts.get(&id).cloned().unwrap_or_else(|| {
            let target = self.default_target().unwrap_or(Target::Channel {
                channel: Channel::FIRST,
                endpoint: None,
            });
            [36, 38, 42, 46]
                .map(|pitch| DrumNote {
                    pitch,
                    target: target.clone(),
                    velocity: 100,
                    duration: Beat::new(1, 8).expect("valid duration"),
                })
                .to_vec()
        });
        let mut lanes = BTreeMap::new();
        for lane in defaults {
            if self.editor.song().validate_target(&lane.target).is_ok() {
                lanes.insert((lane.pitch, lane.target.clone()), lane);
            }
        }
        for event in &phrase.events {
            if let Some(note) = &event.note {
                lanes
                    .entry((note.pitch, event.target.clone()))
                    .or_insert_with(|| DrumNote {
                        pitch: note.pitch,
                        target: event.target.clone(),
                        velocity: note.velocity,
                        duration: note.duration,
                    });
            }
        }
        lanes.into_values().collect()
    }

    /// Keep an emptied custom lane reachable and retain the chosen lane after
    /// sorting. A new document clears these drafts, even if it reuses IDs.
    pub(super) fn remember_drum_lane(&mut self, from: Option<(u8, Target)>, draft: DrumNote) {
        let Some((id, _)) = self.selected_phrase() else {
            return;
        };
        let mut lanes = self.drum_lanes();
        lanes.retain(|lane| {
            let replaced = from
                .as_ref()
                .is_some_and(|(pitch, target)| lane.pitch == *pitch && lane.target == *target);
            let destination = lane.pitch == draft.pitch && lane.target == draft.target;
            !(replaced || destination)
        });
        lanes.push(draft.clone());
        self.drum_drafts.insert(id, lanes);
        self.column = self
            .drum_lanes()
            .iter()
            .position(|lane| lane.pitch == draft.pitch && lane.target == draft.target)
            .unwrap_or(0);
    }

    pub(super) fn add_drum_lane(
        &mut self,
        revision: u64,
        occurrence: Occurrence,
        draft: DrumNote,
    ) -> Result<(), DocumentError> {
        let id = self.checked_source(revision, occurrence)?;
        if !matches!(self.editor.song().phrases[&id].form, PhraseForm::Drum) {
            return Err(DocumentError::Invalid(
                "lane settings require a Drum phrase",
            ));
        }
        self.editor.song().validate_target(&draft.target)?;
        Note::new(draft.pitch, draft.velocity, draft.duration)?;
        self.remember_drum_lane(None, draft);
        self.status = "DRUM LANE READY · Confirm a step to add its first note".into();
        Ok(())
    }

    fn focused_drum(&self) -> Result<(PhraseId, DrumNote, Beat), DocumentError> {
        let (id, phrase) = self
            .selected_phrase()
            .ok_or(DocumentError::Invalid("select a Drum phrase"))?;
        let lane = self
            .drum_lanes()
            .get(self.column)
            .cloned()
            .ok_or(DocumentError::Invalid("select a Drum lane"))?;
        let at = phrase
            .division
            .beat_length()
            .checked_scale(self.row as u64, 1)?;
        Ok((id, lane, at))
    }

    pub(super) fn drum_confirm(&mut self, clear_only: bool) -> Result<(), DocumentError> {
        let (id, lane, at) = self.focused_drum()?;
        if clear_only
            && !self.editor.song().phrases[&id]
                .events
                .iter()
                .any(|event| lane.matches(event, at))
        {
            return Ok(());
        }
        self.drum_toggle(
            self.editor.revision(),
            self.occurrence()
                .ok_or(DocumentError::Invalid("select a phrase occurrence"))?,
            self.row as u32,
            lane,
        )
    }

    pub(super) fn adjust_drum_velocity(&mut self, delta: i32) -> Result<(), DocumentError> {
        let (id, lane, at) = self.focused_drum()?;
        let commands = self.editor.song().phrases[&id]
            .events
            .iter()
            .filter(|event| lane.matches(event, at))
            .map(|event| {
                let mut event = event.clone();
                let note = event.note.as_mut().expect("matched note");
                note.velocity = i32::from(note.velocity).saturating_add(delta).clamp(1, 127) as u8;
                Command::PutEvent { phrase: id, event }
            })
            .collect::<Vec<_>>();
        if commands.is_empty() {
            return Ok(());
        }
        self.command(Command::Batch { commands })
    }
}

impl DrumNote {
    fn matches(&self, event: &Event, at: Beat) -> bool {
        event.at == at
            && event.target == self.target
            && event
                .note
                .as_ref()
                .is_some_and(|note| note.pitch == self.pitch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reduce_queued(app: &mut App) {
        for request in std::mem::take(&mut app.requests) {
            app.apply(request).unwrap();
        }
    }

    fn app() -> App {
        let mut app = App::new();
        app.command(Command::PutInstrument {
            id: InstrumentId::new(17).unwrap(),
            instrument: InstrumentUse {
                name: "Drums".into(),
                resource: None,
            },
        })
        .unwrap();
        app.apply(Request::Confirm).unwrap();
        app.apply(Request::Confirm).unwrap();
        let (id, source) = app.selected_phrase().unwrap();
        let mut phrase = source.clone();
        phrase.form = PhraseForm::Drum;
        phrase.steps = 130;
        app.command(Command::PutPhrase { id, phrase }).unwrap();
        app
    }

    fn add_lane(app: &mut App, channel: u8) -> Result<(), DocumentError> {
        app.apply(Request::DrumAddLane {
            revision: app.editor.revision(),
            occurrence: app.occurrence().unwrap(),
            pitch: 36,
            target: Target::Instrument {
                id: InstrumentId::new(17).unwrap(),
                channel: Channel::new(channel).unwrap(),
            },
            velocity: 93,
            duration: Beat::new(1, 8).unwrap(),
        })
    }

    #[test]
    fn drum_edit_feedback_fences_insert_clear_undo_redo_and_excludes_navigation() {
        let mut app = app();
        app.feedback.enable(crate::feedback::Surface::DrumEdit);
        app.queue(json!({"kind":"move","horizontal":1,"vertical":0}));
        assert_eq!(app.requests.len(), 1);
        for request in std::mem::take(&mut app.requests) {
            app.apply(request).unwrap();
        }
        let initial_revision = app.editor.revision();
        for (index, (kind, count)) in [("confirm", 1), ("clear", 0), ("undo", 1), ("redo", 0)]
            .into_iter()
            .enumerate()
        {
            app.queue(json!({"kind":kind}));
            assert_eq!(app.requests.len(), 2);
            assert!(app.feedback.projection()["pending"].is_null());
            reduce_queued(&mut app);
            let pending = app.feedback.projection()["pending"].clone();
            assert_eq!(
                pending["state"]["revision"],
                initial_revision + index as u64 + 1
            );
            assert_eq!(pending["state"]["focus"]["kind"], "drum_edit");
            assert_eq!(pending["state"]["focus"]["step"], 1);
            assert_eq!(
                pending["state"]["focus"]["hits"].as_array().unwrap().len(),
                count
            );
            app.queue(json!({"kind":"feedback_painted","sequence":pending["sequence"],"state":pending["state"],
                "bounds":[100,100,36,36],"viewport":[1280,800]}));
            assert_eq!(app.feedback.projection()["samples"], index + 1);
        }
        app.queue(json!({"kind":"clear"}));
        for request in std::mem::take(&mut app.requests) {
            app.apply(request).unwrap();
        }
        assert_eq!(app.feedback.report(true)["no_changes"], 1);
    }

    #[test]
    fn drum_controller_selects_exact_lane_and_step_with_one_undo_per_velocity_gesture() {
        let mut app = app();
        let (id, source) = app.selected_phrase().unwrap();
        let mut phrase = source.clone();
        let target = Target::Instrument {
            id: InstrumentId::new(17).unwrap(),
            channel: Channel::new(2).unwrap(),
        };
        let near = Event::note(
            EventId::new(1000).unwrap(),
            Beat::new(160_000_000_001, 10_000_000_000).unwrap(),
            target.clone(),
            Note::new(36, 77, Beat::new(1, 8).unwrap()).unwrap(),
        );
        let mut other = Event::note(
            EventId::new(1001).unwrap(),
            Beat::new(16, 1).unwrap(),
            Target::instrument(InstrumentId::new(17).unwrap()),
            Note::new(36, 88, Beat::new(1, 8).unwrap()).unwrap(),
        );
        other.controls.push(Control::Pressure { value: 73 });
        other.fx[0] = Some(Fx::Velocity { value: 65 });
        phrase.events = vec![near.clone(), other.clone()];
        app.command(Command::PutPhrase { id, phrase }).unwrap();
        let baseline = app.editor.song().clone();
        let revision = app.editor.revision();
        add_lane(&mut app, 2).unwrap();
        assert_eq!(app.editor.revision(), revision);
        assert_eq!(app.editor.song(), &baseline);
        app.apply(Request::Move {
            horizontal: 64,
            vertical: 0,
        })
        .unwrap();
        assert_eq!((app.row, app.column), (64, 1));
        assert_eq!(app.projection()["rowStart"], 64);
        assert_eq!(app.projection()["drumLanes"][1]["target"], json!(target));
        app.apply(Request::Confirm).unwrap();
        let inserted = app.editor.song().clone();
        let event = inserted.phrases[&id].events.last().unwrap();
        assert_eq!(event.at, Beat::new(16, 1).unwrap());
        assert_eq!(event.target, target);
        assert_eq!(event.note.as_ref().unwrap().pitch, 36);
        assert_eq!(event.note.as_ref().unwrap().velocity, 93);
        for delta in [5, 7] {
            app.apply(Request::AdjustGesture { delta, gesture: 99 })
                .unwrap();
        }
        assert_eq!(
            app.editor.song().phrases[&id]
                .events
                .iter()
                .find(|candidate| candidate.id == event.id)
                .unwrap()
                .note
                .as_ref()
                .unwrap()
                .velocity,
            105
        );
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &inserted);
        app.apply(Request::Clear).unwrap();
        assert_eq!(app.editor.song(), &baseline);
        let revision = app.editor.revision();
        app.apply(Request::Clear).unwrap();
        assert_eq!(app.editor.revision(), revision);
        app.apply(Request::Confirm).unwrap();
        app.apply(Request::Move {
            horizontal: 0,
            vertical: -1,
        })
        .unwrap();
        app.apply(Request::Clear).unwrap();
        let events = &app.editor.song().phrases[&id].events;
        assert_eq!(events[0], near);
        assert!(events[1].note.is_none());
        assert_eq!(events[1].controls, other.controls);
        assert_eq!(events[1].fx, other.fx);
        assert_eq!(events[2].target, target);
    }

    #[test]
    fn drum_custom_lane_survives_emptying_and_navigation_but_not_document_replacement() {
        let mut app = app();
        add_lane(&mut app, 10).unwrap();
        let selected = app.column;
        app.apply(Request::Confirm).unwrap();
        app.apply(Request::Confirm).unwrap();
        assert_eq!(app.column, selected);
        assert!(app.selected_phrase().unwrap().1.events.is_empty());
        assert_eq!(app.drum_lanes()[selected].target.channel().get(), 10);
        app.apply(Request::Move {
            horizontal: i32::MAX,
            vertical: i32::MAX,
        })
        .unwrap();
        assert_eq!((app.row, app.column), (129, 4));
        app.apply(Request::Move {
            horizontal: i32::MIN,
            vertical: i32::MIN,
        })
        .unwrap();
        assert_eq!((app.row, app.column), (0, 0));
        app.apply(Request::Back).unwrap();
        app.apply(Request::Confirm).unwrap();
        assert_eq!(app.drum_lanes().len(), 5);
        let before = app.projection()["drumLanes"].clone();
        assert!(app
            .apply(Request::DrumAddLane {
                revision: app.editor.revision() + 1,
                occurrence: app.occurrence().unwrap(),
                pitch: 60,
                target: Target::instrument(InstrumentId::new(17).unwrap()),
                velocity: 100,
                duration: Beat::ONE
            })
            .is_err());
        assert_eq!(app.projection()["drumLanes"], before);
        app.reset_document_editors();
        assert_eq!(app.drum_lanes().len(), 4);
    }

    #[test]
    fn drum_confirm_shared_copy_retains_lane_and_leaves_other_use_unchanged() {
        let mut app = app();
        let source = app.selected_phrase().unwrap().0;
        let mut track = app.editor.song().tracks[0].clone();
        let mut usage = track.slots[0].clone().unwrap();
        usage.id = UseId::new(9000).unwrap();
        track.slots.push(Some(usage));
        app.command(Command::PutTrack { track }).unwrap();
        add_lane(&mut app, 10).unwrap();
        let baseline = app.editor.song().clone();
        assert!(app.apply(Request::Confirm).is_err());
        assert_eq!(app.editor.song(), &baseline);
        let token = app.projection()["sharedEdit"]["token"].as_u64().unwrap();
        app.resolve_shared_edit(token, Some(SharedChoice::Unique), false)
            .unwrap();
        let (copied, phrase) = app.selected_phrase().unwrap();
        assert_ne!(copied, source);
        assert!(app.editor.song().phrases[&source].events.is_empty());
        assert_eq!(phrase.events.len(), 1);
        assert_eq!(phrase.events[0].note.as_ref().unwrap().pitch, 36);
        assert_eq!(phrase.events[0].target.channel().get(), 10);
        assert_eq!(app.drum_lanes()[app.column].target.channel().get(), 10);
        app.apply(Request::Undo).unwrap();
        assert_eq!(app.editor.song(), &baseline);
        assert_eq!(app.drum_lanes()[app.column].target.channel().get(), 10);
    }

    #[test]
    fn drum_feedback_fences_native_lane_identity_after_navigation() {
        let mut app = app();
        add_lane(&mut app, 2).unwrap();
        let baseline = app.editor.song().clone();
        app.feedback.enable(crate::feedback::Surface::Drum);
        app.queue(json!({"kind":"move","horizontal":64,"vertical":0}));
        assert!(app.feedback.projection()["pending"].is_null());
        for request in std::mem::take(&mut app.requests) {
            app.apply(request).unwrap();
        }
        let state = &app.feedback.projection()["pending"]["state"];
        assert_eq!(state["focus"]["kind"], "drum");
        assert_eq!(state["focus"]["step"], 64);
        assert_eq!(state["focus"]["lane"], 1);
        assert_eq!(state["focus"]["pitch"], 36);
        assert_eq!(state["focus"]["target"]["channel"], 2);
        assert_eq!(app.editor.song(), &baseline);
    }
}
