use crate::adapter::scheduled_event_boundary::{
    ScheduledEventAudioHandle, ScheduledEventBoundary, ScheduledEventControlHandle,
};
use crate::real_time::audio_boundary::{
    AudioBoundary, AudioThreadBoundary, BoundaryFull, ControlAudioBoundary,
};
use crate::real_time::audio_command::AudioCommand;
use crate::real_time::parameter_snapshot::ParameterSnapshot;
use crate::real_time::scheduled_event::{
    ScheduledEvent, ScheduledEventEpochExhausted, ScheduledEventPoll, ScheduledEventRejected,
};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use triple_buffer::{triple_buffer, Input, Output};

const INPUT_RECOVERY: u8 = 1;
const GLOBAL_RECOVERY: u8 = 2;

#[derive(Clone, Copy)]
enum QueuedCommand {
    Command(AudioCommand),
    Recovery,
}

/// A complete lock-free control/audio seam backed by fixed-capacity primitives.
pub struct LockFreeAudioBoundary {
    control: LockFreeControlHandle,
    audio: LockFreeAudioHandle,
}

impl LockFreeAudioBoundary {
    /// Allocates all boundary storage before either handle reaches the audio callback.
    pub fn new(command_capacity: usize, initial_parameters: ParameterSnapshot) -> Self {
        assert!(
            command_capacity >= 2,
            "the audio command boundary requires one normal slot and one recovery reserve"
        );
        let (scheduled_control, scheduled_audio) = ScheduledEventBoundary::new(command_capacity)
            .expect("validated capacity")
            .into_handles();
        let (command_producer, command_consumer) = RingBuffer::new(command_capacity);
        let (parameter_input, parameter_output) = triple_buffer(&initial_parameters);
        let recovery_pending = Arc::new(AtomicU8::new(0));

        Self {
            control: LockFreeControlHandle {
                scheduled: scheduled_control,
                commands: command_producer,
                parameters: parameter_input,
                recovery_pending: Arc::clone(&recovery_pending),
            },
            audio: LockFreeAudioHandle {
                scheduled: scheduled_audio,
                commands: command_consumer,
                parameters: parameter_output,
                recovery_pending,
            },
        }
    }
}

impl AudioBoundary for LockFreeAudioBoundary {
    type ControlHandle = LockFreeControlHandle;
    type AudioHandle = LockFreeAudioHandle;

    fn into_handles(self) -> (Self::ControlHandle, Self::AudioHandle) {
        (self.control, self.audio)
    }
}

/// The control-thread half of a LockFreeAudioBoundary.
///
/// This handle owns command publication and latest scalar replacement only.
pub struct LockFreeControlHandle {
    scheduled: ScheduledEventControlHandle,
    commands: Producer<QueuedCommand>,
    parameters: Input<ParameterSnapshot>,
    recovery_pending: Arc<AtomicU8>,
}

impl LockFreeControlHandle {
    fn request_recovery(&mut self, scope: u8, command: AudioCommand) -> Result<(), BoundaryFull> {
        // One reserved marker covers both scopes. A global request upgrades a
        // pending input recovery; a later input request never weakens it.
        if self.recovery_pending.fetch_max(scope, Ordering::AcqRel) != 0 {
            return Ok(());
        }
        match self.commands.push(QueuedCommand::Recovery) {
            Ok(()) => Ok(()),
            Err(_) => {
                self.recovery_pending.store(0, Ordering::Release);
                Err(BoundaryFull::new(command))
            }
        }
    }
}

impl ControlAudioBoundary for LockFreeControlHandle {
    fn scheduled_epoch(&self) -> u64 {
        self.scheduled.epoch()
    }
    fn push_scheduled(&mut self, event: ScheduledEvent) -> Result<(), ScheduledEventRejected> {
        self.scheduled.push(event)
    }
    fn cancel_scheduled(&mut self) -> Result<u64, ScheduledEventEpochExhausted> {
        self.scheduled.cancel()
    }

    fn push_command(&mut self, command: AudioCommand) -> Result<(), BoundaryFull> {
        let reserved = usize::from(self.recovery_pending.load(Ordering::Acquire) == 0);
        if self.commands.slots() <= reserved {
            return Err(BoundaryFull::new(command));
        }
        match self.commands.push(QueuedCommand::Command(command)) {
            Ok(()) => Ok(()),
            Err(PushError::Full(_)) => Err(BoundaryFull::new(command)),
        }
    }

    fn push_recovery_command(&mut self) -> Result<(), BoundaryFull> {
        self.request_recovery(GLOBAL_RECOVERY, AudioCommand::AllNotesOff)
    }

    fn push_input_recovery_command(&mut self) -> Result<(), BoundaryFull> {
        self.request_recovery(INPUT_RECOVERY, AudioCommand::ReleaseImmediateNotes)
    }

    fn has_recovery_reserve(&self) -> bool {
        true
    }

    fn publish_parameters(&mut self, parameters: ParameterSnapshot) {
        self.parameters.write(parameters);
    }
}

/// The callback-only half of a LockFreeAudioBoundary.
///
/// Its methods only touch preallocated lock-free command or scalar storage.
pub struct LockFreeAudioHandle {
    scheduled: ScheduledEventAudioHandle,
    commands: Consumer<QueuedCommand>,
    parameters: Output<ParameterSnapshot>,
    recovery_pending: Arc<AtomicU8>,
}

impl LockFreeAudioHandle {
    /// Borrows the newest published projection for control-side witnesses.
    pub fn read_latest_parameters(&mut self) -> &ParameterSnapshot {
        self.parameters.read()
    }
}

impl AudioThreadBoundary for LockFreeAudioHandle {
    fn scheduled_queue_high_water(&self) -> u64 {
        self.scheduled.queue_high_water()
    }

    fn scheduled_epoch(&self) -> u64 {
        self.scheduled.epoch()
    }
    fn poll_scheduled_before(&mut self, end_frame_exclusive: u64) -> ScheduledEventPoll {
        self.scheduled.poll_before(end_frame_exclusive)
    }

    fn pop_command(&mut self) -> Option<AudioCommand> {
        Some(match self.commands.pop().ok()? {
            QueuedCommand::Command(command) => command,
            QueuedCommand::Recovery => {
                if self.recovery_pending.swap(0, Ordering::AcqRel) == INPUT_RECOVERY {
                    AudioCommand::ReleaseImmediateNotes
                } else {
                    AudioCommand::AllNotesOff
                }
            }
        })
    }

    fn exchange_latest_parameters(&mut self, previous: &mut ParameterSnapshot) -> bool {
        if !self.parameters.update() {
            return false;
        }
        core::mem::swap(self.parameters.output_buffer_mut(), previous);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{LockFreeAudioBoundary, LockFreeAudioHandle, LockFreeControlHandle};
    use crate::kernel::midi_channel::MidiChannel;
    use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
    use crate::kernel::patch_id::PatchId;
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::real_time::audio_boundary::{
        AudioBoundary, AudioThreadBoundary, BoundaryFull, ControlAudioBoundary,
    };
    use crate::real_time::audio_command::AudioCommand;
    use crate::real_time::parameter_snapshot::ParameterSnapshot;

    fn command(note: u8) -> AudioCommand {
        AudioCommand::patch_midi(
            PatchId::new(1).unwrap(),
            MidiMessage::try_new(
                MidiChannel::new(0).unwrap(),
                MidiMessageKind::NoteOn,
                note,
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

    #[test]
    fn bounded_commands_remain_fifo_and_return_rejected_values() {
        let boundary = LockFreeAudioBoundary::new(3, parameters(0));
        let (mut control, mut audio) = boundary.into_handles();

        control.push_command(command(60)).unwrap();
        control.push_command(command(62)).unwrap();
        assert_eq!(
            control.push_command(command(64)),
            Err(BoundaryFull::new(command(64)))
        );

        assert_eq!(audio.pop_command(), Some(command(60)));
        assert_eq!(audio.pop_command(), Some(command(62)));
        assert_eq!(audio.pop_command(), None);
    }

    #[test]
    fn input_recovery_uses_reserve_and_global_recovery_can_upgrade_it() {
        let (mut control, mut audio) = LockFreeAudioBoundary::new(2, parameters(0)).into_handles();
        control.push_command(command(60)).unwrap();
        control.push_input_recovery_command().unwrap();
        control.push_input_recovery_command().unwrap();
        assert_eq!(audio.pop_command(), Some(command(60)));
        assert_eq!(
            audio.pop_command(),
            Some(AudioCommand::ReleaseImmediateNotes)
        );
        assert_eq!(audio.pop_command(), None);
        control.push_input_recovery_command().unwrap();
        control.push_recovery_command().unwrap();
        control.push_input_recovery_command().unwrap();
        assert_eq!(audio.pop_command(), Some(AudioCommand::AllNotesOff));
        assert_eq!(audio.pop_command(), None);
        // An ordinary global command does not consume another queued reserve.
        control.push_command(AudioCommand::AllNotesOff).unwrap();
        control.push_input_recovery_command().unwrap();
        assert_eq!(audio.pop_command(), Some(AudioCommand::AllNotesOff));
        control.push_recovery_command().unwrap();
        assert_eq!(audio.pop_command(), Some(AudioCommand::AllNotesOff));
        assert_eq!(audio.pop_command(), None);
    }

    #[test]
    fn parameter_publication_is_latest_wins_and_complete() {
        let boundary = LockFreeAudioBoundary::new(2, parameters(0));
        let (mut control, mut audio) = boundary.into_handles();

        control.publish_parameters(parameters(1));
        control.publish_parameters(parameters(2));

        assert_eq!(audio.read_latest_parameters().generation(), 2);
    }

    #[test]
    fn final_slot_is_reserved_and_duplicate_recovery_is_coalesced() {
        let boundary = LockFreeAudioBoundary::new(2, parameters(0));
        let (mut control, mut audio) = boundary.into_handles();

        assert!(control.has_recovery_reserve());
        control.push_command(command(60)).unwrap();
        assert_eq!(
            control.push_command(command(62)),
            Err(BoundaryFull::new(command(62)))
        );
        control.push_recovery_command().unwrap();
        control.push_recovery_command().unwrap();

        assert_eq!(audio.pop_command(), Some(command(60)));
        assert_eq!(audio.pop_command(), Some(AudioCommand::AllNotesOff));
        assert_eq!(audio.pop_command(), None);
        control.push_recovery_command().unwrap();
        assert_eq!(audio.pop_command(), Some(AudioCommand::AllNotesOff));
    }

    #[test]
    fn concrete_handles_are_send_but_keep_distinct_capabilities() {
        fn assert_send<T: Send>() {}

        assert_send::<LockFreeControlHandle>();
        assert_send::<LockFreeAudioHandle>();
    }
}
