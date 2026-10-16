//! The authored component vocabulary: controls, compositions, and the
//! non-color signal marks.
//!
//! **Components paint. Views compose. The reducer decides.** The painting now
//! happens in the webview page from the generated token table; what lives
//! here is the *declaration* side of that contract — the closed families the
//! serialized view model, the token generator, the component gallery, and the
//! acceptance proofs all resolve through:
//!
//! - the closed control family ([`ComponentControl`]) and the total
//!   kind × role selector ([`control_for`]);
//! - the closed composition family ([`ShellComposition`]) and its region
//!   binding ([`ShellRegion`]);
//! - the authored non-color signals: hint tones, status marks, the focus
//!   cursor, and the unavailable mark.
//!
//! Nothing here paints, and nothing here decides. A component source owns no
//! state and caches none: it may not name `AppState`, and it may not convert
//! an input into a `SemanticAction`. Deciding what an input means needs
//! focus, the active context, and the reducer's notion of what is valid right
//! now, all of which live on the other side of this boundary. That passivity
//! is the boundary this module exists to hold, and
//! `tests/component_composition.rs` proves it of every component source.
//!
//! Realizes `valueObject.Shell.ComponentControl`,
//! `valueObject.Shell.ShellComposition`,
//! `requirement.configurable_control_family`, and
//! `requirement.reusable_shell_compositions`.

use serde::Serialize;

use crate::control::SemanticControlKind;
use crate::shell::component_state::{
    ComponentState, NonColorSignal, ALL_COMPONENT_STATES, COMPONENT_STATE_COUNT,
    LOADING_PROGRESS_WORDS,
};
use crate::shell::tokens::SemanticColor;

// ===========================================================================
// The control family
// ===========================================================================

/// The role a requesting composition asks a control in.
///
/// The same control kind is a parameter row on a listed surface and a fader in
/// a mixer strip, so kind alone cannot select a shape. The set is closed at
/// four: a fifth role is a design decision about a surface the product does not
/// yet name, and adding one fails compilation at [`control_for`] rather than
/// quietly reusing an existing role's shapes.
///
/// The roles are the surfaces `DESIGN.md` already declares, not an invented
/// taxonomy — the PATCH strip and its rows (`DESIGN.md`), the sixteen
/// compact mixer columns (`:462`), the persistent Utility/Inspector (`:444`,
/// `:466`), and the nested option modal with trapped focus (`:458`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PresentationRole {
    /// PATCH main surface, MIXER inspector, Utility panel — stacked labelled
    /// rows that occupy the full width they are given.
    ListedRow,
    /// MIXER track columns — the sixteen fixed compact columns with hairline
    /// separators.
    VerticalStrip,
    /// Utility/Inspector panel entries that are not full rows.
    PanelEntry,
    /// Focus-trapped option modals and the later Sample Browser.
    ModalEntry,
}

/// Every declared role, in declaration order.
pub const ALL_PRESENTATION_ROLES: [PresentationRole; PRESENTATION_ROLE_COUNT] = [
    PresentationRole::ListedRow,
    PresentationRole::VerticalStrip,
    PresentationRole::PanelEntry,
    PresentationRole::ModalEntry,
];

/// How many roles the vocabulary declares.
pub const PRESENTATION_ROLE_COUNT: usize = 4;

impl PresentationRole {
    /// Returns the canonical name of this role.
    pub const fn canonical_name(self) -> &'static str {
        match self {
            Self::ListedRow => "ListedRow",
            Self::VerticalStrip => "VerticalStrip",
            Self::PanelEntry => "PanelEntry",
            Self::ModalEntry => "ModalEntry",
        }
    }
}

/// The closed family of configurable controls.
///
/// Each presents one `SemanticControlViewModel` in one [`ComponentState`] and
/// returns typed intent rather than acting. The set is closed at eight, and
/// every one of the eight is reachable from [`control_for`] by at least one
/// kind × role pair — a control nothing can ask for is dead code that would
/// otherwise pass every other check in this module.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ComponentControl {
    /// A labelled row carrying a numeric value and its unit.
    ParameterRow,
    /// A labelled row carrying one selected option from a set.
    ChoiceRow,
    /// A labelled row carrying a binary value.
    Toggle,
    /// A short horizontal slider that is not a full row.
    CompactSlider,
    /// The tall vertical level control of a mixer strip.
    Fader,
    /// A read-only level readout in a mixer strip.
    Meter,
    /// A row naming an asset, which opens a browser rather than adjusting.
    BrowserRow,
    /// One selectable entry inside a focus-trapped option modal.
    ModalOption,
}

/// Every declared control, in declaration order.
pub const ALL_COMPONENT_CONTROLS: [ComponentControl; COMPONENT_CONTROL_COUNT] = [
    ComponentControl::ParameterRow,
    ComponentControl::ChoiceRow,
    ComponentControl::Toggle,
    ComponentControl::CompactSlider,
    ComponentControl::Fader,
    ComponentControl::Meter,
    ComponentControl::BrowserRow,
    ComponentControl::ModalOption,
];

/// How many controls the family declares.
pub const COMPONENT_CONTROL_COUNT: usize = 8;

/// The states a control living in a mixer track column can be handed.
///
/// This is every declared state. `Muted` and `Soloed` describe the track a
/// strip represents, so only the controls that represent a track can receive
/// them.
const MIXER_STRIP_STATES: [ComponentState; COMPONENT_STATE_COUNT] = ALL_COMPONENT_STATES;

/// The states a control outside a mixer track column can be handed.
///
/// Every declared state except `Muted` and `Soloed`. Mute and solo are
/// mixer-track concepts: a Utility panel row has no track to be muted, and a
/// control that declares them would owe the gallery a specimen it cannot
/// meaningfully paint.
///
/// A mixer strip's own mute and solo controls are deliberately *not* an
/// exception. Their on-ness is a value they already paint from their view data
/// (`SemanticControlValue::Parameter(ParameterValue::Toggle)`); handing them
/// `Muted` as a state as well would give one fact two representations that can
/// disagree.
const NON_TRACK_STATES: [ComponentState; 8] = [
    ComponentState::Resting,
    ComponentState::Focused,
    ComponentState::Adjusting,
    ComponentState::Disabled,
    ComponentState::Loading,
    ComponentState::Unavailable,
    ComponentState::Error,
    ComponentState::Selected,
];

impl ComponentControl {
    /// Returns the canonical name of this control.
    pub const fn canonical_name(self) -> &'static str {
        match self {
            Self::ParameterRow => "ParameterRow",
            Self::ChoiceRow => "ChoiceRow",
            Self::Toggle => "Toggle",
            Self::CompactSlider => "CompactSlider",
            Self::Fader => "Fader",
            Self::Meter => "Meter",
            Self::BrowserRow => "BrowserRow",
            Self::ModalOption => "ModalOption",
        }
    }

    /// The states this control can be handed.
    ///
    /// A control that can never be muted or soloed declares that here rather
    /// than silently omitting a specimen from the gallery. Every control
    /// declares at least `Resting`, `Focused`, and `Disabled`, and the union
    /// across the family covers all nine declared states, so no state is
    /// applicable to nothing.
    pub const fn applicable_states(self) -> &'static [ComponentState] {
        match self {
            Self::Fader | Self::Meter => &MIXER_STRIP_STATES,
            Self::ParameterRow
            | Self::ChoiceRow
            | Self::Toggle
            | Self::CompactSlider
            | Self::BrowserRow
            | Self::ModalOption => &NON_TRACK_STATES,
        }
    }

    /// Whether this control can be handed `state`.
    pub fn accepts(self, state: ComponentState) -> bool {
        self.applicable_states().contains(&state)
    }
}

/// Every declared semantic control kind, in the order
/// `crate::control::SemanticControlKind` declares them.
///
/// The kind vocabulary is reducer-adjacent and owned elsewhere; this module
/// only enumerates it so selection can be shown total over it. Adding a kind
/// there fails compilation here — in [`control_for`] and in this module's
/// iteration test — rather than leaving a pair nobody selects for.
pub const ALL_SEMANTIC_CONTROL_KINDS: [SemanticControlKind; SEMANTIC_CONTROL_KIND_COUNT] = [
    SemanticControlKind::Continuous,
    SemanticControlKind::Stepped,
    SemanticControlKind::Choice,
    SemanticControlKind::Toggle,
    SemanticControlKind::Asset,
    SemanticControlKind::Identity,
    SemanticControlKind::Surface,
    SemanticControlKind::BrowserParent,
    SemanticControlKind::BrowserFolder,
    SemanticControlKind::BrowserFile,
    SemanticControlKind::BrowserCancel,
];

/// How many semantic control kinds selection covers.
pub const SEMANTIC_CONTROL_KIND_COUNT: usize = 11;

/// What one `(SemanticControlKind, PresentationRole)` pair resolves to.
///
/// The second variant is the point of the type. A pair that no composition can
/// ask — a choice in a mixer track column, say — resolves to an explicit
/// refusal rather than to a generic row, so "we decided this cannot be asked"
/// and "we never thought about it" stay distinguishable.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ControlSelection {
    /// The pair resolves to exactly this control.
    Control(ComponentControl),
    /// This kind is not askable in this role.
    NotAskableInRole,
}

impl ControlSelection {
    /// The control this selection resolves to, if the pair is askable.
    pub const fn control(self) -> Option<ComponentControl> {
        match self {
            Self::Control(control) => Some(control),
            Self::NotAskableInRole => None,
        }
    }

    /// Whether a composition may ask this pair.
    pub const fn is_askable(self) -> bool {
        matches!(self, Self::Control(_))
    }
}

/// Resolves a control kind and the role it is asked in to exactly one control.
///
/// This is a single `match` on the tuple, deliberately. Rust checks tuple-match
/// exhaustiveness, so adding a `SemanticControlKind` or a [`PresentationRole`]
/// is a compile error naming this function. Nested matches with a `_ =>` arm
/// would not be, and a defaulted mapping is behaviorally indistinguishable from
/// a considered one.
pub const fn control_for(kind: SemanticControlKind, role: PresentationRole) -> ControlSelection {
    use ComponentControl as Control;
    use ControlSelection::{Control as Asks, NotAskableInRole};
    use PresentationRole as Role;
    use SemanticControlKind as Kind;

    match (kind, role) {
        // A numeric value. Stepped differs from Continuous only in how it
        // moves, which is the control's adjustment behaviour and not its
        // shape, so the two select identically.
        (Kind::Continuous | Kind::Stepped, Role::ListedRow) => Asks(Control::ParameterRow),
        (Kind::Continuous | Kind::Stepped, Role::VerticalStrip) => Asks(Control::Fader),
        (Kind::Continuous | Kind::Stepped, Role::PanelEntry) => Asks(Control::CompactSlider),
        (Kind::Continuous | Kind::Stepped, Role::ModalEntry) => Asks(Control::ModalOption),

        // One selected option from a set. A mixer column carries a level, a
        // pan, and the two track toggles — never a choice.
        (Kind::Choice, Role::ListedRow | Role::PanelEntry) => Asks(Control::ChoiceRow),
        (Kind::Choice, Role::VerticalStrip) => NotAskableInRole,
        (Kind::Choice, Role::ModalEntry) => Asks(Control::ModalOption),

        // A binary value. The mixer column's mute and solo are toggles in a
        // strip, so this is the one kind every listed role shares a shape for.
        (Kind::Toggle, Role::ListedRow | Role::VerticalStrip | Role::PanelEntry) => {
            Asks(Control::Toggle)
        }
        (Kind::Toggle, Role::ModalEntry) => Asks(Control::ModalOption),

        // A named asset. It opens a browser wherever it is asked, including
        // inside a modal, so the browser row is its shape in three roles and
        // is not askable in a mixer column.
        (Kind::Asset, Role::ListedRow | Role::PanelEntry | Role::ModalEntry) => {
            Asks(Control::BrowserRow)
        }
        (Kind::Asset, Role::VerticalStrip) => NotAskableInRole,

        // A read-only value. In a listed row or a panel it reads as a labelled
        // row; in a mixer column a read-only level *is* the meter. This pair is
        // the whole argument for selecting on the role: one kind, two shapes,
        // decided by where it was asked and not by what it holds.
        (Kind::Identity, Role::ListedRow | Role::PanelEntry) => Asks(Control::ParameterRow),
        (Kind::Identity, Role::VerticalStrip) => Asks(Control::Meter),
        (Kind::Identity, Role::ModalEntry) => Asks(Control::ModalOption),

        // A surface summary — the read-only root the Utility and Inspector
        // panels carry (`DESIGN.md`). It is a row, and a mixer column has
        // no surface to summarize.
        (Kind::Surface, Role::ListedRow | Role::PanelEntry) => Asks(Control::ParameterRow),
        (Kind::Surface, Role::VerticalStrip) => NotAskableInRole,
        (Kind::Surface, Role::ModalEntry) => Asks(Control::ModalOption),

        // Browser navigation rows exist only inside the focus-trapped Sample
        // Browser. They deliberately reuse ModalOption: the semantic kind
        // carries parent/folder/file/cancel meaning while the shared control
        // paints the selectable row and its explicit marker.
        (
            Kind::BrowserParent | Kind::BrowserFolder | Kind::BrowserFile | Kind::BrowserCancel,
            Role::ModalEntry,
        ) => Asks(Control::ModalOption),
        (
            Kind::BrowserParent | Kind::BrowserFolder | Kind::BrowserFile | Kind::BrowserCancel,
            Role::ListedRow | Role::VerticalStrip | Role::PanelEntry,
        ) => NotAskableInRole,
    }
}

/// The mark a reserved region shows when its data did not arrive.
///
/// A reserved region that is simply left blank reads as "there is nothing
/// here"; one filled with a decorative mark reads as data. Neither is true of a
/// region whose data did not arrive, so it is marked explicitly. Not authored
/// in the design file; raised in the component-composition handoff rather than
/// left to look authoritative.
pub const UNAVAILABLE_MARK: &str = "--";

// ===========================================================================
// The composition family
// ===========================================================================

/// A named region of the shell.
///
/// The five that `ShellFrameObservation` emits, plus the whole frame that
/// contains them. The set is closed: a new region is a change to the structural
/// bands, which is a design decision authored in `DESIGN.md` and reflected in
/// the observation, never a layout a composition invents.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShellRegion {
    /// The whole viewport — every band, and the composition that arranges them.
    /// This is not an observation region; it is what contains all of them.
    WholeFrame,
    /// Product · context · status.
    ContextLine,
    /// Identity and header.
    IdentityHeader,
    /// The main surface half of the workspace band.
    MainWorkspace,
    /// The always-visible Utility/Inspector half of the workspace band.
    PersistentSideRegion,
    /// Current path and valid control hints.
    Footer,
}

/// The region names `ShellFrameObservation` emits, in the order it emits them.
///
/// Spelled exactly as the observation spells them, because the correspondence
/// between a composition and the rectangle reported for it is what makes the
/// observation evidence rather than decoration.
pub const OBSERVED_REGION_NAMES: [&str; 5] = [
    "contextLine",
    "identityHeader",
    "mainWorkspace",
    "persistentSideRegion",
    "footer",
];

impl ShellRegion {
    /// The name `ShellFrameObservation` reports this region under, if it
    /// reports it at all.
    ///
    /// [`ShellRegion::WholeFrame`] returns `None`: the frame is not observed as
    /// a region, it is the thing the observed regions tile.
    pub const fn observation_name(self) -> Option<&'static str> {
        match self {
            Self::WholeFrame => None,
            Self::ContextLine => Some("contextLine"),
            Self::IdentityHeader => Some("identityHeader"),
            Self::MainWorkspace => Some("mainWorkspace"),
            Self::PersistentSideRegion => Some("persistentSideRegion"),
            Self::Footer => Some("footer"),
        }
    }
}

/// The closed family of reusable compositions.
///
/// Each fills one shell region. The set is closed at ten, and adding one is
/// a compile error at [`ShellComposition::region`] rather than a composition
/// bound to no region.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShellComposition {
    /// The four structural bands and the workspace split that together tile the
    /// viewport.
    ApplicationShell,
    /// The PATCH / MIXER context indicator and status.
    ContextSwitch,
    /// The identity and header band.
    IdentityHeader,
    /// A titled group of rows within a surface.
    Section,
    /// One row of the PATCH strip.
    PatchStripRow,
    /// The PATCH main workspace: the identity-and-routing header, the
    /// instrument selector, the envelope group, and one titled group per
    /// ordered effect slot — groups arranging groups, the way
    /// [`Self::MixerStripBank`] arranges track columns rather than controls.
    PatchStrip,
    /// The sixteen fixed mixer track columns, side by side.
    MixerStripBank,
    /// The always-visible Utility/Inspector panel.
    UtilityInspectorPanel,
    /// The subordinate detail surface for whichever capability the model
    /// names. One composition, not one per subject kind: the instrument case
    /// and the effect case differ only in the projection slice handed to it.
    CapabilityDetailShell,
    /// The current path and the valid control hints.
    Footer,
}

/// Every declared composition, in declaration order.
pub const ALL_SHELL_COMPOSITIONS: [ShellComposition; SHELL_COMPOSITION_COUNT] = [
    ShellComposition::ApplicationShell,
    ShellComposition::ContextSwitch,
    ShellComposition::IdentityHeader,
    ShellComposition::Section,
    ShellComposition::PatchStripRow,
    ShellComposition::PatchStrip,
    ShellComposition::MixerStripBank,
    ShellComposition::UtilityInspectorPanel,
    ShellComposition::CapabilityDetailShell,
    ShellComposition::Footer,
];

/// How many compositions the family declares.
pub const SHELL_COMPOSITION_COUNT: usize = 10;

impl ShellComposition {
    /// Returns the canonical name of this composition.
    pub const fn canonical_name(self) -> &'static str {
        match self {
            Self::ApplicationShell => "ApplicationShell",
            Self::ContextSwitch => "ContextSwitch",
            Self::IdentityHeader => "IdentityHeader",
            Self::Section => "Section",
            Self::PatchStripRow => "PatchStripRow",
            Self::PatchStrip => "PatchStrip",
            Self::MixerStripBank => "MixerStripBank",
            Self::UtilityInspectorPanel => "UtilityInspectorPanel",
            Self::CapabilityDetailShell => "CapabilityDetailShell",
            Self::Footer => "Footer",
        }
    }

    /// The shell region this composition fills.
    ///
    /// `Section`, `PatchStripRow`, `PatchStrip`, `MixerStripBank`, and
    /// `CapabilityDetailShell` all fill the main workspace: a section groups
    /// rows, a strip row is one of them, the strip and the bank arrange the
    /// PATCH groups and the mixer's sixteen track columns across it, and the
    /// detail shell replaces the workspace body while a detail entry is open.
    /// All five are main-surface structure. The binding is many-to-one by
    /// design and remains so — what must hold is that every observed region has
    /// a composition, not that every composition has a region to itself.
    pub const fn region(self) -> ShellRegion {
        match self {
            Self::ApplicationShell => ShellRegion::WholeFrame,
            Self::ContextSwitch => ShellRegion::ContextLine,
            Self::IdentityHeader => ShellRegion::IdentityHeader,
            Self::Section
            | Self::PatchStripRow
            | Self::PatchStrip
            | Self::MixerStripBank
            | Self::CapabilityDetailShell => ShellRegion::MainWorkspace,
            Self::UtilityInspectorPanel => ShellRegion::PersistentSideRegion,
            Self::Footer => ShellRegion::Footer,
        }
    }
}

// ===========================================================================
// Non-color signals: hint tones, status marks, the focus cursor
// ===========================================================================

/// What separates two hints on the line.
///
/// The footer shows only the actions valid at the focused target, in the
/// design's compact form: `D-PAD NAV · A CONFIRM · B BACK`. Hints are how a
/// player learns the grammar of the interface, so the separator, the order,
/// and the tones are settled here once rather than at each footer.
pub const HINT_SEPARATOR: &str = " · ";

/// The tone a hint reads in.
///
/// Four, as the design file's CLI Hint component set declares.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HintTone {
    /// An ordinary available action.
    Neutral,
    /// An action that moves or confirms focus.
    Focus,
    /// An action that adjusts a value.
    Adjust,
    /// An action that leaves or cancels.
    Back,
}

/// Every declared tone, in declaration order.
pub const ALL_HINT_TONES: [HintTone; 4] = [
    HintTone::Neutral,
    HintTone::Focus,
    HintTone::Adjust,
    HintTone::Back,
];

impl HintTone {
    /// The color role this tone paints in.
    pub const fn color(self) -> SemanticColor {
        match self {
            Self::Neutral => SemanticColor::TextMuted,
            Self::Focus => SemanticColor::AccentFocus,
            Self::Adjust => SemanticColor::AccentAdjust,
            Self::Back => SemanticColor::TextSecondary,
        }
    }

    /// The canonical name of this tone.
    pub const fn canonical_name(self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Focus => "focus",
            Self::Adjust => "adjust",
            Self::Back => "back",
        }
    }
}

/// One valid action, as it reads in the footer.
///
/// This primitive never decides which actions are valid. Validity is
/// reducer-owned and arrives through the view model as an immutable slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActionHint<'a> {
    /// The control that performs it, e.g. `D-PAD` or `A`.
    pub chord: &'a str,
    /// What it does, e.g. `NAV` or `CONFIRM`.
    pub action: &'a str,
    /// How it reads.
    pub tone: HintTone,
}

impl<'a> ActionHint<'a> {
    /// Builds a hint.
    pub const fn new(chord: &'a str, action: &'a str, tone: HintTone) -> Self {
        Self {
            chord,
            action,
            tone,
        }
    }

    /// The rendered text of this hint, e.g. `D-PAD NAV`.
    pub fn text(&self) -> String {
        format!("{} {}", self.chord, self.action)
    }
}

/// The word painted when an erroring component supplies no failure text.
///
/// A caller should supply typed text. This exists so that the absence of it
/// still reads without color, which is the whole point of the status mark.
pub const GENERIC_FAILURE_WORD: &str = "Failed";

/// Which phase of a structural edit a loading component is reporting.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LoadingPhase {
    /// The edit has been requested and awaits admission.
    Loading,
    /// The request is being validated before graph preparation.
    Validating,
    /// The edit has been prepared but is not yet active.
    Preparing,
    /// The prepared edit is being made active.
    Activating,
}

impl LoadingPhase {
    /// The authored word for this phase.
    pub const fn word(self) -> &'static str {
        match self {
            Self::Loading => LOADING_PROGRESS_WORDS[0],
            Self::Validating => LOADING_PROGRESS_WORDS[1],
            Self::Preparing => LOADING_PROGRESS_WORDS[2],
            Self::Activating => LOADING_PROGRESS_WORDS[3],
        }
    }
}

/// The caller-varying text behind a status mark.
///
/// Only two states need it: the phase a loading component reports and the
/// typed failure an erroring one does. Every other state ignores it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusDetail<'a> {
    /// This component's mark is a fixed authored word or a shape.
    None,
    /// The structural-edit phase a `Loading` component reports.
    Progress(LoadingPhase),
    /// The short typed failure an `Error` component reports.
    Failure(&'a str),
}

impl<'a> StatusDetail<'a> {
    /// The progress word a loading component shows.
    ///
    /// Missing detail falls back to the first authored phase rather than to
    /// nothing: a loading component showing only color is the failure the
    /// status mark exists to prevent.
    const fn progress_word(self) -> &'a str {
        match self {
            Self::Progress(phase) => phase.word(),
            Self::None | Self::Failure(_) => LoadingPhase::Loading.word(),
        }
    }

    /// The failure text an erroring component shows.
    ///
    /// Missing or empty detail falls back to [`GENERIC_FAILURE_WORD`], for the
    /// same reason.
    const fn failure_text(self) -> &'a str {
        match self {
            Self::Failure(text) if !text.is_empty() => text,
            Self::Failure(_) | Self::None | Self::Progress(_) => GENERIC_FAILURE_WORD,
        }
    }
}

/// The mark a state puts on screen.
///
/// The mark is resolved from the [`NonColorSignal`] declared on the state, and
/// the accent comes along with it. No state's status is invented here and no
/// second visual language is introduced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusMark<'a> {
    /// Text painted in the state's declared accent.
    Text {
        /// What the mark says.
        text: &'a str,
        /// The role it paints in.
        color: SemanticColor,
    },
    /// A filled row plus a filled mark, for the state that fills its row.
    Selection {
        /// The row fill role.
        fill: SemanticColor,
        /// The mark role, which is not the fill: a mark painted in the fill
        /// color would be invisible on it, leaving selection color-only.
        mark: SemanticColor,
    },
}

/// Resolves the mark `state` carries, or `None` when it carries no status.
///
/// Every state that carries a status carries text or shape: `Disabled` says
/// `Locked`, `Loading` reports `Preparing`/`Activating`, `Error` carries its
/// short typed failure text, mute and solo say `M ON`/`S ON`
/// (`DESIGN.md`), and `Selected` pairs its fill with a filled mark.
/// `Resting`, `Focused`, and `Adjusting` carry no status: rest is the
/// baseline, and focus and adjustment are carried by the frame and the cursor.
pub fn status_mark<'a>(state: ComponentState, detail: StatusDetail<'a>) -> Option<StatusMark<'a>> {
    let appearance = state.appearance();
    match state {
        ComponentState::Resting | ComponentState::Focused | ComponentState::Adjusting => None,
        ComponentState::Disabled
        | ComponentState::Unavailable
        | ComponentState::Muted
        | ComponentState::Soloed => Some(StatusMark::Text {
            text: declared_word(appearance.signal),
            color: appearance.accent,
        }),
        ComponentState::Loading => Some(StatusMark::Text {
            text: detail.progress_word(),
            color: appearance.accent,
        }),
        ComponentState::Error => Some(StatusMark::Text {
            text: detail.failure_text(),
            color: appearance.accent,
        }),
        ComponentState::Selected => Some(StatusMark::Selection {
            fill: appearance.accent,
            mark: SemanticColor::TextPrimary,
        }),
    }
}

/// The fixed authored word a state's declared signal carries.
///
/// A state routed here declares [`NonColorSignal::Word`]. The empty string is
/// unreachable; the test in this module holds that every state routed here
/// declares a word, so this stays total without a panic in a render path.
const fn declared_word(signal: NonColorSignal) -> &'static str {
    match signal {
        NonColorSignal::Word(word) => word,
        NonColorSignal::Shape | NonColorSignal::ProgressWord | NonColorSignal::TypedFailure => "",
    }
}

/// Whether this state's declared keyline paints as a frame.
///
/// True exactly for the states that declare `KEYLINE_EMPHASIS_PX`; the test
/// in this module holds that correspondence rather than letting the two drift.
pub const fn frames(state: ComponentState) -> bool {
    match state {
        ComponentState::Focused
        | ComponentState::Adjusting
        | ComponentState::Loading
        | ComponentState::Unavailable
        | ComponentState::Error => true,
        ComponentState::Resting
        | ComponentState::Disabled
        | ComponentState::Muted
        | ComponentState::Soloed
        | ComponentState::Selected => false,
    }
}

/// Whether this state draws the `>` cursor in the reserved column.
///
/// Adjustment is focus that is being edited, so it keeps the cursor: losing it
/// mid-edit would read as losing the row.
pub const fn draws_cursor(state: ComponentState) -> bool {
    match state {
        ComponentState::Focused | ComponentState::Adjusting => true,
        ComponentState::Resting
        | ComponentState::Disabled
        | ComponentState::Loading
        | ComponentState::Unavailable
        | ComponentState::Error
        | ComponentState::Muted
        | ComponentState::Soloed
        | ComponentState::Selected => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::tokens::{KEYLINE_EMPHASIS_PX, KEYLINE_RESTING_PX};
    use std::collections::HashSet;

    /// The pairs the model declares un-askable, and the only ones.
    ///
    /// Pinned as data rather than left implicit so that a later work package
    /// cannot quietly switch a pair off: turning an askable pair into a refusal
    /// removes a control from a surface, which is a product change, and it
    /// fails here.
    ///
    /// All three are mixer track columns. A column carries a level, a pan, and
    /// the two track toggles (`DESIGN.md`); it never carries a choice,
    /// an asset, or a surface summary.
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

    fn is_declared_not_askable(kind: SemanticControlKind, role: PresentationRole) -> bool {
        NOT_ASKABLE_PAIRS
            .iter()
            .any(|(declared_kind, declared_role)| *declared_kind == kind && *declared_role == role)
    }

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

    #[test]
    fn the_role_vocabulary_holds_exactly_four_roles() {
        assert_eq!(PRESENTATION_ROLE_COUNT, 4);
        assert_eq!(ALL_PRESENTATION_ROLES.len(), PRESENTATION_ROLE_COUNT);
    }

    /// Adding a role without adding it to [`ALL_PRESENTATION_ROLES`] fails
    /// here: the `match` is exhaustive, so the new role must be named, and
    /// naming it makes the iteration check fail until the array is updated.
    #[test]
    fn iteration_yields_every_declared_role() {
        for role in ALL_PRESENTATION_ROLES {
            let named = match role {
                PresentationRole::ListedRow => "ListedRow",
                PresentationRole::VerticalStrip => "VerticalStrip",
                PresentationRole::PanelEntry => "PanelEntry",
                PresentationRole::ModalEntry => "ModalEntry",
            };
            assert_eq!(named, role.canonical_name());
        }
        let seen: HashSet<_> = ALL_PRESENTATION_ROLES.into_iter().collect();
        assert_eq!(
            seen.len(),
            PRESENTATION_ROLE_COUNT,
            "a role appears twice in ALL_PRESENTATION_ROLES"
        );
    }

    #[test]
    fn the_control_family_holds_exactly_eight_controls() {
        assert_eq!(COMPONENT_CONTROL_COUNT, 8);
        assert_eq!(ALL_COMPONENT_CONTROLS.len(), COMPONENT_CONTROL_COUNT);
    }

    #[test]
    fn iteration_yields_every_declared_control() {
        for control in ALL_COMPONENT_CONTROLS {
            let named = match control {
                ComponentControl::ParameterRow => "ParameterRow",
                ComponentControl::ChoiceRow => "ChoiceRow",
                ComponentControl::Toggle => "Toggle",
                ComponentControl::CompactSlider => "CompactSlider",
                ComponentControl::Fader => "Fader",
                ComponentControl::Meter => "Meter",
                ComponentControl::BrowserRow => "BrowserRow",
                ComponentControl::ModalOption => "ModalOption",
            };
            assert_eq!(named, control.canonical_name());
        }
        let seen: HashSet<_> = ALL_COMPONENT_CONTROLS.into_iter().collect();
        assert_eq!(
            seen.len(),
            COMPONENT_CONTROL_COUNT,
            "a control appears twice in ALL_COMPONENT_CONTROLS"
        );
    }

    /// The kind vocabulary is owned outside this module, so its enumeration
    /// here is checked the same way: the `match` in [`kind_name`] is
    /// exhaustive, so an added kind must be named, and naming it fails this
    /// count until [`ALL_SEMANTIC_CONTROL_KINDS`] is updated too.
    #[test]
    fn iteration_yields_every_declared_control_kind() {
        assert_eq!(SEMANTIC_CONTROL_KIND_COUNT, 11);
        assert_eq!(
            ALL_SEMANTIC_CONTROL_KINDS.len(),
            SEMANTIC_CONTROL_KIND_COUNT
        );
        let names: HashSet<&str> = ALL_SEMANTIC_CONTROL_KINDS
            .into_iter()
            .map(kind_name)
            .collect();
        assert_eq!(
            names.len(),
            SEMANTIC_CONTROL_KIND_COUNT,
            "a kind appears twice in ALL_SEMANTIC_CONTROL_KINDS"
        );
    }

    /// Selection is total over kind × role.
    ///
    /// Every declared pair resolves, every pair not declared un-askable
    /// resolves to exactly one control, and the un-askable set is exactly the
    /// three pinned pairs — so neither a missing mapping nor a silently
    /// widened refusal survives.
    #[test]
    fn selection_is_total_over_kind_and_role() {
        let pairs = every_pair();
        assert_eq!(
            pairs.len(),
            SEMANTIC_CONTROL_KIND_COUNT * PRESENTATION_ROLE_COUNT,
            "the pair enumeration lost a kind or a role"
        );
        for (kind, role) in pairs {
            let selection = control_for(kind, role);
            if is_declared_not_askable(kind, role) {
                assert_eq!(
                    selection,
                    ControlSelection::NotAskableInRole,
                    "{} in {} is declared un-askable but resolved to {selection:?}",
                    kind_name(kind),
                    role.canonical_name()
                );
            } else {
                assert!(
                    selection.is_askable(),
                    "{} in {} resolves to nothing; selection is not total",
                    kind_name(kind),
                    role.canonical_name()
                );
            }
        }
    }

    /// Every one of the eight controls is reachable by at least one pair.
    ///
    /// This catches a control that exists but nothing can ask for — dead code
    /// that passes every other check in this module.
    #[test]
    fn every_control_is_reachable_by_at_least_one_pair() {
        let reachable: HashSet<ComponentControl> = every_pair()
            .into_iter()
            .filter_map(|(kind, role)| control_for(kind, role).control())
            .collect();
        let declared: HashSet<ComponentControl> = ALL_COMPONENT_CONTROLS.into_iter().collect();
        let unreachable: Vec<&str> = ALL_COMPONENT_CONTROLS
            .into_iter()
            .filter(|control| !reachable.contains(control))
            .map(ComponentControl::canonical_name)
            .collect();
        assert!(
            unreachable.is_empty(),
            "no kind and role pair asks for: {}",
            unreachable.join(", ")
        );
        assert_eq!(reachable, declared);
    }

    #[test]
    fn every_control_declares_states_drawn_from_the_state_vocabulary() {
        for control in ALL_COMPONENT_CONTROLS {
            for state in control.applicable_states() {
                assert!(
                    ALL_COMPONENT_STATES.contains(state),
                    "{} declares {} which is not a declared state",
                    control.canonical_name(),
                    state.canonical_name()
                );
            }
        }
    }

    #[test]
    fn every_control_declares_resting_focused_and_disabled() {
        for control in ALL_COMPONENT_CONTROLS {
            for required in [
                ComponentState::Resting,
                ComponentState::Focused,
                ComponentState::Disabled,
            ] {
                assert!(
                    control.accepts(required),
                    "{} does not declare {}",
                    control.canonical_name(),
                    required.canonical_name()
                );
            }
        }
    }

    /// No state is applicable to nothing. A state the whole family refuses
    /// would be a state the gallery can never show.
    #[test]
    fn the_declared_states_cover_every_component_state() {
        for state in ALL_COMPONENT_STATES {
            assert!(
                ALL_COMPONENT_CONTROLS
                    .into_iter()
                    .any(|control| control.accepts(state)),
                "no control declares {}",
                state.canonical_name()
            );
        }
    }

    /// Mute and solo reach only the controls that represent a mixer track.
    #[test]
    fn only_mixer_strip_controls_declare_mute_and_solo() {
        for control in ALL_COMPONENT_CONTROLS {
            let in_strip = matches!(control, ComponentControl::Fader | ComponentControl::Meter);
            for state in [ComponentState::Muted, ComponentState::Soloed] {
                assert_eq!(
                    control.accepts(state),
                    in_strip,
                    "{} and {}",
                    control.canonical_name(),
                    state.canonical_name()
                );
            }
        }
    }

    #[test]
    fn the_composition_family_holds_exactly_ten_compositions() {
        assert_eq!(SHELL_COMPOSITION_COUNT, 10);
        assert_eq!(ALL_SHELL_COMPOSITIONS.len(), SHELL_COMPOSITION_COUNT);
    }

    /// Adding a composition without adding it to [`ALL_SHELL_COMPOSITIONS`]
    /// fails here: the `match` is exhaustive, so the new composition must be
    /// named, and naming it makes the iteration check fail until the array is
    /// updated.
    #[test]
    fn iteration_yields_every_declared_composition() {
        for composition in ALL_SHELL_COMPOSITIONS {
            let named = match composition {
                ShellComposition::ApplicationShell => "ApplicationShell",
                ShellComposition::ContextSwitch => "ContextSwitch",
                ShellComposition::IdentityHeader => "IdentityHeader",
                ShellComposition::Section => "Section",
                ShellComposition::PatchStripRow => "PatchStripRow",
                ShellComposition::PatchStrip => "PatchStrip",
                ShellComposition::MixerStripBank => "MixerStripBank",
                ShellComposition::UtilityInspectorPanel => "UtilityInspectorPanel",
                ShellComposition::CapabilityDetailShell => "CapabilityDetailShell",
                ShellComposition::Footer => "Footer",
            };
            assert_eq!(named, composition.canonical_name());
        }
        let seen: HashSet<_> = ALL_SHELL_COMPOSITIONS.into_iter().collect();
        assert_eq!(
            seen.len(),
            SHELL_COMPOSITION_COUNT,
            "a composition appears twice in ALL_SHELL_COMPOSITIONS"
        );
    }

    /// Every region name the observation emits is filled by a composition.
    ///
    /// The webview page fills these regions from the serialized view model
    /// while the observation keeps its shape; if a region name here stopped
    /// matching one, the observation would report a rectangle nothing
    /// produces.
    #[test]
    fn every_observed_region_is_filled_by_a_composition() {
        let filled: HashSet<&str> = ALL_SHELL_COMPOSITIONS
            .into_iter()
            .filter_map(|composition| composition.region().observation_name())
            .collect();
        for name in OBSERVED_REGION_NAMES {
            assert!(
                filled.contains(name),
                "no composition fills the {name} region"
            );
        }
        assert_eq!(
            filled.len(),
            OBSERVED_REGION_NAMES.len(),
            "a composition fills a region the observation does not report"
        );
    }

    /// The application shell is the only composition that fills no observed
    /// region, and every other composition fills one.
    #[test]
    fn only_the_application_shell_fills_the_whole_frame() {
        for composition in ALL_SHELL_COMPOSITIONS {
            let whole_frame = composition.region() == ShellRegion::WholeFrame;
            assert_eq!(
                whole_frame,
                composition == ShellComposition::ApplicationShell,
                "{} and the whole frame",
                composition.canonical_name()
            );
            assert_eq!(
                composition.region().observation_name().is_none(),
                whole_frame,
                "{} region reporting",
                composition.canonical_name()
            );
        }
    }

    #[test]
    fn the_four_declared_tones_are_visually_distinct() {
        assert_eq!(ALL_HINT_TONES.len(), 4);
        let colors: HashSet<_> = ALL_HINT_TONES.iter().map(|tone| tone.color()).collect();
        assert_eq!(colors.len(), 4, "two tones share a color role");
        let names: HashSet<_> = ALL_HINT_TONES
            .iter()
            .map(|tone| tone.canonical_name())
            .collect();
        assert_eq!(names.len(), 4);
    }

    #[test]
    fn a_hint_reads_as_chord_then_action() {
        assert_eq!(
            ActionHint::new("D-PAD", "NAV", HintTone::Focus).text(),
            "D-PAD NAV"
        );
        assert_eq!(HINT_SEPARATOR, " · ");
    }

    #[test]
    fn every_state_that_carries_a_status_says_something_that_is_not_a_color() {
        for state in ALL_COMPONENT_STATES {
            let Some(mark) = status_mark(state, StatusDetail::None) else {
                continue;
            };
            match mark {
                StatusMark::Text { text, .. } => assert!(
                    !text.is_empty(),
                    "{} carries an empty status mark",
                    state.canonical_name()
                ),
                StatusMark::Selection { fill, mark } => assert_ne!(
                    fill,
                    mark,
                    "{} paints its mark in its fill, leaving it invisible",
                    state.canonical_name()
                ),
            }
        }
    }

    #[test]
    fn rest_focus_and_adjustment_carry_no_status_mark() {
        for state in [
            ComponentState::Resting,
            ComponentState::Focused,
            ComponentState::Adjusting,
        ] {
            assert!(
                status_mark(state, StatusDetail::None).is_none(),
                "{} invented a status mark",
                state.canonical_name()
            );
        }
    }

    #[test]
    fn every_status_bearing_state_marks() {
        for state in [
            ComponentState::Disabled,
            ComponentState::Loading,
            ComponentState::Unavailable,
            ComponentState::Error,
            ComponentState::Muted,
            ComponentState::Soloed,
            ComponentState::Selected,
        ] {
            assert!(
                status_mark(state, StatusDetail::None).is_some(),
                "{} carries no status mark",
                state.canonical_name()
            );
        }
    }

    #[test]
    fn mute_and_solo_carry_the_authored_words_in_the_authored_accents() {
        assert_eq!(
            status_mark(ComponentState::Muted, StatusDetail::None),
            Some(StatusMark::Text {
                text: "M ON",
                color: SemanticColor::AccentWarning
            })
        );
        assert_eq!(
            status_mark(ComponentState::Soloed, StatusDetail::None),
            Some(StatusMark::Text {
                text: "S ON",
                color: SemanticColor::AccentPositive
            })
        );
    }

    #[test]
    fn a_disabled_component_says_it_is_locked() {
        assert_eq!(
            status_mark(ComponentState::Disabled, StatusDetail::None),
            Some(StatusMark::Text {
                text: "Locked",
                color: SemanticColor::TextMuted
            })
        );
    }

    #[test]
    fn an_unavailable_component_says_it_is_unavailable() {
        assert_eq!(
            status_mark(ComponentState::Unavailable, StatusDetail::None),
            Some(StatusMark::Text {
                text: "Unavailable",
                color: SemanticColor::AccentWarning
            })
        );
    }

    #[test]
    fn loading_reports_the_phase_the_caller_supplied() {
        assert_eq!(
            status_mark(
                ComponentState::Loading,
                StatusDetail::Progress(LoadingPhase::Activating)
            ),
            Some(StatusMark::Text {
                text: "Activating",
                color: SemanticColor::AccentAdjust
            })
        );
        assert_eq!(LoadingPhase::Preparing.word(), "Preparing");
        assert_eq!(LoadingPhase::Activating.word(), "Activating");
        assert_eq!(LoadingPhase::Loading.word(), "Loading");
        assert_eq!(LoadingPhase::Validating.word(), "Validating");
    }

    #[test]
    fn loading_without_a_phase_still_says_a_word() {
        assert_eq!(
            status_mark(ComponentState::Loading, StatusDetail::None),
            Some(StatusMark::Text {
                text: "Loading",
                color: SemanticColor::AccentAdjust
            })
        );
    }

    #[test]
    fn error_reports_the_typed_failure_the_caller_supplied() {
        assert_eq!(
            status_mark(
                ComponentState::Error,
                StatusDetail::Failure("PRESET MISSING")
            ),
            Some(StatusMark::Text {
                text: "PRESET MISSING",
                color: SemanticColor::AccentWarning
            })
        );
    }

    #[test]
    fn error_without_typed_text_still_says_a_word() {
        for detail in [StatusDetail::None, StatusDetail::Failure("")] {
            assert_eq!(
                status_mark(ComponentState::Error, detail),
                Some(StatusMark::Text {
                    text: GENERIC_FAILURE_WORD,
                    color: SemanticColor::AccentWarning
                })
            );
        }
    }

    #[test]
    fn selection_is_a_fill_plus_a_mark_and_never_the_fill_alone() {
        assert_eq!(
            status_mark(ComponentState::Selected, StatusDetail::None),
            Some(StatusMark::Selection {
                fill: SemanticColor::BgSelected,
                mark: SemanticColor::TextPrimary
            })
        );
    }

    /// The `declared_word` fallback is unreachable in production. This is what
    /// holds that: every state routed to it declares a word.
    #[test]
    fn every_state_routed_to_the_declared_word_declares_one() {
        for state in [
            ComponentState::Disabled,
            ComponentState::Unavailable,
            ComponentState::Muted,
            ComponentState::Soloed,
        ] {
            assert!(
                matches!(state.appearance().signal, NonColorSignal::Word(word) if !word.is_empty()),
                "{} does not declare a fixed word",
                state.canonical_name()
            );
        }
    }

    #[test]
    fn the_mark_reuses_the_accent_the_state_declared() {
        for state in ALL_COMPONENT_STATES {
            let Some(StatusMark::Text { color, .. }) = status_mark(state, StatusDetail::None)
            else {
                continue;
            };
            assert_eq!(
                color,
                state.appearance().accent,
                "{} re-derived its accent instead of reading the declared one",
                state.canonical_name()
            );
        }
    }

    /// A state is framed exactly when it declares the emphasis keyline width.
    #[test]
    fn a_state_is_framed_exactly_when_it_declares_the_emphasis_width() {
        for state in ALL_COMPONENT_STATES {
            assert_eq!(
                frames(state),
                state.appearance().keyline_px == KEYLINE_EMPHASIS_PX,
                "{} frames disagrees with its declared keyline width",
                state.canonical_name()
            );
        }
    }

    #[test]
    fn resting_is_an_absence_rather_than_a_suppressed_frame() {
        assert!(!frames(ComponentState::Resting));
        assert_eq!(
            ComponentState::Resting.appearance().keyline_px,
            KEYLINE_RESTING_PX
        );
    }

    #[test]
    fn focus_is_legible_without_color() {
        // The cursor is the non-color signal. Without it, focus would be a
        // cyan keyline and nothing else.
        assert!(draws_cursor(ComponentState::Focused));
        assert!(draws_cursor(ComponentState::Adjusting));
    }

    #[test]
    fn no_unfocused_state_draws_the_cursor() {
        for state in ALL_COMPONENT_STATES {
            if state == ComponentState::Focused || state == ComponentState::Adjusting {
                continue;
            }
            assert!(
                !draws_cursor(state),
                "{} draws the focus cursor",
                state.canonical_name()
            );
        }
    }

    #[test]
    fn the_unavailable_mark_is_the_declared_treatment() {
        assert_eq!(UNAVAILABLE_MARK, "--");
    }
}
