//! The browsable component gallery, re-hosted on the webview surface.
//!
//! One command opens a real webview window showing every declared component
//! specimen through the authored vocabulary. Digit keys `1`–`9` and `0` select
//! the first ten pages; `[` and `]` step through all fifteen. The active page
//! identity is on screen at all times.
//!
//! # How the re-host is shaped (WP04, spec C-006: rebuild, not retire)
//!
//! The scene keeps what it always owned — the page registry, the scene-local
//! digit and stepping bindings, the coverage assertions, and the post-paint
//! observation. What changed is the painting: instead of native paint passes,
//! the scene builds one [`GalleryDocument`] per page and pushes it to a
//! Tauri v2 webview over the [`GALLERY_DOCUMENT_EVENT`] channel, where
//! `webview-page/gallery.js` renders it from the same generated
//! `webview-page/tokens.css` token table the product page uses and
//! `webview-page/gallery.css` lays it out at both authored densities. The
//! push mechanics mirror `crate::shell::webview::projection_channel` (an
//! identity-gated emit through the tauri `Emitter`); the gallery deliberately
//! selects its own event names so the product's projection transport carries
//! production documents only.
//!
//! The gallery document is a **gallery-scene document**. It lives beside
//! `SemanticGraphicalViewModel`, makes no claim on the production projection
//! schema, and is never pushed on the production projection channel — the
//! byte-identity proof governs production documents, and this type is not one
//! (spec C-002/C-006).
//!
//! # This scene is browsable, not autonomous
//!
//! Every other `demo-live-*` scene is deliberately input-isolated: while
//! active, mapped semantic key input is not dispatched into `AppState`, so an
//! asynchronous edit cannot replace the exact generation a checkpoint awaits
//! (`DESIGN.md`). This scene is the opposite on purpose. It exists to
//! be driven by hand. It therefore accepts input, makes no exact-generation
//! claim, asserts nothing about audio, does not time out, and is not an alias
//! for `demo-live`.
//!
//! It does not weaken the witness contract because it never claims one. The
//! danger runs both ways: giving this scene the witness contract would break
//! paging, and copying this scene's input handling back into a witness would
//! break the generation correlation those scenes depend on. Do neither.
//!
//! # The hard invariant
//!
//! Page selection is scene-local. It never becomes a `SemanticAction`, never
//! reaches `AppState`, and never changes focus, Patch values, graph revision,
//! or audio behavior. The scene binds its digits itself and never consults
//! [`KeyboardInputTranslator`](crate::shell::keyboard_input_translator::KeyboardInputTranslator);
//! `Digit1` and `Digit2` select PATCH and MIXER in the application and select
//! pages here, which is safe precisely because the binding never leaves this
//! file. Keys are captured Rust-side by
//! [`crate::shell::webview::input_capture`] — the same native monitor the
//! product webview shell uses — and the page registers no key handler at all.
//! The scene holds one production `AppState` purely so the claim is
//! *measured*: its generation is read before the window opens and again after
//! it closes, and the difference is reported.
//!
//! # The observation is ack-gated
//!
//! Every count in [`ComponentGalleryObservation`] accumulates from
//! [`GalleryPaintedAck`] values the page emits on
//! [`GALLERY_PAINTED_EVENT`] *after* painting a document — the page reads its
//! own painted DOM back and copies the page, state, and viewport identity
//! actually rendered. A digit request alone proves only that a key was
//! pressed; the page it asked for counts as reached only when its painted ack
//! arrives. There is no constructor that accepts an expected page set, a
//! specimen list, or a coverage plan, so a pre-render plan cannot satisfy the
//! observation.
//!
//! Realizes `valueObject.Shell.ComponentGalleryPage` and
//! `valueObject.Shell.ComponentGalleryObservation` over
//! `requirement.browsable_component_gallery` and
//! `requirement.silent_component_gallery`.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Listener, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use crate::control::app_state::AppState;
use crate::control::SemanticControlKind;
use crate::mixer::global_parameters::GlobalParameters;
use crate::shell::component_state::{ComponentState, ALL_COMPONENT_STATES, COMPONENT_STATE_COUNT};
use crate::shell::component_vocabulary::{
    control_for, draws_cursor, status_mark, ComponentControl, HintTone, LoadingPhase,
    ShellComposition, StatusDetail, StatusMark, ALL_COMPONENT_CONTROLS, ALL_HINT_TONES,
    ALL_PRESENTATION_ROLES, ALL_SEMANTIC_CONTROL_KINDS, ALL_SHELL_COMPOSITIONS,
    COMPONENT_CONTROL_COUNT, OBSERVED_REGION_NAMES, SHELL_COMPOSITION_COUNT,
};
use crate::shell::density::{RepresentativeViewport, ResponsiveShellContract};
use crate::shell::standalone_application::ApplicationConfig;
use crate::shell::tokens::{
    FontWeight, Radius, SemanticColor, SpacingStep, TypeStyle, ALL_COLORS, ALL_RADII,
    ALL_SPACING_STEPS, ALL_TYPE_STYLES, KEYLINE_EMPHASIS_PX, KEYLINE_RESTING_PX,
    MIN_INTERACTIVE_TARGET_PX,
};
use crate::shell::webview::input_capture;
use crate::shell::window_input::{WindowInput, WindowInputKind, WindowKey};
use crate::synth::instrument_capability::{CapabilityError, CapabilityRegistry};
use crate::synth::instrument_capability_provider::InstrumentCapabilityProvider;

/// The stdout marker the gallery observation is printed behind.
pub const COMPONENT_GALLERY_OBSERVATION_MARKER: &str = "CREST_COMPONENT_GALLERY_OBSERVATION ";

/// The native window title.
pub const COMPONENT_GALLERY_WINDOW_TITLE: &str = "crest-synth — component gallery";

const GALLERY_VIEWPORTS: [RepresentativeViewport; 2] = [
    RepresentativeViewport::WideReference,
    RepresentativeViewport::StandardReference,
];

/// The named tauri event carrying each gallery document to the page.
///
/// Deliberately not [`crate::shell::webview::projection_channel::PROJECTION_EVENT`]:
/// the production projection channel carries production documents only, and a
/// gallery document on it would be a schema claim C-002 forbids.
pub const GALLERY_DOCUMENT_EVENT: &str = "crest://gallery";

/// The named tauri event the page emits after painting one gallery document,
/// carrying the page, state, and viewport identity actually rendered.
pub const GALLERY_PAINTED_EVENT: &str = "crest://gallery-painted";

/// The named tauri event the page emits once its script and vendored faces
/// are loaded and it can render documents.
pub const GALLERY_READY_EVENT: &str = "crest://gallery-ready";

/// How many pages the gallery declares.
///
/// Surfaces that must cover every page assert against this rather than against
/// a number they carry themselves.
pub const GALLERY_PAGE_COUNT: usize = 15;

/// How many pages a digit key selects.
///
/// Ten, because there are ten digits. The remaining pages are reached by
/// stepping, which is why stepping exists: a page count larger than the digit
/// count would otherwise make a declared page unreachable.
pub const GALLERY_DIGIT_BINDING_COUNT: usize = 10;

/// The declared mixer track column anatomy, in declared order.
///
/// The bank specimen shows the same column anatomy the shipped MIXER shows,
/// and the observation measures the painted columns against exactly this
/// closed, ordered list.
pub const MIXER_COLUMN_ANATOMY: [&str; 5] = [
    "TrackHeader",
    "LevelFader",
    "LevelReadout",
    "PanReadout",
    "StateLine",
];

/// One named group of component specimens, selected by a locally bound digit or
/// reached by stepping.
///
/// The set is closed and has no catch-all, for the same reason
/// [`ComponentState`] is closed: a page added without a specimen fails the
/// declared coverage assertion rather than becoming a page nobody can see.
///
/// The seven control and composition pages are appended after the eight that
/// existed before them, and nothing renumbers. An operator who knows `Digit4`
/// is `InteractionStates` keeps finding it there; that is FR-012, and
/// [`FROZEN_DIGIT_BINDING_BASELINE`] is what holds it.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ComponentGalleryPage {
    /// Every declared semantic color with its canonical name and hex.
    Colors,
    /// All eight type styles at their authored metrics.
    Type,
    /// The six spacing steps, the radii, the keyline widths, and the minimum target.
    SpacingAndGeometry,
    /// Resting, Focused, and Adjusting side by side.
    InteractionStates,
    /// Text roles at every state, plus hairlines and keylines.
    TextAndHairlines,
    /// Value displays and every status mark.
    ValuesAndStatus,
    /// The four action-hint tones.
    ActionHints,
    /// The five structural bands at both authored viewports.
    ShellBands,
    /// The parameter row and the choice row, in every state they declare.
    ParameterAndChoiceRows,
    /// The toggle and the compact slider, in every state they declare.
    TogglesAndSliders,
    /// The fader and the meter, in every state they declare.
    FadersAndMeters,
    /// The browser row and the modal option, in every state they declare.
    BrowserAndModalOptions,
    /// The application shell and the context switch, with projected content.
    ShellAndContextSwitch,
    /// The identity header and the section, with projected content.
    HeadersAndSections,
    /// The Patch strip row, the mixer strip bank, the Utility/Inspector panel,
    /// and the footer, with projected content.
    StripPanelAndFooter,
}

/// Every declared page, in declared order.
///
/// The first ten carry the ten digit bindings in this order; the last five are
/// reached by stepping.
pub const ALL_GALLERY_PAGES: [ComponentGalleryPage; GALLERY_PAGE_COUNT] = [
    ComponentGalleryPage::Colors,
    ComponentGalleryPage::Type,
    ComponentGalleryPage::SpacingAndGeometry,
    ComponentGalleryPage::InteractionStates,
    ComponentGalleryPage::TextAndHairlines,
    ComponentGalleryPage::ValuesAndStatus,
    ComponentGalleryPage::ActionHints,
    ComponentGalleryPage::ShellBands,
    ComponentGalleryPage::ParameterAndChoiceRows,
    ComponentGalleryPage::TogglesAndSliders,
    ComponentGalleryPage::FadersAndMeters,
    ComponentGalleryPage::BrowserAndModalOptions,
    ComponentGalleryPage::ShellAndContextSwitch,
    ComponentGalleryPage::HeadersAndSections,
    ComponentGalleryPage::StripPanelAndFooter,
];

/// The eight `(page identity, digit)` bindings as they stood before the control
/// and composition pages were added.
///
/// Frozen as data, modelled on `FROZEN_TOPOLOGY_IDENTITY_BASELINE`
/// (`tests/effects_and_buses.rs:59`), which the project already uses for
/// exactly this kind of add-only contract. FR-012 exists because an operator
/// feels a moved binding immediately, so this is a regression gate rather than
/// a promise: reordering [`ALL_GALLERY_PAGES`], renaming a page, or handing an
/// existing page a different digit fails the assertion that reads this.
///
/// New pages append. Nothing here moves.
pub const FROZEN_DIGIT_BINDING_BASELINE: [(&str, WindowKey); 8] = [
    ("Colors", WindowKey::Digit1),
    ("Type", WindowKey::Digit2),
    ("SpacingAndGeometry", WindowKey::Digit3),
    ("InteractionStates", WindowKey::Digit4),
    ("TextAndHairlines", WindowKey::Digit5),
    ("ValuesAndStatus", WindowKey::Digit6),
    ("ActionHints", WindowKey::Digit7),
    ("ShellBands", WindowKey::Digit8),
];

impl ComponentGalleryPage {
    /// The digit that selects this page, or `None` when no digit does.
    ///
    /// There are fifteen pages and ten digits, so five pages carry no binding.
    /// They return `None` rather than a placeholder key: `WindowKey::Other` is
    /// the catch-all for keys the window saw and could not name, and reusing it
    /// here would make "this page has no digit" and "this key is not one we
    /// recognize" the same value.
    pub const fn digit(self) -> Option<WindowKey> {
        match self {
            Self::Colors => Some(WindowKey::Digit1),
            Self::Type => Some(WindowKey::Digit2),
            Self::SpacingAndGeometry => Some(WindowKey::Digit3),
            Self::InteractionStates => Some(WindowKey::Digit4),
            Self::TextAndHairlines => Some(WindowKey::Digit5),
            Self::ValuesAndStatus => Some(WindowKey::Digit6),
            Self::ActionHints => Some(WindowKey::Digit7),
            Self::ShellBands => Some(WindowKey::Digit8),
            Self::ParameterAndChoiceRows => Some(WindowKey::Digit9),
            Self::TogglesAndSliders => Some(WindowKey::Digit0),
            Self::FadersAndMeters
            | Self::BrowserAndModalOptions
            | Self::ShellAndContextSwitch
            | Self::HeadersAndSections
            | Self::StripPanelAndFooter => None,
        }
    }

    /// The page a digit selects, or `None` when the key binds no page.
    ///
    /// This is the whole of the scene's key binding. Nothing here produces a
    /// `SemanticAction`, and no unbound key reaches a default page.
    pub fn for_digit(key: WindowKey) -> Option<Self> {
        ALL_GALLERY_PAGES
            .into_iter()
            .find(|page| page.digit() == Some(key))
    }

    /// The digit as it reads on screen, where one selects this page.
    pub const fn digit_label(self) -> Option<&'static str> {
        match self {
            Self::Colors => Some("1"),
            Self::Type => Some("2"),
            Self::SpacingAndGeometry => Some("3"),
            Self::InteractionStates => Some("4"),
            Self::TextAndHairlines => Some("5"),
            Self::ValuesAndStatus => Some("6"),
            Self::ActionHints => Some("7"),
            Self::ShellBands => Some("8"),
            Self::ParameterAndChoiceRows => Some("9"),
            Self::TogglesAndSliders => Some("0"),
            Self::FadersAndMeters
            | Self::BrowserAndModalOptions
            | Self::ShellAndContextSwitch
            | Self::HeadersAndSections
            | Self::StripPanelAndFooter => None,
        }
    }

    /// The canonical page identity.
    pub const fn canonical_name(self) -> &'static str {
        match self {
            Self::Colors => "Colors",
            Self::Type => "Type",
            Self::SpacingAndGeometry => "SpacingAndGeometry",
            Self::InteractionStates => "InteractionStates",
            Self::TextAndHairlines => "TextAndHairlines",
            Self::ValuesAndStatus => "ValuesAndStatus",
            Self::ActionHints => "ActionHints",
            Self::ShellBands => "ShellBands",
            Self::ParameterAndChoiceRows => "ParameterAndChoiceRows",
            Self::TogglesAndSliders => "TogglesAndSliders",
            Self::FadersAndMeters => "FadersAndMeters",
            Self::BrowserAndModalOptions => "BrowserAndModalOptions",
            Self::ShellAndContextSwitch => "ShellAndContextSwitch",
            Self::HeadersAndSections => "HeadersAndSections",
            Self::StripPanelAndFooter => "StripPanelAndFooter",
        }
    }

    /// The page title, as it reads on screen.
    pub const fn title(self) -> &'static str {
        match self {
            Self::Colors => "COLORS",
            Self::Type => "TYPE",
            Self::SpacingAndGeometry => "SPACING AND GEOMETRY",
            Self::InteractionStates => "INTERACTION STATES",
            Self::TextAndHairlines => "TEXT AND HAIRLINES",
            Self::ValuesAndStatus => "VALUES AND STATUS",
            Self::ActionHints => "ACTION HINTS",
            Self::ShellBands => "SHELL BANDS",
            Self::ParameterAndChoiceRows => "PARAMETER AND CHOICE ROWS",
            Self::TogglesAndSliders => "TOGGLES AND SLIDERS",
            Self::FadersAndMeters => "FADERS AND METERS",
            Self::BrowserAndModalOptions => "BROWSER AND MODAL OPTIONS",
            Self::ShellAndContextSwitch => "SHELL AND CONTEXT SWITCH",
            Self::HeadersAndSections => "HEADERS AND SECTIONS",
            Self::StripPanelAndFooter => "STRIP, PANEL AND FOOTER",
        }
    }

    /// The short title used in the page index line.
    pub const fn index_label(self) -> &'static str {
        match self {
            Self::Colors => "COLORS",
            Self::Type => "TYPE",
            Self::SpacingAndGeometry => "SPACING",
            Self::InteractionStates => "STATES",
            Self::TextAndHairlines => "TEXT",
            Self::ValuesAndStatus => "VALUES",
            Self::ActionHints => "HINTS",
            Self::ShellBands => "BANDS",
            Self::ParameterAndChoiceRows => "ROWS",
            Self::TogglesAndSliders => "TOGGLES",
            Self::FadersAndMeters => "FADERS",
            Self::BrowserAndModalOptions => "BROWSER",
            Self::ShellAndContextSwitch => "SHELL",
            Self::HeadersAndSections => "HEADERS",
            Self::StripPanelAndFooter => "STRIP",
        }
    }

    /// This page's position in [`ALL_GALLERY_PAGES`].
    const fn index(self) -> usize {
        match self {
            Self::Colors => 0,
            Self::Type => 1,
            Self::SpacingAndGeometry => 2,
            Self::InteractionStates => 3,
            Self::TextAndHairlines => 4,
            Self::ValuesAndStatus => 5,
            Self::ActionHints => 6,
            Self::ShellBands => 7,
            Self::ParameterAndChoiceRows => 8,
            Self::TogglesAndSliders => 9,
            Self::FadersAndMeters => 10,
            Self::BrowserAndModalOptions => 11,
            Self::ShellAndContextSwitch => 12,
            Self::HeadersAndSections => 13,
            Self::StripPanelAndFooter => 14,
        }
    }

    /// The page whose canonical name is `name`, or `None`.
    ///
    /// The ack path resolves painted page identities through this rather than
    /// trusting the string, so a page the vocabulary does not declare cannot
    /// enter the ledger.
    fn for_canonical_name(name: &str) -> Option<Self> {
        ALL_GALLERY_PAGES
            .into_iter()
            .find(|page| page.canonical_name() == name)
    }

    /// The page one step before this one, or `None` at the first page.
    ///
    /// Non-wrapping, matching the nonwrapping movement the product uses
    /// everywhere else (`DESIGN.md`). Returning `None` at the end rather
    /// than the same page is what lets the caller report *retained* instead of
    /// *changed*, so a step that did nothing is visible as a step that did
    /// nothing.
    pub fn previous(self) -> Option<Self> {
        self.index()
            .checked_sub(1)
            .map(|index| ALL_GALLERY_PAGES[index])
    }

    /// The page one step after this one, or `None` at the last page.
    pub fn next(self) -> Option<Self> {
        ALL_GALLERY_PAGES.get(self.index() + 1).copied()
    }
}

/// How the previous-page key reads on screen.
pub const STEP_PREVIOUS_LABEL: &str = "[";

/// How the next-page key reads on screen.
pub const STEP_NEXT_LABEL: &str = "]";

/// What the page index shows beside a page no digit selects.
///
/// The two bracket glyphs together, so the index line says how to reach the
/// page rather than leaving a blank where the other ten carry a number.
pub const STEP_ONLY_LABEL: &str = "[ ]";

/// Which way a scene-local step moves through the declared page order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageStep {
    /// Toward the first page.
    Previous,
    /// Toward the last page.
    Next,
}

impl PageStep {
    /// The key that asks for this step.
    pub const fn key(self) -> WindowKey {
        match self {
            Self::Previous => WindowKey::BracketLeft,
            Self::Next => WindowKey::BracketRight,
        }
    }

    /// The step a key asks for, or `None` when the key steps nothing.
    pub fn for_key(key: WindowKey) -> Option<Self> {
        [Self::Previous, Self::Next]
            .into_iter()
            .find(|step| step.key() == key)
    }

    /// Applies this step to `page`, or `None` at the end it points at.
    pub fn apply(self, page: ComponentGalleryPage) -> Option<ComponentGalleryPage> {
        match self {
            Self::Previous => page.previous(),
            Self::Next => page.next(),
        }
    }
}

/// What a consumed input did to the active page.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageSelection {
    /// A bound digit selected this page.
    Changed(ComponentGalleryPage),
    /// A step key moved to this page.
    Stepped(ComponentGalleryPage),
    /// The input bound no page — an unbound key, or a step past an end — so the
    /// current one was kept.
    Retained(ComponentGalleryPage),
}

impl PageSelection {
    /// The page that is active after the input.
    pub const fn page(self) -> ComponentGalleryPage {
        match self {
            Self::Changed(page) | Self::Stepped(page) | Self::Retained(page) => page,
        }
    }
}

/// The scene's page selection.
///
/// This is the entire mutable state paging owns. It is not an `AppState`, it
/// never becomes one, and no value here is projected, persisted, or sent to
/// audio.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GalleryPageSelection {
    active: ComponentGalleryPage,
}

impl Default for GalleryPageSelection {
    fn default() -> Self {
        Self {
            active: ALL_GALLERY_PAGES[0],
        }
    }
}

impl GalleryPageSelection {
    /// The page currently on screen.
    pub const fn active(self) -> ComponentGalleryPage {
        self.active
    }

    /// Applies one normalized window input.
    ///
    /// Only a key-*down* on a bound digit or a step key moves the page. A
    /// key-up, a focus loss, and any key that binds neither retain the current
    /// page and change nothing else.
    ///
    /// Stepping does not wrap. At the first page a previous-step retains the
    /// first page, and at the last a next-step retains the last, which is the
    /// nonwrapping movement the product uses everywhere else
    /// (`DESIGN.md`). A wrapping step would let an operator holding one
    /// bracket cycle forever without ever learning they had reached an end.
    pub fn apply(&mut self, input: WindowInput) -> PageSelection {
        if input.kind() != WindowInputKind::KeyDown {
            return PageSelection::Retained(self.active);
        }
        if let Some(page) = ComponentGalleryPage::for_digit(input.key()) {
            self.active = page;
            return PageSelection::Changed(page);
        }
        match PageStep::for_key(input.key()).and_then(|step| step.apply(self.active)) {
            Some(page) => {
                self.active = page;
                PageSelection::Stepped(page)
            }
            None => PageSelection::Retained(self.active),
        }
    }
}

// ===========================================================================
// The gallery-scene document
// ===========================================================================

/// The declared schema identity of a gallery-scene document.
///
/// Named so the page can refuse a document it does not understand, and so the
/// document can never be mistaken for a serialized
/// `SemanticGraphicalViewModel` — this schema is scene-local by declaration.
pub const GALLERY_DOCUMENT_SCHEMA: &str = "crest-gallery-scene/1";

/// One entry in the on-screen page index line.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GalleryIndexEntry {
    /// The canonical page identity.
    page: &'static str,
    /// The short label the index line shows.
    label: &'static str,
    /// The key that reaches the page: its digit, or [`STEP_ONLY_LABEL`].
    key: &'static str,
    /// Whether this entry is the active page.
    active: bool,
}

/// One representative line inside a composition specimen.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GalleryCompositionEntry {
    /// The label the line reads.
    label: String,
    /// The value beside it, if the line carries one.
    value: Option<String>,
    /// The shared tone class the line paints in (`page.css` tone classes),
    /// if it is not the default.
    tone_class: Option<&'static str>,
}

impl GalleryCompositionEntry {
    fn new(label: &str, value: Option<&str>, tone_class: Option<&'static str>) -> Self {
        Self {
            label: label.to_owned(),
            value: value.map(str::to_owned),
            tone_class,
        }
    }
}

/// One representative mixer track column inside the bank specimen.
///
/// The bank specimen shows the same column anatomy the shipped MIXER shows
/// ([`MIXER_COLUMN_ANATOMY`]); the page renders each column through the same
/// `page.css` column classes and structure attributes the product page uses.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GalleryBankColumn {
    /// The track header, e.g. `T00`.
    track: String,
    /// The level as a fraction of its declared range.
    level: f64,
    /// The level readout in the bound MidiHexadecimal form.
    level_hex: String,
    /// The pan condition, `L`, `C`, or `R`.
    pan: &'static str,
    /// The mute/solo state line, e.g. `M -- · S ON`.
    state_line: String,
    /// Whether this column carries the focus treatment.
    focused: bool,
}

/// One specimen inside a gallery section, dispatched by `kind` in
/// `gallery.js`.
///
/// Every value here is derived from the closed vocabulary sets — colors from
/// [`ALL_COLORS`], metrics from the authored tokens, state treatments from
/// [`ComponentState::appearance`] through the production `status_mark` and
/// `focus` primitives — never typed as page-side literals. CSS custom
/// property names follow the mechanical mapping
/// `token_export` declares (`color/bg/canvas` → `--color-bg-canvas`), so the
/// page resolves every painted value from the generated token table.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum GallerySpecimen {
    /// One declared color role: swatch plus canonical name plus hex.
    #[serde(rename_all = "camelCase")]
    Color {
        /// The canonical authored name.
        name: &'static str,
        /// The authored value as `#rrggbb`.
        hex: String,
        /// The generated custom property the swatch paints from.
        css_var: String,
    },
    /// One declared type style at its authored metrics.
    #[serde(rename_all = "camelCase")]
    Type {
        /// The canonical authored name.
        name: &'static str,
        /// The gallery stylesheet class carrying this style's token metrics.
        css_class: String,
        /// Authored glyph size.
        size_px: f32,
        /// Authored line height.
        line_height_px: f32,
        /// Authored weight as its CSS number.
        weight: u16,
        /// Authored letter spacing.
        tracking_px: f32,
        /// The tone class the sample paints in, if not the default.
        tone_class: Option<&'static str>,
        /// The sample line the style is judged on.
        sample: String,
    },
    /// One authored metric rendered as a measured bar or box.
    #[serde(rename_all = "camelCase")]
    Metric {
        /// The canonical or declared name.
        name: String,
        /// The generated custom property that sizes the bar.
        css_var: String,
        /// The authored value, for the caption.
        px: f32,
        /// How the metric is shown: `width`, `height`, or `radius`.
        axis: &'static str,
    },
    /// One value presentation form, as finished text.
    #[serde(rename_all = "camelCase")]
    Value {
        /// What the form is called.
        label: String,
        /// The representative rendered value.
        value: String,
        /// The tone class it paints in, if not the default.
        tone_class: Option<&'static str>,
    },
    /// One action hint in one declared tone.
    #[serde(rename_all = "camelCase")]
    Hint {
        /// The canonical tone name.
        tone: &'static str,
        /// The custom property of the tone's color role.
        accent_var: String,
        /// The control that performs the action, e.g. `A`.
        chord: String,
        /// What it does, e.g. `CONFIRM`.
        action: String,
    },
    /// One structural shell band in the band diagram.
    #[serde(rename_all = "camelCase")]
    Band {
        /// The observation region name, e.g. `contextLine`.
        region: &'static str,
        /// The visible label inside the band.
        label: String,
        /// The authored band height, used as the diagram's flex weight.
        weight: f32,
    },
    /// One component state specimen, optionally as one control's presentation.
    #[serde(rename_all = "camelCase")]
    State {
        /// The owning control's canonical name, on the control pages.
        control: Option<&'static str>,
        /// The presentation form the page dispatches on: `row`, `toggle`,
        /// `slider`, `fader`, `meter`, or `mark`.
        presentation: &'static str,
        /// The canonical state name.
        state: &'static str,
        /// The visible label beside the specimen.
        label: String,
        /// The visible value beside the specimen.
        value: String,
        /// The custom property of the state's declared accent role.
        accent_var: String,
        /// The custom property of the state's declared keyline width.
        keyline_var: &'static str,
        /// The declared keyline width, for the evidence readback.
        keyline_px: f32,
        /// Whether the state draws the authored halo.
        halo: bool,
        /// Whether the state fills a band behind its row.
        fills_row: bool,
        /// Whether the state draws the reserved-column cursor.
        cursor: bool,
        /// The status mark text the state carries, if it carries one.
        mark: Option<String>,
        /// A normalized position for the level-bearing presentations.
        level: Option<f64>,
    },
    /// One shell composition filled with representative content.
    #[serde(rename_all = "camelCase")]
    Composition {
        /// The canonical composition name.
        composition: &'static str,
        /// The observation name of the region it fills, or `wholeFrame`.
        region: &'static str,
        /// The render form the page dispatches on: `frame`, `contextSwitch`,
        /// `header`, `section`, `stripRow`, `bank`, `panel`, or `footer`.
        form: &'static str,
        /// Representative lines, for every form but the bank.
        entries: Vec<GalleryCompositionEntry>,
        /// Representative track columns, for the bank form.
        columns: Vec<GalleryBankColumn>,
    },
}

/// One titled group of specimens.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GallerySection {
    /// The group heading, as it reads on screen.
    heading: String,
    /// The specimens in declared order.
    specimens: Vec<GallerySpecimen>,
}

/// One representative responsive witness column of a gallery document.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GalleryDensityColumn {
    /// The canonical representative viewport name.
    policy: &'static str,
    /// The column caption, naming the policy and its authored viewport.
    label: String,
    /// The generated custom property of this policy's mixer column width,
    /// so the bank specimen's columns size from the token table.
    column_width_var: &'static str,
    /// The specimen groups.
    sections: Vec<GallerySection>,
}

/// One gallery-scene document: everything the page needs to paint one page at
/// both representative responsive witnesses.
///
/// This is not a `SemanticGraphicalViewModel` and is never pushed on the
/// production projection channel; see the module docs.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GalleryDocument {
    /// The scene-local schema identity, [`GALLERY_DOCUMENT_SCHEMA`].
    schema: &'static str,
    /// The canonical page identity.
    page: &'static str,
    /// The page title, as it reads on screen.
    title: &'static str,
    /// The digit that selects the page, where one does.
    digit_label: Option<&'static str>,
    /// The 1-based position in the declared order.
    page_number: usize,
    /// [`GALLERY_PAGE_COUNT`].
    pages_declared: usize,
    /// The footer legend explaining the scene-local keys.
    step_hint: String,
    /// The full page index, one entry per declared page.
    index: Vec<GalleryIndexEntry>,
    /// The two gallery reference viewports, in [`GALLERY_VIEWPORTS`] order.
    densities: Vec<GalleryDensityColumn>,
}

impl GalleryDocument {
    /// The canonical page identity this document renders.
    pub const fn page(&self) -> &'static str {
        self.page
    }
}

/// The generated custom property name of one color role.
///
/// The mechanical mapping `token_export` declares: lowercase, `/` → `-`,
/// `--` prefix.
fn color_css_var(color: SemanticColor) -> String {
    format!("--{}", color.canonical_name().replace('/', "-"))
}

/// The generated custom property name of one spacing step.
fn spacing_css_var(step: SpacingStep) -> String {
    format!("--{}", step.canonical_name().replace('/', "-"))
}

/// The generated custom property name of one radius.
const fn radius_css_var(radius: Radius) -> &'static str {
    match radius {
        Radius::None => "--radius-none",
        Radius::Small => "--radius-small",
        Radius::Large => "--radius-large",
    }
}

/// The declared name of one radius, for the caption.
const fn radius_name(radius: Radius) -> &'static str {
    match radius {
        Radius::None => "radius/none",
        Radius::Small => "radius/small",
        Radius::Large => "radius/large",
    }
}

/// The gallery stylesheet class carrying one type style's token metrics.
fn type_css_class(style: TypeStyle) -> String {
    format!(
        "g-type-{}",
        style.canonical_name().to_lowercase().replace('/', "-")
    )
}

/// One authored weight as its CSS `font-weight` number.
const fn weight_number(weight: FontWeight) -> u16 {
    match weight {
        FontWeight::Regular => 400,
        FontWeight::Medium => 500,
        FontWeight::SemiBold => 600,
        FontWeight::Bold => 700,
    }
}

/// The authored value of one color role as `#rrggbb`.
fn color_hex(color: SemanticColor) -> String {
    let resolved = color.resolve();
    format!(
        "#{:02x}{:02x}{:02x}",
        resolved.r(),
        resolved.g(),
        resolved.b()
    )
}

/// A normalized level in the bound MidiHexadecimal presentation form:
/// `(value − min) / (max − min) × 127` as two uppercase hex digits.
fn midi_hex(fraction: f64) -> String {
    let scaled = (fraction.clamp(0.0, 1.0) * 127.0).round() as u8;
    format!("{scaled:02X}")
}

/// The keyline custom property a state's declared width resolves to.
///
/// The vocabulary declares exactly two keyline widths, so anything else is a
/// defect this function refuses to paper over.
fn keyline_css_var(keyline_px: f32) -> &'static str {
    if keyline_px == KEYLINE_EMPHASIS_PX {
        "--keyline-emphasis"
    } else {
        debug_assert!(keyline_px == KEYLINE_RESTING_PX);
        "--keyline-resting"
    }
}

/// The representative status detail one state's specimen is derived with.
///
/// Loading shows the first authored progress word and Error a representative
/// typed failure — a gallery privilege; production shows the real lifecycle
/// word and the real typed failure.
const fn specimen_detail(state: ComponentState) -> StatusDetail<'static> {
    match state {
        ComponentState::Loading => StatusDetail::Progress(LoadingPhase::Loading),
        ComponentState::Error => StatusDetail::Failure("REJECTED: OUT OF RANGE"),
        ComponentState::Resting
        | ComponentState::Focused
        | ComponentState::Adjusting
        | ComponentState::Disabled
        | ComponentState::Unavailable
        | ComponentState::Muted
        | ComponentState::Soloed
        | ComponentState::Selected => StatusDetail::None,
    }
}

/// Builds one state specimen from the declared state treatment.
///
/// Everything here is read from the vocabulary's own primitives —
/// [`ComponentState::appearance`], [`status_mark`], [`focus::draws_cursor`] —
/// so the document can never carry a treatment the vocabulary does not
/// declare.
fn state_specimen(
    control: Option<ComponentControl>,
    presentation: &'static str,
    state: ComponentState,
    label: &str,
    value: &str,
    level: Option<f64>,
) -> GallerySpecimen {
    let appearance = state.appearance();
    let mark = match status_mark(state, specimen_detail(state)) {
        Some(StatusMark::Text { text, .. }) => Some(text.to_owned()),
        Some(StatusMark::Selection { mark, .. }) => {
            Some(format!("selection {}", mark.canonical_name()))
        }
        None => None,
    };
    GallerySpecimen::State {
        control: control.map(ComponentControl::canonical_name),
        presentation,
        state: state.canonical_name(),
        label: label.to_owned(),
        value: value.to_owned(),
        accent_var: color_css_var(appearance.accent),
        keyline_var: keyline_css_var(appearance.keyline_px),
        keyline_px: appearance.keyline_px,
        halo: appearance.draws_halo,
        fills_row: appearance.fills_row,
        cursor: draws_cursor(state),
        mark,
        level,
    }
}

/// The presentation form one control's specimens are rendered as.
const fn control_presentation(control: ComponentControl) -> &'static str {
    match control {
        ComponentControl::ParameterRow
        | ComponentControl::ChoiceRow
        | ComponentControl::BrowserRow
        | ComponentControl::ModalOption => "row",
        ComponentControl::Toggle => "toggle",
        ComponentControl::CompactSlider => "slider",
        ComponentControl::Fader => "fader",
        ComponentControl::Meter => "meter",
    }
}

/// The representative content one control's specimens carry.
///
/// Representative, deliberately: the gallery is the one surface allowed to
/// hold specimens; production controls read their content from the projection.
const fn control_content(control: ComponentControl) -> (&'static str, &'static str, Option<f64>) {
    match control {
        ComponentControl::ParameterRow => ("ATTACK", "0.120 s", Some(0.12)),
        ComponentControl::ChoiceRow => ("WAVEFORM", "SINE", None),
        ComponentControl::Toggle => ("SUSTAIN HOLD", "ON", Some(1.0)),
        ComponentControl::CompactSlider => ("PATCH VOLUME", "0.500", Some(0.5)),
        ComponentControl::Fader => ("T00 LEVEL", "4F", Some(0.62)),
        ComponentControl::Meter => ("T00 METER", "0.482", Some(0.482)),
        ComponentControl::BrowserRow => ("SOUNDFONT", "FACTORY.SF2", None),
        ComponentControl::ModalOption => ("ENGINE", "WAVETABLE", None),
    }
}

/// One control's section: a specimen per state the control declares
/// applicable, in declared state order.
fn control_section(control: ComponentControl) -> GallerySection {
    let (label, value, level) = control_content(control);
    let states = control.applicable_states();
    GallerySection {
        heading: format!(
            "{} — {} DECLARED STATES",
            control.canonical_name().to_uppercase(),
            states.len()
        ),
        specimens: states
            .iter()
            .map(|state| {
                state_specimen(
                    Some(control),
                    control_presentation(control),
                    *state,
                    label,
                    value,
                    level,
                )
            })
            .collect(),
    }
}

/// The render form one composition's specimen dispatches on.
const fn composition_form(composition: ShellComposition) -> &'static str {
    match composition {
        ShellComposition::ApplicationShell => "frame",
        ShellComposition::ContextSwitch => "contextSwitch",
        ShellComposition::IdentityHeader => "header",
        ShellComposition::Section => "section",
        ShellComposition::PatchStripRow => "stripRow",
        ShellComposition::PatchStrip => "strip",
        ShellComposition::MixerStripBank => "bank",
        ShellComposition::UtilityInspectorPanel => "panel",
        ShellComposition::CapabilityDetailShell => "detail",
        ShellComposition::Footer => "footer",
    }
}

/// The observation name of the region a composition fills, with the whole
/// frame named explicitly.
fn composition_region_name(composition: ShellComposition) -> &'static str {
    composition
        .region()
        .observation_name()
        .unwrap_or("wholeFrame")
}

/// One representative bank column.
fn bank_column(
    track: &str,
    level: f64,
    pan: &'static str,
    state_line: &str,
    focused: bool,
) -> GalleryBankColumn {
    GalleryBankColumn {
        track: track.to_owned(),
        level,
        level_hex: midi_hex(level),
        pan,
        state_line: state_line.to_owned(),
        focused,
    }
}

/// One composition's specimen, filled with representative content.
fn composition_specimen(composition: ShellComposition) -> GallerySpecimen {
    let (entries, columns) = match composition {
        ShellComposition::ApplicationShell => (
            OBSERVED_REGION_NAMES
                .iter()
                .map(|region| GalleryCompositionEntry::new(region, None, Some("muted")))
                .collect(),
            Vec::new(),
        ),
        ShellComposition::ContextSwitch => (
            vec![
                GalleryCompositionEntry::new("CREST SYNTH", None, None),
                GalleryCompositionEntry::new("* PATCH", None, Some("focus")),
                GalleryCompositionEntry::new("MIXER", None, Some("muted")),
                GalleryCompositionEntry::new("READY", None, Some("positive")),
            ],
            Vec::new(),
        ),
        ShellComposition::IdentityHeader => (
            vec![
                GalleryCompositionEntry::new("PATCH", None, None),
                GalleryCompositionEntry::new("/ Lead Pad · hidef-soundfont", None, Some("muted")),
                GalleryCompositionEntry::new("ENV / ATTACK", None, Some("focus")),
            ],
            Vec::new(),
        ),
        ShellComposition::Section => (
            vec![
                GalleryCompositionEntry::new("ENVELOPE", None, Some("muted")),
                GalleryCompositionEntry::new("ATTACK", Some("0.120 s"), None),
                GalleryCompositionEntry::new("DECAY", Some("0.340 s"), None),
                GalleryCompositionEntry::new("SUSTAIN", Some("0.800"), None),
            ],
            Vec::new(),
        ),
        ShellComposition::PatchStripRow => (
            vec![GalleryCompositionEntry::new(
                "ATTACK",
                Some("0.120 s"),
                None,
            )],
            Vec::new(),
        ),
        // Groups arranging groups: the identity-and-routing header, then a
        // titled group per structure with its rows nested beneath it. An
        // empty slot keeps its group and its occupancy row, because an empty
        // position an operator cannot see is a position they cannot fill.
        ShellComposition::PatchStrip => (
            vec![
                GalleryCompositionEntry::new("LEAD PAD", Some("MIDI 03 · T03"), Some("focus")),
                GalleryCompositionEntry::new("INSTRUMENT", None, Some("muted")),
                GalleryCompositionEntry::new("Engine", Some("HiDef SoundFont"), None),
                GalleryCompositionEntry::new("AMP ENVELOPE", None, Some("muted")),
                GalleryCompositionEntry::new("Attack", Some("0.120 ms"), None),
                GalleryCompositionEntry::new("Slot 1", None, Some("muted")),
                GalleryCompositionEntry::new("Slot 1", Some("Chorus"), None),
                GalleryCompositionEntry::new("Amount", Some("0.500"), None),
                GalleryCompositionEntry::new("Slot 2", None, Some("muted")),
                GalleryCompositionEntry::new("Slot 2", Some("Empty"), Some("muted")),
            ],
            Vec::new(),
        ),
        ShellComposition::MixerStripBank => (
            Vec::new(),
            vec![
                bank_column("T00", 0.62, "C", "M -- · S --", true),
                bank_column("T01", 0.35, "L", "M -- · S --", false),
                bank_column("T02", 0.18, "C", "M ON · S --", false),
                bank_column("T03", 0.85, "R", "M -- · S ON", false),
            ],
        ),
        ShellComposition::UtilityInspectorPanel => (
            vec![
                GalleryCompositionEntry::new("UTILITY", None, Some("muted")),
                GalleryCompositionEntry::new("MASTER VOLUME", Some("--"), Some("muted")),
                GalleryCompositionEntry::new("PATCH VOLUME", Some("0.500"), None),
                GalleryCompositionEntry::new("OUTPUT TRACK", Some("T03"), None),
            ],
            Vec::new(),
        ),
        // One composition for either subject. The read-only row carries the
        // declared interaction as text, so it is distinguishable with the
        // colour removed.
        ShellComposition::CapabilityDetailShell => (
            vec![
                GalleryCompositionEntry::new("DETAIL", Some("HiDef SoundFont"), Some("focus")),
                GalleryCompositionEntry::new("Preset", Some("Grand Piano"), None),
                GalleryCompositionEntry::new("SoundFont File", Some("READ-ONLY"), Some("muted")),
            ],
            Vec::new(),
        ),
        ShellComposition::Footer => (
            vec![
                GalleryCompositionEntry::new("PATCH / ENV / ATTACK", None, Some("secondary")),
                GalleryCompositionEntry::new("D-PAD NAV", None, Some("muted")),
                GalleryCompositionEntry::new("A CONFIRM", None, Some("focus")),
                GalleryCompositionEntry::new("B BACK", None, Some("secondary")),
            ],
            Vec::new(),
        ),
    };
    GallerySpecimen::Composition {
        composition: composition.canonical_name(),
        region: composition_region_name(composition),
        form: composition_form(composition),
        entries,
        columns,
    }
}

/// One composition's titled section.
fn composition_section(composition: ShellComposition) -> GallerySection {
    GallerySection {
        heading: format!(
            "{} — {}",
            composition.canonical_name().to_uppercase(),
            composition_region_name(composition)
        ),
        specimens: vec![composition_specimen(composition)],
    }
}

/// The section groups one page composes at one density.
///
/// Exhaustive over the closed page set with no wildcard, so a sixteenth page
/// is a compile error naming this function rather than a page that renders
/// blank.
fn page_sections(
    page: ComponentGalleryPage,
    policy: RepresentativeViewport,
) -> Vec<GallerySection> {
    match page {
        ComponentGalleryPage::Colors => vec![GallerySection {
            heading: format!("{} DECLARED COLORS", ALL_COLORS.len()),
            specimens: ALL_COLORS
                .into_iter()
                .map(|color| GallerySpecimen::Color {
                    name: color.canonical_name(),
                    hex: color_hex(color),
                    css_var: color_css_var(color),
                })
                .collect(),
        }],
        ComponentGalleryPage::Type => vec![GallerySection {
            heading: format!("{} TYPE STYLES AT AUTHORED METRICS", ALL_TYPE_STYLES.len()),
            specimens: ALL_TYPE_STYLES
                .into_iter()
                .map(|style| {
                    let metrics = style.metrics();
                    GallerySpecimen::Type {
                        name: style.canonical_name(),
                        css_class: type_css_class(style),
                        size_px: metrics.size_px,
                        line_height_px: metrics.line_height_px,
                        weight: weight_number(metrics.weight),
                        tracking_px: metrics.tracking_px,
                        tone_class: None,
                        sample: "CREST SYNTH 0123".to_owned(),
                    }
                })
                .collect(),
        }],
        ComponentGalleryPage::SpacingAndGeometry => vec![
            GallerySection {
                heading: "SPACING STEPS".to_owned(),
                specimens: ALL_SPACING_STEPS
                    .into_iter()
                    .map(|step| GallerySpecimen::Metric {
                        name: step.canonical_name().to_owned(),
                        css_var: spacing_css_var(step),
                        px: step.resolve(),
                        axis: "width",
                    })
                    .collect(),
            },
            GallerySection {
                heading: "RADII".to_owned(),
                specimens: ALL_RADII
                    .into_iter()
                    .map(|radius| GallerySpecimen::Metric {
                        name: radius_name(radius).to_owned(),
                        css_var: radius_css_var(radius).to_owned(),
                        px: radius.resolve(),
                        axis: "radius",
                    })
                    .collect(),
            },
            GallerySection {
                heading: "KEYLINES AND TARGET".to_owned(),
                specimens: vec![
                    GallerySpecimen::Metric {
                        name: "keyline/resting".to_owned(),
                        css_var: "--keyline-resting".to_owned(),
                        px: KEYLINE_RESTING_PX,
                        axis: "height",
                    },
                    GallerySpecimen::Metric {
                        name: "keyline/emphasis".to_owned(),
                        css_var: "--keyline-emphasis".to_owned(),
                        px: KEYLINE_EMPHASIS_PX,
                        axis: "height",
                    },
                    GallerySpecimen::Metric {
                        name: "min-interactive-target".to_owned(),
                        css_var: "--min-interactive-target".to_owned(),
                        px: MIN_INTERACTIVE_TARGET_PX,
                        axis: "width",
                    },
                ],
            },
        ],
        ComponentGalleryPage::InteractionStates => {
            // All ten declared states, grouped: the three interaction states
            // the page is named for, then availability and lifecycle, then the
            // mixer and selection states. This page is the closed-set anchor
            // the coverage assertion leans on: every declared state has a
            // specimen here at both densities.
            let group = |heading: &str, states: &[ComponentState]| GallerySection {
                heading: heading.to_owned(),
                specimens: states
                    .iter()
                    .map(|state| {
                        state_specimen(
                            None,
                            "row",
                            *state,
                            &state.canonical_name().to_uppercase(),
                            "0.500",
                            Some(0.5),
                        )
                    })
                    .collect(),
            };
            vec![
                group(
                    "INTERACTION",
                    &[
                        ComponentState::Resting,
                        ComponentState::Focused,
                        ComponentState::Adjusting,
                    ],
                ),
                group(
                    "AVAILABILITY AND LIFECYCLE",
                    &[
                        ComponentState::Disabled,
                        ComponentState::Loading,
                        ComponentState::Unavailable,
                        ComponentState::Error,
                    ],
                ),
                group(
                    "MIXER AND SELECTION",
                    &[
                        ComponentState::Muted,
                        ComponentState::Soloed,
                        ComponentState::Selected,
                    ],
                ),
            ]
        }
        ComponentGalleryPage::TextAndHairlines => vec![
            GallerySection {
                heading: "TEXT ROLES".to_owned(),
                specimens: [
                    (SemanticColor::TextPrimary, None),
                    (SemanticColor::TextSecondary, Some("secondary")),
                    (SemanticColor::TextMuted, Some("muted")),
                ]
                .into_iter()
                .map(|(color, tone_class)| GallerySpecimen::Type {
                    name: color.canonical_name(),
                    css_class: type_css_class(TypeStyle::BodyDefault),
                    size_px: TypeStyle::BodyDefault.metrics().size_px,
                    line_height_px: TypeStyle::BodyDefault.metrics().line_height_px,
                    weight: weight_number(TypeStyle::BodyDefault.metrics().weight),
                    tracking_px: TypeStyle::BodyDefault.metrics().tracking_px,
                    tone_class,
                    sample: "The quick brown synth 0123".to_owned(),
                })
                .collect(),
            },
            GallerySection {
                heading: "RULES".to_owned(),
                specimens: vec![
                    GallerySpecimen::Metric {
                        name: "hairline (border/default at keyline/resting)".to_owned(),
                        css_var: "--keyline-resting".to_owned(),
                        px: KEYLINE_RESTING_PX,
                        axis: "height",
                    },
                    GallerySpecimen::Metric {
                        name: "keyline (border/strong at keyline/emphasis)".to_owned(),
                        css_var: "--keyline-emphasis".to_owned(),
                        px: KEYLINE_EMPHASIS_PX,
                        axis: "height",
                    },
                ],
            },
        ],
        ComponentGalleryPage::ValuesAndStatus => vec![
            GallerySection {
                heading: "VALUE PRESENTATION FORMS".to_owned(),
                specimens: vec![
                    GallerySpecimen::Value {
                        label: "CONTINUOUS · THREE PLACES".to_owned(),
                        value: "0.500".to_owned(),
                        tone_class: None,
                    },
                    GallerySpecimen::Value {
                        label: "LEVEL · MIDI HEXADECIMAL".to_owned(),
                        value: midi_hex(0.5),
                        tone_class: Some("patch"),
                    },
                    GallerySpecimen::Value {
                        label: "MIDI DECIMAL".to_owned(),
                        value: "64".to_owned(),
                        tone_class: Some("secondary"),
                    },
                    GallerySpecimen::Value {
                        label: "TOGGLE".to_owned(),
                        value: "ON".to_owned(),
                        tone_class: Some("positive"),
                    },
                ],
            },
            GallerySection {
                heading: "STATUS MARKS".to_owned(),
                specimens: ALL_COMPONENT_STATES
                    .into_iter()
                    .filter(|state| status_mark(*state, specimen_detail(*state)).is_some())
                    .map(|state| {
                        state_specimen(
                            None,
                            "mark",
                            state,
                            &state.canonical_name().to_uppercase(),
                            "",
                            None,
                        )
                    })
                    .collect(),
            },
        ],
        ComponentGalleryPage::ActionHints => vec![GallerySection {
            heading: format!("{} HINT TONES", ALL_HINT_TONES.len()),
            specimens: ALL_HINT_TONES
                .into_iter()
                .map(|tone| {
                    let (chord, action) = match tone {
                        HintTone::Neutral => ("D-PAD", "NAV"),
                        HintTone::Focus => ("A", "CONFIRM"),
                        HintTone::Adjust => ("W/S", "ADJUST"),
                        HintTone::Back => ("B", "BACK"),
                    };
                    GallerySpecimen::Hint {
                        tone: tone.canonical_name(),
                        accent_var: color_css_var(tone.color()),
                        chord: chord.to_owned(),
                        action: action.to_owned(),
                    }
                })
                .collect(),
        }],
        ComponentGalleryPage::ShellBands => {
            let contract = ResponsiveShellContract::get();
            let viewport = policy.fixture();
            let workspace = viewport.height_px
                - contract.context_line.preferred_px
                - contract.identity_header.preferred_px
                - contract.footer.preferred_px;
            vec![GallerySection {
                heading: format!(
                    "FIVE STRUCTURAL BANDS · AUTHORED {} × {}",
                    viewport.width_px, viewport.height_px
                ),
                specimens: vec![
                    GallerySpecimen::Band {
                        region: "contextLine",
                        label: format!(
                            "CONTEXT LINE · {} px preferred",
                            contract.context_line.preferred_px
                        ),
                        weight: contract.context_line.preferred_px,
                    },
                    GallerySpecimen::Band {
                        region: "identityHeader",
                        label: format!(
                            "IDENTITY HEADER · {} px preferred",
                            contract.identity_header.preferred_px
                        ),
                        weight: contract.identity_header.preferred_px,
                    },
                    GallerySpecimen::Band {
                        region: "mainWorkspace",
                        label: format!("MAIN WORKSPACE · {} px available", workspace),
                        weight: workspace,
                    },
                    GallerySpecimen::Band {
                        region: "persistentSideRegion",
                        label: format!(
                            "SIDE REGION · {} px preferred",
                            contract.side_track.preferred_px
                        ),
                        weight: workspace,
                    },
                    GallerySpecimen::Band {
                        region: "footer",
                        label: format!("FOOTER · {} px preferred", contract.footer.preferred_px),
                        weight: contract.footer.preferred_px,
                    },
                ],
            }]
        }
        ComponentGalleryPage::ParameterAndChoiceRows => vec![
            control_section(ComponentControl::ParameterRow),
            control_section(ComponentControl::ChoiceRow),
        ],
        ComponentGalleryPage::TogglesAndSliders => vec![
            control_section(ComponentControl::Toggle),
            control_section(ComponentControl::CompactSlider),
        ],
        ComponentGalleryPage::FadersAndMeters => vec![
            control_section(ComponentControl::Fader),
            control_section(ComponentControl::Meter),
        ],
        ComponentGalleryPage::BrowserAndModalOptions => vec![
            control_section(ComponentControl::BrowserRow),
            control_section(ComponentControl::ModalOption),
        ],
        ComponentGalleryPage::ShellAndContextSwitch => vec![
            composition_section(ShellComposition::ApplicationShell),
            composition_section(ShellComposition::ContextSwitch),
        ],
        ComponentGalleryPage::HeadersAndSections => vec![
            composition_section(ShellComposition::IdentityHeader),
            composition_section(ShellComposition::Section),
            composition_section(ShellComposition::CapabilityDetailShell),
        ],
        ComponentGalleryPage::StripPanelAndFooter => vec![
            composition_section(ShellComposition::PatchStripRow),
            composition_section(ShellComposition::PatchStrip),
            composition_section(ShellComposition::MixerStripBank),
            composition_section(ShellComposition::UtilityInspectorPanel),
            composition_section(ShellComposition::Footer),
        ],
    }
}

/// The generated bounded custom property used by mixer-column specimens.
const fn column_width_var(_policy: RepresentativeViewport) -> &'static str {
    "--shell-mixer-column-preferred"
}

/// One representative responsive witness column of one page's document.
fn density_column(
    page: ComponentGalleryPage,
    policy: RepresentativeViewport,
) -> GalleryDensityColumn {
    let viewport = policy.fixture();
    GalleryDensityColumn {
        policy: policy.canonical_name(),
        label: format!(
            "{} · REFERENCE {} × {}",
            policy.canonical_name().to_uppercase(),
            viewport.width_px,
            viewport.height_px
        ),
        column_width_var: column_width_var(policy),
        sections: page_sections(page, policy),
    }
}

/// Builds the gallery-scene document for one page: both authored densities,
/// the full page index, and the scene-local key legend.
pub fn gallery_document(page: ComponentGalleryPage) -> GalleryDocument {
    GalleryDocument {
        schema: GALLERY_DOCUMENT_SCHEMA,
        page: page.canonical_name(),
        title: page.title(),
        digit_label: page.digit_label(),
        page_number: page.index() + 1,
        pages_declared: GALLERY_PAGE_COUNT,
        step_hint: format!(
            "1-9 0 PAGES · {STEP_PREVIOUS_LABEL} PREV · {STEP_NEXT_LABEL} NEXT · CLOSE WINDOW TO FINISH"
        ),
        index: ALL_GALLERY_PAGES
            .into_iter()
            .map(|entry| GalleryIndexEntry {
                page: entry.canonical_name(),
                label: entry.index_label(),
                key: entry.digit_label().unwrap_or(STEP_ONLY_LABEL),
                active: entry == page,
            })
            .collect(),
        densities: GALLERY_VIEWPORTS
            .into_iter()
            .map(|policy| density_column(page, policy))
            .collect(),
    }
}

/// Builds every declared page's document, in declared order.
pub fn all_gallery_documents() -> Vec<GalleryDocument> {
    ALL_GALLERY_PAGES
        .into_iter()
        .map(gallery_document)
        .collect()
}

// ===========================================================================
// Coverage assertions, derived from the closed sets
// ===========================================================================

/// Every way `documents` falls short of the declared page, state, control, and
/// composition sets, as human-readable failures.
///
/// Derived from the closed sets ([`ALL_GALLERY_PAGES`],
/// [`ALL_COMPONENT_STATES`], [`ALL_COMPONENT_CONTROLS`],
/// [`ALL_SHELL_COMPOSITIONS`], [`MIXER_COLUMN_ANATOMY`]) rather than from a
/// list this file carries beside them, so a page or state added to the
/// vocabulary without a specimen fails here rather than being silently
/// omitted. The scene runs this before the window opens and refuses to start
/// on a failure: a gallery missing a declared specimen is a gallery that
/// would show an operator a smaller vocabulary than the one declared.
pub fn gallery_coverage_failures(documents: &[GalleryDocument]) -> Vec<String> {
    let mut failures = Vec::new();

    // Every declared page has exactly one document, in declared order.
    let pages: Vec<&str> = documents.iter().map(|document| document.page).collect();
    let declared: Vec<&str> = ALL_GALLERY_PAGES
        .into_iter()
        .map(ComponentGalleryPage::canonical_name)
        .collect();
    if pages != declared {
        failures.push(format!(
            "the document set does not cover the declared pages in declared order: {pages:?}"
        ));
    }

    // Every declared page is reachable by its digit or by stepping from the
    // first page — asked of the vocabulary itself, so an unbound page that
    // stepping cannot reach fails before the window opens.
    let mut reachable: BTreeSet<ComponentGalleryPage> = ALL_GALLERY_PAGES
        .into_iter()
        .filter(|page| page.digit().is_some())
        .collect();
    let mut walk = ALL_GALLERY_PAGES[0];
    reachable.insert(walk);
    while let Some(next) = walk.next() {
        reachable.insert(next);
        walk = next;
    }
    for page in ALL_GALLERY_PAGES {
        if !reachable.contains(&page) {
            failures.push(format!(
                "{} is reachable by neither digit nor stepping",
                page.canonical_name()
            ));
        }
    }

    // Per density: every document carries the density with at least one
    // specimen, every state has a specimen somewhere, every control covers
    // its declared states, and every composition appears.
    for policy in GALLERY_VIEWPORTS {
        let policy_name = policy.canonical_name();
        let mut states_covered: BTreeSet<&str> = BTreeSet::new();
        let mut controls_covered: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        let mut compositions_covered: BTreeSet<&str> = BTreeSet::new();

        for document in documents {
            let Some(column) = document
                .densities
                .iter()
                .find(|column| column.policy == policy_name)
            else {
                failures.push(format!(
                    "{} carries no {policy_name} density column",
                    document.page
                ));
                continue;
            };
            let specimens: Vec<&GallerySpecimen> = column
                .sections
                .iter()
                .flat_map(|section| section.specimens.iter())
                .collect();
            if specimens.is_empty() {
                failures.push(format!(
                    "{} renders no specimen at {policy_name}",
                    document.page
                ));
            }
            for specimen in specimens {
                match specimen {
                    GallerySpecimen::State { control, state, .. } => {
                        states_covered.insert(state);
                        if let Some(control) = control {
                            controls_covered.entry(control).or_default().insert(state);
                        }
                    }
                    GallerySpecimen::Composition {
                        composition,
                        columns,
                        ..
                    } => {
                        compositions_covered.insert(composition);
                        if *composition == ShellComposition::MixerStripBank.canonical_name()
                            && columns.is_empty()
                        {
                            failures.push(format!(
                                "the bank specimen carries no columns at {policy_name}"
                            ));
                        }
                        for column in columns {
                            if !column
                                .level_hex
                                .chars()
                                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase())
                                || column.level_hex.len() != 2
                            {
                                failures.push(format!(
                                    "bank column {} level readout {:?} is not the bound \
                                     MidiHexadecimal form",
                                    column.track, column.level_hex
                                ));
                            }
                        }
                    }
                    GallerySpecimen::Color { .. }
                    | GallerySpecimen::Type { .. }
                    | GallerySpecimen::Metric { .. }
                    | GallerySpecimen::Value { .. }
                    | GallerySpecimen::Hint { .. }
                    | GallerySpecimen::Band { .. } => {}
                }
            }
        }

        for state in ALL_COMPONENT_STATES {
            if !states_covered.contains(state.canonical_name()) {
                failures.push(format!(
                    "{} has no specimen at {policy_name}",
                    state.canonical_name()
                ));
            }
        }
        for control in ALL_COMPONENT_CONTROLS {
            let covered = controls_covered
                .get(control.canonical_name())
                .cloned()
                .unwrap_or_default();
            for state in control.applicable_states() {
                if !covered.contains(state.canonical_name()) {
                    failures.push(format!(
                        "{} declares {} applicable and has no specimen for it at {policy_name}",
                        control.canonical_name(),
                        state.canonical_name()
                    ));
                }
            }
        }
        for composition in ALL_SHELL_COMPOSITIONS {
            if !compositions_covered.contains(composition.canonical_name()) {
                failures.push(format!(
                    "{} has no specimen at {policy_name}",
                    composition.canonical_name()
                ));
            }
        }
    }

    // Every non-resting state specimen announces itself beyond color: a mark,
    // the halo, a row fill, the cursor, or a keyline width that differs from
    // resting. Checked over the built documents rather than re-asserting the
    // vocabulary's own invariant, so a builder that dropped a mark fails here.
    for document in documents {
        for column in &document.densities {
            for section in &column.sections {
                for specimen in &section.specimens {
                    if let GallerySpecimen::State {
                        state,
                        halo,
                        fills_row,
                        cursor,
                        mark,
                        keyline_px,
                        ..
                    } = specimen
                    {
                        let non_color = *halo
                            || *fills_row
                            || *cursor
                            || mark.is_some()
                            || *keyline_px != KEYLINE_RESTING_PX;
                        if *state != ComponentState::Resting.canonical_name() && !non_color {
                            failures.push(format!(
                                "{state} on {} carries no non-color evidence",
                                document.page
                            ));
                        }
                    }
                }
            }
        }
    }

    failures
}

// ===========================================================================
// The painted acknowledgment the page emits after each document
// ===========================================================================

/// The measured page viewport at ack time.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AckViewport {
    /// The window's inner width in CSS pixels.
    #[serde(default)]
    pub width_px: f32,
    /// The window's inner height in CSS pixels.
    #[serde(default)]
    pub height_px: f32,
}

/// One state specimen the page read back from its painted DOM.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AckState {
    /// The canonical state name, from the painted `data-state` attribute.
    #[serde(default)]
    pub state: String,
    /// The visible label beside the specimen.
    #[serde(default)]
    pub label: String,
    /// The colorless evidence the painted row carries, e.g.
    /// `keyline 3 px · halo · cursor >`.
    #[serde(default)]
    pub evidence: String,
}

/// One control the page read back, with the states its specimens painted.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AckControl {
    /// The canonical control name.
    #[serde(default)]
    pub control: String,
    /// The canonical state names its painted specimens carried.
    #[serde(default)]
    pub states: Vec<String>,
    /// A label a reader actually sees on the control's specimen.
    #[serde(default)]
    pub label: String,
}

/// One composition the page read back.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AckComposition {
    /// The canonical composition name.
    #[serde(default)]
    pub composition: String,
    /// The region name the specimen was painted under.
    #[serde(default)]
    pub region: String,
    /// A label a reader actually sees inside the specimen.
    #[serde(default)]
    pub label: String,
}

/// The bank specimen's painted column anatomy, read back per column.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AckBank {
    /// The `data-structure` names painted in the first column, in paint order.
    #[serde(default)]
    pub structures: Vec<String>,
    /// The first column's level readout, as painted.
    #[serde(default)]
    pub level_readout: String,
}

/// One density column the page read back from its painted DOM.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AckDensity {
    /// The canonical policy name.
    #[serde(default)]
    pub policy: String,
    /// Whether the column painted with positive area.
    #[serde(default)]
    pub painted: bool,
    /// The state specimens actually painted.
    #[serde(default)]
    pub states: Vec<AckState>,
    /// The controls actually painted.
    #[serde(default)]
    pub controls: Vec<AckControl>,
    /// The compositions actually painted.
    #[serde(default)]
    pub compositions: Vec<AckComposition>,
    /// The structural band regions painted (the ShellBands page).
    #[serde(default)]
    pub bands: Vec<String>,
    /// The bank specimen's painted anatomy (the StripPanelAndFooter page).
    #[serde(default)]
    pub bank: Option<AckBank>,
}

/// What the page actually painted for one gallery document, emitted on
/// [`GALLERY_PAINTED_EVENT`] after the paint.
///
/// Everything here is read back from the painted DOM — the page copies the
/// page, state, and viewport identity actually rendered, never the input
/// document — which is what keeps the observation evidence rather than plan.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GalleryPaintedAck {
    /// The canonical page identity painted.
    #[serde(default)]
    pub page: String,
    /// Whether the active page identity (title and number) is visible.
    #[serde(default)]
    pub title_visible: bool,
    /// The measured viewport.
    #[serde(default)]
    pub viewport: AckViewport,
    /// Whether every painted text run resolved to the authored typeface.
    #[serde(default)]
    pub typeface_resolved: bool,
    /// Every distinct computed color painted, as CSS `rgb()`/`rgba()` text.
    #[serde(default)]
    pub painted_colors: Vec<String>,
    /// Clipped or invisible specimens, named so a failure is actionable.
    #[serde(default)]
    pub defects: Vec<String>,
    /// Both authored densities as painted.
    #[serde(default)]
    pub densities: Vec<AckDensity>,
}

// ===========================================================================
// The ack ledger the observation is built from
// ===========================================================================

/// A reference viewport's position in [`GALLERY_VIEWPORTS`].
const fn policy_index(policy: RepresentativeViewport) -> usize {
    match policy {
        RepresentativeViewport::WideReference => 0,
        RepresentativeViewport::StandardReference => 1,
        RepresentativeViewport::Intermediate
        | RepresentativeViewport::Compact
        | RepresentativeViewport::ScaledText => {
            panic!("viewport is not part of the two-column gallery")
        }
    }
}

/// The policy a canonical name resolves to, or `None`.
fn policy_for_name(name: &str) -> Option<RepresentativeViewport> {
    GALLERY_VIEWPORTS
        .into_iter()
        .find(|policy| policy.canonical_name() == name)
}

/// The canonical name of one semantic control kind.
///
/// The kind vocabulary is owned by the control layer; this names it for the
/// observation without the observation depending on a `Debug` format.
const fn kind_name(kind: SemanticControlKind) -> &'static str {
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

/// The first `(kind, role)` pair, in declared order, that selects `control`.
///
/// Derived from [`control_for`] rather than written out here, so the record a
/// control carries names the pair the selection rule actually produces.
fn selecting_pair(control: ComponentControl) -> Option<(&'static str, &'static str)> {
    ALL_SEMANTIC_CONTROL_KINDS
        .into_iter()
        .flat_map(|kind| {
            ALL_PRESENTATION_ROLES
                .into_iter()
                .map(move |role| (kind, role))
        })
        .find(|(kind, role)| control_for(*kind, *role).control() == Some(control))
        .map(|(kind, role)| (kind_name(kind), role.canonical_name()))
}

/// What one density accumulated across the session's acks.
#[derive(Debug, Default)]
struct DensityLedger {
    painted: bool,
    /// state canonical name → (visible label, colorless evidence).
    states: BTreeMap<&'static str, (String, String)>,
    /// control canonical name → (states painted, visible label).
    controls: BTreeMap<&'static str, (BTreeSet<&'static str>, String)>,
    /// composition canonical name → (region, visible label).
    compositions: BTreeMap<&'static str, (String, String)>,
    /// structural band regions painted on the ShellBands page.
    bands: BTreeSet<String>,
    /// the bank specimen's painted anatomy, latest ack wins.
    bank: Option<AckBank>,
}

/// Everything the page acknowledged as painted, accumulated across the
/// session, plus the scene-local key requests the acks are correlated with.
///
/// Nothing writes the painted side of this except [`GalleryAckLedger::record_ack`]
/// consuming page acks; the request side records only that a key was pressed.
/// A page counts as reached by digit or step only when a request for it was
/// followed by its painted ack.
#[derive(Debug)]
struct GalleryAckLedger {
    digit_requests: [bool; GALLERY_PAGE_COUNT],
    step_requests: [bool; GALLERY_PAGE_COUNT],
    pages_acked: [bool; GALLERY_PAGE_COUNT],
    pages_reached_by_digit: [bool; GALLERY_PAGE_COUNT],
    pages_reached_by_step: [bool; GALLERY_PAGE_COUNT],
    pages_visited: Vec<ComponentGalleryPage>,
    unbound_key_presses: usize,
    unbound_key_page_changes: usize,
    densities: [DensityLedger; 2],
    painted_colors: BTreeSet<String>,
    defects: BTreeSet<String>,
    /// AND over every ack; `None` until the first ack arrives.
    typeface_resolved: Option<bool>,
    /// AND over every ack: the active page identity was visible.
    title_visible: bool,
    viewport: AckViewport,
    active_page: ComponentGalleryPage,
    documents_pushed: usize,
    acks_received: usize,
    invalid_acks: usize,
}

impl Default for GalleryAckLedger {
    fn default() -> Self {
        Self {
            digit_requests: [false; GALLERY_PAGE_COUNT],
            step_requests: [false; GALLERY_PAGE_COUNT],
            pages_acked: [false; GALLERY_PAGE_COUNT],
            pages_reached_by_digit: [false; GALLERY_PAGE_COUNT],
            pages_reached_by_step: [false; GALLERY_PAGE_COUNT],
            pages_visited: Vec::new(),
            unbound_key_presses: 0,
            unbound_key_page_changes: 0,
            densities: [DensityLedger::default(), DensityLedger::default()],
            painted_colors: BTreeSet::new(),
            defects: BTreeSet::new(),
            typeface_resolved: None,
            title_visible: true,
            viewport: AckViewport::default(),
            active_page: ALL_GALLERY_PAGES[0],
            documents_pushed: 0,
            acks_received: 0,
            invalid_acks: 0,
        }
    }
}

impl GalleryAckLedger {
    /// Records that a bound digit asked for this page.
    ///
    /// The page is not counted as reached until its painted ack arrives; the
    /// request alone proves only that a key was pressed.
    fn record_digit_request(&mut self, page: ComponentGalleryPage) {
        self.digit_requests[page.index()] = true;
    }

    /// Records that a step key asked for this page. Same contract as the
    /// digit: a step to a page that never acks is not a page the operator
    /// reached.
    fn record_step_request(&mut self, page: ComponentGalleryPage) {
        self.step_requests[page.index()] = true;
    }

    /// Records one key press that bound no page and no step.
    ///
    /// A step that ran into an end is deliberately not counted here: it is a
    /// bound key that correctly declined to move, not a key that binds
    /// nothing — which is the property the observation's
    /// `unbound_digit_retained_page` field exists to establish.
    fn record_unbound_key(&mut self, changed_page: bool) {
        self.unbound_key_presses += 1;
        if changed_page {
            self.unbound_key_page_changes += 1;
        }
    }

    /// Records one document push.
    fn record_document_pushed(&mut self) {
        self.documents_pushed += 1;
    }

    /// Consumes one painted ack, copying the page, state, and viewport
    /// identity the page reports as actually rendered.
    ///
    /// Names are resolved against the closed vocabulary sets; a name the
    /// vocabulary does not declare is recorded as a defect rather than
    /// entering a count.
    fn record_ack(&mut self, ack: &GalleryPaintedAck) {
        self.acks_received += 1;
        let Some(page) = ComponentGalleryPage::for_canonical_name(&ack.page) else {
            self.invalid_acks += 1;
            self.defects
                .insert(format!("ack for undeclared page: {:?}", ack.page));
            return;
        };
        if !self.pages_acked[page.index()] {
            self.pages_acked[page.index()] = true;
            self.pages_visited.push(page);
        }
        if self.digit_requests[page.index()] {
            self.pages_reached_by_digit[page.index()] = true;
        }
        if self.step_requests[page.index()] {
            self.pages_reached_by_step[page.index()] = true;
        }
        self.active_page = page;
        self.viewport = ack.viewport;
        self.title_visible &= ack.title_visible;
        self.typeface_resolved = Some(
            self.typeface_resolved
                .map_or(ack.typeface_resolved, |sofar| {
                    sofar && ack.typeface_resolved
                }),
        );
        for color in &ack.painted_colors {
            self.painted_colors.insert(color.clone());
        }
        for defect in &ack.defects {
            self.defects.insert(defect.clone());
        }
        for density in &ack.densities {
            let Some(policy) = policy_for_name(&density.policy) else {
                self.defects
                    .insert(format!("ack for undeclared density: {:?}", density.policy));
                continue;
            };
            let ledger = &mut self.densities[policy_index(policy)];
            ledger.painted |= density.painted;
            for state in &density.states {
                let Some(declared) = ALL_COMPONENT_STATES
                    .into_iter()
                    .find(|declared| declared.canonical_name() == state.state)
                else {
                    self.defects
                        .insert(format!("ack for undeclared state: {:?}", state.state));
                    continue;
                };
                ledger
                    .states
                    .entry(declared.canonical_name())
                    .or_insert_with(|| (state.label.clone(), state.evidence.clone()));
            }
            for control in &density.controls {
                let Some(declared) = ALL_COMPONENT_CONTROLS
                    .into_iter()
                    .find(|declared| declared.canonical_name() == control.control)
                else {
                    self.defects
                        .insert(format!("ack for undeclared control: {:?}", control.control));
                    continue;
                };
                let entry = ledger
                    .controls
                    .entry(declared.canonical_name())
                    .or_insert_with(|| (BTreeSet::new(), control.label.clone()));
                for state in &control.states {
                    if let Some(declared_state) = ALL_COMPONENT_STATES
                        .into_iter()
                        .find(|declared_state| declared_state.canonical_name() == *state)
                    {
                        entry.0.insert(declared_state.canonical_name());
                    }
                }
            }
            for composition in &density.compositions {
                let Some(declared) = ALL_SHELL_COMPOSITIONS
                    .into_iter()
                    .find(|declared| declared.canonical_name() == composition.composition)
                else {
                    self.defects.insert(format!(
                        "ack for undeclared composition: {:?}",
                        composition.composition
                    ));
                    continue;
                };
                ledger
                    .compositions
                    .entry(declared.canonical_name())
                    .or_insert_with(|| (composition.region.clone(), composition.label.clone()));
            }
            for band in &density.bands {
                ledger.bands.insert(band.clone());
            }
            if let Some(bank) = &density.bank {
                ledger.bank = Some(bank.clone());
            }
        }
    }

    fn acked_page_count(&self) -> usize {
        self.pages_acked.iter().filter(|acked| **acked).count()
    }

    fn digit_reached_page_count(&self) -> usize {
        self.pages_reached_by_digit
            .iter()
            .filter(|reached| **reached)
            .count()
    }

    fn step_reached_page_count(&self) -> usize {
        self.pages_reached_by_step
            .iter()
            .filter(|reached| **reached)
            .count()
    }

    /// How many states acked a specimen at *every* declared policy.
    ///
    /// NFR-004 asks for a specimen at both authored sizes, so a state that
    /// reached only the roomier column is not counted.
    fn acked_state_count(&self) -> usize {
        ALL_COMPONENT_STATES
            .into_iter()
            .filter(|state| {
                self.densities
                    .iter()
                    .all(|ledger| ledger.states.contains_key(state.canonical_name()))
            })
            .count()
    }

    /// Whether one control acked every state it declares, at every policy.
    fn control_fully_acked(&self, control: ComponentControl) -> bool {
        self.densities.iter().all(|ledger| {
            ledger
                .controls
                .get(control.canonical_name())
                .is_some_and(|(states, _)| {
                    control
                        .applicable_states()
                        .iter()
                        .all(|state| states.contains(state.canonical_name()))
                })
        })
    }

    fn acked_control_count(&self) -> usize {
        ALL_COMPONENT_CONTROLS
            .into_iter()
            .filter(|control| self.control_fully_acked(*control))
            .count()
    }

    fn composition_fully_acked(&self, composition: ShellComposition) -> bool {
        self.densities.iter().all(|ledger| {
            ledger
                .compositions
                .contains_key(composition.canonical_name())
        })
    }

    fn acked_composition_count(&self) -> usize {
        ALL_SHELL_COMPOSITIONS
            .into_iter()
            .filter(|composition| self.composition_fully_acked(*composition))
            .count()
    }

    /// How many `(kind, role)` pairs resolve to a control whose specimens the
    /// page has not fully acked — a pair an operator cannot see the answer to.
    fn unmapped_kind_role_pairs(&self) -> usize {
        ALL_SEMANTIC_CONTROL_KINDS
            .into_iter()
            .flat_map(|kind| {
                ALL_PRESENTATION_ROLES
                    .into_iter()
                    .map(move |role| (kind, role))
            })
            .filter(|(kind, role)| {
                control_for(*kind, *role)
                    .control()
                    .is_some_and(|control| !self.control_fully_acked(control))
            })
            .count()
    }

    /// How many declared controls no `(kind, role)` pair resolves to.
    ///
    /// A control nothing can ask for is dead code the gallery would still
    /// happily show, so this is asked of the selection function rather than
    /// of the acks.
    fn controls_unreachable_by_any_pair(&self) -> usize {
        ALL_COMPONENT_CONTROLS
            .into_iter()
            .filter(|control| {
                !ALL_SEMANTIC_CONTROL_KINDS.into_iter().any(|kind| {
                    ALL_PRESENTATION_ROLES
                        .into_iter()
                        .any(|role| control_for(kind, role).control() == Some(*control))
                })
            })
            .count()
    }

    /// Whether no two acked states shared their colorless evidence.
    ///
    /// Compared over what was painted, not over what the vocabulary declares:
    /// a specimen that dropped its status mark fails here even though the
    /// declaration still carries one. Each density is compared against
    /// itself, because the same state legitimately paints once per policy.
    fn states_distinguishable_without_color(&self) -> bool {
        self.densities.iter().all(|ledger| {
            let evidence: Vec<&String> = ledger
                .states
                .values()
                .map(|(_, evidence)| evidence)
                .collect();
            evidence.iter().enumerate().all(|(index, first)| {
                evidence
                    .iter()
                    .skip(index + 1)
                    .all(|second| first != second)
            })
        })
    }

    /// Whether all five structural regions acked at both policies.
    fn bands_retained_both_viewports(&self) -> bool {
        self.densities.iter().all(|ledger| {
            OBSERVED_REGION_NAMES
                .iter()
                .all(|region| ledger.bands.contains(*region))
        })
    }

    /// Whether every color the page reported painting resolves through the
    /// vocabulary — as an authored role, or as a role at the authored halo
    /// opacity.
    fn token_source_exact(&self) -> bool {
        !self.painted_colors.is_empty()
            && self
                .painted_colors
                .iter()
                .all(|color| css_color_resolves(color))
    }

    /// The bank ledger merged over both densities: painted at both, or `None`.
    fn bank_both_densities(&self) -> Option<(&AckBank, &AckBank)> {
        match (&self.densities[0].bank, &self.densities[1].bank) {
            (Some(desktop), Some(deck)) => Some((desktop, deck)),
            _ => None,
        }
    }

    /// How many declared column structures the bank specimen painted at both
    /// densities.
    fn bank_structures_painted(&self) -> usize {
        self.bank_both_densities().map_or(0, |(desktop, deck)| {
            MIXER_COLUMN_ANATOMY
                .iter()
                .filter(|structure| {
                    desktop
                        .structures
                        .iter()
                        .any(|painted| painted == *structure)
                        && deck.structures.iter().any(|painted| painted == *structure)
                })
                .count()
        })
    }

    /// How many painted column names fall outside the declared anatomy.
    ///
    /// Computed here against [`MIXER_COLUMN_ANATOMY`] rather than trusted from
    /// the page: the page reports the raw painted names, and the closed
    /// anatomy the count is judged against is Rust-owned.
    fn bank_names_beyond_anatomy(&self) -> usize {
        self.bank_both_densities().map_or(0, |(desktop, deck)| {
            [desktop, deck]
                .into_iter()
                .flat_map(|bank| bank.structures.iter())
                .filter(|painted| {
                    !MIXER_COLUMN_ANATOMY
                        .iter()
                        .any(|declared| declared == painted)
                })
                .count()
        })
    }

    /// Whether the bank's level readout painted the MidiHexadecimal form at
    /// both densities.
    fn bank_level_readout_is_midi_hex(&self) -> bool {
        self.bank_both_densities().is_some_and(|(desktop, deck)| {
            [desktop, deck].into_iter().all(|bank| {
                bank.level_readout.len() == 2
                    && bank
                        .level_readout
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase())
            })
        })
    }

    /// The states that acked at every declared policy, with their evidence.
    fn states_rendered(&self) -> Vec<PaintedStateRecord> {
        ALL_COMPONENT_STATES
            .into_iter()
            .filter_map(|state| {
                let name = state.canonical_name();
                let (label, evidence) = self.densities[0].states.get(name)?;
                if !self.densities[1].states.contains_key(name) {
                    return None;
                }
                Some(PaintedStateRecord {
                    state: name,
                    visible_label: label.clone(),
                    non_color_evidence: evidence.clone(),
                    viewports: GALLERY_VIEWPORTS
                        .into_iter()
                        .map(RepresentativeViewport::canonical_name)
                        .collect(),
                })
            })
            .collect()
    }

    /// The controls that acked every declared state at every policy.
    fn controls_rendered(&self) -> Vec<PaintedControlRecord> {
        ALL_COMPONENT_CONTROLS
            .into_iter()
            .filter_map(|control| {
                if !self.control_fully_acked(control) {
                    return None;
                }
                let (kind, role) = selecting_pair(control)?;
                let (_, label) = self.densities[0].controls.get(control.canonical_name())?;
                let states = control.applicable_states();
                Some(PaintedControlRecord {
                    control: control.canonical_name(),
                    kind,
                    role,
                    states_painted: states.len(),
                    states_declared: states.len(),
                    visible_label: label.clone(),
                    viewports: GALLERY_VIEWPORTS
                        .into_iter()
                        .map(RepresentativeViewport::canonical_name)
                        .collect(),
                })
            })
            .collect()
    }

    /// The compositions that acked at every policy.
    fn compositions_rendered(&self) -> Vec<PaintedCompositionRecord> {
        ALL_SHELL_COMPOSITIONS
            .into_iter()
            .filter_map(|composition| {
                if !self.composition_fully_acked(composition) {
                    return None;
                }
                let (region, label) = self.densities[0]
                    .compositions
                    .get(composition.canonical_name())?;
                Some(PaintedCompositionRecord {
                    composition: composition.canonical_name(),
                    region: region.clone(),
                    visible_label: label.clone(),
                    viewports: GALLERY_VIEWPORTS
                        .into_iter()
                        .map(RepresentativeViewport::canonical_name)
                        .collect(),
                })
            })
            .collect()
    }
}

/// Whether one CSS computed color resolves through the authored vocabulary.
///
/// Accepts `rgb(r, g, b)` matching an authored role exactly, and
/// `rgba(r, g, b, a)` whose triple matches a role (the halo paints a role at
/// the authored opacity; every opaque role serializes as `rgb()`).
fn css_color_resolves(color: &str) -> bool {
    let Some(rgb) = parse_css_rgb(color) else {
        return false;
    };
    ALL_COLORS.into_iter().any(|role| {
        let resolved = role.resolve();
        (resolved.r(), resolved.g(), resolved.b()) == rgb
    })
}

/// Parses `rgb(r, g, b)` / `rgba(r, g, b, a)` computed-style text.
fn parse_css_rgb(color: &str) -> Option<(u8, u8, u8)> {
    let trimmed = color.trim();
    let inner = trimmed
        .strip_prefix("rgba(")
        .or_else(|| trimmed.strip_prefix("rgb("))?
        .strip_suffix(')')?;
    let mut parts = inner.split(',').map(str::trim);
    let r = parts.next()?.parse::<u8>().ok()?;
    let g = parts.next()?.parse::<u8>().ok()?;
    let b = parts.next()?.parse::<u8>().ok()?;
    Some((r, g, b))
}

// ===========================================================================
// The observation
// ===========================================================================

/// One state identity that reached the screen, with its visible evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PaintedStateRecord {
    state: &'static str,
    visible_label: String,
    non_color_evidence: String,
    viewports: Vec<&'static str>,
}

impl PaintedStateRecord {
    /// The canonical state name.
    pub fn state(&self) -> &str {
        self.state
    }

    /// The label painted beside the specimen.
    pub fn visible_label(&self) -> &str {
        &self.visible_label
    }

    /// What announced the state without color.
    pub fn non_color_evidence(&self) -> &str {
        &self.non_color_evidence
    }

    /// The density policies whose column painted this state.
    pub fn viewports(&self) -> &[&'static str] {
        &self.viewports
    }
}

/// One control identity that reached the screen, with its visible evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PaintedControlRecord {
    control: &'static str,
    kind: &'static str,
    role: &'static str,
    states_painted: usize,
    states_declared: usize,
    visible_label: String,
    viewports: Vec<&'static str>,
}

impl PaintedControlRecord {
    /// The canonical control name.
    pub fn control(&self) -> &str {
        self.control
    }

    /// The semantic control kind whose selection produces this control.
    pub fn kind(&self) -> &str {
        self.kind
    }

    /// The presentation role that selection is asked in.
    pub fn role(&self) -> &str {
        self.role
    }

    /// How many of this control's declared states acked at every policy.
    pub const fn states_painted(&self) -> usize {
        self.states_painted
    }

    /// How many states this control declares applicable.
    pub const fn states_declared(&self) -> usize {
        self.states_declared
    }

    /// A label a reader actually sees on this control's specimen.
    pub fn visible_label(&self) -> &str {
        &self.visible_label
    }

    /// The density policies whose column painted this control.
    pub fn viewports(&self) -> &[&'static str] {
        &self.viewports
    }
}

/// One composition identity that reached the screen, with its visible
/// evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PaintedCompositionRecord {
    composition: &'static str,
    region: String,
    visible_label: String,
    viewports: Vec<&'static str>,
}

impl PaintedCompositionRecord {
    /// The canonical composition name.
    pub fn composition(&self) -> &str {
        self.composition
    }

    /// The region the specimen was painted under.
    pub fn region(&self) -> &str {
        &self.region
    }

    /// A label a reader actually sees inside this composition's specimen.
    pub fn visible_label(&self) -> &str {
        &self.visible_label
    }

    /// The density policies whose column painted it.
    pub fn viewports(&self) -> &[&'static str] {
        &self.viewports
    }
}

/// One immutable observation of the gallery scene as actually painted.
///
/// Constructed only from a [`GalleryAckLedger`] that painted acks filled in.
/// There is no constructor that accepts an expected page set, a specimen
/// list, or a coverage plan, which is what keeps this evidence rather than
/// assertion. The serialized field set is the component-gallery observation
/// schema.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ComponentGalleryObservation {
    pages_declared: usize,
    pages_painted: usize,
    pages_reachable_by_digit: usize,
    pages_reachable_by_step: usize,
    unbound_digit_retained_page: bool,
    states_declared: usize,
    states_painted: usize,
    states_distinguishable_without_color: bool,
    controls_declared: usize,
    controls_painted: usize,
    kind_role_pairs_unmapped: usize,
    controls_unreachable_by_any_pair: usize,
    compositions_declared: usize,
    compositions_painted: usize,
    mixer_column_structures_declared: usize,
    mixer_column_structures_painted: usize,
    mixer_column_names_beyond_track_header: usize,
    mixer_column_level_readout_is_midi_hex: bool,
    desktop_viewport_painted: bool,
    steam_deck_viewport_painted: bool,
    bands_retained_both_viewports: bool,
    clipped_or_overlapping_text: usize,
    token_source_exact: bool,
    typeface_resolved: bool,
    audio_or_midi_constructed: bool,
    app_state_generation_delta: i64,
    window_closed: bool,
    viewport_width: f32,
    viewport_height: f32,
    active_page: &'static str,
    active_page_identity_visible: bool,
    pages_visited: Vec<&'static str>,
    states_rendered: Vec<PaintedStateRecord>,
    controls_rendered: Vec<PaintedControlRecord>,
    compositions_rendered: Vec<PaintedCompositionRecord>,
    documents_pushed: usize,
    acks_received: usize,
    unbound_key_presses: usize,
    text_defects: Vec<String>,
}

impl ComponentGalleryObservation {
    /// Builds the observation from what the page acknowledged as painted.
    fn from_acks(
        ledger: &GalleryAckLedger,
        app_state_generation_delta: i64,
        window_closed: bool,
    ) -> Self {
        Self {
            pages_declared: GALLERY_PAGE_COUNT,
            pages_painted: ledger.acked_page_count(),
            pages_reachable_by_digit: ledger.digit_reached_page_count(),
            pages_reachable_by_step: ledger.step_reached_page_count(),
            unbound_digit_retained_page: ledger.unbound_key_presses > 0
                && ledger.unbound_key_page_changes == 0,
            states_declared: COMPONENT_STATE_COUNT,
            states_painted: ledger.acked_state_count(),
            states_distinguishable_without_color: ledger.states_distinguishable_without_color(),
            controls_declared: COMPONENT_CONTROL_COUNT,
            controls_painted: ledger.acked_control_count(),
            kind_role_pairs_unmapped: ledger.unmapped_kind_role_pairs(),
            controls_unreachable_by_any_pair: ledger.controls_unreachable_by_any_pair(),
            compositions_declared: SHELL_COMPOSITION_COUNT,
            compositions_painted: ledger.acked_composition_count(),
            mixer_column_structures_declared: MIXER_COLUMN_ANATOMY.len(),
            mixer_column_structures_painted: ledger.bank_structures_painted(),
            mixer_column_names_beyond_track_header: ledger.bank_names_beyond_anatomy(),
            mixer_column_level_readout_is_midi_hex: ledger.bank_level_readout_is_midi_hex(),
            desktop_viewport_painted: ledger.densities
                [policy_index(RepresentativeViewport::WideReference)]
            .painted,
            steam_deck_viewport_painted: ledger.densities
                [policy_index(RepresentativeViewport::StandardReference)]
            .painted,
            bands_retained_both_viewports: ledger.bands_retained_both_viewports(),
            clipped_or_overlapping_text: ledger.defects.len(),
            token_source_exact: ledger.token_source_exact(),
            typeface_resolved: ledger.typeface_resolved.unwrap_or(false),
            audio_or_midi_constructed: audio_or_midi_constructed(),
            app_state_generation_delta,
            window_closed,
            viewport_width: ledger.viewport.width_px,
            viewport_height: ledger.viewport.height_px,
            active_page: ledger.active_page.canonical_name(),
            active_page_identity_visible: ledger.title_visible && ledger.acks_received > 0,
            pages_visited: ledger
                .pages_visited
                .iter()
                .map(|page| page.canonical_name())
                .collect(),
            states_rendered: ledger.states_rendered(),
            controls_rendered: ledger.controls_rendered(),
            compositions_rendered: ledger.compositions_rendered(),
            documents_pushed: ledger.documents_pushed,
            acks_received: ledger.acks_received,
            unbound_key_presses: ledger.unbound_key_presses,
            text_defects: ledger.defects.iter().cloned().collect(),
        }
    }

    /// How many pages the vocabulary declares.
    pub const fn pages_declared(&self) -> usize {
        self.pages_declared
    }

    /// How many distinct pages acked a paint.
    pub const fn pages_painted(&self) -> usize {
        self.pages_painted
    }

    /// How many pages a digit press brought on screen.
    pub const fn pages_reachable_by_digit(&self) -> usize {
        self.pages_reachable_by_digit
    }

    /// How many pages a step key brought on screen.
    pub const fn pages_reachable_by_step(&self) -> usize {
        self.pages_reachable_by_step
    }

    /// Whether a key binding no page kept the current one.
    pub const fn unbound_digit_retained_page(&self) -> bool {
        self.unbound_digit_retained_page
    }

    /// How many states the vocabulary declares.
    pub const fn states_declared(&self) -> usize {
        self.states_declared
    }

    /// How many distinct states acked at both densities.
    pub const fn states_painted(&self) -> usize {
        self.states_painted
    }

    /// Whether no two acked states shared their colorless evidence.
    pub const fn states_distinguishable_without_color(&self) -> bool {
        self.states_distinguishable_without_color
    }

    /// How many controls the family declares.
    pub const fn controls_declared(&self) -> usize {
        self.controls_declared
    }

    /// How many controls acked every state they declare, at both densities.
    pub const fn controls_painted(&self) -> usize {
        self.controls_painted
    }

    /// How many `(kind, role)` pairs the gallery could not put on screen.
    pub const fn kind_role_pairs_unmapped(&self) -> usize {
        self.kind_role_pairs_unmapped
    }

    /// How many declared controls no pair resolves to.
    pub const fn controls_unreachable_by_any_pair(&self) -> usize {
        self.controls_unreachable_by_any_pair
    }

    /// How many compositions the family declares.
    pub const fn compositions_declared(&self) -> usize {
        self.compositions_declared
    }

    /// How many compositions acked at both densities.
    pub const fn compositions_painted(&self) -> usize {
        self.compositions_painted
    }

    /// How many structures the declared column anatomy names.
    pub const fn mixer_column_structures_declared(&self) -> usize {
        self.mixer_column_structures_declared
    }

    /// How many declared structures the bank specimen painted at both
    /// densities.
    pub const fn mixer_column_structures_painted(&self) -> usize {
        self.mixer_column_structures_painted
    }

    /// How many painted column names the declared anatomy does not name.
    pub const fn mixer_column_names_beyond_track_header(&self) -> usize {
        self.mixer_column_names_beyond_track_header
    }

    /// Whether the bank's level readout painted the MidiHexadecimal form.
    pub const fn mixer_column_level_readout_is_midi_hex(&self) -> bool {
        self.mixer_column_level_readout_is_midi_hex
    }

    /// Whether the desktop density column painted.
    pub const fn desktop_viewport_painted(&self) -> bool {
        self.desktop_viewport_painted
    }

    /// Whether the Steam Deck density column painted.
    pub const fn steam_deck_viewport_painted(&self) -> bool {
        self.steam_deck_viewport_painted
    }

    /// Whether all five structural regions acked at both densities.
    pub const fn bands_retained_both_viewports(&self) -> bool {
        self.bands_retained_both_viewports
    }

    /// How many named paint defects the page reported.
    pub const fn clipped_or_overlapping_text(&self) -> usize {
        self.clipped_or_overlapping_text
    }

    /// Whether every reported painted color resolved through the vocabulary.
    pub const fn token_source_exact(&self) -> bool {
        self.token_source_exact
    }

    /// Whether every ack reported the authored typeface resolved.
    pub const fn typeface_resolved(&self) -> bool {
        self.typeface_resolved
    }

    /// Whether the scene constructed an audio output or a MIDI event source.
    ///
    /// Derived from what the scene is built out of, never declared. See
    /// [`audio_or_midi_constructed`].
    pub const fn audio_or_midi_constructed(&self) -> bool {
        self.audio_or_midi_constructed
    }

    /// How far the production reducer advanced while the gallery was open.
    pub const fn app_state_generation_delta(&self) -> i64 {
        self.app_state_generation_delta
    }

    /// Whether the window closed and released what it owned.
    pub const fn window_closed(&self) -> bool {
        self.window_closed
    }

    /// The states that reached the screen, with their visible evidence.
    pub fn states_rendered(&self) -> &[PaintedStateRecord] {
        &self.states_rendered
    }

    /// The controls that reached the screen, with their visible evidence.
    pub fn controls_rendered(&self) -> &[PaintedControlRecord] {
        &self.controls_rendered
    }

    /// The compositions that reached the screen, with their visible evidence.
    pub fn compositions_rendered(&self) -> &[PaintedCompositionRecord] {
        &self.compositions_rendered
    }

    /// The pages that reached the screen, in first-ack order.
    pub fn pages_visited(&self) -> &[&'static str] {
        &self.pages_visited
    }

    /// The named defects, so a failure is actionable.
    pub fn text_defects(&self) -> &[String] {
        &self.text_defects
    }
}

// ===========================================================================
// The silence is derived, not declared
// ===========================================================================

/// This module's own production source, embedded at compile time.
///
/// Embedded rather than read from disk so the derivation below works in a
/// shipped binary and needs no filesystem at demo time — the same reason the
/// page assets are embedded rather than looked up.
const GALLERY_SCENE_SOURCE: &str = include_str!("component_gallery_scene.rs");

/// Whether the gallery scene constructs an audio output or a MIDI event source.
///
/// **Derived, not declared.** A hard-coded `false` satisfies the witness
/// predicate and proves nothing: it would keep reporting silence the day
/// somebody wired a stream into this scene. So the answer is computed from what
/// this module is actually built out of — its own production source is searched
/// for any mention of the types that *are* the audio output and the MIDI event
/// source in this product, and the flag is true if it finds one.
///
/// That makes it falsifiable in the only way that matters: adding an audio
/// construction to this scene flips the flag to `true` and the predicate fails,
/// which is exactly the report an operator would want. The test below proves
/// the search is capable of returning `true`, so a scan that had quietly stopped
/// matching anything cannot pass as silence.
///
/// Only the part of the file before the first test module is searched. A test
/// that *names* an audio type — as the one proving this search works must —
/// ships in no binary and constructs nothing at demo time.
fn audio_or_midi_constructed() -> bool {
    source_constructs_audio_or_midi(&production_source(GALLERY_SCENE_SOURCE))
}

/// The part of a source file that ships, with its prose removed.
///
/// Comments are stripped for the same reason the control family strips them
/// before its own boundary scans: prose that *names* the boundary must not be
/// mistaken for code that crosses it. This module's own explanation of why the
/// gallery constructs no MIDI source has to be able to say "MIDI source".
fn production_source(source: &str) -> String {
    let marker = format!("#[cfg({})]", "test");
    let shipping = match source.find(&marker) {
        Some(offset) => &source[..offset],
        None => source,
    };
    shipping
        .lines()
        .map(|line| match line.find("//") {
            Some(offset) => &line[..offset],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether `source` names anything that makes sound or reads MIDI.
///
/// The needles are assembled at run time so that this function's own source
/// does not match them; the project already uses that shape for its
/// `SemanticAction` and visual-literal guards.
///
/// The split points are chosen so that **no fragment spelled here is itself one
/// of the needles**. That is not cosmetic: the first version of this function
/// spelled `"MidiEventSource"` as one half of `FixedMidiEventSource`, and
/// the search duly found that literal in its own source and reported the gallery
/// as constructing a MIDI source. The tests below are what caught it, and they
/// are what will catch the next bad split — a scene that is genuinely silent
/// reports `false`, so a self-match shows up as a failure rather than as
/// caution.
fn source_constructs_audio_or_midi(source: &str) -> bool {
    let needles = [
        // The two production ports the standalone application constructs.
        format!("{}{}", "Cpal", "AudioOutput"),
        format!("{}{}", "FixedMidi", "EventSource"),
        // The port traits themselves, so a scene reaching for either through an
        // abstraction is caught as well as one naming a concrete adapter.
        format!("{}{}", "Audio", "OutputPort"),
        format!("{}{}", "MidiEvent", "Source"),
        // What a prepared stream, a renderer, and a note event are called here.
        format!("{}{}", "Prepared", "Graph"),
        format!("{}{}", "Audio", "Renderer"),
        format!("{}{}", "Midi", "Message"),
    ];
    needles
        .iter()
        .any(|needle| source.contains(needle.as_str()))
}

/// A failure opening or running the gallery.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ComponentGalleryError {
    /// The built documents fall short of the declared page, state, control,
    /// or composition sets. Raised before any window opens: a gallery missing
    /// a declared specimen must fail rather than browse a smaller vocabulary.
    #[error("gallery coverage assertion failed: {0}")]
    Coverage(String),
    /// The production capability registry could not be constructed, so there is
    /// no reducer to measure the generation delta against.
    #[error("gallery reducer witness unavailable: {0}")]
    Capability(CapabilityError),
    /// A gallery document failed to serialize for the push channel.
    #[error("gallery document serialization failed: {0}")]
    Serialization(String),
    /// The window itself failed.
    #[error("component gallery window failed: {0}")]
    Window(String),
}

// ===========================================================================
// The gallery page, served over the scene's own crest:// protocol
// ===========================================================================

/// The single webview window's stable Tauri label.
///
/// `main`, because the declared tauri capability grants the event permissions
/// (`core:event:default`) to the window with this label; a gallery window
/// under another label could neither listen for documents nor emit acks.
const WINDOW_LABEL: &str = "main";

/// The gallery page document. Structure only: one root the render script
/// fills. All paint comes from the generated `tokens.css`, the shared
/// `page.css` primitives, and `gallery.css`; all content comes from
/// `gallery.js` rendering the gallery-scene documents the scene pushes. The
/// page registers no key handler and captures no input — keys are captured
/// Rust-side by the scene.
const GALLERY_INDEX_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>crest-synth — component gallery</title>
  <link rel="stylesheet" href="tokens.css">
  <link rel="stylesheet" href="page.css">
  <link rel="stylesheet" href="gallery.css">
</head>
<body data-scene="component-gallery">
  <div id="gallery"></div>
  <script src="gallery.js"></script>
</body>
</html>
"#;

/// The page assets, embedded at compile time so demo time needs no
/// filesystem. `tokens.css` is the same committed generated table the product
/// page serves — one styling source, no gallery copy.
const GALLERY_TOKENS_CSS: &str = include_str!("../../webview-page/tokens.css");
const GALLERY_PAGE_CSS: &str = include_str!("../../webview-page/page.css");
const GALLERY_CSS: &str = include_str!("../../webview-page/gallery.css");
const GALLERY_JS: &str = include_str!("../../webview-page/gallery.js");
const FONT_AZERET_REGULAR: &[u8] =
    include_bytes!("../../vendor/azeret-mono/AzeretMono-Regular.ttf");
const FONT_AZERET_MEDIUM: &[u8] = include_bytes!("../../vendor/azeret-mono/AzeretMono-Medium.ttf");
const FONT_AZERET_SEMIBOLD: &[u8] =
    include_bytes!("../../vendor/azeret-mono/AzeretMono-SemiBold.ttf");
const FONT_AZERET_BOLD: &[u8] = include_bytes!("../../vendor/azeret-mono/AzeretMono-Bold.ttf");

/// Resolves one `crest://` request path to its embedded gallery asset as a
/// `(content type, body)` pair, or `None` for a path the page never
/// references (served as 404).
fn gallery_asset(path: &str) -> Option<(&'static str, Vec<u8>)> {
    match path {
        "/" | "/index.html" => Some((
            "text/html; charset=utf-8",
            GALLERY_INDEX_HTML.as_bytes().to_vec(),
        )),
        "/tokens.css" => Some((
            "text/css; charset=utf-8",
            GALLERY_TOKENS_CSS.as_bytes().to_vec(),
        )),
        "/page.css" => Some((
            "text/css; charset=utf-8",
            GALLERY_PAGE_CSS.as_bytes().to_vec(),
        )),
        "/gallery.css" => Some(("text/css; charset=utf-8", GALLERY_CSS.as_bytes().to_vec())),
        "/gallery.js" => Some((
            "text/javascript; charset=utf-8",
            GALLERY_JS.as_bytes().to_vec(),
        )),
        "/fonts/AzeretMono-Regular.ttf" => Some(("font/ttf", FONT_AZERET_REGULAR.to_vec())),
        "/fonts/AzeretMono-Medium.ttf" => Some(("font/ttf", FONT_AZERET_MEDIUM.to_vec())),
        "/fonts/AzeretMono-SemiBold.ttf" => Some(("font/ttf", FONT_AZERET_SEMIBOLD.to_vec())),
        "/fonts/AzeretMono-Bold.ttf" => Some(("font/ttf", FONT_AZERET_BOLD.to_vec())),
        _ => None,
    }
}

/// The idle tick cadence — the same 16 ms idle-frame convention the product
/// webview window declares.
const TICK_INTERVAL: Duration = Duration::from_millis(16);

// ===========================================================================
// The scene
// ===========================================================================

/// The browsable component gallery scene.
///
/// Owns the window, the page selection, and one production `AppState` held only
/// so the generation claim is measured rather than asserted.
pub struct ComponentGalleryScene {
    app_state: AppState,
}

impl ComponentGalleryScene {
    /// Builds the scene and the production reducer its observation measures
    /// against.
    ///
    /// The reducer is built from a production capability provider that needs no
    /// asset file: the gallery paints, it does not sound, and a visual scene
    /// that fails to start because a SoundFont is missing would be coupling the
    /// two for no gain.
    pub fn new() -> Result<Self, ComponentGalleryError> {
        let braids = crate::adapter::braids_capability::BraidsCapability::new()
            .map_err(ComponentGalleryError::Capability)?;
        let registry = CapabilityRegistry::new(vec![braids.descriptor()])
            .map_err(ComponentGalleryError::Capability)?;
        Ok(Self {
            app_state: AppState::new(registry, gallery_global_parameters()),
        })
    }

    /// Opens the webview window and returns what was actually painted.
    ///
    /// The coverage assertion runs before any window exists, so a document set
    /// missing a declared specimen is a typed startup failure naming what is
    /// missing rather than a window that opens one page short.
    ///
    /// There is no milestone timeout and no total timeout. Those exist to stop
    /// an autonomous witness hanging; this scene waits for the operator by
    /// design and finishes when the window closes.
    pub fn run(self) -> Result<ComponentGalleryObservation, ComponentGalleryError> {
        let documents = all_gallery_documents();
        let failures = gallery_coverage_failures(&documents);
        if !failures.is_empty() {
            return Err(ComponentGalleryError::Coverage(failures.join("; ")));
        }
        let generation_before = self.app_state.generation();

        // The scene's mutable state. The selection and the ledger live on the
        // main thread (the key sink and the event loop both run there); the
        // ack inbox and the ready flag are written by tauri's event listeners
        // and drained on the main loop.
        let selection = Rc::new(RefCell::new(GalleryPageSelection::default()));
        let ledger = Rc::new(RefCell::new(GalleryAckLedger::default()));
        let ack_inbox: Arc<Mutex<Vec<GalleryPaintedAck>>> = Arc::new(Mutex::new(Vec::new()));
        let page_ready = Arc::new(AtomicBool::new(false));

        // FR-006 / OBS-1 — this handler attaches `Content-Type` and NO
        // content security policy, and that is a recorded decision, not an
        // oversight. Do not "fix" it by adding a CSP here.
        //
        // Why it is safe: the shipped window cannot load gallery sources at
        // all. `page_asset` — the production asset table in
        // `src/shell/webview/window.rs` — enumerates the index document,
        // `tokens.css`, `page.css`, `page.js`, and the four Azeret faces, and
        // has NO gallery entry; every other path is a 404 with no fallback.
        // That table is the reachability precondition, and it is checkable:
        // read it and confirm no gallery asset appears.
        //
        // What this must NOT be confused with: DRIFT-1 of the
        // webview-shell-cutover mission review — the acceptance harness
        // re-implementing the `crest://` response privately without the
        // production CSP, which made the harness measure under a laxer policy
        // than production and hid RISK-1, a shipped paint-fidelity defect
        // (the CSP blocked the page's inline `style` attributes, so every
        // fader fill rendered empty in the real app while the harness saw it
        // paint). The two shapes look identical — a policy-free `crest://`
        // handler in a testing surface. The difference is reachability: that
        // harness served the PRODUCTION page, so its policy had to match
        // production's; this scene serves only gallery assets that production
        // can never request. That is why the production seam is now a single
        // exported source (`protocol_response`/`PAGE_CSP`) with two callers.
        //
        // What would have to change if that stopped being true: the moment
        // `page_asset` gains a gallery entry — or this scene serves any
        // production asset — this handler must serve through
        // `crate::shell::webview::protocol_response` so the gallery is
        // measured under `PAGE_CSP` exactly as the acceptance harness is.
        //
        // Why full CSP parity was not done now (research.md D5, operator
        // decision 2026-08-06): it was drafted and reversed. It would require
        // converting the gallery's JS-built inline-style painting and
        // re-homing 48 gallery-borne proof references in
        // `component_vocabulary`, for no product gain. The risk was never the
        // missing policy — it was that a reader could not tell whether the
        // omission was deliberate. This comment is the fix.
        let app = tauri::Builder::default()
            .register_uri_scheme_protocol("crest", move |_context, request| {
                match gallery_asset(request.uri().path()) {
                    Some((content_type, body)) => tauri::http::Response::builder()
                        .header("Content-Type", content_type)
                        .body(body)
                        .expect("the embedded asset response is well-formed"),
                    None => tauri::http::Response::builder()
                        .status(404)
                        .body(Vec::new())
                        .expect("the empty not-found response is well-formed"),
                }
            })
            .build(crate::shell::webview::tauri_context())
            .map_err(|error| {
                ComponentGalleryError::Window(format!("webview runtime unavailable: {error}"))
            })?;

        // The page's two upward events: readiness (script loaded, faces
        // resolved) and the painted ack per document.
        let ready_flag = Arc::clone(&page_ready);
        app.listen_any(GALLERY_READY_EVENT, move |_event| {
            ready_flag.store(true, Ordering::SeqCst);
        });
        let inbox = Arc::clone(&ack_inbox);
        app.listen_any(GALLERY_PAINTED_EVENT, move |event| {
            let ack = serde_json::from_str::<GalleryPaintedAck>(event.payload()).unwrap_or_else(
                |error| GalleryPaintedAck {
                    page: format!("unparseable ack: {error}"),
                    ..GalleryPaintedAck::default()
                },
            );
            if let Ok(mut inbox) = inbox.lock() {
                inbox.push(ack);
            }
        });

        let url: tauri::Url = "crest://localhost/index.html"
            .parse()
            .expect("the static page url is well-formed");
        let authored = RepresentativeViewport::WideReference.fixture();
        let smallest = RepresentativeViewport::Compact.fixture();
        let window = WebviewWindowBuilder::new(&app, WINDOW_LABEL, WebviewUrl::CustomProtocol(url))
            .title(COMPONENT_GALLERY_WINDOW_TITLE)
            .inner_size(f64::from(authored.width_px), f64::from(authored.height_px))
            .min_inner_size(f64::from(smallest.width_px), f64::from(smallest.height_px))
            .focused(true)
            .build()
            .map_err(|error| {
                ComponentGalleryError::Window(format!("window creation failed: {error}"))
            })?;
        let _ = window.set_focus();

        // Keys are captured Rust-side, exactly as the product webview shell
        // captures them — but the sink is the scene-local page selection, and
        // nothing here reaches `KeyboardInputTranslator`, so no key press can
        // become a `SemanticAction`.
        let capture_selection = Rc::clone(&selection);
        let capture_ledger = Rc::clone(&ledger);
        let _capture_handle = input_capture::install_for_window(&window, move |raw| {
            let input = if raw.pressed() {
                WindowInput::key_down(raw.key())
            } else {
                WindowInput::key_up(raw.key())
            };
            let before = capture_selection.borrow().active();
            let outcome = capture_selection.borrow_mut().apply(input);
            let mut ledger = capture_ledger.borrow_mut();
            match outcome {
                PageSelection::Changed(page) => ledger.record_digit_request(page),
                PageSelection::Stepped(page) => ledger.record_step_request(page),
                PageSelection::Retained(page) => {
                    // A step that ran into an end is a bound key declining to
                    // move, which is a different fact from a key that binds
                    // nothing; only the latter is what the retention field
                    // reports.
                    let bound_nothing = PageStep::for_key(input.key()).is_none();
                    if bound_nothing && input.kind() == WindowInputKind::KeyDown {
                        ledger.record_unbound_key(page != before);
                    }
                }
            }
            raw.key() != crate::shell::WindowKey::Other
        })
        .map_err(|error| ComponentGalleryError::Window(format!("input capture failed: {error}")))?;

        // The tao loop waits when idle; a detached waker keeps the main loop
        // draining acks and pushing page changes at the idle-frame cadence.
        let stop_waker = Arc::new(AtomicBool::new(false));
        let waker_stop = Arc::clone(&stop_waker);
        let waker_handle = app.handle().clone();
        let waker = std::thread::spawn(move || {
            while !waker_stop.load(Ordering::Relaxed) {
                std::thread::sleep(TICK_INTERVAL);
                if waker_handle.run_on_main_thread(|| {}).is_err() {
                    break;
                }
            }
        });

        let loop_selection = Rc::clone(&selection);
        let loop_ledger = Rc::clone(&ledger);
        let loop_inbox = Arc::clone(&ack_inbox);
        let loop_ready = Arc::clone(&page_ready);
        let mut last_pushed: Option<ComponentGalleryPage> = None;
        let mut close_requested = false;
        let push_error: Rc<RefCell<Option<ComponentGalleryError>>> = Rc::new(RefCell::new(None));
        let loop_push_error = Rc::clone(&push_error);

        let exit_code = app.run_return(move |handle, event| match event {
            RunEvent::WindowEvent {
                event: WindowEvent::Focused(false),
                ..
            } => {
                // Parity with the retired native scene: a focus loss is a normalized
                // input the selection retains through.
                let _ = loop_selection.borrow_mut().apply(WindowInput::focus_lost());
            }
            RunEvent::WindowEvent {
                event: WindowEvent::Destroyed,
                ..
            } => {
                // Teardown: the webview is gone. Stop pushing so a late emit
                // cannot turn a clean shutdown into a fatal error.
                close_requested = true;
            }
            RunEvent::MainEventsCleared => {
                {
                    let mut ledger = loop_ledger.borrow_mut();
                    if let Ok(mut inbox) = loop_inbox.lock() {
                        for ack in inbox.drain(..) {
                            ledger.record_ack(&ack);
                        }
                    }
                }
                if close_requested || !loop_ready.load(Ordering::SeqCst) {
                    return;
                }
                // The gallery's own push channel, identity-gated the way the
                // production projection channel is generation-gated: a tick
                // whose active page already painted pushes nothing.
                let active = loop_selection.borrow().active();
                if last_pushed == Some(active) {
                    return;
                }
                let pushed = serde_json::to_value(gallery_document(active))
                    .map_err(|error| ComponentGalleryError::Serialization(error.to_string()))
                    .and_then(|document| {
                        handle
                            .emit(GALLERY_DOCUMENT_EVENT, document)
                            .map_err(|error| {
                                ComponentGalleryError::Window(format!(
                                    "gallery document emit failed: {error}"
                                ))
                            })
                    });
                match pushed {
                    Ok(()) => {
                        last_pushed = Some(active);
                        loop_ledger.borrow_mut().record_document_pushed();
                    }
                    Err(error) => {
                        // Record the first failure, close once, surface it
                        // after the loop — the same treatment the product
                        // window gives a transport failure.
                        loop_push_error.borrow_mut().get_or_insert(error);
                        close_requested = true;
                        if let Some(window) = handle.get_webview_window(WINDOW_LABEL) {
                            let _ = window.close();
                        }
                    }
                }
            }
            _ => {}
        });

        stop_waker.store(true, Ordering::Relaxed);
        let _ = waker.join();

        // The window has closed. Drain any ack that arrived during teardown,
        // then read the reducer again so the delta covers the whole session.
        {
            let mut ledger = ledger.borrow_mut();
            if let Ok(mut inbox) = ack_inbox.lock() {
                for ack in inbox.drain(..) {
                    ledger.record_ack(&ack);
                }
            }
        }
        if let Some(error) = push_error.borrow_mut().take() {
            return Err(error);
        }
        if exit_code != 0 {
            return Err(ComponentGalleryError::Window(format!(
                "tauri event loop exited with code {exit_code}"
            )));
        }

        let generation_after = self.app_state.generation();
        let delta = i64::try_from(generation_after).unwrap_or(i64::MAX)
            - i64::try_from(generation_before).unwrap_or(i64::MAX);
        let painted = ledger.borrow();
        Ok(ComponentGalleryObservation::from_acks(
            &painted, delta, true,
        ))
    }
}

/// The globals the gallery's reducer witness starts from.
fn gallery_global_parameters() -> GlobalParameters {
    ApplicationConfig::default().global_parameters()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::app_event::AppEvent;
    use crate::control::TopLevelContext;

    // ----- the page vocabulary and its bindings ---------------------------

    #[test]
    fn the_gallery_declares_exactly_fifteen_pages() {
        assert_eq!(GALLERY_PAGE_COUNT, 15);
        assert_eq!(ALL_GALLERY_PAGES.len(), GALLERY_PAGE_COUNT);
        let names: BTreeSet<&str> = ALL_GALLERY_PAGES
            .iter()
            .map(|page| page.canonical_name())
            .collect();
        assert_eq!(names.len(), GALLERY_PAGE_COUNT, "two pages share a name");
        let pages: BTreeSet<ComponentGalleryPage> = ALL_GALLERY_PAGES.into_iter().collect();
        assert_eq!(pages.len(), GALLERY_PAGE_COUNT, "a page appears twice");
    }

    /// FR-012 as a regression gate.
    ///
    /// The eight bindings that existed before the control and composition
    /// pages were added are compared against the frozen baseline exactly and
    /// in order. Reordering [`ALL_GALLERY_PAGES`], renaming one of the eight,
    /// or handing one a different digit fails here.
    #[test]
    fn the_eight_pre_existing_digit_bindings_are_exactly_the_frozen_baseline() {
        let current: Vec<(&str, WindowKey)> = ALL_GALLERY_PAGES
            .into_iter()
            .take(FROZEN_DIGIT_BINDING_BASELINE.len())
            .map(|page| {
                (
                    page.canonical_name(),
                    page.digit().expect("one of the first eight pages"),
                )
            })
            .collect();
        // Exact ordered equality, not a containment check: a containment check
        // passes when two of the eight swap digits, and swapping two digits is
        // exactly the change FR-012 exists to catch.
        assert_eq!(
            current,
            FROZEN_DIGIT_BINDING_BASELINE.to_vec(),
            "a pre-existing gallery binding moved; FR-012 forbids it"
        );
    }

    /// Every page is reachable, the ten digit bindings are unique, and no key
    /// outside them selects a page.
    #[test]
    fn ten_digits_bind_ten_pages_and_stepping_reaches_the_rest() {
        let mut digits = std::collections::HashSet::new();
        let mut bound = 0;
        for page in ALL_GALLERY_PAGES {
            match page.digit() {
                Some(digit) => {
                    bound += 1;
                    assert!(
                        digits.insert(digit),
                        "{} shares its digit with another page",
                        page.canonical_name()
                    );
                    assert_eq!(
                        ComponentGalleryPage::for_digit(digit),
                        Some(page),
                        "{} is not reachable by its own digit",
                        page.canonical_name()
                    );
                    assert!(page.digit_label().is_some());
                }
                None => assert!(
                    page.digit_label().is_none(),
                    "{} reads as a digit it does not bind",
                    page.canonical_name()
                ),
            }
        }
        assert_eq!(bound, GALLERY_DIGIT_BINDING_COUNT);
        assert_eq!(digits.len(), GALLERY_DIGIT_BINDING_COUNT, "a digit repeats");

        // The ten bindings are the first ten pages in declared order.
        for (index, page) in ALL_GALLERY_PAGES.into_iter().enumerate() {
            assert_eq!(page.index(), index);
            assert_eq!(page.digit().is_some(), index < GALLERY_DIGIT_BINDING_COUNT);
        }

        for key in [
            WindowKey::Q,
            WindowKey::E,
            WindowKey::W,
            WindowKey::S,
            WindowKey::A,
            WindowKey::D,
            WindowKey::K,
            WindowKey::BracketLeft,
            WindowKey::BracketRight,
            WindowKey::Other,
        ] {
            assert_eq!(
                ComponentGalleryPage::for_digit(key),
                None,
                "{key:?} binds a page it should not"
            );
        }
    }

    /// The ninth and tenth pages in declared order carry the two new digits.
    #[test]
    fn the_two_added_digits_bind_the_ninth_and_tenth_pages() {
        assert_eq!(
            ComponentGalleryPage::for_digit(WindowKey::Digit9),
            Some(ComponentGalleryPage::ParameterAndChoiceRows)
        );
        assert_eq!(
            ComponentGalleryPage::for_digit(WindowKey::Digit0),
            Some(ComponentGalleryPage::TogglesAndSliders)
        );
        assert_eq!(
            ALL_GALLERY_PAGES[8],
            ComponentGalleryPage::ParameterAndChoiceRows
        );
        assert_eq!(
            ALL_GALLERY_PAGES[9],
            ComponentGalleryPage::TogglesAndSliders
        );
    }

    #[test]
    fn a_bound_digit_changes_the_page_and_anything_else_retains_it() {
        let mut selection = GalleryPageSelection::default();
        assert_eq!(selection.active(), ComponentGalleryPage::Colors);

        for page in ALL_GALLERY_PAGES {
            let Some(digit) = page.digit() else {
                continue;
            };
            assert_eq!(
                selection.apply(WindowInput::key_down(digit)),
                PageSelection::Changed(page)
            );
            assert_eq!(selection.active(), page);
        }

        let settled = selection.active();
        for input in [
            WindowInput::key_down(WindowKey::Other),
            WindowInput::key_down(WindowKey::Q),
            WindowInput::key_up(WindowKey::Digit1),
            WindowInput::focus_lost(),
        ] {
            assert_eq!(selection.apply(input), PageSelection::Retained(settled));
            assert_eq!(selection.active(), settled);
        }
    }

    #[test]
    fn stepping_from_either_end_visits_every_declared_page() {
        let mut selection = GalleryPageSelection::default();
        let mut visited = vec![selection.active()];
        loop {
            match selection.apply(WindowInput::key_down(WindowKey::BracketRight)) {
                PageSelection::Stepped(page) => visited.push(page),
                PageSelection::Retained(_) => break,
                PageSelection::Changed(_) => panic!("a bracket is not a digit"),
            }
        }
        assert_eq!(visited, ALL_GALLERY_PAGES.to_vec());

        let mut backwards = vec![selection.active()];
        loop {
            match selection.apply(WindowInput::key_down(WindowKey::BracketLeft)) {
                PageSelection::Stepped(page) => backwards.push(page),
                PageSelection::Retained(_) => break,
                PageSelection::Changed(_) => panic!("a bracket is not a digit"),
            }
        }
        let mut declared = ALL_GALLERY_PAGES.to_vec();
        declared.reverse();
        assert_eq!(backwards, declared);
    }

    #[test]
    fn a_step_past_either_end_retains_the_end_page() {
        let mut selection = GalleryPageSelection::default();
        assert_eq!(
            selection.apply(WindowInput::key_down(WindowKey::BracketLeft)),
            PageSelection::Retained(ComponentGalleryPage::Colors)
        );
        let last = ALL_GALLERY_PAGES[GALLERY_PAGE_COUNT - 1];
        while selection.active() != last {
            selection.apply(WindowInput::key_down(WindowKey::BracketRight));
        }
        assert_eq!(
            selection.apply(WindowInput::key_down(WindowKey::BracketRight)),
            PageSelection::Retained(last)
        );
    }

    #[test]
    fn a_step_key_up_and_a_focus_loss_move_nothing() {
        let mut selection = GalleryPageSelection::default();
        selection.apply(WindowInput::key_down(WindowKey::Digit4));
        let settled = selection.active();
        for input in [
            WindowInput::key_up(WindowKey::BracketRight),
            WindowInput::key_up(WindowKey::BracketLeft),
            WindowInput::focus_lost(),
        ] {
            assert_eq!(selection.apply(input), PageSelection::Retained(settled));
        }
    }

    /// The scene never consults the shared key translator: page selection is
    /// scene-local by construction, so the shipping source of this module
    /// must not name the translator at all.
    #[test]
    fn the_scene_never_names_the_shared_key_translator() {
        let production = production_source(GALLERY_SCENE_SOURCE);
        let translator = format!("{}{}", "KeyboardInput", "Translator");
        assert!(
            !production.contains(&translator),
            "gallery paging must never reach the shared translator"
        );
    }

    /// The hard invariant, measured: walking every page moves the production
    /// reducer not at all.
    #[test]
    fn a_full_page_walk_never_advances_the_production_reducer() {
        let scene = ComponentGalleryScene::new().expect("the braids registry is available");
        let before = scene.app_state.generation();
        let mut selection = GalleryPageSelection::default();
        for page in ALL_GALLERY_PAGES {
            if let Some(digit) = page.digit() {
                selection.apply(WindowInput::key_down(digit));
                selection.apply(WindowInput::key_up(digit));
            }
        }
        for _ in 0..GALLERY_PAGE_COUNT {
            selection.apply(WindowInput::key_down(WindowKey::BracketRight));
        }
        assert_eq!(scene.app_state.generation(), before);
    }

    /// The delta is a measurement, not a constant: the reducer it reads does
    /// advance when an event actually reaches it.
    #[test]
    fn the_reducer_the_delta_is_measured_against_does_advance_when_an_event_reaches_it() {
        let mut scene = ComponentGalleryScene::new().expect("the braids registry is available");
        let before = scene.app_state.generation();
        scene
            .app_state
            .apply(AppEvent::SelectContext(TopLevelContext::Mixer))
            .expect("selecting MIXER on a fresh state is valid");
        assert!(scene.app_state.generation() > before);
    }

    // ----- the gallery documents ------------------------------------------

    #[test]
    fn every_page_document_carries_both_densities_with_specimens() {
        for document in all_gallery_documents() {
            assert_eq!(document.schema, GALLERY_DOCUMENT_SCHEMA);
            assert_eq!(document.pages_declared, GALLERY_PAGE_COUNT);
            assert_eq!(document.index.len(), GALLERY_PAGE_COUNT);
            assert_eq!(
                document
                    .index
                    .iter()
                    .filter(|entry| entry.active)
                    .map(|entry| entry.page)
                    .collect::<Vec<_>>(),
                vec![document.page],
                "exactly the active page is marked in the index"
            );
            let policies: Vec<&str> = document
                .densities
                .iter()
                .map(|column| column.policy)
                .collect();
            assert_eq!(policies, vec!["WideReference", "StandardReference"]);
            for column in &document.densities {
                let specimens: usize = column
                    .sections
                    .iter()
                    .map(|section| section.specimens.len())
                    .sum();
                assert!(
                    specimens > 0,
                    "{} renders no specimen at {}",
                    document.page,
                    column.policy
                );
            }
        }
    }

    #[test]
    fn the_full_document_set_passes_the_coverage_assertion() {
        assert_eq!(
            gallery_coverage_failures(&all_gallery_documents()),
            Vec::<String>::new()
        );
    }

    /// The falsifiability check T016 demands: delete a declared state's
    /// specimens and the coverage assertion fails naming the state; restore
    /// and it passes (`the_full_document_set_passes_the_coverage_assertion`).
    ///
    /// The state set is asserted as a union across the declared pages, so
    /// the deletion strips every page — a state present anywhere is a state
    /// an operator can still see, which is the correct reading of coverage.
    #[test]
    fn deleting_one_state_specimen_fails_the_coverage_assertion_naming_it() {
        let mut documents = all_gallery_documents();
        for document in &mut documents {
            for column in &mut document.densities {
                for section in &mut column.sections {
                    section.specimens.retain(|specimen| {
                        !matches!(
                            specimen,
                            GallerySpecimen::State { state, .. }
                            if *state == ComponentState::Muted.canonical_name()
                        )
                    });
                }
            }
        }
        let failures = gallery_coverage_failures(&documents);
        assert!(
            failures.iter().any(|failure| failure.contains("Muted")),
            "the failure must name the deleted specimen: {failures:?}"
        );
    }

    #[test]
    fn deleting_one_control_state_fails_the_coverage_assertion_naming_it() {
        let mut documents = all_gallery_documents();
        let faders = documents
            .iter_mut()
            .find(|document| document.page == "FadersAndMeters")
            .expect("the fader page exists");
        for column in &mut faders.densities {
            for section in &mut column.sections {
                section.specimens.retain(|specimen| {
                    !matches!(
                        specimen,
                        GallerySpecimen::State { control: Some(control), state, .. }
                        if *control == ComponentControl::Fader.canonical_name()
                            && *state == ComponentState::Soloed.canonical_name()
                    )
                });
            }
        }
        let failures = gallery_coverage_failures(&documents);
        assert!(
            failures
                .iter()
                .any(|failure| failure.contains("Fader") && failure.contains("Soloed")),
            "the failure must name the control and its missing state: {failures:?}"
        );
    }

    #[test]
    fn a_missing_page_fails_the_coverage_assertion() {
        let mut documents = all_gallery_documents();
        documents.pop();
        let failures = gallery_coverage_failures(&documents);
        assert!(!failures.is_empty());
    }

    /// No two declared states share their colorless treatment in the built
    /// documents — the ack evidence the page composes from these fields can
    /// therefore always tell two states apart.
    #[test]
    fn state_specimens_are_pairwise_distinguishable_without_color() {
        let document = gallery_document(ComponentGalleryPage::InteractionStates);
        let column = &document.densities[0];
        let mut signatures = Vec::new();
        for section in &column.sections {
            for specimen in &section.specimens {
                if let GallerySpecimen::State {
                    state,
                    keyline_px,
                    halo,
                    fills_row,
                    cursor,
                    mark,
                    ..
                } = specimen
                {
                    signatures.push((
                        *state,
                        (
                            keyline_px.to_bits(),
                            *halo,
                            *fills_row,
                            *cursor,
                            mark.clone(),
                        ),
                    ));
                }
            }
        }
        assert_eq!(signatures.len(), COMPONENT_STATE_COUNT);
        for (index, (first_name, first)) in signatures.iter().enumerate() {
            for (second_name, second) in signatures.iter().skip(index + 1) {
                assert_ne!(
                    first, second,
                    "{first_name} and {second_name} differ only in color"
                );
            }
        }
    }

    #[test]
    fn the_bank_specimen_carries_the_declared_anatomy_and_the_midi_hex_form() {
        assert_eq!(
            MIXER_COLUMN_ANATOMY,
            [
                "TrackHeader",
                "LevelFader",
                "LevelReadout",
                "PanReadout",
                "StateLine"
            ]
        );
        let document = gallery_document(ComponentGalleryPage::StripPanelAndFooter);
        for column in &document.densities {
            let bank = column
                .sections
                .iter()
                .flat_map(|section| section.specimens.iter())
                .find_map(|specimen| match specimen {
                    GallerySpecimen::Composition {
                        composition,
                        columns,
                        ..
                    } if *composition == "MixerStripBank" => Some(columns),
                    _ => None,
                })
                .expect("the bank specimen exists");
            assert!(!bank.is_empty());
            for track in bank {
                assert_eq!(track.level_hex, midi_hex(track.level));
                assert_eq!(track.level_hex.len(), 2);
            }
            assert!(bank.iter().any(|track| track.focused), "focus is visible");
        }
    }

    #[test]
    fn the_document_serializes_with_camel_case_dispatch_fields() {
        let document = gallery_document(ComponentGalleryPage::Colors);
        let serialized = serde_json::to_value(&document).expect("the gallery document serializes");
        assert_eq!(serialized["schema"], GALLERY_DOCUMENT_SCHEMA);
        assert_eq!(serialized["page"], "Colors");
        assert_eq!(serialized["digitLabel"], "1");
        assert_eq!(serialized["pageNumber"], 1);
        let first = &serialized["densities"][0]["sections"][0]["specimens"][0];
        assert_eq!(first["kind"], "color");
        assert_eq!(first["cssVar"], "--color-bg-canvas");
        // Derived, not spelled: the vocabulary owns the palette, and the
        // repo-wide literal guard forbids restating a hex anywhere else.
        assert_eq!(first["hex"], color_hex(SemanticColor::BgCanvas));
    }

    #[test]
    fn css_var_names_follow_the_generated_token_table() {
        // Spot-checked against the committed generated table so a mapping
        // drift is caught here rather than as an unresolved var at paint time.
        for var in [
            color_css_var(SemanticColor::BgCanvas),
            color_css_var(SemanticColor::AccentFocus),
            spacing_css_var(SpacingStep::S4),
            radius_css_var(Radius::Small).to_owned(),
            "--keyline-resting".to_owned(),
            "--min-interactive-target".to_owned(),
            "--shell-mixer-column-min".to_owned(),
            "--shell-mixer-column-preferred".to_owned(),
            "--shell-mixer-column-max".to_owned(),
        ] {
            assert!(
                GALLERY_TOKENS_CSS.contains(&format!("{var}:")),
                "{var} is not in the generated token table"
            );
        }
    }

    // ----- the ack ledger and the observation -----------------------------

    /// Builds the painted ack the page would emit for one document — the same
    /// readback `gallery.js` performs against its painted DOM, mirrored here
    /// so the ledger's correlation logic is provable without a webview.
    fn ack_for_document(document: &GalleryDocument) -> GalleryPaintedAck {
        let densities = document
            .densities
            .iter()
            .map(|column| {
                let mut states = Vec::new();
                let mut controls: BTreeMap<String, AckControl> = BTreeMap::new();
                let mut compositions = Vec::new();
                let mut bands = Vec::new();
                let mut bank = None;
                for section in &column.sections {
                    for specimen in &section.specimens {
                        match specimen {
                            GallerySpecimen::State {
                                control,
                                state,
                                label,
                                keyline_px,
                                halo,
                                fills_row,
                                cursor,
                                mark,
                                ..
                            } => {
                                let mut evidence = format!("keyline {keyline_px} px");
                                if *halo {
                                    evidence.push_str(" · halo");
                                }
                                if *fills_row {
                                    evidence.push_str(" · row fill");
                                }
                                if *cursor {
                                    evidence.push_str(" · cursor >");
                                }
                                if let Some(mark) = mark {
                                    evidence.push_str(&format!(" · mark {mark}"));
                                }
                                states.push(AckState {
                                    state: (*state).to_owned(),
                                    label: label.clone(),
                                    evidence,
                                });
                                if let Some(control) = control {
                                    let entry = controls
                                        .entry((*control).to_owned())
                                        .or_insert_with(|| AckControl {
                                            control: (*control).to_owned(),
                                            states: Vec::new(),
                                            label: label.clone(),
                                        });
                                    entry.states.push((*state).to_owned());
                                }
                            }
                            GallerySpecimen::Composition {
                                composition,
                                region,
                                columns,
                                entries,
                                ..
                            } => {
                                let label = columns
                                    .first()
                                    .map(|column| column.track.clone())
                                    .or_else(|| entries.first().map(|entry| entry.label.clone()))
                                    .unwrap_or_else(|| (*composition).to_owned());
                                compositions.push(AckComposition {
                                    composition: (*composition).to_owned(),
                                    region: (*region).to_owned(),
                                    label,
                                });
                                if *composition == "MixerStripBank" {
                                    bank = Some(AckBank {
                                        structures: MIXER_COLUMN_ANATOMY
                                            .iter()
                                            .map(|name| (*name).to_owned())
                                            .collect(),
                                        level_readout: columns
                                            .first()
                                            .map(|column| column.level_hex.clone())
                                            .unwrap_or_default(),
                                    });
                                }
                            }
                            GallerySpecimen::Band { region, .. } => {
                                bands.push((*region).to_owned());
                            }
                            GallerySpecimen::Color { .. }
                            | GallerySpecimen::Type { .. }
                            | GallerySpecimen::Metric { .. }
                            | GallerySpecimen::Value { .. }
                            | GallerySpecimen::Hint { .. } => {}
                        }
                    }
                }
                AckDensity {
                    policy: column.policy.to_owned(),
                    painted: true,
                    states,
                    controls: controls.into_values().collect(),
                    compositions,
                    bands,
                    bank,
                }
            })
            .collect();
        GalleryPaintedAck {
            page: document.page.to_owned(),
            title_visible: true,
            viewport: AckViewport {
                width_px: 1512.0,
                height_px: 916.0,
            },
            typeface_resolved: true,
            painted_colors: vec![
                "rgb(12, 16, 21)".to_owned(),
                "rgb(242, 246, 248)".to_owned(),
                "rgba(101, 229, 255, 0.28)".to_owned(),
            ],
            defects: Vec::new(),
            densities,
        }
    }

    /// Feeds one input through the same request-recording logic the capture
    /// sink runs.
    fn apply_input(
        selection: &mut GalleryPageSelection,
        ledger: &mut GalleryAckLedger,
        input: WindowInput,
    ) {
        let before = selection.active();
        match selection.apply(input) {
            PageSelection::Changed(page) => ledger.record_digit_request(page),
            PageSelection::Stepped(page) => ledger.record_step_request(page),
            PageSelection::Retained(page) => {
                let bound_nothing = PageStep::for_key(input.key()).is_none();
                if bound_nothing && input.kind() == WindowInputKind::KeyDown {
                    ledger.record_unbound_key(page != before);
                }
            }
        }
    }

    #[test]
    fn a_digit_request_alone_does_not_make_a_page_reachable() {
        let mut ledger = GalleryAckLedger::default();
        ledger.record_digit_request(ComponentGalleryPage::Type);
        let observation = ComponentGalleryObservation::from_acks(&ledger, 0, false);
        assert_eq!(observation.pages_reachable_by_digit(), 0);
        assert_eq!(observation.pages_painted(), 0);
    }

    #[test]
    fn an_ack_makes_a_requested_page_reached() {
        let mut ledger = GalleryAckLedger::default();
        ledger.record_digit_request(ComponentGalleryPage::Type);
        ledger.record_ack(&ack_for_document(&gallery_document(
            ComponentGalleryPage::Type,
        )));
        let observation = ComponentGalleryObservation::from_acks(&ledger, 0, false);
        assert_eq!(observation.pages_reachable_by_digit(), 1);
        assert_eq!(observation.pages_painted(), 1);
        assert_eq!(observation.pages_visited(), ["Type"]);
    }

    #[test]
    fn an_ack_for_an_undeclared_page_is_a_defect_not_a_count() {
        let mut ledger = GalleryAckLedger::default();
        ledger.record_ack(&GalleryPaintedAck {
            page: "NotAPage".to_owned(),
            ..GalleryPaintedAck::default()
        });
        let observation = ComponentGalleryObservation::from_acks(&ledger, 0, false);
        assert_eq!(observation.pages_painted(), 0);
        assert_eq!(observation.clipped_or_overlapping_text(), 1);
        assert!(observation.text_defects()[0].contains("NotAPage"));
    }

    #[test]
    fn a_state_acked_in_only_one_density_is_not_counted_as_covered() {
        let mut ledger = GalleryAckLedger::default();
        let mut ack = ack_for_document(&gallery_document(ComponentGalleryPage::InteractionStates));
        ack.densities.truncate(1);
        ledger.record_ack(&ack);
        let observation = ComponentGalleryObservation::from_acks(&ledger, 0, false);
        assert_eq!(observation.states_painted(), 0);
        assert!(!observation.steam_deck_viewport_painted());
    }

    #[test]
    fn the_unbound_key_predicate_needs_a_press_that_changed_nothing() {
        let mut selection = GalleryPageSelection::default();
        let mut ledger = GalleryAckLedger::default();
        let observation = ComponentGalleryObservation::from_acks(&ledger, 0, false);
        assert!(
            !observation.unbound_digit_retained_page(),
            "no press, no claim"
        );
        apply_input(
            &mut selection,
            &mut ledger,
            WindowInput::key_down(WindowKey::Q),
        );
        let observation = ComponentGalleryObservation::from_acks(&ledger, 0, false);
        assert!(observation.unbound_digit_retained_page());
        // A step at an end is a bound key declining to move, never an unbound
        // press.
        apply_input(
            &mut selection,
            &mut ledger,
            WindowInput::key_down(WindowKey::BracketLeft),
        );
        assert_eq!(ledger.unbound_key_presses, 1);
    }

    /// The full operator walk, correlated with acks built from the real
    /// documents, satisfies every declared witness predicate.
    #[test]
    fn a_complete_session_satisfies_the_declared_witness_predicates() {
        let mut selection = GalleryPageSelection::default();
        let mut ledger = GalleryAckLedger::default();
        let ack = |ledger: &mut GalleryAckLedger, page: ComponentGalleryPage| {
            ledger.record_document_pushed();
            ledger.record_ack(&ack_for_document(&gallery_document(page)));
        };
        // The initial paint.
        ack(&mut ledger, selection.active());
        // The ten digits, in declared order.
        for page in ALL_GALLERY_PAGES {
            if let Some(digit) = page.digit() {
                apply_input(&mut selection, &mut ledger, WindowInput::key_down(digit));
                ack(&mut ledger, selection.active());
            }
        }
        // Step back to the first page, then forward through all fifteen.
        while selection.active() != ALL_GALLERY_PAGES[0] {
            apply_input(
                &mut selection,
                &mut ledger,
                WindowInput::key_down(WindowKey::BracketLeft),
            );
            ack(&mut ledger, selection.active());
        }
        while selection.active() != ALL_GALLERY_PAGES[GALLERY_PAGE_COUNT - 1] {
            apply_input(
                &mut selection,
                &mut ledger,
                WindowInput::key_down(WindowKey::BracketRight),
            );
            ack(&mut ledger, selection.active());
        }
        // One key that binds nothing.
        apply_input(
            &mut selection,
            &mut ledger,
            WindowInput::key_down(WindowKey::Q),
        );

        let observation = ComponentGalleryObservation::from_acks(&ledger, 0, true);
        assert_eq!(observation.pages_declared(), 15);
        assert_eq!(observation.pages_painted(), 15);
        assert_eq!(observation.pages_reachable_by_digit(), 10);
        assert_eq!(observation.pages_reachable_by_step(), 15);
        assert!(observation.unbound_digit_retained_page());
        assert_eq!(observation.states_declared(), COMPONENT_STATE_COUNT);
        assert_eq!(observation.states_painted(), COMPONENT_STATE_COUNT);
        assert!(observation.states_distinguishable_without_color());
        assert_eq!(observation.controls_declared(), 8);
        assert_eq!(observation.controls_painted(), 8);
        assert_eq!(observation.kind_role_pairs_unmapped(), 0);
        assert_eq!(observation.controls_unreachable_by_any_pair(), 0);
        assert_eq!(observation.compositions_declared(), 10);
        assert_eq!(observation.compositions_painted(), 10);
        assert_eq!(observation.mixer_column_structures_declared(), 5);
        assert_eq!(observation.mixer_column_structures_painted(), 5);
        assert_eq!(observation.mixer_column_names_beyond_track_header(), 0);
        assert!(observation.mixer_column_level_readout_is_midi_hex());
        assert!(observation.desktop_viewport_painted());
        assert!(observation.steam_deck_viewport_painted());
        assert!(observation.bands_retained_both_viewports());
        assert_eq!(observation.clipped_or_overlapping_text(), 0);
        assert!(observation.token_source_exact());
        assert!(observation.typeface_resolved());
        assert!(!observation.audio_or_midi_constructed());
        assert_eq!(observation.app_state_generation_delta(), 0);
        assert!(observation.window_closed());
        assert_eq!(observation.states_rendered().len(), COMPONENT_STATE_COUNT);
        assert_eq!(observation.controls_rendered().len(), 8);
        assert_eq!(observation.compositions_rendered().len(), 10);
    }

    /// The serialized observation carries every field the witness schema
    /// declares.
    #[test]
    fn the_serialized_observation_carries_every_declared_field() {
        let ledger = GalleryAckLedger::default();
        let observation = ComponentGalleryObservation::from_acks(&ledger, 0, true);
        let serialized = serde_json::to_value(&observation).expect("the observation serializes");
        for field in [
            "pages_declared",
            "pages_painted",
            "pages_reachable_by_digit",
            "pages_reachable_by_step",
            "unbound_digit_retained_page",
            "states_declared",
            "states_painted",
            "states_distinguishable_without_color",
            "controls_declared",
            "controls_painted",
            "kind_role_pairs_unmapped",
            "controls_unreachable_by_any_pair",
            "compositions_declared",
            "compositions_painted",
            "mixer_column_structures_declared",
            "mixer_column_structures_painted",
            "mixer_column_names_beyond_track_header",
            "mixer_column_level_readout_is_midi_hex",
            "desktop_viewport_painted",
            "steam_deck_viewport_painted",
            "bands_retained_both_viewports",
            "clipped_or_overlapping_text",
            "token_source_exact",
            "typeface_resolved",
            "audio_or_midi_constructed",
            "app_state_generation_delta",
            "window_closed",
        ] {
            assert!(
                serialized.get(field).is_some(),
                "the observation must carry {field}"
            );
        }
    }

    #[test]
    fn css_colors_resolve_only_through_the_vocabulary() {
        assert_eq!(parse_css_rgb("rgb(12, 16, 21)"), Some((12, 16, 21)));
        assert_eq!(
            parse_css_rgb("rgba(101, 229, 255, 0.28)"),
            Some((101, 229, 255))
        );
        assert_eq!(parse_css_rgb("color(srgb 0 0 0)"), None);
        assert!(css_color_resolves("rgb(12, 16, 21)"), "bg canvas resolves");
        assert!(
            css_color_resolves("rgba(101, 229, 255, 0.28)"),
            "the halo is the focus accent at the authored opacity"
        );
        assert!(
            !css_color_resolves("rgb(1, 2, 3)"),
            "a foreign color must not resolve"
        );
    }

    // ----- the derived silence --------------------------------------------

    #[test]
    fn the_gallery_scene_constructs_no_audio_output_and_no_midi_source() {
        assert!(!audio_or_midi_constructed());
    }

    /// The scan is capable of returning `true`, so a scan that had quietly
    /// stopped matching anything cannot pass as silence.
    #[test]
    fn the_silence_derivation_reports_true_when_a_construction_is_present() {
        let constructed = format!("let output = {}{}::new();", "Cpal", "AudioOutput");
        assert!(source_constructs_audio_or_midi(&constructed));
        let midi = format!("let source = {}{}::new();", "CorridorsMidi", "EventSource");
        assert!(source_constructs_audio_or_midi(&midi));
    }

    #[test]
    fn the_silence_derivation_does_not_match_its_own_needles() {
        // The production half of this very module, comments stripped, must
        // not contain a needle — otherwise the derivation would report its
        // own detector as a construction.
        assert!(!source_constructs_audio_or_midi(&production_source(
            GALLERY_SCENE_SOURCE
        )));
    }

    #[test]
    fn the_silence_derivation_reads_this_modules_shipping_source() {
        let production = production_source(GALLERY_SCENE_SOURCE);
        assert!(
            production.contains("fn gallery_document"),
            "the scan must read the module that builds the documents"
        );
        assert!(
            production.contains("fn run"),
            "the scan must read the module that runs the window"
        );
        assert!(
            !production.contains("fn ack_for_document"),
            "the scan must stop at the first test module"
        );
    }
}
