//! Optional native UI receipt-to-paint diagnostics. Never used by the audio path.
use crest_tracker::{
    composition::{EventId, PhraseId, Target},
    editing::Occurrence,
    time::Beat,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Instant;

const MAX_SAMPLES: usize = 10_000;
const TIMEOUT_NS: u64 = 2_000_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Surface {
    #[default]
    Grid,
    Piano,
    PianoSelection,
    PianoEdit,
    Drum,
    DrumEdit,
    Timeline,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Focus {
    Timeline {
        occurrence: Occurrence,
        phrase: PhraseId,
        start: Beat,
        row: Option<usize>,
        at: Option<Beat>,
        cells: Vec<String>,
    },
    Grid {
        row: usize,
        column: usize,
        label: String,
    },
    Piano {
        phrase: PhraseId,
        at: Beat,
        pitch: u8,
    },
    PianoSelection {
        phrase: PhraseId,
        at: Beat,
        pitch: u8,
        selected: Option<EventId>,
    },
    PianoEdit {
        phrase: PhraseId,
        notes: Vec<EditedNote>,
    },
    Drum {
        phrase: PhraseId,
        step: usize,
        lane: usize,
        pitch: u8,
        target: Target,
    },
    DrumEdit {
        phrase: PhraseId,
        step: usize,
        lane: usize,
        pitch: u8,
        target: Target,
        hits: Vec<EventId>,
    },
}
impl Focus {
    fn surface(&self) -> Surface {
        match self {
            Self::Grid { .. } => Surface::Grid,
            Self::Timeline { .. } => Surface::Timeline,
            Self::Piano { .. } => Surface::Piano,
            Self::PianoSelection { .. } => Surface::PianoSelection,
            Self::PianoEdit { .. } => Surface::PianoEdit,
            Self::Drum { .. } => Surface::Drum,
            Self::DrumEdit { .. } => Surface::DrumEdit,
        }
    }
}

/// The rendered fields of selected stored notes, excluding unrelated raw curves.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EditedNote {
    pub id: EventId,
    pub at: Beat,
    pub pitch: u8,
    pub duration: Beat,
    pub velocity: u8,
    pub release_velocity: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PaintState {
    pub view: String,
    pub revision: u64,
    pub playing: bool,
    pub focus: Focus,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Origin {
    NativeKeyboard,
    Webview,
}

#[derive(Debug)]
struct Pending {
    sequence: u64,
    origin: Origin,
    received_ns: u64,
    before: PaintState,
    processed: Option<PaintState>,
}

#[derive(Serialize)]
struct Sample {
    sequence: u64,
    origin: Origin,
    receipt_to_ack_ns: u64,
    state: PaintState,
    bounds: [f64; 4],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PaintAck {
    pub kind: String,
    pub sequence: u64,
    pub state: PaintState,
    pub bounds: [f64; 4],
    pub viewport: [f64; 2],
}

#[derive(Default)]
pub(crate) struct Feedback {
    started: Option<Instant>,
    surface: Surface,
    sequence: u64,
    pending: Option<Pending>,
    samples: Vec<Sample>,
    failures: Vec<String>,
    no_changes: usize,
    capacity_exceeded: bool,
}

impl Feedback {
    pub fn enable(&mut self, surface: Surface) {
        self.surface = surface;
        self.started = Some(Instant::now());
    }

    pub fn enabled(&self) -> bool {
        self.started.is_some()
    }

    pub fn surface(&self) -> Surface {
        self.surface
    }

    fn now_ns(&self) -> u64 {
        self.started
            .map(|started| started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64)
            .unwrap_or(0)
    }

    pub fn begin(&mut self, before: PaintState, origin: Origin, received: Instant) -> Option<u64> {
        if !self.enabled() || before.focus.surface() != self.surface {
            return None;
        }
        self.reject_pending("Another measured action arrived before paint acknowledgement");
        if self.sequence >= MAX_SAMPLES as u64 {
            self.capacity_exceeded = true;
            return None;
        }
        self.sequence += 1;
        self.pending = Some(Pending {
            sequence: self.sequence,
            origin,
            received_ns: received
                .saturating_duration_since(self.started.expect("enabled capture"))
                .as_nanos()
                .min(u128::from(u64::MAX)) as u64,
            before,
            processed: None,
        });
        Some(self.sequence)
    }

    pub fn processed(&mut self, sequence: u64, after: Option<PaintState>) {
        let Some(pending) = &mut self.pending else {
            return;
        };
        if pending.sequence != sequence {
            return;
        }
        let Some(after) = after else {
            self.reject_pending("Measured action left the supported surface");
            return;
        };
        if self.surface == Surface::PianoSelection && after.revision != pending.before.revision {
            self.reject_pending("Selection capture changed source content");
            return;
        }
        if self.surface == Surface::PianoEdit
            && (after.revision == pending.before.revision || after.focus == pending.before.focus)
        {
            self.pending = None;
            self.no_changes += 1;
            return;
        }
        if pending.before == after {
            self.pending = None;
            self.no_changes += 1;
        } else {
            pending.processed = Some(after);
        }
    }

    pub fn reject_pending(&mut self, reason: &str) {
        if let Some(pending) = self.pending.take() {
            self.failures
                .push(format!("{}: {reason}", pending.sequence));
        }
    }

    pub fn expire(&mut self) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| self.now_ns().saturating_sub(pending.received_ns) >= TIMEOUT_NS)
        {
            self.reject_pending("Paint acknowledgement timed out");
        }
    }

    pub fn projection(&self) -> Value {
        let pending = self.pending.as_ref().and_then(|pending| {
            pending
                .processed
                .as_ref()
                .map(|state| json!({"sequence":pending.sequence,"state":state}))
        });
        let keyboard_playback = self
            .samples
            .iter()
            .filter(|sample| {
                sample.state.playing && matches!(sample.origin, Origin::NativeKeyboard)
            })
            .count();
        json!({"enabled":self.enabled(),"pending":pending,"samples":self.samples.len(),
            "nativeKeyboardPlaybackSamples":keyboard_playback,"failures":self.failures.len()})
    }

    pub fn acknowledge(&mut self, ack: PaintAck) {
        self.acknowledge_at(ack, self.now_ns());
    }

    fn acknowledge_at(&mut self, ack: PaintAck, now_ns: u64) {
        let Some(pending) = &self.pending else { return };
        // A stale frame cannot satisfy or retire a newer action.
        if pending.sequence != ack.sequence {
            return;
        }
        if ack.kind != "feedback_painted"
            || pending.processed.as_ref() != Some(&ack.state)
            || !visible_bounds(ack.bounds, ack.viewport)
        {
            self.reject_pending("Paint identity or visible geometry did not match");
            return;
        }
        let elapsed = now_ns.saturating_sub(pending.received_ns);
        if elapsed >= TIMEOUT_NS {
            self.reject_pending("Paint acknowledgement timed out");
            return;
        }
        let pending = self.pending.take().expect("matching pending sample");
        self.samples.push(Sample {
            sequence: pending.sequence,
            origin: pending.origin,
            receipt_to_ack_ns: elapsed,
            state: ack.state,
            bounds: ack.bounds,
        });
    }

    pub fn report(&mut self, application_completed: bool) -> Value {
        self.reject_pending("Application closed before paint acknowledgement");
        let playback: Vec<_> = self
            .samples
            .iter()
            .filter(|sample| sample.state.playing)
            .collect();
        let keyboard: Vec<_> = playback
            .iter()
            .copied()
            .filter(|sample| matches!(sample.origin, Origin::NativeKeyboard))
            .collect();
        let p95 = percentile95(&playback);
        let keyboard_p95 = percentile95(&keyboard);
        let passed = application_completed
            && self.failures.is_empty()
            && !self.capacity_exceeded
            && keyboard.len() >= 100
            && p95.is_some_and(|value| value < 50_000_000)
            && keyboard_p95.is_some_and(|value| value < 50_000_000);
        json!({
            "schema_version":2,"scope":"tracker_native_feedback","surface":self.surface,
            "complete":application_completed,"performance_passed":passed,
            "measurement":"native input/event receipt to matching double-animation-frame acknowledgement",
            "limitations":"Includes return IPC; excludes physical input, GPU presentation and display scanout. Covers only the selected surface: grid/Piano/Drum/Expanded/Minimap cursor navigation, Piano single-note selection, selected Piano note edits, or Drum hit insertion/removal/undo/redo. Does not certify selection-range changes, or Synth parameter edits.",
            "attempts":self.sequence,"no_changes":self.no_changes,
            "capacity_exceeded":self.capacity_exceeded,"failures":self.failures,
            "playback_samples":playback.len(),"playback_p95_ns":p95,
            "native_keyboard_playback_samples":keyboard.len(),
            "native_keyboard_playback_p95_ns":keyboard_p95,"samples":self.samples
        })
    }
}

fn percentile95(samples: &[&Sample]) -> Option<u64> {
    let mut values: Vec<_> = samples
        .iter()
        .map(|sample| sample.receipt_to_ack_ns)
        .collect();
    values.sort_unstable();
    values
        .get((values.len() * 95).div_ceil(100).saturating_sub(1))
        .copied()
}

fn visible_bounds([x, y, width, height]: [f64; 4], [vw, vh]: [f64; 2]) -> bool {
    [x, y, width, height, vw, vh]
        .iter()
        .all(|value| value.is_finite())
        && width > 0.0
        && height > 0.0
        && vw > 0.0
        && vh > 0.0
        && x >= 0.0
        && y >= 0.0
        && x + width <= vw + 1.0
        && y + height <= vh + 1.0
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state(row: usize) -> PaintState {
        PaintState {
            view: "song".into(),
            revision: 0,
            focus: Focus::Grid {
                row,
                column: 0,
                label: format!("Track 1, row {}: 78", row + 1),
            },
            playing: true,
        }
    }
    fn ack(sequence: u64, row: usize) -> PaintAck {
        PaintAck {
            kind: "feedback_painted".into(),
            sequence,
            state: state(row),
            bounds: [10.0, 20.0, 100.0, 24.0],
            viewport: [800.0, 600.0],
        }
    }
    #[test]
    fn only_processed_matching_visible_state_finishes_an_action() {
        let mut feedback = Feedback::default();
        feedback.enable(Surface::Grid);
        let sequence = feedback
            .begin(state(0), Origin::NativeKeyboard, Instant::now())
            .unwrap();
        feedback.processed(sequence, Some(state(1)));
        feedback.acknowledge_at(ack(sequence + 1, 1), 1_000_000);
        assert!(feedback.samples.is_empty());
        feedback.acknowledge_at(ack(sequence, 1), 1_000_000);
        assert_eq!(feedback.samples.len(), 1);
        assert!(feedback.failures.is_empty());
    }
    #[test]
    fn unrelated_frame_hidden_geometry_and_timeout_cannot_pass() {
        for case in 0..3 {
            let mut feedback = Feedback::default();
            feedback.enable(Surface::Grid);
            let sequence = feedback
                .begin(state(0), Origin::NativeKeyboard, Instant::now())
                .unwrap();
            feedback.processed(sequence, Some(state(1)));
            let mut painted = ack(sequence, 1);
            if case == 0 {
                painted.state = state(2);
            }
            if case == 1 {
                painted.bounds[2] = 0.0;
            }
            feedback.acknowledge_at(
                painted,
                if case == 2 {
                    TIMEOUT_NS + 1_000_000
                } else {
                    1_000_000
                },
            );
            assert!(feedback.samples.is_empty());
            assert_eq!(feedback.failures.len(), 1);
        }
    }
    #[test]
    fn missing_and_coalesced_actions_fail_instead_of_biasing_percentiles() {
        let mut feedback = Feedback::default();
        feedback.enable(Surface::Grid);
        feedback.begin(state(0), Origin::NativeKeyboard, Instant::now());
        feedback.begin(state(1), Origin::NativeKeyboard, Instant::now());
        let report = feedback.report(true);
        assert_eq!(report["failures"].as_array().unwrap().len(), 2);
        assert_eq!(report["performance_passed"], false);
    }
    #[test]
    fn no_change_and_stopped_samples_cannot_satisfy_playback_gate() {
        let mut feedback = Feedback::default();
        feedback.enable(Surface::Grid);
        let sequence = feedback
            .begin(state(0), Origin::NativeKeyboard, Instant::now())
            .unwrap();
        feedback.processed(sequence, Some(state(0)));
        let report = feedback.report(true);
        assert_eq!(report["no_changes"], 1);
        assert_eq!(report["performance_passed"], false);
    }

    #[test]
    fn live_counter_excludes_stopped_and_webview_actions() {
        let mut feedback = Feedback::default();
        feedback.enable(Surface::Grid);
        for (playing, origin) in [
            (false, Origin::NativeKeyboard),
            (true, Origin::Webview),
            (true, Origin::NativeKeyboard),
        ] {
            let mut before = state(0);
            before.playing = playing;
            let sequence = feedback.begin(before, origin, Instant::now()).unwrap();
            let mut receipt = ack(sequence, 1);
            receipt.state.playing = playing;
            feedback.processed(sequence, Some(receipt.state.clone()));
            feedback.acknowledge_at(receipt, 1_000_000);
        }
        let live = feedback.projection();
        assert_eq!(live["samples"], 3);
        assert_eq!(live["nativeKeyboardPlaybackSamples"], 1);
        assert_eq!(feedback.report(true)["native_keyboard_playback_samples"], 1);
    }

    #[test]
    fn piano_edit_requires_changed_source_and_selected_note_values() {
        let before = PaintState {
            view: "phrase".into(),
            revision: 4,
            playing: true,
            focus: Focus::PianoEdit {
                phrase: PhraseId::new(104).unwrap(),
                notes: vec![EditedNote {
                    id: EventId::new(9_007_199_254_740_993).unwrap(),
                    at: Beat::new(1, 3).unwrap(),
                    pitch: 60,
                    duration: Beat::new(1, 4).unwrap(),
                    velocity: 88,
                    release_velocity: 45,
                }],
            },
        };
        let mut feedback = Feedback::default();
        feedback.enable(Surface::PianoEdit);
        let mut changed = before.clone();
        if let Focus::PianoEdit { notes, .. } = &mut changed.focus {
            notes[0].velocity = 89;
        }
        // Selection-only changes or unrelated document edits are not note-edit evidence.
        for mut after in [before.clone(), changed.clone()] {
            if after.focus == before.focus {
                after.revision += 1;
            }
            let sequence = feedback
                .begin(before.clone(), Origin::NativeKeyboard, Instant::now())
                .unwrap();
            feedback.processed(sequence, Some(after));
            assert!(feedback.pending.is_none());
        }
        assert_eq!(feedback.no_changes, 2);
        changed.revision += 1;
        let sequence = feedback
            .begin(before, Origin::NativeKeyboard, Instant::now())
            .unwrap();
        feedback.processed(sequence, Some(changed.clone()));
        let mut painted = ack(sequence, 0);
        painted.state = changed;
        feedback.acknowledge_at(painted, 1_000_000);
        assert_eq!(feedback.samples.len(), 1);
        assert!(feedback.failures.is_empty());
    }

    #[test]
    fn piano_capture_requires_its_processed_exact_cursor_and_excludes_grids() {
        let mut feedback = Feedback::default();
        feedback.enable(Surface::Piano);
        assert!(feedback
            .begin(state(0), Origin::NativeKeyboard, Instant::now())
            .is_none());
        let piano = |at| PaintState {
            view: "phrase".into(),
            revision: 4,
            playing: true,
            focus: Focus::Piano {
                phrase: PhraseId::new(9_007_199_254_740_993).unwrap(),
                at,
                pitch: 67,
            },
        };
        let before = piano(Beat::ZERO);
        let after = piano(Beat::new(1, 4).unwrap());
        let sequence = feedback
            .begin(before, Origin::NativeKeyboard, Instant::now())
            .unwrap();
        feedback.processed(sequence, Some(after.clone()));
        let mut painted = ack(sequence, 0);
        painted.state = piano(Beat::new(1, 3).unwrap());
        feedback.acknowledge_at(painted, 1_000_000);
        assert_eq!(feedback.failures.len(), 1);
        assert!(feedback.samples.is_empty());

        let sequence = feedback
            .begin(piano(Beat::ZERO), Origin::NativeKeyboard, Instant::now())
            .unwrap();
        feedback.processed(sequence, Some(after.clone()));
        let mut painted = ack(sequence, 0);
        painted.state = after;
        feedback.acknowledge_at(painted, 1_000_000);
        assert_eq!(feedback.samples.len(), 1);
        assert_eq!(feedback.report(true)["surface"], "piano");
    }
}
