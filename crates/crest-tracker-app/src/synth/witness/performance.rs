//! Released firmware capture, library loop engine and actual hosted Braids DSP.
use super::*;
use crest_synth::shell::webview::host_presentation::WorkspaceEvent;

fn activate(runtime: &Runtime, id: &str) {
    crate::workspace::event(
        &mut runtime.app.borrow_mut(),
        WorkspaceEvent::Activate { id: id.into() },
    );
}

#[test]
#[ignore = "production DSP witness; run explicitly in release mode"]
fn released_performance_clip_loops_through_native_owned_expression_and_stops() {
    let root = scenarios::temp_root("performance");
    let mut stage = 0;
    let mut started = 0;
    let evidence = run(
        &root,
        Box::new(move |runtime, audio, evidence| {
            match stage {
                0 if runtime.initial_sound.is_some() => {
                    activate(runtime, "menu");
                    activate(runtime, "performance");
                    activate(runtime, "performance_example");
                    stage = 1;
                }
                1 if runtime.app.borrow().editor.song().name
                    == "Expressive performance example" =>
                {
                    assert!(runtime.performance.is_some());
                    activate(runtime, "menu");
                    activate(runtime, "performance");
                    activate(runtime, "performance_audition:0");
                    stage = 2;
                }
                2 if runtime.performance_playback.is_some() => {
                    started = audio.rendered_frames();
                    evidence.measured = true;
                    stage = 3;
                }
                3 if audio.rendered_frames() > started + 48_000 * 4 => {
                    assert!(
                        runtime.app.borrow().playing,
                        "{}",
                        runtime.app.borrow().status
                    );
                    assert!(runtime.app.borrow().playback_fault.is_none());
                    activate(runtime, "stop");
                    stage = 4;
                }
                4 if !runtime.app.borrow().playing && audio.active_notes() == 0 => return true,
                _ => {}
            }
            false
        }),
        Duration::from_secs(45),
    );
    evidence.check_cleanup();
    assert_eq!(evidence.peak_voices, 3);
    assert!(
        evidence.audible_tracks[0],
        "the actual embedded engine must produce audible output"
    );
    assert!(evidence.scheduled_queue_high_water > 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "production DSP witness; run explicitly in release mode"]
fn performance_audition_refuses_an_engine_without_owned_expression_support() {
    let root = scenarios::temp_root("performance-unsupported");
    let mut issued = false;
    let evidence = run_with_instrument(
        &root,
        Box::new(move |runtime, _audio, _evidence| {
            if runtime.initial_sound.is_none() {
                return false;
            }
            if !issued {
                let instrument = *runtime
                    .app
                    .borrow()
                    .editor
                    .song()
                    .instruments
                    .keys()
                    .next()
                    .unwrap();
                let mut document = super::super::performance::example_document()
                    .unwrap()
                    .performance
                    .unwrap();
                document.routes[0].instrument = instrument;
                runtime.performance = Some(
                    crest_tracker::performance::Performance::new(
                        document,
                        crest_linnstrument::Geometry::Pads200,
                        crest_performance_core::Limits::default(),
                    )
                    .unwrap(),
                );
                runtime
                    .app
                    .borrow_mut()
                    .requests
                    .push(Request::PerformanceAudition { slot: 0 });
                issued = true;
                return false;
            }
            assert!(runtime.performance_playback.is_none());
            assert!(!runtime.app.borrow().playing);
            assert!(
                runtime
                    .app
                    .borrow()
                    .status
                    .contains("no proven owned-expression profile"),
                "{}",
                runtime.app.borrow().status
            );
            true
        }),
        Duration::from_secs(45),
        DEFAULT_INSTRUMENT,
    );
    evidence.check_cleanup();
    assert_eq!(evidence.peak_voices, 0);
    assert_eq!(evidence.scheduled_queue_high_water, 0);
    std::fs::remove_dir_all(root).unwrap();
}

fn independent_expression_document() -> crest_tracker::performance::Document {
    use crest_performance_core::{BeatTime, Command, Limits};
    let mut document = super::super::performance::example_document()
        .unwrap()
        .performance
        .unwrap();
    document.session = crest_performance_core::Session::default();
    let mut engine = crest_tracker::performance::Performance::new(
        document,
        crest_linnstrument::Geometry::Pads200,
        Limits::default(),
    )
    .unwrap();
    engine.command(Command::BeginRecord { slot: 0 }).unwrap();
    // Three simultaneous voices share one native Patch. Independently specified
    // gestures bend only A, change only B's timbre, then scale only C's pressure.
    for bytes in [
        [0xd1, 127, 0xf8],
        [0xd2, 127, 0xf8],
        [0xd3, 127, 0xf8],
        [0x91, 60, 100],
        [0x92, 65, 100],
        [0x93, 71, 100],
    ] {
        engine.accept_midi(Beat::ZERO, &bytes).unwrap();
        engine.drain_output().for_each(drop);
    }
    for (at, bytes) in [
        (2, [0xe1, 85, 66]),
        (4, [0xb2, 74, 110]),
        (6, [0xd3, 32, 0xf8]),
        (8, [0x81, 60, 0]),
        (8, [0x82, 65, 0]),
        (8, [0x83, 71, 0]),
    ] {
        engine.accept_midi(Beat::integer(at), &bytes).unwrap();
        engine.drain_output().for_each(drop);
    }
    engine.advance(Beat::integer(9)).unwrap();
    engine.command(Command::FinishRecord).unwrap();
    engine.launch_quantum = BeatTime(0);
    engine.document().unwrap()
}

/// Narrow-band amplitude in a stable quarter-second window. Expected frequencies
/// come from MIDI pitch, independently of library output or native trace generation.
fn magnitude(pcm: &[f32], start: usize, frequency: f64) -> f64 {
    let frames = 12_000;
    let mut real = 0.0;
    let mut imaginary = 0.0;
    for frame in 0..frames {
        let window = 0.5 - 0.5 * (std::f64::consts::TAU * frame as f64 / (frames - 1) as f64).cos();
        let phase = std::f64::consts::TAU * frequency * frame as f64 / 48_000.0;
        let sample = f64::from(pcm[(start + frame) * 2]) * window;
        real += sample * phase.cos();
        imaginary += sample * phase.sin();
    }
    real.hypot(imaginary) / frames as f64
}

#[test]
#[ignore = "production DSP and spectral witness; run explicitly in release mode"]
fn three_library_notes_keep_independent_pitch_timbre_and_pressure_in_rendered_audio() {
    let root = scenarios::temp_root("performance-expression-audio");
    let output = audio::Output::default();
    output.record();
    let anchor = Rc::new(std::cell::Cell::new(0_u64));
    let observed = Rc::clone(&anchor);
    let mut stage = 0;
    let evidence = run_with_output(
        &root,
        Box::new(move |runtime, audio, evidence| {
            match stage {
                0 if runtime.initial_sound.is_some() => {
                    runtime.app.borrow_mut().requests.push(Request::File {
                        action: FileAction::PerformanceExample,
                    });
                    stage = 1;
                }
                1 if runtime.app.borrow().editor.song().name
                    == "Expressive performance example" =>
                {
                    runtime.performance = Some(
                        crest_tracker::performance::Performance::new(
                            independent_expression_document(),
                            crest_linnstrument::Geometry::Pads200,
                            crest_performance_core::Limits::default(),
                        )
                        .unwrap(),
                    );
                    runtime
                        .app
                        .borrow_mut()
                        .requests
                        .push(Request::PerformanceAudition { slot: 0 });
                    stage = 2;
                }
                2 if runtime.performance_playback.is_some() => {
                    observed.set(runtime.performance_playback.as_ref().unwrap().anchor());
                    evidence.measured = true;
                    stage = 3;
                }
                3 if audio.rendered_frames() > observed.get() + 48_000 * 4 => {
                    assert!(
                        runtime.app.borrow().playing,
                        "{}",
                        runtime.app.borrow().status
                    );
                    runtime.app.borrow_mut().requests.push(Request::Stop);
                    stage = 4;
                }
                4 if !runtime.app.borrow().playing && audio.active_notes() == 0 => return true,
                _ => {}
            }
            false
        }),
        Duration::from_secs(45),
        crest_synth::adapter::braids_capability::BRAIDS_CAPABILITY_ID,
        false,
        output.clone(),
    );
    evidence.check_cleanup();
    assert_eq!(evidence.peak_voices, 3);
    let pcm = output.take_recording();
    verify_independent_audio(&pcm, anchor.get());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "production scheduling fault witness; run explicitly in release mode"]
fn missed_performance_deadline_cancels_owned_voices_without_replaying_overdue_events() {
    let root = scenarios::temp_root("performance-deadline");
    let mut stage = 0;
    let evidence = run(
        &root,
        Box::new(move |runtime, audio, evidence| {
            match stage {
                0 if runtime.initial_sound.is_some() => {
                    runtime.app.borrow_mut().requests.push(Request::File {
                        action: FileAction::PerformanceExample,
                    });
                    stage = 1;
                }
                1 if runtime.app.borrow().editor.song().name
                    == "Expressive performance example" =>
                {
                    runtime
                        .app
                        .borrow_mut()
                        .requests
                        .push(Request::PerformanceAudition { slot: 0 });
                    stage = 2;
                }
                2 if audio.active_notes() == 3 => {
                    evidence.measured = true;
                    evidence.control_stall_blocks = 24; // 128 ms, beyond the 100 ms published window.
                    stage = 3;
                }
                3 if runtime.performance_playback.is_none() && audio.active_notes() == 0 => {
                    assert!(!runtime.app.borrow().playing);
                    assert!(runtime
                        .app
                        .borrow()
                        .playback_fault
                        .as_ref()
                        .is_some_and(|e| e.contains("missed its audio deadline")));
                    return true;
                }
                _ => {}
            }
            false
        }),
        Duration::from_secs(45),
    );
    evidence.check_cleanup();
    assert_eq!(evidence.peak_voices, 3);
    std::fs::remove_dir_all(root).unwrap();
}

fn verify_independent_audio(pcm: &[f32], anchor: u64) {
    let at = |seconds: f64| anchor as usize + (seconds * 48_000.0) as usize;
    let midi_frequency = |key: f64| 440.0 * 2.0_f64.powf((key - 69.0) / 12.0);
    let amplitude = |second, key| magnitude(pcm, at(second), midi_frequency(key));
    let a_before = amplitude(0.65, 60.0);
    let a_after = amplitude(1.65, 62.0);
    assert!(a_before > 0.0001 && a_after > 0.0001);
    assert!(
        amplitude(1.65, 60.0) < a_before * 0.05,
        "A's original pitch must disappear"
    );
    for key in [65.0, 71.0] {
        let ratio = amplitude(1.65, key) / amplitude(0.65, key);
        assert!(
            (0.95..1.05).contains(&ratio),
            "B/C changed during A bend: {key} {ratio}"
        );
    }
    for key in [62.0, 71.0] {
        let ratio = amplitude(2.65, key) / amplitude(1.65, key);
        assert!(
            (0.95..1.05).contains(&ratio),
            "A/C changed during B timbre: {key} {ratio}"
        );
    }
    let timbre_before =
        magnitude(pcm, at(1.65), midi_frequency(65.0) * 3.0) / amplitude(1.65, 65.0);
    let timbre_after = magnitude(pcm, at(2.65), midi_frequency(65.0) * 3.0) / amplitude(2.65, 65.0);
    assert!(
        (timbre_after - timbre_before).abs() > 0.01,
        "B's harmonic spectrum must respond to timbre: {timbre_before} {timbre_after}"
    );
    let pressure_ratio = amplitude(3.65, 71.0) / amplitude(2.65, 71.0);
    assert!(
        (pressure_ratio - 32.0 / 127.0).abs() < 0.03,
        "C pressure gain: {pressure_ratio}"
    );
    for key in [62.0, 65.0] {
        let ratio = amplitude(3.65, key) / amplitude(2.65, key);
        assert!(
            (0.95..1.05).contains(&ratio),
            "A/B changed during C pressure: {key} {ratio}"
        );
    }
}
