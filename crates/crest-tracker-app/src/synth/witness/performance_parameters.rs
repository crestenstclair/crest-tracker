//! Unsupported parameter data must never start a partial native schedule or
//! cancel an unrelated clip that is already sounding.
use super::*;
use crest_linnstrument::Geometry;
use crest_performance_core::Limits;
use crest_tracker::performance::Performance;

fn install_locked_and_plain_clips(runtime: &mut Runtime) {
    let mut document = runtime.performance.as_ref().unwrap().document().unwrap();
    let clip = document.session.slots[0].clip.as_ref().unwrap().clone();
    let note = clip.notes().unwrap().last().unwrap().id;
    document.session.slots[1].clip = Some(clip.clone());
    document.session.slots[0].clip = Some(clip.with_note_parameter(note, 4, Some(54_321)).unwrap());
    runtime.performance =
        Some(Performance::new(document, Geometry::Pads200, Limits::default()).unwrap());
}

#[test]
#[ignore = "production DSP witness; run explicitly in release mode"]
fn performance_parameter_rejection_preserves_an_independent_sounding_clip() {
    let root = scenarios::temp_root("performance-parameters");
    let mut stage = 0;
    let mut started = 0;
    let evidence = run(
        &root,
        Box::new(move |runtime, audio, evidence| {
            match stage {
                0 if runtime.initial_sound.is_some() => {
                    for id in ["menu", "performance", "performance_example"] {
                        crate::workspace::event(
                            &mut runtime.app.borrow_mut(),
                            crest_synth::shell::webview::host_presentation::WorkspaceEvent::Activate { id: id.into() },
                        );
                    }
                    stage = 1;
                }
                1 if runtime.app.borrow().editor.song().name
                    == "Expressive performance example" =>
                {
                    install_locked_and_plain_clips(runtime);
                    runtime
                        .app
                        .borrow_mut()
                        .requests
                        .push(Request::PerformanceAudition { slot: 1 });
                    stage = 2;
                }
                2 if audio.active_notes() > 0 => {
                    started = audio.rendered_frames();
                    evidence.measured = true;
                    runtime
                        .app
                        .borrow_mut()
                        .requests
                        .push(Request::PerformanceAudition { slot: 0 });
                    stage = 3;
                }
                3 => {
                    let bank = runtime
                        .performance_playback
                        .as_ref()
                        .expect("unrelated clip survives");
                    assert!(bank.contains(1));
                    assert!(!bank.contains(0));
                    assert!(runtime.app.borrow().playing);
                    assert!(
                        runtime
                            .app
                            .borrow()
                            .status
                            .contains("does not support per-note parameter locks"),
                        "{}",
                        runtime.app.borrow().status
                    );
                    let clip = runtime
                        .performance
                        .as_ref()
                        .unwrap()
                        .clip(0)
                        .unwrap()
                        .unwrap();
                    assert_eq!(clip.parameter_locks[0].value, 54_321);
                    if audio.rendered_frames() > started + 48_000 * 2 {
                        runtime.app.borrow_mut().requests.push(Request::Stop);
                        stage = 4;
                    }
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
    assert!(evidence.audible_tracks[0]);
    assert!(evidence.scheduled_queue_high_water > 0);
    std::fs::remove_dir_all(root).unwrap();
}
