use super::*;
use crate::control::SendControlId;

pub(super) fn project(
    state: &AppState,
    resolver: &SemanticResolver<'_>,
    status: &SemanticLifecycleStatus,
    errors: &[SemanticError],
) -> Result<Vec<SemanticSurfaceViewModel>, SemanticGraphicalViewModelError> {
    let bus = state.interaction().selected_send();
    let send = state
        .bus_returns()
        .get(bus)
        .ok_or(SemanticGraphicalViewModelError::InvalidFocusPath)?;
    let active = state.interaction().focus_path();
    let paths = resolver.send_paths(bus).map_err(map_resolver_error)?;
    let mut controls = Vec::new();
    let mut sections = Vec::new();
    let mut choice_group_label = None;
    if let Some(origin) = state.interaction().send_choice_origin() {
        let SemanticControlId::Send(SendControlId::EffectSlot { slot_id, .. }) =
            origin.control_id()
        else {
            return Err(SemanticGraphicalViewModelError::InvalidFocusPath);
        };
        let options = resolver
            .send_choices(bus, *slot_id)
            .map_err(map_resolver_error)?;
        let category = match active.control_id() {
            SemanticControlId::Send(SendControlId::Choice { entry, .. }) => options
                .iter()
                .find(|option| option.id() == entry)
                .and_then(|option| option.category()),
            _ => None,
        };
        choice_group_label = category.map(|category| category.label().to_owned());
        for option in options {
            let path = FocusPath::send(
                SendControlId::Choice {
                    bus,
                    slot_id: *slot_id,
                    entry: option.id().to_owned(),
                },
                None,
            );
            let mut control = row(
                state,
                path,
                option.label().to_owned(),
                SemanticControlKind::Choice,
                SemanticControlValue::Identity(option.id().to_owned()),
            );
            control.selected_label = option.is_current().then(|| "CURRENT".to_owned());
            control.visible = option.category() == category;
            control.enabled = option.is_enabled();
            control.focusable = option.is_enabled();
            control.availability_label = if option.is_enabled() {
                None
            } else {
                Some("Unavailable".to_owned())
            };
            controls.push(control);
        }
    } else {
        for path in paths {
            let SemanticControlId::Send(control_id) = path.control_id().clone() else {
                unreachable!()
            };
            let mut control = match control_id.clone() {
                SendControlId::Name { .. } => row(
                    state,
                    path,
                    "Name".to_owned(),
                    SemanticControlKind::Identity,
                    SemanticControlValue::Identity(send.name().to_owned()),
                ),
                SendControlId::Level { .. } => {
                    let mut row = row(
                        state,
                        path,
                        "Return Level".to_owned(),
                        SemanticControlKind::Continuous,
                        SemanticControlValue::Scalar(send.return_level() as f64),
                    );
                    let descriptor = crate::mixer::bus_return::RETURN_LEVEL_DESCRIPTOR;
                    row.numeric_range = Some(SemanticNumericRange::new(
                        descriptor.minimum() as f64,
                        descriptor.maximum() as f64,
                        descriptor.fine_step() as f64,
                        descriptor.coarse_step() as f64,
                    ));
                    row.editable = true;
                    row
                }
                SendControlId::EffectSlot { slot_id, .. } => {
                    let config = send.effect_at(slot_id);
                    let label = config
                        .and_then(|config| state.effects().descriptor(config.capability_id()))
                        .map(|d| d.label())
                        .unwrap_or("EMPTY");
                    let ordinal = send
                        .effects()
                        .iter()
                        .position(|effect| effect.slot_id() == slot_id)
                        .map(|index| index + 1);
                    row(
                        state,
                        path,
                        ordinal
                            .map(|index| format!("FX {index:02}"))
                            .unwrap_or_else(|| "Add Effect".to_owned()),
                        SemanticControlKind::Choice,
                        SemanticControlValue::Identity(label.to_owned()),
                    )
                }
                SendControlId::EffectParameter {
                    slot_id, parameter, ..
                } => {
                    let config = send
                        .effect_at(slot_id)
                        .ok_or(SemanticGraphicalViewModelError::InvalidEffectConfig)?;
                    let descriptor = state
                        .effects()
                        .descriptor(config.capability_id())
                        .ok_or(SemanticGraphicalViewModelError::InvalidEffectConfig)?;
                    let spec = descriptor
                        .parameter(&parameter)
                        .ok_or(SemanticGraphicalViewModelError::InvalidEffectConfig)?;
                    control_from_parameter(
                        path,
                        spec,
                        effect_parameter_value(spec, config)?,
                        ParameterControlProjection {
                            active,
                            enabled: true,
                            visible: true,
                            focusable: true,
                            editable: spec.patch_interaction() == PatchInteraction::ScalarEdit,
                            status: None,
                            errors,
                        },
                    )
                }
                SendControlId::Choice { .. } => unreachable!(),
            };
            if let Some(correlation) = state.engine_selection().correlation() {
                if let crate::control::StructuralEditIntent::SetSendEffect {
                    bus: target,
                    slot_id,
                    ..
                } = correlation.intent()
                {
                    if *target == bus
                        && control.path.control_id()
                            == &SemanticControlId::Send(SendControlId::EffectSlot {
                                bus,
                                slot_id: *slot_id,
                            })
                    {
                        control.status = Some(status.clone());
                        control.error = error_for_path(errors, &control.path);
                    }
                }
            }
            match control_id {
                SendControlId::Name { .. } => control.editable = true,
                SendControlId::EffectSlot { .. } => {
                    control.editable = !state.engine_selection().is_in_flight()
                }
                _ => {}
            }
            controls.push(control);
        }
        let general = controls
            .iter()
            .filter(|c| {
                matches!(
                    c.path.control_id(),
                    SemanticControlId::Send(
                        SendControlId::Name { .. } | SendControlId::Level { .. }
                    )
                )
            })
            .map(|c| c.path.clone())
            .collect();
        sections.push(SemanticSurfaceSectionViewModel {
            leading_controls: false,
            id: "send.identity".to_owned(),
            label: "Send".to_owned(),
            control_paths: general,
            control_summaries: Vec::new(),
        });
        for config in send.effects() {
            let paths = controls.iter().filter(|c| matches!(c.path.control_id(), SemanticControlId::Send(SendControlId::EffectSlot { slot_id, .. } | SendControlId::EffectParameter { slot_id, .. }) if *slot_id == config.slot_id())).map(|c| c.path.clone()).collect();
            let label = state
                .effects()
                .descriptor(config.capability_id())
                .ok_or(SemanticGraphicalViewModelError::InvalidEffectConfig)?
                .label()
                .to_owned();
            sections.push(SemanticSurfaceSectionViewModel {
                leading_controls: false,
                id: format!("send.effect.{}", config.slot_id()),
                label,
                control_paths: paths,
                control_summaries: Vec::new(),
            });
        }
        if let Some(slot_id) = send.next_slot_id() {
            sections.push(SemanticSurfaceSectionViewModel {
                leading_controls: false,
                id: "send.add".to_owned(),
                label: "Effects".to_owned(),
                control_paths: vec![FocusPath::send(
                    SendControlId::EffectSlot { bus, slot_id },
                    None,
                )],
                control_summaries: Vec::new(),
            });
        }
    }
    Ok(vec![SemanticSurfaceViewModel {
        id: SurfaceId::Sends,
        label: "SENDS".to_owned(),
        role: SemanticSurfaceRole::Main,
        controls,
        sections,
        visualizations: Vec::new(),
        summary: SemanticSurfaceSummary::Sends {
            bus,
            name: send.name().to_owned(),
            count: state.bus_returns().len(),
            editing_name: state.interaction().send_name_editing(),
            choosing_effect: state.interaction().send_choice_origin().is_some(),
            choice_group_label,
        },
    }])
}

fn row(
    state: &AppState,
    path: FocusPath,
    label: String,
    kind: SemanticControlKind,
    value: SemanticControlValue,
) -> SemanticControlViewModel {
    SemanticControlViewModel {
        focused: state.interaction().focus_path() == &path,
        path,
        label,
        kind,
        value,
        selected_label: None,
        numeric_range: None,
        unit: None,
        browser_metadata: None,
        availability_label: None,
        read_only_label: None,
        enabled: true,
        visible: true,
        focusable: true,
        editable: false,
        status: None,
        error: None,
        requested_value: None,
        requested_label: None,
        patch_interaction: None,
        valid_actions: Vec::new(),
    }
}
