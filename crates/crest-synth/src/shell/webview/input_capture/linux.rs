//! GTK capture precedes WebKit's asynchronous key handling on X11 and Wayland.
use super::{InputCaptureError, RawKeyEvent};
use crate::shell::WindowKey;
use gtk::prelude::*;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

/// GDK uses XKB hardware codes (evdev + 8) on both Linux window backends.
/// Physical positions preserve WASD bindings across keyboard layouts.
pub const fn window_key_from_linux_key_code(code: u32) -> WindowKey {
    match code {
        10 => WindowKey::Digit1,
        11 => WindowKey::Digit2,
        12 => WindowKey::Digit3,
        13 => WindowKey::Digit4,
        14 => WindowKey::Digit5,
        15 => WindowKey::Digit6,
        16 => WindowKey::Digit7,
        17 => WindowKey::Digit8,
        18 => WindowKey::Digit9,
        19 => WindowKey::Digit0,
        34 => WindowKey::BracketLeft,
        35 => WindowKey::BracketRight,
        24 => WindowKey::Q,
        26 => WindowKey::E,
        25 | 111 => WindowKey::W,
        39 | 116 => WindowKey::S,
        38 | 113 => WindowKey::A,
        40 | 114 => WindowKey::D,
        45 => WindowKey::K,
        50 => WindowKey::Shift,
        36 => WindowKey::Return,
        65 => WindowKey::Space,
        28 => WindowKey::T,
        _ => WindowKey::Other,
    }
}

/// Owns the main-thread controller and disconnects its callbacks on retirement.
pub struct InputCaptureHandle {
    controller: gtk::EventControllerKey,
    signals: Vec<gtk::glib::SignalHandlerId>,
}

impl Drop for InputCaptureHandle {
    fn drop(&mut self) {
        self.controller
            .set_propagation_phase(gtk::PropagationPhase::None);
        for signal in self.signals.drain(..) {
            self.controller.disconnect(signal);
        }
    }
}

/// A true sink result consumes the key; false leaves it for native text entry.
pub fn install_for_window(
    window: &tauri::WebviewWindow,
    sink: impl FnMut(RawKeyEvent) -> bool + 'static,
) -> Result<InputCaptureHandle, InputCaptureError> {
    if !gtk::is_initialized_main_thread() {
        return Err(InputCaptureError::NotMainThread);
    }
    let native = window
        .gtk_window()
        .map_err(|_| InputCaptureError::MonitorRejected)?;
    let controller = gtk::EventControllerKey::new(&native);
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    let sink = Rc::new(RefCell::new(sink));
    let held = Rc::new(RefCell::new(HashSet::<u32>::new()));
    let press_sink = Rc::clone(&sink);
    let press_held = Rc::clone(&held);
    let pressed = controller.connect_key_pressed(move |_, _, code, modifiers| {
        // Native File-menu accelerators and desktop shortcuts remain native;
        // Ctrl+S must never also navigate the synth.
        let shortcut = gtk::gdk::ModifierType::CONTROL_MASK
            | gtk::gdk::ModifierType::MOD1_MASK
            | gtk::gdk::ModifierType::SUPER_MASK
            | gtk::gdk::ModifierType::META_MASK;
        let send_shortcut = code == 13
            && modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK)
            && !modifiers.intersects(shortcut - gtk::gdk::ModifierType::CONTROL_MASK);
        if modifiers.intersects(shortcut) && !send_shortcut {
            return false;
        }
        let key = window_key_from_linux_key_code(code);
        let repeat = !press_held.borrow_mut().insert(code);
        press_sink.borrow_mut()(RawKeyEvent::new(key, true, repeat))
    });
    let release_held = Rc::clone(&held);
    let released = controller.connect_key_released(move |_, _, code, _| {
        if release_held.borrow_mut().remove(&code) {
            sink.borrow_mut()(RawKeyEvent::new(
                window_key_from_linux_key_code(code),
                false,
                false,
            ));
        }
    });
    let unfocused = controller.connect_focus_out(move |_| held.borrow_mut().clear());
    Ok(InputCaptureHandle {
        controller,
        signals: vec![pressed, released, unfocused],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::window_input::ALL_WINDOW_KEYS;

    #[test]
    fn every_window_key_has_its_physical_linux_aliases() {
        for key in ALL_WINDOW_KEYS {
            if key == WindowKey::Other {
                continue;
            }
            let expected = if matches!(
                key,
                WindowKey::W | WindowKey::S | WindowKey::A | WindowKey::D
            ) {
                2
            } else {
                1
            };
            assert_eq!(
                (0..256)
                    .filter(|code| window_key_from_linux_key_code(*code) == key)
                    .count(),
                expected,
                "{key:?}"
            );
        }
        assert_eq!(window_key_from_linux_key_code(42), WindowKey::Other);
    }
}
