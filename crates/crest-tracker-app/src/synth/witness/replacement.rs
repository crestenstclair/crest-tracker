//! File decisions and failed native candidates exercise the same host as playback.
use super::scenarios::{choose, queue, temp_root};
use super::*;

struct Journey {
    stage: u8,
    saved: PathBuf,
    rejected: PathBuf,
    before_rejection: Option<Document>,
}

fn decide(runtime: &Runtime, decision: &str) -> bool {
    let app = runtime.app.borrow();
    if app.file_dialog["mode"] != "confirm" {
        return false;
    }
    let token = app.file_dialog["token"].as_u64().unwrap();
    drop(app);
    queue(
        runtime,
        Request::FileDecision {
            token,
            decision: decision.into(),
        },
    );
    true
}

fn renamed_example(runtime: &Runtime, name: &str) {
    queue(runtime, Request::Rename { name: name.into() });
    queue(runtime, Request::Example);
}

fn snapshot(runtime: &Runtime) -> Document {
    let app = runtime.app.borrow();
    let mut document = Document::new(
        app.editor.song().clone(),
        runtime.saved_sound.clone().unwrap(),
        app.mappings.clone(),
    );
    document.pinned_sounds = runtime.pinned_sounds.clone();
    document.resources = runtime.resources.clone();
    document
}

impl Journey {
    fn decisions(&mut self, runtime: &mut Runtime) {
        let name = runtime.app.borrow().editor.song().name.clone();
        match self.stage {
            0 => {
                queue(runtime, Request::Example);
                self.stage = 1;
            }
            1 if name == "Window Light" => {
                renamed_example(runtime, "Keep unsaved work");
                self.stage = 2;
            }
            2 if decide(runtime, "cancel") => {
                assert_eq!(name, "Keep unsaved work");
                self.stage = 3;
            }
            3 if runtime.dialog.is_none() => {
                assert_eq!(name, "Keep unsaved work");
                assert!(runtime.app.borrow().dirty());
                assert_eq!(runtime.pinned_sounds.len(), 5);
                queue(runtime, Request::Example);
                self.stage = 4;
            }
            4 if decide(runtime, "save") => self.stage = 5,
            5 if choose(runtime, &self.saved) => self.stage = 6,
            6 if name == "Window Light" && runtime.pending_open.is_none() => {
                let saved = Document::read(&self.saved).unwrap();
                assert_eq!(saved.song.name, "Keep unsaved work");
                assert_eq!(saved.pinned_sounds.len(), 5);
                assert!(runtime.app.borrow().path.is_none());
                renamed_example(runtime, "Discard this edit");
                self.stage = 7;
            }
            7 if decide(runtime, "discard") => self.stage = 8,
            _ => {}
        }
    }

    fn replacement(&mut self, runtime: &mut Runtime, audio: AudioObservationSnapshot) -> bool {
        let name = runtime.app.borrow().editor.song().name.clone();
        match self.stage {
            8 if name == "Window Light" && runtime.pending_open.is_none() => {
                queue(runtime, Request::Play);
                self.stage = 9;
            }
            9 if audio.active_notes() > 0 => {
                queue(
                    runtime,
                    Request::File {
                        action: FileAction::New,
                    },
                );
                self.stage = 10;
            }
            10 if name == "Untitled"
                && runtime.pending_open.is_none()
                && audio.active_notes() == 0 =>
            {
                self.before_rejection = Some(snapshot(runtime));
                queue(
                    runtime,
                    Request::File {
                        action: FileAction::Open,
                    },
                );
                self.stage = 11;
            }
            11 if choose(runtime, &self.rejected) => self.stage = 12,
            12 if runtime.dialog.is_none()
                && runtime.loading.is_none()
                && runtime.pending_open.is_none() =>
            {
                let current = snapshot(runtime);
                assert_eq!(
                    serde_json::to_value(current).unwrap(),
                    serde_json::to_value(self.before_rejection.as_ref().unwrap()).unwrap()
                );
                assert!(
                    runtime.app.borrow().status.contains("required"),
                    "{}",
                    runtime.app.borrow().status
                );
                assert!(!runtime.app.borrow().playing);
                return audio.active_notes() == 0;
            }
            _ => {}
        }
        false
    }
}

#[test]
#[ignore = "integrated file decisions and native graph replacement; run explicitly and serially"]
fn example_decisions_and_rejected_candidate_preserve_complete_document() {
    let root = temp_root("replacement-journey");
    let rejected = root.join("missing-required-patch.crest-song");
    let mut invalid = demo::document().unwrap();
    *invalid.installed_patches.values_mut().next().unwrap() = 999_999;
    std::fs::write(&rejected, serde_json::to_vec(&invalid).unwrap()).unwrap();
    let mut journey = Journey {
        stage: 0,
        saved: root.join("kept.crest-song"),
        rejected,
        before_rejection: None,
    };
    let evidence = run(
        &root,
        Box::new(move |runtime, audio, _| {
            if journey.stage < 8 {
                journey.decisions(runtime);
                false
            } else {
                journey.replacement(runtime, audio)
            }
        }),
        Duration::from_secs(30),
    );
    evidence.check_cleanup();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "integrated retained-take request admission; run explicitly and serially"]
fn retained_take_allows_piano_navigation_but_rejects_note_edits() {
    use crate::app::{PianoControl, PianoValue};
    let root = temp_root("retained-navigation");
    let mut stage = 0;
    let evidence = run(
        &root,
        Box::new(move |runtime, _, _| {
            match stage {
                0 => {
                    queue(runtime, Request::Example);
                    stage = 1;
                }
                1 if runtime.app.borrow().editor.song().name == "Window Light" => {
                    let capture = runtime.app.borrow().capture.clone();
                    let destinations = capture["destinations"].as_array().unwrap();
                    assert_eq!(destinations.len(), 5);
                    assert!(destinations.iter().all(|item| item["mpe_error"].is_null()));
                    // Isolate admission policy with a retained baseline. Capture/MPE
                    // lifecycle and persistence have their own production-path witnesses.
                    runtime.capture_baseline = Some(snapshot(runtime));
                    queue(runtime, Request::View { view: View::Song });
                    queue(runtime, Request::Confirm);
                    queue(
                        runtime,
                        Request::Move {
                            horizontal: 0,
                            vertical: 1,
                        },
                    );
                    queue(runtime, Request::Confirm);
                    for control in [
                        PianoControl::Grid {
                            grid: Beat::new(1, 8).unwrap(),
                        },
                        PianoControl::Value {
                            value: PianoValue::Velocity,
                        },
                        PianoControl::Cursor {
                            at: Beat::ONE,
                            pitch: 60,
                        },
                        PianoControl::Navigate {
                            horizontal: 1,
                            vertical: -1,
                        },
                    ] {
                        queue(runtime, Request::PianoControl { control });
                    }
                    stage = 2;
                }
                2 => {
                    let projection = runtime.app.borrow().projection();
                    assert_eq!(projection["view"], "phrase");
                    assert_eq!(projection["phrase"]["id"], 73);
                    assert_eq!(
                        projection["pianoControls"]["grid"],
                        serde_json::to_value(Beat::new(1, 8).unwrap()).unwrap()
                    );
                    assert_eq!(
                        projection["pianoControls"]["at"],
                        serde_json::to_value(Beat::new(9, 8).unwrap()).unwrap()
                    );
                    assert_eq!(projection["pianoControls"]["pitch"], 61);
                    assert_eq!(projection["pianoControls"]["value"], "velocity");
                    queue(
                        runtime,
                        Request::PianoControl {
                            control: PianoControl::Nudge { delta: 1 },
                        },
                    );
                    stage = 3;
                }
                3 => {
                    assert!(runtime.app.borrow().status.contains("retained take"));
                    assert_eq!(
                        runtime.app.borrow().editor.song(),
                        &runtime.capture_baseline.as_ref().unwrap().song
                    );
                    return true;
                }
                _ => {}
            }
            false
        }),
        Duration::from_secs(30),
    );
    evidence.check_cleanup();
    std::fs::remove_dir_all(root).unwrap();
}
