use crate::real_time::scheduled_event::{
    ScheduledEvent, ScheduledEventEpochExhausted, ScheduledEventPoll, ScheduledEventRejected,
    ScheduledEventRejectionReason,
};
use core::fmt;
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// Reserving the terminal value makes even epoch exhaustion invalidate every
// previously accepted event without wrapping into an older schedule.
const EXHAUSTED_EPOCH: u64 = u64::MAX;

/// A bounded schedule lane independent of immediate MIDI and its recovery slot.
///
/// Construct and destroy both handles off the audio callback. The callback only
/// loads the shared cancellation epoch and peeks/pops Copy events; it never
/// clones or drops shared ownership. One control thread owns publication.
pub struct ScheduledEventBoundary {
    control: ScheduledEventControlHandle,
    audio: ScheduledEventAudioHandle,
}

impl ScheduledEventBoundary {
    /// Allocates all queue and cancellation storage before callback ownership.
    pub fn new(capacity: usize) -> Result<Self, ScheduledEventCapacityError> {
        if capacity == 0 {
            return Err(ScheduledEventCapacityError);
        }
        let (producer, consumer) = RingBuffer::new(capacity);
        let epoch = Arc::new(AtomicU64::new(0));
        let high_water = Arc::new(AtomicU64::new(0));
        Ok(Self {
            control: ScheduledEventControlHandle {
                events: producer,
                published_epoch: Arc::clone(&epoch),
                epoch: 0,
                last_frame: None,
                high_water: Arc::clone(&high_water),
                capacity,
            },
            audio: ScheduledEventAudioHandle {
                events: consumer,
                epoch,
                high_water,
            },
        })
    }

    pub fn into_handles(self) -> (ScheduledEventControlHandle, ScheduledEventAudioHandle) {
        (self.control, self.audio)
    }
}

/// The schedule cannot be constructed without event storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScheduledEventCapacityError;

impl fmt::Display for ScheduledEventCapacityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("scheduled MIDI queue capacity must be nonzero")
    }
}

impl std::error::Error for ScheduledEventCapacityError {}

/// Sole producer of one ordered, cancellable schedule.
pub struct ScheduledEventControlHandle {
    events: Producer<ScheduledEvent>,
    published_epoch: Arc<AtomicU64>,
    epoch: u64,
    last_frame: Option<u64>,
    high_water: Arc<AtomicU64>,
    capacity: usize,
}

impl ScheduledEventControlHandle {
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Publishes in nondecreasing frame order, retaining FIFO order for ties.
    /// Failed publication changes neither the queue nor its ordering watermark.
    pub fn push(&mut self, event: ScheduledEvent) -> Result<(), ScheduledEventRejected> {
        let reason = if self.epoch == EXHAUSTED_EPOCH {
            Some(ScheduledEventRejectionReason::EpochExhausted)
        } else if event.epoch() != self.epoch {
            Some(ScheduledEventRejectionReason::StaleEpoch {
                current_epoch: self.epoch,
            })
        } else {
            self.last_frame
                .filter(|last_frame| event.absolute_frame() < *last_frame)
                .map(|last_frame| ScheduledEventRejectionReason::OutOfOrder { last_frame })
        };
        if let Some(reason) = reason {
            return Err(ScheduledEventRejected::new(event, reason));
        }
        match self.events.push(event) {
            Ok(()) => {
                self.last_frame = Some(event.absolute_frame());
                let occupied = (self.capacity - self.events.slots()) as u64;
                if occupied > self.high_water.load(Ordering::Relaxed) {
                    self.high_water.store(occupied, Ordering::Relaxed);
                }
                Ok(())
            }
            Err(PushError::Full(event)) => Err(ScheduledEventRejected::new(
                event,
                ScheduledEventRejectionReason::Full,
            )),
        }
    }

    /// Invalidates queued events immediately, independently of queue pressure.
    ///
    /// The callback removes stale entries under its existing work budget; this
    /// does not synchronously reclaim ring capacity or stop already sounding
    /// notes. The renderer owns note cleanup when it observes an epoch change.
    /// At exhaustion, cancellation still takes effect, but further publication
    /// is permanently rejected. Epochs never wrap or make an old event current.
    pub fn cancel(&mut self) -> Result<u64, ScheduledEventEpochExhausted> {
        self.epoch = self.epoch.saturating_add(1);
        self.last_frame = None;
        self.published_epoch.store(self.epoch, Ordering::Release);
        if self.epoch == EXHAUSTED_EPOCH {
            Err(ScheduledEventEpochExhausted)
        } else {
            Ok(self.epoch)
        }
    }
}

/// Callback-only consumer; each polling call examines at most one event.
pub struct ScheduledEventAudioHandle {
    events: Consumer<ScheduledEvent>,
    epoch: Arc<AtomicU64>,
    high_water: Arc<AtomicU64>,
}

impl ScheduledEventAudioHandle {
    /// Maximum occupied slots sampled after successful producer publication.
    /// Concurrent consumption may make this lower than the instantaneous peak.
    pub fn queue_high_water(&self) -> u64 {
        self.high_water.load(Ordering::Relaxed)
    }

    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }

    /// Consumes one stale or due event, or leaves a future event in the ring.
    ///
    /// The end frame is exclusive: an event exactly at block end belongs to
    /// the next block. Staleness is checked before time, so canceled future
    /// events cannot hold the queue until their original delivery frame. No
    /// internal loop drains stale entries; the renderer charges each consumed
    /// result against its per-block budget. A cancellation concurrent with a
    /// successful poll takes effect at the next renderer epoch observation.
    pub fn poll_before(&mut self, end_frame: u64) -> ScheduledEventPoll {
        let Ok(event) = self.events.peek().copied() else {
            return ScheduledEventPoll::Empty;
        };
        if event.epoch() == self.epoch() && event.absolute_frame() >= end_frame {
            return ScheduledEventPoll::Future {
                absolute_frame: event.absolute_frame(),
            };
        }
        let Ok(event) = self.events.pop() else {
            return ScheduledEventPoll::Empty;
        };
        if event.epoch() != self.epoch() {
            ScheduledEventPoll::Stale
        } else {
            ScheduledEventPoll::Ready(event)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
    use crate::kernel::{MidiChannel, PatchId};
    use crate::real_time::graph_revision::GraphRevision;

    fn event(frame: u64, epoch: u64, note: u8) -> ScheduledEvent {
        ScheduledEvent::new(
            frame,
            epoch,
            GraphRevision::INITIAL,
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

    #[test]
    fn zero_capacity_is_typed_and_one_slot_is_supported() {
        assert!(matches!(
            ScheduledEventBoundary::new(0),
            Err(ScheduledEventCapacityError)
        ));
        let (mut control, mut audio) = ScheduledEventBoundary::new(1).unwrap().into_handles();
        control.push(event(0, 0, 60)).unwrap();
        assert_eq!(
            audio.poll_before(1),
            ScheduledEventPoll::Ready(event(0, 0, 60))
        );
    }

    #[test]
    fn future_head_stays_queued_and_equal_frames_keep_fifo_order() {
        let (mut control, mut audio) = ScheduledEventBoundary::new(2).unwrap().into_handles();
        let first = event(64, 0, 60);
        let second = event(64, 0, 62);
        control.push(first).unwrap();
        control.push(second).unwrap();
        for end in [0, 63, 64, 64] {
            assert_eq!(
                audio.poll_before(end),
                ScheduledEventPoll::Future { absolute_frame: 64 }
            );
        }
        let rejected = control.push(event(65, 0, 64)).unwrap_err();
        assert_eq!(rejected.reason(), ScheduledEventRejectionReason::Full);
        assert_eq!(audio.poll_before(65), ScheduledEventPoll::Ready(first));
        assert_eq!(audio.poll_before(65), ScheduledEventPoll::Ready(second));
        assert_eq!(audio.poll_before(65), ScheduledEventPoll::Empty);
    }

    #[test]
    fn rejections_preserve_events_and_do_not_advance_ordering() {
        let (mut control, mut audio) = ScheduledEventBoundary::new(1).unwrap().into_handles();
        control.push(event(10, 0, 60)).unwrap();
        let future = event(30, 0, 62);
        let full = control.push(future).unwrap_err();
        assert_eq!(full.event(), future);
        assert_eq!(full.reason(), ScheduledEventRejectionReason::Full);
        assert_eq!(
            audio.poll_before(11),
            ScheduledEventPoll::Ready(event(10, 0, 60))
        );
        control.push(event(20, 0, 64)).unwrap();
        let earlier = event(19, 0, 65);
        let order = control.push(earlier).unwrap_err();
        assert_eq!(order.event(), earlier);
        assert_eq!(
            order.reason(),
            ScheduledEventRejectionReason::OutOfOrder { last_frame: 20 }
        );
        let stale = event(20, 1, 67);
        let epoch = control.push(stale).unwrap_err();
        assert_eq!(epoch.event(), stale);
        assert_eq!(
            epoch.reason(),
            ScheduledEventRejectionReason::StaleEpoch { current_epoch: 0 }
        );
    }

    #[test]
    fn cancellation_invalidates_pending_future_without_unbounded_drain() {
        let (mut control, mut audio) = ScheduledEventBoundary::new(2).unwrap().into_handles();
        control.push(event(10_000, 0, 60)).unwrap();
        control.push(event(20_000, 0, 62)).unwrap();
        assert_eq!(
            audio.poll_before(1),
            ScheduledEventPoll::Future {
                absolute_frame: 10_000
            }
        );
        assert_eq!(control.cancel(), Ok(1));
        assert_eq!(audio.epoch(), 1);
        assert_eq!(audio.poll_before(1), ScheduledEventPoll::Stale);
        control.push(event(0, 1, 64)).unwrap();
        assert_eq!(audio.poll_before(1), ScheduledEventPoll::Stale);
        assert_eq!(
            audio.poll_before(1),
            ScheduledEventPoll::Ready(event(0, 1, 64))
        );
        assert_eq!(audio.poll_before(1), ScheduledEventPoll::Empty);
    }

    #[test]
    fn epoch_exhaustion_cancels_without_wrapping_or_losing_rejected_event() {
        let (mut control, mut audio) = ScheduledEventBoundary::new(1).unwrap().into_handles();
        control.epoch = EXHAUSTED_EPOCH - 1;
        control
            .published_epoch
            .store(control.epoch, Ordering::Release);
        control.push(event(10_000, control.epoch(), 60)).unwrap();
        assert_eq!(control.cancel(), Err(ScheduledEventEpochExhausted));
        assert_eq!(audio.poll_before(0), ScheduledEventPoll::Stale);
        let rejected_event = event(0, control.epoch(), 62);
        let rejected = control.push(rejected_event).unwrap_err();
        assert_eq!(rejected.event(), rejected_event);
        assert_eq!(
            rejected.reason(),
            ScheduledEventRejectionReason::EpochExhausted
        );
        assert_eq!(control.cancel(), Err(ScheduledEventEpochExhausted));
        assert_eq!(audio.epoch(), EXHAUSTED_EPOCH);
    }
}
