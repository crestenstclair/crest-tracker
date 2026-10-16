use crate::kernel::midi_message::MidiMessage;
use crate::testing::instrument_part::InstrumentPart;
use core::fmt;
use std::time::Duration;

/// Maximum number of due MIDI events that one poll call can append.
pub const FIXED_EVENT_BATCH_CAPACITY: usize = 256;

/// Caller-owned bounded storage reused across source polls.
#[derive(Clone, Debug)]
pub struct FixedEventBatch {
    events: [Option<MidiMessage>; FIXED_EVENT_BATCH_CAPACITY],
    len: usize,
}

impl FixedEventBatch {
    /// Number of events this batch can hold without allocation.
    pub const CAPACITY: usize = FIXED_EVENT_BATCH_CAPACITY;

    /// Creates one empty reusable batch.
    pub const fn new() -> Self {
        Self {
            events: [None; FIXED_EVENT_BATCH_CAPACITY],
            len: 0,
        }
    }

    /// Appends an event without allocation.
    pub fn try_push(&mut self, event: MidiMessage) -> Result<(), MidiSourceError> {
        if self.len == Self::CAPACITY {
            return Err(MidiSourceError::new(
                "fixed MIDI event batch capacity was exceeded",
            ));
        }

        self.events[self.len] = Some(event);
        self.len += 1;
        Ok(())
    }

    /// Removes all events while retaining the fixed storage for reuse.
    pub fn clear(&mut self) {
        for slot in &mut self.events[..self.len] {
            *slot = None;
        }
        self.len = 0;
    }

    /// Returns the number of appended events.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Reports whether the batch contains no events.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Reports whether no more events can be appended during this poll.
    pub const fn is_full(&self) -> bool {
        self.len == Self::CAPACITY
    }

    /// Returns the number of events that can still be appended without allocation.
    pub const fn remaining_capacity(&self) -> usize {
        Self::CAPACITY - self.len
    }

    /// Returns an event by its append order.
    pub fn get(&self, index: usize) -> Option<&MidiMessage> {
        self.events.get(index).and_then(Option::as_ref)
    }

    /// Visits appended events in their stable source order.
    pub fn iter(&self) -> impl Iterator<Item = &MidiMessage> {
        self.events[..self.len].iter().filter_map(Option::as_ref)
    }
}

impl Default for FixedEventBatch {
    fn default() -> Self {
        Self::new()
    }
}

/// A failure while preparing or polling the automatic MIDI source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MidiSourceError {
    message: String,
}

impl MidiSourceError {
    /// Creates an actionable fixture preparation or polling error.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Returns the source-provided failure description.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for MidiSourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for MidiSourceError {}

/// Inbound port for one automatic, run-once MIDI fixture.
///
/// Implementations parse and prepare outside the audio callback. After `start`,
/// `poll` appends due normalized messages to the caller's reusable batch. The
/// boundary intentionally exposes no seek, pause, loop, recording, editing,
/// timeline, song, clip, pattern, or transport operation.
pub trait MidiEventSource {
    /// Parses the source and returns parts in stable first-sounding order.
    fn prepare(&mut self) -> Result<Vec<InstrumentPart>, MidiSourceError>;

    /// Starts the prepared source automatically at elapsed zero.
    fn start(&mut self);

    /// Appends due messages in source order up to the output's fixed capacity.
    ///
    /// Implementations retain any remaining overdue messages for later polls;
    /// elapsed-time catch-up alone is not a capacity error.
    fn poll(
        &mut self,
        elapsed: Duration,
        output: &mut FixedEventBatch,
    ) -> Result<(), MidiSourceError>;

    /// Reports whether the run-once source reached its end.
    fn finished(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::{FixedEventBatch, MidiEventSource, MidiSourceError, FIXED_EVENT_BATCH_CAPACITY};
    use crate::kernel::midi_channel::MidiChannel;
    use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
    use crate::testing::instrument_part::InstrumentPart;
    use std::time::Duration;

    fn event(part_index: usize) -> MidiMessage {
        let channel = MidiChannel::new((part_index % 16) as u8).unwrap();
        MidiMessage::try_new(channel, MidiMessageKind::NoteOn, 60, 100).unwrap()
    }

    #[test]
    fn batch_appends_in_order_and_reuses_its_fixed_storage() {
        let mut batch = FixedEventBatch::new();

        batch.try_push(event(2)).unwrap();
        batch.try_push(event(5)).unwrap();

        assert_eq!(batch.len(), 2);
        assert_eq!(batch.get(0).copied(), Some(event(2)));
        assert_eq!(
            batch.iter().copied().collect::<Vec<_>>(),
            vec![event(2), event(5)]
        );

        batch.clear();
        assert!(batch.is_empty());
        assert_eq!(batch.remaining_capacity(), FixedEventBatch::CAPACITY);
        batch.try_push(event(7)).unwrap();
        assert_eq!(batch.get(0).copied(), Some(event(7)));
        assert!(!batch.is_full());
        assert_eq!(batch.remaining_capacity(), FixedEventBatch::CAPACITY - 1);
    }

    #[test]
    fn batch_rejects_events_beyond_its_fixed_capacity() {
        let mut batch = FixedEventBatch::new();

        for index in 0..FIXED_EVENT_BATCH_CAPACITY {
            batch.try_push(event(index)).unwrap();
        }

        assert!(batch.is_full());
        assert_eq!(batch.remaining_capacity(), 0);

        let error = batch
            .try_push(event(FIXED_EVENT_BATCH_CAPACITY))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "fixed MIDI event batch capacity was exceeded"
        );
        assert_eq!(batch.len(), FixedEventBatch::CAPACITY);
    }

    struct TestSource {
        started: bool,
        finished: bool,
    }

    impl MidiEventSource for TestSource {
        fn prepare(&mut self) -> Result<Vec<InstrumentPart>, MidiSourceError> {
            Ok(Vec::new())
        }

        fn start(&mut self) {
            self.started = true;
        }

        fn poll(
            &mut self,
            elapsed: Duration,
            output: &mut FixedEventBatch,
        ) -> Result<(), MidiSourceError> {
            assert!(self.started);
            assert_eq!(elapsed, Duration::from_millis(20));
            output.try_push(event(1))?;
            self.finished = true;
            Ok(())
        }

        fn finished(&self) -> bool {
            self.finished
        }
    }

    #[test]
    fn source_contract_is_object_safe_and_poll_appends() {
        let mut source = TestSource {
            started: false,
            finished: false,
        };
        let source: &mut dyn MidiEventSource = &mut source;
        let mut output = FixedEventBatch::new();
        output.try_push(event(0)).unwrap();

        assert!(source.prepare().unwrap().is_empty());
        source.start();
        source.poll(Duration::from_millis(20), &mut output).unwrap();

        assert_eq!(output.len(), 2);
        assert_eq!(output.get(0).copied(), Some(event(0)));
        assert_eq!(output.get(1).copied(), Some(event(1)));
        assert!(source.finished());
    }

    #[test]
    fn source_error_preserves_its_actionable_message() {
        let error = MidiSourceError::new("fixture is malformed");

        assert_eq!(error.message(), "fixture is malformed");
        assert_eq!(error.to_string(), "fixture is malformed");
    }
}
