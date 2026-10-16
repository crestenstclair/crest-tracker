//! MIDI surface drafts preserve exact endpoint identities and existing conflict validation.
use super::*;
use crate::preferences::{MidiControlAction as A, MidiControlBinding, MidiControlTrigger as T};
const ACTIONS: [A; 15] = [
    A::Up,
    A::Down,
    A::Left,
    A::Right,
    A::Confirm,
    A::Back,
    A::Play,
    A::Stop,
    A::Undo,
    A::Redo,
    A::Menu,
    A::Song,
    A::Instruments,
    A::Increase,
    A::Decrease,
];

pub(super) fn rows(app: &App) -> Vec<WorkspaceRow> {
    let mut rows = commands(&[
        ("midi_discover", "Refresh MIDI destinations"),
        ("synth_settings", "Choose MIDI input / controller mapping"),
    ]);
    if let Some(endpoints) = app.outputs["endpoints"].as_array() {
        rows.extend(endpoints.iter().filter_map(|e| {
            let id = e["id"].as_str()?;
            let connected = app.outputs["connected"]
                .as_array()
                .is_some_and(|v| v.iter().any(|v| v == id));
            Some(row(
                format!("midi_endpoint:{id}"),
                e["name"].as_str().unwrap_or(id),
                if connected {
                    "Connected · select to disconnect"
                } else {
                    "Disconnected · select to connect"
                },
            ))
        }));
    }
    if let Some(endpoints) = app.outputs["endpoints"].as_array() {
        for endpoint in endpoints {
            if let Some(id) = endpoint["id"].as_str() {
                for (kind, label) in [("lower", "Lower"), ("upper", "Upper")] {
                    rows.push(row(
                        format!("mpe_{kind}:{id}"),
                        format!(
                            "{} · {label} MPE members",
                            endpoint["name"].as_str().unwrap_or(id)
                        ),
                        zone_members(app, id, kind).to_string(),
                    ));
                }
            }
        }
    }
    rows.extend(commands(&[("midi_new", "Add control-surface binding")]));
    if let Some(bindings) = app.preferences["midi_controls"].as_array() {
        rows.extend(bindings.iter().enumerate().map(|(i, b)| {
            row(
                format!("midi_binding:{i}"),
                format!(
                    "{} · CH {} · {} {}",
                    b["source"].as_str().unwrap_or(""),
                    b["channel"],
                    b["trigger"].as_str().unwrap_or(""),
                    b["number"]
                ),
                b["action"].as_str().unwrap_or(""),
            )
        }));
    }
    rows.push(row("back", "Back", ""));
    rows
}
pub(super) fn binding_rows(app: &App) -> Vec<WorkspaceRow> {
    let Some(b) = &app.workspace.midi_binding else {
        return commands(&[("back", "Back")]);
    };
    vec![
        row("binding_source", "Source (selected input)", &b.source),
        row("binding_channel", "Channel", b.channel.to_string()),
        row("binding_trigger", "Trigger", format!("{:?}", b.trigger)),
        row("binding_number", "Note / CC number", b.number.to_string()),
        row("binding_action", "Action", format!("{:?}", b.action)),
        row("binding_save", "Save binding", "Conflicts are rejected"),
        row("binding_delete", "Delete binding", ""),
        row("back", "Cancel", ""),
    ]
}
pub(super) fn adjust(app: &mut App, id: &str, delta: i32) {
    if adjust_mpe(app, id, delta) {
        return;
    }
    let Some(b) = &mut app.workspace.midi_binding else {
        return;
    };
    match id {
        "binding_source" => {
            if let Some(id) = app.preferences["active_input"]["id"].as_str() {
                b.source = id.into();
            }
        }
        "binding_channel" => b.channel = (b.channel as i32 + delta).clamp(1, 16) as u8,
        "binding_trigger" => b.trigger = if b.trigger == T::Note { T::Cc } else { T::Note },
        "binding_number" => b.number = (b.number as i32 + delta).clamp(0, 127) as u8,
        "binding_action" => {
            let i = ACTIONS.iter().position(|v| v == &b.action).unwrap_or(0);
            b.action = ACTIONS[(i as i32 + delta).rem_euclid(ACTIONS.len() as i32) as usize];
        }
        _ => {}
    }
}
pub(super) fn activate(app: &mut App, id: &str) -> bool {
    if adjust_mpe(app, id, 1) {
        return true;
    }
    if id == "midi_discover" {
        app.requests.push(Request::MidiOutput {
            action: "discover".into(),
            endpoint: None,
        });
        return true;
    }
    if let Some(endpoint) = id.strip_prefix("midi_endpoint:") {
        let connected = app.outputs["connected"]
            .as_array()
            .is_some_and(|v| v.iter().any(|v| v == endpoint));
        app.requests.push(Request::MidiOutput {
            action: if connected { "disconnect" } else { "connect" }.into(),
            endpoint: Some(endpoint.into()),
        });
        return true;
    }
    if id == "midi_new" || id.starts_with("midi_binding:") {
        let index = id
            .strip_prefix("midi_binding:")
            .and_then(|v| v.parse::<usize>().ok());
        let binding = index
            .and_then(|i| serde_json::from_value(app.preferences["midi_controls"][i].clone()).ok())
            .unwrap_or(MidiControlBinding {
                source: app.preferences["active_input"]["id"]
                    .as_str()
                    .unwrap_or("")
                    .into(),
                channel: 1,
                trigger: T::Cc,
                number: 20,
                action: A::Play,
            });
        app.workspace.midi_binding = Some(binding);
        app.workspace.midi_binding_index = index;
        open(app, Some(Panel::MidiBinding));
        return true;
    }
    if !id.starts_with("binding_") {
        return false;
    }
    if matches!(id, "binding_save" | "binding_delete") {
        let mut bindings: Vec<MidiControlBinding> =
            serde_json::from_value(app.preferences["midi_controls"].clone()).unwrap_or_default();
        if let Some(i) = app.workspace.midi_binding_index {
            if i < bindings.len() {
                bindings.remove(i);
            }
        }
        if id == "binding_save" {
            if let Some(b) = app.workspace.midi_binding.clone() {
                bindings.push(b);
            }
        }
        match crate::preferences::validate_midi_controls(&bindings) {
            Ok(()) => app.requests.push(Request::MidiControls { bindings }),
            Err(e) => app.status = e,
        };
    } else {
        adjust(app, id, 1);
    }
    true
}

fn zone_members(app: &App, endpoint: &str, kind: &str) -> u8 {
    app.preferences["mpe_outputs"]
        .as_array()
        .and_then(|v| v.iter().find(|p| p["endpoint"] == endpoint))
        .and_then(|p| p["zones"].as_array())
        .and_then(|v| v.iter().find(|z| z["kind"] == kind))
        .and_then(|z| z["members"].as_u64())
        .unwrap_or(0) as u8
}
fn adjust_mpe(app: &mut App, id: &str, delta: i32) -> bool {
    let Some((kind, endpoint)) = id.strip_prefix("mpe_").and_then(|s| s.split_once(':')) else {
        return false;
    };
    if !matches!(kind, "lower" | "upper") {
        return false;
    }
    let mut profiles: Vec<crate::midi_output::MpeOutputProfile> =
        serde_json::from_value(app.preferences["mpe_outputs"].clone()).unwrap_or_default();
    let mut zones = profiles
        .iter_mut()
        .find(|p| p.endpoint == endpoint)
        .map(|p| std::mem::take(&mut p.zones))
        .unwrap_or_default();
    let master = if kind == "lower" { 1 } else { 16 };
    zones.retain(|z| z.master().get() != master);
    let count = (zone_members(app, endpoint, kind) as i32 + delta).clamp(0, 15) as u8;
    if count > 0 {
        zones.push(
            if kind == "lower" {
                crest_tracker::mpe::MpeZone::lower(count)
            } else {
                crest_tracker::mpe::MpeZone::upper(count)
            }
            .unwrap(),
        );
    }
    let profile = crate::midi_output::MpeOutputProfile {
        endpoint: endpoint.into(),
        zones: zones.clone(),
    };
    if let Err(error) = if zones.is_empty() {
        Ok(())
    } else {
        profile.validate()
    } {
        app.status = error;
    } else {
        app.requests.push(Request::MpeOutputProfile {
            endpoint: endpoint.into(),
            zones,
        });
    }
    true
}
