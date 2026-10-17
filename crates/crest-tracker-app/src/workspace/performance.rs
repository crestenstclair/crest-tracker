//! The existing semantic workspace presents the independently owned clip bank.
use super::*;

pub(super) fn rows(app: &App) -> Vec<WorkspaceRow> {
    let mut rows = commands(&[
        ("performance_example", "Open expressive example"),
        (
            "performance_sample_example",
            "Open expressive Sample example",
        ),
    ]);
    rows.extend(commands(&[(
        "performance_harmony",
        "Harmony · chords and strumplate",
    )]));
    rows.extend(surface_rows(app));
    rows.push(row(
        "performance_launch_grid",
        "Launch grid · press to cycle",
        match app.performance["launch_grid"].as_str() {
            Some("beat") => "1 BEAT · AUDIO CLOCK",
            Some("four_beats") => "4 BEATS · AUDIO CLOCK",
            _ => "OFF · IMMEDIATE",
        },
    ));
    rows.extend(commands(&[
        ("performance_input", "Enable LinnStrument MPE input"),
        ("performance_finish", "Finish take and stop input"),
        ("performance_cancel", "Discard take and stop input"),
    ]));
    rows.push(row(
        "performance_input_status",
        "LinnStrument input",
        if app.performance["input_fault"].is_string() {
            "INTERRUPTED"
        } else if app.performance["armed_slot"].is_number() {
            "ARMED · First note starts capture"
        } else if app.performance["input"].as_bool() == Some(true) {
            "LIVE · MPE · 50 ms monitor lead"
        } else {
            "OFF"
        },
    ));
    if let Some(slots) = app.performance["session"]["slots"].as_array() {
        rows.extend(slots.iter().enumerate().map(|(slot, state)| {
            let playback = clip_playback_state(app, slot);
            let queued = playback == Some("queued");
            let playing = playback == Some("playing");
            let mut clip = row(
                format!("performance_audition:{slot}"),
                format!(
                    "{} clip {}",
                    if queued || playing { "Stop" } else { "Launch" },
                    slot + 1
                ),
                if queued {
                    "QUEUED".into()
                } else if playing {
                    "LOOPING".into()
                } else {
                    state.as_str().unwrap_or("unavailable").to_uppercase()
                },
            );
            clip.playing = playing;
            clip
        }));
        rows.extend(
            slots
                .iter()
                .enumerate()
                .filter(|(_, state)| *state == "recorded")
                .map(|(slot, _)| {
                    row(
                        format!("performance_edit:{slot}"),
                        format!("Edit clip {}", slot + 1),
                        "PIANO ROLL",
                    )
                }),
        );
        rows.extend(slots.iter().enumerate().map(|(slot, _)| {
            row(
                format!("performance_recent:{slot}"),
                format!("Capture last 4 beats to clip {}", slot + 1),
                "LIVE INPUT · UNDOABLE",
            )
        }));
        rows.extend(commands(&[(
            "performance_undo_clip",
            "Undo last clip or chord bank edit",
        )]));
        rows.extend(slots.iter().enumerate().map(|(slot, state)| {
            row(
                format!("performance_record:{slot}"),
                format!("Record clip {}", slot + 1),
                if app.performance["armed_slot"].as_u64() == Some(slot as u64) {
                    "ARMED".into()
                } else {
                    state.as_str().unwrap_or("unavailable").to_uppercase()
                },
            )
        }));
    }
    rows.push(row("performance_status", "Playback", playback_status(app)));
    rows.extend(commands(&[("stop", "Stop"), ("back", "Back")]));
    rows
}

fn clip_playback_state(app: &App, slot: usize) -> Option<&str> {
    app.performance["playing_slots"]
        .as_array()?
        .iter()
        .find(|entry| entry[0].as_u64() == Some(slot as u64))?[1]
        .as_str()
}

fn playback_status(app: &App) -> &'static str {
    let Some(players) = app.performance["playing_slots"].as_array() else {
        return "STOPPED";
    };
    if players.iter().any(|entry| entry[1] == "playing") {
        "LOOPING"
    } else if players.iter().any(|entry| entry[1] == "queued") {
        "QUEUED"
    } else {
        "STOPPED"
    }
}

fn surface_rows(app: &App) -> Vec<WorkspaceRow> {
    let mut rows = commands(&[
        ("midi_discover", "Refresh MIDI feedback destinations"),
        ("synth_settings", "Choose LinnStrument MIDI input"),
        ("performance_play", "Return LinnStrument to Play"),
    ]);
    rows.push(row(
        "performance_surface_status",
        "LinnStrument surface",
        app.performance["surface"]["status"]
            .as_str()
            .unwrap_or("PLAY"),
    ));
    if let Some(endpoints) = app.outputs["endpoints"].as_array() {
        for endpoint in endpoints {
            rows.extend(surface_endpoint_rows(app, endpoint));
        }
    }
    rows
}
fn surface_endpoint_rows(app: &App, endpoint: &serde_json::Value) -> Vec<WorkspaceRow> {
    let Some(id) = endpoint["id"].as_str() else {
        return vec![];
    };
    let name = endpoint["name"].as_str().unwrap_or(id);
    let connected = app.outputs["connected"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item == id));
    if !connected {
        return vec![row(
            format!("performance_connect:{id}"),
            format!("Connect feedback · {name}"),
            "DISCONNECTED",
        )];
    }
    [200, 128]
        .into_iter()
        .map(|pads| {
            row(
                format!("performance_clips:{pads}:{id}"),
                format!("Clips mode · {pads} pads · {name}"),
                if app.performance["surface"]["endpoint"] == id
                    && app.performance["surface"]["pads"].as_u64() == Some(pads)
                {
                    "SELECTED"
                } else {
                    "Select matching LinnStrument feedback port"
                },
            )
        })
        .collect()
}

pub(super) fn activate(app: &mut App, id: &str) -> bool {
    if app.workspace.panel != Some(Panel::Performance) {
        return false;
    }
    let request = if id == "performance_launch_grid" {
        use crate::app::PerformanceLaunchGrid;
        Request::PerformanceLaunchGrid {
            grid: match app.performance["launch_grid"].as_str() {
                Some("beat") => PerformanceLaunchGrid::FourBeats,
                Some("four_beats") => PerformanceLaunchGrid::Immediate,
                _ => PerformanceLaunchGrid::Beat,
            },
        }
    } else if let Some(slot) = id
        .strip_prefix("performance_edit:")
        .and_then(|s| s.parse::<usize>().ok())
    {
        Request::PerformanceEdit { slot }
    } else if id == "performance_play" {
        Request::PerformancePlay
    } else if let Some(endpoint) = id.strip_prefix("performance_connect:") {
        Request::MidiOutput {
            action: "connect".into(),
            endpoint: Some(endpoint.into()),
        }
    } else if let Some((pads, endpoint)) = id
        .strip_prefix("performance_clips:")
        .and_then(|s| s.split_once(':'))
    {
        let Ok(pads) = pads.parse::<u16>() else {
            return true;
        };
        Request::PerformanceClips {
            endpoint: endpoint.into(),
            pads,
        }
    } else if id == "performance_input" {
        Request::PerformanceInput
    } else if id == "performance_undo_clip" {
        Request::PerformanceUndoClipChange
    } else if let Some(slot) = id
        .strip_prefix("performance_recent:")
        .and_then(|s| s.parse::<usize>().ok())
    {
        if app.performance["session"]["slots"]
            .as_array()
            .is_none_or(|slots| slot >= slots.len())
        {
            return true;
        }
        Request::PerformanceCaptureRecent { slot }
    } else if id == "performance_finish" {
        Request::PerformanceFinish
    } else if id == "performance_cancel" {
        Request::PerformanceCancel
    } else if let Some(slot) = id
        .strip_prefix("performance_record:")
        .and_then(|s| s.parse::<usize>().ok())
    {
        if app.performance["session"]["slots"]
            .as_array()
            .is_none_or(|slots| slot >= slots.len())
        {
            return true;
        }
        Request::PerformanceRecord { slot }
    } else if id == "performance_example" {
        Request::File {
            action: FileAction::PerformanceExample,
        }
    } else if id == "performance_sample_example" {
        Request::File {
            action: FileAction::PerformanceSampleExample,
        }
    } else if let Some(slot) = id
        .strip_prefix("performance_audition:")
        .and_then(|s| s.parse::<usize>().ok())
    {
        if app.performance["session"]["slots"]
            .as_array()
            .is_none_or(|slots| slot >= slots.len())
        {
            return true;
        }
        Request::PerformanceAudition { slot }
    } else {
        return false;
    };
    app.requests.push(request);
    true
}

pub(super) fn transport(app: &App) -> Request {
    if app.playing {
        Request::Stop
    } else {
        Request::PerformanceAudition {
            slot: app
                .performance_editor
                .as_ref()
                .filter(|_| app.workspace.panel == Some(Panel::PerformanceEdit))
                .map_or(0, |editor| editor.slot),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retrospective_actions_are_semantic_and_reject_unknown_slots() {
        let mut app = App::new();
        app.workspace.panel = Some(Panel::Performance);
        app.performance = serde_json::json!({"session":{"slots":["recorded","empty"]}});
        assert!(rows(&app)
            .iter()
            .any(|row| row.id == "performance_recent:1" && row.label.contains("last 4 beats")));
        assert!(activate(&mut app, "performance_recent:1"));
        assert!(matches!(
            app.requests.pop(),
            Some(Request::PerformanceCaptureRecent { slot: 1 })
        ));
        assert!(activate(&mut app, "performance_recent:99"));
        assert!(app.requests.is_empty());
        assert!(activate(&mut app, "performance_undo_clip"));
        assert!(matches!(
            app.requests.pop(),
            Some(Request::PerformanceUndoClipChange)
        ));
    }
    #[test]
    fn launch_grid_cycles_semantically_and_queued_feedback_is_not_looping() {
        use crate::app::PerformanceLaunchGrid;
        let mut app = App::new();
        app.workspace.panel = Some(Panel::Performance);
        for (shown, next) in [
            ("immediate", PerformanceLaunchGrid::Beat),
            ("beat", PerformanceLaunchGrid::FourBeats),
            ("four_beats", PerformanceLaunchGrid::Immediate),
        ] {
            app.performance["launch_grid"] = shown.into();
            assert!(activate(&mut app, "performance_launch_grid"));
            assert!(
                matches!(app.requests.last(), Some(Request::PerformanceLaunchGrid { grid }) if *grid == next)
            );
        }
        app.performance = serde_json::json!({"session":{"slots":["recorded"]},"playing_slots":[[0,"queued"]],"auditioning":true});
        let projected = rows(&app);
        let clip = projected
            .iter()
            .find(|r| r.id == "performance_audition:0")
            .unwrap();
        assert_eq!(clip.value, "QUEUED");
        assert!(!clip.playing);
        assert_eq!(
            projected
                .iter()
                .find(|r| r.id == "performance_status")
                .unwrap()
                .value,
            "QUEUED"
        );
        assert!(serde_json::from_str::<Request>(
            r#"{"kind":"performance_launch_grid","grid":"unsupported"}"#
        )
        .is_err());
    }
    #[test]
    fn feedback_selection_preserves_opaque_endpoint_and_geometry() {
        let mut app = App::new();
        super::super::commands::activate(&mut app, "menu");
        super::super::commands::activate(&mut app, "performance");
        app.outputs = serde_json::json!({"endpoints":[{"id":"opaque:feedback:7", "name":"LinnStrument feedback"}], "connected":[]});
        assert!(rows(&app)
            .iter()
            .any(|r| r.id == "performance_connect:opaque:feedback:7"));
        assert!(activate(&mut app, "performance_connect:opaque:feedback:7"));
        assert!(
            matches!(app.requests.last(), Some(Request::MidiOutput { action, endpoint: Some(endpoint) }) if action == "connect" && endpoint == "opaque:feedback:7")
        );
        app.outputs["connected"] = serde_json::json!(["opaque:feedback:7"]);
        assert!(rows(&app)
            .iter()
            .any(|r| r.id == "performance_clips:128:opaque:feedback:7"));
        assert!(activate(
            &mut app,
            "performance_clips:128:opaque:feedback:7"
        ));
        assert!(
            matches!(app.requests.last(), Some(Request::PerformanceClips { endpoint, pads: 128 }) if endpoint == "opaque:feedback:7")
        );
        assert!(activate(&mut app, "performance_play"));
        assert!(matches!(
            app.requests.last(),
            Some(Request::PerformancePlay)
        ));
    }
    #[test]
    fn independent_clips_show_distinct_states_and_keep_stop_identity() {
        let mut app = App::new();
        app.performance = serde_json::json!({
            "session":{"slots":["recorded","recorded","recorded"]},
            "playing_slots":[[0,"playing"],[1,"queued"]]
        });
        let projected = rows(&app);
        for (slot, label, state, playing) in [
            (0, "Stop clip 1", "LOOPING", true),
            (1, "Stop clip 2", "QUEUED", false),
            (2, "Launch clip 3", "RECORDED", false),
        ] {
            let clip = projected
                .iter()
                .find(|r| r.id == format!("performance_audition:{slot}"))
                .unwrap();
            assert_eq!(clip.label, label);
            assert_eq!(clip.value, state);
            assert_eq!(clip.playing, playing);
        }
        assert_eq!(playback_status(&app), "LOOPING");
        app.performance["playing_slots"] = serde_json::json!([[1, "queued"]]);
        assert_eq!(playback_status(&app), "QUEUED");
        assert_eq!(clip_playback_state(&app, 0), None);
    }

    #[test]
    fn performance_navigation_preserves_back_focus_and_uses_semantic_transport() {
        let mut app = App::new();
        super::super::commands::activate(&mut app, "menu");
        super::super::commands::activate(&mut app, "performance");
        app.performance =
            serde_json::json!({"session":{"slots":["recorded","empty"]},"auditioning":false});
        assert!(rows(&app).iter().any(|row| row.value == "RECORDED"));
        app.performance["playing_slots"] = serde_json::json!([[0, "playing"]]);
        assert!(rows(&app)
            .iter()
            .any(|row| row.id == "performance_audition:0"
                && row.playing
                && row.value == "LOOPING"));
        assert!(activate(&mut app, "performance_audition:0"));
        assert!(matches!(
            app.requests.last(),
            Some(Request::PerformanceAudition { slot: 0 })
        ));
        let count = app.requests.len();
        assert!(activate(&mut app, "performance_audition:100"));
        assert_eq!(app.requests.len(), count);
        app.playing = true;
        action(&mut app, SemanticAction::PreviewStart);
        assert!(matches!(app.requests.last(), Some(Request::Stop)));
        super::super::commands::activate(&mut app, "back");
        assert_eq!(app.workspace.panel, Some(Panel::Menu));
        assert!(!activate(&mut app, "performance_example"));
    }
}
