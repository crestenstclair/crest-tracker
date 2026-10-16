mod sends;
use crate::control::Direction;
use crate::control::{
    AppState, EventRejection, FocusPath, MixerControlId, PatchChoiceSubject, PatchControlId,
    PatchDetailSubject, PatchPositionId, PatchSubordinateSession, ProspectivePatch, SemanticAction,
    SemanticActionAvailability, SemanticControlId, SurfaceId, ValidAction,
};
use crate::kernel::PatchId;
use crate::mixer::bus_id::BusId;
use crate::mixer::global_parameters::GlobalParameters;
use crate::mixer::mixer_track_id::{MixerTrackId, MixerTrackId as TrackId};
use crate::mixer::mixer_track_parameters::MixerTrackParameter;
use crate::synth::instrument_capability::{ParameterSpec, ParameterValue};
use crate::synth::{
    EffectCategory, InstrumentCategory, ParameterId, ParameterKind, PatchInteraction,
    PostEffectConfig,
};
use std::collections::{BTreeSet, HashSet};

/// Pure descriptor-backed authority for semantic focus order and recovery.
///
/// The resolver borrows one immutable accepted state. It never owns interaction
/// state, layout data, or runtime objects, and every returned path is expressed
/// only with stable domain identities.
pub struct SemanticResolver<'a> {
    state: &'a AppState,
}

enum ResolverPatchSource<'a> {
    Created(&'a crate::synth::Patch),
    Pending(&'a crate::synth::Patch),
    Prospective(&'a ProspectivePatch),
}

impl ResolverPatchSource<'_> {
    fn instrument_config(&self) -> &crate::synth::InstrumentConfig {
        match self {
            Self::Created(patch) | Self::Pending(patch) => patch.instrument_config(),
            Self::Prospective(patch) => patch.instrument_config(),
        }
    }

    fn output(&self) -> Option<crate::mixer::patch_output::PatchOutput> {
        match self {
            Self::Created(patch) | Self::Pending(patch) => Some(patch.output()),
            Self::Prospective(patch) => patch.output(),
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
}

/// The family of an instrument or effect in the shared picker.
#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(tag = "kind", content = "category", rename_all = "camelCase")]
pub enum ChoiceCategory {
    Instrument(InstrumentCategory),
    Effect(EffectCategory),
}

impl ChoiceCategory {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Instrument(category) => category.label(),
            Self::Effect(category) => category.label(),
        }
    }
}

/// One available value in the shared trapped option modal.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedChoiceOption {
    id: String,
    label: String,
    current: bool,
    enabled: bool,
    availability: crate::synth::CapabilityAvailability,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    category: Option<ChoiceCategory>,
}

impl ResolvedChoiceOption {
    fn enabled(id: impl Into<String>, label: impl Into<String>, current: bool) -> Self {
        Self::with_availability(
            id,
            label,
            current,
            crate::synth::CapabilityAvailability::Available,
        )
    }

    fn with_availability(
        id: impl Into<String>,
        label: impl Into<String>,
        current: bool,
        availability: crate::synth::CapabilityAvailability,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            current,
            enabled: availability.is_enabled(),
            availability,
            category: None,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub const fn is_current(&self) -> bool {
        self.current
    }

    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub const fn availability(&self) -> &crate::synth::CapabilityAvailability {
        &self.availability
    }

    pub const fn category(&self) -> Option<ChoiceCategory> {
        self.category
    }
}

/// Ephemeral resolution of a generic choice subject against canonical state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedChoiceSource {
    subject: PatchChoiceSubject,
    origin_label: String,
    options: Vec<ResolvedChoiceOption>,
}

impl ResolvedChoiceSource {
    pub const fn subject(&self) -> &PatchChoiceSubject {
        &self.subject
    }

    pub fn origin_label(&self) -> &str {
        &self.origin_label
    }

    pub fn options(&self) -> &[ResolvedChoiceOption] {
        &self.options
    }

    fn option_path(&self, option: &ResolvedChoiceOption) -> FocusPath {
        FocusPath::patch_choice_at(
            self.subject.patch_position(),
            self.subject.stable_id(),
            option.id().to_owned(),
        )
    }

    /// The focused stable option owns the active group, not a second cursor.
    pub fn active_category(&self, focus: &FocusPath) -> Option<ChoiceCategory> {
        if !matches!(
            self.subject.control_id(),
            PatchControlId::Engine | PatchControlId::EffectSlot(_)
        ) {
            return None;
        }
        let SemanticControlId::Modal(crate::control::ModalControlId::Choice(id)) =
            focus.control_id()
        else {
            return None;
        };
        self.options
            .iter()
            .find(|option| option.id() == id)
            .and_then(ResolvedChoiceOption::category)
    }

    pub fn visible_options<'a>(
        &'a self,
        focus: &FocusPath,
    ) -> impl Iterator<Item = &'a ResolvedChoiceOption> {
        let category = self.active_category(focus);
        self.options
            .iter()
            .filter(move |option| option.category() == category)
    }

    /// Horizontal navigation loops over installed groups with an enabled row.
    fn adjacent_category_path(&self, focus: &FocusPath, forward: bool) -> Option<FocusPath> {
        let current = self.active_category(focus)?;
        let categories: BTreeSet<_> = self
            .options
            .iter()
            .filter(|option| option.is_enabled())
            .filter_map(ResolvedChoiceOption::category)
            .collect();
        let target = if forward {
            categories
                .iter()
                .find(|category| **category > current)
                .or_else(|| categories.first())
        } else {
            categories
                .iter()
                .rev()
                .find(|category| **category < current)
                .or_else(|| categories.last())
        }
        .copied()?;
        if target == current {
            return Some(focus.clone());
        }
        let mut options = self
            .options
            .iter()
            .filter(|option| option.is_enabled() && option.category() == Some(target));
        let first = options.next()?;
        let option = if first.is_current() {
            first
        } else {
            options.find(|option| option.is_current()).unwrap_or(first)
        };
        Some(self.option_path(option))
    }
}

impl<'a> SemanticResolver<'a> {
    pub const fn new(state: &'a AppState) -> Self {
        Self { state }
    }

    /// Derives a generic choice subject from the focused canonical control.
    pub fn choice_subject(&self, path: &FocusPath) -> Option<PatchChoiceSubject> {
        if !matches!(
            path.surface(),
            SurfaceId::PatchMain | SurfaceId::PatchUtility | SurfaceId::PatchDetail
        ) {
            return None;
        }
        let patch_position = path.patch_position()?;
        let SemanticControlId::Patch(control) = path.control_id() else {
            return None;
        };
        let subject = PatchChoiceSubject::at(patch_position, control.clone());
        self.choice_source(&subject).ok().map(|_| subject)
    }

    /// Resolves installed, fixed-domain, or descriptor-owned choices without
    /// storing an option list in interaction state.
    pub fn choice_source(
        &self,
        subject: &PatchChoiceSubject,
    ) -> Result<ResolvedChoiceSource, EventRejection> {
        let prospective;
        let patch = match subject.patch_position() {
            PatchPositionId::Created(patch_id) => ResolverPatchSource::Created(
                self.state
                    .patches()
                    .iter()
                    .find(|patch| patch.id() == patch_id)
                    .ok_or(EventRejection::NoPatchesInstalled)?,
            ),
            PatchPositionId::TrailingEmpty => {
                if let Some(pending) = self.state.pending_patch_creation() {
                    ResolverPatchSource::Pending(pending)
                } else {
                    let blueprint = self
                        .state
                        .patch_creation_blueprint()
                        .ok_or(EventRejection::EngineSelectionUnavailable)?;
                    prospective = blueprint
                        .prospective(
                            self.state.patches().len(),
                            blueprint.instrument_capability_id(),
                        )
                        .map_err(|_| EventRejection::EngineSelectionUnavailable)?;
                    ResolverPatchSource::Prospective(&prospective)
                }
            }
        };
        let (origin_label, options) = match subject.control_id() {
            PatchControlId::Engine => {
                let current = patch.instrument_config().capability_id();
                (
                    "Instrument".to_owned(),
                    self.state
                        .capabilities()
                        .descriptors()
                        .iter()
                        .map(|descriptor| {
                            let mut option = ResolvedChoiceOption::with_availability(
                                descriptor.id().to_string(),
                                descriptor.label(),
                                descriptor.id() == current,
                                descriptor.availability().clone(),
                            );
                            option.category =
                                Some(ChoiceCategory::Instrument(descriptor.instrument_category()));
                            option
                        })
                        .collect(),
                )
            }
            PatchControlId::EffectSlot(slot) => {
                let current = patch
                    .effect_slot(*slot)
                    .map(|effect| effect.capability_id());
                let options = sends::effect_occupancy_options(self.state.effects(), current);
                (format!("Effect Slot {}", slot.index() + 1), options)
            }
            PatchControlId::Output(
                crate::mixer::patch_output::PatchOutputParameter::OutputTrack,
            ) => {
                let current = patch.output().map(|output| output.track_id());
                (
                    "Output Track".to_owned(),
                    MixerTrackId::ALL
                        .into_iter()
                        .map(|track| {
                            let id = track.to_string();
                            ResolvedChoiceOption::enabled(id.clone(), id, Some(track) == current)
                        })
                        .collect(),
                )
            }
            PatchControlId::Capability(parameter_id) => {
                let descriptor = self
                    .state
                    .capabilities()
                    .descriptor_for_config(patch.instrument_config())
                    .ok_or(EventRejection::InvalidInstrumentConfig)?;
                let spec = descriptor
                    .parameter(parameter_id)
                    .filter(|spec| spec.kind() == ParameterKind::Choice)
                    .ok_or(EventRejection::InvalidSelection)?;
                let current = match patch.instrument_config().value(parameter_id) {
                    Some(ParameterValue::Choice(current)) => current.as_str(),
                    _ => return Err(EventRejection::InvalidInstrumentConfig),
                };
                (
                    spec.label().to_owned(),
                    spec.choices()
                        .iter()
                        .map(|choice| {
                            ResolvedChoiceOption::enabled(
                                choice.id(),
                                choice.label(),
                                choice.id() == current,
                            )
                        })
                        .collect(),
                )
            }
            PatchControlId::Effect(slot_id, parameter_id) => {
                let config = patch
                    .effect_slots()
                    .iter()
                    .flatten()
                    .find(|config| config.slot_id() == *slot_id)
                    .ok_or(EventRejection::InvalidEffectConfig)?;
                let descriptor = self
                    .state
                    .effects()
                    .descriptor(config.capability_id())
                    .ok_or(EventRejection::InvalidEffectConfig)?;
                let spec = descriptor
                    .parameter(parameter_id)
                    .filter(|spec| spec.kind() == ParameterKind::Choice)
                    .ok_or(EventRejection::InvalidSelection)?;
                let current = match config.value(parameter_id) {
                    Some(ParameterValue::Choice(current)) => current.as_str(),
                    _ => return Err(EventRejection::InvalidEffectConfig),
                };
                (
                    spec.label().to_owned(),
                    spec.choices()
                        .iter()
                        .map(|choice| {
                            ResolvedChoiceOption::enabled(
                                choice.id(),
                                choice.label(),
                                choice.id() == current,
                            )
                        })
                        .collect(),
                )
            }
            PatchControlId::Output(_)
            | PatchControlId::Envelope(_)
            | PatchControlId::Global(_)
            | PatchControlId::MidiInput
            | PatchControlId::Send(_)
            | PatchControlId::VoiceLimit => return Err(EventRejection::InvalidSelection),
        };
        if options.is_empty() {
            return Err(EventRejection::ActionUnavailableInContext);
        }
        Ok(ResolvedChoiceSource {
            subject: subject.clone(),
            origin_label,
            options,
        })
    }

    pub fn patch_choice_paths(
        &self,
        subject: &PatchChoiceSubject,
    ) -> Result<Vec<FocusPath>, EventRejection> {
        let source = self.choice_source(subject)?;
        let paths = source
            .options()
            .iter()
            .filter(|option| option.is_enabled())
            .map(|option| {
                FocusPath::patch_choice_at(
                    subject.patch_position(),
                    subject.stable_id(),
                    option.id().to_owned(),
                )
            })
            .collect::<Vec<_>>();
        if paths.is_empty() {
            return Err(EventRejection::ActionUnavailableInContext);
        }
        ensure_unique(&paths)?;
        Ok(paths)
    }

    pub fn navigate_patch_choice(
        &self,
        direction: crate::control::Direction,
    ) -> Result<FocusPath, EventRejection> {
        use crate::control::Direction;
        let Some(PatchSubordinateSession::Choice { subject, .. }) =
            self.state.interaction().subordinate_session()
        else {
            return Err(EventRejection::ActionUnavailableInContext);
        };
        let source = self.choice_source(subject)?;
        let focus = self.state.interaction().focus_path();
        if matches!(direction, Direction::Left | Direction::Right) {
            return source
                .adjacent_category_path(focus, direction == Direction::Right)
                .ok_or(EventRejection::ActionUnavailableInContext);
        }
        let paths: Vec<_> = source
            .visible_options(focus)
            .filter(|option| option.is_enabled())
            .map(|option| source.option_path(option))
            .collect();
        let current = paths
            .iter()
            .position(|path| path == focus)
            .ok_or(EventRejection::InvalidSelection)?;
        let next = if direction == Direction::Down {
            current.checked_add(1)
        } else {
            current.checked_sub(1)
        };
        let next = next.filter(|index| *index < paths.len()).or_else(|| {
            source.active_category(focus).map(|_| {
                if direction == Direction::Down {
                    0
                } else {
                    paths.len() - 1
                }
            })
        });
        next.and_then(|index| paths.get(index))
            .cloned()
            .ok_or(EventRejection::ActionUnavailableInContext)
    }

    pub fn file_browser_paths(&self) -> Result<Vec<FocusPath>, EventRejection> {
        let (patch_id, parameter_id) = match self.state.interaction().subordinate_session() {
            Some(PatchSubordinateSession::FileBrowser {
                patch_position,
                asset_parameter_id,
                ..
            }) => (
                patch_position.and_then(PatchPositionId::patch_id),
                asset_parameter_id,
            ),
            _ => return Err(EventRejection::ActionUnavailableInContext),
        };
        if self.state.file_browser().patch_id() != patch_id
            || self.state.file_browser().asset_parameter_id() != Some(parameter_id)
        {
            return Err(EventRejection::InvalidSelection);
        }
        let paths = self
            .state
            .file_browser()
            .rows()
            .iter()
            .map(|row| {
                FocusPath::file_browser_for_origin(
                    self.state
                        .interaction()
                        .return_path()
                        .expect("browser retains origin")
                        .origin(),
                    parameter_id.as_str().to_owned(),
                    row.id().to_owned(),
                )
            })
            .collect::<Vec<_>>();
        ensure_unique(&paths)?;
        Ok(paths)
    }

    /// Returns PATCH Main's canonical focus order for one installed Patch.
    pub fn patch_main_paths(&self, patch_id: PatchId) -> Result<Vec<FocusPath>, EventRejection> {
        let patch = self
            .state
            .patches()
            .iter()
            .find(|patch| patch.id() == patch_id)
            .ok_or(EventRejection::NoPatchesInstalled)?;
        self.state
            .capabilities()
            .descriptor_for_config(patch.instrument_config())
            .ok_or(EventRejection::InvalidInstrumentConfig)?;
        let mut paths = Vec::with_capacity(1 + crate::synth::effect_slot_id::MAX_EFFECT_SLOTS);
        if self
            .state
            .patch_overview_origin_enabled(patch_id, &PatchControlId::Engine)
        {
            paths.push(FocusPath::patch_main(
                patch_id,
                None,
                PatchControlId::Engine,
            ));
        }
        paths.extend(
            crate::synth::effect_slot_id::EffectSlotIndex::ALL
                .into_iter()
                .filter(|slot| {
                    self.state
                        .patch_overview_origin_enabled(patch_id, &PatchControlId::EffectSlot(*slot))
                })
                .map(|slot| {
                    FocusPath::patch_main(patch_id, None, PatchControlId::EffectSlot(slot))
                }),
        );
        ensure_unique(&paths)?;
        Ok(paths)
    }

    /// Returns PATCH Main's semantic order for either persisted content or
    /// the one prospective endpoint. Overview needs no placeholder Patch:
    /// its stable order is Engine followed by three occupancy positions.
    pub fn patch_main_paths_for_position(
        &self,
        position: PatchPositionId,
    ) -> Result<Vec<FocusPath>, EventRejection> {
        match position {
            PatchPositionId::Created(patch_id) => self.patch_main_paths(patch_id),
            PatchPositionId::TrailingEmpty => {
                let mut paths =
                    Vec::with_capacity(1 + crate::synth::effect_slot_id::MAX_EFFECT_SLOTS);
                paths.push(FocusPath::patch_main_at(
                    position,
                    None,
                    PatchControlId::Engine,
                ));
                paths.extend(
                    crate::synth::effect_slot_id::EffectSlotIndex::ALL
                        .into_iter()
                        .map(|slot| {
                            FocusPath::patch_main_at(
                                position,
                                None,
                                PatchControlId::EffectSlot(slot),
                            )
                        }),
                );
                ensure_unique(&paths)?;
                Ok(paths)
            }
        }
    }

    /// Derives the detail subject one PatchMain path opens, or `None`.
    ///
    /// The engine row resolves `Instrument`; an occupied effect slot resolves
    /// `Effect` carrying that slot's exact identity, so two positions holding
    /// the same registry entry are distinct subjects. Every other path resolves
    /// `None` because a subject-less detail surface would be an empty shell.
    pub fn detail_subject(&self, path: &FocusPath) -> Option<PatchDetailSubject> {
        if path.surface() != SurfaceId::PatchMain {
            return None;
        }
        let SemanticControlId::Patch(control) = path.control_id() else {
            return None;
        };
        if path.patch_position() == Some(PatchPositionId::TrailingEmpty) {
            let blueprint = self.state.patch_creation_blueprint()?;
            return match control {
                PatchControlId::Engine | PatchControlId::Capability(_) => Some(
                    PatchDetailSubject::instrument(blueprint.instrument_capability_id().clone()),
                ),
                PatchControlId::EffectSlot(_)
                | PatchControlId::Effect(_, _)
                | PatchControlId::Envelope(_)
                | PatchControlId::Output(_)
                | PatchControlId::Global(_)
                | PatchControlId::MidiInput
                | PatchControlId::Send(_)
                | PatchControlId::VoiceLimit => None,
            };
        }
        let patch = self
            .state
            .patches()
            .iter()
            .find(|patch| Some(patch.id()) == path.patch_id())?;
        match control {
            PatchControlId::Engine | PatchControlId::Capability(_) => Some(
                PatchDetailSubject::instrument(patch.instrument_config().capability_id().clone()),
            ),
            // The occupancy row names a position; only an occupied one names a
            // capability. An empty slot is deliberately not repaired into a
            // neighbouring subject.
            PatchControlId::EffectSlot(index) => {
                let occupant = patch.effect_slot(*index)?;
                Some(PatchDetailSubject::effect(
                    occupant.slot_id(),
                    occupant.capability_id().clone(),
                ))
            }
            PatchControlId::Effect(slot_id, _) => {
                let occupant = patch
                    .effect_slots()
                    .iter()
                    .flatten()
                    .find(|effect| effect.slot_id() == *slot_id)?;
                Some(PatchDetailSubject::effect(
                    occupant.slot_id(),
                    occupant.capability_id().clone(),
                ))
            }
            PatchControlId::Envelope(_)
            | PatchControlId::Output(_)
            | PatchControlId::Global(_)
            | PatchControlId::MidiInput
            | PatchControlId::Send(_)
            | PatchControlId::VoiceLimit => None,
        }
    }

    /// Reports whether a detail subject still names something live on the
    /// Patch it was opened on.
    ///
    /// An `Instrument` subject is live only while it *is* the Patch's active
    /// instrument capability; an `Effect` subject only while its exact slot is
    /// still occupied by that exact registry entry. Deliberately not the same
    /// question as [`Self::resolves`]: the detail order is resolved from the
    /// subject's own capability id, so a subject left behind by an engine swap
    /// still resolves to a full row list — one belonging to a capability the
    /// Patch no longer has.
    pub fn detail_subject_is_live(&self, patch_id: PatchId, subject: &PatchDetailSubject) -> bool {
        let Some(patch) = self
            .state
            .patches()
            .iter()
            .find(|patch| patch.id() == patch_id)
        else {
            return false;
        };
        match subject {
            PatchDetailSubject::Instrument { capability_id } => {
                patch.instrument_config().capability_id() == capability_id
            }
            PatchDetailSubject::Effect {
                slot_id,
                capability_id,
            } => patch.effect_slots().iter().flatten().any(|effect| {
                effect.slot_id() == *slot_id && effect.capability_id() == capability_id
            }),
        }
    }

    /// Returns the detail surface's focus order for one subject: the subject
    /// descriptor's visible enabled rows, in descriptor order.
    ///
    /// The rows are resolved from the installed descriptor at call time. The
    /// subject supplies only a capability identity, so nothing here is a
    /// second copy of the schema.
    pub fn patch_detail_paths(
        &self,
        patch_id: PatchId,
        subject: &PatchDetailSubject,
    ) -> Result<Vec<FocusPath>, EventRejection> {
        self.patch_detail_paths_for_position(PatchPositionId::Created(patch_id), subject)
    }

    pub fn patch_detail_paths_for_position(
        &self,
        position: PatchPositionId,
        subject: &PatchDetailSubject,
    ) -> Result<Vec<FocusPath>, EventRejection> {
        let prospective;
        let patch = match position {
            PatchPositionId::Created(patch_id) => ResolverPatchSource::Created(
                self.state
                    .patches()
                    .iter()
                    .find(|patch| patch.id() == patch_id)
                    .ok_or(EventRejection::NoPatchesInstalled)?,
            ),
            PatchPositionId::TrailingEmpty => {
                if let Some(pending) = self.state.pending_patch_creation() {
                    ResolverPatchSource::Pending(pending)
                } else {
                    let blueprint = self
                        .state
                        .patch_creation_blueprint()
                        .ok_or(EventRejection::EngineSelectionUnavailable)?;
                    prospective = blueprint
                        .prospective(
                            self.state.patches().len(),
                            blueprint.instrument_capability_id(),
                        )
                        .map_err(|_| EventRejection::EngineSelectionUnavailable)?;
                    ResolverPatchSource::Prospective(&prospective)
                }
            }
        };
        let capability_id = subject.focus_capability_id();
        let paths = match subject {
            PatchDetailSubject::Instrument { capability_id: _ } => {
                let config = patch.instrument_config();
                let descriptor = self
                    .state
                    .capabilities()
                    .descriptor_for_config(config)
                    .ok_or(EventRejection::InvalidInstrumentConfig)?;
                let mut paths = descriptor
                    .parameters()
                    .filter(|spec| row_is_visible_and_enabled(spec, |id| config.value(id)))
                    .map(|spec| {
                        FocusPath::patch_detail_at(
                            position,
                            capability_id.clone(),
                            PatchControlId::Capability(spec.id().clone()),
                        )
                    })
                    .collect::<Vec<_>>();
                paths.extend(
                    crate::synth::VoiceEnvelope::surface_descriptor()
                        .iter()
                        .map(|parameter| {
                            FocusPath::patch_detail_at(
                                position,
                                capability_id.clone(),
                                PatchControlId::Envelope(parameter.parameter()),
                            )
                        }),
                );
                paths
            }
            PatchDetailSubject::Effect {
                slot_id,
                capability_id: id,
            } => {
                let occupant = patch
                    .effect_slots()
                    .iter()
                    .flatten()
                    .find(|effect| effect.slot_id() == *slot_id)
                    .ok_or(EventRejection::InvalidEffectConfig)?;
                let descriptor = self
                    .state
                    .effects()
                    .descriptor(id)
                    .ok_or(EventRejection::InvalidEffectConfig)?;
                descriptor
                    .parameters()
                    .filter(|spec| row_is_visible_and_enabled(spec, |id| occupant.value(id)))
                    .map(|spec| {
                        FocusPath::patch_detail_at(
                            position,
                            capability_id.clone(),
                            PatchControlId::Effect(*slot_id, spec.id().clone()),
                        )
                    })
                    .collect::<Vec<_>>()
            }
        };
        ensure_unique(&paths)?;
        Ok(paths)
    }

    /// Returns the sixteen fixed MIXER Main track sections in identity order.
    pub fn mixer_main_sections(&self) -> Result<Vec<Vec<FocusPath>>, EventRejection> {
        let mut sections = Vec::with_capacity(MixerTrackId::COUNT);
        for track_id in MixerTrackId::ALL {
            let paths = MixerTrackParameter::MAIN
                .into_iter()
                .map(|parameter| FocusPath::mixer_track(track_id, parameter))
                .collect::<Vec<_>>();
            ensure_unique(&paths)?;
            sections.push(paths);
        }
        Ok(sections)
    }

    pub fn mixer_main_paths(&self) -> Result<Vec<FocusPath>, EventRejection> {
        Ok(self.mixer_main_sections()?.into_iter().flatten().collect())
    }

    /// Returns PATCH Utility's canonical focus order: the five declared rows.
    ///
    /// The order comes from [`PatchControlId::utility_surface_descriptor`] —
    /// one written declaration — rather than from iterating any parameter
    /// descriptor, so reordering a descriptor cannot reshuffle the panel.
    pub fn patch_utility_paths(&self, patch_id: PatchId) -> Result<Vec<FocusPath>, EventRejection> {
        if !self
            .state
            .patches()
            .iter()
            .any(|patch| patch.id() == patch_id)
        {
            return Err(EventRejection::NoPatchesInstalled);
        }
        let mut paths = PatchControlId::utility_surface_descriptor()
            .iter()
            .map(|control| FocusPath::patch_utility(patch_id, control.clone()))
            .collect::<Vec<_>>();
        paths.extend(
            self.state
                .bus_returns()
                .returns()
                .iter()
                .filter(|send| send.is_occupied())
                .map(|send| FocusPath::patch_utility(patch_id, PatchControlId::Send(send.id()))),
        );
        ensure_unique(&paths)?;
        Ok(paths)
    }

    pub fn patch_utility_paths_for_position(
        &self,
        position: PatchPositionId,
    ) -> Result<Vec<FocusPath>, EventRejection> {
        if let PatchPositionId::Created(patch_id) = position {
            return self.patch_utility_paths(patch_id);
        }
        let paths = PatchControlId::utility_surface_descriptor()
            .iter()
            .map(|control| FocusPath::patch_utility_at(position, control.clone()))
            .collect::<Vec<_>>();
        ensure_unique(&paths)?;
        Ok(paths)
    }

    /// Returns MIXER Inspector's canonical focus order: the selected track's
    /// eight indexed sends in ascending `BusId` order, then each of the eight
    /// bus returns' occupancy row, return level, and — when occupied — the
    /// occupying registry entry's visible enabled `ScalarEdit` rows, then the
    /// distinct global controls (master gain alone).
    pub fn mixer_inspector_paths(
        &self,
        track_id: TrackId,
    ) -> Result<Vec<FocusPath>, EventRejection> {
        let mut paths = self
            .state
            .bus_returns()
            .returns()
            .iter()
            .map(|send| send.id())
            .map(|bus| FocusPath::mixer_send(track_id, bus))
            .collect::<Vec<_>>();
        for bus in self
            .state
            .bus_returns()
            .returns()
            .iter()
            .map(|send| send.id())
        {
            paths.push(FocusPath::mixer_return_occupancy(bus));
            paths.push(FocusPath::mixer_return_level(bus));
            let bus_return = self.state.bus_returns().bus_return(bus);
            let Some(config) = bus_return.effect() else {
                continue;
            };
            let descriptor = self
                .state
                .effects()
                .descriptor(config.capability_id())
                .ok_or(EventRejection::InvalidEffectConfig)?;
            for spec in descriptor.parameters() {
                if (spec.patch_interaction() == PatchInteraction::ScalarEdit
                    || spec.kind() == crate::synth::ParameterKind::Asset)
                    && row_is_visible_and_enabled(spec, |id| config.value(id))
                {
                    paths.push(FocusPath::mixer_return_effect(
                        bus,
                        spec.id().clone(),
                        config.capability_id().clone(),
                    ));
                }
            }
        }
        paths.extend(
            GlobalParameters::surface_descriptor()
                .iter()
                .map(|descriptor| FocusPath::mixer_global(descriptor.parameter())),
        );
        ensure_unique(&paths)?;
        Ok(paths)
    }

    fn selected_mixer_track(&self) -> Result<MixerTrackId, EventRejection> {
        self.state
            .interaction()
            .remembered_mixer_main()
            .control_id()
            .as_mixer_track_id()
            .ok_or(EventRejection::InvalidSelection)
    }

    pub fn midi_input_settings_paths(&self) -> Result<Vec<FocusPath>, EventRejection> {
        let context = self.state.context();
        let paths = if self.state.midi_input().registry().is_empty() {
            vec![FocusPath::midi_device_settings_root(context)]
        } else {
            self.state
                .midi_input()
                .registry()
                .iter()
                .map(|entry| {
                    FocusPath::midi_device_settings(context, entry.descriptor().id().clone())
                })
                .collect()
        };
        ensure_unique(&paths)?;
        Ok(paths)
    }

    /// Returns the canonical focus order for a main surface. Side surfaces
    /// always contain their single read-only root anchor.
    pub fn ordered_paths(&self, surface: SurfaceId) -> Result<Vec<FocusPath>, EventRejection> {
        match surface {
            SurfaceId::Sends => self.send_paths(self.state.interaction().selected_send()),
            SurfaceId::PatchMain => {
                let position = self
                    .state
                    .interaction()
                    .remembered_patch_main()
                    .and_then(FocusPath::patch_position)
                    .or_else(|| {
                        self.state
                            .patches()
                            .first()
                            .map(|patch| PatchPositionId::Created(patch.id()))
                    })
                    .ok_or(EventRejection::NoPatchesInstalled)?;
                self.patch_main_paths_for_position(position)
            }
            SurfaceId::MixerMain => self.mixer_main_paths(),
            SurfaceId::PatchUtility => {
                let position = self
                    .state
                    .interaction()
                    .patch_position_focus()
                    .ok_or(EventRejection::NoPatchesInstalled)?;
                self.patch_utility_paths_for_position(position)
            }
            // The detail surface's order belongs to the open subject alone.
            // With no entry open there is no order to resolve — which is
            // exactly the state in which no detail path can be valid.
            SurfaceId::PatchDetail => {
                let interaction = self.state.interaction();
                let subject = interaction
                    .detail_subject()
                    .ok_or(EventRejection::ActionUnavailableInContext)?;
                let position = interaction
                    .patch_position_focus()
                    .ok_or(EventRejection::NoPatchesInstalled)?;
                self.patch_detail_paths_for_position(position, subject)
            }
            SurfaceId::PatchChoice => match self.state.interaction().subordinate_session() {
                Some(PatchSubordinateSession::Choice { subject, .. }) => {
                    self.patch_choice_paths(subject)
                }
                _ => Err(EventRejection::ActionUnavailableInContext),
            },
            SurfaceId::FileBrowser => self.file_browser_paths(),
            SurfaceId::MixerInspector => self.mixer_inspector_paths(self.selected_mixer_track()?),
            SurfaceId::MidiDeviceSettings => self.midi_input_settings_paths(),
            SurfaceId::SaveLoadSettings => Ok(crate::control::SessionCommand::SETTINGS_ACTIONS
                .into_iter()
                .map(|action| FocusPath::save_load_settings(self.state.context(), action))
                .collect()),
            SurfaceId::ControllerSettings => Ok(crate::control::ControllerSettingId::all()
                .map(|setting| FocusPath::controller_settings(self.state.context(), setting))
                .collect()),
        }
    }

    /// Resolves a path against the exact currently installed schema.
    pub fn resolves(&self, path: &FocusPath) -> bool {
        path.validate().is_ok()
            && self
                .ordered_paths(path.surface())
                .is_ok_and(|paths| paths.iter().any(|candidate| candidate == path))
    }

    /// Returns the ordered duplicate-free action set accepted by the exact
    /// current reducer state.
    ///
    /// The sweep reuses one scratch state rather than cloning twice per
    /// question (see [`SemanticActionAvailability`]). The question asked is
    /// bit-for-bit the one [`AppState::accepts_semantic_action`] asks —
    /// literally the same code — so the per-row answer still cannot drift from
    /// the production reducer.
    pub fn valid_actions(&self) -> Vec<ValidAction> {
        let mut availability = SemanticActionAvailability::new(self.state);
        let mut actions: Vec<_> = SemanticAction::surface_descriptor()
            .iter()
            .filter(|action| availability.accepts(action))
            .cloned()
            .map(|action| {
                let (label, hint) = match action {
                    SemanticAction::Activate
                        if self.state.interaction().active_surface()
                            == SurfaceId::SaveLoadSettings =>
                    {
                        match self.state.interaction().focus_path().control_id() {
                            crate::control::SemanticControlId::SessionFileAction(action) => {
                                (action.label(), Some("Return"))
                            }
                            _ => unreachable!("Save & Load has only file action rows"),
                        }
                    }
                    SemanticAction::ToggleTestMidi if self.state.test_midi_enabled() => {
                        ("Stop test MIDI", Some("T"))
                    }
                    SemanticAction::ToggleTestMidi => ("Start test MIDI", Some("T")),
                    SemanticAction::Activate
                        if self.state.interaction().active_surface()
                            == SurfaceId::ControllerSettings =>
                    {
                        if self.state.controller().capture().is_some() {
                            ("Cancel button capture", Some("Return"))
                        } else if matches!(
                            self.state.interaction().focus_path().control_id(),
                            crate::control::SemanticControlId::ControllerSetting(
                                crate::control::ControllerSettingId::ResetDefaults
                            )
                        ) {
                            ("Restore default buttons", Some("Return"))
                        } else {
                            ("Assign button", Some("Return"))
                        }
                    }
                    SemanticAction::NavigatePage(Direction::Right)
                        if self.state.controller().capture().is_some() =>
                    {
                        ("Cancel button capture", Some("Shift+Right / Shift+D"))
                    }
                    SemanticAction::NavigatePage(Direction::Down)
                        if self.state.controller().capture().is_some() =>
                    {
                        ("Cancel button capture", Some("Shift+Down / Shift+S"))
                    }
                    SemanticAction::Return if self.state.controller().capture().is_some() => {
                        ("Cancel button capture", None)
                    }
                    SemanticAction::Activate
                        if self.state.interaction().active_surface() == SurfaceId::PatchDetail =>
                    {
                        ("Browse files", Some("Return"))
                    }
                    _ => action_presentation(&action, self.state.interaction().active_surface()),
                };
                ValidAction::new(action, label, hint)
            })
            .collect();
        if self.state.interaction().active_surface().is_system() {
            // Keep the focused setting's primary action visible before the
            // compatibility shortcuts when footer guidance wraps or scrolls.
            actions.sort_by_key(|action| action.action() != &SemanticAction::Activate);
        }
        actions
    }

    /// Maps one MIXER stable identity back to compatibility coordinates. The
    /// coordinates never become stored interaction identity.
    pub fn mixer_coordinates(&self, path: &FocusPath) -> Option<(usize, usize)> {
        let SemanticControlId::Mixer(control) = path.control_id() else {
            return None;
        };
        let sections = self.mixer_main_sections().ok()?;
        match control {
            MixerControlId::Track { track_id, .. } => {
                if path.surface() != SurfaceId::MixerMain {
                    return None;
                }
                let section = track_id.index();
                let parameter = sections[section]
                    .iter()
                    .position(|candidate| candidate == path)?;
                Some((section, parameter))
            }
            MixerControlId::Send { .. }
            | MixerControlId::ReturnOccupancy { .. }
            | MixerControlId::ReturnLevel { .. }
            | MixerControlId::ReturnEffect { .. }
            | MixerControlId::Global { .. } => None,
        }
    }

    /// Recovers one old path in old canonical order. Exact stable identities
    /// win; otherwise candidates are searched outward, next before previous on
    /// equal distance, with the first surviving path as the final fallback.
    pub fn recover(
        old_path: &FocusPath,
        old_order: &[FocusPath],
        new_order: &[FocusPath],
    ) -> Option<FocusPath> {
        if !old_order.iter().any(|candidate| candidate == old_path)
            && !new_order.iter().any(|candidate| candidate == old_path)
        {
            return None;
        }
        Self::recovered_index(old_path, old_order, new_order)
            .map(|index| new_order[index].clone())
            .or_else(|| new_order.first().cloned())
    }

    /// The one deterministic focus-recovery rule, over any comparable identity.
    ///
    /// Returns the index in `new_keys` the old identity recovers to: its exact
    /// position when the new order still hosts it, otherwise the nearest
    /// surviving sibling walking outward from its old position — next before
    /// previous at equal distance. `None` means nothing near it survived, or
    /// it was never in the old order at all; callers decide what a total miss
    /// means for their surface.
    ///
    /// It is generic so a patch switch can recover over *control* identities,
    /// where the two orders carry different PatchIds and whole paths can never
    /// compare equal — without growing a second, subtly different rule.
    pub fn recovered_index<T: PartialEq>(old: &T, old_keys: &[T], new_keys: &[T]) -> Option<usize> {
        if let Some(index) = new_keys.iter().position(|candidate| candidate == old) {
            return Some(index);
        }
        let old_index = old_keys.iter().position(|candidate| candidate == old)?;
        for distance in 1..old_keys.len() {
            if let Some(next) = old_keys.get(old_index + distance) {
                if let Some(index) = new_keys.iter().position(|candidate| candidate == next) {
                    return Some(index);
                }
            }
            if let Some(previous) = old_index
                .checked_sub(distance)
                .map(|index| &old_keys[index])
            {
                if let Some(index) = new_keys.iter().position(|candidate| candidate == previous) {
                    return Some(index);
                }
            }
        }
        None
    }
}

/// Reports whether one descriptor row's visibility and enablement predicates
/// are both satisfied by the configuration `value` reads.
///
/// This is the one place predicate satisfaction is decided. Every surface
/// resolver — PatchMain, PatchDetail, and MixerInspector — reads it, so the
/// same configuration cannot make a row visible on one surface and hidden on
/// another.
pub(crate) fn row_is_visible_and_enabled<'a>(
    spec: &ParameterSpec,
    value: impl Fn(&ParameterId) -> Option<&'a ParameterValue>,
) -> bool {
    [spec.visible_when(), spec.enabled_when()]
        .into_iter()
        .flatten()
        .all(|predicate| value(predicate.parameter_id()) == Some(predicate.equals()))
}

fn ensure_unique(paths: &[FocusPath]) -> Result<(), EventRejection> {
    let unique = paths.iter().collect::<HashSet<_>>();
    if unique.len() == paths.len() && !paths.is_empty() {
        Ok(())
    } else {
        Err(EventRejection::InvalidSelection)
    }
}

fn action_presentation(
    action: &SemanticAction,
    surface: SurfaceId,
) -> (&'static str, Option<&'static str>) {
    use crate::control::{Direction, InteractionMode, TopLevelContext};
    match action {
        SemanticAction::SelectContext(TopLevelContext::Mixer) => ("Open MIXER", Some("1")),
        SemanticAction::SelectContext(TopLevelContext::Patch) => ("Open PATCH", Some("2")),
        SemanticAction::SelectPatch(Direction::Left) if surface == SurfaceId::Sends => {
            ("Previous send", Some("Q"))
        }
        SemanticAction::SelectPatch(Direction::Right) if surface == SurfaceId::Sends => {
            ("Next send", Some("E"))
        }
        SemanticAction::SelectPatch(Direction::Left) => ("Previous patch", Some("Q")),
        SemanticAction::SelectPatch(Direction::Right) => ("Next patch", Some("E")),
        SemanticAction::SelectPatch(Direction::Up)
        | SemanticAction::SelectPatch(Direction::Down) => ("Unavailable patch step", None),
        SemanticAction::NavigatePage(direction) => {
            let label = match (surface, direction) {
                (SurfaceId::Sends, Direction::Left) => "Previous send",
                (SurfaceId::Sends, Direction::Right) => "Next send",
                (SurfaceId::PatchMain, Direction::Up) => "Open highlighted Detail",
                (SurfaceId::PatchMain, Direction::Down) => "Open MIXER",
                (SurfaceId::PatchMain, Direction::Left) => "Open Settings",
                (SurfaceId::MixerMain | SurfaceId::MixerInspector, Direction::Up)
                | (SurfaceId::PatchDetail, Direction::Down) => "Return to Overview",
                (SurfaceId::PatchDetail, Direction::Up) => "Browse files",
                (
                    SurfaceId::MidiDeviceSettings
                    | SurfaceId::ControllerSettings
                    | SurfaceId::SaveLoadSettings,
                    Direction::Right | Direction::Down,
                ) => "Return to performance",
                (_, Direction::Down) => "Return / cancel",
                (_, Direction::Left) => "Previous patch",
                (_, Direction::Right) => "Next patch",
                _ => "Open related",
            };
            let hint = match direction {
                Direction::Up => "Shift+Up / Shift+W",
                Direction::Down => "Shift+Down / Shift+S",
                Direction::Left => "Shift+Left / Shift+A",
                Direction::Right => "Shift+Right / Shift+D",
            };
            (label, Some(hint))
        }
        SemanticAction::Navigate(Direction::Up) => ("Move up", Some("W")),
        SemanticAction::Navigate(Direction::Down) => ("Move down", Some("S")),
        SemanticAction::Navigate(Direction::Left) if surface == SurfaceId::PatchChoice => {
            ("Previous group", Some("A"))
        }
        SemanticAction::Navigate(Direction::Right) if surface == SurfaceId::PatchChoice => {
            ("Next group", Some("D"))
        }
        SemanticAction::Navigate(Direction::Left) if surface.is_system() => {
            ("Previous Settings page", Some("Left / A"))
        }
        SemanticAction::Navigate(Direction::Right) if surface.is_system() => {
            ("Next Settings page", Some("Right / D"))
        }
        SemanticAction::Navigate(Direction::Left) => ("Move left", Some("A")),
        SemanticAction::Navigate(Direction::Right) => ("Move right", Some("D")),
        SemanticAction::Adjust(Direction::Up) => ("Coarse increase", Some("K+W")),
        SemanticAction::Adjust(Direction::Down) => ("Coarse decrease", Some("K+S")),
        SemanticAction::Adjust(Direction::Left) => ("Fine decrease", Some("K+A")),
        SemanticAction::Adjust(Direction::Right) => ("Fine increase", Some("K+D")),
        SemanticAction::SetInteractionMode(InteractionMode::Navigate) => {
            ("Navigate mode", Some("release K"))
        }
        SemanticAction::SetInteractionMode(InteractionMode::Adjust) => {
            ("Adjust mode", Some("hold K"))
        }
        SemanticAction::SetInteractionMode(InteractionMode::Modal)
        | SemanticAction::SetInteractionMode(InteractionMode::MultiSelect) => {
            ("Unavailable mode", None)
        }
        SemanticAction::OpenRelated => ("Open related", None),
        SemanticAction::OpenMidiSettings => ("Open Settings", Some("Shift+Space / Shift+Start")),
        SemanticAction::Activate if surface == SurfaceId::PatchMain => {
            ("Open Detail", Some("Return"))
        }
        SemanticAction::Activate => ("Choose", Some("Return")),
        SemanticAction::PreviewStart => ("Preview", Some("hold Space")),
        SemanticAction::ToggleTestMidi => ("Toggle test MIDI", Some("T")),
        SemanticAction::PreviewStop => ("Stop preview", Some("release Space")),
        SemanticAction::SetSlotOccupancy { .. } => ("Set slot occupancy", None),
        SemanticAction::Send(crate::control::SendAction::Open) => ("Sends", Some("Ctrl+4")),
        SemanticAction::Send(_) => ("Edit send", None),
        SemanticAction::SetReturnOccupancy { .. } => ("Set return occupancy", None),
        SemanticAction::EnterSurface(SurfaceId::PatchUtility) => ("Open Utility", Some("D")),
        SemanticAction::EnterSurface(SurfaceId::Sends) => ("Sends", Some("Ctrl+4")),
        // Return is bound to Activate, which resolves the current surface.
        // This compatibility action must not advertise a second key binding.
        SemanticAction::EnterSurface(SurfaceId::PatchDetail) => ("Open Detail", None),
        SemanticAction::EnterSurface(SurfaceId::PatchChoice)
        | SemanticAction::EnterSurface(SurfaceId::FileBrowser) => ("Unavailable surface", None),
        SemanticAction::EnterSurface(SurfaceId::MixerInspector) => ("Open Inspector", None),
        SemanticAction::EnterSurface(SurfaceId::PatchMain)
        | SemanticAction::EnterSurface(SurfaceId::MixerMain)
        | SemanticAction::EnterSurface(SurfaceId::MidiDeviceSettings)
        | SemanticAction::EnterSurface(SurfaceId::ControllerSettings)
        | SemanticAction::EnterSurface(SurfaceId::SaveLoadSettings) => {
            ("Unavailable surface", None)
        }
        SemanticAction::Return => ("Return", None),
    }
}

#[cfg(test)]
mod tests {
    use super::SemanticResolver;
    use crate::control::{FocusPath, PatchControlId};
    use crate::kernel::PatchId;

    #[test]
    fn overview_return_hint_matches_detail_activation_without_duplicate_binding() {
        use crate::control::{AppEvent, AppState, SemanticAction, SurfaceId};
        let mut state = AppState::new_with_effects(
            crate::adapter::production_instruments::production_capability_registry().unwrap(),
            crate::adapter::production_effects::production_effect_registry().unwrap(),
            crate::mixer::global_parameters::GlobalParameters::new(0.0).unwrap(),
        );
        let patch = crate::synth::Patch::new(
            PatchId::new(1).unwrap(),
            "Test".into(),
            crate::adapter::braids_capability::BraidsCapability::new()
                .unwrap()
                .default_config()
                .unwrap(),
            crate::kernel::MidiChannel::new(0).unwrap(),
            crate::mixer::patch_output::PatchOutput::to_track(
                crate::mixer::mixer_track_id::MixerTrackId::new(0).unwrap(),
            ),
        );
        state.apply(AppEvent::InstallPatches(vec![patch])).unwrap();
        state
            .apply_semantic_action(SemanticAction::SelectContext(
                crate::control::TopLevelContext::Patch,
            ))
            .unwrap();
        let actions = SemanticResolver::new(&state).valid_actions();
        let returns: Vec<_> = actions
            .iter()
            .filter(|action| action.hint() == Some("Return"))
            .collect();
        assert_eq!(
            returns.len(),
            1,
            "one physical Return binding has one action guide"
        );
        assert_eq!(returns[0].action(), &SemanticAction::Activate);
        assert_eq!(returns[0].label(), "Open Detail");
        state
            .apply_semantic_action(SemanticAction::Activate)
            .unwrap();
        assert_eq!(state.interaction().active_surface(), SurfaceId::PatchDetail);
    }

    #[test]
    fn gapped_chain_resolves_overview_rows_per_position() {
        // Slot 0 empty, slot 1 occupied: the shape a compacting view would
        // silently squeeze down to position 0.
        let mut state = crate::control::AppState::new_with_effects(
            crate::adapter::production_instruments::production_capability_registry().unwrap(),
            crate::adapter::production_effects::production_effect_registry().unwrap(),
            crate::mixer::global_parameters::GlobalParameters::new(0.0).unwrap(),
        );
        let mut patch = crate::synth::Patch::new(
            PatchId::new(7).unwrap(),
            "Gapped".to_owned(),
            crate::adapter::braids_capability::BraidsCapability::new()
                .unwrap()
                .default_config()
                .unwrap(),
            crate::kernel::MidiChannel::new(3).unwrap(),
            crate::mixer::patch_output::PatchOutput::to_track(
                crate::mixer::mixer_track_id::MixerTrackId::new(3).unwrap(),
            ),
        );
        patch
            .set_slot_occupancy(
                crate::synth::effect_slot_id::EffectSlotIndex::new(1).unwrap(),
                Some(
                    crate::adapter::production_effects::production_chorus_config(
                        crate::synth::EffectSlotId::new(2).unwrap(),
                    )
                    .unwrap(),
                ),
            )
            .unwrap();
        state
            .apply(crate::control::AppEvent::InstallPatches(vec![patch]))
            .unwrap();

        let patch_id = PatchId::new(7).unwrap();
        let paths = SemanticResolver::new(&state)
            .patch_main_paths(patch_id)
            .unwrap();

        // Every position contributes its occupancy row, occupied or empty.
        for index in crate::synth::effect_slot_id::EffectSlotIndex::ALL {
            assert!(
                paths.contains(&FocusPath::patch_main(
                    patch_id,
                    None,
                    PatchControlId::EffectSlot(index),
                )),
                "position {index} must keep its occupancy row"
            );
        }
        assert_eq!(paths.len(), 4, "parameters remain on Detail");
    }

    #[test]
    fn recovery_is_exact_then_next_before_previous() {
        let patch = PatchId::new(1).unwrap();
        let engine = FocusPath::patch_main(patch, None, PatchControlId::Engine);
        let attack = FocusPath::patch_main(
            patch,
            None,
            PatchControlId::Envelope(crate::synth::VoiceEnvelopeParameter::AttackMilliseconds),
        );
        let decay = FocusPath::patch_main(
            patch,
            None,
            PatchControlId::Envelope(crate::synth::VoiceEnvelopeParameter::DecayMilliseconds),
        );
        let old = vec![engine.clone(), attack.clone(), decay.clone()];
        assert_eq!(
            SemanticResolver::recover(&attack, &old, &[engine.clone(), decay.clone()]),
            Some(decay.clone())
        );
        assert_eq!(
            SemanticResolver::recover(&engine, &old, &[engine.clone(), decay]),
            Some(engine)
        );
    }
}
