//! Timestamped stock-MPE ingress, library harmony and actual production DSP.
//! Only the device connection and audio device are replaced by the harness.
use super::*;
use crate::app::{PerformanceHarmonyEdit as Edit, PerformanceInputMode};
use crate::synth::performance::Live;
use crest_performance_core::{Gesture, SurfaceMode};
use std::cell::Cell;

const INPUT: &[(u64, &[u8])] = &[
    (4_800, &[0x91, 30, 100]),
    (9_600, &[0xe1, 0, 65]),
    (9_600, &[0xb1, 74, 110]),
    (9_600, &[0xd1, 90]),
    (12_000, &[0x92, 31, 90]),
    (14_400, &[0xe2, 0, 66]),
    (14_400, &[0xb2, 74, 25]),
    (14_400, &[0xd2, 64]),
    (18_000, &[0x81, 30, 0]),
    (24_000, &[0x82, 31, 0]),
];
struct Journey {
    stage: u8,
    sample: bool,
    live: Option<Live>,
    anchor: u64,
    packet: usize,
    changed: bool,
    survivor: u64,
    done: Rc<Cell<bool>>,
}
impl Journey {
    fn mode(
        runtime: &mut Runtime,
        sound: &mut Sound<'_>,
        clock: HostClock,
        mode: PerformanceInputMode,
    ) {
        runtime
            .request(sound, clock, Request::PerformanceInputMode { mode })
            .unwrap();
    }
    fn start_live(
        &mut self,
        runtime: &mut Runtime,
        sound: &mut Sound<'_>,
        clock: HostClock,
        record: bool,
    ) {
        let live = Live::new(
            runtime.performance_document().unwrap().unwrap(),
            &runtime.app.borrow(),
            sound,
            clock,
            1,
            0,
        )
        .unwrap();
        runtime.performance_live = Some(live);
        runtime.performance = None;
        Self::mode(runtime, sound, clock, PerformanceInputMode::Chords);
        if record {
            runtime
                .performance_live
                .as_mut()
                .unwrap()
                .engine
                .arm_recording(1)
                .unwrap();
            assert!(
                runtime
                    .request(sound, clock, Request::PerformancePlay)
                    .is_err(),
                "an armed take must not be silently disarmed"
            );
        }
        self.live = runtime.performance_live.take();
        self.anchor = clock.audio.rendered_frames();
        self.packet = 0;
    }
    fn edit_held_chord(&mut self, runtime: &mut Runtime, sound: &mut Sound<'_>, clock: HostClock) {
        runtime.performance_live = self.live.take();
        let before =
            serde_json::to_value(runtime.performance_live.as_ref().unwrap().engine.snapshot())
                .unwrap();
        assert!(runtime
            .request(sound, clock, Request::PerformancePlay)
            .is_err());
        assert_eq!(
            serde_json::to_value(runtime.performance_live.as_ref().unwrap().engine.snapshot())
                .unwrap(),
            before
        );
        for edit in [Edit::Expression, Edit::Inversion { delta: 1 }] {
            runtime
                .request(sound, clock, Request::PerformanceHarmony { edit })
                .unwrap();
        }
        assert!(runtime.performance_live.as_ref().unwrap().recording());
        self.live = runtime.performance_live.take();
        self.changed = true;
    }
    fn feed(&mut self, runtime: &mut Runtime, sound: &mut Sound<'_>, clock: HostClock) {
        let elapsed = clock.audio.rendered_frames() - self.anchor;
        if !self.changed && elapsed >= 10_000 {
            self.edit_held_chord(runtime, sound, clock);
        }
        while self.packet < INPUT.len() && INPUT[self.packet].0 <= elapsed {
            let (offset, bytes) = INPUT[self.packet];
            self.live
                .as_mut()
                .unwrap()
                .accept(
                    performance_live::packet(clock, self.anchor + offset, bytes),
                    clock,
                )
                .unwrap();
            self.packet += 1;
        }
        self.flush(runtime, sound, clock);
    }
    fn flush(&mut self, runtime: &mut Runtime, sound: &mut Sound<'_>, clock: HostClock) {
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
    fn reopen_and_launch(
        &mut self,
        runtime: &mut Runtime,
        sound: &mut Sound<'_>,
        clock: HostClock,
    ) {
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
        let document = reopened.performance.unwrap();
        assert_eq!(Some(&document), saved.performance.as_ref());
        verify_phrase(&document);
        runtime.performance = Some(
            crest_tracker::performance::Performance::new(
                document,
                crest_linnstrument::Geometry::Pads200,
                crest_performance_core::Limits::default(),
            )
            .unwrap(),
        );
        assert_eq!(
            runtime.performance.as_ref().unwrap().snapshot().mode,
            SurfaceMode::Play
        );
        self.survivor = runtime
            .performance_playback
            .as_ref()
            .unwrap()
            .get(0)
            .launch_frame();
        runtime
            .request(sound, clock, Request::PerformanceAudition { slot: 1 })
            .unwrap();
        self.start_live(runtime, sound, clock, false);
        // A sounding chord plus another attack still waiting in the host queue.
        self.live
            .as_mut()
            .unwrap()
            .accept(
                performance_live::packet(clock, self.anchor, &[0x91, 30, 100]),
                clock,
            )
            .unwrap();
        self.flush(runtime, sound, clock);
    }
    fn cancel_live_and_play_again(
        &mut self,
        runtime: &mut Runtime,
        sound: &mut Sound<'_>,
        clock: HostClock,
    ) {
        let now = clock.audio.rendered_frames();
        self.live
            .as_mut()
            .unwrap()
            .accept(performance_live::packet(clock, now, &[0x92, 31, 90]), clock)
            .unwrap();
        runtime.performance_live = self.live.take();
        runtime
            .request(sound, clock, Request::PerformancePlay)
            .unwrap();
        let live = runtime.performance_live.as_mut().unwrap();
        assert_eq!(live.engine.snapshot().active_voices, 0);
        assert_eq!(live.engine.snapshot().mode, SurfaceMode::Play);
        // Reuse the former member immediately: it must have fresh output ownership.
        live.accept(
            performance_live::packet(clock, now, &[0x91, 72, 100]),
            clock,
        )
        .unwrap();
        assert_eq!(live.engine.snapshot().active_voices, 1);
        self.live = runtime.performance_live.take();
        self.flush(runtime, sound, clock);
        assert_eq!(
            runtime
                .performance_playback
                .as_ref()
                .unwrap()
                .get(0)
                .launch_frame(),
            self.survivor
        );
        assert!(runtime.performance_playback.as_ref().unwrap().contains(1));
    }
    fn step(&mut self, runtime: &mut Runtime, sound: &mut Sound<'_>, clock: HostClock) {
        match self.stage {
            0 if runtime.initial_sound.is_some() => {
                runtime
                    .request(
                        sound,
                        clock,
                        Request::File {
                            action: if self.sample {
                                FileAction::PerformanceSampleExample
                            } else {
                                FileAction::PerformanceExample
                            },
                        },
                    )
                    .unwrap();
                self.stage = 1;
            }
            1 if runtime.performance.is_some() && runtime.pending_open.is_none() => {
                runtime.audition_performance(sound, clock, 0).unwrap();
                self.start_live(runtime, sound, clock, true);
                self.stage = 2;
            }
            2 => {
                self.feed(runtime, sound, clock);
                if clock.audio.rendered_frames() >= self.anchor + 28_800 {
                    self.reopen_and_launch(runtime, sound, clock);
                    self.stage = 3;
                }
            }
            3 if clock.audio.rendered_frames() >= self.anchor + 9_600 => {
                assert!(
                    clock.audio.active_notes() >= 6,
                    "live chord overlaps the two running clips"
                );
                self.cancel_live_and_play_again(runtime, sound, clock);
                self.stage = 4;
            }
            4 if clock.audio.rendered_frames() >= self.anchor + 19_200 => {
                // Both clips remain scheduled after Monitor cleanup; finish the one Play note.
                runtime.performance_live = self.live.take();
                runtime
                    .request(sound, clock, Request::PerformanceFinish)
                    .unwrap();
                assert_eq!(
                    runtime
                        .performance_playback
                        .as_ref()
                        .unwrap()
                        .get(0)
                        .launch_frame(),
                    self.survivor
                );
                assert!(runtime.performance_playback.as_ref().unwrap().contains(1));
                self.stage = 5;
            }
            5 if clock.audio.rendered_frames() >= self.anchor + 96_000 => {
                runtime.request(sound, clock, Request::Stop).unwrap();
                self.done.set(true);
                self.stage = 6;
            }
            _ => {}
        }
    }
}

fn verify_phrase(document: &crest_tracker::performance::Document) {
    let clip = document.session.slots[1].clip.as_ref().unwrap();
    let attacks: Vec<_> = clip
        .events
        .iter()
        .filter_map(|e| match e.gesture {
            Gesture::On { key, .. } => Some((e.note, key)),
            _ => None,
        })
        .collect();
    assert_eq!(
        attacks.iter().map(|(_, pitch)| *pitch).collect::<Vec<_>>(),
        [60, 64, 67, 65, 69, 74]
    );
    assert_eq!(clip.events.len(), 24);
    assert_eq!(clip.events[0].at.0, 0);
    for (index, (note, _)) in attacks.iter().enumerate() {
        verify_expression(clip, *note, index);
    }
}
fn verify_expression(
    clip: &crest_performance_core::Clip,
    note: crest_performance_core::NoteKey,
    index: usize,
) {
    let controls: Vec<_> = clip
        .events
        .iter()
        .filter(|e| e.note == note)
        .filter_map(|e| match e.gesture {
            Gesture::Expression { expression } => Some((e.at, expression)),
            _ => None,
        })
        .collect();
    if index < 3 {
        assert_eq!(controls.len(), 3);
        assert_eq!(controls[0].0 .0, 172_800);
        assert_eq!(controls[0].1.pitch, 750_000);
        assert_eq!(controls[1].1.timbre, 56_762);
        assert_eq!(controls[2].1.pressure, 46_442);
    } else if index == 5 {
        assert_eq!(controls.len(), 3);
        assert_eq!(controls[0].1.pitch, 1_500_000);
        assert_eq!(controls[1].1.timbre, 12_900);
        assert_eq!(controls[2].1.pressure, 33_025);
    } else {
        assert!(
            controls.is_empty(),
            "Top voice expression must not reach lower chord tones"
        );
    }
}
fn exercise(sample: bool) {
    let root = scenarios::temp_root("performance-harmony");
    let done = Rc::new(Cell::new(false));
    let complete = done.clone();
    let mut journey = Journey {
        stage: 0,
        sample,
        live: None,
        anchor: 0,
        packet: 0,
        changed: false,
        survivor: 0,
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
    assert!(evidence.peak_voices >= 6);
    assert!(evidence.audible_tracks[0]);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
#[ignore = "production DSP witness; run explicitly in release mode"]
fn sample_harmony_records_reopens_and_releases_monitor_without_stopping_clips() {
    exercise(true);
}
#[test]
#[ignore = "production DSP witness; run explicitly in release mode"]
fn braids_harmony_records_reopens_and_releases_monitor_without_stopping_clips() {
    exercise(false);
}
