use crate::adapter::braids_capability::BRAIDS_CAPABILITY_ID;
use crate::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID;
use crate::control::app_event::AppEvent;
use crate::control::app_loop::AppLoop;
use crate::control::app_state::{exercise_reducer_table_rejections, EventRejection};
use crate::control::event_log::{EventCoverage, EventLog, EventLogError};
use crate::control::event_record::{
    AudioEffect, EmittedEvent, EventDirection, EventInput, EventOutcome, EventRecord, EventSource,
    MidiKind,
};
use crate::control::state_tree::StateTree;
use crate::control::text_projection::TextProjection;
use crate::control::{
    EngineSelectionFailure, EngineSelectionRequestId, EngineSelectionStatusKind, PatchControlId,
    StructuralEditIntent,
};
use crate::control::{InteractionMode, SemanticAction, TopLevelContext};
use crate::mixer::mixer_track_id::MixerTrackId;
use crate::real_time::audio_boundary::{AudioThreadBoundary, BoundaryFull, ControlAudioBoundary};
use crate::real_time::audio_command::AudioCommand;
use crate::real_time::audio_observation::{CallbackAudioObservation, ControlAudioObservation};
use crate::real_time::audio_renderer::AudioRenderer;
use crate::real_time::structural_graph_boundary::AudioStructuralGraphBoundary;
use crate::shell::keyboard_input_translator::KeyboardInputTranslator;
use crate::shell::window_input::{WindowInput, WindowInputKind, WindowKey};
use crate::testing::demo_scene::{
    DemoEngineExpectation, DemoEngineProbe, DemoPatchAdsrExpectation, DemoPresetExpectation,
    DemoScene, DemoSceneStep, DemoWorkerAdvance,
};
use crate::testing::demo_scene_report::{
    DemoAudioEvidence, DemoCoverageGroup, DemoEngineCheckpoint, DemoPatchAdsrCheckpoint,
    DemoPresetCheckpoint, DemoSceneCheckpoint, DemoSceneCheckpointError, DemoSceneCoverage,
    DemoSceneReport, DemoSceneReportError,
};
use crate::testing::DeterministicGraphPreparationHandle;
use core::fmt;
use serde_json::Value;
use std::collections::BTreeSet;
use std::time::Duration;

const COVERAGE_GROUPS: [DemoCoverageGroup; 11] = [
    DemoCoverageGroup::Inputs,
    DemoCoverageGroup::Events,
    DemoCoverageGroup::Contexts,
    DemoCoverageGroup::Directions,
    DemoCoverageGroup::MidiKinds,
    DemoCoverageGroup::EditableParameters,
    DemoCoverageGroup::PatchControls,
    DemoCoverageGroup::SerializedProperties,
    DemoCoverageGroup::Rejections,
    DemoCoverageGroup::Projections,
    DemoCoverageGroup::AudioEffects,
];

enum DemoDispatchInput {
    Event(Box<AppEvent>),
    Action(SemanticAction),
}

impl From<AppEvent> for DemoDispatchInput {
    fn from(event: AppEvent) -> Self {
        Self::Event(Box::new(event))
    }
}

impl From<SemanticAction> for DemoDispatchInput {
    fn from(action: SemanticAction) -> Self {
        Self::Action(action)
    }
}

/// A structural or production-seam failure while running an exhaustive scene.
#[derive(Clone, Debug, PartialEq)]
pub enum ExhaustiveGuiDemoError {
    EmptyAudioBuffer,
    MissingInstalledFixtureEvent,
    SourceEventLogDropped {
        dropped: u64,
    },
    ExpectedRejectionAccepted {
        expected: EventRejection,
    },
    RejectionMismatch {
        expected: EventRejection,
        actual: EventRejection,
    },
    CheckpointRejectionMismatch {
        step: String,
        expected: EventRejection,
        actual: Option<EventRejection>,
    },
    AudioBoundaryFull(BoundaryFull),
    NonFiniteAudioMeasurement {
        step: String,
    },
    StateTreeSerialization,
    EventLogSerialization,
    ProjectionStateMismatch,
    EventLog(EventLogError),
    Checkpoint(DemoSceneCheckpointError),
    Report(DemoSceneReportError),
    MissingDeterministicWorker,
    WorkerDidNotAdvance,
    Structural(String),
    EngineCheckpoint {
        step: String,
        reason: String,
    },
}

impl fmt::Display for ExhaustiveGuiDemoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyAudioBuffer => {
                formatter.write_str("the exhaustive demo requires a nonempty prepared audio buffer")
            }
            Self::MissingInstalledFixtureEvent => formatter.write_str(
                "the exhaustive demo must begin after the fixture Patch installation event",
            ),
            Self::SourceEventLogDropped { dropped } => write!(
                formatter,
                "the production AppLoop dropped {dropped} event records before report assembly"
            ),
            Self::ExpectedRejectionAccepted { expected } => write!(
                formatter,
                "an event expected to be rejected as {expected} was accepted"
            ),
            Self::RejectionMismatch { expected, actual } => write!(
                formatter,
                "event rejection mismatch: expected {expected}, got {actual}"
            ),
            Self::CheckpointRejectionMismatch {
                step,
                expected,
                actual,
            } => match actual {
                Some(actual) => write!(
                    formatter,
                    "checkpoint {step} expected rejection {expected}, got {actual}"
                ),
                None => write!(
                    formatter,
                    "checkpoint {step} expected rejection {expected}, but the last event was accepted"
                ),
            },
            Self::AudioBoundaryFull(error) => error.fmt(formatter),
            Self::NonFiniteAudioMeasurement { step } => {
                write!(formatter, "audio measurement after {step} was not finite")
            }
            Self::StateTreeSerialization => {
                formatter.write_str("the final StateTree was not valid JSON")
            }
            Self::EventLogSerialization => {
                formatter.write_str("the complete EventLog was not valid JSON")
            }
            Self::ProjectionStateMismatch => formatter.write_str(
                "the current text projection and StateTree do not identify the same accepted state",
            ),
            Self::EventLog(error) => error.fmt(formatter),
            Self::Checkpoint(error) => error.fmt(formatter),
            Self::Report(error) => error.fmt(formatter),
            Self::MissingDeterministicWorker => formatter.write_str(
                "the exhaustive engine scene requires its injected deterministic worker handle",
            ),
            Self::WorkerDidNotAdvance => {
                formatter.write_str("the deterministic graph worker had no pending request")
            }
            Self::Structural(error) => write!(formatter, "structural advancement failed: {error}"),
            Self::EngineCheckpoint { step, reason } => {
                write!(formatter, "engine checkpoint {step} failed: {reason}")
            }
        }
    }
}

impl std::error::Error for ExhaustiveGuiDemoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::AudioBoundaryFull(error) => Some(error),
            Self::EventLog(error) => Some(error),
            Self::Checkpoint(error) => Some(error),
            Self::Report(error) => Some(error),
            _ => None,
        }
    }
}

impl From<EventLogError> for ExhaustiveGuiDemoError {
    fn from(error: EventLogError) -> Self {
        Self::EventLog(error)
    }
}

impl From<DemoSceneCheckpointError> for ExhaustiveGuiDemoError {
    fn from(error: DemoSceneCheckpointError) -> Self {
        Self::Checkpoint(error)
    }
}

impl From<DemoSceneReportError> for ExhaustiveGuiDemoError {
    fn from(error: DemoSceneReportError) -> Self {
        Self::Report(error)
    }
}

/// Runs a DemoScene through the same translator, AppLoop, boundary, renderer,
/// engine, and global mixer used by the standalone application.
///
/// The caller retains ownership of the already initialized production services.
/// The scene runner performs only control-thread work and renders into caller-
/// supplied storage that was allocated before the run.
pub struct ExhaustiveGuiDemo<'a, ControlBoundary, RenderBoundary, Structural, Observation>
where
    ControlBoundary: ControlAudioBoundary,
    RenderBoundary: AudioThreadBoundary,
    Structural: AudioStructuralGraphBoundary,
    Observation: CallbackAudioObservation,
{
    app_loop: &'a mut AppLoop<ControlBoundary>,
    renderer: &'a mut AudioRenderer<RenderBoundary, Structural, Observation>,
    audio_buffer: &'a mut [f32],
    translator: KeyboardInputTranslator,
    worker: Option<DeterministicGraphPreparationHandle>,
    control_observation: Option<&'a dyn ControlAudioObservation>,
}

impl<'a, ControlBoundary, RenderBoundary, Structural, Observation>
    ExhaustiveGuiDemo<'a, ControlBoundary, RenderBoundary, Structural, Observation>
where
    ControlBoundary: ControlAudioBoundary,
    RenderBoundary: AudioThreadBoundary,
    Structural: AudioStructuralGraphBoundary,
    Observation: CallbackAudioObservation,
{
    /// Injects the already initialized control and callback-side production services.
    pub fn new(
        app_loop: &'a mut AppLoop<ControlBoundary>,
        renderer: &'a mut AudioRenderer<RenderBoundary, Structural, Observation>,
        audio_buffer: &'a mut [f32],
    ) -> Self {
        Self {
            app_loop,
            renderer,
            audio_buffer,
            translator: KeyboardInputTranslator::new(),
            worker: None,
            control_observation: None,
        }
    }

    pub fn new_with_worker(
        app_loop: &'a mut AppLoop<ControlBoundary>,
        renderer: &'a mut AudioRenderer<RenderBoundary, Structural, Observation>,
        audio_buffer: &'a mut [f32],
        worker: DeterministicGraphPreparationHandle,
    ) -> Self {
        Self {
            app_loop,
            renderer,
            audio_buffer,
            translator: KeyboardInputTranslator::new(),
            worker: Some(worker),
            control_observation: None,
        }
    }

    pub fn new_with_worker_and_observation<ControlObservation>(
        app_loop: &'a mut AppLoop<ControlBoundary>,
        renderer: &'a mut AudioRenderer<RenderBoundary, Structural, Observation>,
        audio_buffer: &'a mut [f32],
        worker: DeterministicGraphPreparationHandle,
        control_observation: &'a ControlObservation,
    ) -> Self
    where
        ControlObservation: ControlAudioObservation + 'a,
    {
        Self {
            app_loop,
            renderer,
            audio_buffer,
            translator: KeyboardInputTranslator::new(),
            worker: Some(worker),
            control_observation: Some(control_observation),
        }
    }

    /// Executes every deterministic scene step and returns a complete diagnostic report.
    pub fn run(&mut self, scene: DemoScene) -> Result<DemoSceneReport, ExhaustiveGuiDemoError> {
        if self.audio_buffer.is_empty() {
            return Err(ExhaustiveGuiDemoError::EmptyAudioBuffer);
        }

        self.translator = KeyboardInputTranslator::new();
        let startup_log = self.app_loop.event_log();
        ensure_installed_fixture(&startup_log)?;

        let initial_measurement = self.render_audio("scene.initial")?;
        let mut run = RunObservations::new(
            initial_measurement,
            self.mixed_engine_stems_are_nonzero(),
            self.renderer.active_revision(),
            self.app_loop.patches().to_vec(),
        );
        self.observe_effect_stage(&mut run)?;

        self.dispatch_semantic(
            AppEvent::InstallPatches(Vec::new()),
            EventSource::DemoScene,
            Some(EventRejection::InstallationClosed),
            &mut run,
        )?;

        for step in scene.steps() {
            match step {
                DemoSceneStep::WindowInput(input) => {
                    run.observed
                        .insert(format!("input.{}", window_input_identifier(*input)));

                    let actual = self.translator.translate(*input);
                    if let Some(event) = actual {
                        self.dispatch_semantic(event, EventSource::Keyboard, None, &mut run)?;
                    }
                }
                DemoSceneStep::PassiveAction(action) => {
                    self.dispatch_semantic(action.clone(), EventSource::DemoScene, None, &mut run)?;
                }
                DemoSceneStep::MidiProbe(probe) => {
                    self.dispatch_semantic(
                        AppEvent::Midi {
                            patch_id: probe.patch_id(),
                            message: probe.message(),
                        },
                        EventSource::DemoScene,
                        probe.expected_rejection(),
                        &mut run,
                    )?;
                }
                DemoSceneStep::AudioCommandProbe(command) => {
                    self.app_loop
                        .push_recovery_command(*command)
                        .map_err(ExhaustiveGuiDemoError::AudioBoundaryFull)?;
                    match command {
                        AudioCommand::PatchMidi { .. }
                        | AudioCommand::InitializedVoice { .. }
                        | AudioCommand::VoiceMidi { .. }
                        | AudioCommand::OwnedMidi { .. } => {
                            run.observed
                                .insert("effect.emitted.audioCommand.patchMidi".to_owned());
                        }
                        AudioCommand::AllNotesOff
                        | AudioCommand::ReleaseImmediateNotes
                        | AudioCommand::ReleaseSource { .. } => {
                            run.observed
                                .insert("effect.emitted.audioCommand.allNotesOff".to_owned());
                        }
                        AudioCommand::PreviewStart { .. } => {
                            run.observed
                                .insert("effect.emitted.audioCommand.previewStart".to_owned());
                        }
                        AudioCommand::PreviewStop { .. } => {
                            run.observed
                                .insert("effect.emitted.audioCommand.previewStop".to_owned());
                        }
                    }
                }
                DemoSceneStep::Tick(elapsed) => {
                    run.audio_measurement = self.render_audio_tick(*elapsed)?;
                    run.mixed_engine_stems_nonzero |= self.mixed_engine_stems_are_nonzero();
                    self.observe_effect_stage(&mut run)?;
                }
                DemoSceneStep::AdvanceWorker(advance) => {
                    let worker = self
                        .worker
                        .as_ref()
                        .ok_or(ExhaustiveGuiDemoError::MissingDeterministicWorker)?;
                    if let DemoWorkerAdvance::Fail(failure) = advance {
                        worker.fail_next(*failure);
                    }
                    if !worker.advance() {
                        return Err(ExhaustiveGuiDemoError::WorkerDidNotAdvance);
                    }
                }
                DemoSceneStep::AdvanceStructural => {
                    let progress = self
                        .app_loop
                        .advance_structural()
                        .map_err(|error| ExhaustiveGuiDemoError::Structural(error.to_string()))?;
                    run.last_rejection = progress.rejected_worker_event();
                }
                DemoSceneStep::EngineProbe(probe) => {
                    let (event, rejection) = self.engine_probe(*probe)?;
                    self.dispatch_semantic(event, EventSource::Worker, Some(rejection), &mut run)?;
                }
                DemoSceneStep::Checkpoint(checkpoint) => {
                    if let Some(expected) = checkpoint.expected_last_rejection() {
                        if run.last_rejection != Some(expected) {
                            return Err(ExhaustiveGuiDemoError::CheckpointRejectionMismatch {
                                step: checkpoint.name().to_owned(),
                                expected,
                                actual: run.last_rejection,
                            });
                        }
                    }
                    run.last_rejection = None;

                    run.audio_measurement = self.render_audio(checkpoint.name())?;
                    run.mixed_engine_stems_nonzero |= self.mixed_engine_stems_are_nonzero();
                    self.observe_effect_stage(&mut run)?;
                    if let Some(page) = self.app_loop.current_patch_page() {
                        run.observed
                            .insert(format!("patchControl.{}", page.focused_control_id()));
                    }
                    // Generic topology checkpoints also establish the active baseline
                    // for later engine/preset lifecycle assertions.
                    if checkpoint.engine_expectation().is_none()
                        && checkpoint.preset_expectation().is_none()
                        && self.app_loop.engine_selection_status().kind()
                            == EngineSelectionStatusKind::Ready
                    {
                        if self.app_loop.graph_revision() != self.renderer.active_revision() {
                            return Err(ExhaustiveGuiDemoError::EngineCheckpoint {
                                step: checkpoint.name().to_owned(),
                                reason: "acknowledged topology differs from the renderer"
                                    .to_owned(),
                            });
                        }
                        run.last_ready_graph_revision = self.app_loop.graph_revision();
                    }
                    let tree = self.app_loop.current_state_tree();
                    let mut observation = DemoSceneCheckpoint::new(
                        checkpoint.name(),
                        tree.state_hash(),
                        tree.generation(),
                        tree.selected_line(),
                        tree.generation(),
                        run.audio_measurement,
                    )?;
                    if let Some(expectation) = checkpoint.engine_expectation() {
                        observation =
                            observation.with_engine_selection(self.verify_engine_checkpoint(
                                checkpoint.name(),
                                expectation,
                                &mut run,
                            )?);
                    }
                    if let Some(expectation) = checkpoint.preset_expectation() {
                        observation =
                            observation.with_preset_selection(self.verify_preset_checkpoint(
                                checkpoint.name(),
                                expectation,
                                &mut run,
                            )?);
                    }
                    if let Some(expectation) = checkpoint.patch_adsr_expectation() {
                        observation =
                            observation.with_patch_adsr(self.verify_patch_adsr_checkpoint(
                                checkpoint.name(),
                                *expectation,
                                &mut run,
                            )?);
                    }
                    run.checkpoints.push(observation);
                }
            }
        }

        let source_log = self.app_loop.event_log();
        if source_log.dropped_records() != 0 {
            return Err(ExhaustiveGuiDemoError::SourceEventLogDropped {
                dropped: source_log.dropped_records(),
            });
        }
        observe_records(source_log.records().iter(), &mut run.observed);
        let probe_patch = self
            .app_loop
            .state()
            .patches()
            .first()
            .expect("the exhaustive scene requires an installed Patch");
        for rejection in exercise_reducer_table_rejections(
            self.app_loop.capabilities(),
            probe_patch.instrument_config(),
        ) {
            run.observed
                .insert(format!("rejection.{}", rejection.name()));
        }

        let expected = scene.expected_coverage().to_vec();
        let mut event_log = rebuild_event_log(&source_log, &expected, scene.event_log_capacity())?;
        let final_tree = self.app_loop.current_state_tree();
        let final_text = self.app_loop.current_text();
        if final_text.state_hash() != final_tree.state_hash() {
            return Err(ExhaustiveGuiDemoError::ProjectionStateMismatch);
        }

        observe_serialized_properties(
            &expected,
            &final_tree,
            &final_text,
            &event_log,
            &mut run.observed,
        )?;

        if run.effect_observed && run.effect_target_exact {
            run.observed
                .insert("effect.patchEffect.targetExact".to_owned());
        }
        if run.effect_difference_nonzero {
            run.observed
                .insert("effect.patchEffect.differenceNonzero".to_owned());
        }
        if run.effect_side_nonzero {
            run.observed
                .insert("effect.patchEffect.sideNonzero".to_owned());
        }
        if run.effect_before_mix_stem_exact {
            run.observed
                .insert("effect.patchEffect.beforeMixStemExact".to_owned());
        }
        if run.unconfigured_patch_isolated {
            run.observed
                .insert("effect.patchEffect.unconfiguredIsolation".to_owned());
        }
        let structural_effects_preserved = self.app_loop.patches().iter().all(|patch| {
            run.baseline_patches
                .iter()
                .find(|baseline| baseline.id() == patch.id())
                .is_some_and(|baseline| baseline.effect_slots() == patch.effect_slots())
        });
        let effect_stage_required = run
            .baseline_patches
            .iter()
            .any(|patch| patch.effect_slots().iter().any(Option::is_some));
        if effect_stage_required && structural_effects_preserved {
            run.observed
                .insert("effect.patchEffect.structuralPreservation".to_owned());
        }

        for identifier in &run.observed {
            event_log.mark_exercised(identifier.clone());
        }

        let coverage = build_coverage(&expected, &run.observed);
        let audio_evidence = DemoAudioEvidence::new(
            run.mixed_engine_stems_nonzero,
            run.mixed_engine_stems_nonzero && run.all_accepted_adjustments_isolated,
        )
        .with_patch_effect(
            run.effect_observed && run.effect_target_exact,
            run.effect_difference_nonzero,
            run.effect_side_nonzero,
            run.effect_before_mix_stem_exact,
            run.unconfigured_patch_isolated,
            structural_effects_preserved,
        );
        DemoSceneReport::new(
            scene.name(),
            coverage,
            run.checkpoints,
            event_log,
            final_tree,
        )
        .map(|report| report.with_audio_evidence(audio_evidence))
        .map_err(ExhaustiveGuiDemoError::from)
    }

    fn engine_probe(
        &self,
        probe: DemoEngineProbe,
    ) -> Result<(AppEvent, EventRejection), ExhaustiveGuiDemoError> {
        let correlation = self
            .app_loop
            .state()
            .engine_selection()
            .correlation()
            .ok_or_else(|| ExhaustiveGuiDemoError::EngineCheckpoint {
                step: "engine.probe".to_owned(),
                reason: "canonical state has no pending correlation".to_owned(),
            })?;
        let target_revision = correlation
            .source_graph_revision()
            .checked_next()
            .map_err(|error| ExhaustiveGuiDemoError::Structural(error.to_string()))?;
        Ok(match probe {
            DemoEngineProbe::StaleWorkerFailure => {
                let stale_request = EngineSelectionRequestId::new(
                    correlation.request_id().value().saturating_add(1),
                )
                .map_err(|error| ExhaustiveGuiDemoError::Structural(error.to_string()))?;
                (
                    AppEvent::EnginePreparationFailed {
                        request_id: stale_request,
                        patch_id: correlation.patch_id().ok_or_else(|| {
                            ExhaustiveGuiDemoError::Structural(
                                "engine correlation carries a Patch".to_owned(),
                            )
                        })?,
                        intent: correlation.intent().clone(),
                        source_capability_id: correlation
                            .source_capability_id()
                            .ok_or_else(|| {
                                ExhaustiveGuiDemoError::Structural(
                                    "engine correlation carries a source capability".to_owned(),
                                )
                            })?
                            .clone(),
                        target_capability_id: correlation
                            .target_capability_id()
                            .ok_or_else(|| {
                                ExhaustiveGuiDemoError::Structural(
                                    "engine correlation carries a target capability".to_owned(),
                                )
                            })?
                            .clone(),
                        source_graph_revision: correlation.source_graph_revision(),
                        target_graph_revision: target_revision,
                        failure: EngineSelectionFailure::PreparationFailed,
                    },
                    EventRejection::StaleEngineSelection,
                )
            }
            DemoEngineProbe::EarlyAcknowledgement => (
                AppEvent::EngineActivationAcknowledged {
                    request_id: correlation.request_id(),
                    intent: correlation.intent().clone(),
                    target_graph_revision: target_revision,
                    retired_graph_revision: correlation.source_graph_revision(),
                    collected: true,
                },
                EventRejection::StaleEngineSelection,
            ),
            DemoEngineProbe::MismatchedAcknowledgement => (
                AppEvent::EngineActivationAcknowledged {
                    request_id: correlation.request_id(),
                    intent: correlation.intent().clone(),
                    target_graph_revision: target_revision,
                    retired_graph_revision: target_revision,
                    collected: true,
                },
                EventRejection::MismatchedEngineSelection,
            ),
        })
    }

    fn verify_engine_checkpoint(
        &self,
        step: &str,
        expected: &DemoEngineExpectation,
        run: &mut RunObservations,
    ) -> Result<DemoEngineCheckpoint, ExhaustiveGuiDemoError> {
        let page = self.app_loop.current_patch_page().ok_or_else(|| {
            ExhaustiveGuiDemoError::EngineCheckpoint {
                step: step.to_owned(),
                reason: "PATCH projection is unavailable".to_owned(),
            }
        })?;
        run.observed
            .insert(format!("patchControl.{}", page.focused_control_id()));
        let engine = page.engine();
        let mismatch = engine.status() != expected.status()
            || engine.active_capability_id() != expected.active_capability_id()
            || engine.requested_capability_id() != expected.requested_capability_id()
            || engine.failure() != expected.failure();
        if mismatch {
            return Err(ExhaustiveGuiDemoError::EngineCheckpoint {
                step: step.to_owned(),
                reason: format!(
                    "expected {:?}/{}, requested {:?}, failure {:?}; got {:?}/{}, requested {:?}, failure {:?}",
                    expected.status(),
                    expected.active_capability_id(),
                    expected.requested_capability_id(),
                    expected.failure(),
                    engine.status(),
                    engine.active_capability_id(),
                    engine.requested_capability_id(),
                    engine.failure(),
                ),
            });
        }
        if engine.request_id().is_some() {
            run.observed
                .insert("projection.structuralIntent.replaceCapability".to_owned());
            for property in [
                "patchId",
                "requestId",
                "intent",
                "sourceCapabilityId",
                "sourceGraphRevision",
                "targetCapabilityId",
                "targetGraphRevision",
            ] {
                run.observed.insert(format!(
                    "property.stateTree.engineSelection.correlation.{property}"
                ));
            }
        }

        let state_revision = self.app_loop.graph_revision();
        let renderer_revision = self.renderer.active_revision();
        if state_revision != renderer_revision {
            return Err(ExhaustiveGuiDemoError::EngineCheckpoint {
                step: step.to_owned(),
                reason: format!(
                    "state targets graph {state_revision} while renderer owns {renderer_revision}"
                ),
            });
        }
        match expected.status() {
            EngineSelectionStatusKind::Loading
            | EngineSelectionStatusKind::Validating
            | EngineSelectionStatusKind::Preparing
            | EngineSelectionStatusKind::Failed
                if state_revision != run.last_ready_graph_revision =>
            {
                return Err(ExhaustiveGuiDemoError::EngineCheckpoint {
                    step: step.to_owned(),
                    reason: "preparation or failure changed the active graph revision".to_owned(),
                });
            }
            EngineSelectionStatusKind::Activating
                if state_revision <= run.last_ready_graph_revision =>
            {
                return Err(ExhaustiveGuiDemoError::EngineCheckpoint {
                    step: step.to_owned(),
                    reason: "activating scalar transport revision did not advance".to_owned(),
                });
            }
            EngineSelectionStatusKind::Ready => {
                if state_revision <= run.last_ready_graph_revision {
                    return Err(ExhaustiveGuiDemoError::EngineCheckpoint {
                        step: step.to_owned(),
                        reason: "acknowledged graph revision did not advance".to_owned(),
                    });
                }
                run.last_ready_graph_revision = state_revision;
            }
            _ => {}
        }

        let target_index = self
            .app_loop
            .patches()
            .iter()
            .position(|patch| Some(patch.id()) == page.patch().id())
            .ok_or_else(|| ExhaustiveGuiDemoError::EngineCheckpoint {
                step: step.to_owned(),
                reason: "focused Patch identity is absent from canonical state".to_owned(),
            })?;
        let target_peak = self
            .renderer
            .active_patch_audio()
            .stem(
                target_index,
                page.patch()
                    .id()
                    .ok_or(ExhaustiveGuiDemoError::ProjectionStateMismatch)?,
            )
            .map(|stem| {
                stem.samples()
                    .iter()
                    .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
            })
            .unwrap_or(0.0);
        if expected.require_target_audio() && target_peak <= 1.0e-6 {
            return Err(ExhaustiveGuiDemoError::EngineCheckpoint {
                step: step.to_owned(),
                reason: "the selected target Patch stem is silent".to_owned(),
            });
        }

        DemoEngineCheckpoint::new(
            engine.status(),
            engine.active_capability_id().clone(),
            engine.requested_capability_id().cloned(),
            engine.request_id(),
            state_revision,
            renderer_revision,
            engine.failure(),
            target_peak,
        )
        .map_err(ExhaustiveGuiDemoError::from)
    }

    fn verify_preset_checkpoint(
        &self,
        step: &str,
        expected: &DemoPresetExpectation,
        run: &mut RunObservations,
    ) -> Result<DemoPresetCheckpoint, ExhaustiveGuiDemoError> {
        let page = self.app_loop.current_patch_page().ok_or_else(|| {
            ExhaustiveGuiDemoError::EngineCheckpoint {
                step: step.to_owned(),
                reason: "preset checkpoint has no PATCH projection".to_owned(),
            }
        })?;
        let row = page
            .sections()
            .iter()
            .flat_map(|section| section.parameters())
            .find(|row| row.id() == expected.parameter_id())
            .ok_or_else(|| ExhaustiveGuiDemoError::EngineCheckpoint {
                step: step.to_owned(),
                reason: "descriptor-derived preset row is absent".to_owned(),
            })?;
        let requested_choice_id = expected.requested_choice().map(|(id, _)| id);
        let requested_label = expected.requested_choice().map(|(_, label)| label);
        let expected_row_status =
            (expected.status() != EngineSelectionStatusKind::Ready).then_some(expected.status());
        let mismatch = row.status() != expected_row_status
            || row.selected_choice_id() != Some(expected.selected_choice_id())
            || row.selected_label() != Some(expected.selected_label())
            || row.requested_choice_id() != requested_choice_id
            || row.requested_label() != requested_label
            || row.failure() != expected.failure();
        if mismatch {
            return Err(ExhaustiveGuiDemoError::EngineCheckpoint {
                step: step.to_owned(),
                reason: format!(
                    "expected preset {:?}/{}/{} requested {:?}/{:?} failure {:?}; got {:?}/{:?}/{:?} requested {:?}/{:?} failure {:?}",
                    expected.status(),
                    expected.selected_choice_id(),
                    expected.selected_label(),
                    requested_choice_id,
                    requested_label,
                    expected.failure(),
                    row.status(),
                    row.selected_choice_id(),
                    row.selected_label(),
                    row.requested_choice_id(),
                    row.requested_label(),
                    row.failure(),
                ),
            });
        }

        let status = self.app_loop.state().engine_selection();
        let intent = status
            .correlation()
            .map(|correlation| correlation.intent().clone());
        let intent_exact = match (intent.as_ref(), requested_choice_id) {
            (None, None) => true,
            (
                Some(StructuralEditIntent::ReplaceParameterChoice {
                    capability_id,
                    parameter_id,
                    choice_id,
                }),
                Some(expected_choice),
            ) => {
                capability_id == page.engine().active_capability_id()
                    && parameter_id == expected.parameter_id()
                    && choice_id == expected_choice
            }
            _ => false,
        };
        if !intent_exact {
            return Err(ExhaustiveGuiDemoError::EngineCheckpoint {
                step: step.to_owned(),
                reason: "preset structural intent does not match the projected request".to_owned(),
            });
        }
        if status.correlation().is_some() {
            run.observed
                .insert("projection.structuralIntent.replaceParameterChoice".to_owned());
            for property in [
                "patchId",
                "requestId",
                "intent",
                "sourceCapabilityId",
                "sourceGraphRevision",
                "targetCapabilityId",
                "targetGraphRevision",
            ] {
                run.observed.insert(format!(
                    "property.stateTree.engineSelection.correlation.{property}"
                ));
            }
        }

        let state_revision = self.app_loop.graph_revision();
        let renderer_revision = self.renderer.active_revision();
        if state_revision != renderer_revision {
            return Err(ExhaustiveGuiDemoError::EngineCheckpoint {
                step: step.to_owned(),
                reason: format!(
                    "preset state targets graph {state_revision} while renderer owns {renderer_revision}"
                ),
            });
        }
        match expected.status() {
            EngineSelectionStatusKind::Loading
            | EngineSelectionStatusKind::Validating
            | EngineSelectionStatusKind::Preparing
            | EngineSelectionStatusKind::Failed
                if state_revision != run.last_ready_graph_revision =>
            {
                return Err(ExhaustiveGuiDemoError::EngineCheckpoint {
                    step: step.to_owned(),
                    reason: "preset preparation or failure changed the active graph revision"
                        .to_owned(),
                });
            }
            EngineSelectionStatusKind::Activating
                if state_revision <= run.last_ready_graph_revision =>
            {
                return Err(ExhaustiveGuiDemoError::EngineCheckpoint {
                    step: step.to_owned(),
                    reason: "preset activating scalar transport revision did not advance"
                        .to_owned(),
                });
            }
            EngineSelectionStatusKind::Ready => {
                if state_revision <= run.last_ready_graph_revision {
                    return Err(ExhaustiveGuiDemoError::EngineCheckpoint {
                        step: step.to_owned(),
                        reason: "preset acknowledged revision did not advance".to_owned(),
                    });
                }
                run.last_ready_graph_revision = state_revision;
            }
            _ => {}
        }

        let patch_index = self
            .app_loop
            .patches()
            .iter()
            .position(|patch| patch.id() == expected.patch_id())
            .ok_or_else(|| ExhaustiveGuiDemoError::EngineCheckpoint {
                step: step.to_owned(),
                reason: "preset target Patch is absent".to_owned(),
            })?;
        let patch = &self.app_loop.patches()[patch_index];
        let target_peak = self
            .renderer
            .active_patch_audio()
            .stem(patch_index, expected.patch_id())
            .map(|stem| {
                stem.samples()
                    .iter()
                    .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
            })
            .unwrap_or(0.0);
        if expected.require_target_audio() && target_peak <= 1.0e-6 {
            return Err(ExhaustiveGuiDemoError::EngineCheckpoint {
                step: step.to_owned(),
                reason: "preset target Patch stem is silent".to_owned(),
            });
        }

        let descriptor = self
            .app_loop
            .capabilities()
            .descriptor_for_config(patch.instrument_config())
            .ok_or_else(|| ExhaustiveGuiDemoError::EngineCheckpoint {
                step: step.to_owned(),
                reason: "preset capability descriptor is absent".to_owned(),
            })?;
        let spec = descriptor
            .parameter(expected.parameter_id())
            .ok_or_else(|| ExhaustiveGuiDemoError::EngineCheckpoint {
                step: step.to_owned(),
                reason: "preset parameter spec is absent".to_owned(),
            })?;
        let authored_order_exact = row.choices() == spec.choices()
            && row
                .choices()
                .iter()
                .find(|choice| choice.id() == expected.selected_choice_id())
                .is_some_and(|choice| choice.label() == expected.selected_label());
        for choice in row.choices() {
            run.observed.insert(format!(
                "projection.patch.choice.{}.{}={}",
                expected.parameter_id(),
                choice.id(),
                choice.label()
            ));
        }

        let baseline = run
            .baseline_patches
            .iter()
            .find(|baseline| baseline.id() == expected.patch_id())
            .ok_or_else(|| ExhaustiveGuiDemoError::EngineCheckpoint {
                step: step.to_owned(),
                reason: "preset baseline Patch is absent".to_owned(),
            })?;
        let assignment_delta_exact = config_assignment_delta_matches(
            baseline.instrument_config(),
            patch.instrument_config(),
            expected.parameter_id(),
            expected.expected_assignment_changes(),
        ) && baseline
            .instrument_config()
            .value(expected.parameter_id())
            .is_some_and(|value| {
                matches!(value, crate::synth::ParameterValue::Choice(choice) if choice == expected.baseline_choice_id())
            });
        let untargeted_patches_exact = self
            .app_loop
            .patches()
            .iter()
            .filter(|candidate| candidate.id() != expected.patch_id())
            .all(|candidate| {
                run.baseline_patches
                    .iter()
                    .find(|baseline| baseline.id() == candidate.id())
                    == Some(candidate)
            });
        let tree = self.app_loop.current_state_tree();
        let tree_json: Value = serde_json::from_str(tree.json())
            .map_err(|_| ExhaustiveGuiDemoError::StateTreeSerialization)?;
        let text = self.app_loop.current_text();
        let control = expected.control_id();
        let control_id = control.as_str();
        let selected_text = text
            .body()
            .lines()
            .nth(text.selected_line())
            .unwrap_or_default();
        let focus_projection_exact = page.patch().id() == Some(expected.patch_id())
            && page.focused_control_id() == control
            && row.control_id() == Some(control.clone())
            && text.context() == TopLevelContext::Patch
            && text.state_hash() == tree.state_hash()
            && selected_text.starts_with("> DETAIL_PARAMETER ")
            && selected_text.contains(expected.selected_label())
            && tree_json
                .pointer("/interaction/activeFocus/controlId/id")
                .and_then(Value::as_str)
                == Some(control_id.as_ref())
            && tree_json
                .pointer("/patchPage/focusedControlId")
                .and_then(Value::as_str)
                == Some(control_id.as_ref());
        run.observed.insert(format!("patchControl.{control}"));

        DemoPresetCheckpoint::new(
            expected.patch_id(),
            expected.parameter_id().clone(),
            expected.status(),
            expected.selected_choice_id().to_owned(),
            expected.selected_label().to_owned(),
            requested_choice_id.map(str::to_owned),
            requested_label.map(str::to_owned),
            row.choices().to_vec(),
            intent,
            status
                .correlation()
                .map(|correlation| correlation.request_id()),
            state_revision,
            renderer_revision,
            expected.failure(),
            target_peak,
            authored_order_exact,
            focus_projection_exact,
            assignment_delta_exact,
            untargeted_patches_exact,
        )
        .map_err(ExhaustiveGuiDemoError::from)
    }

    fn verify_patch_adsr_checkpoint(
        &self,
        step: &str,
        expected: DemoPatchAdsrExpectation,
        run: &mut RunObservations,
    ) -> Result<DemoPatchAdsrCheckpoint, ExhaustiveGuiDemoError> {
        let control = expected.control_id();
        let page = self.app_loop.current_patch_page().ok_or_else(|| {
            ExhaustiveGuiDemoError::EngineCheckpoint {
                step: step.to_owned(),
                reason: "PATCH ADSR checkpoint has no PATCH projection".to_owned(),
            }
        })?;
        let tree = self.app_loop.current_state_tree();
        let text = self.app_loop.current_text();
        let patch = self
            .app_loop
            .patches()
            .iter()
            .find(|patch| patch.id() == expected.patch_id())
            .ok_or_else(|| ExhaustiveGuiDemoError::EngineCheckpoint {
                step: step.to_owned(),
                reason: "PATCH ADSR target Patch is absent".to_owned(),
            })?;
        let page_row = page
            .envelope()
            .iter()
            .find(|row| row.control_id() == control)
            .ok_or_else(|| ExhaustiveGuiDemoError::EngineCheckpoint {
                step: step.to_owned(),
                reason: "PATCH ADSR row is absent".to_owned(),
            })?;
        let snapshot = self
            .app_loop
            .current_parameters()
            .patch(expected.patch_id())
            .ok_or_else(|| ExhaustiveGuiDemoError::EngineCheckpoint {
                step: step.to_owned(),
                reason: "PATCH ADSR scalar snapshot is absent".to_owned(),
            })?;
        let renderer_snapshot = self
            .renderer
            .parameters()
            .patch(expected.patch_id())
            .ok_or_else(|| ExhaustiveGuiDemoError::EngineCheckpoint {
                step: step.to_owned(),
                reason: "PATCH ADSR renderer snapshot is absent".to_owned(),
            })?;
        let tree_json: Value = serde_json::from_str(tree.json())
            .map_err(|_| ExhaustiveGuiDemoError::StateTreeSerialization)?;
        let selected_text = text
            .body()
            .lines()
            .nth(text.selected_line())
            .unwrap_or_default();
        let control_id = control.as_str();
        let focus_projection_exact = self.app_loop.state().context() == TopLevelContext::Patch
            && page.patch().id() == Some(expected.patch_id())
            && page.focused_control_id() == control
            && text.context() == TopLevelContext::Patch
            && text.state_hash() == tree.state_hash()
            && text.selected_line() == tree.selected_line()
            && selected_text.starts_with("> ENVELOPE ")
            && selected_text.contains(control_id.as_ref())
            && tree_json
                .pointer("/interaction/activeFocus/controlId/id")
                .and_then(Value::as_str)
                == Some(control_id.as_ref())
            && tree_json
                .pointer("/patchPage/focusedControlId")
                .and_then(Value::as_str)
                == Some(control_id.as_ref());
        let records = self.app_loop.event_log_ref().records();
        let adjustment_index = records
            .iter()
            .rposition(|record| matches!(record.input(), EventInput::Adjust { .. }));
        let scalar_only = adjustment_index.is_some_and(|index| {
            let record = &records[index];
            record.outcome() == EventOutcome::Accepted
                && record.parameter_generation()
                    == tree
                        .generation()
                        .saturating_sub((records.len() - index - 1) as u64)
                && record.emitted_events().len() == 2
                && matches!(
                    record.emitted_events()[0],
                    EmittedEvent::StateAccepted { .. }
                )
                && matches!(
                    record.emitted_events()[1],
                    EmittedEvent::ParameterSnapshotPublished { .. }
                )
                && records.iter().skip(index + 1).all(|record| {
                    record.outcome() == EventOutcome::Accepted
                        && matches!(record.input(), EventInput::SetInteractionMode { .. })
                        && record.emitted_events().iter().all(|effect| {
                            matches!(
                                effect,
                                EmittedEvent::StateAccepted { .. }
                                    | EmittedEvent::ParameterSnapshotPublished { .. }
                            )
                        })
                })
        });
        let all_envelope_values_exact = patch.envelope() == snapshot.envelope()
            && snapshot.envelope() == renderer_snapshot.envelope();
        let untargeted_patches_exact = self
            .app_loop
            .patches()
            .iter()
            .filter(|patch| patch.id() != expected.patch_id())
            .all(|patch| {
                run.baseline_patches
                    .iter()
                    .find(|baseline| baseline.id() == patch.id())
                    == Some(patch)
            });
        if expected
            .lifecycle()
            .is_some_and(|status| self.app_loop.state().engine_selection().kind() != status)
        {
            return Err(ExhaustiveGuiDemoError::EngineCheckpoint {
                step: step.to_owned(),
                reason: "PATCH ADSR lifecycle status differs from its frozen expectation"
                    .to_owned(),
            });
        }
        run.observed.insert(format!("patchControl.{control}"));

        DemoPatchAdsrCheckpoint::new(
            expected.patch_id(),
            expected.parameter(),
            expected.expected_value(),
            patch.envelope().value(expected.parameter()),
            page_row.value(),
            snapshot.envelope().value(expected.parameter()),
            renderer_snapshot.envelope().value(expected.parameter()),
            expected.lifecycle(),
            self.app_loop.graph_revision(),
            self.app_loop.current_parameters().graph_revision(),
            self.renderer.active_revision(),
            focus_projection_exact,
            all_envelope_values_exact,
            scalar_only,
            untargeted_patches_exact,
            run.audio_measurement.is_finite(),
        )
        .map_err(|_| ExhaustiveGuiDemoError::EngineCheckpoint {
            step: step.to_owned(),
            reason: format!(
                "ADSR evidence mismatch: expected={} state={} page={} snapshot={} renderer={} revisions={}/{}/{} focus={} envelopes={} scalarOnly={} untargeted={} audioFinite={}",
                expected.expected_value(),
                patch.envelope().value(expected.parameter()),
                page_row.value(),
                snapshot.envelope().value(expected.parameter()),
                renderer_snapshot.envelope().value(expected.parameter()),
                self.app_loop.graph_revision(),
                self.app_loop.current_parameters().graph_revision(),
                self.renderer.active_revision(),
                focus_projection_exact,
                all_envelope_values_exact,
                scalar_only,
                untargeted_patches_exact,
                run.audio_measurement.is_finite(),
            ),
        })
    }

    fn dispatch_semantic<Input>(
        &mut self,
        input: Input,
        source: EventSource,
        expected_rejection: Option<EventRejection>,
        run: &mut RunObservations,
    ) -> Result<(), ExhaustiveGuiDemoError>
    where
        Input: Into<DemoDispatchInput>,
    {
        let input = input.into();
        let event = match &input {
            DemoDispatchInput::Event(event) => event.as_ref().clone(),
            DemoDispatchInput::Action(action) => AppEvent::from_semantic_action(action.clone()),
        };
        let before_tree = self.app_loop.current_state_tree();
        let adjustment = matches!(event, AppEvent::Adjust(_))
            && (self.app_loop.state().context() == TopLevelContext::Mixer
                || matches!(
                    self.app_loop.state().interaction().patch_control_focus(),
                    Some(
                        PatchControlId::Output(_)
                            | PatchControlId::Envelope(_)
                            | PatchControlId::Effect(_, _)
                    )
                ));

        let dispatched = match input {
            DemoDispatchInput::Event(event) => self.app_loop.dispatch_from(*event, source),
            DemoDispatchInput::Action(action) => self.app_loop.dispatch_action_from(action, source),
        };
        match dispatched {
            Ok(result) => {
                if let Some(expected) = expected_rejection {
                    return Err(ExhaustiveGuiDemoError::ExpectedRejectionAccepted { expected });
                }
                if let Some(error) = result.boundary_full() {
                    return Err(ExhaustiveGuiDemoError::AudioBoundaryFull(error));
                }

                if !matches!(
                    event,
                    AppEvent::SetInteractionMode(InteractionMode::Navigate)
                ) {
                    run.last_rejection = None;
                }
                let after_tree = self.app_loop.current_state_tree();
                let measurement = self.render_audio("accepted event")?;
                run.mixed_engine_stems_nonzero |= self.mixed_engine_stems_are_nonzero();
                if adjustment {
                    if let Some(identifier) =
                        changed_parameter_identifier(&before_tree, &after_tree)
                    {
                        run.observed.insert(identifier.clone());
                        let suffix = identifier
                            .strip_prefix("parameter.")
                            .expect("parameter identifiers have a stable prefix");
                        run.observed
                            .insert(format!("effect.parameterSnapshot.{suffix}"));
                    } else {
                        run.all_accepted_adjustments_isolated = false;
                    }
                }
                run.audio_measurement = measurement;
                self.observe_effect_stage(run)?;
                Ok(())
            }
            Err(actual) => {
                run.last_rejection = Some(actual);
                if let Some(expected) = expected_rejection {
                    if actual != expected {
                        return Err(ExhaustiveGuiDemoError::RejectionMismatch { expected, actual });
                    }
                }
                Ok(())
            }
        }
    }

    fn render_audio_tick(&mut self, _elapsed: Duration) -> Result<f64, ExhaustiveGuiDemoError> {
        self.render_audio("deterministic tick")
    }

    fn render_audio(&mut self, step: &str) -> Result<f64, ExhaustiveGuiDemoError> {
        self.renderer.render(self.audio_buffer);

        let mut measurement = 0.0_f64;
        for (index, sample) in self.audio_buffer.iter().copied().enumerate() {
            if !sample.is_finite() {
                return Err(ExhaustiveGuiDemoError::NonFiniteAudioMeasurement {
                    step: step.to_owned(),
                });
            }
            let channel_weight = if index % 2 == 0 {
                1.0
            } else {
                1.618_033_988_75
            };
            let position_weight = 1.0 + (index % 17) as f64 * 0.000_1;
            measurement += f64::from(sample).abs() * channel_weight * position_weight;
        }
        measurement /= self.audio_buffer.len() as f64;

        if !measurement.is_finite() {
            return Err(ExhaustiveGuiDemoError::NonFiniteAudioMeasurement {
                step: step.to_owned(),
            });
        }
        Ok(measurement)
    }

    fn mixed_engine_stems_are_nonzero(&self) -> bool {
        let stems = self.renderer.active_patch_audio();
        let mut soundfont = false;
        let mut braids = false;
        for (index, patch) in self.app_loop.patches().iter().enumerate() {
            let sounding = stems
                .stem(index, patch.id())
                .is_some_and(|stem| stem.samples().iter().any(|sample| sample.abs() > 1.0e-6));
            if !sounding {
                continue;
            }
            match patch.instrument_config().capability_id().as_str() {
                HIDEF_CAPABILITY_ID => soundfont = true,
                BRAIDS_CAPABILITY_ID => braids = true,
                _ => {}
            }
        }
        soundfont && braids
    }

    fn observe_effect_stage(
        &self,
        run: &mut RunObservations,
    ) -> Result<(), ExhaustiveGuiDemoError> {
        let Some(reader) = self.control_observation else {
            return Ok(());
        };
        let observation = reader.read_latest_on_control();
        let effect = observation.patch_effect();
        let configured = self
            .app_loop
            .patches()
            .iter()
            .filter(|patch| patch.effect_slots().iter().any(Option::is_some))
            .collect::<Vec<_>>();
        if configured.is_empty() {
            return Ok(());
        }
        let finite = effect.input_rms().is_finite()
            && effect.output_rms().is_finite()
            && effect.difference_rms().is_finite()
            && effect.side_rms().is_finite();
        if !finite {
            return Err(ExhaustiveGuiDemoError::NonFiniteAudioMeasurement {
                step: "Patch effect observation".to_owned(),
            });
        }
        run.effect_observed = true;
        let configured_patch = configured.first().expect("configured effects are nonempty");
        let parameter_projection_matches_transport =
            if self.app_loop.engine_selection_status().kind()
                == EngineSelectionStatusKind::Activating
            {
                // Activating deliberately publishes the prepared candidate-layout
                // scalar snapshot while the immutable UI projection retains the
                // acknowledged source layout. Their audio values need not compare
                // structurally, but both must name the exact activated target
                // revision; focused candidate-scalar tests prove the value merge.
                self.renderer.parameters().graph_revision()
                    == self.app_loop.current_parameters().graph_revision()
            } else {
                self.renderer
                    .parameters()
                    .audio_values_equal(self.app_loop.current_parameters())
            };
        run.effect_target_exact &= configured.len() == 1
            && effect.patch_id() == Some(configured_patch.id())
            && observation.parameter_generation() == self.renderer.parameters().generation()
            && parameter_projection_matches_transport
            && observation.active_graph_revision() == self.renderer.active_revision()
            && observation.routing_failures() == 0;
        run.effect_difference_nonzero |= effect.difference_rms() > 1.0e-7;
        run.effect_side_nonzero |= effect.side_rms() > 1.0e-7;

        if let Some(index) = self
            .app_loop
            .patches()
            .iter()
            .position(|patch| patch.id() == configured_patch.id())
        {
            if let Some(stem) = self
                .renderer
                .active_patch_audio()
                .stem(index, configured_patch.id())
            {
                let samples = stem.samples();
                let energy = samples.iter().fold(0.0_f64, |sum, sample| {
                    sum + f64::from(*sample) * f64::from(*sample)
                });
                let rms = (energy / samples.len().max(1) as f64).sqrt() as f32;
                run.effect_before_mix_stem_exact |= (rms - effect.output_rms()).abs() <= 1.0e-6;
            }
        }
        run.unconfigured_patch_isolated |= self
            .app_loop
            .patches()
            .iter()
            .enumerate()
            .filter(|(_, patch)| patch.effect_slots().iter().all(Option::is_none))
            .any(|(index, patch)| {
                effect.patch_id() != Some(patch.id())
                    && self
                        .renderer
                        .active_patch_audio()
                        .stem(index, patch.id())
                        .is_some_and(|stem| {
                            stem.samples().iter().any(|sample| sample.abs() > 1.0e-6)
                        })
            });
        Ok(())
    }
}

struct RunObservations {
    observed: BTreeSet<String>,
    checkpoints: Vec<DemoSceneCheckpoint>,
    audio_measurement: f64,
    last_rejection: Option<EventRejection>,
    mixed_engine_stems_nonzero: bool,
    all_accepted_adjustments_isolated: bool,
    last_ready_graph_revision: crate::real_time::GraphRevision,
    baseline_patches: Vec<crate::synth::Patch>,
    effect_observed: bool,
    effect_target_exact: bool,
    effect_difference_nonzero: bool,
    effect_side_nonzero: bool,
    effect_before_mix_stem_exact: bool,
    unconfigured_patch_isolated: bool,
}

impl RunObservations {
    fn new(
        audio_measurement: f64,
        mixed_engine_stems_nonzero: bool,
        last_ready_graph_revision: crate::real_time::GraphRevision,
        baseline_patches: Vec<crate::synth::Patch>,
    ) -> Self {
        Self {
            observed: BTreeSet::new(),
            checkpoints: Vec::new(),
            audio_measurement,
            last_rejection: None,
            mixed_engine_stems_nonzero,
            all_accepted_adjustments_isolated: true,
            last_ready_graph_revision,
            baseline_patches,
            effect_observed: false,
            effect_target_exact: true,
            effect_difference_nonzero: false,
            effect_side_nonzero: false,
            effect_before_mix_stem_exact: false,
            unconfigured_patch_isolated: false,
        }
    }
}

fn config_assignment_delta_matches(
    baseline: &crate::synth::InstrumentConfig,
    candidate: &crate::synth::InstrumentConfig,
    parameter_id: &crate::synth::ParameterId,
    expected_changes: usize,
) -> bool {
    if baseline.capability_id() != candidate.capability_id()
        || baseline.asset_references() != candidate.asset_references()
        || baseline.values().len() != candidate.values().len()
    {
        return false;
    }
    let mut changed = Vec::new();
    for (source, target) in baseline.values().iter().zip(candidate.values()) {
        if source.parameter_id() != target.parameter_id() {
            return false;
        }
        if source != target {
            changed.push(source.parameter_id());
        }
    }
    changed.len() == expected_changes
        && changed
            .into_iter()
            .all(|changed_parameter| changed_parameter == parameter_id)
}

fn ensure_installed_fixture(event_log: &EventLog) -> Result<(), ExhaustiveGuiDemoError> {
    if event_log.dropped_records() != 0 {
        return Err(ExhaustiveGuiDemoError::SourceEventLogDropped {
            dropped: event_log.dropped_records(),
        });
    }

    let installed = event_log.records().iter().any(|record| {
        record.outcome() == EventOutcome::Accepted
            && matches!(
                record.input(),
                EventInput::InstallPatches { patches } if !patches.is_empty()
            )
    });
    if installed {
        Ok(())
    } else {
        Err(ExhaustiveGuiDemoError::MissingInstalledFixtureEvent)
    }
}

fn rebuild_event_log(
    source: &EventLog,
    expected: &[String],
    scene_capacity: usize,
) -> Result<EventLog, ExhaustiveGuiDemoError> {
    let capacity = scene_capacity
        .saturating_add(2)
        .max(source.records().len())
        .max(1);
    let mut event_log =
        EventLog::with_coverage(capacity, EventCoverage::new(expected.iter().cloned()))?;
    for record in source.records() {
        event_log.append(record.clone())?;
    }
    Ok(event_log)
}

fn build_coverage(expected: &[String], observed: &BTreeSet<String>) -> DemoSceneCoverage {
    let mut coverage = DemoSceneCoverage::new();
    for group in COVERAGE_GROUPS {
        coverage.declare_expected(
            group,
            expected
                .iter()
                .filter(|identifier| coverage_group(identifier) == Some(group))
                .cloned(),
        );
    }

    for identifier in observed {
        if let Some(group) = coverage_group(identifier) {
            coverage.mark_exercised(group, identifier.clone());
        }
    }
    coverage
}

fn coverage_group(identifier: &str) -> Option<DemoCoverageGroup> {
    if identifier.starts_with("input.") {
        Some(DemoCoverageGroup::Inputs)
    } else if identifier.starts_with("event.") {
        Some(DemoCoverageGroup::Events)
    } else if identifier.starts_with("context.") {
        Some(DemoCoverageGroup::Contexts)
    } else if identifier.starts_with("direction.") {
        Some(DemoCoverageGroup::Directions)
    } else if identifier.starts_with("interactionMode.") || identifier.starts_with("surface.") {
        Some(DemoCoverageGroup::Contexts)
    } else if identifier.starts_with("midi.") {
        Some(DemoCoverageGroup::MidiKinds)
    } else if identifier.starts_with("parameter.") {
        Some(DemoCoverageGroup::EditableParameters)
    } else if identifier.starts_with("patchControl.") {
        Some(DemoCoverageGroup::PatchControls)
    } else if identifier.starts_with("rejection.") {
        Some(DemoCoverageGroup::Rejections)
    } else if identifier.starts_with("effect.") {
        Some(DemoCoverageGroup::AudioEffects)
    } else if identifier.starts_with("property.stateTree.projection.")
        || identifier.starts_with("property.textProjection.")
        || identifier.starts_with("projection.")
    {
        Some(DemoCoverageGroup::Projections)
    } else if identifier.starts_with("property.") {
        Some(DemoCoverageGroup::SerializedProperties)
    } else {
        None
    }
}

fn observe_records<'a>(
    records: impl Iterator<Item = &'a EventRecord>,
    observed: &mut BTreeSet<String>,
) {
    for record in records {
        match record.input() {
            EventInput::Send { .. } => {
                observed.insert("event.send".to_owned());
            }
            EventInput::SelectContext { context } => {
                observed.insert("event.selectContext".to_owned());
                observed.insert(format!("context.{}", context.label().to_ascii_lowercase()));
            }
            EventInput::SelectPatch { direction } => {
                observed.insert("event.selectPatch".to_owned());
                observed.insert(format!("direction.{}", direction_identifier(*direction)));
            }
            EventInput::NavigatePage { direction } => {
                observed.insert("event.navigatePage".to_owned());
                observed.insert(format!("direction.{}", direction_identifier(*direction)));
            }
            EventInput::Navigate { direction } => {
                observed.insert("event.navigate".to_owned());
                observed.insert(format!("direction.{}", direction_identifier(*direction)));
            }
            EventInput::Adjust { direction } => {
                observed.insert("event.adjust".to_owned());
                observed.insert(format!("direction.{}", direction_identifier(*direction)));
            }
            EventInput::SetInteractionMode { mode } => {
                observed.insert("event.setInteractionMode".to_owned());
                observed.insert(format!(
                    "interactionMode.{}",
                    mode.label().to_ascii_lowercase()
                ));
            }
            EventInput::OpenRelated => {
                observed.insert("event.openRelated".to_owned());
            }
            EventInput::OpenMidiSettings => {
                observed.insert("event.openMidiSettings".to_owned());
            }
            EventInput::Activate => {
                observed.insert("event.activate".to_owned());
            }
            EventInput::PreviewStart => {
                observed.insert("event.previewStart".to_owned());
            }
            EventInput::PreviewStop => {
                observed.insert("event.previewStop".to_owned());
            }
            EventInput::EnterSurface { surface } => {
                observed.insert("event.enterSurface".to_owned());
                observed.insert(format!("surface.{}", surface.label().to_ascii_lowercase()));
            }
            EventInput::Return => {
                observed.insert("event.return".to_owned());
            }
            EventInput::InstallPatches { .. } => {
                observed.insert("event.installPatches".to_owned());
            }
            EventInput::Midi { message, .. } => {
                observed.insert("event.midi".to_owned());
                observed.insert(format!("midi.{}", midi_kind_identifier(message.kind())));
            }
            EventInput::SetPatchOverviewOriginEnabled { .. } => {}
            EventInput::EngineSelectionLifecycleAdvanced { .. } => {
                observed.insert("event.engineSelectionLifecycleAdvanced".to_owned());
            }
            EventInput::EnginePrepared { .. } => {
                observed.insert("event.enginePrepared".to_owned());
            }
            EventInput::SampleAssetLifecycleAdvanced { .. } => {}
            EventInput::FileCatalogRefreshed { .. } => {}
            EventInput::EnginePreparationFailed { .. } => {
                observed.insert("event.enginePreparationFailed".to_owned());
            }
            EventInput::EngineActivationAcknowledged { .. } => {
                observed.insert("event.engineActivationAcknowledged".to_owned());
            }
            EventInput::SetSlotOccupancy { .. } => {
                observed.insert("event.setSlotOccupancy".to_owned());
            }
            EventInput::SetReturnOccupancy { .. } => {
                observed.insert("event.setReturnOccupancy".to_owned());
            }
            EventInput::TopologyPrepared { .. } => {
                observed.insert("event.topologyPrepared".to_owned());
            }
            EventInput::TopologyPreparationFailed { .. } => {
                observed.insert("event.topologyPreparationFailed".to_owned());
            }
            EventInput::MidiInputPreferenceRestored { .. }
            | EventInput::MidiInputPreferenceStoreFailed { .. }
            | EventInput::MidiInputScanStarted
            | EventInput::MidiInputScanSucceeded { .. }
            | EventInput::MidiInputScanFailed { .. }
            | EventInput::MidiInputConnectRequested { .. }
            | EventInput::MidiInputConnectionPrepared { .. }
            | EventInput::MidiInputActivationAcknowledged { .. }
            | EventInput::MidiInputDisconnectRequested { .. }
            | EventInput::MidiInputConnectionLost { .. }
            | EventInput::MidiInputOperationFailed { .. }
            | EventInput::Controller { .. }
            | EventInput::MidiInputShutdownRequested
            | EventInput::ReplacePersistedSession { .. }
            | EventInput::AssetImported { .. }
            | EventInput::ToggleTestMidi => {}
        }

        if let Some(rejection) = record.rejection() {
            observed.insert(format!("rejection.{}", rejection_identifier(rejection)));
        }

        for emitted in record.emitted_events() {
            match emitted {
                EmittedEvent::StateAccepted { .. } => {
                    observed.insert("effect.emitted.stateAccepted".to_owned());
                }
                EmittedEvent::ParameterSnapshotPublished { .. } => {
                    observed.insert("effect.emitted.parameterSnapshotPublished".to_owned());
                }
                EmittedEvent::AudioCommand { effect } => match effect {
                    AudioEffect::PatchMidi { .. }
                    | AudioEffect::InitializedVoice { .. }
                    | AudioEffect::VoiceMidi { .. }
                    | AudioEffect::OwnedMidi { .. } => {
                        observed.insert("effect.emitted.audioCommand.patchMidi".to_owned());
                    }
                    AudioEffect::AllNotesOff
                    | AudioEffect::ReleaseImmediateNotes
                    | AudioEffect::ReleaseSource { .. } => {
                        observed.insert("effect.emitted.audioCommand.allNotesOff".to_owned());
                    }
                    AudioEffect::PreviewStart { .. } => {
                        observed.insert("effect.emitted.audioCommand.previewStart".to_owned());
                    }
                    AudioEffect::PreviewStop { .. } => {
                        observed.insert("effect.emitted.audioCommand.previewStop".to_owned());
                    }
                },
                EmittedEvent::EngineSelection { effect } => {
                    observed.insert(format!(
                        "effect.emitted.engineSelection.{}",
                        effect.kind().name()
                    ));
                }
            }
        }
    }
}

fn observe_serialized_properties(
    expected: &[String],
    tree: &StateTree,
    text: &TextProjection,
    event_log: &EventLog,
    observed: &mut BTreeSet<String>,
) -> Result<(), ExhaustiveGuiDemoError> {
    let tree_json: Value = serde_json::from_str(tree.json())
        .map_err(|_| ExhaustiveGuiDemoError::StateTreeSerialization)?;
    let event_log_string = event_log
        .to_json()
        .map_err(|_| ExhaustiveGuiDemoError::EventLogSerialization)?;
    let event_log_json: Value = serde_json::from_str(&event_log_string)
        .map_err(|_| ExhaustiveGuiDemoError::EventLogSerialization)?;

    for identifier in expected
        .iter()
        .filter(|identifier| identifier.starts_with("property."))
    {
        if property_present(identifier, &tree_json, text, &event_log_json) {
            observed.insert(identifier.clone());
        }
    }
    Ok(())
}

fn property_present(
    identifier: &str,
    tree: &Value,
    text: &TextProjection,
    event_log: &Value,
) -> bool {
    let Some(property) = identifier.strip_prefix("property.") else {
        return false;
    };

    if let Some(rest) = property.strip_prefix("stateTree.patch.") {
        return dynamic_patch_property(tree, rest, false);
    }
    if let Some(rest) = property.strip_prefix("stateTree.capability.") {
        return dynamic_capability_property(tree, rest);
    }
    if let Some(rest) = property.strip_prefix("stateTree.effectCapability.") {
        return dynamic_effect_capability_property(tree, rest);
    }
    if let Some(rest) = property.strip_prefix("stateTree.parameters.patch.") {
        return dynamic_patch_property(tree, rest, true);
    }
    if let Some(path) = property.strip_prefix("stateTree.returns.") {
        // The canonical serialized returns section: the property must exist
        // on every entry of the eight-return array.
        return tree
            .get("returns")
            .and_then(Value::as_array)
            .is_some_and(|entries| {
                !entries.is_empty() && entries.iter().all(|entry| entry.get(path).is_some())
            });
    }
    if let Some(path) = property.strip_prefix("stateTree.") {
        return json_path_exists(tree, path);
    }
    if let Some(path) = property.strip_prefix("eventLog.") {
        return json_path_exists(event_log, path);
    }
    if let Some(path) = property.strip_prefix("eventRecord.") {
        let Some(records) = event_log.get("records").and_then(Value::as_array) else {
            return false;
        };
        return !records.is_empty() && records.iter().all(|record| json_path_exists(record, path));
    }
    if let Some(path) = property.strip_prefix("textProjection.") {
        return match path {
            "body" => !text.body().is_empty(),
            "selectedLine" => true,
            "stateHash" => !text.state_hash().is_empty(),
            "context" => TopLevelContext::surface_descriptor().contains(&text.context()),
            _ => false,
        };
    }
    false
}

fn dynamic_patch_property(tree: &Value, rest: &str, parameter_projection: bool) -> bool {
    let Some((patch_id, path)) = rest.split_once('.') else {
        return false;
    };
    let Ok(patch_id) = patch_id.parse::<u64>() else {
        return false;
    };

    let patches = if parameter_projection {
        tree.get("parameters")
            .and_then(|parameters| parameters.get("patches"))
    } else {
        tree.get("patches")
    };
    let Some(patches) = patches.and_then(Value::as_array) else {
        return false;
    };

    let patch = patches.iter().find(|patch| {
        let identity = if parameter_projection {
            patch.get("patchId")
        } else {
            patch.get("id")
        };
        identity.and_then(Value::as_u64) == Some(patch_id)
    });
    let Some(patch) = patch else {
        return false;
    };

    if parameter_projection {
        if let Some(rest) = path.strip_prefix("effect.") {
            // One live entry per ordered position — the property must
            // exist on every entry of the per-position `effects` array.
            return patch
                .get("effects")
                .and_then(Value::as_array)
                .is_some_and(|entries| {
                    !entries.is_empty() && entries.iter().all(|entry| entry.get(rest).is_some())
                });
        }
        if path.starts_with("envelope.")
            || path.starts_with("instrument.")
            || path.starts_with("output.")
        {
            json_path_exists(patch, path)
        } else {
            json_path_exists(patch, &format!("parameters.{path}"))
        }
    } else if let Some(rest) = path.strip_prefix("instrument.value.") {
        semantic_array_property(
            patch
                .get("instrument")
                .and_then(|instrument| instrument.get("values")),
            rest,
        )
    } else if let Some(rest) = path.strip_prefix("instrument.asset.") {
        semantic_array_property(
            patch
                .get("instrument")
                .and_then(|instrument| instrument.get("assetReferences")),
            rest,
        )
    } else if let Some(rest) = path.strip_prefix("postEffect.") {
        dynamic_post_effect_property(patch, rest)
    } else {
        json_path_exists(patch, path)
    }
}

fn dynamic_post_effect_property(patch: &Value, rest: &str) -> bool {
    let Some((slot_id, path)) = rest.split_once('.') else {
        return false;
    };
    let Ok(slot_id) = slot_id.parse::<u64>() else {
        return false;
    };
    let Some(config) = patch
        .get("postEffects")
        .and_then(Value::as_array)
        .and_then(|effects| {
            effects
                .iter()
                .find(|effect| effect.get("slotId").and_then(Value::as_u64) == Some(slot_id))
        })
    else {
        return false;
    };
    if let Some(rest) = path.strip_prefix("value.") {
        semantic_array_property(config.get("values"), rest)
    } else if let Some(rest) = path.strip_prefix("asset.") {
        semantic_array_property(config.get("assetReferences"), rest)
    } else {
        json_path_exists(config, path)
    }
}

fn dynamic_capability_property(tree: &Value, rest: &str) -> bool {
    let Some(descriptors) = tree
        .get("capabilities")
        .and_then(|capabilities| capabilities.get("descriptors"))
        .and_then(Value::as_array)
    else {
        return false;
    };
    let Some((descriptor, path)) = semantic_object(descriptors, rest) else {
        return false;
    };

    if let Some(rest) = path.strip_prefix("section.") {
        let Some(sections) = descriptor.get("sections").and_then(Value::as_array) else {
            return false;
        };
        let Some((section, path)) = semantic_object(sections, rest) else {
            return false;
        };
        return json_path_exists(section, path);
    }
    if let Some(rest) = path.strip_prefix("parameter.") {
        let Some(sections) = descriptor.get("sections").and_then(Value::as_array) else {
            return false;
        };
        for section in sections {
            if semantic_array_property(section.get("parameters"), rest) {
                return true;
            }
        }
        return false;
    }
    if let Some(rest) = path.strip_prefix("asset.") {
        return semantic_array_property(descriptor.get("assetRequirements"), rest);
    }
    json_path_exists(descriptor, path)
}

fn dynamic_effect_capability_property(tree: &Value, rest: &str) -> bool {
    let Some(descriptors) = tree
        .get("effects")
        .and_then(|capabilities| capabilities.get("descriptors"))
        .and_then(Value::as_array)
    else {
        return false;
    };
    let Some((descriptor, path)) = semantic_object(descriptors, rest) else {
        return false;
    };
    if let Some(rest) = path.strip_prefix("section.") {
        let Some(sections) = descriptor.get("sections").and_then(Value::as_array) else {
            return false;
        };
        let Some((section, path)) = semantic_object(sections, rest) else {
            return false;
        };
        return json_path_exists(section, path);
    }
    if let Some(rest) = path.strip_prefix("parameter.") {
        let Some(sections) = descriptor.get("sections").and_then(Value::as_array) else {
            return false;
        };
        return sections
            .iter()
            .any(|section| semantic_array_property(section.get("parameters"), rest));
    }
    if let Some(rest) = path.strip_prefix("asset.") {
        return semantic_array_property(descriptor.get("assetRequirements"), rest);
    }
    json_path_exists(descriptor, path)
}

fn semantic_array_property(array: Option<&Value>, rest: &str) -> bool {
    let Some(array) = array.and_then(Value::as_array) else {
        return false;
    };
    let Some((value, path)) = semantic_object(array, rest) else {
        return false;
    };
    json_path_exists(value, path)
}

fn semantic_object<'a>(array: &'a [Value], rest: &'a str) -> Option<(&'a Value, &'a str)> {
    array.iter().find_map(|value| {
        let id = value
            .get("id")
            .or_else(|| value.get("parameterId"))?
            .as_str()?;
        rest.strip_prefix(id)
            .and_then(|suffix| suffix.strip_prefix('.'))
            .map(|path| (value, path))
    })
}

fn json_path_exists(value: &Value, path: &str) -> bool {
    let mut current = value;
    for part in path.split('.') {
        let Some(next) = current.get(part) else {
            return false;
        };
        current = next;
    }
    true
}

fn changed_parameter_identifier(before: &StateTree, after: &StateTree) -> Option<String> {
    let before: Value = serde_json::from_str(before.json()).ok()?;
    let after: Value = serde_json::from_str(after.json()).ok()?;

    if before.pointer("/interaction/activeFocus") != after.pointer("/interaction/activeFocus") {
        return None;
    }

    let before_patches = before.get("patches")?.as_array()?;
    let after_patches = after.get("patches")?.as_array()?;
    if before_patches.len() != after_patches.len() {
        return None;
    }

    let mut changes = Vec::new();
    for (before_patch, after_patch) in before_patches.iter().zip(after_patches) {
        for property in ["id", "name", "channel"] {
            if before_patch.get(property) != after_patch.get(property) {
                return None;
            }
        }
        let patch_id = after_patch.get("id")?.as_u64()?;
        for (field, parameter) in [("trimGainDb", "trimGainDb"), ("trackId", "outputTrack")] {
            let before_value = before_patch
                .get("output")
                .and_then(|output| output.get(field))?;
            let after_value = after_patch
                .get("output")
                .and_then(|output| output.get(field))?;
            if before_value != after_value {
                changes.push(format!("parameter.patch.{patch_id}.output.{parameter}"));
            }
        }
        for parameter in [
            "attackMilliseconds",
            "decayMilliseconds",
            "sustain",
            "releaseMilliseconds",
        ] {
            let before_value = before_patch
                .get("envelope")
                .and_then(|envelope| envelope.get(parameter))?;
            let after_value = after_patch
                .get("envelope")
                .and_then(|envelope| envelope.get(parameter))?;
            if before_value != after_value {
                changes.push(format!("parameter.patch.{patch_id}.{parameter}"));
            }
        }

        let before_instrument = before_patch.get("instrument")?;
        let after_instrument = after_patch.get("instrument")?;
        for property in ["capabilityId", "assetReferences"] {
            if before_instrument.get(property) != after_instrument.get(property) {
                return None;
            }
        }
        let before_values = before_instrument.get("values")?.as_array()?;
        let after_values = after_instrument.get("values")?.as_array()?;
        if before_values.len() != after_values.len() {
            return None;
        }
        for (before_value, after_value) in before_values.iter().zip(after_values) {
            let parameter_id = after_value.get("parameterId")?.as_str()?;
            if before_value.get("parameterId") != after_value.get("parameterId") {
                return None;
            }
            if before_value.get("value") != after_value.get("value") {
                changes.push(format!("parameter.patch.{patch_id}.{parameter_id}"));
            }
        }

        let before_effects = before_patch.get("postEffects")?.as_array()?;
        let after_effects = after_patch.get("postEffects")?.as_array()?;
        if before_effects.len() != after_effects.len() {
            return None;
        }
        for (before_effect, after_effect) in before_effects.iter().zip(after_effects) {
            for property in ["slotId", "capabilityId", "assetReferences"] {
                if before_effect.get(property) != after_effect.get(property) {
                    return None;
                }
            }
            let slot_id = after_effect.get("slotId")?.as_u64()?;
            let before_values = before_effect.get("values")?.as_array()?;
            let after_values = after_effect.get("values")?.as_array()?;
            if before_values.len() != after_values.len() {
                return None;
            }
            for (before_value, after_value) in before_values.iter().zip(after_values) {
                let parameter_id = after_value.get("parameterId")?.as_str()?;
                if before_value.get("parameterId") != after_value.get("parameterId") {
                    return None;
                }
                if before_value.get("value") != after_value.get("value") {
                    changes.push(format!(
                        "parameter.patch.{patch_id}.effect.{slot_id}.{parameter_id}"
                    ));
                }
            }
        }
    }

    let before_tracks = before.pointer("/mixer/tracks")?.as_array()?;
    let after_tracks = after.pointer("/mixer/tracks")?.as_array()?;
    if before_tracks.len() != MixerTrackId::COUNT || after_tracks.len() != MixerTrackId::COUNT {
        return None;
    }
    for (track_id, (before_track, after_track)) in MixerTrackId::ALL
        .into_iter()
        .zip(before_tracks.iter().zip(after_tracks))
    {
        for parameter in ["levelDb", "pan", "mute", "solo"] {
            if before_track.get(parameter)? != after_track.get(parameter)? {
                changes.push(format!("parameter.track.{track_id}.{parameter}"));
            }
        }
        // All eight sends are one indexed array addressed by BusId.
        let before_sends = before_track.get("sends")?.as_array()?;
        let after_sends = after_track.get("sends")?.as_array()?;
        if before_sends.len() != after_sends.len() {
            return None;
        }
        for (send_index, (before_send, after_send)) in
            before_sends.iter().zip(after_sends).enumerate()
        {
            if before_send != after_send {
                changes.push(format!("parameter.track.{track_id}.sends[{send_index}]"));
            }
        }
    }

    {
        let before_value = before.pointer("/global/masterGainDb")?;
        let after_value = after.pointer("/global/masterGainDb")?;
        if before_value != after_value {
            changes.push("parameter.global.masterGainDb".to_owned());
        }
    }
    // Return-owned state: the return level plus the occupying registry
    // entry's values, addressed positionally by BusId. Occupancy identity
    // changes are structural, never a single scalar adjustment.
    let before_returns = before.pointer("/returns")?.as_array()?;
    let after_returns = after.pointer("/returns")?.as_array()?;
    if before_returns.len() != after_returns.len() {
        return None;
    }
    for (bus_index, (before_return, after_return)) in
        before_returns.iter().zip(after_returns).enumerate()
    {
        if before_return.get("returnLevel")? != after_return.get("returnLevel")? {
            changes.push(format!("parameter.return.B{bus_index}.returnLevel"));
        }
        let before_effect = before_return.get("effect")?;
        let after_effect = after_return.get("effect")?;
        match (before_effect.is_null(), after_effect.is_null()) {
            (true, true) => continue,
            (false, false) => {}
            _ => return None,
        }
        for property in ["slotId", "capabilityId", "assetReferences"] {
            if before_effect.get(property) != after_effect.get(property) {
                return None;
            }
        }
        let before_values = before_effect.get("values")?.as_array()?;
        let after_values = after_effect.get("values")?.as_array()?;
        if before_values.len() != after_values.len() {
            return None;
        }
        for (before_value, after_value) in before_values.iter().zip(after_values) {
            let parameter_id = after_value.get("parameterId")?.as_str()?;
            if before_value.get("parameterId") != after_value.get("parameterId") {
                return None;
            }
            if before_value.get("value") != after_value.get("value") {
                changes.push(format!("parameter.return.B{bus_index}.{parameter_id}"));
            }
        }
    }

    if changes.len() == 1 {
        changes.pop()
    } else {
        None
    }
}

const fn direction_identifier(direction: EventDirection) -> &'static str {
    match direction {
        EventDirection::Up => "up",
        EventDirection::Down => "down",
        EventDirection::Left => "left",
        EventDirection::Right => "right",
    }
}

const fn midi_kind_identifier(kind: MidiKind) -> &'static str {
    match kind {
        MidiKind::NoteOn => "noteOn",
        MidiKind::NoteOff => "noteOff",
        MidiKind::ControlChange => "controlChange",
        MidiKind::ProgramChange => "programChange",
        MidiKind::ChannelPressure => "channelPressure",
        MidiKind::PitchBend => "pitchBend",
        MidiKind::AllNotesOff => "allNotesOff",
        MidiKind::PolyPressure => "polyPressure",
    }
}

const fn rejection_identifier(rejection: EventRejection) -> &'static str {
    match rejection {
        EventRejection::InstallationClosed => "installationClosed",
        EventRejection::TooManyPatches => "tooManyPatches",
        EventRejection::InvalidInstrumentConfig => "invalidInstrumentConfig",
        EventRejection::InvalidEffectConfig => "invalidEffectConfig",
        EventRejection::NoPatchesInstalled => "noPatchesInstalled",
        EventRejection::UnknownPatch => "unknownPatch",
        EventRejection::InvalidSelection => "invalidSelection",
        EventRejection::ParameterAtBoundary => "parameterAtBoundary",
        EventRejection::InvalidParameterValue => "invalidParameterValue",
        EventRejection::ActionUnavailableInContext => "actionUnavailableInContext",
        EventRejection::EngineSelectionUnavailable => "engineSelectionUnavailable",
        EventRejection::StructuralEditBusy => "structuralEditBusy",
        EventRejection::StaleEngineSelection => "staleEngineSelection",
        EventRejection::MismatchedEngineSelection => "mismatchedEngineSelection",
        EventRejection::RequestIdOverflow => "requestIdOverflow",
        EventRejection::GenerationOverflow => "generationOverflow",
    }
}

const fn window_input_identifier(input: WindowInput) -> &'static str {
    match (input.kind(), input.key()) {
        (WindowInputKind::KeyDown, WindowKey::Digit1) => "keyDown.digit1",
        (WindowInputKind::KeyDown, WindowKey::Digit2) => "keyDown.digit2",
        (WindowInputKind::KeyDown, WindowKey::Digit3) => "keyDown.digit3",
        (WindowInputKind::KeyDown, WindowKey::Digit4) => "keyDown.digit4",
        (WindowInputKind::KeyDown, WindowKey::Digit5) => "keyDown.digit5",
        (WindowInputKind::KeyDown, WindowKey::Digit6) => "keyDown.digit6",
        (WindowInputKind::KeyDown, WindowKey::Digit7) => "keyDown.digit7",
        (WindowInputKind::KeyDown, WindowKey::Digit8) => "keyDown.digit8",
        (WindowInputKind::KeyDown, WindowKey::Digit9) => "keyDown.digit9",
        (WindowInputKind::KeyDown, WindowKey::Digit0) => "keyDown.digit0",
        (WindowInputKind::KeyDown, WindowKey::BracketLeft) => "keyDown.bracketLeft",
        (WindowInputKind::KeyDown, WindowKey::BracketRight) => "keyDown.bracketRight",
        (WindowInputKind::KeyDown, WindowKey::Q) => "keyDown.q",
        (WindowInputKind::KeyDown, WindowKey::E) => "keyDown.e",
        (WindowInputKind::KeyDown, WindowKey::W) => "keyDown.w",
        (WindowInputKind::KeyDown, WindowKey::S) => "keyDown.s",
        (WindowInputKind::KeyDown, WindowKey::A) => "keyDown.a",
        (WindowInputKind::KeyDown, WindowKey::D) => "keyDown.d",
        (WindowInputKind::KeyDown, WindowKey::K) => "keyDown.k",
        (WindowInputKind::KeyDown, WindowKey::Shift) => "keyDown.shift",
        (WindowInputKind::KeyDown, WindowKey::Return) => "keyDown.return",
        (WindowInputKind::KeyDown, WindowKey::Space) => "keyDown.space",
        (WindowInputKind::KeyDown, WindowKey::Other) => "keyDown.other",
        (WindowInputKind::KeyUp, WindowKey::Digit1) => "keyUp.digit1",
        (WindowInputKind::KeyUp, WindowKey::Digit2) => "keyUp.digit2",
        (WindowInputKind::KeyUp, WindowKey::Digit3) => "keyUp.digit3",
        (WindowInputKind::KeyUp, WindowKey::Digit4) => "keyUp.digit4",
        (WindowInputKind::KeyUp, WindowKey::Digit5) => "keyUp.digit5",
        (WindowInputKind::KeyUp, WindowKey::Digit6) => "keyUp.digit6",
        (WindowInputKind::KeyUp, WindowKey::Digit7) => "keyUp.digit7",
        (WindowInputKind::KeyUp, WindowKey::Digit8) => "keyUp.digit8",
        (WindowInputKind::KeyUp, WindowKey::Digit9) => "keyUp.digit9",
        (WindowInputKind::KeyUp, WindowKey::Digit0) => "keyUp.digit0",
        (WindowInputKind::KeyUp, WindowKey::BracketLeft) => "keyUp.bracketLeft",
        (WindowInputKind::KeyUp, WindowKey::BracketRight) => "keyUp.bracketRight",
        (WindowInputKind::KeyUp, WindowKey::Q) => "keyUp.q",
        (WindowInputKind::KeyUp, WindowKey::E) => "keyUp.e",
        (WindowInputKind::KeyUp, WindowKey::W) => "keyUp.w",
        (WindowInputKind::KeyUp, WindowKey::S) => "keyUp.s",
        (WindowInputKind::KeyUp, WindowKey::A) => "keyUp.a",
        (WindowInputKind::KeyUp, WindowKey::D) => "keyUp.d",
        (WindowInputKind::KeyUp, WindowKey::K) => "keyUp.k",
        (WindowInputKind::KeyUp, WindowKey::Shift) => "keyUp.shift",
        (WindowInputKind::KeyUp, WindowKey::Return) => "keyUp.return",
        (WindowInputKind::KeyUp, WindowKey::Space) => "keyUp.space",
        (WindowInputKind::KeyUp, WindowKey::Other) => "keyUp.other",
        (WindowInputKind::KeyDown, WindowKey::T) => "keyDown.t",
        (WindowInputKind::KeyUp, WindowKey::T) => "keyUp.t",
        (WindowInputKind::FocusLost, _) => "focusLost",
    }
}

#[cfg(test)]
mod tests {
    use super::ExhaustiveGuiDemo;
    use crate::adapter::braids_capability::BRAIDS_CAPABILITY_ID;
    use crate::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID;
    use crate::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
    use crate::adapter::lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary;
    use crate::adapter::production_instruments::{
        production_capability_registry, production_instrument_preparers,
        production_instrument_providers,
    };
    use crate::control::app_event::AppEvent;
    use crate::control::app_loop::AppLoop;
    use crate::control::app_state::AppState;
    use crate::control::event_log::EventLog;
    use crate::control::event_record::{EventOutcome, EventSource};
    use crate::control::state_projector::StateProjector;
    use crate::kernel::midi_channel::MidiChannel;
    use crate::kernel::patch_id::PatchId;
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_state::MixerState;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::audio_boundary::AudioBoundary;
    use crate::real_time::audio_renderer::AudioRenderer;
    use crate::real_time::parameter_snapshot::ParameterSnapshot;
    use crate::real_time::prepared_graph_builder::PreparedGraphBuilder;
    use crate::real_time::{GraphHandoffStatus, GraphRevision, StructuralGraphBoundary};
    use crate::shell::audio_output::{AudioDeviceConfig, AudioSampleFormat};
    use crate::synth::{CapabilityId, DescriptorDefaultConfigFactory, Patch};
    use crate::testing::demo_scene::DemoScene;
    use crate::testing::demo_scene_report::DemoCoverageGroup;
    use crate::testing::DeterministicGraphPreparationWorker;

    fn globals() -> GlobalParameters {
        GlobalParameters::new(0.0).unwrap()
    }

    #[test]
    fn exhaustive_gui_demo_scene_uses_production_seams_and_has_no_coverage_gaps() {
        let registry = production_capability_registry().unwrap();
        let factory = DescriptorDefaultConfigFactory::new(
            registry.clone(),
            production_instrument_providers().unwrap(),
        );
        let patches = vec![
            Patch::new(
                PatchId::new(3).unwrap(),
                "Fixture 3".to_owned(),
                factory
                    .create(&CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap())
                    .unwrap(),
                MidiChannel::new(1).unwrap(),
                PatchOutput::to_track(MixerTrackId::new(1).unwrap()),
            ),
            Patch::new(
                PatchId::new(11).unwrap(),
                "Fixture 11".to_owned(),
                factory
                    .create(&CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap())
                    .unwrap(),
                MidiChannel::new(9).unwrap(),
                PatchOutput::to_track(MixerTrackId::new(9).unwrap()),
            ),
        ];
        let effects = crate::adapter::production_effects::production_effect_registry().unwrap();
        let startup_returns = crate::adapter::production_effects::startup_bus_returns(&effects);
        let scene = DemoScene::exhaustive_with_effects(
            &registry,
            &effects,
            &patches,
            &globals(),
            &startup_returns,
        )
        .unwrap();
        let initial_parameters =
            ParameterSnapshot::new(0, globals(), MixerState::default(), &[]).unwrap();
        let boundary = LockFreeAudioBoundary::new(64, initial_parameters);
        let (control, audio) = boundary.into_handles();

        let event_log = EventLog::new(scene.event_log_capacity().saturating_add(2)).unwrap();
        let mut app_loop = AppLoop::with_event_log(
            AppState::new_with_effects(registry.clone(), effects.clone(), globals())
                .with_initial_returns(startup_returns),
            StateProjector::new(),
            control,
            event_log,
        )
        .unwrap();
        app_loop
            .dispatch_from(
                AppEvent::InstallPatches(patches),
                EventSource::AutomaticMidi,
            )
            .unwrap();

        let preparers = production_instrument_preparers().unwrap();
        let effect_preparers =
            crate::adapter::production_effects::production_effect_preparers().unwrap();
        let graph = PreparedGraphBuilder::new(app_loop.capabilities(), &preparers)
            .with_effects(app_loop.effects(), &effect_preparers)
            .with_returns(app_loop.bus_returns())
            .build(
                crate::real_time::GraphRevision::INITIAL,
                app_loop.patches(),
                app_loop.current_parameters().clone(),
                48_000.0,
                512,
            )
            .unwrap();
        let structural = LockFreeStructuralGraphBoundary::new(
            1,
            1,
            GraphHandoffStatus::with_active(GraphRevision::INITIAL),
        )
        .unwrap();
        let (structural_control, structural_audio) = structural.into_handles();
        let audio_config =
            AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 512).unwrap();
        let worker = DeterministicGraphPreparationWorker::new_with_effects(
            registry.clone(),
            production_instrument_preparers().unwrap(),
            effects.clone(),
            crate::adapter::production_effects::production_effect_preparers().unwrap(),
            audio_config,
        );
        let worker_handle = worker.advance_handle();
        app_loop
            .configure_engine_selection(
                DescriptorDefaultConfigFactory::new(
                    registry,
                    production_instrument_providers().unwrap(),
                ),
                worker,
                structural_control,
                &graph,
                audio_config,
            )
            .unwrap();
        let mut renderer = AudioRenderer::new(audio, structural_audio, graph);
        let mut audio_buffer = vec![0.0; 1_024];

        let mut demo = ExhaustiveGuiDemo::new_with_worker(
            &mut app_loop,
            &mut renderer,
            &mut audio_buffer,
            worker_handle,
        );
        let report = demo.run(scene).unwrap();

        assert!(
            report.is_complete(),
            "coverage={:?}; eventCoverageMissing={:?}; eventCoverageUnexpected={:?}",
            report.coverage(),
            report.event_log().coverage().missing(),
            report.event_log().coverage().unexpected()
        );
        assert_eq!(report.coverage().missing_count(), 0);
        assert_eq!(report.event_log().dropped_records(), 0);
        assert_eq!(report.final_state_tree().patch_count(), 2);
        assert!(report
            .checkpoints()
            .iter()
            .all(|checkpoint| checkpoint.audio_measurement().is_finite()));

        for group in [
            DemoCoverageGroup::Inputs,
            DemoCoverageGroup::Events,
            DemoCoverageGroup::Contexts,
            DemoCoverageGroup::Directions,
            DemoCoverageGroup::MidiKinds,
            DemoCoverageGroup::EditableParameters,
            DemoCoverageGroup::SerializedProperties,
            DemoCoverageGroup::Rejections,
            DemoCoverageGroup::Projections,
            DemoCoverageGroup::AudioEffects,
        ] {
            assert!(report.coverage().group(group).is_complete());
        }

        let accepted = report
            .event_log()
            .records()
            .iter()
            .filter(|record| record.outcome() == EventOutcome::Accepted)
            .count();
        let rejected = report
            .event_log()
            .records()
            .iter()
            .filter(|record| record.outcome() == EventOutcome::Rejected)
            .count();
        assert!(accepted > 0);
        assert!(rejected >= 4);
    }
}
