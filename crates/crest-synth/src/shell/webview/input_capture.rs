//! Rust-side native key capture for the webview shell.
//!
//! macOS uses an `NSEvent` local monitor; Linux uses a GTK capture-phase key
//! controller on the owned window. Both observe native input before WebKit
//! dispatch. `tauri::WindowEvent` has no keyboard variant, so input capture
//! belongs at this native boundary, never in the projection page.
//!
//! The module emits [`RawKeyEvent`] values only. It owns no translator, no
//! modifier state, and no application state: the sink is expected to
//! normalize into [`crate::shell::window_input::WindowInput`] and feed the
//! shared [`crate::shell::KeyboardInputTranslator`], exactly as the retired native
//! adapter does — that wiring belongs to the webview window composition
//! (WP02), not here.
//!
//! Threading contract: [`install_for_window`] must be called on the main thread,
//! and the sink is invoked on the main thread — the same thread that runs the
//! Tauri event loop — so a `KeyboardInputTranslator` living in main-thread
//! state needs no synchronization. Dropping the returned handle removes the
//! native capture.

use crate::shell::window_input::WindowKey;
use core::fmt;

/// One key transition observed Rust-side before webview dispatch.
///
/// `key` is normalized to the canonical window-boundary vocabulary
/// ([`WindowKey`]) here, inside the platform module, so no platform key code
/// leaks past this boundary. The optional Shift snapshot lets the window
/// reconcile a held modifier when focus changes or an input source sends a
/// chord without a separate modifier transition.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RawKeyEvent {
    key: WindowKey,
    pressed: bool,
    repeat: bool,
    shift: Option<bool>,
}

impl RawKeyEvent {
    /// Creates a raw key transition.
    pub const fn new(key: WindowKey, pressed: bool, repeat: bool) -> Self {
        Self {
            key,
            pressed,
            repeat,
            shift: None,
        }
    }

    /// Attaches the native event's aggregate Shift state (either physical key).
    pub const fn with_shift(mut self, held: bool) -> Self {
        self.shift = Some(held);
        self
    }

    pub const fn shift(&self) -> Option<bool> {
        self.shift
    }

    /// The normalized key identity.
    pub const fn key(&self) -> WindowKey {
        self.key
    }

    /// `true` for a press, `false` for a release.
    pub const fn pressed(&self) -> bool {
        self.pressed
    }

    /// `true` when this press is an OS auto-repeat of a held key.
    ///
    /// Releases are never repeats. The retired native path received
    /// auto-repeats too; the consumer decides what repeats mean.
    pub const fn repeat(&self) -> bool {
        self.repeat
    }
}

/// A failure installing the native key monitor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InputCaptureError {
    /// Capture was installed off the main thread.
    NotMainThread,
    /// The platform rejected the monitor installation.
    MonitorRejected,
    /// This platform has no native capture implementation yet.
    UnsupportedPlatform,
}

impl fmt::Display for InputCaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotMainThread => {
                formatter.write_str("input capture must be installed on the main thread")
            }
            Self::MonitorRejected => {
                formatter.write_str("the platform rejected the local key-event monitor")
            }
            Self::UnsupportedPlatform => formatter
                .write_str("native webview input capture is not implemented on this platform"),
        }
    }
}

impl std::error::Error for InputCaptureError {}

/// Maps a macOS virtual key code (ANSI layout positions) to the canonical
/// window-boundary vocabulary.
///
/// Virtual key codes name physical key positions, so this matches the
/// `kVK_ANSI_*` constants; on non-ANSI hardware layouts the physical position
/// wins, which is the standard behavior for position-based bindings (WASD).
/// Every unmapped code normalizes to [`WindowKey::Other`], mirroring the
/// retired native adapter's `normalize_key`.
pub const fn window_key_from_macos_key_code(key_code: u16) -> WindowKey {
    match key_code {
        18 => WindowKey::Digit1,
        19 => WindowKey::Digit2,
        20 => WindowKey::Digit3,
        21 => WindowKey::Digit4,
        23 => WindowKey::Digit5,
        22 => WindowKey::Digit6,
        26 => WindowKey::Digit7,
        28 => WindowKey::Digit8,
        25 => WindowKey::Digit9,
        29 => WindowKey::Digit0,
        33 => WindowKey::BracketLeft,
        30 => WindowKey::BracketRight,
        12 => WindowKey::Q,
        14 => WindowKey::E,
        13 | 126 => WindowKey::W,
        1 | 125 => WindowKey::S,
        0 | 123 => WindowKey::A,
        2 | 124 => WindowKey::D,
        40 => WindowKey::K,
        56 | 60 => WindowKey::Shift,
        36 => WindowKey::Return,
        49 => WindowKey::Space,
        17 => WindowKey::T,
        _ => WindowKey::Other,
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{window_key_from_macos_key_code, InputCaptureError, RawKeyEvent};
    use block2::RcBlock;
    use core::ptr::NonNull;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{ClassType, MainThreadMarker};
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSPanel};
    use objc2_foundation::NSObjectProtocol;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    /// How many recently delivered key-event signatures the monitor retains
    /// to recognize WebKit's re-dispatch of an unhandled key event.
    ///
    /// WKWebView processes key events asynchronously: an event the page does
    /// not handle comes back through `sendEvent:` a second time (the
    /// unhandled-key round-trip), and a local monitor observes that second
    /// delivery too. One physical transition must feed the translator
    /// exactly once (mission webview-shell-cutover WP05, the FR-003
    /// key-injection witness), so the monitor skips an event whose
    /// `(type, keyCode, timestamp)` it has already delivered — two distinct
    /// physical transitions can never share an `NSEvent` timestamp, while
    /// the round-trip preserves it verbatim. The window is bounded; the
    /// round-trip normally returns within a frame or two, but WebKit may defer
    /// the first replay until after a burst spanning the complete normalized
    /// key vocabulary. Keep the ring bounded while retaining that 68-edge
    /// production witness with margin.
    const REDISPATCH_WINDOW: usize = 128;

    /// The identity of one delivered key event: down/up, hardware key code,
    /// and the event's own timestamp bit pattern.
    type DeliveredSignature = (bool, u16, u64);

    /// Reads AppKit's repeat flag only for a real key-down event.
    ///
    /// Keeping the read lazy is material: `FlagsChanged` is how Shift enters
    /// the monitor, and querying a key-only property from that event aborts at
    /// the Objective-C callback boundary before Rust can unwind.
    pub(super) fn repeat_for_event(
        event_type: NSEventType,
        read_key_repeat: impl FnOnce() -> bool,
    ) -> bool {
        match event_type {
            NSEventType::KeyDown => read_key_repeat(),
            NSEventType::KeyUp | NSEventType::FlagsChanged => false,
            _ => false,
        }
    }

    /// Owns the installed local monitor; dropping it removes the monitor.
    pub struct InputCaptureHandle {
        monitor: Retained<AnyObject>,
        // The monitor block holds the sink; keep the handle main-thread-bound
        // so removal happens where installation did.
        _not_send: core::marker::PhantomData<*const ()>,
    }

    impl Drop for InputCaptureHandle {
        fn drop(&mut self) {
            // SAFETY: `monitor` is exactly the token returned by
            // `addLocalMonitorForEventsMatchingMask:handler:`.
            unsafe { NSEvent::removeMonitor(&self.monitor) };
        }
    }

    /// Installs the `NSEvent` local key monitor and feeds every key
    /// transition to `sink` — exactly once per physical transition. Events
    /// are returned to AppKit unchanged, so the webview still receives them;
    /// whether the product shell swallows vocabulary keys is the window
    /// composition's decision (WP02).
    ///
    /// A key event WebKit re-dispatches after its asynchronous
    /// unhandled-key round-trip is recognized by its verbatim
    /// `(type, keyCode, timestamp)` signature and passed through without a
    /// second sink delivery (see [`REDISPATCH_WINDOW`]).
    pub fn install(
        sink: impl FnMut(RawKeyEvent) + 'static,
    ) -> Result<InputCaptureHandle, InputCaptureError> {
        let main_thread = MainThreadMarker::new().ok_or(InputCaptureError::NotMainThread)?;
        let sink = RefCell::new(sink);
        let delivered: RefCell<VecDeque<DeliveredSignature>> =
            RefCell::new(VecDeque::with_capacity(REDISPATCH_WINDOW));
        let handler = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit hands the monitor a valid event for the duration
            // of the call; we only read from it.
            let observed = unsafe { event.as_ref() };
            // AppKit owns menu/text shortcuts; Cmd+S must never also move
            // semantic focus down, nor Cmd+W up, before the menu handles it.
            let flags = observed.modifierFlags();
            let send_shortcut = observed.keyCode() == 21
                && flags.contains(NSEventModifierFlags(1 << 18))
                && !flags.intersects(NSEventModifierFlags((1 << 20) | (1 << 19)));
            if flags.intersects(NSEventModifierFlags((1 << 20) | (1 << 19) | (1 << 18)))
                && !send_shortcut
            {
                return event.as_ptr();
            }
            // Native Open/Save panels own their typing. Their keys must not
            // become instrument edits when the asynchronous panel is open.
            if observed
                .window(main_thread)
                .is_some_and(|window| window.isKindOfClass(NSPanel::class()))
            {
                return event.as_ptr();
            }
            let event_type = observed.r#type();
            let pressed = match event_type {
                NSEventType::KeyDown => true,
                NSEventType::KeyUp => false,
                NSEventType::FlagsChanged if matches!(observed.keyCode(), 56 | 60) => observed
                    .modifierFlags()
                    .contains(NSEventModifierFlags(1 << 17)),
                _ => return event.as_ptr(),
            };
            // WebKit's unhandled-key round-trip delivers the same event a
            // second time; one physical transition feeds the translator
            // exactly once. Two distinct transitions can never share a
            // timestamp, so the verbatim signature identifies the replay.
            let signature: DeliveredSignature =
                (pressed, observed.keyCode(), observed.timestamp().to_bits());
            {
                let mut recent = delivered.borrow_mut();
                if recent.contains(&signature) {
                    return event.as_ptr();
                }
                if recent.len() == REDISPATCH_WINDOW {
                    recent.pop_front();
                }
                recent.push_back(signature);
            }
            let raw = RawKeyEvent::new(
                window_key_from_macos_key_code(observed.keyCode()),
                pressed,
                // `isARepeat` is a key-event property. Shift reaches this
                // monitor as `FlagsChanged`, not `KeyDown`; asking AppKit for
                // the key-repeat property on that event panics inside the
                // Objective-C callback and the process must abort because the
                // callback cannot unwind. Modifier transitions never repeat.
                repeat_for_event(event_type, || observed.isARepeat()),
            )
            .with_shift(flags.contains(NSEventModifierFlags::Shift));
            (sink.borrow_mut())(raw);
            // Pass the event through untouched.
            event.as_ptr()
        });
        // SAFETY: the block returns the pointer it was given (a valid event).
        let monitor = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::KeyDown | NSEventMask::KeyUp | NSEventMask::FlagsChanged,
                &handler,
            )
        };
        monitor
            .map(|monitor| InputCaptureHandle {
                monitor,
                _not_send: core::marker::PhantomData,
            })
            .ok_or(InputCaptureError::MonitorRejected)
    }
}

#[cfg(target_os = "macos")]
pub use platform::{install, InputCaptureHandle};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{install_for_window, window_key_from_linux_key_code, InputCaptureHandle};

/// Installs capture before webview dispatch. The sink returns whether Linux
/// should consume the key; false preserves native text entry. AppKit continues
/// to pass events through its asynchronous WebKit dispatch as before.
#[cfg(target_os = "macos")]
pub fn install_for_window(
    _window: &tauri::WebviewWindow,
    mut sink: impl FnMut(RawKeyEvent) -> bool + 'static,
) -> Result<InputCaptureHandle, InputCaptureError> {
    install(move |raw| {
        sink(raw);
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn install_for_window(
    _window: &tauri::WebviewWindow,
    _sink: impl FnMut(RawKeyEvent) -> bool + 'static,
) -> Result<(), InputCaptureError> {
    Err(InputCaptureError::UnsupportedPlatform)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
/// Platforms without a native capture adapter fail explicitly.
pub fn install(_sink: impl FnMut(RawKeyEvent) + 'static) -> Result<(), InputCaptureError> {
    Err(InputCaptureError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use super::{window_key_from_macos_key_code, RawKeyEvent};
    use crate::shell::window_input::{WindowKey, ALL_WINDOW_KEYS};

    /// Every key in the normalized window vocabulary except `Other` is
    /// reachable from its exact macOS virtual key codes, so the capture path
    /// covers the full MIXER vocabulary the translator consumes.
    #[test]
    fn every_normalized_key_has_exactly_its_declared_macos_aliases() {
        for key in ALL_WINDOW_KEYS {
            if key == WindowKey::Other {
                continue;
            }
            let codes = (0_u16..=127)
                .filter(|code| window_key_from_macos_key_code(*code) == key)
                .count();
            let expected = if matches!(
                key,
                WindowKey::W | WindowKey::S | WindowKey::A | WindowKey::D | WindowKey::Shift
            ) {
                2
            } else {
                1
            };
            assert_eq!(
                codes, expected,
                "{key:?} must have its exact physical aliases"
            );
        }
    }

    #[test]
    fn unmapped_key_codes_normalize_to_other() {
        // kVK_ANSI_G (5) is outside the vocabulary.
        assert_eq!(window_key_from_macos_key_code(5), WindowKey::Other);
        assert_eq!(window_key_from_macos_key_code(60), WindowKey::Shift);
    }

    #[test]
    fn actual_arrows_and_wasd_have_identical_page_and_focus_meanings() {
        use crate::control::{Direction, SemanticAction};
        use crate::shell::{KeyboardInputTranslator, WindowInput};
        for (arrow, wasd, direction) in [
            (126, 13, Direction::Up),
            (125, 1, Direction::Down),
            (123, 0, Direction::Left),
            (124, 2, Direction::Right),
        ] {
            for code in [arrow, wasd] {
                let key = window_key_from_macos_key_code(code);
                let mut translator = KeyboardInputTranslator::new();
                assert_eq!(
                    translator.translate(WindowInput::key_down(key)),
                    Some(SemanticAction::Navigate(direction))
                );
                translator.translate(WindowInput::key_up(key));
                translator.translate(WindowInput::key_down(WindowKey::Shift));
                assert_eq!(
                    translator.translate(WindowInput::key_down(key)),
                    Some(SemanticAction::NavigatePage(direction))
                );
                assert_eq!(translator.translate(WindowInput::key_down(key)), None);
            }
        }
    }

    #[test]
    fn raw_key_events_are_copyable_shell_data() {
        let event = RawKeyEvent::new(WindowKey::W, true, false);
        let copied = event;
        assert_eq!(copied, event);
        assert_eq!(event.key(), WindowKey::W);
        assert!(event.pressed());
        assert!(!event.repeat());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn modifier_transitions_never_query_the_key_repeat_property() {
        use objc2_app_kit::NSEventType;

        for _ in 0..64 {
            assert!(!super::platform::repeat_for_event(
                NSEventType::FlagsChanged,
                || panic!("modifier input must not query key repeat")
            ));
        }
        assert!(super::platform::repeat_for_event(
            NSEventType::KeyDown,
            || true
        ));
        assert!(!super::platform::repeat_for_event(
            NSEventType::KeyUp,
            || panic!("key release must not query key repeat")
        ));
    }
}
