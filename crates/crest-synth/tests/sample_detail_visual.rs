use crest_synth::control::{AppEvent, Direction, SemanticAction, StateProjector};
use serde_json::Value;

#[path = "support/sample_detail_fixtures.rs"]
mod sample_detail_fixtures;

#[test]
fn sample_detail_keeps_acknowledged_waveform_through_replacement() {
    let mut ready_pairs = None;
    for (label, state) in sample_detail_fixtures::sample_detail_states() {
        let (_, _, _, shell, _) = StateProjector::new().project_with_shell(&state).unwrap();
        let document: Value = serde_json::to_value(shell.semantic_model()).unwrap();
        let detail = document["surfaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|surface| surface["id"] == "patchDetail")
            .unwrap();
        let waveform = detail["visualizations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|visualization| visualization["data"]["kind"] == "waveform")
            .unwrap();
        let data = &waveform["data"];
        if label == "patch-sample-ready" {
            assert!(!data["pairs"].as_array().unwrap().is_empty());
            ready_pairs = Some(data["pairs"].clone());
        } else if matches!(
            label,
            "patch-sample-loading"
                | "patch-sample-validating"
                | "patch-sample-preparing"
                | "patch-sample-activating"
                | "patch-sample-asset-unavailable"
                | "patch-sample-failed"
        ) {
            assert_eq!(
                data["asset"]["locator"],
                sample_detail_fixtures::ACTIVE_ASSET,
                "{label}"
            );
            assert_eq!(
                Some(&data["pairs"]),
                ready_pairs.as_ref(),
                "{label}: the active waveform remains available"
            );
            let asset = detail["controls"]
                .as_array()
                .unwrap()
                .iter()
                .find(|control| control["kind"] == "asset")
                .unwrap();
            assert_eq!(
                asset["requestedValue"]["value"]["locator"],
                sample_detail_fixtures::REQUESTED_ASSET
            );
        } else if matches!(
            label,
            "patch-sample-unavailable" | "patch-sample-incompatible"
        ) {
            assert!(data["pairs"].as_array().unwrap().is_empty());
            assert!(data["status"]
                .as_str()
                .unwrap()
                .contains("WAVEFORM UNAVAILABLE"));
        }
        assert_eq!(
            detail["controls"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|control| control["focused"] == true)
                .count(),
            1,
            "{label}"
        );
    }
}

#[test]
fn sample_landmarks_follow_scalar_edits_without_replacing_waveform() {
    let states = sample_detail_fixtures::sample_detail_states();
    let waveform = |label| {
        let state = &states.iter().find(|(name, _)| *name == label).unwrap().1;
        let shell = StateProjector::new().project_with_shell(state).unwrap().3;
        let document = serde_json::to_value(shell.semantic_model()).unwrap();
        document["surfaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|surface| surface["id"] == "patchDetail")
            .unwrap()["visualizations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["data"]["kind"] == "waveform")
            .unwrap()["data"]
            .clone()
    };
    let ready = waveform("patch-sample-ready");
    let adjusted = waveform("patch-sample-start-adjusting");
    assert_eq!(ready["pairs"], adjusted["pairs"]);
    assert_eq!(ready["asset"], adjusted["asset"]);
    let start = adjusted["landmarks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|landmark| landmark["role"] == "playbackStart")
        .unwrap();
    assert!(
        start["normalizedPosition"].as_f64().unwrap() > 0.0,
        "Start must follow the current scalar control, not cached preparation landmarks"
    );
}

#[test]
fn sample_detail_preserves_exact_focus_through_utility_and_return() {
    for (label, mut state) in sample_detail_fixtures::sample_detail_states()
        .into_iter()
        .filter(|(label, _)| {
            matches!(
                *label,
                "patch-sample-ready"
                    | "patch-sample-short-descriptor"
                    | "patch-sample-reordered-descriptor"
            )
        })
    {
        let focus = state.interaction().focus_path().clone();
        let before = serde_json::to_value(
            StateProjector::new()
                .project_with_shell(&state)
                .unwrap()
                .3
                .semantic_model(),
        )
        .unwrap();
        state.apply(AppEvent::Navigate(Direction::Right)).unwrap();
        state.apply(AppEvent::Navigate(Direction::Left)).unwrap();
        assert_eq!(state.interaction().focus_path(), &focus, "{label}");
        let after = serde_json::to_value(
            StateProjector::new()
                .project_with_shell(&state)
                .unwrap()
                .3
                .semantic_model(),
        )
        .unwrap();
        assert_eq!(before["returnPath"], after["returnPath"], "{label}");
        state.apply_semantic_action(SemanticAction::Return).unwrap();
        assert_eq!(
            serde_json::to_value(state.interaction().focus_path()).unwrap(),
            before["returnPath"]["origin"],
            "{label}"
        );
    }
}
