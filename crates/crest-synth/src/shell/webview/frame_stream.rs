//! The qualifying-frame stream: the control-side seam that answers "has a
//! qualifying frame for this accepted identity been painted?" from recorded
//! evidence instead of from a sleep (mission webview-shell-cutover WP02,
//! T006).
//!
//! The window's painted-ack forwarding records every forwarded
//! [`ShellFrameObservation`] here, right where it also hands the observation
//! to the `AppWindow` port's frame callback. Control-side consumers hold a
//! [`QualifyingFrameStream`] handle cloned from
//! [`TauriWebviewWindow::frame_stream`] before the window runs and put that
//! question to it as a non-blocking [`poll`] from inside their tick loop.
//!
//! # What qualifies
//!
//! A recorded observation qualifies for a [`FrameExpectation`] when its
//! `generation` and `stateHash` equal the awaited accepted generation's and
//! its `context` and `activeSurface` equal the expectation's — the same
//! identity fields the retired live path's crediting started from
//! (`src/testing/live_demo_runner.rs`, `shell_frame_qualifies`). The stream
//! never fabricates or relaxes anything: it only replays observations the
//! forwarding actually constructed from painted acks.
//!
//! # No sleeps, bounded memory
//!
//! There is no polling loop and no sleep anywhere in this module: a poll
//! answers immediately from what the forwarding already recorded, and a
//! consumer that is not yet satisfied simply asks again on its next tick.
//! The stream retains only the most recent [`RECENT_OBSERVATION_CAPACITY`]
//! observations; an observation superseded past that window is display
//! evidence already consumed, and losing it degrades observation only.
//!
//! [`poll`]: QualifyingFrameStream::poll
//! [`TauriWebviewWindow::frame_stream`]: crate::shell::webview::TauriWebviewWindow::frame_stream

use crate::control::{SurfaceId, TopLevelContext};
use crate::shell::ShellFrameObservation;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// How many recent forwarded observations the stream retains for polling.
///
/// The live cadence forwards one observation per accepted generation, so a
/// consumer that polls every tick is at most one generation behind; eight
/// leaves generous room for a harness thread that wakes late.
pub const RECENT_OBSERVATION_CAPACITY: usize = 8;

/// The accepted-frame identity a consumer awaits: the accepted generation
/// and state hash, plus the context and active surface the projection
/// declared for it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameExpectation {
    generation: u64,
    state_hash: String,
    context: TopLevelContext,
    active_surface: SurfaceId,
}

impl FrameExpectation {
    /// Describes the accepted generation a qualifying frame must have
    /// painted.
    pub fn new(
        generation: u64,
        state_hash: impl Into<String>,
        context: TopLevelContext,
        active_surface: SurfaceId,
    ) -> Self {
        Self {
            generation,
            state_hash: state_hash.into(),
            context,
            active_surface,
        }
    }

    /// The awaited accepted generation.
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// The awaited accepted state hash.
    pub fn state_hash(&self) -> &str {
        &self.state_hash
    }

    /// The awaited top-level context.
    pub const fn context(&self) -> TopLevelContext {
        self.context
    }

    /// The awaited active surface.
    pub const fn active_surface(&self) -> SurfaceId {
        self.active_surface
    }

    /// True when the forwarded observation's identity satisfies this
    /// expectation (generation, state hash, context, and active surface all
    /// equal — mirroring the identity gate the retired live crediting applied).
    pub fn matches(&self, observation: &ShellFrameObservation) -> bool {
        observation.generation() == self.generation
            && observation.state_hash() == self.state_hash
            && observation.context() == self.context
            && observation.active_surface() == self.active_surface
    }
}

#[derive(Debug, Default)]
struct StreamShared {
    recent: Mutex<VecDeque<ShellFrameObservation>>,
}

/// Shared handle onto the window's forwarded-observation stream.
///
/// Clones share one underlying stream: the window records into it, any
/// number of control-side consumers poll it.
#[derive(Clone, Debug, Default)]
pub struct QualifyingFrameStream {
    shared: Arc<StreamShared>,
}

impl QualifyingFrameStream {
    /// Creates an empty stream.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one forwarded observation.
    ///
    /// Called by the window's painted-ack forwarding, next to the frame
    /// callback — the stream only ever sees observations that were
    /// constructed from a real painted ack. Nothing is woken: consumers ask
    /// [`poll`] again on their next tick.
    ///
    /// [`poll`]: QualifyingFrameStream::poll
    pub fn record(&self, observation: ShellFrameObservation) {
        let mut recent = self
            .shared
            .recent
            .lock()
            .expect("the frame stream lock is never poisoned");
        if recent.len() == RECENT_OBSERVATION_CAPACITY {
            recent.pop_front();
        }
        recent.push_back(observation);
    }

    /// Non-blocking: returns the most recent retained observation satisfying
    /// `expectation`, if one has been forwarded.
    pub fn poll(&self, expectation: &FrameExpectation) -> Option<ShellFrameObservation> {
        let recent = self
            .shared
            .recent
            .lock()
            .expect("the frame stream lock is never poisoned");
        recent
            .iter()
            .rev()
            .find(|observation| expectation.matches(observation))
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameExpectation, QualifyingFrameStream, RECENT_OBSERVATION_CAPACITY};
    use crate::control::TopLevelContext;
    use crate::shell::{
        ShellFrameObservation, ShellRegionId, ShellRegionObservation, ShellRegionRect,
    };

    fn observation(generation: u64, state_hash: &str) -> ShellFrameObservation {
        ShellFrameObservation::try_new(
            1920.0,
            1080.0,
            generation,
            state_hash,
            TopLevelContext::Mixer,
            [
                ShellRegionObservation::new(
                    ShellRegionId::ContextLine,
                    ShellRegionRect::new(0.0, 0.0, 1920.0, 48.0),
                    "CREST SYNTH",
                ),
                ShellRegionObservation::new(
                    ShellRegionId::IdentityHeader,
                    ShellRegionRect::new(0.0, 48.0, 1920.0, 120.0),
                    "MIXER",
                ),
                ShellRegionObservation::new(
                    ShellRegionId::MainWorkspace,
                    ShellRegionRect::new(0.0, 120.0, 1500.0, 1016.0),
                    "MIXER WORKSPACE",
                ),
                ShellRegionObservation::new(
                    ShellRegionId::PersistentSideRegion,
                    ShellRegionRect::new(1500.0, 120.0, 1920.0, 1016.0),
                    "INSPECTOR",
                ),
                ShellRegionObservation::new(
                    ShellRegionId::Footer,
                    ShellRegionRect::new(0.0, 1016.0, 1920.0, 1080.0),
                    "MIXER",
                ),
            ],
        )
        .expect("the stream test fixture observation is coherent")
    }

    fn expectation_for(observation: &ShellFrameObservation) -> FrameExpectation {
        FrameExpectation::new(
            observation.generation(),
            observation.state_hash(),
            observation.context(),
            observation.active_surface(),
        )
    }

    #[test]
    fn a_recorded_qualifying_observation_satisfies_poll() {
        let stream = QualifyingFrameStream::new();
        let painted = observation(7, "state-7");
        let expectation = expectation_for(&painted);

        assert!(stream.poll(&expectation).is_none(), "nothing painted yet");

        stream.record(painted.clone());

        let polled = stream.poll(&expectation).expect("poll sees the frame");
        assert_eq!(polled, painted);
    }

    #[test]
    fn a_non_matching_observation_does_not_qualify() {
        let stream = QualifyingFrameStream::new();
        stream.record(observation(7, "state-7"));

        // Wrong generation and wrong state hash each fail the identity gate.
        let stale_generation = FrameExpectation::new(
            8,
            "state-7",
            TopLevelContext::Mixer,
            observation(7, "state-7").active_surface(),
        );
        assert!(stream.poll(&stale_generation).is_none());

        let stale_hash = FrameExpectation::new(
            7,
            "state-8",
            TopLevelContext::Mixer,
            observation(7, "state-7").active_surface(),
        );
        assert!(stream.poll(&stale_hash).is_none());
    }

    #[test]
    fn a_frame_recorded_through_one_clone_is_visible_through_another() {
        let stream = QualifyingFrameStream::new();
        let painted = observation(9, "state-9");
        let expectation = expectation_for(&painted);

        let recorder = {
            let stream = stream.clone();
            let painted = painted.clone();
            std::thread::spawn(move || stream.record(painted))
        };
        recorder.join().expect("the recorder thread completes");

        // Clones share one underlying stream: the window records into its
        // handle, and a control-side consumer holding a different clone sees
        // the same observation.
        let polled = stream
            .poll(&expectation)
            .expect("the clone's recording is visible to the original handle");
        assert_eq!(polled, painted);
    }

    #[test]
    fn the_stream_retains_only_the_declared_recent_window() {
        let stream = QualifyingFrameStream::new();
        let first = observation(1, "state-1");
        stream.record(first.clone());
        for generation in 2..=(RECENT_OBSERVATION_CAPACITY as u64 + 1) {
            stream.record(observation(generation, &format!("state-{generation}")));
        }

        // The oldest observation fell out of the bounded window; the newest
        // is still visible. Losing a superseded frame degrades observation
        // only.
        assert!(stream.poll(&expectation_for(&first)).is_none());
        let newest = observation(
            RECENT_OBSERVATION_CAPACITY as u64 + 1,
            &format!("state-{}", RECENT_OBSERVATION_CAPACITY as u64 + 1),
        );
        assert!(stream.poll(&expectation_for(&newest)).is_some());
    }
}
