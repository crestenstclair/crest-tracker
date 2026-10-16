//! Native application composition without exposing Synth's state or device owners.
use crate::control::{
    ControllerBindings, ControllerButton, PhysicalMidiEvent, SemanticAction, SessionCommand,
};
use crate::real_time::audio_boundary::ControlAudioBoundary;
use crate::real_time::AudioObservationSnapshot;
use crate::shell::hosted_synth::HostedSynthControl;
use crate::shell::SessionDocumentProjection;
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub struct HostClock {
    /// Duration since the preceding shell tick, not a timestamp.
    pub elapsed: Duration,
    /// Cumulative shell control time. Physical events delivered during this
    /// tick carry this value in `control_receipt_micros`; their backend
    /// timestamps retain their backend clock domain.
    pub control_micros: u64,
    pub sample_rate: u32,
    pub audio: AudioObservationSnapshot,
    pub audio_clock: Option<crate::shell::audio_output::AudioFrameClock>,
}

#[derive(Clone, Copy, Debug)]
pub enum HostControllerInput {
    /// Normalized signed stick motion; hosts own dead zones and semantics.
    Axis {
        device: usize,
        axis: HostControllerAxis,
        value: i16,
    },
    Pressed {
        device: usize,
        button: ControllerButton,
    },
    Released {
        device: usize,
        button: ControllerButton,
    },
    Disconnected {
        device: usize,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HostControllerAxis {
    LeftX,
    LeftY,
    RightX,
    RightY,
}

/// The application owner supplies composition, navigation and its document lifecycle.
/// Synth keeps its own reducer, renderer, preparation workers and devices.
pub trait ApplicationHost<Boundary: ControlAudioBoundary>: 'static {
    fn tick(&mut self, synth: &mut HostedSynthControl<'_, Boundary>, clock: HostClock);
    fn document(&self) -> SessionDocumentProjection;
    fn session_command(
        &mut self,
        synth: &mut HostedSynthControl<'_, Boundary>,
        command: SessionCommand,
    ) -> bool;
    /// Consume actions only while the host owns focus; false preserves Synth's path.
    fn action(&mut self, _action: SemanticAction) -> bool {
        false
    }
    /// Optional semantic destination for the shared SDL translator. The handler
    /// receives the same actions as the keyboard, after saved button mappings,
    /// chord detection and hold suppression. Returning false retains Synth behavior.
    fn action_handler(&self) -> Option<std::rc::Rc<dyn Fn(SemanticAction) -> bool>> {
        None
    }

    fn controller_input(
        &mut self,
        _input: HostControllerInput,
        _bindings: &ControllerBindings,
    ) -> bool {
        false
    }
    /// Receives each accepted physical event once on the control thread,
    /// before default channel monitoring. Return true to consume monitoring;
    /// false preserves Synth's existing route. The subsequent `tick` receives
    /// the same clock and can query the active input identity through Synth.
    fn midi_input(&mut self, _event: PhysicalMidiEvent, _clock: HostClock) -> bool {
        false
    }
    /// MIDI transport shares the ordered physical ingress but has no DSP route.
    fn midi_transport(
        &mut self,
        _event: crate::control::PhysicalMidiTransportEvent,
        _clock: HostClock,
    ) {
    }
    fn owns_input(&self) -> bool {
        false
    }
    /// Completes an asynchronous host Save-and-Close or an in-page close action.
    fn take_close_request(&mut self) -> bool {
        false
    }
}
