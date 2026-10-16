use crest_synth::adapter::sample_capability::{SampleCapability, SAMPLE_ASSET_PARAMETER_ID};
use crest_synth::control::{
    AppEvent, AppState, Direction, EngineSelectionEffect, EngineSelectionFailure,
    EngineSelectionStatusKind, InteractionMode, SampleAssetLifecycle, SemanticAction,
    TopLevelContext,
};
use crest_synth::kernel::{MidiChannel, PatchId};
use crest_synth::mixer::{global_parameters::GlobalParameters, patch_output::PatchOutput};
use crest_synth::real_time::GraphRevision;
use crest_synth::synth::{
    AssetAssignment, AssetFileId, AssetKind, AssetReference, CapabilityDescriptor, CapabilityId,
    CapabilityRegistry, CapabilityVisualization, FileBrowserFolderId, FileBrowserListing,
    FileBrowserRow, FileBrowserRowKind, InstrumentCapabilityProvider, InstrumentConfig,
    ParameterId, Patch, PreparedSamplePcm, PreparedSampleVisualization, WaveformPair,
};
use std::sync::Arc;

pub const ACTIVE_ASSET: &str = "Textures/Cloud C3.wav";
pub const REQUESTED_ASSET: &str =
    "Textures/An exceptionally long evolving stereo texture recorded at dusk C3.wav";

fn begin_assignment(state: &mut AppState, next: bool) -> EngineSelectionEffect {
    state
        .apply_semantic_action(SemanticAction::OpenRelated)
        .unwrap();
    if next {
        state.apply(AppEvent::Navigate(Direction::Down)).unwrap();
    }
    state
        .apply_semantic_action(SemanticAction::Activate)
        .unwrap()
        .engine_selection_effect()
        .unwrap()
        .clone()
}

fn advance(state: &mut AppState, effect: &EngineSelectionEffect, preparing: bool) {
    state
        .apply(AppEvent::SampleAssetLifecycleAdvanced {
            request_id: effect.request_id(),
            lifecycle: if preparing {
                SampleAssetLifecycle::Preparing
            } else {
                SampleAssetLifecycle::Validating
            },
        })
        .unwrap();
    state
        .apply(AppEvent::EngineSelectionLifecycleAdvanced {
            request_id: effect.request_id(),
            lifecycle: if preparing {
                EngineSelectionStatusKind::Preparing
            } else {
                EngineSelectionStatusKind::Validating
            },
        })
        .unwrap();
}

fn preparation_event(
    state: &AppState,
    provider: &SampleCapability,
    effect: &EngineSelectionEffect,
    asset: &str,
) -> AppEvent {
    let config = provider
        .create_config(
            state.patches()[0].instrument_config().values(),
            &[AssetAssignment::new(
                ParameterId::new(SAMPLE_ASSET_PARAMETER_ID).unwrap(),
                AssetReference::new(AssetKind::Sample, asset).unwrap(),
            )],
        )
        .unwrap();
    let visualization = prepared_waveform(provider, &config, asset);
    AppEvent::EnginePrepared {
        request_id: effect.request_id(),
        patch_id: effect.patch_id().unwrap(),
        intent: effect.intent().clone(),
        source_capability_id: effect.source_capability_id().unwrap().clone(),
        target_capability_id: effect.target_capability_id().unwrap().clone(),
        source_graph_revision: effect.source_graph_revision(),
        target_graph_revision: effect.source_graph_revision().checked_next().unwrap(),
        candidate_config: config,
        prepared_visualization: Some(vec![visualization]),
    }
}

fn acknowledge(state: &mut AppState, effect: &EngineSelectionEffect) {
    state
        .apply(AppEvent::EngineActivationAcknowledged {
            request_id: effect.request_id(),
            intent: effect.intent().clone(),
            target_graph_revision: effect.source_graph_revision().checked_next().unwrap(),
            retired_graph_revision: effect.source_graph_revision(),
            collected: true,
        })
        .unwrap();
}

fn prepared_waveform(
    provider: &SampleCapability,
    config: &InstrumentConfig,
    asset: &str,
) -> PreparedSampleVisualization {
    // Deterministic stereo PCM, with a silent opening and right-only bins.
    // Summaries are measured from these samples, never from a drawn fixture.
    let samples = (0..8192)
        .flat_map(|frame| {
            let amplitude = if frame < 128 {
                0.0
            } else {
                (1.0 - frame as f32 / 8192.0) * 0.8
            };
            let right = (frame as f32 * 0.17).sin() * amplitude;
            let left = if frame < 1024 {
                0.0
            } else {
                (frame as f32 * 0.13).sin() * amplitude
            };
            [left, right]
        })
        .collect::<Vec<_>>();
    let pairs = samples
        .chunks(64)
        .map(|bin| {
            let mut pair = WaveformPair {
                left_min: 1.0,
                left_max: -1.0,
                right_min: 1.0,
                right_max: -1.0,
            };
            for frame in bin.chunks_exact(2) {
                pair.left_min = pair.left_min.min(frame[0]);
                pair.left_max = pair.left_max.max(frame[0]);
                pair.right_min = pair.right_min.min(frame[1]);
                pair.right_max = pair.right_max.max(frame[1]);
            }
            pair
        })
        .collect::<Vec<_>>();
    let pcm = PreparedSamplePcm::new(
        AssetFileId::new(asset).unwrap(),
        48_000,
        2,
        Arc::from(samples),
        Arc::from(pairs),
    )
    .unwrap();
    let landmarks = provider
        .playback_config(config)
        .unwrap()
        .prepared_landmarks(pcm.frames(), pcm.sample_rate())
        .unwrap();
    PreparedSampleVisualization::new(&pcm, landmarks)
}

/// Every state traverses the production reducer, including preparation and
/// activation acknowledgement. Shared by serialization and native witnesses.
pub fn sample_detail_states() -> Vec<(&'static str, AppState)> {
    let provider = SampleCapability::new(AssetFileId::new("Factory.wav").unwrap()).unwrap();
    let folder = FileBrowserFolderId::default();
    let rows = [ACTIVE_ASSET, REQUESTED_ASSET]
        .into_iter()
        .map(|asset| {
            FileBrowserRow::new(
                format!("file:{asset}"),
                asset,
                FileBrowserRowKind::File(AssetFileId::new(asset).unwrap()),
                Some(65536),
            )
            .unwrap()
        })
        .collect();
    let mut state = AppState::for_graph(
        CapabilityRegistry::new(vec![provider.descriptor()]).unwrap(),
        GlobalParameters::new(-3.0).unwrap(),
        GraphRevision::INITIAL,
    )
    .with_sample_catalog([(
        folder.clone(),
        Ok(FileBrowserListing::new(folder, rows).unwrap()),
    )]);
    state
        .apply(AppEvent::InstallPatches(vec![Patch::new(
            PatchId::new(1).unwrap(),
            "Cloud Texture".to_owned(),
            provider.default_config().unwrap(),
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        )]))
        .unwrap();
    state
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::OpenRelated)
        .unwrap();
    let mut states = vec![("patch-sample-unavailable", state.clone())];
    let first = begin_assignment(&mut state, false);
    advance(&mut state, &first, false);
    advance(&mut state, &first, true);
    let prepared = preparation_event(&state, &provider, &first, ACTIVE_ASSET);
    state.apply(prepared).unwrap();
    acknowledge(&mut state, &first);
    states.push(("patch-sample-ready", state.clone()));

    let mut focused = state.clone();
    focused.apply(AppEvent::Navigate(Direction::Down)).unwrap();
    states.push(("patch-sample-root-focused", focused.clone()));
    focused.apply(AppEvent::Navigate(Direction::Down)).unwrap();
    focused
        .apply_semantic_action(SemanticAction::SetInteractionMode(InteractionMode::Adjust))
        .unwrap();
    focused
        .apply_semantic_action(SemanticAction::Adjust(Direction::Right))
        .unwrap();
    states.push(("patch-sample-start-adjusting", focused));

    let replacement = begin_assignment(&mut state, true);
    states.push(("patch-sample-loading", state.clone()));
    advance(&mut state, &replacement, false);
    states.push(("patch-sample-validating", state.clone()));
    advance(&mut state, &replacement, true);
    states.push(("patch-sample-preparing", state.clone()));
    for (label, failure) in [
        (
            "patch-sample-asset-unavailable",
            EngineSelectionFailure::AssetUnavailable,
        ),
        (
            "patch-sample-failed",
            EngineSelectionFailure::AllocationFailed,
        ),
    ] {
        let mut failed = state.clone();
        failed
            .apply(AppEvent::EnginePreparationFailed {
                request_id: replacement.request_id(),
                patch_id: replacement.patch_id().unwrap(),
                intent: replacement.intent().clone(),
                source_capability_id: replacement.source_capability_id().unwrap().clone(),
                target_capability_id: replacement.target_capability_id().unwrap().clone(),
                source_graph_revision: replacement.source_graph_revision(),
                target_graph_revision: replacement.source_graph_revision().checked_next().unwrap(),
                failure,
            })
            .unwrap();
        states.push((label, failed));
    }
    // A worker summary for another asset must not be painted as the newly
    // acknowledged asset. Keep this negative on the production event path.
    let mut incompatible = state.clone();
    let mut wrong_summary = preparation_event(&state, &provider, &replacement, REQUESTED_ASSET);
    if let AppEvent::EnginePrepared {
        prepared_visualization,
        ..
    } = &mut wrong_summary
    {
        *prepared_visualization = Some(vec![prepared_waveform(
            &provider,
            state.patches()[0].instrument_config(),
            ACTIVE_ASSET,
        )]);
    }
    incompatible.apply(wrong_summary).unwrap();
    acknowledge(&mut incompatible, &replacement);
    states.push(("patch-sample-incompatible", incompatible));

    let prepared = preparation_event(&state, &provider, &replacement, REQUESTED_ASSET);
    state.apply(prepared).unwrap();
    states.push(("patch-sample-activating", state.clone()));
    acknowledge(&mut state, &replacement);
    states.push(("patch-sample-long-asset", state));
    states.push(("patch-sample-short-descriptor", shaped_detail_state(true)));
    states.push((
        "patch-sample-reordered-descriptor",
        shaped_detail_state(false),
    ));
    states
}

fn shaped_detail_state(short: bool) -> AppState {
    let provider = SampleCapability::new(AssetFileId::new(REQUESTED_ASSET).unwrap()).unwrap();
    let prototype = provider.descriptor();
    let mut sections = prototype.sections().to_vec();
    if short {
        sections.truncate(1);
    } else {
        sections.reverse();
    }
    let mut waveform = prototype
        .visualizations()
        .iter()
        .find(|visualization| matches!(visualization, CapabilityVisualization::Waveform { .. }))
        .unwrap()
        .clone();
    if short {
        if let CapabilityVisualization::Waveform { landmarks, .. } = &mut waveform {
            landmarks.truncate(2);
        }
    }
    let descriptor = CapabilityDescriptor::new(
        CapabilityId::new("fixture.waveform").unwrap(),
        "Waveform Fixture",
        "instrument.sample",
        sections,
        prototype.asset_requirements().to_vec(),
        prototype.voice_policy(),
        prototype.supported_midi_kinds().to_vec(),
    )
    .unwrap()
    .with_visualizations([waveform])
    .unwrap();
    let defaults = provider.default_config().unwrap();
    let values = defaults
        .values()
        .iter()
        .filter(|value| descriptor.parameter(value.parameter_id()).is_some())
        .cloned()
        .collect::<Vec<_>>();
    let config = descriptor
        .create_config(&values, defaults.asset_references())
        .unwrap();
    let mut state = AppState::new(
        CapabilityRegistry::new(vec![descriptor]).unwrap(),
        GlobalParameters::new(-3.0).unwrap(),
    );
    state
        .apply(AppEvent::InstallPatches(vec![Patch::new(
            PatchId::new(1).unwrap(),
            "Descriptor Shape".to_owned(),
            config,
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        )]))
        .unwrap();
    state
        .apply(AppEvent::SelectContext(TopLevelContext::Patch))
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::OpenRelated)
        .unwrap();
    state
}
