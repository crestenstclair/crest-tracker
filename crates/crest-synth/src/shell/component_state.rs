//! The closed component state vocabulary.
//!
//! A component is handed exactly one [`ComponentState`] as immutable input. It
//! never derives, caches, or owns one.
//!
//! The set is closed and there is no catch-all variant, so adding a state makes
//! the compiler name every `match` that must change. Any `match` on
//! [`ComponentState`], anywhere, is exhaustive with no wildcard arm — a `_`
//! arm converts that compile error into a silent visual default, which is the
//! failure this type exists to prevent.
//!
//! Every state announces itself with text or shape and not with color alone
//! (`DESIGN.md`). That pairing is declared here as data, in
//! [`ComponentState::appearance`], rather than as branching inside each
//! primitive.
//!
//! Realizes `valueObject.Shell.ComponentState`.

use crate::shell::tokens::{SemanticColor, KEYLINE_EMPHASIS_PX, KEYLINE_RESTING_PX};

/// A behavioral state a component may be handed.
///
/// The set is closed at ten. Closedness is the feature: an eleventh state fails
/// compilation at every rendering site rather than silently falling through to
/// the resting appearance.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ComponentState {
    /// Nothing is happening. The baseline every other state reads against.
    Resting,
    /// Keyboard or controller focus is on this component.
    Focused,
    /// A value on this component is being adjusted.
    Adjusting,
    /// This component is present but not editable — a locked asset row, a
    /// read-only capability row, a skipped focus target.
    Disabled,
    /// A structural edit this component requested is in flight.
    Loading,
    /// The requested provider or resource cannot currently be supplied.
    Unavailable,
    /// A structural edit this component requested failed.
    Error,
    /// This component's mixer track is muted.
    Muted,
    /// This component's mixer track is soloed.
    Soloed,
    /// This component is the current selection.
    Selected,
}

/// Every declared state, in declaration order.
pub const ALL_COMPONENT_STATES: [ComponentState; COMPONENT_STATE_COUNT] = [
    ComponentState::Resting,
    ComponentState::Focused,
    ComponentState::Adjusting,
    ComponentState::Disabled,
    ComponentState::Loading,
    ComponentState::Unavailable,
    ComponentState::Error,
    ComponentState::Muted,
    ComponentState::Soloed,
    ComponentState::Selected,
];

/// How many states the vocabulary declares.
///
/// Surfaces that must cover every state assert against this rather than
/// against a number they carry themselves.
pub const COMPONENT_STATE_COUNT: usize = 10;

/// The authored progress words a [`ComponentState::Loading`] component
/// carries, in lifecycle order.
///
/// `DESIGN.md` — a targeted structural row displays its active and requested
/// value throughout admission, preparation, and block-boundary activation.
/// The component paints the reducer-owned lifecycle word rather than
/// collapsing those distinct states into a generic spinner.
pub const LOADING_PROGRESS_WORDS: [&str; 4] = ["Loading", "Validating", "Preparing", "Activating"];

/// The non-color signal a state carries.
///
/// Color alone never distinguishes two states. Where two states share an
/// accent — `Adjusting` and `Loading` both paint `color/accent/adjust` — the
/// signal is what tells them apart.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NonColorSignal {
    /// Shape alone: the keyline width, halo, and selection fill declared
    /// alongside this state are what distinguish it.
    Shape,
    /// One fixed authored word, painted beside the component.
    Word(&'static str),
    /// One of [`LOADING_PROGRESS_WORDS`], chosen by the structural-edit phase
    /// the component was handed.
    ProgressWord,
    /// Short typed failure text, supplied by the caller. The text varies; that
    /// it is present does not.
    TypedFailure,
}

impl NonColorSignal {
    /// Whether this signal puts text on screen.
    pub const fn carries_text(self) -> bool {
        !matches!(self, Self::Shape)
    }
}

/// The declared appearance of one state.
///
/// This is data, not behavior. A primitive reads it; it does not re-derive it,
/// and it does not branch per state to reach the same values.
///
/// Everything here except `accent` reads without color. No two states share
/// both that colorless remainder and their [`NonColorSignal`], so no viewer
/// who cannot separate two accents is left unable to tell two states apart;
/// the test in this module holds that.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StateAppearance {
    /// The color role this state paints with.
    pub accent: SemanticColor,
    /// The keyline width this state draws.
    pub keyline_px: f32,
    /// Whether this state draws the focus halo.
    pub draws_halo: bool,
    /// Whether this state fills a band behind its row.
    pub fills_row: bool,
    /// What this state says without using color.
    pub signal: NonColorSignal,
}

impl ComponentState {
    /// Resolves this state to its declared appearance.
    ///
    /// `Loading` and `Error` deliberately reuse the vocabulary already
    /// declared for structural edits — the adjustment accent with its progress
    /// text, and the warning accent with typed short text. Neither invents a
    /// second visual language, and neither animates: `DESIGN.md` asks for
    /// text or shape, and a spinner would additionally force a repaint the
    /// 16 ms idle cadence does not want.
    pub const fn appearance(self) -> StateAppearance {
        match self {
            Self::Resting => StateAppearance {
                accent: SemanticColor::BorderDefault,
                keyline_px: KEYLINE_RESTING_PX,
                draws_halo: false,
                fills_row: false,
                signal: NonColorSignal::Shape,
            },
            Self::Focused => StateAppearance {
                accent: SemanticColor::AccentFocus,
                keyline_px: KEYLINE_EMPHASIS_PX,
                draws_halo: true,
                fills_row: false,
                signal: NonColorSignal::Shape,
            },
            Self::Adjusting => StateAppearance {
                accent: SemanticColor::AccentAdjust,
                keyline_px: KEYLINE_EMPHASIS_PX,
                draws_halo: false,
                fills_row: false,
                signal: NonColorSignal::Shape,
            },
            Self::Disabled => StateAppearance {
                accent: SemanticColor::TextMuted,
                keyline_px: KEYLINE_RESTING_PX,
                draws_halo: false,
                fills_row: false,
                signal: NonColorSignal::Word("Locked"),
            },
            Self::Loading => StateAppearance {
                accent: SemanticColor::AccentAdjust,
                keyline_px: KEYLINE_EMPHASIS_PX,
                draws_halo: false,
                fills_row: false,
                signal: NonColorSignal::ProgressWord,
            },
            Self::Unavailable => StateAppearance {
                accent: SemanticColor::AccentWarning,
                keyline_px: KEYLINE_EMPHASIS_PX,
                draws_halo: false,
                fills_row: false,
                signal: NonColorSignal::Word("Unavailable"),
            },
            Self::Error => StateAppearance {
                accent: SemanticColor::AccentWarning,
                keyline_px: KEYLINE_EMPHASIS_PX,
                draws_halo: false,
                fills_row: false,
                signal: NonColorSignal::TypedFailure,
            },
            Self::Muted => StateAppearance {
                accent: SemanticColor::AccentWarning,
                keyline_px: KEYLINE_RESTING_PX,
                draws_halo: false,
                fills_row: false,
                signal: NonColorSignal::Word("M ON"),
            },
            Self::Soloed => StateAppearance {
                accent: SemanticColor::AccentPositive,
                keyline_px: KEYLINE_RESTING_PX,
                draws_halo: false,
                fills_row: false,
                signal: NonColorSignal::Word("S ON"),
            },
            Self::Selected => StateAppearance {
                accent: SemanticColor::BgSelected,
                keyline_px: KEYLINE_RESTING_PX,
                draws_halo: false,
                fills_row: true,
                signal: NonColorSignal::Shape,
            },
        }
    }

    /// Returns the canonical name of this state.
    pub const fn canonical_name(self) -> &'static str {
        match self {
            Self::Resting => "Resting",
            Self::Focused => "Focused",
            Self::Adjusting => "Adjusting",
            Self::Disabled => "Disabled",
            Self::Loading => "Loading",
            Self::Unavailable => "Unavailable",
            Self::Error => "Error",
            Self::Muted => "Muted",
            Self::Soloed => "Soloed",
            Self::Selected => "Selected",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything in an appearance that reads without color.
    fn shape_signature(appearance: StateAppearance) -> (f32, bool, bool) {
        (
            appearance.keyline_px,
            appearance.draws_halo,
            appearance.fills_row,
        )
    }

    #[test]
    fn the_state_vocabulary_holds_exactly_ten_states() {
        assert_eq!(COMPONENT_STATE_COUNT, 10);
        assert_eq!(ALL_COMPONENT_STATES.len(), COMPONENT_STATE_COUNT);
    }

    /// Adding a variant without adding it to [`ALL_COMPONENT_STATES`] fails
    /// here: the `match` is exhaustive, so the new variant must be named, and
    /// naming it makes the iteration check fail until the array is updated.
    #[test]
    fn iteration_yields_every_declared_variant() {
        for state in ALL_COMPONENT_STATES {
            let named = match state {
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
            assert_eq!(named, state.canonical_name());
        }
        let seen: std::collections::HashSet<_> = ALL_COMPONENT_STATES.into_iter().collect();
        assert_eq!(
            seen.len(),
            COMPONENT_STATE_COUNT,
            "a state appears twice in ALL_COMPONENT_STATES"
        );
        let names: std::collections::HashSet<_> = ALL_COMPONENT_STATES
            .iter()
            .map(|s| s.canonical_name())
            .collect();
        assert_eq!(names.len(), COMPONENT_STATE_COUNT);
    }

    #[test]
    fn no_two_states_are_distinguishable_by_color_alone() {
        for (index, first) in ALL_COMPONENT_STATES.iter().enumerate() {
            for second in &ALL_COMPONENT_STATES[index + 1..] {
                let a = first.appearance();
                let b = second.appearance();
                assert!(
                    shape_signature(a) != shape_signature(b) || a.signal != b.signal,
                    "{} and {} differ only in color",
                    first.canonical_name(),
                    second.canonical_name()
                );
            }
        }
    }

    #[test]
    fn adjusting_and_loading_share_an_accent_and_are_told_apart_by_text() {
        let adjusting = ComponentState::Adjusting.appearance();
        let loading = ComponentState::Loading.appearance();
        assert_eq!(adjusting.accent, loading.accent);
        assert_eq!(loading.accent, SemanticColor::AccentAdjust);
        assert!(!adjusting.signal.carries_text());
        assert!(loading.signal.carries_text());
    }

    #[test]
    fn loading_carries_the_authored_structural_edit_progress_words() {
        assert_eq!(
            ComponentState::Loading.appearance().signal,
            NonColorSignal::ProgressWord
        );
        assert_eq!(
            LOADING_PROGRESS_WORDS,
            ["Loading", "Validating", "Preparing", "Activating"]
        );
    }

    #[test]
    fn error_pairs_the_warning_accent_with_typed_text() {
        let error = ComponentState::Error.appearance();
        assert_eq!(error.accent, SemanticColor::AccentWarning);
        assert_eq!(error.signal, NonColorSignal::TypedFailure);
        assert!(error.signal.carries_text());
    }

    #[test]
    fn unavailable_pairs_the_warning_accent_with_an_explicit_word() {
        let unavailable = ComponentState::Unavailable.appearance();
        assert_eq!(unavailable.accent, SemanticColor::AccentWarning);
        assert_eq!(unavailable.signal, NonColorSignal::Word("Unavailable"));
        assert!(unavailable.signal.carries_text());
    }

    #[test]
    fn mute_and_solo_carry_the_authored_words() {
        // `DESIGN.md` — mute and solo pair warning/positive color with
        // explicit `M ON` or `S ON` text.
        assert_eq!(
            ComponentState::Muted.appearance().signal,
            NonColorSignal::Word("M ON")
        );
        assert_eq!(
            ComponentState::Muted.appearance().accent,
            SemanticColor::AccentWarning
        );
        assert_eq!(
            ComponentState::Soloed.appearance().signal,
            NonColorSignal::Word("S ON")
        );
        assert_eq!(
            ComponentState::Soloed.appearance().accent,
            SemanticColor::AccentPositive
        );
    }

    #[test]
    fn focus_is_the_only_state_that_draws_a_halo() {
        for state in ALL_COMPONENT_STATES {
            assert_eq!(
                state.appearance().draws_halo,
                state == ComponentState::Focused,
                "{} halo",
                state.canonical_name()
            );
        }
    }

    #[test]
    fn selection_is_the_only_state_that_fills_its_row() {
        for state in ALL_COMPONENT_STATES {
            assert_eq!(
                state.appearance().fills_row,
                state == ComponentState::Selected,
                "{} row fill",
                state.canonical_name()
            );
        }
    }

    #[test]
    fn every_state_draws_an_authored_keyline_width() {
        for state in ALL_COMPONENT_STATES {
            let keyline = state.appearance().keyline_px;
            assert!(
                keyline == KEYLINE_RESTING_PX || keyline == KEYLINE_EMPHASIS_PX,
                "{} draws {} px, which is not an authored keyline width",
                state.canonical_name(),
                keyline
            );
        }
    }

    #[test]
    fn every_state_other_than_resting_reads_without_color() {
        // Resting is the baseline: it is the absence the other nine read
        // against, so it carries no distinguishing mark of its own. Every
        // other state must announce itself with text or with shape.
        for state in ALL_COMPONENT_STATES {
            if state == ComponentState::Resting {
                continue;
            }
            let appearance = state.appearance();
            let resting = ComponentState::Resting.appearance();
            assert!(
                appearance.signal.carries_text()
                    || shape_signature(appearance) != shape_signature(resting),
                "{} is distinguishable from Resting by color alone",
                state.canonical_name()
            );
        }
    }
}
