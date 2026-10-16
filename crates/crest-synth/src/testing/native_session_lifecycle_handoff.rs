/// Outcome of the scoped native document-lifecycle handoff. Environmental
/// inability to host native application chrome is distinct from a failed
/// journey and never reported as completion.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum NativeSessionLifecycleHandoffReport {
    Completed {
        evidence: NativeSessionLifecycleHandoffEvidence,
    },
    Failed {
        reason: String,
        evidence: NativeSessionLifecycleHandoffEvidence,
    },
    EnvironmentalSkip {
        reason: String,
    },
}

impl NativeSessionLifecycleHandoffReport {
    pub fn from_evidence(evidence: NativeSessionLifecycleHandoffEvidence) -> Self {
        if evidence.complete() {
            Self::Completed { evidence }
        } else {
            Self::Failed {
                reason: "one or more scoped native lifecycle checks were incomplete".to_owned(),
                evidence,
            }
        }
    }

    pub fn environmental_skip(reason: impl Into<String>) -> Self {
        Self::EnvironmentalSkip {
            reason: reason.into(),
        }
    }

    pub const fn complete(&self) -> bool {
        matches!(self, Self::Completed { .. })
    }
}

/// Exact bounded evidence for Phase 01's native handoff. It deliberately
/// claims no broad visual or Figma parity.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeSessionLifecycleHandoffEvidence {
    pub menu_shortcuts_available: bool,
    pub new_cancelled: bool,
    pub save_as_completed: bool,
    pub open_round_trip_completed: bool,
    pub dirty_close_cancelled: bool,
    pub dirty_close_saved: bool,
    pub dirty_close_discarded: bool,
    pub controlled_failure_visible: bool,
    pub clean_teardown: bool,
}

impl NativeSessionLifecycleHandoffEvidence {
    pub const fn complete(self) -> bool {
        self.menu_shortcuts_available
            && self.new_cancelled
            && self.save_as_completed
            && self.open_round_trip_completed
            && self.dirty_close_cancelled
            && self.dirty_close_saved
            && self.dirty_close_discarded
            && self.controlled_failure_visible
            && self.clean_teardown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_never_confuses_failure_or_environmental_skip_with_completion() {
        let incomplete = NativeSessionLifecycleHandoffEvidence::default();
        assert!(!NativeSessionLifecycleHandoffReport::from_evidence(incomplete).complete());
        assert!(!NativeSessionLifecycleHandoffReport::environmental_skip(
            "window server unavailable"
        )
        .complete());
        let complete = NativeSessionLifecycleHandoffEvidence {
            menu_shortcuts_available: true,
            new_cancelled: true,
            save_as_completed: true,
            open_round_trip_completed: true,
            dirty_close_cancelled: true,
            dirty_close_saved: true,
            dirty_close_discarded: true,
            controlled_failure_visible: true,
            clean_teardown: true,
        };
        assert!(NativeSessionLifecycleHandoffReport::from_evidence(complete).complete());
    }
}
