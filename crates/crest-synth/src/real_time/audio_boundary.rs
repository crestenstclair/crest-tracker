use crate::real_time::audio_command::AudioCommand;
use crate::real_time::parameter_snapshot::ParameterSnapshot;
use crate::real_time::scheduled_event::{
    ScheduledEvent, ScheduledEventEpochExhausted, ScheduledEventPoll, ScheduledEventRejected,
    ScheduledEventRejectionReason,
};
use core::fmt;

/// Returned when the bounded command queue cannot accept another command.
///
/// The rejected command is preserved so control-side code can retry or apply
/// its explicit overflow policy without reconstructing the event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoundaryFull {
    command: AudioCommand,
}

impl BoundaryFull {
    pub const fn new(command: AudioCommand) -> Self {
        Self { command }
    }

    pub const fn command(&self) -> AudioCommand {
        self.command
    }

    pub const fn into_command(self) -> AudioCommand {
        self.command
    }
}

impl fmt::Display for BoundaryFull {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the bounded audio command queue is full")
    }
}

impl std::error::Error for BoundaryFull {}

/// Control-thread operations for the real-time seam.
///
/// Implementations use the producer side of a bounded SPSC command queue, the
/// publisher side of a latest-wins snapshot transfer. Structural graph
/// ownership and retirement use `StructuralGraphBoundary` instead.
pub trait ControlAudioBoundary: Send {
    fn push_command(&mut self, command: AudioCommand) -> Result<(), BoundaryFull>;

    /// Publishes the coalesced global recovery command through its reserved
    /// transport capacity. Boundaries that do not advertise a reserve retain
    /// their legacy behavior through this default implementation.
    fn push_recovery_command(&mut self) -> Result<(), BoundaryFull> {
        self.push_command(AudioCommand::all_notes_off())
    }

    /// Input replacement/loss cleans only default live input. Hosts separately
    /// own monitor cancellation; scheduled accompaniment must remain active.
    fn push_input_recovery_command(&mut self) -> Result<(), BoundaryFull> {
        self.push_command(AudioCommand::ReleaseImmediateNotes)
    }

    /// Whether normal commands are prevented from consuming the final slot.
    fn has_recovery_reserve(&self) -> bool {
        false
    }

    fn scheduled_epoch(&self) -> u64 {
        0
    }

    fn push_scheduled(&mut self, event: ScheduledEvent) -> Result<(), ScheduledEventRejected> {
        Err(ScheduledEventRejected::new(
            event,
            ScheduledEventRejectionReason::Unsupported,
        ))
    }

    fn cancel_scheduled(&mut self) -> Result<u64, ScheduledEventEpochExhausted> {
        Ok(0)
    }

    fn publish_parameters(&mut self, parameters: ParameterSnapshot);
}

/// Hard real-time operations available to the audio callback.
///
/// Implementations must make every method allocation-free, lock-free,
/// non-blocking, and free of I/O, logging, formatting, and destruction.
pub trait AudioThreadBoundary: Send {
    fn pop_command(&mut self) -> Option<AudioCommand>;

    fn scheduled_epoch(&self) -> u64 {
        0
    }
    /// Producer-sampled occupancy peak; zero for boundaries without a schedule.
    fn scheduled_queue_high_water(&self) -> u64 {
        0
    }
    fn poll_scheduled_before(&mut self, _end_frame_exclusive: u64) -> ScheduledEventPoll {
        ScheduledEventPoll::Empty
    }

    /// Exchanges a newly published snapshot with caller-owned retirement
    /// storage. Old allocations return to the producer; the callback neither
    /// clones nor destroys snapshot storage.
    fn exchange_latest_parameters(&mut self, previous: &mut ParameterSnapshot) -> bool;
}

/// Factory contract for one complete lock-free control/audio boundary.
///
/// Splitting consumes the factory and returns narrow handles. Callback code that
/// owns only AudioHandle cannot call publish_parameters or push_command.
pub trait AudioBoundary: Send + Sized {
    type ControlHandle: ControlAudioBoundary;
    type AudioHandle: AudioThreadBoundary;

    fn into_handles(self) -> (Self::ControlHandle, Self::AudioHandle);
}

#[cfg(test)]
mod tests {
    use super::{AudioBoundary, AudioThreadBoundary, BoundaryFull, ControlAudioBoundary};
    use crate::kernel::midi_channel::MidiChannel;
    use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
    use crate::kernel::patch_id::PatchId;
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::real_time::audio_command::AudioCommand;
    use crate::real_time::parameter_snapshot::ParameterSnapshot;

    fn command() -> AudioCommand {
        AudioCommand::patch_midi(
            PatchId::new(1).unwrap(),
            MidiMessage::try_new(
                MidiChannel::new(0).unwrap(),
                MidiMessageKind::NoteOn,
                60,
                100,
            )
            .unwrap(),
        )
    }

    fn parameters(generation: u64) -> ParameterSnapshot {
        ParameterSnapshot::new(
            generation,
            GlobalParameters::new(0.0).unwrap(),
            crate::mixer::mixer_state::MixerState::default(),
            &[],
        )
        .unwrap()
    }

    struct TestControl {
        queued: Option<AudioCommand>,
        latest: ParameterSnapshot,
    }

    impl ControlAudioBoundary for TestControl {
        fn push_command(&mut self, command: AudioCommand) -> Result<(), BoundaryFull> {
            if self.queued.is_some() {
                Err(BoundaryFull::new(command))
            } else {
                self.queued = Some(command);
                Ok(())
            }
        }

        fn publish_parameters(&mut self, parameters: ParameterSnapshot) {
            self.latest = parameters;
        }
    }

    struct TestAudio {
        queued: Option<AudioCommand>,
        latest: ParameterSnapshot,
    }

    impl AudioThreadBoundary for TestAudio {
        fn pop_command(&mut self) -> Option<AudioCommand> {
            self.queued.take()
        }

        fn exchange_latest_parameters(&mut self, previous: &mut ParameterSnapshot) -> bool {
            if self.latest.generation() < previous.generation() {
                return false;
            }
            core::mem::swap(&mut self.latest, previous);
            true
        }
    }

    struct TestBoundary;

    impl AudioBoundary for TestBoundary {
        type ControlHandle = TestControl;
        type AudioHandle = TestAudio;

        fn into_handles(self) -> (Self::ControlHandle, Self::AudioHandle) {
            (
                TestControl {
                    queued: None,
                    latest: parameters(0),
                },
                TestAudio {
                    queued: Some(command()),
                    latest: parameters(1),
                },
            )
        }
    }

    #[test]
    fn boundary_full_preserves_the_rejected_command() {
        let rejected = command();
        let error = BoundaryFull::new(rejected);

        assert_eq!(error.command(), rejected);
        assert_eq!(error.into_command(), rejected);
        assert_eq!(error.to_string(), "the bounded audio command queue is full");
    }

    #[test]
    fn split_handles_expose_their_narrow_operations() {
        let (mut control, mut audio) = TestBoundary.into_handles();

        control.push_command(command()).unwrap();
        assert_eq!(
            control.push_command(AudioCommand::all_notes_off()),
            Err(BoundaryFull::new(AudioCommand::all_notes_off()))
        );
        control.publish_parameters(parameters(2));

        assert_eq!(audio.pop_command(), Some(command()));
        assert_eq!(audio.pop_command(), None);
        assert_eq!(audio.latest.generation(), 1);
        assert_eq!(control.latest.generation(), 2);
    }

    #[test]
    fn discrete_commands_are_copyable_and_need_no_destruction() {
        fn assert_copy<T: Copy>() {}

        assert_copy::<AudioCommand>();
        assert!(!core::mem::needs_drop::<AudioCommand>());
    }
}
