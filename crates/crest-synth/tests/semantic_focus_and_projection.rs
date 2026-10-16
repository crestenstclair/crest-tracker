//! Semantic focus and projection for effect slots and bus returns.
//!
//! Proves FR-002 (adjacent-choice occupancy, no modal — C-008), FR-003
//! (descriptor-driven slot parameters), FR-014 (observable outcome), FR-017
//! (deterministic focus survival), and C-003 (PATCH and MIXER remain the only
//! top-level contexts) against the production reducer and projection path.

#[allow(dead_code)]
mod support;

use crest_synth::adapter::braids_capability::BraidsCapability;
use crest_synth::adapter::production_effects::{
    production_chorus_config, production_default_bus_returns, production_effect_registry,
};
use crest_synth::adapter::production_instruments::production_capability_registry;
use crest_synth::control::app_event::{AppEvent, Direction};
use crest_synth::control::{
    AppState, EngineSelectionStatusKind, EventRejection, FocusPath, InteractionMode,
    MixerControlId, PatchControlId, PatchDetailSubject, SemanticAction, SemanticControlId,
    StateProjector, StructuralEditIntent, SurfaceId, TopLevelContext,
};
use crest_synth::kernel::midi_channel::MidiChannel;
use crest_synth::kernel::patch_id::PatchId;
use crest_synth::mixer::bus_id::BusId;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::shell::{KeyboardInputTranslator, WindowInput, WindowKey};
use crest_synth::synth::effect_slot_id::{EffectSlotIndex, MAX_EFFECT_SLOTS};
use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
use crest_synth::synth::{
    CapabilitySection, EffectCapabilityDescriptor, EffectCapabilityId, EffectCapabilityRegistry,
    EffectSlotId, Patch,
};
use crest_synth::testing::automatic_midi_test::create_soundfont_config;

fn soundfont_patch(id: u32, channel: u8, track: u8) -> Patch {
    let provider =
        crest_synth::adapter::production_instruments::production_soundfont_capability().unwrap();
    Patch::new(
        PatchId::new(id).unwrap(),
        format!("Focus {id}"),
        create_soundfont_config(&provider, SoundFontInstrument::new(128, 0, false).unwrap())
            .unwrap(),
        MidiChannel::new(channel).unwrap(),
        PatchOutput::to_track(MixerTrackId::new(track).unwrap()),
    )
}

/// Two installed Patches; the first occupies slot 0 with the production
/// chorus config, and returns 0/1 carry the default production occupancy.
fn installed_state() -> AppState {
    let effects = production_effect_registry().unwrap();
    let returns = production_default_bus_returns(&effects).unwrap();
    let mut state = AppState::new_with_effects(
        production_capability_registry().unwrap(),
        effects,
        support::globals(),
    )
    .with_initial_returns(returns);
    let first = soundfont_patch(1, 0, 2).with_effect_slot(
        EffectSlotIndex::ALL[0],
        production_chorus_config(EffectSlotId::new(1).unwrap()).unwrap(),
    );
    state
        .apply(AppEvent::InstallPatches(vec![
            first,
            soundfont_patch(2, 1, 3),
        ]))
        .unwrap();
    state
}

fn duplicate_effect_detail_state() -> AppState {
    let effects = production_effect_registry().unwrap();
    let returns = production_default_bus_returns(&effects).unwrap();
    let mut state = AppState::new_with_effects(
        production_capability_registry().unwrap(),
        effects,
        support::globals(),
    )
    .with_initial_returns(returns);
    let mut patch = soundfont_patch(1, 0, 2);
    for (offset, slot) in EffectSlotIndex::ALL.into_iter().enumerate() {
        patch = patch.with_effect_slot(
            slot,
            production_chorus_config(EffectSlotId::new(offset as u16 + 1).unwrap()).unwrap(),
        );
    }
    state.apply(AppEvent::InstallPatches(vec![patch])).unwrap();
    state
}

/// Drives one accepted occupancy request through TopologyPrepared and the
/// activation acknowledgement, exactly as the production orchestration does.
fn commit_pending_topology(state: &mut AppState) {
    let correlation = state.engine_selection().correlation().unwrap().clone();
    for lifecycle in [
        EngineSelectionStatusKind::Validating,
        EngineSelectionStatusKind::Preparing,
    ] {
        state
            .apply(AppEvent::EngineSelectionLifecycleAdvanced {
                request_id: correlation.request_id(),
                lifecycle,
            })
            .unwrap();
    }
    let source = correlation.source_graph_revision();
    let target = source.checked_next().unwrap();
    state
        .apply(AppEvent::TopologyPrepared {
            prepared_visualization: None,
            request_id: correlation.request_id(),
            intent: correlation.intent().clone(),
            source_graph_revision: source,
            target_graph_revision: target,
        })
        .unwrap();
    state
        .apply(AppEvent::EngineActivationAcknowledged {
            request_id: correlation.request_id(),
            intent: correlation.intent().clone(),
            target_graph_revision: target,
            retired_graph_revision: source,
            collected: true,
        })
        .unwrap();
}

fn set_mode(state: &mut AppState, mode: InteractionMode) {
    state
        .apply_semantic_action(SemanticAction::SetInteractionMode(mode))
        .unwrap();
}

fn focused_patch_control(state: &AppState) -> PatchControlId {
    state.interaction().patch_control_focus().unwrap()
}

/// Patch Main is the Overview order: Engine followed by one occupancy control
/// per canonical slot. Parameters remain reachable through Detail and cannot
/// survive as hidden Main targets.
#[test]
fn all_three_slot_rows_are_reachable_and_the_context_set_is_closed() {
    let mut state = installed_state();
    state
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    let controls = state.focused_patch_controls().unwrap();
    assert_eq!(
        controls,
        vec![
            PatchControlId::Engine,
            PatchControlId::EffectSlot(EffectSlotIndex::ALL[0]),
            PatchControlId::EffectSlot(EffectSlotIndex::ALL[1]),
            PatchControlId::EffectSlot(EffectSlotIndex::ALL[2]),
        ],
        "Patch Main exposes Overview controls only"
    );

    // Bare Up at the top and Down past the last row are rejected unchanged.
    let top = state.clone();
    assert_eq!(
        state.apply(AppEvent::Navigate(Direction::Up)),
        Err(EventRejection::ActionUnavailableInContext)
    );
    assert_eq!(state, top);
    for expected in controls.iter().skip(1) {
        state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
        assert_eq!(&focused_patch_control(&state), expected);
    }
    let bottom = state.clone();
    assert_eq!(
        state.apply(AppEvent::Navigate(Direction::Down)),
        Err(EventRejection::ActionUnavailableInContext)
    );
    assert_eq!(state, bottom);

    // C-003: two top-level contexts and the closed surface vocabulary —
    // four persistent surfaces, three PATCH subordinates, and two Settings
    // pages, which suspends rather than replaces PATCH/MIXER.
    assert_eq!(
        TopLevelContext::surface_descriptor(),
        &[TopLevelContext::Patch, TopLevelContext::Mixer]
    );
    assert_eq!(
        SurfaceId::surface_descriptor(),
        &[
            SurfaceId::Sends,
            SurfaceId::PatchMain,
            SurfaceId::PatchUtility,
            SurfaceId::PatchDetail,
            SurfaceId::PatchChoice,
            SurfaceId::FileBrowser,
            SurfaceId::MixerMain,
            SurfaceId::MixerInspector,
            SurfaceId::MidiDeviceSettings,
            SurfaceId::ControllerSettings,
            SurfaceId::SaveLoadSettings,
        ]
    );
    assert_eq!(
        SurfaceId::ALL
            .iter()
            .filter(|surface| surface.is_subordinate())
            .count(),
        3,
        "exactly the three declared PATCH subordinate surfaces exist"
    );
    assert!(state.interaction().focus_path().modal_id().is_none());
    assert_eq!(
        InteractionMode::PHASE_TWO,
        [InteractionMode::Navigate, InteractionMode::Adjust]
    );
}

#[test]
fn overview_roots_drive_choice_detail_return_and_patch_switch_workflows() {
    let mut state = installed_state();
    state
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    let engine_origin = state.interaction().focus_path().clone();

    set_mode(&mut state, InteractionMode::Adjust);
    state.apply(AppEvent::Adjust(Direction::Up)).unwrap();
    assert_eq!(state.interaction().active_surface(), SurfaceId::PatchChoice);
    state.apply_semantic_action(SemanticAction::Return).unwrap();
    assert_eq!(state.interaction().focus_path(), &engine_origin);

    set_mode(&mut state, InteractionMode::Navigate);
    state
        .apply(AppEvent::EnterSurface(SurfaceId::PatchDetail))
        .unwrap();
    assert_eq!(state.interaction().active_surface(), SurfaceId::PatchDetail);
    state.apply_semantic_action(SemanticAction::Return).unwrap();
    assert_eq!(state.interaction().focus_path(), &engine_origin);

    state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
    let slot_origin = state.interaction().focus_path().clone();
    set_mode(&mut state, InteractionMode::Adjust);
    state.apply(AppEvent::Adjust(Direction::Up)).unwrap();
    assert_eq!(state.interaction().active_surface(), SurfaceId::PatchChoice);
    state.apply_semantic_action(SemanticAction::Return).unwrap();
    assert_eq!(state.interaction().focus_path(), &slot_origin);

    set_mode(&mut state, InteractionMode::Navigate);
    state
        .apply(AppEvent::EnterSurface(SurfaceId::PatchDetail))
        .unwrap();
    assert_eq!(state.interaction().active_surface(), SurfaceId::PatchDetail);
    state.apply_semantic_action(SemanticAction::Return).unwrap();
    assert_eq!(state.interaction().focus_path(), &slot_origin);

    state
        .apply(AppEvent::SelectPatch(Direction::Right))
        .unwrap();
    assert_eq!(
        state.interaction().patch_focus(),
        Some(PatchId::new(2).unwrap())
    );
    assert_eq!(
        state.interaction().focus_path().control_id(),
        slot_origin.control_id(),
        "sibling Patch navigation preserves the canonical Overview identity"
    );
}

/// Instrument and FX Detail admission is owned by the production reducer.
/// Duplicate capabilities remain distinct by their slot instance and
/// canonical origin position; an empty position cannot manufacture a subject.
#[test]
fn detail_subjects_origins_and_empty_slot_admission_are_canonical() {
    let mut instrument = duplicate_effect_detail_state();
    instrument
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    let engine_origin = instrument.interaction().focus_path().clone();
    let capability = instrument.patches()[0]
        .instrument_config()
        .capability_id()
        .clone();
    instrument
        .apply(AppEvent::EnterSurface(SurfaceId::PatchDetail))
        .unwrap();
    assert_eq!(
        instrument.interaction().detail_subject(),
        Some(&PatchDetailSubject::instrument(capability))
    );
    assert_eq!(
        instrument.interaction().return_path().unwrap().origin(),
        &engine_origin
    );
    let instrument_model = StateProjector::new()
        .project_with_shell(&instrument)
        .unwrap()
        .3;
    let instrument_detail = instrument_model
        .semantic_model()
        .surface(SurfaceId::PatchDetail)
        .unwrap();
    assert_eq!(
        instrument_detail
            .controls()
            .iter()
            .filter(|control| control.focused())
            .count(),
        1
    );
    instrument.apply(AppEvent::Return).unwrap();
    assert_eq!(instrument.interaction().focus_path(), &engine_origin);

    let mut subjects = Vec::new();
    for (position, slot) in EffectSlotIndex::ALL.into_iter().enumerate() {
        let mut state = duplicate_effect_detail_state();
        state
            .apply(AppEvent::SelectContext(TopLevelContext::Patch))
            .unwrap();
        for _ in 0..=position {
            state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
        }
        let origin = state.interaction().focus_path().clone();
        assert_eq!(
            origin.control_id(),
            &SemanticControlId::Patch(PatchControlId::EffectSlot(slot))
        );
        let occupant = state.patches()[0].effect_slot(slot).unwrap();
        let occupant_slot_id = occupant.slot_id();
        let occupant_capability_id = occupant.capability_id().clone();
        let expected = PatchDetailSubject::effect(occupant_slot_id, occupant_capability_id);
        state
            .apply(AppEvent::EnterSurface(SurfaceId::PatchDetail))
            .unwrap();
        assert_eq!(state.interaction().detail_subject(), Some(&expected));
        assert_eq!(state.interaction().return_path().unwrap().origin(), &origin);
        let serialized = serde_json::to_value(
            StateProjector::new()
                .project_with_shell(&state)
                .unwrap()
                .3
                .semantic_model(),
        )
        .unwrap();
        let summary = serialized
            .get("surfaces")
            .and_then(serde_json::Value::as_array)
            .unwrap()
            .iter()
            .find(|surface| {
                surface.get("id").and_then(serde_json::Value::as_str) == Some("patchDetail")
            })
            .unwrap()
            .get("summary")
            .unwrap();
        assert_eq!(
            summary
                .pointer("/subject/kind")
                .and_then(serde_json::Value::as_str),
            Some("effect")
        );
        assert_eq!(
            summary
                .pointer("/subject/slotId")
                .and_then(serde_json::Value::as_u64),
            Some(u64::from(occupant_slot_id.value()))
        );
        assert_eq!(
            serialized
                .pointer("/returnPath/origin/controlId/id")
                .and_then(serde_json::Value::as_str),
            Some(format!("patch.effectSlot.{position}").as_str())
        );
        subjects.push(expected);
        state.apply(AppEvent::Return).unwrap();
        assert_eq!(state.interaction().focus_path(), &origin);
    }
    assert_eq!(subjects.len(), 3);
    assert!(subjects.windows(2).all(|pair| pair[0] != pair[1]));

    let mut empty = installed_state();
    empty
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    empty.apply(AppEvent::Navigate(Direction::Down)).unwrap();
    empty.apply(AppEvent::Navigate(Direction::Down)).unwrap();
    assert_eq!(
        empty.interaction().patch_control_focus(),
        Some(PatchControlId::EffectSlot(EffectSlotIndex::ALL[1]))
    );
    let empty_model = StateProjector::new().project_with_shell(&empty).unwrap().3;
    assert!(empty_model
        .semantic_model()
        .valid_actions()
        .iter()
        .all(|valid| valid.action() != &SemanticAction::EnterSurface(SurfaceId::PatchDetail)));
    let before = empty.clone();
    assert_eq!(
        empty.apply(AppEvent::EnterSurface(SurfaceId::PatchDetail)),
        Err(EventRejection::ActionUnavailableInContext)
    );
    assert_eq!(empty, before);
    assert!(empty.interaction().detail_subject().is_none());
}

/// The production keyboard adapter emits semantic page actions;
/// only `AppState::apply` changes the subordinate session and focus.
#[test]
fn physical_shift_entry_and_close_restore_every_detail_origin() {
    for target_offset in 0..=EffectSlotIndex::ALL.len() {
        let mut state = duplicate_effect_detail_state();
        state
            .apply(AppEvent::SelectContext(TopLevelContext::Patch))
            .unwrap();
        for _ in 0..target_offset {
            state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
        }
        let origin = state.interaction().focus_path().clone();
        let mut translator = KeyboardInputTranslator::new();
        assert_eq!(
            translator.translate(WindowInput::key_down(WindowKey::Shift)),
            None
        );
        let enter = translator
            .translate(WindowInput::key_down(WindowKey::W))
            .expect("Shift+Up maps to one semantic action");
        assert_eq!(enter, SemanticAction::NavigatePage(Direction::Up));
        state.apply_semantic_action(enter).unwrap();
        assert_eq!(
            translator.translate(WindowInput::key_up(WindowKey::W)),
            None
        );
        assert_eq!(state.interaction().active_surface(), SurfaceId::PatchDetail);
        assert_eq!(
            StateProjector::new()
                .project_with_shell(&state)
                .unwrap()
                .3
                .semantic_model()
                .surfaces()
                .iter()
                .flat_map(|surface| surface.controls())
                .filter(|control| control.focused())
                .count(),
            1
        );
        let close = translator
            .translate(WindowInput::key_down(WindowKey::S))
            .expect("Shift+Down maps to one semantic action");
        assert_eq!(close, SemanticAction::NavigatePage(Direction::Down));
        state.apply_semantic_action(close).unwrap();
        assert_eq!(
            translator.translate(WindowInput::key_up(WindowKey::S)),
            None
        );
        assert_eq!(state.interaction().active_surface(), SurfaceId::PatchMain);
        assert_eq!(state.interaction().focus_path(), &origin);
        assert_eq!(
            translator.translate(WindowInput::key_up(WindowKey::Shift)),
            None
        );
    }
}

/// Differently shaped production descriptors retain their authored section
/// and control order through the one canonical Detail projection. Instrument
/// Detail appends the canonical shared envelope once; effect Detail never does.
#[test]
fn differently_shaped_descriptors_share_one_ordered_detail_projection() {
    let soundfont = soundfont_patch(1, 0, 0);
    let braids = Patch::new(
        PatchId::new(2).unwrap(),
        "Braids Shape".to_owned(),
        BraidsCapability::new().unwrap().default_config().unwrap(),
        MidiChannel::new(1).unwrap(),
        PatchOutput::to_track(MixerTrackId::new(1).unwrap()),
    );
    let mut instruments = AppState::new_with_effects(
        production_capability_registry().unwrap(),
        production_effect_registry().unwrap(),
        support::globals(),
    );
    instruments
        .apply(AppEvent::InstallPatches(vec![soundfont, braids]))
        .unwrap();
    instruments
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();

    let mut instrument_shapes = Vec::new();
    for patch_offset in 0..2 {
        if patch_offset > 0 {
            instruments
                .apply(AppEvent::SelectPatch(Direction::Right))
                .unwrap();
        }
        let patch_id = instruments.interaction().patch_focus().unwrap();
        let descriptor = instruments
            .capabilities()
            .descriptor(
                instruments
                    .patches()
                    .iter()
                    .find(|patch| patch.id() == patch_id)
                    .unwrap()
                    .instrument_config()
                    .capability_id(),
            )
            .unwrap();
        let mut expected_section_ids = descriptor
            .sections()
            .iter()
            .map(|section| section.id().to_owned())
            .collect::<Vec<_>>();
        expected_section_ids.push("shared.envelope".to_owned());
        let overview_ids = StateProjector::new()
            .project_with_shell(&instruments)
            .unwrap()
            .3
            .semantic_model()
            .surface(SurfaceId::PatchMain)
            .unwrap()
            .controls()
            .iter()
            .map(|control| control.path().control_id().clone())
            .collect::<Vec<_>>();
        assert_eq!(
            overview_ids,
            vec![
                SemanticControlId::Patch(PatchControlId::Engine),
                SemanticControlId::Patch(PatchControlId::EffectSlot(EffectSlotIndex::ALL[0])),
                SemanticControlId::Patch(PatchControlId::EffectSlot(EffectSlotIndex::ALL[1])),
                SemanticControlId::Patch(PatchControlId::EffectSlot(EffectSlotIndex::ALL[2])),
            ],
            "descriptor rows never leak into Patch Overview membership"
        );
        instruments
            .apply(AppEvent::EnterSurface(SurfaceId::PatchDetail))
            .unwrap();
        let projection = StateProjector::new()
            .project_with_shell(&instruments)
            .unwrap()
            .3;
        let detail = projection
            .semantic_model()
            .surface(SurfaceId::PatchDetail)
            .unwrap();
        assert_eq!(
            detail
                .sections()
                .iter()
                .map(|section| section.id().to_owned())
                .collect::<Vec<_>>(),
            expected_section_ids
        );
        assert!(detail
            .controls()
            .iter()
            .all(|control| control.visible() && control.enabled()));
        assert_eq!(
            detail
                .sections()
                .last()
                .unwrap()
                .control_paths()
                .iter()
                .filter(|path| matches!(
                    path.control_id(),
                    SemanticControlId::Patch(PatchControlId::Envelope(_))
                ))
                .count(),
            crest_synth::synth::VoiceEnvelope::surface_descriptor().len()
        );
        instrument_shapes.push((detail.sections().len(), detail.controls().len()));
        instruments.apply(AppEvent::Return).unwrap();
    }
    assert_ne!(
        instrument_shapes[0], instrument_shapes[1],
        "the production instrument fixtures must exercise different shapes"
    );

    let production_effects = production_effect_registry().unwrap();
    let prototypes = production_effects.descriptors()[0]
        .parameters()
        .cloned()
        .collect::<Vec<_>>();
    assert!(prototypes.len() >= 2);
    let short = EffectCapabilityDescriptor::new(
        EffectCapabilityId::new("effect.fixture-short").unwrap(),
        "Fixture Short",
        "fixture.short",
        vec![CapabilitySection::new(
            "fixture.short.primary",
            "Primary",
            vec![prototypes[0].clone()],
        )
        .unwrap()],
        Vec::new(),
    )
    .unwrap();
    let long = EffectCapabilityDescriptor::new(
        EffectCapabilityId::new("effect.fixture-long").unwrap(),
        "Fixture Long",
        "fixture.long",
        vec![
            CapabilitySection::new(
                "fixture.long.primary",
                "Primary",
                vec![prototypes[0].clone()],
            )
            .unwrap(),
            CapabilitySection::new(
                "fixture.long.secondary",
                "Secondary",
                prototypes[1..].to_vec(),
            )
            .unwrap(),
        ],
        Vec::new(),
    )
    .unwrap();
    let effects = EffectCapabilityRegistry::new(vec![short, long]).unwrap();
    let mut patch = soundfont_patch(3, 2, 2);
    for (position, descriptor) in effects.descriptors().iter().enumerate() {
        patch = patch.with_effect_slot(
            EffectSlotIndex::ALL[position],
            descriptor
                .default_config(EffectSlotIndex::ALL[position].instance_identity())
                .unwrap(),
        );
    }
    let mut effect_state = AppState::new_with_effects(
        production_capability_registry().unwrap(),
        effects,
        support::globals(),
    );
    effect_state
        .apply(AppEvent::InstallPatches(vec![patch]))
        .unwrap();
    effect_state
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    let mut effect_shapes = Vec::new();
    for (position, slot) in EffectSlotIndex::ALL
        .into_iter()
        .take(effect_state.effects().descriptors().len())
        .enumerate()
    {
        effect_state
            .apply(AppEvent::Navigate(Direction::Down))
            .unwrap();
        let occupant = effect_state.patches()[0].effect_slot(slot).unwrap();
        let slot_id = occupant.slot_id();
        let descriptor = effect_state
            .effects()
            .descriptor(occupant.capability_id())
            .unwrap();
        let expected_section_ids = descriptor
            .sections()
            .iter()
            .map(|section| section.id().to_owned())
            .collect::<Vec<_>>();
        let expected_parameter_ids = descriptor
            .parameters()
            .map(|parameter| parameter.id().clone())
            .collect::<Vec<_>>();
        effect_state
            .apply(AppEvent::EnterSurface(SurfaceId::PatchDetail))
            .unwrap();
        let projection = StateProjector::new()
            .project_with_shell(&effect_state)
            .unwrap()
            .3;
        let detail = projection
            .semantic_model()
            .surface(SurfaceId::PatchDetail)
            .unwrap();
        assert_eq!(
            detail
                .sections()
                .iter()
                .map(|section| section.id().to_owned())
                .collect::<Vec<_>>(),
            expected_section_ids
        );
        assert_eq!(
            detail
                .controls()
                .iter()
                .map(|control| match control.path().control_id() {
                    SemanticControlId::Patch(PatchControlId::Effect(projected_slot, parameter)) => {
                        assert_eq!(*projected_slot, slot_id);
                        parameter.clone()
                    }
                    unexpected => panic!("unexpected FX Detail row {unexpected:?}"),
                })
                .collect::<Vec<_>>(),
            expected_parameter_ids
        );
        assert!(detail
            .controls()
            .iter()
            .all(|control| control.visible() && control.enabled()));
        effect_shapes.push((detail.sections().len(), detail.controls().len()));
        effect_state.apply(AppEvent::Return).unwrap();
        if position + 1 == effect_state.effects().descriptors().len() {
            break;
        }
    }
    assert!(
        effect_shapes.windows(2).any(|pair| pair[0] != pair[1]),
        "the effect fixtures must exercise different shapes"
    );
}

/// Occupancy rows use the engine row's adjacent nonwrapping choice
/// contract — empty, then each installed registry entry in declared order —
/// while Adjust+Up opens the shared descriptor-driven choice modal and
/// Adjust+Down remains unavailable. The adjacent cycle itself stays on
/// Adjust(Left/Right) in place.
#[test]
fn slot_occupancy_cycles_adjacent_choices_without_wrapping() {
    let mut state = installed_state();
    state
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    let controls = state.focused_patch_controls().unwrap();
    let empty_slot = controls
        .iter()
        .position(|control| control == &PatchControlId::EffectSlot(EffectSlotIndex::ALL[1]))
        .unwrap();
    for _ in 0..empty_slot {
        state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
    }
    set_mode(&mut state, InteractionMode::Adjust);

    // Up opens the shared Phase 7 choice modal and Return restores the exact
    // occupancy origin; Down has no structural meaning.
    let origin = state.interaction().focus_path().clone();
    state.apply(AppEvent::Adjust(Direction::Up)).unwrap();
    assert_eq!(state.interaction().active_surface(), SurfaceId::PatchChoice);
    state.apply_semantic_action(SemanticAction::Return).unwrap();
    assert_eq!(state.interaction().focus_path(), &origin);
    set_mode(&mut state, InteractionMode::Adjust);
    let before = state.clone();
    assert_eq!(
        state.apply(AppEvent::Adjust(Direction::Down)),
        Err(EventRejection::ActionUnavailableInContext)
    );
    // Empty is the first choice: Left at empty is the boundary, not a wrap.
    assert_eq!(
        state.apply(AppEvent::Adjust(Direction::Left)),
        Err(EventRejection::ParameterAtBoundary)
    );
    assert_eq!(state, before);

    // Right walks empty -> each installed entry in declared registry order.
    let entries = production_effect_registry()
        .unwrap()
        .descriptors()
        .iter()
        .map(|descriptor| descriptor.id().clone())
        .collect::<Vec<EffectCapabilityId>>();
    for expected_entry in &entries {
        state.apply(AppEvent::Adjust(Direction::Right)).unwrap();
        let correlation = state.engine_selection().correlation().unwrap();
        assert_eq!(
            correlation.intent(),
            &StructuralEditIntent::SetSlotOccupancy {
                patch_id: PatchId::new(1).unwrap(),
                slot: EffectSlotIndex::ALL[1],
                entry: Some(expected_entry.clone()),
            }
        );
        // One in flight application-wide: a second adjacent request is a
        // synchronous busy refusal, never a queue.
        assert_eq!(
            state.apply(AppEvent::Adjust(Direction::Right)),
            Err(EventRejection::StructuralEditBusy)
        );
        commit_pending_topology(&mut state);
        assert_eq!(
            state.patches()[0]
                .effect_slot(EffectSlotIndex::ALL[1])
                .map(|config| config.capability_id().clone()),
            Some(expected_entry.clone())
        );
        // Focus stayed on the occupancy row while its parameter rows
        // appeared beneath.
        assert_eq!(
            focused_patch_control(&state),
            PatchControlId::EffectSlot(EffectSlotIndex::ALL[1])
        );
    }
    // The last installed entry is the far endpoint: no wrap back to empty.
    let end = state.clone();
    assert_eq!(
        state.apply(AppEvent::Adjust(Direction::Right)),
        Err(EventRejection::ParameterAtBoundary)
    );
    assert_eq!(state, end);
}

/// The same contract on a bus return's occupancy row.
#[test]
fn return_occupancy_uses_the_same_adjacent_choice_contract() {
    let mut state = installed_state();
    // Reach the same stable return identity independent of configured bank size.
    state
        .apply_semantic_action(SemanticAction::EnterSurface(SurfaceId::MixerInspector))
        .unwrap();
    let target = FocusPath::mixer_return_occupancy(BusId::new(2).unwrap());
    for _ in 0..crest_synth::control::SemanticResolver::new(&state)
        .ordered_paths(SurfaceId::MixerInspector)
        .unwrap()
        .len()
    {
        if state.interaction().focus_path() == &target {
            break;
        }
        state
            .apply_semantic_action(SemanticAction::Navigate(Direction::Down))
            .unwrap();
    }
    assert_eq!(
        state.interaction().focus_path(),
        &FocusPath::mixer_return_occupancy(BusId::new(2).unwrap())
    );
    set_mode(&mut state, InteractionMode::Adjust);
    let before = state.clone();
    assert_eq!(
        state.apply(AppEvent::Adjust(Direction::Up)),
        Err(EventRejection::ActionUnavailableInContext)
    );
    assert_eq!(
        state.apply(AppEvent::Adjust(Direction::Left)),
        Err(EventRejection::ParameterAtBoundary)
    );
    assert_eq!(state, before);
    state.apply(AppEvent::Adjust(Direction::Right)).unwrap();
    let correlation = state.engine_selection().correlation().unwrap();
    let first_entry = production_effect_registry().unwrap().descriptors()[0]
        .id()
        .clone();
    assert_eq!(
        correlation.intent(),
        &StructuralEditIntent::SetReturnOccupancy {
            bus: BusId::new(2).unwrap(),
            entry: Some(first_entry.clone()),
        }
    );
    commit_pending_topology(&mut state);
    assert_eq!(
        state
            .bus_returns()
            .bus_return(BusId::new(2).unwrap())
            .effect()
            .map(|config| config.capability_id().clone()),
        Some(first_entry)
    );
    // Focus survived on the occupancy row; the occupant's rows now follow it.
    assert_eq!(
        state.interaction().focus_path(),
        &FocusPath::mixer_return_occupancy(BusId::new(2).unwrap())
    );
    state
        .apply_semantic_action(SemanticAction::SetInteractionMode(
            InteractionMode::Navigate,
        ))
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::Navigate(Direction::Down))
        .unwrap();
    assert_eq!(
        state.interaction().focus_path(),
        &FocusPath::mixer_return_level(BusId::new(2).unwrap())
    );
}

/// Clearing an occupied Overview slot preserves its positional occupancy
/// identity, and two identical runs land on the same focus path.
#[test]
fn clearing_a_focused_slot_recovers_focus_deterministically() {
    let run = || -> (FocusPath, AppState) {
        let mut state = installed_state();
        state
            .apply(AppEvent::SelectContext(TopLevelContext::Patch))
            .unwrap();
        let controls = state.focused_patch_controls().unwrap();
        let slot0 = controls
            .iter()
            .position(|control| control == &PatchControlId::EffectSlot(EffectSlotIndex::ALL[0]))
            .unwrap();
        for _ in 0..slot0 {
            state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
        }
        assert_eq!(
            focused_patch_control(&state),
            PatchControlId::EffectSlot(EffectSlotIndex::ALL[0])
        );
        state
            .apply_semantic_action(SemanticAction::SetSlotOccupancy {
                patch_id: PatchId::new(1).unwrap(),
                slot: EffectSlotIndex::ALL[0],
                entry: None,
            })
            .unwrap();
        commit_pending_topology(&mut state);
        (state.interaction().focus_path().clone(), state)
    };

    let (first_focus, state) = run();
    assert_eq!(
        first_focus.control_id(),
        &SemanticControlId::Patch(PatchControlId::EffectSlot(EffectSlotIndex::ALL[0]))
    );
    // The recovered path resolves against the new schema.
    assert!(state
        .focused_patch_controls()
        .unwrap()
        .contains(&PatchControlId::EffectSlot(EffectSlotIndex::ALL[0])));

    assert_eq!(run().0, first_focus);
}

/// Installing any effect registry entry keeps only its occupancy identity on
/// Overview and exposes its descriptor rows on Detail.
#[test]
fn slot_projection_is_descriptor_driven_for_every_registry_entry() {
    let registry = production_effect_registry().unwrap();
    for entry in registry.descriptors() {
        let mut state = installed_state();
        state
            .apply_semantic_action(SemanticAction::SetSlotOccupancy {
                patch_id: PatchId::new(2).unwrap(),
                slot: EffectSlotIndex::ALL[2],
                entry: Some(entry.id().clone()),
            })
            .unwrap();
        commit_pending_topology(&mut state);
        state
            .apply(AppEvent::SelectContext(TopLevelContext::Patch))
            .unwrap();
        state
            .apply(AppEvent::SelectPatch(Direction::Right))
            .unwrap();
        let occupant = state.patches()[1]
            .effect_slot(EffectSlotIndex::ALL[2])
            .unwrap();
        assert_eq!(occupant.capability_id(), entry.id());
        let descriptor = registry.descriptor(entry.id()).unwrap();
        let expected_rows = descriptor
            .parameters()
            .map(|spec| PatchControlId::Effect(occupant.slot_id(), spec.id().clone()))
            .collect::<Vec<_>>();
        for _ in 0..3 {
            state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
        }
        state
            .apply_semantic_action(SemanticAction::EnterSurface(SurfaceId::PatchDetail))
            .unwrap();
        let (_, _, _, shell, _) = StateProjector::new().project_with_shell(&state).unwrap();
        let detail_rows = shell
            .semantic_model()
            .surface(SurfaceId::PatchDetail)
            .unwrap()
            .controls()
            .iter()
            .filter_map(|control| match control.path().control_id() {
                SemanticControlId::Patch(control @ PatchControlId::Effect(_, _)) => {
                    Some(control.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(detail_rows, expected_rows);
    }
}

/// The MIXER text projection carries all eight indexed sends per track,
/// a distinct RETURNS section with every return's occupancy, values, and
/// return level, and a final GLOBAL section holding master gain alone.
#[test]
fn mixer_projection_carries_eight_sends_eight_returns_and_one_global() {
    let state = installed_state();
    let projector = StateProjector::new();
    let (_, _, text, shell, _) = projector.project_with_shell(&state).unwrap();
    let body = text.body();
    for track in MixerTrackId::ALL {
        assert!(body.contains(&format!("TRACK {track}")));
    }
    for bus in BusId::ALL {
        assert!(body.contains(&format!("RETURN {bus}")));
    }
    assert_eq!(body.matches("send[0]=").count(), MixerTrackId::COUNT);
    assert_eq!(body.matches("send[7]=").count(), MixerTrackId::COUNT);
    assert_eq!(body.matches("returnLevel=").count(), BusId::COUNT);
    assert_eq!(body.matches("occupancy=").count(), BusId::COUNT);
    assert_eq!(body.matches("occupancy=empty").count(), BusId::COUNT - 2);
    let global_section = body.split("GLOBAL").nth(1).unwrap();
    assert!(global_section.contains("masterGainDb="));
    assert!(!global_section.contains("reverb"));
    assert!(!global_section.contains("delay"));

    // The semantic inspector projects the occupied returns' descriptor rows.
    let semantic = shell.semantic_model();
    let inspector = semantic.surface(SurfaceId::MixerInspector).unwrap();
    let return_effect_rows = inspector
        .controls()
        .iter()
        .filter(|control| {
            matches!(
                control.path().control_id(),
                SemanticControlId::Mixer(MixerControlId::ReturnEffect { .. })
            )
        })
        .count();
    // Production defaults: reverb and delay each declare two scalar rows.
    assert_eq!(return_effect_rows, 4);
    let occupancy_rows = inspector
        .controls()
        .iter()
        .filter(|control| {
            matches!(
                control.path().control_id(),
                SemanticControlId::Mixer(MixerControlId::ReturnOccupancy { .. })
            )
        })
        .count();
    assert_eq!(occupancy_rows, BusId::COUNT);
    assert_eq!(MAX_EFFECT_SLOTS, 3);
}
