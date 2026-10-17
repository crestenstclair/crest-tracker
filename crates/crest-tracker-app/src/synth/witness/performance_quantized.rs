//! The production scheduler must retain a queued boundary while preparing
//! audio ahead, cancel it silently, then deliver the next launch on that grid.
use super::*;
use crate::app::PerformanceLaunchGrid;
use crest_performance_core::SlotState;

#[derive(Default)]
struct Journey {
    stage: u8,
    launch: u64,
}
fn request(runtime: &Runtime, request: Request) {
    runtime.app.borrow_mut().requests.push(request);
}
fn launch(runtime: &Runtime) {
    request(
        runtime,
        Request::PerformanceLaunchGrid {
            grid: PerformanceLaunchGrid::FourBeats,
        },
    );
    request(runtime, Request::PerformanceAudition { slot: 0 });
}
impl Journey {
    fn cancel_prepared_launch(
        &mut self,
        runtime: &Runtime,
        audio: AudioObservationSnapshot,
        evidence: &mut Evidence,
    ) {
        let playback = runtime.performance_playback.as_ref().unwrap().get(0);
        self.launch = playback.launch_frame();
        assert_eq!(playback.slot_state().1, SlotState::Queued);
        assert_eq!(
            playback.scheduler_slot_state(),
            SlotState::Playing,
            "future events are already prepared while display remains queued"
        );
        assert!(playback.launch_frame() > audio.rendered_frames());
        assert_eq!(audio.active_notes(), 0);
        evidence.measured = true;
        request(runtime, Request::Stop);
        self.stage = 3;
    }
    fn check_playback_and_stop(
        &mut self,
        runtime: &Runtime,
        audio: AudioObservationSnapshot,
        evidence: &Evidence,
    ) {
        let playback = runtime.performance_playback.as_ref().unwrap().get(0);
        assert_eq!(
            playback.launch_frame(),
            self.launch,
            "preference changes cannot move queued playback"
        );
        if audio.rendered_frames() < self.launch {
            assert_eq!(playback.slot_state().1, SlotState::Queued);
            assert_eq!(audio.active_notes(), 0);
            assert!(
                !evidence.audible_tracks[0],
                "no audio may precede launch boundary"
            );
        } else {
            assert_eq!(playback.slot_state().1, SlotState::Playing);
        }
        if audio.rendered_frames() > self.launch + 48_000 {
            request(runtime, Request::Stop);
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
                request(
                    runtime,
                    Request::File {
                        action: FileAction::PerformanceSampleExample,
                    },
                );
                self.stage = 1;
            }
            1 if runtime.app.borrow().editor.song().name
                == "Expressive Sample performance example" =>
            {
                launch(runtime);
                self.stage = 2;
            }
            2 if runtime
                .performance_playback
                .as_ref()
                .is_some_and(|playback| {
                    audio.rendered_frames().saturating_add(4_000) >= playback.get(0).launch_frame()
                }) =>
            {
                self.cancel_prepared_launch(runtime, audio, evidence);
            }
            3 if runtime.performance_playback.is_none() => {
                assert_eq!(audio.active_notes(), 0, "canceled queue must remain silent");
                if audio.rendered_frames() > self.launch + 4_800 {
                    launch(runtime);
                    self.stage = 4;
                }
            }
            4 if runtime.performance_playback.is_some() => {
                let playback = runtime.performance_playback.as_ref().unwrap().get(0);
                self.launch = playback.launch_frame();
                assert_eq!(playback.slot_state().1, SlotState::Queued);
                request(
                    runtime,
                    Request::PerformanceLaunchGrid {
                        grid: PerformanceLaunchGrid::Immediate,
                    },
                );
                self.stage = 5;
            }
            5 => {
                self.check_playback_and_stop(runtime, audio, evidence);
            }
            6 if runtime.performance_playback.is_none() && audio.active_notes() == 0 => {
                return true
            }
            _ => {}
        }
        false
    }
}

#[test]
#[ignore = "production DSP witness; run explicitly in release mode"]
fn quantized_sample_launch_stays_queued_cancels_and_delivers_without_late_events() {
    let root = scenarios::temp_root("performance-quantized");
    let mut journey = Journey::default();
    let evidence = run(
        &root,
        Box::new(move |runtime, audio, evidence| journey.step(runtime, audio, evidence)),
        Duration::from_secs(45),
    );
    evidence.check_cleanup();
    assert_eq!(evidence.peak_voices, 3);
    assert!(evidence.audible_tracks[0]);
    std::fs::remove_dir_all(root).unwrap();
}
