use super::*;

#[test]
#[ignore = "production cursor event scene and DSP; run with --test-threads=1"]
fn independent_cursor_event_scene_checks_shared_phrases_cells_loops_and_cleanup() {
    let root = scenarios::temp_root("cursor-scene");
    let mut scene = super::super::scene::Scene::cursors();
    let evidence = run_with_instrument(
        &root,
        Box::new(move |runtime, _audio, evidence| {
            evidence.measured = true;
            let done = scene
                .advance(
                    runtime,
                    runtime.last_clock.unwrap(),
                    evidence.projection.as_ref(),
                )
                .unwrap_or_else(|error| {
                    panic!(
                        "{error}\n{}",
                        serde_json::to_string_pretty(&scene.report).unwrap()
                    )
                });
            if done {
                eprintln!("{}", serde_json::to_string_pretty(&scene.report).unwrap());
            }
            done
        }),
        Duration::from_secs(60),
        DEFAULT_INSTRUMENT,
    );
    evidence.check_cleanup();
    assert!(
        evidence.audible_tracks[0],
        "demo must produce audible output"
    );
    assert!(
        evidence.peak_voices >= 2,
        "shared phrase occurrences must overlap in DSP"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "renders the full Corridors arrangement through production DSP; run with --release --test-threads=1"]
fn corridors_event_scene_imports_edits_navigates_and_plays_complete_song() {
    let root = scenarios::temp_root("corridors-scene");
    let mut scene = super::super::scene::Scene::new();
    let evidence = run(
        &root,
        Box::new(move |runtime, _audio, evidence| {
            evidence.measured = true;
            let done = scene
                .advance(
                    runtime,
                    runtime.last_clock.unwrap(),
                    evidence.projection.as_ref(),
                )
                .unwrap_or_else(|error| {
                    panic!(
                        "{error}\n{}",
                        serde_json::to_string_pretty(&scene.report).unwrap()
                    );
                });
            if done {
                eprintln!("{}", serde_json::to_string_pretty(&scene.report).unwrap());
            }
            done
        }),
        Duration::from_secs(360),
    );
    evidence.check_cleanup();
    assert!(evidence.audible_tracks.iter().all(|audible| *audible));
    eprintln!("Full-song DSP: peak voices {}, audible lanes {:?}, callback allocations {}, deallocations {}, late events {}", evidence.peak_voices, evidence.audible_tracks, evidence.allocations, evidence.deallocations, evidence.scheduled_late_events);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "production Piano Roll editing scene and DSP; run with --test-threads=1"]
fn piano_event_scene_previews_applies_cancels_undoes_and_plays() {
    let root = scenarios::temp_root("piano-scene");
    let mut scene = super::super::scene::Scene::piano();
    let evidence = run_with_instrument(
        &root,
        Box::new(move |runtime, _audio, evidence| {
            evidence.measured = true;
            let done = scene
                .advance(
                    runtime,
                    runtime.last_clock.unwrap(),
                    evidence.projection.as_ref(),
                )
                .unwrap_or_else(|error| panic!("{error}"));
            if done {
                eprintln!("{}", serde_json::to_string_pretty(&scene.report).unwrap());
            }
            done
        }),
        Duration::from_secs(60),
        DEFAULT_INSTRUMENT,
    );
    evidence.check_cleanup();
    assert!(
        evidence.audible_tracks[0],
        "edited note must render audibly"
    );
    std::fs::remove_dir_all(root).unwrap();
}
