use crate::control::{GraphicalShellProjection, MidiActivityObservation, SemanticAction};
use crate::real_time::AudioObservationSnapshot;
use crate::shell::{
    SessionDialogPort, SessionDocumentProjection, ShellFrameObservation, UnavailableSessionDialog,
};
use core::fmt;
use std::time::Duration;

/// Semantic input emitted by a window adapter.
pub type AppInputCallback = Box<dyn FnMut(SemanticAction) + 'static>;

/// Immutable view projection requested by a window adapter.
pub type ProjectionCallback = Box<dyn Fn() -> GraphicalShellProjection + 'static>;

/// Latest immutable numeric audio observation requested by a window adapter.
pub type AudioObservationCallback = Box<dyn Fn() -> AudioObservationSnapshot + 'static>;

/// Latest compatible physical-MIDI activity requested by a window adapter.
pub type MidiActivityObservationCallback = Box<dyn Fn() -> MidiActivityObservation + 'static>;

/// Periodic control-side work requested by a window adapter.
///
/// Returning `false` asks the disposable window to close after application
/// control ownership has retained a typed runtime failure.
pub type TickCallback = Box<dyn FnMut(Duration) -> bool + 'static>;

/// Post-paint evidence emitted by a graphical window adapter.
pub type FrameObservationCallback = Box<dyn FnMut(ShellFrameObservation) + 'static>;

pub use crate::control::SessionCommand;

/// Handles one normalized document command. `Close` returns whether the
/// current native close request may proceed immediately.
pub type SessionCommandCallback = Box<dyn FnMut(SessionCommand) -> bool + 'static>;

/// Immutable path-free document presentation requested by the window.
pub type SessionDocumentProjectionCallback = Box<dyn Fn() -> SessionDocumentProjection + 'static>;

/// A failure while creating or running the application window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowError {
    message: String,
}

impl WindowError {
    /// Creates an actionable window startup or runtime error.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Returns the adapter-provided failure description.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for WindowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for WindowError {}

/// Outbound port for the disposable graphical application shell.
///
/// Implementations own raw key and modifier state. They emit only semantic
/// `SemanticAction` values, request complete immutable `GraphicalShellProjection`
/// values, report elapsed control-side time through `on_tick`, and emit only
/// post-paint `ShellFrameObservation` evidence. They never own synth
/// parameters, application selection, or accepted application state.
pub trait AppWindow {
    /// Returns the dialog port paired with this window adapter. Harness
    /// windows that do not implement native dialogs fail explicitly if a
    /// lifecycle command requests one.
    fn session_dialog_port(&self) -> Box<dyn SessionDialogPort> {
        Box::new(UnavailableSessionDialog)
    }

    /// Runs the window event loop until the player closes it.
    #[allow(clippy::too_many_arguments)]
    fn run(
        &self,
        on_input: AppInputCallback,
        projection: ProjectionCallback,
        audio_observation: AudioObservationCallback,
        midi_activity: MidiActivityObservationCallback,
        on_session_command: SessionCommandCallback,
        document_projection: SessionDocumentProjectionCallback,
        on_tick: TickCallback,
        on_frame: FrameObservationCallback,
    ) -> Result<(), WindowError>;
}

#[cfg(test)]
mod tests {
    use super::{
        AppInputCallback, AppWindow, AudioObservationCallback, FrameObservationCallback,
        MidiActivityObservationCallback, ProjectionCallback, SessionCommandCallback,
        SessionDocumentProjectionCallback, TickCallback, WindowError,
    };
    use crate::control::app_event::Direction;
    use crate::control::{
        GraphicalShellProjection, SemanticAction, SemanticGraphicalViewModel, ShellContextLine,
        ShellFooter, ShellIdentityHeader, TextProjection, TopLevelContext,
    };
    use crate::shell::{
        ShellFrameObservation, ShellRegionId, ShellRegionObservation, ShellRegionRect,
    };
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::time::Duration;

    struct TestWindow;

    impl AppWindow for TestWindow {
        fn run(
            &self,
            mut on_input: AppInputCallback,
            projection: ProjectionCallback,
            audio_observation: AudioObservationCallback,
            midi_activity: MidiActivityObservationCallback,
            _on_session_command: SessionCommandCallback,
            _document_projection: SessionDocumentProjectionCallback,
            mut on_tick: TickCallback,
            mut on_frame: FrameObservationCallback,
        ) -> Result<(), WindowError> {
            on_input(SemanticAction::Navigate(Direction::Down));
            assert!(on_tick(Duration::from_millis(16)));
            let projection = projection();
            assert_eq!(projection.workspace().diagnostic().body(), "KEYS: test");
            assert_eq!(audio_observation().sequence(), 0);
            assert_eq!(midi_activity().snapshot(), None);
            on_frame(frame_observation(&projection));
            Ok(())
        }
    }

    fn graphical_projection() -> GraphicalShellProjection {
        GraphicalShellProjection::new(
            1,
            "state-hash",
            SemanticGraphicalViewModel::fixture(1, "state-hash", TopLevelContext::Mixer),
            ShellContextLine::new("CREST SYNTH", "MIXER", "READY"),
            ShellIdentityHeader::new("MIXER · GLOBAL", "1 PATCH · MASTER OUTPUT"),
            "MIXER WORKSPACE",
            "INSPECTOR",
            TextProjection::for_context(
                TopLevelContext::Mixer,
                "KEYS: test".to_owned(),
                0,
                "state-hash".to_owned(),
            ),
            ShellFooter::new("MIXER / GLOBAL", vec!["1 MIXER".to_owned()]),
        )
        .unwrap()
    }

    fn frame_observation(projection: &GraphicalShellProjection) -> ShellFrameObservation {
        ShellFrameObservation::try_new(
            1280.0,
            800.0,
            projection.generation(),
            projection.state_hash(),
            projection.context(),
            [
                ShellRegionObservation::new(
                    ShellRegionId::ContextLine,
                    ShellRegionRect::new(0.0, 0.0, 1280.0, 48.0),
                    projection.context_line().context_label(),
                ),
                ShellRegionObservation::new(
                    ShellRegionId::IdentityHeader,
                    ShellRegionRect::new(0.0, 48.0, 1280.0, 120.0),
                    projection.identity_header().primary_label(),
                ),
                ShellRegionObservation::new(
                    ShellRegionId::MainWorkspace,
                    ShellRegionRect::new(0.0, 120.0, 960.0, 736.0),
                    projection.workspace().main_label(),
                ),
                ShellRegionObservation::new(
                    ShellRegionId::PersistentSideRegion,
                    ShellRegionRect::new(960.0, 120.0, 1280.0, 736.0),
                    projection.workspace().side_label(),
                ),
                ShellRegionObservation::new(
                    ShellRegionId::Footer,
                    ShellRegionRect::new(0.0, 736.0, 1280.0, 800.0),
                    projection.footer().path_label(),
                ),
            ],
        )
        .unwrap()
    }

    #[test]
    fn port_is_object_safe_and_routes_only_callbacks() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let projection_requests = Rc::new(Cell::new(0));
        let ticks = Rc::new(RefCell::new(Vec::new()));
        let frames = Rc::new(RefCell::new(Vec::new()));

        let events_for_callback = Rc::clone(&events);
        let on_input: AppInputCallback = Box::new(move |event| {
            events_for_callback.borrow_mut().push(event);
        });

        let requests_for_callback = Rc::clone(&projection_requests);
        let projection: ProjectionCallback = Box::new(move || {
            requests_for_callback.set(requests_for_callback.get() + 1);
            graphical_projection()
        });
        let audio_observation: AudioObservationCallback =
            Box::new(crate::real_time::AudioObservationSnapshot::default);
        let midi_activity: MidiActivityObservationCallback =
            Box::new(crate::control::MidiActivityObservation::default);
        let on_session_command: SessionCommandCallback = Box::new(|_| false);
        let document_projection: SessionDocumentProjectionCallback = Box::new(|| {
            panic!("the basic window-port test does not request document presentation")
        });

        let ticks_for_callback = Rc::clone(&ticks);
        let on_tick: TickCallback = Box::new(move |duration| {
            ticks_for_callback.borrow_mut().push(duration);
            true
        });
        let frames_for_callback = Rc::clone(&frames);
        let on_frame: FrameObservationCallback = Box::new(move |observation| {
            frames_for_callback.borrow_mut().push(observation);
        });

        let window: &dyn AppWindow = &TestWindow;
        window
            .run(
                on_input,
                projection,
                audio_observation,
                midi_activity,
                on_session_command,
                document_projection,
                on_tick,
                on_frame,
            )
            .expect("test window should run");

        assert_eq!(
            events.borrow().as_slice(),
            &[SemanticAction::Navigate(Direction::Down)]
        );
        assert_eq!(projection_requests.get(), 1);
        assert_eq!(ticks.borrow().as_slice(), &[Duration::from_millis(16)]);
        assert_eq!(frames.borrow().len(), 1);
        assert_eq!(frames.borrow()[0].context(), TopLevelContext::Mixer);
    }

    #[test]
    fn window_error_preserves_its_actionable_message() {
        let error = WindowError::new("window event loop failed");

        assert_eq!(error.message(), "window event loop failed");
        assert_eq!(error.to_string(), "window event loop failed");
    }
}
