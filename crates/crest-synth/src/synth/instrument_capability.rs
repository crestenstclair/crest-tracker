use crate::kernel::midi_message::MidiMessageKind;
use crate::synth::capability_id::{validate_namespaced_identifier, CapabilityId};
use crate::synth::capability_visualization::CapabilityVisualization;
use crate::synth::parameter_id::ParameterId;
use core::fmt;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// Registry-owned availability of one installed capability descriptor.
///
/// An unavailable descriptor remains installed and may still name the
/// acknowledged configuration, but it cannot receive Choice focus or start a
/// new structural request. The reason is presentation data from the registry,
/// never a substitute capability or a renderer-owned rule.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CapabilityAvailability {
    #[default]
    Available,
    Unavailable {
        reason: String,
    },
}

impl CapabilityAvailability {
    pub const fn is_enabled(&self) -> bool {
        matches!(self, Self::Available)
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Available => None,
            Self::Unavailable { reason } => Some(reason),
        }
    }
}

/// The semantic kind of a stable asset reference.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialOrd, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AssetKind {
    SoundFont,
    SysEx,
    Sfz,
    NeuralModel,
    ImpulseResponse,
    Sample,
    Other,
}

/// A control-side asset identity containing no decoded or prepared state.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetReference {
    kind: AssetKind,
    locator: String,
}

impl AssetReference {
    pub fn new(kind: AssetKind, locator: impl Into<String>) -> Result<Self, CapabilityError> {
        let locator = locator.into();
        if locator.is_empty() {
            return Err(CapabilityError::EmptyAssetLocator);
        }
        Ok(Self { kind, locator })
    }

    pub const fn kind(&self) -> AssetKind {
        self.kind
    }

    pub fn locator(&self) -> &str {
        &self.locator
    }
}

/// The closed tagged value union shared by every instrument capability.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum ParameterValue {
    Continuous(f64),
    Stepped(i64),
    Choice(String),
    Toggle(bool),
}

impl ParameterValue {
    pub fn continuous(value: f64) -> Result<Self, CapabilityError> {
        if !value.is_finite() {
            return Err(CapabilityError::NonFiniteContinuousValue);
        }
        Ok(Self::Continuous(value))
    }

    pub const fn kind(&self) -> ParameterKind {
        match self {
            Self::Continuous(_) => ParameterKind::Continuous,
            Self::Stepped(_) => ParameterKind::Stepped,
            Self::Choice(_) => ParameterKind::Choice,
            Self::Toggle(_) => ParameterKind::Toggle,
        }
    }
}

/// One semantic parameter paired with its typed value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParameterAssignment {
    parameter_id: ParameterId,
    value: ParameterValue,
}

impl ParameterAssignment {
    pub const fn new(parameter_id: ParameterId, value: ParameterValue) -> Self {
        Self {
            parameter_id,
            value,
        }
    }

    pub const fn parameter_id(&self) -> &ParameterId {
        &self.parameter_id
    }

    pub const fn value(&self) -> &ParameterValue {
        &self.value
    }
}

/// One asset-valued semantic parameter paired with its stable reference.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetAssignment {
    parameter_id: ParameterId,
    reference: AssetReference,
}

impl AssetAssignment {
    pub const fn new(parameter_id: ParameterId, reference: AssetReference) -> Self {
        Self {
            parameter_id,
            reference,
        }
    }

    pub const fn parameter_id(&self) -> &ParameterId {
        &self.parameter_id
    }

    pub const fn reference(&self) -> &AssetReference {
        &self.reference
    }
}

/// The data representation accepted by a parameter specification.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ParameterKind {
    Continuous,
    Stepped,
    Choice,
    Toggle,
    Asset,
}

/// The future real-time handoff category declared by a parameter.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ParameterUpdate {
    Scalar,
    Structural,
}

/// Whether a descriptor parameter participates in the PATCH focus/edit surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PatchInteraction {
    ReadOnly,
    StructuralChoice,
    ScalarEdit,
}

/// Patch-local note-allocation policy declared by one capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum VoicePolicy {
    /// Every prepared Patch owns this exact independent voice capacity.
    FixedPerPatch { voices: u16 },
    /// One Patch-local engine instance owns allocation under its prepared bound.
    EngineManaged,
    /// Voice storage is prepared from the Patch budget; this value only seeds new Patches.
    Configurable {
        #[serde(rename = "defaultVoices")]
        default_voices: u16,
    },
}

impl VoicePolicy {
    /// Returns how many of one Patch's notes this capability can sound at once.
    ///
    /// A `FixedPerPatch` capability declares its own per-Patch capacity
    /// directly. `EngineManaged` delegates allocation to one Patch-local engine
    /// instance prepared with
    /// [`ENGINE_MANAGED_POLYPHONY_CEILING`](crate::synth::voice_limit::ENGINE_MANAGED_POLYPHONY_CEILING)
    /// voice slots; the variant carries no per-capability number because
    /// exactly one engine-managed capability is installed, and that constant
    /// documents the pinning that keeps the two in step.
    ///
    /// This is the seeding oracle for [`crate::synth::VoiceLimit`]: each Patch's
    /// limit comes from its own capability's ceiling, never from a value shared
    /// across capabilities.
    pub const fn initial_voices(self) -> u16 {
        match self {
            Self::Configurable { default_voices } => default_voices,
            _ => self.polyphony_ceiling(),
        }
    }

    pub const fn polyphony_ceiling(self) -> u16 {
        match self {
            Self::Configurable { .. } => u16::MAX,
            Self::FixedPerPatch { voices } => voices,
            Self::EngineManaged => crate::synth::voice_limit::ENGINE_MANAGED_POLYPHONY_CEILING,
        }
    }
}

/// A semantic scalar adjustment independent of keyboard/controller bindings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParameterAdjustment {
    FineDecrease,
    FineIncrease,
    CoarseDecrease,
    CoarseIncrease,
}

impl ParameterAdjustment {
    const fn increases(self) -> bool {
        matches!(self, Self::FineIncrease | Self::CoarseIncrease)
    }

    const fn coarse(self) -> bool {
        matches!(self, Self::CoarseDecrease | Self::CoarseIncrease)
    }
}

/// A typed default for either a scalar parameter or an asset parameter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum ParameterDefault {
    Value(ParameterValue),
    Asset(AssetReference),
}

/// Inclusive numeric bounds declared by a continuous or stepped parameter.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParameterRange {
    minimum: f64,
    maximum: f64,
}

impl ParameterRange {
    pub fn new(minimum: f64, maximum: f64) -> Result<Self, CapabilityError> {
        if !minimum.is_finite() || !maximum.is_finite() || minimum > maximum {
            return Err(CapabilityError::InvalidNumericRange);
        }
        Ok(Self { minimum, maximum })
    }

    pub const fn minimum(self) -> f64 {
        self.minimum
    }

    pub const fn maximum(self) -> f64 {
        self.maximum
    }

    fn contains(self, value: f64) -> bool {
        value >= self.minimum && value <= self.maximum
    }
}

/// One stable choice identity and its presentation label.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParameterChoice {
    id: String,
    label: String,
}

impl ParameterChoice {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Result<Self, CapabilityError> {
        let id = id.into();
        validate_namespaced_identifier(&id)
            .map_err(|_| CapabilityError::InvalidMetadataIdentifier(id.clone()))?;
        let label = label.into();
        if label.is_empty() {
            return Err(CapabilityError::EmptyLabel);
        }
        Ok(Self { id, label })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }
}

/// A declarative dependency on an earlier parameter value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParameterPredicate {
    parameter_id: ParameterId,
    equals: ParameterValue,
}

impl ParameterPredicate {
    pub const fn new(parameter_id: ParameterId, equals: ParameterValue) -> Self {
        Self {
            parameter_id,
            equals,
        }
    }

    pub const fn parameter_id(&self) -> &ParameterId {
        &self.parameter_id
    }

    pub const fn equals(&self) -> &ParameterValue {
        &self.equals
    }
}

/// A named region of a continuous parameter, evaluated on its f32 DSP scalar.
/// Optional affine amounts use the upstream's whole-number display convention.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinuousValueLabel {
    range: ParameterRange,
    label: String,
    value_scale: Option<f64>,
    value_offset: f64,
    value_unit: String,
}

impl ContinuousValueLabel {
    pub fn new(range: ParameterRange, label: impl Into<String>) -> Self {
        Self {
            range,
            label: label.into(),
            value_scale: None,
            value_offset: 0.0,
            value_unit: String::new(),
        }
    }

    pub fn with_amount(mut self, scale: f64, offset: f64, unit: impl Into<String>) -> Self {
        self.value_scale = Some(scale);
        self.value_offset = offset;
        self.value_unit = unit.into();
        self
    }

    fn text(&self, value: f64) -> Cow<'_, str> {
        match self.value_scale {
            Some(scale) => Cow::Owned(format!(
                "{} {:.0}{}",
                self.label,
                value * scale + self.value_offset,
                self.value_unit
            )),
            None => Cow::Borrowed(&self.label),
        }
    }
}

/// Immutable schema and presentation metadata for one capability parameter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParameterSpec {
    id: ParameterId,
    label: String,
    kind: ParameterKind,
    update: ParameterUpdate,
    patch_interaction: PatchInteraction,
    default_value: ParameterDefault,
    range: Option<ParameterRange>,
    choices: Vec<ParameterChoice>,
    /// Optional names for every integer from the inclusive range minimum.
    #[serde(default)]
    stepped_labels: Vec<String>,
    /// Presentation only: continuous editing and saved values remain unchanged.
    #[serde(default)]
    continuous_labels: Vec<ContinuousValueLabel>,
    fine_step: Option<f64>,
    coarse_step: Option<f64>,
    unit: Option<String>,
    formatter: String,
    enabled_when: Option<ParameterPredicate>,
    visible_when: Option<ParameterPredicate>,
}

impl ParameterSpec {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: ParameterId,
        label: impl Into<String>,
        kind: ParameterKind,
        update: ParameterUpdate,
        default_value: ParameterDefault,
        range: Option<ParameterRange>,
        choices: Vec<ParameterChoice>,
        fine_step: Option<f64>,
        coarse_step: Option<f64>,
        unit: Option<String>,
        formatter: impl Into<String>,
        enabled_when: Option<ParameterPredicate>,
        visible_when: Option<ParameterPredicate>,
    ) -> Result<Self, CapabilityError> {
        Self::new_with_patch_interaction(
            id,
            label,
            kind,
            update,
            PatchInteraction::ReadOnly,
            default_value,
            range,
            choices,
            fine_step,
            coarse_step,
            unit,
            formatter,
            enabled_when,
            visible_when,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_patch_interaction(
        id: ParameterId,
        label: impl Into<String>,
        kind: ParameterKind,
        update: ParameterUpdate,
        patch_interaction: PatchInteraction,
        default_value: ParameterDefault,
        range: Option<ParameterRange>,
        choices: Vec<ParameterChoice>,
        fine_step: Option<f64>,
        coarse_step: Option<f64>,
        unit: Option<String>,
        formatter: impl Into<String>,
        enabled_when: Option<ParameterPredicate>,
        visible_when: Option<ParameterPredicate>,
    ) -> Result<Self, CapabilityError> {
        let spec = Self {
            id,
            label: label.into(),
            kind,
            update,
            patch_interaction,
            default_value,
            range,
            choices,
            stepped_labels: Vec::new(),
            continuous_labels: Vec::new(),
            fine_step,
            coarse_step,
            unit,
            formatter: formatter.into(),
            enabled_when,
            visible_when,
        };
        spec.validate_shape()?;
        Ok(spec)
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

    pub const fn default_value(&self) -> &ParameterDefault {
        &self.default_value
    }

    pub const fn range(&self) -> Option<ParameterRange> {
        self.range
    }

    pub fn choices(&self) -> &[ParameterChoice] {
        &self.choices
    }

    /// Adds display names without changing the stored numeric value or RT encoding.
    pub fn with_stepped_labels(mut self, labels: Vec<String>) -> Result<Self, CapabilityError> {
        self.stepped_labels = labels;
        self.validate_shape()?;
        Ok(self)
    }

    pub fn with_continuous_labels(
        mut self,
        labels: Vec<ContinuousValueLabel>,
    ) -> Result<Self, CapabilityError> {
        self.continuous_labels = labels;
        self.validate_shape()?;
        Ok(self)
    }

    /// Resolves descriptor-owned presentation without changing the stored value.
    pub fn value_label(&self, value: &ParameterValue) -> Option<Cow<'_, str>> {
        match value {
            ParameterValue::Choice(id) => self
                .choices
                .iter()
                .find(|choice| choice.id() == id)
                .map(|choice| Cow::Borrowed(choice.label())),
            ParameterValue::Stepped(value) => {
                let minimum = self.range?.minimum as i128;
                let index = usize::try_from(i128::from(*value).checked_sub(minimum)?).ok()?;
                self.stepped_labels
                    .get(index)
                    .map(|label| Cow::Borrowed(label.as_str()))
            }
            ParameterValue::Continuous(value) => {
                if !value.is_finite() || !self.range?.contains(*value) {
                    return None;
                }
                let scalar = f64::from(*value as f32);
                self.continuous_labels
                    .iter()
                    .find(|label| label.range.contains(scalar))
                    .map(|label| label.text(scalar))
            }
            _ => None,
        }
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

    pub const fn enabled_when(&self) -> Option<&ParameterPredicate> {
        self.enabled_when.as_ref()
    }

    pub const fn visible_when(&self) -> Option<&ParameterPredicate> {
        self.visible_when.as_ref()
    }

    /// Encodes one validated Scalar value for descriptor-ordered RT storage.
    pub fn scalar_value(&self, value: &ParameterValue) -> Result<f32, CapabilityError> {
        if self.update != ParameterUpdate::Scalar || self.kind == ParameterKind::Asset {
            return Err(CapabilityError::StructuralParameter(self.id.clone()));
        }
        validate_value(self, value)?;
        let encoded = match value {
            ParameterValue::Continuous(value) => *value as f32,
            ParameterValue::Stepped(value) => *value as f32,
            ParameterValue::Choice(value) => self
                .choices
                .iter()
                .position(|choice| choice.id() == value)
                .ok_or_else(|| CapabilityError::UnknownChoice(self.id.clone()))?
                as f32,
            ParameterValue::Toggle(value) => u8::from(*value) as f32,
        };
        if !encoded.is_finite() {
            return Err(CapabilityError::ScalarEncodingOutOfRange(self.id.clone()));
        }
        Ok(encoded)
    }

    /// Produces a typed, bounded replacement for one current Scalar value.
    pub fn adjusted_scalar_value(
        &self,
        current: &ParameterValue,
        adjustment: ParameterAdjustment,
    ) -> Result<ParameterValue, CapabilityError> {
        if self.update != ParameterUpdate::Scalar || self.kind == ParameterKind::Asset {
            return Err(CapabilityError::StructuralParameter(self.id.clone()));
        }
        validate_value(self, current)?;
        let next = match current {
            ParameterValue::Continuous(current) => {
                let range = self
                    .range
                    .ok_or_else(|| CapabilityError::InvalidParameterShape {
                        parameter_id: self.id.clone(),
                        reason: "continuous Scalar has no numeric range",
                    })?;
                let step = if adjustment.coarse() {
                    self.coarse_step
                } else {
                    self.fine_step
                }
                .ok_or_else(|| CapabilityError::InvalidParameterShape {
                    parameter_id: self.id.clone(),
                    reason: "continuous Scalar has no adjustment step",
                })?;
                ParameterValue::Continuous(adjust_numeric(
                    *current,
                    range.minimum(),
                    range.maximum(),
                    self.fine_step
                        .expect("validated numeric Scalar has a fine step"),
                    step,
                    adjustment.increases(),
                ))
            }
            ParameterValue::Stepped(current) => {
                let range = self
                    .range
                    .ok_or_else(|| CapabilityError::InvalidParameterShape {
                        parameter_id: self.id.clone(),
                        reason: "stepped Scalar has no numeric range",
                    })?;
                let step = if adjustment.coarse() {
                    self.coarse_step
                } else {
                    self.fine_step
                }
                .ok_or_else(|| CapabilityError::InvalidParameterShape {
                    parameter_id: self.id.clone(),
                    reason: "stepped Scalar has no adjustment step",
                })? as i64;
                let delta = if adjustment.increases() { step } else { -step };
                ParameterValue::Stepped(
                    current
                        .saturating_add(delta)
                        .clamp(range.minimum() as i64, range.maximum() as i64),
                )
            }
            ParameterValue::Choice(current) => {
                let index = self
                    .choices
                    .iter()
                    .position(|choice| choice.id() == current)
                    .ok_or_else(|| CapabilityError::UnknownChoice(self.id.clone()))?;
                let next = if adjustment.increases() {
                    index.saturating_add(1).min(self.choices.len() - 1)
                } else {
                    index.saturating_sub(1)
                };
                ParameterValue::Choice(self.choices[next].id().to_owned())
            }
            ParameterValue::Toggle(_) => ParameterValue::Toggle(adjustment.increases()),
        };
        if &next == current {
            return Err(CapabilityError::ScalarValueAtBoundary(self.id.clone()));
        }
        validate_value(self, &next)?;
        Ok(next)
    }

    pub(crate) fn validate_shape(&self) -> Result<(), CapabilityError> {
        if self.label.is_empty() {
            return Err(CapabilityError::EmptyLabel);
        }
        validate_namespaced_identifier(&self.formatter)
            .map_err(|_| CapabilityError::InvalidMetadataIdentifier(self.formatter.clone()))?;
        if self.unit.as_ref().is_some_and(String::is_empty) {
            return Err(CapabilityError::EmptyUnit);
        }
        if !self.stepped_labels.is_empty() {
            let valid = self.kind == ParameterKind::Stepped
                && self.range.is_some_and(|range| {
                    let minimum = range.minimum as i128;
                    let maximum = range.maximum as i128;
                    range.minimum.is_finite()
                        && range.maximum.is_finite()
                        && range.minimum.fract() == 0.0
                        && range.maximum.fract() == 0.0
                        && minimum >= i128::from(i64::MIN)
                        && maximum <= i128::from(i64::MAX)
                        && maximum
                            .checked_sub(minimum)
                            .and_then(|span| span.checked_add(1))
                            == Some(self.stepped_labels.len() as i128)
                })
                && self
                    .stepped_labels
                    .iter()
                    .all(|label| !label.trim().is_empty());
            if !valid {
                return Err(CapabilityError::InvalidParameterShape {
                    parameter_id: self.id.clone(),
                    reason: "stepped labels require one nonempty name per integer in the range",
                });
            }
        }
        if !self.continuous_labels.is_empty() {
            let valid = self.kind == ParameterKind::Continuous
                && self.range.is_some_and(|range| {
                    self.continuous_labels.iter().all(|label| {
                        let bounds = label.range;
                        bounds.minimum.is_finite()
                            && bounds.maximum.is_finite()
                            && bounds.minimum <= bounds.maximum
                            && range.contains(bounds.minimum)
                            && range.contains(bounds.maximum)
                            && !label.label.trim().is_empty()
                            && label.value_offset.is_finite()
                            && label.value_scale.is_none_or(|scale| {
                                scale.is_finite()
                                    && (bounds.minimum * scale + label.value_offset).is_finite()
                                    && (bounds.maximum * scale + label.value_offset).is_finite()
                            })
                    })
                })
                && self
                    .continuous_labels
                    .windows(2)
                    .all(|pair| pair[0].range.maximum < pair[1].range.minimum);
            if !valid {
                return Err(CapabilityError::InvalidParameterShape {
                    parameter_id: self.id.clone(),
                    reason: "continuous labels require ordered nonoverlapping finite ranges and nonempty names",
                });
            }
        }
        if self.choices.len() > 1 {
            // Large asset catalogs must not make every config validation
            // quadratic. Keep declaration ordinals so the first repeated
            // choice still determines the same typed error as the schema walk.
            let mut choice_ids = self
                .choices
                .iter()
                .enumerate()
                .map(|(index, choice)| (choice.id.as_str(), index))
                .collect::<Vec<_>>();
            choice_ids.sort_unstable();
            if let Some((choice_id, _)) = choice_ids
                .windows(2)
                .filter(|pair| pair[0].0 == pair[1].0)
                .map(|pair| pair[1])
                .min_by_key(|(_, index)| *index)
            {
                return Err(CapabilityError::DuplicateChoice {
                    parameter_id: self.id.clone(),
                    choice_id: choice_id.to_owned(),
                });
            }
        }
        match self.patch_interaction {
            PatchInteraction::StructuralChoice
                if self.kind != ParameterKind::Choice
                    || self.update != ParameterUpdate::Structural
                    || self.choices.len() < 2 =>
            {
                return Err(CapabilityError::InvalidParameterShape {
                    parameter_id: self.id.clone(),
                    reason: "StructuralChoice PATCH interaction requires a Structural Choice with at least two choices",
                });
            }
            PatchInteraction::ScalarEdit
                if self.kind == ParameterKind::Asset || self.update != ParameterUpdate::Scalar =>
            {
                return Err(CapabilityError::InvalidParameterShape {
                    parameter_id: self.id.clone(),
                    reason: "ScalarEdit PATCH interaction requires a non-asset Scalar",
                });
            }
            _ => {}
        }

        match self.kind {
            ParameterKind::Continuous | ParameterKind::Stepped => {
                let range = self
                    .range
                    .ok_or_else(|| CapabilityError::InvalidParameterShape {
                        parameter_id: self.id.clone(),
                        reason: "numeric parameters require finite ordered bounds",
                    })?;
                if !self.choices.is_empty()
                    || !positive_finite(self.fine_step)
                    || !positive_finite(self.coarse_step)
                {
                    return Err(CapabilityError::InvalidParameterShape {
                        parameter_id: self.id.clone(),
                        reason: "numeric parameters require positive steps and no choices",
                    });
                }
                validate_value(self, default_parameter_value(&self.default_value)?)?;
                if self.kind == ParameterKind::Stepped
                    && (range.minimum.fract() != 0.0 || range.maximum.fract() != 0.0)
                {
                    return Err(CapabilityError::InvalidParameterShape {
                        parameter_id: self.id.clone(),
                        reason: "stepped bounds must be integral",
                    });
                }
            }
            ParameterKind::Choice => {
                if self.range.is_some()
                    || self.fine_step.is_some()
                    || self.coarse_step.is_some()
                    || self.choices.is_empty()
                {
                    return Err(CapabilityError::InvalidParameterShape {
                        parameter_id: self.id.clone(),
                        reason: "choice parameters require choices and no numeric metadata",
                    });
                }
                validate_value(self, default_parameter_value(&self.default_value)?)?;
            }
            ParameterKind::Toggle => {
                if self.range.is_some()
                    || !self.choices.is_empty()
                    || self.fine_step.is_some()
                    || self.coarse_step.is_some()
                {
                    return Err(CapabilityError::InvalidParameterShape {
                        parameter_id: self.id.clone(),
                        reason: "toggle parameters cannot declare numeric or choice metadata",
                    });
                }
                validate_value(self, default_parameter_value(&self.default_value)?)?;
            }
            ParameterKind::Asset => {
                if self.update != ParameterUpdate::Structural
                    || !matches!(self.default_value, ParameterDefault::Asset(_))
                    || self.range.is_some()
                    || !self.choices.is_empty()
                    || self.fine_step.is_some()
                    || self.coarse_step.is_some()
                {
                    return Err(CapabilityError::InvalidParameterShape {
                        parameter_id: self.id.clone(),
                        reason: "asset parameters require a Structural asset default only",
                    });
                }
            }
        }
        Ok(())
    }
}

fn positive_finite(value: Option<f64>) -> bool {
    value.is_some_and(|value| value.is_finite() && value > 0.0)
}

fn adjust_numeric(
    current: f64,
    minimum: f64,
    maximum: f64,
    fine_step: f64,
    adjustment_step: f64,
    increase: bool,
) -> f64 {
    let scale = decimal_scale(fine_step);
    let current_units = (current * scale).round();
    let step_units = (adjustment_step * scale).round();
    let delta = if increase { step_units } else { -step_units };
    ((current_units + delta) / scale).clamp(minimum, maximum)
}

fn decimal_scale(step: f64) -> f64 {
    let mut scale = 1.0;
    while scale < 1_000_000.0 && (step * scale).fract().abs() > f64::EPSILON * scale {
        scale *= 10.0;
    }
    scale
}

fn default_parameter_value(default: &ParameterDefault) -> Result<&ParameterValue, CapabilityError> {
    match default {
        ParameterDefault::Value(value) => Ok(value),
        ParameterDefault::Asset(_) => Err(CapabilityError::InvalidDefaultKind),
    }
}

/// An ordered presentation section owned by a capability descriptor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitySection {
    #[serde(default)]
    leading_controls: bool,
    id: String,
    label: String,
    parameters: Vec<ParameterSpec>,
}

impl CapabilitySection {
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        parameters: Vec<ParameterSpec>,
    ) -> Result<Self, CapabilityError> {
        let id = id.into();
        validate_namespaced_identifier(&id)
            .map_err(|_| CapabilityError::InvalidMetadataIdentifier(id.clone()))?;
        let label = label.into();
        if label.is_empty() {
            return Err(CapabilityError::EmptyLabel);
        }
        if parameters.is_empty() {
            return Err(CapabilityError::EmptySection(id));
        }
        Ok(Self {
            leading_controls: false,
            id,
            label,
            parameters,
        })
    }

    /// Controls selecting the displayed part precede its visualizations.
    pub fn with_leading_controls(mut self) -> Self {
        self.leading_controls = true;
        self
    }

    pub const fn leading_controls(&self) -> bool {
        self.leading_controls
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn parameters(&self) -> &[ParameterSpec] {
        &self.parameters
    }
}

/// Whether an asset parameter must occur in every config.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetRequirement {
    parameter_id: ParameterId,
    required: bool,
}

impl AssetRequirement {
    pub const fn new(parameter_id: ParameterId, required: bool) -> Self {
        Self {
            parameter_id,
            required,
        }
    }

    pub const fn parameter_id(&self) -> &ParameterId {
        &self.parameter_id
    }

    pub const fn required(&self) -> bool {
        self.required
    }
}

/// Musical families used to browse installed instruments. Providers own the
/// classification; the order is independent of registry installation order.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstrumentCategory {
    #[default]
    Synths,
    KeysAndOrgans,
    Strings,
    WindsAndVoices,
    Resonators,
    DrumsAndPercussion,
    Samplers,
}

impl InstrumentCategory {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Synths => "Synths",
            Self::KeysAndOrgans => "Keys & Organs",
            Self::Strings => "Strings",
            Self::WindsAndVoices => "Winds & Voices",
            Self::Resonators => "Resonators",
            Self::DrumsAndPercussion => "Drums & Percussion",
            Self::Samplers => "Samplers",
        }
    }
}

/// The immutable ordered control-side schema for one instrument capability.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDescriptor {
    id: CapabilityId,
    label: String,
    semantic_accent: String,
    #[serde(default)]
    instrument_category: InstrumentCategory,
    #[serde(default)]
    availability: CapabilityAvailability,
    sections: Vec<CapabilitySection>,
    #[serde(default)]
    visualizations: Vec<CapabilityVisualization>,
    asset_requirements: Vec<AssetRequirement>,
    voice_policy: VoicePolicy,
    supported_midi_kinds: Vec<MidiMessageKind>,
    #[serde(default)]
    asset_scoped_choices: bool,
}

impl CapabilityDescriptor {
    pub fn new(
        id: CapabilityId,
        label: impl Into<String>,
        semantic_accent: impl Into<String>,
        sections: Vec<CapabilitySection>,
        asset_requirements: Vec<AssetRequirement>,
        voice_policy: VoicePolicy,
        supported_midi_kinds: Vec<MidiMessageKind>,
    ) -> Result<Self, CapabilityError> {
        let descriptor = Self {
            id,
            label: label.into(),
            semantic_accent: semantic_accent.into(),
            instrument_category: InstrumentCategory::default(),
            availability: CapabilityAvailability::Available,
            sections,
            visualizations: vec![CapabilityVisualization::envelope(
                "shared.envelope",
                "Envelope",
            )?],
            asset_requirements,
            voice_policy,
            supported_midi_kinds,
            asset_scoped_choices: false,
        };
        descriptor.validate()?;
        Ok(descriptor)
    }

    pub const fn id(&self) -> &CapabilityId {
        &self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn semantic_accent(&self) -> &str {
        &self.semantic_accent
    }

    pub const fn instrument_category(&self) -> InstrumentCategory {
        self.instrument_category
    }

    pub fn with_instrument_category(mut self, category: InstrumentCategory) -> Self {
        self.instrument_category = category;
        self
    }

    pub const fn availability(&self) -> &CapabilityAvailability {
        &self.availability
    }

    pub fn with_availability(mut self, availability: CapabilityAvailability) -> Self {
        self.availability = availability;
        self
    }

    pub fn sections(&self) -> &[CapabilitySection] {
        &self.sections
    }

    pub fn visualizations(&self) -> &[CapabilityVisualization] {
        &self.visualizations
    }

    /// Adds descriptor-owned informative shapes without changing the
    /// descriptor's canonical control schema.
    pub fn with_visualizations(
        mut self,
        visualizations: impl IntoIterator<Item = CapabilityVisualization>,
    ) -> Result<Self, CapabilityError> {
        self.visualizations.extend(visualizations);
        self.validate()?;
        Ok(self)
    }

    pub fn asset_requirements(&self) -> &[AssetRequirement] {
        &self.asset_requirements
    }

    pub const fn voice_policy(&self) -> VoicePolicy {
        self.voice_policy
    }

    pub fn supported_midi_kinds(&self) -> &[MidiMessageKind] {
        &self.supported_midi_kinds
    }

    /// Structural choices must be resolved against this config's exact asset set.
    pub fn with_asset_scoped_choices(mut self) -> Self {
        self.asset_scoped_choices = true;
        self
    }

    pub fn parameter(&self, id: &ParameterId) -> Option<&ParameterSpec> {
        self.sections
            .iter()
            .flat_map(CapabilitySection::parameters)
            .find(|parameter| parameter.id() == id)
    }

    pub fn parameters(&self) -> impl Iterator<Item = &ParameterSpec> {
        self.sections.iter().flat_map(CapabilitySection::parameters)
    }

    pub fn default_assets(&self) -> Vec<AssetAssignment> {
        self.parameters()
            .filter(|parameter| {
                self.asset_requirements
                    .iter()
                    .any(|asset| asset.parameter_id() == parameter.id() && asset.required())
            })
            .filter_map(|parameter| match parameter.default_value() {
                ParameterDefault::Asset(reference) => Some(AssetAssignment::new(
                    parameter.id().clone(),
                    reference.clone(),
                )),
                _ => None,
            })
            .collect()
    }

    /// Returns live Scalar parameters exactly once in immutable descriptor order.
    pub fn scalar_parameters(&self) -> impl Iterator<Item = &ParameterSpec> {
        self.parameters()
            .filter(|parameter| parameter.update() == ParameterUpdate::Scalar)
    }

    pub fn scalar_parameter_count(&self) -> usize {
        self.scalar_parameters().count()
    }

    /// Validates caller-owned assignments and returns canonical descriptor order.
    pub fn create_config(
        &self,
        values: &[ParameterAssignment],
        asset_references: &[AssetAssignment],
    ) -> Result<InstrumentConfig, CapabilityError> {
        self.validate()?;
        reject_duplicate_values(values)?;
        reject_duplicate_assets(asset_references)?;

        for assignment in values {
            let spec = self.parameter(assignment.parameter_id()).ok_or_else(|| {
                CapabilityError::UndeclaredParameter(assignment.parameter_id().clone())
            })?;
            if spec.kind() == ParameterKind::Asset {
                return Err(CapabilityError::WrongValueKind(spec.id().clone()));
            }
        }
        for assignment in asset_references {
            let spec = self.parameter(assignment.parameter_id()).ok_or_else(|| {
                CapabilityError::UndeclaredAsset(assignment.parameter_id().clone())
            })?;
            if spec.kind() != ParameterKind::Asset {
                return Err(CapabilityError::WrongAssetKind(spec.id().clone()));
            }
        }

        let mut ordered_values = Vec::with_capacity(values.len());
        let mut ordered_assets = Vec::with_capacity(asset_references.len());
        for spec in self.parameters() {
            if spec.kind() == ParameterKind::Asset {
                let supplied = asset_references
                    .iter()
                    .find(|assignment| assignment.parameter_id() == spec.id());
                let required = self
                    .asset_requirements
                    .iter()
                    .find(|requirement| requirement.parameter_id() == spec.id())
                    .is_some_and(AssetRequirement::required);
                match supplied {
                    Some(assignment) => {
                        validate_asset(spec, assignment.reference())?;
                        ordered_assets.push(assignment.clone());
                    }
                    None if required => {
                        return Err(CapabilityError::MissingAsset(spec.id().clone()));
                    }
                    None => {}
                }
            } else {
                let assignment = values
                    .iter()
                    .find(|assignment| assignment.parameter_id() == spec.id())
                    .ok_or_else(|| CapabilityError::MissingParameter(spec.id().clone()))?;
                validate_value(spec, assignment.value())?;
                ordered_values.push(assignment.clone());
            }
        }

        validate_dependencies(self, &ordered_values)?;
        Ok(InstrumentConfig {
            capability_id: self.id.clone(),
            values: ordered_values,
            asset_references: ordered_assets,
        })
    }

    fn validate(&self) -> Result<(), CapabilityError> {
        if self.label.is_empty() {
            return Err(CapabilityError::EmptyLabel);
        }
        if self.availability.reason().is_some_and(str::is_empty) {
            return Err(CapabilityError::EmptyAvailabilityReason);
        }
        validate_namespaced_identifier(&self.semantic_accent).map_err(|_| {
            CapabilityError::InvalidMetadataIdentifier(self.semantic_accent.clone())
        })?;
        if matches!(
            self.voice_policy,
            VoicePolicy::FixedPerPatch { voices: 0 }
                | VoicePolicy::Configurable { default_voices: 0 }
        ) {
            return Err(CapabilityError::ZeroFixedVoiceCount);
        }
        if self.supported_midi_kinds.is_empty() {
            return Err(CapabilityError::NoSupportedMidiKinds);
        }

        let mut prior_parameters: Vec<&ParameterSpec> = Vec::new();
        for (section_index, section) in self.sections.iter().enumerate() {
            if self.sections[..section_index]
                .iter()
                .any(|prior| prior.id == section.id)
            {
                return Err(CapabilityError::DuplicateSection(section.id.clone()));
            }
            if section.parameters.is_empty() {
                return Err(CapabilityError::EmptySection(section.id.clone()));
            }
            for spec in &section.parameters {
                spec.validate_shape()?;
                if prior_parameters.iter().any(|prior| prior.id == spec.id) {
                    return Err(CapabilityError::DuplicateParameter(spec.id.clone()));
                }
                for predicate in [spec.enabled_when(), spec.visible_when()]
                    .into_iter()
                    .flatten()
                {
                    let dependency = prior_parameters
                        .iter()
                        .copied()
                        .find(|prior| prior.id() == predicate.parameter_id())
                        .ok_or_else(|| CapabilityError::InvalidDependency {
                            parameter_id: spec.id.clone(),
                            dependency_id: predicate.parameter_id().clone(),
                        })?;
                    validate_value(dependency, predicate.equals()).map_err(|_| {
                        CapabilityError::InvalidDependency {
                            parameter_id: spec.id.clone(),
                            dependency_id: predicate.parameter_id().clone(),
                        }
                    })?;
                }
                prior_parameters.push(spec);
            }
        }

        for (index, visualization) in self.visualizations.iter().enumerate() {
            if self.visualizations[..index]
                .iter()
                .any(|prior| prior.id() == visualization.id())
            {
                return Err(CapabilityError::DuplicateVisualization(
                    visualization.id().to_owned(),
                ));
            }
            if let CapabilityVisualization::Waveform {
                asset_parameter_id,
                landmarks,
                ..
            } = visualization
            {
                if self
                    .parameter(asset_parameter_id)
                    .is_none_or(|parameter| parameter.kind() != ParameterKind::Asset)
                    || landmarks
                        .iter()
                        .any(|landmark| self.parameter(landmark.parameter_id()).is_none())
                {
                    return Err(CapabilityError::InvalidVisualization(
                        visualization.id().to_owned(),
                    ));
                }
            }
        }

        for (index, requirement) in self.asset_requirements.iter().enumerate() {
            if self.asset_requirements[..index]
                .iter()
                .any(|prior| prior.parameter_id == requirement.parameter_id)
            {
                return Err(CapabilityError::DuplicateAssetRequirement(
                    requirement.parameter_id.clone(),
                ));
            }
            let parameter = self.parameter(&requirement.parameter_id).ok_or_else(|| {
                CapabilityError::UndeclaredAsset(requirement.parameter_id.clone())
            })?;
            if parameter.kind() != ParameterKind::Asset {
                return Err(CapabilityError::WrongAssetKind(
                    requirement.parameter_id.clone(),
                ));
            }
        }
        for parameter in self
            .parameters()
            .filter(|parameter| parameter.kind == ParameterKind::Asset)
        {
            if !self
                .asset_requirements
                .iter()
                .any(|requirement| requirement.parameter_id() == parameter.id())
            {
                return Err(CapabilityError::MissingAssetRequirement(
                    parameter.id().clone(),
                ));
            }
        }

        for (index, kind) in self.supported_midi_kinds.iter().enumerate() {
            if self.supported_midi_kinds[..index].contains(kind) {
                return Err(CapabilityError::DuplicateMidiKind(*kind));
            }
        }
        Ok(())
    }
}

fn reject_duplicate_values(values: &[ParameterAssignment]) -> Result<(), CapabilityError> {
    for (index, assignment) in values.iter().enumerate() {
        if values[..index]
            .iter()
            .any(|prior| prior.parameter_id() == assignment.parameter_id())
        {
            return Err(CapabilityError::DuplicateAssignment(
                assignment.parameter_id().clone(),
            ));
        }
    }
    Ok(())
}

fn reject_duplicate_assets(values: &[AssetAssignment]) -> Result<(), CapabilityError> {
    for (index, assignment) in values.iter().enumerate() {
        if values[..index]
            .iter()
            .any(|prior| prior.parameter_id() == assignment.parameter_id())
        {
            return Err(CapabilityError::DuplicateAsset(
                assignment.parameter_id().clone(),
            ));
        }
    }
    Ok(())
}

fn validate_value(spec: &ParameterSpec, value: &ParameterValue) -> Result<(), CapabilityError> {
    if value.kind() != spec.kind() {
        return Err(CapabilityError::WrongValueKind(spec.id().clone()));
    }
    match value {
        ParameterValue::Continuous(value) => {
            if !value.is_finite() {
                return Err(CapabilityError::NonFiniteContinuousValue);
            }
            if !spec.range.is_some_and(|range| range.contains(*value)) {
                return Err(CapabilityError::ValueOutOfRange(spec.id().clone()));
            }
        }
        ParameterValue::Stepped(value) => {
            if !spec
                .range
                .is_some_and(|range| range.contains(*value as f64))
            {
                return Err(CapabilityError::ValueOutOfRange(spec.id().clone()));
            }
        }
        ParameterValue::Choice(value) => {
            if !spec.choices.iter().any(|choice| choice.id() == value) {
                return Err(CapabilityError::UnknownChoice(spec.id().clone()));
            }
        }
        ParameterValue::Toggle(_) => {}
    }
    Ok(())
}

fn validate_asset(spec: &ParameterSpec, value: &AssetReference) -> Result<(), CapabilityError> {
    let ParameterDefault::Asset(expected) = &spec.default_value else {
        return Err(CapabilityError::WrongAssetKind(spec.id().clone()));
    };
    // The descriptor default declares the semantic asset family and the
    // initial locator. Asset-valued controls such as Sample selection must be
    // able to replace that locator without inventing a capability-specific
    // config representation. Engines that intentionally pin one exact file
    // (the bundled SoundFont, for example) enforce that stronger constraint in
    // their own preparer.
    if value.kind() != expected.kind() {
        return Err(CapabilityError::AssetDoesNotMatch(spec.id().clone()));
    }
    Ok(())
}

fn validate_dependencies(
    descriptor: &CapabilityDescriptor,
    assignments: &[ParameterAssignment],
) -> Result<(), CapabilityError> {
    for spec in descriptor.parameters() {
        if spec.kind() == ParameterKind::Asset {
            continue;
        }
        let assignment = assignments
            .iter()
            .find(|assignment| assignment.parameter_id() == spec.id())
            .expect("complete canonical assignments were built above");
        // Visibility selects a presentation; hidden parts retain their settings.
        if let Some(predicate) = spec.enabled_when() {
            let satisfied = assignments.iter().any(|candidate| {
                candidate.parameter_id() == predicate.parameter_id()
                    && candidate.value() == predicate.equals()
            });
            if !satisfied
                && !matches!(
                    spec.default_value(),
                    ParameterDefault::Value(default) if default == assignment.value()
                )
            {
                return Err(CapabilityError::DependencyUnsatisfied(spec.id().clone()));
            }
        }
    }
    Ok(())
}

/// One Patch's capability identity and ordered immutable configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstrumentConfig {
    capability_id: CapabilityId,
    values: Vec<ParameterAssignment>,
    asset_references: Vec<AssetAssignment>,
}

impl InstrumentConfig {
    /// Creates a candidate config. A provider or registry must validate it before installation.
    pub const fn from_parts(
        capability_id: CapabilityId,
        values: Vec<ParameterAssignment>,
        asset_references: Vec<AssetAssignment>,
    ) -> Self {
        Self {
            capability_id,
            values,
            asset_references,
        }
    }

    pub const fn capability_id(&self) -> &CapabilityId {
        &self.capability_id
    }

    pub fn values(&self) -> &[ParameterAssignment] {
        &self.values
    }

    pub fn asset_references(&self) -> &[AssetAssignment] {
        &self.asset_references
    }

    pub fn value(&self, id: &ParameterId) -> Option<&ParameterValue> {
        self.values
            .iter()
            .find(|assignment| assignment.parameter_id() == id)
            .map(ParameterAssignment::value)
    }

    pub fn asset_reference(&self, id: &ParameterId) -> Option<&AssetReference> {
        self.asset_references
            .iter()
            .find(|assignment| assignment.parameter_id() == id)
            .map(AssetAssignment::reference)
    }

    /// Replaces one descriptor-classified Scalar assignment and revalidates
    /// the complete canonical config without substituting any value.
    pub fn with_scalar_value(
        &self,
        descriptor: &CapabilityDescriptor,
        id: &ParameterId,
        value: ParameterValue,
    ) -> Result<Self, CapabilityError> {
        if descriptor.id() != self.capability_id() {
            return Err(CapabilityError::ProviderRegistryMismatch(
                self.capability_id.clone(),
            ));
        }
        let spec = descriptor
            .parameter(id)
            .ok_or_else(|| CapabilityError::UndeclaredParameter(id.clone()))?;
        if spec.update() != ParameterUpdate::Scalar || spec.kind() == ParameterKind::Asset {
            return Err(CapabilityError::StructuralParameter(id.clone()));
        }
        validate_value(spec, &value)?;
        let mut values = self.values.clone();
        let assignment = values
            .iter_mut()
            .find(|assignment| assignment.parameter_id() == id)
            .ok_or_else(|| CapabilityError::MissingParameter(id.clone()))?;
        assignment.value = value;
        descriptor.create_config(&values, &self.asset_references)
    }
}

/// Immutable ordered descriptors installed in canonical application state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityRegistry {
    descriptors: Vec<CapabilityDescriptor>,
    #[serde(default)]
    asset_descriptors: Vec<CapabilityDescriptor>,
}

impl CapabilityRegistry {
    pub fn new(descriptors: Vec<CapabilityDescriptor>) -> Result<Self, CapabilityError> {
        if descriptors.is_empty() {
            return Err(CapabilityError::EmptyRegistry);
        }
        for (index, descriptor) in descriptors.iter().enumerate() {
            descriptor.validate()?;
            if descriptors[..index]
                .iter()
                .any(|prior| prior.id() == descriptor.id())
            {
                return Err(CapabilityError::DuplicateCapability(
                    descriptor.id().clone(),
                ));
            }
        }
        Ok(Self {
            descriptors,
            asset_descriptors: Vec::new(),
        })
    }

    pub fn descriptors(&self) -> &[CapabilityDescriptor] {
        &self.descriptors
    }

    pub fn descriptor(&self, id: &CapabilityId) -> Option<&CapabilityDescriptor> {
        self.descriptors
            .iter()
            .find(|descriptor| descriptor.id() == id)
    }

    pub fn validate_config(&self, config: &InstrumentConfig) -> Result<(), CapabilityError> {
        let descriptor = self
            .descriptor_for_config(config)
            .ok_or_else(|| CapabilityError::UnknownCapability(config.capability_id().clone()))?;
        let canonical = descriptor.create_config(config.values(), config.asset_references())?;
        if canonical != *config {
            return Err(CapabilityError::ConfigOrderMismatch(
                config.capability_id().clone(),
            ));
        }
        Ok(())
    }

    /// Asset catalogs refine structural choices without replacing the installed engine schema.
    pub fn with_asset_descriptor(
        mut self,
        descriptor: CapabilityDescriptor,
    ) -> Result<Self, CapabilityError> {
        descriptor.validate()?;
        let installed = self
            .descriptor(descriptor.id())
            .ok_or_else(|| CapabilityError::UnknownCapability(descriptor.id().clone()))?;
        let mut normalized = descriptor.clone();
        for parameter in normalized
            .sections
            .iter_mut()
            .flat_map(|section| &mut section.parameters)
        {
            let base = installed.parameter(parameter.id()).ok_or_else(|| {
                CapabilityError::ProviderRegistryMismatch(descriptor.id().clone())
            })?;
            if parameter.kind() == ParameterKind::Asset
                || (parameter.kind() == ParameterKind::Choice
                    && parameter.update() == ParameterUpdate::Structural)
            {
                // Preset browsing becomes editable when an imported library
                // has multiple choices. Normalize only this cardinality-driven
                // change; arbitrary interaction-policy changes remain invalid.
                if parameter.kind() == ParameterKind::Choice {
                    let interaction = |count| {
                        if count > 1 {
                            PatchInteraction::StructuralChoice
                        } else {
                            PatchInteraction::ReadOnly
                        }
                    };
                    if parameter.patch_interaction == interaction(parameter.choices.len())
                        && base.patch_interaction == interaction(base.choices.len())
                    {
                        parameter.patch_interaction = base.patch_interaction;
                    }
                }
                parameter.default_value = base.default_value.clone();
                parameter.choices = base.choices.clone();
            }
        }
        if normalized != *installed
            || !installed.asset_scoped_choices
            || !descriptor
                .parameters()
                .any(|p| p.kind() == ParameterKind::Asset)
        {
            return Err(CapabilityError::ProviderRegistryMismatch(
                descriptor.id().clone(),
            ));
        }
        let assets = descriptor.default_assets();
        if descriptor == *installed {
            return Ok(self);
        }
        self.asset_descriptors
            .retain(|prior| prior.id() != descriptor.id() || prior.default_assets() != assets);
        self.asset_descriptors.push(descriptor);
        Ok(self)
    }

    pub fn asset_descriptors(&self) -> &[CapabilityDescriptor] {
        &self.asset_descriptors
    }

    pub fn retain_asset_descriptors<'a>(
        &mut self,
        configs: impl IntoIterator<Item = &'a InstrumentConfig>,
    ) {
        let configs = configs.into_iter().collect::<Vec<_>>();
        self.asset_descriptors.retain(|descriptor| {
            configs.iter().any(|config| {
                descriptor.id() == config.capability_id()
                    && descriptor.default_assets() == config.asset_references()
            })
        });
    }

    pub fn descriptor_for_assets(
        &self,
        id: &CapabilityId,
        assets: &[AssetAssignment],
    ) -> Option<&CapabilityDescriptor> {
        self.asset_descriptors
            .iter()
            .find(|descriptor| descriptor.id() == id && descriptor.default_assets() == assets)
            .or_else(|| {
                self.descriptor(id).filter(|descriptor| {
                    !descriptor.asset_scoped_choices || descriptor.default_assets() == assets
                })
            })
    }

    pub fn descriptor_for_config(
        &self,
        config: &InstrumentConfig,
    ) -> Option<&CapabilityDescriptor> {
        self.descriptor_for_assets(config.capability_id(), config.asset_references())
    }

    /// A new asset catalog resets its structural choices to authored defaults; scalar values remain exact.
    pub fn replace_asset(
        &self,
        source: &InstrumentConfig,
        id: &ParameterId,
        reference: AssetReference,
    ) -> Result<InstrumentConfig, CapabilityError> {
        self.validate_config(source)?;
        let mut assets = source.asset_references().to_vec();
        if let Some(assignment) = assets.iter_mut().find(|asset| asset.parameter_id() == id) {
            *assignment = AssetAssignment::new(id.clone(), reference);
        } else {
            assets.push(AssetAssignment::new(id.clone(), reference));
        }
        let descriptor = self
            .descriptor_for_assets(source.capability_id(), &assets)
            .ok_or_else(|| CapabilityError::UnknownCapability(source.capability_id().clone()))?;
        let spec = descriptor
            .parameter(id)
            .ok_or_else(|| CapabilityError::UndeclaredAsset(id.clone()))?;
        if spec.kind() != ParameterKind::Asset || spec.update() != ParameterUpdate::Structural {
            return Err(CapabilityError::StructuralParameter(id.clone()));
        }
        let scoped = descriptor.asset_scoped_choices;
        let values = source
            .values()
            .iter()
            .map(|assignment| {
                if scoped && source.asset_references() != assets {
                    if let Some(spec) =
                        descriptor.parameter(assignment.parameter_id()).filter(|p| {
                            p.kind() == ParameterKind::Choice
                                && p.update() == ParameterUpdate::Structural
                        })
                    {
                        if let ParameterDefault::Value(value) = spec.default_value() {
                            return ParameterAssignment::new(spec.id().clone(), value.clone());
                        }
                    }
                }
                assignment.clone()
            })
            .collect::<Vec<_>>();
        descriptor.create_config(&values, &assets)
    }
}

/// A typed descriptor or config validation failure. No variant implies fallback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CapabilityError {
    EmptyAssetLocator,
    EmptyAvailabilityReason,
    EmptyLabel,
    EmptyUnit,
    EmptyRegistry,
    NoSections,
    NoSupportedMidiKinds,
    ZeroFixedVoiceCount,
    NonFiniteContinuousValue,
    InvalidNumericRange,
    InvalidDefaultKind,
    InvalidMetadataIdentifier(String),
    InvalidParameterShape {
        parameter_id: ParameterId,
        reason: &'static str,
    },
    EmptySection(String),
    DuplicateCapability(CapabilityId),
    DuplicateSection(String),
    DuplicateVisualization(String),
    InvalidVisualization(String),
    DuplicateParameter(ParameterId),
    DuplicateChoice {
        parameter_id: ParameterId,
        choice_id: String,
    },
    DuplicateAssetRequirement(ParameterId),
    MissingAssetRequirement(ParameterId),
    DuplicateMidiKind(MidiMessageKind),
    InvalidDependency {
        parameter_id: ParameterId,
        dependency_id: ParameterId,
    },
    UnknownCapability(CapabilityId),
    MissingParameter(ParameterId),
    DuplicateAssignment(ParameterId),
    UndeclaredParameter(ParameterId),
    WrongValueKind(ParameterId),
    ValueOutOfRange(ParameterId),
    UnknownChoice(ParameterId),
    MissingAsset(ParameterId),
    DuplicateAsset(ParameterId),
    UndeclaredAsset(ParameterId),
    WrongAssetKind(ParameterId),
    AssetDoesNotMatch(ParameterId),
    DependencyUnsatisfied(ParameterId),
    StructuralParameter(ParameterId),
    ScalarValueAtBoundary(ParameterId),
    ScalarEncodingOutOfRange(ParameterId),
    ConfigOrderMismatch(CapabilityId),
    ProviderRegistryMismatch(CapabilityId),
}

impl fmt::Display for CapabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyAssetLocator => formatter.write_str("asset locator must not be empty"),
            Self::EmptyAvailabilityReason => {
                formatter.write_str("unavailable capability reason must not be empty")
            }
            Self::EmptyLabel => formatter.write_str("presentation labels must not be empty"),
            Self::EmptyUnit => formatter.write_str("parameter units must not be empty"),
            Self::EmptyRegistry => formatter.write_str("capability registry must not be empty"),
            Self::NoSections => formatter.write_str("capability descriptor requires a section"),
            Self::NoSupportedMidiKinds => {
                formatter.write_str("capability descriptor requires supported MIDI semantics")
            }
            Self::ZeroFixedVoiceCount => {
                formatter.write_str("fixed-per-Patch voice count must be nonzero")
            }
            Self::NonFiniteContinuousValue => {
                formatter.write_str("continuous parameter values must be finite")
            }
            Self::InvalidNumericRange => {
                formatter.write_str("numeric range is not finite and ordered")
            }
            Self::InvalidDefaultKind => formatter.write_str("parameter default has the wrong kind"),
            Self::InvalidMetadataIdentifier(value) => {
                write!(formatter, "metadata identifier {value:?} is invalid")
            }
            Self::InvalidParameterShape {
                parameter_id,
                reason,
            } => {
                write!(formatter, "parameter {parameter_id} is invalid: {reason}")
            }
            Self::EmptySection(id) => write!(formatter, "capability section {id} is empty"),
            Self::DuplicateCapability(id) => write!(formatter, "duplicate capability {id}"),
            Self::DuplicateSection(id) => write!(formatter, "duplicate capability section {id}"),
            Self::DuplicateVisualization(id) => {
                write!(formatter, "duplicate capability visualization {id}")
            }
            Self::InvalidVisualization(id) => {
                write!(formatter, "capability visualization {id} is invalid")
            }
            Self::DuplicateParameter(id) => write!(formatter, "duplicate parameter {id}"),
            Self::DuplicateChoice {
                parameter_id,
                choice_id,
            } => write!(
                formatter,
                "parameter {parameter_id} repeats choice {choice_id}"
            ),
            Self::DuplicateAssetRequirement(id) => {
                write!(formatter, "duplicate asset requirement for {id}")
            }
            Self::MissingAssetRequirement(id) => {
                write!(
                    formatter,
                    "asset parameter {id} has no requirement declaration"
                )
            }
            Self::DuplicateMidiKind(kind) => write!(formatter, "duplicate MIDI kind {kind:?}"),
            Self::InvalidDependency {
                parameter_id,
                dependency_id,
            } => write!(
                formatter,
                "parameter {parameter_id} has invalid dependency {dependency_id}"
            ),
            Self::UnknownCapability(id) => write!(formatter, "unknown capability {id}"),
            Self::MissingParameter(id) => write!(formatter, "missing parameter {id}"),
            Self::DuplicateAssignment(id) => write!(formatter, "duplicate assignment {id}"),
            Self::UndeclaredParameter(id) => write!(formatter, "undeclared parameter {id}"),
            Self::WrongValueKind(id) => {
                write!(formatter, "parameter {id} has the wrong value kind")
            }
            Self::ValueOutOfRange(id) => write!(formatter, "parameter {id} is outside its range"),
            Self::UnknownChoice(id) => write!(formatter, "parameter {id} has an unknown choice"),
            Self::MissingAsset(id) => write!(formatter, "missing asset {id}"),
            Self::DuplicateAsset(id) => write!(formatter, "duplicate asset {id}"),
            Self::UndeclaredAsset(id) => write!(formatter, "undeclared asset {id}"),
            Self::WrongAssetKind(id) => write!(formatter, "parameter {id} is not an asset"),
            Self::AssetDoesNotMatch(id) => {
                write!(formatter, "asset {id} does not match its descriptor")
            }
            Self::DependencyUnsatisfied(id) => {
                write!(formatter, "parameter {id} violates a dependency")
            }
            Self::StructuralParameter(id) => {
                write!(formatter, "parameter {id} requires structural preparation")
            }
            Self::ScalarValueAtBoundary(id) => {
                write!(
                    formatter,
                    "Scalar parameter {id} is already at its boundary"
                )
            }
            Self::ScalarEncodingOutOfRange(id) => {
                write!(
                    formatter,
                    "Scalar parameter {id} cannot be encoded as finite f32"
                )
            }
            Self::ConfigOrderMismatch(id) => {
                write!(formatter, "config for {id} is not in descriptor order")
            }
            Self::ProviderRegistryMismatch(id) => {
                write!(
                    formatter,
                    "provider descriptor for {id} is not installed exactly"
                )
            }
        }
    }
}

impl std::error::Error for CapabilityError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> ParameterId {
        ParameterId::new(value).unwrap()
    }

    fn stepped(id_value: &str, default: i64, minimum: i64, maximum: i64) -> ParameterSpec {
        ParameterSpec::new(
            id(id_value),
            id_value,
            ParameterKind::Stepped,
            ParameterUpdate::Structural,
            ParameterDefault::Value(ParameterValue::Stepped(default)),
            Some(ParameterRange::new(minimum as f64, maximum as f64).unwrap()),
            Vec::new(),
            Some(1.0),
            Some(8.0),
            None,
            "integer",
            None,
            None,
        )
        .unwrap()
    }

    fn scalar_parameter(
        id_value: &str,
        kind: ParameterKind,
        default: ParameterValue,
    ) -> ParameterSpec {
        let (range, choices, fine_step, coarse_step) = match kind {
            ParameterKind::Continuous => (
                Some(ParameterRange::new(0.0, 1.0).unwrap()),
                Vec::new(),
                Some(0.01),
                Some(0.1),
            ),
            ParameterKind::Stepped => (
                Some(ParameterRange::new(0.0, 8.0).unwrap()),
                Vec::new(),
                Some(1.0),
                Some(2.0),
            ),
            ParameterKind::Choice => (
                None,
                vec![
                    ParameterChoice::new("choice.one", "One").unwrap(),
                    ParameterChoice::new("choice.two", "Two").unwrap(),
                    ParameterChoice::new("choice.three", "Three").unwrap(),
                ],
                None,
                None,
            ),
            ParameterKind::Toggle => (None, Vec::new(), None, None),
            ParameterKind::Asset => unreachable!("asset values are never Scalar"),
        };
        ParameterSpec::new(
            id(id_value),
            id_value,
            kind,
            ParameterUpdate::Scalar,
            ParameterDefault::Value(default),
            range,
            choices,
            fine_step,
            coarse_step,
            None,
            "scalar",
            None,
            None,
        )
        .unwrap()
    }

    fn descriptor() -> CapabilityDescriptor {
        let file_id = id("test.file");
        let file = ParameterSpec::new(
            file_id.clone(),
            "File",
            ParameterKind::Asset,
            ParameterUpdate::Structural,
            ParameterDefault::Asset(AssetReference::new(AssetKind::Other, "fixture.bin").unwrap()),
            None,
            Vec::new(),
            None,
            None,
            None,
            "asset",
            None,
            None,
        )
        .unwrap();
        CapabilityDescriptor::new(
            CapabilityId::new("instrument.test").unwrap(),
            "Test",
            "instrument.test",
            vec![
                CapabilitySection::new("main", "Main", vec![stepped("test.step", 2, 0, 8), file])
                    .unwrap(),
            ],
            vec![AssetRequirement::new(file_id, true)],
            VoicePolicy::FixedPerPatch { voices: 4 },
            vec![MidiMessageKind::NoteOn, MidiMessageKind::NoteOff],
        )
        .unwrap()
    }

    #[test]
    fn capability_registry_canonicalizes_and_validates_configs() {
        let descriptor = descriptor();
        let registry = CapabilityRegistry::new(vec![descriptor.clone()]).unwrap();
        let config = descriptor
            .create_config(
                &[ParameterAssignment::new(
                    id("test.step"),
                    ParameterValue::Stepped(7),
                )],
                &[AssetAssignment::new(
                    id("test.file"),
                    AssetReference::new(AssetKind::Other, "fixture.bin").unwrap(),
                )],
            )
            .unwrap();

        registry.validate_config(&config).unwrap();
        assert_eq!(config.values()[0].parameter_id().as_str(), "test.step");
        assert_eq!(
            config.asset_references()[0].parameter_id().as_str(),
            "test.file"
        );
        assert_eq!(
            serde_json::from_str::<InstrumentConfig>(&serde_json::to_string(&config).unwrap())
                .unwrap(),
            config
        );
    }

    #[test]
    fn capability_registry_rejects_unknown_duplicate_missing_wrong_and_out_of_range_values() {
        let descriptor = descriptor();
        let registry = CapabilityRegistry::new(vec![descriptor.clone()]).unwrap();
        assert!(matches!(
            CapabilityRegistry::new(vec![descriptor.clone(), descriptor.clone()]),
            Err(CapabilityError::DuplicateCapability(_))
        ));
        assert!(matches!(
            descriptor.create_config(&[], &[]),
            Err(CapabilityError::MissingParameter(_))
        ));
        assert!(matches!(
            descriptor.create_config(
                &[
                    ParameterAssignment::new(id("test.step"), ParameterValue::Stepped(2)),
                    ParameterAssignment::new(id("test.step"), ParameterValue::Stepped(3)),
                ],
                &[]
            ),
            Err(CapabilityError::DuplicateAssignment(_))
        ));
        assert!(matches!(
            descriptor.create_config(
                &[ParameterAssignment::new(
                    id("test.unknown"),
                    ParameterValue::Stepped(2)
                )],
                &[]
            ),
            Err(CapabilityError::UndeclaredParameter(_))
        ));
        assert!(matches!(
            descriptor.create_config(
                &[ParameterAssignment::new(
                    id("test.step"),
                    ParameterValue::Toggle(true)
                )],
                &[]
            ),
            Err(CapabilityError::WrongValueKind(_))
        ));
        assert!(matches!(
            descriptor.create_config(
                &[ParameterAssignment::new(
                    id("test.step"),
                    ParameterValue::Stepped(9)
                )],
                &[]
            ),
            Err(CapabilityError::ValueOutOfRange(_))
        ));
        let unknown = InstrumentConfig::from_parts(
            CapabilityId::new("instrument.unknown").unwrap(),
            Vec::new(),
            Vec::new(),
        );
        assert!(matches!(
            registry.validate_config(&unknown),
            Err(CapabilityError::UnknownCapability(_))
        ));
    }

    #[test]
    fn descriptor_rejects_invalid_defaults_dependencies_capacity_and_midi_duplicates() {
        assert!(ParameterSpec::new(
            id("test.step"),
            "Step",
            ParameterKind::Stepped,
            ParameterUpdate::Structural,
            ParameterDefault::Value(ParameterValue::Stepped(9)),
            Some(ParameterRange::new(0.0, 8.0).unwrap()),
            Vec::new(),
            Some(1.0),
            Some(2.0),
            None,
            "integer",
            None,
            None,
        )
        .is_err());

        let step = stepped("test.step", 2, 0, 8);
        assert!(matches!(
            CapabilityDescriptor::new(
                CapabilityId::new("instrument.invalid").unwrap(),
                "Invalid",
                "instrument.invalid",
                vec![CapabilitySection::new("main", "Main", vec![step.clone()]).unwrap()],
                Vec::new(),
                VoicePolicy::FixedPerPatch { voices: 0 },
                vec![MidiMessageKind::NoteOn],
            ),
            Err(CapabilityError::ZeroFixedVoiceCount)
        ));
        assert!(matches!(
            CapabilityDescriptor::new(
                CapabilityId::new("instrument.invalid").unwrap(),
                "Invalid",
                "instrument.invalid",
                vec![CapabilitySection::new("main", "Main", vec![step]).unwrap()],
                Vec::new(),
                VoicePolicy::FixedPerPatch { voices: 1 },
                vec![MidiMessageKind::NoteOn, MidiMessageKind::NoteOn],
            ),
            Err(CapabilityError::DuplicateMidiKind(MidiMessageKind::NoteOn))
        ));
    }

    #[test]
    fn descriptor_rejects_duplicate_sections_parameters_choices_and_asset_requirements() {
        let step = stepped("test.step", 2, 0, 8);
        let section = CapabilitySection::new("main", "Main", vec![step.clone()]).unwrap();
        assert!(matches!(
            CapabilityDescriptor::new(
                CapabilityId::new("instrument.invalid").unwrap(),
                "Invalid",
                "instrument.invalid",
                vec![section.clone(), section],
                Vec::new(),
                VoicePolicy::FixedPerPatch { voices: 1 },
                vec![MidiMessageKind::NoteOn],
            ),
            Err(CapabilityError::DuplicateSection(_))
        ));
        assert!(matches!(
            CapabilityDescriptor::new(
                CapabilityId::new("instrument.invalid").unwrap(),
                "Invalid",
                "instrument.invalid",
                vec![CapabilitySection::new("main", "Main", vec![step.clone(), step]).unwrap()],
                Vec::new(),
                VoicePolicy::FixedPerPatch { voices: 1 },
                vec![MidiMessageKind::NoteOn],
            ),
            Err(CapabilityError::DuplicateParameter(_))
        ));

        let duplicate_choice = ParameterSpec::new(
            id("test.choice"),
            "Choice",
            ParameterKind::Choice,
            ParameterUpdate::Structural,
            ParameterDefault::Value(ParameterValue::Choice("one".to_owned())),
            None,
            vec![
                ParameterChoice::new("one", "One").unwrap(),
                ParameterChoice::new("one", "Duplicate").unwrap(),
            ],
            None,
            None,
            None,
            "choice",
            None,
            None,
        );
        assert!(matches!(
            duplicate_choice,
            Err(CapabilityError::DuplicateChoice { .. })
        ));

        let valid = descriptor();
        assert!(matches!(
            CapabilityDescriptor::new(
                CapabilityId::new("instrument.invalid").unwrap(),
                "Invalid",
                "instrument.invalid",
                valid.sections.clone(),
                vec![
                    AssetRequirement::new(id("test.file"), true),
                    AssetRequirement::new(id("test.file"), true),
                ],
                VoicePolicy::FixedPerPatch { voices: 1 },
                vec![MidiMessageKind::NoteOn],
            ),
            Err(CapabilityError::DuplicateAssetRequirement(_))
        ));
        assert!(matches!(
            CapabilityDescriptor::new(
                CapabilityId::new("instrument.invalid").unwrap(),
                "Invalid",
                "instrument.invalid",
                valid.sections,
                Vec::new(),
                VoicePolicy::FixedPerPatch { voices: 1 },
                vec![MidiMessageKind::NoteOn],
            ),
            Err(CapabilityError::MissingAssetRequirement(_))
        ));
    }

    #[test]
    fn catalog_validation_preserves_declaration_order_and_first_duplicate_error() {
        let mut spec = scalar_parameter(
            "test.catalog",
            ParameterKind::Choice,
            ParameterValue::Choice("choice.one".into()),
        );
        spec.choices.extend((0..1024).rev().map(|index| {
            ParameterChoice::new(format!("preset.{index}"), format!("Preset {index}")).unwrap()
        }));
        let declared = spec.choices.clone();
        spec.validate_shape().unwrap();
        assert_eq!(spec.choices, declared);
        // The earliest duplicate is not the lexicographically first ID.
        spec.choices
            .push(ParameterChoice::new("preset.8", "Duplicate 8").unwrap());
        spec.choices
            .push(ParameterChoice::new("choice.one", "Duplicate one").unwrap());
        assert_eq!(
            spec.validate_shape(),
            Err(CapabilityError::DuplicateChoice {
                parameter_id: id("test.catalog"),
                choice_id: "preset.8".into(),
            })
        );
    }

    #[test]
    fn capability_registry_rejects_every_invalid_asset_and_dependency_case() {
        let descriptor = descriptor();
        let values = [ParameterAssignment::new(
            id("test.step"),
            ParameterValue::Stepped(2),
        )];
        let valid_asset = AssetAssignment::new(
            id("test.file"),
            AssetReference::new(AssetKind::Other, "fixture.bin").unwrap(),
        );
        assert!(matches!(
            descriptor.create_config(&values, &[]),
            Err(CapabilityError::MissingAsset(_))
        ));
        assert!(matches!(
            descriptor.create_config(&values, &[valid_asset.clone(), valid_asset]),
            Err(CapabilityError::DuplicateAsset(_))
        ));
        assert!(matches!(
            descriptor.create_config(
                &values,
                &[AssetAssignment::new(
                    id("test.unknown"),
                    AssetReference::new(AssetKind::Other, "fixture.bin").unwrap(),
                )],
            ),
            Err(CapabilityError::UndeclaredAsset(_))
        ));
        assert!(matches!(
            descriptor.create_config(
                &values,
                &[AssetAssignment::new(
                    id("test.step"),
                    AssetReference::new(AssetKind::Other, "fixture.bin").unwrap(),
                )],
            ),
            Err(CapabilityError::WrongAssetKind(_))
        ));
        let replaced = descriptor
            .create_config(
                &values,
                &[AssetAssignment::new(
                    id("test.file"),
                    AssetReference::new(AssetKind::Other, "other.bin").unwrap(),
                )],
            )
            .unwrap();
        assert_eq!(
            replaced.asset_references()[0].reference().locator(),
            "other.bin"
        );
        assert!(matches!(
            descriptor.create_config(
                &values,
                &[AssetAssignment::new(
                    id("test.file"),
                    AssetReference::new(AssetKind::Sample, "other.wav").unwrap(),
                )],
            ),
            Err(CapabilityError::AssetDoesNotMatch(_))
        ));

        let enabled = ParameterSpec::new(
            id("test.enabled"),
            "Enabled",
            ParameterKind::Toggle,
            ParameterUpdate::Structural,
            ParameterDefault::Value(ParameterValue::Toggle(false)),
            None,
            Vec::new(),
            None,
            None,
            None,
            "toggle",
            None,
            None,
        )
        .unwrap();
        let dependent = ParameterSpec::new(
            id("test.dependent"),
            "Dependent",
            ParameterKind::Stepped,
            ParameterUpdate::Structural,
            ParameterDefault::Value(ParameterValue::Stepped(0)),
            Some(ParameterRange::new(0.0, 8.0).unwrap()),
            Vec::new(),
            Some(1.0),
            Some(2.0),
            None,
            "integer",
            Some(ParameterPredicate::new(
                id("test.enabled"),
                ParameterValue::Toggle(true),
            )),
            None,
        )
        .unwrap();
        let dependent_descriptor = CapabilityDescriptor::new(
            CapabilityId::new("instrument.dependent").unwrap(),
            "Dependent",
            "instrument.dependent",
            vec![CapabilitySection::new("main", "Main", vec![enabled, dependent]).unwrap()],
            Vec::new(),
            VoicePolicy::FixedPerPatch { voices: 1 },
            vec![MidiMessageKind::NoteOn],
        )
        .unwrap();
        assert!(matches!(
            dependent_descriptor.create_config(
                &[
                    ParameterAssignment::new(id("test.enabled"), ParameterValue::Toggle(false)),
                    ParameterAssignment::new(id("test.dependent"), ParameterValue::Stepped(1)),
                ],
                &[],
            ),
            Err(CapabilityError::DependencyUnsatisfied(_))
        ));

        let invalid_dependency = ParameterSpec::new(
            id("test.dependent"),
            "Dependent",
            ParameterKind::Stepped,
            ParameterUpdate::Structural,
            ParameterDefault::Value(ParameterValue::Stepped(0)),
            Some(ParameterRange::new(0.0, 8.0).unwrap()),
            Vec::new(),
            Some(1.0),
            Some(2.0),
            None,
            "integer",
            Some(ParameterPredicate::new(
                id("test.later"),
                ParameterValue::Toggle(true),
            )),
            None,
        )
        .unwrap();
        assert!(matches!(
            CapabilityDescriptor::new(
                CapabilityId::new("instrument.invalid").unwrap(),
                "Invalid",
                "instrument.invalid",
                vec![CapabilitySection::new("main", "Main", vec![invalid_dependency]).unwrap()],
                Vec::new(),
                VoicePolicy::FixedPerPatch { voices: 1 },
                vec![MidiMessageKind::NoteOn],
            ),
            Err(CapabilityError::InvalidDependency { .. })
        ));
    }

    #[test]
    fn scalar_values_encode_replace_and_adjust_without_fallback() {
        let continuous = scalar_parameter(
            "test.continuous",
            ParameterKind::Continuous,
            ParameterValue::continuous(0.25).unwrap(),
        );
        let stepped_scalar = scalar_parameter(
            "test.stepped-scalar",
            ParameterKind::Stepped,
            ParameterValue::Stepped(2),
        );
        let choice = scalar_parameter(
            "test.choice",
            ParameterKind::Choice,
            ParameterValue::Choice("choice.two".to_owned()),
        );
        let toggle = scalar_parameter(
            "test.toggle",
            ParameterKind::Toggle,
            ParameterValue::Toggle(false),
        );
        let descriptor = CapabilityDescriptor::new(
            CapabilityId::new("instrument.scalar-test").unwrap(),
            "Scalar test",
            "instrument.scalar-test",
            vec![CapabilitySection::new(
                "main",
                "Main",
                vec![
                    continuous.clone(),
                    stepped("test.structural", 1, 0, 8),
                    stepped_scalar.clone(),
                    choice.clone(),
                    toggle.clone(),
                ],
            )
            .unwrap()],
            Vec::new(),
            VoicePolicy::FixedPerPatch { voices: 4 },
            vec![MidiMessageKind::NoteOn],
        )
        .unwrap();
        let config = descriptor
            .create_config(
                &[
                    ParameterAssignment::new(
                        id("test.continuous"),
                        ParameterValue::continuous(0.25).unwrap(),
                    ),
                    ParameterAssignment::new(id("test.structural"), ParameterValue::Stepped(1)),
                    ParameterAssignment::new(id("test.stepped-scalar"), ParameterValue::Stepped(2)),
                    ParameterAssignment::new(
                        id("test.choice"),
                        ParameterValue::Choice("choice.two".to_owned()),
                    ),
                    ParameterAssignment::new(id("test.toggle"), ParameterValue::Toggle(false)),
                ],
                &[],
            )
            .unwrap();

        let encoded = descriptor
            .scalar_parameters()
            .map(|spec| spec.scalar_value(config.value(spec.id()).unwrap()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(encoded, vec![0.25, 2.0, 1.0, 0.0]);
        assert_eq!(
            continuous
                .adjusted_scalar_value(
                    config.value(continuous.id()).unwrap(),
                    ParameterAdjustment::CoarseIncrease,
                )
                .unwrap(),
            ParameterValue::Continuous(0.35)
        );
        assert_eq!(
            stepped_scalar
                .adjusted_scalar_value(
                    config.value(stepped_scalar.id()).unwrap(),
                    ParameterAdjustment::CoarseIncrease,
                )
                .unwrap(),
            ParameterValue::Stepped(4)
        );
        assert_eq!(
            choice
                .adjusted_scalar_value(
                    config.value(choice.id()).unwrap(),
                    ParameterAdjustment::FineIncrease,
                )
                .unwrap(),
            ParameterValue::Choice("choice.three".to_owned())
        );

        let replaced = config
            .with_scalar_value(&descriptor, toggle.id(), ParameterValue::Toggle(true))
            .unwrap();
        assert_eq!(
            replaced.value(toggle.id()),
            Some(&ParameterValue::Toggle(true))
        );
        assert_eq!(
            config.value(toggle.id()),
            Some(&ParameterValue::Toggle(false))
        );
        assert!(matches!(
            config.with_scalar_value(
                &descriptor,
                &id("test.structural"),
                ParameterValue::Stepped(2),
            ),
            Err(CapabilityError::StructuralParameter(_))
        ));
        assert!(matches!(
            config.with_scalar_value(
                &descriptor,
                choice.id(),
                ParameterValue::Choice("choice.missing".to_owned()),
            ),
            Err(CapabilityError::UnknownChoice(_))
        ));
    }

    #[test]
    fn descriptor_accepts_large_scalar_schemas() {
        let parameters = (0..257)
            .map(|index| {
                scalar_parameter(
                    &format!("test.scalar-{index}"),
                    ParameterKind::Toggle,
                    ParameterValue::Toggle(false),
                )
            })
            .collect();
        assert!(CapabilityDescriptor::new(
            CapabilityId::new("instrument.too-many-scalars").unwrap(),
            "Too many Scalars",
            "instrument.too-many-scalars",
            vec![CapabilitySection::new("main", "Main", parameters).unwrap()],
            Vec::new(),
            VoicePolicy::FixedPerPatch { voices: 1 },
            vec![MidiMessageKind::NoteOn],
        )
        .is_ok());
    }
}
