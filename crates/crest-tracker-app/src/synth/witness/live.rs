//! Real live adoption and monitor cancellation under DSP load. Monitoring starts
//! at Runtime's accepted-packet queue; this does not certify physical MIDI ingress.
use super::scenarios::{choose, queue, report_audio, temp_root, workload_document};
use super::*;
use crest_tracker::time::Division;

fn dense_phrase(mut phrase: Phrase, index: usize, edit: u64) -> Phrase {
    phrase.steps = 12;
    phrase.division = Division::new(16, if index.is_multiple_of(2) { 1 } else { 3 }, 1).unwrap();
    for (voice, event) in phrase.events.iter_mut().enumerate() {
        let note = event.note.as_mut().unwrap();
        note.duration = Beat::new(3, 4).unwrap();
        note.velocity = 70 + (edit % 20) as u8;
        note.expression = (0..24)
            .map(|point| Expression {
                offset: Beat::new(point, 32).unwrap(),
                control: Control::Bend {
                    value: 8192 + (point % 4) as u16 * 64,
                },
                sequence: None,
            })
            .collect();
        // Separate expressive voices and retriggered voices: this is an
        // admitted workload, not the unsupported shared-expression fanout.
        if voice.is_multiple_of(2) {
            note.expression.clear();
            event.fx[0] = Some(Fx::Retrigger {
                count: 3,
                spacing: Beat::new(1, 4).unwrap(),
            });
        }
    }
    phrase
}

fn edit_live_phrases(runtime: &Runtime, edit: u64) {
    let app = runtime.app.borrow();
    let commands = app
        .editor
        .song()
        .phrases
        .iter()
        .enumerate()
        .map(|(index, (&id, phrase))| Command::PutPhrase {
            id,
            phrase: dense_phrase(phrase.clone(), index, edit),
        })
        .collect();
    let revision = app.editor.revision();
    drop(app);
    queue(
        runtime,
        Request::Edit {
            revision,
            command: Box::new(Command::Batch { commands }),
        },
    );
}

struct Journey {
    stage: u8,
    path: PathBuf,
    started: u64,
    monitored_at: u64,
    dense_attacks: u64,
    live_revision: u64,
    seconds: u64,
    next_edit_frame: u64,
    next_monitor_frame: u64,
    monitoring: bool,
}
impl Journey {
    fn step(
        &mut self,
        runtime: &mut Runtime,
        audio: AudioObservationSnapshot,
        evidence: &mut Evidence,
    ) -> bool {
        match self.stage {
            0 => {
                queue(
                    runtime,
                    Request::File {
                        action: FileAction::Open,
                    },
                );
                self.stage = 1;
            }
            1 if choose(runtime, &self.path) => self.stage = 2,
            2 if runtime.app.borrow().editor.song().name == "Integrated 16-track workload" => {
                queue(runtime, Request::Play);
                self.stage = 3;
            }
            3 if audio.active_notes() == 64 => {
                evidence.measured = true;
                self.started = audio.rendered_frames();
                // Inject an already accepted monitor packet on the same pitch,
                // channel and Patch as a scheduled voice. Sources must stay distinct.
                runtime.capture_target = Some(Target::Instrument {
                    id: InstrumentId::new(17).unwrap(),
                    channel: Channel::FIRST,
                });
                runtime.monitor_cleanup = false;
                runtime.monitor_pending.push(
                    MidiMessage::try_new(
                        MidiChannel::new(0).unwrap(),
                        MidiMessageKind::NoteOn,
                        48,
                        90,
                    )
                    .unwrap(),
                );
                self.stage = 4;
            }
            4 if audio.active_notes() == 65 => {
                self.monitored_at = audio.rendered_frames();
                runtime.monitor_cleanup = true;
                self.stage = 5;
            }
            5 if audio.rendered_frames() > self.monitored_at + 48_000 => {
                assert_eq!(
                    audio.active_notes(),
                    64,
                    "monitor cancel must preserve playback"
                );
                evidence.monitor_cancellations += 1;
                edit_live_phrases(runtime, 0);
                evidence.live_edits += 1;
                self.live_revision = runtime.app.borrow().editor.revision() + 1;
                self.stage = 6;
            }
            6 => self.exercise_live(runtime, audio, evidence),
            7 if !runtime.app.borrow().playing && audio.active_notes() == 0 => {
                assert!(!runtime.app.borrow().playback_pending);
                return true;
            }
            _ => {}
        }
        false
    }

    fn exercise_live(
        &mut self,
        runtime: &mut Runtime,
        audio: AudioObservationSnapshot,
        evidence: &mut Evidence,
    ) {
        let app = runtime.app.borrow();
        assert!(app.playing, "{}", app.status);
        assert_eq!(app.editor.revision(), self.live_revision, "{}", app.status);
        let playback = runtime.playback.as_ref().unwrap();
        assert!(
            playback.revision() == self.live_revision || playback.pending_revision(),
            "{}",
            app.status
        );
        if audio.rendered_blocks().is_multiple_of(128) {
            let projection = app.projection();
            assert!(projection["playing"].as_bool().unwrap());
            assert_eq!(projection["playbackPending"], playback.pending_revision());
        }
        drop(app);
        if audio.rendered_frames() < self.started + 48_000 * 34 {
            return;
        }
        if playback.pending_revision() {
            return;
        }
        assert_eq!(playback.revision(), self.live_revision);
        assert!(!playback.pending_revision());
        if self.dense_attacks == 0 {
            self.dense_attacks = runtime.voices.next;
            self.next_edit_frame = audio.rendered_frames() + 48_000 * 4;
            self.next_monitor_frame = audio.rendered_frames() + 48_000;
        }
        evidence.dense_attacks = runtime.voices.next - self.dense_attacks;
        if audio.rendered_frames() > self.started + 48_000 * self.seconds {
            assert!(
                runtime.voices.next > self.dense_attacks + 1_000,
                "live dense/retriggered phrases must actually emit fresh attacks"
            );
            queue(runtime, Request::Stop);
            self.stage = 7;
            return;
        }
        if audio.rendered_frames() >= self.next_edit_frame {
            edit_live_phrases(runtime, evidence.live_edits);
            self.live_revision = runtime.app.borrow().editor.revision() + 1;
            evidence.live_edits += 1;
            queue(
                runtime,
                Request::View {
                    view: if evidence.live_edits.is_multiple_of(2) {
                        View::Expanded
                    } else {
                        View::Minimap
                    },
                },
            );
            self.next_edit_frame = audio.rendered_frames() + 48_000 * 8;
        }
        if audio.rendered_frames() >= self.next_monitor_frame {
            if self.monitoring {
                runtime.monitor_cleanup = true;
                evidence.monitor_cancellations += 1;
            } else {
                runtime.monitor_pending.push(
                    MidiMessage::try_new(
                        MidiChannel::new(0).unwrap(),
                        MidiMessageKind::NoteOn,
                        48,
                        90,
                    )
                    .unwrap(),
                );
            }
            self.monitoring = !self.monitoring;
            self.next_monitor_frame = audio.rendered_frames() + 48_000;
        }
    }
}

#[test]
#[ignore = "production live editing, dense retrigger/expression and monitor cleanup"]
fn live_edits_dense_retriggers_and_monitor_cancel_preserve_playback_ownership() {
    let seconds = std::env::var("CREST_AUDIO_SECONDS")
        .map(|text| text.parse::<u64>().unwrap())
        .unwrap_or(45);
    assert!((45..=3600).contains(&seconds));
    let root = temp_root("live-audio");
    let path = root.join("live.crest-song");
    std::fs::write(&path, serde_json::to_vec(&workload_document()).unwrap()).unwrap();
    let mut journey = Journey {
        stage: 0,
        path,
        started: 0,
        monitored_at: 0,
        dense_attacks: 0,
        live_revision: 0,
        seconds,
        next_edit_frame: 0,
        next_monitor_frame: 0,
        monitoring: false,
    };
    let evidence = run(
        &root,
        Box::new(move |runtime, audio, evidence| journey.step(runtime, audio, evidence)),
        Duration::from_secs(seconds.max(120) * 2),
    );
    report_audio(&evidence, seconds, "mixed");
    evidence.check_cleanup();
    assert!(evidence.peak_voices >= 65);
    assert!(evidence.audible_tracks.iter().all(|audible| *audible));
    std::fs::remove_dir_all(root).unwrap();
}
