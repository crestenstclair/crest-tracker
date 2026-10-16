/// A platform-independent key understood at the application window boundary.
///
/// `Digit3` through `Digit9`, `Digit0`, and the two bracket keys are normalized
/// here because the window sees them, not because anything at this boundary
/// binds them. What a normalized key means is decided downstream: the
/// translator binds `Digit1` and `Digit2` to the two top-level contexts and
/// nothing else, and a scene that pages a gallery binds its own digits and
/// brackets locally without those ever becoming a semantic action.
///
/// The two bracket keys are here for the same reason the digits are. A gallery
/// that declares more pages than there are digits needs a way to reach the
/// remainder, and stepping is that way — but stepping is a *scene's* gesture,
/// so the window normalizes the key and stops there.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WindowKey {
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    Digit0,
    /// `[` — bound to no application meaning.
    BracketLeft,
    /// `]` — bound to no application meaning.
    BracketRight,
    /// Step to the previous installed Patch.
    Q,
    /// Step to the next installed Patch.
    E,
    W,
    S,
    A,
    D,
    K,
    Shift,
    Return,
    Space,
    T,
    Other,
}

/// Every normalized key the window boundary accepts, in declaration order.
///
/// The descriptor below carries a key-down and a key-up for each of these plus
/// the single focus-loss value, so this list and
/// [`WINDOW_INPUT_SURFACE_DESCRIPTOR_LEN`] move together.
pub const ALL_WINDOW_KEYS: [WindowKey; 24] = [
    WindowKey::Digit1,
    WindowKey::Digit2,
    WindowKey::Digit3,
    WindowKey::Digit4,
    WindowKey::Digit5,
    WindowKey::Digit6,
    WindowKey::Digit7,
    WindowKey::Digit8,
    WindowKey::Digit9,
    WindowKey::Digit0,
    WindowKey::BracketLeft,
    WindowKey::BracketRight,
    WindowKey::Q,
    WindowKey::E,
    WindowKey::W,
    WindowKey::S,
    WindowKey::A,
    WindowKey::D,
    WindowKey::K,
    WindowKey::Shift,
    WindowKey::Return,
    WindowKey::Space,
    WindowKey::T,
    WindowKey::Other,
];

/// How many unique normalized inputs the window boundary accepts.
///
/// Twenty keys in each of two kinds, plus focus loss. Declared here and
/// asserted equal to the constructed descriptor, so a key added to
/// [`ALL_WINDOW_KEYS`] without a matching pair of descriptor entries fails
/// rather than shipping a vocabulary the descriptor does not cover.
pub const WINDOW_INPUT_SURFACE_DESCRIPTOR_LEN: usize = 49;

/// The normalized kind of a window-boundary input.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WindowInputKind {
    KeyDown,
    KeyUp,
    FocusLost,
}

/// A normalized window-boundary input shared by native and deterministic adapters.
///
/// Platform key codes are converted to `WindowKey` values before this type is
/// constructed. The value contains no application, projection, parameter, or
/// audio state and must be translated into an application event before it can
/// reach the control layer.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WindowInput {
    key: WindowKey,
    kind: WindowInputKind,
}

/// The complete normalized vocabulary accepted at the window boundary.
///
/// Entries are concrete production values so deterministic adapters can feed
/// them directly through the same translator as the native window.
const WINDOW_INPUT_SURFACE_DESCRIPTOR: [WindowInput; WINDOW_INPUT_SURFACE_DESCRIPTOR_LEN] = [
    WindowInput::key_down(WindowKey::Digit2),
    WindowInput::key_down(WindowKey::Digit1),
    WindowInput::key_down(WindowKey::Digit3),
    WindowInput::key_down(WindowKey::Digit4),
    WindowInput::key_down(WindowKey::Digit5),
    WindowInput::key_down(WindowKey::Digit6),
    WindowInput::key_down(WindowKey::Digit7),
    WindowInput::key_down(WindowKey::Digit8),
    WindowInput::key_down(WindowKey::Digit9),
    WindowInput::key_down(WindowKey::Digit0),
    WindowInput::key_down(WindowKey::BracketLeft),
    WindowInput::key_down(WindowKey::BracketRight),
    WindowInput::key_down(WindowKey::Q),
    WindowInput::key_down(WindowKey::E),
    WindowInput::key_down(WindowKey::W),
    WindowInput::key_down(WindowKey::S),
    WindowInput::key_down(WindowKey::A),
    WindowInput::key_down(WindowKey::D),
    WindowInput::key_down(WindowKey::K),
    WindowInput::key_down(WindowKey::Shift),
    WindowInput::key_down(WindowKey::Return),
    WindowInput::key_down(WindowKey::Space),
    WindowInput::key_down(WindowKey::T),
    WindowInput::key_down(WindowKey::Other),
    WindowInput::key_up(WindowKey::Digit1),
    WindowInput::key_up(WindowKey::Digit2),
    WindowInput::key_up(WindowKey::Digit3),
    WindowInput::key_up(WindowKey::Digit4),
    WindowInput::key_up(WindowKey::Digit5),
    WindowInput::key_up(WindowKey::Digit6),
    WindowInput::key_up(WindowKey::Digit7),
    WindowInput::key_up(WindowKey::Digit8),
    WindowInput::key_up(WindowKey::Digit9),
    WindowInput::key_up(WindowKey::Digit0),
    WindowInput::key_up(WindowKey::BracketLeft),
    WindowInput::key_up(WindowKey::BracketRight),
    WindowInput::key_up(WindowKey::Q),
    WindowInput::key_up(WindowKey::E),
    WindowInput::key_up(WindowKey::W),
    WindowInput::key_up(WindowKey::S),
    WindowInput::key_up(WindowKey::A),
    WindowInput::key_up(WindowKey::D),
    WindowInput::key_up(WindowKey::K),
    WindowInput::key_up(WindowKey::Shift),
    WindowInput::key_up(WindowKey::Return),
    WindowInput::key_up(WindowKey::Space),
    WindowInput::key_up(WindowKey::T),
    WindowInput::key_up(WindowKey::Other),
    WindowInput::focus_lost(),
];

impl WindowInput {
    /// Returns all 41 unique valid normalized input values.
    ///
    /// This production-owned descriptor is the only exhaustive GUI-input
    /// vocabulary deterministic scenes and acceptance tests need to consume.
    pub const fn surface_descriptor() -> &'static [Self] {
        &WINDOW_INPUT_SURFACE_DESCRIPTOR
    }

    /// Creates normalized input data.
    ///
    /// Focus loss is not associated with a physical key, so its key is always
    /// canonicalized to `WindowKey::Other`.
    pub const fn new(key: WindowKey, kind: WindowInputKind) -> Self {
        let key = match kind {
            WindowInputKind::KeyDown | WindowInputKind::KeyUp => key,
            WindowInputKind::FocusLost => WindowKey::Other,
        };

        Self { key, kind }
    }

    /// Creates a normalized key-down input.
    pub const fn key_down(key: WindowKey) -> Self {
        Self::new(key, WindowInputKind::KeyDown)
    }

    /// Creates a normalized key-up input.
    pub const fn key_up(key: WindowKey) -> Self {
        Self::new(key, WindowInputKind::KeyUp)
    }

    /// Creates a normalized focus-loss input.
    pub const fn focus_lost() -> Self {
        Self::new(WindowKey::Other, WindowInputKind::FocusLost)
    }

    /// Returns the normalized key.
    pub const fn key(&self) -> WindowKey {
        self.key
    }

    /// Returns the normalized input kind.
    pub const fn kind(&self) -> WindowInputKind {
        self.kind
    }
}

#[cfg(test)]
mod tests {
    use super::{
        WindowInput, WindowInputKind, WindowKey, ALL_WINDOW_KEYS,
        WINDOW_INPUT_SURFACE_DESCRIPTOR_LEN,
    };

    #[test]
    fn surface_descriptor_contains_exactly_the_normalized_vocabulary() {
        let descriptor = WindowInput::surface_descriptor();

        // Exact equality, not a minimum: the point of this assertion is to
        // fail when the key vocabulary grows without the descriptor growing
        // with it, and a `>=` would pass through exactly that change.
        assert_eq!(descriptor.len(), 49);
        assert_eq!(descriptor.len(), WINDOW_INPUT_SURFACE_DESCRIPTOR_LEN);
        assert_eq!(ALL_WINDOW_KEYS.len() * 2 + 1, 49);
        for (index, input) in descriptor.iter().enumerate() {
            assert!(
                !descriptor[..index].contains(input),
                "duplicate descriptor entry: {input:?}"
            );
        }

        for key in ALL_WINDOW_KEYS {
            assert!(
                descriptor.contains(&WindowInput::key_down(key)),
                "{key:?} has no key-down entry"
            );
            assert!(
                descriptor.contains(&WindowInput::key_up(key)),
                "{key:?} has no key-up entry"
            );
        }

        let focus_lost = descriptor
            .iter()
            .filter(|input| input.kind() == WindowInputKind::FocusLost)
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(focus_lost, vec![WindowInput::focus_lost()]);
        assert_eq!(focus_lost[0].key(), WindowKey::Other);
    }

    /// The declared key list is complete and holds no duplicates, so the
    /// descriptor test above cannot be satisfied by a list that quietly lost a
    /// key.
    #[test]
    fn the_declared_key_list_names_every_key_once() {
        assert_eq!(ALL_WINDOW_KEYS.len(), 24);
        for (index, key) in ALL_WINDOW_KEYS.iter().enumerate() {
            assert!(
                !ALL_WINDOW_KEYS[..index].contains(key),
                "duplicate declared key: {key:?}"
            );
        }
        for key in [
            WindowKey::Digit3,
            WindowKey::Digit4,
            WindowKey::Digit5,
            WindowKey::Digit6,
            WindowKey::Digit7,
            WindowKey::Digit8,
            WindowKey::Digit9,
            WindowKey::Digit0,
            WindowKey::BracketLeft,
            WindowKey::BracketRight,
        ] {
            assert!(
                ALL_WINDOW_KEYS.contains(&key),
                "{key:?} is missing from the declared vocabulary"
            );
        }
    }

    /// The four keys added for the gallery carry no application binding.
    ///
    /// The invariant this holds is the design's: an unbound key reaching
    /// the translator produces *no* `SemanticAction`, not a substitute one. It
    /// is asserted here, beside the vocabulary that normalizes them, because
    /// this is where a later change would add one.
    #[test]
    fn the_four_keys_added_for_the_gallery_reach_no_semantic_action() {
        let mut translator =
            crate::shell::keyboard_input_translator::KeyboardInputTranslator::new();
        for key in [
            WindowKey::Digit9,
            WindowKey::Digit0,
            WindowKey::BracketLeft,
            WindowKey::BracketRight,
        ] {
            assert_eq!(
                translator.translate(WindowInput::key_down(key)),
                None,
                "{key:?} became a semantic action"
            );
            assert_eq!(
                translator.translate(WindowInput::key_up(key)),
                None,
                "releasing {key:?} became a semantic action"
            );
        }
    }

    #[test]
    fn key_down_preserves_every_normalized_key() {
        for key in ALL_WINDOW_KEYS {
            let input = WindowInput::key_down(key);
            assert_eq!(input.key(), key);
            assert_eq!(input.kind(), WindowInputKind::KeyDown);
        }
    }

    #[test]
    fn key_up_preserves_every_normalized_key() {
        for key in ALL_WINDOW_KEYS {
            let input = WindowInput::key_up(key);
            assert_eq!(input.key(), key);
            assert_eq!(input.kind(), WindowInputKind::KeyUp);
        }
    }

    #[test]
    fn focus_loss_has_one_canonical_representation() {
        let canonical = WindowInput::focus_lost();
        let normalized = WindowInput::new(WindowKey::W, WindowInputKind::FocusLost);

        assert_eq!(canonical, normalized);
        assert_eq!(canonical.key(), WindowKey::Other);
        assert_eq!(canonical.kind(), WindowInputKind::FocusLost);
    }

    #[test]
    fn normalized_input_is_copyable_shell_data() {
        let original = WindowInput::key_down(WindowKey::K);
        let copied = original;

        assert_eq!(copied, original);
    }
}
