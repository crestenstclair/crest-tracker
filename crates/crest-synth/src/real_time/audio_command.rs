use crate::kernel::midi_message::MidiMessage;
use crate::kernel::patch_id::PatchId;

/// A bounded command transferred from the control side to the audio callback.
///
/// Commands contain only copyable domain values, so moving them through the
/// event ring does not allocate or transfer heap ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioCommand {
    /// Delivers one normalized MIDI message to its configured patch.
    PatchMidi {
        patch_id: PatchId,
        message: MidiMessage,
    },
    /// Delivers MIDI with independent note/controller ownership.
    OwnedMidi {
        source: crate::synth::MidiSource,
        patch_id: PatchId,
        message: MidiMessage,
    },
    /// Immediate attack-owned expression, including isolated monitoring.
    VoiceMidi {
        source: crate::synth::MidiSource,
        patch_id: PatchId,
        voice: crate::synth::MidiVoiceId,
        scope: crate::synth::MidiExpressionScope,
        message: MidiMessage,
    },
    InitializedVoice {
        source: crate::synth::MidiSource,
        patch_id: PatchId,
        voice: crate::synth::MidiVoiceId,
        initial: crate::synth::OwnedVoiceInitialization,
    },
    ReleaseSource {
        source: crate::synth::MidiSource,
        patch_id: PatchId,
    },
    /// Starts the exact graph-prepared audition for one origin Patch.
    PreviewStart { patch_id: PatchId, audition_id: u64 },
    /// Releases the exact graph-prepared audition with its prepared de-click.
    PreviewStop { patch_id: PatchId, audition_id: u64 },
    /// Releases default live-input voices across Patches without touching
    /// Scheduled/Monitor ownership, send leases or their expression.
    ReleaseImmediateNotes,
    /// Requests bounded recovery by silencing every active note.
    AllNotesOff,
}

impl AudioCommand {
    /// Creates a patch-targeted MIDI command.
    pub const fn patch_midi(patch_id: PatchId, message: MidiMessage) -> Self {
        Self::PatchMidi { patch_id, message }
    }

    /// Creates the global all-notes-off recovery command.
    pub const fn all_notes_off() -> Self {
        Self::AllNotesOff
    }

    pub const fn preview_start(patch_id: PatchId, audition_id: u64) -> Self {
        Self::PreviewStart {
            patch_id,
            audition_id,
        }
    }

    pub const fn preview_stop(patch_id: PatchId, audition_id: u64) -> Self {
        Self::PreviewStop {
            patch_id,
            audition_id,
        }
    }

    /// Returns the target patch for a patch MIDI command.
    pub const fn patch_id(self) -> Option<PatchId> {
        match self {
            Self::PatchMidi { patch_id, .. }
            | Self::InitializedVoice { patch_id, .. }
            | Self::VoiceMidi { patch_id, .. }
            | Self::OwnedMidi { patch_id, .. }
            | Self::ReleaseSource { patch_id, .. }
            | Self::PreviewStart { patch_id, .. }
            | Self::PreviewStop { patch_id, .. } => Some(patch_id),
            Self::ReleaseImmediateNotes | Self::AllNotesOff => None,
        }
    }

    pub const fn audition_id(self) -> Option<u64> {
        match self {
            Self::PreviewStart { audition_id, .. } | Self::PreviewStop { audition_id, .. } => {
                Some(audition_id)
            }
            Self::InitializedVoice { .. }
            | Self::PatchMidi { .. }
            | Self::VoiceMidi { .. }
            | Self::OwnedMidi { .. }
            | Self::ReleaseSource { .. }
            | Self::ReleaseImmediateNotes
            | Self::AllNotesOff => None,
        }
    }

    /// Returns the normalized MIDI payload for a patch MIDI command.
    pub const fn message(self) -> Option<MidiMessage> {
        match self {
            Self::InitializedVoice { initial, .. } => Some(initial.message()),
            Self::PatchMidi { message, .. }
            | Self::VoiceMidi { message, .. }
            | Self::OwnedMidi { message, .. } => Some(message),
            Self::PreviewStart { .. }
            | Self::PreviewStop { .. }
            | Self::ReleaseSource { .. }
            | Self::ReleaseImmediateNotes
            | Self::AllNotesOff => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AudioCommand;
    use crate::kernel::midi_channel::MidiChannel;
    use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
    use crate::kernel::patch_id::PatchId;

    fn example_message() -> MidiMessage {
        MidiMessage::try_new(
            MidiChannel::new(2).unwrap(),
            MidiMessageKind::NoteOn,
            60,
            100,
        )
        .unwrap()
    }

    #[test]
    fn patch_midi_keeps_the_target_and_message_together() {
        let patch_id = PatchId::new(7).unwrap();
        let message = example_message();
        let command = AudioCommand::patch_midi(patch_id, message);

        assert_eq!(command.patch_id(), Some(patch_id));
        assert_eq!(command.message(), Some(message));
        assert_eq!(command, AudioCommand::PatchMidi { patch_id, message });
    }

    #[test]
    fn all_notes_off_has_no_patch_or_message_payload() {
        let command = AudioCommand::all_notes_off();

        assert_eq!(command, AudioCommand::AllNotesOff);
        assert_eq!(command.patch_id(), None);
        assert_eq!(command.message(), None);
    }

    #[test]
    fn commands_are_copyable_and_require_no_destruction() {
        fn assert_copy<T: Copy>() {}

        assert_copy::<AudioCommand>();
        let command = AudioCommand::patch_midi(PatchId::new(1).unwrap(), example_message());
        let copied = command;

        assert_eq!(command, copied);
        assert!(!core::mem::needs_drop::<AudioCommand>());
    }
}
