//! Learned voicings cross the real host, persistence and resident Sample engine.
use super::performance_live::packet;
use super::*;
use crate::app::{PerformanceChordEdit, PerformanceInputMode};
use crate::synth::performance::Live;
use crest_performance_core::{Gesture, LearnedChord};
use std::cell::Cell;

const PLAYED: &[(u64, &[u8])] = &[
    (4_800, &[0x91, 60, 100]),
    (4_800, &[0x92, 63, 90]),
    (4_800, &[0x93, 70, 80]),
    (9_600, &[0xe1, 0, 65]),
    (9_600, &[0xb2, 74, 110]),
    (9_600, &[0xd3, 90]),
    (14_400, &[0x83, 70, 0]),
    (14_400, &[0x81, 60, 0]),
    (14_400, &[0x82, 63, 0]),
];
const TRIGGER: &[(u64, &[u8])] = &[
    (4_800, &[0x91, 30, 100]),
    (9_600, &[0xe1, 0, 65]),
    (9_600, &[0xb1, 74, 110]),
    (9_600, &[0xd1, 90]),
    (14_400, &[0x81, 30, 0]),
];
struct Journey {
    stage: u8,
    anchor: u64,
    packet: usize,
    survivor: u64,
    live: Option<Live>,
    done: Rc<Cell<bool>>,
}
impl Journey {
    fn start_input(&mut self, runtime: &mut Runtime, sound: &Sound<'_>, clock: HostClock) {
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
        runtime.performance = None;
        self.anchor = clock.audio.rendered_frames();
        self.packet = 0;
    }

    fn feed(
        &mut self,
        runtime: &mut Runtime,
        sound: &mut Sound<'_>,
        clock: HostClock,
        input: &[(u64, &[u8])],
    ) {
        while self.packet < input.len()
            && self.anchor + input[self.packet].0 <= clock.audio.rendered_frames()
        {
            let (offset, bytes) = input[self.packet];
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

    fn learn_and_undo(&mut self, runtime: &mut Runtime, sound: &mut Sound<'_>, clock: HostClock) {
        runtime.performance_live = self.live.take();
        let before = runtime.performance_live.as_ref().unwrap().engine.snapshot();
        runtime
            .request(
                sound,
                clock,
                Request::PerformanceChord {
                    edit: PerformanceChordEdit::LearnHeld,
                },
            )
            .unwrap();
        let expected = LearnedChord::new(&[60, 63, 70], 60).unwrap();
        let learned = runtime.performance_document().unwrap().unwrap();
        assert_eq!(learned.session.harmony.learned[0], Some(expected));
        let after = runtime.performance_live.as_ref().unwrap().engine.snapshot();
        assert_eq!(after.at, before.at);
        assert_eq!(after.active_voices, 3);
        assert!(runtime.app.borrow().performance_dirty);
        runtime
            .request(
                sound,
                clock,
                Request::PerformanceChord {
                    edit: PerformanceChordEdit::RestoreDiatonic,
                },
            )
            .unwrap();
        assert!(runtime
            .performance_document()
            .unwrap()
            .unwrap()
            .session
            .harmony
            .learned[0]
            .is_none());
        runtime
            .request(sound, clock, Request::PerformanceUndoClipChange)
            .unwrap();
        assert_eq!(runtime.performance_document().unwrap().unwrap(), learned);
        self.live = runtime.performance_live.take();
    }

    fn reopen(
        &mut self,
        runtime: &mut Runtime,
        sound: &mut Sound<'_>,
        clock: HostClock,
    ) -> Document {
        runtime.performance_live = self.live.take();
        runtime
            .request(sound, clock, Request::PerformanceFinish)
            .unwrap();
        let saved = runtime
            .capture_document(serde_json::to_value(sound.capture_saved_session().unwrap()).unwrap())
            .unwrap();
        let reopened: Document =
            serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        reopened.validate().unwrap();
        assert_eq!(reopened.performance, saved.performance);
        runtime.performance = Some(
            crest_tracker::performance::Performance::new(
                reopened.performance.clone().unwrap(),
                crest_linnstrument::Geometry::Pads200,
                crest_performance_core::Limits::default(),
            )
            .unwrap(),
        );
        saved
    }

    fn record_learned(&mut self, runtime: &mut Runtime, sound: &mut Sound<'_>, clock: HostClock) {
        self.reopen(runtime, sound, clock);
        self.start_input(runtime, sound, clock);
        runtime.performance_live = self.live.take();
        runtime
            .request(
                sound,
                clock,
                Request::PerformanceInputMode {
                    mode: PerformanceInputMode::Chords,
                },
            )
            .unwrap();
        runtime
            .performance_live
            .as_mut()
            .unwrap()
            .engine
            .arm_recording(1)
            .unwrap();
        // Learning must not destroy a pending take or substitute generated voices.
        assert!(runtime
            .request(
                sound,
                clock,
                Request::PerformanceChord {
                    edit: PerformanceChordEdit::LearnHeld
                }
            )
            .is_err());
        assert_eq!(
            runtime
                .performance_live
                .as_ref()
                .unwrap()
                .engine
                .armed_slot(),
            Some(1)
        );
        self.live = runtime.performance_live.take();
    }

    fn replay(&mut self, runtime: &mut Runtime, sound: &mut Sound<'_>, clock: HostClock) {
        let saved = self.reopen(runtime, sound, clock);
        let performance = saved.performance.unwrap();
        assert_eq!(
            performance.session.harmony.learned[0].unwrap().notes(),
            &[60, 63, 70]
        );
        let clip = performance.session.slots[1].clip.as_ref().unwrap();
        let notes = clip.notes().unwrap();
        assert_eq!(
            notes.iter().map(|note| note.key).collect::<Vec<_>>(),
            [60, 63, 70]
        );
        assert_eq!(
            clip.events
                .iter()
                .filter(|event| matches!(event.gesture, Gesture::Off { .. }))
                .count(),
            3
        );
        for note in notes {
            let expression = note.expression.points.last().unwrap().value;
            assert_eq!(
                (expression.pitch, expression.timbre, expression.pressure),
                (750_000, 56_762, 46_442)
            );
        }
        assert_eq!(
            runtime
                .performance_playback
                .as_ref()
                .unwrap()
                .get(0)
                .launch_frame(),
            self.survivor
        );
        runtime
            .request(sound, clock, Request::PerformanceAudition { slot: 1 })
            .unwrap();
        self.anchor = clock.audio.rendered_frames();
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
                runtime
                    .request(sound, clock, Request::PerformanceAudition { slot: 0 })
                    .unwrap();
                self.survivor = runtime
                    .performance_playback
                    .as_ref()
                    .unwrap()
                    .get(0)
                    .launch_frame();
                self.start_input(runtime, sound, clock);
                self.stage = 2;
            }
            2 | 3 => {
                self.feed(runtime, sound, clock, PLAYED);
                let elapsed = clock.audio.rendered_frames() - self.anchor;
                if self.stage == 2 && elapsed >= 12_000 {
                    self.learn_and_undo(runtime, sound, clock);
                    self.stage = 3;
                } else if self.stage == 3 && elapsed >= 18_000 {
                    self.record_learned(runtime, sound, clock);
                    self.stage = 4;
                }
            }
            4 => {
                self.feed(runtime, sound, clock, TRIGGER);
                if clock.audio.rendered_frames() - self.anchor >= 18_000 {
                    self.replay(runtime, sound, clock);
                    self.stage = 5;
                }
            }
            5 if clock.audio.rendered_frames() - self.anchor >= 48_000 => {
                runtime.stop(sound).unwrap();
                self.done.set(true);
                self.stage = 6;
            }
            _ => {}
        }
    }
}

#[test]
#[ignore = "production DSP witness; run explicitly in release mode"]
fn native_sample_learned_chord_survives_undo_reopen_and_expressive_recording() {
    let root = scenarios::temp_root("performance-learned");
    let done = Rc::new(Cell::new(false));
    let complete = done.clone();
    let mut journey = Journey {
        stage: 0,
        anchor: 0,
        packet: 0,
        survivor: 0,
        live: None,
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
    assert!(
        evidence.peak_voices >= 6,
        "learned voices overlap an unrelated clip"
    );
    assert!(evidence.audible_tracks[0]);
    std::fs::remove_dir_all(root).unwrap();
}
