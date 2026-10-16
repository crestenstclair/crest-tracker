use super::*;

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
                .advance(runtime, runtime.last_clock.unwrap())
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
