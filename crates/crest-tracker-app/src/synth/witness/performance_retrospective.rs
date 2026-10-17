//! Capture held MPE after playing, without advancing ingress time, then keep
//! accepting original packets and replay the saved copy through resident Sample.
use super::performance_live::packet;
use super::*;
use crate::synth::performance::Live;
use crest_performance_core::{BeatTime, Clip, Gesture};
use std::cell::Cell;

struct Journey {
    stage: u8,
    anchor: u64,
    packet: usize,
    live: Option<Live>,
    captured: Option<Clip>,
    playback_started: u64,
    done: Rc<Cell<bool>>,
}
const INPUT: &[(u64, &[u8])] = &[
    (4_800, &[0x91, 60, 100]),
    (4_800, &[0x92, 64, 90]),
    (4_800, &[0x93, 67, 80]),
    (9_600, &[0xe1, 0, 65]),
    (9_600, &[0xb2, 74, 110]),
    (9_600, &[0xd3, 90]),
    (14_400, &[0x81, 60, 0]),
    (14_448, &[0x82, 64, 0]),
    (14_496, &[0x83, 67, 0]),
];
impl Journey {
    fn feed(&mut self, runtime: &mut Runtime, sound: &mut Sound<'_>, clock: HostClock) {
        while self.packet < INPUT.len()
            && self.anchor + INPUT[self.packet].0 <= clock.audio.rendered_frames()
        {
            let (offset, bytes) = INPUT[self.packet];
            self.live
                .as_mut()
                .unwrap()
                .accept(packet(clock, self.anchor + offset, bytes), clock)
                .unwrap();
            self.packet += 1;
        }
        self.live
            .as_mut()
            .unwrap()
            .advance(
                sound,
                clock,
                runtime.app.borrow().editor.song().tempo,
                &mut runtime.voices,
            )
            .unwrap();
    }
    fn capture_and_undo(&mut self, runtime: &mut Runtime, sound: &mut Sound<'_>, clock: HostClock) {
        runtime.performance_live = self.live.take();
        let before = runtime.performance_live.as_ref().unwrap().engine.snapshot();
        runtime
            .request(sound, clock, Request::PerformanceCaptureRecent { slot: 1 })
            .unwrap();
        let live = runtime.performance_live.as_ref().unwrap();
        assert_eq!(live.engine.snapshot().at, before.at);
        assert_eq!(live.engine.snapshot().active_voices, 3);
        let clip = live.engine.clip(1).unwrap().unwrap().clone();
        verify_captured_phrase(&clip);
        assert!(runtime.app.borrow().performance_dirty);
        runtime
            .request(sound, clock, Request::PerformanceUndoClipChange)
            .unwrap();
        assert!(runtime
            .performance_live
            .as_ref()
            .unwrap()
            .engine
            .clip(1)
            .unwrap()
            .is_none());
        assert_eq!(
            runtime
                .performance_live
                .as_ref()
                .unwrap()
                .engine
                .snapshot()
                .active_voices,
            3
        );
        runtime
            .request(sound, clock, Request::PerformanceCaptureRecent { slot: 1 })
            .unwrap();
        assert_eq!(
            runtime
                .performance_live
                .as_ref()
                .unwrap()
                .engine
                .clip(1)
                .unwrap(),
            Some(&clip)
        );
        // A delayed packet predating the copied cutoff remains admissible.
        let frame = clock.audio.rendered_frames() - 128;
        assert!(runtime.midi_input(packet(clock, frame, &[0xe1, 0, 66]), clock));
        let live = runtime.performance_live.as_ref().unwrap();
        assert!(live.fault.is_none());
        assert!(live.engine.snapshot().at < clip.length);
        assert_eq!(live.engine.clip(1).unwrap(), Some(&clip));
        self.captured = Some(clip);
        self.live = runtime.performance_live.take();
    }
    fn reopen_and_play(&mut self, runtime: &mut Runtime, sound: &mut Sound<'_>, clock: HostClock) {
        runtime.performance_live = self.live.take();
        runtime
            .request(sound, clock, Request::PerformanceFinish)
            .unwrap();
        let document = runtime
            .capture_document(serde_json::to_value(sound.capture_saved_session().unwrap()).unwrap())
            .unwrap();
        let reopened: Document =
            serde_json::from_slice(&serde_json::to_vec(&document).unwrap()).unwrap();
        reopened.validate().unwrap();
        let performance = reopened.performance.unwrap();
        assert_eq!(performance.session.slots[1].clip, self.captured);
        runtime.performance = Some(
            crest_tracker::performance::Performance::new(
                performance,
                crest_linnstrument::Geometry::Pads200,
                crest_performance_core::Limits::default(),
            )
            .unwrap(),
        );
        runtime.audition_performance(sound, clock, 1).unwrap();
        self.playback_started = clock.audio.rendered_frames();
    }
    fn step(&mut self, runtime: &mut Runtime, sound: &mut Sound<'_>, clock: HostClock) {
        match self.stage {
            0 if runtime.initial_sound.is_some() => {
                runtime
                    .request(
                        sound,
                        clock,
                        Request::File {
                            action: FileAction::PerformanceSampleExample,
                        },
                    )
                    .unwrap();
                self.stage = 1;
            }
            1 if runtime.app.borrow().editor.song().name
                == "Expressive Sample performance example" =>
            {
                self.live = Some(
                    Live::new(
                        runtime.performance_document().unwrap().unwrap(),
                        &runtime.app.borrow(),
                        sound,
                        clock,
                        1,
                        0,
                    )
                    .unwrap(),
                );
                self.anchor = clock.audio.rendered_frames();
                self.stage = 2;
            }
            2 | 3 => {
                self.feed(runtime, sound, clock);
                let elapsed = clock.audio.rendered_frames() - self.anchor;
                if self.stage == 2 && elapsed >= 12_000 {
                    self.capture_and_undo(runtime, sound, clock);
                    self.stage = 3;
                } else if self.stage == 3 && elapsed > 17_000 {
                    self.reopen_and_play(runtime, sound, clock);
                    self.stage = 4;
                }
            }
            4 if clock.audio.rendered_frames() > self.playback_started + 48_000 => {
                runtime.stop(sound).unwrap();
                self.done.set(true);
                self.stage = 5;
            }
            _ => {}
        }
    }
}
fn verify_captured_phrase(clip: &Clip) {
    let notes = clip.notes().unwrap();
    assert_eq!(notes.len(), 3);
    assert_eq!(clip.events.len(), 9);
    for note in &notes {
        assert_eq!(note.start, BeatTime(172_800)); // 100 ms at 108 BPM.
        assert_eq!(note.duration.0, clip.length.0 - note.start.0);
        assert_eq!(note.expression.points[1].offset, BeatTime(172_800));
    }
    assert!(
        matches!(clip.events[3].gesture, Gesture::Expression { expression } if expression.pitch == 750_000)
    );
    assert!(
        matches!(clip.events[4].gesture, Gesture::Expression { expression } if expression.timbre == 56_762)
    );
    assert!(
        matches!(clip.events[5].gesture, Gesture::Expression { expression } if expression.pressure == 46_442)
    );
}

#[test]
#[ignore = "production DSP witness; run explicitly in release mode"]
fn native_sample_retrospective_capture_keeps_held_voices_and_replays_saved_copy() {
    let root = scenarios::temp_root("performance-retrospective");
    let done = Rc::new(Cell::new(false));
    let complete = done.clone();
    let mut journey = Journey {
        stage: 0,
        anchor: 0,
        packet: 0,
        live: None,
        captured: None,
        playback_started: 0,
        done,
    };
    let evidence = run_with_output_and_host_step(
        &root,
        Box::new(move |runtime, audio, evidence| {
            evidence.measured = true;
            assert!(
                runtime.app.borrow().playback_fault.is_none(),
                "{}",
                runtime.app.borrow().status
            );
            complete.get() && audio.active_notes() == 0
        }),
        Duration::from_secs(60),
        crest_synth::adapter::braids_capability::BRAIDS_CAPABILITY_ID,
        false,
        audio::Output::default(),
        Some(Box::new(move |runtime, sound, clock| {
            journey.step(runtime, sound, clock)
        })),
    );
    evidence.check_cleanup();
    assert_eq!(evidence.peak_voices, 3);
    assert!(evidence.audible_tracks[0]);
    std::fs::remove_dir_all(root).unwrap();
}
