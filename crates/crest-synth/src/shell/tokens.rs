//! The authored semantic visual vocabulary.
//!
//! This module is the single source of every color, type style, spacing step,
//! and geometry value the interface paints with. Raw values — RGB channels,
//! point sizes, pixel gaps — are private to this file. Everything outside
//! reaches them through a named token, so changing an authored value here
//! changes every appearance of it and no second definition can survive.
//!
//! Names are the canonical names published by the design file. Where
//! `DESIGN.md` uses a shorter name for the same value, the design file's name
//! wins in code and the mapping is recorded on the variant.
//!
//! Realizes `valueObject.Shell.SemanticVisualToken`.

/// One authored opaque sRGB color, as the vocabulary declares it.
///
/// The vocabulary owns its own value type: the webview shell consumes colors
/// as generated CSS custom properties (`webview-page/tokens.css`), so no
/// renderer's color type is imported here. Alpha does not exist on this type
/// — every authored role is opaque, and the one authored opacity (the focus
/// halo's) is its own token, [`FOCUS_HALO_OPACITY`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AuthoredRgb {
    red: u8,
    green: u8,
    blue: u8,
}

impl AuthoredRgb {
    /// Builds an authored color from its channels. Private: the only place a
    /// raw channel may be spelled is this file's vocabulary.
    const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    /// The red channel.
    pub const fn r(self) -> u8 {
        self.red
    }

    /// The green channel.
    pub const fn g(self) -> u8 {
        self.green
    }

    /// The blue channel.
    pub const fn b(self) -> u8 {
        self.blue
    }
}

/// A named background, border, text, or accent role.
///
/// The set is closed. A color the interface needs and this enum does not
/// declare is added here and to `DESIGN.md`, never introduced as a local
/// literal.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SemanticColor {
    /// `color/bg/canvas` — the application background.
    BgCanvas,
    /// `color/bg/surface` — primary surfaces.
    BgSurface,
    /// `color/bg/panel` — grouped regions.
    BgPanel,
    /// `color/bg/elevated` — controls and modals. Declared by `DESIGN.md`; the
    /// design file does not publish it as a variable.
    BgElevated,
    /// `color/bg/selected` — the selected-row background. Published by the
    /// design file and retained in the as-built vocabulary in `DESIGN.md`.
    BgSelected,
    /// `color/border/default` — resting hairlines.
    BorderDefault,
    /// `color/border/strong` — structural separation. Declared by `DESIGN.md`.
    BorderStrong,
    /// `color/text/primary` — primary content.
    TextPrimary,
    /// `color/text/secondary` — secondary content.
    TextSecondary,
    /// `color/text/muted` — labels and inactive state.
    TextMuted,
    /// `color/accent/focus` — focus, and nothing else.
    AccentFocus,
    /// `color/accent/adjust` — active adjustment and labeled sample identity.
    AccentAdjust,
    /// `color/accent/positive` — ready, solo, positive.
    AccentPositive,
    /// `color/accent/warning` — error, mute, destructive.
    AccentWarning,
    /// `color/accent/instrument/plates` — instrument identity. `DESIGN.md`
    /// names this `instrument`; both denote the same value.
    AccentInstrument,
    /// `color/accent/patch` — patch identity. Declared by `DESIGN.md`.
    AccentPatch,
    /// `color/accent/chorus` — chorus identity. Declared by `DESIGN.md`.
    AccentChorus,
}

/// Every declared color, in declaration order.
///
/// The vocabulary is the union of the design file's thirteen published
/// variables and the four additional semantic roles recorded in `DESIGN.md`.
/// Neither source is trimmed to match the other.
pub const ALL_COLORS: [SemanticColor; 17] = [
    SemanticColor::BgCanvas,
    SemanticColor::BgSurface,
    SemanticColor::BgPanel,
    SemanticColor::BgElevated,
    SemanticColor::BgSelected,
    SemanticColor::BorderDefault,
    SemanticColor::BorderStrong,
    SemanticColor::TextPrimary,
    SemanticColor::TextSecondary,
    SemanticColor::TextMuted,
    SemanticColor::AccentFocus,
    SemanticColor::AccentAdjust,
    SemanticColor::AccentPositive,
    SemanticColor::AccentWarning,
    SemanticColor::AccentInstrument,
    SemanticColor::AccentPatch,
    SemanticColor::AccentChorus,
];

impl SemanticColor {
    /// Resolves this role to the authored color.
    ///
    /// `BgSelected` and `BorderDefault` deliberately share `#2a3745`. They mean
    /// different things and are kept as two named tokens so they can diverge
    /// without a search-and-replace.
    pub const fn resolve(self) -> AuthoredRgb {
        let (r, g, b) = match self {
            Self::BgCanvas => (0x0c, 0x10, 0x15),
            Self::BgSurface => (0x12, 0x18, 0x21),
            Self::BgPanel => (0x17, 0x20, 0x2a),
            Self::BgElevated => (0x1d, 0x27, 0x33),
            Self::BgSelected => (0x2a, 0x37, 0x45),
            Self::BorderDefault => (0x2a, 0x37, 0x45),
            Self::BorderStrong => (0x41, 0x51, 0x66),
            Self::TextPrimary => (0xf2, 0xf6, 0xf8),
            Self::TextSecondary => (0xb8, 0xc4, 0xd1),
            Self::TextMuted => (0x6f, 0x80, 0x95),
            Self::AccentFocus => (0x65, 0xe5, 0xff),
            Self::AccentAdjust => (0xff, 0xb4, 0x54),
            Self::AccentPositive => (0x58, 0xe8, 0x87),
            Self::AccentWarning => (0xff, 0x68, 0x68),
            Self::AccentInstrument => (0xb8, 0x94, 0xff),
            Self::AccentPatch => (0xff, 0x6f, 0xbe),
            Self::AccentChorus => (0xf6, 0xf1, 0x78),
        };
        AuthoredRgb::new(r, g, b)
    }

    /// Returns the canonical authored name, as published by the design file.
    pub const fn canonical_name(self) -> &'static str {
        match self {
            Self::BgCanvas => "color/bg/canvas",
            Self::BgSurface => "color/bg/surface",
            Self::BgPanel => "color/bg/panel",
            Self::BgElevated => "color/bg/elevated",
            Self::BgSelected => "color/bg/selected",
            Self::BorderDefault => "color/border/default",
            Self::BorderStrong => "color/border/strong",
            Self::TextPrimary => "color/text/primary",
            Self::TextSecondary => "color/text/secondary",
            Self::TextMuted => "color/text/muted",
            Self::AccentFocus => "color/accent/focus",
            Self::AccentAdjust => "color/accent/adjust",
            Self::AccentPositive => "color/accent/positive",
            Self::AccentWarning => "color/accent/warning",
            Self::AccentInstrument => "color/accent/instrument/plates",
            Self::AccentPatch => "color/accent/patch",
            Self::AccentChorus => "color/accent/chorus",
        }
    }
}

/// An authored typeface weight.
///
/// The interface ships four static faces rather than the upstream variable
/// font: the projection page binds one `@font-face` per numeric weight, and a
/// static face per weight keeps what is served identical to what was vendored
/// and hashed.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FontWeight {
    Regular,
    Medium,
    SemiBold,
    Bold,
}

/// Every authored weight, lightest first.
pub const ALL_WEIGHTS: [FontWeight; 4] = [
    FontWeight::Regular,
    FontWeight::Medium,
    FontWeight::SemiBold,
    FontWeight::Bold,
];

impl FontWeight {
    /// Returns the numeric weight the design file declares.
    pub const fn numeric(self) -> u16 {
        match self {
            Self::Regular => 400,
            Self::Medium => 500,
            Self::SemiBold => 600,
            Self::Bold => 700,
        }
    }
}

/// One of the eight authored text styles.
///
/// The set is closed. Text painted by the interface resolves through one of
/// these; an ad-hoc point size is exactly what this vocabulary removes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TypeStyle {
    /// `Display/Screen`
    DisplayScreen,
    /// `Heading/Section`
    HeadingSection,
    /// `Heading/Panel`
    HeadingPanel,
    /// `Body/Default`
    BodyDefault,
    /// `Body/Compact`
    BodyCompact,
    /// `Label/Control`
    LabelControl,
    /// `Code/Value`
    CodeValue,
    /// `Instruction/Hint`
    InstructionHint,
}

/// Every declared type style, in declaration order.
pub const ALL_TYPE_STYLES: [TypeStyle; 8] = [
    TypeStyle::DisplayScreen,
    TypeStyle::HeadingSection,
    TypeStyle::HeadingPanel,
    TypeStyle::BodyDefault,
    TypeStyle::BodyCompact,
    TypeStyle::LabelControl,
    TypeStyle::CodeValue,
    TypeStyle::InstructionHint,
];

/// The authored metrics of one text style.
///
/// `tracking_px` is applied faithfully — it is emitted as the style's
/// `--type-*-tracking` custom property and reaches the rendered glyph run as
/// CSS `letter-spacing`, rather than being declared and dropped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypeStyleMetrics {
    /// Glyph size in pixels.
    pub size_px: f32,
    /// Line box height in pixels.
    pub line_height_px: f32,
    /// The weight this style is set in.
    pub weight: FontWeight,
    /// Additional letter spacing in pixels.
    pub tracking_px: f32,
}

impl TypeStyle {
    /// Resolves this style to its authored metrics.
    pub const fn metrics(self) -> TypeStyleMetrics {
        let (size_px, line_height_px, weight, tracking_px) = match self {
            Self::DisplayScreen => (32.0, 40.0, FontWeight::SemiBold, 0.4),
            Self::HeadingSection => (18.0, 24.0, FontWeight::SemiBold, 1.4),
            Self::HeadingPanel => (14.0, 20.0, FontWeight::Bold, 1.2),
            Self::BodyDefault => (15.0, 22.0, FontWeight::Regular, 0.0),
            Self::BodyCompact => (13.0, 18.0, FontWeight::Regular, 0.0),
            Self::LabelControl => (12.0, 16.0, FontWeight::Medium, 0.8),
            Self::CodeValue => (14.0, 20.0, FontWeight::SemiBold, 0.2),
            Self::InstructionHint => (11.0, 16.0, FontWeight::Medium, 0.8),
        };
        TypeStyleMetrics {
            size_px,
            line_height_px,
            weight,
            tracking_px,
        }
    }

    /// Returns the canonical authored name.
    pub const fn canonical_name(self) -> &'static str {
        match self {
            Self::DisplayScreen => "Display/Screen",
            Self::HeadingSection => "Heading/Section",
            Self::HeadingPanel => "Heading/Panel",
            Self::BodyDefault => "Body/Default",
            Self::BodyCompact => "Body/Compact",
            Self::LabelControl => "Label/Control",
            Self::CodeValue => "Code/Value",
            Self::InstructionHint => "Instruction/Hint",
        }
    }
}

/// One of the six authored spacing steps.
///
/// The set is closed so that an arbitrary pixel gap has no place to hide.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SpacingStep {
    /// `space/4`
    S4,
    /// `space/8`
    S8,
    /// `space/12`
    S12,
    /// `space/16`
    S16,
    /// `space/24`
    S24,
    /// `space/32`
    S32,
}

/// Every declared spacing step, smallest first.
pub const ALL_SPACING_STEPS: [SpacingStep; 6] = [
    SpacingStep::S4,
    SpacingStep::S8,
    SpacingStep::S12,
    SpacingStep::S16,
    SpacingStep::S24,
    SpacingStep::S32,
];

impl SpacingStep {
    /// Resolves this step to its authored pixel value.
    pub const fn resolve(self) -> f32 {
        match self {
            Self::S4 => 4.0,
            Self::S8 => 8.0,
            Self::S12 => 12.0,
            Self::S16 => 16.0,
            Self::S24 => 24.0,
            Self::S32 => 32.0,
        }
    }

    /// Returns the canonical authored name.
    pub const fn canonical_name(self) -> &'static str {
        match self {
            Self::S4 => "space/4",
            Self::S8 => "space/8",
            Self::S12 => "space/12",
            Self::S16 => "space/16",
            Self::S24 => "space/24",
            Self::S32 => "space/32",
        }
    }
}

/// A named corner radius.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Radius {
    /// Square corners.
    None,
    /// Controls and inputs.
    Small,
    /// Panels and modals.
    Large,
}

/// Every declared radius, in declaration order.
pub const ALL_RADII: [Radius; 3] = [Radius::None, Radius::Small, Radius::Large];

impl Radius {
    /// Resolves this radius to its authored pixel value.
    pub const fn resolve(self) -> f32 {
        match self {
            Self::None => 0.0,
            Self::Small => 4.0,
            Self::Large => 8.0,
        }
    }
}

/// The authored width of a resting hairline.
pub const KEYLINE_RESTING_PX: f32 = 1.0;

/// The authored width of a focus or adjustment keyline.
pub const KEYLINE_EMPHASIS_PX: f32 = 3.0;

/// The authored minimum size of any interactive target.
pub const MIN_INTERACTIVE_TARGET_PX: f32 = 48.0;

/// The authored blur radius of the focus halo.
pub const FOCUS_HALO_RADIUS_PX: f32 = 8.0;

/// The authored spread of the focus halo.
pub const FOCUS_HALO_SPREAD_PX: f32 = 1.0;

/// The authored opacity of the focus halo.
///
/// The design file carries `0x47` alpha — 71/255 = 0.2784. `DESIGN.md` rounds
/// to 0.28 and that rounded value is what ships, so the two documents read the
/// same number.
pub const FOCUS_HALO_OPACITY: f32 = 0.28;

// The mixer fader specimen geometry, measured from the authored design
// (`DESIGN.md` § visual vocabulary; the design file's mixer fader specimen).
// Declared here so the projection page resolves them as generated custom
// properties rather than carrying hand-copied pixel literals.

/// The authored width of the fader track.
pub const FADER_TRACK_WIDTH_PX: f32 = 14.0;

/// The authored width of the fader's level fill inside the track.
pub const FADER_FILL_WIDTH_PX: f32 = 8.0;

/// The authored bottom shoulder the fader fill rises from.
pub const FADER_SHOULDER_PX: f32 = 3.0;

/// The authored width of the fader cap.
pub const FADER_CAP_WIDTH_PX: f32 = 34.0;

/// The authored height of the fader cap.
pub const FADER_CAP_HEIGHT_PX: f32 = 6.0;

/// The authored corner rounding of the fader fill and cap.
pub const FADER_ROUNDING_PX: f32 = 2.0;

#[cfg(test)]
mod tests {
    use super::*;

    /// Expected values are spelled out here rather than derived from the
    /// vocabulary. A test that compares the vocabulary to itself proves
    /// nothing.
    #[test]
    fn every_color_resolves_to_its_authored_value() {
        assert_eq!(
            SemanticColor::BgCanvas.resolve(),
            AuthoredRgb::new(0x0c, 0x10, 0x15)
        );
        assert_eq!(
            SemanticColor::BgSurface.resolve(),
            AuthoredRgb::new(0x12, 0x18, 0x21)
        );
        assert_eq!(
            SemanticColor::BgPanel.resolve(),
            AuthoredRgb::new(0x17, 0x20, 0x2a)
        );
        assert_eq!(
            SemanticColor::BgElevated.resolve(),
            AuthoredRgb::new(0x1d, 0x27, 0x33)
        );
        assert_eq!(
            SemanticColor::BgSelected.resolve(),
            AuthoredRgb::new(0x2a, 0x37, 0x45)
        );
        assert_eq!(
            SemanticColor::BorderDefault.resolve(),
            AuthoredRgb::new(0x2a, 0x37, 0x45)
        );
        assert_eq!(
            SemanticColor::BorderStrong.resolve(),
            AuthoredRgb::new(0x41, 0x51, 0x66)
        );
        assert_eq!(
            SemanticColor::TextPrimary.resolve(),
            AuthoredRgb::new(0xf2, 0xf6, 0xf8)
        );
        assert_eq!(
            SemanticColor::TextSecondary.resolve(),
            AuthoredRgb::new(0xb8, 0xc4, 0xd1)
        );
        assert_eq!(
            SemanticColor::TextMuted.resolve(),
            AuthoredRgb::new(0x6f, 0x80, 0x95)
        );
        assert_eq!(
            SemanticColor::AccentFocus.resolve(),
            AuthoredRgb::new(0x65, 0xe5, 0xff)
        );
        assert_eq!(
            SemanticColor::AccentAdjust.resolve(),
            AuthoredRgb::new(0xff, 0xb4, 0x54)
        );
        assert_eq!(
            SemanticColor::AccentPositive.resolve(),
            AuthoredRgb::new(0x58, 0xe8, 0x87)
        );
        assert_eq!(
            SemanticColor::AccentWarning.resolve(),
            AuthoredRgb::new(0xff, 0x68, 0x68)
        );
        assert_eq!(
            SemanticColor::AccentInstrument.resolve(),
            AuthoredRgb::new(0xb8, 0x94, 0xff)
        );
        assert_eq!(
            SemanticColor::AccentPatch.resolve(),
            AuthoredRgb::new(0xff, 0x6f, 0xbe)
        );
        assert_eq!(
            SemanticColor::AccentChorus.resolve(),
            AuthoredRgb::new(0xf6, 0xf1, 0x78)
        );
    }

    #[test]
    fn focus_accent_is_cyan_not_the_pre_mission_green() {
        // The adapter painted `rgb(110, 205, 174)` before this mission. That
        // value must not survive anywhere in the vocabulary.
        let green = AuthoredRgb::new(110, 205, 174);
        for color in ALL_COLORS {
            assert_ne!(
                color.resolve(),
                green,
                "{} still paints the old green",
                color.canonical_name()
            );
        }
        assert_eq!(
            SemanticColor::AccentFocus.resolve(),
            AuthoredRgb::new(0x65, 0xe5, 0xff)
        );
    }

    #[test]
    fn the_color_vocabulary_holds_exactly_seventeen_unique_roles() {
        assert_eq!(ALL_COLORS.len(), 17);
        let mut seen = std::collections::HashSet::new();
        for color in ALL_COLORS {
            assert!(
                seen.insert(color),
                "{} appears twice",
                color.canonical_name()
            );
        }
        let names: std::collections::HashSet<_> =
            ALL_COLORS.iter().map(|c| c.canonical_name()).collect();
        assert_eq!(names.len(), 17, "two roles share a canonical name");
    }

    #[test]
    fn selected_background_and_default_border_share_a_value_but_stay_distinct() {
        assert_eq!(
            SemanticColor::BgSelected.resolve(),
            SemanticColor::BorderDefault.resolve()
        );
        assert_ne!(SemanticColor::BgSelected, SemanticColor::BorderDefault);
    }

    #[test]
    fn every_type_style_resolves_to_its_authored_metrics() {
        let expected: [(TypeStyle, f32, f32, FontWeight, f32); 8] = [
            (
                TypeStyle::DisplayScreen,
                32.0,
                40.0,
                FontWeight::SemiBold,
                0.4,
            ),
            (
                TypeStyle::HeadingSection,
                18.0,
                24.0,
                FontWeight::SemiBold,
                1.4,
            ),
            (TypeStyle::HeadingPanel, 14.0, 20.0, FontWeight::Bold, 1.2),
            (TypeStyle::BodyDefault, 15.0, 22.0, FontWeight::Regular, 0.0),
            (TypeStyle::BodyCompact, 13.0, 18.0, FontWeight::Regular, 0.0),
            (TypeStyle::LabelControl, 12.0, 16.0, FontWeight::Medium, 0.8),
            (TypeStyle::CodeValue, 14.0, 20.0, FontWeight::SemiBold, 0.2),
            (
                TypeStyle::InstructionHint,
                11.0,
                16.0,
                FontWeight::Medium,
                0.8,
            ),
        ];
        for (style, size, line, weight, tracking) in expected {
            let metrics = style.metrics();
            assert_eq!(metrics.size_px, size, "{} size", style.canonical_name());
            assert_eq!(
                metrics.line_height_px,
                line,
                "{} line height",
                style.canonical_name()
            );
            assert_eq!(metrics.weight, weight, "{} weight", style.canonical_name());
            assert_eq!(
                metrics.tracking_px,
                tracking,
                "{} tracking",
                style.canonical_name()
            );
        }
    }

    #[test]
    fn the_type_vocabulary_holds_exactly_eight_unique_styles() {
        assert_eq!(ALL_TYPE_STYLES.len(), 8);
        let names: std::collections::HashSet<_> =
            ALL_TYPE_STYLES.iter().map(|s| s.canonical_name()).collect();
        assert_eq!(names.len(), 8);
    }

    #[test]
    fn authored_weights_carry_their_numeric_values() {
        assert_eq!(FontWeight::Regular.numeric(), 400);
        assert_eq!(FontWeight::Medium.numeric(), 500);
        assert_eq!(FontWeight::SemiBold.numeric(), 600);
        assert_eq!(FontWeight::Bold.numeric(), 700);
        assert_eq!(ALL_WEIGHTS.len(), 4);
    }

    #[test]
    fn every_spacing_step_resolves_to_its_authored_value() {
        assert_eq!(SpacingStep::S4.resolve(), 4.0);
        assert_eq!(SpacingStep::S8.resolve(), 8.0);
        assert_eq!(SpacingStep::S12.resolve(), 12.0);
        assert_eq!(SpacingStep::S16.resolve(), 16.0);
        assert_eq!(SpacingStep::S24.resolve(), 24.0);
        assert_eq!(SpacingStep::S32.resolve(), 32.0);
        assert_eq!(ALL_SPACING_STEPS.len(), 6);
    }

    #[test]
    fn every_geometry_value_equals_its_authored_number() {
        assert_eq!(Radius::None.resolve(), 0.0);
        assert_eq!(Radius::Small.resolve(), 4.0);
        assert_eq!(Radius::Large.resolve(), 8.0);
        assert_eq!(ALL_RADII.len(), 3);
        assert_eq!(KEYLINE_RESTING_PX, 1.0);
        assert_eq!(KEYLINE_EMPHASIS_PX, 3.0);
        assert_eq!(MIN_INTERACTIVE_TARGET_PX, 48.0);
        assert_eq!(FOCUS_HALO_RADIUS_PX, 8.0);
        assert_eq!(FOCUS_HALO_SPREAD_PX, 1.0);
        assert_eq!(FOCUS_HALO_OPACITY, 0.28);
    }

    #[test]
    fn the_fader_specimen_carries_its_authored_geometry() {
        assert_eq!(FADER_TRACK_WIDTH_PX, 14.0);
        assert_eq!(FADER_FILL_WIDTH_PX, 8.0);
        assert_eq!(FADER_SHOULDER_PX, 3.0);
        assert_eq!(FADER_CAP_WIDTH_PX, 34.0);
        assert_eq!(FADER_CAP_HEIGHT_PX, 6.0);
        assert_eq!(FADER_ROUNDING_PX, 2.0);
    }
}
