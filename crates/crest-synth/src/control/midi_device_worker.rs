use crate::control::{
    ActiveMidiInput, ConnectMidiInput, MidiDeviceFailure, MidiInputDescriptor, MidiInputPreference,
    MidiInputScanId, PhysicalMidiIngress,
};
use core::fmt;

/// Bounded control-to-worker ownership transfer for physical MIDI work.
#[derive(Debug)]
pub enum MidiDeviceWorkerCommand {
    Scan {
        scan_id: MidiInputScanId,
    },
    Connect {
        request: ConnectMidiInput,
        ingress: PhysicalMidiIngress,
    },
    Retire {
        active: ActiveMidiInput,
    },
    LoadPreference,
    StorePreference {
        preference: MidiInputPreference,
    },
}

impl MidiDeviceWorkerCommand {
    pub const fn is_scan(&self) -> bool {
        matches!(self, Self::Scan { .. })
    }

    pub const fn is_connect(&self) -> bool {
        matches!(self, Self::Connect { .. })
    }
}

/// One worker result. Prepared/retired handle ownership always travels in the
/// value so stale control-side correlations can be retired explicitly.
#[derive(Debug)]
pub enum MidiDeviceWorkerResult {
    ScanSucceeded {
        scan_id: MidiInputScanId,
        descriptors: Vec<MidiInputDescriptor>,
    },
    ScanFailed {
        scan_id: MidiInputScanId,
        failure: MidiDeviceFailure,
    },
    ConnectionPrepared {
        request: ConnectMidiInput,
        active: ActiveMidiInput,
    },
    ConnectionFailed {
        request: ConnectMidiInput,
        failure: MidiDeviceFailure,
    },
    Retired {
        identity: crate::control::MidiInputDeviceId,
        request_id: crate::control::MidiConnectionRequestId,
        revision: crate::control::MidiConnectionRevision,
        failure: Option<MidiDeviceFailure>,
    },
    PreferenceLoaded {
        preference: Option<MidiInputPreference>,
        failure: Option<MidiDeviceFailure>,
    },
    PreferenceStored {
        preference: MidiInputPreference,
        failure: Option<MidiDeviceFailure>,
    },
}

impl MidiDeviceWorkerResult {
    pub const fn is_scan(&self) -> bool {
        matches!(self, Self::ScanSucceeded { .. } | Self::ScanFailed { .. })
    }

    pub const fn is_connect(&self) -> bool {
        matches!(
            self,
            Self::ConnectionPrepared { .. } | Self::ConnectionFailed { .. }
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MidiDeviceWorkerBusyReason {
    ScanOutstanding,
    ConnectOutstanding,
    QueueFull,
    WorkerUnavailable,
    Shutdown,
}

/// A nonblocking refusal that returns every owned command value to control.
#[must_use = "a refused MIDI worker command retains owned ingress/handle values"]
#[derive(Debug)]
pub struct MidiDeviceWorkerBusy {
    reason: MidiDeviceWorkerBusyReason,
    command: Box<MidiDeviceWorkerCommand>,
}

impl MidiDeviceWorkerBusy {
    pub fn new(reason: MidiDeviceWorkerBusyReason, command: MidiDeviceWorkerCommand) -> Self {
        Self {
            reason,
            command: Box::new(command),
        }
    }

    pub const fn reason(&self) -> MidiDeviceWorkerBusyReason {
        self.reason
    }

    pub const fn command(&self) -> &MidiDeviceWorkerCommand {
        &self.command
    }

    pub fn into_command(self) -> MidiDeviceWorkerCommand {
        *self.command
    }
}

impl fmt::Display for MidiDeviceWorkerBusy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.reason {
            MidiDeviceWorkerBusyReason::ScanOutstanding => {
                "a MIDI input scan is already outstanding"
            }
            MidiDeviceWorkerBusyReason::ConnectOutstanding => {
                "a MIDI input connection is already outstanding"
            }
            MidiDeviceWorkerBusyReason::QueueFull => "the MIDI device worker queue is full",
            MidiDeviceWorkerBusyReason::WorkerUnavailable => {
                "the MIDI device worker is unavailable"
            }
            MidiDeviceWorkerBusyReason::Shutdown => "the MIDI device worker is shut down",
        })
    }
}

impl std::error::Error for MidiDeviceWorkerBusy {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MidiDeviceWorkerShutdownError {
    ThreadPanicked,
    RetirementFailed,
}

impl fmt::Display for MidiDeviceWorkerShutdownError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ThreadPanicked => "the MIDI device worker thread panicked",
            Self::RetirementFailed => {
                "one or more MIDI input handles failed worker-side retirement during shutdown"
            }
        })
    }
}

impl std::error::Error for MidiDeviceWorkerShutdownError {}

/// Nonblocking control-facing worker capability.
pub trait MidiDeviceWorker {
    fn try_submit(&mut self, command: MidiDeviceWorkerCommand) -> Result<(), MidiDeviceWorkerBusy>;

    fn try_poll(&mut self) -> Option<MidiDeviceWorkerResult>;

    /// Transfers every still-control-owned backend handle to the worker,
    /// joins it, and reports worker-side retirement failure. This teardown
    /// operation may wait; regular control-facing operations remain bounded
    /// and nonblocking.
    fn shutdown_with_retirements_on_control(
        &mut self,
        retirements: Vec<ActiveMidiInput>,
    ) -> Result<(), MidiDeviceWorkerShutdownError>;

    fn shutdown_on_control(&mut self) -> Result<(), MidiDeviceWorkerShutdownError> {
        self.shutdown_with_retirements_on_control(Vec::new())
    }
}
