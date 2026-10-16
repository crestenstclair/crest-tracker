//! Measured proof that the control and composition families exist, that the
//! shipped webview shell is made of them, and that nothing outside the
//! visual module — and nothing in the webview transport adapter — decides
//! how anything looks.
//!
//! Realizes `asset.ComponentCompositionAcceptanceTests` and the declared
//! project validation `validation.component_composition`, which asserts exit
//! code 0 and the exact marker [`ACCEPTANCE_MARKER`] in stdout.
//!
//! Retargeted by mission webview-shell-cutover-01KZAC7Q WP05 (T020): the
//! render-path drive is now the webview document. The proofs kept, and what
//! drives each:
//!
//! - **Selection totality and control reachability** — declaration-level,
//!   unchanged: `control_for` is total over kind × role with the three
//!   declared un-askable pairs, exhaustive rather than defaulted, and every
//!   declared control reachable.
//! - **State applicability** — the declarations are unchanged; the render
//!   half now derives each projected control's state from the serialized
//!   document through the page's own transcribed derivation
//!   ([`page_row_state`]) and proves every production control resolves to
//!   exactly one declared treatment.
//! - **Region from declared composition** — the five declared shell regions
//!   are the five semantic band containers of the committed page, in
//!   canonical order; every region binding a declared composition carries
//!   maps onto that set; and the forwarded `ShellFrameObservation`s (WP02's
//!   seam over the production `ProjectionChannel`) report exactly the
//!   declared regions at the authored extents for both contexts at both
//!   viewports.
//! - **No invented value** — the page's designed Utility entries are parsed
//!   out of the committed render script and compared against the authored
//!   table, driver for driver; the production document carries exactly the
//!   two driven rows, so the three undriven designs must take the marked
//!   path; and every projected value renders through the page's transcribed
//!   value contract — an unknown kind is an explicit `?` marker, never a
//!   fabricated number.
//! - **Ownership boundary** — the visual-module source scan is unchanged;
//!   the page-side twin proves the render script owns no clock, no
//!   randomness, no storage, and no key handler, and the transport renders
//!   deterministically (two pushes of one projection are byte-identical).
//! - **"The render adapter holds no paint decision"** now asserts against
//!   [`TauriWebviewWindow`]'s composition sources (transport-only): no paint
//!   API name, no visual-decision family, and no declared band extent may
//!   appear in the webview shell's production code —
//!   [`the_transport_guard_reports_a_planted_decision`] proves the guard
//!   fails when one reappears.
//!
//! The DOM-level twin — that the page's painted anatomy matches these
//! documents — is `tests/webview_projection_shell.rs` (T024), gated on a
//! live window because a DOM needs one.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crest_synth::adapter::production_instruments::{
    production_capability_registry, production_soundfont_capability,
};
use crest_synth::control::app_event::AppEvent;
use crest_synth::control::app_loop::AppLoop;
use crest_synth::control::app_state::AppState;
use crest_synth::control::state_projector::StateProjector;
use crest_synth::control::{
    GraphicalShellProjection, SemanticAction, SemanticControlKind, SemanticControlViewModel,
    TopLevelContext,
};
use crest_synth::kernel::midi_channel::MidiChannel;
use crest_synth::kernel::patch_id::PatchId;
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::audio_boundary::{BoundaryFull, ControlAudioBoundary};
use crest_synth::real_time::audio_command::AudioCommand;
use crest_synth::real_time::parameter_snapshot::ParameterSnapshot;
use crest_synth::shell::component_state::{
    ComponentState, NonColorSignal, ALL_COMPONENT_STATES, COMPONENT_STATE_COUNT,
};
use crest_synth::shell::component_vocabulary::{
    control_for, ComponentControl, ControlSelection, PresentationRole, ShellComposition,
    ShellRegion, ALL_COMPONENT_CONTROLS, ALL_PRESENTATION_ROLES, ALL_SEMANTIC_CONTROL_KINDS,
    ALL_SHELL_COMPOSITIONS, COMPONENT_CONTROL_COUNT, HINT_SEPARATOR, PRESENTATION_ROLE_COUNT,
    SEMANTIC_CONTROL_KIND_COUNT, SHELL_COMPOSITION_COUNT, UNAVAILABLE_MARK,
};
use crest_synth::shell::density::{
    RepresentativeViewport, ResponsiveLayoutMode, ResponsiveShellContract, ALL_LAYOUT_MODES,
};
use crest_synth::shell::webview::projection_channel::{
    ForwardedAck, ProjectionChannel, ProjectionPush,
};
use crest_synth::shell::webview::TauriWebviewWindow;
use crest_synth::shell::{ShellFrameObservation, ShellRegionId};
use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
use crest_synth::synth::Patch;
use crest_synth::testing::automatic_midi_test::create_soundfont_config;
use serde_json::{json, Value};

/// The exact string `validation.component_composition` asserts on stdout.
///
/// Printed by [`component_composition_acceptance`] and nowhere else, strictly
/// after every declared check has run and passed.
const ACCEPTANCE_MARKER: &str = "CREST_ACCEPTANCE component_composition passed";

/// Representative witness inputs; neither is production geometry authority.
const REFERENCE_VIEWPORTS: [([f32; 2], ResponsiveLayoutMode); 3] = [
    ([1_920.0, 1_080.0], ResponsiveLayoutMode::Wide),
    ([1_280.0, 800.0], ResponsiveLayoutMode::Standard),
    ([900.0, 800.0], ResponsiveLayoutMode::Compact),
];

/// The pairs the control family declares un-askable, and the only ones.
///
/// Transcribed from `DESIGN.md` — a mixer track column carries a
/// level, a pan, and the two track toggles, so it never carries a choice, an
/// asset, or a surface summary. Pinned here rather than read back from the
/// selector, so that switching an askable pair off fails rather than
/// agreeing with itself.
const NOT_ASKABLE_PAIRS: [(SemanticControlKind, PresentationRole); 15] = [
    (SemanticControlKind::Choice, PresentationRole::VerticalStrip),
    (SemanticControlKind::Asset, PresentationRole::VerticalStrip),
    (
        SemanticControlKind::Surface,
        PresentationRole::VerticalStrip,
    ),
    (
        SemanticControlKind::BrowserParent,
        PresentationRole::ListedRow,
    ),
    (
        SemanticControlKind::BrowserParent,
        PresentationRole::VerticalStrip,
    ),
    (
        SemanticControlKind::BrowserParent,
        PresentationRole::PanelEntry,
    ),
    (
        SemanticControlKind::BrowserFolder,
        PresentationRole::ListedRow,
    ),
    (
        SemanticControlKind::BrowserFolder,
        PresentationRole::VerticalStrip,
    ),
    (
        SemanticControlKind::BrowserFolder,
        PresentationRole::PanelEntry,
    ),
    (
        SemanticControlKind::BrowserFile,
        PresentationRole::ListedRow,
    ),
    (
        SemanticControlKind::BrowserFile,
        PresentationRole::VerticalStrip,
    ),
    (
        SemanticControlKind::BrowserFile,
        PresentationRole::PanelEntry,
    ),
    (
        SemanticControlKind::BrowserCancel,
        PresentationRole::ListedRow,
    ),
    (
        SemanticControlKind::BrowserCancel,
        PresentationRole::VerticalStrip,
    ),
    (
        SemanticControlKind::BrowserCancel,
        PresentationRole::PanelEntry,
    ),
];

/// The semantic control kinds the shipped reducer actually projects.
///
/// Measured, not assumed. `Stepped` joined this set when the PATCH Utility
/// voice-limit row gained a canonical descriptor that classifies itself
/// `Stepped` and a reducer arm that edits it — the first production producer
/// of the kind. `Surface` is still built only on the fixture projection path.
/// Pinned so that a kind appearing or disappearing fails
/// [`the_production_projection_carries_the_kinds_this_target_can_drive`]
/// rather than silently shrinking what the document sweep covers.
const PRODUCTION_PROJECTED_KINDS: [SemanticControlKind; 5] = [
    SemanticControlKind::Continuous,
    SemanticControlKind::Stepped,
    SemanticControlKind::Choice,
    SemanticControlKind::Toggle,
    SemanticControlKind::Identity,
];

/// The kinds the shipped reducer projects nothing of.
const PRODUCTION_UNPROJECTED_KINDS: [SemanticControlKind; 6] = [
    SemanticControlKind::Asset,
    SemanticControlKind::Surface,
    SemanticControlKind::BrowserParent,
    SemanticControlKind::BrowserFolder,
    SemanticControlKind::BrowserFile,
    SemanticControlKind::BrowserCancel,
];

/// The declared column anatomy, closed and ordered, transcribed from the design
/// authority rather than read back from the page.
const COLUMN_ANATOMY: [&str; 5] = [
    "TrackHeader",
    "LevelFader",
    "LevelReadout",
    "PanReadout",
    "StateLine",
];

/// The declared unavailable treatment, and the fabrications it must not be.
const FORBIDDEN_MARKERS: [&str; 5] = ["", " ", "0", "0.0", "0.000"];

/// The five entries `DESIGN.md` draws in the PATCH Utility panel, in
/// authored order, with the projected driver each driven entry reads —
/// transcribed from the product authority rather than parsed back from the
/// page, so a panel that quietly rebinds or drops one fails here.
const AUTHORED_UTILITY_ENTRIES: [(&str, Option<&str>); 5] = [
    ("MASTER VOLUME", Some("patch.global.masterGainDb")),
    ("PATCH VOLUME", Some("patch.output.trimGainDb")),
    ("MIDI INPUT", Some("patch.midiInput")),
    ("OUTPUT TRACK", Some("patch.output.outputTrack")),
    ("VOICE LIMIT", Some("patch.voiceLimit")),
];

// ===========================================================================
// The production projection
// ===========================================================================

struct NullBoundary;

impl ControlAudioBoundary for NullBoundary {
    fn push_command(&mut self, _command: AudioCommand) -> Result<(), BoundaryFull> {
        Ok(())
    }

    fn publish_parameters(&mut self, _parameters: ParameterSnapshot) {}
}

/// The production reducer with one installed patch, so every surface has a
/// real projection to serialize rather than an empty one.
fn installed_state() -> AppState {
    let provider = production_soundfont_capability().expect("the production SoundFont capability");
    let config =
        create_soundfont_config(&provider, SoundFontInstrument::new(0, 40, false).unwrap())
            .expect("an installed instrument configuration");
    let patch = Patch::new(
        PatchId::new(1).unwrap(),
        "Component Composition".to_owned(),
        config,
        MidiChannel::new(0).unwrap(),
        PatchOutput::new(MixerTrackId::new(0).unwrap(), -6.0).unwrap(),
    );
    let mut state = AppState::new(
        production_capability_registry().expect("the production capability registry"),
        GlobalParameters::new(-3.0).unwrap(),
    );
    state
        .apply(AppEvent::InstallPatches(vec![patch]))
        .expect("the patch installs");
    state
}

/// The graphical shell projection the window is handed, for one context.
///
/// Reached through the production `AppLoop` and the production
/// `StateProjector`, which is the same call the window's projection callback
/// makes.
fn production_projection(context: TopLevelContext) -> GraphicalShellProjection {
    let mut app_loop = AppLoop::new(installed_state(), StateProjector::new(), NullBoundary)
        .expect("the production reducer");
    app_loop
        .dispatch_action(SemanticAction::SelectContext(context))
        .expect("the reducer accepts the context selection");
    let projection = app_loop.current_graphical_shell();
    assert_eq!(
        projection.semantic_model().context(),
        context,
        "the reducer did not reach {context:?}"
    );
    projection
}

/// The serialized rendered document for one context — the exact bytes the
/// production transport emits to the page.
fn production_document(context: TopLevelContext) -> Value {
    let projection = production_projection(context);
    let mut channel = ProjectionChannel::new();
    let mut emitted = None;
    let outcome = channel
        .push(&projection, |document| {
            emitted = Some(document);
            Ok(())
        })
        .expect("the production emit succeeds");
    assert_eq!(outcome, ProjectionPush::Emitted);
    emitted.expect("an Emitted push hands the emitter exactly one document")
}

/// Every projected control the shipped reducer carries, across both contexts.
fn production_controls() -> Vec<SemanticControlViewModel> {
    [TopLevelContext::Patch, TopLevelContext::Mixer]
        .into_iter()
        .flat_map(|context| {
            production_projection(context)
                .semantic_model()
                .surfaces()
                .iter()
                .flat_map(|surface| surface.controls().to_vec())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The first projected control of one semantic kind, or `None` when the
/// shipped reducer projects none. `None` is a real answer.
fn projected_control_of_kind(kind: SemanticControlKind) -> Option<SemanticControlViewModel> {
    production_controls()
        .into_iter()
        .find(|control| control.kind() == kind)
}

fn kind_name(kind: SemanticControlKind) -> &'static str {
    match kind {
        SemanticControlKind::Continuous => "Continuous",
        SemanticControlKind::Stepped => "Stepped",
        SemanticControlKind::Choice => "Choice",
        SemanticControlKind::Toggle => "Toggle",
        SemanticControlKind::Asset => "Asset",
        SemanticControlKind::Identity => "Identity",
        SemanticControlKind::Surface => "Surface",
        SemanticControlKind::BrowserParent => "BrowserParent",
        SemanticControlKind::BrowserFolder => "BrowserFolder",
        SemanticControlKind::BrowserFile => "BrowserFile",
        SemanticControlKind::BrowserCancel => "BrowserCancel",
    }
}

// ===========================================================================
// The committed page sources
// ===========================================================================

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn page_source(name: &str) -> String {
    let path = repository_root().join("webview-page").join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} must be readable: {error}", path.display()))
}

/// Parses one `var NAME = [ ... ];` array literal out of the committed
/// render script, returning the raw entry lines.
fn page_array_block(page_js: &str, name: &str) -> Vec<String> {
    let opener = format!("var {name} = [");
    let start = page_js
        .find(&opener)
        .unwrap_or_else(|| panic!("page.js declares {name}"));
    let body = &page_js[start + opener.len()..];
    let end = body.find("];").expect("the array literal closes");
    body[..end]
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

// ===========================================================================
// The page's transcribed derivations (the render-path contract)
// ===========================================================================

/// Derives one serialized control's row state exactly as the committed
/// render script does (`controlState` in `webview-page/page.js`) — the
/// declared ComponentState precedence: a failed edit outranks an in-flight
/// one, focus outranks read-only-ness, and anything unrecognized is an
/// explicit unknown, never a silent resting row.
fn page_row_state(control: &Value, mode: &str) -> String {
    if control.get("error").is_some_and(|error| !error.is_null()) {
        return "error".to_owned();
    }
    if let Some(kind) = control.pointer("/status/kind").and_then(Value::as_str) {
        match kind {
            "preparing" | "activating" => return "loading".to_owned(),
            "ready" | "failed" => {}
            _ => return "unknown".to_owned(),
        }
    }
    if control.get("focused").and_then(Value::as_bool) == Some(true) {
        return match mode {
            "adjust" => "adjusting".to_owned(),
            "navigate" | "modal" | "multiSelect" => "focused".to_owned(),
            _ => "unknown".to_owned(),
        };
    }
    if control.get("enabled").and_then(Value::as_bool) == Some(true)
        && control.get("editable").and_then(Value::as_bool) == Some(true)
    {
        "resting".to_owned()
    } else {
        "disabled".to_owned()
    }
}

fn page_normalized_percentage(control: &Value) -> bool {
    if control
        .get("selectedLabel")
        .is_some_and(|label| !label.is_null())
    {
        return false;
    }
    if control.get("unit").is_some_and(|unit| !unit.is_null()) {
        return false;
    }
    matches!(
        (
            control
                .pointer("/numericRange/minimum")
                .and_then(Value::as_f64),
            control
                .pointer("/numericRange/maximum")
                .and_then(Value::as_f64),
        ),
        (Some(0.0), Some(1.0)) | (Some(-1.0), Some(1.0))
    )
}

fn page_numeric_value_text(control: &Value, value: f64) -> String {
    if !value.is_finite() {
        return UNAVAILABLE_MARK.to_owned();
    }
    let scale = if page_normalized_percentage(control) {
        100.0
    } else {
        1.0
    };
    let step = control
        .pointer("/numericRange/fineStep")
        .and_then(Value::as_f64)
        .unwrap_or(0.001)
        * scale;
    let step_places = if !step.is_finite() || step <= 0.0 {
        3
    } else {
        (0..=6)
            .find(|places| {
                let scaled = step * 10_f64.powi(*places);
                (scaled - scaled.round()).abs() < 0.000_000_1
            })
            .unwrap_or(6) as usize
    };
    let mut text = format!("{:.*}", step_places.max(3), value * scale);
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    if text == "-0" {
        "0".to_owned()
    } else {
        text
    }
}

/// Renders one serialized control's value exactly as the committed render
/// script does (`controlValueText` in `webview-page/page.js`).
fn page_value_text(control: &Value) -> String {
    fn display(value: &Value) -> String {
        match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        }
    }
    let Some(value) = control.get("value").filter(|value| value.is_object()) else {
        return UNAVAILABLE_MARK.to_owned();
    };
    match value.get("kind").and_then(Value::as_str) {
        Some("scalar") => {
            let number = value["value"].as_f64().unwrap_or(f64::NAN);
            if control.get("kind").and_then(Value::as_str) == Some("stepped") {
                format!("{}", number.round() as i64)
            } else {
                page_numeric_value_text(control, number)
            }
        }
        Some("parameter") => {
            let Some(parameter) = value.get("value").filter(|value| value.is_object()) else {
                return UNAVAILABLE_MARK.to_owned();
            };
            match parameter.get("kind").and_then(Value::as_str) {
                Some("continuous") => control
                    .get("selectedLabel")
                    .filter(|label| !label.is_null())
                    .map_or_else(
                        || {
                            page_numeric_value_text(
                                control,
                                parameter["value"].as_f64().unwrap_or(f64::NAN),
                            )
                        },
                        display,
                    ),
                // A choice reads its projected authored name, falling back to
                // the stored id only when the descriptor declared none. The
                // page does the same (`controlValueText`); painting the id was
                // mission finding F-33.
                Some("stepped" | "choice") => control
                    .get("selectedLabel")
                    .filter(|label| !label.is_null())
                    .map_or_else(|| display(&parameter["value"]), display),
                Some("toggle") => if parameter["value"] == Value::Bool(true) {
                    "ON"
                } else {
                    "OFF"
                }
                .to_owned(),
                Some(other) => format!("?{other}"),
                None => UNAVAILABLE_MARK.to_owned(),
            }
        }
        Some("asset") => value
            .pointer("/value/locator")
            .and_then(Value::as_str)
            .map_or_else(|| UNAVAILABLE_MARK.to_owned(), str::to_owned),
        Some("identity") | Some("summary") => display(&value["value"]),
        Some(other) => format!("?{other}"),
        None => UNAVAILABLE_MARK.to_owned(),
    }
}

/// One serialized surface's control array.
fn surface_controls(document: &Value, id: &str) -> Vec<Value> {
    document
        .get("surfaces")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|surface| surface.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("the document carries the {id} surface"))
        .get("controls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Every visible control across every surface of one document.
fn visible_controls(document: &Value) -> Vec<Value> {
    document
        .get("surfaces")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|surface| {
            surface
                .get("controls")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .filter(|control| control.get("visible").and_then(Value::as_bool) == Some(true))
        .collect()
}

// ===========================================================================
// T042 / NFR-004 — the visual-decision guard
// ===========================================================================

/// One visual decision found outside the visual module.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct VisualDecision {
    file: String,
    line: usize,
    kind: &'static str,
    evidence: String,
}

impl VisualDecision {
    /// The actionable one-line report a failure prints.
    fn report(&self) -> String {
        format!(
            "{}:{}: {} decision outside the authored vocabulary — {}",
            self.file, self.line, self.kind, self.evidence
        )
    }
}

/// The retired painting subtree, excluded from the scan while it is
/// still in the tree (it is deleted at the end of the cutover mission, at
/// which point this prefix matches nothing).
const VISUAL_MODULE: &str = "src/shell/visual/";

/// The authored vocabulary declaration sources — the only files allowed to
/// decide how anything looks (mission webview-shell-cutover WP07 relocated
/// them out of the retired painting subtree).
const VOCABULARY_SOURCES: [&str; 5] = [
    "src/shell/tokens.rs",
    "src/shell/typeface.rs",
    "src/shell/density.rs",
    "src/shell/component_state.rs",
    "src/shell/component_vocabulary.rs",
];

/// Color constructors that build a color out of raw channels.
const COLOR_CONSTRUCTORS: [&str; 12] = [
    "from_rgb(",
    "from_rgba_unmultiplied(",
    "from_rgba_premultiplied(",
    "from_gray(",
    "from_black_alpha(",
    "from_white_alpha(",
    "from_additive_luminance(",
    "from_srgba_unmultiplied(",
    "from_srgba_premultiplied(",
    "from_luminance_alpha(",
    "from_rgb_additive(",
    "gray(",
];

/// Call positions whose numeric argument is a type size.
const FONT_SIZE_POSITIONS: [&str; 4] = [
    "FontId::new(",
    "FontId::proportional(",
    "FontId::monospace(",
    "set_font_size(",
];

/// Call positions whose numeric argument is a spacing, margin, keyline, or
/// target-size constant.
const SPACING_POSITIONS: [&str; 12] = [
    "add_space(",
    "Margin::same(",
    "Margin::symmetric(",
    "CornerRadius::same(",
    "Stroke::new(",
    "set_min_width(",
    "set_min_height(",
    "Size::exact(",
    "min_size(",
    "Vec2::splat(",
    "rect_filled(",
    "rect_stroke(",
];

/// Assignment targets whose numeric right-hand side is a spacing or target
/// constant.
const SPACING_ASSIGNMENTS: [&str; 4] = [
    "item_spacing",
    "interact_size",
    "button_padding",
    "indent =",
];

/// The authored palette, transcribed from `DESIGN.md` § Colors, plus the
/// palette the shell painted before the vocabulary landed.
const PALETTE_HEXES: [(&str, &str); 24] = [
    ("color/bg/canvas", "#0c1015"),
    ("color/bg/surface", "#121821"),
    ("color/bg/panel", "#17202a"),
    ("color/bg/elevated", "#1d2733"),
    ("color/bg/selected", "#2a3745"),
    ("color/border/strong", "#415166"),
    ("color/text/primary", "#f2f6f8"),
    ("color/text/secondary", "#b8c4d1"),
    ("color/text/muted", "#6f8095"),
    ("color/accent/focus", "#65e5ff"),
    ("color/accent/adjust", "#ffb454"),
    ("color/accent/positive", "#58e887"),
    ("color/accent/warning", "#ff6868"),
    ("color/accent/instrument/plates", "#b894ff"),
    ("color/accent/patch", "#ff6fbe"),
    ("color/accent/chorus", "#f6f178"),
    ("retired focus green", "#6ecdae"),
    ("retired canvas", "#101216"),
    ("retired surface", "#181b20"),
    ("retired elevated", "#1d2127"),
    ("retired text primary", "#e6eaef"),
    ("retired text muted", "#969ea9"),
    ("retired adjust amber", "#e8ae4c"),
    ("retired border", "#2a3140"),
];

/// Every responsive bound, read from the contract itself.
fn declared_band_extents() -> Vec<(String, f32)> {
    ResponsiveShellContract::get()
        .all_bounds()
        .into_iter()
        .flat_map(|(name, bounds)| {
            [
                (format!("{name} minimum"), bounds.minimum_px),
                (format!("{name} preferred"), bounds.preferred_px),
                (format!("{name} maximum"), bounds.maximum_px),
            ]
        })
        .collect()
}

/// Strips line comments, and optionally the contents of string literals.
fn strip(line: &str, keep_strings: bool) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let mut in_string = false;
    while let Some(character) = chars.next() {
        if in_string {
            if character == '\\' {
                if let Some(escaped) = chars.next() {
                    if keep_strings {
                        out.push(escaped);
                    }
                }
            } else if character == '"' {
                in_string = false;
                out.push('"');
            } else if keep_strings {
                out.push(character);
            }
            continue;
        }
        match character {
            '"' => {
                in_string = true;
                out.push('"');
            }
            '/' if chars.peek() == Some(&'/') => break,
            other => out.push(other),
        }
    }
    out
}

/// Whether a token is a bare numeric literal other than zero.
fn is_nonzero_numeric_literal(token: &str) -> bool {
    let token = token.trim();
    let token = token.strip_suffix("_f32").unwrap_or(token);
    let token = token.strip_suffix("f32").unwrap_or(token);
    let token = token.strip_suffix("f64").unwrap_or(token);
    let token = token.strip_suffix("u8").unwrap_or(token);
    if token.is_empty() {
        return false;
    }
    let digits: String = token
        .chars()
        .filter(|character| *character != '_')
        .collect();
    let numeric = if let Some(hex) = digits.strip_prefix("0x") {
        !hex.is_empty() && hex.chars().all(|character| character.is_ascii_hexdigit())
    } else {
        digits
            .chars()
            .all(|character| character.is_ascii_digit() || character == '.')
            && digits.chars().any(|character| character.is_ascii_digit())
    };
    if !numeric {
        return false;
    }
    digits.parse::<f64>().map_or(
        !digits.trim_start_matches("0x").trim_matches('0').is_empty(),
        |value| value != 0.0,
    )
}

/// The top-level arguments of the call opening immediately after `at`.
fn call_arguments(code: &str, at: usize) -> Vec<String> {
    let mut depth = 0_i32;
    let mut current = String::new();
    let mut arguments = Vec::new();
    for character in code[at..].chars() {
        match character {
            '(' | '[' | '<' if depth > 0 => {
                depth += i32::from(character == '(' || character == '[');
                current.push(character);
            }
            '(' => {
                depth += 1;
                if depth > 1 {
                    current.push(character);
                }
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    arguments.push(current);
                    return arguments;
                }
                current.push(character);
            }
            '[' => {
                depth += 1;
                current.push(character);
            }
            ']' => {
                depth -= 1;
                current.push(character);
            }
            ',' if depth == 1 => {
                arguments.push(std::mem::take(&mut current));
            }
            other => current.push(other),
        }
    }
    arguments.push(current);
    arguments
}

/// Every whole numeric token on one line of code, underscores normalized
/// away.
fn numeric_tokens(code: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut previous_was_word = false;
    for character in code.chars() {
        if character.is_ascii_digit() || character == '.' || character == '_' {
            current.push(character);
            continue;
        }
        let is_word = character.is_ascii_alphabetic();
        if !current.is_empty() {
            if !previous_was_word && !is_word {
                tokens.push(current.replace('_', ""));
            }
            current.clear();
        }
        previous_was_word = is_word;
    }
    if !current.is_empty() && !previous_was_word {
        tokens.push(current.replace('_', ""));
    }
    tokens
}

/// Scans one source for visual decisions.
///
/// Exposed as a plain function over text so the guard can be fed a planted
/// sample and shown failing, which is what makes it a guard rather than a
/// decoration. Only production code is read: everything from the first
/// `#[cfg(test)]` marker onward is a test module.
fn scan_visual_decisions(path: &str, source: &str) -> Vec<VisualDecision> {
    let test_marker = format!("#[cfg({})]", "test");
    let mut violations = Vec::new();
    let paints = production_code(source).contains(concat!("eg", "ui"));
    let band_extents = declared_band_extents();

    for (index, raw) in source.lines().enumerate() {
        if raw.trim_start().starts_with(&test_marker) {
            break;
        }
        let line = index + 1;
        let code = strip(raw, false);
        let lowered = strip(raw, true).to_ascii_lowercase();

        let mut flag = |kind: &'static str, evidence: String| {
            violations.push(VisualDecision {
                file: path.to_owned(),
                line,
                kind,
                evidence,
            });
        };

        for needle in COLOR_CONSTRUCTORS {
            let mut from = 0;
            while let Some(offset) = code[from..].find(needle) {
                let open = from + offset + needle.len() - 1;
                for argument in call_arguments(&code, open) {
                    if is_nonzero_numeric_literal(&argument) {
                        flag(
                            "color",
                            format!(
                                "{needle}{} builds a color from raw channels",
                                argument.trim()
                            ),
                        );
                        break;
                    }
                }
                from = open + 1;
            }
        }

        if let Some(offset) = code.find("from_hex(") {
            let open = offset + "from_hex(".len() - 1;
            if call_arguments(&code, open)
                .iter()
                .any(|argument| argument.contains('"'))
            {
                flag("color", "from_hex(\"…\") spells a color".to_owned());
            }
        }

        for (name, hex) in PALETTE_HEXES {
            let bare = hex.trim_start_matches('#');
            let channels = {
                let value = u32::from_str_radix(bare, 16).expect("an authored hex parses");
                format!(
                    "0x{:02x}, 0x{:02x}, 0x{:02x}",
                    (value >> 16) & 0xff,
                    (value >> 8) & 0xff,
                    value & 0xff
                )
            };
            if lowered.contains(hex)
                || lowered.contains(&format!("0x{bare}"))
                || lowered.replace("  ", " ").contains(&channels)
            {
                flag("palette", format!("{name} ({hex}) is spelled here"));
            }
        }

        for needle in FONT_SIZE_POSITIONS {
            if let Some(offset) = code.find(needle) {
                let open = offset + needle.len() - 1;
                if let Some(first) = call_arguments(&code, open).first() {
                    if is_nonzero_numeric_literal(first) {
                        flag(
                            "type size",
                            format!("{needle}{}) sets a raw point size", first.trim()),
                        );
                    }
                }
            }
        }

        for needle in SPACING_POSITIONS {
            let mut from = 0;
            while let Some(offset) = code[from..].find(needle) {
                let open = from + offset + needle.len() - 1;
                for argument in call_arguments(&code, open) {
                    if is_nonzero_numeric_literal(&argument) {
                        flag(
                            "spacing",
                            format!("{needle}…{}…) sets a raw extent", argument.trim()),
                        );
                        break;
                    }
                }
                from = open + 1;
            }
        }

        for needle in SPACING_ASSIGNMENTS {
            if let Some(offset) = code.find(needle) {
                if let Some(rest) = code[offset..].split_once('=') {
                    let right = rest.1.trim_end_matches(';').trim();
                    if is_nonzero_numeric_literal(right) {
                        flag("spacing", format!("{needle} … = {right} sets a raw extent"));
                    }
                }
            }
        }

        if paints {
            for token in numeric_tokens(&code) {
                // An integer is a count, an index, or a block size. A band
                // extent is a pixel measurement and is spelled as one.
                if !token.contains('.') {
                    continue;
                }
                let Ok(value) = token.parse::<f32>() else {
                    continue;
                };
                for (name, extent) in &band_extents {
                    if (value - *extent).abs() < f32::EPSILON {
                        flag(
                            "band height",
                            format!("{token} is the declared {name} extent"),
                        );
                    }
                }
            }
        }
    }
    violations.sort();
    violations.dedup();
    violations
}

/// Every source the guard scans: the whole `src/` tree, less the visual
/// module. The exclusion is the *only* one.
fn scanned_sources() -> Vec<String> {
    fn walk(directory: &Path, into: &mut Vec<PathBuf>) {
        let entries = std::fs::read_dir(directory)
            .unwrap_or_else(|error| panic!("{} is unreadable: {error}", directory.display()));
        for entry in entries {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                walk(&path, into);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                into.push(path);
            }
        }
    }

    let root = repository_root();
    let mut absolute = Vec::new();
    walk(&root.join("src"), &mut absolute);
    let mut relative: Vec<String> = absolute
        .into_iter()
        .map(|path| {
            path.strip_prefix(&root)
                .expect("a scanned source lives under the repository root")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .filter(|path| {
            !path.starts_with(VISUAL_MODULE) && !VOCABULARY_SOURCES.contains(&path.as_str())
        })
        .collect();
    relative.sort();
    relative
}

/// Runs the guard over the delivered tree, returning what it read and what
/// it found.
fn scan_delivered_tree() -> (Vec<String>, usize, Vec<VisualDecision>) {
    let root = repository_root();
    let sources = scanned_sources();
    let mut lines_read = 0;
    let mut violations = Vec::new();
    for path in &sources {
        let source = std::fs::read_to_string(root.join(path))
            .unwrap_or_else(|error| panic!("{path} is unreadable: {error}"));
        lines_read += source.lines().count();
        violations.extend(scan_visual_decisions(path, &source));
    }
    violations.sort();
    (sources, lines_read, violations)
}

/// Every production source of the authored vocabulary, for the ownership
/// checks — the relocated declaration files, plus whatever remains of the
/// retired painting subtree while it is still in the tree.
fn visual_module_sources() -> Vec<String> {
    fn walk(directory: &Path, into: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(directory).expect("the visual module directory") {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                walk(&path, into);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                into.push(path);
            }
        }
    }
    let root = repository_root();
    let mut absolute = Vec::new();
    if root.join(VISUAL_MODULE).is_dir() {
        walk(&root.join(VISUAL_MODULE), &mut absolute);
    }
    let mut relative: Vec<String> = absolute
        .into_iter()
        .map(|path| {
            path.strip_prefix(&root)
                .expect("a visual source lives under the repository root")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    for source in VOCABULARY_SOURCES {
        relative.push(source.to_owned());
    }
    relative.sort();
    relative
}

/// One source's production code — everything before its first
/// `#[cfg(test)]` marker — with line comments stripped.
fn production_code(source: &str) -> String {
    let marker = format!("#[cfg({})]", "test");
    source
        .lines()
        .take_while(|line| !line.trim_start().starts_with(&marker))
        .map(|line| strip(line, true))
        .collect::<Vec<_>>()
        .join("\n")
}

// ===========================================================================
// The webview transport guard — "the render adapter holds no paint decision"
// ===========================================================================

/// The webview window composition sources: transport only, by declaration.
/// `token_export.rs` is deliberately absent — it is the generator that reads
/// the vocabulary, and it must name the vocabulary's color type to do so.
const WEBVIEW_TRANSPORT_SOURCES: [&str; 6] = [
    "src/shell/webview/mod.rs",
    "src/shell/webview/window.rs",
    "src/shell/webview/projection_channel.rs",
    "src/shell/webview/frame_stream.rs",
    "src/shell/webview/meter_channel.rs",
    "src/shell/webview/input_capture.rs",
];

/// Paint and layout API names that must not appear in the transport
/// adapter's production code — a `TauriWebviewWindow` that names one has
/// started deciding how something looks.
const PAINT_DECISION_NEEDLES: [&str; 9] = [
    // The two retired-stack crate names are assembled so this guard's own
    // source stays clean under the zero-reference sweep (SC-003) while the
    // needle still catches a reintroduction.
    concat!("eg", "ui"),
    concat!("ep", "aint"),
    "Painter",
    "rect_filled",
    "rect_stroke",
    "galley",
    "FontId",
    "Color32",
    "tessellat",
];

/// The transport-only rule over one source: the visual-decision families,
/// the paint API names, and the declared band extents are all absent from
/// production code. Exposed over text so the guard can be fed a planted
/// sample and shown failing.
fn webview_transport_violations(path: &str, source: &str) -> Vec<String> {
    let mut violations: Vec<String> = scan_visual_decisions(path, source)
        .iter()
        .map(VisualDecision::report)
        .collect();
    let code = production_code(source);
    for needle in PAINT_DECISION_NEEDLES {
        if code.contains(needle) {
            violations.push(format!(
                "{path}: names {needle} — a paint API inside the transport adapter"
            ));
        }
    }
    let band_extents = declared_band_extents();
    for (index, line) in code.lines().enumerate() {
        for token in numeric_tokens(line) {
            if !token.contains('.') {
                continue;
            }
            let Ok(value) = token.parse::<f32>() else {
                continue;
            };
            for (name, extent) in &band_extents {
                if (value - *extent).abs() < f32::EPSILON {
                    violations.push(format!(
                        "{path}:{}: {token} is the declared {name} extent — a layout \
                         decision inside the transport adapter",
                        index + 1
                    ));
                }
            }
        }
    }
    violations
}

/// `TauriWebviewWindow` and its transports hold no paint or layout decision.
fn check_the_webview_window_holds_no_paint_decision() {
    let root = repository_root();
    let mut lines_read = 0_usize;
    for path in WEBVIEW_TRANSPORT_SOURCES {
        let source = std::fs::read_to_string(root.join(path))
            .unwrap_or_else(|error| panic!("{path} is unreadable: {error}"));
        lines_read += production_code(&source).lines().count();
        let violations = webview_transport_violations(path, &source);
        assert!(
            violations.is_empty(),
            "the webview transport adapter decides how something looks:\n{}",
            violations.join("\n")
        );
    }
    assert!(
        lines_read > 800,
        "the transport guard read only {lines_read} production lines, which cannot \
         have covered the window composition"
    );
}

// ===========================================================================
// T041 — kind × role totality, control reachability, per-state legibility
// ===========================================================================

/// Every `(kind, role)` pair, in declaration order.
fn every_pair() -> Vec<(SemanticControlKind, PresentationRole)> {
    ALL_SEMANTIC_CONTROL_KINDS
        .into_iter()
        .flat_map(|kind| {
            ALL_PRESENTATION_ROLES
                .into_iter()
                .map(move |role| (kind, role))
        })
        .collect()
}

fn is_declared_not_askable(kind: SemanticControlKind, role: PresentationRole) -> bool {
    NOT_ASKABLE_PAIRS
        .iter()
        .any(|(declared_kind, declared_role)| *declared_kind == kind && *declared_role == role)
}

/// Selection is total over kind × role, and every declared control is
/// reachable.
fn check_selection_is_total_and_every_control_reachable() {
    assert_eq!(SEMANTIC_CONTROL_KIND_COUNT, 11);
    assert_eq!(PRESENTATION_ROLE_COUNT, 4);
    assert_eq!(COMPONENT_CONTROL_COUNT, 8);

    let pairs = every_pair();
    assert_eq!(
        pairs.len(),
        SEMANTIC_CONTROL_KIND_COUNT * PRESENTATION_ROLE_COUNT,
        "the pair enumeration lost a kind or a role"
    );

    let declared: BTreeSet<&str> = ALL_COMPONENT_CONTROLS
        .into_iter()
        .map(ComponentControl::canonical_name)
        .collect();
    assert_eq!(
        declared.len(),
        COMPONENT_CONTROL_COUNT,
        "a control appears twice in ALL_COMPONENT_CONTROLS"
    );

    let mut reachable: BTreeSet<&str> = BTreeSet::new();
    let mut refused = 0_usize;
    for (kind, role) in pairs {
        let selection = control_for(kind, role);
        let label = format!("{} in {}", kind_name(kind), role.canonical_name());
        if is_declared_not_askable(kind, role) {
            assert_eq!(
                selection,
                ControlSelection::NotAskableInRole,
                "{label} is declared un-askable but resolved to {selection:?}"
            );
            refused += 1;
            continue;
        }
        let control = selection
            .control()
            .unwrap_or_else(|| panic!("{label} resolves to nothing; selection is not total"));
        assert!(
            declared.contains(control.canonical_name()),
            "{label} resolves to {}, which is not a declared control",
            control.canonical_name()
        );
        reachable.insert(control.canonical_name());
    }

    assert_eq!(
        refused,
        NOT_ASKABLE_PAIRS.len(),
        "the un-askable set no longer matches the declared pairs"
    );
    assert_eq!(
        reachable, declared,
        "a declared control is asked for by no pair, or a pair asks for something undeclared"
    );
}

/// The selector is exhaustive rather than defaulted.
fn check_the_selector_is_exhaustive_rather_than_defaulted() {
    let source =
        std::fs::read_to_string(repository_root().join("src/shell/component_vocabulary.rs"))
            .expect("the component vocabulary module");
    let signature = "pub const fn control_for(";
    let start = source
        .find(signature)
        .expect("control_for is declared in the control family's module root");
    let body = &source[start..];
    let end = body
        .find("\n}\n")
        .expect("control_for's body is brace-delimited");
    let body = strip_block_comments(&body[..end]);

    for needle in [format!("{}{}", "_ =", ">"), format!("{}{}", ".. =", ">")] {
        assert!(
            !body.contains(&needle),
            "control_for has a `{needle}` arm; a defaulted mapping answers for a pair \
             nobody considered"
        );
    }
    assert!(
        body.contains("match (kind, role)"),
        "the selector scan did not read control_for's match"
    );
    assert!(
        body.lines().count() > 20,
        "the selector scan read only {} lines of control_for",
        body.lines().count()
    );
}

/// Strips HTML comment spans (`<!-- ... -->`, which may run across lines)
/// from a served document, so the purity scan reads the markup the window
/// serves and not the narration describing it.
fn strip_html_comments(document: &str) -> String {
    let mut out = String::with_capacity(document.len());
    let mut rest = document;
    while let Some(open) = rest.find("<!--") {
        out.push_str(&rest[..open]);
        rest = match rest[open..].find("-->") {
            Some(close) => &rest[open + close + "-->".len()..],
            // An unterminated comment swallows the rest of the document in a
            // browser too; reading it as comment matches what ships.
            None => "",
        };
    }
    out.push_str(rest);
    out
}

/// Strips line comments from a block of source.
fn strip_block_comments(block: &str) -> String {
    block
        .lines()
        .map(|line| strip(line, false))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The shipped reducer projects exactly the kinds this target's document
/// sweep can drive.
fn check_the_production_projection_carries_the_kinds_this_target_can_drive() {
    let projected: BTreeSet<&str> = production_controls()
        .into_iter()
        .map(|control| kind_name(control.kind()))
        .collect();
    let expected: BTreeSet<&str> = PRODUCTION_PROJECTED_KINDS
        .into_iter()
        .map(kind_name)
        .collect();
    assert_eq!(
        projected, expected,
        "the set of production-projected control kinds changed"
    );
    for kind in PRODUCTION_UNPROJECTED_KINDS {
        assert!(
            projected_control_of_kind(kind).is_none(),
            "{} is now projected; the document sweep must drive it",
            kind_name(kind)
        );
    }
    assert_eq!(
        PRODUCTION_PROJECTED_KINDS.len() + PRODUCTION_UNPROJECTED_KINDS.len(),
        SEMANTIC_CONTROL_KIND_COUNT,
        "the two pinned kind sets no longer partition the kind vocabulary"
    );
}

/// Every visible projected control across both contexts renders through the
/// page's transcribed value contract: a non-empty label, a value that is
/// neither blank nor an explicit unknown marker nor a fabricated NaN, and a
/// derived state within the pinned page state set.
///
/// Returns how many controls were driven, so the caller can assert a
/// denominator.
fn check_every_projected_control_renders_through_the_document() -> usize {
    let mut driven = 0_usize;
    let mut kinds_seen: BTreeSet<&str> = BTreeSet::new();
    let page_states: BTreeSet<&str> = [
        "resting",
        "focused",
        "adjusting",
        "disabled",
        "loading",
        "error",
        "unknown",
    ]
    .into_iter()
    .collect();

    for context in [TopLevelContext::Patch, TopLevelContext::Mixer] {
        let document = production_document(context);
        let mode = document
            .get("interactionMode")
            .and_then(Value::as_str)
            .expect("the document names its interaction mode");
        let controls = visible_controls(&document);
        assert!(
            !controls.is_empty(),
            "{context:?}: the production document projects visible controls"
        );
        for control in &controls {
            let label = control
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                !label.trim().is_empty(),
                "{context:?}: a projected control carries no label: {control}"
            );
            let value = page_value_text(control);
            assert!(
                !value.trim().is_empty(),
                "{context:?}: {label} renders a blank value"
            );
            assert!(
                !value.starts_with('?'),
                "{context:?}: {label} carries a value kind the page cannot render: {value}"
            );
            assert_ne!(
                value, UNAVAILABLE_MARK,
                "{context:?}: {label} projects a value yet renders the unavailable mark"
            );
            assert!(
                !value.contains("NaN"),
                "{context:?}: {label} renders a fabricated number: {value}"
            );
            let state = page_row_state(control, mode);
            assert!(
                page_states.contains(state.as_str()),
                "{context:?}: {label} derives the undeclared state {state}"
            );
            if let Some(kind) = control.get("kind").and_then(Value::as_str) {
                for declared in PRODUCTION_PROJECTED_KINDS {
                    if kind_name(declared).eq_ignore_ascii_case(kind) {
                        kinds_seen.insert(kind_name(declared));
                    }
                }
            }
            driven += 1;
        }
    }

    let expected: BTreeSet<&str> = PRODUCTION_PROJECTED_KINDS
        .into_iter()
        .map(kind_name)
        .collect();
    assert_eq!(
        kinds_seen, expected,
        "a production-projected kind never reached the rendered document"
    );
    driven
}

/// The state-applicability declarations survive: every control declares its
/// states, `accepts` agrees with the declaration, the union covers the
/// closed vocabulary, and mute/solo stay mixer-strip-only.
fn check_state_applicability_declarations() {
    assert_eq!(COMPONENT_STATE_COUNT, 10);
    let union: BTreeSet<&str> = ALL_COMPONENT_CONTROLS
        .into_iter()
        .flat_map(|control| control.applicable_states().iter().copied())
        .map(ComponentState::canonical_name)
        .collect();
    assert_eq!(
        union.len(),
        COMPONENT_STATE_COUNT,
        "a declared state is applicable to no control, so no surface can ever show it"
    );

    for control in ALL_COMPONENT_CONTROLS {
        let states = control.applicable_states();
        assert!(
            !states.is_empty(),
            "{} declares no applicable state",
            control.canonical_name()
        );
        for state in ALL_COMPONENT_STATES {
            let declared = states.contains(&state);
            assert_eq!(
                control.accepts(state),
                declared,
                "{} and {} disagree about applicability",
                control.canonical_name(),
                state.canonical_name()
            );
        }
        let in_strip = matches!(control, ComponentControl::Fader | ComponentControl::Meter);
        for state in [ComponentState::Muted, ComponentState::Soloed] {
            assert_eq!(
                control.accepts(state),
                in_strip,
                "{} declares {} applicable",
                control.canonical_name(),
                state.canonical_name()
            );
        }
    }

    // Every state that declares a fixed word carries a real word.
    for state in ALL_COMPONENT_STATES {
        if let NonColorSignal::Word(word) = state.appearance().signal {
            assert!(
                !word.trim().is_empty(),
                "{} declares an empty word",
                state.canonical_name()
            );
        }
    }
}

// ===========================================================================
// T042 — every region is a declared band, produced from the document
// ===========================================================================

/// The page band vocabulary: the declared shell region, the page's band
/// attribute, and the page element that carries it — transcribed from the
/// committed page so a drifted band fails against this table.
const PAGE_BANDS: [(&str, &str, &str); 5] = [
    ("contextLine", "contextLine", "context-line"),
    ("identityHeader", "identityHeader", "identity-header"),
    ("mainWorkspace", "workspace", "workspace"),
    ("persistentSideRegion", "inspector", "inspector"),
    ("footer", "footer", "footer"),
];

/// The page's paint-acknowledgment role, played headless (identity from the
/// pushed document, geometry from the authored policy).
fn page_painted_ack(document: &Value, viewport: [f32; 2]) -> Value {
    let mode = ResponsiveLayoutMode::resolve(viewport[0]);
    let contract = ResponsiveShellContract::get();
    let context_bottom = contract.context_line.preferred_px;
    let identity_bottom = context_bottom + contract.identity_header.preferred_px;
    let workspace_bottom = viewport[1] - contract.footer.preferred_px;
    let workspace_height = workspace_bottom - identity_bottom;
    let side_width = contract
        .side_track
        .preferred_px
        .min(viewport[0] - contract.side_track.minimum_px);
    let (main_width, main_height, side_x, side_y, side_height) = match mode {
        ResponsiveLayoutMode::Compact => (
            viewport[0],
            workspace_height / 2.0,
            0.0,
            identity_bottom + workspace_height / 2.0,
            workspace_height / 2.0,
        ),
        ResponsiveLayoutMode::Wide | ResponsiveLayoutMode::Standard => (
            viewport[0] - side_width,
            workspace_height,
            viewport[0] - side_width,
            identity_bottom,
            workspace_height,
        ),
    };
    let context = document
        .get("context")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_uppercase();
    json!({
        "generation": document["generation"],
        "stateHash": document["stateHash"],
        "context": document["context"],
        "activeSurface": document["activeSurface"],
        "focusPath": document["focusPath"],
        "interactionMode": document["interactionMode"],
        "viewport": { "widthPx": viewport[0], "heightPx": viewport[1] },
        "regions": [
            { "id": "contextLine", "xPx": 0.0, "yPx": 0.0,
              "widthPx": viewport[0], "heightPx": context_bottom,
              "label": "CREST SYNTH" },
            { "id": "identityHeader", "xPx": 0.0, "yPx": context_bottom,
              "widthPx": viewport[0], "heightPx": contract.identity_header.preferred_px,
              "label": context },
            { "id": "mainWorkspace", "xPx": 0.0, "yPx": identity_bottom,
              "widthPx": main_width, "heightPx": main_height,
              "label": "WORKSPACE" },
            { "id": "persistentSideRegion", "xPx": side_x, "yPx": side_y,
              "widthPx": if mode == ResponsiveLayoutMode::Compact { viewport[0] } else { side_width },
              "heightPx": side_height,
              "label": "SIDE" },
            { "id": "footer", "xPx": 0.0, "yPx": workspace_bottom,
              "widthPx": viewport[0], "heightPx": contract.footer.preferred_px,
              "label": context },
        ],
    })
}

/// Forwards one context's document through the production seam at one
/// viewport and returns its observation.
fn forwarded_observation(
    projection: &GraphicalShellProjection,
    viewport: [f32; 2],
) -> (Value, ShellFrameObservation) {
    let mut channel = ProjectionChannel::new();
    let mut emitted = None;
    channel
        .push(projection, |document| {
            emitted = Some(document);
            Ok(())
        })
        .expect("the production emit succeeds");
    let document = emitted.expect("an Emitted push hands the emitter exactly one document");
    let observation = match channel
        .forward_ack(&page_painted_ack(&document, viewport).to_string())
        .expect("the painted ack for the pushed document forwards")
    {
        ForwardedAck::Observation(observation) => observation,
        ForwardedAck::SupersededLate { generation } => {
            panic!("the ack for the just-pushed document cannot be late (generation {generation})")
        }
    };
    (document, observation)
}

/// Every region the shipped shell shows is a declared band: the composition
/// family's region bindings, the committed page's semantic containers, and
/// the forwarded observations all name exactly the declared region set — at
/// the authored extents, for both contexts, at both viewports.
fn check_every_region_is_a_declared_band() {
    // The composition family still declares its region bindings, and every
    // observed region is bound by at least one declared composition.
    assert_eq!(SHELL_COMPOSITION_COUNT, 10);
    let declared: BTreeSet<&str> = ALL_SHELL_COMPOSITIONS
        .into_iter()
        .map(ShellComposition::canonical_name)
        .collect();
    assert_eq!(declared.len(), SHELL_COMPOSITION_COUNT);
    let bound_regions: BTreeSet<&str> = ALL_SHELL_COMPOSITIONS
        .into_iter()
        .filter_map(|composition| composition.region().observation_name())
        .collect();
    for id in ShellRegionId::ALL {
        assert!(
            bound_regions.contains(id.name()),
            "{} is bound by no declared composition",
            id.name()
        );
    }
    assert!(
        ALL_SHELL_COMPOSITIONS
            .into_iter()
            .any(|composition| composition.region() == ShellRegion::WholeFrame),
        "the whole-frame composition disappeared"
    );

    // The committed page carries the five declared bands as semantic
    // containers, in canonical order, and the render script rebuilds exactly
    // those five.
    let index_html = page_source("index.html");
    let page_js = page_source("page.js");
    let mut found_order: Vec<String> = Vec::new();
    let mut from = 0;
    while let Some(offset) = index_html[from..].find("data-band=\"") {
        let start = from + offset + "data-band=\"".len();
        let name: String = index_html[start..]
            .chars()
            .take_while(|c| *c != '"')
            .collect();
        found_order.push(name);
        from = start;
    }
    assert_eq!(
        found_order,
        PAGE_BANDS
            .iter()
            .map(|(_, band, _)| (*band).to_owned())
            .collect::<Vec<_>>(),
        "the page's semantic band containers are not the declared five in canonical order"
    );
    assert_eq!(
        ShellRegionId::surface_descriptor()
            .iter()
            .map(|id| id.name())
            .collect::<Vec<_>>(),
        PAGE_BANDS
            .iter()
            .map(|(region, _, _)| *region)
            .collect::<Vec<_>>(),
        "the declared region order and the page band table disagree"
    );
    for (_, _, element) in PAGE_BANDS {
        assert!(
            page_js.contains(&format!("getElementById(\"{element}\")")),
            "the render script does not rebuild the {element} band"
        );
    }

    // The forwarded observations: the declared regions, exactly, at the
    // authored extents — for both contexts at both viewports.
    for context in [TopLevelContext::Mixer, TopLevelContext::Patch] {
        let projection = production_projection(context);
        for (viewport, mode) in REFERENCE_VIEWPORTS {
            let label = format!("{context:?} at {viewport:?}");
            let (document, observation) = forwarded_observation(&projection, viewport);
            assert_eq!(
                document["context"].as_str(),
                Some(match context {
                    TopLevelContext::Mixer => "mixer",
                    TopLevelContext::Patch => "patch",
                })
            );
            assert_eq!(
                observation
                    .regions()
                    .iter()
                    .map(|region| region.id())
                    .collect::<Vec<_>>(),
                ShellRegionId::surface_descriptor(),
                "{label}: the shell dropped or reordered a structural region"
            );
            assert!(
                observation.regions_are_non_overlapping(),
                "{label}: two structural regions overlap"
            );
            assert_eq!(ResponsiveLayoutMode::resolve(viewport[0]), mode);
            let contract = ResponsiveShellContract::get();
            let context_line = observation.region(ShellRegionId::ContextLine).rect();
            let identity = observation.region(ShellRegionId::IdentityHeader).rect();
            let main = observation.region(ShellRegionId::MainWorkspace).rect();
            let side = observation
                .region(ShellRegionId::PersistentSideRegion)
                .rect();
            let footer = observation.region(ShellRegionId::Footer).rect();
            assert_eq!(
                context_line.height(),
                contract.context_line.preferred_px,
                "{label} context line"
            );
            assert_eq!(
                identity.height(),
                contract.identity_header.preferred_px,
                "{label} identity header"
            );
            assert_eq!(
                footer.height(),
                contract.footer.preferred_px,
                "{label} footer"
            );
            assert!(
                side.width() >= 320.0,
                "{label}: the side region narrowed to {} px",
                side.width()
            );
            match mode {
                ResponsiveLayoutMode::Compact => {
                    assert_eq!(main.width(), viewport[0]);
                    assert_eq!(side.width(), viewport[0]);
                    assert_eq!(
                        context_line.height()
                            + identity.height()
                            + main.height()
                            + side.height()
                            + footer.height(),
                        viewport[1]
                    );
                }
                ResponsiveLayoutMode::Wide | ResponsiveLayoutMode::Standard => {
                    assert_eq!(main.width() + side.width(), viewport[0]);
                    assert_eq!(
                        context_line.height() + identity.height() + main.height() + footer.height(),
                        viewport[1]
                    );
                }
            }
            for region in observation.regions() {
                assert!(
                    !region.visible_label().trim().is_empty(),
                    "{label}: {} painted no visible label",
                    region.id().name()
                );
            }
        }
    }
}

/// The mixer-column anatomy: the committed page walks exactly the declared
/// closed structure list, in order, and the production document supplies the
/// per-track controls that drive it.
fn check_the_mixer_column_anatomy_is_declared_and_driven() {
    let page_js = page_source("page.js");

    // The page's anatomy array is the declared anatomy, verbatim and in
    // order.
    let entries = page_array_block(&page_js, "COLUMN_ANATOMY");
    let parsed: Vec<String> = entries
        .iter()
        .map(|entry| entry.trim_matches(|c| c == '"' || c == ',').to_owned())
        .collect();
    assert_eq!(
        parsed,
        COLUMN_ANATOMY
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>(),
        "the page's column anatomy drifted from the declared closed structure list"
    );
    // Each structure is actually emitted as a measurable data attribute.
    for structure in COLUMN_ANATOMY {
        assert!(
            page_js.contains(&format!("data-structure=\"{structure}\"")),
            "the render script never paints the {structure} structure"
        );
    }

    // The document drives it: sixteen tracks, each carrying the four main
    // controls the five structures read.
    let document = production_document(TopLevelContext::Mixer);
    let controls = surface_controls(&document, "mixerMain");
    assert_eq!(controls.len(), MixerTrackId::COUNT * 4);
    for track in 0..MixerTrackId::COUNT as u64 {
        for parameter in ["level", "pan", "mute", "solo"] {
            assert!(
                controls.iter().any(|control| {
                    control
                        .pointer("/path/controlId/id/trackId")
                        .and_then(Value::as_u64)
                        == Some(track)
                        && control
                            .pointer("/path/controlId/id/parameter")
                            .and_then(Value::as_str)
                            == Some(parameter)
                }),
                "track {track} carries no {parameter} control in the rendered document"
            );
        }
    }
}

// ===========================================================================
// T043 — the no-placeholder rule and the ownership boundary
// ===========================================================================

/// A structure with no view data behind it is marked, never painted with a
/// placeholder. Utility itself is projection-driven: the canonical surface
/// supplies the complete ordered row set and the page owns no parallel table.
fn check_a_designed_structure_with_no_view_data_is_marked() {
    assert_eq!(
        UNAVAILABLE_MARK, "--",
        "the declared unavailable treatment changed"
    );
    for forbidden in FORBIDDEN_MARKERS {
        assert_ne!(
            UNAVAILABLE_MARK, forbidden,
            "the unavailable mark is indistinguishable from a real empty or zero value"
        );
    }

    let page_js = page_source("page.js");
    // The page's mark and separator are the declared vocabulary's, verbatim.
    assert!(
        page_js.contains(&format!("var UNAVAILABLE_MARK = \"{UNAVAILABLE_MARK}\";")),
        "the page's unavailable mark drifted from the declared treatment"
    );
    assert!(
        page_js.contains(&format!("var HINT_SEPARATOR = \"{HINT_SEPARATOR}\";")),
        "the page's hint separator drifted from the authored separator"
    );

    assert!(
        !page_js.contains("DESIGNED_UTILITY_ENTRIES"),
        "the page must not own a second Utility registry"
    );
    // The marked path exists and paints the declared mark beside the
    // structure's name.
    assert!(
        page_js.contains("markUnavailableRowHtml"),
        "the page has no marked-unavailable path"
    );

    // The production document drives the complete declared surface in order.
    let document = production_document(TopLevelContext::Patch);
    let utility: Vec<(String, String)> = surface_controls(&document, "patchUtility")
        .iter()
        .filter(|control| control.get("visible").and_then(Value::as_bool) == Some(true))
        .filter_map(|control| {
            Some((
                control.pointer("/path/controlId/id")?.as_str()?.to_owned(),
                control.get("label")?.as_str()?.to_owned(),
            ))
        })
        .collect();
    let declared_ids: Vec<String> = AUTHORED_UTILITY_ENTRIES
        .iter()
        .filter_map(|(_, driver)| driver.map(str::to_owned))
        .collect();
    assert_eq!(
        utility.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
        declared_ids,
        "the projected Utility surface must carry the declared identities in order"
    );
    assert!(
        utility.iter().all(|(_, label)| !label.trim().is_empty()),
        "every projected Utility control must carry its canonical display label"
    );
}

/// No component owns, caches, or derives application state; the page owns no
/// clock, storage, or input; and the transport renders deterministically.
fn check_no_component_owns_or_dispatches_application_state() {
    let root = repository_root();
    let sources = visual_module_sources();
    assert!(
        sources.len() >= VOCABULARY_SOURCES.len(),
        "the ownership scan read only {} vocabulary sources",
        sources.len()
    );

    let forbidden_action = format!("{}{}", "Semantic", "Action");
    let interior_mutability = [
        "static mut ",
        "OnceLock",
        "OnceCell",
        "RefCell",
        "thread_local!",
        "lazy_static",
        "Mutex<",
        "RwLock<",
        "AtomicUsize",
        "AtomicBool",
    ];
    let application_state = [
        "AppState",
        "AppLoop",
        "AudioObservationSnapshot",
        "AppEvent",
    ];

    let mut lines_read = 0_usize;
    for path in &sources {
        let source = std::fs::read_to_string(root.join(path)).expect("a visual source");
        let code = production_code(&source);
        lines_read += code.lines().count();

        assert!(
            !code.contains(&forbidden_action),
            "{path} names {forbidden_action}; a component presents the immutable view \
             data it is handed and never converts an input into an action"
        );
        for needle in interior_mutability {
            assert!(
                !code.contains(needle),
                "{path} holds `{needle}`, through which a component could cache what it \
                 was handed"
            );
        }
        for needle in application_state {
            assert!(
                !code.contains(needle),
                "{path} names `{needle}`; a component reads only the immutable view data \
                 it is given"
            );
        }
        if !path.ends_with("density.rs") {
            assert!(
                !code.contains("screen_rect"),
                "{path} reads a raw viewport size instead of asking the density policy"
            );
            assert!(
                !code.contains("ResponsiveLayoutMode::resolve"),
                "{path} resolves presentation mode from a raw width of its own"
            );
        }
    }
    assert!(
        lines_read > 1_500,
        "the ownership scan read only {lines_read} lines of the authored vocabulary"
    );

    // The page's half of the boundary: a pure render with no clock, no
    // randomness, no storage, and no input capture — keys stay Rust-side.
    // Comments narrate the rule and are not ownership, so they are stripped
    // with the same string-aware discipline the Rust guard applies.
    let page_js: String = page_source("page.js")
        .lines()
        .map(|line| strip(line, true))
        .collect::<Vec<_>>()
        .join("\n");
    let index_html = page_source("index.html");
    let gallery_js: String = page_source("gallery.js")
        .lines()
        .map(|line| strip(line, true))
        .collect::<Vec<_>>()
        .join("\n");
    // `strip` is a `//` line-comment stripper and does not see HTML comment
    // syntax, so the served document is stripped span-wise instead. Without
    // it a document whose header narrates the rule the way `page.js`'s does
    // ("no Date.now, no Math.random") would read as owning what it forbids.
    let index_html_markup = strip_html_comments(&index_html);
    // Every page source this function reads is bound, each failure naming its
    // source and its needle. `page.js` and `gallery.js` are the two scripts
    // the render path and the gallery scene serve; `index.html` is in the
    // loop because an inline `<script>` or an `on*` attribute would own a
    // clock, a store, or a fetch just as surely as a script file would, and
    // no other guard in this file would catch it. The stylesheets
    // (`tokens.css`, `page.css`, `gallery.css`) are deliberately absent:
    // every needle below is a JavaScript host API that CSS cannot name, and
    // paint is guarded by the vocabulary scans instead.
    for (name, source) in [
        ("page.js", &page_js),
        ("index.html", &index_html_markup),
        ("gallery.js", &gallery_js),
    ] {
        for needle in [
            "Date.now",
            "Math.random",
            "performance.now",
            "setInterval",
            "setTimeout",
            "localStorage",
            "sessionStorage",
            "fetch(",
            "XMLHttpRequest",
        ] {
            assert!(
                !source.contains(needle),
                "{name} owns `{needle}`, which a pure render cannot"
            );
        }
    }
    // Performance keys stay Rust-side. The focused text input owns only
    // Escape cancellation while native capture is suspended. Exempt that
    // exact handler; any other page or gallery key handler still fails.
    const NAME_CANCEL_HANDLER: &str = r#"input.addEventListener("keydown", function (event) { if (event.key === "Escape") { event.preventDefault(); emit({ kind: "cancelRename" }); } });"#;
    assert_eq!(page_js.matches(NAME_CANCEL_HANDLER).count(), 1);
    let performance_page_js = page_js.replace(NAME_CANCEL_HANDLER, "");
    for (name, source) in [
        ("page.js", &performance_page_js),
        ("index.html", &index_html),
        ("gallery.js", &gallery_js),
    ] {
        for needle in ["keydown", "keyup", "keypress"] {
            assert!(
                !source.contains(needle),
                "{name} registers a performance key handler (`{needle}`); keys are captured Rust-side"
            );
        }
    }

    // The transport's half: rendering the same immutable projection twice
    // produces byte-identical documents — nothing between the reducer and
    // the page retains or rewrites anything.
    for context in [TopLevelContext::Patch, TopLevelContext::Mixer] {
        let projection = production_projection(context);
        let render = |projection: &GraphicalShellProjection| -> String {
            let mut channel = ProjectionChannel::new();
            let mut emitted = None;
            channel
                .push(projection, |document| {
                    emitted = Some(document);
                    Ok(())
                })
                .expect("the determinism probe emit succeeds");
            serde_json::to_string(&emitted.expect("one document")).expect("the document serializes")
        };
        assert_eq!(
            render(&projection),
            render(&projection),
            "{context:?}: two renders of one projection must serialize identically"
        );
    }
}

/// Representative viewports reach all three presentation modes while sharing
/// one bounded geometry contract.
fn check_both_viewports_resolve_from_the_declared_policy() {
    for (viewport, expected) in REFERENCE_VIEWPORTS {
        assert_eq!(
            ResponsiveLayoutMode::resolve(viewport[0]),
            expected,
            "the reference {viewport:?} viewport does not resolve to {}",
            expected.canonical_name()
        );
    }

    let mut policies: BTreeSet<&str> = BTreeSet::new();
    for width in [
        320.0_f32, 800.0, 1_024.0, 1_280.0, 1_281.0, 1_366.0, 1_600.0, 1_920.0, 2_560.0, 3_840.0,
    ] {
        let policy = ResponsiveLayoutMode::resolve(width);
        policies.insert(policy.canonical_name());
    }
    assert_eq!(
        policies.len(),
        ALL_LAYOUT_MODES.len(),
        "the sweep did not reach all declared presentation modes"
    );

    assert!(ResponsiveShellContract::get()
        .all_bounds()
        .into_iter()
        .all(|(_, bounds)| bounds.is_ordered()));
    assert_eq!(
        RepresentativeViewport::StandardReference.fixture().width_px,
        1_280.0
    );

    // The production window reads the same representative-size helpers; the
    // window itself remains transport rather than layout authority.
    let _ = TauriWebviewWindow::default();
}

// ===========================================================================
// The declared checks, and the marker
// ===========================================================================

#[test]
fn selection_is_total_over_kind_and_role_with_every_control_reachable() {
    check_selection_is_total_and_every_control_reachable();
    check_the_selector_is_exhaustive_rather_than_defaulted();
}

#[test]
fn the_production_projection_carries_the_kinds_this_target_can_drive() {
    check_the_production_projection_carries_the_kinds_this_target_can_drive();
}

#[test]
fn every_projected_control_renders_through_the_document() {
    let driven = check_every_projected_control_renders_through_the_document();
    assert!(
        driven >= 60,
        "only {driven} projected controls were driven through the rendered document"
    );
}

#[test]
fn every_control_declares_its_applicable_states() {
    check_state_applicability_declarations();
}

#[test]
fn every_shipped_region_is_a_declared_band() {
    check_every_region_is_a_declared_band();
    check_the_mixer_column_anatomy_is_declared_and_driven();
}

#[test]
fn no_visual_decision_survives_outside_the_visual_module() {
    let (sources, lines, violations) = scan_delivered_tree();
    assert!(
        sources.len() >= 40,
        "the guard scanned only {} sources",
        sources.len()
    );
    assert!(lines > 20_000, "the guard read only {lines} lines");
    for required in [
        "src/shell/webview/window.rs",
        "src/shell/webview/projection_channel.rs",
        "src/testing/component_gallery_scene.rs",
        "src/shell/standalone_application.rs",
        "src/control/state_projector.rs",
    ] {
        assert!(
            sources.iter().any(|path| path == required),
            "the guard did not scan {required}"
        );
    }
    assert!(
        !sources.iter().any(|path| {
            path.starts_with(VISUAL_MODULE) || VOCABULARY_SOURCES.contains(&path.as_str())
        }),
        "the guard scanned the authored vocabulary, which is the one place decisions belong"
    );
    assert!(
        violations.is_empty(),
        "{} visual decision(s) survive outside the authored vocabulary:\n{}",
        violations.len(),
        violations
            .iter()
            .map(VisualDecision::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn the_webview_window_holds_no_paint_decision() {
    check_the_webview_window_holds_no_paint_decision();
}

/// A guard that has never failed is indistinguishable from no guard: a paint
/// decision planted in a webview transport source must be reported.
#[test]
fn the_transport_guard_reports_a_planted_decision() {
    let planted = concat!(
        "use ",
        "efr",
        "ame::eg",
        "ui::Color32;\n",
        "pub const ACCENT: Color32 = Color32::from_rgb(0x65, 0xe5, 0xff);\n",
        "pub fn seat_side_region() -> f32 {\n",
        "    420.0\n",
        "}\n",
    );
    let violations = webview_transport_violations("src/shell/webview/planted.rs", planted);
    let joined = violations.join("\n");
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("color")),
        "the transport guard did not report the planted color decision:\n{joined}"
    );
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("Color32")),
        "the transport guard did not report the paint API name:\n{joined}"
    );
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("side track")),
        "the transport guard did not report the planted band extent:\n{joined}"
    );

    // And the shipped transport sources pass the same function, so what the
    // planted test proves failing is exactly what the shipped check runs.
    let root = repository_root();
    for path in WEBVIEW_TRANSPORT_SOURCES {
        let source = std::fs::read_to_string(root.join(path)).expect("a transport source");
        assert!(
            webview_transport_violations(path, &source).is_empty(),
            "{path} carries a paint or layout decision"
        );
    }
}

#[test]
fn the_visual_decision_guard_reads_the_delivered_tree() {
    let (sources, lines, _) = scan_delivered_tree();
    assert!(sources.len() >= 40, "{} sources", sources.len());
    assert!(lines > 20_000, "{lines} lines");
}

/// Each family the requirement names is planted, and each must be reported
/// with file, line, and kind. The band-height plant is the constant the
/// retired adapter used to carry.
#[test]
fn the_visual_decision_guard_reports_a_planted_decision() {
    let planted = concat!(
        "use ",
        "efr",
        "ame::eg",
        "ui::Color32;\n",
        "pub const ACCENT: Color32 = Color32::from_rgb(0x65, 0xe5, 0xff);\n",
        "pub fn paint(ui: &mut ",
        "eg",
        "ui::Ui) {\n",
        "    ui.add_space(12.0);\n",
        "    let id = FontId::new(14.0, FontFamily::Proportional);\n",
        "    let hex = Color32::from_hex(\"#0c1015\");\n",
        "}\n",
        "const WORKSPACE_TITLE_ROW_PX: f32 = 48.0;\n",
    );
    let found = scan_visual_decisions("src/adapter/planted.rs", planted);
    let reported: Vec<String> = found.iter().map(VisualDecision::report).collect();
    let joined = reported.join("\n");

    for (kind, line) in [
        ("color", 2_usize),
        ("palette", 2),
        ("spacing", 4),
        ("type size", 5),
        ("color", 6),
        ("palette", 6),
        ("band height", 8),
    ] {
        assert!(
            found
                .iter()
                .any(|decision| decision.kind == kind && decision.line == line),
            "the guard did not report a {kind} decision on line {line}:\n{joined}"
        );
    }
    assert!(
        reported
            .iter()
            .all(|report| report.starts_with("src/adapter/planted.rs:")),
        "every report must name its file so a failure is actionable:\n{joined}"
    );

    // The retired palette is caught too, spelled either way.
    assert!(
        scan_visual_decisions(
            "src/adapter/r.rs",
            "let f = Color32::from_rgb(110, 205, 174);\n"
        )
        .iter()
        .any(|decision| decision.kind == "color"),
        "the guard accepted a raw-channel color"
    );
    assert!(
        scan_visual_decisions("src/adapter/r.rs", "const OLD: &str = \"#6ecdae\";\n")
            .iter()
            .any(|decision| decision.kind == "palette"),
        "the guard accepted the retired focus green spelled as hex"
    );
    // The side-region split width is a layout decision wherever it is
    // spelled.
    assert!(
        scan_visual_decisions(
            "src/adapter/r.rs",
            concat!("use ", "efr", "ame::eg", "ui;\nlet side = 420.0;\n")
        )
        .iter()
        .any(|decision| decision.kind == "band height"),
        "the guard accepted the declared side-region width"
    );
}

/// The guard does not flag what the vocabulary permits.
#[test]
fn the_visual_decision_guard_allows_what_the_vocabulary_permits() {
    let permitted = concat!(
        "use crate::shell::tokens::{SemanticColor, SpacingStep, MIN_INTERACTIVE_TARGET_PX};\n",
        "pub fn paint(ui: &mut Ui, contract: &ResponsiveShellContract) {\n",
        "    ui.add_space(SpacingStep::S12.resolve());\n",
        "    let clear = Color32::TRANSPARENT;\n",
        "    let id = FontId::new(style.metrics().size_px, family_for(style));\n",
        "    let corner = CornerRadius::same(Radius::Small.resolve() as u8);\n",
        "    painter.rect_filled(row, 0.0, SemanticColor::BgPanel.resolve());\n",
        "    let band = contract.context_line.preferred_px;\n",
        "    let button = Button::new(label).min_size(vec2(0.0, MIN_INTERACTIVE_TARGET_PX));\n",
        "    let label = \"PATCH 01 · Lead\";\n",
        "}\n",
    );
    let found = scan_visual_decisions("src/adapter/permitted.rs", permitted);
    assert!(
        found.is_empty(),
        "the guard flagged a permitted construction:\n{}",
        found
            .iter()
            .map(VisualDecision::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn a_designed_structure_with_no_view_data_is_marked_rather_than_invented() {
    check_a_designed_structure_with_no_view_data_is_marked();
}

#[test]
fn no_component_owns_caches_or_dispatches_application_state() {
    check_no_component_owns_or_dispatches_application_state();
}

#[test]
fn both_authored_viewports_resolve_from_the_declared_policy() {
    check_both_viewports_resolve_from_the_declared_policy();
}

/// The declared acceptance target.
///
/// Every check above runs here, in order, and the marker
/// `validation.component_composition` asserts on is printed strictly after
/// the last of them returns. A failing check panics before the print, so the
/// marker cannot appear on a red run.
#[test]
fn component_composition_acceptance() {
    check_selection_is_total_and_every_control_reachable();
    check_the_selector_is_exhaustive_rather_than_defaulted();
    check_the_production_projection_carries_the_kinds_this_target_can_drive();
    let controls_driven = check_every_projected_control_renders_through_the_document();
    check_state_applicability_declarations();

    check_every_region_is_a_declared_band();
    check_the_mixer_column_anatomy_is_declared_and_driven();

    let (sources, lines, violations) = scan_delivered_tree();
    assert!(
        sources.len() >= 40 && lines > 20_000,
        "the guard read too little"
    );
    assert!(
        violations.is_empty(),
        "{} visual decision(s) survive outside the authored vocabulary:\n{}",
        violations.len(),
        violations
            .iter()
            .map(VisualDecision::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
    check_the_webview_window_holds_no_paint_decision();

    check_a_designed_structure_with_no_view_data_is_marked();
    check_no_component_owns_or_dispatches_application_state();
    check_both_viewports_resolve_from_the_declared_policy();

    println!(
        "CREST_COMPONENT_COMPOSITION_OBSERVATION controls={} compositions={} kinds={} roles={} \
         pairs={} controls_driven={} states={} sources_scanned={} lines_scanned={}",
        COMPONENT_CONTROL_COUNT,
        SHELL_COMPOSITION_COUNT,
        SEMANTIC_CONTROL_KIND_COUNT,
        PRESENTATION_ROLE_COUNT,
        SEMANTIC_CONTROL_KIND_COUNT * PRESENTATION_ROLE_COUNT,
        controls_driven,
        COMPONENT_STATE_COUNT,
        sources.len(),
        lines,
    );
    println!("{ACCEPTANCE_MARKER}");
}
