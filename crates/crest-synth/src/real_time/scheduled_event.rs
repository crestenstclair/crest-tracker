use crate::kernel::midi_message::MidiMessage;
use crate::kernel::patch_id::PatchId;
use crate::real_time::graph_revision::GraphRevision;
use crate::real_time::send_leases::SendLease;
use core::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduledPayload {
    Midi {
        message: MidiMessage,
        voice: Option<crate::synth::MidiVoiceId>,
        scope: crate::synth::MidiExpressionScope,
    },
    InitializedVoice {
        voice: crate::synth::MidiVoiceId,
        initial: crate::synth::OwnedVoiceInitialization,
    },
    SendLease(SendLease),
    EndSendLease(crate::real_time::SendLeaseId),
}

/// One ordered MIDI or temporary-control delivery in the renderer's absolute stereo-frame clock.
///
/// Frames count from renderer creation, not from a musical transport position.
/// The epoch identifies one cancellable schedule; graph revision prevents an
/// event prepared for an old graph from reaching a replacement Patch identity.
/// Publication validates the epoch and ordering against the owning producer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScheduledEvent {
    absolute_frame: u64,
    epoch: u64,
    graph_revision: GraphRevision,
    patch_id: PatchId,
    payload: ScheduledPayload,
}

impl ScheduledEvent {
    pub const fn new(
        absolute_frame: u64,
        epoch: u64,
        graph_revision: GraphRevision,
        patch_id: PatchId,
        message: MidiMessage,
    ) -> Self {
        Self {
            absolute_frame,
            epoch,
            graph_revision,
            patch_id,
            payload: ScheduledPayload::Midi {
                message,
                voice: None,
                scope: crate::synth::MidiExpressionScope::Member,
            },
        }
    }

    pub const fn with_voice(mut self, voice: Option<crate::synth::MidiVoiceId>) -> Self {
        if let ScheduledPayload::Midi { message, scope, .. } = self.payload {
            self.payload = ScheduledPayload::Midi {
                message,
                voice,
                scope,
            };
        }
        self
    }
    pub const fn with_expression_scope(mut self, scope: crate::synth::MidiExpressionScope) -> Self {
        if let ScheduledPayload::Midi { message, voice, .. } = self.payload {
            self.payload = ScheduledPayload::Midi {
                message,
                voice,
                scope,
            };
        }
        self
    }
    pub const fn initialized_voice(
        absolute_frame: u64,
        epoch: u64,
        graph_revision: GraphRevision,
        patch_id: PatchId,
        voice: crate::synth::MidiVoiceId,
        initial: crate::synth::OwnedVoiceInitialization,
    ) -> Self {
        Self {
            absolute_frame,
            epoch,
            graph_revision,
            patch_id,
            payload: ScheduledPayload::InitializedVoice { voice, initial },
        }
    }
    pub const fn send_lease(
        absolute_frame: u64,
        epoch: u64,
        graph_revision: GraphRevision,
        patch_id: PatchId,
        lease: SendLease,
    ) -> Self {
        Self {
            absolute_frame,
            epoch,
            graph_revision,
            patch_id,
            payload: ScheduledPayload::SendLease(lease),
        }
    }
    pub const fn end_send_lease(
        absolute_frame: u64,
        epoch: u64,
        graph_revision: GraphRevision,
        patch_id: PatchId,
        id: crate::real_time::SendLeaseId,
    ) -> Self {
        Self {
            absolute_frame,
            epoch,
            graph_revision,
            patch_id,
            payload: ScheduledPayload::EndSendLease(id),
        }
    }
    pub const fn payload(self) -> ScheduledPayload {
        self.payload
    }

    pub const fn absolute_frame(self) -> u64 {
        self.absolute_frame
    }

    pub const fn epoch(self) -> u64 {
        self.epoch
    }

    pub const fn graph_revision(self) -> GraphRevision {
        self.graph_revision
    }

    pub const fn patch_id(self) -> PatchId {
        self.patch_id
    }

    pub const fn message(self) -> Option<MidiMessage> {
        match self.payload {
            ScheduledPayload::InitializedVoice { initial, .. } => Some(initial.message()),
            ScheduledPayload::Midi { message, .. } => Some(message),
            ScheduledPayload::SendLease(_) | ScheduledPayload::EndSendLease(_) => None,
        }
    }
}

/// Why an event could not join the ordered schedule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduledEventRejectionReason {
    Full,
    Unsupported,
    OutOfOrder { last_frame: u64 },
    StaleEpoch { current_epoch: u64 },
    EpochExhausted,
}

/// An unsuccessful publication retains the complete event for explicit recovery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScheduledEventRejected {
    event: ScheduledEvent,
    reason: ScheduledEventRejectionReason,
}

impl ScheduledEventRejected {
    pub const fn new(event: ScheduledEvent, reason: ScheduledEventRejectionReason) -> Self {
        Self { event, reason }
    }

    pub const fn event(self) -> ScheduledEvent {
        self.event
    }

    pub const fn reason(self) -> ScheduledEventRejectionReason {
        self.reason
    }
}

impl fmt::Display for ScheduledEventRejected {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.reason {
            ScheduledEventRejectionReason::Unsupported => {
                formatter.write_str("this boundary does not support scheduled event")
            }
            ScheduledEventRejectionReason::Full => {
                formatter.write_str("the bounded scheduled event queue is full")
            }
            ScheduledEventRejectionReason::OutOfOrder { .. } => {
                formatter.write_str("scheduled event frames must be nondecreasing within an epoch")
            }
            ScheduledEventRejectionReason::StaleEpoch { .. } => {
                formatter.write_str("scheduled event belongs to a different cancellation epoch")
            }
            ScheduledEventRejectionReason::EpochExhausted => {
                formatter.write_str("scheduled event cancellation epochs are exhausted")
            }
        }
    }
}

impl std::error::Error for ScheduledEventRejected {}

/// Cancellation succeeded, but no further schedule epoch can be admitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScheduledEventEpochExhausted;

impl fmt::Display for ScheduledEventEpochExhausted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("scheduled event was canceled and its epoch range is exhausted")
    }
}

impl std::error::Error for ScheduledEventEpochExhausted {}

/// Result of examining at most one queued event.
///
/// Both `Ready` and `Stale` consume one entry and must charge the caller's
/// per-block event budget. `Future` retains the head without consuming it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduledEventPoll {
    Empty,
    Future { absolute_frame: u64 },
    Stale,
    Ready(ScheduledEvent),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{midi_message::MidiMessageKind, MidiChannel};

    #[test]
    fn envelope_preserves_routing_time_and_normalized_message_without_destruction() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<ScheduledEvent>();
        assert_copy::<ScheduledEventRejected>();
        assert_copy::<ScheduledEventPoll>();
        assert!(!core::mem::needs_drop::<ScheduledEvent>());
        assert!(!core::mem::needs_drop::<ScheduledEventRejected>());
        assert!(!core::mem::needs_drop::<ScheduledEventPoll>());

        let revision = GraphRevision::new(3).unwrap();
        let patch = PatchId::new(7).unwrap();
        let message = MidiMessage::try_new(
            MidiChannel::new(2).unwrap(),
            MidiMessageKind::NoteOn,
            60,
            100,
        )
        .unwrap();
        let event = ScheduledEvent::new(123, 9, revision, patch, message);
        assert_eq!(event.absolute_frame(), 123);
        assert_eq!(event.epoch(), 9);
        assert_eq!(event.graph_revision(), revision);
        assert_eq!(event.patch_id(), patch);
        assert_eq!(event.message(), Some(message));
    }
}
