//! The existing semantic workspace presents the independently owned clip bank.
use super::*;

pub(super) fn rows(app: &App) -> Vec<WorkspaceRow> {
    let mut rows = commands(&[("performance_example", "Open expressive example")]);
    if let Some(slots) = app.performance["session"]["slots"].as_array() {
        rows.extend(slots.iter().enumerate().map(|(slot, state)| {
            let playing = app.performance["playing_slot"].as_u64() == Some(slot as u64);
            let mut clip = row(
                format!("performance_audition:{slot}"),
                format!("Audition clip {}", slot + 1),
                if playing {
                    "LOOPING".into()
                } else {
                    state.as_str().unwrap_or("unavailable").to_uppercase()
                },
            );
            clip.playing = playing;
            clip
        }));
    }
    rows.push(row(
        "performance_status",
        "Playback",
        if app.performance["auditioning"].as_bool() == Some(true) {
            "LOOPING"
        } else {
            "STOPPED"
        },
    ));
    rows.extend(commands(&[("stop", "Stop"), ("back", "Back")]));
    rows
}

pub(super) fn activate(app: &mut App, id: &str) -> bool {
    if app.workspace.panel != Some(Panel::Performance) {
        return false;
    }
    let request = if id == "performance_example" {
        Request::File {
            action: FileAction::PerformanceExample,
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn performance_navigation_preserves_back_focus_and_uses_semantic_transport() {
        let mut app = App::new();
        super::super::commands::activate(&mut app, "menu");
        super::super::commands::activate(&mut app, "performance");
        app.performance =
            serde_json::json!({"session":{"slots":["recorded","empty"]},"auditioning":false});
        assert!(rows(&app).iter().any(|row| row.value == "RECORDED"));
        app.performance["playing_slot"] = 0.into();
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
