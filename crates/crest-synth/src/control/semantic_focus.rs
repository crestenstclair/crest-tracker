use crate::control::{MidiInputDeviceId, PatchControlId, PatchPositionId, TopLevelContext};
use crate::kernel::PatchId;
use crate::mixer::bus_id::BusId;
use crate::mixer::global_parameters::GlobalParameter;
use crate::mixer::mixer_track_id::MixerTrackId;
use crate::mixer::mixer_track_parameters::MixerTrackParameter;
use crate::synth::{CapabilityId, EffectCapabilityId, EffectSlotId, ParameterId};
use core::fmt;
use serde::{Deserialize, Serialize};

/// Stable graphical surfaces independent of host layout or rectangle placement.
///
/// PATCH and MIXER have persistent mains and sides; Sends is another MIXER
/// main surface. Three surfaces are subordinate: never the resting surface of a context, and
/// leaving one restores the exact origin. One detail surface identity serves
/// both instrument and effect subjects, because the surface is the shell and
/// the subject supplies the content. Settings surfaces suspend rather than
/// replace PATCH or MIXER.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SurfaceId {
    Sends,
    PatchMain,
    PatchUtility,
    PatchDetail,
    PatchChoice,
    FileBrowser,
    MixerMain,
    MixerInspector,
    MidiDeviceSettings,
    ControllerSettings,
    SaveLoadSettings,
}

impl SurfaceId {
    pub const ALL: [Self; 11] = [
        Self::Sends,
        Self::PatchMain,
        Self::PatchUtility,
        Self::PatchDetail,
        Self::PatchChoice,
        Self::FileBrowser,
        Self::MixerMain,
        Self::MixerInspector,
        Self::MidiDeviceSettings,
        Self::ControllerSettings,
        Self::SaveLoadSettings,
    ];

    pub const fn surface_descriptor() -> &'static [Self] {
        &Self::ALL
    }

    /// Returns the owning performance context, or `None` for a temporary
    /// system surface that suspends rather than replaces PATCH/MIXER.
    pub const fn context(self) -> Option<TopLevelContext> {
        match self {
            Self::PatchMain | Self::PatchUtility | Self::PatchDetail | Self::PatchChoice => {
                Some(TopLevelContext::Patch)
            }
            Self::Sends | Self::MixerMain | Self::MixerInspector => Some(TopLevelContext::Mixer),
            Self::MidiDeviceSettings
            | Self::ControllerSettings
            | Self::SaveLoadSettings
            | Self::FileBrowser => None,
        }
    }

    pub const fn main_for(context: TopLevelContext) -> Self {
        match context {
            TopLevelContext::Patch => Self::PatchMain,
            TopLevelContext::Mixer => Self::MixerMain,
        }
    }

    /// Returns the context's *persistent* side surface. `PatchDetail` is never
    /// a result: it is subordinate, so it has no resting place to be returned
    /// as a context's side.
    pub const fn side_for(context: TopLevelContext) -> Self {
        match context {
            TopLevelContext::Patch => Self::PatchUtility,
            TopLevelContext::Mixer => Self::MixerInspector,
        }
    }

    pub const fn is_main(self) -> bool {
        matches!(self, Self::PatchMain | Self::MixerMain | Self::Sends)
    }

    pub const fn is_persistent_side(self) -> bool {
        matches!(self, Self::PatchUtility | Self::MixerInspector)
    }

    /// Reports whether this surface is subordinate: entered from a main path,
    /// remembered by exactly one [`ReturnPath`], and left through `Return`.
    pub const fn is_subordinate(self) -> bool {
        matches!(
            self,
            Self::PatchDetail | Self::PatchChoice | Self::FileBrowser
        )
    }

    pub const fn is_system(self) -> bool {
        matches!(
            self,
            Self::MidiDeviceSettings | Self::ControllerSettings | Self::SaveLoadSettings
        )
    }

    /// Reports whether a [`ReturnPath`] may name this surface as the one it was
    /// entered *into*: structurally, every non-main surface.
    ///
    /// This is the shape rule, and it is deliberately separate from
    /// [`Self::is_enterable`], which is the *admission* rule for the
    /// `EnterSurface` action. Keeping them apart is what lets the reducer own,
    /// remember, and leave a surface that the action vocabulary does not yet
    /// offer.
    pub const fn is_return_target(self) -> bool {
        self.is_persistent_side() || self.is_subordinate()
    }

    /// Reports whether `EnterSurface` currently admits this surface as a target.
    ///
    /// This is the single predicate `EnterSurface` admission is decided by, so
    /// the admitted action vocabulary and the reducer cannot drift apart.
    ///
    /// `PatchDetail` was withheld here for exactly as long as no projection
    /// could render a detail focus — advertising an action whose accepted state
    /// the shell cannot show is worse than not advertising it, and `AppLoop`
    /// treats a projection failure on an accepted state as a panic. The detail
    /// projection now exists, so the gate is gone and every non-main surface is
    /// offered.
    ///
    /// The predicate stays separate from [`Self::is_return_target`] even though
    /// the two currently agree on every surface: one is the *shape* rule for
    /// what a `ReturnPath` may name, the other the *admission* rule for an
    /// action, and collapsing them would mean the next surface that needs to be
    /// reducer-owned before it is offered has nowhere to say so. The match is
    /// exhaustive so a new surface must answer it rather than inherit one.
    pub const fn is_enterable(self) -> bool {
        match self {
            Self::PatchUtility | Self::MixerInspector | Self::PatchDetail => true,
            Self::Sends
            | Self::PatchChoice
            | Self::FileBrowser
            | Self::PatchMain
            | Self::MixerMain
            | Self::MidiDeviceSettings
            | Self::ControllerSettings
            | Self::SaveLoadSettings => false,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Sends => "SENDS",
            Self::PatchMain => "PATCH",
            Self::PatchUtility => "UTILITY",
            Self::PatchDetail => "DETAIL",
            Self::PatchChoice => "OPTIONS",
            Self::FileBrowser => "FILE BROWSER",
            Self::MixerMain => "MIXER",
            Self::MixerInspector => "INSPECTOR",
            Self::MidiDeviceSettings => "MIDI DEVICES",
            Self::ControllerSettings => "CONTROLLER BUTTONS",
            Self::SaveLoadSettings => "SAVE & LOAD",
        }
    }
}

/// The capability whose schema fills the polymorphic PATCH detail surface.
///
/// The subject names a capability identity **only**. It carries no section
/// list, control list, value, label, accent, geometry, or descriptor copy:
/// those resolve from the installed descriptor at projection time and would
/// otherwise become a second schema. `Effect` carries the exact occupied slot
/// identity, so two positions holding the same registry entry are distinct
/// subjects.
// `rename_all` renames the *variants* of a tagged enum, never a struct
// variant's fields, so this needs `rename_all_fields` as well or the subject's
// fields land snake_case inside an otherwise camelCase schema.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PatchDetailSubject {
    Instrument {
        capability_id: CapabilityId,
    },
    Effect {
        slot_id: EffectSlotId,
        capability_id: EffectCapabilityId,
    },
}

impl PatchDetailSubject {
    /// Names one instrument capability as the detail subject.
    pub const fn instrument(capability_id: CapabilityId) -> Self {
        Self::Instrument { capability_id }
    }

    /// Names one *exact occupied slot* as the detail subject.
    pub const fn effect(slot_id: EffectSlotId, capability_id: EffectCapabilityId) -> Self {
        Self::Effect {
            slot_id,
            capability_id,
        }
    }

    /// Returns the occupied slot identity for an effect subject.
    pub const fn slot_id(&self) -> Option<EffectSlotId> {
        match self {
            Self::Effect { slot_id, .. } => Some(*slot_id),
            Self::Instrument { .. } => None,
        }
    }

    /// Returns the subject as the capability identity a `FocusPath` carries.
    pub fn focus_capability_id(&self) -> FocusCapabilityId {
        match self {
            Self::Instrument { capability_id } => {
                FocusCapabilityId::Instrument(capability_id.clone())
            }
            Self::Effect { capability_id, .. } => FocusCapabilityId::Effect(capability_id.clone()),
        }
    }
}

/// The exact generic PATCH control whose available values fill one choice modal.
///
/// The subject carries no option list or active value. Those are resolved from
/// the current instrument/effect registries, Mixer track domain, or owning
/// descriptor every time the modal is projected.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchChoiceSubject {
    #[serde(rename = "patchId")]
    patch_position: PatchPositionId,
    control_id: PatchControlId,
}

impl PatchChoiceSubject {
    pub const fn new(patch_id: PatchId, control_id: PatchControlId) -> Self {
        Self::at(PatchPositionId::Created(patch_id), control_id)
    }

    pub const fn at(patch_position: PatchPositionId, control_id: PatchControlId) -> Self {
        Self {
            patch_position,
            control_id,
        }
    }

    pub const fn patch_id(&self) -> Option<PatchId> {
        self.patch_position.patch_id()
    }

    pub const fn patch_position(&self) -> PatchPositionId {
        self.patch_position
    }

    pub const fn control_id(&self) -> &PatchControlId {
        &self.control_id
    }

    pub fn stable_id(&self) -> String {
        // The FocusPath already owns the Patch-position root. Keeping that
        // root out of the modal-local identity lets the exact Choice row
        // survive the empty-to-created rekey without rewriting a second,
        // stringly encoded copy of the same identity.
        format!("patch.choice.{}", self.control_id.as_str())
    }

    pub(crate) fn rekey_trailing_empty(&mut self, patch_id: PatchId) {
        if self.patch_position == PatchPositionId::TrailingEmpty {
            self.patch_position = PatchPositionId::Created(patch_id);
        }
    }
}

/// Stable semantic identity of one row inside a trapped modal surface.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "camelCase")]
pub enum ModalControlId {
    Choice(String),
    BrowserEntry(String),
}

impl ModalControlId {
    pub fn stable_id(&self) -> &str {
        match self {
            Self::Choice(id) | Self::BrowserEntry(id) => id,
        }
    }
}

/// Stable identity of one MIXER control, independent of section coordinates.
///
/// Sends and returns are index-addressed: a send is the pair of one
/// `MixerTrackId` and one `BusId`; return rows are addressed by `BusId` and,
/// for the occupying entry's scalar rows, a descriptor `ParameterId`. No
/// variant names a concrete effect, and a return's contents changing never
/// changes its identity.
// Same `rename_all` limitation as [`PatchDetailSubject`] above: without
// `rename_all_fields` the `track_id` leaf serializes snake_case inside an
// otherwise camelCase schema. Fixing one tagged union and not the others left
// the schema *mixed*, which is harder to reason about than uniformly wrong, so
// all three moved together (mission finding F-18).
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MixerControlId {
    Track {
        track_id: MixerTrackId,
        parameter: MixerTrackParameter,
    },
    Send {
        track_id: MixerTrackId,
        bus: BusId,
    },
    ReturnOccupancy {
        bus: BusId,
    },
    ReturnLevel {
        bus: BusId,
    },
    ReturnEffect {
        bus: BusId,
        parameter: ParameterId,
    },
    Global {
        parameter: GlobalParameter,
    },
}

impl MixerControlId {
    pub const fn track_id(&self) -> Option<MixerTrackId> {
        match self {
            Self::Track { track_id, .. } | Self::Send { track_id, .. } => Some(*track_id),
            Self::ReturnOccupancy { .. }
            | Self::ReturnLevel { .. }
            | Self::ReturnEffect { .. }
            | Self::Global { .. } => None,
        }
    }

    pub const fn track_parameter(&self) -> Option<MixerTrackParameter> {
        match self {
            Self::Track { parameter, .. } => Some(*parameter),
            Self::Send { .. }
            | Self::ReturnOccupancy { .. }
            | Self::ReturnLevel { .. }
            | Self::ReturnEffect { .. }
            | Self::Global { .. } => None,
        }
    }

    /// Returns the addressed bus for send and return identities.
    pub const fn bus(&self) -> Option<BusId> {
        match self {
            Self::Send { bus, .. }
            | Self::ReturnOccupancy { bus }
            | Self::ReturnLevel { bus }
            | Self::ReturnEffect { bus, .. } => Some(*bus),
            Self::Track { .. } | Self::Global { .. } => None,
        }
    }

    pub const fn global_parameter(&self) -> Option<GlobalParameter> {
        match self {
            Self::Global { parameter } => Some(*parameter),
            Self::Track { .. }
            | Self::Send { .. }
            | Self::ReturnOccupancy { .. }
            | Self::ReturnLevel { .. }
            | Self::ReturnEffect { .. } => None,
        }
    }
}

/// Optional capability identity carried by a semantic PATCH focus path.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "camelCase")]
pub enum FocusCapabilityId {
    Instrument(CapabilityId),
    Effect(EffectCapabilityId),
}

/// Stable control identity used across PATCH, MIXER, and side-surface roots.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "camelCase")]
pub enum SemanticControlId {
    Send(crate::control::SendControlId),
    Patch(PatchControlId),
    Mixer(MixerControlId),
    Modal(ModalControlId),
    MidiInputDevice(MidiInputDeviceId),
    MidiInputListRoot,
    ControllerSetting(crate::control::ControllerSettingId),
    SessionFileAction(crate::control::SessionCommand),
    SurfaceRoot,
}

impl SemanticControlId {
    pub const fn as_mixer_track_id(&self) -> Option<MixerTrackId> {
        match self {
            Self::Mixer(MixerControlId::Track { track_id, .. }) => Some(*track_id),
            Self::Send(_)
            | Self::Mixer(_)
            | Self::Patch(_)
            | Self::Modal(_)
            | Self::MidiInputDevice(_)
            | Self::MidiInputListRoot
            | Self::ControllerSetting(_)
            | Self::SessionFileAction(_)
            | Self::SurfaceRoot => None,
        }
    }
}

/// A malformed semantic path rejected before it can enter canonical state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FocusPathError {
    ContextSurfaceMismatch,
    ControlSurfaceMismatch,
    PatchIdentityMismatch,
    CapabilityIdentityMismatch,
    ModalIdentityUnavailable,
}

impl fmt::Display for FocusPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ContextSurfaceMismatch => "focus context and surface are incompatible",
            Self::ControlSurfaceMismatch => "focus control and surface are incompatible",
            Self::PatchIdentityMismatch => "focus path has an invalid Patch identity shape",
            Self::CapabilityIdentityMismatch => {
                "focus path has an invalid capability identity shape"
            }
            Self::ModalIdentityUnavailable => "focus path has an invalid modal identity",
        })
    }
}

impl std::error::Error for FocusPathError {}

/// The single canonical semantic location of interaction.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusPath {
    context: TopLevelContext,
    surface: SurfaceId,
    #[serde(rename = "patchId")]
    patch_position: Option<PatchPositionId>,
    capability_id: Option<FocusCapabilityId>,
    control_id: SemanticControlId,
    modal_id: Option<String>,
}

impl FocusPath {
    pub fn patch_main(
        patch_id: PatchId,
        capability_id: Option<FocusCapabilityId>,
        control_id: PatchControlId,
    ) -> Self {
        Self::patch_main_at(patch_id.into(), capability_id, control_id)
    }

    pub fn patch_main_at(
        patch_position: PatchPositionId,
        capability_id: Option<FocusCapabilityId>,
        control_id: PatchControlId,
    ) -> Self {
        Self {
            context: TopLevelContext::Patch,
            surface: SurfaceId::PatchMain,
            patch_position: Some(patch_position),
            capability_id,
            control_id: SemanticControlId::Patch(control_id),
            modal_id: None,
        }
    }

    pub const fn patch_utility(patch_id: PatchId, control_id: PatchControlId) -> Self {
        Self::patch_utility_at(PatchPositionId::Created(patch_id), control_id)
    }

    pub const fn patch_utility_at(
        patch_position: PatchPositionId,
        control_id: PatchControlId,
    ) -> Self {
        Self {
            context: TopLevelContext::Patch,
            surface: SurfaceId::PatchUtility,
            patch_position: Some(patch_position),
            capability_id: None,
            control_id: SemanticControlId::Patch(control_id),
            modal_id: None,
        }
    }

    /// One control on the subordinate PATCH detail surface.
    ///
    /// The path carries the subject's capability identity, so a row belonging
    /// to one capability is never mistaken for the same-named row of another.
    pub const fn patch_detail(
        patch_id: PatchId,
        capability_id: FocusCapabilityId,
        control_id: PatchControlId,
    ) -> Self {
        Self::patch_detail_at(
            PatchPositionId::Created(patch_id),
            capability_id,
            control_id,
        )
    }

    pub const fn patch_detail_at(
        patch_position: PatchPositionId,
        capability_id: FocusCapabilityId,
        control_id: PatchControlId,
    ) -> Self {
        Self {
            context: TopLevelContext::Patch,
            surface: SurfaceId::PatchDetail,
            patch_position: Some(patch_position),
            capability_id: Some(capability_id),
            control_id: SemanticControlId::Patch(control_id),
            modal_id: None,
        }
    }

    /// One stable installed/descriptor choice inside a trapped PATCH modal.
    pub fn patch_choice(
        patch_id: PatchId,
        modal_id: impl Into<String>,
        choice_id: impl Into<String>,
    ) -> Self {
        Self::patch_choice_at(patch_id.into(), modal_id, choice_id)
    }

    pub fn patch_choice_at(
        patch_position: PatchPositionId,
        modal_id: impl Into<String>,
        choice_id: impl Into<String>,
    ) -> Self {
        Self {
            context: TopLevelContext::Patch,
            surface: SurfaceId::PatchChoice,
            patch_position: Some(patch_position),
            capability_id: None,
            control_id: SemanticControlId::Modal(ModalControlId::Choice(choice_id.into())),
            modal_id: Some(modal_id.into()),
        }
    }

    /// One stable parent/folder/file/cancel row inside the Sample Browser.
    pub fn file_browser(
        patch_id: PatchId,
        modal_id: impl Into<String>,
        entry_id: impl Into<String>,
    ) -> Self {
        Self::file_browser_at(patch_id.into(), modal_id, entry_id)
    }

    pub fn file_browser_at(
        patch_position: PatchPositionId,
        modal_id: impl Into<String>,
        entry_id: impl Into<String>,
    ) -> Self {
        Self {
            context: TopLevelContext::Patch,
            surface: SurfaceId::FileBrowser,
            patch_position: Some(patch_position),
            capability_id: None,
            control_id: SemanticControlId::Modal(ModalControlId::BrowserEntry(entry_id.into())),
            modal_id: Some(modal_id.into()),
        }
    }

    /// Browser rows retain the owning context and optional Patch identity.
    pub fn file_browser_for_origin(
        origin: &FocusPath,
        modal_id: impl Into<String>,
        entry_id: impl Into<String>,
    ) -> Self {
        Self {
            context: origin.context,
            surface: SurfaceId::FileBrowser,
            patch_position: origin.patch_position,
            capability_id: None,
            control_id: SemanticControlId::Modal(ModalControlId::BrowserEntry(entry_id.into())),
            modal_id: Some(modal_id.into()),
        }
    }

    pub fn send(
        control: crate::control::SendControlId,
        capability_id: Option<EffectCapabilityId>,
    ) -> Self {
        Self {
            context: TopLevelContext::Mixer,
            surface: SurfaceId::Sends,
            patch_position: None,
            capability_id: capability_id.map(FocusCapabilityId::Effect),
            control_id: SemanticControlId::Send(control),
            modal_id: None,
        }
    }

    pub const fn mixer_track(track_id: MixerTrackId, parameter: MixerTrackParameter) -> Self {
        Self {
            context: TopLevelContext::Mixer,
            surface: SurfaceId::MixerMain,
            patch_position: None,
            capability_id: None,
            control_id: SemanticControlId::Mixer(MixerControlId::Track {
                track_id,
                parameter,
            }),
            modal_id: None,
        }
    }

    /// One indexed send row on the MIXER Inspector: `(track, bus)`.
    pub const fn mixer_send(track_id: MixerTrackId, bus: BusId) -> Self {
        Self::mixer_inspector_control(MixerControlId::Send { track_id, bus }, None)
    }

    /// One bus return's occupancy row on the MIXER Inspector.
    pub const fn mixer_return_occupancy(bus: BusId) -> Self {
        Self::mixer_inspector_control(MixerControlId::ReturnOccupancy { bus }, None)
    }

    /// One bus return's return-owned level row on the MIXER Inspector.
    pub const fn mixer_return_level(bus: BusId) -> Self {
        Self::mixer_inspector_control(MixerControlId::ReturnLevel { bus }, None)
    }

    /// One occupying registry entry's scalar row on the MIXER Inspector. The
    /// path carries the occupant's registry identity so a different occupant's
    /// row is a different focus target.
    pub const fn mixer_return_effect(
        bus: BusId,
        parameter: ParameterId,
        capability_id: EffectCapabilityId,
    ) -> Self {
        Self::mixer_inspector_control(
            MixerControlId::ReturnEffect { bus, parameter },
            Some(FocusCapabilityId::Effect(capability_id)),
        )
    }

    const fn mixer_inspector_control(
        control: MixerControlId,
        capability_id: Option<FocusCapabilityId>,
    ) -> Self {
        Self {
            context: TopLevelContext::Mixer,
            surface: SurfaceId::MixerInspector,
            patch_position: None,
            capability_id,
            control_id: SemanticControlId::Mixer(control),
            modal_id: None,
        }
    }

    pub const fn mixer_global(parameter: GlobalParameter) -> Self {
        Self::mixer_inspector_control(MixerControlId::Global { parameter }, None)
    }

    pub fn side_root(surface: SurfaceId) -> Result<Self, FocusPathError> {
        if !surface.is_persistent_side() {
            return Err(FocusPathError::ControlSurfaceMismatch);
        }
        Ok(Self {
            context: surface
                .context()
                .ok_or(FocusPathError::ContextSurfaceMismatch)?,
            surface,
            patch_position: None,
            capability_id: None,
            control_id: SemanticControlId::SurfaceRoot,
            modal_id: None,
        })
    }

    /// One exact device row on the temporary MIDI Devices system surface.
    pub const fn midi_device_settings(
        suspended_context: TopLevelContext,
        identity: MidiInputDeviceId,
    ) -> Self {
        Self {
            context: suspended_context,
            surface: SurfaceId::MidiDeviceSettings,
            patch_position: None,
            capability_id: None,
            control_id: SemanticControlId::MidiInputDevice(identity),
            modal_id: None,
        }
    }

    /// Stable focus target used when discovery currently has no device rows.
    pub const fn midi_device_settings_root(suspended_context: TopLevelContext) -> Self {
        Self {
            context: suspended_context,
            surface: SurfaceId::MidiDeviceSettings,
            patch_position: None,
            capability_id: None,
            control_id: SemanticControlId::MidiInputListRoot,
            modal_id: None,
        }
    }

    pub fn save_load_settings(
        context: TopLevelContext,
        action: crate::control::SessionCommand,
    ) -> Self {
        Self {
            context,
            surface: SurfaceId::SaveLoadSettings,
            patch_position: None,
            capability_id: None,
            control_id: SemanticControlId::SessionFileAction(action),
            modal_id: None,
        }
    }

    pub const fn controller_settings(
        context: TopLevelContext,
        setting: crate::control::ControllerSettingId,
    ) -> Self {
        Self {
            context,
            surface: SurfaceId::ControllerSettings,
            patch_position: None,
            capability_id: None,
            control_id: SemanticControlId::ControllerSetting(setting),
            modal_id: None,
        }
    }

    /// Revalidates a deserialized or externally constructed path shape.
    pub fn validate(&self) -> Result<(), FocusPathError> {
        if let SemanticControlId::SessionFileAction(action) = self.control_id {
            if !crate::control::SessionCommand::SETTINGS_ACTIONS.contains(&action) {
                return Err(FocusPathError::ControlSurfaceMismatch);
            }
        }
        if let Some(surface_context) = self.surface.context() {
            if surface_context != self.context {
                return Err(FocusPathError::ContextSurfaceMismatch);
            }
        }
        match (&self.surface, &self.control_id) {
            (SurfaceId::Sends, SemanticControlId::Send(control)) => {
                if self.patch_position.is_some() || self.modal_id.is_some() {
                    return Err(FocusPathError::ControlSurfaceMismatch);
                }
                let valid_capability = match control {
                    crate::control::SendControlId::EffectParameter { .. } => {
                        matches!(self.capability_id, Some(FocusCapabilityId::Effect(_)))
                    }
                    _ => self.capability_id.is_none(),
                };
                if !valid_capability {
                    return Err(FocusPathError::CapabilityIdentityMismatch);
                }
            }
            (SurfaceId::PatchMain, SemanticControlId::Patch(control)) => {
                if self.patch_position.is_none() {
                    return Err(FocusPathError::PatchIdentityMismatch);
                }
                // The five Utility identities and the PatchMain order are
                // disjoint by declaration; the split is decided in exactly one
                // place so the two surfaces cannot both claim a row.
                if control.is_utility() {
                    return Err(FocusPathError::ControlSurfaceMismatch);
                }
            }
            (SurfaceId::PatchUtility, SemanticControlId::Patch(control))
                if control.is_utility() =>
            {
                if self.patch_position.is_none() || self.capability_id.is_some() {
                    return Err(FocusPathError::PatchIdentityMismatch);
                }
            }
            // The detail surface hosts the subject capability's own descriptor
            // rows: an instrument capability parameter, or one occupant's
            // parameter at its exact slot. The path always carries the
            // subject's capability identity, matched to the row's kind.
            (
                SurfaceId::PatchDetail,
                SemanticControlId::Patch(
                    PatchControlId::Capability(_) | PatchControlId::Envelope(_),
                ),
            ) => {
                if self.patch_position.is_none() {
                    return Err(FocusPathError::PatchIdentityMismatch);
                }
                if !matches!(self.capability_id, Some(FocusCapabilityId::Instrument(_))) {
                    return Err(FocusPathError::CapabilityIdentityMismatch);
                }
            }
            (SurfaceId::PatchDetail, SemanticControlId::Patch(PatchControlId::Effect(..))) => {
                if self.patch_position.is_none() {
                    return Err(FocusPathError::PatchIdentityMismatch);
                }
                if !matches!(self.capability_id, Some(FocusCapabilityId::Effect(_))) {
                    return Err(FocusPathError::CapabilityIdentityMismatch);
                }
            }
            (SurfaceId::PatchChoice, SemanticControlId::Modal(ModalControlId::Choice(id))) => {
                if self.patch_position.is_none()
                    || self.capability_id.is_some()
                    || self.modal_id.as_ref().is_none_or(String::is_empty)
                    || id.is_empty()
                {
                    return Err(FocusPathError::ModalIdentityUnavailable);
                }
            }
            (
                SurfaceId::FileBrowser,
                SemanticControlId::Modal(ModalControlId::BrowserEntry(id)),
            ) => {
                if (self.context == TopLevelContext::Patch) != self.patch_position.is_some()
                    || self.capability_id.is_some()
                    || self.modal_id.as_ref().is_none_or(String::is_empty)
                    || id.is_empty()
                {
                    return Err(FocusPathError::ModalIdentityUnavailable);
                }
            }
            (
                SurfaceId::MixerMain,
                SemanticControlId::Mixer(MixerControlId::Track { parameter, .. }),
            ) => {
                if !MixerTrackParameter::MAIN.contains(parameter)
                    || self.patch_position.is_some()
                    || self.capability_id.is_some()
                {
                    return Err(FocusPathError::ControlSurfaceMismatch);
                }
            }
            (
                SurfaceId::MixerInspector,
                SemanticControlId::Mixer(
                    MixerControlId::Send { .. }
                    | MixerControlId::ReturnOccupancy { .. }
                    | MixerControlId::ReturnLevel { .. },
                ),
            ) => {
                if self.patch_position.is_some() || self.capability_id.is_some() {
                    return Err(FocusPathError::ControlSurfaceMismatch);
                }
            }
            (
                SurfaceId::MixerInspector,
                SemanticControlId::Mixer(MixerControlId::ReturnEffect { .. }),
            ) => {
                if self.patch_position.is_some()
                    || !matches!(self.capability_id, Some(FocusCapabilityId::Effect(_)))
                {
                    return Err(FocusPathError::CapabilityIdentityMismatch);
                }
            }
            (
                SurfaceId::MixerInspector,
                SemanticControlId::Mixer(MixerControlId::Global { .. }),
            ) => {
                if self.patch_position.is_some() || self.capability_id.is_some() {
                    return Err(FocusPathError::PatchIdentityMismatch);
                }
            }
            (surface, SemanticControlId::SurfaceRoot) if surface.is_persistent_side() => {
                if self.patch_position.is_some() || self.capability_id.is_some() {
                    return Err(FocusPathError::CapabilityIdentityMismatch);
                }
            }
            (
                SurfaceId::MidiDeviceSettings,
                SemanticControlId::MidiInputDevice(_) | SemanticControlId::MidiInputListRoot,
            )
            | (SurfaceId::ControllerSettings, SemanticControlId::ControllerSetting(_))
            | (SurfaceId::SaveLoadSettings, SemanticControlId::SessionFileAction(_)) => {
                if self.patch_position.is_some()
                    || self.capability_id.is_some()
                    || self.modal_id.is_some()
                {
                    return Err(FocusPathError::ControlSurfaceMismatch);
                }
            }
            _ => return Err(FocusPathError::ControlSurfaceMismatch),
        }
        if self.surface != SurfaceId::PatchChoice
            && self.surface != SurfaceId::FileBrowser
            && self.modal_id.is_some()
        {
            return Err(FocusPathError::ModalIdentityUnavailable);
        }
        Ok(())
    }

    pub const fn context(&self) -> TopLevelContext {
        self.context
    }

    pub const fn surface(&self) -> SurfaceId {
        self.surface
    }

    pub const fn patch_id(&self) -> Option<PatchId> {
        match self.patch_position {
            Some(PatchPositionId::Created(patch_id)) => Some(patch_id),
            Some(PatchPositionId::TrailingEmpty) | None => None,
        }
    }

    pub const fn patch_position(&self) -> Option<PatchPositionId> {
        self.patch_position
    }

    pub(crate) fn rekey_trailing_empty(&mut self, patch_id: PatchId) {
        if self.patch_position == Some(PatchPositionId::TrailingEmpty) {
            self.patch_position = Some(PatchPositionId::Created(patch_id));
        }
    }

    pub const fn capability_id(&self) -> Option<&FocusCapabilityId> {
        self.capability_id.as_ref()
    }

    pub const fn control_id(&self) -> &SemanticControlId {
        &self.control_id
    }

    pub const fn midi_input_device_id(&self) -> Option<&MidiInputDeviceId> {
        match &self.control_id {
            SemanticControlId::MidiInputDevice(identity) => Some(identity),
            _ => None,
        }
    }

    pub fn modal_id(&self) -> Option<&str> {
        self.modal_id.as_deref()
    }
}

/// Exact main-surface origin restored after leaving a subordinate surface.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReturnPath {
    origin: FocusPath,
    entered_surface: SurfaceId,
}

impl ReturnPath {
    /// Captures one exact origin before entering a subordinate or persistent
    /// side surface.
    ///
    /// Persistent side and Detail entry still require a main origin. A Choice
    /// or Sample Browser may replace a PATCH main, Utility, or Detail surface;
    /// the browser also admits MIXER Inspector and Sends. They remember that
    /// exact origin, but a modal/browser origin is rejected;
    /// therefore there is one replaceable subordinate session, never a stack.
    pub fn new(origin: FocusPath, entered_surface: SurfaceId) -> Result<Self, FocusPathError> {
        origin.validate()?;
        let origin_allowed = match entered_surface {
            SurfaceId::FileBrowser => matches!(
                origin.surface(),
                SurfaceId::PatchMain
                    | SurfaceId::PatchUtility
                    | SurfaceId::PatchDetail
                    | SurfaceId::MixerInspector
                    | SurfaceId::Sends
            ),
            SurfaceId::PatchChoice => {
                matches!(
                    origin.surface(),
                    SurfaceId::PatchMain | SurfaceId::PatchUtility | SurfaceId::PatchDetail
                )
            }
            _ => origin.surface().is_main(),
        };
        if !origin_allowed
            || !entered_surface.is_return_target()
            || entered_surface
                .context()
                .is_some_and(|context| context != origin.context())
        {
            return Err(FocusPathError::ContextSurfaceMismatch);
        }
        Ok(Self {
            origin,
            entered_surface,
        })
    }

    pub const fn origin(&self) -> &FocusPath {
        &self.origin
    }

    pub const fn entered_surface(&self) -> SurfaceId {
        self.entered_surface
    }

    pub(crate) fn rekey_trailing_empty(&mut self, patch_id: PatchId) {
        self.origin.rekey_trailing_empty(patch_id);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FocusPath, FocusPathError, MixerControlId, PatchDetailSubject, ReturnPath,
        SemanticControlId, SurfaceId,
    };
    use crate::control::{PatchControlId, PatchPositionId};
    use crate::kernel::PatchId;
    use crate::mixer::global_parameters::GlobalParameter;

    #[test]
    fn performance_and_system_surfaces_are_classified_without_a_third_context() {
        use crate::control::TopLevelContext;

        assert_eq!(SurfaceId::surface_descriptor().len(), 11);
        assert_eq!(SurfaceId::PatchMain.context(), Some(TopLevelContext::Patch));
        assert_eq!(SurfaceId::Sends.context(), Some(TopLevelContext::Mixer));
        assert!(SurfaceId::Sends.is_main());
        assert!(!SurfaceId::Sends.is_system());
        assert_eq!(
            SurfaceId::MixerInspector.context(),
            Some(crate::control::TopLevelContext::Mixer)
        );
        assert_eq!(SurfaceId::MidiDeviceSettings.context(), None);
        assert!(SurfaceId::MidiDeviceSettings.is_system());
        assert_eq!(SurfaceId::ControllerSettings.context(), None);
        assert!(SurfaceId::ControllerSettings.is_system());
        assert!(SurfaceId::PatchUtility.is_persistent_side());
        assert!(SurfaceId::MixerMain.is_main());

        // Every surface is exactly one of main, persistent side, subordinate,
        // or system — a surface that were two at once would let the reducer
        // treat it as a resting place and a return target simultaneously.
        for surface in SurfaceId::ALL {
            let roles = usize::from(surface.is_main())
                + usize::from(surface.is_persistent_side())
                + usize::from(surface.is_subordinate())
                + usize::from(surface.is_system());
            assert_eq!(roles, 1, "{surface:?} must hold exactly one surface role");
        }
        assert!(!SurfaceId::MidiDeviceSettings.is_return_target());
        assert!(!SurfaceId::MidiDeviceSettings.is_enterable());
        assert_eq!(
            TopLevelContext::surface_descriptor(),
            &[TopLevelContext::Patch, TopLevelContext::Mixer]
        );
        assert_eq!(
            serde_json::to_string(&TopLevelContext::Patch).unwrap(),
            "\"patch\""
        );
        assert_eq!(
            serde_json::to_string(&TopLevelContext::Mixer).unwrap(),
            "\"mixer\""
        );
    }

    /// Persistent sides and Detail are direct entry targets; Choice and the
    /// Sample Browser are opened only through their stable subjects.
    ///
    /// `EnterSurface` admission was deliberately narrower than the structural
    /// return-target rule for exactly one surface, for exactly one reason: no
    /// projection could render a detail focus. The detail projection landed, so
    /// admission and shape now agree on every surface — and this asserts that
    /// agreement rather than assuming it, so a surface that silently stops
    /// being offered is a failure and not a shrug.
    #[test]
    fn direct_entry_and_subject_opened_surfaces_have_distinct_admission() {
        assert!(SurfaceId::PatchDetail.is_return_target());
        assert!(
            SurfaceId::PatchDetail.is_enterable(),
            "the detail projection exists, so the entry gate is gone"
        );
        for offered in [
            SurfaceId::PatchUtility,
            SurfaceId::PatchDetail,
            SurfaceId::MixerInspector,
        ] {
            assert!(offered.is_enterable());
            assert!(offered.is_return_target());
        }
        for subject_opened in [SurfaceId::PatchChoice, SurfaceId::FileBrowser] {
            assert!(!subject_opened.is_enterable());
            assert!(subject_opened.is_return_target());
        }
        for main in [SurfaceId::PatchMain, SurfaceId::MixerMain, SurfaceId::Sends] {
            assert!(!main.is_enterable());
            assert!(!main.is_return_target());
        }
    }

    /// The subordinate surface belongs to PATCH, is never a context's resting
    /// place, and is reachable only by entering it from a main path.
    #[test]
    fn the_detail_surface_is_subordinate_and_never_a_resting_surface() {
        use crate::control::TopLevelContext;

        assert_eq!(
            SurfaceId::PatchDetail.context(),
            Some(TopLevelContext::Patch)
        );
        assert!(SurfaceId::PatchDetail.is_subordinate());
        assert!(!SurfaceId::PatchDetail.is_main());
        assert!(!SurfaceId::PatchDetail.is_persistent_side());

        for context in [TopLevelContext::Patch, TopLevelContext::Mixer] {
            assert_ne!(SurfaceId::main_for(context), SurfaceId::PatchDetail);
            assert_ne!(SurfaceId::side_for(context), SurfaceId::PatchDetail);
        }
    }

    /// One detail surface identity serves both subject kinds, and an effect
    /// subject is distinguished by its *slot*, not just its capability — so
    /// two positions holding the same registry entry are distinct subjects.
    #[test]
    fn a_detail_subject_names_a_capability_and_an_effect_names_its_exact_slot() {
        use crate::synth::{CapabilityId, EffectCapabilityId, EffectSlotId};

        let capability = EffectCapabilityId::new("effect.fixture").unwrap();
        let first = PatchDetailSubject::effect(EffectSlotId::new(1).unwrap(), capability.clone());
        let second = PatchDetailSubject::effect(EffectSlotId::new(2).unwrap(), capability);
        assert_ne!(
            first, second,
            "the same registry entry in two slots must be two subjects"
        );
        assert_eq!(first.slot_id(), Some(EffectSlotId::new(1).unwrap()));

        let instrument =
            PatchDetailSubject::instrument(CapabilityId::new("instrument.fixture").unwrap());
        assert_eq!(instrument.slot_id(), None);
        assert_ne!(
            instrument.focus_capability_id(),
            first.focus_capability_id()
        );
    }

    #[test]
    fn focus_paths_round_trip_stable_semantic_ids() {
        let patch_id = PatchId::new(7).unwrap();
        let patch = FocusPath::patch_main(patch_id, None, PatchControlId::Engine);
        let global = FocusPath::mixer_global(GlobalParameter::MasterGainDb);
        let patch_json = serde_json::to_string(&patch).unwrap();
        let global_json = serde_json::to_string(&global).unwrap();
        assert_eq!(
            serde_json::from_str::<FocusPath>(&patch_json).unwrap(),
            patch
        );
        assert_eq!(
            serde_json::from_str::<FocusPath>(&global_json).unwrap(),
            global
        );
        assert_eq!(patch.patch_id(), Some(patch_id));
        assert_eq!(
            global.control_id(),
            &SemanticControlId::Mixer(MixerControlId::Global {
                parameter: GlobalParameter::MasterGainDb
            })
        );
    }

    #[test]
    fn sends_focus_preserves_bus_slot_and_effect_capability_identity() {
        use crate::control::{SendControlId, TopLevelContext};
        use crate::mixer::bus_id::BusId;
        use crate::synth::{EffectCapabilityId, EffectSlotId, ParameterId};

        let bus = BusId::new(18).unwrap();
        let slot_id = EffectSlotId::new(9).unwrap();
        let capability = EffectCapabilityId::new("effect.fixture").unwrap();
        for (control, capability) in [
            (SendControlId::Name { bus }, None),
            (SendControlId::Level { bus }, None),
            (SendControlId::EffectSlot { bus, slot_id }, None),
            (
                SendControlId::EffectParameter {
                    bus,
                    slot_id,
                    parameter: ParameterId::new("amount").unwrap(),
                },
                Some(capability),
            ),
        ] {
            let focus = FocusPath::send(control.clone(), capability);
            assert!(focus.validate().is_ok());
            assert_eq!(focus.context(), TopLevelContext::Mixer);
            assert_eq!(focus.surface(), SurfaceId::Sends);
            assert_eq!(focus.patch_id(), None);
            assert_eq!(focus.control_id(), &SemanticControlId::Send(control));
            let json = serde_json::to_string(&focus).unwrap();
            assert_eq!(serde_json::from_str::<FocusPath>(&json).unwrap(), focus);
        }
    }

    #[test]
    fn send_effect_browser_keeps_exact_origin_and_rejects_wrong_capability_shape() {
        use crate::control::SendControlId;
        use crate::mixer::bus_id::BusId;
        use crate::synth::{CapabilityId, EffectCapabilityId, EffectSlotId, ParameterId};

        let bus = BusId::new(5).unwrap();
        let mut origin = FocusPath::send(
            SendControlId::EffectParameter {
                bus,
                slot_id: EffectSlotId::new(3).unwrap(),
                parameter: ParameterId::new("effect.asset").unwrap(),
            },
            Some(EffectCapabilityId::new("effect.fixture").unwrap()),
        );
        let path = ReturnPath::new(origin.clone(), SurfaceId::FileBrowser).unwrap();
        assert_eq!(path.origin(), &origin);
        assert_eq!(path.entered_surface(), SurfaceId::FileBrowser);
        origin.capability_id = Some(super::FocusCapabilityId::Instrument(
            CapabilityId::new("instrument.fixture").unwrap(),
        ));
        assert_eq!(
            origin.validate(),
            Err(FocusPathError::CapabilityIdentityMismatch)
        );
        assert_eq!(
            ReturnPath::new(origin.clone(), SurfaceId::FileBrowser),
            Err(FocusPathError::CapabilityIdentityMismatch)
        );
        origin.capability_id = None;
        assert_eq!(
            origin.validate(),
            Err(FocusPathError::CapabilityIdentityMismatch)
        );
        let named = FocusPath::send(
            SendControlId::Name { bus },
            Some(EffectCapabilityId::new("effect.fixture").unwrap()),
        );
        assert_eq!(
            named.validate(),
            Err(FocusPathError::CapabilityIdentityMismatch)
        );
    }

    #[test]
    fn trailing_empty_focus_is_explicit_without_fabricating_a_patch_id() {
        let empty =
            FocusPath::patch_main_at(PatchPositionId::TrailingEmpty, None, PatchControlId::Engine);
        assert!(empty.validate().is_ok());
        assert_eq!(empty.patch_position(), Some(PatchPositionId::TrailingEmpty));
        assert_eq!(empty.patch_id(), None);

        let json = serde_json::to_value(&empty).unwrap();
        assert_eq!(json["patchId"], "trailingEmpty");
        assert_eq!(serde_json::from_value::<FocusPath>(json).unwrap(), empty);
    }

    #[test]
    fn created_focus_keeps_its_numeric_patch_id_serialization() {
        let created = FocusPath::patch_main(PatchId::new(9).unwrap(), None, PatchControlId::Engine);
        let json = serde_json::to_value(&created).unwrap();
        assert_eq!(json["patchId"], 9);
        assert_eq!(
            created.patch_position(),
            Some(PatchPositionId::Created(PatchId::new(9).unwrap()))
        );
    }

    #[test]
    fn send_and_return_paths_are_index_addressed_inspector_targets() {
        use crate::mixer::bus_id::BusId;
        let send = FocusPath::mixer_send(
            crate::mixer::mixer_track_id::MixerTrackId::default(),
            BusId::new(5).unwrap(),
        );
        assert!(send.validate().is_ok());
        assert_eq!(send.surface(), SurfaceId::MixerInspector);

        let occupancy = FocusPath::mixer_return_occupancy(BusId::new(7).unwrap());
        let level = FocusPath::mixer_return_level(BusId::new(7).unwrap());
        assert!(occupancy.validate().is_ok());
        assert!(level.validate().is_ok());
        assert_ne!(occupancy, level);

        let scalar = FocusPath::mixer_return_effect(
            BusId::new(0).unwrap(),
            crate::synth::ParameterId::new("amount").unwrap(),
            crate::synth::EffectCapabilityId::new("effect.fixture").unwrap(),
        );
        assert!(scalar.validate().is_ok());
        let json = serde_json::to_string(&scalar).unwrap();
        assert_eq!(serde_json::from_str::<FocusPath>(&json).unwrap(), scalar);
    }

    #[test]
    fn return_path_requires_exact_same_context_main_origin() {
        let origin = FocusPath::patch_main(PatchId::new(1).unwrap(), None, PatchControlId::Engine);
        let path = ReturnPath::new(origin.clone(), SurfaceId::PatchUtility).unwrap();
        assert_eq!(path.origin(), &origin);
        assert_eq!(path.entered_surface(), SurfaceId::PatchUtility);
        assert_eq!(
            ReturnPath::new(origin, SurfaceId::MixerInspector),
            Err(FocusPathError::ContextSurfaceMismatch)
        );
        assert_eq!(
            FocusPath::side_root(SurfaceId::PatchMain),
            Err(FocusPathError::ControlSurfaceMismatch)
        );
    }

    #[test]
    fn midi_settings_paths_round_trip_exact_device_and_empty_root_identities() {
        use crate::control::{MidiInputDeviceId, TopLevelContext};

        let identity = MidiInputDeviceId::new("midir-v1", "coremidi:opaque-a").unwrap();
        let device = FocusPath::midi_device_settings(TopLevelContext::Patch, identity.clone());
        let root = FocusPath::midi_device_settings_root(TopLevelContext::Mixer);

        assert!(device.validate().is_ok());
        assert!(root.validate().is_ok());
        assert_eq!(device.midi_input_device_id(), Some(&identity));
        assert_eq!(root.midi_input_device_id(), None);
        assert_eq!(device.surface(), SurfaceId::MidiDeviceSettings);
        assert_eq!(root.surface(), SurfaceId::MidiDeviceSettings);
        assert_eq!(device.context(), TopLevelContext::Patch);
        assert_eq!(root.context(), TopLevelContext::Mixer);

        let device_json = serde_json::to_value(&device).unwrap();
        assert_eq!(device_json["surface"], "midiDeviceSettings");
        assert_eq!(device_json["controlId"]["kind"], "midiInputDevice");
        assert_eq!(
            device_json["controlId"]["id"]["identity"],
            "coremidi:opaque-a"
        );
        assert_eq!(
            serde_json::from_value::<FocusPath>(device_json).unwrap(),
            device
        );

        let root_json = serde_json::to_value(&root).unwrap();
        assert_eq!(root_json["controlId"]["kind"], "midiInputListRoot");
        assert_eq!(
            serde_json::from_value::<FocusPath>(root_json).unwrap(),
            root
        );
    }
}
