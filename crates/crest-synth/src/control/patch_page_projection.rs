use crate::control::app_state::AppState;
use crate::control::top_level_context::TopLevelContext;
use crate::control::{
    EngineSelectionFailure, EngineSelectionRequestId, EngineSelectionStatusKind, PatchControlId,
    PatchDetailSubject, PatchPositionId, ProspectivePatch, StructuralEditIntent, SurfaceId,
};
use crate::kernel::{MidiChannel, PatchId, MAX_ACTIVE_PATCHES};
use crate::mixer::patch_output::{PatchOutputParameter, PatchOutputParameterKind};
use crate::real_time::GraphRevision;
use crate::synth::instrument_capability::{
    AssetReference, ParameterChoice, ParameterDefault, ParameterKind, ParameterRange,
    ParameterUpdate, ParameterValue, PatchInteraction,
};
use crate::synth::voice_limit::VoiceLimit;
use crate::synth::{
    CapabilityId, EffectCapabilityDescriptor, EffectCapabilityId, EffectSlotId, ParameterId,
    ParameterSpec, PostEffectConfig,
};
use core::fmt;
use serde::{Serialize, Serializer};
use std::sync::Arc;

/// The focused PATCH position, explicitly distinguishing acknowledged content
/// from the interaction-only trailing empty endpoint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PatchPageIdentity {
    Created {
        id: PatchId,
        name: String,
        midi_channel: MidiChannel,
        active_count: usize,
        capacity: usize,
    },
    Empty {
        label: String,
        active_count: usize,
        capacity: usize,
        creation_available: bool,
    },
}

impl PatchPageIdentity {
    pub const fn id(&self) -> Option<PatchId> {
        match self {
            Self::Created { id, .. } => Some(*id),
            Self::Empty { .. } => None,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Created { name, .. } => name,
            Self::Empty { label, .. } => label,
        }
    }

    pub const fn midi_channel(&self) -> Option<MidiChannel> {
        match self {
            Self::Created { midi_channel, .. } => Some(*midi_channel),
            Self::Empty { .. } => None,
        }
    }

    pub const fn is_empty(&self) -> bool {
        matches!(self, Self::Empty { .. })
    }

    pub const fn active_count(&self) -> usize {
        match self {
            Self::Created { active_count, .. } | Self::Empty { active_count, .. } => *active_count,
        }
    }

    pub const fn capacity(&self) -> usize {
        match self {
            Self::Created { capacity, .. } | Self::Empty { capacity, .. } => *capacity,
        }
    }

    pub const fn creation_available(&self) -> bool {
        match self {
            Self::Created { .. } => false,
            Self::Empty {
                creation_available, ..
            } => *creation_available,
        }
    }
}

/// One installed capability available to a future structural engine selector.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchPageEngineChoice {
    capability_id: CapabilityId,
    label: String,
}

impl PatchPageEngineChoice {
    pub const fn capability_id(&self) -> &CapabilityId {
        &self.capability_id
    }

    pub fn label(&self) -> &str {
        &self.label
    }
}

/// Active engine identity plus the complete installed registry choice surface.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchPageEngine {
    control_id: PatchControlId,
    active_capability_id: CapabilityId,
    active_label: String,
    choices: Vec<PatchPageEngineChoice>,
    status: EngineSelectionStatusKind,
    active_graph_revision: GraphRevision,
    requested_capability_id: Option<CapabilityId>,
    request_id: Option<EngineSelectionRequestId>,
    target_graph_revision: Option<GraphRevision>,
    failure: Option<EngineSelectionFailure>,
    editable: bool,
}

impl PatchPageEngine {
    pub fn control_id(&self) -> PatchControlId {
        self.control_id.clone()
    }

    pub const fn active_capability_id(&self) -> &CapabilityId {
        &self.active_capability_id
    }

    pub fn active_label(&self) -> &str {
        &self.active_label
    }

    pub fn choices(&self) -> &[PatchPageEngineChoice] {
        &self.choices
    }

    pub const fn status(&self) -> EngineSelectionStatusKind {
        self.status
    }

    pub const fn active_graph_revision(&self) -> GraphRevision {
        self.active_graph_revision
    }

    pub const fn requested_capability_id(&self) -> Option<&CapabilityId> {
        self.requested_capability_id.as_ref()
    }

    pub const fn request_id(&self) -> Option<EngineSelectionRequestId> {
        self.request_id
    }

    pub const fn target_graph_revision(&self) -> Option<GraphRevision> {
        self.target_graph_revision
    }

    pub const fn failure(&self) -> Option<EngineSelectionFailure> {
        self.failure
    }

    pub const fn editable(&self) -> bool {
        self.editable
    }
}

/// One canonical editable ADSR row projected without a second field list.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchPageEnvelopeRow {
    control_id: PatchControlId,
    id: String,
    label: String,
    value: f32,
    minimum: f32,
    maximum: f32,
    fine_step: f32,
    coarse_step: f32,
    unit: Option<String>,
    editable: bool,
}

/// One canonical PATCH Utility output row.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchPageOutputRow {
    control_id: PatchControlId,
    id: String,
    label: String,
    kind: String,
    scalar_value: Option<f32>,
    choice_value: Option<String>,
    minimum: Option<f32>,
    maximum: Option<f32>,
    fine_step: Option<f32>,
    coarse_step: Option<f32>,
    unit: Option<String>,
    editable: bool,
}

impl PatchPageOutputRow {
    fn for_parameter(
        parameter: PatchOutputParameter,
        output: Option<crate::mixer::patch_output::PatchOutput>,
        editable: bool,
    ) -> Self {
        let descriptor = parameter.descriptor();
        Self {
            control_id: PatchControlId::Output(parameter),
            id: descriptor.name().to_owned(),
            label: descriptor.label().to_owned(),
            kind: match descriptor.kind() {
                PatchOutputParameterKind::Continuous => "continuous",
                PatchOutputParameterKind::TrackChoice => "choice",
            }
            .to_owned(),
            scalar_value: (parameter == PatchOutputParameter::TrimGain)
                .then(|| output.map(|output| output.trim_gain_db()))
                .flatten(),
            choice_value: (parameter == PatchOutputParameter::OutputTrack)
                .then(|| output.map(|output| output.track_id().to_string()))
                .flatten(),
            minimum: descriptor.minimum(),
            maximum: descriptor.maximum(),
            fine_step: descriptor.fine_step(),
            coarse_step: descriptor.coarse_step(),
            unit: descriptor.unit().map(str::to_owned),
            editable: editable && output.is_some(),
        }
    }

    /// Projects one PATCH Utility row that is not an output parameter.
    ///
    /// Master gain reads the one canonical global descriptor and the one
    /// canonical value — PATCH holds no copy — while MIDI input and the voice
    /// limit read the focused Patch's own. Every bound and step comes from the
    /// same descriptor the reducer edits through.
    fn for_utility_control(
        control: &PatchControlId,
        output: Option<crate::mixer::patch_output::PatchOutput>,
        channel: Option<MidiChannel>,
        voice_limit: VoiceLimit,
        patch_owned_editable: bool,
        global: &crate::mixer::global_parameters::GlobalParameters,
    ) -> Option<Self> {
        let row = match control {
            PatchControlId::Output(parameter) => {
                return Some(Self::for_parameter(
                    *parameter,
                    output,
                    patch_owned_editable,
                ))
            }
            PatchControlId::Global(parameter) => {
                let descriptor = parameter.descriptor();
                Self {
                    control_id: control.clone(),
                    // `id` is the serialization key and `label` is the
                    // authored label. They are two vocabularies for two
                    // readers, and this row used to project the key for both.
                    id: descriptor.name().to_owned(),
                    label: descriptor.label().to_owned(),
                    kind: "continuous".to_owned(),
                    scalar_value: Some(global.master_gain_db()),
                    choice_value: None,
                    minimum: Some(descriptor.minimum()),
                    maximum: Some(descriptor.maximum()),
                    fine_step: Some(descriptor.fine_step()),
                    coarse_step: Some(descriptor.coarse_step()),
                    unit: None,
                    editable: true,
                }
            }
            PatchControlId::MidiInput => Self {
                control_id: control.clone(),
                id: "midiInput".to_owned(),
                label: "MIDI Input".to_owned(),
                kind: "choice".to_owned(),
                scalar_value: None,
                choice_value: channel.map(|channel| (u16::from(channel.value()) + 1).to_string()),
                minimum: Some(f32::from(MidiChannel::MIN) + 1.0),
                maximum: Some(f32::from(MidiChannel::MAX) + 1.0),
                fine_step: Some(1.0),
                coarse_step: Some(1.0),
                unit: None,
                editable: patch_owned_editable && channel.is_some(),
            },
            PatchControlId::VoiceLimit => {
                let descriptor = VoiceLimit::descriptor();
                Self {
                    control_id: control.clone(),
                    id: descriptor.name().to_owned(),
                    label: descriptor.label().to_owned(),
                    kind: "stepped".to_owned(),
                    scalar_value: Some(f32::from(voice_limit.value())),
                    choice_value: None,
                    minimum: Some(f32::from(descriptor.minimum())),
                    maximum: Some(f32::from(descriptor.maximum())),
                    fine_step: Some(f32::from(descriptor.fine_step())),
                    coarse_step: Some(f32::from(descriptor.coarse_step())),
                    unit: descriptor.unit().map(str::to_owned),
                    editable: patch_owned_editable,
                }
            }
            PatchControlId::Engine
            | PatchControlId::Envelope(_)
            | PatchControlId::Capability(_)
            | PatchControlId::EffectSlot(_)
            | PatchControlId::Send(_)
            | PatchControlId::Effect(..) => return None,
        };
        Some(row)
    }

    pub(crate) fn selected_text(
        parameter: PatchOutputParameter,
        output: crate::mixer::patch_output::PatchOutput,
    ) -> Result<String, serde_json::Error> {
        serde_json::to_string(&Self::for_parameter(parameter, Some(output), true))
            .map(|row| format!("> OUTPUT {row}"))
    }

    pub fn control_id(&self) -> PatchControlId {
        self.control_id.clone()
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub const fn scalar_value(&self) -> Option<f32> {
        self.scalar_value
    }

    pub fn choice_value(&self) -> Option<&str> {
        self.choice_value.as_deref()
    }
}

impl PatchPageEnvelopeRow {
    pub(crate) fn for_parameter(
        parameter: crate::synth::VoiceEnvelopeParameter,
        value: f32,
    ) -> Self {
        let spec = parameter.descriptor();
        Self {
            control_id: PatchControlId::Envelope(parameter),
            id: spec.name().to_owned(),
            label: spec.label().to_owned(),
            value,
            minimum: spec.minimum(),
            maximum: spec.maximum(),
            fine_step: spec.fine_step(),
            coarse_step: spec.coarse_step(),
            unit: spec.unit().map(str::to_owned),
            editable: true,
        }
    }

    pub(crate) fn selected_text(
        parameter: crate::synth::VoiceEnvelopeParameter,
        value: f32,
    ) -> Result<String, serde_json::Error> {
        serde_json::to_string(&Self::for_parameter(parameter, value))
            .map(|row| format!("> ENVELOPE {row}"))
    }

    pub fn control_id(&self) -> PatchControlId {
        self.control_id.clone()
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub const fn value(&self) -> f32 {
        self.value
    }

    pub const fn minimum(&self) -> f32 {
        self.minimum
    }

    pub const fn maximum(&self) -> f32 {
        self.maximum
    }

    pub const fn fine_step(&self) -> f32 {
        self.fine_step
    }

    pub const fn coarse_step(&self) -> f32 {
        self.coarse_step
    }

    pub fn unit(&self) -> Option<&str> {
        self.unit.as_deref()
    }

    pub const fn editable(&self) -> bool {
        self.editable
    }
}

/// The exact typed config source used by one descriptor-projected row.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "source", rename_all = "camelCase")]
pub enum PatchPageParameterValue {
    EmptyAsset,
    Parameter { value: ParameterValue },
    Asset { reference: AssetReference },
}

impl PatchPageParameterValue {
    pub const fn parameter(&self) -> Option<&ParameterValue> {
        match self {
            Self::Parameter { value } => Some(value),
            Self::Asset { .. } | Self::EmptyAsset => None,
        }
    }

    pub const fn asset(&self) -> Option<&AssetReference> {
        match self {
            Self::Parameter { .. } | Self::EmptyAsset => None,
            Self::Asset { reference } => Some(reference),
        }
    }
}

/// One exact active-capability parameter row in descriptor order.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchPageParameterRow {
    control_id: Option<PatchControlId>,
    id: ParameterId,
    label: String,
    kind: ParameterKind,
    update: ParameterUpdate,
    patch_interaction: PatchInteraction,
    value: PatchPageParameterValue,
    selected_choice_id: Option<String>,
    selected_label: Option<String>,
    range: Option<ParameterRange>,
    choices: Vec<ParameterChoice>,
    fine_step: Option<f64>,
    coarse_step: Option<f64>,
    unit: Option<String>,
    formatter: String,
    requested_choice_id: Option<String>,
    requested_label: Option<String>,
    status: Option<EngineSelectionStatusKind>,
    request_id: Option<EngineSelectionRequestId>,
    active_graph_revision: Option<GraphRevision>,
    target_graph_revision: Option<GraphRevision>,
    failure: Option<EngineSelectionFailure>,
    enabled: bool,
    visible: bool,
    editable: bool,
}

impl PatchPageParameterRow {
    pub(crate) fn for_effect_parameter(
        descriptor: &EffectCapabilityDescriptor,
        config: &PostEffectConfig,
        spec: &ParameterSpec,
        active_graph_revision: GraphRevision,
    ) -> Result<Self, PatchPageProjectionError> {
        if descriptor.id() != config.capability_id() || descriptor.parameter(spec.id()).is_none() {
            return Err(PatchPageProjectionError::InvalidEffectConfig);
        }
        let value = if spec.kind() == ParameterKind::Asset {
            let reference = config
                .asset_reference(spec.id())
                .or_else(|| match spec.default_value() {
                    ParameterDefault::Asset(reference) => Some(reference),
                    ParameterDefault::Value(_) => None,
                })
                .ok_or(PatchPageProjectionError::InvalidEffectConfig)?;
            PatchPageParameterValue::Asset {
                reference: reference.clone(),
            }
        } else {
            PatchPageParameterValue::Parameter {
                value: config
                    .value(spec.id())
                    .ok_or(PatchPageProjectionError::InvalidEffectConfig)?
                    .clone(),
            }
        };
        let predicate_satisfied = |predicate: Option<&crate::synth::ParameterPredicate>| {
            predicate.is_none_or(|predicate| {
                config.value(predicate.parameter_id()) == Some(predicate.equals())
            })
        };
        let enabled = predicate_satisfied(spec.enabled_when());
        let visible = predicate_satisfied(spec.visible_when());
        let control_id =
            (spec.patch_interaction() == PatchInteraction::ScalarEdit && enabled && visible)
                .then(|| PatchControlId::Effect(config.slot_id(), spec.id().clone()));
        Ok(Self {
            control_id: control_id.clone(),
            id: spec.id().clone(),
            label: spec.label().to_owned(),
            kind: spec.kind(),
            update: spec.update(),
            patch_interaction: spec.patch_interaction(),
            selected_label: match &value {
                PatchPageParameterValue::Parameter { value } => {
                    spec.value_label(value).map(|label| label.into_owned())
                }
                _ => None,
            },
            value,
            selected_choice_id: None,
            range: spec.range(),
            choices: spec.choices().to_vec(),
            fine_step: spec.fine_step(),
            coarse_step: spec.coarse_step(),
            unit: spec.unit().map(str::to_owned),
            formatter: spec.formatter().to_owned(),
            requested_choice_id: None,
            requested_label: None,
            status: None,
            request_id: None,
            active_graph_revision: control_id.as_ref().map(|_| active_graph_revision),
            target_graph_revision: None,
            failure: None,
            enabled,
            visible,
            editable: control_id.is_some(),
        })
    }

    pub(crate) fn selected_instrument_detail_text(
        descriptor: &crate::synth::CapabilityDescriptor,
        config: &crate::synth::InstrumentConfig,
        parameter_id: &ParameterId,
    ) -> Result<String, PatchPageProjectionError> {
        let sections = detail_sections(
            descriptor.sections(),
            &|id| config.value(id),
            &|id| config.asset_reference(id),
            &|id| PatchControlId::Capability(id.clone()),
        )?;
        selected_detail_parameter_text(&sections, parameter_id)
    }

    pub(crate) fn selected_effect_detail_text(
        descriptor: &EffectCapabilityDescriptor,
        config: &PostEffectConfig,
        parameter_id: &ParameterId,
    ) -> Result<String, PatchPageProjectionError> {
        let sections = detail_sections(
            descriptor.sections(),
            &|id| config.value(id),
            &|id| config.asset_reference(id),
            &|id| PatchControlId::Effect(config.slot_id(), id.clone()),
        )?;
        selected_detail_parameter_text(&sections, parameter_id)
    }

    pub fn control_id(&self) -> Option<PatchControlId> {
        self.control_id.clone()
    }

    pub const fn id(&self) -> &ParameterId {
        &self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub const fn kind(&self) -> ParameterKind {
        self.kind
    }

    pub const fn update(&self) -> ParameterUpdate {
        self.update
    }

    pub const fn patch_interaction(&self) -> PatchInteraction {
        self.patch_interaction
    }

    pub const fn value(&self) -> &PatchPageParameterValue {
        &self.value
    }

    pub fn selected_choice_id(&self) -> Option<&str> {
        self.selected_choice_id.as_deref()
    }

    pub fn selected_label(&self) -> Option<&str> {
        self.selected_label.as_deref()
    }

    pub const fn range(&self) -> Option<ParameterRange> {
        self.range
    }

    pub fn choices(&self) -> &[ParameterChoice] {
        &self.choices
    }

    pub const fn fine_step(&self) -> Option<f64> {
        self.fine_step
    }

    pub const fn coarse_step(&self) -> Option<f64> {
        self.coarse_step
    }

    pub fn unit(&self) -> Option<&str> {
        self.unit.as_deref()
    }

    pub fn formatter(&self) -> &str {
        &self.formatter
    }

    pub fn requested_choice_id(&self) -> Option<&str> {
        self.requested_choice_id.as_deref()
    }

    pub fn requested_label(&self) -> Option<&str> {
        self.requested_label.as_deref()
    }

    pub const fn status(&self) -> Option<EngineSelectionStatusKind> {
        self.status
    }

    pub const fn request_id(&self) -> Option<EngineSelectionRequestId> {
        self.request_id
    }

    pub const fn active_graph_revision(&self) -> Option<GraphRevision> {
        self.active_graph_revision
    }

    pub const fn target_graph_revision(&self) -> Option<GraphRevision> {
        self.target_graph_revision
    }

    pub const fn failure(&self) -> Option<EngineSelectionFailure> {
        self.failure
    }

    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    pub const fn visible(&self) -> bool {
        self.visible
    }

    pub const fn editable(&self) -> bool {
        self.editable
    }
}

fn selected_detail_parameter_text(
    sections: &[PatchPageSection],
    parameter_id: &ParameterId,
) -> Result<String, PatchPageProjectionError> {
    let row = sections
        .iter()
        .flat_map(PatchPageSection::parameters)
        .find(|row| row.id() == parameter_id)
        .ok_or(PatchPageProjectionError::InvalidInstrumentConfig)?;
    serde_json::to_string(row)
        .map(|row| format!("> DETAIL_PARAMETER {row}"))
        .map_err(|_| PatchPageProjectionError::InvalidInstrumentConfig)
}

/// One active capability section and its descriptor-ordered rows.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchPageSection {
    id: String,
    label: String,
    parameters: Vec<PatchPageParameterRow>,
}

/// The stable choice id used by the empty occupancy choice. Registry entry
/// ids are namespaced (`effect.*`), so the value cannot collide.
pub const EMPTY_OCCUPANCY_CHOICE_ID: &str = "empty";

/// What currently occupies one ordered effect slot.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PatchPageSlotOccupancy {
    Empty,
    Occupied {
        slot_id: EffectSlotId,
        capability_id: EffectCapabilityId,
        label: String,
    },
}

impl PatchPageSlotOccupancy {
    pub const fn capability_id(&self) -> Option<&EffectCapabilityId> {
        match self {
            Self::Empty => None,
            Self::Occupied { capability_id, .. } => Some(capability_id),
        }
    }
}

/// One adjacent occupancy choice: empty or one installed registry entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchPageOccupancyChoice {
    id: String,
    label: String,
}

impl PatchPageOccupancyChoice {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }
}

/// One of the three ordered effect slots, whether occupied or empty.
///
/// The occupancy row shares the structural lifecycle projection used by the
/// engine row, and its adjacent nonwrapping choices are empty plus every
/// installed registry entry. Any occupying entry is resolved generically
/// through its descriptor; the projection never names a concrete effect.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchPageEffectSlot {
    slot_index: crate::synth::effect_slot_id::EffectSlotIndex,
    occupancy_control_id: PatchControlId,
    occupancy: PatchPageSlotOccupancy,
    choices: Vec<PatchPageOccupancyChoice>,
    requested_choice_id: Option<String>,
    status: Option<EngineSelectionStatusKind>,
    request_id: Option<EngineSelectionRequestId>,
    target_graph_revision: Option<GraphRevision>,
    failure: Option<EngineSelectionFailure>,
    editable: bool,
    sections: Vec<PatchPageSection>,
}

impl PatchPageEffectSlot {
    pub const fn slot_index(&self) -> crate::synth::effect_slot_id::EffectSlotIndex {
        self.slot_index
    }

    pub fn occupancy_control_id(&self) -> PatchControlId {
        self.occupancy_control_id.clone()
    }

    pub const fn occupancy(&self) -> &PatchPageSlotOccupancy {
        &self.occupancy
    }

    pub fn choices(&self) -> &[PatchPageOccupancyChoice] {
        &self.choices
    }

    pub fn requested_choice_id(&self) -> Option<&str> {
        self.requested_choice_id.as_deref()
    }

    pub const fn status(&self) -> Option<EngineSelectionStatusKind> {
        self.status
    }

    pub const fn request_id(&self) -> Option<EngineSelectionRequestId> {
        self.request_id
    }

    pub const fn target_graph_revision(&self) -> Option<GraphRevision> {
        self.target_graph_revision
    }

    pub const fn failure(&self) -> Option<EngineSelectionFailure> {
        self.failure
    }

    pub const fn editable(&self) -> bool {
        self.editable
    }

    pub fn sections(&self) -> &[PatchPageSection] {
        &self.sections
    }
}

/// The open detail entry's content, resolved entirely from the installed
/// descriptor its [`PatchDetailSubject`] names.
///
/// Present exactly while the reducer holds a detail entry and absent otherwise,
/// so a page never renders a stale detail and never synthesizes one. It carries
/// the subject rather than a copy of the subject's schema: label, sections,
/// rows, ranges, and units all resolve from the descriptor at projection time.
///
/// Every row is a focus target — the detail order is the subject descriptor's
/// visible enabled rows, not just its structural ones — which is why these rows
/// carry a `controlId` where the same specs projected on PATCH Main do not.
/// None of them is editable: the reducer accepts no adjustment on the detail
/// surface in this phase, so `editable` says so rather than inviting an edit
/// that would be refused.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchPageDetail {
    subject: PatchDetailSubject,
    label: String,
    status: EngineSelectionStatusKind,
    sections: Vec<PatchPageSection>,
}

impl PatchPageDetail {
    pub const fn subject(&self) -> &PatchDetailSubject {
        &self.subject
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub const fn status(&self) -> EngineSelectionStatusKind {
        self.status
    }

    pub fn sections(&self) -> &[PatchPageSection] {
        &self.sections
    }
}

/// Builds one detail surface's sections from a descriptor's own ordered
/// sections and this Patch's canonical values.
///
/// This is the single walk both subject kinds run. The caller has already
/// resolved which descriptor and which config to read; nothing here can tell an
/// instrument subject from an effect one, which is the whole of what makes one
/// shell serve both.
fn detail_sections<'a>(
    sections: &[crate::synth::CapabilitySection],
    value: &dyn Fn(&ParameterId) -> Option<&'a ParameterValue>,
    asset: &dyn Fn(&ParameterId) -> Option<&'a AssetReference>,
    control_id: &dyn Fn(&ParameterId) -> PatchControlId,
) -> Result<Vec<PatchPageSection>, PatchPageProjectionError> {
    sections
        .iter()
        .map(|section| {
            let parameters = section
                .parameters()
                .iter()
                .map(|spec| {
                    let resolved = if spec.kind() == ParameterKind::Asset {
                        match asset(spec.id()) {
                            Some(reference) => PatchPageParameterValue::Asset {
                                reference: reference.clone(),
                            },
                            None => PatchPageParameterValue::EmptyAsset,
                        }
                    } else {
                        PatchPageParameterValue::Parameter {
                            value: value(spec.id())
                                .ok_or(PatchPageProjectionError::InvalidInstrumentConfig)?
                                .clone(),
                        }
                    };
                    let satisfied = |predicate: Option<&crate::synth::ParameterPredicate>| {
                        predicate.is_none_or(|predicate| {
                            value(predicate.parameter_id()) == Some(predicate.equals())
                        })
                    };
                    let enabled = satisfied(spec.enabled_when());
                    let visible = satisfied(spec.visible_when());
                    let selected_choice_id = match &resolved {
                        PatchPageParameterValue::Parameter {
                            value: ParameterValue::Choice(choice_id),
                        } => Some(choice_id.clone()),
                        _ => None,
                    };
                    let selected_label = match &resolved {
                        PatchPageParameterValue::Parameter { value } => {
                            spec.value_label(value).map(|label| label.into_owned())
                        }
                        _ => None,
                    };
                    Ok(PatchPageParameterRow {
                        // Every visible enabled detail row is a focus target,
                        // whatever its patch interaction: the detail order is
                        // the descriptor's own rows, which is exactly why a
                        // Braids detail focus has no PATCH Main row to land on.
                        control_id: (enabled && visible).then(|| control_id(spec.id())),
                        id: spec.id().clone(),
                        label: spec.label().to_owned(),
                        kind: spec.kind(),
                        update: spec.update(),
                        patch_interaction: spec.patch_interaction(),
                        value: resolved,
                        selected_choice_id,
                        selected_label,
                        range: spec.range(),
                        choices: spec.choices().to_vec(),
                        fine_step: spec.fine_step(),
                        coarse_step: spec.coarse_step(),
                        unit: spec.unit().map(str::to_owned),
                        formatter: spec.formatter().to_owned(),
                        requested_choice_id: None,
                        requested_label: None,
                        status: None,
                        request_id: None,
                        active_graph_revision: None,
                        target_graph_revision: None,
                        failure: None,
                        enabled,
                        visible,
                        editable: false,
                    })
                })
                .collect::<Result<Vec<_>, PatchPageProjectionError>>()?;
            Ok(PatchPageSection {
                id: section.id().to_owned(),
                label: section.label().to_owned(),
                parameters,
            })
        })
        .collect()
}

impl PatchPageSection {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn parameters(&self) -> &[PatchPageParameterRow] {
        &self.parameters
    }
}

enum PatchProjectionSource<'a> {
    Created(&'a crate::synth::Patch),
    Pending(&'a crate::synth::Patch),
    Prospective(&'a ProspectivePatch),
}

impl PatchProjectionSource<'_> {
    fn send(&self, bus: crate::mixer::bus_id::BusId) -> f32 {
        match self {
            Self::Created(patch) | Self::Pending(patch) => patch.send(bus),
            Self::Prospective(_) => 0.0,
        }
    }
    fn instrument_config(&self) -> &crate::synth::InstrumentConfig {
        match self {
            Self::Created(patch) | Self::Pending(patch) => patch.instrument_config(),
            Self::Prospective(patch) => patch.instrument_config(),
        }
    }

    fn envelope(&self) -> &crate::synth::VoiceEnvelope {
        match self {
            Self::Created(patch) | Self::Pending(patch) => patch.envelope(),
            Self::Prospective(patch) => patch.envelope(),
        }
    }

    fn output(&self) -> Option<crate::mixer::patch_output::PatchOutput> {
        match self {
            Self::Created(patch) | Self::Pending(patch) => Some(patch.output()),
            Self::Prospective(patch) => patch.output(),
        }
    }

    fn channel(&self) -> Option<MidiChannel> {
        match self {
            Self::Created(patch) | Self::Pending(patch) => Some(patch.channel()),
            Self::Prospective(patch) => patch.channel(),
        }
    }

    fn effect_slots(
        &self,
    ) -> &[Option<PostEffectConfig>; crate::synth::effect_slot_id::MAX_EFFECT_SLOTS] {
        match self {
            Self::Created(patch) | Self::Pending(patch) => patch.effect_slots(),
            Self::Prospective(patch) => patch.effect_slots(),
        }
    }

    fn effect_slot(
        &self,
        index: crate::synth::effect_slot_id::EffectSlotIndex,
    ) -> Option<&PostEffectConfig> {
        self.effect_slots()[index.index()].as_ref()
    }

    fn voice_limit(&self) -> VoiceLimit {
        match self {
            Self::Created(patch) | Self::Pending(patch) => patch.voice_limit(),
            Self::Prospective(patch) => patch.voice_limit(),
        }
    }

    fn correlation_patch_id(&self) -> Option<PatchId> {
        match self {
            Self::Created(patch) | Self::Pending(patch) => Some(patch.id()),
            Self::Prospective(_) => None,
        }
    }

    fn created_patch(&self) -> Option<&crate::synth::Patch> {
        match self {
            Self::Created(patch) => Some(patch),
            Self::Pending(_) | Self::Prospective(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct PatchPageContent {
    context: TopLevelContext,
    prospective: bool,
    focused_control_id: PatchControlId,
    patch: PatchPageIdentity,
    output: Vec<PatchPageOutputRow>,
    engine: PatchPageEngine,
    envelope: Vec<PatchPageEnvelopeRow>,
    sections: Vec<PatchPageSection>,
    effects: Vec<PatchPageEffectSlot>,
    detail: Option<PatchPageDetail>,
}

/// Immutable host-neutral PATCH view model derived by one generic schema walk.
#[derive(Clone, Debug)]
pub struct PatchPageProjection {
    content: Arc<PatchPageContent>,
    state_hash: Arc<str>,
}

impl PatchPageProjection {
    pub const SERIALIZED_LEAF_DESCRIPTOR: &'static [&'static str] = &[
        "context",
        "detail",
        "detail.label",
        "detail.sections[].id",
        "detail.sections[].label",
        "detail.sections[].parameters[].activeGraphRevision",
        "detail.sections[].parameters[].choices[].id",
        "detail.sections[].parameters[].choices[].label",
        "detail.sections[].parameters[].coarseStep",
        "detail.sections[].parameters[].controlId",
        "detail.sections[].parameters[].editable",
        "detail.sections[].parameters[].enabled",
        "detail.sections[].parameters[].failure",
        "detail.sections[].parameters[].fineStep",
        "detail.sections[].parameters[].formatter",
        "detail.sections[].parameters[].id",
        "detail.sections[].parameters[].kind",
        "detail.sections[].parameters[].label",
        "detail.sections[].parameters[].patchInteraction",
        "detail.sections[].parameters[].range",
        "detail.sections[].parameters[].range.maximum",
        "detail.sections[].parameters[].range.minimum",
        "detail.sections[].parameters[].requestId",
        "detail.sections[].parameters[].requestedChoiceId",
        "detail.sections[].parameters[].requestedLabel",
        "detail.sections[].parameters[].selectedChoiceId",
        "detail.sections[].parameters[].selectedLabel",
        "detail.sections[].parameters[].status",
        "detail.sections[].parameters[].targetGraphRevision",
        "detail.sections[].parameters[].unit",
        "detail.sections[].parameters[].update",
        "detail.sections[].parameters[].value.reference.kind",
        "detail.sections[].parameters[].value.reference.locator",
        "detail.sections[].parameters[].value.source",
        "detail.sections[].parameters[].value.value.kind",
        "detail.sections[].parameters[].value.value.value",
        "detail.sections[].parameters[].visible",
        "detail.status",
        "detail.subject.capabilityId",
        "detail.subject.kind",
        "detail.subject.slotId",
        "engine.activeCapabilityId",
        "engine.activeGraphRevision",
        "engine.activeLabel",
        "engine.choices[].capabilityId",
        "engine.choices[].label",
        "engine.controlId",
        "engine.editable",
        "engine.failure",
        "engine.requestId",
        "engine.requestedCapabilityId",
        "engine.status",
        "engine.targetGraphRevision",
        "envelope[].coarseStep",
        "envelope[].controlId",
        "envelope[].editable",
        "envelope[].fineStep",
        "envelope[].id",
        "envelope[].label",
        "envelope[].maximum",
        "envelope[].minimum",
        "envelope[].unit",
        "envelope[].value",
        "focusedControlId",
        "effects[].slotIndex",
        "effects[].occupancyControlId",
        "effects[].occupancy.kind",
        "effects[].occupancy.slotId",
        "effects[].occupancy.capabilityId",
        "effects[].occupancy.label",
        "effects[].choices[].id",
        "effects[].choices[].label",
        "effects[].requestedChoiceId",
        "effects[].status",
        "effects[].requestId",
        "effects[].targetGraphRevision",
        "effects[].failure",
        "effects[].editable",
        "effects[].sections[].id",
        "effects[].sections[].label",
        "effects[].sections[].parameters[].coarseStep",
        "effects[].sections[].parameters[].controlId",
        "effects[].sections[].parameters[].editable",
        "effects[].sections[].parameters[].enabled",
        "effects[].sections[].parameters[].activeGraphRevision",
        "effects[].sections[].parameters[].failure",
        "effects[].sections[].parameters[].fineStep",
        "effects[].sections[].parameters[].formatter",
        "effects[].sections[].parameters[].id",
        "effects[].sections[].parameters[].kind",
        "effects[].sections[].parameters[].label",
        "effects[].sections[].parameters[].patchInteraction",
        "effects[].sections[].parameters[].requestId",
        "effects[].sections[].parameters[].range.maximum",
        "effects[].sections[].parameters[].range.minimum",
        "effects[].sections[].parameters[].requestedChoiceId",
        "effects[].sections[].parameters[].requestedLabel",
        "effects[].sections[].parameters[].selectedChoiceId",
        "effects[].sections[].parameters[].selectedLabel",
        "effects[].sections[].parameters[].status",
        "effects[].sections[].parameters[].targetGraphRevision",
        "effects[].sections[].parameters[].unit",
        "effects[].sections[].parameters[].update",
        "effects[].sections[].parameters[].value.source",
        "effects[].sections[].parameters[].value.value.kind",
        "effects[].sections[].parameters[].value.value.value",
        "effects[].sections[].parameters[].visible",
        "patch.activeCount",
        "patch.capacity",
        "patch.creationAvailable",
        "patch.id",
        "patch.kind",
        "patch.label",
        "patch.midiChannel",
        "patch.name",
        "prospective",
        "output[].coarseStep",
        "output[].controlId",
        "output[].editable",
        "output[].fineStep",
        "output[].id",
        "output[].kind",
        "output[].label",
        "output[].maximum",
        "output[].minimum",
        "output[].scalarValue",
        "output[].choiceValue",
        "output[].unit",
        "sections[].id",
        "sections[].label",
        "sections[].parameters[].activeGraphRevision",
        "sections[].parameters[].choices[].id",
        "sections[].parameters[].choices[].label",
        "sections[].parameters[].coarseStep",
        "sections[].parameters[].controlId",
        "sections[].parameters[].editable",
        "sections[].parameters[].enabled",
        "sections[].parameters[].failure",
        "sections[].parameters[].fineStep",
        "sections[].parameters[].formatter",
        "sections[].parameters[].id",
        "sections[].parameters[].kind",
        "sections[].parameters[].label",
        "sections[].parameters[].patchInteraction",
        "sections[].parameters[].range.maximum",
        "sections[].parameters[].range.minimum",
        "sections[].parameters[].range",
        "sections[].parameters[].requestId",
        "sections[].parameters[].requestedChoiceId",
        "sections[].parameters[].requestedLabel",
        "sections[].parameters[].selectedChoiceId",
        "sections[].parameters[].selectedLabel",
        "sections[].parameters[].status",
        "sections[].parameters[].targetGraphRevision",
        "sections[].parameters[].unit",
        "sections[].parameters[].update",
        "sections[].parameters[].value.reference.kind",
        "sections[].parameters[].value.reference.locator",
        "sections[].parameters[].value.source",
        "sections[].parameters[].value.value.kind",
        "sections[].parameters[].value.value.value",
        "sections[].parameters[].visible",
        "stateHash",
    ];

    pub const fn serialized_leaf_descriptor() -> &'static [&'static str] {
        Self::SERIALIZED_LEAF_DESCRIPTOR
    }

    pub fn context(&self) -> TopLevelContext {
        self.content.context
    }

    pub fn is_prospective(&self) -> bool {
        self.content.prospective
    }

    pub fn focused_control_id(&self) -> PatchControlId {
        self.content.focused_control_id.clone()
    }

    pub fn patch(&self) -> &PatchPageIdentity {
        &self.content.patch
    }

    pub fn output(&self) -> &[PatchPageOutputRow] {
        &self.content.output
    }

    pub fn engine(&self) -> &PatchPageEngine {
        &self.content.engine
    }

    pub fn envelope(&self) -> &[PatchPageEnvelopeRow] {
        &self.content.envelope
    }

    pub fn sections(&self) -> &[PatchPageSection] {
        &self.content.sections
    }

    pub fn effects(&self) -> &[PatchPageEffectSlot] {
        &self.content.effects
    }

    /// The open detail entry's capability-resolved content, or `None` while no
    /// entry is open.
    pub fn detail(&self) -> Option<&PatchPageDetail> {
        self.content.detail.as_ref()
    }

    pub fn state_hash(&self) -> &str {
        &self.state_hash
    }

    pub(crate) fn with_state_hash(&self, state_hash: String) -> Self {
        Self {
            content: Arc::clone(&self.content),
            state_hash: Arc::from(state_hash),
        }
    }

    #[cfg(test)]
    pub(crate) fn shares_content_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.content, &other.content)
    }

    pub(crate) fn project(
        state: &AppState,
        state_hash: &str,
    ) -> Result<Self, PatchPageProjectionError> {
        let position = state
            .interaction()
            .patch_position_focus()
            .ok_or(PatchPageProjectionError::MissingPatchFocus)?;
        let focused_control_id = state
            .interaction()
            .patch_control_focus()
            .ok_or(PatchPageProjectionError::MissingPatchControlFocus)?;
        let prospective_defaults;
        let prospective = position == PatchPositionId::TrailingEmpty;
        let source = match position {
            PatchPositionId::Created(focus) => PatchProjectionSource::Created(
                state
                    .patches()
                    .iter()
                    .find(|patch| patch.id() == focus)
                    .ok_or(PatchPageProjectionError::UnknownPatchFocus)?,
            ),
            PatchPositionId::TrailingEmpty => {
                if let Some(pending) = state.pending_patch_creation() {
                    PatchProjectionSource::Pending(pending)
                } else {
                    let blueprint = state
                        .patch_creation_blueprint()
                        .ok_or(PatchPageProjectionError::InvalidInstrumentConfig)?;
                    prospective_defaults = blueprint
                        .prospective(state.patches().len(), blueprint.instrument_capability_id())
                        .map_err(|_| PatchPageProjectionError::InvalidInstrumentConfig)?;
                    PatchProjectionSource::Prospective(&prospective_defaults)
                }
            }
        };
        let descriptor = state
            .capabilities()
            .descriptor_for_config(source.instrument_config())
            .ok_or(PatchPageProjectionError::InvalidInstrumentConfig)?;
        state
            .capabilities()
            .validate_config(source.instrument_config())
            .map_err(|_| PatchPageProjectionError::InvalidInstrumentConfig)?;
        crate::control::app_state::validate_effect_slots(state.effects(), source.effect_slots())
            .map_err(|_| PatchPageProjectionError::InvalidEffectConfig)?;
        // The detail entry, resolved before the containment check because it is
        // one of the three orders a focused row may belong to.
        let detail = state
            .interaction()
            .detail_subject()
            .map(|subject| project_detail(state, &source, subject))
            .transpose()?;

        // Which order the focused row belongs to is decided by the one
        // Utility/PatchMain split, so a new Utility row cannot fall through to
        // the main order and read as an invalid config.
        //
        // A focus on the subordinate detail surface belongs to neither of those
        // orders — it is the open subject's own order, which the surface it is
        // focused on names. Resolving it from the open entry rather than from
        // the main order is what makes a Braids detail focus projectable at
        // all: Braids' main order hosts no `Capability` row, so its detail rows
        // exist in exactly one place.
        let modal_origin_surface = state
            .interaction()
            .return_path()
            .filter(|_| {
                matches!(
                    state.interaction().active_surface(),
                    SurfaceId::PatchChoice | SurfaceId::FileBrowser
                )
            })
            .map(|path| path.origin().surface());
        let resolved_controls = if state.interaction().active_surface() == SurfaceId::PatchDetail
            || modal_origin_surface == Some(SurfaceId::PatchDetail)
        {
            detail
                .as_ref()
                .map(|detail| {
                    let mut controls = detail
                        .sections()
                        .iter()
                        .flat_map(|section| section.parameters())
                        .filter_map(PatchPageParameterRow::control_id)
                        .collect::<Vec<_>>();
                    if matches!(detail.subject(), PatchDetailSubject::Instrument { .. }) {
                        controls.extend(
                            crate::synth::VoiceEnvelope::surface_descriptor()
                                .iter()
                                .map(|spec| PatchControlId::Envelope(spec.parameter())),
                        );
                    }
                    controls
                })
                .ok_or(PatchPageProjectionError::InvalidInstrumentConfig)?
        } else if state.interaction().active_surface() == SurfaceId::PatchChoice {
            // Choice retains the exact subject even when schema repair removes
            // its Overview return origin while the modal is open. The repaired
            // ReturnPath owns the destination; this compatibility projection
            // must not invalidate the still-live modal merely because its
            // original row is no longer focusable on Patch Main.
            vec![focused_control_id.clone()]
        } else if focused_control_id.is_utility() {
            let mut controls = PatchControlId::utility_surface_descriptor().to_vec();
            if !prospective {
                controls.extend(
                    state
                        .bus_returns()
                        .returns()
                        .iter()
                        .filter(|send| send.is_occupied())
                        .map(|send| PatchControlId::Send(send.id())),
                );
            }
            controls
        } else {
            state
                .focused_patch_controls()
                .map_err(|_| PatchPageProjectionError::InvalidInstrumentConfig)?
        };
        if !resolved_controls.contains(&focused_control_id) {
            return Err(PatchPageProjectionError::InvalidInstrumentConfig);
        }

        let choices = state
            .capabilities()
            .descriptors()
            .iter()
            .map(|descriptor| PatchPageEngineChoice {
                capability_id: descriptor.id().clone(),
                label: descriptor.label().to_owned(),
            })
            .collect();
        // The five declared Utility rows, in the one declared order, so every
        // focusable Utility row has a projected row to be selected on.
        let mut output: Vec<_> = PatchControlId::utility_surface_descriptor()
            .iter()
            .filter_map(|control| {
                PatchPageOutputRow::for_utility_control(
                    control,
                    source.output(),
                    source.channel(),
                    source.voice_limit(),
                    !prospective || state.patches().len() < MAX_ACTIVE_PATCHES,
                    state.global(),
                )
            })
            .collect();
        if !prospective && source.output().is_some() {
            let descriptor = crate::mixer::mixer_track_parameters::BUS_SEND_DESCRIPTOR;
            for send in state
                .bus_returns()
                .returns()
                .iter()
                .filter(|send| send.is_occupied())
            {
                output.push(PatchPageOutputRow {
                    control_id: PatchControlId::Send(send.id()),
                    id: format!("send.{}", send.id().value()),
                    label: format!("Send {} · {}", send.id().index() + 1, send.name()),
                    kind: "continuous".to_owned(),
                    scalar_value: Some(source.send(send.id())),
                    choice_value: None,
                    minimum: Some(descriptor.minimum()),
                    maximum: Some(descriptor.maximum()),
                    fine_step: Some(descriptor.fine_step()),
                    coarse_step: Some(descriptor.coarse_step()),
                    unit: None,
                    editable: true,
                });
            }
        }
        let engine_selection = state.engine_selection();
        let correlation = engine_selection.correlation();
        let engine_targeted = correlation.is_some_and(|correlation| {
            (matches!(
                correlation.intent(),
                StructuralEditIntent::ReplaceCapability { .. }
            ) || (prospective && correlation.intent().is_append_patch()))
                && correlation.patch_id() == source.correlation_patch_id()
        });
        let creation_available = !prospective || state.patches().len() < MAX_ACTIVE_PATCHES;
        let editable = creation_available
            && if prospective {
                !state.capabilities().descriptors().is_empty()
            } else {
                state.capabilities().descriptors().len() >= 2
            }
            && matches!(
                engine_selection.kind(),
                EngineSelectionStatusKind::Ready
                    | EngineSelectionStatusKind::Unavailable
                    | EngineSelectionStatusKind::Failed
            );
        let envelope = crate::synth::VoiceEnvelope::surface_descriptor()
            .iter()
            .map(|spec| {
                let mut row = PatchPageEnvelopeRow::for_parameter(
                    spec.parameter(),
                    source.envelope().value(spec.parameter()),
                );
                row.editable = creation_available;
                row
            })
            .collect();
        let sections = descriptor
            .sections()
            .iter()
            .map(|section| {
                let parameters = section
                    .parameters()
                    .iter()
                    .map(|spec| {
                        let value = if spec.kind() == ParameterKind::Asset {
                            match source.instrument_config().asset_reference(spec.id()) {
                                Some(reference) => PatchPageParameterValue::Asset { reference: reference.clone() },
                                None => PatchPageParameterValue::EmptyAsset,
                            }
                        } else {
                            let value = source
                                .instrument_config()
                                .value(spec.id())
                                .ok_or(PatchPageProjectionError::InvalidInstrumentConfig)?;
                            PatchPageParameterValue::Parameter {
                                value: value.clone(),
                            }
                        };
                        let predicate_satisfied =
                            |predicate: Option<&crate::synth::ParameterPredicate>| {
                                predicate.is_none_or(|predicate| {
                                    source.instrument_config().value(predicate.parameter_id())
                                        == Some(predicate.equals())
                                })
                            };
                        let enabled = predicate_satisfied(spec.enabled_when());
                        let visible = predicate_satisfied(spec.visible_when());
                        let control_id = (spec.patch_interaction()
                            == PatchInteraction::StructuralChoice
                            && enabled
                            && visible)
                            .then(|| PatchControlId::Capability(spec.id().clone()));
                        let selected_choice_id = match &value {
                            PatchPageParameterValue::Parameter {
                                value: ParameterValue::Choice(choice_id),
                            } => Some(choice_id.clone()),
                            _ => None,
                        };
                        let selected_label = match &value {
                            PatchPageParameterValue::Parameter { value } => spec.value_label(value).map(|label| label.into_owned()),
                            _ => None,
                        };
                        let row_correlation = correlation.filter(|correlation| {
                            correlation.patch_id() == source.correlation_patch_id()
                                && matches!(
                                    correlation.intent(),
                                    StructuralEditIntent::ReplaceParameterChoice {
                                        capability_id,
                                        parameter_id,
                                        ..
                                    } if capability_id == descriptor.id() && parameter_id == spec.id()
                                )
                        });
                        let requested_choice_id = row_correlation.and_then(|correlation| {
                            correlation.intent().choice_id().map(str::to_owned)
                        });
                        let requested_label = requested_choice_id.as_deref().and_then(|choice_id| {
                            spec.choices()
                                .iter()
                                .find(|choice| choice.id() == choice_id)
                                .map(|choice| choice.label().to_owned())
                        });
                        Ok(PatchPageParameterRow {
                            control_id: control_id.clone(),
                            id: spec.id().clone(),
                            label: spec.label().to_owned(),
                            kind: spec.kind(),
                            update: spec.update(),
                            patch_interaction: spec.patch_interaction(),
                            value,
                            selected_choice_id,
                            selected_label,
                            range: spec.range(),
                            choices: spec.choices().to_vec(),
                            fine_step: spec.fine_step(),
                            coarse_step: spec.coarse_step(),
                            unit: spec.unit().map(str::to_owned),
                            formatter: spec.formatter().to_owned(),
                            requested_choice_id,
                            requested_label,
                            status: row_correlation.map(|_| engine_selection.kind()),
                            request_id: row_correlation
                                .map(|correlation| correlation.request_id()),
                            active_graph_revision: control_id
                                .as_ref()
                                .map(|_| engine_selection.active_graph_revision()),
                            target_graph_revision: row_correlation.and_then(|correlation| {
                                correlation.target_graph_revision()
                            }),
                            failure: row_correlation.and_then(|_| engine_selection.failure()),
                            enabled,
                            visible,
                            editable: control_id.is_some()
                                && creation_available
                                && !engine_selection.is_in_flight(),
                        })
                    })
                    .collect::<Result<Vec<_>, PatchPageProjectionError>>()?;
                Ok(PatchPageSection {
                    id: section.id().to_owned(),
                    label: section.label().to_owned(),
                    parameters,
                })
            })
            .collect::<Result<Vec<_>, PatchPageProjectionError>>()?;
        let occupancy_choices =
            core::iter::once(PatchPageOccupancyChoice {
                id: EMPTY_OCCUPANCY_CHOICE_ID.to_owned(),
                label: "Empty".to_owned(),
            })
            .chain(state.effects().descriptors().iter().map(|descriptor| {
                PatchPageOccupancyChoice {
                    id: descriptor.id().to_string(),
                    label: descriptor.label().to_owned(),
                }
            }))
            .collect::<Vec<_>>();
        let occupancy_editable = creation_available
            && !state.effects().descriptors().is_empty()
            && matches!(
                engine_selection.kind(),
                EngineSelectionStatusKind::Ready | EngineSelectionStatusKind::Failed
            );
        let effects = crate::synth::effect_slot_id::EffectSlotIndex::ALL
            .into_iter()
            .map(|slot_index| {
                let (occupancy, sections) = match source.effect_slot(slot_index) {
                    None => (PatchPageSlotOccupancy::Empty, Vec::new()),
                    Some(config) => {
                        let effect_descriptor = state
                            .effects()
                            .descriptor(config.capability_id())
                            .ok_or(PatchPageProjectionError::InvalidEffectConfig)?;
                        let sections = effect_descriptor
                            .sections()
                            .iter()
                            .map(|section| {
                                let parameters = section
                                    .parameters()
                                    .iter()
                                    .map(|spec| {
                                        PatchPageParameterRow::for_effect_parameter(
                                            effect_descriptor,
                                            config,
                                            spec,
                                            engine_selection.active_graph_revision(),
                                        )
                                    })
                                    .collect::<Result<Vec<_>, PatchPageProjectionError>>()?;
                                Ok(PatchPageSection {
                                    id: section.id().to_owned(),
                                    label: section.label().to_owned(),
                                    parameters,
                                })
                            })
                            .collect::<Result<Vec<_>, PatchPageProjectionError>>()?;
                        (
                            PatchPageSlotOccupancy::Occupied {
                                slot_id: config.slot_id(),
                                capability_id: config.capability_id().clone(),
                                label: effect_descriptor.label().to_owned(),
                            },
                            sections,
                        )
                    }
                };
                let slot_correlation = correlation.filter(|correlation| {
                    matches!(
                        correlation.intent(),
                        StructuralEditIntent::SetSlotOccupancy {
                            patch_id: target_patch,
                            slot,
                            ..
                        } if Some(*target_patch) == source.correlation_patch_id() && *slot == slot_index
                    )
                });
                let requested_choice_id =
                    slot_correlation.and_then(|correlation| match correlation.intent() {
                        StructuralEditIntent::SetSlotOccupancy { entry, .. } => {
                            Some(entry.as_ref().map_or_else(
                                || EMPTY_OCCUPANCY_CHOICE_ID.to_owned(),
                                ToString::to_string,
                            ))
                        }
                        _ => None,
                    });
                Ok(PatchPageEffectSlot {
                    slot_index,
                    occupancy_control_id: PatchControlId::EffectSlot(slot_index),
                    occupancy,
                    choices: occupancy_choices.clone(),
                    requested_choice_id,
                    status: slot_correlation.map(|_| engine_selection.kind()),
                    request_id: slot_correlation.map(|correlation| correlation.request_id()),
                    target_graph_revision: slot_correlation
                        .and_then(|correlation| correlation.target_graph_revision()),
                    failure: slot_correlation.and_then(|_| engine_selection.failure()),
                    editable: occupancy_editable,
                    sections,
                })
            })
            .collect::<Result<Vec<_>, PatchPageProjectionError>>()?;

        Ok(Self {
            content: Arc::new(PatchPageContent {
                context: TopLevelContext::Patch,
                prospective,
                focused_control_id,
                patch: if prospective {
                    PatchPageIdentity::Empty {
                        label: "NEW PATCH".to_owned(),
                        active_count: state.patches().len(),
                        capacity: MAX_ACTIVE_PATCHES,
                        creation_available: state.patches().len() < MAX_ACTIVE_PATCHES,
                    }
                } else {
                    PatchPageIdentity::Created {
                        id: source
                            .created_patch()
                            .ok_or(PatchPageProjectionError::UnknownPatchFocus)?
                            .id(),
                        name: source
                            .created_patch()
                            .ok_or(PatchPageProjectionError::UnknownPatchFocus)?
                            .name()
                            .to_owned(),
                        midi_channel: source
                            .created_patch()
                            .ok_or(PatchPageProjectionError::UnknownPatchFocus)?
                            .channel(),
                        active_count: state.patches().len(),
                        capacity: MAX_ACTIVE_PATCHES,
                    }
                },
                output,
                engine: PatchPageEngine {
                    control_id: PatchControlId::Engine,
                    active_capability_id: descriptor.id().clone(),
                    active_label: descriptor.label().to_owned(),
                    choices,
                    status: if engine_targeted {
                        engine_selection.kind()
                    } else {
                        EngineSelectionStatusKind::Ready
                    },
                    active_graph_revision: engine_selection.active_graph_revision(),
                    requested_capability_id: correlation
                        .filter(|_| engine_targeted)
                        .and_then(|correlation| correlation.target_capability_id().cloned()),
                    request_id: correlation
                        .filter(|_| engine_targeted)
                        .map(|correlation| correlation.request_id()),
                    target_graph_revision: correlation
                        .filter(|_| engine_targeted)
                        .and_then(|correlation| correlation.target_graph_revision()),
                    failure: engine_targeted
                        .then(|| engine_selection.failure())
                        .flatten(),
                    editable,
                },
                envelope,
                sections,
                effects,
                detail,
            }),
            state_hash: Arc::from(state_hash),
        })
    }
}

/// Resolves the open detail entry's whole content from the descriptor its
/// subject names.
///
/// The single `match` below picks *which descriptor and which config* the
/// subject resolves to, and nothing else: no section list is declared here, and
/// both arms hand the identical [`detail_sections`] walk their descriptor's own
/// ordered sections. Adding a third subject kind would add one arm here and
/// change nothing downstream.
fn project_detail(
    state: &AppState,
    patch: &PatchProjectionSource<'_>,
    subject: &PatchDetailSubject,
) -> Result<PatchPageDetail, PatchPageProjectionError> {
    // A row's identity on the detail surface follows the subject's shape: an
    // effect row is addressed by its exact occupied slot, an instrument row by
    // the capability parameter alone. This is the row *identity*, not its
    // content — the same distinction `SemanticResolver::patch_detail_paths`
    // draws when it resolves the focus order.
    let slot_id = subject.slot_id();
    let control_id = |id: &ParameterId| match slot_id {
        Some(slot) => PatchControlId::Effect(slot, id.clone()),
        None => PatchControlId::Capability(id.clone()),
    };
    let (label, sections) = match subject {
        PatchDetailSubject::Instrument { capability_id: _ } => {
            let descriptor = state
                .capabilities()
                .descriptor_for_config(patch.instrument_config())
                .ok_or(PatchPageProjectionError::InvalidInstrumentConfig)?;
            let config = patch.instrument_config();
            (
                descriptor.label().to_owned(),
                detail_sections(
                    descriptor.sections(),
                    &|id| config.value(id),
                    &|id| config.asset_reference(id),
                    &control_id,
                )?,
            )
        }
        PatchDetailSubject::Effect {
            slot_id,
            capability_id,
        } => {
            let config = patch
                .effect_slots()
                .iter()
                .flatten()
                .find(|effect| effect.slot_id() == *slot_id)
                .ok_or(PatchPageProjectionError::InvalidEffectConfig)?;
            let descriptor = state
                .effects()
                .descriptor(capability_id)
                .ok_or(PatchPageProjectionError::InvalidEffectConfig)?;
            (
                descriptor.label().to_owned(),
                detail_sections(
                    descriptor.sections(),
                    &|id| config.value(id),
                    &|id| config.asset_reference(id),
                    &control_id,
                )?,
            )
        }
    };
    Ok(PatchPageDetail {
        subject: subject.clone(),
        label,
        // A capability mid-preparation reports its typed lifecycle rather than
        // an empty or stale section set: the sections above are the installed
        // descriptor's, and this says what is happening to them.
        status: state.engine_selection().kind(),
        sections,
    })
}

impl PartialEq for PatchPageProjection {
    fn eq(&self, other: &Self) -> bool {
        self.content == other.content && self.state_hash == other.state_hash
    }
}

impl Serialize for PatchPageProjection {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct SerializablePatchPage<'a> {
            context: TopLevelContext,
            prospective: bool,
            focused_control_id: PatchControlId,
            patch: &'a PatchPageIdentity,
            output: &'a [PatchPageOutputRow],
            engine: &'a PatchPageEngine,
            envelope: &'a [PatchPageEnvelopeRow],
            sections: &'a [PatchPageSection],
            effects: &'a [PatchPageEffectSlot],
            detail: Option<&'a PatchPageDetail>,
            state_hash: &'a str,
        }

        SerializablePatchPage {
            context: self.context(),
            prospective: self.is_prospective(),
            focused_control_id: self.focused_control_id(),
            patch: self.patch(),
            output: self.output(),
            engine: self.engine(),
            envelope: self.envelope(),
            sections: self.sections(),
            effects: self.effects(),
            detail: self.detail(),
            state_hash: self.state_hash(),
        }
        .serialize(serializer)
    }
}

/// A typed invariant failure while resolving the focused schema projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PatchPageProjectionError {
    MissingPatchFocus,
    MissingPatchControlFocus,
    UnknownPatchFocus,
    InvalidInstrumentConfig,
    InvalidEffectConfig,
}

impl fmt::Display for PatchPageProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MissingPatchFocus => "PATCH context has no stable Patch focus",
            Self::MissingPatchControlFocus => "PATCH context has no stable control focus",
            Self::UnknownPatchFocus => "PATCH focus does not resolve to an installed Patch",
            Self::InvalidInstrumentConfig => {
                "focused Patch config does not resolve through its capability descriptor"
            }
            Self::InvalidEffectConfig => {
                "focused Patch effect config does not resolve through its capability descriptor"
            }
        })
    }
}

impl std::error::Error for PatchPageProjectionError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::braids_capability::BraidsCapability;
    use crate::adapter::production_effects::{
        production_chorus_config, production_effect_registry,
    };
    use crate::adapter::production_instruments::{
        production_capability_registry, production_instrument_providers,
    };
    use crate::control::{AppEvent, AppState, StateProjector, SurfaceId, TopLevelContext};
    use crate::kernel::{MidiChannel, PatchId};
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::patch_output::PatchOutput;
    use crate::synth::sound_font_instrument::SoundFontInstrument;
    use crate::synth::{DescriptorDefaultConfigFactory, Patch};
    use crate::testing::automatic_midi_test::create_soundfont_config;
    use serde_json::Value;
    use std::collections::BTreeSet;

    fn state_with_config(config: crate::synth::InstrumentConfig) -> AppState {
        let mut state = AppState::new(
            production_capability_registry().unwrap(),
            GlobalParameters::new(0.0).unwrap(),
        );
        let patch = Patch::new(
            PatchId::new(7).unwrap(),
            "Focused".to_owned(),
            config,
            MidiChannel::new(3).unwrap(),
            PatchOutput::to_track(MixerTrackId::new(3).unwrap()),
        )
        .with_envelope(crate::synth::VoiceEnvelope::new(12.0, 34.0, 0.56, 78.0).unwrap());
        state.apply(AppEvent::InstallPatches(vec![patch])).unwrap();
        state
            .apply(AppEvent::SelectContext(TopLevelContext::Patch))
            .unwrap();
        state
    }

    fn empty_state() -> AppState {
        let registry = production_capability_registry().unwrap();
        let blueprint = crate::control::PatchCreationBlueprint::resolve(
            &crate::synth::CapabilityId::new(
                crate::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID,
            )
            .unwrap(),
            &DescriptorDefaultConfigFactory::new(
                registry,
                production_instrument_providers().unwrap(),
            ),
        )
        .unwrap();
        let mut state =
            state_with_config(BraidsCapability::new().unwrap().default_config().unwrap())
                .with_patch_creation_blueprint(blueprint);
        state
            .apply(AppEvent::SelectPatch(crate::control::Direction::Right))
            .unwrap();
        state
    }

    fn state_with_configured_effect(config: crate::synth::InstrumentConfig) -> AppState {
        let mut state = AppState::new_with_effects(
            production_capability_registry().unwrap(),
            production_effect_registry().unwrap(),
            GlobalParameters::new(0.0).unwrap(),
        );
        let mut patch = Patch::new(
            PatchId::new(7).unwrap(),
            "Focused".to_owned(),
            config,
            MidiChannel::new(3).unwrap(),
            PatchOutput::to_track(MixerTrackId::new(3).unwrap()),
        )
        .with_envelope(crate::synth::VoiceEnvelope::new(12.0, 34.0, 0.56, 78.0).unwrap());
        patch
            .set_slot_occupancy(
                crate::synth::effect_slot_id::EffectSlotIndex::new(0).unwrap(),
                Some(production_chorus_config(EffectSlotId::new(1).unwrap()).unwrap()),
            )
            .unwrap();
        state.apply(AppEvent::InstallPatches(vec![patch])).unwrap();
        state
            .apply(AppEvent::SelectContext(TopLevelContext::Patch))
            .unwrap();
        state
    }

    /// A state whose focused patch occupies only slot 1: slot 0 is empty and
    /// stays empty. This is exactly the shape a compacting view would
    /// silently squeeze down to position 0.
    fn state_with_gapped_effect_chain(config: crate::synth::InstrumentConfig) -> AppState {
        let mut state = AppState::new_with_effects(
            production_capability_registry().unwrap(),
            production_effect_registry().unwrap(),
            GlobalParameters::new(0.0).unwrap(),
        );
        let mut patch = Patch::new(
            PatchId::new(7).unwrap(),
            "Focused".to_owned(),
            config,
            MidiChannel::new(3).unwrap(),
            PatchOutput::to_track(MixerTrackId::new(3).unwrap()),
        );
        patch
            .set_slot_occupancy(
                crate::synth::effect_slot_id::EffectSlotIndex::new(1).unwrap(),
                Some(production_chorus_config(EffectSlotId::new(2).unwrap()).unwrap()),
            )
            .unwrap();
        state.apply(AppEvent::InstallPatches(vec![patch])).unwrap();
        state
            .apply(AppEvent::SelectContext(TopLevelContext::Patch))
            .unwrap();
        state
    }

    /// Which detail subject a fixture opens. The two subjects resolve different
    /// descriptors and different row identities, so a fixture must say which.
    enum DetailSubjectFixture {
        Instrument,
        Effect,
    }

    /// A focused Patch with one detail entry open, driven through the reducer
    /// rather than assembled: the subject is derived from the originating row,
    /// which is the only way a detail entry is ever reached.
    fn state_with_open_detail(
        config: crate::synth::InstrumentConfig,
        subject: DetailSubjectFixture,
    ) -> AppState {
        let mut state = state_with_configured_effect(config);
        if matches!(subject, DetailSubjectFixture::Effect) {
            for _ in 0..64 {
                if matches!(
                    state.interaction().focus_path().control_id(),
                    crate::control::SemanticControlId::Patch(PatchControlId::EffectSlot(slot))
                        if slot.index() == 0
                ) {
                    break;
                }
                state
                    .apply(AppEvent::Navigate(crate::control::Direction::Down))
                    .expect("the occupied slot row is inside the canonical order");
            }
        }
        state
            .apply(AppEvent::EnterSurface(
                crate::control::SurfaceId::PatchDetail,
            ))
            .expect("the fixture row resolves a detail subject");
        state
    }

    /// The capability-declared read-only fact reaches the detail page, and it
    /// *discriminates* — which `editable` cannot, because every detail row is
    /// uneditable in this phase regardless of what its capability declared.
    ///
    /// SoundFont is the discriminating descriptor: one section, two rows, two
    /// different declarations. `preset` is `StructuralChoice`, `file` is
    /// `ReadOnly` (the default of `ParameterSpec::new`). A projection that
    /// dropped the declaration and hardcoded either value would agree with this
    /// test on one row and disagree on the other, so both halves are asserted
    /// against the descriptor rather than against a literal.
    ///
    /// This is the producer a page reads to mark a read-only section in text or
    /// shape. The concept is declared by `ParameterSpec::patch_interaction`,
    /// and this is where it reaches the projection.
    #[test]
    fn the_declared_patch_interaction_reaches_the_detail_page_and_discriminates() {
        let soundfont =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        let state = state_with_open_detail(
            create_soundfont_config(
                &soundfont,
                SoundFontInstrument::new(128, 11, false).unwrap(),
            )
            .unwrap(),
            DetailSubjectFixture::Instrument,
        );
        let page = project(&state);
        let detail = page.detail().expect("the detail entry is open");
        let rows = detail
            .sections()
            .iter()
            .flat_map(PatchPageSection::parameters)
            .collect::<Vec<_>>();

        let installed = state
            .capabilities()
            .descriptor(state.patches()[0].instrument_config().capability_id())
            .expect("the fixture's engine is installed");
        let declared = installed
            .parameters()
            .map(|spec| (spec.id().to_string(), spec.patch_interaction()))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert!(!declared.is_empty());
        for row in &rows {
            assert_eq!(
                Some(row.patch_interaction()),
                declared.get(&row.id().to_string()).copied(),
                "{} projects an interaction its capability did not declare",
                row.id()
            );
        }

        // The discrimination itself, spelled out: one descriptor, two rows, two
        // answers. Assert it on the *projection*, so a page reading this leaf
        // can tell the two apart.
        let interaction = |id: &str| {
            rows.iter()
                .find(|row| row.id().to_string() == id)
                .unwrap_or_else(|| panic!("{id} is a projected detail row"))
                .patch_interaction()
        };
        assert_eq!(
            interaction(crate::adapter::hidef_soundfont_capability::SOUNDFONT_FILE_PARAMETER_ID),
            PatchInteraction::ReadOnly
        );
        assert_eq!(
            interaction(crate::adapter::hidef_soundfont_capability::SOUNDFONT_PRESET_PARAMETER_ID),
            PatchInteraction::StructuralChoice
        );

        // `editable` is uniform across exactly the pair that differ, which is
        // why it is not the producer for this fact.
        assert!(rows.iter().all(|row| !row.editable()));
    }

    fn project(state: &AppState) -> PatchPageProjection {
        let projector = StateProjector::new();
        let snapshot = projector.state_snapshot(state).unwrap();
        projector
            .patch_page_projection(state, snapshot.hash())
            .unwrap()
            .unwrap()
    }

    fn assert_descriptor_walk(state: &AppState, page: &PatchPageProjection) {
        let patch = &state.patches()[0];
        let descriptor = state
            .capabilities()
            .descriptor_for_config(patch.instrument_config())
            .unwrap();
        assert_eq!(page.patch().id(), Some(patch.id()));
        assert_eq!(page.patch().name(), patch.name());
        assert_eq!(page.patch().midi_channel(), Some(patch.channel()));
        assert_eq!(
            page.focused_control_id(),
            state.interaction().patch_control_focus().unwrap()
        );
        assert_eq!(page.engine().active_capability_id(), descriptor.id());
        assert_eq!(page.engine().active_label(), descriptor.label());
        assert_eq!(page.engine().control_id(), PatchControlId::Engine);
        assert_eq!(page.engine().status(), EngineSelectionStatusKind::Ready);
        assert_eq!(
            page.engine().active_graph_revision(),
            GraphRevision::INITIAL
        );
        assert_eq!(page.engine().requested_capability_id(), None);
        assert_eq!(page.engine().request_id(), None);
        assert_eq!(page.engine().target_graph_revision(), None);
        assert_eq!(page.engine().failure(), None);
        assert!(page.engine().editable());
        assert_eq!(
            page.engine().choices().len(),
            state.capabilities().descriptors().len()
        );
        for (choice, installed) in page
            .engine()
            .choices()
            .iter()
            .zip(state.capabilities().descriptors())
        {
            assert_eq!(choice.capability_id(), installed.id());
            assert_eq!(choice.label(), installed.label());
        }
        for (row, spec) in page
            .envelope()
            .iter()
            .zip(crate::synth::VoiceEnvelope::surface_descriptor())
        {
            assert_eq!(row.control_id(), PatchControlId::Envelope(spec.parameter()));
            assert_eq!(row.id(), spec.name());
            assert_eq!(row.label(), spec.label());
            assert_eq!(row.value(), patch.envelope().value(spec.parameter()));
            assert_eq!(row.minimum(), spec.minimum());
            assert_eq!(row.maximum(), spec.maximum());
            assert_eq!(row.unit(), spec.unit());
            assert!(row.editable());
        }
        assert_eq!(page.sections().len(), descriptor.sections().len());
        for (section, declared) in page.sections().iter().zip(descriptor.sections()) {
            assert_eq!(section.id(), declared.id());
            assert_eq!(section.label(), declared.label());
            assert_eq!(section.parameters().len(), declared.parameters().len());
            for (row, spec) in section.parameters().iter().zip(declared.parameters()) {
                assert_eq!(row.id(), spec.id());
                assert_eq!(row.label(), spec.label());
                assert_eq!(row.kind(), spec.kind());
                assert_eq!(row.update(), spec.update());
                assert_eq!(row.range(), spec.range());
                assert_eq!(row.choices(), spec.choices());
                assert_eq!(row.unit(), spec.unit());
                assert!(row.enabled());
                assert!(row.visible());
                assert_eq!(row.patch_interaction(), spec.patch_interaction());
                let structural =
                    spec.patch_interaction() == crate::synth::PatchInteraction::StructuralChoice;
                assert_eq!(row.editable(), structural);
                assert_eq!(
                    row.control_id(),
                    structural.then(|| PatchControlId::Capability(spec.id().clone()))
                );
                match row.value() {
                    PatchPageParameterValue::EmptyAsset => assert!(patch
                        .instrument_config()
                        .asset_reference(spec.id())
                        .is_none()),
                    PatchPageParameterValue::Parameter { value } => {
                        assert_eq!(Some(value), patch.instrument_config().value(spec.id()));
                    }
                    PatchPageParameterValue::Asset { reference } => {
                        assert_eq!(
                            Some(reference),
                            patch.instrument_config().asset_reference(spec.id())
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn gapped_chain_projects_the_occupant_at_its_true_position() {
        let page = project(&state_with_gapped_effect_chain(
            BraidsCapability::new().unwrap().default_config().unwrap(),
        ));
        let slots = page.effects();
        assert_eq!(slots.len(), crate::synth::effect_slot_id::MAX_EFFECT_SLOTS);
        for (position, slot) in slots.iter().enumerate() {
            assert_eq!(slot.slot_index().index(), position);
        }
        // Slot 0 stays projected as empty — a compacting view would have
        // squeezed the occupant down into this position.
        assert!(matches!(
            slots[0].occupancy(),
            PatchPageSlotOccupancy::Empty
        ));
        assert!(slots[0].sections().is_empty());
        let PatchPageSlotOccupancy::Occupied {
            slot_id,
            capability_id,
            ..
        } = slots[1].occupancy()
        else {
            panic!("the occupant must stay projected at slot position 1");
        };
        assert_eq!(slot_id.value(), 2);
        assert_eq!(
            capability_id.as_str(),
            crate::adapter::chorus_capability::CHORUS_CAPABILITY_ID
        );
        assert!(!slots[1].sections().is_empty());
        assert!(matches!(
            slots[2].occupancy(),
            PatchPageSlotOccupancy::Empty
        ));
    }

    #[test]
    fn both_production_capabilities_share_one_exact_generic_projection_walk() {
        let soundfont =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        let soundfont_state = state_with_config(
            create_soundfont_config(
                &soundfont,
                SoundFontInstrument::new(128, 11, false).unwrap(),
            )
            .unwrap(),
        );
        let braids_state =
            state_with_config(BraidsCapability::new().unwrap().default_config().unwrap());

        assert_descriptor_walk(&soundfont_state, &project(&soundfont_state));
        assert_descriptor_walk(&braids_state, &project(&braids_state));
    }

    #[test]
    fn focus_identity_is_independent_from_engine_action_availability() {
        fn selected_row_count(page: &PatchPageProjection) -> usize {
            usize::from(page.engine().control_id() == page.focused_control_id())
                + page
                    .envelope()
                    .iter()
                    .filter(|row| row.control_id() == page.focused_control_id())
                    .count()
        }

        let soundfont =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        let mut state = state_with_config(
            create_soundfont_config(
                &soundfont,
                SoundFontInstrument::new(128, 11, false).unwrap(),
            )
            .unwrap(),
        );

        let engine_ready = project(&state);
        assert_eq!(engine_ready.focused_control_id(), PatchControlId::Engine);
        assert!(engine_ready.engine().editable());
        assert_eq!(selected_row_count(&engine_ready), 1);

        state
            .apply(AppEvent::EnterSurface(SurfaceId::PatchDetail))
            .unwrap();
        for _ in 0..32 {
            if state.interaction().patch_control_focus()
                == Some(PatchControlId::Envelope(
                    crate::synth::VoiceEnvelopeParameter::AttackMilliseconds,
                ))
            {
                break;
            }
            state
                .apply(AppEvent::Navigate(crate::control::Direction::Down))
                .unwrap();
        }
        let attack_ready = project(&state);
        assert_eq!(
            attack_ready.focused_control_id(),
            PatchControlId::Envelope(crate::synth::VoiceEnvelopeParameter::AttackMilliseconds)
        );
        assert!(attack_ready.engine().editable());
        assert_eq!(selected_row_count(&attack_ready), 1);

        state.apply(AppEvent::Return).unwrap();
        state
            .apply(AppEvent::Adjust(crate::control::Direction::Right))
            .unwrap();
        let engine_preparing = project(&state);
        assert_eq!(
            engine_preparing.focused_control_id(),
            PatchControlId::Engine
        );
        assert!(!engine_preparing.engine().editable());
        assert_eq!(selected_row_count(&engine_preparing), 1);

        state
            .apply(AppEvent::EnterSurface(SurfaceId::PatchDetail))
            .unwrap();
        for _ in 0..32 {
            if state.interaction().patch_control_focus()
                == Some(PatchControlId::Envelope(
                    crate::synth::VoiceEnvelopeParameter::AttackMilliseconds,
                ))
            {
                break;
            }
            state
                .apply(AppEvent::Navigate(crate::control::Direction::Down))
                .unwrap();
        }
        let attack_preparing = project(&state);
        assert_eq!(
            attack_preparing.focused_control_id(),
            PatchControlId::Envelope(crate::synth::VoiceEnvelopeParameter::AttackMilliseconds)
        );
        assert!(!attack_preparing.engine().editable());
        assert_eq!(selected_row_count(&attack_preparing), 1);
    }

    #[test]
    fn projection_serialization_and_midi_hash_update_keep_owned_schema_content() {
        let state = state_with_config(BraidsCapability::new().unwrap().default_config().unwrap());
        let page = project(&state);
        let advanced = page.with_state_hash("advanced-hash".to_owned());
        assert!(page.shares_content_with(&advanced));
        assert_eq!(advanced.state_hash(), "advanced-hash");

        fn leaves(value: &Value, prefix: &str, output: &mut BTreeSet<String>) {
            match value {
                Value::Object(object) => {
                    for (name, child) in object {
                        let path = if prefix.is_empty() {
                            name.to_owned()
                        } else {
                            format!("{prefix}.{name}")
                        };
                        leaves(child, &path, output);
                    }
                }
                Value::Array(array) => {
                    for child in array {
                        leaves(child, &format!("{prefix}[]"), output);
                    }
                }
                _ => {
                    output.insert(prefix.to_owned());
                }
            }
        }

        let mut discovered = BTreeSet::new();
        let soundfont =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        for page in [
            project(&state_with_config(
                create_soundfont_config(
                    &soundfont,
                    SoundFontInstrument::new(128, 11, false).unwrap(),
                )
                .unwrap(),
            )),
            page,
            project(&state_with_configured_effect(
                BraidsCapability::new().unwrap().default_config().unwrap(),
            )),
            // Both detail subjects, because they do not carry the same leaves:
            // an instrument subject leaves `detail.slotId` null while an effect
            // subject names its exact occupied slot, and only the SoundFont
            // subject reaches the asset-valued `detail...value.reference.*`
            // rows. A fixture that opens one detail entry cannot see the
            // other's leaves — which is the same partial-enumeration trap that
            // let the subject's own leaves go undeclared upstream.
            project(&state_with_open_detail(
                create_soundfont_config(
                    &soundfont,
                    SoundFontInstrument::new(128, 11, false).unwrap(),
                )
                .unwrap(),
                DetailSubjectFixture::Instrument,
            )),
            project(&state_with_open_detail(
                BraidsCapability::new().unwrap().default_config().unwrap(),
                DetailSubjectFixture::Effect,
            )),
            project(&empty_state()),
        ] {
            leaves(&serde_json::to_value(page).unwrap(), "", &mut discovered);
        }
        let described = PatchPageProjection::serialized_leaf_descriptor()
            .iter()
            .map(|path| (*path).to_owned())
            .collect::<BTreeSet<_>>();
        assert_eq!(described, discovered);
    }
}
