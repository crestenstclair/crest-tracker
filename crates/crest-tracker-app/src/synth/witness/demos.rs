//! The native tours also execute without physical input, audio devices or a window.
use super::*;

fn run_scene(scene: super::super::scene::Scene, audible: bool, output: Option<PathBuf>) {
    run_scene_with_stalls(scene, audible, output, &[]);
}

fn run_scene_with_stalls(
    scene: super::super::scene::Scene,
    audible: bool,
    output: Option<PathBuf>,
    stalls: &'static [u64],
) {
    let root = scenarios::temp_root("feature-scene");
    let mut scene = scene;
    let mut injected = 0;
    let mut last_stall_frame = 0;
    let mut previous_beat = Beat::ZERO;
    let evidence = run_with_instrument(
        &root,
        Box::new(move |runtime, audio, evidence| {
            evidence.measured = true;
            if !stalls.is_empty() {
                assert_eq!(runtime.playback_recoveries, injected as u64);
                if let Some(at) = runtime.app.borrow().play_beat {
                    assert!(at >= previous_beat, "recovery must not rewind the cursor");
                    previous_beat = at;
                }
            }
            let done = scene
                .advance(
                    runtime,
                    runtime.last_clock.unwrap(),
                    evidence.projection.as_ref(),
                )
                .unwrap_or_else(|error| {
                    scene.report.record_failure(error.clone());
                    if let Some(path) = &output {
                        let report = serde_json::to_vec_pretty(&scene.report).unwrap();
                        crate::persistence::write_new(path, &report).unwrap();
                    }
                    panic!(
                        "{error}\n{}",
                        serde_json::to_string_pretty(&scene.report).unwrap()
                    )
                });
            if done {
                assert_eq!(injected, stalls.len(), "all planned stalls must execute");
                let report = serde_json::to_vec_pretty(&scene.report).unwrap();
                eprintln!("{}", String::from_utf8_lossy(&report));
                if let Some(path) = &output {
                    crate::persistence::write_new(path, &report).unwrap();
                }
            }
            if let Some(blocks) = stalls.get(injected) {
                let app = runtime.app.borrow();
                if app.playing
                    && !crate::workspace::owns_input(&app)
                    && app.workspace.synth_surface
                        == Some(crest_synth::control::SurfaceId::MixerMain)
                    && audio.rendered_frames() > last_stall_frame + 48_000
                {
                    // The device keeps rendering while the producer cannot
                    // tick. UI edits still enter through Scene's normal events.
                    evidence.control_stall_blocks = *blocks;
                    last_stall_frame = audio.rendered_frames() + blocks * FRAMES as u64;
                    injected += 1;
                }
            }
            done
        }),
        Duration::from_secs(
            std::env::var("CREST_DEMO_TIMEOUT_SECONDS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|v| *v > 0)
                .unwrap_or(360),
        ),
        DEFAULT_INSTRUMENT,
    );
    evidence.check_cleanup();
    if audible {
        assert!(
            evidence.audible_tracks.iter().any(|v| *v),
            "scene must render audible notes"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

macro_rules! feature_scene {
    ($test:ident, $scene:literal, $audible:literal) => {
        #[test]
        #[ignore = "production event scene and DSP; run with --release --test-threads=1"]
        fn $test() {
            let output = std::env::var_os("CREST_DEMO_REPORT_DIR")
                .map(|dir| PathBuf::from(dir).join(concat!($scene, ".json")));
            run_scene(
                super::super::scene::Scene::named($scene).unwrap(),
                $audible,
                output,
            );
        }
    };
}
feature_scene!(feature_event_scene_transport, "transport", true);
feature_scene!(feature_event_scene_drums, "drums", true);
feature_scene!(feature_event_scene_euclidean, "euclidean", true);
feature_scene!(feature_event_scene_random, "random", true);
feature_scene!(feature_event_scene_chords, "chords", true);
feature_scene!(feature_event_scene_fx, "fx", true);
feature_scene!(feature_event_scene_navigation, "navigation", false);
feature_scene!(feature_event_scene_rest_kill, "rest-kill", true);

#[test]
#[ignore = "set CREST_DEMO_SONG and CREST_DEMO_REPORT; production imported-song DSP"]
fn imported_song_event_scene() {
    let path = PathBuf::from(
        std::env::var_os("CREST_DEMO_SONG")
            .expect("set CREST_DEMO_SONG to an imported .crest-song"),
    );
    let output = std::env::var_os("CREST_DEMO_REPORT").map(PathBuf::from);
    run_scene(
        super::super::scene::Scene::song(&path).unwrap(),
        true,
        output,
    );
}

#[test]
#[ignore = "set CREST_DEMO_SONG; production imported-song Mixer scene and DSP"]
fn imported_song_mixer_event_scene() {
    let path = PathBuf::from(
        std::env::var_os("CREST_DEMO_SONG")
            .expect("set CREST_DEMO_SONG to an imported .crest-song"),
    );
    let output = std::env::var_os("CREST_DEMO_REPORT").map(PathBuf::from);
    run_scene(
        super::super::scene::Scene::mixer(&path).unwrap(),
        true,
        output,
    );
}

#[test]
#[ignore = "set CREST_DEMO_SONG; imported-song Mixer scene with forced audio deadlines"]
fn imported_song_mixer_recovers_from_missed_deadlines() {
    let path = PathBuf::from(
        std::env::var_os("CREST_DEMO_SONG")
            .expect("set CREST_DEMO_SONG to an imported .crest-song"),
    );
    let output = std::env::var_os("CREST_DEMO_REPORT").map(PathBuf::from);
    run_scene_with_stalls(
        super::super::scene::Scene::mixer(&path).unwrap(),
        true,
        output,
        &[40, 400, 80],
    );
}

#[test]
#[ignore = "production Mixer input scheduling and DSP; run with --test-threads=1"]
fn mixer_input_burst_yields_to_transport_and_preserves_order() {
    use crest_synth::control::{
        Direction, InteractionMode, MixerControlId, SemanticControlId, SemanticControlValue,
        SurfaceId,
    };
    use crest_synth::mixer::mixer_track_parameters::MixerTrackParameter;
    let root = scenarios::temp_root("mixer-input-burst");
    let mut stage = 0;
    let mut pending = 10;
    let mut graph_revision = 0;
    let mut baseline = (0.0, 0.0);
    let evidence = run(
        &root,
        Box::new(move |runtime, audio, evidence| {
            let values = evidence.projection.as_ref().and_then(|projection| {
                let model = projection.semantic_model();
                if model.active_surface() != SurfaceId::MixerMain {
                    return None;
                }
                let surface = model.surface(SurfaceId::MixerMain)?;
                let scalar = |parameter| {
                    surface.controls().iter().find_map(|control| {
                        match (control.path().control_id(), control.value()) {
                            (
                                SemanticControlId::Mixer(MixerControlId::Track {
                                    track_id,
                                    parameter: actual,
                                }),
                                SemanticControlValue::Scalar(value),
                            ) if track_id.value() == 0 && *actual == parameter => Some(*value),
                            _ => None,
                        }
                    })
                };
                Some((
                    scalar(MixerTrackParameter::Level)?,
                    scalar(MixerTrackParameter::Pan)?,
                ))
            });
            match stage {
                0 => {
                    scenarios::queue(runtime, Request::Example);
                    stage = 1;
                }
                1 if runtime.app.borrow().editor.song().name == "Window Light" => {
                    scenarios::queue(runtime, Request::Play);
                    scenarios::queue(
                        runtime,
                        Request::SynthPage {
                            page: SynthPage::Mixer,
                        },
                    );
                    stage = 2;
                }
                2 if audio.active_notes() > 0
                    && runtime.app.borrow().requests.is_empty()
                    && values.is_some() =>
                {
                    baseline = values.unwrap();
                    let mut app = runtime.app.borrow_mut();
                    assert!(!crate::workspace::owns_input(&app));
                    for action in [
                        SemanticAction::SetInteractionMode(InteractionMode::Adjust),
                        SemanticAction::Adjust(Direction::Down),
                        SemanticAction::Adjust(Direction::Down),
                        SemanticAction::Adjust(Direction::Down),
                        SemanticAction::Adjust(Direction::Down),
                        SemanticAction::SetInteractionMode(InteractionMode::Navigate),
                        SemanticAction::Navigate(Direction::Down),
                        SemanticAction::SetInteractionMode(InteractionMode::Adjust),
                        SemanticAction::Adjust(Direction::Right),
                        SemanticAction::SetInteractionMode(InteractionMode::Navigate),
                    ] {
                        assert!(crate::workspace::input::action(&mut app, action));
                    }
                    assert_eq!(app.requests.len(), pending);
                    graph_revision = runtime.graph_revision;
                    evidence.measured = true;
                    stage = 3;
                }
                3 => {
                    let app = runtime.app.borrow();
                    assert!(app.playing, "{:?}", app.playback_fault);
                    assert_eq!(runtime.graph_revision, graph_revision);
                    pending -= 1;
                    assert_eq!(
                        app.requests.len(),
                        pending,
                        "one input transaction per playback tick"
                    );
                    if pending == 0 {
                        stage = 4;
                    }
                }
                4 => {
                    let Some((level, pan)) = values else {
                        return false;
                    };
                    if (pan - baseline.1 - 0.01).abs() > 0.00001 {
                        return false;
                    }
                    assert_eq!(level, baseline.0 - 24.0);
                    assert!(runtime.app.borrow().playing);
                    scenarios::queue(runtime, Request::Stop);
                    stage = 5;
                }
                5 if !runtime.app.borrow().playing && audio.active_notes() == 0 => return true,
                _ => {}
            }
            false
        }),
        Duration::from_secs(30),
    );
    evidence.check_cleanup();
    assert!(evidence.audible_tracks.iter().any(|value| *value));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "set CREST_DEMO_SONG and CREST_STRESS_SCENE; corpus edit/navigation/Mixer production DSP"]
fn imported_song_stress_event_scene() {
    let path = PathBuf::from(std::env::var_os("CREST_DEMO_SONG").expect("set CREST_DEMO_SONG"));
    let name = std::env::var("CREST_STRESS_SCENE").unwrap_or_else(|_| "edit".into());
    let output = std::env::var_os("CREST_DEMO_REPORT").map(PathBuf::from);
    run_scene(
        super::super::scene::Scene::stress(&path, &name).unwrap(),
        true,
        output,
    );
}
