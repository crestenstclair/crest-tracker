use crate::control::app_event::AppEvent;
use crate::control::app_loop::AppLoop;
use crate::control::app_state::EventRejection;
use crate::control::event_record::{
    AudioEffect, EmittedEvent, EventInput, EventOutcome, EventRecord, EventSource,
};
use crate::control::state_tree::StateTree;
use crate::control::text_projection::TextProjection;
use crate::control::{
    Direction, EngineSelectionEffectKind, EngineSelectionRequestId, EngineSelectionStatusKind,
    FocusPath, GraphicalShellProjection, PatchControlId, SemanticResolver, StructuralEditIntent,
    SurfaceId, TopLevelContext,
};
use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crate::real_time::audio_boundary::ControlAudioBoundary;
use crate::real_time::audio_observation::ControlAudioObservation;
use crate::real_time::audio_observation_snapshot::AudioObservationSnapshot;
use crate::shell::{ShellFrameObservation, ShellRegionId};
use crate::synth::effect_slot_id::MAX_EFFECT_SLOTS;
use crate::synth::{
    InstrumentConfig, ParameterDefault, ParameterKind, ParameterValue, PostEffectConfig,
    VoiceEnvelope,
};
use crate::testing::automatic_midi_test::{AutomaticMidiTest, TestInputError};
use crate::testing::functional_patch_editor_observation::{
    per_track_rms_deltas, ObservedAudibleEdit, ObservedOccupancy, PatchEditorMeasurement,
};
use crate::testing::live_demo_checkpoint::{
    LiveCheckpoint, LiveDemoCheckpoint, LiveDemoCheckpointError, LiveEngineCheckpoint,
    LivePresetProjection, LiveTopologyCheckpoint,
};
use crate::testing::live_demo_report::{
    LiveDemoCoverage, LiveDemoReport, LiveDemoReportError, LiveDetailAssetsEvidence,
    LiveShellCoverage, RuntimeAudioWitness,
};
use crate::testing::live_demo_scene::{
    selected_parameter_value, LiveDemoScene, LiveDemoSceneError, LiveDemoStep,
    LiveEngineTransition, LiveExpectedTransition,
};
use crate::testing::midi_event_source::MidiEventSource;
use core::fmt;
use std::time::Duration;

/// Maximum elapsed control time without a qualifying autonomous milestone.
pub const LIVE_DEMO_NO_PROGRESS_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum elapsed control time for the complete autonomous live scene.
pub const LIVE_DEMO_TOTAL_TIMEOUT: Duration = Duration::from_secs(120);
/// How many recent per-generation projections the runner retains so an
/// asynchronously forwarded painted frame (mission webview-shell-cutover
/// WP03) can be correlated against the exact projection it painted. The
/// webview shell acks within a frame or two of each push, so the live
/// cadence stays one or two generations behind at most; eight mirrors the
/// forwarding side's own bounded in-flight tracker. A frame superseded past
/// this window degrades observation only.
const RECENT_PROJECTION_CAPACITY: usize = 8;

/// Control-thread state machine for the real-window live scene.
pub struct LiveDemoRunner<Source, Observation> {
    automatic_midi: AutomaticMidiTest<Source>,
    observation: Observation,
    scene: LiveDemoScene,
    step_index: usize,
    elapsed: Duration,
    last_progress_at: Duration,
    deferred_fixture_elapsed: Duration,
    /// The newest painted accepted generation observed through the shell's
    /// forwarded frame seam (mission webview-shell-cutover WP03). Monotone:
    /// a painted document proves its generation and every earlier one were
    /// produced and displayed, so visible-projection checkpoints gate on
    /// `painted_generation >= dispatched generation` — real paint evidence,
    /// never a tick count or wall-clock stand-in.
    painted_generation: u64,
    /// The accepted generation of the most recent semantic-tail step whose
    /// projection has not yet been proven painted. The retired shell painted
    /// every tick, so "one step per tick" implicitly meant "one step per
    /// painted frame"; on the webview shell the next tail step dispatches
    /// only after the forwarded frame seam proves the previous one visible
    /// (mission webview-shell-cutover WP03). Dispatch stalls, the fixture
    /// holds, and the frozen generation is always the page's newest — the
    /// same await-the-paint rule the parameter checkpoints use.
    awaiting_step_paint: Option<u64>,
    pending_checkpoint: Option<PendingCheckpoint>,
    checkpoints: Vec<LiveCheckpoint>,
    coverage: LiveDemoCoverage,
    shell_coverage: LiveShellCoverage,
    /// The bounded per-generation window of recent canonical projections,
    /// newest at the back. Painted-frame correlation looks a frame's own
    /// generation up here, because forwarded webview acks arrive after the
    /// control side may have accepted a newer generation.
    recent_shells: std::collections::VecDeque<GraphicalShellProjection>,
    engine_transition_index: usize,
    engine_phase: LiveEnginePhase,
    topology_transition_index: usize,
    topology_phase: LiveTopologyPhase,
    pending_projection_measure: Option<PendingProjectionMeasure>,
    last_ready_graph_revision: crate::real_time::GraphRevision,
    engine_envelope_baseline: Option<VoiceEnvelope>,
    structural_config_baseline: Option<InstrumentConfig>,
    structural_effect_baseline: Option<[Option<PostEffectConfig>; MAX_EFFECT_SLOTS]>,
    pending_focus_recovery: Option<(FocusPath, Vec<FocusPath>)>,
    cleanup_sequence_before: Option<u64>,
    completed_report: Option<LiveDemoReport>,
    runtime_audio: RuntimeAudioWitness,
    aborted: bool,
    /// Present only for the functional Patch editor scene. It samples the
    /// canonical projection the shell is already painting, so measuring costs
    /// no extra projection.
    patch_editor: Option<PatchEditorMeasurement>,
    detail_and_assets: Option<LiveDetailAssetsEvidence>,
}

impl<Source, Observation> LiveDemoRunner<Source, Observation>
where
    Source: MidiEventSource,
    Observation: ControlAudioObservation,
{
    /// Starts around an already initialized fixture service. Construction does
    /// not mutate AppState or open an audio/window device.
    pub fn start(
        scene: LiveDemoScene,
        automatic_midi: AutomaticMidiTest<Source>,
        observation: Observation,
        runtime_audio: RuntimeAudioWitness,
    ) -> Self {
        let coverage = LiveDemoCoverage::with_engine_and_topology_transitions(
            scene.expected_editable_parameters(),
            scene.expected_engine_transitions(),
            scene.expected_topology_transitions(),
        );
        let measures_patch_editor = scene.measures_patch_editor();
        let detail_and_assets = scene
            .detail_assets_subject()
            .map(|(patch, track, asset)| LiveDetailAssetsEvidence::new(patch, track, asset));
        let last_ready_graph_revision = runtime_audio.active_graph_revision();
        Self {
            automatic_midi,
            observation,
            scene,
            step_index: 0,
            elapsed: Duration::ZERO,
            last_progress_at: Duration::ZERO,
            deferred_fixture_elapsed: Duration::ZERO,
            painted_generation: 0,
            awaiting_step_paint: None,
            pending_checkpoint: None,
            checkpoints: Vec::new(),
            coverage,
            shell_coverage: LiveShellCoverage::default(),
            recent_shells: std::collections::VecDeque::new(),
            engine_transition_index: 0,
            engine_phase: LiveEnginePhase::SelectPatch,
            topology_transition_index: 0,
            topology_phase: LiveTopologyPhase::Support { index: 0 },
            pending_projection_measure: None,
            last_ready_graph_revision,
            engine_envelope_baseline: None,
            structural_config_baseline: None,
            structural_effect_baseline: None,
            pending_focus_recovery: None,
            cleanup_sequence_before: None,
            completed_report: None,
            runtime_audio,
            aborted: false,
            patch_editor: measures_patch_editor.then(PatchEditorMeasurement::default),
            detail_and_assets,
        }
    }

    /// Advances at most one scene AppEvent and never sleeps or blocks.
    pub fn advance<Boundary>(
        &mut self,
        elapsed: Duration,
        app_loop: &mut AppLoop<Boundary>,
    ) -> Result<Option<LiveCheckpoint>, LiveDemoError>
    where
        Boundary: ControlAudioBoundary,
    {
        if self.completed_report.is_some() || self.aborted {
            return Ok(None);
        }

        self.elapsed = self.elapsed.saturating_add(elapsed);
        self.deferred_fixture_elapsed = self.deferred_fixture_elapsed.saturating_add(elapsed);

        if self.elapsed >= self.scene.total_timeout() {
            return Err(LiveDemoError::TotalTimeout {
                elapsed: self.elapsed,
                stage: self.progress_stage(),
                step: self.step_index,
            });
        }

        let result = self.advance_current(app_loop);
        let shell = app_loop.current_graphical_shell();
        if self.patch_editor.is_some() {
            // The projection the shell is painting, sampled every advance: the
            // measured facts are gathered across the whole run rather than at
            // one convenient moment, and no second projection is built.
            let audio = self.observation.read_latest_on_control();
            let state = app_loop.state();
            if let Some(measurement) = self.patch_editor.as_mut() {
                measurement.observe_projection(state, shell.semantic_model());
                measurement.observe_audio(audio);
            }
        }
        if let Some(measurement) = self.detail_and_assets.as_mut() {
            let audio = self.observation.read_latest_on_control();
            measurement.observe(app_loop.state(), &shell, audio);
        }
        if self
            .recent_shells
            .back()
            .is_none_or(|recent| recent.generation() != shell.generation())
        {
            if self.recent_shells.len() == RECENT_PROJECTION_CAPACITY {
                self.recent_shells.pop_front();
            }
            self.recent_shells.push_back(shell);
        }
        if result.is_ok()
            && self.completed_report.is_none()
            && !self.aborted
            && self.elapsed.saturating_sub(self.last_progress_at) >= LIVE_DEMO_NO_PROGRESS_TIMEOUT
        {
            return Err(LiveDemoError::ProgressTimedOut {
                stage: self.progress_stage(),
                stalled_for: self.elapsed.saturating_sub(self.last_progress_at),
                step: self.pending_checkpoint.as_ref().map(|pending| pending.step),
                predicate: self
                    .pending_checkpoint
                    .as_ref()
                    .map(|pending| pending.predicate),
            });
        }
        result
    }

    /// Correlates one production-painted shell frame with canonical state and
    /// the latest bounded production-path audio observation.
    ///
    /// On the webview shell (mission webview-shell-cutover WP03) frames
    /// arrive through WP02's forwarded-observation seam: the window
    /// constructs each observation from a painted ack the projection channel
    /// already matched field for field against the pushed document, then
    /// hands it to the port's frame callback, which lands here. Forwarding
    /// is asynchronous, so an ack may arrive after the control side has
    /// already accepted a newer generation: such a superseded painted frame
    /// is correlated against the retained canonical projection of its own
    /// generation and credited under the asynchronous audio rule (the
    /// callback can legitimately have consumed a newer parameter generation
    /// by then). A frame superseded past the bounded retention window is
    /// display evidence only — it advances the painted-generation seam and
    /// the projection-visibility measure, exactly like a lost meter frame
    /// degrades display only. A frame claiming a generation the control
    /// side has not produced remains fabricated evidence and stays a typed
    /// rejection.
    pub fn observe_shell_frame(
        &mut self,
        frame: ShellFrameObservation,
    ) -> Result<(), LiveDemoError> {
        if self.completed_report.is_some() || self.aborted {
            return Err(LiveDemoError::UnexpectedShellFrame);
        }
        if let Some(pending) = &mut self.pending_projection_measure {
            // The NFR-008 window counts frames painted AFTER the acceptance.
            // Generations are monotone and documents are pushed and acked in
            // order, so a post-acceptance paint always carries a generation
            // at or above the accepted one; an asynchronously forwarded
            // straggler below it is a pre-acceptance paint (WP03) and is not
            // part of the window. A shell that painted a stale document
            // after acceptance would never resolve this measure and fails
            // the run through the no-progress guard.
            if pending.resolved.is_none() && frame.generation() >= pending.generation {
                pending.frames_seen = pending.frames_seen.saturating_add(1);
                pending.resolved = Some(pending.frames_seen);
            }
        }
        let latest_generation = self
            .recent_shells
            .back()
            .ok_or(LiveDemoError::MissingShellProjection)?
            .generation();
        if frame.generation() > latest_generation {
            // A frame for a generation the control side has not produced
            // cannot be painted evidence of any pushed document.
            return Err(LiveDemoError::ShellFrameMismatch);
        }
        let audio = self.observation.read_latest_on_control();
        let qualifies = if frame.generation() == latest_generation {
            let shell = self
                .recent_shells
                .back()
                .expect("the retained window was just read");
            shell_frame_qualifies(&frame, shell, audio)?
        } else if let Some(shell) = self
            .recent_shells
            .iter()
            .rev()
            .find(|shell| shell.generation() == frame.generation())
        {
            superseded_shell_frame_qualifies(&frame, shell, audio)?
        } else {
            // Superseded past the bounded retention window: authentic paint
            // evidence too old to correlate — observation degrades only.
            false
        };
        if qualifies {
            self.shell_coverage.mark_qualifying_frame(&frame);
        }
        self.painted_generation = self.painted_generation.max(frame.generation());
        Ok(())
    }

    fn advance_current<Boundary>(
        &mut self,
        app_loop: &mut AppLoop<Boundary>,
    ) -> Result<Option<LiveCheckpoint>, LiveDemoError>
    where
        Boundary: ControlAudioBoundary,
    {
        if self.pending_checkpoint.is_some() {
            return self.advance_pending(app_loop);
        }

        if self.step_index == self.scene.scalar_step_count()
            && self.engine_phase != LiveEnginePhase::Complete
        {
            return self.advance_engine(app_loop);
        }

        if self.step_index == self.scene.scalar_step_count()
            && self.topology_transition_index < self.scene.expected_topology_transitions().len()
        {
            return self.advance_topology(app_loop);
        }

        // Phase 7's preview interval is an observed transport predicate, not
        // merely two dispatched gestures. Freeze the semantic tail while the
        // compatible Playing block and then its de-clicked Idle block cross
        // the callback observation seam. Without this gate, a MIDI dwell event
        // advanced canonical generation on every tick and could keep the
        // otherwise-valid audio observation perpetually one generation behind.
        if let Some(evidence) = self.detail_and_assets.as_ref() {
            let preview = app_loop.state().file_browser().preview();
            if (matches!(preview, crate::control::SamplePreviewState::Playing { .. })
                && !evidence.preview_revision_compatible())
                || (evidence.preview_revision_compatible()
                    && matches!(preview, crate::control::SamplePreviewState::Idle)
                    && !evidence.preview_release_observed())
            {
                return Ok(None);
            }
        }

        // The semantic visibility tail (the steps after the scalar phase)
        // is where surface, mode, and return-trip coverage is harvested,
        // and several of its states last exactly one accepted generation.
        // Before advancing past one, the shell must have painted it: stall
        // (fixture held, generation frozen as the page's newest) until the
        // forwarded frame seam confirms the paint. No sleep — the stall is
        // the event loop's own cadence, bounded by the no-progress guard.
        if self.step_index >= self.scene.scalar_step_count() {
            if let Some(generation) = self.awaiting_step_paint {
                if self.painted_generation < generation {
                    return Ok(None);
                }
                self.awaiting_step_paint = None;
            }
        }

        if self.step_index == self.scene.steps().len() {
            self.try_complete(app_loop)?;
            return Ok(None);
        }

        let step = self.scene.steps()[self.step_index].clone();
        if !step.is_cleanup() {
            self.tick_fixture(app_loop)?;
        }
        self.dispatch_step(step, app_loop)
    }

    fn mark_progress(&mut self) {
        self.last_progress_at = self.elapsed;
    }

    fn progress_stage(&self) -> &'static str {
        if let Some(pending) = &self.pending_checkpoint {
            return if pending.checkpoint.is_some() {
                if self.painted_generation < pending.record.generation_after() {
                    // Awaiting the forwarded painted frame that proves the
                    // dispatched projection is visible (WP03): a stall here
                    // is a shell that stopped painting, named as such.
                    "parameter projection paint confirmation"
                } else {
                    "parameter projection dwell"
                }
            } else if pending.predicate.is_patch_effect() {
                "Patch effect audio observation"
            } else {
                "parameter audio observation"
            };
        }
        if self.step_index < self.scene.scalar_step_count() {
            return "autonomous scene dispatch";
        }
        if self.engine_phase != LiveEnginePhase::Complete {
            if self
                .scene
                .expected_engine_transitions()
                .get(self.engine_transition_index)
                .is_some_and(LiveEngineTransition::is_preset)
            {
                return match self.engine_phase {
                    LiveEnginePhase::AwaitSourceAudio { .. } => "preset source audio observation",
                    LiveEnginePhase::AwaitActivating { .. } => "preset preparation",
                    LiveEnginePhase::AwaitReady { .. } => "preset graph activation",
                    LiveEnginePhase::AwaitTargetAudio { .. } => "preset target audio observation",
                    _ => self.engine_phase.stage_name(),
                };
            }
            return self.engine_phase.stage_name();
        }
        if self.topology_transition_index < self.scene.expected_topology_transitions().len() {
            return self.topology_phase.stage_name();
        }
        if self.awaiting_step_paint.is_some() {
            // Awaiting the forwarded painted frame proving the last
            // semantic-tail state visible (WP03): a stall here is a shell
            // that stopped painting, named as such.
            return "semantic step paint confirmation";
        }
        if self.step_index < self.scene.steps().len() {
            return "semantic note cleanup dispatch";
        }
        "zero-active-notes cleanup observation"
    }

    fn tick_fixture<Boundary>(
        &mut self,
        app_loop: &mut AppLoop<Boundary>,
    ) -> Result<(), LiveDemoError>
    where
        Boundary: ControlAudioBoundary,
    {
        let elapsed = core::mem::take(&mut self.deferred_fixture_elapsed);
        self.automatic_midi.tick(elapsed, app_loop)?;
        Ok(())
    }

    fn dispatch_step<Boundary>(
        &mut self,
        step: LiveDemoStep,
        app_loop: &mut AppLoop<Boundary>,
    ) -> Result<Option<LiveCheckpoint>, LiveDemoError>
    where
        Boundary: ControlAudioBoundary,
    {
        let before_tree = app_loop.current_state_tree();
        let observed_value = step
            .editable_parameter()
            .map(|parameter| {
                selected_parameter_value(&before_tree, parameter, step.patch_control_id())
            })
            .transpose()?;
        let expected = LiveExpectedTransition::for_step(
            &step,
            before_tree.generation(),
            before_tree.graph_revision(),
            before_tree.selected_line(),
            observed_value,
        )?;
        let audio_sequence_before = self.observation.read_latest_on_control().sequence();
        let records_before = app_loop.event_log_ref().total_observed();
        let dispatch = app_loop.dispatch_from(step.event().clone(), EventSource::DemoScene);

        match (step.expected_outcome(), dispatch) {
            (EventOutcome::Accepted, Ok(result)) => {
                if !result.audio_effects_published() {
                    return Err(LiveDemoError::AudioBoundaryFull);
                }
            }
            (EventOutcome::Rejected, Err(actual)) if Some(actual) == step.expected_rejection() => {}
            (EventOutcome::Accepted, Err(actual)) => {
                return Err(LiveDemoError::UnexpectedRejection(actual));
            }
            (EventOutcome::Rejected, Ok(_)) => {
                return Err(LiveDemoError::ExpectedRejectionWasAccepted);
            }
            (EventOutcome::Rejected, Err(actual)) => {
                return Err(LiveDemoError::WrongRejection {
                    expected: step
                        .expected_rejection()
                        .expect("rejected steps carry a rejection"),
                    actual,
                });
            }
        }

        let event_log = app_loop.event_log_ref();
        if event_log.total_observed() != records_before.saturating_add(1) {
            return Err(LiveDemoError::MissingEventRecord);
        }
        let record = event_log
            .records()
            .back()
            .ok_or(LiveDemoError::MissingEventRecord)?
            .clone();
        verify_record(&step, &expected, &record)?;
        self.mark_progress();

        if step.requires_checkpoint() {
            let parameter = step
                .editable_parameter()
                .expect("checkpoint steps carry a typed parameter");
            self.pending_checkpoint = Some(PendingCheckpoint {
                step: self.step_index,
                expected,
                record,
                state_tree: app_loop.current_state_tree(),
                text: app_loop.current_text(),
                audio_sequence_before,
                predicate: parameter.audio_predicate(),
                dispatched_at: self.elapsed,
                checkpoint: None,
            });
        } else {
            if step.is_cleanup() {
                self.cleanup_sequence_before = Some(audio_sequence_before);
            } else if self.step_index >= self.scene.scalar_step_count() {
                // Semantic-tail visibility gate (WP03): the next tail step
                // waits until this accepted state has provably painted.
                self.awaiting_step_paint = Some(record.generation_after());
            }
            self.step_index += 1;
        }

        Ok(None)
    }

    fn advance_pending<Boundary>(
        &mut self,
        app_loop: &mut AppLoop<Boundary>,
    ) -> Result<Option<LiveCheckpoint>, LiveDemoError>
    where
        Boundary: ControlAudioBoundary,
    {
        let pending = self
            .pending_checkpoint
            .as_mut()
            .expect("pending state was checked");
        let captured_now = if pending.checkpoint.is_none() {
            let observation = self.observation.read_latest_on_control();
            if observation.sequence() <= pending.audio_sequence_before
                || observation.parameter_generation() < pending.record.parameter_generation()
            {
                return Ok(None);
            }
            if observation.parameter_generation() > pending.record.parameter_generation() {
                return Err(LiveDemoError::MissedAudioGeneration {
                    expected: pending.record.parameter_generation(),
                    actual: observation.parameter_generation(),
                });
            }
            if !audio_is_finite(observation) {
                return Err(LiveDemoError::NonFiniteAudioObservation);
            }
            if !pending.predicate.evaluate(observation) {
                return Ok(None);
            }
            pending.checkpoint = Some(LiveDemoCheckpoint::new(
                pending.step,
                pending.expected.clone(),
                &pending.record,
                &pending.state_tree,
                &pending.text,
                pending.audio_sequence_before,
                observation,
                pending.predicate,
            )?);
            true
        } else {
            false
        };
        if captured_now {
            self.mark_progress();
        }

        // Once exact-generation audio is captured, fixture time may advance;
        // the immutable checkpoint retains the correlated production values.
        self.tick_fixture(app_loop)?;

        let pending = self
            .pending_checkpoint
            .as_ref()
            .expect("fixture ticking does not clear pending state");
        // The visible-projection gate (mission webview-shell-cutover WP03):
        // the checkpoint completes only after the shell's forwarded frame
        // seam has proven a painted document covering the dispatched
        // generation. No tick count or wall-clock pause stands in for paint
        // confirmation; the dwell below is deliberate scene rhythm, not a
        // paint proxy. A paint that never arrives stalls this stage into the
        // typed no-progress timeout.
        let projection_painted = self.painted_generation >= pending.record.generation_after();
        let dwell_complete = self.elapsed.saturating_sub(pending.dispatched_at)
            >= self.scene.minimum_parameter_dwell();
        if !projection_painted || !dwell_complete {
            return Ok(None);
        }

        let pending = self
            .pending_checkpoint
            .take()
            .expect("completed pending state exists");
        let checkpoint = pending
            .checkpoint
            .expect("completion requires an exact-generation observation");
        let parameter = checkpoint
            .expected_transition()
            .editable_parameter()
            .expect("parameter checkpoints carry a typed parameter");
        self.coverage.mark_exercised(parameter);
        let checkpoint = LiveCheckpoint::parameter(checkpoint);
        self.checkpoints.push(checkpoint.clone());
        self.step_index += 1;
        self.mark_progress();
        Ok(Some(checkpoint))
    }

    fn advance_engine<Boundary>(
        &mut self,
        app_loop: &mut AppLoop<Boundary>,
    ) -> Result<Option<LiveCheckpoint>, LiveDemoError>
    where
        Boundary: ControlAudioBoundary,
    {
        let phase = self.engine_phase.clone();
        if !matches!(
            phase,
            LiveEnginePhase::AwaitSourceAudio { .. } | LiveEnginePhase::AwaitTargetAudio { .. }
        ) {
            self.tick_fixture(app_loop)?;
        }
        match phase {
            LiveEnginePhase::SelectPatch => {
                dispatch_engine_event(app_loop, AppEvent::SelectContext(TopLevelContext::Patch))?;
                let transition = self.current_engine_transition()?;
                let patch = app_loop
                    .patches()
                    .iter()
                    .find(|patch| patch.id() == transition.patch_id())
                    .ok_or(LiveDemoError::EngineLifecycleMismatch)?;
                self.engine_envelope_baseline = Some(*patch.envelope());
                self.structural_config_baseline = Some(patch.instrument_config().clone());
                self.structural_effect_baseline = Some(patch.effect_slots().clone());
                self.engine_phase = LiveEnginePhase::FocusControl;
                self.mark_progress();
                Ok(None)
            }
            LiveEnginePhase::FocusControl => {
                let transition = self.current_engine_transition()?.clone();
                let page = app_loop
                    .current_patch_page()
                    .ok_or(LiveDemoError::MissingPatchProjection)?;
                let target = transition.focused_control_id();
                if page.focused_control_id() == target {
                    self.engine_phase = LiveEnginePhase::Request;
                } else if app_loop.state().interaction().active_surface() == SurfaceId::PatchDetail
                {
                    if matches!(target, PatchControlId::Capability(_)) {
                        let paths = SemanticResolver::new(app_loop.state())
                            .ordered_paths(SurfaceId::PatchDetail)?;
                        let current = paths
                            .iter()
                            .position(|path| {
                                path.control_id()
                                    == app_loop.state().interaction().focus_path().control_id()
                            })
                            .ok_or(LiveDemoError::EngineProjectionMismatch)?;
                        let target_index = paths
                            .iter()
                            .position(|path| {
                                path.control_id()
                                    == &crate::control::SemanticControlId::Patch(target.clone())
                            })
                            .ok_or(LiveDemoError::EngineProjectionMismatch)?;
                        let direction = if current < target_index {
                            Direction::Down
                        } else {
                            Direction::Up
                        };
                        dispatch_engine_event(app_loop, AppEvent::Navigate(direction))?;
                    } else {
                        dispatch_engine_event(app_loop, AppEvent::Return)?;
                    }
                } else {
                    let controls = app_loop.state().focused_patch_controls()?;
                    let current = controls
                        .iter()
                        .position(|control| control == &page.focused_control_id())
                        .ok_or(LiveDemoError::EngineProjectionMismatch)?;
                    if matches!(target, PatchControlId::Capability(_)) && current == 0 {
                        dispatch_engine_event(
                            app_loop,
                            AppEvent::EnterSurface(SurfaceId::PatchDetail),
                        )?;
                    } else {
                        let target_index = controls
                            .iter()
                            .position(|control| control == &target)
                            .unwrap_or(0);
                        let direction = if current < target_index {
                            Direction::Down
                        } else {
                            Direction::Up
                        };
                        dispatch_engine_event(app_loop, AppEvent::Navigate(direction))?;
                    }
                }
                self.mark_progress();
                Ok(None)
            }
            LiveEnginePhase::Request => {
                let transition = self.current_engine_transition()?.clone();
                let page = app_loop
                    .current_patch_page()
                    .ok_or(LiveDemoError::MissingPatchProjection)?;
                if page.patch().id() != Some(transition.patch_id())
                    || page.engine().active_capability_id() != transition.source_capability_id()
                    || app_loop.engine_selection_status().kind() != EngineSelectionStatusKind::Ready
                    || app_loop.graph_revision() != self.last_ready_graph_revision
                    || !focused_config_matches_transition_source(app_loop, &transition)?
                {
                    return Err(LiveDemoError::EngineLifecycleMismatch);
                }
                if transition.is_preset() {
                    let audio_sequence_before =
                        self.observation.read_latest_on_control().sequence();
                    let note = MidiMessage::try_new(
                        transition.channel(),
                        MidiMessageKind::NoteOn,
                        60,
                        112,
                    )
                    .expect("the frozen live source MIDI constants are valid");
                    let note_record = dispatch_engine_event(
                        app_loop,
                        AppEvent::Midi {
                            patch_id: transition.patch_id(),
                            message: note,
                        },
                    )?;
                    self.engine_phase = LiveEnginePhase::AwaitSourceAudio {
                        source_revision: app_loop.graph_revision(),
                        generation: note_record.generation_after(),
                        audio_sequence_before,
                    };
                } else {
                    self.engine_phase = LiveEnginePhase::DispatchRequest { source_audio: None };
                }
                self.mark_progress();
                Ok(None)
            }
            LiveEnginePhase::AwaitSourceAudio {
                source_revision,
                generation,
                audio_sequence_before,
            } => {
                let transition = self.current_engine_transition()?.clone();
                let observation = self.observation.read_latest_on_control();
                if observation.sequence() <= audio_sequence_before
                    || observation.parameter_generation() < generation
                {
                    return Ok(None);
                }
                if observation.parameter_generation() > generation {
                    return Err(LiveDemoError::MissedAudioGeneration {
                        expected: generation,
                        actual: observation.parameter_generation(),
                    });
                }
                if !audio_is_finite(observation)
                    || observation.active_graph_revision() != source_revision
                    || observation.primary_patch_id() != Some(transition.patch_id())
                    || observation.primary_active_notes() == 0
                    || observation.primary_patch_rms() <= 0.0
                    || !effect_stage_matches(observation, transition.patch_id())
                    || observation.routing_failures() != 0
                {
                    return Ok(None);
                }
                self.engine_phase = LiveEnginePhase::DispatchRequest {
                    source_audio: Some(observation),
                };
                self.mark_progress();
                Ok(None)
            }
            LiveEnginePhase::DispatchRequest { source_audio } => {
                let transition = self.current_engine_transition()?.clone();
                let source_revision = app_loop.graph_revision();
                let request_record =
                    dispatch_engine_event(app_loop, AppEvent::Adjust(transition.direction()))?;
                let request_id = app_loop
                    .engine_selection_status()
                    .correlation()
                    .map(|correlation| correlation.request_id())
                    .ok_or(LiveDemoError::EngineLifecycleMismatch)?;
                verify_engine_effects(
                    app_loop,
                    &transition,
                    request_id,
                    source_revision,
                    None,
                    EngineSelectionStatusKind::Loading,
                )?;
                let checkpoint = self.capture_engine_checkpoint(
                    app_loop,
                    &transition,
                    EngineSelectionStatusKind::Loading,
                    request_id,
                    request_record.sequence(),
                    source_audio,
                    None,
                )?;
                if matches!(
                    transition.intent(),
                    StructuralEditIntent::ReplaceCapability { .. }
                ) {
                    let old_order = SemanticResolver::new(app_loop.state())
                        .patch_main_paths(transition.patch_id())?;
                    self.pending_focus_recovery = Some((
                        app_loop.state().interaction().focus_path().clone(),
                        old_order,
                    ));
                }
                self.engine_phase = LiveEnginePhase::AwaitActivating {
                    request_id,
                    source_revision,
                };
                self.mark_progress();
                self.push_engine_checkpoint(checkpoint)
            }
            LiveEnginePhase::AwaitActivating {
                request_id,
                source_revision,
            } => {
                let transition = self.current_engine_transition()?.clone();
                match app_loop.engine_selection_status().kind() {
                    EngineSelectionStatusKind::Loading
                    | EngineSelectionStatusKind::Validating
                    | EngineSelectionStatusKind::Preparing => Ok(None),
                    EngineSelectionStatusKind::Activating => {
                        let target_revision = app_loop
                            .engine_selection_status()
                            .correlation()
                            .and_then(|correlation| correlation.target_graph_revision())
                            .ok_or(LiveDemoError::EngineLifecycleMismatch)?;
                        if target_revision <= source_revision
                            || target_revision <= self.last_ready_graph_revision
                        {
                            return Err(LiveDemoError::EngineLifecycleMismatch);
                        }
                        let event_sequence = verify_engine_effects(
                            app_loop,
                            &transition,
                            request_id,
                            source_revision,
                            Some(target_revision),
                            EngineSelectionStatusKind::Activating,
                        )?;
                        let checkpoint = self.capture_engine_checkpoint(
                            app_loop,
                            &transition,
                            EngineSelectionStatusKind::Activating,
                            request_id,
                            event_sequence,
                            None,
                            None,
                        )?;
                        self.engine_phase = LiveEnginePhase::AwaitReady {
                            request_id,
                            source_revision,
                            target_revision,
                        };
                        self.mark_progress();
                        self.push_engine_checkpoint(checkpoint)
                    }
                    EngineSelectionStatusKind::Unavailable
                    | EngineSelectionStatusKind::Failed
                    | EngineSelectionStatusKind::Ready => {
                        Err(LiveDemoError::EngineLifecycleMismatch)
                    }
                }
            }
            LiveEnginePhase::AwaitReady {
                request_id,
                source_revision,
                target_revision,
            } => {
                let transition = self.current_engine_transition()?.clone();
                match app_loop.engine_selection_status().kind() {
                    EngineSelectionStatusKind::Activating => Ok(None),
                    EngineSelectionStatusKind::Ready => {
                        if let Some((removed, old_order)) = self.pending_focus_recovery.take() {
                            let new_order = SemanticResolver::new(app_loop.state())
                                .patch_main_paths(transition.patch_id())?;
                            let expected =
                                SemanticResolver::recover(&removed, &old_order, &new_order)
                                    .ok_or(LiveDemoError::EngineProjectionMismatch)?;
                            let projected = app_loop.current_graphical_shell();
                            if app_loop.state().interaction().focus_path() != &expected
                                || projected.semantic_model().focus_path() != &expected
                                || projected.generation()
                                    != app_loop.current_state_tree().generation()
                                || projected.state_hash()
                                    != app_loop.current_state_tree().state_hash()
                            {
                                return Err(LiveDemoError::EngineProjectionMismatch);
                            }
                            self.shell_coverage.mark_focus_recovery();
                            let mut restored = 0usize;
                            while app_loop.current_patch_page().is_some_and(|page| {
                                page.focused_control_id() != PatchControlId::Engine
                            }) {
                                dispatch_engine_event(app_loop, AppEvent::Navigate(Direction::Up))?;
                                restored = restored.saturating_add(1);
                                if restored >= new_order.len() {
                                    return Err(LiveDemoError::EngineProjectionMismatch);
                                }
                            }
                        }
                        verify_engine_effects(
                            app_loop,
                            &transition,
                            request_id,
                            source_revision,
                            Some(target_revision),
                            EngineSelectionStatusKind::Ready,
                        )?;
                        if transition.is_preset() {
                            dispatch_engine_event(
                                app_loop,
                                AppEvent::Midi {
                                    patch_id: transition.patch_id(),
                                    message: MidiMessage::all_notes_off(transition.channel()),
                                },
                            )?;
                        }
                        self.engine_phase = LiveEnginePhase::StartTargetNote {
                            request_id,
                            source_revision,
                            target_revision,
                        };
                        self.mark_progress();
                        Ok(None)
                    }
                    EngineSelectionStatusKind::Loading
                    | EngineSelectionStatusKind::Validating
                    | EngineSelectionStatusKind::Preparing
                    | EngineSelectionStatusKind::Unavailable
                    | EngineSelectionStatusKind::Failed => {
                        Err(LiveDemoError::EngineLifecycleMismatch)
                    }
                }
            }
            LiveEnginePhase::StartTargetNote {
                request_id,
                source_revision,
                target_revision,
            } => {
                let transition = self.current_engine_transition()?.clone();
                let audio_sequence_before = self.observation.read_latest_on_control().sequence();
                let note = MidiMessage::try_new(
                    transition.channel(),
                    MidiMessageKind::NoteOn,
                    64_u8.saturating_add(self.engine_transition_index as u8),
                    112,
                )
                .expect("the frozen live target MIDI constants are valid");
                let note_record = dispatch_engine_event(
                    app_loop,
                    AppEvent::Midi {
                        patch_id: transition.patch_id(),
                        message: note,
                    },
                )?;
                self.engine_phase = LiveEnginePhase::AwaitTargetAudio {
                    request_id,
                    source_revision,
                    target_revision,
                    generation: note_record.generation_after(),
                    event_sequence: note_record.sequence(),
                    audio_sequence_before,
                };
                self.mark_progress();
                Ok(None)
            }
            LiveEnginePhase::AwaitTargetAudio {
                request_id,
                source_revision,
                target_revision,
                generation,
                event_sequence,
                audio_sequence_before,
            } => {
                let transition = self.current_engine_transition()?.clone();
                let observation = self.observation.read_latest_on_control();
                if observation.sequence() <= audio_sequence_before
                    || observation.parameter_generation() < generation
                {
                    return Ok(None);
                }
                if observation.parameter_generation() > generation {
                    return Err(LiveDemoError::MissedAudioGeneration {
                        expected: generation,
                        actual: observation.parameter_generation(),
                    });
                }
                if !audio_is_finite(observation)
                    || observation.active_graph_revision() != target_revision
                    || observation.routing_failures() != 0
                {
                    return Err(LiveDemoError::EngineTargetAudioMismatch);
                }
                if observation.primary_patch_id() != Some(transition.patch_id())
                    || observation.primary_active_notes() == 0
                    || observation.primary_patch_rms() <= 0.0
                    || !effect_stage_matches(observation, transition.patch_id())
                {
                    return Ok(None);
                }
                verify_engine_effects(
                    app_loop,
                    &transition,
                    request_id,
                    source_revision,
                    Some(target_revision),
                    EngineSelectionStatusKind::Ready,
                )?;
                let checkpoint = self.capture_engine_checkpoint(
                    app_loop,
                    &transition,
                    EngineSelectionStatusKind::Ready,
                    request_id,
                    event_sequence,
                    None,
                    Some(observation),
                )?;
                if !self
                    .runtime_audio
                    .record_ready_capability(transition.target_capability_id(), target_revision)
                {
                    return Err(LiveDemoError::EngineLifecycleMismatch);
                }
                self.coverage.mark_engine_exercised(&transition);
                self.last_ready_graph_revision = target_revision;
                self.engine_phase = LiveEnginePhase::StopTargetNote;
                self.mark_progress();
                self.push_engine_checkpoint(checkpoint)
            }
            LiveEnginePhase::StopTargetNote => {
                let transition = self.current_engine_transition()?.clone();
                dispatch_engine_event(
                    app_loop,
                    AppEvent::Midi {
                        patch_id: transition.patch_id(),
                        message: MidiMessage::all_notes_off(transition.channel()),
                    },
                )?;
                self.engine_transition_index = self.engine_transition_index.saturating_add(1);
                self.engine_phase = if self.engine_transition_index
                    < self.scene.expected_engine_transitions().len()
                {
                    let next = self.current_engine_transition()?;
                    let patch = app_loop
                        .patches()
                        .iter()
                        .find(|patch| patch.id() == next.patch_id())
                        .ok_or(LiveDemoError::EngineLifecycleMismatch)?;
                    self.engine_envelope_baseline = Some(*patch.envelope());
                    self.structural_config_baseline = Some(patch.instrument_config().clone());
                    self.structural_effect_baseline = Some(patch.effect_slots().clone());
                    LiveEnginePhase::FocusControl
                } else {
                    LiveEnginePhase::RestoreMixer
                };
                self.mark_progress();
                Ok(None)
            }
            LiveEnginePhase::RestoreMixer => {
                dispatch_engine_event(app_loop, AppEvent::SelectContext(TopLevelContext::Mixer))?;
                self.engine_phase = LiveEnginePhase::Complete;
                self.mark_progress();
                Ok(None)
            }
            LiveEnginePhase::Complete => Ok(None),
        }
    }

    /// Drives the retained scene's topology transitions: support
    /// events, the occupancy dispatch, the correlated
    /// Preparing/Activating/Ready lifecycle, and the NFR-008 responsiveness
    /// measurement, capturing one topology checkpoint per transition.
    #[allow(clippy::too_many_lines)]
    fn advance_topology<Boundary>(
        &mut self,
        app_loop: &mut AppLoop<Boundary>,
    ) -> Result<Option<LiveCheckpoint>, LiveDemoError>
    where
        Boundary: ControlAudioBoundary,
    {
        let transition = self
            .scene
            .expected_topology_transitions()
            .get(self.topology_transition_index)
            .ok_or(LiveDemoError::TopologyLifecycleMismatch)?
            .clone();
        match self.topology_phase.clone() {
            LiveTopologyPhase::Support { index } => {
                self.tick_fixture(app_loop)?;
                if let Some(item) = transition.support_before().get(index) {
                    if execute_topology_support(app_loop, item, self.patch_editor.as_mut())? {
                        self.topology_phase = LiveTopologyPhase::Support {
                            index: index.saturating_add(1),
                        };
                    }
                } else {
                    // Freeze the fixture from here through capture so the
                    // probe note is the correlated sounding cause.
                    dispatch_engine_event(app_loop, transition.probe_note_on())?;
                    self.topology_phase = LiveTopologyPhase::Dispatch;
                }
                self.mark_progress();
                Ok(None)
            }
            LiveTopologyPhase::Dispatch => {
                let audio_before = self.observation.read_latest_on_control();
                let source_revision = app_loop.graph_revision();
                let context = TopologyContext {
                    focus_before: app_loop.state().interaction().focus_path().clone(),
                    audio_before,
                    source_revision,
                    request_id: None,
                    target_revision: None,
                    last_source_sequence: audio_before.sequence(),
                    first_target_sequence: None,
                    target_note_sequence: None,
                    last_seen_sequence: audio_before.sequence(),
                    rejection: None,
                };
                if let Some(action) = transition.action() {
                    // A transition declaring both the expected occupancy and
                    // an adjacent-choice direction is journey-driven: the
                    // measured cause is the Adjust gesture on the focused
                    // occupancy row, and the committed result is still
                    // verified against the declared action at Activating.
                    let event = match transition.adjust() {
                        Some(direction) if transition.expected_rejection().is_none() => {
                            AppEvent::Adjust(direction)
                        }
                        _ => AppEvent::from_semantic_action(action.clone()),
                    };
                    if let Some(expected) = transition.expected_rejection() {
                        let rejection =
                            dispatch_rejected_topology_event(app_loop, event, expected)?;
                        if matches!(action, crate::control::SemanticAction::SelectPatch(_)) {
                            let unchanged = app_loop.state().interaction().focus_path()
                                == &context.focus_before;
                            if let Some(measurement) = self.patch_editor.as_mut() {
                                measurement.observe_switch_refused_at_end(unchanged);
                            }
                            if !unchanged {
                                return Err(LiveDemoError::TopologySupportMismatch);
                            }
                        }
                        self.topology_phase = LiveTopologyPhase::AwaitRejectionAudio {
                            context: TopologyContext {
                                rejection: Some(rejection),
                                ..context
                            },
                        };
                    } else {
                        let record = dispatch_engine_event(app_loop, event)?;
                        let status = app_loop.engine_selection_status();
                        if status.kind() != EngineSelectionStatusKind::Loading {
                            return Err(LiveDemoError::TopologyLifecycleMismatch);
                        }
                        let request_id = status
                            .correlation()
                            .map(|correlation| correlation.request_id())
                            .ok_or(LiveDemoError::TopologyLifecycleMismatch)?;
                        self.pending_projection_measure = Some(PendingProjectionMeasure {
                            generation: record.generation_after(),
                            frames_seen: 0,
                            resolved: None,
                        });
                        self.topology_phase = LiveTopologyPhase::AwaitActivating {
                            context: TopologyContext {
                                request_id: Some(request_id),
                                ..context
                            },
                        };
                    }
                } else if let Some(direction) = transition.adjust() {
                    dispatch_engine_event(app_loop, AppEvent::Adjust(direction))?;
                    self.topology_phase = LiveTopologyPhase::AwaitScalarAudible { context };
                } else {
                    self.topology_phase = LiveTopologyPhase::AwaitScalarAudible { context };
                }
                self.mark_progress();
                Ok(None)
            }
            LiveTopologyPhase::AwaitActivating { mut context } => {
                self.observe_topology_revision(&mut context);
                match app_loop.engine_selection_status().kind() {
                    EngineSelectionStatusKind::Loading
                    | EngineSelectionStatusKind::Validating
                    | EngineSelectionStatusKind::Preparing => {
                        self.topology_phase = LiveTopologyPhase::AwaitActivating { context };
                        Ok(None)
                    }
                    EngineSelectionStatusKind::Activating => {
                        let target = app_loop
                            .engine_selection_status()
                            .correlation()
                            .and_then(|correlation| correlation.target_graph_revision())
                            .ok_or(LiveDemoError::TopologyLifecycleMismatch)?;
                        if target <= context.source_revision {
                            return Err(LiveDemoError::TopologyLifecycleMismatch);
                        }
                        // Activating retains the acknowledged occupancy while
                        // the prepared candidate waits for graph acknowledgement.
                        if topology_occupancy_committed(app_loop, &transition)? {
                            return Err(LiveDemoError::TopologyProjectionMismatch);
                        }
                        context.target_revision = Some(target);
                        self.topology_phase = LiveTopologyPhase::AwaitReady { context };
                        self.mark_progress();
                        Ok(None)
                    }
                    EngineSelectionStatusKind::Ready
                    | EngineSelectionStatusKind::Unavailable
                    | EngineSelectionStatusKind::Failed => {
                        Err(LiveDemoError::TopologyLifecycleMismatch)
                    }
                }
            }
            LiveTopologyPhase::AwaitReady { mut context } => {
                self.observe_topology_revision(&mut context);
                match app_loop.engine_selection_status().kind() {
                    EngineSelectionStatusKind::Activating => {
                        self.topology_phase = LiveTopologyPhase::AwaitReady { context };
                        Ok(None)
                    }
                    EngineSelectionStatusKind::Ready => {
                        if !topology_occupancy_committed(app_loop, &transition)? {
                            return Err(LiveDemoError::TopologyProjectionMismatch);
                        }
                        // A held-note transition never re-sounds its
                        // probe — the note sounded before the dispatch must
                        // itself survive the block-boundary activation via
                        // voice carry-over, so the audible capture measures
                        // the carried voice. Every other transition keeps the
                        // engine-transition model: sound the target note so
                        // the activated topology is audible.
                        if !transition.hold_note_through_activation() {
                            dispatch_engine_event(app_loop, transition.probe_note_on())?;
                        }
                        context.target_note_sequence =
                            Some(self.observation.read_latest_on_control().sequence());
                        self.topology_phase = LiveTopologyPhase::AwaitAudible { context };
                        self.mark_progress();
                        Ok(None)
                    }
                    EngineSelectionStatusKind::Loading
                    | EngineSelectionStatusKind::Validating
                    | EngineSelectionStatusKind::Preparing
                    | EngineSelectionStatusKind::Unavailable
                    | EngineSelectionStatusKind::Failed => {
                        Err(LiveDemoError::TopologyLifecycleMismatch)
                    }
                }
            }
            LiveTopologyPhase::AwaitAudible { mut context } => {
                self.observe_topology_revision(&mut context);
                let observation = self.observation.read_latest_on_control();
                let Some(note_sequence) = context.target_note_sequence else {
                    return Err(LiveDemoError::TopologyLifecycleMismatch);
                };
                if observation.sequence() <= note_sequence {
                    self.topology_phase = LiveTopologyPhase::AwaitAudible { context };
                    return Ok(None);
                }
                // The change is structurally effective at the activation
                // block boundary (the recorded activation gap); audibility
                // develops as fast as the sounded note's attack and the
                // occupying effect's own latency allow. Poll until the
                // witness holds and record the measured block count.
                let audible_on_activated_graph =
                    topology_witness_holds(transition.audible(), &transition, observation);
                if !audible_on_activated_graph {
                    self.topology_phase = LiveTopologyPhase::AwaitAudible { context };
                    return Ok(None);
                }
                let Some(frames) = self
                    .pending_projection_measure
                    .as_ref()
                    .and_then(|pending| pending.resolved)
                else {
                    self.topology_phase = LiveTopologyPhase::AwaitAudible { context };
                    return Ok(None);
                };
                let target = context
                    .target_revision
                    .ok_or(LiveDemoError::TopologyLifecycleMismatch)?;
                let first_target_sequence = context
                    .first_target_sequence
                    .unwrap_or_else(|| observation.sequence());
                let gap = first_target_sequence
                    .saturating_sub(context.last_source_sequence)
                    .max(1);
                let blocks = observation.sequence().saturating_sub(note_sequence).max(1);
                let tree = app_loop.current_state_tree();
                let audio_uninterrupted = audio_is_finite(context.audio_before)
                    && audio_is_finite(observation)
                    && observation.sequence() > context.audio_before.sequence()
                    && observation.active_notes() > 0
                    && observation.routing_failures() == 0;
                // For a held-note transition the runner dispatched no
                // note after activation, so an active note observed on the
                // target graph is the carried voice itself — measured, never
                // assumed.
                let held_note_carried = transition
                    .hold_note_through_activation()
                    .then(|| observation.active_notes() > 0);
                let checkpoint = LiveTopologyCheckpoint::new(
                    transition.identifier(),
                    self.topology_transition_index,
                    transition.action().cloned(),
                    EventOutcome::Accepted,
                    None,
                    context.request_id,
                    context.source_revision,
                    Some(target),
                    tree.generation(),
                    tree.state_hash(),
                    Some(frames),
                    Some(gap),
                    Some(blocks),
                    audible_on_activated_graph,
                    transition.audible(),
                    context.audio_before,
                    observation,
                    audio_uninterrupted,
                    self.runtime_audio.callback_allocations(),
                    self.runtime_audio.callback_destructions(),
                    held_note_carried,
                )?;
                self.pending_projection_measure = None;
                dispatch_engine_event(app_loop, transition.probe_note_off())?;
                self.coverage
                    .mark_topology_exercised(transition.identifier());
                self.topology_phase = LiveTopologyPhase::SupportAfter { index: 0 };
                self.mark_progress();
                self.push_topology_checkpoint(checkpoint, &transition, &context, app_loop)
            }
            LiveTopologyPhase::AwaitScalarAudible { mut context } => {
                let observation = self.observation.read_latest_on_control();
                if observation.sequence() > context.last_seen_sequence {
                    context.last_seen_sequence = observation.sequence();
                    self.mark_progress();
                }
                if observation.sequence() <= context.audio_before.sequence() {
                    self.topology_phase = LiveTopologyPhase::AwaitScalarAudible { context };
                    return Ok(None);
                }
                if observation.active_graph_revision() != context.source_revision {
                    return Err(LiveDemoError::TopologyLifecycleMismatch);
                }
                if !topology_witness_holds(transition.audible(), &transition, observation) {
                    self.topology_phase = LiveTopologyPhase::AwaitScalarAudible { context };
                    return Ok(None);
                }
                let tree = app_loop.current_state_tree();
                let audio_uninterrupted = audio_is_finite(context.audio_before)
                    && audio_is_finite(observation)
                    && observation.active_notes() > 0
                    && observation.routing_failures() == 0;
                let checkpoint = LiveTopologyCheckpoint::new(
                    transition.identifier(),
                    self.topology_transition_index,
                    None,
                    EventOutcome::Accepted,
                    None,
                    None,
                    context.source_revision,
                    None,
                    tree.generation(),
                    tree.state_hash(),
                    None,
                    None,
                    None,
                    true,
                    transition.audible(),
                    context.audio_before,
                    observation,
                    audio_uninterrupted,
                    self.runtime_audio.callback_allocations(),
                    self.runtime_audio.callback_destructions(),
                    None,
                )?;
                dispatch_engine_event(app_loop, transition.probe_note_off())?;
                self.coverage
                    .mark_topology_exercised(transition.identifier());
                self.topology_phase = LiveTopologyPhase::SupportAfter { index: 0 };
                self.mark_progress();
                self.push_topology_checkpoint(checkpoint, &transition, &context, app_loop)
            }
            LiveTopologyPhase::AwaitRejectionAudio { mut context } => {
                let observation = self.observation.read_latest_on_control();
                if observation.sequence() > context.last_seen_sequence {
                    context.last_seen_sequence = observation.sequence();
                    self.mark_progress();
                }
                if observation.sequence() <= context.audio_before.sequence() {
                    self.topology_phase = LiveTopologyPhase::AwaitRejectionAudio { context };
                    return Ok(None);
                }
                // FR-015/SC-006: the refused change left the active graph,
                // the held notes, and the audio path untouched.
                if observation.active_graph_revision() != context.source_revision {
                    return Err(LiveDemoError::TopologyLifecycleMismatch);
                }
                if !topology_witness_holds(transition.audible(), &transition, observation) {
                    self.topology_phase = LiveTopologyPhase::AwaitRejectionAudio { context };
                    return Ok(None);
                }
                let tree = app_loop.current_state_tree();
                let audio_uninterrupted = audio_is_finite(context.audio_before)
                    && audio_is_finite(observation)
                    && observation.active_notes() > 0
                    && observation.routing_failures() == 0;
                let checkpoint = LiveTopologyCheckpoint::new(
                    transition.identifier(),
                    self.topology_transition_index,
                    transition.action().cloned(),
                    EventOutcome::Rejected,
                    context.rejection.clone(),
                    None,
                    context.source_revision,
                    None,
                    tree.generation(),
                    tree.state_hash(),
                    None,
                    None,
                    None,
                    true,
                    transition.audible(),
                    context.audio_before,
                    observation,
                    audio_uninterrupted,
                    self.runtime_audio.callback_allocations(),
                    self.runtime_audio.callback_destructions(),
                    None,
                )?;
                dispatch_engine_event(app_loop, transition.probe_note_off())?;
                self.coverage
                    .mark_topology_exercised(transition.identifier());
                self.topology_phase = LiveTopologyPhase::SupportAfter { index: 0 };
                self.mark_progress();
                self.push_topology_checkpoint(checkpoint, &transition, &context, app_loop)
            }
            LiveTopologyPhase::SupportAfter { index } => {
                self.tick_fixture(app_loop)?;
                if let Some(item) = transition.support_after().get(index) {
                    if execute_topology_support(app_loop, item, self.patch_editor.as_mut())? {
                        self.topology_phase = LiveTopologyPhase::SupportAfter {
                            index: index.saturating_add(1),
                        };
                    }
                } else {
                    self.topology_transition_index =
                        self.topology_transition_index.saturating_add(1);
                    self.topology_phase = LiveTopologyPhase::Support { index: 0 };
                }
                self.mark_progress();
                Ok(None)
            }
        }
    }

    /// Tracks the last block rendered on the source graph and the first
    /// observed block on the target graph — the block-boundary activation
    /// evidence (NFR-003, NFR-008).
    fn observe_topology_revision(&mut self, context: &mut TopologyContext) {
        let observation = self.observation.read_latest_on_control();
        // A rendering pipeline is not a stalled one: audibility can lag the
        // activation by the occupying effect's own latency (a delay return's
        // wet arrives only after its delay time), so advancing blocks count
        // as progress while the awaited witness develops. The scene's total
        // timeout still bounds the run.
        if observation.sequence() > context.last_seen_sequence {
            context.last_seen_sequence = observation.sequence();
            self.mark_progress();
        }
        if observation.active_graph_revision() == context.source_revision {
            context.last_source_sequence = observation.sequence();
        } else if context.first_target_sequence.is_none()
            && context.target_revision == Some(observation.active_graph_revision())
        {
            context.first_target_sequence = Some(observation.sequence());
        }
    }

    /// Pushes one topology checkpoint and, for the functional Patch editor
    /// scene, the correlating patch-editor checkpoint beside it.
    ///
    /// Every completion path funnels through here, so the correlation is
    /// emitted for each observed transition exactly once and cannot drift
    /// from the topology evidence it is built out of.
    fn push_topology_checkpoint<Boundary>(
        &mut self,
        checkpoint: LiveTopologyCheckpoint,
        transition: &crate::testing::live_effects_and_buses_scene::LiveTopologyTransition,
        context: &TopologyContext,
        app_loop: &AppLoop<Boundary>,
    ) -> Result<Option<LiveCheckpoint>, LiveDemoError>
    where
        Boundary: ControlAudioBoundary,
    {
        let correlating = self
            .patch_editor
            .as_ref()
            .map(|_| build_patch_editor_checkpoint(&checkpoint, transition, context, app_loop));
        let topology = LiveCheckpoint::topology(checkpoint);
        self.checkpoints.push(topology.clone());
        if let (Some(correlating), Some(measurement)) = (correlating, self.patch_editor.as_mut()) {
            if let Some(slot) = slot_of(transition.action()) {
                measurement.observe_occupancy(ObservedOccupancy {
                    patch_id: correlating
                        .destination_patch_id()
                        .ok_or(LiveDemoError::TopologySupportMismatch)?,
                    slot,
                    // Focus was on this slot's own row at dispatch and still
                    // is after the commit: the journey never left the row it
                    // claims to have edited.
                    focus_verified: focus_names_slot(&context.focus_before, slot)
                        && focus_names_slot(correlating.focus_after(), slot),
                });
            }
            if transition.measures_audible_edit() {
                if let Some(patch_id) = correlating.destination_patch_id() {
                    measurement.observe_audible_edit(ObservedAudibleEdit {
                        patch_id,
                        deltas_by_track: per_track_rms_deltas(
                            context.audio_before,
                            self.observation.read_latest_on_control(),
                        ),
                    });
                }
            }
            if correlating.correlates(measurement.reached_patch_id()) {
                measurement.observe_correlating_checkpoint();
            }
            self.checkpoints
                .push(LiveCheckpoint::patch_editor(correlating));
        }
        Ok(Some(topology))
    }

    fn current_engine_transition(&self) -> Result<&LiveEngineTransition, LiveDemoError> {
        self.scene
            .expected_engine_transitions()
            .get(self.engine_transition_index)
            .ok_or(LiveDemoError::EngineLifecycleMismatch)
    }

    fn push_engine_checkpoint(
        &mut self,
        checkpoint: LiveEngineCheckpoint,
    ) -> Result<Option<LiveCheckpoint>, LiveDemoError> {
        let checkpoint = LiveCheckpoint::engine(checkpoint);
        self.checkpoints.push(checkpoint.clone());
        Ok(Some(checkpoint))
    }

    #[allow(clippy::too_many_arguments)]
    fn capture_engine_checkpoint<Boundary>(
        &self,
        app_loop: &AppLoop<Boundary>,
        transition: &LiveEngineTransition,
        status: EngineSelectionStatusKind,
        request_id: EngineSelectionRequestId,
        event_sequence: u64,
        source_audio_observation: Option<AudioObservationSnapshot>,
        audio_observation: Option<AudioObservationSnapshot>,
    ) -> Result<LiveEngineCheckpoint, LiveDemoError>
    where
        Boundary: ControlAudioBoundary,
    {
        let page = app_loop
            .current_patch_page()
            .ok_or(LiveDemoError::MissingPatchProjection)?;
        let engine = page.engine();
        let tree = app_loop.current_state_tree();
        let text = app_loop.current_text();
        let lifecycle = app_loop.engine_selection_status();
        let handoff = app_loop
            .engine_graph_handoff_status()
            .ok_or(LiveDemoError::EngineRuntimeUnavailable)?;
        if page.patch().id() != Some(transition.patch_id())
            || page.focused_control_id() != transition.focused_control_id()
            || page.state_hash() != tree.state_hash()
            || text.state_hash() != tree.state_hash()
            || tree.graph_revision() != app_loop.graph_revision()
            || lifecycle.kind() != status
            || lifecycle.failure().is_some()
        {
            return Err(LiveDemoError::EngineProjectionMismatch);
        }
        if status == EngineSelectionStatusKind::Ready {
            if lifecycle.correlation().is_some() {
                return Err(LiveDemoError::EngineProjectionMismatch);
            }
        } else if !lifecycle.correlation().is_some_and(|correlation| {
            correlation.request_id() == request_id
                && correlation.patch_id() == Some(transition.patch_id())
                && correlation.intent() == transition.intent()
                && correlation.source_capability_id() == Some(transition.source_capability_id())
                && correlation.target_capability_id() == Some(transition.target_capability_id())
        }) {
            return Err(LiveDemoError::EngineProjectionMismatch);
        }
        let patch = app_loop
            .patches()
            .iter()
            .find(|patch| patch.id() == transition.patch_id())
            .ok_or(LiveDemoError::EngineProjectionMismatch)?;
        if self.engine_envelope_baseline.as_ref() != Some(patch.envelope()) {
            return Err(LiveDemoError::EngineProjectionMismatch);
        }
        if self.structural_effect_baseline.as_ref() != Some(patch.effect_slots()) {
            return Err(LiveDemoError::EngineTargetConfigMismatch);
        }
        let baseline = self
            .structural_config_baseline
            .as_ref()
            .ok_or(LiveDemoError::EngineTargetConfigMismatch)?;
        let pending = status != EngineSelectionStatusKind::Ready;
        let preset = match transition.intent() {
            StructuralEditIntent::SetVoiceBudget { .. }
            | StructuralEditIntent::ReplaceEffectAsset { .. }
            | StructuralEditIntent::SetSlotOccupancy { .. }
            | StructuralEditIntent::SetSendEffect { .. }
            | StructuralEditIntent::SetReturnOccupancy { .. }
            | StructuralEditIntent::AppendPatch { .. } => {
                return Err(LiveDemoError::EngineProjectionMismatch)
            }
            StructuralEditIntent::ReplaceCapability { .. } => {
                let expected_request = pending.then_some(request_id);
                let expected_requested = pending.then_some(transition.target_capability_id());
                let expected_active = if pending {
                    transition.source_capability_id()
                } else {
                    transition.target_capability_id()
                };
                if engine.status() != status
                    || engine.active_capability_id() != expected_active
                    || engine.request_id() != expected_request
                    || engine.requested_capability_id() != expected_requested
                    || engine.failure().is_some()
                    || (pending && patch.instrument_config() != baseline)
                    || (!pending && !focused_config_is_descriptor_default(app_loop, transition)?)
                {
                    return Err(LiveDemoError::EngineTargetConfigMismatch);
                }
                None
            }
            StructuralEditIntent::ReplaceParameterChoice {
                parameter_id,
                choice_id,
                ..
            } => {
                if engine.status() != EngineSelectionStatusKind::Ready
                    || engine.active_capability_id() != transition.source_capability_id()
                    || engine.request_id().is_some()
                    || engine.requested_capability_id().is_some()
                    || engine.failure().is_some()
                {
                    return Err(LiveDemoError::EngineProjectionMismatch);
                }
                let row = page
                    .sections()
                    .iter()
                    .flat_map(|section| section.parameters())
                    .find(|row| row.id() == parameter_id)
                    .ok_or(LiveDemoError::EngineProjectionMismatch)?;
                let active_choice = if pending {
                    transition
                        .source_choice_id()
                        .ok_or(LiveDemoError::EngineProjectionMismatch)?
                } else {
                    transition
                        .target_choice_id()
                        .ok_or(LiveDemoError::EngineProjectionMismatch)?
                };
                let active_label = if pending {
                    transition
                        .source_label()
                        .ok_or(LiveDemoError::EngineProjectionMismatch)?
                } else {
                    transition
                        .target_label()
                        .ok_or(LiveDemoError::EngineProjectionMismatch)?
                };
                let requested_choice = pending.then_some(choice_id.as_str());
                let requested_label = pending.then(|| transition.target_label()).flatten();
                let expected_row_status = pending.then_some(status);
                if row.control_id() != Some(transition.focused_control_id())
                    || row.selected_choice_id() != Some(active_choice)
                    || row.selected_label() != Some(active_label)
                    || row.requested_choice_id() != requested_choice
                    || row.requested_label() != requested_label
                    || row.status() != expected_row_status
                    || row.failure().is_some()
                    || (pending && patch.instrument_config() != baseline)
                    || (!pending
                        && !preset_config_delta_is_exact(
                            baseline,
                            patch.instrument_config(),
                            parameter_id,
                            choice_id,
                        ))
                {
                    return Err(LiveDemoError::EngineTargetConfigMismatch);
                }
                Some(LivePresetProjection::new(
                    transition
                        .source_choice_id()
                        .ok_or(LiveDemoError::EngineProjectionMismatch)?,
                    transition
                        .source_label()
                        .ok_or(LiveDemoError::EngineProjectionMismatch)?,
                    transition
                        .target_choice_id()
                        .ok_or(LiveDemoError::EngineProjectionMismatch)?,
                    transition
                        .target_label()
                        .ok_or(LiveDemoError::EngineProjectionMismatch)?,
                    active_choice,
                    active_label,
                    requested_choice.map(str::to_owned),
                    requested_label.map(str::to_owned),
                ))
            }
            StructuralEditIntent::ReplaceAsset { .. }
            | StructuralEditIntent::PrepareAudition { .. } => {
                return Err(LiveDemoError::EngineProjectionMismatch)
            }
        };

        LiveEngineCheckpoint::new(
            transition.identifier(),
            self.engine_transition_index,
            status,
            request_id,
            transition.patch_id(),
            page.focused_control_id(),
            transition.source_capability_id().clone(),
            transition.target_capability_id().clone(),
            transition.intent().clone(),
            preset,
            engine.active_capability_id().clone(),
            engine.requested_capability_id().cloned(),
            tree.generation(),
            tree.state_hash(),
            app_loop.graph_revision(),
            handoff.active_revision(),
            handoff.retired_revision(),
            app_loop.staged_graph_revision(),
            app_loop.in_flight_graph_revision(),
            event_sequence,
            source_audio_observation,
            audio_observation,
            self.runtime_audio.callback_allocations(),
            self.runtime_audio.callback_destructions(),
        )
        .map_err(LiveDemoError::from)
    }

    fn try_complete<Boundary>(&mut self, app_loop: &AppLoop<Boundary>) -> Result<(), LiveDemoError>
    where
        Boundary: ControlAudioBoundary,
    {
        let cleanup_sequence_before = self
            .cleanup_sequence_before
            .ok_or(LiveDemoError::MissingCleanupObservation)?;
        let observation = self.observation.read_latest_on_control();
        let tree = app_loop.current_state_tree();
        if observation.sequence() <= cleanup_sequence_before
            || observation.parameter_generation() < tree.generation()
            || observation.active_notes() != 0
        {
            return Ok(());
        }
        if observation.parameter_generation() > tree.generation() {
            return Err(LiveDemoError::MissedAudioGeneration {
                expected: tree.generation(),
                actual: observation.parameter_generation(),
            });
        }
        if !audio_is_finite(observation) {
            return Err(LiveDemoError::NonFiniteAudioObservation);
        }
        if self.detail_and_assets.is_some()
            && (!matches!(
                app_loop.state().file_browser().preview(),
                crate::control::SamplePreviewState::Idle
            ) || app_loop
                .state()
                .file_browser()
                .preview_request_id()
                .is_some()
                || app_loop.state().file_browser().request_id().is_some()
                || app_loop.state().file_browser().requested_asset().is_some()
                || observation.preview_playing()
                || app_loop.staged_graph_revision().is_some()
                || app_loop.in_flight_graph_revision().is_some())
        {
            return Ok(());
        }
        let focused_patch = self
            .scene
            .patch_ids()
            .next()
            .ok_or(LiveDemoError::EngineTargetConfigMismatch)?;
        let final_patch = app_loop
            .patches()
            .iter()
            .find(|patch| patch.id() == focused_patch)
            .ok_or(LiveDemoError::EngineTargetConfigMismatch)?;
        if self.structural_effect_baseline.as_ref() != Some(final_patch.effect_slots()) {
            return Err(LiveDemoError::EngineTargetConfigMismatch);
        }

        let installed: Vec<_> = self.scene.patch_ids().collect();
        if !self.shell_coverage.is_complete() {
            return Err(LiveDemoError::MissingShellCoverage(
                self.shell_coverage.clone(),
            ));
        }
        let active_graph_revision = tree.graph_revision();
        let graphical_shell = app_loop.current_graphical_shell();
        // The two facts the canonical event log is the only exact source for.
        let event_log = app_loop.event_log();
        if let Some(measurement) = self.patch_editor.as_mut() {
            measurement.observe_switches_from_event_log(&event_log);
            measurement.observe_note_offs_from_event_log(&event_log);
        }
        let patch_editor = self.patch_editor.clone();
        let detail_and_assets = self.detail_and_assets.clone();
        self.completed_report = Some(LiveDemoReport::new(
            self.scene.name(),
            self.checkpoints.clone(),
            app_loop.event_log(),
            tree,
            graphical_shell,
            self.coverage.clone(),
            self.shell_coverage.clone(),
            &installed,
            cleanup_sequence_before,
            observation,
            self.runtime_audio
                .with_active_graph_revision(active_graph_revision)
                .measured(),
            patch_editor,
            detail_and_assets,
        )?);
        self.mark_progress();
        Ok(())
    }

    /// Best-effort semantic cleanup for a user-closed window. It never emits a
    /// success report and makes later advances inert.
    pub fn cleanup_before_close<Boundary>(
        &mut self,
        app_loop: &mut AppLoop<Boundary>,
    ) -> Result<(), LiveDemoError>
    where
        Boundary: ControlAudioBoundary,
    {
        if self.completed_report.is_some() || self.aborted {
            return Ok(());
        }
        for step in self.scene.steps().iter().filter(|step| step.is_cleanup()) {
            let result = app_loop.dispatch_from(step.event().clone(), EventSource::DemoScene)?;
            if !result.audio_effects_published() {
                return Err(LiveDemoError::AudioBoundaryFull);
            }
        }
        self.aborted = true;
        self.pending_checkpoint = None;
        Ok(())
    }

    pub const fn completed_report(&self) -> Option<&LiveDemoReport> {
        self.completed_report.as_ref()
    }

    pub(crate) fn latest_audio_observation(&self) -> AudioObservationSnapshot {
        self.observation.read_latest_on_control()
    }

    pub const fn shell_coverage(&self) -> &LiveShellCoverage {
        &self.shell_coverage
    }

    pub const fn is_aborted(&self) -> bool {
        self.aborted
    }
}

struct PendingCheckpoint {
    step: usize,
    expected: LiveExpectedTransition,
    record: EventRecord,
    state_tree: StateTree,
    text: TextProjection,
    audio_sequence_before: u64,
    predicate: crate::testing::live_demo_scene::LiveAudioPredicate,
    dispatched_at: Duration,
    checkpoint: Option<LiveDemoCheckpoint>,
}

#[derive(Clone, Debug, PartialEq)]
// The inline snapshot keeps this bounded control-side state free of a second
// heap ownership protocol; it is never stored or manipulated by the callback.
#[allow(clippy::large_enum_variant)]
enum LiveEnginePhase {
    SelectPatch,
    FocusControl,
    Request,
    AwaitSourceAudio {
        source_revision: crate::real_time::GraphRevision,
        generation: u64,
        audio_sequence_before: u64,
    },
    DispatchRequest {
        source_audio: Option<AudioObservationSnapshot>,
    },
    AwaitActivating {
        request_id: EngineSelectionRequestId,
        source_revision: crate::real_time::GraphRevision,
    },
    AwaitReady {
        request_id: EngineSelectionRequestId,
        source_revision: crate::real_time::GraphRevision,
        target_revision: crate::real_time::GraphRevision,
    },
    StartTargetNote {
        request_id: EngineSelectionRequestId,
        source_revision: crate::real_time::GraphRevision,
        target_revision: crate::real_time::GraphRevision,
    },
    AwaitTargetAudio {
        request_id: EngineSelectionRequestId,
        source_revision: crate::real_time::GraphRevision,
        target_revision: crate::real_time::GraphRevision,
        generation: u64,
        event_sequence: u64,
        audio_sequence_before: u64,
    },
    StopTargetNote,
    RestoreMixer,
    Complete,
}

impl LiveEnginePhase {
    const fn stage_name(&self) -> &'static str {
        match self {
            Self::SelectPatch => "engine PATCH selection",
            Self::FocusControl => "structural control focus",
            Self::Request => "structural source-note dispatch",
            Self::AwaitSourceAudio { .. } => "preset source audio observation",
            Self::DispatchRequest { .. } => "structural request dispatch",
            Self::AwaitActivating { .. } => "engine preparation",
            Self::AwaitReady { .. } => "engine graph activation",
            Self::StartTargetNote { .. } => "structural target-note dispatch",
            Self::AwaitTargetAudio { .. } => "engine target audio observation",
            Self::StopTargetNote => "structural target-note cleanup",
            Self::RestoreMixer => "MIXER restoration",
            Self::Complete => "engine transitions complete",
        }
    }
}

/// The measured NFR-008 projection window: how many frames paint between an
/// accepted edit and the first frame whose semantic generation covers it.
#[derive(Clone, Debug, PartialEq)]
struct PendingProjectionMeasure {
    generation: u64,
    frames_seen: u64,
    resolved: Option<u64>,
}

/// Bounded per-transition correlation state for the topology phase.
#[derive(Clone, Debug, PartialEq)]
struct TopologyContext {
    /// The canonical focus path at the moment of dispatch: the "before" half
    /// of every occupancy transition's focus verification.
    focus_before: FocusPath,
    audio_before: AudioObservationSnapshot,
    source_revision: crate::real_time::GraphRevision,
    request_id: Option<EngineSelectionRequestId>,
    target_revision: Option<crate::real_time::GraphRevision>,
    last_source_sequence: u64,
    first_target_sequence: Option<u64>,
    target_note_sequence: Option<u64>,
    last_seen_sequence: u64,
    rejection: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
enum LiveTopologyPhase {
    Support { index: usize },
    Dispatch,
    AwaitActivating { context: TopologyContext },
    AwaitReady { context: TopologyContext },
    AwaitAudible { context: TopologyContext },
    AwaitScalarAudible { context: TopologyContext },
    AwaitRejectionAudio { context: TopologyContext },
    SupportAfter { index: usize },
}

impl LiveTopologyPhase {
    const fn stage_name(&self) -> &'static str {
        match self {
            Self::Support { .. } => "topology support dispatch",
            Self::Dispatch => "topology occupancy dispatch",
            Self::AwaitActivating { .. } => "topology preparation",
            Self::AwaitReady { .. } => "topology graph activation",
            Self::AwaitAudible { .. } => "topology audible observation",
            Self::AwaitScalarAudible { .. } => "topology scalar observation",
            Self::AwaitRejectionAudio { .. } => "topology rejection continuity",
            Self::SupportAfter { .. } => "topology support restoration",
        }
    }
}

/// Executes one support item; returns whether the queue may advance past it
/// (machine-driven navigation repeats until its target focus is reached).
fn execute_topology_support<Boundary>(
    app_loop: &mut AppLoop<Boundary>,
    item: &crate::testing::live_effects_and_buses_scene::LiveTopologySupport,
    patch_editor: Option<&mut PatchEditorMeasurement>,
) -> Result<bool, LiveDemoError>
where
    Boundary: ControlAudioBoundary,
{
    use crate::testing::live_effects_and_buses_scene::LiveTopologySupport;
    match item {
        LiveTopologySupport::Event { event } => {
            dispatch_engine_event(app_loop, event.clone())?;
            Ok(true)
        }
        LiveTopologySupport::FocusMixerTrack { track_id } => {
            let focused = app_loop
                .state()
                .interaction()
                .focus_path()
                .control_id()
                .clone();
            let selected = match &focused {
                crate::control::SemanticControlId::Mixer(control) => control.track_id(),
                _ => None,
            }
            .ok_or(LiveDemoError::TopologySupportMismatch)?;
            if selected == *track_id {
                return Ok(true);
            }
            let direction = if selected.index() < track_id.index() {
                Direction::Right
            } else {
                Direction::Left
            };
            dispatch_engine_event(app_loop, AppEvent::Navigate(direction))?;
            Ok(false)
        }
        LiveTopologySupport::FocusPatchControl { control } => {
            match crate::testing::patch_control_navigation::next_patch_control_action(
                app_loop.state(),
                control,
            )? {
                Some(action) => {
                    dispatch_engine_event(app_loop, AppEvent::from_semantic_action(action))?;
                    Ok(false)
                }
                None => Ok(true),
            }
        }
        LiveTopologySupport::FocusInspectorControl { track_id, control } => {
            let focused = app_loop.state().interaction().focus_path().clone();
            let paths = SemanticResolver::new(app_loop.state()).mixer_inspector_paths(*track_id)?;
            let current = paths
                .iter()
                .position(|path| path == &focused)
                .ok_or(LiveDemoError::TopologySupportMismatch)?;
            let target = paths
                .iter()
                .position(|path| {
                    matches!(
                        path.control_id(),
                        crate::control::SemanticControlId::Mixer(actual) if actual == control
                    )
                })
                .ok_or(LiveDemoError::TopologySupportMismatch)?;
            if current == target {
                return Ok(true);
            }
            let direction = if current < target {
                Direction::Down
            } else {
                Direction::Up
            };
            dispatch_engine_event(app_loop, AppEvent::Navigate(direction))?;
            Ok(false)
        }
        LiveTopologySupport::VerifyMixerFocus { control } => {
            let focused = app_loop
                .state()
                .interaction()
                .focus_path()
                .control_id()
                .clone();
            let matches = match &focused {
                crate::control::SemanticControlId::Mixer(actual) => {
                    // The indexed addressing seam: the focused control must
                    // address the exact expected bus, never a repositioned
                    // one.
                    actual == control && actual.bus() == control.bus()
                }
                _ => false,
            };
            if matches {
                Ok(true)
            } else {
                Err(LiveDemoError::TopologySupportMismatch)
            }
        }
        LiveTopologySupport::VerifyPatchFocus { control } => {
            let page = app_loop
                .current_patch_page()
                .ok_or(LiveDemoError::MissingPatchProjection)?;
            if page.focused_control_id() == *control {
                Ok(true)
            } else {
                Err(LiveDemoError::TopologySupportMismatch)
            }
        }
        LiveTopologySupport::VerifyPatchSubject { patch_id } => {
            // Read from the canonical projection the shell is painting, not
            // from the reducer's field directly: what is on screen is the
            // claim, and a projection speaking for another Patch is the
            // defect this assertion exists for.
            let projected = app_loop.current_semantic_model().focus_path().patch_id();
            if projected == Some(*patch_id) {
                Ok(true)
            } else {
                Err(LiveDemoError::TopologySupportMismatch)
            }
        }
        LiveTopologySupport::VerifyEmptyPatchPosition => {
            let projected = app_loop
                .current_semantic_model()
                .focus_path()
                .patch_position();
            if projected == Some(crate::control::PatchPositionId::TrailingEmpty) {
                Ok(true)
            } else {
                Err(LiveDemoError::TopologySupportMismatch)
            }
        }
        LiveTopologySupport::ExpectRejected { event, rejection } => {
            let focus_before = app_loop.state().interaction().focus_path().clone();
            dispatch_rejected_topology_event(app_loop, event.clone(), rejection)?;
            let unchanged = app_loop.state().interaction().focus_path() == &focus_before;
            if let Some(measurement) = patch_editor {
                match event {
                    AppEvent::SelectPatch(_) => {
                        measurement.observe_switch_refused_at_end(unchanged);
                    }
                    AppEvent::EnterSurface(crate::control::SurfaceId::PatchDetail) => {
                        measurement.observe_detail_entry_refused_from_empty_slot();
                    }
                    _ => {}
                }
            }
            if unchanged {
                Ok(true)
            } else {
                Err(LiveDemoError::TopologySupportMismatch)
            }
        }
    }
}

/// Builds the patch-editor correlation for one observed topology transition:
/// the destination Patch the projection now speaks for, the focus before and
/// after, and the per-track audible consequence measured over the transition's
/// own observation window.
///
/// Everything here is read out of evidence the transition already produced;
/// nothing is re-derived from what the scene intended.
fn build_patch_editor_checkpoint<Boundary>(
    checkpoint: &LiveTopologyCheckpoint,
    transition: &crate::testing::live_effects_and_buses_scene::LiveTopologyTransition,
    context: &TopologyContext,
    app_loop: &AppLoop<Boundary>,
) -> crate::testing::live_demo_checkpoint::LivePatchEditorCheckpoint
where
    Boundary: ControlAudioBoundary,
{
    let focus_after = app_loop.state().interaction().focus_path().clone();
    let destination_patch_id = focus_after.patch_id();
    let deltas = per_track_rms_deltas(checkpoint.audio_before(), checkpoint.audio_after());
    let track_of = |patch_id: crate::kernel::PatchId| {
        app_loop
            .patches()
            .iter()
            .find(|patch| patch.id() == patch_id)
            .map(|patch| patch.output().track_id().index())
    };
    let destination_track = destination_patch_id.and_then(track_of);
    let destination_track_delta = destination_track.map_or(0.0, |index| deltas[index]);
    // The largest delta on any *other* installed Patch's own output track over
    // the same window. Patches may share a track, so a Patch whose route is
    // the destination's own is not counted against it — that would measure the
    // mixer's summing rather than the edit's reach.
    let other_patch_track_delta = app_loop
        .patches()
        .iter()
        .filter(|patch| Some(patch.id()) != destination_patch_id)
        .map(|patch| patch.output().track_id().index())
        .filter(|index| Some(*index) != destination_track)
        .map(|index| deltas[index])
        .fold(0.0_f32, f32::max);
    crate::testing::live_demo_checkpoint::LivePatchEditorCheckpoint::new(
        checkpoint.transition(),
        destination_patch_id,
        context.focus_before.clone(),
        focus_after,
        checkpoint.generation(),
        checkpoint
            .target_graph_revision()
            .unwrap_or_else(|| checkpoint.source_graph_revision()),
        slot_of(transition.action()),
        edited_parameter_of(&context.focus_before),
        deltas.to_vec(),
        destination_track_delta,
        other_patch_track_delta,
        checkpoint.audio_before().sequence(),
        checkpoint.audio_after().sequence(),
    )
}

/// The occupancy position one declared action changes, if it changes one.
fn slot_of(
    action: Option<&crate::control::SemanticAction>,
) -> Option<crate::synth::effect_slot_id::EffectSlotIndex> {
    match action {
        Some(crate::control::SemanticAction::SetSlotOccupancy { slot, .. }) => Some(*slot),
        _ => None,
    }
}

/// Whether one focus path names the given slot's own occupancy row.
fn focus_names_slot(
    focus: &FocusPath,
    slot: crate::synth::effect_slot_id::EffectSlotIndex,
) -> bool {
    matches!(
        focus.control_id(),
        crate::control::SemanticControlId::Patch(PatchControlId::EffectSlot(actual))
            if *actual == slot
    )
}

/// The parameter identity a focused PATCH row addresses, carried on the
/// correlation so the audible edit names what it edited.
fn edited_parameter_of(focus: &FocusPath) -> Option<PatchControlId> {
    match focus.control_id() {
        crate::control::SemanticControlId::Patch(control) => Some(control.clone()),
        crate::control::SemanticControlId::Mixer(_)
        | crate::control::SemanticControlId::Send(_)
        | crate::control::SemanticControlId::Modal(_)
        | crate::control::SemanticControlId::MidiInputDevice(_)
        | crate::control::SemanticControlId::ControllerSetting(_)
        | crate::control::SemanticControlId::SessionFileAction(_)
        | crate::control::SemanticControlId::MidiInputListRoot
        | crate::control::SemanticControlId::SurfaceRoot => None,
    }
}

/// Dispatches one topology event that must be rejected with exactly the
/// expected typed reason, recording the rejection in the event log.
fn dispatch_rejected_topology_event<Boundary>(
    app_loop: &mut AppLoop<Boundary>,
    event: AppEvent,
    expected: &str,
) -> Result<String, LiveDemoError>
where
    Boundary: ControlAudioBoundary,
{
    let records_before = app_loop.event_log_ref().total_observed();
    match app_loop.dispatch_from(event.clone(), EventSource::DemoScene) {
        Ok(_) => Err(LiveDemoError::ExpectedRejectionWasAccepted),
        Err(actual) => {
            if actual.name() != expected {
                return Err(LiveDemoError::UnexpectedRejection(actual));
            }
            let log = app_loop.event_log_ref();
            if log.total_observed() != records_before.saturating_add(1) {
                return Err(LiveDemoError::MissingEventRecord);
            }
            let record = log
                .records()
                .back()
                .ok_or(LiveDemoError::MissingEventRecord)?;
            if record.source() != EventSource::DemoScene
                || record.outcome() != EventOutcome::Rejected
                || record.input() != &EventInput::from(&event)
                || record.rejection().map(|value| value.name()) != Some(expected)
            {
                return Err(LiveDemoError::EventRecordMismatch);
            }
            Ok(expected.to_owned())
        }
    }
}

/// Verifies that the canonical projection reflects one committed occupancy
/// change exactly at Activating: the named position holds the named entry
/// (or is empty), nothing weaker.
fn topology_occupancy_committed<Boundary>(
    app_loop: &AppLoop<Boundary>,
    transition: &crate::testing::live_effects_and_buses_scene::LiveTopologyTransition,
) -> Result<bool, LiveDemoError>
where
    Boundary: ControlAudioBoundary,
{
    use crate::control::SemanticAction;
    match transition.action() {
        Some(SemanticAction::SetSlotOccupancy {
            patch_id,
            slot,
            entry,
        }) => {
            let patch = app_loop
                .patches()
                .iter()
                .find(|patch| patch.id() == *patch_id)
                .ok_or(LiveDemoError::TopologyProjectionMismatch)?;
            let occupant = patch.effect_slot(*slot);
            Ok(match entry {
                Some(entry) => occupant.is_some_and(|config| config.capability_id() == entry),
                None => occupant.is_none(),
            })
        }
        Some(SemanticAction::SetReturnOccupancy { bus, entry }) => {
            let occupant = app_loop.bus_returns().bus_return(*bus).effect();
            Ok(match entry {
                Some(entry) => occupant.is_some_and(|config| config.capability_id() == entry),
                None => occupant.is_none(),
            })
        }
        _ => Ok(false),
    }
}

/// Evaluates one topology transition's audible witness against a bounded
/// production-path observation.
fn topology_witness_holds(
    witness: crate::testing::live_effects_and_buses_scene::LiveTopologyAudibleWitness,
    transition: &crate::testing::live_effects_and_buses_scene::LiveTopologyTransition,
    observation: AudioObservationSnapshot,
) -> bool {
    use crate::testing::live_effects_and_buses_scene::LiveTopologyAudibleWitness;
    if !audio_is_finite(observation) || observation.routing_failures() != 0 {
        return false;
    }
    match witness {
        LiveTopologyAudibleWitness::PatchChain => {
            let effect = observation.patch_effect();
            observation.primary_patch_id() == Some(transition.sounding_patch())
                && observation.primary_active_notes() > 0
                && observation.primary_patch_rms() > 0.0
                && effect.patch_id() == Some(transition.sounding_patch())
                && effect.input_rms() > 0.0
                && effect.output_rms() > 0.0
                && effect.difference_rms() > 0.0
        }
        LiveTopologyAudibleWitness::WetReturn => {
            observation.active_notes() > 0
                && observation.output_rms() > 0.0
                && observation.wet_output_rms() > 0.0
        }
        LiveTopologyAudibleWitness::DryContinuity => {
            observation.active_notes() > 0 && observation.output_rms() > 0.0
        }
    }
}

fn dispatch_engine_event<Boundary>(
    app_loop: &mut AppLoop<Boundary>,
    event: AppEvent,
) -> Result<EventRecord, LiveDemoError>
where
    Boundary: ControlAudioBoundary,
{
    let records_before = app_loop.event_log_ref().total_observed();
    let result = app_loop
        .dispatch_from(event.clone(), EventSource::DemoScene)
        .map_err(LiveDemoError::UnexpectedRejection)?;
    if !result.audio_effects_published() {
        return Err(LiveDemoError::AudioBoundaryFull);
    }
    let log = app_loop.event_log_ref();
    if log.total_observed() != records_before.saturating_add(1) {
        return Err(LiveDemoError::MissingEventRecord);
    }
    let record = log
        .records()
        .back()
        .ok_or(LiveDemoError::MissingEventRecord)?;
    if record.source() != EventSource::DemoScene
        || record.outcome() != EventOutcome::Accepted
        || record.input() != &EventInput::from(&event)
    {
        return Err(LiveDemoError::EventRecordMismatch);
    }
    Ok(record.clone())
}

fn verify_engine_effects<Boundary>(
    app_loop: &AppLoop<Boundary>,
    transition: &LiveEngineTransition,
    request_id: EngineSelectionRequestId,
    source_revision: crate::real_time::GraphRevision,
    target_revision: Option<crate::real_time::GraphRevision>,
    status: EngineSelectionStatusKind,
) -> Result<u64, LiveDemoError>
where
    Boundary: ControlAudioBoundary,
{
    let expected: &[EngineSelectionEffectKind] = match status {
        EngineSelectionStatusKind::Loading
        | EngineSelectionStatusKind::Validating
        | EngineSelectionStatusKind::Preparing => &[EngineSelectionEffectKind::PrepareRequested],
        EngineSelectionStatusKind::Activating => &[
            EngineSelectionEffectKind::PrepareRequested,
            EngineSelectionEffectKind::CandidatePrepared,
            EngineSelectionEffectKind::GraphStaged,
            EngineSelectionEffectKind::GraphPublished,
        ],
        EngineSelectionStatusKind::Ready => &[
            EngineSelectionEffectKind::PrepareRequested,
            EngineSelectionEffectKind::CandidatePrepared,
            EngineSelectionEffectKind::GraphStaged,
            EngineSelectionEffectKind::GraphPublished,
            EngineSelectionEffectKind::ActivationAcknowledged,
        ],
        EngineSelectionStatusKind::Unavailable | EngineSelectionStatusKind::Failed => {
            return Err(LiveDemoError::EngineLifecycleMismatch)
        }
    };
    let mut actual = Vec::new();
    let mut endpoint = None;
    for record in app_loop.event_log_ref().records() {
        for emitted in record.emitted_events() {
            let EmittedEvent::EngineSelection { effect } = emitted else {
                continue;
            };
            if effect.request_id() != request_id {
                continue;
            }
            let expected_target = if effect.kind() == EngineSelectionEffectKind::PrepareRequested {
                None
            } else {
                target_revision
            };
            if effect.patch_id() != Some(transition.patch_id())
                || effect.intent() != transition.intent()
                || effect.source_capability_id() != Some(transition.source_capability_id())
                || effect.target_capability_id() != Some(transition.target_capability_id())
                || effect.source_graph_revision() != source_revision
                || effect.target_graph_revision() != expected_target
            {
                return Err(LiveDemoError::EngineEffectMismatch);
            }
            actual.push(effect.kind());
            endpoint = Some(record.sequence());
        }
    }
    if actual != expected {
        return Err(LiveDemoError::EngineEffectMismatch);
    }
    endpoint.ok_or(LiveDemoError::EngineEffectMismatch)
}

fn focused_config_matches_transition_source<Boundary>(
    app_loop: &AppLoop<Boundary>,
    transition: &LiveEngineTransition,
) -> Result<bool, LiveDemoError>
where
    Boundary: ControlAudioBoundary,
{
    let patch = app_loop
        .patches()
        .iter()
        .find(|patch| patch.id() == transition.patch_id())
        .ok_or(LiveDemoError::EngineTargetConfigMismatch)?;
    let config = patch.instrument_config();
    if config.capability_id() != transition.source_capability_id() {
        return Ok(false);
    }
    match transition.intent() {
        StructuralEditIntent::ReplaceCapability { .. } => Ok(true),
        StructuralEditIntent::ReplaceParameterChoice { parameter_id, .. } => Ok(matches!(
            config.value(parameter_id),
            Some(ParameterValue::Choice(choice_id))
                if Some(choice_id.as_str()) == transition.source_choice_id()
        )),
        StructuralEditIntent::ReplaceAsset { .. }
        | StructuralEditIntent::PrepareAudition { .. } => Ok(false),
        StructuralEditIntent::SetVoiceBudget { .. }
        | StructuralEditIntent::ReplaceEffectAsset { .. }
        | StructuralEditIntent::SetSlotOccupancy { .. }
        | StructuralEditIntent::SetSendEffect { .. }
        | StructuralEditIntent::SetReturnOccupancy { .. }
        | StructuralEditIntent::AppendPatch { .. } => Ok(false),
    }
}

fn preset_config_delta_is_exact(
    source: &InstrumentConfig,
    candidate: &InstrumentConfig,
    parameter_id: &crate::synth::ParameterId,
    target_choice_id: &str,
) -> bool {
    if source.capability_id() != candidate.capability_id()
        || source.asset_references() != candidate.asset_references()
        || source.values().len() != candidate.values().len()
        || !matches!(
            candidate.value(parameter_id),
            Some(ParameterValue::Choice(choice_id)) if choice_id == target_choice_id
        )
    {
        return false;
    }
    let mut changed = 0usize;
    for (before, after) in source.values().iter().zip(candidate.values()) {
        if before.parameter_id() != after.parameter_id() {
            return false;
        }
        if before != after {
            if before.parameter_id() != parameter_id {
                return false;
            }
            changed = changed.saturating_add(1);
        }
    }
    changed == 1
}

fn focused_config_is_descriptor_default<Boundary>(
    app_loop: &AppLoop<Boundary>,
    transition: &LiveEngineTransition,
) -> Result<bool, LiveDemoError>
where
    Boundary: ControlAudioBoundary,
{
    let patch = app_loop
        .patches()
        .iter()
        .find(|patch| patch.id() == transition.patch_id())
        .ok_or(LiveDemoError::EngineTargetConfigMismatch)?;
    let config = patch.instrument_config();
    let descriptor = app_loop
        .capabilities()
        .descriptor(transition.target_capability_id())
        .ok_or(LiveDemoError::EngineTargetConfigMismatch)?;
    if config.capability_id() != transition.target_capability_id()
        || app_loop.capabilities().validate_config(config).is_err()
    {
        return Ok(false);
    }
    let expected_values = descriptor
        .parameters()
        .filter(|parameter| parameter.kind() != ParameterKind::Asset)
        .count();
    let expected_assets = descriptor
        .parameters()
        .filter(|parameter| parameter.kind() == ParameterKind::Asset)
        .count();
    if config.values().len() != expected_values
        || config.asset_references().len() != expected_assets
    {
        return Ok(false);
    }
    Ok(descriptor
        .parameters()
        .all(|parameter| match parameter.default_value() {
            ParameterDefault::Value(value) => config.value(parameter.id()) == Some(value),
            ParameterDefault::Asset(reference) => {
                config.asset_reference(parameter.id()) == Some(reference)
            }
        }))
}

fn verify_record(
    step: &LiveDemoStep,
    expected: &LiveExpectedTransition,
    record: &EventRecord,
) -> Result<(), LiveDemoError> {
    let emitted_match = record.emitted_events() == expected.emitted_effects()
        || live_tail_correlated_effect_matches(
            step,
            expected.emitted_effects(),
            record.emitted_events(),
        );
    if record.source() != EventSource::DemoScene
        || record.input() != expected.input()
        || record.input() != &EventInput::from(step.event())
        || record.outcome() != expected.outcome()
        || record.generation_before() != expected.generation_before()
        || record.generation_after() != expected.generation_after()
        || record.parameter_generation() != expected.parameter_generation()
        || !emitted_match
        || record.rejection().map(|value| value.name()) != expected.rejection()
    {
        return Err(LiveDemoError::EventRecordMismatch);
    }
    if record.outcome() == EventOutcome::Rejected
        && (record.state_hash_before() != record.state_hash_after()
            || !record.emitted_events().is_empty())
    {
        return Err(LiveDemoError::EventRecordMismatch);
    }
    Ok(())
}

/// Structural Sample and preview effects contain reducer-assigned correlation
/// identities, so the pre-dispatch live expectation can specify their exact
/// semantic shape but cannot manufacture their request number. The immutable
/// record must still equal the deterministic prefix and carry exactly one
/// attributable extra effect of the required kind.
fn live_tail_correlated_effect_matches(
    step: &LiveDemoStep,
    expected: &[EmittedEvent],
    actual: &[EmittedEvent],
) -> bool {
    if actual.len() != expected.len().saturating_add(1)
        || actual.get(..expected.len()) != Some(expected)
    {
        return false;
    }
    match (step.event(), actual.last()) {
        (AppEvent::PreviewStart, Some(EmittedEvent::EngineSelection { effect })) => {
            effect.kind() == EngineSelectionEffectKind::PrepareRequested
                && matches!(
                    effect.intent(),
                    StructuralEditIntent::PrepareAudition { .. }
                )
        }
        (AppEvent::Activate, Some(EmittedEvent::EngineSelection { effect })) => {
            effect.kind() == EngineSelectionEffectKind::PrepareRequested
                && matches!(effect.intent(), StructuralEditIntent::ReplaceAsset { .. })
        }
        (
            AppEvent::PreviewStop | AppEvent::Return | AppEvent::Navigate(_),
            Some(EmittedEvent::AudioCommand {
                effect: AudioEffect::PreviewStop { .. },
            }),
        ) => true,
        _ => false,
    }
}

fn audio_is_finite(observation: AudioObservationSnapshot) -> bool {
    let effect = observation.patch_effect();
    observation.left_peak().is_finite()
        && observation.right_peak().is_finite()
        && observation.output_rms().is_finite()
        && observation.reverb_input_rms().is_finite()
        && observation.delay_input_rms().is_finite()
        && observation.wet_output_rms().is_finite()
        && observation.primary_patch_rms().is_finite()
        && effect.input_rms().is_finite()
        && effect.output_rms().is_finite()
        && effect.difference_rms().is_finite()
        && effect.side_rms().is_finite()
        && observation.non_finite_samples() == 0
}

/// Proves one painted frame is display evidence of exactly `shell`: full
/// semantic identity equality, non-overlapping regions, and the five
/// declared regions in order with visible-label evidence.
fn shell_frame_matches_projection(
    frame: &ShellFrameObservation,
    shell: &GraphicalShellProjection,
) -> Result<(), LiveDemoError> {
    if frame.generation() != shell.generation()
        || frame.state_hash() != shell.state_hash()
        || frame.context() != shell.context()
        || frame.active_surface() != shell.semantic_model().active_surface()
        || frame.focus_path() != shell.semantic_model().focus_path()
        || frame.interaction_mode() != shell.semantic_model().interaction_mode()
        || frame.return_path() != shell.semantic_model().return_path()
        || frame.valid_actions()
            != shell
                .semantic_model()
                .valid_actions()
                .iter()
                .map(|valid| valid.action().clone())
                .collect::<Vec<_>>()
        || frame.status() != shell.semantic_model().status()
        || frame.errors() != shell.semantic_model().errors()
    {
        return Err(LiveDemoError::ShellFrameMismatch);
    }
    if !frame.regions_are_non_overlapping() {
        return Err(LiveDemoError::OverlappingShellRegions);
    }

    // The five declared shell regions, in order, each carrying visible-label
    // evidence. The exact painted text is shell presentation: each AppWindow
    // adapter owns how a band renders the canonical document (the webview
    // page paints its authored composition, the retired shell painted the band
    // projections' strings), and the frame's semantic identity was already
    // required to equal the canonical projection's above — so qualification
    // demands measured evidence that every declared band is visible with
    // content, never one adapter's band vocabulary (mission
    // webview-shell-cutover WP03).
    if frame
        .regions()
        .iter()
        .zip(ShellRegionId::ALL)
        .any(|(actual, id)| actual.id() != id || actual.visible_label().trim().is_empty())
    {
        return Err(LiveDemoError::ShellFrameMismatch);
    }
    Ok(())
}

/// The synchronous crediting rule: the frame paints the current canonical
/// projection, so the audio callback can never have consumed a parameter
/// generation newer than the painted one — if it has, the frame is stale
/// fabricated evidence.
fn shell_frame_qualifies(
    frame: &ShellFrameObservation,
    shell: &GraphicalShellProjection,
    audio: AudioObservationSnapshot,
) -> Result<bool, LiveDemoError> {
    shell_frame_matches_projection(frame, shell)?;
    if audio.sequence() == 0 {
        return Ok(false);
    }
    if audio.parameter_generation() > frame.generation() {
        return Err(LiveDemoError::ShellAudioGenerationMismatch {
            frame: frame.generation(),
            audio: audio.parameter_generation(),
        });
    }
    if !audio_is_finite(audio) {
        return Err(LiveDemoError::NonFiniteAudioObservation);
    }
    if audio.active_graph_revision() != frame.status().graph_revision() {
        return Ok(false);
    }
    Ok(audio.output_rms() > 0.0 || audio.left_peak() > 0.0 || audio.right_peak() > 0.0)
}

/// The asynchronous-forwarding crediting rule (mission webview-shell-cutover
/// WP03): the frame paints a retained superseded projection, so by ack time
/// the audio callback may legitimately have consumed a newer parameter
/// generation — that ordering is not an error here. Every other requirement
/// is the synchronous rule's: exact identity against the frame's own
/// generation's canonical projection, matching active graph revision, and
/// finite nonzero physical audio at observation time.
fn superseded_shell_frame_qualifies(
    frame: &ShellFrameObservation,
    shell: &GraphicalShellProjection,
    audio: AudioObservationSnapshot,
) -> Result<bool, LiveDemoError> {
    shell_frame_matches_projection(frame, shell)?;
    if audio.sequence() == 0 {
        return Ok(false);
    }
    if !audio_is_finite(audio) {
        return Err(LiveDemoError::NonFiniteAudioObservation);
    }
    if audio.active_graph_revision() != frame.status().graph_revision() {
        return Ok(false);
    }
    Ok(audio.output_rms() > 0.0 || audio.left_peak() > 0.0 || audio.right_peak() > 0.0)
}

fn effect_stage_matches(
    observation: AudioObservationSnapshot,
    patch_id: crate::kernel::PatchId,
) -> bool {
    let effect = observation.patch_effect();
    effect.patch_id() == Some(patch_id)
        && effect.input_rms() > 0.0
        && effect.output_rms() > 0.0
        && effect.difference_rms() > 0.0
        && effect.side_rms() > 0.0
}

/// A typed live orchestration failure suitable for visible standalone output.
#[derive(Debug)]
pub enum LiveDemoError {
    Scene(LiveDemoSceneError),
    Fixture(TestInputError),
    Checkpoint(LiveDemoCheckpointError),
    Report(LiveDemoReportError),
    UnexpectedRejection(EventRejection),
    WrongRejection {
        expected: EventRejection,
        actual: EventRejection,
    },
    ExpectedRejectionWasAccepted,
    AudioBoundaryFull,
    MissingEventRecord,
    EventRecordMismatch,
    MissedAudioGeneration {
        expected: u64,
        actual: u64,
    },
    NonFiniteAudioObservation,
    MissingShellProjection,
    ShellFrameMismatch,
    OverlappingShellRegions,
    ShellAudioGenerationMismatch {
        frame: u64,
        audio: u64,
    },
    MissingShellCoverage(LiveShellCoverage),
    UnexpectedShellFrame,
    MissingCleanupObservation,
    MissingPatchProjection,
    EngineRuntimeUnavailable,
    EngineLifecycleMismatch,
    EngineProjectionMismatch,
    EngineEffectMismatch,
    EngineTargetConfigMismatch,
    EngineTargetAudioMismatch,
    TopologyLifecycleMismatch,
    TopologyProjectionMismatch,
    TopologySupportMismatch,
    ProgressTimedOut {
        stage: &'static str,
        stalled_for: Duration,
        step: Option<usize>,
        predicate: Option<crate::testing::live_demo_scene::LiveAudioPredicate>,
    },
    TotalTimeout {
        elapsed: Duration,
        stage: &'static str,
        step: usize,
    },
}

impl From<LiveDemoSceneError> for LiveDemoError {
    fn from(error: LiveDemoSceneError) -> Self {
        Self::Scene(error)
    }
}

impl From<TestInputError> for LiveDemoError {
    fn from(error: TestInputError) -> Self {
        Self::Fixture(error)
    }
}

impl From<LiveDemoCheckpointError> for LiveDemoError {
    fn from(error: LiveDemoCheckpointError) -> Self {
        Self::Checkpoint(error)
    }
}

impl From<LiveDemoReportError> for LiveDemoError {
    fn from(error: LiveDemoReportError) -> Self {
        Self::Report(error)
    }
}

impl From<EventRejection> for LiveDemoError {
    fn from(error: EventRejection) -> Self {
        Self::UnexpectedRejection(error)
    }
}

impl fmt::Display for LiveDemoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scene(error) => error.fmt(formatter),
            Self::Fixture(error) => error.fmt(formatter),
            Self::Checkpoint(error) => error.fmt(formatter),
            Self::Report(error) => error.fmt(formatter),
            Self::UnexpectedRejection(error) => {
                write!(formatter, "live event was unexpectedly rejected: {error}")
            }
            Self::WrongRejection { expected, actual } => {
                write!(
                    formatter,
                    "live event expected {expected:?}, got {actual:?}"
                )
            }
            Self::ExpectedRejectionWasAccepted => {
                formatter.write_str("live boundary probe was unexpectedly accepted")
            }
            Self::AudioBoundaryFull => {
                formatter.write_str("live event audio command did not cross the bounded boundary")
            }
            Self::MissingEventRecord => {
                formatter.write_str("live event did not append exactly one EventRecord")
            }
            Self::EventRecordMismatch => {
                formatter.write_str("live EventRecord differs from its pre-dispatch expectation")
            }
            Self::MissedAudioGeneration { expected, actual } => write!(
                formatter,
                "latest audio observation generation {actual} passed required generation {expected}"
            ),
            Self::NonFiniteAudioObservation => {
                formatter.write_str("live audio observation contains non-finite output")
            }
            Self::MissingShellProjection => {
                formatter.write_str("live shell frame arrived before a canonical shell projection")
            }
            Self::ShellFrameMismatch => formatter.write_str(
                "live shell frame identity or visible labels differ from the canonical projection",
            ),
            Self::OverlappingShellRegions => {
                formatter.write_str("live shell frame contains overlapping required regions")
            }
            Self::ShellAudioGenerationMismatch { frame, audio } => write!(
                formatter,
                "live shell frame generation {frame} trails audio generation {audio}"
            ),
            Self::MissingShellCoverage(coverage) => write!(
                formatter,
                "live demo completed its control scene without qualifying PATCH and MIXER frames: {coverage:?}",
            ),
            Self::UnexpectedShellFrame => {
                formatter.write_str("live shell frame arrived after completion or cancellation")
            }
            Self::MissingCleanupObservation => {
                formatter.write_str("live cleanup did not establish an observation sequence")
            }
            Self::MissingPatchProjection => {
                formatter.write_str("live engine transition has no canonical PATCH projection")
            }
            Self::EngineRuntimeUnavailable => formatter.write_str(
                "live engine transition has no configured structural runtime observation",
            ),
            Self::EngineLifecycleMismatch => formatter
                .write_str("live engine transition skipped or contradicted its ordered lifecycle"),
            Self::EngineProjectionMismatch => formatter.write_str(
                "live engine state, PATCH, text, parameter, and tree projections disagree",
            ),
            Self::EngineEffectMismatch => formatter.write_str(
                "live engine EventLog effects do not match the correlated structural lifecycle",
            ),
            Self::EngineTargetConfigMismatch => formatter
                .write_str("live engine target is not the exact descriptor-default configuration"),
            Self::EngineTargetAudioMismatch => formatter.write_str(
                "live engine target did not produce finite nonzero generation-tagged output",
            ),
            Self::TopologyLifecycleMismatch => formatter.write_str(
                "live topology transition skipped or contradicted its ordered lifecycle",
            ),
            Self::TopologyProjectionMismatch => formatter.write_str(
                "live topology commit is not reflected by the canonical occupancy projection",
            ),
            Self::TopologySupportMismatch => formatter.write_str(
                "live topology support navigation does not match the frozen focus expectation",
            ),
            Self::ProgressTimedOut {
                stage,
                stalled_for,
                step,
                predicate,
            } => write!(
                formatter,
                "live demo made no progress while awaiting {stage} at step {step:?} with predicate {predicate:?} for {:.1} seconds",
                stalled_for.as_secs_f32(),
            ),
            Self::TotalTimeout {
                elapsed,
                stage,
                step,
            } => write!(
                formatter,
                "live demo exceeded its {}-second total bound after {:.1} seconds at step {step} while awaiting {stage}",
                LIVE_DEMO_TOTAL_TIMEOUT.as_secs(),
                elapsed.as_secs_f32(),
            ),
        }
    }
}

impl std::error::Error for LiveDemoError {}

#[cfg(test)]
mod tests {
    use super::{
        audio_is_finite, shell_frame_qualifies, superseded_shell_frame_qualifies, LiveDemoError,
    };
    use crate::control::{
        GraphicalShellProjection, ShellContextLine, ShellFooter, ShellIdentityHeader,
        TextProjection, TopLevelContext,
    };
    use crate::real_time::audio_observation_snapshot::AudioObservationSnapshot;
    use crate::shell::{
        ShellFrameObservation, ShellFrameObservationError, ShellRegionId, ShellRegionObservation,
        ShellRegionRect,
    };

    fn shell() -> GraphicalShellProjection {
        let diagnostic = TextProjection::for_context(
            TopLevelContext::Patch,
            "PATCH diagnostic".to_owned(),
            0,
            "state-7".to_owned(),
        );
        GraphicalShellProjection::new(
            7,
            "state-7",
            crate::control::SemanticGraphicalViewModel::fixture(
                7,
                "state-7",
                TopLevelContext::Patch,
            ),
            ShellContextLine::new("CREST SYNTH", "PATCH", "READY"),
            ShellIdentityHeader::new("PATCH 01 · Test", "MIDI CH 01"),
            "PATCH WORKSPACE · Test",
            "UTILITY",
            diagnostic,
            ShellFooter::new("PATCH / patch.engine", vec!["1 MIXER".to_owned()]),
        )
        .unwrap()
    }

    fn regions(shell: &GraphicalShellProjection) -> [ShellRegionObservation; 5] {
        [
            ShellRegionObservation::new(
                ShellRegionId::ContextLine,
                ShellRegionRect::new(0.0, 0.0, 1_920.0, 48.0),
                shell.context_line().context_label(),
            ),
            ShellRegionObservation::new(
                ShellRegionId::IdentityHeader,
                ShellRegionRect::new(0.0, 48.0, 1_920.0, 120.0),
                shell.identity_header().primary_label(),
            ),
            ShellRegionObservation::new(
                ShellRegionId::MainWorkspace,
                ShellRegionRect::new(0.0, 120.0, 1_500.0, 1_016.0),
                shell.workspace().main_label(),
            ),
            ShellRegionObservation::new(
                ShellRegionId::PersistentSideRegion,
                ShellRegionRect::new(1_500.0, 120.0, 1_920.0, 1_016.0),
                shell.workspace().side_label(),
            ),
            ShellRegionObservation::new(
                ShellRegionId::Footer,
                ShellRegionRect::new(0.0, 1_016.0, 1_920.0, 1_080.0),
                shell.footer().path_label(),
            ),
        ]
    }

    fn frame(
        shell: &GraphicalShellProjection,
        generation: u64,
        regions: [ShellRegionObservation; 5],
    ) -> ShellFrameObservation {
        let semantic = shell
            .semantic_model()
            .with_generation(generation, shell.state_hash().to_owned());
        ShellFrameObservation::try_new_semantic(1_920.0, 1_080.0, &semantic, regions).unwrap()
    }

    fn audio(generation: u64, level: f32) -> AudioObservationSnapshot {
        AudioObservationSnapshot::from_parts(
            1, 1, 64, generation, 0, 1, level, level, level, 0.0, 0.0, 0.0, 0, 0,
        )
    }

    #[test]
    fn non_finite_callback_measurements_are_rejected_before_checkpointing() {
        let observation = AudioObservationSnapshot::from_parts(
            1,
            1,
            64,
            7,
            0,
            1,
            f32::NAN,
            0.2,
            0.1,
            0.1,
            0.1,
            0.1,
            1,
            0,
        );

        assert!(!audio_is_finite(observation));
    }

    #[test]
    fn shell_frame_credit_rejects_stale_overlap_and_future_audio() {
        let shell = shell();
        let stale = frame(&shell, 6, regions(&shell));
        assert!(matches!(
            shell_frame_qualifies(&stale, &shell, audio(7, 0.2)),
            Err(LiveDemoError::ShellFrameMismatch)
        ));

        let mut overlapping = regions(&shell);
        overlapping[3] = ShellRegionObservation::new(
            ShellRegionId::PersistentSideRegion,
            ShellRegionRect::new(1_400.0, 120.0, 1_920.0, 1_016.0),
            shell.workspace().side_label(),
        );
        let overlapping = frame(&shell, 7, overlapping);
        assert!(matches!(
            shell_frame_qualifies(&overlapping, &shell, audio(7, 0.2)),
            Err(LiveDemoError::OverlappingShellRegions)
        ));

        let current = frame(&shell, 7, regions(&shell));
        assert!(matches!(
            shell_frame_qualifies(&current, &shell, audio(8, 0.2)),
            Err(LiveDemoError::ShellAudioGenerationMismatch { frame: 7, audio: 8 })
        ));
    }

    #[test]
    fn shell_frame_credit_requires_complete_named_regions_and_nonzero_audio() {
        let shell = shell();
        let mut missing = regions(&shell);
        missing[3] = ShellRegionObservation::new(
            ShellRegionId::MainWorkspace,
            ShellRegionRect::new(1_500.0, 120.0, 1_920.0, 1_016.0),
            shell.workspace().side_label(),
        );
        assert_eq!(
            ShellFrameObservation::try_new(
                1_920.0,
                1_080.0,
                7,
                shell.state_hash(),
                shell.context(),
                missing,
            )
            .unwrap_err(),
            ShellFrameObservationError::RegionOrder
        );

        let current = frame(&shell, 7, regions(&shell));
        assert!(!shell_frame_qualifies(&current, &shell, audio(7, 0.0)).unwrap());
        assert!(shell_frame_qualifies(&current, &shell, audio(7, 0.2)).unwrap());
    }

    /// The webview page paints its authored band composition, so the
    /// measured visible labels are the page's vocabulary, not the retired-era
    /// band strings (mission webview-shell-cutover WP03). Qualification
    /// demands identity equality plus visible content in every declared
    /// region — a differently worded but visibly labeled band qualifies.
    #[test]
    fn shell_frame_credit_accepts_the_painting_shells_own_band_vocabulary() {
        let shell = shell();
        let mut page_vocabulary = regions(&shell);
        page_vocabulary[0] = ShellRegionObservation::new(
            ShellRegionId::ContextLine,
            page_vocabulary[0].rect(),
            "PATCH",
        );
        page_vocabulary[4] = ShellRegionObservation::new(
            ShellRegionId::Footer,
            page_vocabulary[4].rect(),
            "PATCH / patch.engine · 1 MIXER",
        );
        let painted = frame(&shell, 7, page_vocabulary);
        assert!(shell_frame_qualifies(&painted, &shell, audio(7, 0.2)).unwrap());
    }

    /// Asynchronously forwarded webview acks can land after the callback has
    /// consumed a newer parameter generation (WP03): for a frame correlated
    /// against its own generation's retained projection that ordering is
    /// legitimate — audio ahead does not reject, silence still refuses
    /// credit, and identity drift still rejects.
    #[test]
    fn superseded_frames_credit_under_the_asynchronous_audio_rule() {
        let shell = shell();
        let painted = frame(&shell, 7, regions(&shell));
        assert!(superseded_shell_frame_qualifies(&painted, &shell, audio(9, 0.2)).unwrap());
        assert!(!superseded_shell_frame_qualifies(&painted, &shell, audio(9, 0.0)).unwrap());

        let drifted = frame(&shell, 6, regions(&shell));
        assert!(matches!(
            superseded_shell_frame_qualifies(&drifted, &shell, audio(9, 0.2)),
            Err(LiveDemoError::ShellFrameMismatch)
        ));
    }
}
