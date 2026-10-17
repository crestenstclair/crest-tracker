//! The Tauri v2 webview window — the product's one shell behind the
//! `AppWindow` port.
//!
//! Composition follows the original input-capture probe verdict:
//!
//! - keys are captured Rust-side by [`input_capture::install_for_window`]
//!   (AppKit monitor or GTK capture controller on the event thread) and
//!   normalized through the same [`KeyboardInputTranslator`] the retired native
//!   window uses — one shared key state machine, no page key handler;
//! - the event loop runs through `run_return`, never `run`, so
//!   `AppWindow::run` returns and `StandaloneApplication::run` performs the
//!   same owned shutdown the retired native close performed (stream release before
//!   worker completion, graph ownership collection, normal exit);
//! - the tao loop waits when idle, so a detached waker pings the main thread
//!   at the 16 ms idle-frame cadence the retired native path used and control-side
//!   ticks run on `MainEventsCleared`;
//! - `Focused(false)` feeds [`WindowInput::focus_lost`], clearing the held-K
//!   modifier exactly as the retired native adapter did;
//! - the page is served over the registered `crest://` custom protocol
//!   (`WebviewUrl::External` requires http(s) and data URLs are rejected at
//!   the config layer).
//!
//! The window serves the authored projection page (`webview-page/`) with its
//! generated token table, composition stylesheet, render script, and
//! vendored Azeret Mono faces, all embedded at compile time and routed by
//! request path through the `crest://` protocol handler under a restrictive
//! Content-Security-Policy ([`PAGE_CSP`]). Page resolution happens before
//! any window exists, so an unloadable page is a typed
//! [`WebviewShellError::PageLoadFailed`] and the process ends with no blank
//! window. The deterministic trigger for that path is the debug-build-only
//! `CREST_WEBVIEW_PAGE` override documented in [`crate::shell::webview`].
//!
//! Two page-reported conditions come back over IPC events and are handled
//! here (mission webview-shell-cutover WP02): each painted document's ack
//! (`crest://painted`) is forwarded through the projection channel into
//! exactly one `ShellFrameObservation` on the port's frame callback and the
//! qualifying-frame stream, and a thrown page render failure
//! (`crest://render-error`) is a typed
//! [`WebviewShellError::PageRenderFailed`] on the same fatal runtime path a
//! startup failure uses. A failed `window.close()` is retried once; a second
//! failure is recorded typed and ends the event loop directly, so the
//! shutdown path surfaces it — never swallowed, and never a loop left
//! waiting on a close that is not going to happen (mission shell-hygiene
//! RISK-3).

use crate::shell::app_window::{
    AppInputCallback, AppWindow, AudioObservationCallback, FrameObservationCallback,
    MidiActivityObservationCallback, ProjectionCallback, SessionCommand, SessionCommandCallback,
    SessionDocumentProjectionCallback, TickCallback, WindowError,
};
use crate::shell::density::RepresentativeViewport;
use crate::shell::keyboard_input_translator::KeyboardInputTranslator;
use crate::shell::webview::frame_stream::QualifyingFrameStream;
use crate::shell::webview::host_presentation::HostPresentation;
use crate::shell::webview::meter_channel::{MeterChannel, METER_EVENT};
use crate::shell::webview::projection_channel::{
    ForwardedAck, ProjectionChannel, PAINTED_EVENT, PROJECTION_EVENT, READY_EVENT,
    RENDER_ERROR_EVENT,
};
use crate::shell::webview::{input_capture, WebviewShellError};
use crate::shell::window_input::WindowInput;
use crate::shell::{
    NativeSessionDialogBridge, SessionDialogPort, SessionDialogRequest, SessionDialogResult,
    SessionDocumentMarker, SessionDocumentProjection, SessionFileDialogKind, UnsavedChoice,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use tauri::{Emitter, Listener, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};

const MENU_NEW: &str = "session.new";
const MENU_OPEN: &str = "session.open";
const MENU_SAVE: &str = "session.save";
const MENU_SAVE_AS: &str = "session.save_as";
const MENU_CLOSE: &str = "session.close";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeFileMenuItem {
    id: &'static str,
    label: &'static str,
    accelerator: &'static str,
    command: SessionCommand,
}

impl NativeFileMenuItem {
    pub const fn id(self) -> &'static str {
        self.id
    }

    pub const fn label(self) -> &'static str {
        self.label
    }

    pub const fn accelerator(self) -> &'static str {
        self.accelerator
    }

    pub const fn command(self) -> SessionCommand {
        self.command
    }
}

const NATIVE_FILE_MENU: [NativeFileMenuItem; 5] = [
    NativeFileMenuItem {
        id: MENU_NEW,
        label: "New",
        accelerator: "CmdOrCtrl+N",
        command: SessionCommand::New,
    },
    NativeFileMenuItem {
        id: MENU_OPEN,
        label: "Open…",
        accelerator: "CmdOrCtrl+O",
        command: SessionCommand::Open,
    },
    NativeFileMenuItem {
        id: MENU_SAVE,
        label: "Save",
        accelerator: "CmdOrCtrl+S",
        command: SessionCommand::Save,
    },
    NativeFileMenuItem {
        id: MENU_SAVE_AS,
        label: "Save As…",
        accelerator: "CmdOrCtrl+Shift+S",
        command: SessionCommand::SaveAs,
    },
    NativeFileMenuItem {
        id: MENU_CLOSE,
        label: "Close",
        accelerator: "CmdOrCtrl+W",
        command: SessionCommand::Close,
    },
];

pub const fn native_file_menu_items() -> &'static [NativeFileMenuItem] {
    &NATIVE_FILE_MENU
}

/// Normalizes one Tauri menu identity into the host-neutral lifecycle
/// command. Unknown menu items are deliberately ignored.
pub fn session_command_for_menu_id(id: &str) -> Option<SessionCommand> {
    match id {
        MENU_NEW => Some(SessionCommand::New),
        MENU_OPEN => Some(SessionCommand::Open),
        MENU_SAVE => Some(SessionCommand::Save),
        MENU_SAVE_AS => Some(SessionCommand::SaveAs),
        MENU_CLOSE => Some(SessionCommand::Close),
        _ => None,
    }
}

fn native_document_title(document: &SessionDocumentProjection) -> String {
    let marker = match document.marker() {
        SessionDocumentMarker::Ready => "",
        SessionDocumentMarker::Busy => " [WORKING]",
        SessionDocumentMarker::Error => " [ERROR]",
    };
    let dirty = if document.dirty() { " *" } else { "" };
    let status = document.failure().unwrap_or(document.status());
    format!(
        "{}{dirty}{marker} — crest-synth — {status}",
        document.name()
    )
}

fn present_native_dialog(request: SessionDialogRequest) -> SessionDialogResult {
    let request_id = request.request_id();
    match request {
        SessionDialogRequest::File {
            kind,
            suggested_name,
            ..
        } => {
            let mut dialog = rfd::FileDialog::new().add_filter("Crest Synth Session", &["crest"]);
            if let Some(name) = suggested_name {
                dialog = dialog.set_file_name(name);
            }
            let selected = match kind {
                SessionFileDialogKind::Open => dialog.pick_file(),
                SessionFileDialogKind::SaveAs => dialog.save_file(),
            };
            selected.map_or(SessionDialogResult::Cancelled { request_id }, |path| {
                SessionDialogResult::FileSelected { request_id, path }
            })
        }
        SessionDialogRequest::UnsavedChanges { document_name, .. } => {
            let result = rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Warning)
                .set_title("Unsaved Crest Synth session")
                .set_description(format!(
                    "Save changes to {document_name} before continuing?"
                ))
                .set_buttons(rfd::MessageButtons::YesNoCancelCustom(
                    "Save".to_owned(),
                    "Discard".to_owned(),
                    "Cancel".to_owned(),
                ))
                .show();
            let choice = match result {
                rfd::MessageDialogResult::Yes | rfd::MessageDialogResult::Ok => UnsavedChoice::Save,
                rfd::MessageDialogResult::No => UnsavedChoice::Discard,
                rfd::MessageDialogResult::Custom(ref label) if label == "Save" => {
                    UnsavedChoice::Save
                }
                rfd::MessageDialogResult::Custom(ref label) if label == "Discard" => {
                    UnsavedChoice::Discard
                }
                _ => UnsavedChoice::Cancel,
            };
            SessionDialogResult::UnsavedChoice { request_id, choice }
        }
    }
}

/// The idle tick cadence — the same 16 ms idle-frame convention the retired
/// adapter declares as its repaint interval.
const TICK_INTERVAL: Duration = Duration::from_millis(16);

/// The single webview window's stable Tauri label.
const WINDOW_LABEL: &str = "main";

/// The deterministic init-failure and page-injection hook consumed by the
/// acceptance tests: when set, the window serves the file at this path
/// instead of the built-in page, and an unreadable path is a typed
/// [`WebviewShellError::PageLoadFailed`] before any window opens.
///
/// Debug builds only: release builds compile this constant and its serving
/// branch out entirely (`cfg(debug_assertions)`), so the override path does
/// not exist in a release binary (WP02 T007).
#[cfg(debug_assertions)]
const PAGE_OVERRIDE_ENV: &str = "CREST_WEBVIEW_PAGE";

/// The deterministic double-close-failure hook consumed by the acceptance
/// tests: when set, every close attempt the shell makes reports failure
/// instead of being issued, so the window stays open and the exhausted-retry
/// path below is reached on demand. Without it that path is reachable only
/// from a window server that refuses the same close twice in a row, and
/// FR-001 has to be provable.
///
/// Debug builds only: release builds compile this constant and the branch
/// that reads it out entirely (`cfg(debug_assertions)`), exactly as
/// [`PAGE_OVERRIDE_ENV`] does, so the forced-failure path does not exist in
/// a release binary.
#[cfg(debug_assertions)]
const CLOSE_FAILURE_OVERRIDE_ENV: &str = "CREST_WEBVIEW_FORCE_CLOSE_FAILURE";

/// The exit code the shell requests when it ends the event loop itself
/// (see [`close_window_once_with_retry`]). Zero on purpose: the terminal
/// outcome is the typed error already recorded on the first-error slot,
/// which the existing post-`run_return` path surfaces, and the exit code
/// must not become a second, competing failure signal.
const REQUESTED_EXIT_CODE: i32 = 0;

/// The restrictive Content-Security-Policy attached to the served document
/// (foundation review open item 5, WP02 T007; hardened by mission
/// webview-render-fidelity-hardening WP02, NFR-001).
///
/// `default-src 'none'` is the baseline; the page is then allowed exactly
/// what the embedded `crest://` assets need — its own origin's stylesheets
/// (`tokens.css`, `page.css`), render script (`page.js`), and vendored
/// Azeret Mono faces. No remote host, no inline script or style, no eval.
/// The only non-`'self'` source is the tauri IPC loopback (`ipc:` /
/// `http://ipc.localhost`), which the injected `__TAURI__` API fetches for
/// event delivery — it never leaves the process.
///
/// `base-uri` and `form-action` do not inherit from `default-src`, so the
/// hardened policy denies both explicitly: the document can never rebase
/// its relative URL resolution and never submits a form anywhere.
///
/// `style-src 'self'` blocks parsed inline style attributes; the page
/// applies its dynamic fader/position geometry through CSSOM
/// custom-property assignment (`element.style.setProperty`, research D1)
/// precisely because of that. The policy is never weakened to make the
/// page paint (C-004).
///
/// This constant is the deliberate single source of the served policy
/// (research D3): both callers of [`protocol_response`] — the production
/// window and the acceptance harness — ship exactly these directives,
/// never a restated copy.
pub const PAGE_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; \
     font-src 'self'; connect-src ipc: http://ipc.localhost; \
     base-uri 'none'; form-action 'none'";

/// The authored MIXER projection page (WP05, `webview-page/`), embedded at
/// compile time and served over the registered `crest://` protocol together
/// with its generated token table, composition stylesheet, render script,
/// and the vendored Azeret Mono faces. The page renders projections pushed
/// on the projection, meter, and MIDI-activity transports; it registers no
/// input (keys stay Rust-side).
const PAGE_INDEX_HTML: &str = include_str!("../../../webview-page/index.html");
const PAGE_TOKENS_CSS: &str = include_str!("../../../webview-page/tokens.css");
const PAGE_CSS: &str = include_str!("../../../webview-page/page.css");
const PAGE_JS: &str = include_str!("../../../webview-page/page.js");
const FONT_AZERET_REGULAR: &[u8] =
    include_bytes!("../../../vendor/azeret-mono/AzeretMono-Regular.ttf");
const FONT_AZERET_MEDIUM: &[u8] =
    include_bytes!("../../../vendor/azeret-mono/AzeretMono-Medium.ttf");
const FONT_AZERET_SEMIBOLD: &[u8] =
    include_bytes!("../../../vendor/azeret-mono/AzeretMono-SemiBold.ttf");
const FONT_AZERET_BOLD: &[u8] = include_bytes!("../../../vendor/azeret-mono/AzeretMono-Bold.ttf");

/// Resolves one `crest://` request path to its embedded page asset as a
/// `(content type, body)` pair, or `None` for a path the page never
/// references (served as 404). `index_html` is passed in because the index
/// document — and only the index document — honors the deterministic
/// `CREST_WEBVIEW_PAGE` override.
fn page_asset(path: &str, index_html: &str) -> Option<(&'static str, Vec<u8>)> {
    match path {
        "/" | "/index.html" => Some(("text/html; charset=utf-8", index_html.as_bytes().to_vec())),
        "/tokens.css" => Some((
            "text/css; charset=utf-8",
            PAGE_TOKENS_CSS.as_bytes().to_vec(),
        )),
        "/page.css" => Some(("text/css; charset=utf-8", PAGE_CSS.as_bytes().to_vec())),
        "/page.js" => Some((
            "text/javascript; charset=utf-8",
            PAGE_JS.as_bytes().to_vec(),
        )),
        "/fonts/AzeretMono-Regular.ttf" => Some(("font/ttf", FONT_AZERET_REGULAR.to_vec())),
        "/fonts/AzeretMono-Medium.ttf" => Some(("font/ttf", FONT_AZERET_MEDIUM.to_vec())),
        "/fonts/AzeretMono-SemiBold.ttf" => Some(("font/ttf", FONT_AZERET_SEMIBOLD.to_vec())),
        "/fonts/AzeretMono-Bold.ttf" => Some(("font/ttf", FONT_AZERET_BOLD.to_vec())),
        _ => None,
    }
}

/// Builds the complete `crest://` protocol response for one request path:
/// the embedded asset with its declared content type — the document
/// additionally carrying the restrictive [`PAGE_CSP`] — or a 404 for a path
/// the page never references (no fallback page).
///
/// This function is the deliberate SINGLE SOURCE the served policy ships
/// through, public because it has exactly two callers by design: the
/// production window's `crest://` protocol handler in
/// [`TauriWebviewWindow`]'s event loop, and the acceptance harness in
/// `tests/webview_projection_shell.rs`, which must serve the page EXACTLY
/// as production does — `requirement.graphical_shell_behavioral_proof`:
/// "policy parity asserted from the single policy source" (research D3).
/// The harness once re-implemented this response privately without the CSP
/// header, and that drift hid a paint-fidelity defect (DRIFT-1/RISK-1);
/// the export makes that drift structurally impossible. This is a
/// two-caller seam, not dead public API: do not remove the export, and do
/// not add harness-only parameters — the harness adapts to production,
/// never the reverse.
pub fn protocol_response(path: &str, index_html: &str) -> tauri::http::Response<Vec<u8>> {
    match page_asset(path, index_html) {
        Some((content_type, body)) => {
            let builder = tauri::http::Response::builder().header("Content-Type", content_type);
            let builder = if content_type.starts_with("text/html") {
                builder.header("Content-Security-Policy", PAGE_CSP)
            } else {
                builder
            };
            builder
                .body(body)
                .expect("the embedded asset response is well-formed")
        }
        None => tauri::http::Response::builder()
            .status(404)
            .body(Vec::new())
            .expect("the empty not-found response is well-formed"),
    }
}

/// Production Tauri v2 webview window: the one `AppWindow` implementation
/// the product composes. There is no fallback shell in any direction.
#[derive(Clone, Debug)]
pub struct TauriWebviewWindow {
    title: String,
    frames: QualifyingFrameStream,
    dialogs: NativeSessionDialogBridge,
    host: Option<HostPresentation>,
}

impl TauriWebviewWindow {
    /// Creates a webview window with the supplied native title.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            frames: QualifyingFrameStream::new(),
            dialogs: NativeSessionDialogBridge::default(),
            host: None,
        }
    }

    pub fn with_host(mut self, host: HostPresentation) -> Self {
        self.host = Some(host);
        self
    }

    /// Returns the native window title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns a handle onto the qualifying-frame stream this window's
    /// painted-ack forwarding records into (WP02 T006).
    ///
    /// Take the handle before [`AppWindow::run`] and poll or await from the
    /// control side; clones (including clones of the window) share one
    /// stream. What qualifies and what a timeout means are documented on
    /// [`crate::shell::webview::frame_stream`].
    pub fn frame_stream(&self) -> QualifyingFrameStream {
        self.frames.clone()
    }
}

impl Default for TauriWebviewWindow {
    fn default() -> Self {
        Self::new("crest-synth")
    }
}

/// Reads the debug-only `CREST_WEBVIEW_PAGE` override page. An unreadable
/// path is the typed init failure the acceptance tests trigger
/// deterministically.
#[cfg(debug_assertions)]
fn read_override_page(path: &std::path::Path) -> Result<String, WebviewShellError> {
    std::fs::read_to_string(path).map_err(|source| WebviewShellError::PageLoadFailed {
        page: path.display().to_string(),
        source,
    })
}

/// Resolves the page the window serves. Called before any window exists so
/// a failure is a typed startup error with no blank window.
///
/// Debug builds honor the deterministic `CREST_WEBVIEW_PAGE` override;
/// release builds serve the embedded page unconditionally — the env read
/// and the override-serving branch are compiled out entirely
/// (`cfg(debug_assertions)`, WP02 T007), so a release binary contains no
/// page override path at all.
fn resolve_page_source() -> Result<String, WebviewShellError> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os(PAGE_OVERRIDE_ENV).map(std::path::PathBuf::from) {
        return read_override_page(&path);
    }
    Ok(PAGE_INDEX_HTML.to_owned())
}

/// Feeds captured raw keys through the shared production translator into the
/// application input callback. This is plumbing only: the one key state
/// machine is [`KeyboardInputTranslator`], unchanged across the cutover.
struct KeyPipeline {
    translator: KeyboardInputTranslator,
    on_input: AppInputCallback,
}

impl KeyPipeline {
    /// Reconciles native modifier snapshots before delivering the key once.
    fn feed_raw(&mut self, raw: input_capture::RawKeyEvent) {
        if let Some(held) = raw.shift() {
            if raw.key() != crate::shell::WindowKey::Shift {
                self.feed(if held {
                    WindowInput::key_down(crate::shell::WindowKey::Shift)
                } else {
                    WindowInput::key_up(crate::shell::WindowKey::Shift)
                });
            }
        }
        self.feed(if raw.pressed() {
            WindowInput::key_down(raw.key())
        } else {
            WindowInput::key_up(raw.key())
        });
    }

    fn feed(&mut self, input: WindowInput) {
        if let Some(action) = self.translator.translate(input) {
            let release_edit = input.kind() == crate::shell::WindowInputKind::FocusLost
                && action == crate::control::SemanticAction::PreviewStop;
            (self.on_input)(action);
            // Focus loss releases both holds even when the translator's one
            // immediate action is preview cleanup. The shared input coordinator
            // preserves any controller that is still holding Edit.
            if release_edit {
                (self.on_input)(crate::control::SemanticAction::SetInteractionMode(
                    crate::control::InteractionMode::Navigate,
                ));
            }
        }
    }
}

/// One page-reported condition carried from the Send tauri listener context
/// onto the window's event thread, where the non-Send callbacks live.
enum PageSignal {
    /// All Rust→page event listeners are registered and can receive the
    /// first projection, meter, and MIDI-activity documents without a race.
    Ready,
    /// A painted-document ack payload from `crest://painted`.
    PaintedAck(String),
    /// A thrown page render failure payload from `crest://render-error`.
    RenderError(String),
}

/// Extracts the page's thrown render failure text from the event payload —
/// a bare JSON string stays a string, anything else is kept verbatim.
fn render_error_detail(payload: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(payload) {
        Ok(serde_json::Value::String(message)) => message,
        _ => payload.to_owned(),
    }
}

/// Records a page-reported render failure (`crest://render-error`) as the
/// typed [`WebviewShellError::PageRenderFailed`] on the window's
/// first-error slot — the same fatal runtime path a startup failure takes,
/// so the process ends nonzero (FR-006). The payload's thrown text lands
/// in `detail` verbatim: the JSON transport encoding is unwrapped, nothing
/// is parsed out of console output.
///
/// First error wins: `get_or_insert` latches the slot, so a later render
/// error — the page latches its own emission, but the shared channel
/// cannot know that — never overwrites the recorded failure. The caller
/// stops draining after the first (teardown is in flight), later signals
/// are simply dropped with the channel — the latch never blocks the
/// receiver the painted acks share. A render error is never creditable as
/// a painted ack: it is a distinct [`PageSignal`] arm that never reaches
/// the projection channel's ack forwarding.
fn record_render_failure(payload: &str, first_error: &RefCell<Option<WindowError>>) {
    first_error.borrow_mut().get_or_insert(WindowError::from(
        WebviewShellError::PageRenderFailed {
            detail: render_error_detail(payload),
        },
    ));
}

/// What one close-once-with-retry sequence decided about how the event loop
/// ends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CloseOutcome {
    /// A close was issued: the ordinary
    /// `CloseRequested → Destroyed → ExitRequested` teardown carries
    /// `run_return` out on its own, exactly as it always has.
    Teardown,
    /// Both attempts failed. Nothing is going to close the window, so
    /// nothing is going to end the loop either — the caller must.
    RetriesExhausted,
}

/// The synthetic close failure the debug-only forced-failure seam
/// substitutes for a real `window.close()`, or `None` when the shell must
/// issue the real close.
///
/// This is a test seam, not a product surface: release builds always return
/// `None` because [`CLOSE_FAILURE_OVERRIDE_ENV`], the environment read, and
/// the substitution are compiled out (`cfg(debug_assertions)`), mirroring
/// the `CREST_WEBVIEW_PAGE` page override. The acceptance harness arms it
/// around a single live run to prove FR-001: with both closes failing the
/// window never goes away, so the loop can only end through the exit edge
/// in [`close_window_once_with_retry`].
fn forced_close_failure() -> Option<tauri::Error> {
    #[cfg(debug_assertions)]
    if std::env::var_os(CLOSE_FAILURE_OVERRIDE_ENV).is_some() {
        return Some(tauri::Error::Anyhow(anyhow::anyhow!(
            "forced close failure ({CLOSE_FAILURE_OVERRIDE_ENV})"
        )));
    }
    None
}

/// Runs the close-once-with-retry sequence against `attempt` and reports
/// what it decided.
///
/// Exactly two attempts on the failure path — the close and one retry, never
/// a bounded loop and never a backoff. A second failure is recorded as a
/// typed [`WebviewShellError::WindowClose`] carrying the failure verbatim,
/// through `get_or_insert`, so an error already on the slot still wins: the
/// `PageSignal::RenderError` arm records its `PageRenderFailed` and only
/// then asks the window to close, and the operator must be told the page
/// render failed — the close failure is a consequence, not the cause.
///
/// Split out from [`close_window_once_with_retry`] so the exhausted-retry
/// decision and the latch ordering are provable without a tauri runtime.
fn close_with_retry(
    mut attempt: impl FnMut() -> Result<(), tauri::Error>,
    first_error: &RefCell<Option<WindowError>>,
) -> CloseOutcome {
    if attempt().is_ok() {
        return CloseOutcome::Teardown;
    }
    match attempt() {
        Ok(()) => CloseOutcome::Teardown,
        Err(retry_failure) => {
            first_error.borrow_mut().get_or_insert(WindowError::from(
                WebviewShellError::WindowClose(retry_failure),
            ));
            CloseOutcome::RetriesExhausted
        }
    }
}

/// Closes the single webview window, retrying a failed close once, and ends
/// the event loop itself when both attempts fail.
///
/// A second failure is recorded as a typed
/// [`WebviewShellError::WindowClose`] on the first-error slot so the
/// shutdown path surfaces it rather than swallowing it (foundation RISK-2)
/// — and then [`tauri::AppHandle::exit`] requests loop termination, because
/// nothing else will. Recording alone left `run_return` waiting on a close
/// that was never going to happen, so a correctly recorded fatal error never
/// reached the operator (mission shell-hygiene RISK-3, FR-001).
///
/// The exit request reports nothing itself: the recorded first error is
/// surfaced by the existing post-`run_return` path, unchanged and
/// un-duplicated, which is why the requested code is
/// [`REQUESTED_EXIT_CODE`]. The edge lives here rather than at the call
/// sites so every caller of this function gets it — all four arms of the
/// event-loop callback close through exactly this helper.
///
/// Shutdown ordering is unchanged wherever a close succeeds: a first close
/// or a retry that succeeds still runs the ordinary
/// `CloseRequested → Destroyed → ExitRequested` teardown, still records
/// nothing, and still requests no exit.
fn close_window_once_with_retry(
    handle: &tauri::AppHandle,
    first_error: &RefCell<Option<WindowError>>,
) {
    let Some(window) = handle.get_webview_window(WINDOW_LABEL) else {
        // Already gone: the normal Destroyed → ExitRequested teardown is in
        // flight; there is nothing left to close.
        return;
    };
    let outcome = close_with_retry(
        || match forced_close_failure() {
            Some(forced) => Err(forced),
            None => window.close(),
        },
        first_error,
    );
    if outcome == CloseOutcome::RetriesExhausted {
        handle.exit(REQUESTED_EXIT_CODE);
    }
}

impl AppWindow for TauriWebviewWindow {
    fn session_dialog_port(&self) -> Box<dyn SessionDialogPort> {
        Box::new(self.dialogs.clone())
    }

    fn run(
        &self,
        on_input: AppInputCallback,
        projection: ProjectionCallback,
        audio_observation: AudioObservationCallback,
        midi_activity: MidiActivityObservationCallback,
        mut on_session_command: SessionCommandCallback,
        document_projection: SessionDocumentProjectionCallback,
        mut on_tick: TickCallback,
        mut on_frame: FrameObservationCallback,
    ) -> Result<(), WindowError> {
        // Page resolution comes first: an unloadable page is a typed error
        // before any runtime, monitor, or window exists (FR-007). The
        // debug-only override lives inside `resolve_page_source`; release
        // builds serve the embedded page unconditionally.
        let page = resolve_page_source().map_err(WindowError::from)?;
        let host = self.host.clone();
        // `ShellFrameObservation` is post-paint evidence: the page paints
        // the five declared shell regions and echoes each rendered
        // document's semantic identity with measured region bounds and
        // visible labels on the `crest://painted` event. This window owns
        // that forwarding (mission webview-shell-cutover WP02, the
        // foundation's DRIFT-4/RISK-1 successor): the listener below relays
        // each ack onto the event thread, where the projection channel
        // correlates it against the in-flight pushed document and constructs
        // exactly one observation per painted document for `on_frame` and
        // the qualifying-frame stream. An ack no pushed document backs is a
        // typed rejection — pre-paint expectations alone can never construct
        // an observation, which is what the port invariant demands.
        let frames = self.frames.clone();

        let pipeline = Rc::new(RefCell::new(KeyPipeline {
            translator: KeyboardInputTranslator::new().with_host_navigation(host.is_some()),
            on_input,
        }));

        let app = tauri::Builder::default()
            .register_uri_scheme_protocol("crest", move |_context, request| {
                protocol_response(request.uri().path(), &page)
            })
            .build(crate::shell::webview::tauri_context())
            .map_err(|error| WindowError::from(WebviewShellError::RuntimeUnavailable(error)))?;

        let (host_sender, host_events) =
            mpsc::sync_channel::<super::host_presentation::WorkspaceEvent>(256);
        if host.is_some() {
            app.listen_any("crest://host-input", move |event| {
                if event.payload().len() <= 65536 {
                    if let Ok(value) = serde_json::from_str(event.payload()) {
                        let _ = host_sender.try_send(value);
                    }
                }
            });
        }
        let product_title = self.title.clone();
        let menu_specs = native_file_menu_items();
        let menu_new = tauri::menu::MenuItem::with_id(
            &app,
            menu_specs[0].id(),
            menu_specs[0].label(),
            true,
            Some(menu_specs[0].accelerator()),
        )
        .map_err(|error| WindowError::new(format!("File menu New item failed: {error}")))?;
        let menu_open = tauri::menu::MenuItem::with_id(
            &app,
            menu_specs[1].id(),
            menu_specs[1].label(),
            true,
            Some(menu_specs[1].accelerator()),
        )
        .map_err(|error| WindowError::new(format!("File menu Open item failed: {error}")))?;
        let menu_save = tauri::menu::MenuItem::with_id(
            &app,
            menu_specs[2].id(),
            menu_specs[2].label(),
            true,
            Some(menu_specs[2].accelerator()),
        )
        .map_err(|error| WindowError::new(format!("File menu Save item failed: {error}")))?;
        let menu_save_as = tauri::menu::MenuItem::with_id(
            &app,
            menu_specs[3].id(),
            menu_specs[3].label(),
            true,
            Some(menu_specs[3].accelerator()),
        )
        .map_err(|error| WindowError::new(format!("File menu Save As item failed: {error}")))?;
        let menu_close = tauri::menu::MenuItem::with_id(
            &app,
            menu_specs[4].id(),
            menu_specs[4].label(),
            true,
            Some(menu_specs[4].accelerator()),
        )
        .map_err(|error| WindowError::new(format!("File menu Close item failed: {error}")))?;
        let file_menu = tauri::menu::Submenu::with_items(
            &app,
            "File",
            true,
            &[
                &menu_new,
                &menu_open,
                &menu_save,
                &menu_save_as,
                &menu_close,
            ],
        )
        .map_err(|error| WindowError::new(format!("File menu creation failed: {error}")))?;
        // WKWebView relies on AppKit's responder-chain menu commands for
        // Command+A/C/X/V. A File-only menu leaves text shortcuts unavailable.
        let edit_menu = tauri::menu::SubmenuBuilder::new(&app, "Edit")
            .cut()
            .copy()
            .paste()
            .separator()
            .select_all()
            .build()
            .map_err(|error| WindowError::new(format!("Edit menu creation failed: {error}")))?;
        let menu =
            tauri::menu::Menu::with_items(&app, &[&file_menu, &edit_menu]).map_err(|error| {
                WindowError::new(format!("application menu creation failed: {error}"))
            })?;
        app.set_menu(menu).map_err(|error| {
            WindowError::new(format!("application menu install failed: {error}"))
        })?;

        let (session_command_sender, session_commands) = mpsc::channel::<SessionCommand>();
        app.on_menu_event(move |_handle, event| {
            if let Some(command) = session_command_for_menu_id(event.id().as_ref()) {
                let _ = session_command_sender.send(command);
            }
        });

        // Page→Rust signals: transport readiness, painted acks, and thrown
        // render failures arrive
        // on tauri events in a Send listener context; relay them through a
        // channel onto the event thread below, where the non-Send frame
        // callback and projection channel live.
        let (page_signal_sender, page_signals) = mpsc::channel::<PageSignal>();
        let ready_sender = page_signal_sender.clone();
        let (send_name_sender, send_name_events) = mpsc::channel::<crate::control::SendAction>();
        app.listen_any("crest://send-name", move |event| {
            if let Ok(
                action @ (crate::control::SendAction::Rename { .. }
                | crate::control::SendAction::CancelRename),
            ) = serde_json::from_str::<crate::control::SendAction>(event.payload())
            {
                let _ = send_name_sender.send(action);
            }
        });
        app.listen_any(READY_EVENT, move |_| {
            let _ = ready_sender.send(PageSignal::Ready);
        });
        let painted_sender = page_signal_sender.clone();
        app.listen_any(PAINTED_EVENT, move |event| {
            let _ = painted_sender.send(PageSignal::PaintedAck(event.payload().to_owned()));
        });
        let render_error_sender = page_signal_sender;
        app.listen_any(RENDER_ERROR_EVENT, move |event| {
            let _ = render_error_sender.send(PageSignal::RenderError(event.payload().to_owned()));
        });

        let url: tauri::Url = "crest://localhost/index.html"
            .parse()
            .expect("the static page url is well-formed");
        let authored = RepresentativeViewport::WideReference.fixture();
        let smallest = RepresentativeViewport::Compact.fixture();
        let window = WebviewWindowBuilder::new(&app, WINDOW_LABEL, WebviewUrl::CustomProtocol(url))
            .title(&self.title)
            .inner_size(f64::from(authored.width_px), f64::from(authored.height_px))
            .min_inner_size(f64::from(smallest.width_px), f64::from(smallest.height_px))
            // A synth keeps running when its editor is behind another window.
            // WebKit's default hidden-view suspension would stop the page's
            // requestAnimationFrame paint acknowledgements and can unload the
            // listener-bearing document. Keep the bounded projection/meter
            // transports alive; the audio callback remains wholly separate.
            .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
            .focused(true)
            .build()
            .map_err(|error| WindowError::from(WebviewShellError::WindowCreation(error)))?;
        let _ = window.set_focus();

        // Native capture runs on the event thread before WebKit dispatch:
        // an AppKit monitor on macOS and a GTK capture controller on Linux.
        // The page owns no keyboard handling or product state.
        let text_entry_active = Rc::new(std::cell::Cell::new(false));
        let capture_text_entry = Rc::clone(&text_entry_active);
        let capture_pipeline = Rc::clone(&pipeline);
        let _capture_handle = input_capture::install_for_window(&window, move |raw| {
            if capture_text_entry.get() {
                // Retire held performance keys without dispatching text edits.
                capture_pipeline
                    .borrow_mut()
                    .feed(WindowInput::focus_lost());
                return false;
            }
            capture_pipeline.borrow_mut().feed_raw(raw);
            raw.key() != crate::shell::WindowKey::Other
        })
        .map_err(|error| WindowError::from(WebviewShellError::InputCapture(error)))?;

        // The tao loop waits when idle; a detached waker keeps control-side
        // ticks flowing (fixture MIDI, structural advance, device status) at
        // the retired idle cadence.
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

        let mut last_tick = Instant::now();
        let mut close_requested = false;
        let mut page_ready = false;
        let dialogs = self.dialogs.clone();
        let mut last_document_title = None::<String>;
        let mut last_page_document = None::<SessionDocumentProjection>;
        let mut last_host_document = None::<super::host_presentation::HostView>;
        let mut synth_was_visible = host.is_none();

        // The first runtime failure while the window still lives — a
        // transport emit, a rejected painted ack, a page render failure, or
        // a failed close — surfaced after the loop through the window's
        // declared error path: the same record-first-error-and-close-once
        // treatment the retired native adapter gave an invalid late frame.
        let runtime_error: Rc<RefCell<Option<WindowError>>> = Rc::new(RefCell::new(None));
        let loop_runtime_error = Rc::clone(&runtime_error);
        let mut projection_channel = ProjectionChannel::new();
        let mut meter_channel = MeterChannel::new();
        let mut midi_activity_channel =
            crate::shell::webview::midi_activity_channel::MidiActivityChannel::new();

        let loop_pipeline = Rc::clone(&pipeline);
        let exit_code = app.run_return(move |handle, event| match event {
            RunEvent::WindowEvent {
                event: WindowEvent::CloseRequested { api, .. },
                ..
            } => {
                if !close_requested && !on_session_command(SessionCommand::Close) {
                    api.prevent_close();
                } else {
                    close_requested = true;
                }
            }
            RunEvent::WindowEvent {
                event: WindowEvent::Focused(false),
                ..
            } => {
                loop_pipeline.borrow_mut().feed(WindowInput::focus_lost());
            }
            RunEvent::WindowEvent {
                event: WindowEvent::Destroyed,
                ..
            } => {
                // Teardown: the webview is gone. Stop ticking and pushing so
                // a late transport emit cannot turn a clean shutdown into a
                // fatal error (WP03 tolerated-teardown rule).
                close_requested = true;
            }
            RunEvent::MainEventsCleared => {
                if close_requested {
                    return;
                }
                // WP02 T005/T008: drain the page's relayed signals on the
                // event thread. Each painted ack becomes exactly one
                // forwarded observation; a rejected ack or a thrown page
                // render failure is a typed fatal runtime error on the same
                // path a startup failure uses — record first, close once
                // (with retry), surface after the loop.
                while let Ok(signal) = page_signals.try_recv() {
                    match signal {
                        PageSignal::Ready => {
                            page_ready = true;
                        }
                        PageSignal::PaintedAck(payload) => {
                            match projection_channel.forward_ack(&payload) {
                                Ok(ForwardedAck::Observation(observation)) => {
                                    // The same control-side seam the retired
                                    // adapter presents: the port's frame
                                    // callback credits live reports, and the
                                    // qualifying-frame stream answers
                                    // "generation N painted?" from recorded
                                    // evidence when a scene polls it, instead
                                    // of the scene sleeping (T006).
                                    frames.record(observation.clone());
                                    on_frame(observation);
                                }
                                Ok(ForwardedAck::SupersededLate { .. }) => {
                                    // A lost frame: the paint happened but
                                    // its document was superseded out of the
                                    // bounded tracker before the ack landed.
                                    // Display evidence degrades only —
                                    // nothing is forwarded, nothing fails.
                                }
                                Err(error) => {
                                    loop_runtime_error
                                        .borrow_mut()
                                        .get_or_insert(WindowError::from(error));
                                    close_requested = true;
                                    close_window_once_with_retry(handle, &loop_runtime_error);
                                    return;
                                }
                            }
                        }
                        PageSignal::RenderError(payload) => {
                            record_render_failure(&payload, &loop_runtime_error);
                            close_requested = true;
                            close_window_once_with_retry(handle, &loop_runtime_error);
                            return;
                        }
                    }
                }
                if let Some(host) = &host {
                    for _ in 0..256 {
                        let Ok(event) = host_events.try_recv() else {
                            break;
                        };
                        (host.event)(event);
                    }
                }
                while let Ok(action) = send_name_events.try_recv() {
                    (loop_pipeline.borrow_mut().on_input)(crate::control::SemanticAction::Send(
                        action,
                    ));
                }
                while let Ok(command) = session_commands.try_recv() {
                    let close_now = on_session_command(command);
                    if command == SessionCommand::Close && close_now {
                        close_window_once_with_retry(handle, &loop_runtime_error);
                        return;
                    }
                }
                if let Some(request) = dialogs.try_take_request() {
                    let result = present_native_dialog(request);
                    if dialogs.complete(result).is_err() {
                        loop_runtime_error.borrow_mut().get_or_insert_with(|| {
                            WindowError::new("native session dialog returned a stale result")
                        });
                        close_requested = true;
                        close_window_once_with_retry(handle, &loop_runtime_error);
                        return;
                    }
                }
                let now = Instant::now();
                let elapsed = now.duration_since(last_tick);
                last_tick = now;
                if on_tick(elapsed) {
                    let document = document_projection();
                    let title = native_document_title(&document)
                        .replace("— crest-synth —", &format!("— {product_title} —"));
                    if last_document_title.as_ref() != Some(&title) {
                        if let Some(window) = handle.get_webview_window(WINDOW_LABEL) {
                            if let Err(error) = window.set_title(&title) {
                                loop_runtime_error.borrow_mut().get_or_insert_with(|| {
                                    WindowError::new(format!(
                                        "document title update failed: {error}"
                                    ))
                                });
                                close_requested = true;
                                close_window_once_with_retry(handle, &loop_runtime_error);
                                return;
                            }
                        }
                        last_document_title = Some(title);
                    }
                    if !page_ready {
                        return;
                    }
                    // Document facts have a shell lifecycle independent of product
                    // generations (for example, a save can finish without an edit).
                    if last_page_document.as_ref() != Some(&document) {
                        if let Err(error) = handle.emit("crest://session-document", &document) {
                            loop_runtime_error.borrow_mut().get_or_insert_with(|| {
                                WindowError::new(format!("session document update failed: {error}"))
                            });
                            close_requested = true;
                            close_window_once_with_retry(handle, &loop_runtime_error);
                            return;
                        }
                        last_page_document = Some(document);
                    }
                    // Port invariant: each interactive frame advances the
                    // injected control-side tick and then requests the
                    // current immutable projection.
                    let host_view = host.as_ref().map(|host| (host.projection)());
                    if let Some(view) = &host_view {
                        if last_host_document.as_ref() != Some(view) {
                            if let Err(error) = handle.emit("crest://host-projection", view) {
                                loop_runtime_error.borrow_mut().get_or_insert_with(|| {
                                    WindowError::new(format!("host projection failed: {error}"))
                                });
                                close_requested = true;
                                close_window_once_with_retry(handle, &loop_runtime_error);
                                return;
                            }
                            last_host_document = Some(view.clone());
                        }
                    }
                    let synth_visible = host_view.as_ref().is_none_or(|view| view.synth_visible());
                    if synth_visible && !synth_was_visible {
                        projection_channel = ProjectionChannel::new();
                    }
                    synth_was_visible = synth_visible;
                    let projection_for_page = projection();
                    text_entry_active.set(
                        host_view.as_ref().is_some_and(|view| view.text_entry())
                            || projection_for_page.semantic_model().surfaces().iter().any(
                                |surface| {
                                    matches!(
                                        surface.summary(),
                                        crate::control::SemanticSurfaceSummary::Sends {
                                            editing_name: true,
                                            ..
                                        }
                                    )
                                },
                            ),
                    );
                    // WP03 projection transport: generation-gated push of
                    // the serde serialization of this projection's embedded
                    // SemanticGraphicalViewModel. A failure while the window
                    // lives is typed: record the first, close once, surface
                    // it after the loop.
                    if let Err(error) = if synth_visible {
                        projection_channel
                            .push(&projection_for_page, |document| {
                                handle.emit(PROJECTION_EVENT, document)
                            })
                            .map(|_| ())
                    } else {
                        Ok(())
                    } {
                        loop_runtime_error
                            .borrow_mut()
                            .get_or_insert(WindowError::from(error));
                        close_requested = true;
                        close_window_once_with_retry(handle, &loop_runtime_error);
                        return;
                    }
                    // WP03 meter transport: latest-value snapshot, coalesced
                    // to the declared 30 Hz pace; a lost frame is display-only
                    // degradation (meter_channel module docs), never fatal.
                    meter_channel.observe(audio_observation());
                    let _ = meter_channel.emit_if_due(now, |frame| handle.emit(METER_EVENT, frame));
                    midi_activity_channel.observe(midi_activity());
                    let _ = midi_activity_channel.emit_if_due(now, |frame| {
                        handle.emit(
                            crate::shell::webview::midi_activity_channel::MIDI_ACTIVITY_EVENT,
                            frame,
                        )
                    });
                } else {
                    // Port invariant, verbatim: "a false tick result closes
                    // the disposable window only after application control
                    // ownership has retained a terminal outcome: either the
                    // completed live report or a typed fatal runtime error."
                    // The tick callback is that control ownership; on false
                    // this window only closes, mechanically, exactly as the
                    // retired adapter's request_close_once did — except that
                    // a close that fails twice is recorded typed rather
                    // than swallowed (T008).
                    close_requested = true;
                    // The normal owned path:
                    // CloseRequested -> Destroyed -> ExitRequested.
                    close_window_once_with_retry(handle, &loop_runtime_error);
                }
            }
            _ => {}
        });

        stop_waker.store(true, Ordering::Relaxed);
        let _ = waker.join();
        drop(pipeline);
        if let Some(error) = runtime_error.borrow_mut().take() {
            return Err(error);
        }
        if exit_code == 0 {
            Ok(())
        } else {
            Err(WindowError::new(format!(
                "tauri event loop exited with code {exit_code}"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        native_document_title, native_file_menu_items, page_asset, protocol_response,
        session_command_for_menu_id, TauriWebviewWindow, MENU_CLOSE, MENU_NEW, MENU_OPEN,
        MENU_SAVE, MENU_SAVE_AS, PAGE_CSP, PAGE_CSS, PAGE_INDEX_HTML, PAGE_JS, PAGE_TOKENS_CSS,
    };

    #[test]
    fn native_modifier_snapshots_recover_missing_edges_and_suppress_chord_repeats() {
        use crate::control::{Direction, SemanticAction};
        use crate::shell::webview::input_capture::RawKeyEvent;
        use crate::shell::{KeyboardInputTranslator, WindowInput, WindowKey};
        use std::{cell::RefCell, rc::Rc};
        let actions = Rc::new(RefCell::new(Vec::new()));
        let observed = actions.clone();
        let mut pipeline = super::KeyPipeline {
            translator: KeyboardInputTranslator::new(),
            on_input: Box::new(move |action| observed.borrow_mut().push(action)),
        };
        pipeline.feed_raw(RawKeyEvent::new(WindowKey::W, true, false).with_shift(true));
        assert_eq!(
            *actions.borrow(),
            vec![SemanticAction::NavigatePage(Direction::Up)]
        );
        pipeline.feed_raw(RawKeyEvent::new(WindowKey::W, false, false).with_shift(true));
        // The next chord needs no new FlagsChanged edge.
        actions.borrow_mut().clear();
        pipeline.feed_raw(RawKeyEvent::new(WindowKey::D, true, false).with_shift(true));
        pipeline.feed_raw(RawKeyEvent::new(WindowKey::D, true, true).with_shift(true));
        pipeline.feed_raw(RawKeyEvent::new(WindowKey::D, false, false).with_shift(true));
        assert_eq!(
            *actions.borrow(),
            vec![SemanticAction::NavigatePage(Direction::Right)]
        );
        pipeline.feed(WindowInput::focus_lost());
        actions.borrow_mut().clear();
        pipeline.feed_raw(RawKeyEvent::new(WindowKey::W, true, false).with_shift(true));
        pipeline.feed_raw(RawKeyEvent::new(WindowKey::W, false, false).with_shift(true));
        // A missing release edge must not leave the next bare key shifted.
        pipeline.feed_raw(RawKeyEvent::new(WindowKey::S, true, false).with_shift(false));
        assert_eq!(
            *actions.borrow(),
            vec![
                SemanticAction::NavigatePage(Direction::Up),
                SemanticAction::Navigate(Direction::Down),
            ]
        );
    }

    #[test]
    fn focus_loss_releases_preview_and_edit_through_the_native_input_pipeline() {
        use crate::control::{InteractionMode, SemanticAction};
        use crate::shell::{KeyboardInputTranslator, WindowInput, WindowKey};
        use std::{cell::RefCell, rc::Rc};
        let actions = Rc::new(RefCell::new(Vec::new()));
        let observed = actions.clone();
        let mut pipeline = super::KeyPipeline {
            translator: KeyboardInputTranslator::new(),
            on_input: Box::new(move |action| observed.borrow_mut().push(action)),
        };
        pipeline.feed(WindowInput::key_down(WindowKey::K));
        pipeline.feed(WindowInput::key_down(WindowKey::Space));
        actions.borrow_mut().clear();
        pipeline.feed(WindowInput::focus_lost());
        assert_eq!(
            *actions.borrow(),
            vec![
                SemanticAction::PreviewStop,
                SemanticAction::SetInteractionMode(InteractionMode::Navigate)
            ]
        );
        actions.borrow_mut().clear();
        pipeline.feed(WindowInput::key_down(WindowKey::D));
        assert_eq!(
            *actions.borrow(),
            vec![SemanticAction::Navigate(crate::control::Direction::Right)]
        );
    }

    #[test]
    fn file_menu_ids_normalize_once_to_host_neutral_commands() {
        use crate::shell::app_window::SessionCommand;

        assert_eq!(
            session_command_for_menu_id(MENU_NEW),
            Some(SessionCommand::New)
        );
        assert_eq!(
            session_command_for_menu_id(MENU_OPEN),
            Some(SessionCommand::Open)
        );
        assert_eq!(
            session_command_for_menu_id(MENU_SAVE),
            Some(SessionCommand::Save)
        );
        assert_eq!(
            session_command_for_menu_id(MENU_SAVE_AS),
            Some(SessionCommand::SaveAs)
        );
        assert_eq!(
            session_command_for_menu_id(MENU_CLOSE),
            Some(SessionCommand::Close)
        );
        assert_eq!(session_command_for_menu_id("unrelated"), None);
        assert_eq!(native_file_menu_items().len(), 5);
        assert_eq!(
            native_file_menu_items()
                .iter()
                .map(|item| item.accelerator())
                .collect::<Vec<_>>(),
            [
                "CmdOrCtrl+N",
                "CmdOrCtrl+O",
                "CmdOrCtrl+S",
                "CmdOrCtrl+Shift+S",
                "CmdOrCtrl+W",
            ]
        );
        assert!(native_file_menu_items().iter().all(|item| {
            session_command_for_menu_id(item.id()) == Some(item.command())
                && !item.label().is_empty()
        }));
    }

    #[test]
    fn native_title_carries_name_dirty_shape_status_and_failure_text() {
        use crate::shell::{SessionDocumentMarker, SessionDocumentProjection};

        let busy = SessionDocumentProjection::new(
            "Song.crest",
            true,
            SessionDocumentMarker::Busy,
            Some("OPEN".to_owned()),
            "VALIDATING AND PREPARING SESSION",
            None,
        );
        assert_eq!(
            native_document_title(&busy),
            "Song.crest * [WORKING] — crest-synth — VALIDATING AND PREPARING SESSION"
        );
        let failed = SessionDocumentProjection::new(
            "Song.crest",
            true,
            SessionDocumentMarker::Error,
            None,
            "FAILED",
            Some("Open failed during decode".to_owned()),
        );
        assert!(native_document_title(&failed).contains("[ERROR]"));
        assert!(native_document_title(&failed).contains("Open failed during decode"));
    }

    #[test]
    fn webview_window_default_uses_the_product_name() {
        assert_eq!(TauriWebviewWindow::default().title(), "crest-synth");
    }

    #[test]
    fn clones_of_one_window_share_one_qualifying_frame_stream() {
        // The WP03 scene hosting takes the seam handle before `run`; the
        // window's own clone (moved into the event loop) must feed the same
        // stream the control side polls.
        use crate::control::TopLevelContext;
        use crate::shell::webview::frame_stream::FrameExpectation;
        use crate::shell::{
            ShellFrameObservation, ShellRegionId, ShellRegionObservation, ShellRegionRect,
        };

        let window = TauriWebviewWindow::default();
        let control_side = window.frame_stream();
        let observation = ShellFrameObservation::try_new(
            1280.0,
            800.0,
            3,
            "state-3",
            TopLevelContext::Mixer,
            [
                ShellRegionObservation::new(
                    ShellRegionId::ContextLine,
                    ShellRegionRect::new(0.0, 0.0, 1280.0, 48.0),
                    "CREST SYNTH",
                ),
                ShellRegionObservation::new(
                    ShellRegionId::IdentityHeader,
                    ShellRegionRect::new(0.0, 48.0, 1280.0, 120.0),
                    "MIXER",
                ),
                ShellRegionObservation::new(
                    ShellRegionId::MainWorkspace,
                    ShellRegionRect::new(0.0, 120.0, 960.0, 736.0),
                    "MIXER WORKSPACE",
                ),
                ShellRegionObservation::new(
                    ShellRegionId::PersistentSideRegion,
                    ShellRegionRect::new(960.0, 120.0, 1280.0, 736.0),
                    "INSPECTOR",
                ),
                ShellRegionObservation::new(
                    ShellRegionId::Footer,
                    ShellRegionRect::new(0.0, 736.0, 1280.0, 800.0),
                    "MIXER",
                ),
            ],
        )
        .expect("the window test fixture observation is coherent");
        let expectation = FrameExpectation::new(
            observation.generation(),
            observation.state_hash(),
            observation.context(),
            observation.active_surface(),
        );

        window.clone().frame_stream().record(observation);
        assert!(control_side.poll(&expectation).is_some());
    }

    #[test]
    fn page_resolution_serves_the_projection_page_without_an_override() {
        let page = PAGE_INDEX_HTML;
        // The five declared shell bands as semantic containers, with the
        // generated token table and the split page assets linked in.
        for marker in [
            "data-band=\"contextLine\"",
            "data-band=\"identityHeader\"",
            "data-band=\"workspace\"",
            "data-band=\"inspector\"",
            "data-band=\"footer\"",
            "tokens.css",
            "page.css",
            "page.js",
        ] {
            assert!(page.contains(marker), "index.html must carry {marker}");
        }
        // Performance keys stay Rust-side. The focused name input owns only
        // text editing and Escape cancellation while native capture is suspended.
        assert!(!page.contains("keydown") && !page.contains("keyup"));
        assert_eq!(PAGE_JS.matches("addEventListener(\"keydown\"").count(), 1);
        assert!(PAGE_JS.contains("input.addEventListener(\"keydown\""));
        assert!(!PAGE_JS.contains("addEventListener(\"keyup\""));
        assert!(PAGE_JS.contains("var projectionListener = tauri.event.listen("));
        assert!(PAGE_JS.contains("var meterListener = tauri.event.listen("));
        assert!(PAGE_JS.contains("var midiActivityListener = tauri.event.listen("));
        assert!(PAGE_JS.contains("MIDI_ACTIVITY_EVENT"));
        assert!(PAGE_JS.contains("var sessionDocumentListener = tauri.event.listen("));
        assert!(PAGE_JS
            .contains("Promise.all([projectionListener, meterListener, midiActivityListener, sessionDocumentListener, workspaceListener])"));
        assert!(PAGE_JS.contains("tauri.event.emit(READY_EVENT"));
    }

    /// Every asset the page references resolves over the protocol with its
    /// declared content type; anything else is a 404, not a fallback page.
    #[test]
    fn the_protocol_serves_each_declared_page_asset_and_nothing_else() {
        let index = PAGE_INDEX_HTML.to_owned();
        for (path, expected_type) in [
            ("/", "text/html; charset=utf-8"),
            ("/index.html", "text/html; charset=utf-8"),
            ("/tokens.css", "text/css; charset=utf-8"),
            ("/page.css", "text/css; charset=utf-8"),
            ("/page.js", "text/javascript; charset=utf-8"),
            ("/fonts/AzeretMono-Regular.ttf", "font/ttf"),
            ("/fonts/AzeretMono-Medium.ttf", "font/ttf"),
            ("/fonts/AzeretMono-SemiBold.ttf", "font/ttf"),
            ("/fonts/AzeretMono-Bold.ttf", "font/ttf"),
        ] {
            let (content_type, body) =
                page_asset(path, &index).unwrap_or_else(|| panic!("{path} must resolve"));
            assert_eq!(content_type, expected_type, "{path}");
            assert!(!body.is_empty(), "{path} must carry its embedded bytes");
        }
        assert!(page_asset("/unknown.css", &index).is_none());
        assert!(page_asset("/../Cargo.toml", &index).is_none());
    }

    /// The embedded token table is the committed generated file, so the page
    /// styles from the authored vocabulary rather than a drifted copy.
    #[test]
    fn the_embedded_token_table_matches_the_generated_vocabulary() {
        crate::shell::webview::token_export::committed_tokens_are_fresh(PAGE_TOKENS_CSS)
            .expect("the embedded tokens.css must match the authored vocabulary");
    }

    /// The served document carries the restrictive CSP; subresources need
    /// none (the document's policy governs them).
    #[test]
    fn the_document_response_carries_the_restrictive_csp() {
        for document_path in ["/", "/index.html"] {
            let response = protocol_response(document_path, PAGE_INDEX_HTML);
            let csp = response
                .headers()
                .get("Content-Security-Policy")
                .unwrap_or_else(|| panic!("{document_path} must carry the CSP header"))
                .to_str()
                .expect("the CSP header is ascii");
            assert_eq!(csp, PAGE_CSP);
        }
        let script = protocol_response("/page.js", PAGE_INDEX_HTML);
        assert!(script.headers().get("Content-Security-Policy").is_none());
        assert_eq!(
            protocol_response("/unknown.css", PAGE_INDEX_HTML).status(),
            404
        );
    }

    /// The CSP is deny-by-default and allows exactly the asset classes the
    /// shipped page references from its own `crest://` origin — nothing
    /// inline, nothing evaluated, no remote host. The single non-`'self'`
    /// source is the documented tauri IPC loopback. Hardening is pinned at
    /// directive level (mission webview-render-fidelity-hardening WP02
    /// T009, NFR-001): a future ADDITIVE hardening passes, any weakening
    /// of any directive fails.
    #[test]
    fn the_csp_allows_exactly_what_the_shipped_page_needs() {
        assert!(PAGE_CSP.starts_with("default-src 'none'"));
        // What the page actually references: linked stylesheets, one linked
        // render script, and fonts pulled by the stylesheet.
        assert!(PAGE_INDEX_HTML.contains("<link rel=\"stylesheet\""));
        assert!(PAGE_INDEX_HTML.contains("<script src="));
        assert!(PAGE_CSS.contains("url(\"fonts/"));
        for (directive, need) in [
            ("style-src 'self'", "linked stylesheets"),
            ("script-src 'self'", "the linked render script"),
            ("font-src 'self'", "the vendored faces"),
        ] {
            assert!(PAGE_CSP.contains(directive), "CSP must allow {need}");
        }
        // The hardened denials (NFR-001): `base-uri` and `form-action` do
        // not inherit from `default-src`, so the policy must deny both
        // explicitly.
        for directive in ["base-uri 'none'", "form-action 'none'"] {
            assert!(PAGE_CSP.contains(directive), "CSP must pin {directive}");
        }
        // Nothing looser, checked per directive (C-004): no directive
        // anywhere may carry 'unsafe-inline', 'unsafe-eval', a wildcard
        // source, or a remote scheme source. The page document itself
        // inlines nothing (its styles and script are linked; dynamic
        // geometry goes through CSSOM), so nothing needs 'unsafe-inline'.
        for directive in PAGE_CSP.split(';').map(str::trim) {
            assert!(
                !directive.contains("unsafe-inline"),
                "directive {directive:?} must not allow inline sources"
            );
            assert!(
                !directive.contains("unsafe-eval"),
                "directive {directive:?} must not allow eval"
            );
            assert!(
                !directive.contains('*'),
                "directive {directive:?} must not carry a wildcard source"
            );
            assert!(
                !directive.contains("https:"),
                "directive {directive:?} must not allow a remote scheme source"
            );
        }
        assert!(!PAGE_INDEX_HTML.contains("<style"));
        assert!(!PAGE_INDEX_HTML.contains("style=\""));
        // The one http token is the in-process tauri IPC loopback.
        assert!(PAGE_CSP.contains("connect-src ipc: http://ipc.localhost"));
        assert_eq!(PAGE_CSP.matches("http").count(), 1);
    }

    /// A page render failure relayed as `PageSignal::RenderError` is
    /// recorded typed and first-error-wins (mission
    /// webview-render-fidelity-hardening WP02 T008, FR-006): the thrown
    /// text lands in `PageRenderFailed { detail }` verbatim on the same
    /// `WindowError` slot the fatal runtime path surfaces after the loop,
    /// and a later render-error signal never overwrites the recorded
    /// failure.
    #[test]
    fn a_render_error_signal_records_the_first_typed_failure_and_latches() {
        use super::{record_render_failure, render_error_detail, PageSignal};
        use std::cell::RefCell;

        // Tauri event payloads arrive JSON-encoded, so the page's thrown
        // text is a JSON string on the wire; synthesize the signals the
        // shared channel would relay — no live webview involved.
        let signals = [
            PageSignal::RenderError("\"TypeError: renderMixer is not a function\"".to_owned()),
            PageSignal::RenderError("\"later failure that must not win\"".to_owned()),
        ];
        let first_error = RefCell::new(None);
        for signal in signals {
            match signal {
                PageSignal::RenderError(payload) => {
                    record_render_failure(&payload, &first_error);
                }
                PageSignal::PaintedAck(_) => {
                    unreachable!("a render error is never a painted ack")
                }
                PageSignal::Ready => {
                    unreachable!("the render-error fixture contains no ready signal")
                }
            }
        }
        let recorded = first_error.borrow();
        let error = recorded
            .as_ref()
            .expect("the first render error must be recorded");
        assert_eq!(
            error.message(),
            "webview page render failed: TypeError: renderMixer is not a function"
        );

        // The transport decoding is the JSON unwrap and nothing else: a
        // payload that is not a bare JSON string is kept verbatim.
        assert_eq!(
            render_error_detail("not-json plain text"),
            "not-json plain text"
        );
    }

    /// A synthetic `tauri::Error` standing in for a failed `window.close()`.
    /// The runtime dispatch failure a real close produces has no public
    /// constructor, so these tests use the transparent `Anyhow` variant the
    /// debug seam also uses: what is under test is the typed `WindowClose`
    /// payload and the decision around it, not tauri's own variant.
    fn synthetic_close_failure() -> tauri::Error {
        tauri::Error::Anyhow(anyhow::anyhow!("synthetic close failure"))
    }

    /// The close path's decision, at the seam where it is made. A close that
    /// succeeds — first attempt or retry — records nothing and leaves the
    /// ordinary `CloseRequested → Destroyed → ExitRequested` teardown to end
    /// the loop, exactly as before (FR-002). Both attempts failing is a
    /// *terminating* decision rather than a silent return, because nothing
    /// else is going to end a loop whose window refuses to close (mission
    /// shell-hygiene RISK-3, FR-001), and the second failure is still
    /// recorded as the typed `WindowClose` carrying the failure verbatim.
    #[test]
    fn a_close_that_fails_twice_terminates_and_records_the_typed_failure() {
        use super::{close_with_retry, CloseOutcome};
        use crate::shell::app_window::WindowError;
        use crate::shell::webview::WebviewShellError;
        use std::cell::{Cell, RefCell};

        // A first close that succeeds: one attempt, nothing recorded,
        // ordinary teardown.
        let attempts = Cell::new(0_u32);
        let first_error = RefCell::new(None);
        let outcome = close_with_retry(
            || {
                attempts.set(attempts.get() + 1);
                Ok(())
            },
            &first_error,
        );
        assert_eq!(outcome, CloseOutcome::Teardown);
        assert_eq!(attempts.get(), 1, "a close that succeeds is never retried");
        assert!(first_error.borrow().is_none());

        // One failure then a successful retry: exactly two attempts, still
        // nothing recorded, still the ordinary teardown.
        let attempts = Cell::new(0_u32);
        let first_error = RefCell::new(None);
        let outcome = close_with_retry(
            || {
                attempts.set(attempts.get() + 1);
                if attempts.get() == 1 {
                    Err(synthetic_close_failure())
                } else {
                    Ok(())
                }
            },
            &first_error,
        );
        assert_eq!(outcome, CloseOutcome::Teardown);
        assert_eq!(attempts.get(), 2, "a failed close is retried exactly once");
        assert!(
            first_error.borrow().is_none(),
            "a retry that succeeds records nothing"
        );

        // Both failing: still exactly two attempts — never a third — the
        // typed WindowClose recorded, and the terminating decision.
        let attempts = Cell::new(0_u32);
        let first_error = RefCell::new(None);
        let outcome = close_with_retry(
            || {
                attempts.set(attempts.get() + 1);
                Err(synthetic_close_failure())
            },
            &first_error,
        );
        assert_eq!(
            outcome,
            CloseOutcome::RetriesExhausted,
            "an exhausted retry must terminate the loop, not return quietly"
        );
        assert_eq!(attempts.get(), 2, "the retry is never turned into a loop");
        assert_eq!(
            first_error.borrow().clone(),
            Some(WindowError::from(WebviewShellError::WindowClose(
                synthetic_close_failure()
            ))),
            "the second failure is recorded as the typed WindowClose"
        );
    }

    /// First error wins across kinds, which is exactly the interleaving the
    /// `PageSignal::RenderError` arm produces: it records the page render
    /// failure and then asks the window to close, so when both closes fail
    /// the operator must still be told the page render failed — the close
    /// failure is a consequence, not the cause (FR-002). The expectation is
    /// built from the typed variants rather than a formatted literal so a
    /// latch inversion cannot hide behind matching prose.
    #[test]
    fn a_recorded_render_failure_still_wins_over_a_later_close_failure() {
        use super::{close_with_retry, record_render_failure, CloseOutcome};
        use crate::shell::app_window::WindowError;
        use crate::shell::webview::WebviewShellError;
        use std::cell::RefCell;

        let first_error = RefCell::new(None);
        record_render_failure("\"TypeError: renderMixer is not a function\"", &first_error);
        let outcome = close_with_retry(|| Err(synthetic_close_failure()), &first_error);

        // The close still failed twice, so the loop still terminates ...
        assert_eq!(outcome, CloseOutcome::RetriesExhausted);
        // ... and the latch still holds the render failure, not the close
        // failure that followed it.
        assert_eq!(
            first_error.borrow().clone(),
            Some(WindowError::from(WebviewShellError::PageRenderFailed {
                detail: "TypeError: renderMixer is not a function".to_owned(),
            })),
            "the first recorded error must win over the later close failure"
        );
        assert_ne!(
            first_error.borrow().clone(),
            Some(WindowError::from(WebviewShellError::WindowClose(
                synthetic_close_failure()
            ))),
        );
    }

    /// The debug-only forced-close-failure seam is armed by its environment
    /// variable and by nothing else, so every other debug run — the rest of
    /// this module, the acceptance harness's ordinary sections, and
    /// `make demo-live` — issues real closes.
    #[cfg(debug_assertions)]
    #[test]
    fn the_debug_seam_forces_a_close_failure_only_when_armed() {
        use super::{forced_close_failure, CLOSE_FAILURE_OVERRIDE_ENV};

        assert!(
            forced_close_failure().is_none(),
            "an unarmed seam must leave the real close in place"
        );
        std::env::set_var(CLOSE_FAILURE_OVERRIDE_ENV, "1");
        let forced = forced_close_failure().expect("the armed seam forces a close failure");
        assert!(
            forced.to_string().contains(CLOSE_FAILURE_OVERRIDE_ENV),
            "the forced failure must name the seam that produced it, got {forced}"
        );
        std::env::remove_var(CLOSE_FAILURE_OVERRIDE_ENV);
        assert!(forced_close_failure().is_none());
    }

    /// Release builds compile the forced-close-failure seam out entirely:
    /// `CLOSE_FAILURE_OVERRIDE_ENV` and the environment read exist only
    /// under `cfg(debug_assertions)` — referencing the constant from
    /// non-debug code fails the release compile, which is the compile-time
    /// assertion that the seam is absent — so an armed environment variable
    /// has no effect on a release binary and the shell can only ever issue
    /// real closes.
    #[cfg(not(debug_assertions))]
    #[test]
    fn release_builds_compile_the_forced_close_failure_seam_out() {
        std::env::set_var("CREST_WEBVIEW_FORCE_CLOSE_FAILURE", "1");
        assert!(super::forced_close_failure().is_none());
        std::env::remove_var("CREST_WEBVIEW_FORCE_CLOSE_FAILURE");
    }

    /// The debug-only deterministic init-failure trigger still works (the
    /// acceptance tests and WP01's page validation drive it): an unloadable
    /// override page is a typed `PageLoadFailed` carrying the path and the
    /// io cause, raised before any window exists.
    #[cfg(debug_assertions)]
    #[test]
    fn an_unloadable_override_page_is_a_typed_page_load_failure() {
        use crate::shell::webview::WebviewShellError;

        let missing = std::path::Path::new("/nonexistent/crest-webview-page.html");
        let error = super::read_override_page(missing)
            .expect_err("a missing override page must fail typed");
        match error {
            WebviewShellError::PageLoadFailed { page, source } => {
                assert_eq!(page, "/nonexistent/crest-webview-page.html");
                assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
            }
            other => panic!("expected PageLoadFailed, got {other:?}"),
        }
    }

    /// The debug seam serves a readable override page verbatim, keeping the
    /// deterministic page-injection path the T025-style tests rely on.
    #[cfg(debug_assertions)]
    #[test]
    fn a_readable_override_page_is_served_in_debug_builds() {
        let path = std::env::temp_dir().join("crest-webview-override-seam-test.html");
        std::fs::write(&path, "<!doctype html><title>override seam</title>")
            .expect("the temp override page writes");
        let served = super::read_override_page(&path).expect("a readable override page serves");
        assert_eq!(served, "<!doctype html><title>override seam</title>");
        let _ = std::fs::remove_file(&path);
    }

    /// Release builds compile the `CREST_WEBVIEW_PAGE` seam out entirely:
    /// `PAGE_OVERRIDE_ENV` and `read_override_page` exist only under
    /// `cfg(debug_assertions)` — referencing either from non-debug code
    /// fails the release compile, which is the compile-time assertion that
    /// the override path is absent — so release page resolution can only
    /// ever serve the embedded page.
    #[cfg(not(debug_assertions))]
    #[test]
    fn release_page_resolution_serves_only_the_embedded_page() {
        let page = super::resolve_page_source().expect("the embedded page always resolves");
        assert_eq!(page, PAGE_INDEX_HTML);
    }
}
