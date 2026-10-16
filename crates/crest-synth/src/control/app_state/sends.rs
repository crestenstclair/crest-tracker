use super::*;
use crate::control::{SendAction, SendControlId};

impl AppState {
    pub(super) fn reduce_send_action(
        &mut self,
        action: SendAction,
    ) -> Result<ReducerEffects, EventRejection> {
        match action {
            SendAction::Open => {
                if self.interaction.active_surface().is_system()
                    || self.file_browser.preview_is_held()
                {
                    return Err(EventRejection::ActionUnavailableInContext);
                }
                self.select_context(TopLevelContext::Mixer)?;
                let focus = self.interaction.remembered_send.clone();
                self.interaction
                    .set_active_main(focus)
                    .map_err(|_| EventRejection::InvalidSelection)?;
                self.interaction.send_choice_origin = None;
                self.interaction.send_name_editing = false;
            }
            SendAction::Rename { bus, name } => {
                if self.interaction.active_surface() != SurfaceId::Sends
                    || !self.interaction.send_name_editing
                    || self.interaction.selected_send() != bus
                {
                    return Err(EventRejection::ActionUnavailableInContext);
                }
                Arc::make_mut(&mut self.returns)
                    .set_name(bus, &name)
                    .map_err(|_| EventRejection::InvalidParameterValue)?;
                self.interaction.send_name_editing = false;
            }
            SendAction::CancelRename => {
                self.interaction.send_name_editing = false;
            }
            SendAction::SetEffect {
                bus,
                slot_id,
                entry,
            } => {
                let send = self
                    .returns
                    .get(bus)
                    .ok_or(EventRejection::InvalidSelection)?;
                if send.effect_at(slot_id).map(|effect| effect.capability_id()) == entry.as_ref() {
                    return Ok(ReducerEffects::default());
                }
                let effect = self.request_topology_change(StructuralEditIntent::SetSendEffect {
                    bus,
                    slot_id,
                    entry,
                })?;
                return Ok(ReducerEffects {
                    engine_selection_effect: Some(effect),
                    ..ReducerEffects::default()
                });
            }
        }
        Ok(ReducerEffects::default())
    }

    pub(super) fn select_send(
        &mut self,
        direction: Direction,
    ) -> Result<ReducerEffects, EventRejection> {
        if self.interaction.mode() != crate::control::InteractionMode::Navigate
            || self.interaction.send_choice_origin.is_some()
            || self.interaction.send_name_editing
            || !matches!(direction, Direction::Left | Direction::Right)
        {
            return Err(EventRejection::ActionUnavailableInContext);
        }
        let current = self.interaction.selected_send().index();
        let next = match direction {
            Direction::Left => current.checked_sub(1),
            _ => current.checked_add(1).filter(|i| *i < self.returns.len()),
        }
        .ok_or(EventRejection::ParameterAtBoundary)?;
        let bus = BusId::new(next as u16).map_err(|_| EventRejection::InvalidSelection)?;
        self.interaction
            .set_active_main(FocusPath::send(SendControlId::Name { bus }, None))
            .map_err(|_| EventRejection::InvalidSelection)?;
        Ok(ReducerEffects::default())
    }

    pub(super) fn navigate_send(
        &mut self,
        direction: Direction,
    ) -> Result<ReducerEffects, EventRejection> {
        if self.interaction.send_name_editing {
            return Err(EventRejection::ActionUnavailableInContext);
        }
        let mut paths = SemanticResolver::new(self).send_paths(self.interaction.selected_send())?;
        let active = self.interaction.focus_path();
        if let SemanticControlId::Send(SendControlId::Choice {
            bus,
            slot_id,
            entry,
        }) = active.control_id()
        {
            let options = SemanticResolver::new(self).send_choices(*bus, *slot_id)?;
            let category = options
                .iter()
                .find(|o| o.id() == entry)
                .and_then(|o| o.category());
            if matches!(direction, Direction::Left | Direction::Right) {
                let mut groups = options
                    .iter()
                    .filter(|o| o.is_enabled())
                    .filter_map(|o| o.category())
                    .collect::<Vec<_>>();
                groups.sort();
                groups.dedup();
                let index = groups
                    .iter()
                    .position(|g| Some(*g) == category)
                    .unwrap_or(0);
                let next = if direction == Direction::Left {
                    (index + groups.len() - 1) % groups.len()
                } else {
                    (index + 1) % groups.len()
                };
                let option = options
                    .iter()
                    .find(|o| o.is_enabled() && o.category() == Some(groups[next]))
                    .ok_or(EventRejection::InvalidSelection)?;
                self.interaction.active_focus = FocusPath::send(
                    SendControlId::Choice {
                        bus: *bus,
                        slot_id: *slot_id,
                        entry: option.id().to_owned(),
                    },
                    None,
                );
                return Ok(ReducerEffects::default());
            }
            paths.retain(|path| match path.control_id() {
                SemanticControlId::Send(SendControlId::Choice { entry, .. }) => options
                    .iter()
                    .any(|o| o.id() == entry && o.category() == category),
                _ => false,
            });
        }
        let index = paths
            .iter()
            .position(|path| path == self.interaction.focus_path())
            .ok_or(EventRejection::InvalidSelection)?;
        let next = if matches!(direction, Direction::Up | Direction::Left) {
            index.checked_sub(1).unwrap_or(paths.len() - 1)
        } else {
            (index + 1) % paths.len()
        };
        self.interaction.active_focus = paths[next].clone();
        if self.interaction.send_choice_origin.is_none() {
            self.interaction.remembered_send = paths[next].clone();
        }
        Ok(ReducerEffects::default())
    }

    pub(super) fn activate_send(&mut self) -> Result<ReducerEffects, EventRejection> {
        let SemanticControlId::Send(control) = self.interaction.focus_path().control_id().clone()
        else {
            return Err(EventRejection::InvalidSelection);
        };
        match control {
            SendControlId::Name { .. } => {
                if self.interaction.mode() != crate::control::InteractionMode::Navigate {
                    return Err(EventRejection::ActionUnavailableInContext);
                }
                self.interaction.send_name_editing = true;
            }
            SendControlId::EffectSlot { bus, slot_id } => {
                if self.engine_selection.is_in_flight() {
                    return Err(EventRejection::StructuralEditBusy);
                }
                let choices = SemanticResolver::new(self).send_choices(bus, slot_id)?;
                let current = choices
                    .iter()
                    .find(|o| o.is_current() && o.is_enabled())
                    .or_else(|| choices.iter().find(|o| o.is_enabled()))
                    .ok_or(EventRejection::InvalidSelection)?;
                self.interaction.send_choice_origin = Some(self.interaction.active_focus.clone());
                self.interaction.active_focus = FocusPath::send(
                    SendControlId::Choice {
                        bus,
                        slot_id,
                        entry: current.id().to_owned(),
                    },
                    None,
                );
                self.interaction.mode = crate::control::InteractionMode::Modal;
            }
            SendControlId::Choice {
                bus,
                slot_id,
                entry,
            } => {
                let entry = if entry == crate::control::EMPTY_OCCUPANCY_CHOICE_ID {
                    None
                } else {
                    Some(
                        EffectCapabilityId::new(entry)
                            .map_err(|_| EventRejection::InvalidEffectConfig)?,
                    )
                };
                let outcome = self.reduce_send_action(SendAction::SetEffect {
                    bus,
                    slot_id,
                    entry,
                })?;
                self.close_send_choice()?;
                return Ok(outcome);
            }
            SendControlId::EffectParameter { .. } => {
                self.open_related_surface()?;
            }
            _ => return Err(EventRejection::ActionUnavailableInContext),
        }
        Ok(ReducerEffects::default())
    }

    pub(super) fn close_send_choice(&mut self) -> Result<(), EventRejection> {
        let origin = self
            .interaction
            .send_choice_origin
            .take()
            .ok_or(EventRejection::ActionUnavailableInContext)?;
        self.interaction.active_focus = origin.clone();
        self.interaction.remembered_send = origin;
        self.interaction.mode = crate::control::InteractionMode::Navigate;
        Ok(())
    }

    pub(super) fn adjust_send_control(
        &mut self,
        direction: Direction,
    ) -> Result<ReducerEffects, EventRejection> {
        if self.interaction.send_choice_origin.is_some() || self.interaction.send_name_editing {
            return Err(EventRejection::ActionUnavailableInContext);
        }
        let SemanticControlId::Send(control) = self.interaction.focus_path().control_id().clone()
        else {
            return Err(EventRejection::InvalidSelection);
        };
        match control {
            SendControlId::Level { bus } => self.adjust_return_level(bus, direction)?,
            SendControlId::EffectSlot { bus, slot_id } => {
                if direction == Direction::Up {
                    return self.activate_send();
                }
                let config = self.returns.bus_return(bus).effect_at(slot_id);
                let entry = self.adjacent_occupancy_entry(
                    config.map(|effect| effect.capability_id()),
                    direction,
                )?;
                return self.reduce_send_action(SendAction::SetEffect {
                    bus,
                    slot_id,
                    entry,
                });
            }
            SendControlId::EffectParameter {
                bus,
                slot_id,
                parameter,
            } => {
                let config = self
                    .returns
                    .bus_return(bus)
                    .effect_at(slot_id)
                    .ok_or(EventRejection::InvalidSelection)?
                    .clone();
                let descriptor = self
                    .effects
                    .descriptor(config.capability_id())
                    .ok_or(EventRejection::InvalidEffectConfig)?;
                let spec = descriptor
                    .parameter(&parameter)
                    .ok_or(EventRejection::InvalidSelection)?;
                if spec.update() != crate::synth::ParameterUpdate::Scalar
                    || spec.patch_interaction() != PatchInteraction::ScalarEdit
                {
                    return Err(EventRejection::ActionUnavailableInContext);
                }
                let current = config
                    .value(&parameter)
                    .ok_or(EventRejection::InvalidEffectConfig)?;
                let value = spec
                    .adjusted_scalar_value(current, parameter_adjustment(direction))
                    .map_err(map_scalar_adjustment_error)?;
                let candidate = config
                    .with_scalar_value(descriptor, &parameter, value)
                    .map_err(|_| EventRejection::InvalidEffectConfig)?;
                Arc::make_mut(&mut self.returns)
                    .replace_occupant_values(bus, candidate)
                    .map_err(|_| EventRejection::InvalidEffectConfig)?;
            }
            _ => return Err(EventRejection::ActionUnavailableInContext),
        }
        Ok(ReducerEffects::default())
    }

    pub(super) fn repair_send_focus(&mut self) -> Result<(), EventRejection> {
        let repair = |path: &FocusPath| -> Result<FocusPath, EventRejection> {
            let resolver = SemanticResolver::new(self);
            if resolver.resolves(path) {
                return Ok(path.clone());
            }
            match path.control_id() {
                SemanticControlId::Send(control) => {
                    let paths = resolver.send_paths(control.bus())?;
                    let slot = match control {
                        SendControlId::EffectParameter { bus, slot_id, .. } => {
                            Some(FocusPath::send(
                                SendControlId::EffectSlot {
                                    bus: *bus,
                                    slot_id: *slot_id,
                                },
                                None,
                            ))
                        }
                        _ => None,
                    };
                    Ok(slot
                        .filter(|path| paths.contains(path))
                        .unwrap_or_else(|| paths[0].clone()))
                }
                SemanticControlId::Patch(PatchControlId::Send(_)) => {
                    let paths = resolver.patch_utility_paths(
                        path.patch_id().ok_or(EventRejection::InvalidSelection)?,
                    )?;
                    Ok(paths
                        .last()
                        .cloned()
                        .ok_or(EventRejection::InvalidSelection)?)
                }
                _ => Ok(path.clone()),
            }
        };
        let active = repair(&self.interaction.active_focus)?;
        let remembered = repair(&self.interaction.remembered_send)?;
        let origin = self
            .interaction
            .return_path()
            .map(|path| repair(path.origin()))
            .transpose()?;
        let suspended = self
            .interaction
            .settings_session()
            .map(|session| repair(session.suspended_focus()))
            .transpose()?;
        let suspended_origin = self
            .interaction
            .settings_session()
            .and_then(|session| session.suspended_return_path())
            .map(|path| repair(path.origin()))
            .transpose()?;
        self.interaction.active_focus = active;
        self.interaction.remembered_send = remembered;
        if let Some(origin) = origin {
            self.interaction
                .replace_return_origin(origin)
                .map_err(|_| EventRejection::InvalidSelection)?;
        }
        if let Some(focus) = suspended {
            self.interaction
                .replace_settings_suspended_focus(focus)
                .map_err(|_| EventRejection::InvalidSelection)?;
        }
        if let Some(origin) = suspended_origin {
            self.interaction
                .replace_settings_suspended_return_origin(origin)
                .map_err(|_| EventRejection::InvalidSelection)?;
        }
        Ok(())
    }
}
