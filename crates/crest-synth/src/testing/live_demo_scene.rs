use crate::control::app_event::{AppEvent, Direction};
use crate::control::app_state::EventRejection;
use crate::control::event_record::{EmittedEvent, EventDirection, EventInput, EventOutcome};
use crate::control::patch_page_projection::{
    PatchPageEnvelopeRow, PatchPageOutputRow, PatchPageParameterRow,
};
use crate::control::state_tree::StateTree;
use crate::control::{
    FocusPath, InteractionMode, MixerControlId, PatchControlId, SemanticControlId,
    StructuralEditIntent, SurfaceId, TopLevelContext,
};
use crate::kernel::midi_channel::MidiChannel;
use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crate::kernel::patch_id::PatchId;
use crate::mixer::global_parameters::{GlobalParameter, GlobalParameters};
use crate::mixer::mixer_state::MixerState;
use crate::mixer::mixer_track_id::MixerTrackId;
use crate::mixer::mixer_track_parameters::{
    MixerTrackParameter, MixerTrackParameterKind, MixerTrackParameters,
};
use crate::mixer::patch_output::{PatchOutput, PatchOutputParameter};
use crate::real_time::audio_command::AudioCommand;
use crate::real_time::audio_observation_snapshot::AudioObservationSnapshot;
use crate::real_time::GraphRevision;
use crate::synth::instrument_capability::{
    CapabilityDescriptor, CapabilityRegistry, InstrumentConfig, ParameterAdjustment, ParameterValue,
};
use crate::synth::patch::{resolve_patch_editable_targets, PatchEditableTarget};
use crate::synth::voice_envelope::VoiceEnvelope;
use crate::synth::{
    CapabilityId, EffectCapabilityRegistry, EffectSlotId, ParameterId, ParameterKind,
    PatchInteraction, PostEffectConfig,
};
use core::fmt;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const PARAMETER_PROBE_NOTE: u8 = 60;
const PARAMETER_PROBE_VELOCITY: u8 = 112;

/// One canonical editable value in the installed live-demo surface.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(tag = "scope", rename_all = "camelCase")]
pub enum LiveEditableParameter {
    Patch {
        #[serde(rename = "patchId")]
        patch_id: PatchId,
        target: PatchEditableTarget,
    },
    PatchOutput {
        #[serde(rename = "patchId")]
        patch_id: PatchId,
        parameter: PatchOutputParameter,
    },
    Track {
        #[serde(rename = "trackId")]
        track_id: MixerTrackId,
        parameter: MixerTrackParameter,
    },
    Send {
        #[serde(rename = "trackId")]
        track_id: MixerTrackId,
        bus: crate::mixer::bus_id::BusId,
    },
    Global {
        parameter: GlobalParameter,
    },
    Effect {
        #[serde(rename = "patchId")]
        patch_id: PatchId,
        #[serde(rename = "slotId")]
        slot_id: EffectSlotId,
        #[serde(rename = "parameterId")]
        parameter_id: ParameterId,
    },
}

impl LiveEditableParameter {
    pub const fn patch_output(patch_id: PatchId, parameter: PatchOutputParameter) -> Self {
        Self::PatchOutput {
            patch_id,
            parameter,
        }
    }

    pub const fn track(track_id: MixerTrackId, parameter: MixerTrackParameter) -> Self {
        Self::Track {
            track_id,
            parameter,
        }
    }

    pub const fn send(track_id: MixerTrackId, bus: crate::mixer::bus_id::BusId) -> Self {
        Self::Send { track_id, bus }
    }

    pub fn patch_target(patch_id: PatchId, target: PatchEditableTarget) -> Self {
        Self::Patch { patch_id, target }
    }

    pub const fn global(parameter: GlobalParameter) -> Self {
        Self::Global { parameter }
    }

    pub fn effect(patch_id: PatchId, slot_id: EffectSlotId, parameter_id: ParameterId) -> Self {
        Self::Effect {
            patch_id,
            slot_id,
            parameter_id,
        }
    }

    /// Returns a stable descriptor-derived coverage identifier.
    pub fn identifier(&self) -> String {
        match self {
            Self::Patch { patch_id, target } => {
                format!("patch.{}.{}", patch_id.value(), target.name())
            }
            Self::PatchOutput {
                patch_id,
                parameter,
            } => format!("patch.{}.output.{parameter}", patch_id.value()),
            Self::Track {
                track_id,
                parameter,
            } => format!("track.{track_id}.{parameter}"),
            Self::Send { track_id, bus } => {
                format!("track.{track_id}.sends[{}]", bus.index())
            }
            Self::Global { parameter } => format!("global.{parameter}"),
            Self::Effect {
                patch_id,
                slot_id,
                parameter_id,
            } => format!(
                "patch.{}.effect.{}.{}",
                patch_id.value(),
                slot_id.value(),
                parameter_id
            ),
        }
    }

    pub const fn audio_predicate(&self) -> LiveAudioPredicate {
        match self {
            Self::PatchOutput { .. }
            | Self::Global {
                parameter: GlobalParameter::MasterGainDb,
            } => LiveAudioPredicate::OutputLevel,
            Self::Track {
                track_id,
                parameter: MixerTrackParameter::Level | MixerTrackParameter::Solo,
            } => LiveAudioPredicate::TrackOutput {
                track_id: *track_id,
            },
            Self::Track {
                track_id,
                parameter: MixerTrackParameter::Pan,
            } => LiveAudioPredicate::TrackStereoBalance {
                track_id: *track_id,
            },
            Self::Track {
                track_id,
                parameter: MixerTrackParameter::Mute,
            } => LiveAudioPredicate::TrackPreGateMeter {
                track_id: *track_id,
            },
            // The production composition occupies returns 0 and 1 with the
            // default reverb and delay entries, so sends toward those buses
            // are measurable at the matching effect inputs. Other buses fall
            // back to the track-output witness.
            Self::Send { track_id, bus } => match bus.index() {
                0 => LiveAudioPredicate::TrackReverbInput {
                    track_id: *track_id,
                },
                1 => LiveAudioPredicate::TrackDelayInput {
                    track_id: *track_id,
                },
                _ => LiveAudioPredicate::TrackOutput {
                    track_id: *track_id,
                },
            },
            Self::Patch { .. } => LiveAudioPredicate::OutputLevel,
            Self::Effect { patch_id, .. } => LiveAudioPredicate::PatchEffect {
                patch_id: *patch_id,
            },
        }
    }

    pub fn field_name(&self) -> String {
        match self {
            Self::Patch { target, .. } => target.name().to_owned(),
            Self::PatchOutput { parameter, .. } => parameter.name().to_owned(),
            Self::Track { parameter, .. } => parameter.name().to_owned(),
            Self::Send { bus, .. } => format!("sends[{}]", bus.index()),
            Self::Global { parameter } => parameter.name().to_owned(),
            Self::Effect { parameter_id, .. } => parameter_id.as_str().to_owned(),
        }
    }
}

/// The causal measurement required for one editable parameter.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LiveAudioPredicate {
    OutputLevel,
    StereoBalance,
    ReverbInput,
    DelayInput,
    WetOutput,
    TrackOutput {
        #[serde(rename = "trackId")]
        track_id: MixerTrackId,
    },
    TrackStereoBalance {
        #[serde(rename = "trackId")]
        track_id: MixerTrackId,
    },
    TrackPreGateMeter {
        #[serde(rename = "trackId")]
        track_id: MixerTrackId,
    },
    TrackReverbInput {
        #[serde(rename = "trackId")]
        track_id: MixerTrackId,
    },
    TrackDelayInput {
        #[serde(rename = "trackId")]
        track_id: MixerTrackId,
    },
    PatchEffect {
        #[serde(rename = "patchId")]
        patch_id: PatchId,
    },
}

impl LiveAudioPredicate {
    /// Tests only finite fields measured by the production render path.
    pub fn evaluate(self, observation: AudioObservationSnapshot) -> bool {
        let finite = observation.left_peak().is_finite()
            && observation.right_peak().is_finite()
            && observation.output_rms().is_finite()
            && observation.reverb_input_rms().is_finite()
            && observation.delay_input_rms().is_finite()
            && observation.wet_output_rms().is_finite()
            && observation.non_finite_samples() == 0;
        if !finite {
            return false;
        }

        match self {
            Self::OutputLevel => observation.output_rms() > 0.0,
            Self::StereoBalance => {
                observation.output_rms() > 0.0
                    && (observation.left_peak() - observation.right_peak()).abs() > f32::EPSILON
            }
            Self::ReverbInput => observation.reverb_input_rms() > 0.0,
            Self::DelayInput => observation.delay_input_rms() > 0.0,
            Self::WetOutput => observation.wet_output_rms() > 0.0,
            Self::TrackOutput { track_id } => {
                track_meter_is_nonzero(observation, track_id) && observation.output_rms() > 0.0
            }
            Self::TrackStereoBalance { track_id } => {
                track_meter_is_nonzero(observation, track_id)
                    && observation.output_rms() > 0.0
                    && (observation.left_peak() - observation.right_peak()).abs() > f32::EPSILON
            }
            Self::TrackPreGateMeter { track_id } => track_meter_is_nonzero(observation, track_id),
            Self::TrackReverbInput { track_id } => {
                track_meter_is_nonzero(observation, track_id)
                    && observation.reverb_input_rms() > 0.0
            }
            Self::TrackDelayInput { track_id } => {
                track_meter_is_nonzero(observation, track_id) && observation.delay_input_rms() > 0.0
            }
            Self::PatchEffect { patch_id } => {
                let effect = observation.patch_effect();
                effect.patch_id() == Some(patch_id)
                    && effect.input_rms() > 0.0
                    && effect.output_rms() > 0.0
                    && effect.difference_rms() > 0.0
                    && effect.side_rms() > 0.0
            }
        }
    }

    pub const fn is_patch_effect(self) -> bool {
        matches!(self, Self::PatchEffect { .. })
    }
}

fn track_meter_is_nonzero(observation: AudioObservationSnapshot, track_id: MixerTrackId) -> bool {
    let meter = observation.track(track_id);
    meter.left_peak().is_finite()
        && meter.right_peak().is_finite()
        && meter.rms().is_finite()
        && meter.rms() > 0.0
}

/// A transition oracle fixed before its semantic event is dispatched.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveExpectedTransition {
    input: EventInput,
    outcome: EventOutcome,
    generation_before: u64,
    generation_after: u64,
    parameter_generation: u64,
    editable_parameter: Option<LiveEditableParameter>,
    patch_control_id: Option<PatchControlId>,
    value_before: Option<f32>,
    value_after: Option<f32>,
    selected_line: Option<usize>,
    selected_text: Option<String>,
    emitted_effects: Vec<EmittedEvent>,
    rejection: Option<String>,
}

impl LiveExpectedTransition {
    pub(crate) fn for_step(
        step: &LiveDemoStep,
        generation_before: u64,
        graph_revision: GraphRevision,
        selected_line: usize,
        observed_value: Option<f32>,
    ) -> Result<Self, LiveDemoSceneError> {
        if let Some(expected_before) = step.value_before {
            let actual = observed_value.ok_or(LiveDemoSceneError::SelectedParameterMismatch)?;
            if actual != expected_before {
                return Err(LiveDemoSceneError::ExpectedValueMismatch {
                    expected: expected_before,
                    actual,
                });
            }
        }

        let generation_after = if step.expected_outcome == EventOutcome::Accepted {
            generation_before
                .checked_add(1)
                .ok_or(LiveDemoSceneError::GenerationOverflow)?
        } else {
            generation_before
        };

        let selected_text = step.selected_text_after.clone();
        let selected_line = step.editable_parameter.as_ref().map(|_| selected_line);
        let mut emitted_effects = Vec::new();
        if step.expected_outcome == EventOutcome::Accepted {
            emitted_effects.push(EmittedEvent::StateAccepted {
                generation: generation_after,
            });
            if EventInput::from(&step.event).publishes_parameters_on_acceptance() {
                emitted_effects.push(EmittedEvent::ParameterSnapshotPublished {
                    generation: generation_after,
                    graph_revision,
                });
            }
            if let AppEvent::Midi { patch_id, message } = &step.event {
                emitted_effects.push(EmittedEvent::AudioCommand {
                    effect: AudioCommand::patch_midi(*patch_id, *message).into(),
                });
            }
        }

        Ok(Self {
            input: EventInput::from(&step.event),
            outcome: step.expected_outcome,
            generation_before,
            generation_after,
            parameter_generation: generation_after,
            editable_parameter: step.editable_parameter.clone(),
            patch_control_id: step.patch_control_id.clone(),
            value_before: step.value_before,
            value_after: step.value_after,
            selected_line,
            selected_text,
            emitted_effects,
            rejection: step.expected_rejection.map(|value| value.name().to_owned()),
        })
    }

    pub const fn input(&self) -> &EventInput {
        &self.input
    }

    pub const fn outcome(&self) -> EventOutcome {
        self.outcome
    }

    pub const fn generation_before(&self) -> u64 {
        self.generation_before
    }

    pub const fn generation_after(&self) -> u64 {
        self.generation_after
    }

    pub const fn parameter_generation(&self) -> u64 {
        self.parameter_generation
    }

    pub const fn editable_parameter(&self) -> Option<&LiveEditableParameter> {
        self.editable_parameter.as_ref()
    }

    pub fn patch_control_id(&self) -> Option<PatchControlId> {
        self.patch_control_id.clone()
    }

    pub const fn value_before(&self) -> Option<f32> {
        self.value_before
    }

    pub const fn value_after(&self) -> Option<f32> {
        self.value_after
    }

    pub const fn selected_line(&self) -> Option<usize> {
        self.selected_line
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selected_text.as_deref()
    }

    pub fn emitted_effects(&self) -> &[EmittedEvent] {
        &self.emitted_effects
    }

    pub fn rejection(&self) -> Option<&str> {
        self.rejection.as_deref()
    }
}

/// One bounded semantic action in the live scene.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveDemoStep {
    event: AppEvent,
    expected_outcome: EventOutcome,
    expected_rejection: Option<EventRejection>,
    editable_parameter: Option<LiveEditableParameter>,
    patch_control_id: Option<PatchControlId>,
    value_before: Option<f32>,
    value_after: Option<f32>,
    selected_text_after: Option<String>,
    checkpoint: bool,
    cleanup: bool,
}

impl LiveDemoStep {
    fn accepted_event(event: AppEvent) -> Self {
        Self {
            event,
            expected_outcome: EventOutcome::Accepted,
            expected_rejection: None,
            editable_parameter: None,
            patch_control_id: None,
            value_before: None,
            value_after: None,
            selected_text_after: None,
            checkpoint: false,
            cleanup: false,
        }
    }

    fn adjustment(
        event: AppEvent,
        parameter: LiveEditableParameter,
        value_before: f32,
        value_after: f32,
        selected_text_after: String,
    ) -> Self {
        Self {
            event,
            expected_outcome: EventOutcome::Accepted,
            expected_rejection: None,
            editable_parameter: Some(parameter),
            patch_control_id: None,
            value_before: Some(value_before),
            value_after: Some(value_after),
            selected_text_after: Some(selected_text_after),
            checkpoint: true,
            cleanup: false,
        }
    }

    fn patch_adjustment(
        parameter: LiveEditableParameter,
        patch_control_id: PatchControlId,
        direction: Direction,
        value_before: f32,
        value_after: f32,
        selected_text_after: String,
    ) -> Self {
        Self {
            event: AppEvent::Adjust(direction),
            expected_outcome: EventOutcome::Accepted,
            expected_rejection: None,
            editable_parameter: Some(parameter),
            patch_control_id: Some(patch_control_id),
            value_before: Some(value_before),
            value_after: Some(value_after),
            selected_text_after: Some(selected_text_after),
            checkpoint: true,
            cleanup: false,
        }
    }

    fn rejected_patch_adjustment(
        parameter: LiveEditableParameter,
        patch_control_id: PatchControlId,
        direction: Direction,
        value: f32,
        selected_text_after: String,
        rejection: EventRejection,
    ) -> Self {
        Self {
            event: AppEvent::Adjust(direction),
            expected_outcome: EventOutcome::Rejected,
            expected_rejection: Some(rejection),
            editable_parameter: Some(parameter),
            patch_control_id: Some(patch_control_id),
            value_before: Some(value),
            value_after: Some(value),
            selected_text_after: Some(selected_text_after),
            checkpoint: false,
            cleanup: false,
        }
    }

    fn cleanup(event: AppEvent) -> Self {
        let mut step = Self::accepted_event(event);
        step.cleanup = true;
        step
    }

    pub fn event(&self) -> &AppEvent {
        &self.event
    }

    pub const fn expected_outcome(&self) -> EventOutcome {
        self.expected_outcome
    }

    pub const fn expected_rejection(&self) -> Option<EventRejection> {
        self.expected_rejection
    }

    pub const fn editable_parameter(&self) -> Option<&LiveEditableParameter> {
        self.editable_parameter.as_ref()
    }

    pub fn patch_control_id(&self) -> Option<PatchControlId> {
        self.patch_control_id.clone()
    }

    pub const fn value_before(&self) -> Option<f32> {
        self.value_before
    }

    pub const fn value_after(&self) -> Option<f32> {
        self.value_after
    }

    pub const fn requires_checkpoint(&self) -> bool {
        self.checkpoint
    }

    pub const fn is_cleanup(&self) -> bool {
        self.cleanup
    }
}

/// The frozen live engine-transition checkpoint-identity baseline: the exact
/// ordered identities the retained scenes have emitted since the engine
/// lifecycle was frozen. Across the webview shell cutover (spec C-004) this
/// set survives byte-identically and in order; any later-era identity must
/// be a pure insertion between its members.
pub const FROZEN_ENGINE_TRANSITION_IDENTITY_BASELINE: [&str; 3] = [
    "SoundFontPresetToNext",
    "SoundFontToBraids",
    "BraidsToDescriptorDefaultSoundFont",
];

/// True when `declared` preserves `baseline` byte-identically and in order:
/// every baseline identity appears, unaltered, in baseline order, and every
/// other declared identity is a pure insertion around them (spec C-004).
pub(crate) fn frozen_identity_baseline_is_preserved<'a>(
    declared: impl IntoIterator<Item = &'a str>,
    baseline: &[&str],
) -> bool {
    let mut awaited = baseline.iter();
    let mut next = awaited.next();
    for identity in declared {
        if next.is_some_and(|frozen| *frozen == identity) {
            next = awaited.next();
        }
    }
    next.is_none()
}

/// A bounded descriptor-derived scene for the installed production fixture.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveDemoScene {
    name: String,
    minimum_parameter_dwell: Duration,
    total_timeout: Duration,
    steps: Vec<LiveDemoStep>,
    scalar_step_count: usize,
    expected_editable_parameters: Vec<LiveEditableParameter>,
    expected_engine_transitions: Vec<LiveEngineTransition>,
    expected_topology_transitions:
        Vec<crate::testing::live_effects_and_buses_scene::LiveTopologyTransition>,
    patches: Vec<LivePatch>,
    /// True for the functional Patch editor scene: the runner samples the
    /// canonical projection into a `PatchEditorMeasurement` while this scene
    /// runs, and the host emits the resulting observation after teardown.
    measures_patch_editor: bool,
    /// Present only for the Phase 7 cumulative scene. The runner observes the
    /// exact Sample Patch/route without naming its capability implementation.
    detail_assets_subject: Option<(PatchId, MixerTrackId, String)>,
}

impl LiveDemoScene {
    pub const SCHEMA_VERSION: u32 = 6;
    pub const MINIMUM_PARAMETER_DWELL: Duration = Duration::from_millis(500);

    /// Freezes the installed patch and current typed descriptor surface.
    pub fn from_installed_state(tree: &StateTree) -> Result<Self, LiveDemoSceneError> {
        let state = decode_state_tree(tree)?;
        if state.patches.is_empty() {
            return Err(LiveDemoSceneError::NoInstalledPatches);
        }
        let source_capability = state.patches[0].instrument.capability_id().clone();
        let alternate_capability = state
            .capabilities
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.id() != &source_capability)
            .map(|descriptor| descriptor.id().clone())
            .ok_or(LiveDemoSceneError::EngineFixtureUnavailable)?;
        if state.capabilities.descriptor(&source_capability).is_none() {
            return Err(LiveDemoSceneError::EngineFixtureUnavailable);
        }
        let expected_effect = state
            .effects
            .descriptors()
            .first()
            .ok_or(LiveDemoSceneError::InvalidEffectConfig)?;
        if state.patches[0].post_effects.len() != 1
            || state.patches[0].post_effects[0].capability_id() != expected_effect.id()
            || state.patches[0].post_effects[0].slot_id()
                != crate::synth::effect_slot_id::EffectSlotIndex::ALL[0].instance_identity()
            || state
                .patches
                .iter()
                .skip(1)
                .any(|patch| !patch.post_effects.is_empty())
            || state.patches.iter().any(|patch| {
                state
                    .effects
                    .validate_patch_effects(&patch.post_effects)
                    .is_err()
            })
        {
            return Err(LiveDemoSceneError::InvalidEffectConfig);
        }
        if state.interaction.active_focus.context() != TopLevelContext::Mixer
            || state.interaction.active_focus.control_id()
                != &SemanticControlId::Mixer(MixerControlId::Track {
                    track_id: MixerTrackId::default(),
                    parameter: MixerTrackParameter::Level,
                })
        {
            return Err(LiveDemoSceneError::UnexpectedInitialSelection);
        }

        let mut patches = Vec::with_capacity(state.patches.len());
        let mut expected = Vec::new();
        for (patch_index, patch) in state.patches.iter().enumerate() {
            let patch_id =
                PatchId::new(patch.id).map_err(|_| LiveDemoSceneError::InvalidPatchId)?;
            if patches
                .iter()
                .any(|item: &LivePatch| item.patch_id == patch_id)
            {
                return Err(LiveDemoSceneError::DuplicatePatchId(patch.id));
            }
            let channel = MidiChannel::new(patch.channel)
                .map_err(|_| LiveDemoSceneError::InvalidMidiChannel(patch.channel))?;
            patches.push(LivePatch {
                patch_id,
                channel,
                output: patch.output,
            });
            let descriptor = state
                .capabilities
                .descriptor_for_config(&patch.instrument)
                .ok_or(LiveDemoSceneError::InvalidInstrumentConfig)?;
            let targets = resolve_patch_editable_targets(descriptor, &patch.instrument)
                .map_err(|_| LiveDemoSceneError::InvalidInstrumentConfig)?;
            if patch_index == 0 {
                expected.extend(
                    targets
                        .into_iter()
                        .map(|target| LiveEditableParameter::patch_target(patch_id, target)),
                );
                expected.extend(
                    PatchOutputParameter::ALL
                        .into_iter()
                        .map(|parameter| LiveEditableParameter::patch_output(patch_id, parameter)),
                );
            }
            for effect in &patch.post_effects {
                let effect_descriptor = state
                    .effects
                    .descriptor(effect.capability_id())
                    .ok_or(LiveDemoSceneError::InvalidEffectConfig)?;
                expected.extend(effect_descriptor.parameters().filter_map(|spec| {
                    let predicate_satisfied =
                        |predicate: Option<&crate::synth::ParameterPredicate>| {
                            predicate.is_none_or(|predicate| {
                                effect.value(predicate.parameter_id()) == Some(predicate.equals())
                            })
                        };
                    (spec.patch_interaction() == PatchInteraction::ScalarEdit
                        && predicate_satisfied(spec.visible_when())
                        && predicate_satisfied(spec.enabled_when()))
                    .then(|| {
                        LiveEditableParameter::effect(patch_id, effect.slot_id(), spec.id().clone())
                    })
                }));
            }
        }
        expected.extend(MixerTrackId::ALL.into_iter().flat_map(|track_id| {
            MixerTrackParameter::MAIN
                .into_iter()
                .map(move |parameter| LiveEditableParameter::track(track_id, parameter))
        }));
        // The frozen live scene exercises each track's two production-audible
        // indexed sends (the buses whose returns are occupied by default).
        expected.extend(MixerTrackId::ALL.into_iter().flat_map(|track_id| {
            [
                crate::mixer::bus_id::BusId::new(0).expect("bus 0 is always valid"),
                crate::mixer::bus_id::BusId::new(1).expect("bus 1 is always valid"),
            ]
            .into_iter()
            .map(move |bus| LiveEditableParameter::send(track_id, bus))
        }));
        expected.extend(
            GlobalParameters::surface_descriptor()
                .iter()
                .map(|descriptor| LiveEditableParameter::global(descriptor.parameter())),
        );

        let source_descriptor = state
            .capabilities
            .descriptor(&source_capability)
            .ok_or(LiveDemoSceneError::PresetFixtureUnavailable)?;
        let preset_spec = source_descriptor
            .parameters()
            .find(|parameter| {
                parameter.patch_interaction() == PatchInteraction::StructuralChoice
                    && parameter.choices().len() > 1
            })
            .ok_or(LiveDemoSceneError::PresetFixtureUnavailable)?;
        let preset_parameter = preset_spec.id().clone();
        let source_preset_id = match state.patches[0].instrument.value(&preset_parameter) {
            Some(ParameterValue::Choice(choice_id)) => choice_id,
            _ => return Err(LiveDemoSceneError::PresetFixtureUnavailable),
        };
        let source_preset_index = preset_spec
            .choices()
            .iter()
            .position(|choice| choice.id() == source_preset_id)
            .ok_or(LiveDemoSceneError::PresetFixtureUnavailable)?;
        let source_preset = preset_spec
            .choices()
            .get(source_preset_index)
            .ok_or(LiveDemoSceneError::PresetFixtureUnavailable)?;
        let target_preset = preset_spec
            .choices()
            .get(source_preset_index.saturating_add(1))
            .ok_or(LiveDemoSceneError::PresetFixtureUnavailable)?;

        let mut steps = Vec::new();
        build_focused_patch_envelope_steps(&state, patches[0], &mut steps)?;
        build_focused_patch_instrument_steps(&state, patches[0], &mut steps)?;
        build_focused_patch_effect_steps(&state, patches[0], &mut steps)?;
        build_patch_output_steps(&state, patches[0], &mut steps)?;
        let shared_route = patches[0]
            .output
            .track_id()
            .adjacent(true)
            .map_err(|_| LiveDemoSceneError::InvalidPlannedAdjustment)?;
        let shared_patch = patches
            .iter()
            .copied()
            .skip(1)
            .find(|patch| patch.output.track_id() == shared_route)
            .ok_or(LiveDemoSceneError::InvalidPlannedAdjustment)?;
        build_mixer_track_steps(&state, patches[0], shared_patch, &mut steps)?;
        build_global_steps(&state, patches[0], &mut steps)?;
        let scalar_step_count = steps.len();
        let first = patches[0];
        let expected_engine_transitions = vec![
            LiveEngineTransition::preset(
                "SoundFontPresetToNext",
                first.patch_id,
                first.channel,
                source_capability.clone(),
                preset_parameter,
                source_preset.id(),
                source_preset.label(),
                target_preset.id(),
                target_preset.label(),
            ),
            LiveEngineTransition::capability(
                "SoundFontToBraids",
                first.patch_id,
                first.channel,
                Direction::Right,
                source_capability.clone(),
                alternate_capability.clone(),
            ),
            LiveEngineTransition::capability(
                "BraidsToDescriptorDefaultSoundFont",
                first.patch_id,
                first.channel,
                Direction::Left,
                alternate_capability,
                source_capability,
            ),
        ];
        // C-004 (mission webview-shell-cutover): the frozen engine
        // checkpoint-identity baseline survives the shell cutover
        // byte-identically and in order; anything else declared here must be
        // a pure insertion. Asserted at declaration time so every
        // deterministic twin and live run fails at scene construction —
        // before rig time is spent — if the frozen identity set regresses.
        assert!(
            frozen_identity_baseline_is_preserved(
                expected_engine_transitions
                    .iter()
                    .map(LiveEngineTransition::identifier),
                &FROZEN_ENGINE_TRANSITION_IDENTITY_BASELINE,
            ),
            "the frozen live engine-transition identity baseline must be preserved byte-identically and in order",
        );
        steps.extend([
            LiveDemoStep::accepted_event(AppEvent::Midi {
                patch_id: first.patch_id,
                message: MidiMessage::try_new(first.channel, MidiMessageKind::NoteOn, 67, 112)
                    .expect("semantic live probe MIDI is valid"),
            }),
            LiveDemoStep::accepted_event(AppEvent::SelectContext(TopLevelContext::Patch)),
            LiveDemoStep::accepted_event(AppEvent::EnterSurface(SurfaceId::PatchUtility)),
            LiveDemoStep::accepted_event(AppEvent::Return),
            LiveDemoStep::accepted_event(AppEvent::SelectContext(TopLevelContext::Mixer)),
            LiveDemoStep::accepted_event(AppEvent::EnterSurface(SurfaceId::MixerInspector)),
            LiveDemoStep::accepted_event(AppEvent::Return),
            LiveDemoStep::accepted_event(AppEvent::SetInteractionMode(InteractionMode::Adjust)),
            LiveDemoStep::accepted_event(AppEvent::SetInteractionMode(InteractionMode::Navigate)),
            LiveDemoStep::accepted_event(AppEvent::Midi {
                patch_id: first.patch_id,
                message: MidiMessage::all_notes_off(first.channel),
            }),
        ]);
        for patch in &patches {
            steps.push(LiveDemoStep::cleanup(AppEvent::Midi {
                patch_id: patch.patch_id,
                message: MidiMessage::all_notes_off(patch.channel),
            }));
        }

        Ok(Self {
            name: "sixteen-track-mixer-routing-live-demo".to_owned(),
            minimum_parameter_dwell: Self::MINIMUM_PARAMETER_DWELL,
            total_timeout: crate::testing::live_demo_runner::LIVE_DEMO_TOTAL_TIMEOUT,
            steps,
            scalar_step_count,
            expected_editable_parameters: expected,
            expected_engine_transitions,
            expected_topology_transitions: Vec::new(),
            patches,
            measures_patch_editor: false,
            detail_assets_subject: None,
        })
    }

    /// The additive Mixer witness. It deliberately reuses the frozen
    /// production-path scene plan: that plan already traverses every one of
    /// the 16×4 main controls, opens each track's Inspector, edits two
    /// audible indexed sends per track, walks returns/globals, and carries
    /// real MIDI probes throughout. Only the scene identity differs, so the
    /// retained sixteen-track target and the `demo-live` alias do not move.
    pub fn mixer_from_installed_state(tree: &StateTree) -> Result<Self, LiveDemoSceneError> {
        let mut scene = Self::from_installed_state(tree)?;
        scene.name = "live-mixer".to_owned();
        Ok(scene)
    }

    /// Extends the cumulative scene with the controller-native Sample journey.
    ///
    /// The Sample Patch is discovered from its canonical asset kind, never a
    /// capability name. The added events stay on the normal reducer path and
    /// leave asynchronous preparation to the threaded production worker.
    pub fn detail_and_assets_from_installed_state(
        tree: &StateTree,
        defeat_preview: bool,
    ) -> Result<Self, LiveDemoSceneError> {
        let decoded = decode_state_tree(tree)?;
        let sample_index = decoded
            .patches
            .iter()
            .position(|patch| {
                patch
                    .instrument
                    .asset_references()
                    .iter()
                    .any(|assignment| {
                        assignment.reference().kind() == crate::synth::AssetKind::Sample
                    })
            })
            .ok_or(LiveDemoSceneError::SampleFixtureUnavailable)?;
        let initial_asset = decoded.patches[sample_index]
            .instrument
            .asset_references()
            .iter()
            .find(|assignment| assignment.reference().kind() == crate::synth::AssetKind::Sample)
            .map(|assignment| assignment.reference().locator().to_owned())
            .ok_or(LiveDemoSceneError::SampleFixtureUnavailable)?;
        let mut scene = Self::from_installed_state(tree)?;
        let probe = scene
            .patches
            .first()
            .copied()
            .ok_or(LiveDemoSceneError::NoInstalledPatches)?;
        let cleanup_at = scene
            .steps
            .iter()
            .position(LiveDemoStep::is_cleanup)
            .unwrap_or(scene.steps.len());
        let mut journey = vec![LiveDemoStep::accepted_event(AppEvent::SelectContext(
            TopLevelContext::Patch,
        ))];
        let detail_patch = decoded
            .patches
            .first()
            .ok_or(LiveDemoSceneError::NoInstalledPatches)?;
        let detail_descriptor = decoded
            .capabilities
            .descriptor(detail_patch.instrument.capability_id())
            .ok_or(LiveDemoSceneError::InvalidInstrumentConfig)?;
        let mut detail_slots = detail_patch
            .post_effects
            .iter()
            .cloned()
            .map(Some)
            .collect::<Vec<_>>();
        detail_slots.resize(crate::synth::effect_slot_id::MAX_EFFECT_SLOTS, None);
        let detail_parameters = detail_descriptor
            .parameters()
            .filter(|parameter| {
                parameter_conditions_are_satisfied(parameter, |id| {
                    detail_patch.instrument.value(id)
                })
            })
            .collect::<Vec<_>>();
        let descriptor_choice_index = detail_parameters
            .iter()
            .position(|parameter| {
                parameter.patch_interaction() == PatchInteraction::StructuralChoice
                    && !parameter.choices().is_empty()
            })
            .ok_or(LiveDemoSceneError::PresetFixtureUnavailable)?;
        let occupied_slot = detail_slots
            .iter()
            .position(Option::is_some)
            .and_then(|index| crate::synth::effect_slot_id::EffectSlotIndex::new(index).ok())
            .ok_or(LiveDemoSceneError::InvalidEffectConfig)?;

        // The shared Phase 7 surfaces are exercised before the Sample-specific
        // tail. Each generic choice source is opened twice: Return proves an
        // unchanged cancellation and Activate confirms the already-current
        // stable identity. Choosing the current entry deliberately avoids an
        // undeclared fourth structural lifecycle while still traversing the
        // production modal confirmation path.
        journey.push(LiveDemoStep::accepted_event(AppEvent::EnterSurface(
            SurfaceId::PatchDetail,
        )));
        journey.push(LiveDemoStep::accepted_event(AppEvent::Return));
        push_live_choice_cancel_and_current_commit(&mut journey);
        journey.push(LiveDemoStep::accepted_event(AppEvent::EnterSurface(
            SurfaceId::PatchDetail,
        )));
        journey.extend(
            (0..descriptor_choice_index)
                .map(|_| LiveDemoStep::accepted_event(AppEvent::Navigate(Direction::Down))),
        );
        push_live_choice_cancel_and_current_commit(&mut journey);
        journey.push(LiveDemoStep::accepted_event(AppEvent::Return));
        journey.extend(
            (0..=occupied_slot.index())
                .map(|_| LiveDemoStep::accepted_event(AppEvent::Navigate(Direction::Down))),
        );
        journey.push(LiveDemoStep::accepted_event(AppEvent::EnterSurface(
            SurfaceId::PatchDetail,
        )));
        journey.push(LiveDemoStep::accepted_event(AppEvent::Return));
        push_live_choice_cancel_and_current_commit(&mut journey);
        journey.push(LiveDemoStep::accepted_event(AppEvent::EnterSurface(
            SurfaceId::PatchUtility,
        )));
        journey.extend(
            (0..2).map(|_| LiveDemoStep::accepted_event(AppEvent::Navigate(Direction::Down))),
        );
        push_live_choice_cancel_and_current_commit(&mut journey);
        journey.push(LiveDemoStep::accepted_event(AppEvent::Return));
        journey.extend(
            (0..=occupied_slot.index())
                .map(|_| LiveDemoStep::accepted_event(AppEvent::Navigate(Direction::Up))),
        );

        let sample_patch = decoded
            .patches
            .get(sample_index)
            .ok_or(LiveDemoSceneError::SampleFixtureUnavailable)?;
        let sample_descriptor = decoded
            .capabilities
            .descriptor(sample_patch.instrument.capability_id())
            .ok_or(LiveDemoSceneError::InvalidInstrumentConfig)?;
        let sample_detail_parameters = sample_descriptor
            .parameters()
            .filter(|parameter| {
                parameter_conditions_are_satisfied(parameter, |id| {
                    sample_patch.instrument.value(id)
                })
            })
            .collect::<Vec<_>>();
        let sample_asset_index = sample_detail_parameters
            .iter()
            .position(|parameter| {
                parameter.kind() == ParameterKind::Asset
                    && sample_patch
                        .instrument
                        .asset_references()
                        .iter()
                        .any(|assignment| {
                            assignment.parameter_id() == parameter.id()
                                && assignment.reference().kind() == crate::synth::AssetKind::Sample
                        })
            })
            .ok_or(LiveDemoSceneError::SampleFixtureUnavailable)?;
        let sample_scalar_index = sample_detail_parameters
            .iter()
            .enumerate()
            .skip(sample_asset_index + 1)
            .find(|(_, parameter)| parameter.patch_interaction() == PatchInteraction::ScalarEdit)
            .map(|(index, _)| index)
            .ok_or(LiveDemoSceneError::InvalidPlannedAdjustment)?;

        journey.extend(
            (0..sample_index)
                .map(|_| LiveDemoStep::accepted_event(AppEvent::SelectPatch(Direction::Right))),
        );
        journey.extend([LiveDemoStep::accepted_event(AppEvent::EnterSurface(
            SurfaceId::PatchDetail,
        ))]);
        journey.extend(
            (0..sample_asset_index)
                .map(|_| LiveDemoStep::accepted_event(AppEvent::Navigate(Direction::Down))),
        );
        journey.extend([
            LiveDemoStep::accepted_event(AppEvent::OpenRelated),
            // Root listing: A active fixture, B alternate fixture, Z invalid
            // fixture, then CANCEL. Move to the alternate without indices in
            // domain state; these are bounded semantic Down actions.
            LiveDemoStep::accepted_event(AppEvent::Navigate(Direction::Down)),
        ]);
        if !defeat_preview {
            journey.push(LiveDemoStep::accepted_event(AppEvent::PreviewStart));
        }
        push_phase7_worker_dwell(&mut journey, scene.patches[sample_index], 12);
        if !defeat_preview {
            journey.push(LiveDemoStep::accepted_event(AppEvent::PreviewStop));
        }
        journey.extend([
            LiveDemoStep::accepted_event(AppEvent::Return),
            // Re-enter and select the production-adapter invalid WAV.
            LiveDemoStep::accepted_event(AppEvent::OpenRelated),
            LiveDemoStep::accepted_event(AppEvent::Navigate(Direction::Down)),
            LiveDemoStep::accepted_event(AppEvent::Navigate(Direction::Down)),
            LiveDemoStep::accepted_event(AppEvent::Activate),
        ]);
        push_phase7_worker_dwell(&mut journey, probe, 12);
        journey.extend([
            // Recover by assigning the valid alternate asset.
            LiveDemoStep::accepted_event(AppEvent::OpenRelated),
            LiveDemoStep::accepted_event(AppEvent::Navigate(Direction::Down)),
            LiveDemoStep::accepted_event(AppEvent::Activate),
        ]);
        // Once the compatible B graph is published, keep driving semantic
        // note pairs on the Sample Patch itself. Early pairs may land while
        // preparation advances; later pairs audibly prove the committed
        // asset on its Patch-local route.
        push_phase7_worker_dwell(&mut journey, scene.patches[sample_index], 16);
        journey.extend(
            (sample_asset_index..sample_scalar_index)
                .map(|_| LiveDemoStep::accepted_event(AppEvent::Navigate(Direction::Down))),
        );
        journey.extend([
            // Exercise one live Sample detail scalar through the same mode and
            // reducer path as every other descriptor-owned control.
            LiveDemoStep::accepted_event(AppEvent::SetInteractionMode(InteractionMode::Adjust)),
            LiveDemoStep::accepted_event(AppEvent::Adjust(Direction::Right)),
            LiveDemoStep::accepted_event(AppEvent::SetInteractionMode(InteractionMode::Navigate)),
            // Finish on the remembered Mixer root so the final cumulative
            // report can re-verify all sixteen stable track/control identities
            // after the Patch-detail tail, not merely rely on an earlier frame.
            LiveDemoStep::accepted_event(AppEvent::SelectContext(TopLevelContext::Mixer)),
        ]);
        scene.steps.splice(cleanup_at..cleanup_at, journey);
        scene.name = "detail-and-assets-live-demo".to_owned();
        scene.total_timeout = Duration::from_secs(180);
        scene.detail_assets_subject = Some((
            scene.patches[sample_index].patch_id,
            scene.patches[sample_index].output.track_id(),
            initial_asset,
        ));
        Ok(scene)
    }

    /// Extends one frozen base scene into the retained cumulative
    /// effects-and-buses scene by appending the ordered topology transitions
    /// the runner drives after the engine phase. The scalar steps, engine
    /// transitions, and teardown contract are inherited unchanged.
    pub(crate) fn with_topology_extension(
        mut self,
        name: impl Into<String>,
        transitions: Vec<crate::testing::live_effects_and_buses_scene::LiveTopologyTransition>,
        total_timeout: Duration,
    ) -> Self {
        self.name = name.into();
        self.expected_topology_transitions = transitions;
        self.total_timeout = total_timeout;
        self
    }

    /// Extends one frozen base scene into the functional Patch editor scene.
    /// The scalar steps, the three engine transitions, and the teardown
    /// contract are inherited unchanged (mission finding F-49): a new live
    /// scene either performs the base scene's engine transitions or both of
    /// the runner's undeclared couplings must be relaxed, and inheriting is
    /// the honest half of that choice.
    pub(crate) fn with_patch_editor_extension(
        mut self,
        name: impl Into<String>,
        transitions: Vec<crate::testing::live_effects_and_buses_scene::LiveTopologyTransition>,
        total_timeout: Duration,
    ) -> Self {
        self.name = name.into();
        self.expected_topology_transitions = transitions;
        self.total_timeout = total_timeout;
        self.measures_patch_editor = true;
        self
    }

    /// Whether the runner measures the functional Patch editor observation
    /// while this scene runs.
    pub const fn measures_patch_editor(&self) -> bool {
        self.measures_patch_editor
    }

    pub fn detail_assets_subject(&self) -> Option<(PatchId, MixerTrackId, &str)> {
        self.detail_assets_subject
            .as_ref()
            .map(|(patch, track, asset)| (*patch, *track, asset.as_str()))
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn schema_version(&self) -> u32 {
        Self::SCHEMA_VERSION
    }

    pub const fn minimum_parameter_dwell(&self) -> Duration {
        self.minimum_parameter_dwell
    }

    pub fn steps(&self) -> &[LiveDemoStep] {
        &self.steps
    }

    pub fn expected_editable_parameters(&self) -> &[LiveEditableParameter] {
        &self.expected_editable_parameters
    }

    pub fn expected_engine_transitions(&self) -> &[LiveEngineTransition] {
        &self.expected_engine_transitions
    }

    pub fn expected_topology_transitions(
        &self,
    ) -> &[crate::testing::live_effects_and_buses_scene::LiveTopologyTransition] {
        &self.expected_topology_transitions
    }

    /// Maximum elapsed control time for this scene; the cumulative retained
    /// scene declares a wider bound than the base scene.
    pub const fn total_timeout(&self) -> Duration {
        self.total_timeout
    }

    pub(crate) const fn scalar_step_count(&self) -> usize {
        self.scalar_step_count
    }

    pub fn patch_ids(&self) -> impl ExactSizeIterator<Item = PatchId> + '_ {
        self.patches.iter().map(|patch| patch.patch_id)
    }

    pub fn required_event_log_capacity(&self, fixture_allowance: usize) -> usize {
        1usize
            .saturating_add(self.steps.len())
            .saturating_add(self.expected_engine_transitions.len().saturating_mul(6))
            .saturating_add(
                self.expected_topology_transitions
                    .iter()
                    .map(crate::testing::live_effects_and_buses_scene::LiveTopologyTransition::event_budget)
                    .sum::<usize>(),
            )
            .saturating_add(2)
            .saturating_add(fixture_allowance)
    }
}

/// One frozen ordered live engine replacement for the focused fixture Patch.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveEngineTransition {
    id: String,
    patch_id: PatchId,
    channel: MidiChannel,
    direction: EventDirection,
    source_capability_id: CapabilityId,
    target_capability_id: CapabilityId,
    intent: StructuralEditIntent,
    source_choice_id: Option<String>,
    source_label: Option<String>,
    target_choice_id: Option<String>,
    target_label: Option<String>,
}

impl LiveEngineTransition {
    fn capability(
        id: impl Into<String>,
        patch_id: PatchId,
        channel: MidiChannel,
        direction: Direction,
        source_capability_id: CapabilityId,
        target_capability_id: CapabilityId,
    ) -> Self {
        let intent = StructuralEditIntent::ReplaceCapability {
            target_capability_id: target_capability_id.clone(),
        };
        Self {
            id: id.into(),
            patch_id,
            channel,
            direction: direction.into(),
            source_capability_id,
            target_capability_id,
            intent,
            source_choice_id: None,
            source_label: None,
            target_choice_id: None,
            target_label: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn preset(
        id: impl Into<String>,
        patch_id: PatchId,
        channel: MidiChannel,
        capability_id: CapabilityId,
        parameter_id: ParameterId,
        source_choice_id: impl Into<String>,
        source_label: impl Into<String>,
        target_choice_id: impl Into<String>,
        target_label: impl Into<String>,
    ) -> Self {
        let target_choice_id = target_choice_id.into();
        let intent = StructuralEditIntent::ReplaceParameterChoice {
            capability_id: capability_id.clone(),
            parameter_id,
            choice_id: target_choice_id.clone(),
        };
        Self {
            id: id.into(),
            patch_id,
            channel,
            direction: Direction::Right.into(),
            source_capability_id: capability_id.clone(),
            target_capability_id: capability_id,
            intent,
            source_choice_id: Some(source_choice_id.into()),
            source_label: Some(source_label.into()),
            target_choice_id: Some(target_choice_id),
            target_label: Some(target_label.into()),
        }
    }

    pub fn identifier(&self) -> &str {
        &self.id
    }

    pub const fn patch_id(&self) -> PatchId {
        self.patch_id
    }

    pub const fn channel(&self) -> MidiChannel {
        self.channel
    }

    pub const fn direction(&self) -> Direction {
        match self.direction {
            EventDirection::Up => Direction::Up,
            EventDirection::Down => Direction::Down,
            EventDirection::Left => Direction::Left,
            EventDirection::Right => Direction::Right,
        }
    }

    pub const fn source_capability_id(&self) -> &CapabilityId {
        &self.source_capability_id
    }

    pub const fn target_capability_id(&self) -> &CapabilityId {
        &self.target_capability_id
    }

    pub const fn intent(&self) -> &StructuralEditIntent {
        &self.intent
    }

    pub fn focused_control_id(&self) -> PatchControlId {
        match &self.intent {
            StructuralEditIntent::ReplaceCapability { .. } => PatchControlId::Engine,
            StructuralEditIntent::ReplaceParameterChoice { parameter_id, .. } => {
                PatchControlId::Capability(parameter_id.clone())
            }
            StructuralEditIntent::ReplaceAsset { parameter_id, .. }
            | StructuralEditIntent::PrepareAudition { parameter_id, .. } => {
                PatchControlId::Capability(parameter_id.clone())
            }
            StructuralEditIntent::SetVoiceBudget { .. }
            | StructuralEditIntent::ReplaceEffectAsset { .. }
            | StructuralEditIntent::SetSlotOccupancy { .. }
            | StructuralEditIntent::SetSendEffect { .. }
            | StructuralEditIntent::SetReturnOccupancy { .. }
            | StructuralEditIntent::AppendPatch { .. } => {
                unreachable!("live demo engine transitions carry instrument intents")
            }
        }
    }

    pub const fn is_preset(&self) -> bool {
        matches!(
            self.intent,
            StructuralEditIntent::ReplaceParameterChoice { .. }
        )
    }

    pub fn source_choice_id(&self) -> Option<&str> {
        self.source_choice_id.as_deref()
    }

    pub fn source_label(&self) -> Option<&str> {
        self.source_label.as_deref()
    }

    pub fn target_choice_id(&self) -> Option<&str> {
        self.target_choice_id.as_deref()
    }

    pub fn target_label(&self) -> Option<&str> {
        self.target_label.as_deref()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct LivePatch {
    patch_id: PatchId,
    channel: MidiChannel,
    output: PatchOutput,
}

fn build_patch_output_steps(
    state: &DecodedStateTree,
    patch: LivePatch,
    steps: &mut Vec<LiveDemoStep>,
) -> Result<(), LiveDemoSceneError> {
    let decoded = state
        .patches
        .iter()
        .find(|candidate| candidate.id == patch.patch_id.value())
        .ok_or(LiveDemoSceneError::InvalidInstrumentConfig)?;
    steps.push(LiveDemoStep::accepted_event(AppEvent::SelectContext(
        TopLevelContext::Patch,
    )));
    steps.push(LiveDemoStep::accepted_event(AppEvent::EnterSurface(
        SurfaceId::PatchUtility,
    )));

    let trim = PatchOutputParameter::TrimGain;
    let descriptor = trim.descriptor();
    let before = decoded.output.trim_gain_db();
    let direction = if before < descriptor.maximum().unwrap_or(before) {
        Direction::Right
    } else {
        Direction::Left
    };
    let after = adjusted_value(
        before,
        descriptor.minimum().unwrap_or(before),
        descriptor.maximum().unwrap_or(before),
        direction,
        descriptor.fine_step().unwrap_or(1.0),
        descriptor.coarse_step().unwrap_or(1.0),
    )?;
    push_probed_checkpoint(
        steps,
        patch,
        LiveDemoStep::patch_adjustment(
            LiveEditableParameter::patch_output(patch.patch_id, trim),
            PatchControlId::Output(trim),
            direction,
            before,
            after,
            PatchPageOutputRow::selected_text(
                trim,
                decoded
                    .output
                    .with_trim_gain_db(after)
                    .map_err(|_| LiveDemoSceneError::InvalidPlannedAdjustment)?,
            )
            .map_err(|_| LiveDemoSceneError::InvalidInstrumentConfig)?,
        ),
    );

    // Step down from the trim row to the output-track row by the distance the
    // declared five-row Utility order puts between them — MIDI input sits
    // between the two — rather than assuming they are adjacent.
    for _ in 0..utility_rows_between(
        &PatchControlId::Output(PatchOutputParameter::TrimGain),
        &PatchControlId::Output(PatchOutputParameter::OutputTrack),
    ) {
        steps.push(LiveDemoStep::accepted_event(AppEvent::Navigate(
            Direction::Down,
        )));
    }
    let route = decoded.output.track_id();
    let (direction, next) = if let Ok(next) = route.adjacent(true) {
        (Direction::Right, next)
    } else {
        (
            Direction::Left,
            route
                .adjacent(false)
                .map_err(|_| LiveDemoSceneError::InvalidPlannedAdjustment)?,
        )
    };
    push_probed_checkpoint(
        steps,
        patch,
        LiveDemoStep::patch_adjustment(
            LiveEditableParameter::patch_output(patch.patch_id, PatchOutputParameter::OutputTrack),
            PatchControlId::Output(PatchOutputParameter::OutputTrack),
            direction,
            route.index() as f32,
            next.index() as f32,
            PatchPageOutputRow::selected_text(
                PatchOutputParameter::OutputTrack,
                decoded.output.with_track_id(next),
            )
            .map_err(|_| LiveDemoSceneError::InvalidInstrumentConfig)?,
        ),
    );
    if route != MixerTrackId::default() {
        return Err(LiveDemoSceneError::InvalidPlannedAdjustment);
    }
    steps.push(LiveDemoStep::rejected_patch_adjustment(
        LiveEditableParameter::patch_output(patch.patch_id, PatchOutputParameter::OutputTrack),
        PatchControlId::Output(PatchOutputParameter::OutputTrack),
        Direction::Left,
        route.index() as f32,
        PatchPageOutputRow::selected_text(PatchOutputParameter::OutputTrack, decoded.output)
            .map_err(|_| LiveDemoSceneError::InvalidInstrumentConfig)?,
        EventRejection::ParameterAtBoundary,
    ));
    steps.push(LiveDemoStep::accepted_event(AppEvent::Return));
    Ok(())
}

fn build_mixer_track_steps(
    state: &DecodedStateTree,
    probe_patch: LivePatch,
    shared_patch: LivePatch,
    steps: &mut Vec<LiveDemoStep>,
) -> Result<(), LiveDemoSceneError> {
    steps.push(LiveDemoStep::accepted_event(AppEvent::SelectContext(
        TopLevelContext::Mixer,
    )));
    for track_id in MixerTrackId::ALL {
        let values = state.mixer.track(track_id).clone();
        for parameter in MixerTrackParameter::MAIN {
            let descriptor = parameter.descriptor();
            let (before, after, direction) = match descriptor.kind() {
                MixerTrackParameterKind::Continuous => {
                    let before = values
                        .scalar_value(parameter)
                        .ok_or(LiveDemoSceneError::InvalidPlannedAdjustment)?;
                    let direction = if before < descriptor.maximum() {
                        Direction::Right
                    } else {
                        Direction::Left
                    };
                    let after = adjusted_value(
                        before,
                        descriptor.minimum(),
                        descriptor.maximum(),
                        direction,
                        descriptor.fine_step(),
                        descriptor.coarse_step(),
                    )?;
                    (before, after, direction)
                }
                MixerTrackParameterKind::Toggle => {
                    let before = values
                        .toggle_value(parameter)
                        .ok_or(LiveDemoSceneError::InvalidPlannedAdjustment)?;
                    (
                        if before { 1.0 } else { 0.0 },
                        if before { 0.0 } else { 1.0 },
                        Direction::Right,
                    )
                }
            };
            let checkpoint = LiveDemoStep::adjustment(
                AppEvent::Adjust(direction),
                LiveEditableParameter::track(track_id, parameter),
                before,
                after,
                track_selected_text(parameter, after),
            );
            if parameter == MixerTrackParameter::Level && track_id == shared_patch.output.track_id()
            {
                push_shared_probed_checkpoint(steps, probe_patch, shared_patch, checkpoint);
            } else {
                push_probed_checkpoint(steps, probe_patch, checkpoint);
            }
            if parameter != MixerTrackParameter::Solo {
                steps.push(LiveDemoStep::accepted_event(AppEvent::Navigate(
                    Direction::Down,
                )));
            }
        }

        // The Inspector's first rows are the selected track's indexed sends;
        // the frozen scene exercises the two production-audible ones (the
        // buses whose returns the composition occupies by default).
        steps.push(LiveDemoStep::accepted_event(AppEvent::EnterSurface(
            SurfaceId::MixerInspector,
        )));
        let audible_sends = [
            crate::mixer::bus_id::BusId::new(0).expect("bus 0 is always valid"),
            crate::mixer::bus_id::BusId::new(1).expect("bus 1 is always valid"),
        ];
        for (send_index, bus) in audible_sends.into_iter().enumerate() {
            let descriptor = crate::mixer::mixer_track_parameters::BUS_SEND_DESCRIPTOR;
            let before = values.send(bus);
            let direction = if before < descriptor.maximum() {
                Direction::Right
            } else {
                Direction::Left
            };
            let after = adjusted_value(
                before,
                descriptor.minimum(),
                descriptor.maximum(),
                direction,
                descriptor.fine_step(),
                descriptor.coarse_step(),
            )?;
            push_probed_checkpoint(
                steps,
                probe_patch,
                LiveDemoStep::adjustment(
                    AppEvent::Adjust(direction),
                    LiveEditableParameter::send(track_id, bus),
                    before,
                    after,
                    scalar_selected_text(&format!("send[{}]", bus.index()), after),
                ),
            );
            if send_index + 1 < audible_sends.len() {
                steps.push(LiveDemoStep::accepted_event(AppEvent::Navigate(
                    Direction::Down,
                )));
            }
        }
        steps.push(LiveDemoStep::accepted_event(AppEvent::Return));

        if track_id != MixerTrackId::ALL[MixerTrackId::COUNT - 1] {
            for _ in 1..MixerTrackParameter::MAIN.len() {
                steps.push(LiveDemoStep::accepted_event(AppEvent::Navigate(
                    Direction::Up,
                )));
            }
            steps.push(LiveDemoStep::accepted_event(AppEvent::Navigate(
                Direction::Right,
            )));
            move_probe_patch_to_next_track(steps);
        }
    }

    restore_probe_patch_to_first_track(steps);
    steps.push(LiveDemoStep::accepted_event(AppEvent::EnterSurface(
        SurfaceId::MixerInspector,
    )));
    // Walk from the first send row past every return row (occupancy, level,
    // and the occupant's visible ScalarEdit rows) to the one distinct global
    // row, master gain.
    let mut return_rows = 0_usize;
    for entry in &state.returns {
        return_rows += 2;
        if let Some(config) = entry.effect.as_ref() {
            let descriptor = state
                .effects
                .descriptor(config.capability_id())
                .ok_or(LiveDemoSceneError::InvalidEffectConfig)?;
            let predicate_satisfied = |predicate: Option<&crate::synth::ParameterPredicate>| {
                predicate.is_none_or(|predicate| {
                    config.value(predicate.parameter_id()) == Some(predicate.equals())
                })
            };
            return_rows += descriptor
                .parameters()
                .filter(|spec| {
                    spec.patch_interaction() == PatchInteraction::ScalarEdit
                        && predicate_satisfied(spec.visible_when())
                        && predicate_satisfied(spec.enabled_when())
                })
                .count();
        }
    }
    for _ in 0..(crate::mixer::bus_id::BusId::COUNT + return_rows) {
        steps.push(LiveDemoStep::accepted_event(AppEvent::Navigate(
            Direction::Down,
        )));
    }
    Ok(())
}

fn build_focused_patch_envelope_steps(
    state: &DecodedStateTree,
    patch: LivePatch,
    steps: &mut Vec<LiveDemoStep>,
) -> Result<(), LiveDemoSceneError> {
    let decoded = state
        .patches
        .iter()
        .find(|candidate| candidate.id == patch.patch_id.value())
        .ok_or(LiveDemoSceneError::InvalidInstrumentConfig)?;
    let instrument_descriptor = state
        .capabilities
        .descriptor(decoded.instrument.capability_id())
        .ok_or(LiveDemoSceneError::InvalidInstrumentConfig)?;
    let instrument_row_count = instrument_descriptor
        .parameters()
        .filter(|parameter| {
            parameter_conditions_are_satisfied(parameter, |id| decoded.instrument.value(id))
        })
        .count();

    steps.push(LiveDemoStep::accepted_event(AppEvent::SelectContext(
        TopLevelContext::Patch,
    )));
    steps.push(LiveDemoStep::accepted_event(AppEvent::EnterSurface(
        SurfaceId::PatchDetail,
    )));
    steps.extend(
        (0..instrument_row_count)
            .map(|_| LiveDemoStep::accepted_event(AppEvent::Navigate(Direction::Down))),
    );
    for (index, descriptor) in VoiceEnvelope::surface_descriptor().iter().enumerate() {
        let parameter = descriptor.parameter();
        let control = PatchControlId::Envelope(parameter);
        let before = decoded.envelope.value(parameter);
        let direction = if before < descriptor.maximum() {
            Direction::Right
        } else {
            Direction::Left
        };
        let after = adjusted_value(
            before,
            descriptor.minimum(),
            descriptor.maximum(),
            direction,
            descriptor.fine_step(),
            descriptor.coarse_step(),
        )?;
        let selected_text = PatchPageEnvelopeRow::selected_text(parameter, after)
            .map_err(|_| LiveDemoSceneError::InvalidInstrumentConfig)?;
        if index > 0 {
            steps.push(LiveDemoStep::accepted_event(AppEvent::Navigate(
                Direction::Down,
            )));
        }
        push_probed_checkpoint(
            steps,
            patch,
            LiveDemoStep::patch_adjustment(
                LiveEditableParameter::patch_target(
                    patch.patch_id,
                    PatchEditableTarget::Envelope(parameter),
                ),
                control,
                direction,
                before,
                after,
                selected_text,
            ),
        );
    }
    steps.push(LiveDemoStep::accepted_event(AppEvent::Return));
    Ok(())
}

fn build_focused_patch_instrument_steps(
    state: &DecodedStateTree,
    patch: LivePatch,
    steps: &mut Vec<LiveDemoStep>,
) -> Result<(), LiveDemoSceneError> {
    let decoded = state
        .patches
        .iter()
        .find(|candidate| candidate.id == patch.patch_id.value())
        .ok_or(LiveDemoSceneError::InvalidInstrumentConfig)?;
    let descriptor = state
        .capabilities
        .descriptor(decoded.instrument.capability_id())
        .ok_or(LiveDemoSceneError::InvalidInstrumentConfig)?;
    let config = decoded.instrument.clone();
    let detail_parameters = descriptor
        .parameters()
        .filter(|parameter| parameter_conditions_are_satisfied(parameter, |id| config.value(id)))
        .collect::<Vec<_>>();
    steps.push(LiveDemoStep::accepted_event(AppEvent::SelectContext(
        TopLevelContext::Patch,
    )));
    steps.push(LiveDemoStep::accepted_event(AppEvent::EnterSurface(
        SurfaceId::PatchDetail,
    )));
    for (index, spec) in detail_parameters.into_iter().enumerate() {
        if index > 0 {
            steps.push(LiveDemoStep::accepted_event(AppEvent::Navigate(
                Direction::Down,
            )));
        }
        if spec.patch_interaction() != PatchInteraction::ScalarEdit {
            continue;
        }
        let parameter_id = spec.id().clone();
        let before_value = config
            .value(&parameter_id)
            .ok_or(LiveDemoSceneError::InvalidInstrumentConfig)?;
        let before = spec
            .scalar_value(before_value)
            .map_err(|_| LiveDemoSceneError::InvalidInstrumentConfig)?;
        let range = spec
            .range()
            .ok_or(LiveDemoSceneError::InvalidInstrumentConfig)?;
        let direction = if f64::from(before) < range.maximum() {
            Direction::Right
        } else {
            Direction::Left
        };
        let next = spec
            .adjusted_scalar_value(before_value, parameter_adjustment(direction))
            .map_err(|_| LiveDemoSceneError::InvalidPlannedAdjustment)?;
        let after = spec
            .scalar_value(&next)
            .map_err(|_| LiveDemoSceneError::InvalidInstrumentConfig)?;
        let updated = config
            .with_scalar_value(descriptor, &parameter_id, next)
            .map_err(|_| LiveDemoSceneError::InvalidInstrumentConfig)?;
        let selected_text = PatchPageParameterRow::selected_instrument_detail_text(
            descriptor,
            &updated,
            &parameter_id,
        )
        .map_err(|_| LiveDemoSceneError::InvalidInstrumentConfig)?;
        push_probed_checkpoint(
            steps,
            patch,
            LiveDemoStep::patch_adjustment(
                LiveEditableParameter::patch_target(
                    patch.patch_id,
                    PatchEditableTarget::Instrument(parameter_id.clone()),
                ),
                PatchControlId::Capability(parameter_id),
                direction,
                before,
                after,
                selected_text,
            ),
        );
    }
    steps.push(LiveDemoStep::accepted_event(AppEvent::Return));
    Ok(())
}

fn build_focused_patch_effect_steps(
    state: &DecodedStateTree,
    patch: LivePatch,
    steps: &mut Vec<LiveDemoStep>,
) -> Result<(), LiveDemoSceneError> {
    let decoded = state
        .patches
        .iter()
        .find(|candidate| candidate.id == patch.patch_id.value())
        .ok_or(LiveDemoSceneError::InvalidEffectConfig)?;
    if decoded.post_effects.is_empty() {
        steps.push(LiveDemoStep::accepted_event(AppEvent::SelectContext(
            TopLevelContext::Mixer,
        )));
        return Ok(());
    }
    // The serialized compact effect list occupies positions 0..len in order,
    // exactly as fixture installation does; pad the remaining positions.
    let mut effect_slots = decoded
        .post_effects
        .iter()
        .cloned()
        .map(Some)
        .collect::<Vec<_>>();
    effect_slots.resize(crate::synth::effect_slot_id::MAX_EFFECT_SLOTS, None);
    steps.push(LiveDemoStep::accepted_event(AppEvent::SelectContext(
        TopLevelContext::Patch,
    )));
    for occupancy in &effect_slots {
        steps.push(LiveDemoStep::accepted_event(AppEvent::Navigate(
            Direction::Down,
        )));
        let Some(config) = occupancy else {
            continue;
        };
        let descriptor = state
            .effects
            .descriptor(config.capability_id())
            .ok_or(LiveDemoSceneError::InvalidEffectConfig)?;
        let detail_parameters = descriptor
            .parameters()
            .filter(|parameter| {
                parameter_conditions_are_satisfied(parameter, |id| config.value(id))
            })
            .collect::<Vec<_>>();
        if detail_parameters.is_empty() {
            continue;
        }
        steps.push(LiveDemoStep::accepted_event(AppEvent::EnterSurface(
            SurfaceId::PatchDetail,
        )));
        for (index, spec) in detail_parameters.into_iter().enumerate() {
            if index > 0 {
                steps.push(LiveDemoStep::accepted_event(AppEvent::Navigate(
                    Direction::Down,
                )));
            }
            if spec.patch_interaction() != PatchInteraction::ScalarEdit {
                continue;
            }
            let parameter_id = spec.id().clone();
            let control = PatchControlId::Effect(config.slot_id(), parameter_id.clone());
            let before_value = config
                .value(&parameter_id)
                .ok_or(LiveDemoSceneError::InvalidEffectConfig)?;
            let before = spec
                .scalar_value(before_value)
                .map_err(|_| LiveDemoSceneError::InvalidEffectConfig)?;
            let range = spec
                .range()
                .ok_or(LiveDemoSceneError::InvalidEffectConfig)?;
            let direction = if f64::from(before) < range.maximum() {
                Direction::Right
            } else {
                Direction::Left
            };
            let next = spec
                .adjusted_scalar_value(before_value, parameter_adjustment(direction))
                .map_err(|_| LiveDemoSceneError::InvalidPlannedAdjustment)?;
            let after = spec
                .scalar_value(&next)
                .map_err(|_| LiveDemoSceneError::InvalidEffectConfig)?;
            if before == after {
                return Err(LiveDemoSceneError::InvalidPlannedAdjustment);
            }
            let updated = config
                .with_scalar_value(descriptor, &parameter_id, next)
                .map_err(|_| LiveDemoSceneError::InvalidEffectConfig)?;
            let selected_text = PatchPageParameterRow::selected_effect_detail_text(
                descriptor,
                &updated,
                &parameter_id,
            )
            .map_err(|_| LiveDemoSceneError::InvalidEffectConfig)?;
            push_probed_checkpoint(
                steps,
                patch,
                LiveDemoStep::patch_adjustment(
                    LiveEditableParameter::effect(patch.patch_id, config.slot_id(), parameter_id),
                    control,
                    direction,
                    before,
                    after,
                    selected_text,
                ),
            );
        }
        steps.push(LiveDemoStep::accepted_event(AppEvent::Return));
    }
    steps.extend(
        (0..effect_slots.len())
            .map(|_| LiveDemoStep::accepted_event(AppEvent::Navigate(Direction::Up))),
    );
    steps.push(LiveDemoStep::accepted_event(AppEvent::SelectContext(
        TopLevelContext::Mixer,
    )));
    Ok(())
}

fn build_global_steps(
    state: &DecodedStateTree,
    probe_patch: LivePatch,
    steps: &mut Vec<LiveDemoStep>,
) -> Result<(), LiveDemoSceneError> {
    for (index, descriptor) in GlobalParameters::surface_descriptor().iter().enumerate() {
        let editable = LiveEditableParameter::global(descriptor.parameter());
        let value = state
            .global_row_value(descriptor.parameter())
            .ok_or(LiveDemoSceneError::SelectedParameterMismatch)?;
        let direction = if value < descriptor.maximum() {
            Direction::Right
        } else {
            Direction::Left
        };
        let next = adjusted_value(
            value,
            descriptor.minimum(),
            descriptor.maximum(),
            direction,
            descriptor.fine_step(),
            descriptor.coarse_step(),
        )?;
        push_probed_checkpoint(
            steps,
            probe_patch,
            LiveDemoStep::adjustment(
                AppEvent::Adjust(direction),
                editable,
                value,
                next,
                scalar_selected_text(descriptor.name(), next),
            ),
        );
        if index + 1 < GlobalParameters::surface_descriptor().len() {
            steps.push(LiveDemoStep::accepted_event(AppEvent::Navigate(
                Direction::Down,
            )));
        }
    }
    Ok(())
}

fn push_probed_checkpoint(
    steps: &mut Vec<LiveDemoStep>,
    patch: LivePatch,
    checkpoint: LiveDemoStep,
) {
    debug_assert!(checkpoint.requires_checkpoint());
    let reverse = match checkpoint.event() {
        AppEvent::Adjust(direction) => Some(opposite_direction(*direction)),
        _ => None,
    };
    steps.push(parameter_probe_step(
        patch,
        MidiMessageKind::NoteOn,
        PARAMETER_PROBE_VELOCITY,
    ));
    steps.push(checkpoint);
    steps.push(parameter_probe_step(patch, MidiMessageKind::NoteOff, 0));
    if let Some(direction) = reverse {
        steps.push(LiveDemoStep::accepted_event(AppEvent::Adjust(direction)));
    }
}

/// Gives the threaded structural worker bounded real callback time while the
/// scene continues to prove real MIDI on a separate Patch. Each event still
/// waits for a painted projection in the semantic tail, so this is observable
/// progress rather than a wall-clock sleep or a direct lifecycle injection.
fn push_phase7_worker_dwell(steps: &mut Vec<LiveDemoStep>, patch: LivePatch, note_pairs: usize) {
    for index in 0..note_pairs {
        let note = 48_u8.saturating_add((index % 24) as u8);
        steps.extend([
            LiveDemoStep::accepted_event(AppEvent::Midi {
                patch_id: patch.patch_id,
                message: MidiMessage::try_new(patch.channel, MidiMessageKind::NoteOn, note, 104)
                    .expect("the bounded Phase 7 MIDI probe is valid"),
            }),
            LiveDemoStep::accepted_event(AppEvent::Midi {
                patch_id: patch.patch_id,
                message: MidiMessage::try_new(patch.channel, MidiMessageKind::NoteOff, note, 0)
                    .expect("the bounded Phase 7 MIDI release is valid"),
            }),
        ]);
    }
}

fn push_live_choice_cancel_and_current_commit(steps: &mut Vec<LiveDemoStep>) {
    for exit in [AppEvent::Return, AppEvent::Activate] {
        steps.extend([
            LiveDemoStep::accepted_event(AppEvent::SetInteractionMode(InteractionMode::Adjust)),
            LiveDemoStep::accepted_event(AppEvent::Adjust(Direction::Up)),
            LiveDemoStep::accepted_event(exit),
        ]);
    }
}

fn parameter_conditions_are_satisfied<'a>(
    parameter: &crate::synth::ParameterSpec,
    value: impl Fn(&ParameterId) -> Option<&'a ParameterValue>,
) -> bool {
    [parameter.visible_when(), parameter.enabled_when()]
        .into_iter()
        .flatten()
        .all(|predicate| value(predicate.parameter_id()) == Some(predicate.equals()))
}

fn push_shared_probed_checkpoint(
    steps: &mut Vec<LiveDemoStep>,
    probe_patch: LivePatch,
    shared_patch: LivePatch,
    checkpoint: LiveDemoStep,
) {
    steps.push(parameter_probe_step(
        shared_patch,
        MidiMessageKind::NoteOn,
        PARAMETER_PROBE_VELOCITY,
    ));
    push_probed_checkpoint(steps, probe_patch, checkpoint);
    steps.push(parameter_probe_step(
        shared_patch,
        MidiMessageKind::NoteOff,
        0,
    ));
}

fn move_probe_patch_to_next_track(steps: &mut Vec<LiveDemoStep>) {
    steps.extend([
        LiveDemoStep::accepted_event(AppEvent::SelectContext(TopLevelContext::Patch)),
        LiveDemoStep::accepted_event(AppEvent::EnterSurface(SurfaceId::PatchUtility)),
    ]);
    push_navigation_to_output_track(steps);
    steps.extend([
        LiveDemoStep::accepted_event(AppEvent::Adjust(Direction::Right)),
        LiveDemoStep::accepted_event(AppEvent::Return),
        LiveDemoStep::accepted_event(AppEvent::SelectContext(TopLevelContext::Mixer)),
    ]);
}

/// Walks from the Utility entry row to the output-track row through the one
/// declared order, so a scene step cannot silently land on a neighbouring row
/// and adjust the wrong control.
fn push_navigation_to_output_track(steps: &mut Vec<LiveDemoStep>) {
    for _ in 0..utility_rows_between(
        &PatchControlId::Output(PatchOutputParameter::TrimGain),
        &PatchControlId::Output(PatchOutputParameter::OutputTrack),
    ) {
        steps.push(LiveDemoStep::accepted_event(AppEvent::Navigate(
            Direction::Down,
        )));
    }
}

fn restore_probe_patch_to_first_track(steps: &mut Vec<LiveDemoStep>) {
    steps.extend([
        LiveDemoStep::accepted_event(AppEvent::SelectContext(TopLevelContext::Patch)),
        LiveDemoStep::accepted_event(AppEvent::EnterSurface(SurfaceId::PatchUtility)),
    ]);
    push_navigation_to_output_track(steps);
    for _ in MixerTrackId::MIN..MixerTrackId::MAX {
        steps.push(LiveDemoStep::accepted_event(AppEvent::Adjust(
            Direction::Left,
        )));
    }
    steps.extend([
        LiveDemoStep::accepted_event(AppEvent::Return),
        LiveDemoStep::accepted_event(AppEvent::SelectContext(TopLevelContext::Mixer)),
    ]);
}

const fn opposite_direction(direction: Direction) -> Direction {
    match direction {
        Direction::Up => Direction::Down,
        Direction::Down => Direction::Up,
        Direction::Left => Direction::Right,
        Direction::Right => Direction::Left,
    }
}

fn parameter_probe_step(patch: LivePatch, kind: MidiMessageKind, velocity: u8) -> LiveDemoStep {
    let message = MidiMessage::try_new(patch.channel, kind, PARAMETER_PROBE_NOTE, velocity)
        .expect("the bounded live parameter probe bytes are valid");
    LiveDemoStep::accepted_event(AppEvent::Midi {
        patch_id: patch.patch_id,
        message,
    })
}

fn patch_target_value(
    patch: &DecodedPatch,
    descriptor: &CapabilityDescriptor,
    target: &PatchEditableTarget,
) -> Result<f32, LiveDemoSceneError> {
    match target {
        PatchEditableTarget::Envelope(parameter) => Ok(patch.envelope.value(*parameter)),
        PatchEditableTarget::Instrument(parameter_id) => {
            let spec = descriptor
                .parameter(parameter_id)
                .ok_or(LiveDemoSceneError::InvalidInstrumentConfig)?;
            let value = patch
                .instrument
                .value(parameter_id)
                .ok_or(LiveDemoSceneError::InvalidInstrumentConfig)?;
            spec.scalar_value(value)
                .map_err(|_| LiveDemoSceneError::InvalidInstrumentConfig)
        }
    }
}

const fn parameter_adjustment(direction: Direction) -> ParameterAdjustment {
    match direction {
        Direction::Left => ParameterAdjustment::FineDecrease,
        Direction::Right => ParameterAdjustment::FineIncrease,
        Direction::Down => ParameterAdjustment::CoarseDecrease,
        Direction::Up => ParameterAdjustment::CoarseIncrease,
    }
}

fn scalar_selected_text(name: &str, value: f32) -> String {
    format!("> {name}={value}")
}

fn track_selected_text(parameter: MixerTrackParameter, value: f32) -> String {
    match parameter.descriptor().kind() {
        MixerTrackParameterKind::Continuous => scalar_selected_text(parameter.name(), value),
        MixerTrackParameterKind::Toggle => {
            format!("> {}={}", parameter.name(), value != 0.0)
        }
    }
}

fn adjusted_value(
    current: f32,
    minimum: f32,
    maximum: f32,
    direction: Direction,
    fine_step: f32,
    coarse_step: f32,
) -> Result<f32, LiveDemoSceneError> {
    let scale = decimal_scale(fine_step);
    let current_units = (current * scale).round();
    let fine_units = (fine_step * scale).round();
    let coarse_units = (coarse_step * scale).round();
    let delta = match direction {
        Direction::Left => -fine_units,
        Direction::Right => fine_units,
        Direction::Down => -coarse_units,
        Direction::Up => coarse_units,
    };
    let adjusted = ((current_units + delta) / scale).clamp(minimum, maximum);
    if adjusted == current {
        Err(LiveDemoSceneError::InvalidPlannedAdjustment)
    } else {
        Ok(adjusted)
    }
}

fn decimal_scale(step: f32) -> f32 {
    let mut scale = 1.0;
    while scale < 1_000_000.0 && (step * scale - (step * scale).round()).abs() > f32::EPSILON {
        scale *= 10.0;
    }
    scale
}

/// Returns how many downward navigations separate two declared PATCH Utility
/// rows, so the scene follows the declared order instead of assuming adjacency.
fn utility_rows_between(from: &PatchControlId, to: &PatchControlId) -> usize {
    let order = PatchControlId::utility_surface_descriptor();
    let index = |control: &PatchControlId| {
        order
            .iter()
            .position(|candidate| candidate == control)
            .expect("the scene only walks declared Utility rows")
    };
    index(to)
        .checked_sub(index(from))
        .expect("the scene walks the declared Utility order downward")
}

pub(crate) fn selected_parameter_value(
    tree: &StateTree,
    parameter: &LiveEditableParameter,
    patch_control_id: Option<PatchControlId>,
) -> Result<f32, LiveDemoSceneError> {
    let state = decode_state_tree(tree)?;
    match parameter {
        LiveEditableParameter::Patch { patch_id, target } => {
            let control = patch_control_id
                .as_ref()
                .ok_or(LiveDemoSceneError::SelectedParameterMismatch)?;
            let expected_control = match target {
                PatchEditableTarget::Envelope(parameter) => PatchControlId::Envelope(*parameter),
                PatchEditableTarget::Instrument(parameter_id) => {
                    PatchControlId::Capability(parameter_id.clone())
                }
            };
            if state.interaction.active_focus.context() != TopLevelContext::Patch
                || state.interaction.active_focus.patch_id() != Some(*patch_id)
                || state.interaction.active_focus.control_id()
                    != &SemanticControlId::Patch(control.clone())
                || control != &expected_control
            {
                return Err(LiveDemoSceneError::SelectedParameterMismatch);
            }
            let patch = state
                .patches
                .iter()
                .find(|patch| patch.id == patch_id.value())
                .ok_or(LiveDemoSceneError::SelectedParameterMismatch)?;
            let descriptor = state
                .capabilities
                .descriptor_for_config(&patch.instrument)
                .ok_or(LiveDemoSceneError::InvalidInstrumentConfig)?;
            patch_target_value(patch, descriptor, target)
        }
        LiveEditableParameter::PatchOutput {
            patch_id,
            parameter,
        } => {
            let expected_control = PatchControlId::Output(*parameter);
            if patch_control_id.as_ref() != Some(&expected_control)
                || state.interaction.active_focus.context() != TopLevelContext::Patch
                || state.interaction.active_focus.patch_id() != Some(*patch_id)
                || state.interaction.active_focus.control_id()
                    != &SemanticControlId::Patch(expected_control)
            {
                return Err(LiveDemoSceneError::SelectedParameterMismatch);
            }
            let patch = state
                .patches
                .iter()
                .find(|patch| patch.id == patch_id.value())
                .ok_or(LiveDemoSceneError::SelectedParameterMismatch)?;
            Ok(match parameter {
                PatchOutputParameter::TrimGain => patch.output.trim_gain_db(),
                PatchOutputParameter::OutputTrack => patch.output.track_id().index() as f32,
            })
        }
        LiveEditableParameter::Track {
            track_id,
            parameter,
        } => {
            if patch_control_id.is_some()
                || state.interaction.active_focus.context() != TopLevelContext::Mixer
                || state.interaction.active_focus.control_id()
                    != &SemanticControlId::Mixer(MixerControlId::Track {
                        track_id: *track_id,
                        parameter: *parameter,
                    })
            {
                return Err(LiveDemoSceneError::SelectedParameterMismatch);
            }
            let values = state.mixer.track(*track_id).clone();
            values
                .scalar_value(*parameter)
                .or_else(|| {
                    values
                        .toggle_value(*parameter)
                        .map(|value| if value { 1.0 } else { 0.0 })
                })
                .ok_or(LiveDemoSceneError::SelectedParameterMismatch)
        }
        LiveEditableParameter::Send { track_id, bus } => {
            if patch_control_id.is_some()
                || state.interaction.active_focus.context() != TopLevelContext::Mixer
                || state.interaction.active_focus.control_id()
                    != &SemanticControlId::Mixer(MixerControlId::Send {
                        track_id: *track_id,
                        bus: *bus,
                    })
            {
                return Err(LiveDemoSceneError::SelectedParameterMismatch);
            }
            Ok(state.mixer.track(*track_id).send(*bus))
        }
        LiveEditableParameter::Global { parameter } => {
            if patch_control_id.is_some()
                || state.interaction.active_focus.context() != TopLevelContext::Mixer
                || state.interaction.active_focus.control_id()
                    != &SemanticControlId::Mixer(MixerControlId::Global {
                        parameter: *parameter,
                    })
            {
                return Err(LiveDemoSceneError::SelectedParameterMismatch);
            }
            state
                .global_row_value(*parameter)
                .ok_or(LiveDemoSceneError::SelectedParameterMismatch)
        }
        LiveEditableParameter::Effect {
            patch_id,
            slot_id,
            parameter_id,
        } => {
            let control = patch_control_id
                .as_ref()
                .ok_or(LiveDemoSceneError::SelectedParameterMismatch)?;
            if state.interaction.active_focus.context() != TopLevelContext::Patch
                || state.interaction.active_focus.patch_id() != Some(*patch_id)
                || state.interaction.active_focus.control_id()
                    != &SemanticControlId::Patch(control.clone())
                || control != &PatchControlId::Effect(*slot_id, parameter_id.clone())
            {
                return Err(LiveDemoSceneError::SelectedParameterMismatch);
            }
            let patch = state
                .patches
                .iter()
                .find(|patch| patch.id == patch_id.value())
                .ok_or(LiveDemoSceneError::SelectedParameterMismatch)?;
            let config = patch
                .post_effects
                .iter()
                .find(|config| config.slot_id() == *slot_id)
                .ok_or(LiveDemoSceneError::SelectedParameterMismatch)?;
            let descriptor = state
                .effects
                .descriptor(config.capability_id())
                .ok_or(LiveDemoSceneError::InvalidEffectConfig)?;
            descriptor
                .parameter(parameter_id)
                .ok_or(LiveDemoSceneError::InvalidEffectConfig)?
                .scalar_value(
                    config
                        .value(parameter_id)
                        .ok_or(LiveDemoSceneError::InvalidEffectConfig)?,
                )
                .map_err(|_| LiveDemoSceneError::InvalidEffectConfig)
        }
    }
}

pub(crate) fn projected_parameter_values(
    tree: &StateTree,
    parameter: &LiveEditableParameter,
) -> Result<(f32, f32), LiveDemoSceneError> {
    let state = decode_state_tree(tree)?;
    match parameter {
        LiveEditableParameter::Patch { patch_id, target } => {
            let patch = state
                .patches
                .iter()
                .find(|patch| patch.id == patch_id.value())
                .ok_or(LiveDemoSceneError::SelectedParameterMismatch)?;
            let projected = state
                .parameters
                .patches
                .iter()
                .find(|patch| patch.patch_id == patch_id.value())
                .ok_or(LiveDemoSceneError::SelectedParameterMismatch)?;
            let descriptor = state
                .capabilities
                .descriptor_for_config(&patch.instrument)
                .ok_or(LiveDemoSceneError::InvalidInstrumentConfig)?;
            let state_value = patch_target_value(patch, descriptor, target)?;
            let projected_value = match target {
                PatchEditableTarget::Envelope(parameter) => projected.envelope.value(*parameter),
                PatchEditableTarget::Instrument(parameter_id) => {
                    let scalar_index = descriptor
                        .scalar_parameters()
                        .position(|spec| spec.id() == parameter_id)
                        .ok_or(LiveDemoSceneError::InvalidInstrumentConfig)?;
                    if projected.instrument.count != descriptor.scalar_parameter_count()
                        || projected.instrument.values.len() != projected.instrument.count
                    {
                        return Err(LiveDemoSceneError::InvalidInstrumentConfig);
                    }
                    *projected
                        .instrument
                        .values
                        .get(scalar_index)
                        .ok_or(LiveDemoSceneError::InvalidInstrumentConfig)?
                }
            };
            Ok((state_value, projected_value))
        }
        LiveEditableParameter::PatchOutput {
            patch_id,
            parameter,
        } => {
            let patch = state
                .patches
                .iter()
                .find(|patch| patch.id == patch_id.value())
                .ok_or(LiveDemoSceneError::SelectedParameterMismatch)?;
            let projected = state
                .parameters
                .patches
                .iter()
                .find(|patch| patch.patch_id == patch_id.value())
                .ok_or(LiveDemoSceneError::SelectedParameterMismatch)?;
            let value = |output: PatchOutput| match parameter {
                PatchOutputParameter::TrimGain => output.trim_gain_db(),
                PatchOutputParameter::OutputTrack => output.track_id().index() as f32,
            };
            Ok((value(patch.output), value(projected.output)))
        }
        LiveEditableParameter::Track {
            track_id,
            parameter,
        } => {
            let value = |track: MixerTrackParameters| {
                track.scalar_value(*parameter).or_else(|| {
                    track
                        .toggle_value(*parameter)
                        .map(|value| if value { 1.0 } else { 0.0 })
                })
            };
            let state_value = value(state.mixer.track(*track_id).clone())
                .ok_or(LiveDemoSceneError::SelectedParameterMismatch)?;
            let projected_value = value(state.parameters.mixer_tracks[track_id.index()].clone())
                .ok_or(LiveDemoSceneError::SelectedParameterMismatch)?;
            Ok((state_value, projected_value))
        }
        LiveEditableParameter::Send { track_id, bus } => Ok((
            state.mixer.track(*track_id).send(*bus),
            state.parameters.mixer_tracks[track_id.index()].send(*bus),
        )),
        LiveEditableParameter::Global { parameter } => Ok((
            state
                .global_row_value(*parameter)
                .ok_or(LiveDemoSceneError::SelectedParameterMismatch)?,
            state
                .parameters
                .projected_global_value(*parameter)
                .ok_or(LiveDemoSceneError::SelectedParameterMismatch)?,
        )),
        LiveEditableParameter::Effect {
            patch_id,
            slot_id,
            parameter_id,
        } => {
            let patch = state
                .patches
                .iter()
                .find(|patch| patch.id == patch_id.value())
                .ok_or(LiveDemoSceneError::SelectedParameterMismatch)?;
            let config = patch
                .post_effects
                .iter()
                .find(|config| config.slot_id() == *slot_id)
                .ok_or(LiveDemoSceneError::SelectedParameterMismatch)?;
            let descriptor = state
                .effects
                .descriptor(config.capability_id())
                .ok_or(LiveDemoSceneError::InvalidEffectConfig)?;
            let scalar_index = descriptor
                .scalar_parameters()
                .position(|spec| spec.id() == parameter_id)
                .ok_or(LiveDemoSceneError::InvalidEffectConfig)?;
            let state_value = descriptor
                .parameter(parameter_id)
                .ok_or(LiveDemoSceneError::InvalidEffectConfig)?
                .scalar_value(
                    config
                        .value(parameter_id)
                        .ok_or(LiveDemoSceneError::InvalidEffectConfig)?,
                )
                .map_err(|_| LiveDemoSceneError::InvalidEffectConfig)?;
            let projected = state
                .parameters
                .patches
                .iter()
                .find(|patch| patch.patch_id == patch_id.value())
                .ok_or(LiveDemoSceneError::SelectedParameterMismatch)?;
            let entry = projected
                .effects
                .iter()
                .find(|entry| entry.active && entry.slot_id == Some(*slot_id))
                .ok_or(LiveDemoSceneError::InvalidEffectConfig)?;
            if entry.scalar_count != descriptor.scalar_parameter_count()
                || entry.scalars.len() != entry.scalar_count
            {
                return Err(LiveDemoSceneError::InvalidEffectConfig);
            }
            let projected_value = *entry
                .scalars
                .get(scalar_index)
                .ok_or(LiveDemoSceneError::InvalidEffectConfig)?;
            Ok((state_value, projected_value))
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DecodedStateTree {
    pub(crate) capabilities: CapabilityRegistry,
    #[serde(default)]
    pub(crate) effects: EffectCapabilityRegistry,
    pub(crate) patches: Vec<DecodedPatch>,
    mixer: MixerState,
    global: DecodedGlobal,
    #[serde(default)]
    returns: Vec<DecodedSerializedReturn>,
    interaction: DecodedInteraction,
    pub(crate) parameters: DecodedParameterSnapshot,
}

/// One serialized return: the occupying configuration plus the return-owned
/// level, decoded from the tree's canonical `returns` section.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DecodedSerializedReturn {
    #[serde(default)]
    effect: Option<crate::synth::PostEffectConfig>,
    /// Part of the serialized return shape, decoded so a missing or renamed
    /// key fails here; row counting itself needs only the occupancy shape.
    #[allow(dead_code)]
    return_level: f32,
}

impl DecodedStateTree {
    /// Resolves one MIXER global row's state-side value: master gain alone.
    fn global_row_value(&self, parameter: GlobalParameter) -> Option<f32> {
        match parameter {
            GlobalParameter::MasterGainDb => Some(self.global.master_gain_db),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DecodedInteraction {
    active_focus: FocusPath,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DecodedPatch {
    pub(crate) id: u32,
    pub(crate) channel: u8,
    pub(crate) instrument: InstrumentConfig,
    #[serde(default)]
    pub(crate) post_effects: Vec<PostEffectConfig>,
    envelope: VoiceEnvelope,
    pub(crate) output: PatchOutput,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DecodedGlobal {
    master_gain_db: f32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DecodedParameterSnapshot {
    #[serde(rename = "graphRevision")]
    _graph_revision: GraphRevision,
    pub(crate) patches: Vec<DecodedParameterPatch>,
    mixer_tracks: [MixerTrackParameters; MixerTrackId::COUNT],
    /// Part of the serialized snapshot shape, decoded so the return section
    /// must be present and well-formed even where this scene's assertions
    /// read only the patch and track sections.
    #[allow(dead_code)]
    returns: Vec<DecodedReturnParameters>,
    global: DecodedParameterGlobal,
}

/// The snapshot's global object keeps only master gain: return-owned values
/// travel as the indexed `returns` entries.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DecodedParameterGlobal {
    master_gain_db: f32,
}

impl DecodedParameterSnapshot {
    /// Reads the projected value for one global surface parameter: master
    /// gain alone. Return-owned values travel as the indexed `returns`
    /// entries and are addressed by `BusId`, never through a global row.
    fn projected_global_value(&self, parameter: GlobalParameter) -> Option<f32> {
        match parameter {
            GlobalParameter::MasterGainDb => Some(self.global.master_gain_db),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DecodedParameterPatch {
    pub(crate) patch_id: u32,
    /// The canonical per-Patch ceiling, published beside the envelope so a
    /// scene can derive how far it must step to make the limit bite instead
    /// of assuming a starting value.
    pub(crate) voice_limit: u16,
    envelope: VoiceEnvelope,
    instrument: DecodedInstrumentParameters,
    /// One live entry per ordered effect position.
    effects: Vec<DecodedEffectParameters>,
    output: PatchOutput,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DecodedEffectParameters {
    active: bool,
    slot_id: Option<EffectSlotId>,
    scalar_count: usize,
    scalars: Vec<f32>,
}

/// One live bus-return entry: the occupying instance's scalars plus the
/// return-owned level.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct DecodedReturnParameters {
    active: bool,
    slot_id: Option<EffectSlotId>,
    scalar_count: usize,
    scalars: Vec<f32>,
    return_level: f32,
}

#[derive(Clone, Debug, Deserialize)]
struct DecodedInstrumentParameters {
    count: usize,
    values: Vec<f32>,
}

pub(crate) fn decode_state_tree(tree: &StateTree) -> Result<DecodedStateTree, LiveDemoSceneError> {
    serde_json::from_str(tree.json()).map_err(|_| LiveDemoSceneError::StateTreeDeserialization)
}

/// A structural or expectation error in the frozen live scene.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LiveDemoSceneError {
    StateTreeDeserialization,
    NoInstalledPatches,
    UnexpectedInitialSelection,
    InvalidPatchId,
    DuplicatePatchId(u32),
    InvalidMidiChannel(u8),
    InvalidInstrumentConfig,
    InvalidEffectConfig,
    EngineFixtureUnavailable,
    PresetFixtureUnavailable,
    SampleFixtureUnavailable,
    InvalidPlannedAdjustment,
    SelectedParameterMismatch,
    ExpectedValueMismatch { expected: f32, actual: f32 },
    GenerationOverflow,
}

impl fmt::Display for LiveDemoSceneError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::StateTreeDeserialization => {
                formatter.write_str("canonical state tree could not be decoded")
            }
            Self::NoInstalledPatches => {
                formatter.write_str("live demo requires installed fixture patches")
            }
            Self::UnexpectedInitialSelection => {
                formatter.write_str("live demo must start at the first Patch gain parameter")
            }
            Self::InvalidPatchId => {
                formatter.write_str("installed state contains an invalid PatchId")
            }
            Self::DuplicatePatchId(value) => {
                write!(formatter, "installed state repeats PatchId {value}")
            }
            Self::InvalidMidiChannel(value) => write!(
                formatter,
                "installed state contains invalid MIDI channel {value}"
            ),
            Self::InvalidInstrumentConfig => {
                formatter.write_str("installed state contains an invalid instrument schema/config")
            }
            Self::InvalidEffectConfig => {
                formatter.write_str("installed state contains an invalid Patch effect schema/config")
            }
            Self::EngineFixtureUnavailable => formatter.write_str(
                "live engine proof requires the focused first Patch and an adjacent installed capability",
            ),
            Self::PresetFixtureUnavailable => formatter.write_str(
                "live preset proof requires a descriptor structural choice with an adjacent option",
            ),
            Self::SampleFixtureUnavailable => formatter.write_str(
                "Phase 7 live proof requires one installed Patch with a Sample asset reference",
            ),
            Self::InvalidPlannedAdjustment => {
                formatter.write_str("descriptor-derived adjustment would not change its parameter")
            }
            Self::SelectedParameterMismatch => {
                formatter.write_str("live scene selection no longer matches its frozen plan")
            }
            Self::ExpectedValueMismatch { expected, actual } => write!(
                formatter,
                "live scene expected selected value {expected}, got {actual}"
            ),
            Self::GenerationOverflow => formatter.write_str("live scene generation cannot advance"),
        }
    }
}

impl std::error::Error for LiveDemoSceneError {}

#[cfg(test)]
mod tests {
    use super::{frozen_identity_baseline_is_preserved, LiveAudioPredicate};
    use crate::real_time::audio_observation_snapshot::AudioObservationSnapshot;

    /// The C-004 guard the scenes assert at declaration time: the frozen
    /// identity baseline must survive byte-identically and in order, and
    /// only pure insertions are tolerated around it.
    #[test]
    fn frozen_identity_baselines_admit_only_pure_insertions() {
        let baseline = ["first", "second", "third"];
        // Byte-identical and in order.
        assert!(frozen_identity_baseline_is_preserved(
            ["first", "second", "third"],
            &baseline,
        ));
        // Pure insertions between, before, and after baseline members.
        assert!(frozen_identity_baseline_is_preserved(
            [
                "zeroth",
                "first",
                "firstCycle1",
                "second",
                "third",
                "fourth"
            ],
            &baseline,
        ));
        // A dropped baseline identity is a regression.
        assert!(!frozen_identity_baseline_is_preserved(
            ["first", "third"],
            &baseline,
        ));
        // A reordered baseline is a regression.
        assert!(!frozen_identity_baseline_is_preserved(
            ["second", "first", "third"],
            &baseline,
        ));
        // A renamed (non-byte-identical) baseline identity is a regression.
        assert!(!frozen_identity_baseline_is_preserved(
            ["first", "second-renamed", "third"],
            &baseline,
        ));
    }

    #[allow(clippy::too_many_arguments)]
    fn observation(
        left: f32,
        right: f32,
        output: f32,
        reverb: f32,
        delay: f32,
        wet: f32,
        non_finite: u64,
    ) -> AudioObservationSnapshot {
        AudioObservationSnapshot::from_parts(
            2, 2, 64, 7, 1, 1, left, right, output, reverb, delay, wet, non_finite, 0,
        )
    }

    #[test]
    fn audible_predicates_are_finite_and_stage_specific() {
        assert!(
            LiveAudioPredicate::OutputLevel.evaluate(observation(0.4, 0.3, 0.2, 0.0, 0.0, 0.0, 0,))
        );
        assert!(LiveAudioPredicate::StereoBalance
            .evaluate(observation(0.4, 0.3, 0.2, 0.0, 0.0, 0.0, 0,)));
        assert!(!LiveAudioPredicate::StereoBalance
            .evaluate(observation(0.4, 0.4, 0.2, 0.0, 0.0, 0.0, 0,)));
        assert!(
            LiveAudioPredicate::ReverbInput.evaluate(observation(0.4, 0.3, 0.2, 0.1, 0.0, 0.0, 0,))
        );
        assert!(
            LiveAudioPredicate::DelayInput.evaluate(observation(0.4, 0.3, 0.2, 0.0, 0.1, 0.0, 0,))
        );
        assert!(
            LiveAudioPredicate::WetOutput.evaluate(observation(0.4, 0.3, 0.2, 0.0, 0.0, 0.1, 0,))
        );

        let non_finite = observation(f32::NAN, 0.3, 0.2, 0.1, 0.1, 0.1, 1);
        for predicate in [
            LiveAudioPredicate::OutputLevel,
            LiveAudioPredicate::StereoBalance,
            LiveAudioPredicate::ReverbInput,
            LiveAudioPredicate::DelayInput,
            LiveAudioPredicate::WetOutput,
        ] {
            assert!(!predicate.evaluate(non_finite));
        }
    }
}
