//! Webview shell modules (mission webview-shell-foundation-01KZ9DN7,
//! hardened and given its observation forwarding by mission
//! webview-shell-cutover-01KZAC7Q WP02; the product's sole shell since the
//! WP07 cutover).
//!
//! The foundation contributed the native input-capture path, the typed
//! webview startup error ([`WebviewShellError`]), the Tauri v2 window
//! composition ([`TauriWebviewWindow`]), and the two page-bound transports:
//! [`projection_channel`] (generation-gated serialized view models) and
//! [`meter_channel`] (30 Hz latest-value meter frames). The cutover mission
//! adds the painted-ack → `ShellFrameObservation` forwarding
//! ([`projection_channel::ProjectionChannel::forward_ack`]), the
//! qualifying-frame seam ([`frame_stream`]), the restrictive page CSP,
//! release-gating of the page override, and typed render-exception and
//! window-close surfacing.
//!
//! # One shell, no fallback
//!
//! The webview shell is the only shell: the composition root constructs
//! [`TauriWebviewWindow`] directly, and no launch flag chooses a renderer.
//! A webview startup failure is a typed [`WebviewShellError`] that ends the
//! process through the same fatal-error path every other startup failure
//! uses — it never opens an alternate window, retries silently, or leaves a
//! blank window behind.
//!
//! # The qualifying-frame seam (WP02 T006)
//!
//! Scenes and harnesses ask "was a qualifying frame for accepted generation
//! N painted?" through [`frame_stream::QualifyingFrameStream`] (a handle
//! from [`TauriWebviewWindow::frame_stream`]), never by waiting out a
//! wall-clock sleep. A forwarded observation qualifies for a
//! [`frame_stream::FrameExpectation`] when its generation and stateHash
//! equal the awaited accepted generation's and its context and activeSurface
//! equal the expectation's — the identity gate the retained live scenes'
//! crediting has always started from. The non-blocking poll answers from
//! inside the tick loop, and it answers only from observations the
//! forwarding actually built out of a painted ack: a poll that finds nothing
//! means no honest ack for that identity has arrived yet, not that a frame
//! may be assumed.
//!
//! # Deterministic init-failure hook (debug builds only)
//!
//! For the acceptance tests that prove the typed failure path and inject
//! fixture pages: in debug builds, setting the `CREST_WEBVIEW_PAGE`
//! environment variable overrides the page the window serves with the file
//! at that path. An unreadable path is
//! [`WebviewShellError::PageLoadFailed`], raised before any window is
//! created, so the process exits nonzero with no blank window lingering.
//! The hook is an internal test seam, not an operator surface, and release
//! builds compile it out entirely (`cfg(debug_assertions)`, WP02 T007).

pub mod frame_stream;
pub mod input_capture;
pub mod meter_channel;
pub mod midi_activity_channel;
pub mod projection_channel;
pub mod token_export;
pub mod window;

pub use window::TauriWebviewWindow;
/// The single-source policy seam (mission webview-render-fidelity-hardening
/// WP02, research D3): the production window and the acceptance harness
/// both serve the page through this exact response builder and policy
/// constant — `requirement.graphical_shell_behavioral_proof` demands
/// "policy parity asserted from the single policy source", never a
/// restated copy. Two callers by design, not dead public API.
pub use window::{protocol_response, PAGE_CSP};

/// The single embedded tauri context for this crate.
///
/// `tauri::generate_context!` embeds link-time artifacts (the Info.plist
/// symbol among them) and therefore may be invoked exactly once per binary.
/// Every composition that builds a tauri app — the production
/// [`TauriWebviewWindow`] and the webview-hosted component gallery scene —
/// obtains the context here rather than invoking the macro again.
pub fn tauri_context() -> tauri::Context {
    tauri::generate_context!()
}

use crate::shell::app_window::WindowError;
use core::fmt;
use input_capture::InputCaptureError;

/// A typed webview shell runtime failure.
///
/// Every variant carries its underlying cause. The error surfaces through
/// the `AppWindow` port's declared `WindowError` into
/// `ApplicationError::Window` — the same top-level fatal path every other
/// startup failure uses — and ends the process nonzero. There is no
/// fallback shell.
#[derive(Debug)]
pub enum WebviewShellError {
    /// The Tauri runtime (system webview) failed to initialize.
    RuntimeUnavailable(tauri::Error),
    /// The page the window must serve could not be loaded.
    PageLoadFailed {
        /// The page source that failed to load.
        page: String,
        /// The underlying read failure.
        source: std::io::Error,
    },
    /// The webview window itself could not be created.
    WindowCreation(tauri::Error),
    /// The native Rust-side key monitor could not be installed.
    InputCapture(InputCaptureError),
    /// The page loaded but threw while rendering a pushed document
    /// (reported on the `crest://render-error` IPC event). Fatal on the
    /// same runtime path a startup failure uses — never a frozen window
    /// (WP02 T008).
    PageRenderFailed {
        /// The failure the page reported.
        detail: String,
    },
    /// Closing the webview window failed twice (one retry). Surfaced
    /// through the shutdown path rather than swallowed (foundation RISK-2).
    WindowClose(tauri::Error),
}

impl fmt::Display for WebviewShellError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeUnavailable(error) => {
                write!(formatter, "webview runtime unavailable: {error}")
            }
            Self::PageLoadFailed { page, source } => {
                write!(formatter, "webview page {page} failed to load: {source}")
            }
            Self::WindowCreation(error) => {
                write!(formatter, "webview window creation failed: {error}")
            }
            Self::InputCapture(error) => {
                write!(formatter, "webview input capture failed: {error}")
            }
            Self::PageRenderFailed { detail } => {
                write!(formatter, "webview page render failed: {detail}")
            }
            Self::WindowClose(error) => {
                write!(
                    formatter,
                    "webview window close failed after one retry: {error}"
                )
            }
        }
    }
}

impl std::error::Error for WebviewShellError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::RuntimeUnavailable(error)
            | Self::WindowCreation(error)
            | Self::WindowClose(error) => Some(error),
            Self::PageLoadFailed { source, .. } => Some(source),
            Self::InputCapture(error) => Some(error),
            Self::PageRenderFailed { .. } => None,
        }
    }
}

/// Joins the typed webview failure onto the `AppWindow` port's declared
/// error, preserving the cause text, so it travels the same
/// `ApplicationError::Window` fatal path as every other window failure.
impl From<WebviewShellError> for WindowError {
    fn from(error: WebviewShellError) -> Self {
        WindowError::new(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{InputCaptureError, WebviewShellError};
    use crate::shell::app_window::WindowError;
    use std::error::Error;

    #[test]
    fn webview_shell_error_preserves_each_typed_cause() {
        let page_error = WebviewShellError::PageLoadFailed {
            page: "/missing/page.html".to_owned(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"),
        };
        assert_eq!(
            page_error.to_string(),
            "webview page /missing/page.html failed to load: no such file"
        );
        assert!(page_error.source().is_some());

        let capture_error = WebviewShellError::InputCapture(InputCaptureError::NotMainThread);
        assert_eq!(
            capture_error.to_string(),
            "webview input capture failed: input capture must be installed on the main thread"
        );
        assert!(capture_error.source().is_some());
    }

    #[test]
    fn render_and_close_failures_are_typed_with_their_causes() {
        let render = WebviewShellError::PageRenderFailed {
            detail: "TypeError: renderPatch is not a function".to_owned(),
        };
        assert_eq!(
            render.to_string(),
            "webview page render failed: TypeError: renderPatch is not a function"
        );
        assert!(render.source().is_none());

        let close = WebviewShellError::WindowClose(tauri::Error::from(std::io::Error::other(
            "close refused",
        )));
        assert!(close
            .to_string()
            .starts_with("webview window close failed after one retry:"));
        assert!(close.source().is_some());
    }

    #[test]
    fn webview_shell_error_joins_the_window_error_path_with_its_cause_text() {
        let error = WebviewShellError::PageLoadFailed {
            page: "/missing/page.html".to_owned(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"),
        };
        let window_error = WindowError::from(error);
        assert_eq!(
            window_error.message(),
            "webview page /missing/page.html failed to load: no such file"
        );
    }
}

pub mod host_presentation;
