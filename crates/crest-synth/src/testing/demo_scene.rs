use crate::control::app_event::{AppEvent, Direction};
use crate::control::app_state::EventRejection;
use crate::control::event_log::EventLog;
use crate::control::event_record::EventRecord;
use crate::control::state_tree::StateTree;
use crate::control::text_projection::TextProjection;
use crate::control::{
    EngineSelectionFailure, EngineSelectionStatusKind, PatchControlId, SemanticAction, SurfaceId,
};
use crate::kernel::midi_channel::MidiChannel;
use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crate::kernel::patch_id::PatchId;
use crate::mixer::global_parameters::{GlobalParameter, GlobalParameters};
use crate::mixer::mixer_track_id::MixerTrackId;
use crate::mixer::mixer_track_parameters::{
    MixerTrackParameter, MixerTrackParameterKind, MixerTrackParameters,
};
use crate::mixer::patch_output::{PatchOutputParameter, PatchOutputParameterKind};
use crate::real_time::audio_command::AudioCommand;
use crate::shell::window_input::{WindowInput, WindowInputKind, WindowKey};
use crate::synth::effect_slot_id::MAX_EFFECT_SLOTS;
use crate::synth::instrument_capability::{CapabilityRegistry, ParameterValue};
use crate::synth::patch::{Patch, PatchEditableTarget};
use crate::synth::{
    EffectCapabilityRegistry, ParameterId, PatchInteraction, PostEffectConfig,
    VoiceEnvelopeParameter,
};
use core::fmt;
use std::collections::BTreeSet;
use std::time::Duration;

const TICK_DURATION: Duration = Duration::from_millis(10);
/// An immutable point at which a scene runner records coherent projections.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DemoCheckpoint {
    name: String,
    expected_last_rejection: Option<EventRejection>,
    engine: Option<DemoEngineExpectation>,
    preset: Option<DemoPresetExpectation>,
    patch_adsr: Option<DemoPatchAdsrExpectation>,
}

impl DemoCheckpoint {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            expected_last_rejection: None,
            engine: None,
            preset: None,
            patch_adsr: None,
        }
    }

    pub fn after_rejection(
        name: impl Into<String>,
        expected_last_rejection: EventRejection,
    ) -> Self {
        Self {
            name: name.into(),
            expected_last_rejection: Some(expected_last_rejection),
            engine: None,
            preset: None,
            patch_adsr: None,
        }
    }

    pub fn engine(
        name: impl Into<String>,
        status: EngineSelectionStatusKind,
        active_capability_id: crate::synth::CapabilityId,
        requested_capability_id: Option<crate::synth::CapabilityId>,
        failure: Option<EngineSelectionFailure>,
        require_target_audio: bool,
    ) -> Self {
        Self {
            name: name.into(),
            expected_last_rejection: None,
            engine: Some(DemoEngineExpectation {
                status,
                active_capability_id,
                requested_capability_id,
                failure,
                require_target_audio,
            }),
            preset: None,
            patch_adsr: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn preset(
        name: impl Into<String>,
        patch_id: PatchId,
        status: EngineSelectionStatusKind,
        parameter_id: ParameterId,
        baseline_choice_id: impl Into<String>,
        selected_choice_id: impl Into<String>,
        selected_label: impl Into<String>,
        requested_choice: Option<(String, String)>,
        failure: Option<EngineSelectionFailure>,
        require_target_audio: bool,
        expected_assignment_changes: usize,
    ) -> Self {
        Self {
            name: name.into(),
            expected_last_rejection: None,
            engine: None,
            preset: Some(DemoPresetExpectation {
                patch_id,
                status,
                parameter_id,
                baseline_choice_id: baseline_choice_id.into(),
                selected_choice_id: selected_choice_id.into(),
                selected_label: selected_label.into(),
                requested_choice,
                failure,
                require_target_audio,
                expected_assignment_changes,
            }),
            patch_adsr: None,
        }
    }

    pub fn patch_adsr(
        name: impl Into<String>,
        patch_id: PatchId,
        parameter: VoiceEnvelopeParameter,
        expected_value: f32,
        lifecycle: Option<EngineSelectionStatusKind>,
    ) -> Self {
        Self {
            name: name.into(),
            expected_last_rejection: None,
            engine: None,
            preset: None,
            patch_adsr: Some(DemoPatchAdsrExpectation {
                patch_id,
                parameter,
                expected_value_bits: expected_value.to_bits(),
                lifecycle,
            }),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn expected_last_rejection(&self) -> Option<EventRejection> {
        self.expected_last_rejection
    }

    pub const fn engine_expectation(&self) -> Option<&DemoEngineExpectation> {
        self.engine.as_ref()
    }

    pub const fn preset_expectation(&self) -> Option<&DemoPresetExpectation> {
        self.preset.as_ref()
    }

    pub const fn patch_adsr_expectation(&self) -> Option<&DemoPatchAdsrExpectation> {
        self.patch_adsr.as_ref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DemoPresetExpectation {
    patch_id: PatchId,
    status: EngineSelectionStatusKind,
    parameter_id: ParameterId,
    baseline_choice_id: String,
    selected_choice_id: String,
    selected_label: String,
    requested_choice: Option<(String, String)>,
    failure: Option<EngineSelectionFailure>,
    require_target_audio: bool,
    expected_assignment_changes: usize,
}

impl DemoPresetExpectation {
    pub const fn patch_id(&self) -> PatchId {
        self.patch_id
    }

    pub const fn status(&self) -> EngineSelectionStatusKind {
        self.status
    }

    pub const fn parameter_id(&self) -> &ParameterId {
        &self.parameter_id
    }

    pub fn control_id(&self) -> PatchControlId {
        PatchControlId::Capability(self.parameter_id.clone())
    }

    pub fn baseline_choice_id(&self) -> &str {
        &self.baseline_choice_id
    }

    pub fn selected_choice_id(&self) -> &str {
        &self.selected_choice_id
    }

    pub fn selected_label(&self) -> &str {
        &self.selected_label
    }

    pub fn requested_choice(&self) -> Option<(&str, &str)> {
        self.requested_choice
            .as_ref()
            .map(|(id, label)| (id.as_str(), label.as_str()))
    }

    pub const fn failure(&self) -> Option<EngineSelectionFailure> {
        self.failure
    }

    pub const fn require_target_audio(&self) -> bool {
        self.require_target_audio
    }

    pub const fn expected_assignment_changes(&self) -> usize {
        self.expected_assignment_changes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DemoPatchAdsrExpectation {
    patch_id: PatchId,
    parameter: VoiceEnvelopeParameter,
    expected_value_bits: u32,
    lifecycle: Option<EngineSelectionStatusKind>,
}

impl DemoPatchAdsrExpectation {
    pub const fn patch_id(self) -> PatchId {
        self.patch_id
    }

    pub const fn parameter(self) -> VoiceEnvelopeParameter {
        self.parameter
    }

    pub const fn control_id(self) -> PatchControlId {
        PatchControlId::Envelope(self.parameter)
    }

    pub const fn expected_value(self) -> f32 {
        f32::from_bits(self.expected_value_bits)
    }

    pub const fn lifecycle(self) -> Option<EngineSelectionStatusKind> {
        self.lifecycle
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DemoEngineExpectation {
    status: EngineSelectionStatusKind,
    active_capability_id: crate::synth::CapabilityId,
    requested_capability_id: Option<crate::synth::CapabilityId>,
    failure: Option<EngineSelectionFailure>,
    require_target_audio: bool,
}

impl DemoEngineExpectation {
    pub const fn status(&self) -> EngineSelectionStatusKind {
        self.status
    }

    pub const fn active_capability_id(&self) -> &crate::synth::CapabilityId {
        &self.active_capability_id
    }

    pub const fn requested_capability_id(&self) -> Option<&crate::synth::CapabilityId> {
        self.requested_capability_id.as_ref()
    }

    pub const fn failure(&self) -> Option<EngineSelectionFailure> {
        self.failure
    }

    pub const fn require_target_audio(&self) -> bool {
        self.require_target_audio
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DemoWorkerAdvance {
    Healthy,
    Fail(EngineSelectionFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DemoEngineProbe {
    StaleWorkerFailure,
    EarlyAcknowledgement,
    MismatchedAcknowledgement,
}

/// One semantic MIDI input and the reducer outcome expected from it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MidiProbe {
    patch_id: PatchId,
    message: MidiMessage,
    expected_rejection: Option<EventRejection>,
}

impl MidiProbe {
    pub const fn accepted(patch_id: PatchId, message: MidiMessage) -> Self {
        Self {
            patch_id,
            message,
            expected_rejection: None,
        }
    }

    pub const fn rejected(
        patch_id: PatchId,
        message: MidiMessage,
        expected_rejection: EventRejection,
    ) -> Self {
        Self {
            patch_id,
            message,
            expected_rejection: Some(expected_rejection),
        }
    }

    pub const fn patch_id(&self) -> PatchId {
        self.patch_id
    }

    pub const fn message(&self) -> MidiMessage {
        self.message
    }

    pub const fn expected_rejection(&self) -> Option<EventRejection> {
        self.expected_rejection
    }
}

/// One deterministic action in the exhaustive headless scene.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DemoSceneStep {
    WindowInput(WindowInput),
    PassiveAction(SemanticAction),
    MidiProbe(MidiProbe),
    AudioCommandProbe(AudioCommand),
    Tick(Duration),
    AdvanceWorker(DemoWorkerAdvance),
    AdvanceStructural,
    EngineProbe(DemoEngineProbe),
    Checkpoint(Box<DemoCheckpoint>),
}

/// A complete deterministic control-surface scene derived from installed Patches.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DemoScene {
    name: String,
    schema_version: u32,
    steps: Vec<DemoSceneStep>,
    expected_coverage: Vec<String>,
}

impl DemoScene {
    pub const NAME: &'static str = "exhaustive-gui-demo";
    pub const SCHEMA_VERSION: u32 = 6;

    /// Derives the complete current scene from the accepted fixture Patch list.
    ///
    /// At least two Patches are required because routing and mixer observations
    /// must discriminate the edited Patch from an unaffected Patch.
    pub fn exhaustive(
        capabilities: &CapabilityRegistry,
        installed_patches: &[Patch],
        global_parameters: &GlobalParameters,
    ) -> Result<Self, DemoSceneError> {
        Self::exhaustive_with_effects(
            capabilities,
            &EffectCapabilityRegistry::default(),
            installed_patches,
            global_parameters,
            &crate::mixer::bus_return::BusReturnBank::default(),
        )
    }

    /// Derives the complete scene from both installed capability families.
    pub fn exhaustive_with_effects(
        capabilities: &CapabilityRegistry,
        effects: &EffectCapabilityRegistry,
        installed_patches: &[Patch],
        global_parameters: &GlobalParameters,
        returns: &crate::mixer::bus_return::BusReturnBank,
    ) -> Result<Self, DemoSceneError> {
        if installed_patches.len() < 2 {
            return Err(DemoSceneError::InsufficientPatches {
                actual: installed_patches.len(),
            });
        }
        if installed_patches.iter().any(|patch| {
            capabilities
                .validate_config(patch.instrument_config())
                .is_err()
        }) {
            return Err(DemoSceneError::InvalidInstrumentConfig);
        }
        if installed_patches
            .iter()
            .any(|patch| !effect_chain_is_valid(effects, patch.effect_slots()))
        {
            return Err(DemoSceneError::InvalidEffectConfig);
        }
        if installed_patches[0]
            .instrument_config()
            .capability_id()
            .as_str()
            != crate::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID
            || capabilities
                .descriptor(
                    &crate::synth::CapabilityId::new(
                        crate::adapter::braids_capability::BRAIDS_CAPABILITY_ID,
                    )
                    .expect("the production Braids id is valid"),
                )
                .is_none()
        {
            return Err(DemoSceneError::EngineFixtureUnavailable);
        }

        let preset_fixture = DemoPresetFixture::from_patch(capabilities, &installed_patches[0])
            .ok_or(DemoSceneError::PresetFixtureUnavailable)?;

        Ok(Self {
            name: Self::NAME.to_owned(),
            schema_version: Self::SCHEMA_VERSION,
            steps: build_steps(
                capabilities,
                effects,
                installed_patches,
                global_parameters,
                returns,
                &preset_fixture,
            ),
            expected_coverage: build_expected_coverage(
                capabilities,
                effects,
                installed_patches,
                returns,
            ),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn steps(&self) -> &[DemoSceneStep] {
        &self.steps
    }

    pub fn expected_coverage(&self) -> &[String] {
        &self.expected_coverage
    }

    /// A safe journal capacity for startup plus every potentially observable step.
    pub fn event_log_capacity(&self) -> usize {
        self.steps.len().saturating_add(1)
    }

    pub fn into_steps(self) -> Vec<DemoSceneStep> {
        self.steps
    }
}

/// A scene cannot prove multi-Patch isolation without two installed Patches.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DemoSceneError {
    InsufficientPatches { actual: usize },
    InvalidInstrumentConfig,
    InvalidEffectConfig,
    EngineFixtureUnavailable,
    PresetFixtureUnavailable,
}

impl fmt::Display for DemoSceneError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::InsufficientPatches { actual } => write!(
                formatter,
                "the exhaustive demo requires at least two installed Patches, got {actual}"
            ),
            Self::InvalidInstrumentConfig => {
                formatter.write_str("installed Patch config does not match the capability registry")
            }
            Self::InvalidEffectConfig => formatter
                .write_str("installed Patch effect config does not match the effect registry"),
            Self::EngineFixtureUnavailable => formatter.write_str(
                "the exhaustive demo requires a first SoundFont Patch and installed Braids capability",
            ),
            Self::PresetFixtureUnavailable => formatter.write_str(
                "the exhaustive demo requires an installed SoundFont preset with an adjacent choice",
            ),
        }
    }
}

impl std::error::Error for DemoSceneError {}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DemoPresetFixture {
    parameter_id: ParameterId,
    source_choice_id: String,
    source_label: String,
    target_choice_id: String,
    target_label: String,
}

impl DemoPresetFixture {
    fn from_patch(capabilities: &CapabilityRegistry, patch: &Patch) -> Option<Self> {
        let parameter_id = ParameterId::new(
            crate::adapter::hidef_soundfont_capability::SOUNDFONT_PRESET_PARAMETER_ID,
        )
        .ok()?;
        let descriptor = capabilities.descriptor_for_config(patch.instrument_config())?;
        let spec = descriptor.parameter(&parameter_id)?;
        let source_choice_id = match patch.instrument_config().value(&parameter_id)? {
            ParameterValue::Choice(choice_id) => choice_id,
            _ => return None,
        };
        let source_index = spec
            .choices()
            .iter()
            .position(|choice| choice.id() == source_choice_id)?;
        let source = spec.choices().get(source_index)?;
        let target = spec.choices().get(source_index.checked_add(1)?)?;
        Some(Self {
            parameter_id,
            source_choice_id: source.id().to_owned(),
            source_label: source.label().to_owned(),
            target_choice_id: target.id().to_owned(),
            target_label: target.label().to_owned(),
        })
    }
}

fn build_steps(
    capabilities: &CapabilityRegistry,
    effects: &EffectCapabilityRegistry,
    patches: &[Patch],
    global_parameters: &GlobalParameters,
    returns: &crate::mixer::bus_return::BusReturnBank,
    preset_fixture: &DemoPresetFixture,
) -> Vec<DemoSceneStep> {
    let mut steps = Vec::new();
    let mut boundary_probed = BTreeSet::new();
    push_checkpoint(&mut steps, DemoCheckpoint::new("scene.start"));

    // Exercise the still-unavailable vertical PATCH action before the exhaustive input sweep.
    // Both rejected actions leave its page and generation unchanged, and the
    // direct MIXER key proves the next semantic event is still accepted.
    push_key_press(&mut steps, WindowKey::Digit2);

    // Step to the next installed Patch and back. The fixture installs one
    // Patch per MIDI part, so this gesture is the only way any instrument
    // after the first is reachable at all. The round trip is deliberate: it
    // proves the move in both directions and leaves the focused Patch exactly
    // where the rest of the scene expects it.
    push_key_press(&mut steps, WindowKey::E);
    push_checkpoint(&mut steps, DemoCheckpoint::new("context.patch.nextPatch"));
    push_key_press(&mut steps, WindowKey::Q);
    push_checkpoint(
        &mut steps,
        DemoCheckpoint::new("context.patch.previousPatch"),
    );
    // At the first position there is nowhere further left: an unchanged
    // rejection, never a wrap onto the last Patch.
    push_key_press(&mut steps, WindowKey::Q);
    push_checkpoint(
        &mut steps,
        DemoCheckpoint::after_rejection(
            "context.patch.previousPatchRejected",
            EventRejection::ParameterAtBoundary,
        ),
    );

    push_key_press(&mut steps, WindowKey::W);
    push_checkpoint(
        &mut steps,
        DemoCheckpoint::after_rejection(
            "context.patch.navigateRejected",
            EventRejection::ActionUnavailableInContext,
        ),
    );
    steps.push(DemoSceneStep::WindowInput(WindowInput::key_down(
        WindowKey::K,
    )));
    push_key_press(&mut steps, WindowKey::W);
    push_checkpoint(
        &mut steps,
        DemoCheckpoint::new("context.patch.choiceOpened"),
    );
    steps.push(DemoSceneStep::WindowInput(WindowInput::key_up(
        WindowKey::K,
    )));
    push_key_press(&mut steps, WindowKey::Digit1);
    push_checkpoint(&mut steps, DemoCheckpoint::new("context.mixer.recovered"));

    for input in complete_window_vocabulary() {
        steps.push(DemoSceneStep::WindowInput(*input));
    }

    // Shift+Start in the exhaustive vocabulary opens global MIDI Settings.
    // Return through the semantic boundary so the remainder of the scene
    // resumes at the exact reducer-owned origin, then exercise unmodified
    // Start independently so PreviewStart and PreviewStop remain covered.
    steps.push(DemoSceneStep::PassiveAction(SemanticAction::Return));
    push_key_press(&mut steps, WindowKey::Space);

    // Prove that focus loss clears modifier state, then restore the canonical
    // T00/Level startup focus after the exhaustive vocabulary's T01/Pan end.
    steps.push(DemoSceneStep::WindowInput(WindowInput::key_down(
        WindowKey::K,
    )));
    steps.push(DemoSceneStep::WindowInput(WindowInput::focus_lost()));
    push_key_press(&mut steps, WindowKey::W);
    push_key_press(&mut steps, WindowKey::A);
    push_checkpoint(&mut steps, DemoCheckpoint::new("input.vocabulary"));

    // The vocabulary's accepted Right and Down actions leave T01/Pan focused.
    // Return explicitly to the canonical startup focus before the surface and
    // parameter sweeps so every following path has a stable origin.
    push_key_press(&mut steps, WindowKey::W);
    push_key_press(&mut steps, WindowKey::A);
    // The vocabulary also opens Sends and advances its remembered destination
    // with E. Restore that independent root through the same keyboard path.
    push_key_press(&mut steps, WindowKey::Digit4);
    push_key_press(&mut steps, WindowKey::Q);
    push_key_press(&mut steps, WindowKey::Digit1);
    push_checkpoint(
        &mut steps,
        DemoCheckpoint::new("input.startupFocusRestored"),
    );

    // Exercise both persistent side surfaces through the passive semantic
    // action boundary, including exact reducer-owned return origins.
    push_key_press(&mut steps, WindowKey::Digit1);
    steps.push(DemoSceneStep::PassiveAction(SemanticAction::EnterSurface(
        SurfaceId::MixerInspector,
    )));
    push_checkpoint(&mut steps, DemoCheckpoint::new("surface.inspector.entered"));
    steps.push(DemoSceneStep::PassiveAction(SemanticAction::Return));
    push_checkpoint(
        &mut steps,
        DemoCheckpoint::new("surface.inspector.returned"),
    );
    push_key_press(&mut steps, WindowKey::Digit2);
    steps.push(DemoSceneStep::PassiveAction(SemanticAction::EnterSurface(
        SurfaceId::PatchUtility,
    )));
    push_checkpoint(&mut steps, DemoCheckpoint::new("surface.utility.entered"));
    steps.push(DemoSceneStep::PassiveAction(SemanticAction::Return));
    push_checkpoint(&mut steps, DemoCheckpoint::new("surface.utility.returned"));
    // The generic related-surface action reaches the same descriptor-owned
    // detail subject through the Phase 7 semantic vocabulary. Keep the
    // explicit EnterSurface journey below as the independent passive-surface
    // admission witness; both return to the identical engine-row origin.
    steps.push(DemoSceneStep::PassiveAction(SemanticAction::OpenRelated));
    push_checkpoint(
        &mut steps,
        DemoCheckpoint::new("surface.detail.openRelatedEntered"),
    );
    steps.push(DemoSceneStep::PassiveAction(SemanticAction::Return));
    push_checkpoint(
        &mut steps,
        DemoCheckpoint::new("surface.detail.openRelatedReturned"),
    );
    for direction in [Direction::Down, Direction::Up] {
        steps.push(DemoSceneStep::PassiveAction(SemanticAction::NavigatePage(
            direction,
        )));
    }
    // The subordinate detail surface, entered and left through the same passive
    // semantic action boundary as the two persistent sides. Return restores the
    // exact origin, so the pair leaves the scene's focus where it found it and
    // every following sweep still starts from the canonical PATCH row.
    steps.push(DemoSceneStep::PassiveAction(SemanticAction::EnterSurface(
        SurfaceId::PatchDetail,
    )));
    push_checkpoint(&mut steps, DemoCheckpoint::new("surface.detail.entered"));
    steps.push(DemoSceneStep::PassiveAction(SemanticAction::Return));
    push_checkpoint(&mut steps, DemoCheckpoint::new("surface.detail.returned"));
    push_key_press(&mut steps, WindowKey::Digit1);

    push_patch_output_control_steps(&mut steps, &patches[0], &mut boundary_probed);
    push_patch_adsr_control_steps(&mut steps, capabilities, &patches[0], &mut boundary_probed);
    push_patch_effect_control_steps(&mut steps, effects, &patches[0], &mut boundary_probed);

    push_mixer_control_steps(
        &mut steps,
        effects,
        global_parameters,
        returns,
        &mut boundary_probed,
    );
    push_checkpoint(&mut steps, DemoCheckpoint::new("selection.restored"));

    push_preset_selection_steps(&mut steps, capabilities, &patches[0], preset_fixture);
    push_engine_selection_steps(&mut steps, capabilities, &patches[0]);
    push_topology_occupancy_steps(&mut steps, effects, &patches[0]);

    for patch in patches {
        let descriptor = capabilities
            .descriptor_for_config(patch.instrument_config())
            .expect("validated MIDI Patch capability is installed");
        for message in midi_messages(patch.channel(), descriptor.supported_midi_kinds()) {
            steps.push(DemoSceneStep::MidiProbe(MidiProbe::accepted(
                patch.id(),
                message,
            )));
            steps.push(DemoSceneStep::Tick(TICK_DURATION));
        }
        push_checkpoint(
            &mut steps,
            DemoCheckpoint::new(format!("midi.patch.{}", patch.id().value())),
        );
    }

    steps.push(DemoSceneStep::AudioCommandProbe(
        AudioCommand::all_notes_off(),
    ));
    steps.push(DemoSceneStep::Tick(TICK_DURATION));
    push_checkpoint(&mut steps, DemoCheckpoint::new("audioCommand.allNotesOff"));

    let unknown_patch = first_unknown_patch_id(patches);
    let unknown_message = midi_message(patches[0].channel(), MidiMessageKind::NoteOn, 67, 91);
    steps.push(DemoSceneStep::MidiProbe(MidiProbe::rejected(
        unknown_patch,
        unknown_message,
        EventRejection::UnknownPatch,
    )));
    push_checkpoint(
        &mut steps,
        DemoCheckpoint::after_rejection("midi.unknownPatch", EventRejection::UnknownPatch),
    );
    push_checkpoint(&mut steps, DemoCheckpoint::new("scene.complete"));

    steps
}

fn push_preset_selection_steps(
    steps: &mut Vec<DemoSceneStep>,
    capabilities: &CapabilityRegistry,
    patch: &Patch,
    fixture: &DemoPresetFixture,
) {
    let requested_target = || {
        Some((
            fixture.target_choice_id.clone(),
            fixture.target_label.clone(),
        ))
    };
    let requested_source = || {
        Some((
            fixture.source_choice_id.clone(),
            fixture.source_label.clone(),
        ))
    };

    push_key_press(steps, WindowKey::Digit2);
    steps.push(DemoSceneStep::PassiveAction(SemanticAction::EnterSurface(
        SurfaceId::PatchDetail,
    )));
    let descriptor = capabilities
        .descriptor_for_config(patch.instrument_config())
        .expect("validated preset Patch capability is installed");
    let preset_index = descriptor
        .parameters()
        .filter(|parameter| parameter_is_visible_and_enabled(parameter, patch.instrument_config()))
        .position(|parameter| parameter.id() == &fixture.parameter_id)
        .expect("the preset fixture names one visible enabled Detail row");
    for _ in 0..preset_index {
        push_key_press(steps, WindowKey::S);
    }
    push_single_adjustment(steps, WindowKey::S);
    push_checkpoint(
        steps,
        DemoCheckpoint::after_rejection(
            "preset.vertical.unavailable",
            EventRejection::ActionUnavailableInContext,
        ),
    );

    steps.push(DemoSceneStep::MidiProbe(MidiProbe::accepted(
        patch.id(),
        midi_message(patch.channel(), MidiMessageKind::NoteOn, 60, 110),
    )));
    steps.push(DemoSceneStep::Tick(TICK_DURATION));
    push_single_adjustment(steps, WindowKey::D);
    push_structural_admission(steps);
    push_checkpoint(
        steps,
        DemoCheckpoint::preset(
            "preset.forward.preparing",
            patch.id(),
            EngineSelectionStatusKind::Preparing,
            fixture.parameter_id.clone(),
            &fixture.source_choice_id,
            &fixture.source_choice_id,
            &fixture.source_label,
            requested_target(),
            None,
            true,
            0,
        ),
    );
    push_single_adjustment(steps, WindowKey::D);
    push_checkpoint(
        steps,
        DemoCheckpoint::after_rejection(
            "preset.forward.busyRejected",
            EventRejection::StructuralEditBusy,
        ),
    );
    steps.push(DemoSceneStep::EngineProbe(
        DemoEngineProbe::StaleWorkerFailure,
    ));
    push_checkpoint(
        steps,
        DemoCheckpoint::after_rejection(
            "preset.forward.staleRejected",
            EventRejection::StaleEngineSelection,
        ),
    );
    steps.push(DemoSceneStep::EngineProbe(
        DemoEngineProbe::EarlyAcknowledgement,
    ));
    push_checkpoint(
        steps,
        DemoCheckpoint::after_rejection(
            "preset.forward.earlyAckRejected",
            EventRejection::StaleEngineSelection,
        ),
    );
    push_successful_preset_transition(
        steps,
        patch,
        fixture,
        "preset.forward",
        &fixture.source_choice_id,
        &fixture.source_label,
        0,
        &fixture.target_choice_id,
        &fixture.target_label,
        requested_target(),
        1,
    );

    push_single_adjustment(steps, WindowKey::A);
    push_structural_admission(steps);
    push_checkpoint(
        steps,
        DemoCheckpoint::preset(
            "preset.restore.preparing",
            patch.id(),
            EngineSelectionStatusKind::Preparing,
            fixture.parameter_id.clone(),
            &fixture.source_choice_id,
            &fixture.target_choice_id,
            &fixture.target_label,
            requested_source(),
            None,
            false,
            1,
        ),
    );
    push_successful_preset_transition(
        steps,
        patch,
        fixture,
        "preset.restore",
        &fixture.target_choice_id,
        &fixture.target_label,
        1,
        &fixture.source_choice_id,
        &fixture.source_label,
        requested_source(),
        0,
    );

    push_single_adjustment(steps, WindowKey::D);
    push_structural_admission(steps);
    push_checkpoint(
        steps,
        DemoCheckpoint::preset(
            "preset.failure.preparing",
            patch.id(),
            EngineSelectionStatusKind::Preparing,
            fixture.parameter_id.clone(),
            &fixture.source_choice_id,
            &fixture.source_choice_id,
            &fixture.source_label,
            requested_target(),
            None,
            false,
            0,
        ),
    );
    steps.push(DemoSceneStep::AdvanceWorker(DemoWorkerAdvance::Fail(
        EngineSelectionFailure::PresetUnavailable,
    )));
    steps.push(DemoSceneStep::AdvanceStructural);
    push_checkpoint(
        steps,
        DemoCheckpoint::preset(
            "preset.failure.preserved",
            patch.id(),
            EngineSelectionStatusKind::Unavailable,
            fixture.parameter_id.clone(),
            &fixture.source_choice_id,
            &fixture.source_choice_id,
            &fixture.source_label,
            requested_target(),
            Some(EngineSelectionFailure::PresetUnavailable),
            false,
            0,
        ),
    );

    push_single_adjustment(steps, WindowKey::D);
    push_structural_admission(steps);
    push_checkpoint(
        steps,
        DemoCheckpoint::preset(
            "preset.recovery.preparing",
            patch.id(),
            EngineSelectionStatusKind::Preparing,
            fixture.parameter_id.clone(),
            &fixture.source_choice_id,
            &fixture.source_choice_id,
            &fixture.source_label,
            requested_target(),
            None,
            false,
            0,
        ),
    );
    push_successful_preset_transition(
        steps,
        patch,
        fixture,
        "preset.recovery",
        &fixture.source_choice_id,
        &fixture.source_label,
        0,
        &fixture.target_choice_id,
        &fixture.target_label,
        requested_target(),
        1,
    );

    push_single_adjustment(steps, WindowKey::A);
    push_structural_admission(steps);
    push_checkpoint(
        steps,
        DemoCheckpoint::preset(
            "preset.final.preparing",
            patch.id(),
            EngineSelectionStatusKind::Preparing,
            fixture.parameter_id.clone(),
            &fixture.source_choice_id,
            &fixture.target_choice_id,
            &fixture.target_label,
            requested_source(),
            None,
            false,
            1,
        ),
    );
    push_successful_preset_transition(
        steps,
        patch,
        fixture,
        "preset.final",
        &fixture.target_choice_id,
        &fixture.target_label,
        1,
        &fixture.source_choice_id,
        &fixture.source_label,
        requested_source(),
        0,
    );

    steps.push(DemoSceneStep::PassiveAction(SemanticAction::Return));
    push_key_press(steps, WindowKey::Digit1);
    push_checkpoint(steps, DemoCheckpoint::new("preset.context.restored"));
}

#[allow(clippy::too_many_arguments)]
fn push_successful_preset_transition(
    steps: &mut Vec<DemoSceneStep>,
    patch: &Patch,
    fixture: &DemoPresetFixture,
    prefix: &str,
    active_choice_id: &str,
    active_label: &str,
    active_assignment_changes: usize,
    selected_choice_id: &str,
    selected_label: &str,
    requested_choice: Option<(String, String)>,
    expected_assignment_changes: usize,
) {
    steps.push(DemoSceneStep::AdvanceWorker(DemoWorkerAdvance::Healthy));
    steps.push(DemoSceneStep::AdvanceStructural);
    steps.push(DemoSceneStep::EngineProbe(
        DemoEngineProbe::MismatchedAcknowledgement,
    ));
    push_checkpoint(
        steps,
        DemoCheckpoint::after_rejection(
            format!("{prefix}.mismatchedAckRejected"),
            EventRejection::MismatchedEngineSelection,
        ),
    );
    push_checkpoint(
        steps,
        DemoCheckpoint::preset(
            format!("{prefix}.activating"),
            patch.id(),
            EngineSelectionStatusKind::Activating,
            fixture.parameter_id.clone(),
            &fixture.source_choice_id,
            active_choice_id,
            active_label,
            requested_choice,
            None,
            false,
            active_assignment_changes,
        ),
    );
    steps.push(DemoSceneStep::MidiProbe(MidiProbe::accepted(
        patch.id(),
        midi_message(patch.channel(), MidiMessageKind::NoteOn, 60, 110),
    )));
    steps.push(DemoSceneStep::Tick(TICK_DURATION));
    steps.push(DemoSceneStep::AdvanceStructural);
    push_checkpoint(
        steps,
        DemoCheckpoint::preset(
            format!("{prefix}.ready"),
            patch.id(),
            EngineSelectionStatusKind::Ready,
            fixture.parameter_id.clone(),
            &fixture.source_choice_id,
            selected_choice_id,
            selected_label,
            None,
            None,
            true,
            expected_assignment_changes,
        ),
    );
    steps.push(DemoSceneStep::MidiProbe(MidiProbe::accepted(
        patch.id(),
        MidiMessage::all_notes_off(patch.channel()),
    )));
}

fn push_patch_adsr_control_steps(
    steps: &mut Vec<DemoSceneStep>,
    capabilities: &CapabilityRegistry,
    patch: &Patch,
    boundary_probed: &mut BTreeSet<String>,
) {
    push_key_press(steps, WindowKey::Digit2);
    steps.push(DemoSceneStep::PassiveAction(SemanticAction::EnterSurface(
        SurfaceId::PatchDetail,
    )));
    let instrument = capabilities
        .descriptor_for_config(patch.instrument_config())
        .expect("validated envelope Patch capability is installed");
    for _ in instrument
        .parameters()
        .filter(|parameter| parameter_is_visible_and_enabled(parameter, patch.instrument_config()))
    {
        push_key_press(steps, WindowKey::S);
    }
    for (index, descriptor) in crate::synth::VoiceEnvelope::surface_descriptor()
        .iter()
        .enumerate()
    {
        let parameter = descriptor.parameter();
        let initial = patch.envelope().value(parameter);
        if index > 0 {
            push_key_press(steps, WindowKey::S);
        }
        if boundary_probed.insert(descriptor.name().to_owned()) {
            push_parameter_boundary_probe(
                steps,
                &format!("patch.{}.{}", patch.id().value(), descriptor.name()),
                initial,
                descriptor.minimum(),
                descriptor.maximum(),
                descriptor.fine_step(),
                descriptor.coarse_step(),
            );
        }
        push_reversible_adjustments(
            steps,
            initial,
            descriptor.minimum(),
            descriptor.maximum(),
            descriptor.coarse_step(),
        );
        push_checkpoint(
            steps,
            DemoCheckpoint::patch_adsr(
                format!("patch.control.{}.restored", descriptor.name()),
                patch.id(),
                parameter,
                initial,
                None,
            ),
        );
    }
    steps.push(DemoSceneStep::PassiveAction(SemanticAction::Return));
    push_key_press(steps, WindowKey::Digit1);
    push_checkpoint(steps, DemoCheckpoint::new("patch.control.contextRestored"));
}

fn push_patch_output_control_steps(
    steps: &mut Vec<DemoSceneStep>,
    patch: &Patch,
    boundary_probed: &mut BTreeSet<String>,
) {
    push_key_press(steps, WindowKey::Digit2);
    steps.push(DemoSceneStep::PassiveAction(SemanticAction::EnterSurface(
        SurfaceId::PatchUtility,
    )));

    let trim = PatchOutputParameter::TrimGain.descriptor();
    debug_assert_eq!(trim.kind(), PatchOutputParameterKind::Continuous);
    let trim_identifier = format!("patch.{}.output.{}", patch.id().value(), trim.name());
    if boundary_probed.insert(trim.name().to_owned()) {
        push_parameter_boundary_probe(
            steps,
            &trim_identifier,
            patch.output().trim_gain_db(),
            trim.minimum()
                .expect("the trim descriptor declares its lower bound"),
            trim.maximum()
                .expect("the trim descriptor declares its upper bound"),
            trim.fine_step()
                .expect("the trim descriptor declares its fine step"),
            trim.coarse_step()
                .expect("the trim descriptor declares its coarse step"),
        );
    }
    push_reversible_adjustments(
        steps,
        patch.output().trim_gain_db(),
        trim.minimum()
            .expect("the trim descriptor declares its lower bound"),
        trim.maximum()
            .expect("the trim descriptor declares its upper bound"),
        trim.coarse_step()
            .expect("the trim descriptor declares its coarse step"),
    );
    push_checkpoint(
        steps,
        DemoCheckpoint::new(format!("{trim_identifier}.restored")),
    );

    // Step down from the entry row to the output-track row by the distance the
    // declared Utility order actually puts between them, rather than assuming
    // they are adjacent. Entry focuses TrimGain, and the five-row order seats
    // MIDI input between the two.
    for _ in 0..utility_row_distance(
        &PatchControlId::Output(PatchOutputParameter::TrimGain),
        &PatchControlId::Output(PatchOutputParameter::OutputTrack),
    ) {
        push_key_press(steps, WindowKey::S);
    }
    let route = PatchOutputParameter::OutputTrack.descriptor();
    debug_assert_eq!(route.kind(), PatchOutputParameterKind::TrackChoice);
    let route_identifier = format!("patch.{}.output.{}", patch.id().value(), route.name());
    if boundary_probed.insert(route.name().to_owned()) {
        push_route_boundary_probe(steps, &route_identifier, patch.output().track_id());
    }
    push_reversible_route_adjustments(steps, patch.output().track_id());
    push_checkpoint(
        steps,
        DemoCheckpoint::new(format!("{route_identifier}.restored")),
    );

    push_patch_utility_scalar_steps(steps, patch);

    steps.push(DemoSceneStep::PassiveAction(SemanticAction::Return));
    push_key_press(steps, WindowKey::Digit1);
    push_checkpoint(steps, DemoCheckpoint::new("patch.output.contextRestored"));
}

/// Visits the three PATCH Utility rows the corrective gate added — master
/// volume, MIDI input, and voice limit — and reversibly edits each.
///
/// Every edit is immediately undone, so the scene leaves canonical state
/// exactly as it found it while still proving each row is reachable and
/// editable through the production reducer. Master volume is deliberately
/// included here: reaching the one canonical global value from PATCH is the
/// claim that would otherwise be provable only by reading the code.
fn push_patch_utility_scalar_steps(steps: &mut Vec<DemoSceneStep>, patch: &Patch) {
    // Master volume, upward from the output-track row.
    let master = PatchControlId::Global(GlobalParameter::MasterGainDb);
    for _ in 0..utility_row_distance(
        &master,
        &PatchControlId::Output(PatchOutputParameter::OutputTrack),
    ) {
        push_key_press(steps, WindowKey::W);
    }
    steps.push(DemoSceneStep::WindowInput(WindowInput::key_down(
        WindowKey::K,
    )));
    push_key_press(steps, WindowKey::D);
    push_key_press(steps, WindowKey::A);
    steps.push(DemoSceneStep::WindowInput(WindowInput::key_up(
        WindowKey::K,
    )));
    push_checkpoint(
        steps,
        DemoCheckpoint::new(format!("patch.{}.global.restored", patch.id().value())),
    );

    // MIDI input, then the voice limit, walking back down the declared order.
    for _ in 0..utility_row_distance(&master, &PatchControlId::MidiInput) {
        push_key_press(steps, WindowKey::S);
    }
    steps.push(DemoSceneStep::WindowInput(WindowInput::key_down(
        WindowKey::K,
    )));
    push_key_press(steps, WindowKey::D);
    push_key_press(steps, WindowKey::A);
    steps.push(DemoSceneStep::WindowInput(WindowInput::key_up(
        WindowKey::K,
    )));
    push_checkpoint(
        steps,
        DemoCheckpoint::new(format!("patch.{}.midiInput.restored", patch.id().value())),
    );

    for _ in 0..utility_row_distance(&PatchControlId::MidiInput, &PatchControlId::VoiceLimit) {
        push_key_press(steps, WindowKey::S);
    }
    steps.push(DemoSceneStep::WindowInput(WindowInput::key_down(
        WindowKey::K,
    )));
    push_key_press(steps, WindowKey::A);
    push_key_press(steps, WindowKey::D);
    push_topology_completion(steps, "voiceBudget.restored");
    steps.push(DemoSceneStep::WindowInput(WindowInput::key_up(
        WindowKey::K,
    )));
    push_checkpoint(
        steps,
        DemoCheckpoint::new(format!("patch.{}.voiceLimit.restored", patch.id().value())),
    );
}

/// Returns how many downward steps separate two rows of the declared PATCH
/// Utility order.
///
/// Derived from the one declared order, so inserting a row between two the
/// scene walks moves the scene with it instead of silently landing the probe
/// on a neighbour and asserting the wrong control's rejection.
fn utility_row_distance(from: &PatchControlId, to: &PatchControlId) -> usize {
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

fn push_route_boundary_probe(
    steps: &mut Vec<DemoSceneStep>,
    identifier: &str,
    initial: MixerTrackId,
) {
    push_checkpoint(
        steps,
        DemoCheckpoint::new(format!("boundary.{identifier}.start")),
    );
    steps.push(DemoSceneStep::WindowInput(WindowInput::key_down(
        WindowKey::K,
    )));

    for _ in initial.value()..MixerTrackId::MAX {
        push_key_press(steps, WindowKey::D);
    }
    push_key_press(steps, WindowKey::D);
    push_checkpoint(
        steps,
        DemoCheckpoint::after_rejection(
            format!("boundary.{identifier}.upper"),
            EventRejection::ParameterAtBoundary,
        ),
    );

    for _ in MixerTrackId::MIN..MixerTrackId::MAX {
        push_key_press(steps, WindowKey::A);
    }
    push_key_press(steps, WindowKey::A);
    push_checkpoint(
        steps,
        DemoCheckpoint::after_rejection(
            format!("boundary.{identifier}.lower"),
            EventRejection::ParameterAtBoundary,
        ),
    );

    for _ in MixerTrackId::MIN..initial.value() {
        push_key_press(steps, WindowKey::D);
    }
    steps.push(DemoSceneStep::WindowInput(WindowInput::key_up(
        WindowKey::K,
    )));
    push_checkpoint(
        steps,
        DemoCheckpoint::new(format!("boundary.{identifier}.restored")),
    );
}

fn push_reversible_route_adjustments(steps: &mut Vec<DemoSceneStep>, initial: MixerTrackId) {
    let (first, restore) = if initial.value() < MixerTrackId::MAX {
        (WindowKey::D, WindowKey::A)
    } else {
        (WindowKey::A, WindowKey::D)
    };
    steps.push(DemoSceneStep::WindowInput(WindowInput::key_down(
        WindowKey::K,
    )));
    push_key_press(steps, first);
    push_key_press(steps, restore);
    steps.push(DemoSceneStep::WindowInput(WindowInput::key_up(
        WindowKey::K,
    )));
}

fn push_mixer_control_steps(
    steps: &mut Vec<DemoSceneStep>,
    effects: &EffectCapabilityRegistry,
    global_parameters: &GlobalParameters,
    returns: &crate::mixer::bus_return::BusReturnBank,
    boundary_probed: &mut BTreeSet<String>,
) {
    let default_track = MixerTrackParameters::default();
    push_key_press(steps, WindowKey::Digit1);

    for (track_index, track_id) in MixerTrackId::ALL.into_iter().enumerate() {
        for (parameter_index, parameter) in MixerTrackParameter::MAIN.into_iter().enumerate() {
            push_mixer_track_parameter_steps(
                steps,
                track_id,
                parameter,
                &default_track,
                boundary_probed,
            );
            if parameter_index + 1 < MixerTrackParameter::MAIN.len() {
                push_key_press(steps, WindowKey::S);
            }
        }

        // The Inspector's first region is the selected track's indexed
        // sends in ascending BusId order; entry focuses Send(trackId, B0).
        steps.push(DemoSceneStep::PassiveAction(SemanticAction::EnterSurface(
            SurfaceId::MixerInspector,
        )));
        for (bus_index, bus) in crate::mixer::bus_id::BusId::ALL.into_iter().enumerate() {
            push_mixer_send_steps(
                steps,
                track_id,
                bus,
                default_track.send(bus),
                boundary_probed,
            );
            if bus_index + 1 < crate::mixer::bus_id::BusId::COUNT {
                push_key_press(steps, WindowKey::S);
            }
        }
        steps.push(DemoSceneStep::PassiveAction(SemanticAction::Return));

        for _ in 1..MixerTrackParameter::MAIN.len() {
            push_key_press(steps, WindowKey::W);
        }
        if track_index + 1 < MixerTrackId::COUNT {
            push_key_press(steps, WindowKey::D);
        }
    }

    for _ in MixerTrackId::MIN..MixerTrackId::MAX {
        push_key_press(steps, WindowKey::A);
    }
    steps.push(DemoSceneStep::PassiveAction(SemanticAction::EnterSurface(
        SurfaceId::MixerInspector,
    )));
    // Skip the sends; focus lands on the first return's occupancy row.
    for _ in crate::mixer::bus_id::BusId::ALL {
        push_key_press(steps, WindowKey::S);
    }
    // Each return contributes its occupancy row (traversed, not edited in the
    // frozen scalar scene), its return-owned level, and the occupying registry
    // entry's visible ScalarEdit rows.
    for bus in crate::mixer::bus_id::BusId::ALL {
        push_key_press(steps, WindowKey::S);
        let bus_return = returns.bus_return(bus);
        let level_identifier = format!("return.{bus}.returnLevel");
        if boundary_probed.insert("returnLevel".to_owned()) {
            push_parameter_boundary_probe(
                steps,
                &level_identifier,
                bus_return.return_level(),
                crate::mixer::bus_return::RETURN_LEVEL_DESCRIPTOR.minimum(),
                crate::mixer::bus_return::RETURN_LEVEL_DESCRIPTOR.maximum(),
                crate::mixer::bus_return::RETURN_LEVEL_DESCRIPTOR.fine_step(),
                crate::mixer::bus_return::RETURN_LEVEL_DESCRIPTOR.coarse_step(),
            );
        }
        push_reversible_adjustments(
            steps,
            bus_return.return_level(),
            crate::mixer::bus_return::RETURN_LEVEL_DESCRIPTOR.minimum(),
            crate::mixer::bus_return::RETURN_LEVEL_DESCRIPTOR.maximum(),
            crate::mixer::bus_return::RETURN_LEVEL_DESCRIPTOR.coarse_step(),
        );
        push_checkpoint(
            steps,
            DemoCheckpoint::new(format!("{level_identifier}.restored")),
        );
        let Some(config) = bus_return.effect() else {
            push_key_press(steps, WindowKey::S);
            continue;
        };
        let descriptor = effects
            .descriptor(config.capability_id())
            .expect("validated return occupancy capability is installed");
        for spec in descriptor.parameters().filter(|spec| {
            spec.patch_interaction() == PatchInteraction::ScalarEdit
                && spec.visible_when().is_none_or(|predicate| {
                    config.value(predicate.parameter_id()) == Some(predicate.equals())
                })
                && spec.enabled_when().is_none_or(|predicate| {
                    config.value(predicate.parameter_id()) == Some(predicate.equals())
                })
        }) {
            push_key_press(steps, WindowKey::S);
            let value = config
                .value(spec.id())
                .and_then(|value| spec.scalar_value(value).ok())
                .expect("validated return scalar has a finite value");
            let range = spec
                .range()
                .expect("the current effect scalar admission is explicitly bounded");
            let fine =
                spec.fine_step()
                    .expect("editable effect scalar declares a fine step") as f32;
            let coarse =
                spec.coarse_step()
                    .expect("editable effect scalar declares a coarse step") as f32;
            let identifier = format!("return.{bus}.{}", spec.id());
            if boundary_probed.insert(identifier.clone()) {
                push_parameter_boundary_probe(
                    steps,
                    &identifier,
                    value,
                    range.minimum() as f32,
                    range.maximum() as f32,
                    fine,
                    coarse,
                );
            }
            push_reversible_adjustments(
                steps,
                value,
                range.minimum() as f32,
                range.maximum() as f32,
                coarse,
            );
            push_checkpoint(steps, DemoCheckpoint::new(format!("{identifier}.restored")));
        }
        push_key_press(steps, WindowKey::S);
    }
    // Focus is on the single distinct global row: master gain.
    for descriptor in GlobalParameters::surface_descriptor() {
        let row_value = global_parameters.master_gain_db();
        let identifier = format!("global.{}", descriptor.name());
        if boundary_probed.insert(descriptor.name().to_owned()) {
            push_parameter_boundary_probe(
                steps,
                &identifier,
                row_value,
                descriptor.minimum(),
                descriptor.maximum(),
                descriptor.fine_step(),
                descriptor.coarse_step(),
            );
        }
        push_reversible_adjustments(
            steps,
            row_value,
            descriptor.minimum(),
            descriptor.maximum(),
            descriptor.coarse_step(),
        );
        push_checkpoint(steps, DemoCheckpoint::new(format!("{identifier}.restored")));
    }
    steps.push(DemoSceneStep::PassiveAction(SemanticAction::Return));
    push_checkpoint(steps, DemoCheckpoint::new("mixer.controls.restored"));
}

/// Exercises one indexed send row `(track, bus)` on the Inspector through the
/// one shared send descriptor.
fn push_mixer_send_steps(
    steps: &mut Vec<DemoSceneStep>,
    track_id: MixerTrackId,
    bus: crate::mixer::bus_id::BusId,
    initial: f32,
    boundary_probed: &mut BTreeSet<String>,
) {
    let descriptor = crate::mixer::mixer_track_parameters::BUS_SEND_DESCRIPTOR;
    let identifier = format!("track.{track_id}.sends[{}]", bus.index());
    if boundary_probed.insert("sends".to_owned()) {
        push_parameter_boundary_probe(
            steps,
            &identifier,
            initial,
            descriptor.minimum(),
            descriptor.maximum(),
            descriptor.fine_step(),
            descriptor.coarse_step(),
        );
    }
    push_reversible_adjustments(
        steps,
        initial,
        descriptor.minimum(),
        descriptor.maximum(),
        descriptor.coarse_step(),
    );
    push_checkpoint(steps, DemoCheckpoint::new(format!("{identifier}.restored")));
}

fn push_mixer_track_parameter_steps(
    steps: &mut Vec<DemoSceneStep>,
    track_id: MixerTrackId,
    parameter: MixerTrackParameter,
    initial_parameters: &MixerTrackParameters,
    boundary_probed: &mut BTreeSet<String>,
) {
    let descriptor = parameter.descriptor();
    let identifier = format!("track.{track_id}.{}", descriptor.name());
    match descriptor.kind() {
        MixerTrackParameterKind::Continuous => {
            let initial = initial_parameters
                .scalar_value(parameter)
                .expect("continuous track descriptors own scalar values");
            if boundary_probed.insert(descriptor.name().to_owned()) {
                push_parameter_boundary_probe(
                    steps,
                    &identifier,
                    initial,
                    descriptor.minimum(),
                    descriptor.maximum(),
                    descriptor.fine_step(),
                    descriptor.coarse_step(),
                );
            }
            push_reversible_adjustments(
                steps,
                initial,
                descriptor.minimum(),
                descriptor.maximum(),
                descriptor.coarse_step(),
            );
        }
        MixerTrackParameterKind::Toggle => {
            steps.push(DemoSceneStep::WindowInput(WindowInput::key_down(
                WindowKey::K,
            )));
            push_key_press(steps, WindowKey::D);
            push_key_press(steps, WindowKey::A);
            steps.push(DemoSceneStep::WindowInput(WindowInput::key_up(
                WindowKey::K,
            )));
        }
    }
    push_checkpoint(steps, DemoCheckpoint::new(format!("{identifier}.restored")));
}

/// Returns whether every occupied position of one Patch effect chain holds a
/// canonical registry configuration and a unique instance identity. Empty
/// positions are valid wherever they sit; the bounded slot array itself makes
/// a capacity violation unrepresentable.
fn effect_chain_is_valid(
    effects: &EffectCapabilityRegistry,
    slots: &[Option<PostEffectConfig>; MAX_EFFECT_SLOTS],
) -> bool {
    slots.iter().enumerate().all(|(position, occupancy)| {
        occupancy.as_ref().is_none_or(|config| {
            effects.validate_config(config).is_ok()
                && !slots[..position]
                    .iter()
                    .flatten()
                    .any(|prior| prior.slot_id() == config.slot_id())
        })
    })
}

fn push_patch_effect_control_steps(
    steps: &mut Vec<DemoSceneStep>,
    effects: &EffectCapabilityRegistry,
    patch: &Patch,
    boundary_probed: &mut BTreeSet<String>,
) {
    // Even a Patch with no configured effect visits its three occupancy rows:
    // every slot position stays reachable whether occupied or empty (FR-002).
    push_key_press(steps, WindowKey::Digit2);
    for (slot_index, occupancy) in crate::synth::effect_slot_id::EffectSlotIndex::ALL
        .into_iter()
        .zip(patch.effect_slots())
    {
        // PATCH Main contains exactly the engine row followed by the three
        // positional slots, so one bounded step reaches each next slot.
        push_key_press(steps, WindowKey::S);
        push_checkpoint(
            steps,
            DemoCheckpoint::new(format!("patch.effectSlot.{slot_index}.focused")),
        );
        let Some(config) = occupancy else {
            continue;
        };
        let descriptor = effects
            .descriptor(config.capability_id())
            .expect("validated effect scene capability is installed");
        let detail_parameters = descriptor
            .parameters()
            .filter(|parameter| parameter_is_visible_and_enabled(parameter, config))
            .collect::<Vec<_>>();
        if detail_parameters.is_empty() {
            continue;
        }
        steps.push(DemoSceneStep::PassiveAction(SemanticAction::EnterSurface(
            SurfaceId::PatchDetail,
        )));
        for (detail_index, spec) in detail_parameters.into_iter().enumerate() {
            if detail_index > 0 {
                push_key_press(steps, WindowKey::S);
            }
            if spec.patch_interaction() == PatchInteraction::ScalarEdit {
                let value = config
                    .value(spec.id())
                    .and_then(|value| spec.scalar_value(value).ok())
                    .expect("validated effect scalar has a finite value");
                let range = spec
                    .range()
                    .expect("the current effect scalar admission is explicitly bounded");
                let fine = spec
                    .fine_step()
                    .expect("editable effect scalar declares a fine step")
                    as f32;
                let coarse = spec
                    .coarse_step()
                    .expect("editable effect scalar declares a coarse step")
                    as f32;
                let identifier = format!(
                    "patch.{}.effect.{}.{}",
                    patch.id().value(),
                    config.slot_id().value(),
                    spec.id()
                );
                if boundary_probed.insert(identifier.clone()) {
                    push_parameter_boundary_probe(
                        steps,
                        &identifier,
                        value,
                        range.minimum() as f32,
                        range.maximum() as f32,
                        fine,
                        coarse,
                    );
                }
                push_reversible_adjustments(
                    steps,
                    value,
                    range.minimum() as f32,
                    range.maximum() as f32,
                    coarse,
                );
                push_checkpoint(steps, DemoCheckpoint::new(format!("{identifier}.restored")));
            }
        }
        steps.push(DemoSceneStep::PassiveAction(SemanticAction::Return));
    }
    for _ in crate::synth::effect_slot_id::EffectSlotIndex::ALL {
        push_key_press(steps, WindowKey::W);
    }
    push_key_press(steps, WindowKey::Digit1);
    push_checkpoint(steps, DemoCheckpoint::new("patch.effect.contextRestored"));
}

fn push_engine_selection_steps(
    steps: &mut Vec<DemoSceneStep>,
    capabilities: &CapabilityRegistry,
    patch: &Patch,
) {
    let soundfont = crate::synth::CapabilityId::new(
        crate::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID,
    )
    .expect("the production SoundFont id is valid");
    let braids =
        crate::synth::CapabilityId::new(crate::adapter::braids_capability::BRAIDS_CAPABILITY_ID)
            .expect("the production Braids id is valid");

    push_key_press(steps, WindowKey::Digit2);
    push_engine_adjustment(steps, WindowKey::A);
    push_checkpoint(
        steps,
        DemoCheckpoint::after_rejection(
            "engine.previous.unavailable",
            EventRejection::EngineSelectionUnavailable,
        ),
    );
    push_engine_adjustment(steps, WindowKey::D);
    push_structural_admission(steps);
    push_checkpoint(
        steps,
        DemoCheckpoint::engine(
            "engine.forward.preparing",
            EngineSelectionStatusKind::Preparing,
            soundfont.clone(),
            Some(braids.clone()),
            None,
            false,
        ),
    );
    push_engine_adjustment(steps, WindowKey::D);
    push_checkpoint(
        steps,
        DemoCheckpoint::after_rejection(
            "engine.forward.busyRejected",
            EventRejection::StructuralEditBusy,
        ),
    );
    steps.push(DemoSceneStep::EngineProbe(
        DemoEngineProbe::StaleWorkerFailure,
    ));
    push_checkpoint(
        steps,
        DemoCheckpoint::after_rejection(
            "engine.forward.staleRejected",
            EventRejection::StaleEngineSelection,
        ),
    );
    steps.push(DemoSceneStep::EngineProbe(
        DemoEngineProbe::EarlyAcknowledgement,
    ));
    push_checkpoint(
        steps,
        DemoCheckpoint::after_rejection(
            "engine.forward.earlyAckRejected",
            EventRejection::StaleEngineSelection,
        ),
    );
    let attack = VoiceEnvelopeParameter::AttackMilliseconds;
    let decay = VoiceEnvelopeParameter::DecayMilliseconds;
    let attack_initial = patch.envelope().value(attack);
    let decay_initial = patch.envelope().value(decay);
    let attack_edited = attack_initial + attack.descriptor().fine_step();
    let decay_edited = decay_initial + decay.descriptor().fine_step();
    steps.push(DemoSceneStep::PassiveAction(SemanticAction::EnterSurface(
        SurfaceId::PatchDetail,
    )));
    let soundfont_descriptor = capabilities
        .descriptor(&soundfont)
        .expect("the production SoundFont descriptor is installed");
    let braids_descriptor = capabilities
        .descriptor(&braids)
        .expect("the production Braids descriptor is installed");
    for _ in soundfont_descriptor
        .parameters()
        .filter(|parameter| parameter_is_visible_and_enabled(parameter, patch.instrument_config()))
    {
        push_key_press(steps, WindowKey::S);
    }
    push_single_adjustment(steps, WindowKey::D);
    push_checkpoint(
        steps,
        DemoCheckpoint::patch_adsr(
            "engine.forward.preparing.patchAdsr",
            patch.id(),
            attack,
            attack_edited,
            Some(EngineSelectionStatusKind::Preparing),
        ),
    );
    push_successful_engine_transition(
        steps,
        patch,
        "engine.forward",
        DemoEngineTransition {
            activating_capability_id: soundfont.clone(),
            active_capability_id: braids.clone(),
            requested_capability_id: Some(braids.clone()),
            worker: DemoWorkerAdvance::Healthy,
            activating_adsr: Some(DemoTransitionAdsrEdit {
                parameter: decay,
                before: decay_initial,
                after: decay_edited,
                detail_index: 1,
            }),
        },
    );

    // Activation acknowledgement closes the retained SoundFont Detail to its
    // exact Engine origin. Enter the newly committed Braids Detail, reach
    // Decay through its descriptor order, and restore both shared values.
    steps.push(DemoSceneStep::PassiveAction(SemanticAction::EnterSurface(
        SurfaceId::PatchDetail,
    )));
    for _ in 0..=braids_descriptor.parameters().count() {
        push_key_press(steps, WindowKey::S);
    }
    push_single_adjustment(steps, WindowKey::A);
    push_key_press(steps, WindowKey::W);
    push_single_adjustment(steps, WindowKey::A);
    push_checkpoint(
        steps,
        DemoCheckpoint::patch_adsr(
            "engine.forward.patchAdsrRestored",
            patch.id(),
            attack,
            attack_initial,
            Some(EngineSelectionStatusKind::Ready),
        ),
    );
    steps.push(DemoSceneStep::PassiveAction(SemanticAction::Return));

    push_engine_adjustment(steps, WindowKey::A);
    push_structural_admission(steps);
    push_checkpoint(
        steps,
        DemoCheckpoint::engine(
            "engine.reverse.preparing",
            EngineSelectionStatusKind::Preparing,
            braids.clone(),
            Some(soundfont.clone()),
            None,
            false,
        ),
    );
    push_successful_engine_transition(
        steps,
        patch,
        "engine.reverse",
        DemoEngineTransition {
            activating_capability_id: braids.clone(),
            active_capability_id: soundfont.clone(),
            requested_capability_id: Some(soundfont.clone()),
            worker: DemoWorkerAdvance::Healthy,
            activating_adsr: None,
        },
    );

    push_engine_adjustment(steps, WindowKey::D);
    push_structural_admission(steps);
    push_checkpoint(
        steps,
        DemoCheckpoint::engine(
            "engine.failure.preparing",
            EngineSelectionStatusKind::Preparing,
            soundfont.clone(),
            Some(braids.clone()),
            None,
            false,
        ),
    );
    steps.push(DemoSceneStep::AdvanceWorker(DemoWorkerAdvance::Fail(
        EngineSelectionFailure::AssetUnavailable,
    )));
    steps.push(DemoSceneStep::AdvanceStructural);
    push_checkpoint(
        steps,
        DemoCheckpoint::engine(
            "engine.failure.preserved",
            EngineSelectionStatusKind::Unavailable,
            soundfont.clone(),
            Some(braids.clone()),
            Some(EngineSelectionFailure::AssetUnavailable),
            false,
        ),
    );

    push_engine_adjustment(steps, WindowKey::D);
    push_structural_admission(steps);
    push_checkpoint(
        steps,
        DemoCheckpoint::engine(
            "engine.recovery.preparing",
            EngineSelectionStatusKind::Preparing,
            soundfont.clone(),
            Some(braids.clone()),
            None,
            false,
        ),
    );
    push_successful_engine_transition(
        steps,
        patch,
        "engine.recovery",
        DemoEngineTransition {
            activating_capability_id: soundfont.clone(),
            active_capability_id: braids.clone(),
            requested_capability_id: Some(braids.clone()),
            worker: DemoWorkerAdvance::Healthy,
            activating_adsr: None,
        },
    );

    push_engine_adjustment(steps, WindowKey::A);
    push_structural_admission(steps);
    push_checkpoint(
        steps,
        DemoCheckpoint::engine(
            "engine.final.preparing",
            EngineSelectionStatusKind::Preparing,
            braids.clone(),
            Some(soundfont.clone()),
            None,
            false,
        ),
    );
    push_successful_engine_transition(
        steps,
        patch,
        "engine.final",
        DemoEngineTransition {
            activating_capability_id: braids.clone(),
            active_capability_id: soundfont.clone(),
            requested_capability_id: Some(soundfont),
            worker: DemoWorkerAdvance::Healthy,
            activating_adsr: None,
        },
    );
    push_key_press(steps, WindowKey::Digit1);
    push_checkpoint(steps, DemoCheckpoint::new("engine.context.restored"));
}

fn parameter_is_visible_and_enabled<Config>(
    spec: &crate::synth::ParameterSpec,
    config: &Config,
) -> bool
where
    Config: ParameterValueLookup + ?Sized,
{
    [spec.visible_when(), spec.enabled_when()]
        .into_iter()
        .flatten()
        .all(|predicate| {
            config.parameter_value(predicate.parameter_id()) == Some(predicate.equals())
        })
}

trait ParameterValueLookup {
    fn parameter_value(&self, id: &ParameterId) -> Option<&ParameterValue>;
}

impl ParameterValueLookup for crate::synth::InstrumentConfig {
    fn parameter_value(&self, id: &ParameterId) -> Option<&ParameterValue> {
        self.value(id)
    }
}

impl ParameterValueLookup for PostEffectConfig {
    fn parameter_value(&self, id: &ParameterId) -> Option<&ParameterValue> {
        self.value(id)
    }
}

/// Exercises the four occupancy lifecycle events through the
/// canonical structural path — a refused request that mutates nothing, a
/// worker-refused preparation, post-rejection recovery, and (when the effect
/// registry has entries) a real slot occupancy change that is cleared again
/// so the scene ends at its structural baseline.
fn push_topology_occupancy_steps(
    steps: &mut Vec<DemoSceneStep>,
    effects: &EffectCapabilityRegistry,
    patch: &Patch,
) {
    let empty_bus = crate::mixer::bus_id::BusId::ALL[7];
    let open_slot = crate::synth::effect_slot_id::EffectSlotIndex::ALL[2];
    let absent_entry = crate::synth::EffectCapabilityId::new("effect.absent")
        .expect("the static absent id is namespaced");

    // A refused occupancy request: an unknown registry entry is rejected
    // atomically with a typed reason, and nothing enters the lifecycle.
    steps.push(DemoSceneStep::PassiveAction(
        SemanticAction::SetSlotOccupancy {
            patch_id: patch.id(),
            slot: open_slot,
            entry: Some(absent_entry),
        },
    ));
    push_checkpoint(
        steps,
        DemoCheckpoint::after_rejection(
            "topology.slot.unknownEntryRejected",
            EventRejection::InvalidEffectConfig,
        ),
    );

    // A worker-refused preparation: the reducer records the failure with its
    // reason and mutates nothing.
    steps.push(DemoSceneStep::PassiveAction(
        SemanticAction::SetReturnOccupancy {
            bus: empty_bus,
            entry: None,
        },
    ));
    push_structural_admission(steps);
    steps.push(DemoSceneStep::AdvanceWorker(DemoWorkerAdvance::Fail(
        EngineSelectionFailure::PreparationFailed,
    )));
    steps.push(DemoSceneStep::AdvanceStructural);
    push_checkpoint(
        steps,
        DemoCheckpoint::new("topology.return.preparationFailed"),
    );

    // Post-rejection recovery: the same change travels the canonical
    // prepare/stage/activate/acknowledge path.
    steps.push(DemoSceneStep::PassiveAction(
        SemanticAction::SetReturnOccupancy {
            bus: empty_bus,
            entry: None,
        },
    ));
    push_topology_completion(steps, "topology.return.cleared");

    // A real occupancy change when the registry has entries: occupy one open
    // Patch slot with the first registry entry, then clear it so the scene
    // ends at its structural baseline. The entry is positional — nothing in
    // the scene names an effect.
    if let Some(descriptor) = effects.descriptors().first() {
        steps.push(DemoSceneStep::PassiveAction(
            SemanticAction::SetSlotOccupancy {
                patch_id: patch.id(),
                slot: open_slot,
                entry: Some(descriptor.id().clone()),
            },
        ));
        push_topology_completion(steps, "topology.slot.occupied");
        steps.push(DemoSceneStep::PassiveAction(
            SemanticAction::SetSlotOccupancy {
                patch_id: patch.id(),
                slot: open_slot,
                entry: None,
            },
        ));
        push_topology_completion(steps, "topology.slot.restored");
    }
}

/// Drives one accepted occupancy request through prepare, stage, block-boundary
/// activation, and acknowledgement.
fn push_topology_completion(steps: &mut Vec<DemoSceneStep>, checkpoint: &str) {
    push_structural_admission(steps);
    steps.push(DemoSceneStep::AdvanceWorker(DemoWorkerAdvance::Healthy));
    steps.push(DemoSceneStep::AdvanceStructural);
    steps.push(DemoSceneStep::Tick(TICK_DURATION));
    steps.push(DemoSceneStep::AdvanceStructural);
    push_checkpoint(steps, DemoCheckpoint::new(checkpoint));
}

/// Advances one accepted structural request through the explicit off-thread
/// loading and validation gates before the preparation worker can receive it.
fn push_structural_admission(steps: &mut Vec<DemoSceneStep>) {
    steps.push(DemoSceneStep::AdvanceStructural);
    steps.push(DemoSceneStep::AdvanceStructural);
}

#[derive(Clone, Copy)]
struct DemoTransitionAdsrEdit {
    parameter: VoiceEnvelopeParameter,
    before: f32,
    after: f32,
    detail_index: usize,
}

struct DemoEngineTransition {
    activating_capability_id: crate::synth::CapabilityId,
    active_capability_id: crate::synth::CapabilityId,
    requested_capability_id: Option<crate::synth::CapabilityId>,
    worker: DemoWorkerAdvance,
    activating_adsr: Option<DemoTransitionAdsrEdit>,
}

fn push_successful_engine_transition(
    steps: &mut Vec<DemoSceneStep>,
    patch: &Patch,
    prefix: &str,
    transition: DemoEngineTransition,
) {
    steps.push(DemoSceneStep::AdvanceWorker(transition.worker));
    steps.push(DemoSceneStep::AdvanceStructural);
    steps.push(DemoSceneStep::EngineProbe(
        DemoEngineProbe::MismatchedAcknowledgement,
    ));
    push_checkpoint(
        steps,
        DemoCheckpoint::after_rejection(
            format!("{prefix}.mismatchedAckRejected"),
            EventRejection::MismatchedEngineSelection,
        ),
    );
    push_checkpoint(
        steps,
        DemoCheckpoint::engine(
            format!("{prefix}.activating"),
            EngineSelectionStatusKind::Activating,
            transition.activating_capability_id,
            transition.requested_capability_id,
            None,
            false,
        ),
    );
    if let Some(edit) = transition.activating_adsr {
        // The old subject remains open until acknowledgement; move within its
        // stable Detail order instead of trying to enter a second surface.
        for _ in 0..edit.detail_index {
            push_key_press(steps, WindowKey::S);
        }
        push_single_adjustment(steps, WindowKey::D);
        debug_assert_eq!(
            edit.after,
            edit.before + edit.parameter.descriptor().fine_step()
        );
        push_checkpoint(
            steps,
            DemoCheckpoint::patch_adsr(
                format!("{prefix}.activating.patchAdsr"),
                patch.id(),
                edit.parameter,
                edit.after,
                Some(EngineSelectionStatusKind::Activating),
            ),
        );
    }
    steps.push(DemoSceneStep::MidiProbe(MidiProbe::accepted(
        patch.id(),
        midi_message(patch.channel(), MidiMessageKind::NoteOn, 60, 110),
    )));
    steps.push(DemoSceneStep::Tick(TICK_DURATION));
    steps.push(DemoSceneStep::AdvanceStructural);
    push_checkpoint(
        steps,
        DemoCheckpoint::engine(
            format!("{prefix}.ready"),
            EngineSelectionStatusKind::Ready,
            transition.active_capability_id,
            None,
            None,
            true,
        ),
    );
    steps.push(DemoSceneStep::MidiProbe(MidiProbe::accepted(
        patch.id(),
        MidiMessage::all_notes_off(patch.channel()),
    )));
}

fn push_single_adjustment(steps: &mut Vec<DemoSceneStep>, key: WindowKey) {
    steps.push(DemoSceneStep::WindowInput(WindowInput::key_down(
        WindowKey::K,
    )));
    push_key_press(steps, key);
    steps.push(DemoSceneStep::WindowInput(WindowInput::key_up(
        WindowKey::K,
    )));
}

fn push_engine_adjustment(steps: &mut Vec<DemoSceneStep>, key: WindowKey) {
    steps.push(DemoSceneStep::WindowInput(WindowInput::key_down(
        WindowKey::K,
    )));
    push_key_press(steps, key);
    steps.push(DemoSceneStep::WindowInput(WindowInput::key_up(
        WindowKey::K,
    )));
}

fn complete_window_vocabulary() -> &'static [WindowInput] {
    WindowInput::surface_descriptor()
}

fn push_key_press(steps: &mut Vec<DemoSceneStep>, key: WindowKey) {
    steps.push(DemoSceneStep::WindowInput(WindowInput::key_down(key)));
    steps.push(DemoSceneStep::WindowInput(WindowInput::key_up(key)));
}

fn push_reversible_adjustments(
    steps: &mut Vec<DemoSceneStep>,
    initial: f32,
    minimum: f32,
    maximum: f32,
    coarse_step: f32,
) {
    steps.push(DemoSceneStep::WindowInput(WindowInput::key_down(
        WindowKey::K,
    )));
    let can_increase = initial + coarse_step <= maximum || initial <= minimum;
    let (fine_first, fine_restore, coarse_first, coarse_restore) = if can_increase {
        (WindowKey::D, WindowKey::A, WindowKey::W, WindowKey::S)
    } else {
        (WindowKey::A, WindowKey::D, WindowKey::S, WindowKey::W)
    };
    push_key_press(steps, fine_first);
    push_key_press(steps, fine_restore);
    push_key_press(steps, coarse_first);
    push_key_press(steps, coarse_restore);
    steps.push(DemoSceneStep::WindowInput(WindowInput::key_up(
        WindowKey::K,
    )));
}

#[allow(clippy::too_many_arguments)]
fn push_parameter_boundary_probe(
    steps: &mut Vec<DemoSceneStep>,
    identifier: &str,
    initial: f32,
    minimum: f32,
    maximum: f32,
    fine_step: f32,
    coarse_step: f32,
) {
    push_checkpoint(
        steps,
        DemoCheckpoint::new(format!("boundary.{identifier}.start")),
    );
    steps.push(DemoSceneStep::WindowInput(WindowInput::key_down(
        WindowKey::K,
    )));

    for _ in 0..steps_to_boundary(initial, maximum, coarse_step) {
        push_key_press(steps, WindowKey::W);
    }
    push_key_press(steps, WindowKey::W);
    push_checkpoint(
        steps,
        DemoCheckpoint::after_rejection(
            format!("boundary.{identifier}.upper"),
            EventRejection::ParameterAtBoundary,
        ),
    );

    for _ in 0..steps_to_boundary(maximum, minimum, coarse_step) {
        push_key_press(steps, WindowKey::S);
    }
    push_key_press(steps, WindowKey::S);
    push_checkpoint(
        steps,
        DemoCheckpoint::after_rejection(
            format!("boundary.{identifier}.lower"),
            EventRejection::ParameterAtBoundary,
        ),
    );

    let (coarse_restoration_steps, fine_restoration_steps) =
        restoration_steps(minimum, initial, fine_step, coarse_step);
    for _ in 0..coarse_restoration_steps {
        push_key_press(steps, WindowKey::W);
    }
    for _ in 0..fine_restoration_steps {
        push_key_press(steps, WindowKey::D);
    }

    steps.push(DemoSceneStep::WindowInput(WindowInput::key_up(
        WindowKey::K,
    )));
    push_checkpoint(
        steps,
        DemoCheckpoint::new(format!("boundary.{identifier}.restored")),
    );
}

fn steps_to_boundary(from: f32, to: f32, step: f32) -> usize {
    (((to - from).abs() / step).ceil()) as usize
}

fn restoration_steps(from: f32, to: f32, fine_step: f32, coarse_step: f32) -> (usize, usize) {
    let fine_units = ((to - from).abs() / fine_step).round() as usize;
    let coarse_units = (coarse_step / fine_step).round() as usize;
    (fine_units / coarse_units, fine_units % coarse_units)
}

fn push_checkpoint(steps: &mut Vec<DemoSceneStep>, checkpoint: DemoCheckpoint) {
    steps.push(DemoSceneStep::Checkpoint(Box::new(checkpoint)));
}

fn midi_messages(channel: MidiChannel, kinds: &[MidiMessageKind]) -> Vec<MidiMessage> {
    kinds
        .iter()
        .map(|kind| match kind {
            MidiMessageKind::NoteOn => midi_message(channel, *kind, 60, 96),
            MidiMessageKind::NoteOff => midi_message(channel, *kind, 60, 0),
            MidiMessageKind::ControlChange => midi_message(channel, *kind, 1, 64),
            MidiMessageKind::ProgramChange => midi_message(channel, *kind, 10, 0),
            MidiMessageKind::ChannelPressure => midi_message(channel, *kind, 70, 0),
            MidiMessageKind::PitchBend => midi_message(channel, *kind, 0, 64),
            MidiMessageKind::AllNotesOff => MidiMessage::all_notes_off(channel),
            MidiMessageKind::PolyPressure => midi_message(channel, *kind, 60, 70),
        })
        .collect()
}

fn midi_message(channel: MidiChannel, kind: MidiMessageKind, data1: u8, data2: u8) -> MidiMessage {
    MidiMessage::try_new(channel, kind, data1, data2)
        .expect("the declared demo MIDI bytes are all seven-bit values")
}

fn first_unknown_patch_id(patches: &[Patch]) -> PatchId {
    let candidate = (1..=u32::MAX)
        .find(|candidate| !patches.iter().any(|patch| patch.id().value() == *candidate))
        .expect("a bounded installed Patch list cannot occupy every PatchId");
    PatchId::new(candidate).expect("the candidate search starts at one")
}

fn build_expected_coverage(
    capabilities: &CapabilityRegistry,
    effects: &EffectCapabilityRegistry,
    patches: &[Patch],
    returns: &crate::mixer::bus_return::BusReturnBank,
) -> Vec<String> {
    let mut expected = Vec::new();
    for descriptor in AppEvent::surface_descriptor() {
        match descriptor {
            crate::control::app_event::AppEventSurfaceDescriptor::Send { .. } => {
                // The physical digit sweep opens Sends; chain edits have dedicated witnesses.
                expected.push("event.send".to_owned());
            }
            crate::control::app_event::AppEventSurfaceDescriptor::SelectContext { context } => {
                expected.push("event.selectContext".to_owned());
                expected.push(format!("context.{}", context.label().to_ascii_lowercase()));
            }
            crate::control::app_event::AppEventSurfaceDescriptor::SelectPatch { direction } => {
                expected.push("event.selectPatch".to_owned());
                expected.push(format!("direction.{}", direction_identifier(*direction)));
            }
            crate::control::app_event::AppEventSurfaceDescriptor::NavigatePage { direction } => {
                expected.push("event.navigatePage".to_owned());
                expected.push(format!("direction.{}", direction_identifier(*direction)));
            }
            crate::control::app_event::AppEventSurfaceDescriptor::Navigate { direction } => {
                expected.push("event.navigate".to_owned());
                expected.push(format!("direction.{}", direction_identifier(*direction)));
            }
            crate::control::app_event::AppEventSurfaceDescriptor::Adjust { direction } => {
                expected.push("event.adjust".to_owned());
                expected.push(format!("direction.{}", direction_identifier(*direction)));
            }
            crate::control::app_event::AppEventSurfaceDescriptor::SetInteractionMode { mode } => {
                expected.push("event.setInteractionMode".to_owned());
                expected.push(format!("interactionMode.{}", mode.label().to_ascii_lowercase()));
            }
            crate::control::app_event::AppEventSurfaceDescriptor::OpenRelated => {
                expected.push("event.openRelated".to_owned());
            }
            crate::control::app_event::AppEventSurfaceDescriptor::OpenMidiSettings => {
                expected.push("event.openMidiSettings".to_owned());
            }
            crate::control::app_event::AppEventSurfaceDescriptor::Activate => {
                expected.push("event.activate".to_owned());
            }
            crate::control::app_event::AppEventSurfaceDescriptor::PreviewStart => {
                expected.push("event.previewStart".to_owned());
            }
            crate::control::app_event::AppEventSurfaceDescriptor::PreviewStop => {
                expected.push("event.previewStop".to_owned());
            }
            crate::control::app_event::AppEventSurfaceDescriptor::EnterSurface { surface } => {
                expected.push("event.enterSurface".to_owned());
                // Only an *admitted* surface can be exercised by a scene step:
                // expecting coverage the action vocabulary cannot produce would
                // make the scene permanently incomplete. `SurfaceId::is_enterable`
                // is the one predicate that decides admission, so this
                // expectation follows the vocabulary automatically rather than
                // being maintained beside it.
                if surface.is_enterable() {
                    expected.push(format!("surface.{}", surface.label().to_ascii_lowercase()));
                }
            }
            crate::control::app_event::AppEventSurfaceDescriptor::Return => {
                expected.push("event.return".to_owned());
            }
            crate::control::app_event::AppEventSurfaceDescriptor::InstallPatches { .. } => {
                expected.push("event.installPatches".to_owned());
            }
            crate::control::app_event::AppEventSurfaceDescriptor::ReplacePersistedSession {
                ..
            } => {}
            crate::control::app_event::AppEventSurfaceDescriptor::Midi { .. } => {
                expected.push("event.midi".to_owned());
            }
            crate::control::app_event::AppEventSurfaceDescriptor::SetPatchOverviewOriginEnabled {
                ..
            } => {}
            crate::control::app_event::AppEventSurfaceDescriptor::EngineSelectionLifecycleAdvanced {
                ..
            } => {
                expected.push("event.engineSelectionLifecycleAdvanced".to_owned());
            }
            crate::control::app_event::AppEventSurfaceDescriptor::EnginePrepared { .. } => {
                expected.push("event.enginePrepared".to_owned());
            }
            crate::control::app_event::AppEventSurfaceDescriptor::SampleAssetLifecycleAdvanced {
                ..
            } => {}
            crate::control::app_event::AppEventSurfaceDescriptor::FileCatalogRefreshed {
                ..
            } => {}
            crate::control::app_event::AppEventSurfaceDescriptor::EnginePreparationFailed {
                ..
            } => {
                expected.push("event.enginePreparationFailed".to_owned());
            }
            crate::control::app_event::AppEventSurfaceDescriptor::EngineActivationAcknowledged {
                ..
            } => {
                expected.push("event.engineActivationAcknowledged".to_owned());
            }
            crate::control::app_event::AppEventSurfaceDescriptor::SetSlotOccupancy { .. } => {
                expected.push("event.setSlotOccupancy".to_owned());
            }
            crate::control::app_event::AppEventSurfaceDescriptor::SetReturnOccupancy { .. } => {
                expected.push("event.setReturnOccupancy".to_owned());
            }
            crate::control::app_event::AppEventSurfaceDescriptor::TopologyPrepared { .. } => {
                expected.push("event.topologyPrepared".to_owned());
            }
            crate::control::app_event::AppEventSurfaceDescriptor::TopologyPreparationFailed {
                ..
            } => {
                expected.push("event.topologyPreparationFailed".to_owned());
            }
            crate::control::app_event::AppEventSurfaceDescriptor::MidiInputPreferenceRestored {
                ..
            }
            | crate::control::app_event::AppEventSurfaceDescriptor::MidiInputPreferenceStoreFailed {
                ..
            }
            | crate::control::app_event::AppEventSurfaceDescriptor::MidiInputScanSucceeded {
                ..
            }
            | crate::control::app_event::AppEventSurfaceDescriptor::MidiInputScanFailed { .. }
            | crate::control::app_event::AppEventSurfaceDescriptor::MidiInputConnectRequested {
                ..
            }
            | crate::control::app_event::AppEventSurfaceDescriptor::MidiInputConnectionPrepared {
                ..
            }
            | crate::control::app_event::AppEventSurfaceDescriptor::MidiInputActivationAcknowledged {
                ..
            }
            | crate::control::app_event::AppEventSurfaceDescriptor::MidiInputDisconnectRequested {
                ..
            }
            | crate::control::app_event::AppEventSurfaceDescriptor::MidiInputConnectionLost {
                ..
            }
            | crate::control::app_event::AppEventSurfaceDescriptor::MidiInputOperationFailed {
                ..
            }
            | crate::control::app_event::AppEventSurfaceDescriptor::MidiInputScanStarted
            | crate::control::app_event::AppEventSurfaceDescriptor::Controller { .. }
            | crate::control::app_event::AppEventSurfaceDescriptor::MidiInputShutdownRequested
            | crate::control::app_event::AppEventSurfaceDescriptor::AssetImported { .. }
            | crate::control::app_event::AppEventSurfaceDescriptor::ToggleTestMidi => {
            }
        }
    }

    if patches
        .iter()
        .any(|patch| patch.effect_slots().iter().any(Option::is_some))
    {
        expected.extend(
            [
                "effect.patchEffect.targetExact",
                "effect.patchEffect.differenceNonzero",
                "effect.patchEffect.sideNonzero",
                "effect.patchEffect.beforeMixStemExact",
                "effect.patchEffect.unconfiguredIsolation",
                "effect.patchEffect.structuralPreservation",
            ]
            .into_iter()
            .map(str::to_owned),
        );
    }

    expected.extend(
        complete_window_vocabulary()
            .iter()
            .map(|input| format!("input.{}", window_input_identifier(*input))),
    );

    expected.extend(
        Direction::ALL
            .into_iter()
            .map(|direction| format!("direction.{}", direction_identifier(direction))),
    );

    if let Some(patch) = patches.first() {
        let descriptor = capabilities
            .descriptor_for_config(patch.instrument_config())
            .expect("validated coverage Patch capability is installed");
        expected.extend(
            PatchControlId::resolve(
                descriptor,
                patch.instrument_config(),
                effects,
                patch.effect_slots(),
            )
            .into_iter()
            .map(|control| format!("patchControl.{control}")),
        );
        expected.extend(
            PatchControlId::utility_surface_descriptor()
                .iter()
                .map(|control| format!("patchControl.{control}")),
        );
    }

    if let Some(patch) = patches.first() {
        let descriptor = capabilities
            .descriptor_for_config(patch.instrument_config())
            .expect("validated MIDI coverage capability is installed");
        expected.extend(
            midi_messages(patch.channel(), descriptor.supported_midi_kinds())
                .into_iter()
                .map(|message| format!("midi.{}", midi_kind_identifier(message.kind()))),
        );
    }

    if let Some(patch) = patches.first() {
        let descriptor = capabilities
            .descriptor_for_config(patch.instrument_config())
            .expect("validated coverage Patch capability is installed");
        let targets = patch
            .editable_targets(descriptor)
            .expect("validated coverage Patch has a canonical editable surface");
        for target in targets {
            let parameter = target.name();
            expected.push(format!(
                "parameter.patch.{}.{parameter}",
                patch.id().value()
            ));
            expected.push(format!(
                "effect.parameterSnapshot.patch.{}.{parameter}",
                patch.id().value()
            ));
            match target {
                PatchEditableTarget::Envelope(_) => {
                    expected.push(format!(
                        "property.stateTree.patch.{}.envelope.{parameter}",
                        patch.id().value()
                    ));
                    expected.push(format!(
                        "property.stateTree.parameters.patch.{}.envelope.{parameter}",
                        patch.id().value()
                    ));
                }
                PatchEditableTarget::Instrument(_) => {
                    expected.push(format!(
                        "property.stateTree.parameters.patch.{}.instrument.count",
                        patch.id().value()
                    ));
                    expected.push(format!(
                        "property.stateTree.parameters.patch.{}.instrument.values",
                        patch.id().value()
                    ));
                }
            }
        }
        for parameter in PatchOutputParameter::ALL {
            let name = parameter.name();
            expected.push(format!(
                "parameter.patch.{}.output.{name}",
                patch.id().value()
            ));
            expected.push(format!(
                "effect.parameterSnapshot.patch.{}.output.{name}",
                patch.id().value()
            ));
            expected.push(format!(
                "property.stateTree.patch.{}.output.{}",
                patch.id().value(),
                match parameter {
                    PatchOutputParameter::TrimGain => "trimGainDb",
                    PatchOutputParameter::OutputTrack => "trackId",
                }
            ));
            expected.push(format!(
                "property.stateTree.parameters.patch.{}.output.{}",
                patch.id().value(),
                match parameter {
                    PatchOutputParameter::TrimGain => "trimGainDb",
                    PatchOutputParameter::OutputTrack => "trackId",
                }
            ));
        }
    }
    for track_id in MixerTrackId::ALL {
        for parameter in MixerTrackParameter::MAIN {
            expected.push(format!("parameter.track.{track_id}.{parameter}"));
            expected.push(format!(
                "effect.parameterSnapshot.track.{track_id}.{parameter}"
            ));
        }
        for bus in crate::mixer::bus_id::BusId::ALL {
            expected.push(format!("parameter.track.{track_id}.sends[{}]", bus.index()));
            expected.push(format!(
                "effect.parameterSnapshot.track.{track_id}.sends[{}]",
                bus.index()
            ));
        }
    }
    for patch in patches {
        for property in ["id", "name", "channel", "instrument.capabilityId"] {
            expected.push(format!(
                "property.stateTree.patch.{}.{property}",
                patch.id().value()
            ));
        }
        for assignment in patch.instrument_config().values() {
            for property in ["parameterId", "value.kind", "value.value"] {
                expected.push(format!(
                    "property.stateTree.patch.{}.instrument.value.{}.{property}",
                    patch.id().value(),
                    assignment.parameter_id()
                ));
            }
        }
        for assignment in patch.instrument_config().asset_references() {
            for property in ["parameterId", "reference.kind", "reference.locator"] {
                expected.push(format!(
                    "property.stateTree.patch.{}.instrument.asset.{}.{property}",
                    patch.id().value(),
                    assignment.parameter_id()
                ));
            }
        }
        for config in patch.effect_slots().iter().flatten() {
            for property in ["slotId", "capabilityId"] {
                expected.push(format!(
                    "property.stateTree.patch.{}.postEffect.{}.{property}",
                    patch.id().value(),
                    config.slot_id().value()
                ));
            }
            for assignment in config.values() {
                let parameter = assignment.parameter_id();
                expected.push(format!(
                    "parameter.patch.{}.effect.{}.{}",
                    patch.id().value(),
                    config.slot_id().value(),
                    parameter
                ));
                expected.push(format!(
                    "effect.parameterSnapshot.patch.{}.effect.{}.{}",
                    patch.id().value(),
                    config.slot_id().value(),
                    parameter
                ));
                for property in ["parameterId", "value.kind", "value.value"] {
                    expected.push(format!(
                        "property.stateTree.patch.{}.postEffect.{}.value.{}.{property}",
                        patch.id().value(),
                        config.slot_id().value(),
                        parameter
                    ));
                }
            }
            for assignment in config.asset_references() {
                for property in ["parameterId", "reference.kind", "reference.locator"] {
                    expected.push(format!(
                        "property.stateTree.patch.{}.postEffect.{}.asset.{}.{property}",
                        patch.id().value(),
                        config.slot_id().value(),
                        assignment.parameter_id()
                    ));
                }
            }
            for property in ["active", "slotId", "scalarCount", "scalars"] {
                expected.push(format!(
                    "property.stateTree.parameters.patch.{}.effect.{property}",
                    patch.id().value()
                ));
            }
        }
    }

    for descriptor in capabilities.descriptors() {
        for property in [
            "id",
            "label",
            "semanticAccent",
            "voicePolicy",
            "supportedMidiKinds",
        ] {
            expected.push(format!(
                "property.stateTree.capability.{}.{property}",
                descriptor.id()
            ));
        }
        for section in descriptor.sections() {
            for property in ["id", "label"] {
                expected.push(format!(
                    "property.stateTree.capability.{}.section.{}.{property}",
                    descriptor.id(),
                    section.id()
                ));
            }
            for parameter in section.parameters() {
                for property in [
                    "id",
                    "label",
                    "kind",
                    "update",
                    "patchInteraction",
                    "defaultValue",
                    "range",
                    "choices",
                    "fineStep",
                    "coarseStep",
                    "unit",
                    "formatter",
                    "enabledWhen",
                    "visibleWhen",
                ] {
                    expected.push(format!(
                        "property.stateTree.capability.{}.parameter.{}.{property}",
                        descriptor.id(),
                        parameter.id()
                    ));
                }
                if parameter.patch_interaction() == crate::synth::PatchInteraction::StructuralChoice
                {
                    for choice in parameter.choices() {
                        expected.push(format!(
                            "projection.patch.choice.{}.{}={}",
                            parameter.id(),
                            choice.id(),
                            choice.label()
                        ));
                    }
                }
            }
        }
        for requirement in descriptor.asset_requirements() {
            for property in ["parameterId", "required"] {
                expected.push(format!(
                    "property.stateTree.capability.{}.asset.{}.{property}",
                    descriptor.id(),
                    requirement.parameter_id()
                ));
            }
        }
    }

    for descriptor in effects.descriptors() {
        for property in ["id", "label", "semanticAccent"] {
            expected.push(format!(
                "property.stateTree.effectCapability.{}.{property}",
                descriptor.id()
            ));
        }
        for section in descriptor.sections() {
            for property in ["id", "label"] {
                expected.push(format!(
                    "property.stateTree.effectCapability.{}.section.{}.{property}",
                    descriptor.id(),
                    section.id()
                ));
            }
            for parameter in section.parameters() {
                for property in [
                    "id",
                    "label",
                    "kind",
                    "update",
                    "patchInteraction",
                    "defaultValue",
                    "range",
                    "choices",
                    "fineStep",
                    "coarseStep",
                    "unit",
                    "formatter",
                    "enabledWhen",
                    "visibleWhen",
                ] {
                    expected.push(format!(
                        "property.stateTree.effectCapability.{}.parameter.{}.{property}",
                        descriptor.id(),
                        parameter.id()
                    ));
                }
            }
        }
        for requirement in descriptor.asset_requirements() {
            for property in ["parameterId", "required"] {
                expected.push(format!(
                    "property.stateTree.effectCapability.{}.asset.{}.{property}",
                    descriptor.id(),
                    requirement.parameter_id()
                ));
            }
        }
    }

    for descriptor in GlobalParameters::surface_descriptor() {
        let parameter = descriptor.name();
        expected.push(format!("parameter.global.{parameter}"));
        expected.push(format!("effect.parameterSnapshot.global.{parameter}"));
    }
    // Every return contributes its return-owned level plus the occupying
    // registry entry's visible ScalarEdit rows, addressed by BusId.
    for bus in crate::mixer::bus_id::BusId::ALL {
        expected.push(format!("parameter.return.{bus}.returnLevel"));
        expected.push(format!("effect.parameterSnapshot.return.{bus}.returnLevel"));
        let Some(config) = returns.bus_return(bus).effect() else {
            continue;
        };
        let descriptor = effects
            .descriptor(config.capability_id())
            .expect("validated return occupancy capability is installed");
        for spec in descriptor.parameters().filter(|spec| {
            spec.patch_interaction() == PatchInteraction::ScalarEdit
                && spec.visible_when().is_none_or(|predicate| {
                    config.value(predicate.parameter_id()) == Some(predicate.equals())
                })
                && spec.enabled_when().is_none_or(|predicate| {
                    config.value(predicate.parameter_id()) == Some(predicate.equals())
                })
        }) {
            expected.push(format!("parameter.return.{bus}.{}", spec.id()));
            expected.push(format!(
                "effect.parameterSnapshot.return.{bus}.{}",
                spec.id()
            ));
        }
    }
    // The six retired reverb/delay `global` leaves are gone from the tree;
    // return-owned state serializes as the indexed `returns` section.
    expected.push("property.stateTree.global.masterGainDb".to_owned());
    expected.push("property.stateTree.returns.effect".to_owned());
    expected.push("property.stateTree.returns.returnLevel".to_owned());
    // The snapshot's global object keeps only master gain: the six
    // retired reverb/delay leaves no longer exist to be covered; their values
    // are the indexed `parameters.returns` entries, whose array property is
    // covered through the StateTree property descriptor below.
    expected.push("property.stateTree.parameters.global.masterGainDb".to_owned());

    expected.extend(
        EventLog::serialized_property_descriptor()
            .iter()
            .map(|property| format!("property.eventLog.{property}")),
    );
    expected.extend(
        EventRecord::serialized_property_descriptor()
            .iter()
            .map(|property| format!("property.eventRecord.{property}")),
    );
    expected.extend(
        StateTree::serialized_property_descriptor()
            .iter()
            .map(|property| format!("property.stateTree.{property}")),
    );
    expected.extend(
        TextProjection::serialized_property_descriptor()
            .iter()
            .map(|property| format!("property.textProjection.{property}")),
    );
    expected.extend(
        EventRejection::surface_descriptor()
            .iter()
            .map(|descriptor| format!("rejection.{}", descriptor.name())),
    );
    expected.extend(EMITTED_EFFECTS.into_iter().map(str::to_owned));
    expected.extend(
        crate::control::EngineSelectionEffectKind::surface_descriptor()
            .iter()
            .map(|kind| format!("effect.emitted.engineSelection.{}", kind.name())),
    );
    expected.extend([
        "projection.structuralIntent.replaceCapability".to_owned(),
        "projection.structuralIntent.replaceParameterChoice".to_owned(),
    ]);

    expected.sort_unstable();
    expected.dedup();
    expected
}

const EMITTED_EFFECTS: [&str; 4] = [
    "effect.emitted.stateAccepted",
    "effect.emitted.parameterSnapshotPublished",
    "effect.emitted.audioCommand.patchMidi",
    "effect.emitted.audioCommand.allNotesOff",
];

fn direction_identifier(direction: Direction) -> &'static str {
    match direction {
        Direction::Up => "up",
        Direction::Down => "down",
        Direction::Left => "left",
        Direction::Right => "right",
    }
}

fn midi_kind_identifier(kind: MidiMessageKind) -> &'static str {
    match kind {
        MidiMessageKind::NoteOn => "noteOn",
        MidiMessageKind::NoteOff => "noteOff",
        MidiMessageKind::ControlChange => "controlChange",
        MidiMessageKind::ProgramChange => "programChange",
        MidiMessageKind::ChannelPressure => "channelPressure",
        MidiMessageKind::PitchBend => "pitchBend",
        MidiMessageKind::AllNotesOff => "allNotesOff",
        MidiMessageKind::PolyPressure => "polyPressure",
    }
}

fn window_input_identifier(input: WindowInput) -> &'static str {
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
    use super::{DemoScene, DemoSceneError, DemoSceneStep, MidiProbe};
    use crate::control::app_state::EventRejection;
    use crate::kernel::midi_channel::MidiChannel;
    use crate::kernel::patch_id::PatchId;
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::mixer_track_parameters::{MixerTrackParameter, MixerTrackParameters};
    use crate::mixer::patch_output::PatchOutput;
    use crate::shell::window_input::{WindowInput, WindowInputKind, WindowKey};
    use crate::synth::patch::Patch;
    use crate::synth::sound_font_instrument::SoundFontInstrument;
    use crate::synth::voice_envelope::VoiceEnvelope;
    use crate::testing::automatic_midi_test::create_soundfont_config;

    fn patch(id: u32, channel: u8) -> Patch {
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        Patch::new(
            PatchId::new(id).unwrap(),
            format!("Patch {id}"),
            create_soundfont_config(
                &provider,
                SoundFontInstrument::new(0, id as u8, false).unwrap(),
            )
            .unwrap(),
            MidiChannel::new(channel).unwrap(),
            PatchOutput::to_track(MixerTrackId::new(channel).unwrap()),
        )
    }

    fn patches() -> Vec<Patch> {
        vec![patch(1, 2), patch(7, 9)]
    }

    fn globals() -> GlobalParameters {
        GlobalParameters::new(0.0).unwrap()
    }

    fn registry() -> crate::synth::CapabilityRegistry {
        crate::adapter::production_instruments::production_capability_registry().unwrap()
    }

    #[test]
    fn requires_discriminating_multi_patch_input() {
        assert_eq!(
            DemoScene::exhaustive(&registry(), &[], &globals()),
            Err(DemoSceneError::InsufficientPatches { actual: 0 })
        );
        assert_eq!(
            DemoScene::exhaustive(&registry(), &[patch(1, 0)], &globals()),
            Err(DemoSceneError::InsufficientPatches { actual: 1 })
        );
    }

    #[test]
    fn derives_patch_specific_parameters_and_midi_from_the_fixture() {
        let patches = patches();
        let scene = DemoScene::exhaustive(&registry(), &patches, &globals()).unwrap();

        let focused_patch = &patches[0];
        for descriptor in PatchOutput::surface_descriptor() {
            let identifier = format!(
                "parameter.patch.{}.output.{}",
                focused_patch.id().value(),
                descriptor.name()
            );
            assert!(scene.expected_coverage().contains(&identifier));
        }
        for track_id in MixerTrackId::ALL {
            for parameter in MixerTrackParameter::MAIN {
                assert!(scene
                    .expected_coverage()
                    .contains(&format!("parameter.track.{track_id}.{parameter}")));
            }
            for bus in crate::mixer::bus_id::BusId::ALL {
                assert!(scene.expected_coverage().contains(&format!(
                    "parameter.track.{track_id}.sends[{}]",
                    bus.index()
                )));
            }
        }
        for descriptor in GlobalParameters::surface_descriptor() {
            assert!(scene
                .expected_coverage()
                .contains(&format!("parameter.global.{}", descriptor.name())));
        }

        let probes = scene
            .steps()
            .iter()
            .filter_map(|step| match step {
                DemoSceneStep::MidiProbe(probe) => Some(*probe),
                _ => None,
            })
            .collect::<Vec<MidiProbe>>();
        for patch in &patches {
            let kinds = probes
                .iter()
                .filter(|probe| probe.patch_id() == patch.id())
                .map(|probe| probe.message().kind())
                .collect::<Vec<_>>();
            let descriptor = registry()
                .descriptor_for_config(patch.instrument_config())
                .expect("fixture capability is installed")
                .clone();
            let expected = descriptor.supported_midi_kinds();
            assert!(kinds.len() >= expected.len());
            assert_eq!(&kinds[kinds.len() - expected.len()..], expected);
        }
        assert_eq!(
            probes
                .iter()
                .filter(|probe| {
                    probe.expected_rejection() == Some(EventRejection::UnknownPatch)
                })
                .count(),
            1
        );
    }

    #[test]
    fn schema_derived_current_surface() {
        let patches = patches();
        let scene = DemoScene::exhaustive(&registry(), &patches, &globals()).unwrap();
        assert!(scene
            .expected_coverage()
            .windows(2)
            .all(|pair| pair[0] < pair[1]));
        assert_eq!(WindowInput::surface_descriptor().len(), 49);
        assert_eq!(
            crate::control::app_event::AppEvent::surface_descriptor().len(),
            57
        );
        assert_eq!(
            crate::kernel::midi_message::MidiMessageKind::surface_descriptor().len(),
            8
        );
        assert_eq!(PatchOutput::surface_descriptor().len(), 2);
        assert_eq!(MixerTrackParameters::surface_descriptor().len(), 4);
        assert_eq!(GlobalParameters::surface_descriptor().len(), 1);
        assert_eq!(EventRejection::surface_descriptor().len(), 16);
        for descriptor in PatchOutput::surface_descriptor() {
            assert!(scene.expected_coverage().contains(&format!(
                "parameter.patch.{}.output.{}",
                patches[0].id().value(),
                descriptor.name()
            )));
        }
        for track_id in MixerTrackId::ALL {
            for parameter in MixerTrackParameter::MAIN {
                assert!(scene
                    .expected_coverage()
                    .contains(&format!("parameter.track.{track_id}.{parameter}")));
            }
            for bus in crate::mixer::bus_id::BusId::ALL {
                assert!(scene.expected_coverage().contains(&format!(
                    "parameter.track.{track_id}.sends[{}]",
                    bus.index()
                )));
            }
        }
    }

    #[test]
    fn includes_every_normalized_window_input_and_focus_reset() {
        let scene = DemoScene::exhaustive(&registry(), &patches(), &globals()).unwrap();
        let inputs = scene
            .steps()
            .iter()
            .filter_map(|step| match step {
                DemoSceneStep::WindowInput(input) => Some(*input),
                _ => None,
            })
            .collect::<Vec<WindowInput>>();

        for key in [
            WindowKey::W,
            WindowKey::S,
            WindowKey::A,
            WindowKey::D,
            WindowKey::K,
            WindowKey::Other,
        ] {
            assert!(inputs.contains(&WindowInput::key_down(key)));
            assert!(inputs.contains(&WindowInput::key_up(key)));
        }
        assert!(inputs.contains(&WindowInput::focus_lost()));
        assert!(inputs.iter().any(|input| {
            input.kind() == WindowInputKind::FocusLost && input.key() == WindowKey::Other
        }));
    }

    #[test]
    fn boundary_checkpoints_name_nonfatal_rejections_and_restoration() {
        let scene = DemoScene::exhaustive(&registry(), &patches(), &globals()).unwrap();
        let checkpoints = scene
            .steps()
            .iter()
            .filter_map(|step| match step {
                DemoSceneStep::Checkpoint(checkpoint) => Some(checkpoint),
                _ => None,
            })
            .collect::<Vec<_>>();

        let boundary_rejections = checkpoints
            .iter()
            .filter(|checkpoint| {
                checkpoint.expected_last_rejection() == Some(EventRejection::ParameterAtBoundary)
            })
            .count();
        // Continuous track classes (level, pan) plus the one shared indexed
        // send descriptor plus the one shared return-level descriptor.
        let bounded_parameter_count = PatchOutput::surface_descriptor().len()
            + VoiceEnvelope::surface_descriptor().len()
            + MixerTrackParameter::MAIN
                .into_iter()
                .filter(|parameter| {
                    parameter.descriptor().kind()
                        == crate::mixer::mixer_track_parameters::MixerTrackParameterKind::Continuous
                })
                .count()
            + 2
            + GlobalParameters::surface_descriptor().len();
        // Every bounded parameter contributes a rejection at each of its two
        // ends. The one extra is the patch-order boundary: stepping left from
        // the first installed Patch is refused rather than wrapping, and that
        // order is not a bounded parameter.
        assert_eq!(boundary_rejections, bounded_parameter_count * 2 + 1);
        assert_eq!(
            checkpoints
                .iter()
                .filter(|checkpoint| checkpoint.name().starts_with("boundary.")
                    && checkpoint.name().ends_with(".restored"))
                .count(),
            bounded_parameter_count
        );
    }

    #[test]
    fn construction_and_coverage_order_are_deterministic() {
        let patches = patches();
        let first = DemoScene::exhaustive(&registry(), &patches, &globals()).unwrap();
        let second = DemoScene::exhaustive(&registry(), &patches, &globals()).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.name(), DemoScene::NAME);
        assert_eq!(first.schema_version(), DemoScene::SCHEMA_VERSION);
        assert!(first.event_log_capacity() > first.steps().len());
        assert!(first
            .expected_coverage()
            .windows(2)
            .all(|pair| pair[0] < pair[1]));
        assert!(first
            .steps()
            .iter()
            .any(|step| matches!(step, DemoSceneStep::Tick(duration) if !duration.is_zero())));
    }
}
