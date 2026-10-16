use crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
use crest_synth::adapter::lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary;
use crest_synth::adapter::sample_capability::{
    SampleCapability, SAMPLE_ASSET_PARAMETER_ID, SAMPLE_CAPABILITY_ID,
    SAMPLE_LOOP_FORWARD_CHOICE_ID, SAMPLE_LOOP_MODE_PARAMETER_ID, SAMPLE_ROOT_NOTE_PARAMETER_ID,
};
use crest_synth::adapter::sample_preparer::SamplePreparer;
use crest_synth::control::{
    AppEvent, AppLoop, AppState, Direction, EngineSelectionFailure, EngineSelectionRequestId,
    EngineSelectionStatusKind, GraphicalShellProjection, InteractionMode, ModalControlId,
    SampleAssetLifecycle, SamplePreviewState, SemanticAction, SemanticBrowserMetadataStatus,
    SemanticControlId, SemanticControlKind, SemanticControlValue, SemanticResolver,
    SemanticSurfaceSummary, SemanticVisualizationData, StateProjector, SurfaceId,
};
use crest_synth::kernel::{MidiChannel, PatchId};
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mixer_state::MixerState;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::{
    AudioBoundary, AudioCommand, AudioRenderer, GraphHandoffStatus, GraphRevision,
    ParameterSnapshot, PreparedGraphBuilder, StructuralGraphBoundary,
};
use crest_synth::shell::audio_output::{AudioDeviceConfig, AudioSampleFormat};
use crest_synth::synth::{
    AssetAssignment, AssetFileId, AssetKind, AssetReference, CapabilityRegistry, DecodedSample,
    DescriptorDefaultConfigFactory, FileBrowserFolderId, FileBrowserListing, FileBrowserRow,
    FileBrowserRowKind, InstrumentCapabilityProvider, InstrumentPreparer, ParameterId,
    ParameterValue, Patch, SampleEncoding, SampleMetadata,
};
use crest_synth::testing::{
    DeterministicGraphPreparationWorker, DeterministicSampleCatalog, DeterministicSampleDecoder,
};
use std::sync::Arc;

fn fixture() -> (AppState, SampleCapability, PatchId) {
    let provider = SampleCapability::new(AssetFileId::new("Factory.wav").unwrap()).unwrap();
    let registry = CapabilityRegistry::new(vec![provider.descriptor()]).unwrap();
    let patch_id = PatchId::new(1).unwrap();
    let patch = Patch::new(
        patch_id,
        "Sample Patch".to_owned(),
        provider.default_config().unwrap(),
        MidiChannel::new(0).unwrap(),
        PatchOutput::default(),
    );
    let folder = FileBrowserFolderId::default();
    let listing = FileBrowserListing::new(
        folder.clone(),
        vec![
            FileBrowserRow::new(
                "file:Alternate.wav",
                "Alternate.wav",
                FileBrowserRowKind::File(AssetFileId::new("Alternate.wav").unwrap()),
                Some(128),
            )
            .unwrap()
            .with_metadata(Ok(SampleMetadata::new(
                AssetFileId::new("Alternate.wav").unwrap(),
                128,
                48_000,
                2,
                24,
                SampleEncoding::SignedPcm,
                48_000,
            )
            .unwrap()))
            .unwrap(),
            FileBrowserRow::new(
                "cancel:",
                "CANCEL — UNCHANGED",
                FileBrowserRowKind::Cancel,
                None,
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let mut state = AppState::for_graph(
        registry,
        crest_synth::mixer::global_parameters::GlobalParameters::new(0.0).unwrap(),
        GraphRevision::INITIAL,
    )
    .with_sample_catalog([(folder, Ok(listing))]);
    state.apply(AppEvent::InstallPatches(vec![patch])).unwrap();
    state
        .apply_semantic_action(SemanticAction::SelectContext(
            crest_synth::control::TopLevelContext::Patch,
        ))
        .unwrap();
    (state, provider, patch_id)
}

fn open_browser(state: &mut AppState) -> crest_synth::control::FocusPath {
    state
        .apply_semantic_action(SemanticAction::OpenRelated)
        .unwrap();
    assert_eq!(state.interaction().active_surface(), SurfaceId::PatchDetail);
    let asset_origin = state.interaction().focus_path().clone();
    state
        .apply_semantic_action(SemanticAction::OpenRelated)
        .unwrap();
    assert_eq!(state.interaction().active_surface(), SurfaceId::FileBrowser);
    assert_eq!(state.interaction().mode(), InteractionMode::Modal);
    asset_origin
}

fn alternate_candidate(
    state: &AppState,
    provider: &SampleCapability,
) -> crest_synth::synth::InstrumentConfig {
    provider
        .create_config(
            state.patches()[0].instrument_config().values(),
            &[AssetAssignment::new(
                ParameterId::new(SAMPLE_ASSET_PARAMETER_ID).unwrap(),
                AssetReference::new(AssetKind::Sample, "Alternate.wav").unwrap(),
            )],
        )
        .unwrap()
}

fn advance_engine_admission(state: &mut AppState, request_id: EngineSelectionRequestId) {
    for lifecycle in [
        EngineSelectionStatusKind::Validating,
        EngineSelectionStatusKind::Preparing,
    ] {
        state
            .apply(AppEvent::EngineSelectionLifecycleAdvanced {
                request_id,
                lifecycle,
            })
            .unwrap();
    }
}

fn decoded_fixture(asset: &str) -> DecodedSample {
    let asset_id = AssetFileId::new(asset).unwrap();
    let interleaved = (0..128)
        .map(|frame| (frame as f32 / 128.0) * 0.5)
        .collect::<Vec<_>>();
    DecodedSample::new(
        SampleMetadata::new(
            asset_id,
            256,
            48_000,
            1,
            32,
            SampleEncoding::Float,
            interleaved.len() as u64,
        )
        .unwrap(),
        interleaved,
    )
    .unwrap()
}

fn waveform_status(shell: &GraphicalShellProjection) -> String {
    let detail = shell
        .semantic_model()
        .surface(SurfaceId::PatchDetail)
        .expect("the asset assignment returns to Detail");
    match detail
        .visualizations()
        .iter()
        .find(|visualization| visualization.id() == "sample.waveform")
        .expect("the Sample descriptor declares a waveform")
        .data()
    {
        SemanticVisualizationData::Waveform { status, .. } => status.clone(),
        other => panic!("sample.waveform projected the wrong data: {other:?}"),
    }
}

fn activate_preview(
    state: &mut AppState,
    provider: &SampleCapability,
    patch_id: PatchId,
) -> crest_synth::control::EngineSelectionEffect {
    let effect = state
        .apply_semantic_action(SemanticAction::PreviewStart)
        .unwrap()
        .engine_selection_effect()
        .unwrap()
        .clone();
    let target_revision = GraphRevision::INITIAL.checked_next().unwrap();
    advance_engine_admission(state, effect.request_id());
    state
        .apply(AppEvent::EnginePrepared {
            request_id: effect.request_id(),
            patch_id,
            intent: effect.intent().clone(),
            source_capability_id: effect.source_capability_id().unwrap().clone(),
            target_capability_id: effect.target_capability_id().unwrap().clone(),
            source_graph_revision: GraphRevision::INITIAL,
            target_graph_revision: target_revision,
            candidate_config: alternate_candidate(state, provider),
            prepared_visualization: None,
        })
        .unwrap();
    let activated = state
        .apply(AppEvent::EngineActivationAcknowledged {
            request_id: effect.request_id(),
            intent: effect.intent().clone(),
            target_graph_revision: target_revision,
            retired_graph_revision: GraphRevision::INITIAL,
            collected: true,
        })
        .unwrap();
    assert_eq!(
        activated.audio_command(),
        Some(&AudioCommand::preview_start(
            patch_id,
            effect.request_id().value()
        ))
    );
    effect
}

#[test]
fn browser_focus_is_stable_trapped_nonwrapping_and_preview_stops_on_navigation_and_cancel() {
    let (mut state, _, _) = fixture();
    let asset_origin = open_browser(&mut state);
    assert_eq!(
        state.interaction().focus_path().control_id(),
        &SemanticControlId::Modal(ModalControlId::BrowserEntry(
            "file:Alternate.wav".to_owned()
        ))
    );
    let preview = state
        .apply_semantic_action(SemanticAction::PreviewStart)
        .unwrap();
    assert!(preview.audio_command().is_none());
    assert!(preview.engine_selection_effect().is_some());
    assert!(matches!(
        state.file_browser().preview(),
        SamplePreviewState::Preparing { asset_id, held: true }
            if asset_id.as_str() == "Alternate.wav"
    ));
    assert!(state
        .apply_semantic_action(SemanticAction::PreviewStart)
        .is_err());

    state
        .apply_semantic_action(SemanticAction::Navigate(Direction::Down))
        .unwrap();
    assert!(matches!(
        state.file_browser().preview(),
        SamplePreviewState::Preparing { asset_id, held: false }
            if asset_id.as_str() == "Alternate.wav"
    ));
    assert!(state
        .apply_semantic_action(SemanticAction::Navigate(Direction::Down))
        .is_err());
    state
        .apply_semantic_action(SemanticAction::Navigate(Direction::Up))
        .unwrap();
    assert!(state
        .apply_semantic_action(SemanticAction::Navigate(Direction::Up))
        .is_err());

    state.apply_semantic_action(SemanticAction::Return).unwrap();
    assert_eq!(state.interaction().focus_path(), &asset_origin);
    assert_eq!(
        state.file_browser().lifecycle(),
        SampleAssetLifecycle::Cancelled
    );
}

#[test]
fn catalog_refresh_preserves_stable_row_focus_and_reprojects_typed_metadata() {
    let (mut state, _, patch_id) = fixture();
    open_browser(&mut state);
    let focused = state.interaction().focus_path().clone();
    let folder = FileBrowserFolderId::default();
    let refreshed = FileBrowserListing::new(
        folder.clone(),
        vec![
            FileBrowserRow::new(
                "file:Alternate.wav",
                "Alternate.wav",
                FileBrowserRowKind::File(AssetFileId::new("Alternate.wav").unwrap()),
                Some(512),
            )
            .unwrap()
            .with_metadata(Ok(SampleMetadata::new(
                AssetFileId::new("Alternate.wav").unwrap(),
                512,
                96_000,
                1,
                32,
                SampleEncoding::Float,
                96_000,
            )
            .unwrap()))
            .unwrap(),
            FileBrowserRow::new(
                "cancel:",
                "CANCEL — UNCHANGED",
                FileBrowserRowKind::Cancel,
                None,
            )
            .unwrap(),
        ],
    )
    .unwrap();
    state
        .apply(AppEvent::FileCatalogRefreshed {
            asset_kind: crest_synth::synth::AssetKind::Sample,
            folder: folder.clone(),
            listing: Ok(refreshed),
        })
        .unwrap();
    assert_eq!(state.interaction().focus_path(), &focused);
    let (_, _, _, shell, _) = StateProjector::new().project_with_shell(&state).unwrap();
    let metadata = shell
        .semantic_model()
        .surface(SurfaceId::FileBrowser)
        .unwrap()
        .controls()[0]
        .browser_metadata()
        .unwrap();
    assert_eq!(metadata.status(), SemanticBrowserMetadataStatus::Ready);
    assert_eq!(metadata.sample_rate(), Some(96_000));
    assert_eq!(metadata.channels(), Some(1));
    assert!(metadata.text().contains("32-BIT FLOAT"));
    assert!(metadata.text().contains("MONO"));

    state
        .apply(AppEvent::FileCatalogRefreshed {
            asset_kind: crest_synth::synth::AssetKind::Sample,
            folder,
            listing: Err(crest_synth::synth::SampleAssetError::Unavailable),
        })
        .unwrap();
    assert_eq!(state.interaction().active_surface(), SurfaceId::FileBrowser);
    assert_eq!(state.file_browser().rows().len(), 1);
    assert!(matches!(
        state.file_browser().rows()[0].kind(),
        FileBrowserRowKind::Cancel
    ));
    assert!(SemanticResolver::new(&state).resolves(state.interaction().focus_path()));
    assert_eq!(state.interaction().focus_path().patch_id(), Some(patch_id));
    assert_eq!(
        state.file_browser().lifecycle(),
        SampleAssetLifecycle::Unavailable
    );
}

#[test]
fn preview_emits_no_audio_before_activation_and_never_changes_the_assigned_asset() {
    let (mut state, provider, patch_id) = fixture();
    let original = state.patches()[0].instrument_config().clone();
    open_browser(&mut state);
    let effect = state
        .apply_semantic_action(SemanticAction::PreviewStart)
        .unwrap()
        .engine_selection_effect()
        .unwrap()
        .clone();
    let target_revision = GraphRevision::INITIAL.checked_next().unwrap();
    advance_engine_admission(&mut state, effect.request_id());
    let prepared = state
        .apply(AppEvent::EnginePrepared {
            request_id: effect.request_id(),
            patch_id,
            intent: effect.intent().clone(),
            source_capability_id: effect.source_capability_id().unwrap().clone(),
            target_capability_id: effect.target_capability_id().unwrap().clone(),
            source_graph_revision: GraphRevision::INITIAL,
            target_graph_revision: target_revision,
            candidate_config: alternate_candidate(&state, &provider),
            prepared_visualization: None,
        })
        .unwrap();
    assert!(prepared.audio_command().is_none());
    assert_eq!(state.patches()[0].instrument_config(), &original);

    let activated = state
        .apply(AppEvent::EngineActivationAcknowledged {
            request_id: effect.request_id(),
            intent: effect.intent().clone(),
            target_graph_revision: target_revision,
            retired_graph_revision: GraphRevision::INITIAL,
            collected: true,
        })
        .unwrap();
    assert_eq!(
        activated.audio_command(),
        Some(&AudioCommand::preview_start(
            patch_id,
            effect.request_id().value()
        ))
    );
    assert!(matches!(
        state.file_browser().preview(),
        SamplePreviewState::Playing { asset_id } if asset_id.as_str() == "Alternate.wav"
    ));
    assert_eq!(state.patches()[0].instrument_config(), &original);

    let stopped = state
        .apply_semantic_action(SemanticAction::PreviewStop)
        .unwrap();
    assert_eq!(
        stopped.audio_command(),
        Some(&AudioCommand::preview_stop(
            patch_id,
            effect.request_id().value()
        ))
    );
    assert_eq!(state.file_browser().preview(), &SamplePreviewState::Idle);
    assert_eq!(state.patches()[0].instrument_config(), &original);
}

#[test]
fn release_before_preview_activation_suppresses_both_start_and_stop_commands() {
    let (mut state, provider, patch_id) = fixture();
    open_browser(&mut state);
    let effect = state
        .apply_semantic_action(SemanticAction::PreviewStart)
        .unwrap()
        .engine_selection_effect()
        .unwrap()
        .clone();
    let released = state
        .apply_semantic_action(SemanticAction::PreviewStop)
        .unwrap();
    assert!(released.audio_command().is_none());
    assert!(matches!(
        state.file_browser().preview(),
        SamplePreviewState::Preparing { held: false, .. }
    ));

    let target_revision = GraphRevision::INITIAL.checked_next().unwrap();
    advance_engine_admission(&mut state, effect.request_id());
    assert!(state
        .apply(AppEvent::EnginePrepared {
            request_id: effect.request_id(),
            patch_id,
            intent: effect.intent().clone(),
            source_capability_id: effect.source_capability_id().unwrap().clone(),
            target_capability_id: effect.target_capability_id().unwrap().clone(),
            source_graph_revision: GraphRevision::INITIAL,
            target_graph_revision: target_revision,
            candidate_config: alternate_candidate(&state, &provider),
            prepared_visualization: None,
        })
        .unwrap()
        .audio_command()
        .is_none());
    assert!(state
        .apply(AppEvent::EngineActivationAcknowledged {
            request_id: effect.request_id(),
            intent: effect.intent().clone(),
            target_graph_revision: target_revision,
            retired_graph_revision: GraphRevision::INITIAL,
            collected: true,
        })
        .unwrap()
        .audio_command()
        .is_none());
    assert_eq!(state.file_browser().preview(), &SamplePreviewState::Idle);
}

#[test]
fn active_preview_stops_exactly_once_on_navigation_assignment_and_browser_cancel() {
    // Moving focus is an implicit stop, even though the browser stays open.
    let (mut state, provider, patch_id) = fixture();
    open_browser(&mut state);
    let preview = activate_preview(&mut state, &provider, patch_id);
    let navigated = state
        .apply_semantic_action(SemanticAction::Navigate(Direction::Down))
        .unwrap();
    assert_eq!(
        navigated.audio_command(),
        Some(&AudioCommand::preview_stop(
            patch_id,
            preview.request_id().value()
        ))
    );
    assert_eq!(state.file_browser().preview(), &SamplePreviewState::Idle);

    // Activating the previewed file stops that voice and starts only the
    // separately correlated assignment preparation.
    let (mut state, provider, patch_id) = fixture();
    open_browser(&mut state);
    let preview = activate_preview(&mut state, &provider, patch_id);
    let assigned = state
        .apply_semantic_action(SemanticAction::Activate)
        .unwrap();
    assert_eq!(
        assigned.audio_command(),
        Some(&AudioCommand::preview_stop(
            patch_id,
            preview.request_id().value()
        ))
    );
    assert!(assigned.engine_selection_effect().is_some());
    assert_eq!(state.file_browser().preview(), &SamplePreviewState::Idle);

    // Returning from the browser is cancellation and carries the same exact
    // fixed-size stop command. A second Stop is rejected, so it cannot emit a
    // duplicate command after the implicit transition.
    let (mut state, provider, patch_id) = fixture();
    let origin = open_browser(&mut state);
    let preview = activate_preview(&mut state, &provider, patch_id);
    let cancelled = state
        .apply_semantic_action(SemanticAction::NavigatePage(Direction::Down))
        .unwrap();
    assert_eq!(
        cancelled.audio_command(),
        Some(&AudioCommand::preview_stop(
            patch_id,
            preview.request_id().value()
        ))
    );
    assert_eq!(state.interaction().focus_path(), &origin);
    assert_eq!(
        state.file_browser().lifecycle(),
        SampleAssetLifecycle::Cancelled
    );
    assert!(state
        .apply_semantic_action(SemanticAction::PreviewStop)
        .is_err());
}

#[test]
fn preview_rejects_start_outside_browser_and_stale_preparation_without_mutation() {
    let (mut state, provider, patch_id) = fixture();
    let generation = state.generation();
    assert!(state
        .apply_semantic_action(SemanticAction::PreviewStart)
        .is_err());
    assert_eq!(state.generation(), generation);
    assert_eq!(state.file_browser().preview(), &SamplePreviewState::Idle);

    open_browser(&mut state);
    let effect = state
        .apply_semantic_action(SemanticAction::PreviewStart)
        .unwrap()
        .engine_selection_effect()
        .unwrap()
        .clone();
    let expected_preview = state.file_browser().preview().clone();
    let generation = state.generation();
    let stale_request = EngineSelectionRequestId::new(effect.request_id().value() + 1).unwrap();
    let target_revision = GraphRevision::INITIAL.checked_next().unwrap();
    assert!(state
        .apply(AppEvent::EnginePrepared {
            request_id: stale_request,
            patch_id,
            intent: effect.intent().clone(),
            source_capability_id: effect.source_capability_id().unwrap().clone(),
            target_capability_id: effect.target_capability_id().unwrap().clone(),
            source_graph_revision: GraphRevision::INITIAL,
            target_graph_revision: target_revision,
            candidate_config: alternate_candidate(&state, &provider),
            prepared_visualization: None,
        })
        .is_err());
    assert_eq!(state.generation(), generation);
    assert_eq!(state.file_browser().preview(), &expected_preview);
    assert!(state
        .apply(AppEvent::EnginePreparationFailed {
            request_id: stale_request,
            patch_id,
            intent: effect.intent().clone(),
            source_capability_id: effect.source_capability_id().unwrap().clone(),
            target_capability_id: effect.target_capability_id().unwrap().clone(),
            source_graph_revision: GraphRevision::INITIAL,
            target_graph_revision: target_revision,
            failure: EngineSelectionFailure::Cancelled,
        })
        .is_err());
    assert_eq!(state.generation(), generation);
    assert_eq!(state.file_browser().preview(), &expected_preview);
}

#[test]
fn asset_assignment_is_correlated_failure_safe_and_ready_only_after_activation() {
    let (mut state, _, patch_id) = fixture();
    let original = state.patches()[0].instrument_config().clone();
    let asset_origin = open_browser(&mut state);
    let outcome = state
        .apply_semantic_action(SemanticAction::Activate)
        .unwrap();
    let effect = outcome.engine_selection_effect().unwrap().clone();
    assert_eq!(state.interaction().focus_path(), &asset_origin);
    assert_eq!(state.patches()[0].instrument_config(), &original);
    assert_eq!(
        state
            .file_browser()
            .requested_asset()
            .map(AssetFileId::as_str),
        Some("Alternate.wav")
    );
    assert_eq!(
        state.file_browser().lifecycle(),
        SampleAssetLifecycle::Loading
    );

    let target_revision = GraphRevision::INITIAL.checked_next().unwrap();
    state
        .apply(AppEvent::EnginePreparationFailed {
            request_id: effect.request_id(),
            patch_id,
            intent: effect.intent().clone(),
            source_capability_id: effect.source_capability_id().unwrap().clone(),
            target_capability_id: effect.target_capability_id().unwrap().clone(),
            source_graph_revision: GraphRevision::INITIAL,
            target_graph_revision: target_revision,
            failure: EngineSelectionFailure::InvalidAsset,
        })
        .unwrap();
    assert_eq!(state.patches()[0].instrument_config(), &original);
    assert_eq!(
        state.file_browser().lifecycle(),
        SampleAssetLifecycle::Invalid
    );

    // A fresh state proves the valid path independently of the terminal
    // failure above; terminal states never silently restart themselves.
    let (mut state, provider, patch_id) = fixture();
    open_browser(&mut state);
    let effect = state
        .apply_semantic_action(SemanticAction::Activate)
        .unwrap()
        .engine_selection_effect()
        .unwrap()
        .clone();
    let source = state.patches()[0].instrument_config().clone();
    let candidate = provider
        .create_config(
            source.values(),
            &[AssetAssignment::new(
                ParameterId::new(SAMPLE_ASSET_PARAMETER_ID).unwrap(),
                AssetReference::new(AssetKind::Sample, "Alternate.wav").unwrap(),
            )],
        )
        .unwrap();
    for lifecycle in [
        SampleAssetLifecycle::Validating,
        SampleAssetLifecycle::Preparing,
    ] {
        state
            .apply(AppEvent::SampleAssetLifecycleAdvanced {
                request_id: effect.request_id(),
                lifecycle,
            })
            .unwrap();
    }
    advance_engine_admission(&mut state, effect.request_id());
    state
        .apply(AppEvent::EnginePrepared {
            request_id: effect.request_id(),
            patch_id,
            intent: effect.intent().clone(),
            source_capability_id: effect.source_capability_id().unwrap().clone(),
            target_capability_id: effect.target_capability_id().unwrap().clone(),
            source_graph_revision: GraphRevision::INITIAL,
            target_graph_revision: target_revision,
            candidate_config: candidate,
            prepared_visualization: None,
        })
        .unwrap();
    assert_eq!(
        state.file_browser().lifecycle(),
        SampleAssetLifecycle::Activating
    );
    assert_eq!(
        state.patches()[0]
            .instrument_config()
            .asset_reference(&ParameterId::new(SAMPLE_ASSET_PARAMETER_ID).unwrap())
            .map(AssetReference::locator),
        Some("Factory.wav"),
        "the active Patch reference must remain unchanged until graph activation"
    );
    state
        .apply(AppEvent::EngineActivationAcknowledged {
            request_id: effect.request_id(),
            intent: effect.intent().clone(),
            target_graph_revision: target_revision,
            retired_graph_revision: GraphRevision::INITIAL,
            collected: true,
        })
        .unwrap();
    assert_eq!(
        state.file_browser().lifecycle(),
        SampleAssetLifecycle::Ready
    );
    assert_eq!(
        state.patches()[0]
            .instrument_config()
            .asset_reference(&ParameterId::new(SAMPLE_ASSET_PARAMETER_ID).unwrap())
            .map(AssetReference::locator),
        Some("Alternate.wav")
    );
    assert_eq!(
        state.patches()[0]
            .instrument_config()
            .capability_id()
            .as_str(),
        SAMPLE_CAPABILITY_ID
    );
}

#[test]
fn asset_lifecycle_is_ordered_correlated_and_keeps_typed_terminal_states() {
    let (mut state, _, _) = fixture();
    open_browser(&mut state);
    let effect = state
        .apply_semantic_action(SemanticAction::Activate)
        .unwrap()
        .engine_selection_effect()
        .unwrap()
        .clone();
    assert_eq!(
        state.file_browser().lifecycle(),
        SampleAssetLifecycle::Loading
    );

    let before_skip = state.clone();
    assert!(state
        .apply(AppEvent::SampleAssetLifecycleAdvanced {
            request_id: effect.request_id(),
            lifecycle: SampleAssetLifecycle::Preparing,
        })
        .is_err());
    assert_eq!(state, before_skip, "the lifecycle cannot skip validation");
    assert!(state
        .apply(AppEvent::SampleAssetLifecycleAdvanced {
            request_id: effect.request_id().checked_next().unwrap(),
            lifecycle: SampleAssetLifecycle::Validating,
        })
        .is_err());
    for lifecycle in [
        SampleAssetLifecycle::Validating,
        SampleAssetLifecycle::Preparing,
    ] {
        state
            .apply(AppEvent::SampleAssetLifecycleAdvanced {
                request_id: effect.request_id(),
                lifecycle,
            })
            .unwrap();
        assert_eq!(state.file_browser().lifecycle(), lifecycle);
    }

    for (failure, terminal) in [
        (
            EngineSelectionFailure::AssetUnavailable,
            SampleAssetLifecycle::Unavailable,
        ),
        (
            EngineSelectionFailure::UnsupportedAssetFormat,
            SampleAssetLifecycle::Invalid,
        ),
        (
            EngineSelectionFailure::Cancelled,
            SampleAssetLifecycle::Cancelled,
        ),
        (
            EngineSelectionFailure::AllocationFailed,
            SampleAssetLifecycle::Failed(EngineSelectionFailure::AllocationFailed),
        ),
    ] {
        let (mut terminal_state, _, patch_id) = fixture();
        open_browser(&mut terminal_state);
        let terminal_effect = terminal_state
            .apply_semantic_action(SemanticAction::Activate)
            .unwrap()
            .engine_selection_effect()
            .unwrap()
            .clone();
        terminal_state
            .apply(AppEvent::EnginePreparationFailed {
                request_id: terminal_effect.request_id(),
                patch_id,
                intent: terminal_effect.intent().clone(),
                source_capability_id: terminal_effect.source_capability_id().unwrap().clone(),
                target_capability_id: terminal_effect.target_capability_id().unwrap().clone(),
                source_graph_revision: GraphRevision::INITIAL,
                target_graph_revision: GraphRevision::INITIAL.checked_next().unwrap(),
                failure,
            })
            .unwrap();
        assert_eq!(terminal_state.file_browser().lifecycle(), terminal);
    }
}

#[test]
fn asset_projection_keeps_active_and_requested_distinct_and_names_every_lifecycle() {
    let (mut state, _, patch_id) = fixture();
    open_browser(&mut state);
    let effect = state
        .apply_semantic_action(SemanticAction::Activate)
        .unwrap()
        .engine_selection_effect()
        .unwrap()
        .clone();

    let assert_projection = |state: &AppState, expected_status: &str| {
        let (_, _, _, shell, _) = StateProjector::new().project_with_shell(state).unwrap();
        let detail = shell
            .semantic_model()
            .surface(SurfaceId::PatchDetail)
            .expect("assignment returns to the exact asset origin in Detail");
        let asset = detail
            .controls()
            .iter()
            .find(|control| {
                matches!(
                    control.path().control_id(),
                    SemanticControlId::Patch(crest_synth::control::PatchControlId::Capability(id))
                        if id.as_str() == SAMPLE_ASSET_PARAMETER_ID
                )
            })
            .expect("the generic descriptor projects the asset row");
        assert_eq!(
            asset.value(),
            &SemanticControlValue::Asset(
                AssetReference::new(AssetKind::Sample, "Factory.wav").unwrap()
            ),
            "the committed reference remains the active row value"
        );
        assert_eq!(
            asset.requested_value(),
            Some(&SemanticControlValue::Asset(
                AssetReference::new(AssetKind::Sample, "Alternate.wav").unwrap()
            )),
            "the correlated replacement is projected separately"
        );
        let waveform = detail
            .visualizations()
            .iter()
            .find(|visualization| visualization.id() == "sample.waveform")
            .expect("Sample declares one generic waveform visualization");
        assert!(matches!(
            waveform.data(),
            SemanticVisualizationData::Waveform { asset: Some(active), status, .. }
                if active.locator() == "Factory.wav" && status == expected_status
        ));
    };

    assert_eq!(effect.patch_id(), Some(patch_id));
    assert_projection(&state, "LOADING");
    for (lifecycle, label) in [
        (SampleAssetLifecycle::Validating, "VALIDATING"),
        (SampleAssetLifecycle::Preparing, "PREPARING"),
    ] {
        state
            .apply(AppEvent::SampleAssetLifecycleAdvanced {
                request_id: effect.request_id(),
                lifecycle,
            })
            .unwrap();
        assert_projection(&state, label);
    }

    for (failure, label) in [
        (EngineSelectionFailure::AssetUnavailable, "UNAVAILABLE"),
        (EngineSelectionFailure::UnsupportedAssetFormat, "INVALID"),
        (EngineSelectionFailure::Cancelled, "CANCELLED — UNCHANGED"),
        (EngineSelectionFailure::AllocationFailed, "FAILED"),
    ] {
        let (mut terminal, _, terminal_patch) = fixture();
        open_browser(&mut terminal);
        let terminal_effect = terminal
            .apply_semantic_action(SemanticAction::Activate)
            .unwrap()
            .engine_selection_effect()
            .unwrap()
            .clone();
        terminal
            .apply(AppEvent::EnginePreparationFailed {
                request_id: terminal_effect.request_id(),
                patch_id: terminal_patch,
                intent: terminal_effect.intent().clone(),
                source_capability_id: terminal_effect.source_capability_id().unwrap().clone(),
                target_capability_id: terminal_effect.target_capability_id().unwrap().clone(),
                source_graph_revision: GraphRevision::INITIAL,
                target_graph_revision: GraphRevision::INITIAL.checked_next().unwrap(),
                failure,
            })
            .unwrap();
        assert_projection(&terminal, label);
    }

    let (mut browser_cancelled, _, _) = fixture();
    open_browser(&mut browser_cancelled);
    browser_cancelled
        .apply_semantic_action(SemanticAction::Navigate(Direction::Down))
        .unwrap();
    browser_cancelled
        .apply_semantic_action(SemanticAction::Activate)
        .unwrap();
    assert_eq!(
        browser_cancelled.file_browser().lifecycle(),
        SampleAssetLifecycle::Cancelled
    );
    let (_, _, _, cancelled_shell, _) = StateProjector::new()
        .project_with_shell(&browser_cancelled)
        .unwrap();
    assert_eq!(
        waveform_status(&cancelled_shell),
        "CANCELLED — UNCHANGED",
        "browser cancellation remains explicit after exact return to Detail"
    );
}

fn external_file_listing() -> FileBrowserListing {
    FileBrowserListing::new(
        FileBrowserFolderId::default(),
        vec![
            FileBrowserRow::new(
                "file:Alternate.wav",
                "External.wav",
                FileBrowserRowKind::File(AssetFileId::new("@home/External.wav").unwrap()),
                Some(128),
            )
            .unwrap(),
            FileBrowserRow::new(
                "cancel:",
                "CANCEL — UNCHANGED",
                FileBrowserRowKind::Cancel,
                None,
            )
            .unwrap(),
        ],
    )
    .unwrap()
}

#[test]
fn keyboard_confirm_and_back_use_the_shared_file_page_and_preserve_the_asset() {
    use crest_synth::shell::{KeyboardInputTranslator, WindowInput, WindowKey};
    let (mut state, _, _) = fixture();
    let saved = crest_synth::control::SavedSession::capture(&state);
    let mut keys = KeyboardInputTranslator::default();
    for input in [
        WindowInput::key_down(WindowKey::Return),
        WindowInput::key_up(WindowKey::Return),
    ] {
        if let Some(action) = keys.translate(input) {
            state.apply_semantic_action(action).unwrap();
        }
    }
    let origin = state.interaction().focus_path().clone();
    for input in [
        WindowInput::key_down(WindowKey::Return),
        WindowInput::key_up(WindowKey::Return),
        WindowInput::key_down(WindowKey::Shift),
        WindowInput::key_down(WindowKey::S),
        WindowInput::key_up(WindowKey::S),
        WindowInput::key_up(WindowKey::Shift),
    ] {
        if let Some(action) = keys.translate(input) {
            state.apply_semantic_action(action).unwrap();
        }
    }
    assert_eq!(state.interaction().focus_path(), &origin);
    assert_eq!(crest_synth::control::SavedSession::capture(&state), saved);
    assert!(state.file_browser().import_request().is_none());
}

#[test]
fn in_app_file_import_failure_selection_and_stale_completion_are_correlated() {
    use crest_synth::control::{AssetImportResult, SavedSession};
    use crest_synth::synth::SampleAssetError;
    for result in [
        Err(SampleAssetError::UnsupportedContainer),
        Err(SampleAssetError::DownloadRequired),
        Ok(AssetFileId::new("Alternate.wav").unwrap()),
    ] {
        let (mut state, _, _) = fixture();
        state
            .apply_semantic_action(SemanticAction::OpenRelated)
            .unwrap();
        let origin = state.interaction().focus_path().clone();
        let saved = SavedSession::capture(&state);
        state
            .apply_semantic_action(SemanticAction::Activate)
            .unwrap();
        assert_eq!(state.interaction().active_surface(), SurfaceId::FileBrowser);
        state
            .apply(AppEvent::FileCatalogRefreshed {
                asset_kind: crest_synth::synth::AssetKind::Sample,
                folder: FileBrowserFolderId::default(),
                listing: Ok(external_file_listing()),
            })
            .unwrap();
        state
            .apply_semantic_action(SemanticAction::Activate)
            .unwrap();
        let request = state.file_browser().import_request().unwrap().clone();
        assert!(state
            .apply_semantic_action(SemanticAction::Activate)
            .is_err());
        let selection = AssetImportResult {
            descriptor: None,
            request,
            result: result.clone(),
        };
        let outcome = state
            .apply(AppEvent::AssetImported(selection.clone()))
            .unwrap();
        assert_eq!(state.interaction().focus_path(), &origin);
        assert_eq!(
            SavedSession::capture(&state),
            saved,
            "selection never commits before audio activation"
        );
        assert_eq!(outcome.engine_selection_effect().is_some(), result.is_ok());
        assert!(state.file_browser().import_request().is_none());
        assert!(
            state.apply(AppEvent::AssetImported(selection)).is_err(),
            "duplicate completion cannot assign twice"
        );
        if let Err(cause) = result {
            assert_eq!(state.file_browser().file_selection_failure(), Some(cause));
            if cause == SampleAssetError::DownloadRequired {
                assert_eq!(
                    state.file_browser().lifecycle(),
                    SampleAssetLifecycle::Unavailable
                );
                let shell = StateProjector::new().project_with_shell(&state).unwrap().3;
                let detail = shell
                    .semantic_model()
                    .surface(SurfaceId::PatchDetail)
                    .unwrap();
                assert!(detail.visualizations().iter().any(|view| matches!(
                    view.data(), SemanticVisualizationData::Waveform { status, .. }
                        if status.starts_with("UNAVAILABLE")
                            && status.contains("Make available offline")
                            && !status.contains("INVALID")
                )));
            }
        }
    }
}

#[test]
fn cloud_file_metadata_names_download_action_and_refresh_recovers_without_moving_focus() {
    use crest_synth::synth::SampleAssetError;
    let (mut state, _, _) = fixture();
    open_browser(&mut state);
    let focus = state.interaction().focus_path().clone();
    let saved = crest_synth::control::SavedSession::capture(&state);
    let original = state.file_browser().rows().to_vec();
    let unavailable = original
        .iter()
        .cloned()
        .map(|row| {
            if matches!(row.kind(), FileBrowserRowKind::File(_)) {
                row.with_metadata(Err(SampleAssetError::DownloadRequired))
                    .unwrap()
            } else {
                row
            }
        })
        .collect();
    for (rows, expected) in [
        (unavailable, SemanticBrowserMetadataStatus::Unavailable),
        (original, SemanticBrowserMetadataStatus::Ready),
    ] {
        let folder = FileBrowserFolderId::default();
        state
            .apply(AppEvent::FileCatalogRefreshed {
                asset_kind: crest_synth::synth::AssetKind::Sample,
                folder: folder.clone(),
                listing: Ok(FileBrowserListing::new(folder, rows).unwrap()),
            })
            .unwrap();
        assert_eq!(state.interaction().focus_path(), &focus);
        assert_eq!(crest_synth::control::SavedSession::capture(&state), saved);
        let shell = StateProjector::new().project_with_shell(&state).unwrap().3;
        let metadata = shell
            .semantic_model()
            .surface(SurfaceId::FileBrowser)
            .unwrap()
            .controls()[0]
            .browser_metadata()
            .unwrap();
        assert_eq!(metadata.status(), expected);
        if expected == SemanticBrowserMetadataStatus::Unavailable {
            assert!(metadata.text().contains("MAKE AVAILABLE OFFLINE"));
            assert!(!metadata.text().contains("INVALID"));
            let wire = serde_json::to_value(metadata).unwrap();
            assert_eq!(wire["cause"], "downloadRequired");
            assert_eq!(wire["status"], "unavailable");
        }
    }
}

#[test]
fn saved_sample_restore_recovers_waveform_and_edited_landmarks() {
    use crest_synth::control::SavedSession;
    let (mut state, _, _) = fixture();
    state
        .apply_semantic_action(SemanticAction::OpenRelated)
        .unwrap();
    for _ in 0..2 {
        state
            .apply_semantic_action(SemanticAction::Navigate(Direction::Down))
            .unwrap();
    }
    state
        .apply_semantic_action(SemanticAction::SetInteractionMode(InteractionMode::Adjust))
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::Adjust(Direction::Right))
        .unwrap();
    let saved = SavedSession::capture(&state);
    let asset = AssetFileId::new("Factory.wav").unwrap();
    let catalog = Arc::new(DeterministicSampleCatalog::new(
        [],
        [(asset.clone(), Ok(vec![1]))],
    ));
    let decoder = Arc::new(DeterministicSampleDecoder::new([(
        asset,
        Ok(decoded_fixture("Factory.wav")),
    )]));
    let preparers: Vec<Box<dyn InstrumentPreparer>> =
        vec![Box::new(SamplePreparer::new(catalog, decoder).unwrap())];
    let prepared = saved
        .prepare_restore(
            state.capabilities().clone(),
            state.effects().clone(),
            &preparers,
            &[],
            GraphRevision::INITIAL.checked_next().unwrap(),
            48_000.0,
            64,
        )
        .unwrap();
    let (replacement, _graph) = prepared.into_replacement();
    // An in-app selection import may finish after Open replaces the session.
    state
        .apply_semantic_action(SemanticAction::SetInteractionMode(
            InteractionMode::Navigate,
        ))
        .unwrap();
    for _ in 0..2 {
        state
            .apply_semantic_action(SemanticAction::Navigate(Direction::Up))
            .unwrap();
    }
    state
        .apply_semantic_action(SemanticAction::Activate)
        .unwrap();
    state
        .apply(AppEvent::FileCatalogRefreshed {
            asset_kind: crest_synth::synth::AssetKind::Sample,
            folder: FileBrowserFolderId::default(),
            listing: Ok(external_file_listing()),
        })
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::Activate)
        .unwrap();
    let obsolete_import = state.file_browser().import_request().unwrap().clone();
    state
        .apply(AppEvent::ReplacePersistedSession(Box::new(replacement)))
        .unwrap();
    assert!(state
        .apply(AppEvent::AssetImported(
            crest_synth::control::AssetImportResult {
                descriptor: None,
                request: obsolete_import,
                result: Ok(AssetFileId::new("Alternate.wav").unwrap()),
            }
        ))
        .is_err());
    assert_eq!(SavedSession::capture(&state), saved);
    state
        .apply_semantic_action(SemanticAction::SelectContext(
            crest_synth::control::TopLevelContext::Patch,
        ))
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::OpenRelated)
        .unwrap();
    let shell = StateProjector::new().project_with_shell(&state).unwrap().3;
    let document = serde_json::to_value(shell.semantic_model()).unwrap();
    let waveform = document["surfaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|surface| surface["id"] == "patchDetail")
        .unwrap()["visualizations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|view| view["data"]["kind"] == "waveform")
        .unwrap();
    assert_eq!(waveform["data"]["asset"]["locator"], "Factory.wav");
    assert!(!waveform["data"]["pairs"].as_array().unwrap().is_empty());
    assert!(waveform["data"]["landmarks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|landmark| landmark["role"] == "playbackStart"
            && landmark["normalizedPosition"].as_f64().unwrap() > 0.0));
}

#[test]
fn engine_choice_keeps_prepared_waveform_after_sample_activation() {
    use crest_synth::adapter::braids_capability::BraidsCapability;
    use crest_synth::adapter::braids_preparer::BraidsPreparer;

    let source = BraidsCapability::new().unwrap();
    let asset = AssetFileId::new("Factory.wav").unwrap();
    let sample = SampleCapability::new(asset.clone()).unwrap();
    let registry = CapabilityRegistry::new(vec![source.descriptor(), sample.descriptor()]).unwrap();
    let mut state = AppState::for_graph(
        registry.clone(),
        GlobalParameters::new(0.0).unwrap(),
        GraphRevision::INITIAL,
    );
    state
        .apply(AppEvent::InstallPatches(vec![Patch::new(
            PatchId::new(1).unwrap(),
            "Engine choice".to_owned(),
            source.default_config().unwrap(),
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        )]))
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::SelectContext(
            crest_synth::control::TopLevelContext::Patch,
        ))
        .unwrap();

    let transport = ParameterSnapshot::new(
        0,
        GlobalParameters::new(0.0).unwrap(),
        MixerState::default(),
        &[],
    )
    .unwrap();
    let (audio_control, audio_handle) = LockFreeAudioBoundary::new(64, transport).into_handles();
    let mut app_loop = AppLoop::new(
        state,
        StateProjector::for_graph(GraphRevision::INITIAL),
        audio_control,
    )
    .unwrap();
    let audio_config = AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 64).unwrap();
    let initial_preparers: Vec<Box<dyn InstrumentPreparer>> =
        vec![Box::new(BraidsPreparer::new().unwrap())];
    let initial_graph = PreparedGraphBuilder::new(&registry, &initial_preparers)
        .build(
            GraphRevision::INITIAL,
            app_loop.patches(),
            app_loop.current_parameters().clone(),
            audio_config.sample_rate(),
            audio_config.render_capacity_frames(),
        )
        .unwrap();
    let (structural_control, structural_audio) = LockFreeStructuralGraphBoundary::new(
        1,
        1,
        GraphHandoffStatus::with_active(GraphRevision::INITIAL),
    )
    .unwrap()
    .into_handles();
    let catalog = Arc::new(DeterministicSampleCatalog::new(
        [],
        [(asset.clone(), Ok(vec![1]))],
    ));
    let decoder = Arc::new(DeterministicSampleDecoder::new([(
        asset.clone(),
        Ok(decoded_fixture(asset.as_str())),
    )]));
    let worker = DeterministicGraphPreparationWorker::new(
        registry.clone(),
        vec![
            Box::new(BraidsPreparer::new().unwrap()),
            Box::new(SamplePreparer::new(catalog, decoder).unwrap()),
        ],
        audio_config,
    );
    let worker_handle = worker.advance_handle();
    app_loop
        .configure_engine_selection(
            DescriptorDefaultConfigFactory::new(registry, vec![Box::new(source), Box::new(sample)]),
            worker,
            structural_control,
            &initial_graph,
            audio_config,
        )
        .unwrap();
    let mut renderer = AudioRenderer::new(audio_handle, structural_audio, initial_graph);

    // The normal Engine options journey, without a browser assignment to seed the cache.
    app_loop
        .dispatch_action(SemanticAction::SetInteractionMode(InteractionMode::Adjust))
        .unwrap();
    app_loop
        .dispatch_action(SemanticAction::Adjust(Direction::Up))
        .unwrap();
    app_loop
        .dispatch_action(SemanticAction::Navigate(Direction::Right))
        .unwrap();
    // Sample is in the Samplers category, separate from Braids' Synths.
    assert_eq!(
        app_loop.current_semantic_model().focus_path().control_id(),
        &SemanticControlId::Modal(ModalControlId::Choice(SAMPLE_CAPABILITY_ID.to_owned()))
    );
    app_loop.dispatch_action(SemanticAction::Activate).unwrap();
    for lifecycle in [
        EngineSelectionStatusKind::Validating,
        EngineSelectionStatusKind::Preparing,
    ] {
        assert_eq!(
            app_loop
                .advance_structural()
                .unwrap()
                .engine_selection_lifecycle_advanced(),
            Some(lifecycle)
        );
    }
    assert!(worker_handle.advance());
    let target = GraphRevision::INITIAL.checked_next().unwrap();
    assert_eq!(
        app_loop.advance_structural().unwrap().graph_published(),
        Some(target)
    );
    assert_ne!(
        app_loop.patches()[0]
            .instrument_config()
            .capability_id()
            .as_str(),
        SAMPLE_CAPABILITY_ID
    );
    renderer.render(&mut [0.0_f32; 128]);
    assert_eq!(
        app_loop
            .advance_structural()
            .unwrap()
            .activation_acknowledged(),
        Some(target)
    );
    assert_eq!(
        app_loop.patches()[0]
            .instrument_config()
            .capability_id()
            .as_str(),
        SAMPLE_CAPABILITY_ID
    );
    app_loop
        .dispatch_action(SemanticAction::OpenRelated)
        .unwrap();
    let shell = app_loop.current_graphical_shell();
    let detail = shell
        .semantic_model()
        .surfaces()
        .iter()
        .find(|surface| surface.id() == SurfaceId::PatchDetail)
        .unwrap();
    let waveform = detail
        .visualizations()
        .iter()
        .find(|visualization| {
            matches!(
                visualization.data(),
                SemanticVisualizationData::Waveform { .. }
            )
        })
        .unwrap();
    match waveform.data() {
        SemanticVisualizationData::Waveform {
            asset: Some(active),
            pairs,
            status,
            ..
        } => {
            assert_eq!(active.locator(), asset.as_str());
            assert!(
                !pairs.is_empty(),
                "Engine selection must retain the worker's decoded waveform"
            );
            assert_eq!(status, "READY");
        }
        _ => panic!("Sample Detail must project the acknowledged asset waveform"),
    }
    let file = detail
        .controls()
        .iter()
        .find(|control| control.kind() == SemanticControlKind::Asset)
        .unwrap();
    assert!(file
        .valid_actions()
        .iter()
        .any(|action| action.action() == &SemanticAction::OpenRelated));
    drop(renderer);
    app_loop.shutdown_engine_selection_on_control().unwrap();
}

#[test]
fn coordinator_advances_assignment_through_activation_before_committing_the_asset() {
    let (state, provider, patch_id) = fixture();
    let factory_asset = AssetFileId::new("Factory.wav").unwrap();
    let alternate_asset = AssetFileId::new("Alternate.wav").unwrap();
    let catalog = Arc::new(DeterministicSampleCatalog::new(
        [],
        [
            (factory_asset.clone(), Ok(vec![1])),
            (alternate_asset.clone(), Ok(vec![1])),
        ],
    ));
    let decoder = Arc::new(DeterministicSampleDecoder::new([
        (
            factory_asset.clone(),
            Ok(decoded_fixture(factory_asset.as_str())),
        ),
        (
            alternate_asset.clone(),
            Ok(decoded_fixture(alternate_asset.as_str())),
        ),
    ]));
    let registry = state.capabilities().clone();
    let factory = DescriptorDefaultConfigFactory::new(registry.clone(), vec![Box::new(provider)]);
    let blueprint = crest_synth::control::PatchCreationBlueprint::resolve(
        &crest_synth::synth::CapabilityId::new(SAMPLE_CAPABILITY_ID).unwrap(),
        &factory,
    )
    .unwrap();
    let state = state.with_patch_creation_blueprint(blueprint);

    let audio_config = AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 64).unwrap();
    let initial_transport = ParameterSnapshot::new(
        0,
        GlobalParameters::new(0.0).unwrap(),
        MixerState::default(),
        &[],
    )
    .unwrap();
    let audio_boundary = LockFreeAudioBoundary::new(64, initial_transport);
    let (audio_control, audio_handle) = audio_boundary.into_handles();
    let mut app_loop = AppLoop::new(
        state,
        StateProjector::for_graph(GraphRevision::INITIAL),
        audio_control,
    )
    .unwrap();
    let initial_preparers: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(
        SamplePreparer::new(catalog.clone(), decoder.clone()).unwrap(),
    )];
    let initial_graph = PreparedGraphBuilder::new(&registry, &initial_preparers)
        .build(
            GraphRevision::INITIAL,
            app_loop.patches(),
            app_loop.current_parameters().clone(),
            audio_config.sample_rate(),
            audio_config.render_capacity_frames(),
        )
        .unwrap();
    let structural = LockFreeStructuralGraphBoundary::new(
        1,
        1,
        GraphHandoffStatus::with_active(GraphRevision::INITIAL),
    )
    .unwrap();
    let (structural_control, structural_audio) = structural.into_handles();
    let worker_preparers: Vec<Box<dyn InstrumentPreparer>> =
        vec![Box::new(SamplePreparer::new(catalog, decoder).unwrap())];
    let worker =
        DeterministicGraphPreparationWorker::new(registry.clone(), worker_preparers, audio_config);
    let worker_handle = worker.advance_handle();
    app_loop
        .configure_engine_selection(
            factory,
            worker,
            structural_control,
            &initial_graph,
            audio_config,
        )
        .unwrap();
    let mut renderer = AudioRenderer::new(audio_handle, structural_audio, initial_graph);

    app_loop
        .dispatch_action(SemanticAction::OpenRelated)
        .unwrap();
    app_loop
        .dispatch_action(SemanticAction::OpenRelated)
        .unwrap();
    app_loop.dispatch_action(SemanticAction::Activate).unwrap();
    assert!(!worker_handle.is_pending());
    assert_eq!(
        waveform_status(&app_loop.current_graphical_shell()),
        "LOADING"
    );

    let validating = app_loop.advance_structural().unwrap();
    assert_eq!(
        validating.sample_asset_lifecycle_advanced(),
        Some(SampleAssetLifecycle::Validating)
    );
    assert!(!validating.worker_result_polled());
    let preparing = app_loop.advance_structural().unwrap();
    assert_eq!(
        preparing.sample_asset_lifecycle_advanced(),
        Some(SampleAssetLifecycle::Preparing)
    );
    assert!(!preparing.worker_result_polled());
    assert_eq!(
        app_loop.patches()[0]
            .instrument_config()
            .asset_reference(&ParameterId::new(SAMPLE_ASSET_PARAMETER_ID).unwrap())
            .map(AssetReference::locator),
        Some("Factory.wav")
    );

    let validating = app_loop.advance_structural().unwrap();
    assert_eq!(
        validating.engine_selection_lifecycle_advanced(),
        Some(EngineSelectionStatusKind::Validating)
    );
    assert!(!validating.worker_result_polled());
    let preparing = app_loop.advance_structural().unwrap();
    assert_eq!(
        preparing.engine_selection_lifecycle_advanced(),
        Some(EngineSelectionStatusKind::Preparing)
    );
    assert!(!preparing.worker_result_polled());
    assert!(worker_handle.is_pending());
    assert!(worker_handle.advance());

    let staged = app_loop.advance_structural().unwrap();
    let target_revision = GraphRevision::INITIAL.checked_next().unwrap();
    assert!(staged.worker_result_polled());
    assert_eq!(staged.graph_published(), Some(target_revision));
    assert_eq!(
        waveform_status(&app_loop.current_graphical_shell()),
        "ACTIVATING"
    );
    assert_eq!(
        app_loop.patches()[0]
            .instrument_config()
            .asset_reference(&ParameterId::new(SAMPLE_ASSET_PARAMETER_ID).unwrap())
            .map(AssetReference::locator),
        Some("Factory.wav"),
        "staging and publication do not commit the requested asset"
    );

    let mut output = [0.0_f32; 128];
    renderer.render(&mut output);
    assert_eq!(renderer.active_revision(), target_revision);
    let acknowledged = app_loop.advance_structural().unwrap();
    assert_eq!(
        acknowledged.activation_acknowledged(),
        Some(target_revision)
    );
    assert_eq!(
        waveform_status(&app_loop.current_graphical_shell()),
        "READY"
    );
    assert_eq!(
        app_loop.patches()[0]
            .instrument_config()
            .asset_reference(&ParameterId::new(SAMPLE_ASSET_PARAMETER_ID).unwrap())
            .map(AssetReference::locator),
        Some("Alternate.wav"),
        "only the block-boundary activation acknowledgement commits the asset"
    );
    assert_eq!(app_loop.patches()[0].id(), patch_id);

    // Creating another Sample must hydrate its own waveform through the same
    // prepared topology acknowledgement, without first reassigning its file.
    app_loop.dispatch_action(SemanticAction::Return).unwrap();
    app_loop
        .dispatch_action(SemanticAction::SelectPatch(Direction::Right))
        .unwrap();
    app_loop
        .dispatch_action(SemanticAction::SetInteractionMode(InteractionMode::Adjust))
        .unwrap();
    app_loop
        .dispatch_action(SemanticAction::Adjust(Direction::Up))
        .unwrap();
    app_loop.dispatch_action(SemanticAction::Activate).unwrap();
    for _ in 0..2 {
        app_loop.advance_structural().unwrap();
    }
    assert!(worker_handle.advance());
    app_loop.advance_structural().unwrap();
    renderer.render(&mut output);
    app_loop.advance_structural().unwrap();
    assert_eq!(app_loop.patches().len(), 2);
    app_loop.dispatch_action(SemanticAction::Return).unwrap();
    app_loop
        .dispatch_action(SemanticAction::OpenRelated)
        .unwrap();
    let shell = app_loop.current_graphical_shell();
    let document = serde_json::to_value(shell.semantic_model()).unwrap();
    assert!(
        document["surfaces"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|surface| surface["visualizations"].as_array().into_iter().flatten())
            .any(|view| view["data"]["kind"] == "waveform"
                && view["data"]["asset"]["locator"] == "Factory.wav"
                && !view["data"]["pairs"].as_array().unwrap().is_empty()),
        "new Sample waveform survives topology activation"
    );

    drop(renderer);
    app_loop.shutdown_engine_selection_on_control().unwrap();
}

#[test]
fn detail_numeric_and_choice_edits_use_the_reducer_and_return_to_exact_rows() {
    let (mut state, _, _) = fixture();
    state
        .apply_semantic_action(SemanticAction::OpenRelated)
        .unwrap();
    // Asset -> root note.
    state
        .apply_semantic_action(SemanticAction::Navigate(Direction::Down))
        .unwrap();
    assert_eq!(
        state.interaction().focus_path().control_id(),
        &SemanticControlId::Patch(crest_synth::control::PatchControlId::Capability(
            ParameterId::new(SAMPLE_ROOT_NOTE_PARAMETER_ID).unwrap()
        ))
    );
    state
        .apply_semantic_action(SemanticAction::SetInteractionMode(InteractionMode::Adjust))
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::Adjust(Direction::Right))
        .unwrap();
    assert_eq!(
        state.patches()[0]
            .instrument_config()
            .value(&ParameterId::new(SAMPLE_ROOT_NOTE_PARAMETER_ID).unwrap()),
        Some(&ParameterValue::continuous(60.01).unwrap())
    );
    state
        .apply_semantic_action(SemanticAction::SetInteractionMode(
            InteractionMode::Navigate,
        ))
        .unwrap();
    for _ in 0..3 {
        state
            .apply_semantic_action(SemanticAction::Navigate(Direction::Down))
            .unwrap();
    }
    let loop_origin = state.interaction().focus_path().clone();
    assert!(matches!(
        loop_origin.control_id(),
        SemanticControlId::Patch(crest_synth::control::PatchControlId::Capability(id))
            if id.as_str() == SAMPLE_LOOP_MODE_PARAMETER_ID
    ));
    state
        .apply_semantic_action(SemanticAction::SetInteractionMode(InteractionMode::Adjust))
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::Adjust(Direction::Up))
        .unwrap();
    assert_eq!(state.interaction().active_surface(), SurfaceId::PatchChoice);
    state
        .apply_semantic_action(SemanticAction::Navigate(Direction::Down))
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::Activate)
        .unwrap();
    assert_eq!(state.interaction().focus_path(), &loop_origin);
    assert_eq!(
        state.patches()[0]
            .instrument_config()
            .value(&ParameterId::new(SAMPLE_LOOP_MODE_PARAMETER_ID).unwrap()),
        Some(&ParameterValue::Choice(
            SAMPLE_LOOP_FORWARD_CHOICE_ID.to_owned()
        ))
    );
}

#[test]
fn sample_detail_and_browser_project_generic_sections_rows_status_and_visualizations() {
    let (mut state, provider, patch_id) = fixture();
    state
        .apply_semantic_action(SemanticAction::OpenRelated)
        .unwrap();
    let (_, _, _, detail_shell, _) = StateProjector::new().project_with_shell(&state).unwrap();
    let detail = detail_shell
        .semantic_model()
        .surface(SurfaceId::PatchDetail)
        .unwrap();
    assert_eq!(
        detail
            .sections()
            .iter()
            .map(|section| section.id())
            .collect::<Vec<_>>(),
        ["sample.playback", "sample.loop", "shared.envelope"]
    );
    assert_eq!(
        detail
            .controls()
            .iter()
            .map(|control| control.label())
            .collect::<Vec<_>>(),
        provider
            .descriptor()
            .parameters()
            .map(|parameter| parameter.label())
            .chain(
                crest_synth::synth::VoiceEnvelope::surface_descriptor()
                    .iter()
                    .map(|parameter| parameter.label()),
            )
            .collect::<Vec<_>>()
    );
    for dependency in [
        crest_synth::adapter::sample_capability::SAMPLE_LOOP_START_PARAMETER_ID,
        crest_synth::adapter::sample_capability::SAMPLE_LOOP_END_PARAMETER_ID,
        crest_synth::adapter::sample_capability::SAMPLE_CROSSFADE_PARAMETER_ID,
    ] {
        assert!(!detail
            .controls()
            .iter()
            .find(|control| {
                matches!(
                    control.path().control_id(),
                    SemanticControlId::Patch(crest_synth::control::PatchControlId::Capability(id))
                        if id.as_str() == dependency
                )
            })
            .unwrap()
            .enabled());
    }
    assert_eq!(
        detail
            .visualizations()
            .iter()
            .map(|visualization| visualization.id())
            .collect::<Vec<_>>(),
        ["shared.envelope", "sample.waveform", "sample.status"]
    );
    assert!(detail
        .visualizations()
        .iter()
        .all(|visualization| !visualization.focusable()));
    assert!(matches!(
        detail.visualizations()[0].data(),
        SemanticVisualizationData::Envelope { .. }
    ));
    assert!(matches!(
        detail.visualizations()[1].data(),
        SemanticVisualizationData::Waveform { asset: Some(asset), status, .. }
            if asset.locator() == "Factory.wav" && status == "WAVEFORM UNAVAILABLE"
    ));

    state
        .apply_semantic_action(SemanticAction::OpenRelated)
        .unwrap();
    let (_, _, _, browser_shell, _) = StateProjector::new().project_with_shell(&state).unwrap();
    let browser = browser_shell
        .semantic_model()
        .surface(SurfaceId::FileBrowser)
        .unwrap();
    assert_eq!(
        browser
            .controls()
            .iter()
            .map(|control| control.kind())
            .collect::<Vec<_>>(),
        [
            SemanticControlKind::BrowserFile,
            SemanticControlKind::BrowserCancel
        ]
    );
    assert_eq!(
        browser
            .controls()
            .iter()
            .filter(|row| row.focused())
            .count(),
        1
    );
    assert_eq!(browser.visualizations().len(), 1);
    assert!(!browser.visualizations()[0].focusable());
    let metadata = browser.controls()[0]
        .browser_metadata()
        .expect("the catalog projects typed metadata on a file row");
    assert_eq!(metadata.status(), SemanticBrowserMetadataStatus::Ready);
    assert_eq!(metadata.sample_rate(), Some(48_000));
    assert_eq!(metadata.channels(), Some(2));
    assert_eq!(metadata.duration_milliseconds(), Some(1_000));
    assert!(metadata.text().contains("24-BIT SIGNED PCM"));
    assert!(metadata.text().contains("STEREO"));
    assert!(browser.controls()[1].browser_metadata().is_none());
    let SemanticSurfaceSummary::FileBrowser {
        patch_id: projected_patch,
        active_asset,
        requested_asset,
        lifecycle,
        ..
    } = browser.summary()
    else {
        panic!("the browser carries its typed summary");
    };
    assert_eq!(*projected_patch, Some(patch_id));
    assert_eq!(
        active_asset.as_ref().map(AssetReference::locator),
        Some("Factory.wav")
    );
    assert!(requested_asset.is_none());
    assert_eq!(*lifecycle, SampleAssetLifecycle::Ready);
}
