use crate::control::{Direction, SurfaceId, TopLevelContext};
use crate::kernel::patch_id::PatchId;
use crate::mixer::bus_id::BusId;
use crate::synth::effect_slot_id::EffectSlotIndex;
use crate::synth::EffectCapabilityId;
use serde::{Deserialize, Serialize};

/// The explicit reducer-owned interpretation of directional input.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InteractionMode {
    #[default]
    Navigate,
    Adjust,
    Modal,
    MultiSelect,
}

impl InteractionMode {
    pub const ALL: [Self; 4] = [Self::Navigate, Self::Adjust, Self::Modal, Self::MultiSelect];
    pub const PHASE_TWO: [Self; 2] = [Self::Navigate, Self::Adjust];
    pub const PHASE_SEVEN: [Self; 3] = [Self::Navigate, Self::Adjust, Self::Modal];

    pub const fn surface_descriptor() -> &'static [Self] {
        &Self::ALL
    }

    pub const fn is_phase_two_reachable(self) -> bool {
        matches!(self, Self::Navigate | Self::Adjust)
    }

    pub const fn is_phase_seven_reachable(self) -> bool {
        matches!(self, Self::Navigate | Self::Adjust | Self::Modal)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Navigate => "NAVIGATE",
            Self::Adjust => "ADJUST",
            Self::Modal => "MODAL",
            Self::MultiSelect => "MULTI SELECT",
        }
    }
}

/// The semantic user-intent variants accepted at passive input boundaries.
///
/// The two occupancy variants are structural topology edits: each names one
/// exact position (a Patch effect slot or a bus return) and carries an
/// optional registry entry, where `None` clears the position. The vocabulary
/// stays generic — positions and registry identities, never effect names.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    content = "payload",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SemanticAction {
    Send(crate::control::SendAction),
    SelectContext(TopLevelContext),
    /// Moves the focused Patch one position along the installed order. This is
    /// the only way the focused Patch changes: every installed instrument is
    /// reached through the same physical input → semantic action → reducer
    /// path as any other edit, never by a UI-local selection or a backstage
    /// lookup.
    SelectPatch(Direction),
    /// Resolves one directional page edge from the active surface.
    NavigatePage(Direction),
    Navigate(Direction),
    Adjust(Direction),
    SetInteractionMode(InteractionMode),
    /// Opens Detail from a subject control, or the Sample Browser from an
    /// asset row, as resolved from canonical state.
    OpenRelated,
    /// Opens the temporary MIDI Devices system surface over PATCH or MIXER.
    OpenMidiSettings,
    /// Confirms the focused stable option or browser row.
    Activate,
    /// Begins a cancellable browser-only preview hold.
    PreviewStart,
    ToggleTestMidi,
    /// Ends or cancels the current browser preview hold.
    PreviewStop,
    EnterSurface(SurfaceId),
    Return,
    SetSlotOccupancy {
        patch_id: PatchId,
        slot: EffectSlotIndex,
        entry: Option<EffectCapabilityId>,
    },
    SetReturnOccupancy {
        bus: BusId,
        entry: Option<EffectCapabilityId>,
    },
}

/// The variant-level descriptor for the closed semantic action union.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SemanticActionKind {
    Send,
    SelectContext,
    SelectPatch,
    NavigatePage,
    Navigate,
    Adjust,
    SetInteractionMode,
    OpenRelated,
    OpenMidiSettings,
    Activate,
    PreviewStart,
    ToggleTestMidi,
    PreviewStop,
    EnterSurface,
    Return,
    SetSlotOccupancy,
    SetReturnOccupancy,
}

impl SemanticActionKind {
    pub const ALL: [Self; 17] = [
        Self::Send,
        Self::SelectContext,
        Self::SelectPatch,
        Self::NavigatePage,
        Self::Navigate,
        Self::Adjust,
        Self::SetInteractionMode,
        Self::OpenRelated,
        Self::OpenMidiSettings,
        Self::Activate,
        Self::PreviewStart,
        Self::ToggleTestMidi,
        Self::PreviewStop,
        Self::EnterSurface,
        Self::Return,
        Self::SetSlotOccupancy,
        Self::SetReturnOccupancy,
    ];

    pub const fn surface_descriptor() -> &'static [Self] {
        &Self::ALL
    }
}

const SEMANTIC_ACTION_SURFACE_DESCRIPTOR: [SemanticAction; 29] = [
    SemanticAction::Send(crate::control::SendAction::Open),
    SemanticAction::SelectContext(TopLevelContext::Patch),
    SemanticAction::SelectContext(TopLevelContext::Mixer),
    // Only the horizontal pair: moving along the installed Patch order is an
    // adjacent choice, exactly like every other adjacent-choice surface.
    SemanticAction::SelectPatch(Direction::Left),
    SemanticAction::SelectPatch(Direction::Right),
    SemanticAction::NavigatePage(Direction::Up),
    SemanticAction::NavigatePage(Direction::Down),
    SemanticAction::NavigatePage(Direction::Left),
    SemanticAction::NavigatePage(Direction::Right),
    SemanticAction::Navigate(Direction::Up),
    SemanticAction::Navigate(Direction::Down),
    SemanticAction::Navigate(Direction::Left),
    SemanticAction::Navigate(Direction::Right),
    SemanticAction::Adjust(Direction::Up),
    SemanticAction::Adjust(Direction::Down),
    SemanticAction::Adjust(Direction::Left),
    SemanticAction::Adjust(Direction::Right),
    SemanticAction::SetInteractionMode(InteractionMode::Navigate),
    SemanticAction::SetInteractionMode(InteractionMode::Adjust),
    SemanticAction::OpenRelated,
    SemanticAction::OpenMidiSettings,
    SemanticAction::Activate,
    SemanticAction::PreviewStart,
    SemanticAction::ToggleTestMidi,
    SemanticAction::PreviewStop,
    SemanticAction::EnterSurface(SurfaceId::PatchUtility),
    // The descriptor lists the *admitted* surfaces, which `SurfaceId::is_enterable`
    // decides. `PatchDetail` was absent for exactly as long as no projection
    // could render a detail focus; the detail projection exists, so the
    // subordinate surface is offered like every other non-main one.
    SemanticAction::EnterSurface(SurfaceId::PatchDetail),
    SemanticAction::EnterSurface(SurfaceId::MixerInspector),
    SemanticAction::Return,
];

impl SemanticAction {
    /// Whether accepting this action can change a field represented by the
    /// versioned saved-session capture. Session replacement orchestration uses
    /// this closed classification to keep later edits from being silently
    /// discarded after the user has authorized New, Open, or Close.
    pub const fn may_change_saved_session(&self) -> bool {
        matches!(
            self,
            Self::Send(
                crate::control::SendAction::Rename { .. }
                    | crate::control::SendAction::SetEffect { .. }
            ) | Self::Adjust(_)
                | Self::Activate
                | Self::SetSlotOccupancy { .. }
                | Self::SetReturnOccupancy { .. }
        )
    }

    /// Returns the closed Phase 2 directional/navigation surface exactly once.
    ///
    /// The occupancy actions are deliberately outside this const descriptor:
    /// their registry-entry payload is an open identity, so no finite list of
    /// concrete instances exists. Their availability is proved through the
    /// reducer, like every other action.
    pub const fn surface_descriptor() -> &'static [Self] {
        &SEMANTIC_ACTION_SURFACE_DESCRIPTOR
    }

    pub const fn kind(&self) -> SemanticActionKind {
        match self {
            Self::Send(_) => SemanticActionKind::Send,
            Self::SelectContext(_) => SemanticActionKind::SelectContext,
            Self::SelectPatch(_) => SemanticActionKind::SelectPatch,
            Self::NavigatePage(_) => SemanticActionKind::NavigatePage,
            Self::Navigate(_) => SemanticActionKind::Navigate,
            Self::Adjust(_) => SemanticActionKind::Adjust,
            Self::SetInteractionMode(_) => SemanticActionKind::SetInteractionMode,
            Self::OpenRelated => SemanticActionKind::OpenRelated,
            Self::OpenMidiSettings => SemanticActionKind::OpenMidiSettings,
            Self::Activate => SemanticActionKind::Activate,
            Self::PreviewStart => SemanticActionKind::PreviewStart,
            Self::ToggleTestMidi => SemanticActionKind::ToggleTestMidi,
            Self::PreviewStop => SemanticActionKind::PreviewStop,
            Self::EnterSurface(_) => SemanticActionKind::EnterSurface,
            Self::Return => SemanticActionKind::Return,
            Self::SetSlotOccupancy { .. } => SemanticActionKind::SetSlotOccupancy,
            Self::SetReturnOccupancy { .. } => SemanticActionKind::SetReturnOccupancy,
        }
    }

    /// Reports whether the action belongs to the Phase 2 user-intent surface.
    ///
    /// `EnterSurface` admits the two persistent sides and the subordinate
    /// detail surface; a main surface is where entry *starts*, never where it
    /// lands. The predicate lives on `SurfaceId` so this vocabulary and the
    /// reducer's own admission cannot drift apart.
    pub const fn is_phase_two_admitted(&self) -> bool {
        match self {
            Self::SetInteractionMode(mode) => mode.is_phase_two_reachable(),
            Self::EnterSurface(surface) => surface.is_enterable(),
            _ => true,
        }
    }
}

/// One currently accepted semantic action plus host-neutral display metadata.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidAction {
    action: SemanticAction,
    label: String,
    hint: Option<String>,
}

impl ValidAction {
    pub fn new(
        action: SemanticAction,
        label: impl Into<String>,
        hint: Option<impl Into<String>>,
    ) -> Self {
        Self {
            action,
            label: label.into(),
            hint: hint.map(Into::into),
        }
    }

    pub const fn action(&self) -> &SemanticAction {
        &self.action
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::{InteractionMode, SemanticAction, SemanticActionKind, ValidAction};
    use crate::control::{Direction, SendAction, SurfaceId};
    use crate::kernel::PatchId;
    use crate::mixer::bus_id::BusId;
    use crate::synth::effect_slot_id::EffectSlotIndex;
    use crate::synth::EffectCapabilityId;
    use std::collections::HashSet;

    #[test]
    fn semantic_action_descriptors_are_closed_unique_and_phase_two_safe() {
        assert_eq!(SemanticActionKind::surface_descriptor().len(), 17);
        assert_eq!(InteractionMode::surface_descriptor().len(), 4);
        assert_eq!(InteractionMode::PHASE_TWO.len(), 2);
        assert_eq!(InteractionMode::PHASE_SEVEN.len(), 3);
        assert!(SemanticAction::surface_descriptor()
            .iter()
            .all(SemanticAction::is_phase_two_admitted));
        let unique = SemanticAction::surface_descriptor()
            .iter()
            .cloned()
            .collect::<HashSet<_>>();
        assert_eq!(unique.len(), SemanticAction::surface_descriptor().len());
        assert!(!SemanticAction::SetInteractionMode(InteractionMode::Modal).is_phase_two_admitted());
        assert!(!SemanticAction::EnterSurface(SurfaceId::PatchMain).is_phase_two_admitted());
        assert!(!SemanticAction::EnterSurface(SurfaceId::Sends).is_phase_two_admitted());
        assert!(
            SemanticAction::surface_descriptor().contains(&SemanticAction::Send(SendAction::Open))
        );
        assert_eq!(
            SemanticAction::Send(SendAction::Open).kind(),
            SemanticActionKind::Send
        );
        // The subordinate detail surface is offered now that a detail focus
        // projects. The descriptor lists the admitted surfaces, so it must
        // carry the action as well as admit it — one without the other is the
        // drift this pair of assertions exists to catch.
        assert!(SemanticAction::EnterSurface(SurfaceId::PatchDetail).is_phase_two_admitted());
        assert!(SemanticAction::surface_descriptor()
            .contains(&SemanticAction::EnterSurface(SurfaceId::PatchDetail)));
        for surface in SurfaceId::ALL {
            assert_eq!(
                SemanticAction::surface_descriptor()
                    .contains(&SemanticAction::EnterSurface(surface)),
                surface.is_enterable(),
                "{surface:?}: the descriptor lists exactly the admitted surfaces"
            );
        }
        assert_eq!(SemanticAction::surface_descriptor().len(), 29);
    }

    #[test]
    fn sends_shortcut_opens_a_mixer_main_surface_without_editing_the_session() {
        use crate::control::{AppEvent, AppState, SavedSession, SendControlId, TopLevelContext};
        use crate::mixer::global_parameters::GlobalParameters;
        use crate::shell::{KeyboardInputTranslator, WindowInput, WindowKey};
        use crate::synth::{CapabilityRegistry, InstrumentCapabilityProvider};

        let action = KeyboardInputTranslator::new()
            .translate(WindowInput::key_down(WindowKey::Digit4))
            .unwrap();
        assert_eq!(action, SemanticAction::Send(SendAction::Open));
        assert!(!action.may_change_saved_session());
        let mut state = AppState::new(
            CapabilityRegistry::new(vec![
                crate::adapter::braids_capability::BraidsCapability::new()
                    .unwrap()
                    .descriptor(),
            ])
            .unwrap(),
            GlobalParameters::new(0.0).unwrap(),
        );
        let saved = SavedSession::capture(&state);
        state.apply(AppEvent::from_semantic_action(action)).unwrap();
        assert_eq!(state.context(), TopLevelContext::Mixer);
        assert_eq!(state.interaction().active_surface(), SurfaceId::Sends);
        assert!(state.interaction().active_surface().is_main());
        assert_eq!(
            state.interaction().focus_path().control_id(),
            &crate::control::SemanticControlId::Send(SendControlId::Name {
                bus: BusId::default(),
            })
        );
        assert_eq!(SavedSession::capture(&state), saved);
    }

    #[test]
    fn valid_action_round_trips_typed_action_not_its_label() {
        let valid = ValidAction::new(
            SemanticAction::Adjust(Direction::Right),
            "Increase",
            Some("K+D"),
        );
        let json = serde_json::to_string(&valid).unwrap();
        let decoded: ValidAction = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, valid);
        assert_eq!(decoded.action(), &SemanticAction::Adjust(Direction::Right));
        assert_eq!(decoded.hint(), Some("K+D"));
    }

    #[test]
    fn saved_field_action_classification_is_closed_and_conservative() {
        for action in SemanticAction::surface_descriptor() {
            assert_eq!(
                action.may_change_saved_session(),
                matches!(action, SemanticAction::Adjust(_) | SemanticAction::Activate)
            );
        }
        let effect = EffectCapabilityId::new("effect.test").unwrap();
        assert!(SemanticAction::SetSlotOccupancy {
            patch_id: PatchId::new(1).unwrap(),
            slot: EffectSlotIndex::new(0).unwrap(),
            entry: Some(effect.clone()),
        }
        .may_change_saved_session());
        assert!(SemanticAction::SetReturnOccupancy {
            bus: BusId::new(0).unwrap(),
            entry: Some(effect.clone()),
        }
        .may_change_saved_session());
        for action in [
            SendAction::Rename {
                bus: BusId::default(),
                name: "Room".to_owned(),
            },
            SendAction::SetEffect {
                bus: BusId::default(),
                slot_id: crate::synth::EffectSlotId::new(1).unwrap(),
                entry: Some(effect),
            },
            SendAction::SetEffect {
                bus: BusId::default(),
                slot_id: crate::synth::EffectSlotId::new(1).unwrap(),
                entry: None,
            },
        ] {
            assert!(SemanticAction::Send(action).may_change_saved_session());
        }
        assert!(!SemanticAction::Send(SendAction::CancelRename).may_change_saved_session());
    }
}
