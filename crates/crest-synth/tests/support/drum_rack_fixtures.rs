use crest_synth::adapter::drum_rack_capability::{
    pad_parameter_id, DrumRackCapability, DRUM_RACK_PAD_PARAMETER_ID,
};
use crest_synth::adapter::sample_capability::SAMPLE_ASSET_PARAMETER_ID;
use crest_synth::adapter::{
    drum_rack_preparer::DrumRackPreparer, filesystem_sample_catalog::FilesystemSampleCatalog,
    wav_sample_decoder::WavSampleDecoder,
};
use crest_synth::control::{
    AppEvent, AppState, Direction, InteractionMode, SavedSession, SemanticAction, TopLevelContext,
};
use crest_synth::kernel::{MidiChannel, PatchId};
use crest_synth::mixer::{global_parameters::GlobalParameters, patch_output::PatchOutput};
use crest_synth::real_time::GraphRevision;
use crest_synth::synth::{
    AssetFileId, AssetKind, AssetReference, CapabilityRegistry, InstrumentCapabilityProvider,
    InstrumentPreparer, ParameterId, Patch,
};
use std::sync::Arc;

pub fn states() -> Vec<(&'static str, AppState)> {
    let provider = DrumRackCapability::new(AssetFileId::new("sample-test.wav").unwrap()).unwrap();
    let registry = CapabilityRegistry::new(vec![provider.descriptor()]).unwrap();
    let mut state = AppState::for_graph(
        registry.clone(),
        GlobalParameters::new(0.0).unwrap(),
        GraphRevision::INITIAL,
    );
    let patch_id = PatchId::new(1).unwrap();
    state
        .apply(AppEvent::InstallPatches(vec![Patch::new(
            patch_id,
            "Drum Rack".into(),
            provider.default_config().unwrap(),
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        )]))
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::SelectContext(TopLevelContext::Patch))
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::OpenRelated)
        .unwrap();
    let mut result = vec![("patch-drum-rack-empty", state.clone())];
    let mut config = provider.default_config().unwrap();
    for pad in [0, 6] {
        config = registry
            .replace_asset(
                &config,
                &pad_parameter_id(pad, SAMPLE_ASSET_PARAMETER_ID),
                AssetReference::new(AssetKind::Sample, "sample-test.wav").unwrap(),
            )
            .unwrap();
    }
    let mut loaded = AppState::for_graph(
        registry.clone(),
        GlobalParameters::new(0.0).unwrap(),
        GraphRevision::INITIAL,
    );
    loaded
        .apply(AppEvent::InstallPatches(vec![Patch::new(
            patch_id,
            "Drum Rack".into(),
            config,
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        )]))
        .unwrap();
    let catalog = Arc::new(
        FilesystemSampleCatalog::new(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets"),
        )
        .unwrap(),
    );
    let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(
        DrumRackPreparer::new(
            AssetFileId::new("sample-test.wav").unwrap(),
            catalog,
            Arc::new(WavSampleDecoder),
        )
        .unwrap(),
    )];
    let prepared = SavedSession::capture(&loaded)
        .prepare_restore(
            registry,
            loaded.effects().clone(),
            &preparers,
            &[],
            GraphRevision::INITIAL.checked_next().unwrap(),
            48_000.0,
            64,
        )
        .unwrap();
    let (replacement, _graph) = prepared.into_replacement();
    state
        .apply(AppEvent::ReplacePersistedSession(Box::new(replacement)))
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::SelectContext(TopLevelContext::Patch))
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::OpenRelated)
        .unwrap();
    result.push(("patch-drum-rack-kick", state.clone()));
    assert_eq!(
        DrumRackCapability::selected_pad(state.patches()[0].instrument_config()),
        Some(0)
    );
    state
        .apply_semantic_action(SemanticAction::SetInteractionMode(InteractionMode::Adjust))
        .unwrap();
    for _ in 0..6 {
        state
            .apply_semantic_action(SemanticAction::Adjust(Direction::Right))
            .unwrap();
    }
    state
        .apply_semantic_action(SemanticAction::SetInteractionMode(
            InteractionMode::Navigate,
        ))
        .unwrap();
    assert_eq!(
        state.patches()[0]
            .instrument_config()
            .value(&ParameterId::new(DRUM_RACK_PAD_PARAMETER_ID).unwrap()),
        Some(&crest_synth::synth::ParameterValue::Choice("pad-6".into()))
    );
    result.push(("patch-drum-rack-hat", state.clone()));
    state
        .apply_semantic_action(SemanticAction::SetInteractionMode(InteractionMode::Adjust))
        .unwrap();
    state
        .apply_semantic_action(SemanticAction::Adjust(Direction::Up))
        .unwrap();
    result.push(("patch-drum-rack-pad-menu", state));
    result
}
