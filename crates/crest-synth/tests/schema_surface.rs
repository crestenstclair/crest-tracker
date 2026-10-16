mod support;

use crest_synth::adapter::braids_capability::BraidsCapability;
use crest_synth::adapter::production_effects::{
    production_chorus_config, production_effect_registry,
};
use crest_synth::adapter::production_instruments::{
    production_capability_registry, production_instrument_providers,
};
use crest_synth::adapter::sample_capability::SampleCapability;
use crest_synth::control::{
    AppEvent, AppState, EngineSelectionFailure, EngineSelectionStatusKind,
    GraphicalShellProjection, MidiDeviceEffect, MidiDeviceFailure, MidiInputDescriptor,
    MidiInputDeviceId, MidiInputPortFacts, MidiInputTransport, PatchControlId, PatchPageProjection,
    SemanticAction, SemanticControlId, SendAction, SendControlId, StateProjector, StateTree,
    SurfaceId, TopLevelContext,
};
use crest_synth::kernel::midi_channel::MidiChannel;
use crest_synth::kernel::patch_id::PatchId;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::GraphRevision;
use crest_synth::synth::effect_slot_id::EffectSlotIndex;
use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
use crest_synth::synth::{
    AssetAssignment, AssetFileId, CapabilityAvailability, CapabilityId, CapabilityRegistry,
    DescriptorDefaultConfigFactory, EffectCapabilityDescriptor, EffectCapabilityId,
    EffectCapabilityRegistry, EffectSlotId, FileBrowserFolderId, FileBrowserListing,
    FileBrowserRow, FileBrowserRowKind, InstrumentCapabilityProvider, InstrumentConfig, Patch,
    PreparedSampleLandmarks, PreparedSamplePcm, PreparedSampleVisualization, SampleEncoding,
    SampleLoopMode, SampleMetadata, VoiceEnvelope, WaveformPair,
};
use crest_synth::testing::automatic_midi_test::create_soundfont_config;
use crest_synth::testing::{
    BehavioralMutationCase, BehavioralMutationHarness, BehavioralMutationObservation,
    DemoCoverageGroup,
};
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;

fn configured_state(first: InstrumentConfig, second: InstrumentConfig) -> AppState {
    let capabilities = production_capability_registry().unwrap();
    let chorus = production_chorus_config(EffectSlotId::new(1).unwrap()).unwrap();
    let production_effects = production_effect_registry().unwrap();
    let soundfont_id =
        CapabilityId::new(crest_synth::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID)
            .unwrap();
    let soundfont_descriptor = capabilities.descriptor(&soundfont_id).unwrap();
    let schema_effect = EffectCapabilityDescriptor::new(
        EffectCapabilityId::new("effect.schema-fixture").unwrap(),
        "Schema Fixture",
        "effect.schema-fixture",
        soundfont_descriptor.sections().to_vec(),
        soundfont_descriptor.asset_requirements().to_vec(),
    )
    .unwrap();
    let soundfont_provider =
        crest_synth::adapter::production_instruments::production_soundfont_capability().unwrap();
    let schema_source_config = create_soundfont_config(
        &soundfont_provider,
        SoundFontInstrument::new(128, 11, false).unwrap(),
    )
    .unwrap();
    let schema_effect_config = schema_effect
        .create_config(
            EffectSlotId::new(2).unwrap(),
            schema_source_config.values(),
            schema_source_config.asset_references(),
        )
        .unwrap();
    let effects = EffectCapabilityRegistry::new(vec![
        production_effects.descriptors()[0].clone(),
        production_effects
            .descriptor(&EffectCapabilityId::new("effect.mutable.ringsresonator").unwrap())
            .unwrap()
            .clone(),
        production_effects
            .descriptor(&EffectCapabilityId::new("effect.airwindows.biquad2").unwrap())
            .unwrap()
            .clone(),
        production_effects
            .descriptor(&EffectCapabilityId::new("effect.fft.convolver").unwrap())
            .unwrap()
            .clone(),
        schema_effect,
    ])
    .unwrap();
    let patches = [first, second]
        .into_iter()
        .enumerate()
        .map(|(index, config)| {
            let patch = Patch::new(
                PatchId::new(index as u32 + 1).unwrap(),
                format!("Schema {index}"),
                config,
                MidiChannel::new(index as u8).unwrap(),
                PatchOutput::new(MixerTrackId::new(index as u8).unwrap(), -3.0 - index as f32)
                    .unwrap(),
            )
            .with_envelope(VoiceEnvelope::new(12.0, 34.0, 0.56, 78.0).unwrap());
            if index == 0 {
                patch.with_effect_slot(EffectSlotIndex::ALL[0], chorus.clone())
            } else {
                patch.with_effect_slot(EffectSlotIndex::ALL[0], schema_effect_config.clone())
            }
        })
        .collect();
    // Occupy two returns so the canonical serialized `returns` section and
    // the live `parameters.returns` entries expose their complete leaf shape:
    // a scalar-bearing occupant on bus 0 and an asset-bearing one on bus 1.
    let mut returns = crest_synth::mixer::bus_return::BusReturnBank::default();
    returns
        .set_return_occupancy(
            &effects,
            crest_synth::mixer::bus_id::BusId::new(0).unwrap(),
            Some(&EffectCapabilityId::new("effect.chorus").unwrap()),
        )
        .unwrap();
    let first_bus = crest_synth::mixer::bus_id::BusId::new(0).unwrap();
    let next_slot = returns.bus_return(first_bus).next_slot_id().unwrap();
    returns
        .set_effect_slot(
            &effects,
            first_bus,
            next_slot,
            Some(&EffectCapabilityId::new("effect.chorus").unwrap()),
        )
        .unwrap();
    returns.set_name(first_bus, "Schema chain").unwrap();
    returns
        .set_return_occupancy(
            &effects,
            crest_synth::mixer::bus_id::BusId::new(1).unwrap(),
            Some(&EffectCapabilityId::new("effect.schema-fixture").unwrap()),
        )
        .unwrap();
    returns
        .set_return_occupancy(
            &effects,
            crest_synth::mixer::bus_id::BusId::new(2).unwrap(),
            Some(&EffectCapabilityId::new("effect.fft.convolver").unwrap()),
        )
        .unwrap();
    let mut state = AppState::new_with_effects(capabilities, effects, support::globals())
        .with_initial_returns(returns);
    state.apply(AppEvent::InstallPatches(patches)).unwrap();
    state
}

fn state_tree_with_first_config(
    first: InstrumentConfig,
    second: InstrumentConfig,
    context: TopLevelContext,
    request_engine: bool,
    request_preset: bool,
) -> StateTree {
    let mut state = configured_state(first, second);
    if context == TopLevelContext::Patch {
        state.apply(AppEvent::SelectContext(context)).unwrap();
    }
    assert!(!(request_engine && request_preset));
    if request_engine {
        state
            .apply(AppEvent::Adjust(crest_synth::control::Direction::Right))
            .unwrap();
    } else if request_preset {
        state
            .apply(AppEvent::EnterSurface(SurfaceId::PatchDetail))
            .unwrap();
        navigate_down_until(&mut state, |path| {
            matches!(
                path.control_id(),
                crest_synth::control::SemanticControlId::Patch(PatchControlId::Capability(_))
            )
        });
        state
            .apply(AppEvent::Adjust(crest_synth::control::Direction::Right))
            .unwrap();
    }
    StateProjector::new().project_with_tree(&state).unwrap().4
}

fn state_tree_after(
    first: InstrumentConfig,
    second: InstrumentConfig,
    mutate: impl FnOnce(&mut AppState),
) -> StateTree {
    let mut state = configured_state(first, second);
    mutate(&mut state);
    StateProjector::new().project_with_tree(&state).unwrap().4
}

fn navigate_down_until(
    state: &mut AppState,
    predicate: impl Fn(&crest_synth::control::FocusPath) -> bool,
) {
    for _ in 0..64 {
        if predicate(state.interaction().focus_path()) {
            return;
        }
        state
            .apply(AppEvent::Navigate(crest_synth::control::Direction::Down))
            .unwrap();
    }
    panic!("semantic schema fixture could not reach its requested stable path");
}

fn discover_leaves(value: &Value, prefix: &str, output: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            for (name, child) in object {
                let path = if prefix.is_empty() {
                    name.to_owned()
                } else {
                    format!("{prefix}.{name}")
                };
                discover_leaves(child, &path, output);
            }
        }
        Value::Array(array) => {
            for child in array {
                discover_leaves(child, &format!("{prefix}[]"), output);
            }
        }
        _ => {
            output.insert(prefix.to_owned());
        }
    }
}

fn assert_state_tree_leaf_surface_exact() -> BTreeSet<String> {
    let soundfont =
        crest_synth::adapter::production_instruments::production_soundfont_capability().unwrap();
    let soundfont_config = create_soundfont_config(
        &soundfont,
        SoundFontInstrument::new(128, 11, false).unwrap(),
    )
    .unwrap();
    let braids_config = BraidsCapability::new().unwrap().default_config().unwrap();
    let mut trees = vec![
        state_tree_with_first_config(
            soundfont_config.clone(),
            braids_config.clone(),
            TopLevelContext::Mixer,
            false,
            false,
        ),
        state_tree_with_first_config(
            soundfont_config.clone(),
            braids_config.clone(),
            TopLevelContext::Patch,
            false,
            false,
        ),
        state_tree_with_first_config(
            soundfont_config.clone(),
            braids_config.clone(),
            TopLevelContext::Patch,
            true,
            false,
        ),
        state_tree_with_first_config(
            soundfont_config.clone(),
            braids_config.clone(),
            TopLevelContext::Patch,
            false,
            true,
        ),
        state_tree_with_first_config(
            braids_config.clone(),
            soundfont_config.clone(),
            TopLevelContext::Patch,
            false,
            false,
        ),
    ];
    // Observe send interaction through production reducer transitions, including
    // retained parameter focus and the effect picker's precise return identity.
    let mut send_state = configured_state(soundfont_config.clone(), braids_config.clone());
    send_state
        .apply_semantic_action(SemanticAction::Send(SendAction::Open))
        .unwrap();
    trees.push(
        StateProjector::new()
            .project_with_tree(&send_state)
            .unwrap()
            .4,
    );
    send_state.apply(AppEvent::Activate).unwrap();
    assert!(send_state.interaction().send_name_editing());
    trees.push(
        StateProjector::new()
            .project_with_tree(&send_state)
            .unwrap()
            .4,
    );
    send_state
        .apply_semantic_action(SemanticAction::Send(SendAction::CancelRename))
        .unwrap();
    navigate_down_until(&mut send_state, |path| {
        matches!(
            path.control_id(),
            SemanticControlId::Send(SendControlId::EffectParameter { .. })
        )
    });
    trees.push(
        StateProjector::new()
            .project_with_tree(&send_state)
            .unwrap()
            .4,
    );
    navigate_down_until(&mut send_state, |path| {
        matches!(
            path.control_id(),
            SemanticControlId::Send(SendControlId::EffectSlot { .. })
        )
    });
    send_state.apply(AppEvent::Activate).unwrap();
    assert!(send_state.interaction().send_choice_origin().is_some());
    trees.push(
        StateProjector::new()
            .project_with_tree(&send_state)
            .unwrap()
            .4,
    );
    send_state.apply(AppEvent::Return).unwrap();
    let bus = send_state.interaction().selected_send();
    let slot_id = send_state
        .bus_returns()
        .bus_return(bus)
        .next_slot_id()
        .unwrap();
    send_state
        .apply_semantic_action(SemanticAction::Send(SendAction::SetEffect {
            bus,
            slot_id,
            entry: Some(EffectCapabilityId::new("effect.chorus").unwrap()),
        }))
        .unwrap();
    trees.push(
        StateProjector::new()
            .project_with_tree(&send_state)
            .unwrap()
            .4,
    );

    // The asset-bearing send opens the shared browser, so its exact effect-slot
    // identity is observed in the stable return path as well as active focus.
    let mut send_asset_state = configured_state(soundfont_config.clone(), braids_config.clone());
    send_asset_state
        .apply_semantic_action(SemanticAction::Send(SendAction::Open))
        .unwrap();
    send_asset_state
        .apply(AppEvent::SelectPatch(
            crest_synth::control::Direction::Right,
        ))
        .unwrap();
    send_asset_state
        .apply(AppEvent::SelectPatch(
            crest_synth::control::Direction::Right,
        ))
        .unwrap();
    let asset_parameter = send_asset_state
        .effects()
        .descriptor(&EffectCapabilityId::new("effect.fft.convolver").unwrap())
        .unwrap()
        .parameters()
        .find(|spec| spec.kind() == crest_synth::synth::ParameterKind::Asset)
        .unwrap()
        .id()
        .clone();
    navigate_down_until(&mut send_asset_state, |path| {
        matches!(path.control_id(),
            SemanticControlId::Send(SendControlId::EffectParameter { parameter, .. })
                if *parameter == asset_parameter)
    });
    send_asset_state.apply(AppEvent::Activate).unwrap();
    trees.push(
        StateProjector::new()
            .project_with_tree(&send_asset_state)
            .unwrap()
            .4,
    );
    let mut controller_state = AppState::new(
        production_capability_registry().unwrap(),
        support::globals(),
    );
    controller_state
        .apply(AppEvent::Controller(
            crest_synth::control::ControllerEvent::DevicesChanged {
                devices: vec![crest_synth::control::ControllerDevice {
                    id: 1,
                    name: "Schema gamepad".to_owned(),
                }],
            },
        ))
        .unwrap();
    controller_state
        .apply(AppEvent::Controller(
            crest_synth::control::ControllerEvent::PreferencesLoaded {
                result: Err(crest_synth::control::ControllerFailure::PreferenceRead),
            },
        ))
        .unwrap();
    trees.push(
        StateProjector::new()
            .project_with_tree(&controller_state)
            .unwrap()
            .4,
    );
    let creation_registry = production_capability_registry().unwrap();
    trees.push(state_tree_after(
        soundfont_config.clone(),
        braids_config.clone(),
        |state| {
            state
                .apply(AppEvent::SelectContext(TopLevelContext::Patch))
                .unwrap();
            state
                .apply(AppEvent::SetInteractionMode(
                    crest_synth::control::InteractionMode::Adjust,
                ))
                .unwrap();
            state
                .apply(AppEvent::Adjust(crest_synth::control::Direction::Up))
                .unwrap();
        },
    ));
    let creation_blueprint = crest_synth::control::PatchCreationBlueprint::resolve(
        &CapabilityId::new(crest_synth::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID)
            .unwrap(),
        &DescriptorDefaultConfigFactory::new(
            creation_registry,
            production_instrument_providers().unwrap(),
        ),
    )
    .unwrap();
    let mut empty_state = configured_state(soundfont_config.clone(), braids_config.clone())
        .with_patch_creation_blueprint(creation_blueprint);
    empty_state
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    empty_state
        .apply(AppEvent::SelectPatch(
            crest_synth::control::Direction::Right,
        ))
        .unwrap();
    empty_state
        .apply(AppEvent::SelectPatch(
            crest_synth::control::Direction::Right,
        ))
        .unwrap();
    trees.push(
        StateProjector::new()
            .project_with_tree(&empty_state)
            .unwrap()
            .4,
    );
    let unavailable = CapabilityAvailability::Unavailable {
        reason: "schema fixture unavailable".to_owned(),
    };
    let mut capability_descriptors = production_capability_registry()
        .unwrap()
        .descriptors()
        .to_vec();
    capability_descriptors[0] = capability_descriptors[0]
        .clone()
        .with_availability(unavailable.clone());
    let mut effect_descriptors = production_effect_registry().unwrap().descriptors().to_vec();
    effect_descriptors[0] = effect_descriptors[0].clone().with_availability(unavailable);
    let mut unavailable_state = AppState::new_with_effects(
        CapabilityRegistry::new(capability_descriptors).unwrap(),
        EffectCapabilityRegistry::new(effect_descriptors).unwrap(),
        support::globals(),
    );
    unavailable_state
        .apply(AppEvent::InstallPatches(vec![Patch::new(
            PatchId::new(91).unwrap(),
            "Unavailable schema fixture".to_owned(),
            soundfont_config.clone(),
            MidiChannel::new(9).unwrap(),
            PatchOutput::to_track(MixerTrackId::new(9).unwrap()),
        )
        .with_effect_slot(
            EffectSlotIndex::ALL[0],
            production_chorus_config(EffectSlotId::new(1).unwrap()).unwrap(),
        )]))
        .unwrap();
    trees.push(
        StateProjector::new()
            .project_with_tree(&unavailable_state)
            .unwrap()
            .4,
    );
    // Occupancy correlations expose the position-bearing intent leaves
    // (patchId/slot/bus/entry) of the shared structural lifecycle.
    trees.push(state_tree_after(
        soundfont_config.clone(),
        braids_config.clone(),
        |state| {
            state
                .apply(AppEvent::SetReturnOccupancy {
                    bus: crest_synth::mixer::bus_id::BusId::new(4).unwrap(),
                    entry: Some(EffectCapabilityId::new("effect.chorus").unwrap()),
                })
                .unwrap();
        },
    ));
    trees.push(state_tree_after(
        soundfont_config.clone(),
        braids_config.clone(),
        |state| {
            state
                .apply(AppEvent::SetSlotOccupancy {
                    patch_id: PatchId::new(1).unwrap(),
                    slot: crest_synth::synth::effect_slot_id::EffectSlotIndex::new(2).unwrap(),
                    entry: Some(EffectCapabilityId::new("effect.chorus").unwrap()),
                })
                .unwrap();
        },
    ));
    trees.push(state_tree_after(
        soundfont_config.clone(),
        braids_config.clone(),
        |state| {
            state
                .apply(AppEvent::Navigate(crest_synth::control::Direction::Right))
                .unwrap();
            state
                .apply(AppEvent::Navigate(crest_synth::control::Direction::Right))
                .unwrap();
        },
    ));
    trees.push(state_tree_after(
        soundfont_config.clone(),
        braids_config.clone(),
        |state| {
            state
                .apply(AppEvent::SelectContext(TopLevelContext::Patch))
                .unwrap();
            state
                .apply(AppEvent::EnterSurface(SurfaceId::PatchUtility))
                .unwrap();
        },
    ));
    trees.push(state_tree_after(
        soundfont_config.clone(),
        braids_config.clone(),
        |state| {
            state
                .apply(AppEvent::SelectContext(TopLevelContext::Patch))
                .unwrap();
            navigate_down_until(state, |path| {
                matches!(
                    path.control_id(),
                    crest_synth::control::SemanticControlId::Patch(PatchControlId::EffectSlot(_))
                )
            });
            state
                .apply(AppEvent::EnterSurface(SurfaceId::PatchUtility))
                .unwrap();
        },
    ));
    // Both detail subject variants, because they do not carry the same leaves:
    // an `Instrument` subject names a capability alone, while an `Effect`
    // subject also names its exact occupied slot. A fixture that opens only one
    // of them cannot see the other's leaf, which is how the subject leaves went
    // undeclared in the first place. These drive the same reducer event the
    // semantic action vocabulary exposes, so the subject leaves are observed
    // through the production transition.
    trees.push(state_tree_after(
        soundfont_config.clone(),
        braids_config.clone(),
        |state| {
            state
                .apply(AppEvent::SelectContext(TopLevelContext::Patch))
                .unwrap();
            state
                .apply(AppEvent::EnterSurface(SurfaceId::PatchDetail))
                .unwrap();
            assert!(state.interaction().detail_subject().is_some());
        },
    ));
    trees.push(state_tree_after(
        soundfont_config.clone(),
        braids_config.clone(),
        |state| {
            state
                .apply(AppEvent::SelectContext(TopLevelContext::Patch))
                .unwrap();
            navigate_down_until(state, |path| {
                matches!(
                    path.control_id(),
                    crest_synth::control::SemanticControlId::Patch(PatchControlId::EffectSlot(_))
                )
            });
            state
                .apply(AppEvent::EnterSurface(SurfaceId::PatchDetail))
                .unwrap();
            assert_eq!(
                state
                    .interaction()
                    .detail_subject()
                    .and_then(crest_synth::control::PatchDetailSubject::slot_id),
                Some(EffectSlotId::new(1).unwrap()),
                "the effect subject must name the occupied slot's exact identity"
            );
        },
    ));
    trees.push(state_tree_after(
        soundfont_config.clone(),
        braids_config.clone(),
        |state| {
            state
                .apply(AppEvent::Navigate(crest_synth::control::Direction::Right))
                .unwrap();
            state
                .apply(AppEvent::Navigate(crest_synth::control::Direction::Right))
                .unwrap();
            state
                .apply(AppEvent::EnterSurface(SurfaceId::MixerInspector))
                .unwrap();
        },
    ));
    trees.push(state_tree_after(
        soundfont_config.clone(),
        braids_config.clone(),
        |state| {
            state
                .apply(AppEvent::EnterSurface(SurfaceId::MixerInspector))
                .unwrap();
            state
                .apply(AppEvent::Navigate(crest_synth::control::Direction::Down))
                .unwrap();
            state
                .apply(AppEvent::Navigate(crest_synth::control::Direction::Down))
                .unwrap();
        },
    ));
    trees.push(state_tree_after(
        soundfont_config.clone(),
        braids_config.clone(),
        |state| {
            state
                .apply(AppEvent::SelectContext(TopLevelContext::Patch))
                .unwrap();
            state
                .apply(AppEvent::Adjust(crest_synth::control::Direction::Right))
                .unwrap();
            let correlation = state.engine_selection().correlation().unwrap().clone();
            state
                .apply(AppEvent::EnginePreparationFailed {
                    request_id: correlation.request_id(),
                    patch_id: correlation.patch_id().unwrap(),
                    intent: correlation.intent().clone(),
                    source_capability_id: correlation.source_capability_id().unwrap().clone(),
                    target_capability_id: correlation.target_capability_id().unwrap().clone(),
                    source_graph_revision: correlation.source_graph_revision(),
                    target_graph_revision: GraphRevision::new(2).unwrap(),
                    failure: EngineSelectionFailure::AssetUnavailable,
                })
                .unwrap();
        },
    ));
    trees.push(state_tree_after(soundfont_config, braids_config, |state| {
        state
            .apply(AppEvent::SelectContext(TopLevelContext::Patch))
            .unwrap();
        state
            .apply(AppEvent::EnterSurface(SurfaceId::PatchDetail))
            .unwrap();
        state
            .apply(AppEvent::SetPatchOverviewOriginEnabled {
                patch_id: PatchId::new(1).unwrap(),
                control: PatchControlId::Engine,
                enabled: false,
            })
            .unwrap();
    }));
    trees.push(
        StateProjector::new()
            .project_with_tree(&AppState::new(
                production_capability_registry().unwrap(),
                support::globals(),
            ))
            .unwrap()
            .4,
    );
    // The Sample Browser is a transient subordinate union variant and is not
    // reachable from the production registry when no library root is
    // configured. Build it through the same reducer/projector boundary with
    // the real capability so its active asset and correlated preview leaves
    // remain part of the bidirectional schema witness in every environment.
    let sample = SampleCapability::new(AssetFileId::new("Factory.wav").unwrap()).unwrap();
    let sample_patch = Patch::new(
        PatchId::new(1).unwrap(),
        "Schema Sample".to_owned(),
        sample.default_config().unwrap(),
        MidiChannel::new(0).unwrap(),
        PatchOutput::default(),
    );
    let folder = FileBrowserFolderId::default();
    let listing = FileBrowserListing::new(
        folder.clone(),
        vec![FileBrowserRow::new(
            "file:Preview.wav",
            "Preview.wav",
            FileBrowserRowKind::File(AssetFileId::new("Preview.wav").unwrap()),
            Some(128),
        )
        .unwrap()
        .with_metadata(Ok(SampleMetadata::new(
            AssetFileId::new("Preview.wav").unwrap(),
            128,
            48_000,
            1,
            32,
            SampleEncoding::Float,
            48_000,
        )
        .unwrap()))
        .unwrap()],
    )
    .unwrap();
    let mut sample_state = AppState::for_graph(
        CapabilityRegistry::new(vec![sample.descriptor()]).unwrap(),
        support::globals(),
        GraphRevision::INITIAL,
    )
    .with_sample_catalog([(folder, Ok(listing))]);
    sample_state
        .apply(AppEvent::InstallPatches(vec![sample_patch]))
        .unwrap();
    sample_state
        .apply_semantic_action(SemanticAction::SelectContext(TopLevelContext::Patch))
        .unwrap();
    sample_state
        .apply_semantic_action(SemanticAction::OpenRelated)
        .unwrap();
    trees.push(
        StateProjector::new()
            .project_with_tree(&sample_state)
            .unwrap()
            .4,
    );
    sample_state
        .apply_semantic_action(SemanticAction::OpenRelated)
        .unwrap();
    sample_state
        .apply_semantic_action(SemanticAction::PreviewStart)
        .unwrap();
    trees.push(
        StateProjector::new()
            .project_with_tree(&sample_state)
            .unwrap()
            .4,
    );
    let preview_correlation = sample_state
        .engine_selection()
        .correlation()
        .unwrap()
        .clone();
    let mut failed_preview_state = sample_state.clone();
    failed_preview_state
        .apply(AppEvent::EnginePreparationFailed {
            request_id: preview_correlation.request_id(),
            patch_id: preview_correlation.patch_id().unwrap(),
            intent: preview_correlation.intent().clone(),
            source_capability_id: preview_correlation.source_capability_id().unwrap().clone(),
            target_capability_id: preview_correlation.target_capability_id().unwrap().clone(),
            source_graph_revision: preview_correlation.source_graph_revision(),
            target_graph_revision: GraphRevision::INITIAL.checked_next().unwrap(),
            failure: EngineSelectionFailure::UnsupportedAssetFormat,
        })
        .unwrap();
    trees.push(
        StateProjector::new()
            .project_with_tree(&failed_preview_state)
            .unwrap()
            .4,
    );
    let preview_reference = match preview_correlation.intent() {
        crest_synth::control::StructuralEditIntent::PrepareAudition { reference, .. } => {
            reference.clone()
        }
        _ => panic!("preview fixture must carry an audition intent"),
    };
    let source_config = sample_state.patches()[0].instrument_config();
    let candidate_assets = source_config
        .asset_references()
        .iter()
        .map(|assignment| {
            AssetAssignment::new(
                assignment.parameter_id().clone(),
                if assignment.parameter_id().as_str()
                    == crest_synth::adapter::sample_capability::SAMPLE_ASSET_PARAMETER_ID
                {
                    preview_reference.clone()
                } else {
                    assignment.reference().clone()
                },
            )
        })
        .collect::<Vec<_>>();
    let candidate_config = sample
        .descriptor()
        .create_config(source_config.values(), &candidate_assets)
        .unwrap();
    let preview_pcm = PreparedSamplePcm::new(
        AssetFileId::new("Preview.wav").unwrap(),
        48_000,
        1,
        Arc::from([0.0_f32, 0.5, -0.5, 0.0]),
        Arc::from([WaveformPair {
            left_min: -0.5,
            left_max: 0.5,
            right_min: -0.5,
            right_max: 0.5,
        }]),
    )
    .unwrap();
    let prepared_visualization = PreparedSampleVisualization::new(
        &preview_pcm,
        PreparedSampleLandmarks {
            start: 0,
            end: 4,
            loop_start: 0,
            loop_end: 4,
            crossfade_frames: 0,
            loop_mode: SampleLoopMode::Off,
        },
    );
    for lifecycle in [
        EngineSelectionStatusKind::Validating,
        EngineSelectionStatusKind::Preparing,
    ] {
        sample_state
            .apply(AppEvent::EngineSelectionLifecycleAdvanced {
                request_id: preview_correlation.request_id(),
                lifecycle,
            })
            .unwrap();
    }
    sample_state
        .apply(AppEvent::EnginePrepared {
            request_id: preview_correlation.request_id(),
            patch_id: preview_correlation.patch_id().unwrap(),
            intent: preview_correlation.intent().clone(),
            source_capability_id: preview_correlation.source_capability_id().unwrap().clone(),
            target_capability_id: preview_correlation.target_capability_id().unwrap().clone(),
            source_graph_revision: preview_correlation.source_graph_revision(),
            target_graph_revision: GraphRevision::INITIAL.checked_next().unwrap(),
            candidate_config,
            prepared_visualization: Some(vec![prepared_visualization]),
        })
        .unwrap();
    trees.push(
        StateProjector::new()
            .project_with_tree(&sample_state)
            .unwrap()
            .4,
    );
    let mut midi_state = AppState::new(
        production_capability_registry().unwrap(),
        support::globals(),
    );
    let mut controller_state = midi_state.clone();
    controller_state
        .apply(AppEvent::Controller(
            crest_synth::control::ControllerEvent::DevicesChanged {
                devices: vec![crest_synth::control::ControllerDevice {
                    id: 17,
                    name: "Schema gamepad".into(),
                }],
            },
        ))
        .unwrap();
    controller_state
        .apply(AppEvent::Controller(
            crest_synth::control::ControllerEvent::PreferencesLoaded {
                result: Err(crest_synth::control::ControllerFailure::PreferenceDecode),
            },
        ))
        .unwrap();
    trees.push(
        StateProjector::new()
            .project_with_tree(&controller_state)
            .unwrap()
            .4,
    );
    let opened = midi_state
        .apply_semantic_action(SemanticAction::OpenMidiSettings)
        .unwrap();
    let scan_id = opened
        .midi_device_effects()
        .iter()
        .find_map(|effect| match effect {
            MidiDeviceEffect::Scan { scan_id } => Some(*scan_id),
            _ => None,
        })
        .unwrap();
    let midi_identity = MidiInputDeviceId::new("midir-v1", "schema-device").unwrap();
    let midi_descriptor = MidiInputDescriptor::new(
        midi_identity.clone(),
        "Schema Controller",
        Some(
            MidiInputPortFacts::new(
                Some(MidiInputTransport::Usb),
                Some("Crest".to_owned()),
                Some("Schema Keys".to_owned()),
            )
            .unwrap(),
        ),
    )
    .unwrap();
    midi_state
        .apply(AppEvent::MidiInputScanSucceeded {
            scan_id,
            descriptors: vec![midi_descriptor],
        })
        .unwrap();
    trees.push(
        StateProjector::new()
            .project_with_tree(&midi_state)
            .unwrap()
            .4,
    );
    let connect = midi_state
        .apply(AppEvent::MidiInputConnectRequested {
            identity: midi_identity.clone(),
        })
        .unwrap();
    let request = connect
        .midi_device_effects()
        .iter()
        .find_map(|effect| match effect {
            MidiDeviceEffect::Connect { request } => Some(request.clone()),
            _ => None,
        })
        .unwrap();
    trees.push(
        StateProjector::new()
            .project_with_tree(&midi_state)
            .unwrap()
            .4,
    );
    midi_state
        .apply(AppEvent::MidiInputConnectionPrepared {
            request_id: request.request_id(),
            revision: request.revision(),
        })
        .unwrap();
    trees.push(
        StateProjector::new()
            .project_with_tree(&midi_state)
            .unwrap()
            .4,
    );
    midi_state
        .apply(AppEvent::MidiInputActivationAcknowledged {
            request_id: request.request_id(),
            revision: request.revision(),
        })
        .unwrap();
    trees.push(
        StateProjector::new()
            .project_with_tree(&midi_state)
            .unwrap()
            .4,
    );
    let lost_scan = midi_state.apply(AppEvent::MidiInputScanStarted).unwrap();
    let lost_scan_id = lost_scan
        .midi_device_effects()
        .iter()
        .find_map(|effect| match effect {
            MidiDeviceEffect::Scan { scan_id } => Some(*scan_id),
            _ => None,
        })
        .unwrap();
    midi_state
        .apply(AppEvent::MidiInputScanSucceeded {
            scan_id: lost_scan_id,
            descriptors: Vec::new(),
        })
        .unwrap();
    trees.push(
        StateProjector::new()
            .project_with_tree(&midi_state)
            .unwrap()
            .4,
    );
    let mut midi_failure_state = AppState::new(
        production_capability_registry().unwrap(),
        support::globals(),
    );
    midi_failure_state
        .apply(AppEvent::MidiInputPreferenceRestored {
            preference: None,
            failure: Some(MidiDeviceFailure::PreferenceDecodeFailed),
        })
        .unwrap();
    let failed_scan = midi_failure_state
        .apply_semantic_action(SemanticAction::OpenMidiSettings)
        .unwrap();
    let failed_scan_id = failed_scan
        .midi_device_effects()
        .iter()
        .find_map(|effect| match effect {
            MidiDeviceEffect::Scan { scan_id } => Some(*scan_id),
            _ => None,
        })
        .unwrap();
    midi_failure_state
        .apply(AppEvent::MidiInputScanFailed {
            scan_id: failed_scan_id,
            failure: MidiDeviceFailure::EnumerationFailed,
        })
        .unwrap();
    trees.push(
        StateProjector::new()
            .project_with_tree(&midi_failure_state)
            .unwrap()
            .4,
    );
    // Asset-scoped metadata has the same typed surface as installed metadata.
    // Exercise its optional fields through valid registry/state projections.
    for availability in [
        CapabilityAvailability::Available,
        CapabilityAvailability::Unavailable {
            reason: "schema asset unavailable".into(),
        },
    ] {
        let base = sample
            .descriptor()
            .with_asset_scoped_choices()
            .with_availability(availability);
        let mut base = serde_json::to_value(base).unwrap();
        use crest_synth::synth::{
            CapabilityDescriptor, ParameterAssignment, ParameterDefault, ParameterId,
            ParameterKind, ParameterRange, ParameterSpec, ParameterUpdate, ParameterValue,
        };
        let named_id = ParameterId::new("schema.named-step").unwrap();
        let named = ParameterSpec::new(
            named_id.clone(),
            "Named step",
            ParameterKind::Stepped,
            ParameterUpdate::Scalar,
            ParameterDefault::Value(ParameterValue::Stepped(0)),
            Some(ParameterRange::new(0.0, 1.0).unwrap()),
            Vec::new(),
            Some(1.0),
            Some(1.0),
            None,
            "integer",
            None,
            None,
        )
        .unwrap()
        .with_stepped_labels(vec!["First".into(), "Second".into()])
        .unwrap();
        base["sections"][0]["parameters"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::to_value(named).unwrap());
        // Asset-scoped schemas also carry normalized selector presentation.
        let normalized = crest_synth::adapter::upstream_audio::instrument_ports()
            .unwrap()
            .into_iter()
            .find(|port| port.descriptor().id().as_str() == "instrument.mda.jx10")
            .unwrap()
            .descriptor()
            .parameters()
            .nth(3)
            .unwrap()
            .clone();
        let normalized_assignment = match normalized.default_value() {
            ParameterDefault::Value(value) => {
                ParameterAssignment::new(normalized.id().clone(), value.clone())
            }
            _ => unreachable!(),
        };
        base["sections"][0]["parameters"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::to_value(normalized).unwrap());
        // Exercise conditional visibility in both installed and asset-scoped schemas.
        base["sections"][0]["parameters"]
            .as_array_mut()
            .unwrap()
            .last_mut()
            .unwrap()["visibleWhen"] = serde_json::json!({"parameterId": "schema.named-step", "equals": {"kind": "stepped", "value": 0}});
        let mut scoped = base.clone();
        let base: CapabilityDescriptor = serde_json::from_value(base).unwrap();
        let initial = sample.default_config().unwrap();
        let mut values = initial.values().to_vec();
        values.push(normalized_assignment);
        values.push(ParameterAssignment::new(
            named_id,
            ParameterValue::Stepped(0),
        ));
        let initial = base
            .create_config(&values, initial.asset_references())
            .unwrap();
        for section in scoped["sections"].as_array_mut().unwrap() {
            for parameter in section["parameters"].as_array_mut().unwrap() {
                if parameter["kind"] == "asset" {
                    parameter["defaultValue"]["value"]["locator"] =
                        Value::String("Scoped.wav".into());
                }
            }
        }
        let scoped = serde_json::from_value(scoped).unwrap();
        let registry = CapabilityRegistry::new(vec![base])
            .unwrap()
            .with_asset_descriptor(scoped)
            .unwrap();
        let config = registry
            .replace_asset(
                &initial,
                &crest_synth::synth::ParameterId::new(
                    crest_synth::adapter::sample_capability::SAMPLE_ASSET_PARAMETER_ID,
                )
                .unwrap(),
                crest_synth::synth::AssetReference::new(
                    crest_synth::synth::AssetKind::Sample,
                    "Scoped.wav",
                )
                .unwrap(),
            )
            .unwrap();
        let mut state = AppState::new(registry, support::globals());
        state
            .apply(AppEvent::InstallPatches(vec![Patch::new(
                PatchId::new(92).unwrap(),
                "Asset schema fixture".into(),
                config,
                MidiChannel::new(0).unwrap(),
                PatchOutput::default(),
            )]))
            .unwrap();
        trees.push(StateProjector::new().project_with_tree(&state).unwrap().4);
    }
    let mut discovered = BTreeSet::new();
    for tree in trees {
        discover_leaves(
            &serde_json::from_str::<Value>(tree.json()).unwrap(),
            "",
            &mut discovered,
        );
    }
    let descriptor = StateTree::serialized_leaf_descriptor();
    let described = descriptor
        .iter()
        .map(|path| (*path).to_owned())
        .collect::<BTreeSet<_>>();

    assert_eq!(descriptor.len(), described.len(), "duplicate typed leaf");
    assert_eq!(
        described,
        discovered,
        "described-only={:?}\ndiscovered-only={:?}",
        described.difference(&discovered).collect::<Vec<_>>(),
        discovered.difference(&described).collect::<Vec<_>>()
    );
    discovered
}

#[test]
fn typed_descriptors_and_discovered_serialized_leaves_are_bidirectionally_exact() {
    let discovered = assert_state_tree_leaf_surface_exact();
    // Version 13 added the `parameters.patches[].voiceLimit` leaf: a canonical
    // value that crosses the real-time boundary and changes what is audible
    // must be visible in the trace, or no measured proof can correlate a
    // refused note with the limit that refused it. Version 14 added the
    // `interaction.detailSubject` leaves on the same reasoning: the subject
    // decides what the detail surface shows, so a trace without it cannot
    // correlate a detail interaction with its consequence. Version 15 made the
    // detail surface projectable — `patchPage.detail`, plus per-control
    // `requestedValue` and `validActions` on the semantic model — and moved
    // `PatchDetailSubject`'s own fields to camelCase in the same bump. Version
    // 18 added registry-owned capability availability; 19 adds reducer-owned
    // physical MIDI lifecycle facts while excluding handles and observations.
    // Version 20 adds the explicit tagged trailing-empty Patch shape and its
    // prospective/capacity ownership facts without inventing a Patch ID.
    assert_eq!(StateTree::SCHEMA_VERSION, 34);
    for leaf in GraphicalShellProjection::serialized_leaf_descriptor() {
        let tree_leaf = format!("graphicalShell.{leaf}");
        assert!(
            StateTree::serialized_leaf_descriptor().contains(&tree_leaf.as_str()),
            "StateTree is missing {tree_leaf}"
        );
    }
    assert!(StateTree::serialized_leaf_descriptor().contains(&"patchPage.focusedControlId"));
    assert!(StateTree::serialized_leaf_descriptor().contains(&"patchPage.envelope[].controlId"));
    assert!(StateTree::serialized_leaf_descriptor().contains(&"graphicalShell.generation"));
    assert!(StateTree::serialized_leaf_descriptor()
        .contains(&"graphicalShell.workspace.diagnostic.stateHash"));
    assert!(PatchPageProjection::serialized_leaf_descriptor().contains(&"focusedControlId"));
    assert!(PatchPageProjection::serialized_leaf_descriptor().contains(&"envelope[].controlId"));

    for control in PatchControlId::surface_descriptor()
        .iter()
        .chain(PatchControlId::utility_surface_descriptor())
    {
        let serialized = serde_json::to_string(control).unwrap();
        assert_eq!(serialized, format!("\"{}\"", control.as_str()));
        assert_eq!(
            serde_json::from_str::<PatchControlId>(&serialized).unwrap(),
            *control
        );
    }

    let described = StateTree::serialized_leaf_descriptor()
        .iter()
        .map(|path| (*path).to_owned())
        .collect::<BTreeSet<_>>();
    let mut missing_leaf = discovered.clone();
    assert!(missing_leaf.remove("patchPage.focusedControlId"));
    assert_eq!(described.difference(&missing_leaf).count(), 1);
    assert_eq!(missing_leaf.difference(&described).count(), 0);
    let mut unexpected_leaf = discovered;
    assert!(unexpected_leaf.insert("patchPage.unexpectedControlId".to_owned()));
    assert_eq!(described.difference(&unexpected_leaf).count(), 0);
    assert_eq!(unexpected_leaf.difference(&described).count(), 1);

    let soundfont =
        crest_synth::adapter::production_instruments::production_soundfont_capability().unwrap();
    let mut state = AppState::new(
        production_capability_registry().unwrap(),
        support::globals(),
    );
    state
        .apply(AppEvent::InstallPatches(vec![Patch::new(
            PatchId::new(1).unwrap(),
            "Focus schema".to_owned(),
            create_soundfont_config(&soundfont, SoundFontInstrument::new(0, 11, false).unwrap())
                .unwrap(),
            MidiChannel::new(0).unwrap(),
            PatchOutput::to_track(MixerTrackId::new(0).unwrap()),
        )]))
        .unwrap();
    state
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    let tree = StateProjector::new().project_with_tree(&state).unwrap().4;
    let value: Value = serde_json::from_str(tree.json()).unwrap();
    assert_eq!(
        value["interaction"]["activeFocus"]["controlId"]["id"],
        PatchControlId::Engine.as_str().as_ref()
    );
    assert_eq!(
        value["patchPage"]["focusedControlId"],
        PatchControlId::Engine.as_str().as_ref()
    );
    state
        .apply(AppEvent::EnterSurface(SurfaceId::PatchDetail))
        .unwrap();
    navigate_down_until(&mut state, |path| {
        path.control_id()
            == &crest_synth::control::SemanticControlId::Patch(PatchControlId::Envelope(
                crest_synth::synth::VoiceEnvelopeParameter::AttackMilliseconds,
            ))
    });
    for (index, descriptor) in VoiceEnvelope::surface_descriptor().iter().enumerate() {
        let control = PatchControlId::Envelope(descriptor.parameter());
        let tree = StateProjector::new().project_with_tree(&state).unwrap().4;
        let value: Value = serde_json::from_str(tree.json()).unwrap();
        assert_eq!(
            value["interaction"]["activeFocus"]["controlId"]["id"],
            control.as_str().as_ref()
        );
        assert_eq!(
            value["patchPage"]["focusedControlId"],
            control.as_str().as_ref()
        );
        if index + 1 < VoiceEnvelope::surface_descriptor().len() {
            state
                .apply(AppEvent::Navigate(crest_synth::control::Direction::Down))
                .unwrap();
        }
    }

    let run = support::run_demo();
    assert_eq!(run.report.schema_version(), 7);
    let serialized = run
        .report
        .coverage()
        .group(DemoCoverageGroup::SerializedProperties);
    let projections = run.report.coverage().group(DemoCoverageGroup::Projections);
    let expected = serialized
        .expected()
        .iter()
        .chain(projections.expected())
        .cloned()
        .collect::<BTreeSet<_>>();
    let exercised = serialized
        .exercised()
        .iter()
        .chain(projections.exercised())
        .cloned()
        .collect::<BTreeSet<_>>();

    assert!(!expected.is_empty());
    assert_eq!(expected, exercised);
    assert!(serialized.missing().is_empty());
    assert!(serialized.unexpected().is_empty());
    assert!(projections.missing().is_empty());
    assert!(projections.unexpected().is_empty());
    assert!(expected
        .iter()
        .any(|identifier| identifier.starts_with("property.stateTree.")));
    assert!(expected
        .iter()
        .any(|identifier| identifier.starts_with("property.eventRecord.")));
    assert!(expected
        .iter()
        .any(|identifier| identifier.starts_with("property.eventLog.")));
    assert!(expected
        .iter()
        .any(|identifier| identifier.starts_with("property.textProjection.")));

    let tree: Value = serde_json::from_str(run.report.final_state_tree().json()).unwrap();
    assert_eq!(
        tree["capabilities"]["descriptors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|descriptor| descriptor["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        production_capability_registry()
            .unwrap()
            .descriptors()
            .iter()
            .map(|descriptor| descriptor.id().as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        tree["patches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|patch| patch["instrument"]["capabilityId"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["instrument.soundfont.hidef", "instrument.braids"]
    );
    assert!(run.report.audio_evidence().mixed_engine_stems_nonzero());
    assert!(run
        .report
        .audio_evidence()
        .mixed_engine_parameter_isolation());

    let harness = BehavioralMutationHarness::new();
    let healthy = harness.run(BehavioralMutationCase::OmittedStateTreeLeaf, false);
    let mutant = harness.run(BehavioralMutationCase::OmittedStateTreeLeaf, true);
    let (
        BehavioralMutationObservation::OmittedStateTreeLeaf(healthy),
        BehavioralMutationObservation::OmittedStateTreeLeaf(mutant),
    ) = (healthy.into_observation(), mutant.into_observation())
    else {
        panic!("the omitted-leaf case must retain its typed observation schema");
    };

    assert!(healthy.schema_surface_equal);
    assert!(healthy.required_leaf_count > 0);
    assert_eq!(healthy.missing_leaf_count, 0);
    assert_eq!(healthy.unexpected_leaf_count, 0);
    assert!(!mutant.schema_surface_equal);
    assert_eq!(mutant.required_leaf_count, healthy.required_leaf_count);
    assert_eq!(mutant.missing_leaf_count, 1);
    assert_eq!(mutant.unexpected_leaf_count, 0);

    println!("CREST_ACCEPTANCE schema_surface passed");
}
