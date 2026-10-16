use super::*;
use crate::control::SendControlId;
use crate::synth::EffectSlotId;

impl SemanticResolver<'_> {
    pub fn send_paths(&self, bus: BusId) -> Result<Vec<FocusPath>, EventRejection> {
        let send = self
            .state
            .bus_returns()
            .get(bus)
            .ok_or(EventRejection::InvalidSelection)?;
        if let Some(origin) = self.state.interaction().send_choice_origin() {
            let SemanticControlId::Send(SendControlId::EffectSlot { slot_id, .. }) =
                origin.control_id()
            else {
                return Err(EventRejection::InvalidSelection);
            };
            return Ok(self
                .send_choices(bus, *slot_id)?
                .into_iter()
                .filter(|choice| choice.is_enabled())
                .map(|choice| {
                    FocusPath::send(
                        SendControlId::Choice {
                            bus,
                            slot_id: *slot_id,
                            entry: choice.id().to_owned(),
                        },
                        None,
                    )
                })
                .collect());
        }
        let mut paths = vec![
            FocusPath::send(SendControlId::Name { bus }, None),
            FocusPath::send(SendControlId::Level { bus }, None),
        ];
        for config in send.effects() {
            let slot_id = config.slot_id();
            paths.push(FocusPath::send(
                SendControlId::EffectSlot { bus, slot_id },
                None,
            ));
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
                    paths.push(FocusPath::send(
                        SendControlId::EffectParameter {
                            bus,
                            slot_id,
                            parameter: spec.id().clone(),
                        },
                        Some(config.capability_id().clone()),
                    ));
                }
            }
        }
        if let Some(slot_id) = send.next_slot_id() {
            paths.push(FocusPath::send(
                SendControlId::EffectSlot { bus, slot_id },
                None,
            ));
        }
        Ok(paths)
    }

    pub fn send_choices(
        &self,
        bus: BusId,
        slot_id: EffectSlotId,
    ) -> Result<Vec<ResolvedChoiceOption>, EventRejection> {
        let send = self
            .state
            .bus_returns()
            .get(bus)
            .ok_or(EventRejection::InvalidSelection)?;
        let current = send.effect_at(slot_id).map(|config| config.capability_id());
        Ok(effect_occupancy_options(self.state.effects(), current))
    }
}

/// Shared registry order, availability, categories, and EMPTY choice for Patch and send slots.
pub(super) fn effect_occupancy_options(
    registry: &crate::synth::EffectCapabilityRegistry,
    current: Option<&crate::synth::EffectCapabilityId>,
) -> Vec<ResolvedChoiceOption> {
    let descriptors = registry.descriptors();
    let empty_category = descriptors
        .iter()
        .filter(|d| d.availability().is_enabled())
        .map(|d| d.effect_category())
        .min()
        .or_else(|| descriptors.iter().map(|d| d.effect_category()).min())
        .unwrap_or_default();
    let mut empty = ResolvedChoiceOption::enabled(
        crate::control::EMPTY_OCCUPANCY_CHOICE_ID,
        "EMPTY",
        current.is_none(),
    );
    empty.category = Some(ChoiceCategory::Effect(empty_category));
    std::iter::once(empty)
        .chain(descriptors.iter().map(|descriptor| {
            let mut option = ResolvedChoiceOption::with_availability(
                descriptor.id().to_string(),
                descriptor.label(),
                current == Some(descriptor.id()),
                descriptor.availability().clone(),
            );
            option.category = Some(ChoiceCategory::Effect(descriptor.effect_category()));
            option
        }))
        .collect()
}
