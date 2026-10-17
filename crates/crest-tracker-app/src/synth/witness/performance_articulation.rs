//! Held gestures keep generating on the native clock with no intervening packets.
use super::performance_live::packet;
use super::*;
use crate::synth::performance::Live;
use crest_performance_core::{Articulation, BeatTime, Gesture, StrumDirection, SurfaceMode};
use std::cell::Cell;

struct Journey {
    stage: u8,
    sample: bool,
    articulation: Articulation,
    live: Option<Live>,
    anchor: u64,
    sent: usize,
    checked_tones: bool,
    emitted_without_packet: bool,
    done: Rc<Cell<bool>>,
}
const INPUT: &[(u64, &[u8])] = &[
    (4_800, &[0x91, 30, 100]),
    (19_000, &[0xe1, 0, 65]),
    (19_000, &[0xb1, 74, 110]),
    (19_000, &[0xd1, 90]),
    (23_000, &[0x81, 30, 0]),
];
impl Journey {
    fn begin(&mut self, runtime: &Runtime, sound: &Sound<'_>, clock: HostClock, record: bool) {
        let mut document = runtime.performance_document().unwrap().unwrap();
        document.session.harmony.articulation = self.articulation;
        let mut live = Live::new(document, &runtime.app.borrow(), sound, clock, 1, 0).unwrap();
        live.engine
            .command(crest_performance_core::Command::SetMode(
                SurfaceMode::Chords,
            ))
            .unwrap();
        if record {
            live.engine.arm_recording(1).unwrap();
        }
        self.anchor = clock.audio.rendered_frames();
        self.live = Some(live);
    }
    fn feed(&mut self, runtime: &mut Runtime, sound: &mut Sound<'_>, clock: HostClock) {
        let elapsed = clock.audio.rendered_frames() - self.anchor;
        let live = self.live.as_mut().unwrap();
        let sent_before = self.sent;
        while self.sent < INPUT.len() && INPUT[self.sent].0 <= elapsed {
            let (offset, bytes) = INPUT[self.sent];
            live.accept(packet(clock, self.anchor + offset, bytes), clock)
                .unwrap();
            self.sent += 1;
        }
        let emitted = live
            .advance(
                sound,
                clock,
                runtime.app.borrow().editor.song().tempo,
                &mut runtime.voices,
            )
            .unwrap();
        self.emitted_without_packet |= emitted && sent_before == self.sent && self.sent == 1;
        if !self.checked_tones && elapsed >= 18_000 {
            assert_eq!(self.sent, 1, "no packet after the initial trigger yet");
            if !matches!(self.articulation, Articulation::Arpeggio { .. }) {
                assert_eq!(
                    clock.audio.active_notes(),
                    3,
                    "quiet controller must still sound all delayed chord tones"
                );
            }
            assert!(
                self.emitted_without_packet,
                "generated output must dirty recovery without a new packet"
            );
            let checkpoint = live
                .checkpoint(Some(clock.audio.rendered_frames()))
                .unwrap();
            let prefix = checkpoint.session.slots[1].clip.as_ref().unwrap();
            assert_eq!(
                prefix
                    .events
                    .iter()
                    .filter(|event| matches!(event.gesture, Gesture::On { .. }))
                    .count(),
                3
            );
            self.checked_tones = true;
        }
    }
    fn reopen(&mut self, runtime: &mut Runtime, sound: &mut Sound<'_>, clock: HostClock) {
        let live = self.live.as_mut().unwrap();
        let before = serde_json::to_value(live.engine.snapshot()).unwrap();
        // Later than the last input, but older than the already settled past.
        let late = clock.audio.rendered_frames() - 1_440;
        assert!(live
            .accept(packet(clock, late, &[0xe1, 0, 66]), clock)
            .unwrap_err()
            .contains("input horizon"));
        assert_eq!(
            serde_json::to_value(live.engine.snapshot()).unwrap(),
            before
        );
        runtime.performance_live = self.live.take();
        runtime
            .request(sound, clock, Request::PerformanceFinish)
            .unwrap();
        let document = runtime
            .capture_document(serde_json::to_value(sound.capture_saved_session().unwrap()).unwrap())
            .unwrap();
        let restored: Document =
            serde_json::from_slice(&serde_json::to_vec(&document).unwrap()).unwrap();
        restored.validate().unwrap();
        let performance = restored.performance.unwrap();
        assert_eq!(Some(&performance), document.performance.as_ref());
        verify_phrase(
            performance.session.slots[1].clip.as_ref().unwrap(),
            self.articulation,
        );
        runtime.performance = Some(
            crest_tracker::performance::Performance::new(
                performance,
                crest_linnstrument::Geometry::Pads200,
                crest_performance_core::Limits::default(),
            )
            .unwrap(),
        );
        runtime.audition_performance(sound, clock, 0).unwrap();
        runtime.audition_performance(sound, clock, 1).unwrap();
        self.begin(runtime, sound, clock, false);
        let live = self.live.as_mut().unwrap();
        live.accept(packet(clock, self.anchor, &[0x91, 30, 100]), clock)
            .unwrap();
        live.advance(
            sound,
            clock,
            runtime.app.borrow().editor.song().tempo,
            &mut runtime.voices,
        )
        .unwrap();
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
                self.begin(runtime, sound, clock, true);
                self.stage = 2;
            }
            2 => {
                self.feed(runtime, sound, clock);
                if clock.audio.rendered_frames() >= self.anchor + 30_000 {
                    assert!(self.checked_tones);
                    self.reopen(runtime, sound, clock);
                    self.stage = 3;
                }
            }
            3 if clock.audio.rendered_frames() >= self.anchor + 1_000 => {
                self.live
                    .as_mut()
                    .unwrap()
                    .set_mode(sound, SurfaceMode::Play)
                    .unwrap();
                assert!(runtime.performance_playback.as_ref().unwrap().contains(0));
                assert!(runtime.performance_playback.as_ref().unwrap().contains(1));
                self.stage = 4;
            }
            4 => {
                let live = self.live.as_mut().unwrap();
                live.advance(
                    sound,
                    clock,
                    runtime.app.borrow().editor.song().tempo,
                    &mut runtime.voices,
                )
                .unwrap();
                assert_eq!(
                    live.engine.snapshot().active_voices,
                    0,
                    "canceled delayed tones must never reappear"
                );
                if clock.audio.rendered_frames() >= self.anchor + 80_000 {
                    runtime.request(sound, clock, Request::Stop).unwrap();
                    self.done.set(true);
                    self.stage = 5;
                }
            }
            _ => {}
        }
    }
}
fn verify_phrase(clip: &crest_performance_core::Clip, articulation: Articulation) {
    let attacks: Vec<_> = clip
        .events
        .iter()
        .filter_map(|event| match event.gesture {
            Gesture::On {
                key,
                velocity,
                expression,
            } => Some((event.at.0, key, velocity, expression)),
            _ => None,
        })
        .collect();
    let timing: Vec<_> = attacks.iter().map(|(at, key, _, _)| (*at, *key)).collect();
    match articulation {
        Articulation::Strum { .. } => assert_eq!(timing, [(0, 60), (172_800, 64), (345_600, 67)]),
        Articulation::Arpeggio { .. } => {
            assert_eq!(
                timing,
                [(0, 60), (172_800, 64), (345_600, 67), (518_400, 60)]
            );
            let releases: Vec<_> = clip
                .events
                .iter()
                .filter(|e| matches!(e.gesture, Gesture::Off { .. }))
                .map(|e| e.at.0)
                .collect();
            assert_eq!(releases, [86_400, 259_200, 432_000, 604_800]);
            assert_eq!(attacks[3].3.pitch, 750_000);
            assert_eq!(attacks[3].3.timbre, 56_762);
            assert_eq!(attacks[3].3.pressure, 46_442);
        }
        Articulation::Humanized { max_delay, .. } => {
            assert_eq!(attacks.len(), 3);
            let mut pitches: Vec<_> = attacks.iter().map(|event| event.1).collect();
            pitches.sort_unstable();
            assert_eq!(pitches, [60, 64, 67]);
            assert!(attacks
                .iter()
                .all(|(at, _, velocity, _)| *at <= max_delay.0 && (92..=108).contains(velocity)));
            assert!(attacks.iter().any(|event| event.0 > 0));
        }
        _ => panic!("delayed articulation witness"),
    }
    for on in clip
        .events
        .iter()
        .filter(|e| matches!(e.gesture, Gesture::On { .. }))
    {
        assert_eq!(
            clip.events
                .iter()
                .filter(|off| off.note == on.note && matches!(off.gesture, Gesture::Off { .. }))
                .count(),
            1
        );
    }
    assert_eq!(clip.harmony_origin.len(), INPUT.len());
}
fn exercise(sample: bool, articulation: Articulation) {
    let root = scenarios::temp_root("performance-articulation");
    let done = Rc::new(Cell::new(false));
    let complete = done.clone();
    let mut journey = Journey {
        stage: 0,
        sample,
        articulation,
        live: None,
        anchor: 0,
        sent: 0,
        checked_tones: false,
        emitted_without_packet: false,
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
    assert!(evidence.audible_tracks[0]);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
#[ignore = "production DSP witness; run explicitly in release mode"]
fn held_strum_advances_without_packets_and_reopens_in_sample_and_braids() {
    for sample in [true, false] {
        exercise(
            sample,
            Articulation::Strum {
                direction: StrumDirection::Ascending,
                interval: BeatTime(172_800),
            },
        );
    }
}
#[test]
#[ignore = "production DSP witness; run explicitly in release mode"]
fn held_arpeggio_advances_without_packets_and_reopens_in_sample_and_braids() {
    for sample in [true, false] {
        exercise(
            sample,
            Articulation::Arpeggio {
                direction: StrumDirection::Ascending,
                interval: BeatTime(172_800),
                gate: BeatTime(86_400),
            },
        );
    }
}
#[test]
#[ignore = "production DSP witness; run explicitly in release mode"]
fn held_humanized_chord_advances_without_packets_and_reopens_in_sample_and_braids() {
    for sample in [true, false] {
        exercise(
            sample,
            Articulation::Humanized {
                max_delay: BeatTime(120_000),
                velocity_variation: 8,
                seed: 42,
            },
        );
    }
}
