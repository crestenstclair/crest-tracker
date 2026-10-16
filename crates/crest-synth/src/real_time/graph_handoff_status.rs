use crate::real_time::graph_revision::GraphRevision;

/// Fixed-size coherent acknowledgement for structural graph handoff.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GraphHandoffStatus {
    active_revision: Option<GraphRevision>,
    retired_revision: Option<GraphRevision>,
    swaps_applied: u64,
    retirement_retries: u64,
    incompatible_snapshots: u64,
}

impl GraphHandoffStatus {
    /// Creates initial status for the complete graph already owned by audio.
    pub const fn with_active(active_revision: GraphRevision) -> Self {
        Self {
            active_revision: Some(active_revision),
            retired_revision: None,
            swaps_applied: 0,
            retirement_retries: 0,
            incompatible_snapshots: 0,
        }
    }

    pub const fn active_revision(self) -> Option<GraphRevision> {
        self.active_revision
    }

    pub const fn retired_revision(self) -> Option<GraphRevision> {
        self.retired_revision
    }

    pub const fn swaps_applied(self) -> u64 {
        self.swaps_applied
    }

    pub const fn retirement_retries(self) -> u64 {
        self.retirement_retries
    }

    pub const fn incompatible_snapshots(self) -> u64 {
        self.incompatible_snapshots
    }

    /// Records one complete graph activation at a block boundary.
    pub fn record_swap(&mut self, active_revision: GraphRevision) {
        self.active_revision = Some(active_revision);
        self.swaps_applied = self.swaps_applied.saturating_add(1);
    }

    /// Records that the retirement queue now owns the replaced graph.
    pub fn record_retired(&mut self, retired_revision: GraphRevision) {
        self.retired_revision = Some(retired_revision);
    }

    /// Records one callback block where retirement pressure required a retry.
    pub fn record_retirement_retry(&mut self) {
        self.retirement_retries = self.retirement_retries.saturating_add(1);
    }

    /// Records one latest scalar snapshot rejected as graph-incompatible.
    pub fn record_incompatible_snapshot(&mut self) {
        self.incompatible_snapshots = self.incompatible_snapshots.saturating_add(1);
    }

    pub(crate) const fn from_raw_parts(
        active_revision: Option<GraphRevision>,
        retired_revision: Option<GraphRevision>,
        swaps_applied: u64,
        retirement_retries: u64,
        incompatible_snapshots: u64,
    ) -> Self {
        Self {
            active_revision,
            retired_revision,
            swaps_applied,
            retirement_retries,
            incompatible_snapshots,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GraphHandoffStatus;
    use crate::real_time::graph_revision::GraphRevision;

    #[test]
    fn status_is_fixed_copyable_and_saturating() {
        fn assert_copy<T: Copy>() {}

        let first = GraphRevision::new(1).unwrap();
        let second = GraphRevision::new(2).unwrap();
        let mut status = GraphHandoffStatus::with_active(first);
        status.record_swap(second);
        status.record_retired(first);
        status.record_retirement_retry();
        status.record_incompatible_snapshot();

        assert_copy::<GraphHandoffStatus>();
        assert!(!core::mem::needs_drop::<GraphHandoffStatus>());
        assert_eq!(status.active_revision(), Some(second));
        assert_eq!(status.retired_revision(), Some(first));
        assert_eq!(status.swaps_applied(), 1);
        assert_eq!(status.retirement_retries(), 1);
        assert_eq!(status.incompatible_snapshots(), 1);
    }
}
