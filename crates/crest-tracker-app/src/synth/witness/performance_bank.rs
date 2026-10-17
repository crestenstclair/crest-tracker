//! Two real library players share the production schedule and native instrument.
//! A third prepared launch is canceled while the surviving phrase keeps its phase.
use super::*;
use crate::app::PerformanceLaunchGrid;
use crest_performance_core::SlotState;
use std::cell::Cell;

struct Journey {
    stage: u8,
    sample: bool,
    survivor_launch: Rc<Cell<u64>>,
    canceled_launch: u64,
    original: Option<crest_tracker::performance::Document>,
}
impl Journey {
    fn request(runtime: &Runtime, request: Request) {
        runtime.app.borrow_mut().requests.push(request);
    }
    fn launch(runtime: &Runtime, slot: usize) {
        Self::request(runtime, Request::PerformanceAudition { slot });
    }
    fn prepare(&mut self, runtime: &mut Runtime) {
        let mut document = performance::independent_expression_document();
        document.session.slots[1].clip = document.session.slots[0].clip.clone();
        document.session.slots[2].clip = document.session.slots[0].clip.clone();
        self.original = Some(document.clone());
        runtime.performance = Some(
            crest_tracker::performance::Performance::new(
                document,
                crest_linnstrument::Geometry::Pads200,
                crest_performance_core::Limits::default(),
            )
            .unwrap(),
        );
        Self::launch(runtime, 0);
        self.stage = 2;
    }
    fn stop_first_and_queue_third(&mut self, runtime: &Runtime, audio: AudioObservationSnapshot) {
        let bank = runtime.performance_playback.as_ref().unwrap();
        let launch = bank.get(1).launch_frame();
        self.survivor_launch.set(launch);
        if audio.rendered_frames() > launch + 9_600 {
            assert_eq!(audio.active_notes(), 6);
            assert_eq!(
                bank.states(),
                vec![(0, SlotState::Playing), (1, SlotState::Playing)]
            );
            Self::launch(runtime, 0); // Same semantic action independently stops this slot.
            Self::request(
                runtime,
                Request::PerformanceLaunchGrid {
                    grid: PerformanceLaunchGrid::FourBeats,
                },
            );
            Self::launch(runtime, 2);
            self.stage = 4;
        }
    }
    fn cancel_prepared_third(&mut self, runtime: &Runtime, audio: AudioObservationSnapshot) {
        let bank = runtime.performance_playback.as_ref().unwrap();
        assert!(!bank.contains(0));
        assert_eq!(bank.get(1).launch_frame(), self.survivor_launch.get());
        assert_eq!(
            audio.active_notes(),
            3,
            "first clip's sustained voices must be gone"
        );
        self.canceled_launch = bank.get(2).launch_frame();
        if audio.rendered_frames().saturating_add(4_000) >= self.canceled_launch {
            assert_eq!(bank.get(2).slot_state().1, SlotState::Queued);
            assert_eq!(bank.get(2).scheduler_slot_state(), SlotState::Playing);
            Self::launch(runtime, 2);
            self.stage = 5;
        }
    }
    fn check_survivor(&mut self, runtime: &Runtime, audio: AudioObservationSnapshot) {
        let bank = runtime.performance_playback.as_ref().unwrap();
        assert_eq!(bank.states(), vec![(1, SlotState::Playing)]);
        assert_eq!(bank.get(1).launch_frame(), self.survivor_launch.get());
        assert_eq!(
            audio.active_notes(),
            3,
            "canceled lookahead must never attack"
        );
        if audio.rendered_frames() > self.survivor_launch.get() + 48_000 * 4 + 12_000 {
            assert!(audio.rendered_frames() > self.canceled_launch);
            assert_eq!(runtime.performance_document().unwrap(), self.original);
            Self::request(runtime, Request::Stop);
            self.stage = 6;
        }
    }
    fn step(
        &mut self,
        runtime: &mut Runtime,
        audio: AudioObservationSnapshot,
        evidence: &mut Evidence,
    ) -> bool {
        match self.stage {
            0 if runtime.initial_sound.is_some() => {
                Self::request(
                    runtime,
                    Request::File {
                        action: if self.sample {
                            FileAction::PerformanceSampleExample
                        } else {
                            FileAction::PerformanceExample
                        },
                    },
                );
                self.stage = 1;
            }
            1 if runtime.performance.is_some() && runtime.pending_open.is_none() => {
                self.prepare(runtime)
            }
            2 if runtime.performance_playback.as_ref().is_some_and(|bank| {
                audio.rendered_frames() > bank.get(0).launch_frame() + 9_600
            }) =>
            {
                assert_eq!(audio.active_notes(), 3);
                evidence.measured = true;
                Self::launch(runtime, 1);
                self.stage = 3;
            }
            3 if runtime
                .performance_playback
                .as_ref()
                .is_some_and(|bank| bank.contains(1)) =>
            {
                self.stop_first_and_queue_third(runtime, audio)
            }
            // One block after control cancellation proves actual renderer cleanup.
            4 if audio.active_notes() == 3 => self.cancel_prepared_third(runtime, audio),
            5 => self.check_survivor(runtime, audio),
            6 if runtime.performance_playback.is_none() && audio.active_notes() == 0 => {
                return true
            }
            _ => {}
        }
        false
    }
}

fn exercise_bank(sample: bool) {
    let root = scenarios::temp_root("performance-bank");
    let output = audio::Output::default();
    output.record();
    let anchor = Rc::new(Cell::new(0));
    let mut journey = Journey {
        stage: 0,
        sample,
        survivor_launch: Rc::clone(&anchor),
        canceled_launch: 0,
        original: None,
    };
    let evidence = run_with_output(
        &root,
        Box::new(move |runtime, audio, evidence| journey.step(runtime, audio, evidence)),
        Duration::from_secs(45),
        crest_synth::adapter::braids_capability::BRAIDS_CAPABILITY_ID,
        false,
        output.clone(),
    );
    evidence.check_cleanup();
    assert_eq!(evidence.peak_voices, 6);
    assert!(evidence.audible_tracks[0]);
    let pcm = output.take_recording();
    if !sample {
        performance::verify_independent_audio(&pcm, anchor.get());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "production DSP and spectral witness; run explicitly in release mode"]
fn braids_clips_launch_and_stop_independently_preserving_survivor_expression() {
    exercise_bank(false);
}

#[test]
#[ignore = "production DSP witness; run explicitly in release mode"]
fn sample_clips_launch_and_stop_independently_and_cancel_prepared_attacks() {
    exercise_bank(true);
}
