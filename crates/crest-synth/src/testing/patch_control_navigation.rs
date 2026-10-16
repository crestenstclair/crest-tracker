use crate::control::{
    AppState, Direction, EventRejection, InteractionMode, PatchControlId, SemanticAction,
    SemanticControlId, SemanticResolver, SurfaceId, TopLevelContext,
};
use crate::synth::effect_slot_id::EffectSlotIndex;

/// Resolves one ordinary navigation gesture toward a stable PATCH control.
/// Scenes use the same resolver and reducer as physical input; no focus is
/// installed behind the reducer. `None` means the target is already focused.
pub(crate) fn next_patch_control_action(
    state: &AppState,
    target: &PatchControlId,
) -> Result<Option<SemanticAction>, EventRejection> {
    if state.context() != TopLevelContext::Patch {
        return Ok(Some(SemanticAction::SelectContext(TopLevelContext::Patch)));
    }
    let interaction = state.interaction();
    if interaction.focus_path().control_id() == &SemanticControlId::Patch(target.clone()) {
        return Ok(None);
    }
    if interaction.mode() != InteractionMode::Navigate {
        return Ok(Some(SemanticAction::SetInteractionMode(
            InteractionMode::Navigate,
        )));
    }
    let surface = interaction.active_surface();
    if target.is_utility() && surface != SurfaceId::PatchUtility {
        return Ok(Some(SemanticAction::EnterSurface(SurfaceId::PatchUtility)));
    }
    let paths = SemanticResolver::new(state).ordered_paths(surface)?;
    let current = paths
        .iter()
        .position(|path| path == interaction.focus_path())
        .ok_or(EventRejection::InvalidSelection)?;
    if let Some(destination) = paths
        .iter()
        .position(|path| path.control_id() == &SemanticControlId::Patch(target.clone()))
    {
        return Ok(Some(SemanticAction::Navigate(if current < destination {
            Direction::Down
        } else {
            Direction::Up
        })));
    }
    if matches!(surface, SurfaceId::PatchDetail | SurfaceId::PatchUtility) {
        return Ok(Some(SemanticAction::Return));
    }
    let origin = match target {
        PatchControlId::Capability(_) => PatchControlId::Engine,
        PatchControlId::Effect(slot, _) => state
            .patches()
            .iter()
            .find(|patch| Some(patch.id()) == interaction.patch_focus())
            .and_then(|patch| {
                patch.effect_slots().iter().position(|effect| {
                    effect
                        .as_ref()
                        .is_some_and(|effect| effect.slot_id() == *slot)
                })
            })
            .and_then(|index| EffectSlotIndex::new(index).ok())
            .map(PatchControlId::EffectSlot)
            .ok_or(EventRejection::InvalidSelection)?,
        _ => return Err(EventRejection::InvalidSelection),
    };
    if interaction.focus_path().control_id() == &SemanticControlId::Patch(origin.clone()) {
        Ok(Some(SemanticAction::EnterSurface(SurfaceId::PatchDetail)))
    } else {
        next_patch_control_action(state, &origin)
    }
}
