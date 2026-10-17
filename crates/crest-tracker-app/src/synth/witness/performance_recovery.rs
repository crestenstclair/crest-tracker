//! A durable unfinished library take is read by a new production host, adopted
//! with its Sample graph, auditioned and saved. No MIDI device is simulated here.
use super::*;
use crest_performance_core::Command as PerformanceCommand;

fn interrupted_sample(root: &std::path::Path) -> crest_tracker::performance::Document {
    let mut document = super::super::performance::sample_example_document().unwrap();
    let mut performance = crest_tracker::performance::Performance::new(
        document.performance.take().unwrap(),
        crest_linnstrument::Geometry::Pads200,
        crest_performance_core::Limits::default(),
    )
    .unwrap();
    performance
        .command(PerformanceCommand::BeginRecord { slot: 1 })
        .unwrap();
    performance
        .accept_midi(Beat::ZERO, &[0x91, 60, 100, 0x92, 64, 90, 0x93, 67, 80])
        .unwrap();
    performance
        .accept_midi(Beat::ONE, &[0xe1, 0, 65, 0xb2, 74, 110, 0xd3, 90])
        .unwrap();
    let captured = performance
        .checkpoint_document_at(Beat::integer(3))
        .unwrap();
    assert!(
        performance.document().is_err(),
        "The original take must still be running"
    );
    document.performance = Some(captured.clone());
    resources::prepare_save(&mut document, None).unwrap();
    let worker = RecoveryWorker::new(root.join("machine")).unwrap();
    worker
        .submit(RecoverySnapshot::performance(document, 1).unwrap())
        .unwrap();
    drop(worker); // Drains the accepted write; the next host must read from disk.
    assert!(RecoveryWorker::load(&root.join("machine"))
        .unwrap()
        .is_some());
    captured
}

struct Journey {
    stage: u8,
    started: u64,
    path: PathBuf,
    machine: PathBuf,
    expected: crest_tracker::performance::Document,
}
impl Journey {
    fn step(
        &mut self,
        runtime: &mut Runtime,
        audio: AudioObservationSnapshot,
        evidence: &mut Evidence,
    ) -> bool {
        use scenarios::{choose, queue};
        match self.stage {
            0 if runtime.initial_sound.is_some() && runtime.recovery_available.is_some() => {
                queue(
                    runtime,
                    Request::Recovery {
                        action: "recover".into(),
                    },
                );
                self.stage = 1;
            }
            1 if runtime.performance_recovery.is_some() && runtime.pending_open.is_none() => {
                assert!(
                    runtime
                        .app
                        .borrow()
                        .status
                        .starts_with("RECOVERED PERFORMANCE"),
                    "{}",
                    runtime.app.borrow().status
                );
                assert_eq!(
                    runtime.performance_document().unwrap(),
                    Some(self.expected.clone())
                );
                assert!(runtime.app.borrow().performance_dirty);
                assert_eq!(audio.active_notes(), 0);
                queue(runtime, Request::PerformanceAudition { slot: 1 });
                self.stage = 2;
            }
            2 if runtime.performance_playback.is_some() => {
                self.started = audio.rendered_frames();
                evidence.measured = true;
                self.stage = 3;
            }
            3 if audio.rendered_frames() > self.started + 96_000 => {
                assert!(
                    runtime.app.borrow().playing,
                    "{}",
                    runtime.app.borrow().status
                );
                assert!(
                    RecoveryWorker::load(&self.machine).unwrap().is_some(),
                    "Audition/Stop must retain unsaved recovery"
                );
                queue(runtime, Request::Stop);
                self.stage = 4;
            }
            4 if !runtime.app.borrow().playing && audio.active_notes() == 0 => {
                queue(
                    runtime,
                    Request::File {
                        action: FileAction::SaveAs,
                    },
                );
                self.stage = 5;
            }
            5 if choose(runtime, &self.path) => self.stage = 6,
            6 if !runtime.saving && runtime.app.borrow().path.as_ref() == Some(&self.path) => {
                assert_eq!(
                    Document::read(&self.path).unwrap().performance,
                    Some(self.expected.clone())
                );
                assert!(!runtime.app.borrow().performance_dirty);
                assert!(runtime.performance_recovery.is_none());
                if RecoveryWorker::load(&self.machine).unwrap().is_none() {
                    return true;
                }
            }
            _ => {}
        }
        false
    }
}

#[test]
#[ignore = "production Sample DSP, durable recovery and project save witness"]
fn interrupted_expressive_take_recovers_stopped_plays_and_clears_only_after_save() {
    let root = scenarios::temp_root("performance-recovery");
    let expected = interrupted_sample(&root);
    let mut journey = Journey {
        stage: 0,
        started: 0,
        path: root.join("recovered.crest-song"),
        machine: root.join("machine"),
        expected,
    };
    let evidence = run(
        &root,
        Box::new(move |runtime, audio, evidence| journey.step(runtime, audio, evidence)),
        Duration::from_secs(60),
    );
    evidence.check_cleanup();
    assert_eq!(evidence.peak_voices, 3);
    assert!(evidence.audible_tracks[0]);
    std::fs::remove_dir_all(root).unwrap();
}
