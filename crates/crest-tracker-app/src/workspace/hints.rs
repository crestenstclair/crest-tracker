//! Focus-local guidance. Resolve focus first; never call `project` from here.
use super::*;
use crest_synth::control::{ControllerButton, ControllerRole};
use crest_tracker::composition::Fx;

pub(super) fn project(app: &App, view: &WorkspaceProjection) -> String {
    let focus = view
        .rows
        .iter()
        .chain(&view.tabs)
        .find(|row| row.id == view.focus);
    let mut lines = if !modal_list(app) && app.workspace.panel == Some(Panel::PianoVelocity) {
        velocity(app)
    } else if let Some(focus) = focus {
        list(app, view, focus)
    } else {
        grid(app, view)
    };
    if app.edit_held {
        lines[0] = "EDIT HELD · Release Edit: browse".into();
    }
    if app.workspace.panel.is_none()
        || matches!(app.workspace.panel, Some(Panel::Effects | Panel::SongSlot))
    {
        let detail = match app.workspace.panel {
            Some(Panel::SongSlot) => "close slot picker",
            Some(Panel::Effects) => "FX command list",
            _ if app.view == View::Song => "slot type · Kill / Rest / Chain",
            _ if app.view == View::Chain => "arrangement options",
            _ if app.view == View::Phrase && app.column >= 3 && !app.is_piano_roll() => {
                "FX command list"
            }
            _ => "note actions",
        };
        lines.push(format!(
            "{} / [: {detail}",
            button(
                app.workspace
                    .controller_bindings
                    .button(ControllerRole::Popover)
            )
        ));
    }
    lines.push(pages(app));
    // Native text entry owns keyboard shortcuts while its input has focus.
    if app.workspace.panel == Some(Panel::Text) {
        lines.push("Keyboard: type text · Enter: accept".into());
    } else {
        let bindings = &app.workspace.controller_bindings;
        let play = if app.playing { "stop playback" } else { "play" };
        let transport = format!(
            "Start / {} / Space: {play}",
            button(bindings.button(ControllerRole::Preview))
        );
        lines.push(format!("{transport} (also with Shift) · T: menu / global settings · 1: mixer · 2: instruments · 4: sends"));
    }
    lines.join("\n")
}

fn button(button: ControllerButton) -> &'static str {
    // Use the shared physical position names; face-button lettering differs by pad.
    button.label().split(" (").next().unwrap_or(button.label())
}

fn edit(app: &App) -> String {
    format!(
        "Edit ({}{})",
        button(
            app.workspace
                .controller_bindings
                .button(ControllerRole::Edit)
        ),
        if app.workspace.panel == Some(Panel::Text) {
            ""
        } else {
            " / K"
        }
    )
}

fn directions(app: &App, detail: &str) -> String {
    if app.edit_held {
        format!("EDIT HELD · {detail} · Release Edit: finish adjustment")
    } else {
        detail.into()
    }
}

fn list(app: &App, view: &WorkspaceProjection, focus: &WorkspaceRow) -> Vec<String> {
    let id = focus.id.as_str();
    let tabs = view.tabs.iter().any(|row| row.id == id);
    let paged = "↑/↓: previous/next item · ←/→: page up/down";
    let navigation: String = if modal_list(app) {
        paged.into()
    } else if app.workspace.panel == Some(Panel::PianoStack) {
        "↑/←: previous note · ↓/→: next note".into()
    } else if app.is_piano_roll() && piano::panel(app.workspace.panel) {
        "←/→: column · ↑/↓: row".into()
    } else if app.workspace.panel == Some(Panel::ChordEditor) {
        if id.starts_with("chord_key:") {
            "←/→: interval · ↓: inversion / voicing · ↑: interval octave".into()
        } else if chord_editor::is_detail_control(id) {
            if id == "chord_inversion" {
                "↑: interval keyboard · ↓/→: open voicing · ←: interval octave".into()
            } else {
                "↑: previous control · ↓/→: next control · ←: interval octave".into()
            }
        } else {
            "↑/↓: name / quality / octave · →: inversion / voicing / save".into()
        }
    } else if tabs {
        "←/→: previous/next tab · ↓: first field".into()
    } else if app.workspace.panel == Some(Panel::Settings)
        && view.rows.first().is_some_and(|row| row.id == id)
    {
        "↑: tabs · ↓: next field · ←/→: page up/down".into()
    } else {
        paged.into()
    };
    let input = if app.workspace.panel == Some(Panel::Text) {
        "D-pad / left stick"
    } else {
        "D-pad / left stick / arrows"
    };
    let mut lines = vec![format!("{input}: {navigation} · Hold: scan")];
    let activation = activation(app, focus);
    if let Some(activation) = activation {
        lines.push(format!(
            "{}{}: {activation}",
            edit(app),
            if app.workspace.panel == Some(Panel::Text) {
                ""
            } else {
                " / Enter"
            }
        ));
    }
    if let Some(adjustment) = adjustment(app, id).filter(|_| focus.enabled) {
        lines.push(directions(
            app,
            &format!(
                "Edit+direction / right stick — {}: {adjustment}",
                focus.label
            ),
        ));
    }
    lines
}

fn activation(app: &App, row: &WorkspaceRow) -> Option<String> {
    let id = row.id.as_str();
    if !row.enabled {
        return Some(format!("{} unavailable: {}", row.label, row.value));
    }
    let text = match id {
        "piano_apply" => "commit preview; return to roll".into(),
        "piano_close" => "close; discard uncommitted preview".into(),
        "piano_time_apply" => "use chosen timing".into(),
        "piano_start"
        | "piano_exact_length"
        | "piano_new_velocity"
        | "piano_velocity_value"
        | "piano_release"
        | "piano_pitch" => "enter exact value".into(),
        id if id.starts_with("settings_") => return None,
        "tempo" | "name" | "meter" | "phrase_name" | "key_custom" | "sample_folder"
        | "soundfont_folder" | "chord_name" | "notes_start" | "notes_length" | "notes_grid"
        | "arr_track_name" | "arr_chain_name" | "expr_offset" | "expr_value" | "expr_number"
        | "channel_time" | "channel_value" | "channel_number" => "enter value".into(),
        "text_character" | "text_append" => "append character".into(),
        "back" | "parent" | "chords_cancel" | "fx_cancel" => row.label.clone(),
        "fx_apply" => "apply FX; return to Phrase".into(),
        "fx_kind" => "open command list".into(),
        "slot_chain" => "use Chain; edit chain number in Song".into(),
        "slot_rest" | "slot_kill" => format!("use {}; edit measures in Song", row.label),
        "gen_regenerate" => "generate a new preview".into(),
        "fx_value" if matches!(app.workspace.fx, Some(Fx::ChordRevision { .. })) => {
            "choose chord".into()
        }
        "fx_value" if matches!(app.workspace.fx, Some(Fx::Chord { .. } | Fx::Add { .. })) => {
            "enter intervals".into()
        }
        id if id.starts_with("chord:") => format!("apply {}", row.label),
        id if id.starts_with("chord_key:") => "toggle interval".into(),
        id if id.starts_with("fx_command:") => format!("choose {}", row.label),
        id if id.starts_with("piano_pick:") => "select this note; return to roll".into(),
        _ if adjustment(app, id).is_some() => format!(
            "{}: {}",
            row.label,
            match id {
                "notes_zoom" => "zoom in",
                "notes_stick_value" | "phrase_form" | "key_scale" | "key_root" | "key_spelling"
                | "chord_quality" | "clock_output" | "notes_target" | "gen_target"
                | "binding_action" | "binding_trigger" | "expr_kind" | "expr_lane"
                | "fx_control_kind" | "channel_kind" | "channel_lane" | "channel_target" =>
                    "next choice",
                "pitch_display" | "pitch_entry" | "clock_input" | "gen_repeat" | "arr_mute"
                | "arr_loop" | "chord_open" | "expr_scope" => "toggle",
                "key_inherit" => "inherit Song key",
                "binding_source" => "use selected MIDI input",
                "division" => "halve step length",
                _ => "increase",
            }
        ),
        _ => row.label.clone(),
    };
    Some(text)
}

fn adjustment(app: &App, id: &str) -> Option<String> {
    let panel = app.workspace.panel;
    if app.is_piano_roll() && piano::panel(panel) && id.starts_with("piano_") {
        return match id {
            "piano_pitch" => Some("←/→: −1/+1 semitone · ↑/↓: +12/−12 semitones".into()),
            "piano_velocity_value"
            | "piano_release"
            | "piano_new_velocity"
            | "piano_lane_value"
            | "piano_channel" => Some("←/→: −1/+1 · ↑/↓: +10/−10".into()),
            "piano_start" => Some("←/→: −1/+1 grid step · ↑/↓: +10/−10 steps".into()),
            "piano_target" | "piano_new_target" | "piano_target_choice" => {
                Some("←/↓: previous target · →/↑: next target".into())
            }
            "piano_scope" | "piano_starts" | "piano_lengths" => {
                Some("any direction: toggle".into())
            }
            _ => None,
        };
    }
    if panel == Some(Panel::Text) {
        return (id == "text_character")
            .then(|| "←/↓: previous character · →/↑: next character".into());
    }
    // Modal lists do not edit the underlying grid, even if a row uses a familiar ID.
    if !app.file_dialog.is_null()
        || !app.shared_edit_projection().is_null()
        || !app.resource_repair.is_null()
    {
        return None;
    }
    if panel == Some(Panel::FxCommands) {
        return Some("↑/↓: previous/next command · ←/→: page up/down".into());
    }
    if panel == Some(Panel::Chords) {
        return Some("←/→: previous/next chord".into());
    }
    if panel == Some(Panel::ChordEditor) && id.starts_with("chord_key:") {
        return Some("↑/↓: interval octave +1/−1".into());
    }
    let supported = match panel {
        Some(Panel::Settings) => matches!(
            id,
            "tempo"
                | "default_tempo"
                | "clock_delay"
                | "clock_input"
                | "clock_output"
                | "pitch_display"
                | "pitch_entry"
                | "key_root"
                | "key_scale"
                | "key_spelling"
        ),
        Some(Panel::SongSettings) => matches!(id, "tempo" | "patch_capacity"),
        Some(Panel::PhraseSettings) => matches!(
            id,
            "phrase_steps"
                | "phrase_form"
                | "division"
                | "tuplet_notes"
                | "tuplet_time"
                | "key_root"
                | "key_scale"
                | "key_spelling"
                | "key_inherit"
        ),
        Some(Panel::Generator) | None
            if generator::active(app) || panel == Some(Panel::Generator) =>
        {
            matches!(
                id,
                "phrase_steps"
                    | "phrase_form"
                    | "division"
                    | "gen_pulses"
                    | "gen_rotation"
                    | "gen_pitch"
                    | "gen_velocity"
                    | "gen_gate"
                    | "gen_gate_high"
                    | "gen_target"
                    | "gen_seed"
                    | "gen_density"
                    | "gen_low"
                    | "gen_high"
                    | "gen_velocity_low"
                    | "gen_velocity_high"
                    | "gen_repeat"
                    | "gen_regenerate"
            ) || id.starts_with("gen_note:")
        }
        Some(Panel::Effects) => {
            id == "fx_kind"
                || id == "fx_control_kind"
                || effects::numeric_field(app, id)
                || (id == "fx_second" && matches!(app.workspace.fx, Some(Fx::Voicing { .. })))
        }
        Some(Panel::Notes | Panel::NoteValues | Panel::NoteSelection | Panel::PianoView) => {
            matches!(
                id,
                "notes_pitch"
                    | "notes_start"
                    | "notes_velocity"
                    | "notes_release"
                    | "notes_length"
                    | "notes_target"
                    | "notes_channel"
                    | "notes_grid"
                    | "notes_resize"
                    | "notes_nudge"
                    | "notes_zoom"
                    | "notes_stick_value"
            )
        }
        Some(Panel::ChordEditor) => matches!(
            id,
            "chord_quality" | "chord_octave" | "chord_inversion" | "chord_open" | "chord_spread"
        ),
        Some(Panel::Arrangement) => matches!(
            id,
            "arr_mute" | "arr_loop" | "arr_chain_repeat" | "arr_phrase_repeat"
        ),
        Some(Panel::Midi | Panel::MidiBinding) => {
            id.starts_with("mpe_")
                || matches!(
                    id,
                    "binding_source"
                        | "binding_channel"
                        | "binding_number"
                        | "binding_trigger"
                        | "binding_action"
                )
        }
        Some(Panel::Expression) => matches!(
            id,
            "expr_scope" | "expr_lane" | "expr_kind" | "expr_offset" | "expr_value" | "expr_number"
        ),
        Some(Panel::ChannelExpression) => matches!(
            id,
            "channel_target"
                | "channel_lane"
                | "channel_kind"
                | "channel_time"
                | "channel_value"
                | "channel_number"
        ),
        _ => false,
    };
    if !supported {
        return None;
    }
    let special = match id {
        "fx_kind" => Some("any direction: open command list"),
        "gen_regenerate" => Some("←/↓: previous seed · →/↑: next seed"),
        "notes_grid" => Some("←/↓: halve grid · →/↑: double grid"),
        "notes_zoom" => Some("←/↓: zoom out · →/↑: zoom in"),
        "expr_offset" | "channel_time" => {
            Some("←/→: −1/16 / +1/16 beat · ↑/↓: +10/16 / −10/16 beat")
        }
        "expr_lane" | "channel_lane" => Some("←/↓: previous lane · →/↑: next lane"),
        "channel_target" => Some("←/↓: previous destination · →/↑: next destination · browse only"),
        "notes_start" | "notes_nudge" => Some("←/↓: earlier by grid · →/↑: later by grid"),
        "notes_resize" => Some("←/↓: shorten by grid · →/↑: lengthen by grid"),
        "division" => Some("←/↓: double step length · →/↑: halve step length"),
        "pitch_display" | "pitch_entry" | "clock_input" | "gen_repeat" | "arr_mute"
        | "arr_loop" | "chord_open" | "expr_scope" => Some("any direction: toggle"),
        "binding_source" => Some("any direction: use selected MIDI input"),
        "key_inherit" => Some("any direction: inherit Song key"),
        "phrase_form" | "key_root" | "key_scale" | "key_spelling" | "clock_output"
        | "notes_target" | "gen_target" | "chord_quality" | "binding_action"
        | "binding_trigger" | "expr_kind" | "channel_kind" | "fx_control_kind"
        | "notes_stick_value" => Some("←/↓: previous choice · →/↑: next choice"),
        "fx_second" if matches!(app.workspace.fx, Some(Fx::Voicing { .. })) => {
            Some("any direction: toggle open voicing")
        }
        _ => None,
    };
    if let Some(special) = special {
        return Some(special.into());
    }
    let coarse = field_delta(app, id, Direction::Up, 1);
    let unit = if coarse == 12 {
        " semitones"
    } else if id == "notes_length"
        || matches!(
            (&app.workspace.fx, id),
            (Some(Fx::Cut { .. } | Fx::Delay { .. }), "fx_value")
                | (Some(Fx::Retrigger { .. }), "fx_second")
                | (Some(Fx::Send { .. }), "fx_third")
        )
    {
        " × 1/16 beat"
    } else if matches!(id, "gen_gate" | "gen_gate_high")
        || matches!((&app.workspace.fx, id), (Some(Fx::Gate { .. }), "fx_value"))
    {
        " × 1/16"
    } else {
        ""
    };
    Some(format!("←/→: −1/+1{unit} · ↑/↓: +{coarse}/−{coarse}{unit}"))
}

fn grid(app: &App, view: &WorkspaceProjection) -> Vec<String> {
    if app.is_piano_roll() {
        return piano(app);
    }
    let (navigation, activation, adjustment): (&str, String, String) = if app.is_drum() {
        let occupied = view
            .grid
            .iter()
            .flat_map(|row| &row.cells)
            .any(|cell| cell.id == view.focus && cell.selected);
        (
            "←/→: previous/next step · ↑/↓: drum lane",
            if occupied { "remove hit" } else { "insert hit" }.into(),
            if occupied {
                "velocity — ←/→: −1/+1 · ↑/↓: +10/−10"
            } else {
                ""
            }
            .into(),
        )
    } else {
        match app.view {
            View::Song => (
                "←/→: track · ↑/↓: slot",
                if app.selected_special().is_some() {
                    ""
                } else if app.selected_chain().is_some() {
                    "open chain"
                } else {
                    "create chain and open"
                }
                .into(),
                if app.selected_special().is_some() {
                    "rest measures — ←/→: −1/+1 · ↑/↓: +10/−10"
                } else {
                    "chain number — ←/→: −1/+1 · ↑/↓: +10/−10"
                }
                .into(),
            ),
            View::Chain => (
                "←/→: phrase / repeats · ↑/↓: phrase slot",
                if app.selected_phrase().is_some() {
                    "open phrase"
                } else {
                    "create phrase and open"
                }
                .into(),
                if app.column == 0 {
                    "phrase number — ←/↓: −1 · →/↑: +1"
                } else {
                    "repeats — ←/→: −1/+1 · ↑/↓: +10/−10"
                }
                .into(),
            ),
            View::Phrase => {
                let (activate, adjust) = match app.column {
                    0 => (
                        "insert / raise note",
                        if app.chromatic_entry {
                            "pitch — ←/→: −1/+1 semitone · ↑/↓: +1/−1 octave"
                        } else {
                            "pitch — ←/→: previous/next scale note · ↑/↓: +1/−1 octave"
                        },
                    ),
                    1 => ("next instrument", "instrument — ←/↓: previous · →/↑: next"),
                    2 => (
                        "insert / raise velocity",
                        "velocity — ←/→: −1/+1 · ↑/↓: +10/−10",
                    ),
                    _ => {
                        let fx = effects::selected_event(app)
                            .and_then(|e| e.fx[app.column.saturating_sub(3).min(2)].clone());
                        match fx {
                            Some(Fx::ChordRevision { .. }) => (
                                "next chord",
                                "FX — ←/→: previous/next chord · ↑/↓: +10/−10 chords",
                            ),
                            None | Some(Fx::Offset { .. }) => (
                                "insert / raise offset",
                                "FX offset — ←/→: −1/+1 semitone · ↑/↓: +10/−10",
                            ),
                            _ => ("", "FX — use popover to edit parameters"),
                        }
                    }
                };
                ("←/→: column · ↑/↓: row", activate.into(), adjust.into())
            }
            _ => (
                "←/→: track · ↑/↓: time",
                "open source".into(),
                "←/→: zoom · ↑/↓: time".into(),
            ),
        }
    };
    let mut lines = vec![format!(
        "D-pad / left stick / arrows: {navigation} · Hold: scan"
    )];
    if !activation.is_empty() {
        lines.push(format!("{} / Enter: {activation}", edit(app)));
    }
    if !adjustment.is_empty() {
        lines.push(directions(
            app,
            &format!("Edit+direction / right stick: {adjustment}"),
        ));
    }
    lines
}

fn roll_modifiers(app: &App, include_range: bool) -> Vec<String> {
    let roll: crate::preferences::RollBindings =
        serde_json::from_value(app.preferences["roll_bindings"].clone()).unwrap_or_default();
    let mut modifiers = Vec::new();
    for (button_id, name, held) in [
        (roll.fine, "Fine", app.fine_held),
        (roll.range, "Range", app.range_held),
    ] {
        if (include_range || name == "Fine")
            && app.workspace.controller_bindings.role(button_id).is_none()
        {
            modifiers.push(format!(
                "{}: hold {name}{}",
                button(button_id),
                if held { " (held)" } else { "" }
            ));
        }
    }
    modifiers
}

fn velocity(app: &App) -> Vec<String> {
    let mut lines = vec![
        "FOCUS / VELOCITY · ←/→: previous/next note · ↑/↓: roll".into(),
        format!(
            "{} / Enter: exact value · Edit+←/→: −1/+1 velocity",
            edit(app)
        ),
        "Edit+↑/↓ / right stick ↑/↓: +10/−10 · Right stick ←/→: −1/+1".into(),
    ];
    let modifiers = roll_modifiers(app, false);
    if !modifiers.is_empty() {
        lines.push(format!("{} for ±1", modifiers.join(" · ")));
    }
    lines
}

fn piano(app: &App) -> Vec<String> {
    let occupied = !app.piano_notes_at_cursor().is_empty();
    let movement = if app.range_held {
        "extend selection"
    } else {
        "move cursor"
    };
    let step = if app.fine_held { "1/16 grid" } else { "grid" };
    let value = match app.roll_value {
        crate::app::PianoValue::Pitch => "pitch +1/−1 semitone",
        crate::app::PianoValue::Velocity if app.fine_held => "velocity +1/−1",
        crate::app::PianoValue::Velocity => "velocity +10/−10",
        crate::app::PianoValue::ReleaseVelocity if app.fine_held => "release +1/−1",
        crate::app::PianoValue::ReleaseVelocity => "release +10/−10",
    };
    let modifiers = roll_modifiers(app, true);
    let mut lines = vec![
        format!("D-pad / left stick / arrows: ←/→ time · ↑/↓ pitch · {movement} · Hold: scan"),
        format!(
            "{} / Enter: {} · {}",
            edit(app),
            if occupied {
                "select note"
            } else {
                "insert note"
            },
            modifiers.join(" · ")
        ),
    ];
    if occupied || !app.selected_event_ids().is_empty() {
        lines.push(directions(
            app,
            &format!("Edit+←/→: nudge by {step} · Edit+↑/↓: velocity +10/−10"),
        ));
        lines.push(format!(
            "Right stick ←/→: shorten/lengthen by {step} · ↑/↓: {value}"
        ));
    } else {
        lines.push("Insert/select a note to nudge, resize or change values".into());
    }
    lines
}

fn pages(app: &App) -> String {
    let back = if app.workspace.panel.is_some()
        || !app.file_dialog.is_null()
        || !app.shared_edit_projection().is_null()
        || !app.resource_repair.is_null()
    {
        "back / cancel"
    } else {
        match app.view {
            View::Song | View::Instruments => "menu",
            View::Chain => "Song",
            _ => "Chain",
        }
    };
    let mut parts = vec![format!(
        "Shift ({}): +← {back}",
        button(
            app.workspace
                .controller_bindings
                .button(ControllerRole::Shift)
        )
    )];
    if app.workspace.panel == Some(Panel::Menu) {
        parts.push("+↑ previous page".into());
    } else if app.workspace.panel.is_some() {
        parts.push("+↓ back".into());
    } else if app.file_dialog.is_null()
        && app.shared_edit_projection().is_null()
        && app.resource_repair.is_null()
    {
        match app.view {
            View::Song => {
                parts.push("+→ open/create chain · +↑ Song settings · +↓ main menu".into())
            }
            View::Chain => {
                parts.push("+→ open/create phrase · +↑ Chain settings · +↓ main menu".into())
            }
            View::Phrase => parts.push("+↑ phrase settings · +↓ main menu".into()),
            _ => {}
        }
    }
    parts.push(format!(
        "{}{}: {back}",
        button(
            app.workspace
                .controller_bindings
                .button(ControllerRole::PreviousPatch)
        ),
        if app.workspace.panel == Some(Panel::Text) {
            ""
        } else {
            " / Q"
        }
    ));
    if app.workspace.panel.is_none()
        && app.file_dialog.is_null()
        && app.shared_edit_projection().is_null()
        && app.resource_repair.is_null()
    {
        parts.push(format!(
            "{} / E: clear {}",
            button(
                app.workspace
                    .controller_bindings
                    .button(ControllerRole::NextPatch)
            ),
            if app.is_piano_roll() {
                "selected notes"
            } else {
                "selected value"
            }
        ));
    }
    parts.join(" · ")
}
