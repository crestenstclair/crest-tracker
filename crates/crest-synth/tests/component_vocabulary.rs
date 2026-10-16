//! Measured proof of the authored component vocabulary, on the webview
//! render path.
//!
//! Realizes `asset.ComponentVocabularyAcceptanceTests` and the declared
//! project validation `validation.component_vocabulary`, which asserts exit
//! code 0 and the exact marker [`ACCEPTANCE_MARKER`] in stdout.
//!
//! Retargeted by mission webview-shell-cutover-01KZAC7Q WP05 (T020): the
//! retired shape-stream measurement is gone. The render path this target now
//! measures is the webview one — authored Rust value → generated token
//! table (`token_export::tokens_css`) → committed `webview-page/tokens.css`
//! → the committed page's usage of those tokens. Values are compared, not
//! names:
//!
//! 1. **The expected values are written independently of the thing under
//!    test.** [`AUTHORED_COLORS`], [`AUTHORED_TYPE_STYLES`],
//!    [`AUTHORED_SPACING`], and [`AUTHORED_RADII`] are transcribed from
//!    `DESIGN.md` § Colors and § Type and geometry — the as-built token
//!    reference —
//!    not read back from `src/shell/visual/token.rs` or from the generator.
//!    The generated stylesheet must spell each authored value verbatim
//!    (`--color-accent-focus: #65e5ff`), so a vocabulary that drifts from
//!    the design and a generator that drifts from the vocabulary both fail
//!    here.
//! 2. **The comparison follows the value to where the page reads it.** The
//!    committed token table must be byte-fresh generator output
//!    (`committed_tokens_are_fresh` — the file the production window embeds
//!    and serves), and the committed page must actually resolve the tokens
//!    it paints with: the composition stylesheet spells no color and no
//!    extent of its own outside its one declared fader-geometry block.
//!
//! The DOM-level measurement of the same values on the real rendered page —
//! double-render determinism, painted state treatments, seated band
//! geometry — is `tests/webview_projection_shell.rs` (T024), gated on a
//! live window because a DOM needs one; this target is its headless value
//! half and can fail independently of it.
//!
//! # The guard that has to be able to fail
//!
//! [`scan_source`] is the literal-absence guard for `NFR-002`, unchanged in
//! scope: no raw visual value outside the vocabulary module in any adapter,
//! shell, or scene source — which now includes the webview shell sources.
//! [`the_literal_guard_reports_a_planted_literal`] plants each family and
//! asserts the guard reports it with file, line, and kind;
//! [`the_literal_guard_reads_the_delivered_tree`] asserts the scan read a
//! non-trivial number of files and lines, so it cannot pass by scanning
//! nothing. The page-side twin
//! ([`check_page_sources_spell_no_visual_value`]) holds the committed page
//! and the committed gallery pair to the same rule: no hex color, no color
//! constructor, and no raw pixel extent outside the declared fader block and
//! the two narrated gallery allowances.
//!
//! # Recorded limitations
//!
//! - **`Selected` is production-unprojected on the webview surfaces.** The
//!   page derives row states from the document (`controlState`) and fader
//!   states from the mixer toggles (`faderState`); neither can produce the
//!   `Selected` treatment because no production projection carries it. The
//!   derivable page state sets are pinned exactly
//!   ([`PAGE_ROW_STATES`], [`PAGE_FADER_STATES`]), so a page that gains or
//!   loses a derivable state fails and this recorded limitation must be
//!   revisited rather than silently drifting.
//! - **Spacing steps and radii are compared at the token table.** The page
//!   resolves them (`var(--space-12)`, `var(--radius-small)`), and whether a
//!   resolved token produced the authored on-screen gap is a DOM question
//!   the live target answers.
//! - **Per-page painted specimen coverage is proven elsewhere.** The
//!   gallery's paint pass is private to
//!   `src/testing/component_gallery_scene.rs`; this target proves the page
//!   and digit vocabulary is total in both directions and leaves the painted
//!   specimens to that module's own tests, which the declared `test`
//!   validation runs.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crest_synth::adapter::production_instruments::{
    production_capability_registry, production_soundfont_capability,
};
use crest_synth::control::app_event::{AppEvent, Direction};
use crest_synth::control::app_state::AppState;
use crest_synth::control::state_projector::StateProjector;
use crest_synth::control::TopLevelContext;
use crest_synth::kernel::midi_channel::MidiChannel;
use crest_synth::kernel::patch_id::PatchId;
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::shell::component_state::{
    ComponentState, NonColorSignal, ALL_COMPONENT_STATES, COMPONENT_STATE_COUNT,
    LOADING_PROGRESS_WORDS,
};
use crest_synth::shell::density::{ResponsiveShellContract, ALL_LAYOUT_MODES};
use crest_synth::shell::tokens::{
    Radius, SemanticColor, SpacingStep, TypeStyle, ALL_COLORS, ALL_RADII, ALL_SPACING_STEPS,
    ALL_TYPE_STYLES, ALL_WEIGHTS, FOCUS_HALO_OPACITY, FOCUS_HALO_RADIUS_PX, FOCUS_HALO_SPREAD_PX,
    KEYLINE_EMPHASIS_PX, KEYLINE_RESTING_PX, MIN_INTERACTIVE_TARGET_PX,
};
use crest_synth::shell::typeface::{family_name, AuthoredTypeface, TypefaceError, AUTHORED_FAMILY};
use crest_synth::shell::webview::token_export;
use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
use crest_synth::synth::Patch;
use crest_synth::testing::automatic_midi_test::create_soundfont_config;
use crest_synth::testing::component_gallery_scene::{
    ComponentGalleryPage, PageStep, ALL_GALLERY_PAGES, GALLERY_DIGIT_BINDING_COUNT,
    GALLERY_PAGE_COUNT,
};
use serde_json::Value;

/// The exact string `validation.component_vocabulary` asserts on stdout.
///
/// Printed by [`component_vocabulary_acceptance`] and nowhere else, strictly
/// after every declared check has run and passed.
const ACCEPTANCE_MARKER: &str = "CREST_ACCEPTANCE component_vocabulary passed";

// ===========================================================================
// The authored table, transcribed from DESIGN.md
// ===========================================================================
//
// `DESIGN.md` is the as-built authority for the token values. The values below
// are copied from it, not from the vocabulary, so a token change that drifts
// from the recorded contract fails here rather than agreeing with itself.

/// Every authored color: the vocabulary's role, the canonical name the design
/// file publishes, and the authored value as `DESIGN.md` § Colors writes it.
const AUTHORED_COLORS: [(SemanticColor, &str, &str); 17] = [
    (SemanticColor::BgCanvas, "color/bg/canvas", "#0c1015"),
    (SemanticColor::BgSurface, "color/bg/surface", "#121821"),
    (SemanticColor::BgPanel, "color/bg/panel", "#17202a"),
    (SemanticColor::BgElevated, "color/bg/elevated", "#1d2733"),
    (SemanticColor::BgSelected, "color/bg/selected", "#2a3745"),
    (
        SemanticColor::BorderDefault,
        "color/border/default",
        "#2a3745",
    ),
    (
        SemanticColor::BorderStrong,
        "color/border/strong",
        "#415166",
    ),
    (SemanticColor::TextPrimary, "color/text/primary", "#f2f6f8"),
    (
        SemanticColor::TextSecondary,
        "color/text/secondary",
        "#b8c4d1",
    ),
    (SemanticColor::TextMuted, "color/text/muted", "#6f8095"),
    (SemanticColor::AccentFocus, "color/accent/focus", "#65e5ff"),
    (
        SemanticColor::AccentAdjust,
        "color/accent/adjust",
        "#ffb454",
    ),
    (
        SemanticColor::AccentPositive,
        "color/accent/positive",
        "#58e887",
    ),
    (
        SemanticColor::AccentWarning,
        "color/accent/warning",
        "#ff6868",
    ),
    (
        SemanticColor::AccentInstrument,
        "color/accent/instrument/plates",
        "#b894ff",
    ),
    (SemanticColor::AccentPatch, "color/accent/patch", "#ff6fbe"),
    (
        SemanticColor::AccentChorus,
        "color/accent/chorus",
        "#f6f178",
    ),
];

/// Every authored type style: the vocabulary's style, the canonical name, and
/// `DESIGN.md` § Type and geometry's size / line / weight / tracking row.
const AUTHORED_TYPE_STYLES: [(TypeStyle, &str, f32, f32, &str, u16, f32); 8] = [
    (
        TypeStyle::DisplayScreen,
        "Display/Screen",
        32.0,
        40.0,
        "SemiBold",
        600,
        0.4,
    ),
    (
        TypeStyle::HeadingSection,
        "Heading/Section",
        18.0,
        24.0,
        "SemiBold",
        600,
        1.4,
    ),
    (
        TypeStyle::HeadingPanel,
        "Heading/Panel",
        14.0,
        20.0,
        "Bold",
        700,
        1.2,
    ),
    (
        TypeStyle::BodyDefault,
        "Body/Default",
        15.0,
        22.0,
        "Regular",
        400,
        0.0,
    ),
    (
        TypeStyle::BodyCompact,
        "Body/Compact",
        13.0,
        18.0,
        "Regular",
        400,
        0.0,
    ),
    (
        TypeStyle::LabelControl,
        "Label/Control",
        12.0,
        16.0,
        "Medium",
        500,
        0.8,
    ),
    (
        TypeStyle::CodeValue,
        "Code/Value",
        14.0,
        20.0,
        "SemiBold",
        600,
        0.2,
    ),
    (
        TypeStyle::InstructionHint,
        "Instruction/Hint",
        11.0,
        16.0,
        "Medium",
        500,
        0.8,
    ),
];

/// `DESIGN.md`: "Spacing: 4, 8, 12, 16, 24, 32 px."
const AUTHORED_SPACING: [(SpacingStep, &str, f32); 6] = [
    (SpacingStep::S4, "space/4", 4.0),
    (SpacingStep::S8, "space/8", 8.0),
    (SpacingStep::S12, "space/12", 12.0),
    (SpacingStep::S16, "space/16", 16.0),
    (SpacingStep::S24, "space/24", 24.0),
    (SpacingStep::S32, "space/32", 32.0),
];

/// `DESIGN.md`: "Radius: 0, 4, 8 px."
const AUTHORED_RADII: [(Radius, f32); 3] = [
    (Radius::None, 0.0),
    (Radius::Small, 4.0),
    (Radius::Large, 8.0),
];

/// `DESIGN.md`: "Minimum interactive target: 48 px."
const AUTHORED_MIN_TARGET_PX: f32 = 48.0;
/// `DESIGN.md`: "Resting keyline: 1 px."
const AUTHORED_KEYLINE_RESTING_PX: f32 = 1.0;
/// `DESIGN.md`: "Focus: 3 px cyan keyline"; "Adjustment: 3 px amber keyline."
const AUTHORED_KEYLINE_EMPHASIS_PX: f32 = 3.0;
/// `DESIGN.md`: "halo radius 8, spread 1, opacity 0.28."
const AUTHORED_HALO_RADIUS_PX: f32 = 8.0;
const AUTHORED_HALO_SPREAD_PX: f32 = 1.0;
const AUTHORED_HALO_OPACITY: f32 = 0.28;

/// `DESIGN.md`: "Mixer fader specimen: 14 px track, 8 px fill, 3 px bottom
/// shoulder, 34×6 px cap, 2 px rounding." Transcribed here, not read back
/// from the vocabulary, so a drifted fader token fails against the design
/// rather than against itself (WP07 T027, NFR-004).
const AUTHORED_FADER: [(&str, f32); 6] = [
    ("--fader-track-width", 14.0),
    ("--fader-fill-width", 8.0),
    ("--fader-shoulder", 3.0),
    ("--fader-cap-width", 34.0),
    ("--fader-cap-height", 6.0),
    ("--fader-rounding", 2.0),
];

/// The palette the shell painted before the component missions.
///
/// These are what regression looks like: not an arbitrary wrong color, but the
/// specific values the shell used to carry. `#6ecdae` is the focus green the
/// authored cyan replaces.
const RETIRED_COLORS: [(&str, &str); 7] = [
    ("focus green", "#6ecdae"),
    ("canvas", "#101216"),
    ("surface", "#181b20"),
    ("elevated", "#1d2127"),
    ("text primary", "#e6eaef"),
    ("text muted", "#969ea9"),
    ("adjust amber", "#e8ae4c"),
];

/// Parses one authored `#rrggbb` string into its channels.
///
/// Written here rather than reached for from the vocabulary: the vocabulary
/// declares its colors as `Color32::from_rgb(0x.., 0x.., 0x..)`, so comparing
/// against a value produced by that same expression would compare the
/// vocabulary to itself.
fn authored_rgb(hex: &str) -> [u8; 3] {
    let digits = hex
        .strip_prefix('#')
        .unwrap_or_else(|| panic!("{hex} is not written as #rrggbb"));
    assert_eq!(digits.len(), 6, "{hex} is not six hex digits");
    let channel = |at: usize| {
        u8::from_str_radix(&digits[at..at + 2], 16)
            .unwrap_or_else(|_| panic!("{hex} carries a non-hex digit"))
    };
    [channel(0), channel(2), channel(4)]
}

// ===========================================================================
// The committed page sources — the webview render path's delivery surface
// ===========================================================================

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn page_source(name: &str) -> String {
    let path = repository_root().join("webview-page").join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} must be readable: {error}", path.display()))
}

/// The in-test transcription of the mechanical name transform the generator
/// documents (`token_export`): slash-separated canonical names lowercase and
/// swap `/` for `-`. Written here so the expected property names do not come
/// from the generator's own helpers.
fn kebab_canonical(canonical: &str) -> String {
    canonical.to_lowercase().replace('/', "-")
}

/// Kebab-cases a CamelCase policy name (`SteamDeck` → `steam-deck`).
/// Formats an authored pixel value as the generator's documented CSS form.
fn css_px(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{}px", value as i64)
    } else {
        format!("{value}px")
    }
}

// ===========================================================================
// A production document with an in-flight structural edit
// ===========================================================================

/// The production reducer with one installed patch.
fn installed_state() -> AppState {
    let provider = production_soundfont_capability().expect("the production SoundFont capability");
    let config =
        create_soundfont_config(&provider, SoundFontInstrument::new(0, 40, false).unwrap())
            .expect("an installed instrument configuration");
    let patch = Patch::new(
        PatchId::new(1).unwrap(),
        "Component Vocabulary".to_owned(),
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

/// The serialized PATCH document with the engine row preparing — the state
/// whose lifecycle word the page's loading treatment paints.
fn preparing_patch_document() -> Value {
    let mut state = installed_state();
    state
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .expect("selecting PATCH is accepted");
    state
        .apply(AppEvent::Adjust(Direction::Right))
        .expect("requesting the next engine is accepted");
    let projection = StateProjector::new()
        .project_with_shell(&state)
        .expect("the production projector accepts the preparing state")
        .3;
    serde_json::to_value(projection.semantic_model()).expect("the projector's model serializes")
}

// ===========================================================================
// The literal-absence guard
// ===========================================================================

/// One visual literal found outside the vocabulary module.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct LiteralViolation {
    file: String,
    line: usize,
    kind: &'static str,
    evidence: String,
}

impl LiteralViolation {
    /// The actionable one-line report a failure prints.
    fn report(&self) -> String {
        format!(
            "{}:{}: {} literal outside the vocabulary — {}",
            self.file, self.line, self.kind, self.evidence
        )
    }
}

/// The one file allowed to spell a raw visual value.
const VOCABULARY_FILE: &str = "src/shell/tokens.rs";

/// The trees the guard scans: every adapter, view, scene, and shell source —
/// including the webview shell sources.
const SCAN_ROOTS: [&str; 3] = ["src/adapter", "src/shell", "src/testing"];

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
        // A hex literal never parses as a decimal; treat a nonzero hex as a
        // literal and `0x0` as zero.
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
    // An unterminated call means the argument list wraps onto the next source
    // line; what was collected is still worth inspecting.
    arguments.push(current);
    arguments
}

/// Scans one source for visual literals.
///
/// Exposed as a plain function over text so the guard can be fed a planted
/// sample and shown failing, which is what makes it a guard rather than a
/// decoration.
fn scan_source(path: &str, source: &str) -> Vec<LiteralViolation> {
    let mut violations = Vec::new();
    let authored_hexes: Vec<(&str, &str)> = AUTHORED_COLORS
        .iter()
        .map(|(_, name, hex)| (*name, *hex))
        .chain(RETIRED_COLORS.iter().map(|(name, hex)| (*name, *hex)))
        .collect();

    for (index, raw) in source.lines().enumerate() {
        let line = index + 1;
        let code = strip(raw, false);
        let lowered = strip(raw, true).to_ascii_lowercase();

        let mut flag = |kind: &'static str, evidence: String| {
            violations.push(LiteralViolation {
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
                                "{}{} builds a color from raw channels",
                                needle,
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
            let arguments = call_arguments(&code, open);
            if arguments.iter().any(|argument| argument.contains('"')) {
                flag("color", "from_hex(\"…\") spells a color".to_owned());
            }
        }

        for (name, hex) in &authored_hexes {
            let bare = hex.trim_start_matches('#');
            let [r, g, b] = authored_rgb(hex);
            let channels = format!("0x{r:02x}, 0x{g:02x}, 0x{b:02x}");
            if lowered.contains(&hex.to_ascii_lowercase())
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
    }
    violations.sort();
    violations
}

/// Every source the guard scans, repository-relative and sorted.
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
    for scan_root in SCAN_ROOTS {
        walk(&root.join(scan_root), &mut absolute);
    }
    let mut relative: Vec<String> = absolute
        .into_iter()
        .map(|path| {
            path.strip_prefix(&root)
                .expect("a scanned source lives under the repository root")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .filter(|path| path != VOCABULARY_FILE)
        .collect();
    relative.sort();
    relative
}

/// Runs the guard over the delivered tree, returning what it read and what it
/// found.
fn scan_delivered_tree() -> (Vec<String>, usize, Vec<LiteralViolation>) {
    let root = repository_root();
    let sources = scanned_sources();
    let mut lines_read = 0;
    let mut violations = Vec::new();
    for path in &sources {
        let source = std::fs::read_to_string(root.join(path))
            .unwrap_or_else(|error| panic!("{path} is unreadable: {error}"));
        lines_read += source.lines().count();
        violations.extend(scan_source(path, &source));
    }
    violations.sort();
    (sources, lines_read, violations)
}

// ===========================================================================
// T034 — authored-value fidelity (declaration and token table)
// ===========================================================================

/// Every declared value equals its authored counterpart, and the vocabulary
/// holds exactly the declared number of them.
///
/// Both directions are asserted: every authored entry has a declaration, and
/// every declaration has an authored entry. A token dropped from the
/// vocabulary and a token added to it both fail here rather than passing by
/// absence.
fn check_declared_values_match_the_authored_table() {
    assert_eq!(
        ALL_COLORS.len(),
        AUTHORED_COLORS.len(),
        "the vocabulary declares {} colors where DESIGN.md authors {}",
        ALL_COLORS.len(),
        AUTHORED_COLORS.len()
    );
    for (role, name, hex) in AUTHORED_COLORS {
        let resolved = role.resolve();
        assert_eq!(
            [resolved.r(), resolved.g(), resolved.b()],
            authored_rgb(hex),
            "{name} resolves to #{:02x}{:02x}{:02x} where DESIGN.md authors {hex}",
            resolved.r(),
            resolved.g(),
            resolved.b()
        );
        // Opacity is structural: `AuthoredRgb` carries no alpha channel, so
        // a translucent authored role is unrepresentable by construction.
        assert!(
            ALL_COLORS.contains(&role),
            "{name} is authored but absent from ALL_COLORS"
        );
    }
    let authored_roles: BTreeSet<&str> = AUTHORED_COLORS.iter().map(|(_, name, _)| *name).collect();
    assert_eq!(
        authored_roles.len(),
        AUTHORED_COLORS.len(),
        "the authored table names a color twice"
    );
    for role in ALL_COLORS {
        assert!(
            authored_roles.contains(role.canonical_name()),
            "{} is declared but DESIGN.md authors no such color",
            role.canonical_name()
        );
    }

    assert_eq!(
        ALL_TYPE_STYLES.len(),
        AUTHORED_TYPE_STYLES.len(),
        "the vocabulary declares {} type styles where DESIGN.md authors {}",
        ALL_TYPE_STYLES.len(),
        AUTHORED_TYPE_STYLES.len()
    );
    for (style, name, size, line, weight_name, weight_numeric, tracking) in AUTHORED_TYPE_STYLES {
        let metrics = style.metrics();
        assert_eq!(metrics.size_px, size, "{name} size");
        assert_eq!(metrics.line_height_px, line, "{name} line height");
        assert_eq!(metrics.tracking_px, tracking, "{name} tracking");
        assert_eq!(
            metrics.weight.numeric(),
            weight_numeric,
            "{name} numeric weight"
        );
        assert_eq!(
            family_name(metrics.weight),
            format!("{AUTHORED_FAMILY} {weight_name}"),
            "{name} resolves to the wrong face"
        );
        assert!(
            ALL_TYPE_STYLES.contains(&style),
            "{name} is authored but absent from ALL_TYPE_STYLES"
        );
    }

    assert_eq!(ALL_SPACING_STEPS.len(), AUTHORED_SPACING.len());
    for (step, name, expected) in AUTHORED_SPACING {
        assert_eq!(step.resolve(), expected, "{name}");
        assert!(ALL_SPACING_STEPS.contains(&step), "{name} is not declared");
    }

    assert_eq!(ALL_RADII.len(), AUTHORED_RADII.len());
    for (radius, expected) in AUTHORED_RADII {
        assert_eq!(radius.resolve(), expected, "{radius:?} radius");
        assert!(ALL_RADII.contains(&radius));
    }

    assert_eq!(MIN_INTERACTIVE_TARGET_PX, AUTHORED_MIN_TARGET_PX);
    assert_eq!(KEYLINE_RESTING_PX, AUTHORED_KEYLINE_RESTING_PX);
    assert_eq!(KEYLINE_EMPHASIS_PX, AUTHORED_KEYLINE_EMPHASIS_PX);
    assert_eq!(FOCUS_HALO_RADIUS_PX, AUTHORED_HALO_RADIUS_PX);
    assert_eq!(FOCUS_HALO_SPREAD_PX, AUTHORED_HALO_SPREAD_PX);
    assert_eq!(FOCUS_HALO_OPACITY, AUTHORED_HALO_OPACITY);
    assert_eq!(ALL_WEIGHTS.len(), 4);

    // No declared role resolves to a retired value.
    for (name, hex) in RETIRED_COLORS {
        let retired = authored_rgb(hex);
        for role in ALL_COLORS {
            let resolved = role.resolve();
            assert_ne!(
                [resolved.r(), resolved.g(), resolved.b()],
                retired,
                "{} still resolves to the retired {name} {hex}",
                role.canonical_name()
            );
        }
    }
}

/// The generated token table spells every authored value, verbatim — the
/// value half of "authored Rust value → generated token → page usage".
///
/// The expected declaration text is assembled from the DESIGN.md
/// transcription above, never from the generator's helpers, so a generator
/// that reformats, drops, or rewrites a value fails against the authored
/// text. Counts and injectivity close the sweep: a token silently added or
/// dropped changes the declaration count.
fn check_generated_tokens_carry_every_authored_value() {
    let generated = token_export::tokens_css();

    // The committed table — the file the production window embeds and the
    // page resolves — is byte-fresh generator output.
    token_export::committed_tokens_are_fresh(&page_source("tokens.css"))
        .expect("committed webview-page/tokens.css must match the authored vocabulary");

    for (_, name, hex) in AUTHORED_COLORS {
        let needle = format!("  --{}: {hex};\n", kebab_canonical(name));
        assert!(
            generated.contains(&needle),
            "the token table must declare {name} at the authored {hex}"
        );
    }
    for (_, name, size, line, _, weight_numeric, tracking) in AUTHORED_TYPE_STYLES {
        let kebab = kebab_canonical(name);
        for (metric, expected) in [
            ("size", css_px(size)),
            ("line", css_px(line)),
            ("weight", weight_numeric.to_string()),
            ("tracking", css_px(tracking)),
        ] {
            let needle = format!("  --type-{kebab}-{metric}: {expected};\n");
            assert!(
                generated.contains(&needle),
                "the token table must declare {name} {metric} at the authored {expected}"
            );
        }
    }
    for (_, name, value) in AUTHORED_SPACING {
        let needle = format!("  --{}: {};\n", kebab_canonical(name), css_px(value));
        assert!(
            generated.contains(&needle),
            "the token table must declare {name} at the authored {value} px"
        );
    }
    for (radius, value) in AUTHORED_RADII {
        let needle = format!(
            "  --radius-{}: {};\n",
            format!("{radius:?}").to_lowercase(),
            css_px(value)
        );
        assert!(
            generated.contains(&needle),
            "the token table must declare {radius:?} at the authored {value} px"
        );
    }
    for (property, value) in AUTHORED_FADER {
        let needle = format!("  {property}: {};\n", css_px(value));
        assert!(
            generated.contains(&needle),
            "the token table must declare {property} at the authored {value} px"
        );
    }
    for (property, value) in [
        ("--keyline-resting", css_px(AUTHORED_KEYLINE_RESTING_PX)),
        ("--keyline-emphasis", css_px(AUTHORED_KEYLINE_EMPHASIS_PX)),
        ("--min-interactive-target", css_px(AUTHORED_MIN_TARGET_PX)),
        ("--focus-halo-radius", css_px(AUTHORED_HALO_RADIUS_PX)),
        ("--focus-halo-spread", css_px(AUTHORED_HALO_SPREAD_PX)),
        ("--focus-halo-opacity", format!("{AUTHORED_HALO_OPACITY}")),
    ] {
        let needle = format!("  {property}: {value};\n");
        assert!(
            generated.contains(&needle),
            "the token table must declare {property} at the authored {value}"
        );
    }

    // The retired palette is nowhere in the table.
    for (name, hex) in RETIRED_COLORS {
        assert!(
            !generated.to_lowercase().contains(hex),
            "the retired {name} {hex} survives in the generated table"
        );
    }

    // Exactly the declared vocabulary, no more and no fewer, each property
    // once.
    let root = generated
        .split_once("}\n")
        .map(|(root, _)| root)
        .expect("the generated root block closes");
    let properties: Vec<&str> = root
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            if !trimmed.starts_with("--") {
                return None;
            }
            trimmed.split(':').next()
        })
        .collect();
    let expected_count = AUTHORED_COLORS.len()
        + AUTHORED_TYPE_STYLES.len() * 4
        + AUTHORED_SPACING.len()
        + AUTHORED_RADII.len()
        + 6
        + AUTHORED_FADER.len()
        + ResponsiveShellContract::get().all_bounds().len() * 3
        + 9;
    assert_eq!(
        properties.len(),
        expected_count,
        "the token table declares {} custom properties where the authored vocabulary \
         yields {expected_count}",
        properties.len()
    );
    let unique: BTreeSet<&str> = properties.iter().copied().collect();
    assert_eq!(
        unique.len(),
        properties.len(),
        "custom property names must stay injective over the generated table"
    );
}

// ===========================================================================
// T036 — responsive geometry and page usage
// ===========================================================================

/// The responsive bounds clear the interactive minimum, reach the generated
/// token table, and are consumed by the page composition.
fn check_responsive_geometry_reaches_the_page() {
    let generated = token_export::tokens_css();
    let page_css = page_source("page.css");
    let contract = ResponsiveShellContract::get();
    assert!(contract.row_height.minimum_px >= AUTHORED_MIN_TARGET_PX);
    assert!(contract.mixer_column.minimum_px >= AUTHORED_MIN_TARGET_PX);
    for (name, bounds) in contract.all_bounds() {
        assert!(bounds.is_ordered(), "{name} bounds must be ordered");
        let name = name.replace(' ', "-");
        for (suffix, value) in [
            ("min", bounds.minimum_px),
            ("preferred", bounds.preferred_px),
            ("max", bounds.maximum_px),
        ] {
            let property = format!("--shell-{name}-{suffix}");
            let needle = format!("  {property}: {};\n", css_px(value));
            assert!(
                generated.contains(&needle),
                "the token table must declare {property} at {value} px"
            );
        }
    }

    // The page composes from bounded tracks, structural grid properties, and
    // the shared interactive target floor.
    for usage in [
        "var(--shell-mixer-column-min)",
        "var(--workspace-columns)",
        "var(--workspace-areas)",
        "var(--shell-side-track-min)",
        "grid-template-columns: var(--overview-columns)",
        "grid-template-columns: minmax(0, 1fr)",
        "min-height: var(--min-interactive-target)",
    ] {
        assert!(
            page_css.contains(usage),
            "page.css must resolve {usage} rather than restating the geometry"
        );
    }
    assert!(
        generated.contains("  --overview-columns: minmax(0, 1fr);\n"),
        "the generated Overview contract must retain one vertical section track"
    );
}

// ===========================================================================
// The page-side literal guard
// ===========================================================================

/// The committed page spells no visual value of its own: no hex color, no
/// color constructor, and no raw pixel extent outside the one declared
/// fader-geometry block. The page paints with resolved tokens or it does not
/// paint. The committed gallery pair (`gallery.css`/`gallery.js`) is held to
/// the same rule, with exactly two declared allowances — the transparent
/// read-back sentinel and the font-availability probe — each narrated at its
/// check below.
fn check_page_sources_spell_no_visual_value() {
    let page_css = page_source("page.css");
    let page_js = page_source("page.js");
    let index_html = page_source("index.html");
    let gallery_css = page_source("gallery.css");
    let gallery_js = page_source("gallery.js");

    // The computed-style serialization of `transparent`. The gallery's
    // painted-evidence read-back names it solely to SKIP unpainted values —
    // recognizing that nothing painted declares no color of the gallery's
    // own. Mirroring the fader-geometry exemption, the allowance is declared
    // and exact: the sentinel is erased before the scan, so any other color
    // constructor in gallery.js still fails, and no other source shares the
    // allowance.
    const TRANSPARENT_READBACK_SENTINEL: &str = "rgba(0, 0, 0, 0)";

    // No hex color and no color constructor in any page source. (A `#id`
    // selector scans as at most two hex digits and is not a color.)
    for (name, source) in [
        ("page.css", &page_css),
        ("page.js", &page_js),
        ("index.html", &index_html),
        ("gallery.css", &gallery_css),
        ("gallery.js", &gallery_js),
    ] {
        for line in source.lines() {
            let scanned = if name == "gallery.js" {
                line.replace(TRANSPARENT_READBACK_SENTINEL, "")
            } else {
                line.to_owned()
            };
            assert!(
                !scanned.contains("rgb(")
                    && !scanned.contains("rgba(")
                    && !scanned.contains("hsl("),
                "{name} builds a color of its own: {line}"
            );
            let characters: Vec<char> = scanned.chars().collect();
            for (index, character) in characters.iter().enumerate() {
                if *character != '#' {
                    continue;
                }
                let run = characters[index + 1..]
                    .iter()
                    .take_while(|c| c.is_ascii_hexdigit())
                    .count();
                assert!(
                    !matches!(run, 3 | 4 | 6 | 8),
                    "{name} spells a hex color: {line}"
                );
            }
        }
        // The authored and retired palettes are absent as text, too.
        for (color_name, hex) in AUTHORED_COLORS
            .iter()
            .map(|(_, name, hex)| (*name, *hex))
            .chain(RETIRED_COLORS)
        {
            assert!(
                !source.to_lowercase().contains(hex),
                "{name} spells {color_name} ({hex})"
            );
        }
    }

    // Raw pixel extents live only in the declared fader-geometry block.
    for line in page_css.lines() {
        let code = line.split("/*").next().unwrap_or(line);
        let has_raw_px = code
            .char_indices()
            .filter(|(_, character)| character.is_ascii_digit())
            .any(|(index, _)| code[index + 1..].starts_with("px"));
        if has_raw_px {
            assert!(
                code.trim_start().starts_with("--fader-"),
                "page.css sets a raw pixel extent outside the declared fader block: {line}"
            );
        }
    }

    // The gallery pair declares no fader geometry, so it earns no pixel
    // exemption of that kind. gallery.js's one declared allowance is the
    // FontFaceSet.check probe string: the CSS Font Loading API's font
    // shorthand makes a size syntactically mandatory, and the probe gathers
    // the typeface-resolution evidence this proof demands — it paints no
    // extent. A pixel anywhere outside a `fonts.check(` call still fails.
    for (name, source) in [("gallery.css", &gallery_css), ("gallery.js", &gallery_js)] {
        for line in source.lines() {
            let code = line.split("/*").next().unwrap_or(line);
            let has_raw_px = code
                .char_indices()
                .filter(|(_, character)| character.is_ascii_digit())
                .any(|(index, _)| code[index + 1..].starts_with("px"));
            if has_raw_px {
                assert!(
                    name == "gallery.js" && code.contains("fonts.check("),
                    "{name} sets a raw pixel extent outside the declared font-probe allowance: {line}"
                );
            }
        }
    }

    // The document itself styles nothing: no inline style, no style
    // attribute; paint enters only through the two linked stylesheets.
    assert!(!index_html.contains("<style"));
    assert!(!index_html.contains("style=\""));
    assert!(index_html.contains("tokens.css") && index_html.contains("page.css"));
}

// ===========================================================================
// T037 — state exhaustiveness, non-color legibility, page totality
// ===========================================================================

/// The state vocabulary is closed at ten and exhaustive iteration yields
/// every one of them.
fn check_state_set_is_closed_and_exhaustive() {
    assert_eq!(COMPONENT_STATE_COUNT, 10);
    assert_eq!(ALL_COMPONENT_STATES.len(), COMPONENT_STATE_COUNT);

    // The match is exhaustive with no wildcard arm, so an eleventh variant fails
    // to compile here; naming every variant is what makes the count
    // load-bearing.
    let mut named = BTreeSet::new();
    for state in ALL_COMPONENT_STATES {
        let name = match state {
            ComponentState::Resting => "Resting",
            ComponentState::Focused => "Focused",
            ComponentState::Adjusting => "Adjusting",
            ComponentState::Disabled => "Disabled",
            ComponentState::Loading => "Loading",
            ComponentState::Unavailable => "Unavailable",
            ComponentState::Error => "Error",
            ComponentState::Muted => "Muted",
            ComponentState::Soloed => "Soloed",
            ComponentState::Selected => "Selected",
        };
        assert!(
            named.insert(name),
            "{name} appears twice in ALL_COMPONENT_STATES"
        );
    }
    assert_eq!(named.len(), COMPONENT_STATE_COUNT);

    // No two states are told apart by color alone: their declared colorless
    // evidence differs.
    for (index, first) in ALL_COMPONENT_STATES.iter().enumerate() {
        for second in &ALL_COMPONENT_STATES[index + 1..] {
            let a = first.appearance();
            let b = second.appearance();
            let shape_of = |appearance: crest_synth::shell::component_state::StateAppearance| {
                (
                    format!("{}", appearance.keyline_px),
                    appearance.draws_halo,
                    appearance.fills_row,
                )
            };
            assert!(
                shape_of(a) != shape_of(b) || a.signal != b.signal,
                "{} and {} differ only in color",
                first.canonical_name(),
                second.canonical_name()
            );
        }
    }
}

/// The row states the committed render script can derive from a document
/// (`controlState` in `webview-page/page.js`), pinned exactly.
const PAGE_ROW_STATES: [&str; 8] = [
    "resting",
    "focused",
    "adjusting",
    "disabled",
    "loading",
    "unavailable",
    "error",
    "unknown",
];

/// The fader states the committed render script can derive from the mixer
/// document (`faderState` in `webview-page/page.js`), pinned exactly.
const PAGE_FADER_STATES: [&str; 6] = ["focused", "error", "disabled", "muted", "soloed", "resting"];

/// Every state the webview surface can carry announces itself with something
/// a player could read with no color vision at all, and every treatment
/// resolves from the token vocabulary.
///
/// The evidence is the committed page contract: the script's derivable state
/// sets (pinned — a gained or lost state fails here), the vocabulary words
/// the script paints beside a row, and the stylesheet treatments that carry
/// each state's keyline/halo shape. The DOM-level proof that each treatment
/// actually paints is the live target's; the recorded limitation is that
/// `Selected` is production-unprojected on the webview surfaces.
fn check_every_page_state_is_legible_without_color() {
    let page_css = page_source("page.css");
    let page_js = page_source("page.js");

    // The pinned derivable sets are exactly what the script can emit.
    for state in PAGE_ROW_STATES {
        assert!(
            page_js.contains(&format!("name: \"{state}\"")),
            "page.js can no longer derive the {state} row state"
        );
    }
    for state in PAGE_FADER_STATES {
        assert!(
            page_js.contains(&format!("return \"{state}\"")),
            "page.js can no longer derive the {state} fader state"
        );
    }

    // The recorded limitation, falsifiably: every declared ComponentState
    // except Selected is derivable on the page; Selected is exactly the one
    // that is not. A page that gains it (or loses another) fails here and
    // the recorded limitation must be revisited.
    for state in ALL_COMPONENT_STATES {
        let name = state.canonical_name().to_ascii_lowercase();
        let derivable =
            PAGE_ROW_STATES.contains(&name.as_str()) || PAGE_FADER_STATES.contains(&name.as_str());
        assert_eq!(
            derivable,
            state != ComponentState::Selected,
            "{name}: the page's derivable state set changed; revisit the recorded \
             Selected limitation"
        );
    }

    // Every state that declares a fixed word carries that word onto the page
    // as painted text — the vocabulary's word, not one the page invents.
    for state in ALL_COMPONENT_STATES {
        if let NonColorSignal::Word(word) = state.appearance().signal {
            assert!(
                !word.trim().is_empty(),
                "{} declares an empty word",
                state.canonical_name()
            );
            assert!(
                page_js.contains(&format!(">{word}<")),
                "{}'s word {word:?} is painted nowhere in the render script",
                state.canonical_name()
            );
        }
    }

    // The loading vocabulary is the structural-edit vocabulary, not a second
    // one: the page paints the document's own lifecycle word, and the
    // production loading document carries exactly the declared word (in
    // the projection's display case).
    assert!(
        page_js.contains("status.label"),
        "the loading treatment must paint the document's own lifecycle word"
    );
    let preparing = preparing_patch_document();
    let engine = preparing
        .get("surfaces")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|surface| surface.get("id").and_then(Value::as_str) == Some("patchMain"))
        .and_then(|surface| surface.get("controls").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .into_iter()
        .find(|control| {
            control
                .pointer("/path/controlId/id")
                .and_then(Value::as_str)
                == Some("patch.engine")
        })
        .expect("the loading document carries the engine row");
    assert_eq!(
        engine.pointer("/status/kind").and_then(Value::as_str),
        Some("loading")
    );
    assert_eq!(
        engine.pointer("/status/label").and_then(Value::as_str),
        Some(LOADING_PROGRESS_WORDS[0].to_ascii_uppercase().as_str()),
        "the document's lifecycle word must be the declared loading vocabulary"
    );

    // Error and unknown rows say so in text: the typed failure label and the
    // explicit ?state marker.
    assert!(
        page_js.contains("error.label"),
        "the error treatment must paint the typed failure text"
    );
    assert!(
        page_js.contains("\"?\" + String("),
        "an unknown state or value kind must read as an explicit ? marker"
    );

    // Focus and adjustment carry the authored keyline-and-halo shape from
    // the token vocabulary — and differ in shape, not only color: the focus
    // treatments draw the halo, the adjusting treatment draws none.
    let rule_block = |selector: &str| -> String {
        let start = page_css
            .find(selector)
            .unwrap_or_else(|| panic!("page.css declares {selector}"));
        let body = &page_css[start..];
        let end = body.find('}').expect("the rule block closes");
        body[..end].to_owned()
    };
    for focus_selector in [".column.focused", ".prow[data-focus-treatment=\"focused\"]"] {
        let block = rule_block(focus_selector);
        for token in [
            "var(--keyline-emphasis)",
            "var(--color-accent-focus)",
            "var(--focus-halo-radius)",
            "var(--focus-halo-spread)",
            "var(--focus-halo-opacity)",
        ] {
            assert!(
                block.contains(token),
                "{focus_selector} must resolve {token}"
            );
        }
    }
    let adjusting = rule_block(".prow[data-focus-treatment=\"adjusting\"]");
    assert!(adjusting.contains("var(--keyline-emphasis)"));
    assert!(adjusting.contains("var(--color-accent-adjust)"));
    assert!(
        !adjusting.contains("box-shadow"),
        "adjustment is distinguished from focus by shape: no halo"
    );

    // The muted, soloed, disabled, and error fader treatments resolve from
    // the token vocabulary.
    for (selector, token) in [
        (
            ".level-fader[data-state=\"muted\"]",
            "var(--color-accent-warning)",
        ),
        (
            ".level-fader[data-state=\"soloed\"]",
            "var(--color-accent-positive)",
        ),
        (
            ".level-fader[data-state=\"disabled\"]",
            "var(--color-border-strong)",
        ),
        (
            ".level-fader[data-state=\"error\"]",
            "var(--color-accent-warning)",
        ),
    ] {
        let block = rule_block(selector);
        assert!(block.contains(token), "{selector} must resolve {token}");
    }

    // Resting stays the baseline: the stylesheet declares no resting
    // override, and the vocabulary still declares it the shape baseline.
    assert!(
        !page_css.contains("[data-state=\"resting\"]"),
        "resting is the baseline the other states read against"
    );
    assert_eq!(
        ComponentState::Resting.appearance().signal,
        NonColorSignal::Shape,
        "Resting is no longer the declared baseline"
    );

    // Focus is also carried as text beyond any treatment: the header
    // annotation, the section annotation, and the footer breadcrumb.
    for needle in [
        "data-role=\"focus-annotation\"",
        "data-role=\"section-annotation\"",
        "data-role=\"breadcrumb\"",
    ] {
        assert!(
            page_js.contains(needle),
            "the page must carry focus as text ({needle})"
        );
    }
}

/// Every gallery page is reachable — by its digit binding where one exists,
/// and by stepping in every case.
fn check_every_gallery_page_is_reachable() {
    assert_eq!(ALL_GALLERY_PAGES.len(), GALLERY_PAGE_COUNT);
    assert_eq!(GALLERY_PAGE_COUNT, 15);
    assert_eq!(GALLERY_DIGIT_BINDING_COUNT, 10);

    let mut digits = BTreeSet::new();
    let mut labels = BTreeSet::new();
    let mut names = BTreeSet::new();
    for page in ALL_GALLERY_PAGES {
        if let Some(digit) = page.digit() {
            assert!(
                digits.insert(format!("{digit:?}")),
                "{} shares its digit with another page",
                page.canonical_name()
            );
            assert_eq!(
                ComponentGalleryPage::for_digit(digit),
                Some(page),
                "{}'s digit does not select it back",
                page.canonical_name()
            );
            assert!(labels.insert(
                page.digit_label()
                    .expect("a page with a digit reads as one")
                    .to_owned()
            ));
        } else {
            assert!(
                page.digit_label().is_none(),
                "{} reads as a digit it does not bind",
                page.canonical_name()
            );
        }
        assert!(names.insert(page.canonical_name()));
        assert!(!page.title().trim().is_empty());
        assert!(!page.index_label().trim().is_empty());
    }
    assert_eq!(
        digits.len(),
        GALLERY_DIGIT_BINDING_COUNT,
        "a digit binds two pages"
    );
    assert_eq!(
        labels.len(),
        GALLERY_DIGIT_BINDING_COUNT,
        "two pages read as one digit"
    );
    assert_eq!(names.len(), GALLERY_PAGE_COUNT);

    // The digit labels are exactly 1..=9 then 0, so the on-screen index and
    // the binding cannot disagree.
    let mut expected: Vec<String> = (1..=9).map(|digit| digit.to_string()).collect();
    expected.push("0".to_owned());
    expected.sort();
    assert_eq!(labels.into_iter().collect::<Vec<_>>(), expected);

    // Stepping alone reaches all fifteen, forwards from the first page.
    let mut reached = vec![ALL_GALLERY_PAGES[0]];
    while let Some(next) = PageStep::Next.apply(*reached.last().expect("a visited page")) {
        reached.push(next);
    }
    assert_eq!(
        reached,
        ALL_GALLERY_PAGES.to_vec(),
        "stepping does not reach every declared page in declared order"
    );
    // And it does not wrap at either end.
    assert_eq!(PageStep::Previous.apply(ALL_GALLERY_PAGES[0]), None);
    assert_eq!(
        PageStep::Next.apply(ALL_GALLERY_PAGES[GALLERY_PAGE_COUNT - 1]),
        None
    );

    // The rule itself, stated as a disjunction with a denominator on each
    // route: ten pages carry a binding, fifteen are stepped to, and their
    // union is exactly the declared set.
    let by_digit: BTreeSet<&str> = ALL_GALLERY_PAGES
        .into_iter()
        .filter(|page| page.digit().is_some())
        .map(ComponentGalleryPage::canonical_name)
        .collect();
    let by_stepping: BTreeSet<&str> = reached.iter().map(|page| page.canonical_name()).collect();
    let declared: BTreeSet<&str> = ALL_GALLERY_PAGES
        .into_iter()
        .map(ComponentGalleryPage::canonical_name)
        .collect();
    assert_eq!(by_digit.len(), GALLERY_DIGIT_BINDING_COUNT);
    assert_eq!(by_stepping.len(), GALLERY_PAGE_COUNT);
    assert_eq!(declared.len(), GALLERY_PAGE_COUNT);
    let unreachable: Vec<&str> = declared
        .iter()
        .filter(|name| !by_digit.contains(*name) && !by_stepping.contains(*name))
        .copied()
        .collect();
    assert!(
        unreachable.is_empty(),
        "no route reaches: {}",
        unreachable.join(", ")
    );
    assert!(
        by_digit.is_subset(&declared) && by_stepping.is_subset(&declared),
        "a route reaches a page the vocabulary does not declare"
    );

    // No key outside the declared bindings reaches a page: an unbound key
    // normalizes to `Other`, a mapped semantic key binds nothing here, and
    // the two step keys move rather than select.
    for key in [
        crest_synth::shell::window_input::WindowKey::Other,
        crest_synth::shell::window_input::WindowKey::Q,
        crest_synth::shell::window_input::WindowKey::K,
        crest_synth::shell::window_input::WindowKey::BracketLeft,
        crest_synth::shell::window_input::WindowKey::BracketRight,
    ] {
        assert_eq!(
            ComponentGalleryPage::for_digit(key),
            None,
            "{key:?} selects a page it should not bind"
        );
    }
}

// ===========================================================================
// T038 — the typed typeface failure and the served faces
// ===========================================================================

/// An unavailable face is a typed error naming the face, never a
/// substitution.
fn check_missing_typeface_is_a_typed_failure() {
    let missing = repository_root().join("vendor/no-such-typeface-for-component-vocabulary");
    assert!(
        !missing.exists(),
        "the missing-face fixture path must not exist; this test never mutates the \
         repository, it only points the loader somewhere empty"
    );

    let error = AuthoredTypeface::load_from_dir(&missing)
        .expect_err("a missing face directory must not load successfully");
    match &error {
        TypefaceError::FaceUnavailable { weight, path, .. } => {
            assert_eq!(
                *weight, ALL_WEIGHTS[0],
                "the failure must name the first face it could not read"
            );
            assert!(
                path.ends_with("AzeretMono-Regular.ttf"),
                "the failure must name the file, got {}",
                path.display()
            );
        }
        other => panic!("expected a typed FaceUnavailable, got {other:?}"),
    }
    let message = error.to_string();
    assert!(
        message.contains(AUTHORED_FAMILY),
        "the visible failure must name the unavailable face: {message}"
    );
    assert!(
        message.contains("Regular"),
        "the visible failure must name the weight: {message}"
    );
    assert!(
        !message.to_ascii_lowercase().contains("fallback")
            && !message.to_ascii_lowercase().contains("substitut"),
        "the failure must not describe a substitution: {message}"
    );

    // An unreadable face is not silently accepted either.
    let empty = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("component-vocabulary-empty-faces");
    std::fs::create_dir_all(&empty).expect("the fixture directory is creatable");
    for weight in ALL_WEIGHTS {
        let name = format!(
            "AzeretMono-{}.ttf",
            family_name(weight)
                .strip_prefix(AUTHORED_FAMILY)
                .expect("the family name carries the authored prefix")
                .trim()
        );
        std::fs::write(empty.join(name), b"").expect("the empty face is writable");
    }
    assert!(
        matches!(
            AuthoredTypeface::load_from_dir(&empty),
            Err(TypefaceError::FaceUnreadable { .. })
        ),
        "an empty face must be a typed failure rather than an accepted face"
    );
    std::fs::remove_dir_all(&empty).ok();
}

/// The success path, on the webview surface: all four authored weights load
/// from the vendored directory, and the page declares exactly those four
/// faces — authored family, authored numeric weights, vendored files, no
/// fallback stack anywhere.
fn check_the_authored_typeface_serves_completely() {
    let typeface = AuthoredTypeface::load().expect("the vendored faces are present");
    assert_eq!(typeface.registered_weights(), ALL_WEIGHTS.to_vec());

    let page_css = page_source("page.css");
    assert_eq!(
        page_css.matches("@font-face").count(),
        ALL_WEIGHTS.len(),
        "the page declares one face per authored weight"
    );
    assert!(
        !page_css.contains(&format!("\"{AUTHORED_FAMILY}\",")),
        "the authored family must carry no fallback stack"
    );
    for weight in ALL_WEIGHTS {
        assert!(
            page_css.contains(&format!("font-weight: {};", weight.numeric())),
            "the authored numeric weight {} is not declared on the page",
            weight.numeric()
        );
        let face = family_name(weight)
            .strip_prefix(AUTHORED_FAMILY)
            .expect("the family name carries the authored prefix")
            .trim()
            .to_owned();
        let file = format!("AzeretMono-{face}.ttf");
        assert!(
            page_css.contains(&format!("url(\"fonts/{file}\")")),
            "the {face} face is not loaded from the served fonts"
        );
        let vendored = repository_root().join("vendor/azeret-mono").join(&file);
        let bytes = std::fs::read(&vendored)
            .unwrap_or_else(|error| panic!("{} must exist: {error}", vendored.display()));
        assert!(!bytes.is_empty(), "{file} must carry face bytes");
    }
    // The page binds text to the authored family, verbatim, exactly once per
    // face plus the body binding.
    assert_eq!(
        page_css
            .matches(&format!("font-family: \"{AUTHORED_FAMILY}\";"))
            .count(),
        ALL_WEIGHTS.len() + 1
    );
}

// ===========================================================================
// The declared checks, and the marker
// ===========================================================================

#[test]
fn every_declared_value_equals_its_authored_counterpart() {
    check_declared_values_match_the_authored_table();
}

#[test]
fn the_generated_token_table_carries_every_authored_value() {
    check_generated_tokens_carry_every_authored_value();
}

#[test]
fn the_density_policy_geometry_reaches_the_page() {
    check_responsive_geometry_reaches_the_page();
}

#[test]
fn the_page_sources_spell_no_visual_value() {
    check_page_sources_spell_no_visual_value();
}

#[test]
fn the_literal_guard_reads_the_delivered_tree() {
    let (sources, lines, violations) = scan_delivered_tree();
    // A scan that read nothing passes vacuously, so what it read is asserted
    // before anything is asserted about what it found.
    assert!(
        sources.len() >= 25,
        "the guard scanned only {} sources: {sources:?}",
        sources.len()
    );
    assert!(lines > 5_000, "the guard read only {lines} lines");
    assert!(
        sources
            .iter()
            .any(|path| path == "src/shell/webview/window.rs"),
        "the guard did not scan the webview window adapter"
    );
    assert!(
        sources
            .iter()
            .any(|path| path == "src/shell/webview/token_export.rs"),
        "the guard did not scan the token generator"
    );
    assert!(
        sources
            .iter()
            .any(|path| path == "src/testing/component_gallery_scene.rs"),
        "the guard did not scan the component gallery scene"
    );
    assert!(
        !sources.iter().any(|path| path == VOCABULARY_FILE),
        "the guard scanned the vocabulary, which is the one file allowed to spell values"
    );
    assert!(
        violations.is_empty(),
        "visual literals survive outside {VOCABULARY_FILE}:\n{}",
        violations
            .iter()
            .map(LiteralViolation::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn the_literal_guard_reports_a_planted_literal() {
    // A guard that has never failed is indistinguishable from no guard. Each
    // family is planted, and each must be reported with file, line, and kind.
    let planted = concat!(
        "use ",
        "efr",
        "ame::eg",
        "ui::Color32;\n",
        "pub const ACCENT: Color32 = Color32::from_rgb(0x65, 0xe5, 0xff);\n",
        "pub fn paint(ui: &mut Ui) {\n",
        "    ui.add_space(12.0);\n",
        "    let id = FontId::new(14.0, FontFamily::Proportional);\n",
        "    let hex = Color32::from_hex(\"#0c1015\");\n",
        "}\n",
    );
    let violations = scan_source("src/adapter/planted.rs", planted);
    let reported: Vec<String> = violations.iter().map(LiteralViolation::report).collect();
    let joined = reported.join("\n");

    for expected in [
        ("color", 2_usize),
        ("palette", 2),
        ("spacing", 4),
        ("type size", 5),
        ("color", 6),
        ("palette", 6),
    ] {
        assert!(
            violations
                .iter()
                .any(|violation| violation.kind == expected.0 && violation.line == expected.1),
            "the guard did not report a {} literal on line {}:\n{joined}",
            expected.0,
            expected.1
        );
    }
    assert!(
        reported
            .iter()
            .all(|report| report.starts_with("src/adapter/planted.rs:")),
        "every report must name its file so a failure is actionable:\n{joined}"
    );

    // The retired palette is caught too: reintroducing the pre-mission focus
    // green is exactly the regression this guard exists to stop — including
    // in the webview shell sources the scan now covers.
    let regression = "let focus = Color32::from_rgb(110, 205, 174);\n";
    let caught = scan_source("src/shell/webview/regression.rs", regression);
    assert!(
        caught.iter().any(|violation| violation.kind == "color"),
        "the guard accepted a raw-channel color: {caught:?}"
    );
    let retired_hex = "// nothing here\nconst OLD: &str = \"#6ecdae\";\n";
    assert!(
        scan_source("src/shell/webview/retired.rs", retired_hex)
            .iter()
            .any(|violation| violation.kind == "palette"),
        "the guard accepted the retired focus green spelled as hex"
    );
}

#[test]
fn the_literal_guard_allows_what_the_vocabulary_permits() {
    // Named constants, the transparent sentinel, resolved tokens, zero, and
    // narration in comments are not literals. A guard that flagged these
    // would be turned off within a week, which is the other way a guard
    // stops guarding.
    let permitted = concat!(
        "//! The retired #6ecdae green and Color32::from_rgb(0x65, 0xe5, 0xff) are\n",
        "//! narrated here as history, which is not a value the shell paints.\n",
        "use crate::shell::tokens::{SemanticColor, SpacingStep, MIN_INTERACTIVE_TARGET_PX};\n",
        "pub fn paint(ui: &mut Ui) {\n",
        "    ui.add_space(SpacingStep::S12.resolve());\n",
        "    let clear = Color32::TRANSPARENT;\n",
        "    let halo = Color32::from_rgba_unmultiplied(o.r(), o.g(), o.b(), alpha());\n",
        "    let id = FontId::new(style.metrics().size_px, family_for(style));\n",
        "    let corner = CornerRadius::same(Radius::Small.resolve() as u8);\n",
        "    painter.rect_filled(row, 0.0, SemanticColor::BgPanel.resolve());\n",
        "    let button = Button::new(label).min_size(vec2(0.0, MIN_INTERACTIVE_TARGET_PX));\n",
        "    let label = \"PATCH 01 · Lead\";\n",
        "}\n",
    );
    let violations = scan_source("src/adapter/permitted.rs", permitted);
    assert!(
        violations.is_empty(),
        "the guard flagged a permitted construction:\n{}",
        violations
            .iter()
            .map(LiteralViolation::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn the_state_vocabulary_is_closed_exhaustive_and_legible_without_color() {
    check_state_set_is_closed_and_exhaustive();
    check_every_page_state_is_legible_without_color();
}

#[test]
fn every_gallery_page_is_reachable_by_digit_or_by_stepping() {
    check_every_gallery_page_is_reachable();
}

#[test]
fn an_unavailable_typeface_is_a_typed_visible_failure() {
    check_missing_typeface_is_a_typed_failure();
    check_the_authored_typeface_serves_completely();
}

/// The declared acceptance target.
///
/// Every check above runs here, in order, and the marker
/// `validation.component_vocabulary` asserts on is printed strictly after
/// the last of them returns. A failing check panics before the print, so the
/// marker cannot appear on a red run. The checks are also exposed
/// individually so a failure names which claim broke rather than only that
/// something did.
#[test]
fn component_vocabulary_acceptance() {
    check_declared_values_match_the_authored_table();
    check_generated_tokens_carry_every_authored_value();
    check_responsive_geometry_reaches_the_page();
    check_page_sources_spell_no_visual_value();

    let (sources, lines, violations) = scan_delivered_tree();
    assert!(
        sources.len() >= 25 && lines > 5_000,
        "the guard read too little"
    );
    assert!(
        violations.is_empty(),
        "visual literals survive outside {VOCABULARY_FILE}:\n{}",
        violations
            .iter()
            .map(LiteralViolation::report)
            .collect::<Vec<_>>()
            .join("\n")
    );

    check_state_set_is_closed_and_exhaustive();
    check_every_page_state_is_legible_without_color();
    check_every_gallery_page_is_reachable();

    check_missing_typeface_is_a_typed_failure();
    check_the_authored_typeface_serves_completely();

    let token_declarations = token_export::tokens_css()
        .lines()
        .filter(|line| line.trim_start().starts_with("--"))
        .count();
    println!(
        "CREST_COMPONENT_VOCABULARY_OBSERVATION colors={} type_styles={} spacing_steps={} \
         radii={} states={} pages={} density_policies={} token_declarations={} \
         sources_scanned={} lines_scanned={}",
        ALL_COLORS.len(),
        ALL_TYPE_STYLES.len(),
        ALL_SPACING_STEPS.len(),
        ALL_RADII.len(),
        COMPONENT_STATE_COUNT,
        GALLERY_PAGE_COUNT,
        ALL_LAYOUT_MODES.len(),
        token_declarations,
        sources.len(),
        lines,
    );
    println!("{ACCEPTANCE_MARKER}");
}
