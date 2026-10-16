//! Real retained recorder through the production request and recovery lifecycle.
//! MIDI packets enter the pure recorder here; native OS ingress is checked separately.
use super::scenarios::{queue, temp_root};
use super::*;
use crest_tracker::editing::shared::{references, Definition};

struct Journey {
    stage: u8,
    original: Option<Song>,
    independent: Option<Song>,
    recorded: Option<PhraseId>,
    other: Option<PhraseId>,
    root: PathBuf,
}

fn edit(runtime: &Runtime, command: Command) {
    let revision = runtime.app.borrow().editor.revision();
    queue(
        runtime,
        Request::Edit {
            revision,
            command: Box::new(command),
        },
    );
}

fn locate(runtime: &Runtime, occurrence: Occurrence) {
    queue(
        runtime,
        Request::Locate {
            row_at: None,
            occurrence,
            at: Beat::ZERO,
            row: 0,
            view: View::Phrase,
        },
    );
}

impl Journey {
    fn arm(&mut self, runtime: &mut Runtime) {
        let app = runtime.app.borrow();
        let occurrence = app.occurrence().unwrap();
        let (recorded, _) = app.selected_phrase().unwrap();
        let other = *app
            .editor
            .song()
            .phrases
            .keys()
            .find(|id| **id != recorded)
            .unwrap();
        let location = references(app.editor.song(), Definition::Phrase(other))[0].location;
        self.original = Some(app.editor.song().clone());
        self.recorded = Some(recorded);
        self.other = Some(other);
        runtime.recorder.arm(&app.editor, occurrence).unwrap();
        runtime.capture_baseline = Some(Document::new(
            app.editor.song().clone(),
            runtime.saved_sound.clone().unwrap(),
            app.mappings.clone(),
        ));
        let baseline = runtime.capture_baseline.as_mut().unwrap();
        baseline.pinned_sounds = runtime.pinned_sounds.clone();
        baseline.resources = runtime.resources.clone();
        runtime.recorder.request_record(Beat::ZERO).unwrap();
        runtime
            .recorder
            .input(CaptureInput {
                at: Beat::ZERO,
                source_channel: Channel::FIRST,
                target: Target::Instrument {
                    id: InstrumentId::new(17).unwrap(),
                    channel: Channel::FIRST,
                },
                message: CaptureMessage::NoteOn {
                    pitch: 64,
                    velocity: 103,
                },
            })
            .unwrap();
        // An independent new event initially occupies the recorder's private ID.
        let mut event = app.editor.song().phrases[&recorded].events[0].clone();
        event.id = EventId::new(app.editor.song().next_identity().unwrap()).unwrap();
        event.at = Beat::ZERO;
        drop(app);
        queue(
            runtime,
            Request::SoundLibrary {
                action: "publish".into(),
                instrument: InstrumentId::new(17).unwrap(),
            },
        );
        locate(
            runtime,
            Occurrence {
                track: location.track,
                chain_use: location.chain_use,
                phrase_use: location.phrase_use.unwrap(),
            },
        );
        edit(
            runtime,
            Command::PutEvent {
                phrase: other,
                event,
            },
        );
    }

    fn assert_recovery(&self, runtime: &Runtime) {
        let baseline = runtime.capture_baseline.as_ref().unwrap();
        let app = runtime.app.borrow();
        assert_eq!(baseline.song, *app.editor.song());
        let snapshot =
            RecoverySnapshot::new(baseline.clone(), runtime.recorder.checkpoint()).unwrap();
        let mut restored = snapshot.restore(baseline, &app.editor).unwrap();
        assert_eq!(restored.held_count(), 1);
        restored.finish_at(Beat::ONE).unwrap();
        restored.prepare_commit(&app.editor).unwrap();
    }

    fn edits(&mut self, runtime: &mut Runtime) {
        match self.stage {
            0 => {
                queue(runtime, Request::Example);
                self.stage = 1;
            }
            1 if runtime.app.borrow().editor.song().name == "Window Light" => {
                queue(runtime, Request::Confirm);
                queue(runtime, Request::Confirm);
                self.stage = 2;
            }
            2 => {
                self.arm(runtime);
                self.stage = 3;
            }
            3 => {
                assert!(
                    runtime.publishing.is_none(),
                    "retained capture cannot publish a new sound revision"
                );
                let pending = runtime.app.borrow().projection()["sharedEdit"].clone();
                if let Some(token) = pending["token"].as_u64() {
                    queue(runtime, serde_json::from_value(serde_json::json!({
                        "kind":"shared_decision", "token":token, "choice":"shared", "remember":true,
                    })).unwrap());
                    return;
                }
                let song = runtime.app.borrow().editor.song().clone();
                assert_eq!(
                    song.phrases[&self.other.unwrap()].events.len(),
                    self.original.as_ref().unwrap().phrases[&self.other.unwrap()]
                        .events
                        .len()
                        + 1,
                    "{}",
                    runtime.app.borrow().status
                );
                self.independent = Some(song);
                self.assert_recovery(runtime);
                queue(runtime, Request::Undo);
                self.stage = 4;
            }
            4 => {
                assert_eq!(
                    runtime.app.borrow().editor.song(),
                    self.original.as_ref().unwrap()
                );
                self.assert_recovery(runtime);
                queue(runtime, Request::Redo);
                self.stage = 5;
            }
            5 => {
                assert_eq!(
                    runtime.app.borrow().editor.song(),
                    self.independent.as_ref().unwrap()
                );
                self.assert_recovery(runtime);
                let recorded = self.recorded.unwrap();
                locate(runtime, runtime.recorder.armed_phrase().unwrap().0);
                let mut phrase = runtime.app.borrow().editor.song().phrases[&recorded].clone();
                phrase.events.clear();
                edit(
                    runtime,
                    Command::PutPhrase {
                        id: recorded,
                        phrase,
                    },
                );
                self.stage = 6;
            }
            _ => {}
        }
    }

    fn finalize(&mut self, runtime: &mut Runtime) -> bool {
        match self.stage {
            6 => {
                assert!(runtime.app.borrow().status.contains("retained take"));
                assert_eq!(
                    runtime.app.borrow().editor.song(),
                    self.independent.as_ref().unwrap()
                );
                self.assert_recovery(runtime);
                self.stage = 7;
            }
            7 if runtime.checkpoint_generation > 0
                && runtime.checkpoint_saved == runtime.checkpoint_generation =>
            {
                // The worker's durable checkpoint must carry the independent edit too.
                let snapshot = RecoveryWorker::load(&self.root.join("machine"))
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    &snapshot.baseline().song,
                    self.independent.as_ref().unwrap()
                );
                let app = runtime.app.borrow();
                let editor = &app.editor;
                let mut restored = snapshot
                    .restore(runtime.capture_baseline.as_ref().unwrap(), editor)
                    .unwrap();
                restored.finish_at(Beat::ONE).unwrap();
                restored.prepare_commit(editor).unwrap();
                drop(app);
                runtime.recorder.finish_at(Beat::ONE).unwrap();
                queue(
                    runtime,
                    Request::Capture {
                        action: CaptureAction::Commit,
                    },
                );
                self.stage = 8;
            }
            8 => {
                assert!(runtime.capture_baseline.is_none());
                assert!(runtime.recorder.draft().is_none());
                let app = runtime.app.borrow();
                let recorded = self.recorded.unwrap();
                let events = &app.editor.song().phrases[&recorded].events;
                assert_eq!(
                    events.len(),
                    self.original.as_ref().unwrap().phrases[&recorded]
                        .events
                        .len()
                        + 1
                );
                assert!(events
                    .iter()
                    .any(
                        |event| event.note.as_ref().is_some_and(|note| note.pitch == 64
                            && note.velocity == 103
                            && note.duration == Beat::ONE)
                    ));
                drop(app);
                queue(runtime, Request::Undo);
                self.stage = 9;
            }
            9 => {
                assert_eq!(
                    runtime.app.borrow().editor.song(),
                    self.independent.as_ref().unwrap()
                );
                // Finalization released the guard for ordinary document edits.
                queue(
                    runtime,
                    Request::Rename {
                        name: "Capture complete".into(),
                    },
                );
                self.stage = 10;
            }
            10 => {
                assert_eq!(runtime.app.borrow().editor.song().name, "Capture complete");
                return true;
            }
            _ => {}
        }
        false
    }
}

#[test]
#[ignore = "production retained-capture edits and durable recovery; run explicitly and serially"]
fn other_phrase_edits_survive_capture_recovery_commit_and_undo() {
    let root = temp_root("recording-independent-edits");
    let mut journey = Journey {
        stage: 0,
        original: None,
        independent: None,
        recorded: None,
        other: None,
        root: root.clone(),
    };
    let evidence = run(
        &root,
        Box::new(move |runtime, _, _| {
            if journey.stage < 6 {
                journey.edits(runtime);
                false
            } else {
                journey.finalize(runtime)
            }
        }),
        Duration::from_secs(30),
    );
    evidence.check_cleanup();
    std::fs::remove_dir_all(root).unwrap();
}
