use crate::control::event_record::EventRecord;
use core::fmt;
use serde::Serialize;
use std::collections::VecDeque;

/// Named coverage identifiers carried alongside the recorded event history.
///
/// Identifiers are kept sorted and unique so repeated observations and differing
/// discovery order cannot change the serialized trace.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventCoverage {
    expected: Vec<String>,
    exercised: Vec<String>,
    missing: Vec<String>,
    unexpected: Vec<String>,
}

impl EventCoverage {
    /// Creates coverage for the complete named surface expected by a caller.
    ///
    /// Event, parameter, and property names share one namespace. Callers can
    /// use stable prefixes such as event., parameter., and property.
    pub fn new<I, S>(expected: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut expected: Vec<String> = expected.into_iter().map(Into::into).collect();
        expected.sort();
        expected.dedup();

        Self {
            missing: expected.clone(),
            expected,
            exercised: Vec::new(),
            unexpected: Vec::new(),
        }
    }

    /// Returns every expected identifier in deterministic order.
    pub fn expected(&self) -> &[String] {
        &self.expected
    }

    /// Returns every observed identifier in deterministic order.
    pub fn exercised(&self) -> &[String] {
        &self.exercised
    }

    /// Returns expected identifiers that have not been observed.
    pub fn missing(&self) -> &[String] {
        &self.missing
    }

    /// Returns observed identifiers outside the declared surface.
    pub fn unexpected(&self) -> &[String] {
        &self.unexpected
    }

    /// Marks one identifier as exercised.
    ///
    /// The return value is true only for the first observation. Unexpected
    /// identifiers remain visible in the exercised set without changing missing.
    pub fn mark_exercised(&mut self, identifier: impl Into<String>) -> bool {
        let identifier = identifier.into();
        if !insert_sorted_unique(&mut self.exercised, identifier.clone()) {
            return false;
        }

        if let Ok(index) = self.missing.binary_search(&identifier) {
            self.missing.remove(index);
        } else if self.expected.binary_search(&identifier).is_err() {
            insert_sorted_unique(&mut self.unexpected, identifier);
        }
        true
    }

    /// Reports whether all expected coverage has been exercised.
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty() && self.unexpected.is_empty()
    }
}

fn insert_sorted_unique(values: &mut Vec<String>, value: String) -> bool {
    match values.binary_search(&value) {
        Ok(_) => false,
        Err(index) => {
            values.insert(index, value);
            true
        }
    }
}

/// A coherence or capacity failure detected while updating an EventLog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventLogError {
    ZeroCapacity,
    ObservationOverflow,
    EffectRecordUnavailable { sequence: u64 },
    DuplicateStructuralEffect { sequence: u64 },
    SequenceMismatch { expected: u64, actual: u64 },
    GenerationChainMismatch { expected: u64, actual: u64 },
    StateHashChainMismatch { expected: String, actual: String },
}

impl fmt::Display for EventLogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroCapacity => formatter.write_str("event log capacity must be nonzero"),
            Self::ObservationOverflow => {
                formatter.write_str("event log observation count cannot exceed u64::MAX")
            }
            Self::EffectRecordUnavailable { sequence } => write!(
                formatter,
                "event record {sequence} is unavailable for its structural effect"
            ),
            Self::DuplicateStructuralEffect { sequence } => write!(
                formatter,
                "event record {sequence} already contains that structural effect"
            ),
            Self::SequenceMismatch { expected, actual } => write!(
                formatter,
                "event record sequence must be contiguous: expected {expected}, got {actual}"
            ),
            Self::GenerationChainMismatch { expected, actual } => write!(
                formatter,
                "event generation chain is broken: expected {expected}, got {actual}"
            ),
            Self::StateHashChainMismatch { expected, actual } => write!(
                formatter,
                "event state hash chain is broken: expected {expected}, got {actual}"
            ),
        }
    }
}

impl std::error::Error for EventLogError {}

/// A deterministic, bounded, LLM-readable journal of control events.
///
/// The journal is owned and updated on the control thread. Once its configured
/// capacity is reached, appending a coherent record evicts the oldest record
/// and increments droppedRecords; eviction is therefore always observable.
/// Pre-sizing the exhaustive demo for its complete scene retains every record.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventLog {
    schema_version: u32,
    coverage: EventCoverage,
    dropped_records: u64,
    records: VecDeque<EventRecord>,
    total_observed: u64,
    #[serde(skip)]
    capacity: usize,
}

impl EventLog {
    /// The stable schema version emitted in every serialized log.
    ///
    /// Version 6 records the bounded prepared Sample visualization carried by
    /// `EnginePrepared`, making Phase 7 replay lossless without logging PCM.
    /// Version 7 adds file-kind correlation and imported asset descriptors.
    /// Version 8 adds source-specific `NavigatePage` input records.
    /// Version 9 adds instrument families to imported capability descriptors.
    /// Version 10 adds optional stepped value labels to imported descriptors.
    /// Version 12 records multiple prepared Sample summaries and leading controls.
    pub const SCHEMA_VERSION: u32 = 12;
    pub const SERIALIZED_PROPERTY_DESCRIPTOR: &'static [&'static str] = &[
        "schemaVersion",
        "totalObserved",
        "droppedRecords",
        "records",
        "coverage.expected",
        "coverage.exercised",
        "coverage.missing",
        "coverage.unexpected",
    ];

    /// Returns the production-owned serialized journal property surface.
    pub const fn serialized_property_descriptor() -> &'static [&'static str] {
        Self::SERIALIZED_PROPERTY_DESCRIPTOR
    }

    /// Creates an empty bounded journal without predeclared coverage.
    pub fn new(capacity: usize) -> Result<Self, EventLogError> {
        Self::with_coverage(capacity, EventCoverage::default())
    }

    /// Creates an empty bounded journal with its complete expected coverage.
    pub fn with_coverage(capacity: usize, coverage: EventCoverage) -> Result<Self, EventLogError> {
        if capacity == 0 {
            return Err(EventLogError::ZeroCapacity);
        }

        Ok(Self {
            schema_version: Self::SCHEMA_VERSION,
            coverage,
            dropped_records: 0,
            records: VecDeque::with_capacity(capacity),
            total_observed: 0,
            capacity,
        })
    }

    /// Returns the sequence that must be assigned to the next EventRecord.
    pub const fn next_sequence(&self) -> u64 {
        self.total_observed
    }

    /// Appends one record after validating its complete transition chain.
    pub fn append(&mut self, record: EventRecord) -> Result<(), EventLogError> {
        let next_total = self
            .total_observed
            .checked_add(1)
            .ok_or(EventLogError::ObservationOverflow)?;

        if record.sequence() != self.next_sequence() {
            return Err(EventLogError::SequenceMismatch {
                expected: self.next_sequence(),
                actual: record.sequence(),
            });
        }

        if let Some(previous) = self.records.back() {
            if record.generation_before() != previous.generation_after() {
                return Err(EventLogError::GenerationChainMismatch {
                    expected: previous.generation_after(),
                    actual: record.generation_before(),
                });
            }
            if record.state_hash_before() != previous.state_hash_after() {
                return Err(EventLogError::StateHashChainMismatch {
                    expected: previous.state_hash_after().to_owned(),
                    actual: record.state_hash_before().to_owned(),
                });
            }
        }

        if self.records.len() == self.capacity {
            self.records.pop_front();
            self.dropped_records += 1;
        }
        self.records.push_back(record);
        self.total_observed = next_total;
        Ok(())
    }

    /// Marks one named event, parameter, or property identifier as exercised.
    pub fn mark_exercised(&mut self, identifier: impl Into<String>) -> bool {
        self.coverage.mark_exercised(identifier)
    }

    /// Returns the stable JSON schema version.
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub(crate) fn append_engine_selection_effect(
        &mut self,
        sequence: u64,
        effect: crate::control::EngineSelectionEffect,
    ) -> Result<(), EventLogError> {
        let record = self
            .records
            .iter_mut()
            .find(|record| record.sequence() == sequence)
            .ok_or(EventLogError::EffectRecordUnavailable { sequence })?;
        if !record.append_engine_selection_effect(effect) {
            return Err(EventLogError::DuplicateStructuralEffect { sequence });
        }
        Ok(())
    }

    /// Returns the configured maximum number of retained records.
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns retained records in chronological order.
    pub fn records(&self) -> &VecDeque<EventRecord> {
        &self.records
    }

    /// Returns the number of currently retained records.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Reports whether no records are currently retained.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Returns the total number of coherent records ever appended.
    pub const fn total_observed(&self) -> u64 {
        self.total_observed
    }

    /// Returns the number of oldest records evicted by the bounded journal.
    pub const fn dropped_records(&self) -> u64 {
        self.dropped_records
    }

    /// Returns named expected, exercised, and missing coverage.
    pub const fn coverage(&self) -> &EventCoverage {
        &self.coverage
    }

    /// Serializes the journal with stable camelCase field and enum names.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{EventCoverage, EventLog, EventLogError};
    use crate::control::app_event::{AppEvent, Direction};
    use crate::control::app_state::EventRejection;
    use crate::control::event_record::{EventRecord, EventSource};
    use crate::control::state_snapshot::StateSnapshot;
    use crate::control::text_projection::TextProjection;

    fn rejected_record(sequence: u64, state_marker: &str) -> EventRecord {
        let snapshot = StateSnapshot::new(format!(
            "{{\"generation\":4,\"marker\":\"{state_marker}\"}}"
        ));
        let projection = TextProjection::new("state".to_owned(), 0, snapshot.hash().to_owned());

        EventRecord::rejected(
            sequence,
            EventSource::DemoScene,
            &AppEvent::Adjust(Direction::Right),
            4,
            snapshot.hash(),
            4,
            &projection,
            EventRejection::ParameterAtBoundary,
        )
        .unwrap()
    }

    #[test]
    fn bounded_control_thread_journal_reports_every_eviction() {
        let mut log = EventLog::new(2).unwrap();

        for sequence in 0..9 {
            log.append(rejected_record(sequence, "stable")).unwrap();
        }

        assert_eq!(log.total_observed(), 9);
        assert_eq!(log.dropped_records(), 7);
        assert_eq!(log.len(), 2);
        assert_eq!(log.records()[0].sequence(), 7);
        assert_eq!(log.records()[1].sequence(), 8);
        assert_eq!(log.next_sequence(), 9);
        let json: serde_json::Value = serde_json::from_str(&log.to_json().unwrap()).unwrap();
        assert_eq!(json["records"][0]["sequence"], 7);
        assert_eq!(json["records"][1]["sequence"], 8);
    }

    #[test]
    fn exhaustive_capacity_retains_the_complete_chain_without_drops() {
        let mut log = EventLog::new(3).unwrap();

        for sequence in 0..3 {
            log.append(rejected_record(sequence, "scene")).unwrap();
        }

        assert_eq!(log.records().len(), 3);
        assert_eq!(log.total_observed(), 3);
        assert_eq!(log.dropped_records(), 0);
    }

    #[test]
    fn append_rejects_noncontiguous_sequence_without_mutating_the_log() {
        let mut log = EventLog::new(2).unwrap();

        let error = log.append(rejected_record(1, "stable")).unwrap_err();

        assert_eq!(
            error,
            EventLogError::SequenceMismatch {
                expected: 0,
                actual: 1,
            }
        );
        assert!(log.is_empty());
        assert_eq!(log.total_observed(), 0);
    }

    #[test]
    fn append_rejects_broken_state_chain_without_evicting_history() {
        let mut log = EventLog::new(1).unwrap();
        log.append(rejected_record(0, "before")).unwrap();

        let error = log.append(rejected_record(1, "after")).unwrap_err();

        assert!(matches!(
            error,
            EventLogError::StateHashChainMismatch { .. }
        ));
        assert_eq!(log.records()[0].sequence(), 0);
        assert_eq!(log.dropped_records(), 0);
        assert_eq!(log.total_observed(), 1);
    }

    #[test]
    fn named_coverage_and_json_are_sorted_stable_and_explicit() {
        let coverage = EventCoverage::new([
            "property.patch.pan",
            "event.adjust.right",
            "property.patch.pan",
        ]);
        let mut log = EventLog::with_coverage(2, coverage).unwrap();

        assert!(log.mark_exercised("property.patch.pan"));
        assert!(!log.mark_exercised("property.patch.pan"));
        assert!(log.mark_exercised("event.unexpected"));
        assert!(!log.coverage().is_complete());

        let first = log.to_json().unwrap();
        let second = log.to_json().unwrap();
        let json: serde_json::Value = serde_json::from_str(&first).unwrap();

        assert_eq!(first, second);
        assert_eq!(json["schemaVersion"], EventLog::SCHEMA_VERSION);
        assert_eq!(json["droppedRecords"], 0);
        assert_eq!(json["totalObserved"], 0);
        assert_eq!(
            json["coverage"]["expected"],
            serde_json::json!(["event.adjust.right", "property.patch.pan"])
        );
        assert_eq!(
            json["coverage"]["exercised"],
            serde_json::json!(["event.unexpected", "property.patch.pan"])
        );
        assert_eq!(
            json["coverage"]["missing"],
            serde_json::json!(["event.adjust.right"])
        );
        assert_eq!(
            json["coverage"]["unexpected"],
            serde_json::json!(["event.unexpected"])
        );
    }

    #[test]
    fn zero_capacity_is_rejected() {
        assert_eq!(EventLog::new(0), Err(EventLogError::ZeroCapacity));
    }
}
