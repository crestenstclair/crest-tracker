use crate::real_time::scheduled_event::{
    ScheduledEvent, ScheduledEventEpochExhausted, ScheduledEventPoll, ScheduledEventRejected,
    ScheduledEventRejectionReason, ScheduledLane,
};
use core::fmt;
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// Reserving the terminal value makes even epoch exhaustion invalidate every
// previously accepted event without wrapping into an older schedule.
const EXHAUSTED_EPOCH: u64 = u64::MAX;

/// Two bounded schedule lanes independent of immediate MIDI and its recovery slot.
///
/// Construct and destroy both handles off the audio callback. The callback only
/// loads the shared cancellation epoch and peeks/pops Copy events; it never
/// clones or drops shared ownership. One control thread owns publication.
pub struct ScheduledEventBoundary {
    control: ScheduledEventControlHandle,
    audio: ScheduledEventAudioHandle,
}

impl ScheduledEventBoundary {
    /// Allocates `capacity` slots per lane before callback ownership.
    pub fn new(capacity: usize) -> Result<Self, ScheduledEventCapacityError> {
        if capacity == 0 {
            return Err(ScheduledEventCapacityError);
        }
        let [(playback_producer, playback_consumer), (monitor_producer, monitor_consumer)] =
            ScheduledLane::ALL.map(|_| RingBuffer::new(capacity));
        let epoch = Arc::new([AtomicU64::new(0), AtomicU64::new(0)]);
        let high_water = Arc::new(AtomicU64::new(0));
        Ok(Self {
            control: ScheduledEventControlHandle {
                events: [playback_producer, monitor_producer],
                published_epoch: Arc::clone(&epoch),
                epoch: [0; 2],
                last_frame: [None; 2],
                high_water: Arc::clone(&high_water),
                capacity,
            },
            audio: ScheduledEventAudioHandle {
                events: [playback_consumer, monitor_consumer],
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

/// Sole producer of independently ordered, cancellable playback and monitor lanes.
pub struct ScheduledEventControlHandle {
    events: [Producer<ScheduledEvent>; 2],
    published_epoch: Arc<[AtomicU64; 2]>,
    epoch: [u64; 2],
    last_frame: [Option<u64>; 2],
    high_water: Arc<AtomicU64>,
    capacity: usize,
}

impl ScheduledEventControlHandle {
    pub const fn epoch(&self) -> u64 {
        self.epoch_for(ScheduledLane::Playback)
    }

    pub const fn epoch_for(&self, lane: ScheduledLane) -> u64 {
        self.epoch[lane.index()]
    }

    /// Publishes in nondecreasing frame order within the event's lane, retaining FIFO ties.
    /// Failed publication changes neither the queue nor its ordering watermark.
    pub fn push(&mut self, event: ScheduledEvent) -> Result<(), ScheduledEventRejected> {
        let index = event.lane().index();
        let epoch = self.epoch[index];
        let reason = if epoch == EXHAUSTED_EPOCH {
            Some(ScheduledEventRejectionReason::EpochExhausted)
        } else if event.epoch() != epoch {
            Some(ScheduledEventRejectionReason::StaleEpoch {
                current_epoch: epoch,
            })
        } else {
            self.last_frame[index]
                .filter(|last_frame| event.absolute_frame() < *last_frame)
                .map(|last_frame| ScheduledEventRejectionReason::OutOfOrder { last_frame })
        };
        if let Some(reason) = reason {
            return Err(ScheduledEventRejected::new(event, reason));
        }
        match self.events[index].push(event) {
            Ok(()) => {
                self.last_frame[index] = Some(event.absolute_frame());
                let occupied = self
                    .events
                    .iter()
                    .map(|queue| (self.capacity - queue.slots()) as u64)
                    .fold(0, u64::saturating_add);
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
        self.cancel_lane(ScheduledLane::Playback)
    }

    pub fn cancel_lane(
        &mut self,
        lane: ScheduledLane,
    ) -> Result<u64, ScheduledEventEpochExhausted> {
        let index = lane.index();
        self.epoch[index] = self.epoch[index].saturating_add(1);
        self.last_frame[index] = None;
        self.published_epoch[index].store(self.epoch[index], Ordering::Release);
        if self.epoch[index] == EXHAUSTED_EPOCH {
            Err(ScheduledEventEpochExhausted)
        } else {
            Ok(self.epoch[index])
        }
    }
}

/// Callback-only consumer; each poll examines two heads and consumes at most one event.
pub struct ScheduledEventAudioHandle {
    events: [Consumer<ScheduledEvent>; 2],
    epoch: Arc<[AtomicU64; 2]>,
    high_water: Arc<AtomicU64>,
}

impl ScheduledEventAudioHandle {
    /// Maximum combined occupied slots sampled after successful producer publication.
    /// Concurrent consumption may make this lower than the instantaneous peak.
    pub fn queue_high_water(&self) -> u64 {
        self.high_water.load(Ordering::Relaxed)
    }

    pub fn epoch(&self) -> u64 {
        self.epoch_for(ScheduledLane::Playback)
    }

    pub fn epoch_for(&self, lane: ScheduledLane) -> u64 {
        self.epoch[lane.index()].load(Ordering::Acquire)
    }

    /// Consumes one stale or due event, or leaves a future event in the ring.
    ///
    /// The end frame is exclusive: an event exactly at block end belongs to
    /// the next block. Due heads merge by frame; playback wins cross-lane ties.
    /// A due head takes precedence over stale cleanup on the other lane. When
    /// neither head is due, one stale entry is reclaimed regardless of time.
    /// New events behind canceled entries wait for bounded reclamation on their
    /// own lane. The renderer charges each consumed result against its shared
    /// per-block budget. Concurrent cancellation takes effect at its next epoch
    /// observation, never requiring a producer lock or unbounded drain.
    pub fn poll_before(&mut self, end_frame: u64) -> ScheduledEventPoll {
        let mut next: Option<(ScheduledLane, ScheduledEvent)> = None;
        let mut stale = None;
        for lane in ScheduledLane::ALL {
            let Ok(event) = self.events[lane.index()].peek().copied() else {
                continue;
            };
            if event.epoch() != self.epoch_for(lane) {
                stale.get_or_insert(lane);
                continue;
            }
            // Each lane retains FIFO ties; playback wins ties across lanes.
            if next.is_none_or(|(_, current)| event.absolute_frame() < current.absolute_frame()) {
                next = Some((lane, event));
            }
        }
        // Canceled lookahead must not hold up a due release on the other lane.
        // Reclaim at most one stale entry when neither lane has a due head.
        if next.is_none_or(|(_, event)| event.absolute_frame() >= end_frame) {
            if let Some(lane) = stale {
                let _ = self.events[lane.index()].pop();
                return ScheduledEventPoll::Stale;
            }
        }
        let Some((lane, event)) = next else {
            return ScheduledEventPoll::Empty;
        };
        if event.absolute_frame() >= end_frame {
            return ScheduledEventPoll::Future {
                absolute_frame: event.absolute_frame(),
            };
        }
        let Ok(event) = self.events[lane.index()].pop() else {
            return ScheduledEventPoll::Empty;
        };
        if event.epoch() != self.epoch_for(lane) {
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

    fn monitor(frame: u64, epoch: u64, note: u8) -> ScheduledEvent {
        ScheduledEvent::monitor_voice(
            frame,
            epoch,
            GraphRevision::INITIAL,
            PatchId::new(1).unwrap(),
            crate::synth::MidiVoiceId::new(1).unwrap(),
            crate::synth::MidiExpressionScope::Member,
            event(frame, epoch, note).message().unwrap(),
        )
    }

    #[test]
    fn monitor_precedes_playback_lookahead_and_cross_lane_ties_are_stable() {
        let (mut control, mut audio) = ScheduledEventBoundary::new(3).unwrap().into_handles();
        let playback = [event(20, 0, 60), event(20, 0, 62), event(100, 0, 64)];
        for event in playback {
            control.push(event).unwrap();
        }
        let live = [monitor(5, 0, 65), monitor(20, 0, 67), monitor(21, 0, 69)];
        for event in live {
            control.push(event).unwrap();
        }
        assert_eq!(audio.queue_high_water(), 6);
        for event in [
            live[0],
            playback[0],
            playback[1],
            live[1],
            live[2],
            playback[2],
        ] {
            assert_eq!(audio.poll_before(101), ScheduledEventPoll::Ready(event));
        }
        assert_eq!(audio.poll_before(101), ScheduledEventPoll::Empty);
        assert_eq!(
            control.push(monitor(19, 0, 60)).unwrap_err().reason(),
            ScheduledEventRejectionReason::OutOfOrder { last_frame: 21 }
        );
        assert_eq!(
            control.push(event(99, 0, 60)).unwrap_err().reason(),
            ScheduledEventRejectionReason::OutOfOrder { last_frame: 100 }
        );
    }

    #[test]
    fn lane_capacity_cancel_and_stale_reclamation_are_independent() {
        let (mut control, mut audio) = ScheduledEventBoundary::new(2).unwrap().into_handles();
        control.push(event(1_000, 0, 60)).unwrap();
        control.push(event(2_000, 0, 62)).unwrap();
        assert_eq!(
            control.push(event(3_000, 0, 64)).unwrap_err().reason(),
            ScheduledEventRejectionReason::Full
        );
        let live = monitor(5, 0, 65);
        control.push(live).unwrap();
        control.cancel().unwrap();
        assert_eq!(audio.epoch_for(ScheduledLane::Monitor), 0);
        // A one-command callback can deliver live input despite canceled lookahead.
        assert_eq!(audio.poll_before(6), ScheduledEventPoll::Ready(live));
        assert_eq!(audio.poll_before(6), ScheduledEventPoll::Stale);
        assert_eq!(audio.poll_before(6), ScheduledEventPoll::Stale);
        let clip = event(10, 1, 67);
        control.push(clip).unwrap();
        control.push(monitor(5_000, 0, 69)).unwrap();
        control.cancel_lane(ScheduledLane::Monitor).unwrap();
        assert_eq!(audio.poll_before(11), ScheduledEventPoll::Ready(clip));
        assert_eq!(audio.poll_before(11), ScheduledEventPoll::Stale);
        let new_live = monitor(0, 1, 70);
        control.push(new_live).unwrap();
        assert_eq!(audio.poll_before(1), ScheduledEventPoll::Ready(new_live));
    }

    #[test]
    fn monitor_epoch_exhaustion_does_not_disable_playback() {
        let (mut control, mut audio) = ScheduledEventBoundary::new(1).unwrap().into_handles();
        control.epoch[1] = EXHAUSTED_EPOCH - 1;
        control.published_epoch[1].store(control.epoch[1], Ordering::Release);
        control.push(monitor(100, control.epoch[1], 60)).unwrap();
        assert_eq!(
            control.cancel_lane(ScheduledLane::Monitor),
            Err(ScheduledEventEpochExhausted)
        );
        assert_eq!(
            control
                .push(monitor(0, EXHAUSTED_EPOCH, 60))
                .unwrap_err()
                .reason(),
            ScheduledEventRejectionReason::EpochExhausted
        );
        control.push(event(0, 0, 62)).unwrap();
        assert_eq!(
            audio.poll_before(1),
            ScheduledEventPoll::Ready(event(0, 0, 62))
        );
        assert_eq!(audio.poll_before(1), ScheduledEventPoll::Stale);
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
        control.epoch[0] = EXHAUSTED_EPOCH - 1;
        control.published_epoch[0].store(control.epoch[0], Ordering::Release);
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
