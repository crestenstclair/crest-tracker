//! Piano editing drafts and timing pickers. The document changes only on Apply.
use super::*;
use crest_tracker::composition::{Control, DocumentError, Event, PhraseId, Target};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum TimingTarget {
    #[default]
    Step,
    NewLength,
    SelectedLength,
    Quantize,
}
#[derive(Clone)]
pub(super) struct Draft {
    revision: u64,
    phrase: PhraseId,
    events: Vec<Event>,
}

#[derive(Clone)]
pub(super) struct Defaults {
    length: Beat,
    velocity: u8,
    target: Option<Target>,
}
fn defaults(app: &App) -> Defaults {
    app.workspace
        .piano
        .defaults
        .clone()
        .unwrap_or_else(|| Defaults {
            length: app.roll_new_length,
            velocity: app.roll_new_velocity,
            target: app
                .roll_new_target
                .clone()
                .or_else(|| app.default_target().ok()),
        })
}

pub(super) struct PianoEditor {
    pub draft: Option<Draft>,
    pub defaults: Option<Defaults>,
    pub timing: TimingTarget,
    pub triplet: bool,
    pub timing_choice: Beat,
    pub quantize_grid: Beat,
    pub quantize_all: bool,
    pub quantize_starts: bool,
    pub quantize_lengths: bool,
}
impl Default for PianoEditor {
    fn default() -> Self {
        Self {
            draft: None,
            defaults: None,
            timing: TimingTarget::Step,
            triplet: false,
            timing_choice: Beat::new(1, 4).unwrap(),
            quantize_grid: Beat::new(1, 4).unwrap(),
            quantize_all: false,
            quantize_starts: true,
            quantize_lengths: false,
        }
    }
}

pub(super) fn panel(panel: Option<Panel>) -> bool {
    matches!(
        panel,
        Some(
            Panel::Notes
                | Panel::NoteValues
                | Panel::NoteSelection
                | Panel::NoteHistory
                | Panel::PianoView
                | Panel::PianoTiming
                | Panel::PianoTarget
                | Panel::PianoDefaults
                | Panel::PianoQuantize
                | Panel::PianoVelocity
                | Panel::PianoStack
                | Panel::Expression
                | Panel::ChannelExpression
        )
    )
}

pub(super) fn notation(beat: Beat) -> String {
    for denominator in [1, 2, 4, 8, 16, 32, 64] {
        let straight = Beat::new(4, denominator).unwrap();
        if beat == straight {
            return format!("1/{denominator}");
        }
        if beat == straight.checked_scale(2, 3).unwrap() {
            return format!("1/{denominator}T");
        }
    }
    format!("{} beats", generator::beat_label(beat))
}

pub(super) fn pitch_label(app: &App, pitch: u8) -> String {
    app.selected_phrase()
        .and_then(|(_, phrase)| phrase.key.as_ref())
        .unwrap_or(&app.editor.song().key)
        .pitch_name(crest_tracker::harmony::Pitch::new(pitch).expect("validated MIDI pitch"))
}

fn field(id: &str, label: &str, value: impl Into<String>, enabled: bool) -> WorkspaceRow {
    let mut row = row(id, label, value);
    row.enabled = enabled;
    row
}
fn selected(app: &App) -> Vec<Event> {
    let ids = app.selected_event_ids();
    app.selected_phrase().map_or_else(Vec::new, |(_, phrase)| {
        phrase
            .events
            .iter()
            .filter(|event| event.note.is_some() && ids.contains(&event.id))
            .cloned()
            .collect()
    })
}
fn draft_events(app: &App) -> &[Event] {
    app.workspace
        .piano
        .draft
        .as_ref()
        .map_or(&[], |draft| draft.events.as_slice())
}
fn begin_inspector(app: &mut App) {
    let Some((phrase, _)) = app.selected_phrase() else {
        return;
    };
    let events = selected(app);
    if events.is_empty() {
        app.status = "Select notes to edit".into();
        return;
    }
    app.workspace.piano.draft = Some(Draft {
        revision: app.editor.revision(),
        phrase,
        events,
    });
    open(app, Some(Panel::NoteValues));
}
fn begin_quantize(app: &mut App) {
    let Some((phrase, _)) = app.selected_phrase() else {
        return;
    };
    app.workspace.piano.draft = Some(Draft {
        revision: app.editor.revision(),
        phrase,
        events: selected(app),
    });
    app.workspace.piano.quantize_all = false;
    app.workspace.piano.quantize_starts = true;
    app.workspace.piano.quantize_lengths = false;
    open(app, Some(Panel::PianoQuantize));
}

fn quantizing(app: &App) -> bool {
    app.workspace.panel == Some(Panel::PianoQuantize)
        || (app.workspace.panel == Some(Panel::PianoTiming)
            && app.workspace.piano.timing == TimingTarget::Quantize)
}
pub(super) fn preview(app: &App) -> Result<Vec<Event>, DocumentError> {
    let Some(draft) = &app.workspace.piano.draft else {
        return Ok(Vec::new());
    };
    if draft.revision != app.editor.revision() {
        return Err(DocumentError::Invalid(
            "Song changed; cancel and reopen this edit",
        ));
    }
    let phrase = app
        .editor
        .song()
        .phrases
        .get(&draft.phrase)
        .ok_or(DocumentError::Invalid("edited phrase no longer exists"))?;
    if !quantizing(app) {
        return Ok(draft.events.clone());
    }
    let state = &app.workspace.piano;
    let source: Vec<_> = if state.quantize_all {
        phrase
            .events
            .iter()
            .filter(|event| event.note.is_some())
            .collect()
    } else {
        draft.events.iter().collect()
    };
    source
        .into_iter()
        .map(|event| {
            crest_tracker::editing::quantized_event(
                event,
                phrase.length()?,
                state.quantize_grid,
                state.quantize_starts,
                state.quantize_lengths,
            )
        })
        .collect()
}

pub(super) fn title(app: &App) -> String {
    match app.workspace.panel {
        Some(Panel::Notes) => format!(
            "TOOLS / {} NOTE{}",
            selected(app).len(),
            if selected(app).len() == 1 { "" } else { "S" }
        ),
        Some(Panel::NoteValues) => format!(
            "EDIT / {} NOTE{}",
            draft_events(app).len(),
            if draft_events(app).len() == 1 {
                ""
            } else {
                "S"
            }
        ),
        Some(Panel::PianoTiming) => match app.workspace.piano.timing {
            TimingTarget::Step => "STEP GRID",
            TimingTarget::NewLength => "NEW LENGTH",
            TimingTarget::SelectedLength => "SELECTED LENGTH",
            TimingTarget::Quantize => "QUANTIZE GRID",
        }
        .into(),
        Some(Panel::PianoTarget) => "NOTE TARGET".into(),
        Some(Panel::PianoDefaults) => "NEW NOTE DEFAULTS".into(),
        Some(Panel::PianoQuantize) => "QUANTIZE".into(),
        Some(Panel::PianoVelocity) => "VELOCITY LANE".into(),
        Some(Panel::PianoStack) => "OVERLAPPING NOTES".into(),
        Some(Panel::PianoView) => "VIEW".into(),
        Some(Panel::ChannelExpression) => "CHANNEL AUTOMATION".into(),
        _ => "HISTORY / SELECTION".into(),
    }
}
pub(super) fn caption(app: &App) -> String {
    match app.workspace.panel {
        Some(Panel::Notes) => "Popover · Current selection".into(),
        Some(Panel::NoteValues | Panel::PianoQuantize) => match preview(app) {
            Ok(events) => format!(
                "Preview only · {} note{} · Apply commits once{}",
                events.len(),
                if events.len() == 1 { "" } else { "s" },
                if app.workspace.panel == Some(Panel::PianoQuantize) {
                    "\nCyan: result · Amber outline: original\nNearest grid; midpoint goes later."
                } else {
                    ""
                }
            ),
            Err(error) => error.to_string(),
        },
        Some(Panel::PianoTiming) => match app.workspace.piano.timing {
            TimingTarget::Step => "Cursor and new-note placement only. Existing notes and New Length stay unchanged.",
            TimingTarget::NewLength => "Future note lengths only. Existing notes stay unchanged.",
            TimingTarget::SelectedLength => "Selected durations in preview. Starts and New Length stay unchanged. Commit with Apply in Edit Notes.",
            TimingTarget::Quantize => "Quantize preview grid only. Step Grid stays unchanged; notes change on Apply Quantize.",
        }.into(),
        Some(Panel::PianoDefaults) => "Future notes only. Selected notes stay unchanged.".into(),
        Some(Panel::Expression) => "Note-relative beats · Edits apply immediately.
Undo restores edits. Master scope stays separate.".into(),
        Some(Panel::ChannelExpression) => "Independent phrase-time controls. Scope browses destinations.
Edits apply immediately · Undo restores edits.".into(),
        Some(Panel::PianoVelocity) => {
            "Left/right: previous/next note · Edit: adjust velocity".into()
        }
        Some(Panel::PianoStack) => "Notes under the cursor. Choose one to select it and reveal its start.
Cancel keeps the current selection.".into(),
        _ => "Current selection · Edit activates · Shift+Down returns".into(),
    }
}

pub(super) fn tools_rows(app: &App) -> Vec<WorkspaceRow> {
    let has = !selected(app).is_empty();
    let mut rows = vec![
        row("piano_grid", "Step grid", notation(app.roll_grid)),
        row("piano_defaults", "Defaults", "New notes"),
        row("notes_view", "View", "Zoom / follow"),
        field(
            "piano_inspect",
            "Edit notes…",
            "Pitch / timing / target",
            has,
        ),
        row("piano_velocity", "Velocity lane", "1–127"),
        field("notes_copy", "Copy", "", has),
        field("notes_cut", "Cut", "", has),
        row("notes_paste", "Paste", "At cursor"),
        field("notes_duplicate", "Duplicate", "At cursor", has),
        row("piano_quantize", "Quantize…", "Preview"),
        field(
            "piano_off",
            "Note Off",
            "End at cursor",
            off_event(app).is_some(),
        ),
        field("notes_delete", "Delete", "Selection", has),
        field("expression", "Expression / FX", "Owned note data", has),
        row(
            "channel_expression",
            "Channel automation",
            "Independent events",
        ),
        row("notes_all", "Select all", ""),
        field(
            "piano_stack",
            "Overlapping notes…",
            "Choose one",
            app.piano_notes_at_cursor().len() > 1,
        ),
        row("piano_clear_selection", "Clear selection", ""),
        field("undo", "Undo", "", app.editor.can_undo()),
        field("redo", "Redo", "", app.editor.can_redo()),
        row("piano_close", "Cancel", "Piano Roll"),
    ];
    for row in &mut rows {
        row.value.clear();
    }
    rows
}

pub(super) fn rows(app: &App, panel: Panel) -> Vec<WorkspaceRow> {
    match panel {
        Panel::Notes => tools_rows(app),
        Panel::NoteValues => inspector_rows(app),
        Panel::PianoTiming => timing_rows(app),
        Panel::PianoQuantize => quantize_rows(app),
        Panel::PianoTarget => target_rows(app),
        Panel::PianoStack => stack_rows(app),
        Panel::PianoDefaults => vec![
            row(
                "piano_new_length",
                "New length",
                notation(defaults(app).length),
            ),
            row(
                "piano_new_velocity",
                "New velocity",
                format!("{} / 127", defaults(app).velocity),
            ),
            row(
                "piano_new_target",
                "Target",
                defaults(app)
                    .target
                    .as_ref()
                    .map_or("Choose instrument".into(), |target| {
                        app.target_label(target)
                    }),
            ),
            row(
                "piano_defaults_apply",
                "Apply defaults",
                "Future notes only",
            ),
            row("piano_close", "Cancel", "Discard defaults"),
        ],
        Panel::PianoVelocity => vec![
            row("notes_previous", "Previous note", ""),
            row("notes_next", "Next note", ""),
            row(
                "piano_lane_value",
                "Velocity",
                effects::selected_event(app)
                    .and_then(|e| e.note)
                    .map_or("Select a note".into(), |n| format!("{} / 127", n.velocity)),
            ),
            row("piano_close", "Done", "Piano Roll"),
        ],
        _ => Vec::new(),
    }
}

fn stack_rows(app: &App) -> Vec<WorkspaceRow> {
    let Some((_, phrase)) = app.selected_phrase() else {
        return Vec::new();
    };
    let candidates = app.piano_notes_at_cursor();
    let selected = app.selected_event_ids();
    let mut rows: Vec<_> = phrase
        .events
        .iter()
        .filter(|event| candidates.contains(&event.id))
        .filter_map(|event| {
            let note = event.note.as_ref()?;
            let pitch = pitch_label(app, note.pitch);
            let mut item = row(
                format!("piano_pick:{}", event.id.get()),
                format!("{pitch} · {}", app.target_label(&event.target)),
                format!(
                    "Start {} · Length {} · Velocity {}",
                    generator::beat_label(event.at),
                    notation(note.duration),
                    note.velocity
                ),
            );
            item.selected = selected.contains(&event.id);
            Some(item)
        })
        .collect();
    rows.push(row("back", "Cancel", "Keep selection"));
    rows
}

fn pick_note(app: &mut App, id: &str) {
    let Some(event) = id
        .strip_prefix("piano_pick:")
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(|value| crest_tracker::composition::EventId::new(value).ok())
        .filter(|event| app.piano_notes_at_cursor().contains(event))
    else {
        return;
    };
    if let Some(occurrence) = app.occurrence() {
        app.requests.push(Request::SelectEvents {
            revision: app.editor.revision(),
            occurrence,
            events: vec![event],
            toggle: false,
        });
        open(app, None);
    }
}

fn inspector_rows(app: &App) -> Vec<WorkspaceRow> {
    let Some(event) = draft_events(app).first() else {
        return vec![row("piano_close", "Cancel", "No notes selected")];
    };
    let note = event.note.as_ref().expect("draft contains notes");
    vec![
        row("piano_pitch", "Pitch", pitch_label(app, note.pitch)),
        row(
            "piano_start",
            "Start · beats",
            generator::beat_label(event.at),
        ),
        row("piano_length", "Length", notation(note.duration)),
        row(
            "piano_velocity_value",
            "Velocity",
            format!("{} / 127", note.velocity),
        ),
        row(
            "piano_release",
            "Release",
            format!("{} / 127", note.release_velocity),
        ),
        row("piano_target", "Target", app.target_label(&event.target)),
        field("piano_apply", "Apply", "One edit", preview(app).is_ok()),
        row("piano_close", "Cancel", "Restore original"),
        row("piano_previous", "Previous note", "Discard draft"),
        row("piano_next", "Next note", "Discard draft"),
    ]
}
fn quantize_rows(app: &App) -> Vec<WorkspaceRow> {
    let state = &app.workspace.piano;
    let count = preview(app).map_or(0, |events| events.len());
    vec![
        row("piano_quantize_grid", "Grid", notation(state.quantize_grid)),
        row(
            "piano_scope",
            "Scope",
            if state.quantize_all {
                format!("All notes ({count})")
            } else {
                format!("Selected ({count})")
            },
        ),
        row(
            "piano_starts",
            "Starts",
            if state.quantize_starts { "On" } else { "Off" },
        ),
        row(
            "piano_lengths",
            "Lengths",
            if state.quantize_lengths { "On" } else { "Off" },
        ),
        field(
            "piano_apply",
            "Apply quantize",
            "One edit",
            count > 0 && (state.quantize_starts || state.quantize_lengths),
        ),
        row("piano_close", "Cancel", "Restore original"),
    ]
}
fn timing_value(app: &App) -> Beat {
    match app.workspace.piano.timing {
        TimingTarget::Step => app.roll_grid,
        TimingTarget::NewLength => defaults(app).length,
        TimingTarget::SelectedLength => draft_events(app)
            .first()
            .and_then(|e| e.note.as_ref())
            .map_or(app.roll_new_length, |n| n.duration),
        TimingTarget::Quantize => app.workspace.piano.quantize_grid,
    }
}
fn timing_rows(app: &App) -> Vec<WorkspaceRow> {
    let triplet = app.workspace.piano.triplet;
    let mut rows = vec![
        row(
            "piano_straight",
            "Straight",
            if triplet { "" } else { "Selected" },
        ),
        row(
            "piano_triplet",
            "Triplet",
            if triplet { "Selected" } else { "" },
        ),
    ];
    let lengths = matches!(
        app.workspace.piano.timing,
        TimingTarget::NewLength | TimingTarget::SelectedLength
    );
    for denominator in [1, 2, 4, 8, 16, 32, 64]
        .into_iter()
        .filter(|d| lengths || *d >= 4)
    {
        let beat = rhythmic_value(denominator, triplet);
        let mut item = row(
            format!("piano_time:{denominator}"),
            notation(beat),
            format!(
                "{} beat{}",
                generator::beat_label(beat),
                if beat == Beat::ONE { "" } else { "s" }
            ),
        );
        item.selected = beat == app.workspace.piano.timing_choice;
        rows.push(item);
    }
    if lengths {
        rows.push(row(
            "piano_exact_length",
            "Exact beats…",
            generator::beat_label(timing_value(app)),
        ));
    }
    rows.push(row(
        "piano_time_apply",
        "Apply",
        notation(app.workspace.piano.timing_choice),
    ));
    rows.push(row("back", "Cancel", ""));
    rows
}
fn rhythmic_value(denominator: u64, triplet: bool) -> Beat {
    Beat::new(4, denominator)
        .unwrap()
        .checked_scale(if triplet { 2 } else { 1 }, if triplet { 3 } else { 1 })
        .unwrap()
}
fn open_timing(app: &mut App, target: TimingTarget) {
    app.workspace.piano.timing = target;
    app.workspace.piano.timing_choice = timing_value(app);
    app.workspace.piano.triplet = notation(timing_value(app)).ends_with('T');
    open(app, Some(Panel::PianoTiming));
}
fn set_timing(app: &mut App, beat: Beat) -> Result<(), DocumentError> {
    if beat.is_zero() {
        return Err(DocumentError::Invalid("length must be positive"));
    }
    match app.workspace.piano.timing {
        TimingTarget::Step => app.requests.push(Request::PianoControl {
            control: PianoControl::Grid { grid: beat },
        }),
        TimingTarget::NewLength => {
            if let Some(defaults) = &mut app.workspace.piano.defaults {
                defaults.length = beat;
            } else {
                app.requests.push(Request::PianoControl {
                    control: PianoControl::NewLength { length: beat },
                });
            }
        }
        TimingTarget::Quantize => app.workspace.piano.quantize_grid = beat,
        TimingTarget::SelectedLength => update_draft(app, |event| {
            event.note.as_mut().expect("draft note").resize(beat)
        })?,
    }
    Ok(())
}

fn update_draft(
    app: &mut App,
    edit: impl Fn(&mut Event) -> Result<(), DocumentError>,
) -> Result<(), DocumentError> {
    let Some(draft) = &mut app.workspace.piano.draft else {
        return Err(DocumentError::Invalid("open note editor first"));
    };
    let mut candidate = draft.events.clone();
    for event in &mut candidate {
        edit(event)?;
    }
    draft.events = candidate;
    Ok(())
}

pub(super) fn activate(app: &mut App, id: &str) -> bool {
    if !app.is_piano_roll() || !id.starts_with("piano_") {
        return false;
    }
    if activate_timing(app, id) {
        return true;
    }
    if id.starts_with("piano_pick:") {
        pick_note(app, id);
        return true;
    }
    match id {
        "piano_defaults" => {
            app.workspace.piano.defaults = Some(defaults(app));
            open(app, Some(Panel::PianoDefaults));
        }
        "piano_defaults_apply" => apply_defaults(app),
        "piano_target" => open(app, Some(Panel::PianoTarget)),
        "piano_inspect" => begin_inspector(app),
        "piano_scale_guide" => app.requests.push(Request::PianoControl {
            control: PianoControl::ScaleGuide {
                enabled: !app.roll_view.scale_guide,
            },
        }),
        "piano_octave_down" | "piano_octave_up" => app.requests.push(Request::PianoControl {
            control: PianoControl::PitchScroll {
                octaves: if id == "piano_octave_down" { -1 } else { 1 },
            },
        }),
        "piano_quantize" => begin_quantize(app),
        "piano_velocity" => open(app, Some(Panel::PianoVelocity)),
        "piano_stack" => open(app, Some(Panel::PianoStack)),
        "piano_scope" => app.workspace.piano.quantize_all = !app.workspace.piano.quantize_all,
        "piano_starts" => {
            app.workspace.piano.quantize_starts = !app.workspace.piano.quantize_starts
        }
        "piano_lengths" => {
            app.workspace.piano.quantize_lengths = !app.workspace.piano.quantize_lengths
        }
        "piano_close" => {
            app.workspace.piano.draft = None;
            open(app, None);
        }
        "piano_apply" => apply_draft(app),
        "piano_off" => note_off(app),
        "piano_clear_selection" => {
            if let Some(occurrence) = app.occurrence() {
                app.requests.push(Request::SelectEvents {
                    revision: app.editor.revision(),
                    occurrence,
                    events: vec![],
                    toggle: false,
                });
                open(app, None);
            }
        }
        "piano_previous" | "piano_next" => {
            app.workspace.piano.draft = None;
            let delta = if id == "piano_previous" { -1 } else { 1 };
            if let Err(error) = app.apply(Request::PianoControl {
                control: PianoControl::NextNote { delta },
            }) {
                app.status = error.to_string();
            }
            begin_inspector(app);
        }
        "piano_start"
        | "piano_exact_length"
        | "piano_new_velocity"
        | "piano_velocity_value"
        | "piano_release"
        | "piano_pitch" => open_exact(app, id),
        _ => adjust(app, id, 1),
    }
    true
}
fn activate_timing(app: &mut App, id: &str) -> bool {
    match id {
        "piano_grid" => open_timing(app, TimingTarget::Step),
        "piano_new_length" => open_timing(app, TimingTarget::NewLength),
        "piano_length" => open_timing(app, TimingTarget::SelectedLength),
        "piano_quantize_grid" => open_timing(app, TimingTarget::Quantize),
        "piano_straight" => app.workspace.piano.triplet = false,
        "piano_triplet" => app.workspace.piano.triplet = true,
        "piano_time_apply" => match set_timing(app, app.workspace.piano.timing_choice) {
            Ok(()) => back(app),
            Err(error) => app.status = error.to_string(),
        },
        _ if id.starts_with("piano_time:") => {
            if let Ok(d) = id[11..].parse::<u64>() {
                if [1, 2, 4, 8, 16, 32, 64].contains(&d) {
                    app.workspace.piano.timing_choice =
                        rhythmic_value(d, app.workspace.piano.triplet);
                }
            }
        }
        _ => return false,
    }
    true
}
fn apply_defaults(app: &mut App) {
    let defaults = defaults(app);
    app.requests.push(Request::PianoControl {
        control: PianoControl::Defaults {
            length: defaults.length,
            velocity: defaults.velocity,
            target: defaults.target,
        },
    });
    open(app, None);
}
fn target_rows(app: &App) -> Vec<WorkspaceRow> {
    let Some(event) = draft_events(app).first() else {
        return vec![row("back", "Back", "No selected note")];
    };
    vec![
        row(
            "piano_target_choice",
            "Destination",
            app.target_label(&event.target),
        ),
        row(
            "piano_channel",
            "MIDI channel",
            event.target.channel().get().to_string(),
        ),
        row("back", "Done", "Note inspector"),
    ]
}

fn open_exact(app: &mut App, id: &str) {
    let event = draft_events(app).first();
    let note = event.and_then(|e| e.note.as_ref());
    let value = match id {
        "piano_new_velocity" => defaults(app).velocity.to_string(),
        "piano_start" => event.map_or("0".into(), |e| generator::beat_label(e.at)),
        "piano_exact_length" => generator::beat_label(timing_value(app)),
        "piano_pitch" => note.map_or("60".into(), |n| n.pitch.to_string()),
        "piano_release" => note.map_or("64".into(), |n| n.release_velocity.to_string()),
        _ => note.map_or("100".into(), |n| n.velocity.to_string()),
    };
    text_entry(
        app,
        id,
        if matches!(id, "piano_start" | "piano_exact_length") {
            "Exact quarter-note beats (e.g. 1/3)"
        } else {
            "Exact MIDI value"
        },
        &value,
    );
}
fn apply_draft(app: &mut App) {
    let events = match preview(app) {
        Ok(events) => events,
        Err(error) => {
            app.status = error.to_string();
            return;
        }
    };
    let Some(draft) = &app.workspace.piano.draft else {
        return;
    };
    if events.is_empty() {
        app.status = "Select notes or explicitly choose All notes".into();
        return;
    }
    let commands = events
        .into_iter()
        .map(|event| Command::PutEvent {
            phrase: draft.phrase,
            event,
        })
        .collect();
    app.requests.push(Request::Edit {
        revision: draft.revision,
        command: Box::new(Command::Batch { commands }),
    });
    app.workspace.piano.draft = None;
    open(app, None);
}

pub(super) fn adjust(app: &mut App, id: &str, delta: i32) {
    let result = match id {
        "piano_new_velocity" => {
            let mut candidate = defaults(app);
            candidate.velocity = (i32::from(candidate.velocity) + delta).clamp(1, 127) as u8;
            app.workspace.piano.defaults = Some(candidate);
            Ok(())
        }
        "piano_new_target" => {
            let mut candidate = defaults(app);
            if let Some(target) = candidate.target {
                candidate.target = Some(generator::next_target(app, &target, delta));
            }
            app.workspace.piano.defaults = Some(candidate);
            Ok(())
        }
        "piano_lane_value" => {
            app.requests.push(Request::PianoGesture {
                control: PianoControl::Velocity { delta },
                gesture: app.workspace.gesture.max(1),
            });
            Ok(())
        }
        "piano_scope" | "piano_starts" | "piano_lengths" => {
            activate(app, id);
            Ok(())
        }
        "piano_straight" | "piano_triplet" => {
            app.workspace.piano.triplet = id == "piano_triplet";
            Ok(())
        }
        "piano_channel" => update_draft(app, |event| {
            let channel = crest_tracker::composition::Channel::new(
                (i32::from(event.target.channel().get()) + delta).clamp(1, 16) as u8,
            )?;
            match &mut event.target {
                Target::Instrument { channel: value, .. }
                | Target::Channel { channel: value, .. } => *value = channel,
            }
            Ok(())
        }),
        "piano_pitch" | "piano_velocity_value" | "piano_release" => {
            update_draft(app, |event| adjust_value(event, id, delta))
        }
        "piano_start" => {
            let amount = app
                .roll_grid
                .checked_scale(u64::from(delta.unsigned_abs()), 1);
            let length = app.selected_phrase().unwrap().1.length().unwrap();
            update_draft(app, |event| {
                let amount = amount?;
                event.at = if delta < 0 {
                    event.at.checked_sub(amount).unwrap_or(Beat::ZERO)
                } else {
                    event.at.checked_add(amount)?
                };
                if event.at >= length {
                    return Err(DocumentError::Invalid("start must stay inside phrase"));
                }
                Ok(())
            })
        }
        "piano_target" | "piano_target_choice" => {
            let target = draft_events(app)
                .first()
                .map(|e| generator::next_target(app, &e.target, delta));
            update_draft(app, |event| {
                if let Some(target) = &target {
                    event.target = target.clone();
                }
                Ok(())
            })
        }
        _ => return,
    };
    if let Err(error) = result {
        app.status = error.to_string();
    }
}
fn adjust_value(event: &mut Event, id: &str, delta: i32) -> Result<(), DocumentError> {
    let note = event.note.as_mut().expect("draft note");
    match id {
        "piano_pitch" => {
            let pitch = i32::from(note.pitch) + delta;
            if !(0..=127).contains(&pitch) {
                return Err(DocumentError::Invalid("pitch must be 0–127"));
            }
            note.pitch = pitch as u8;
            for point in &mut note.expression {
                if let Control::PolyPressure { pitch, .. } = &mut point.control {
                    *pitch = note.pitch;
                }
            }
        }
        "piano_release" => {
            note.release_velocity = (i32::from(note.release_velocity) + delta).clamp(0, 127) as u8
        }
        _ => note.velocity = (i32::from(note.velocity) + delta).clamp(1, 127) as u8,
    }
    Ok(())
}
pub(super) fn submit(app: &mut App, id: &str, value: &str) -> Result<(), String> {
    if id == "piano_exact_length" {
        let beat = notes::parse_beat(value)?;
        if beat.is_zero() {
            return Err("Length must be positive".into());
        }
        app.workspace.piano.timing_choice = beat;
        return Ok(());
    }
    if id == "piano_start" {
        let at = notes::parse_beat(value)?;
        let first = draft_events(app).first().ok_or("No selected note")?.at;
        let length = app
            .selected_phrase()
            .unwrap()
            .1
            .length()
            .map_err(|e| e.to_string())?;
        return update_draft(app, |event| {
            event.at = if at < first {
                event.at.checked_sub(first.checked_sub(at)?)?
            } else {
                event.at.checked_add(at.checked_sub(first)?)?
            };
            if event.at >= length {
                return Err(DocumentError::Invalid("start must stay inside phrase"));
            }
            Ok(())
        })
        .map_err(|e| e.to_string());
    }
    let value = value
        .parse::<u8>()
        .map_err(|_| "Enter an integer MIDI value".to_owned())?;
    let min = if matches!(id, "piano_release" | "piano_pitch") {
        0
    } else {
        1
    };
    if !(min..=127).contains(&value) {
        return Err(format!("Value must be {min}–127"));
    }
    if id == "piano_new_velocity" {
        let mut candidate = defaults(app);
        candidate.velocity = value;
        app.workspace.piano.defaults = Some(candidate);
        return Ok(());
    }

    let transpose = if id == "piano_pitch" {
        draft_events(app)
            .first()
            .and_then(|event| event.note.as_ref())
            .map(|note| i32::from(value) - i32::from(note.pitch))
    } else {
        None
    };
    update_draft(app, |event| {
        let note = event.note.as_ref().expect("draft note");
        let old = match id {
            "piano_pitch" => note.pitch,
            "piano_release" => note.release_velocity,
            _ => note.velocity,
        };
        adjust_value(
            event,
            id,
            transpose.unwrap_or(i32::from(value) - i32::from(old)),
        )
    })
    .map_err(|e| e.to_string())
}

fn off_event(app: &App) -> Option<Event> {
    let (_, phrase) = app.selected_phrase()?;
    let (at, pitch) = app.piano_cursor();
    let ids = app.selected_event_ids();
    phrase
        .events
        .iter()
        .filter(|event| {
            event.note.as_ref().is_some_and(|note| {
                (ids.contains(&event.id) || (ids.is_empty() && note.pitch == pitch))
                    && event.at < at
                    && event
                        .at
                        .checked_add(note.duration)
                        .is_ok_and(|end| end > at)
            })
        })
        .max_by_key(|event| event.at)
        .cloned()
}
fn note_off(app: &mut App) {
    let Some(mut event) = off_event(app) else {
        app.status = "No sounding note at cursor".into();
        return;
    };
    let duration = app.piano_cursor().0.checked_sub(event.at).unwrap();
    if let Err(error) = event.note.as_mut().unwrap().resize(duration) {
        app.status = error.to_string();
        return;
    }
    let phrase = app.selected_phrase().unwrap().0;
    app.requests.push(Request::Edit {
        revision: app.editor.revision(),
        command: Box::new(Command::PutEvent { phrase, event }),
    });
    open(app, None);
}

/// Field widths are shared by controller navigation and the renderer.
pub(super) fn layout(rows: &mut [WorkspaceRow], panel: Panel) {
    let count = rows.len();
    for (index, row) in rows.iter_mut().enumerate() {
        row.span = if (panel == Panel::Notes && index < 3)
            || (panel == Panel::PianoTiming
                && (row.id.starts_with("piano_time:") || row.id == "piano_exact_length"))
        {
            2
        } else if (panel == Panel::Notes && index + 1 == count)
            || row.id == "piano_new_target"
            || row.id.starts_with("expr_select:")
            || row.id.starts_with("channel_select:")
            || row.id == "channel_target"
            || row.id == "channel_lane"
            || panel == Panel::PianoStack
            || (panel == Panel::ChannelExpression
                && matches!(row.id.as_str(), "expression" | "back"))
        {
            6
        } else {
            3
        };
    }
}

pub(super) fn navigate(app: &mut App, direction: Direction, view: &WorkspaceProjection) {
    let index = view
        .rows
        .iter()
        .position(|row| row.id == view.focus)
        .unwrap_or(0);
    let next = if matches!(direction, Direction::Left | Direction::Right) {
        index
            .saturating_add_signed(if direction == Direction::Left { -1 } else { 1 })
            .min(view.rows.len().saturating_sub(1))
    } else {
        let positions = field_positions(&view.rows, view.panel_columns);
        let Some(&(line, center)) = positions.get(index) else {
            return;
        };
        let target_line = if direction == Direction::Up {
            line.saturating_sub(1)
        } else {
            line + 1
        };
        positions
            .iter()
            .enumerate()
            .filter(|(_, (line, _))| *line == target_line)
            .min_by_key(|(_, (_, candidate))| candidate.abs_diff(center))
            .map_or(index, |(index, _)| index)
    };
    if let Some(row) = view.rows.get(next) {
        app.workspace.focus = Some(row.id.clone());
    }
}
fn field_positions(rows: &[WorkspaceRow], columns: usize) -> Vec<(usize, usize)> {
    let mut line = 0;
    let mut column = 0;
    rows.iter()
        .map(|row| {
            if column + row.span > columns {
                line += 1;
                column = 0;
            }
            let position = (line, column * 2 + row.span);
            column += row.span;
            position
        })
        .collect()
}

pub(super) fn summary(app: &App) -> String {
    let events = if app.workspace.piano.draft.is_some() {
        preview(app).unwrap_or_else(|_| selected(app))
    } else {
        selected(app)
    };
    let Some(event) = events.first() else {
        return format!(
            "NO NOTES SELECTED\nCursor {} beat{} · {}\nEdit: insert / select · West / [: tools",
            generator::beat_label(app.piano_cursor().0),
            if app.piano_cursor().0 == Beat::ONE {
                ""
            } else {
                "s"
            },
            pitch_label(app, app.piano_cursor().1)
        );
    };
    let note = event.note.as_ref().unwrap();
    format!(
        "{} NOTE{} SELECTED\nStart {} beat{} · Length {}\nVelocity {} / 127 · {}",
        events.len(),
        if events.len() == 1 { "" } else { "S" },
        generator::beat_label(event.at),
        if event.at == Beat::ONE { "" } else { "s" },
        notation(note.duration),
        note.velocity,
        app.target_label(&event.target)
    )
}
