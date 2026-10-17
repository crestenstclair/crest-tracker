//! Native Sample capture/playback with deterministic channel ingress replacing
//! the physical connection. Production clock mapping, library and DSP remain real.
use super::*;
use crate::synth::performance::Live;
use crest_performance_core::{BeatTime, Gesture};
use crest_synth::control::{MidiConnectionRevision, PhysicalMidiIngress, PhysicalMidiPacket};
use std::cell::Cell;

const INPUT: &[(u64, &[u8])] = &[
    (4_800, &[0x91, 60, 100]),
    (4_800, &[0x92, 64, 90]),
    (4_800, &[0x93, 67, 80]),
    (9_600, &[0xe1, 0, 65]),
    (9_600, &[0xb2, 74, 110]),
    (9_600, &[0xd3, 90]),
    (12_000, &[0x81, 60, 0]),
    (12_048, &[0x82, 64, 0]),
    (12_096, &[0x83, 67, 0]),
];
pub(super) fn packet(
    clock: HostClock,
    frame: u64,
    bytes: &[u8],
) -> crest_synth::control::PhysicalMidiEvent {
    let timestamp = audio_timestamp_at_frame(clock, frame).unwrap();
    let (mut producer, mut consumer) = PhysicalMidiIngress::bounded(MidiConnectionRevision::FIRST);
    consumer.enable();
    producer.receive_raw(timestamp, bytes);
    let PhysicalMidiPacket::Performance(event) = consumer.try_pop_packet().unwrap() else {
        panic!("channel input")
    };
    event
}
struct Journey {
    stage: u8,
    live: Option<Live>,
    anchor: u64,
    packet: usize,
    relaunched_during_take: bool,
    done: Rc<Cell<bool>>,
}
impl Journey {
    fn begin_input(
        &mut self,
        runtime: &Runtime,
        sound: &Sound<'_>,
        clock: HostClock,
        record: bool,
    ) {
        let mut live = Live::new(
            runtime.performance_document().unwrap().unwrap(),
            &runtime.app.borrow(),
            sound,
            clock,
            1,
            0,
        )
        .unwrap();
        if record {
            live.engine.arm_recording(1).unwrap();
        }
        self.live = Some(live);
        self.anchor = clock.audio.rendered_frames();
        self.packet = 0;
    }
    fn feed(&mut self, runtime: &mut Runtime, sound: &mut Sound<'_>, clock: HostClock) {
        let elapsed = clock.audio.rendered_frames() - self.anchor;
        while self.packet < INPUT.len() && INPUT[self.packet].0 <= elapsed {
            let (offset, bytes) = INPUT[self.packet];
            let event = packet(clock, self.anchor + offset, bytes);
            self.live.as_mut().unwrap().accept(event, clock).unwrap();
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
    fn finish_and_reopen(
        &mut self,
        runtime: &mut Runtime,
        sound: &mut Sound<'_>,
        clock: HostClock,
    ) {
        runtime.performance_live = self.live.take();
        runtime
            .request(sound, clock, Request::PerformanceFinish)
            .unwrap();
        assert!(
            runtime.performance_playback.is_some(),
            "finishing input must preserve the clip"
        );
        let saved = runtime
            .capture_document(serde_json::to_value(sound.capture_saved_session().unwrap()).unwrap())
            .unwrap();
        let restored: Document =
            serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        restored.validate().unwrap();
        let document = restored.performance.unwrap();
        assert_eq!(Some(&document), saved.performance.as_ref());
        let clip = document.session.slots[1].clip.as_ref().unwrap();
        assert_eq!(clip.events.len(), 9);
        assert_eq!(clip.events[0].at, BeatTime(0));
        // At 108 BPM, a 100 ms source interval is exactly 172,800 library ticks.
        assert_eq!(clip.events[3].at, BeatTime(172_800));
        assert!(
            matches!(clip.events[3].gesture, Gesture::Expression { expression } if expression.pitch == 750_000)
        );
        assert!(
            matches!(clip.events[4].gesture, Gesture::Expression { expression } if expression.timbre == 56_762)
        );
        assert!(
            matches!(clip.events[5].gesture, Gesture::Expression { expression } if expression.pressure == 46_442)
        );
        runtime.performance = Some(
            crest_tracker::performance::Performance::new(
                document,
                crest_linnstrument::Geometry::Pads200,
                crest_performance_core::Limits::default(),
            )
            .unwrap(),
        );
        let previous_launch = runtime
            .performance_playback
            .as_ref()
            .unwrap()
            .get(0)
            .launch_frame();
        runtime.audition_performance(sound, clock, 1).unwrap();
        let bank = runtime.performance_playback.as_ref().unwrap();
        assert!(bank.contains(0) && bank.contains(1));
        assert_eq!(
            bank.get(0).launch_frame(),
            previous_launch,
            "adding a clip cannot restart the original"
        );
        self.begin_input(runtime, sound, clock, false);
    }
    fn relaunch_during_take(
        &mut self,
        runtime: &mut Runtime,
        sound: &mut Sound<'_>,
        clock: HostClock,
    ) {
        runtime.performance_live = self.live.take();
        let before = runtime.performance_live.as_ref().unwrap().engine.snapshot();
        assert!(runtime.performance_live.as_ref().unwrap().recording());
        runtime.audition_performance(sound, clock, 0).unwrap(); // Stop only this clip.
        assert!(runtime.performance_playback.is_none());
        runtime.audition_performance(sound, clock, 0).unwrap(); // Start while input records slot 1.
        assert!(runtime.performance_playback.as_ref().unwrap().contains(0));
        let live = runtime.performance_live.as_ref().unwrap();
        assert!(live.recording());
        assert_eq!(
            serde_json::to_value(live.engine.snapshot()).unwrap(),
            serde_json::to_value(before).unwrap()
        );
        assert!(runtime.audition_performance(sound, clock, 1).is_err());
        assert!(runtime.performance_playback.as_ref().unwrap().contains(0));
        self.live = runtime.performance_live.take();
        self.relaunched_during_take = true;
    }
    fn interrupt_and_retain(
        &mut self,
        runtime: &mut Runtime,
        sound: &mut Sound<'_>,
        clock: HostClock,
    ) {
        let mut live = self.live.take().unwrap();
        live.engine.arm_recording(2).unwrap();
        runtime.performance_live = Some(live);
        let now = clock.audio.rendered_frames();
        assert!(runtime.midi_input(packet(clock, now, &[0x91, 60, 100]), clock));
        assert!(runtime.app.borrow().status.starts_with("RECORDING"));
        assert!(runtime.midi_input(packet(clock, now - 48, &[0x81, 60, 0]), clock));
        assert!(runtime
            .performance_live
            .as_ref()
            .unwrap()
            .fault
            .as_ref()
            .unwrap()
            .contains("backwards"));
        runtime.stop(sound).unwrap();
        let document = runtime.performance_document().unwrap().unwrap();
        let retained = document.session.slots[2].clip.as_ref().unwrap();
        assert_eq!(retained.length, BeatTime(1));
        assert_eq!(retained.events.len(), 2);
        assert!(matches!(
            retained.events[0].gesture,
            Gesture::On { key: 60, .. }
        ));
        assert!(matches!(retained.events[1].gesture, Gesture::Off { .. }));
        assert_eq!(
            document.session.slots[1]
                .clip
                .as_ref()
                .unwrap()
                .events
                .len(),
            9
        );
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
                runtime.audition_performance(sound, clock, 0).unwrap();
                self.begin_input(runtime, sound, clock, true);
                self.stage = 2;
            }
            2 => {
                self.feed(runtime, sound, clock);
                if !self.relaunched_during_take
                    && clock.audio.rendered_frames() >= self.anchor + 6_000
                {
                    self.relaunch_during_take(runtime, sound, clock);
                }
                if clock.audio.rendered_frames() >= self.anchor + 14_400 {
                    self.finish_and_reopen(runtime, sound, clock);
                    self.stage = 3;
                }
            }
            3 => {
                self.feed(runtime, sound, clock);
                if clock.audio.rendered_frames() >= self.anchor + 96_000 {
                    self.interrupt_and_retain(runtime, sound, clock);
                    self.done.set(true);
                    self.stage = 4;
                }
            }
            _ => {}
        }
    }
}

#[test]
#[ignore = "production DSP witness; run explicitly in release mode"]
fn native_sample_live_capture_reopens_and_loops_with_independent_live_expression() {
    let root = scenarios::temp_root("performance-live");
    let done = Rc::new(Cell::new(false));
    let complete = done.clone();
    let mut journey = Journey {
        stage: 0,
        live: None,
        anchor: 0,
        packet: 0,
        relaunched_during_take: false,
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
    // Both three-note clips now continue alongside three independent live notes.
    assert_eq!(evidence.peak_voices, 9);
    assert!(evidence.audible_tracks[0]);
    std::fs::remove_dir_all(root).unwrap();
}
