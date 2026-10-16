#[path = "support/soundfont_fixture.rs"]
mod soundfont_fixture;

use crest_synth::adapter::{
    filesystem_soundfont_catalog::FilesystemSoundFontCatalog,
    hidef_soundfont_asset::HiDefSoundFontAsset,
    hidef_soundfont_capability::{
        HiDefSoundFontCapability, HIDEF_SOUNDFONT_PATH, SOUNDFONT_FILE_PARAMETER_ID,
        SOUNDFONT_PRESET_PARAMETER_ID,
    },
    hidef_soundfont_preparer::HiDefSoundFontPreparer,
};
use crest_synth::control::*;
use crest_synth::kernel::{MidiChannel, PatchId};
use crest_synth::mixer::{global_parameters::GlobalParameters, patch_output::PatchOutput};
use crest_synth::synth::*;
use std::sync::Arc;

struct Fixture {
    root: std::path::PathBuf,
    library: Arc<FilesystemSoundFontCatalog>,
    bundled: HiDefSoundFontAsset,
    provider: HiDefSoundFontCapability,
    registry: CapabilityRegistry,
}
impl Fixture {
    fn new() -> Self {
        static NEXT_FIXTURE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "crest-sf2-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("Library")).unwrap();
        std::fs::create_dir_all(root.join("External")).unwrap();
        let bundled =
            HiDefSoundFontAsset::from_bytes(&soundfont_fixture::bank("Default", 0, 160)).unwrap();
        let provider = HiDefSoundFontCapability::new(bundled.catalog()).unwrap();
        let registry = CapabilityRegistry::new(vec![provider.descriptor()]).unwrap();
        let library = Arc::new(
            FilesystemSoundFontCatalog::new(root.join("Library"), bundled.clone()).unwrap(),
        );
        Self {
            root,
            library,
            bundled,
            provider,
            registry,
        }
    }
    fn factory(&self, registry: CapabilityRegistry) -> DescriptorDefaultConfigFactory {
        DescriptorDefaultConfigFactory::new(registry, vec![Box::new(self.provider.clone())])
    }
    fn preparers(&self) -> Vec<Box<dyn InstrumentPreparer>> {
        vec![Box::new(
            HiDefSoundFontPreparer::new(&self.bundled)
                .unwrap()
                .with_library(self.library.clone()),
        )]
    }
    fn state(&self) -> AppState {
        let config = self
            .factory(self.registry.clone())
            .create(self.provider.descriptor().id())
            .unwrap();
        let mut state = AppState::new(self.registry.clone(), GlobalParameters::new(0.0).unwrap());
        state
            .apply(AppEvent::InstallPatches(
                (1..=2)
                    .map(|id| {
                        Patch::new(
                            PatchId::new(id).unwrap(),
                            format!("Patch {id}"),
                            config.clone(),
                            MidiChannel::new((id - 1) as u8).unwrap(),
                            PatchOutput::default(),
                        )
                    })
                    .collect(),
            ))
            .unwrap();
        state
            .apply_semantic_action(SemanticAction::SelectContext(TopLevelContext::Patch))
            .unwrap();
        state
    }
    fn external(&self, label: &str, bank: u16, period: usize) -> std::path::PathBuf {
        let path = self.root.join("External").join(format!("{label}.sf2"));
        std::fs::write(&path, soundfont_fixture::bank(label, bank, period)).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn selected_banks_validate_import_without_overwrite_and_keep_independent_catalogs() {
    let fixture = Fixture::new();
    let alpha = fixture.external("Alpha", 7, 120);
    let (first, descriptor) = fixture.library.import_file(&alpha).unwrap();
    assert_eq!(first.as_str(), "Imported/Alpha.sf2");
    assert_eq!(fixture.library.import_file(&alpha).unwrap().0, first);
    std::fs::write(&alpha, soundfont_fixture::bank("Beta", 9, 240)).unwrap();
    let (second, other) = fixture.library.import_file(&alpha).unwrap();
    assert_eq!(second.as_str(), "Imported/Alpha-1.sf2");
    assert_ne!(descriptor, other);
    let registry = fixture
        .registry
        .clone()
        .with_asset_descriptor(descriptor)
        .unwrap()
        .with_asset_descriptor(other)
        .unwrap();
    let source = fixture.state().patches()[0].instrument_config().clone();
    let file = ParameterId::new(SOUNDFONT_FILE_PARAMETER_ID).unwrap();
    let preset = ParameterId::new(SOUNDFONT_PRESET_PARAMETER_ID).unwrap();
    for (id, bank, label) in [(&first, 7, "Alpha"), (&second, 9, "Beta")] {
        let config = registry
            .replace_asset(
                &source,
                &file,
                AssetReference::new(AssetKind::SoundFont, id.as_str()).unwrap(),
            )
            .unwrap();
        assert_eq!(
            config.value(&preset),
            Some(&ParameterValue::Choice(
                SoundFontPresetId::new(bank, 3).unwrap().choice_id()
            ))
        );
        let descriptor = registry.descriptor_for_config(&config).unwrap();
        assert_eq!(
            descriptor
                .parameter(&preset)
                .unwrap()
                .choices()
                .iter()
                .map(|choice| choice.label().to_owned())
                .collect::<Vec<_>>(),
            [format!("{label} One"), format!("{label} Two")]
        );
    }
    std::fs::write(&alpha, b"invalid SF2").unwrap();
    assert_eq!(
        fixture.library.import_file(&alpha),
        Err(SampleAssetError::MalformedSoundFont)
    );
    std::fs::write(&alpha, []).unwrap();
    assert_eq!(
        fixture.library.import_file(&alpha),
        Err(SampleAssetError::EmptyFile)
    );
    let mut bad_length = soundfont_fixture::bank("Bad", 2, 120);
    bad_length[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(HiDefSoundFontAsset::from_bytes(&bad_length).is_err());
    assert_eq!(
        std::fs::read_dir(fixture.root.join("Library/Imported"))
            .unwrap()
            .count(),
        2
    );
}

use crest_synth::adapter::atomic_audio_observation::{
    AtomicAudioObservation, AtomicAudioObservationWriter,
};
use crest_synth::adapter::lock_free_audio_boundary::{
    LockFreeAudioBoundary, LockFreeAudioHandle, LockFreeControlHandle,
};
use crest_synth::adapter::lock_free_structural_graph_boundary::{
    LockFreeStructuralAudioHandle, LockFreeStructuralGraphBoundary,
};
use crest_synth::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crest_synth::real_time::{
    AudioBoundary, AudioObservation, AudioRenderer, GraphHandoffStatus, GraphRevision,
    ParameterSnapshot, PreparedGraphBuilder, StructuralGraphBoundary,
};
use crest_synth::shell::audio_output::{AudioDeviceConfig, AudioSampleFormat};
use crest_synth::testing::{
    DeterministicGraphPreparationHandle, DeterministicGraphPreparationWorker,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
thread_local! {
    static COUNT_CALLBACK_MEMORY: Cell<bool> = const { Cell::new(false) };
    static CALLBACK_ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static CALLBACK_DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct AcceptanceAllocator;

#[global_allocator]
static ACCEPTANCE_ALLOCATOR: AcceptanceAllocator = AcceptanceAllocator;

unsafe impl GlobalAlloc for AcceptanceAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        record_deallocation();
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record_allocation();
        record_deallocation();
        unsafe { System.realloc(pointer, layout, size) }
    }
}

fn record_allocation() {
    COUNT_CALLBACK_MEMORY.with(|enabled| {
        if enabled.get() {
            CALLBACK_ALLOCATIONS.with(|count| count.set(count.get().saturating_add(1)));
        }
    });
}

fn record_deallocation() {
    COUNT_CALLBACK_MEMORY.with(|enabled| {
        if enabled.get() {
            CALLBACK_DEALLOCATIONS.with(|count| count.set(count.get().saturating_add(1)));
        }
    });
}

fn counted_render<Boundary, Structural, Observation>(
    renderer: &mut AudioRenderer<Boundary, Structural, Observation>,
    output: &mut [f32],
) -> (usize, usize)
where
    Boundary: crest_synth::real_time::AudioThreadBoundary,
    Structural: crest_synth::real_time::AudioStructuralGraphBoundary,
    Observation: crest_synth::real_time::CallbackAudioObservation,
{
    CALLBACK_ALLOCATIONS.with(|count| count.set(0));
    CALLBACK_DEALLOCATIONS.with(|count| count.set(0));
    COUNT_CALLBACK_MEMORY.with(|enabled| enabled.set(true));
    renderer.render(output);
    COUNT_CALLBACK_MEMORY.with(|enabled| enabled.set(false));
    (
        CALLBACK_ALLOCATIONS.with(Cell::get),
        CALLBACK_DEALLOCATIONS.with(Cell::get),
    )
}

type TestRenderer =
    AudioRenderer<LockFreeAudioHandle, LockFreeStructuralAudioHandle, AtomicAudioObservationWriter>;
struct Rig {
    app: AppLoop<LockFreeControlHandle>,
    renderer: TestRenderer,
    worker: DeterministicGraphPreparationHandle,
}
impl Rig {
    fn new(fixture: &Fixture) -> Self {
        let state = fixture.state();
        let (control, callback) = LockFreeAudioBoundary::new(
            128,
            ParameterSnapshot::new(0, *state.global(), state.mixer().clone(), &[]).unwrap(),
        )
        .into_handles();
        let mut app = AppLoop::new(state, StateProjector::new(), control).unwrap();
        let preparers = fixture.preparers();
        let graph = PreparedGraphBuilder::new(&fixture.registry, &preparers)
            .build(
                GraphRevision::INITIAL,
                app.patches(),
                app.current_parameters().clone(),
                48_000.0,
                256,
            )
            .unwrap();
        let config = AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 256).unwrap();
        let (structural_control, structural_audio) = LockFreeStructuralGraphBoundary::new(
            1,
            1,
            GraphHandoffStatus::with_active(GraphRevision::INITIAL),
        )
        .unwrap()
        .into_handles();
        let worker = DeterministicGraphPreparationWorker::new(
            fixture.registry.clone(),
            fixture.preparers(),
            config,
        );
        let handle = worker.advance_handle();
        app.configure_engine_selection(
            fixture.factory(fixture.registry.clone()),
            worker,
            structural_control,
            &graph,
            config,
        )
        .unwrap();
        let (writer, _) = AtomicAudioObservation::default().into_handles();
        Self {
            app,
            renderer: AudioRenderer::with_observation(callback, structural_audio, graph, writer),
            worker: handle,
        }
    }
    fn render(&mut self) -> Vec<f32> {
        let mut samples = vec![0.0; 512];
        assert_eq!(counted_render(&mut self.renderer, &mut samples), (0, 0));
        assert!(samples.iter().all(|value| value.is_finite()));
        samples
    }
    fn finish(&mut self) {
        for _ in 0..12 {
            self.app.advance_structural().unwrap();
            self.worker.advance();
            self.render();
            if !self.app.engine_selection_status().is_in_flight() {
                return;
            }
        }
        panic!(
            "bank graph never acknowledged: {:?}",
            self.app.engine_selection_status()
        );
    }
    fn play(&mut self, id: u32) -> Vec<f32> {
        for patch in 1..=2 {
            self.app
                .dispatch(AppEvent::Midi {
                    patch_id: PatchId::new(patch).unwrap(),
                    message: MidiMessage::try_new(
                        MidiChannel::new((patch - 1) as u8).unwrap(),
                        MidiMessageKind::AllNotesOff,
                        0,
                        0,
                    )
                    .unwrap(),
                })
                .unwrap();
        }
        self.render();
        self.app
            .dispatch(AppEvent::Midi {
                patch_id: PatchId::new(id).unwrap(),
                message: MidiMessage::try_new(
                    MidiChannel::new((id - 1) as u8).unwrap(),
                    MidiMessageKind::NoteOn,
                    60,
                    110,
                )
                .unwrap(),
            })
            .unwrap();
        let samples = self.render();
        assert!(samples.iter().any(|value| value.abs() > 0.0001));
        samples
    }
    fn key(&mut self, key: crest_synth::shell::WindowKey) {
        use crest_synth::shell::{KeyboardInputTranslator, WindowInput};
        let mut keys = KeyboardInputTranslator::default();
        for input in [WindowInput::key_down(key), WindowInput::key_up(key)] {
            if let Some(action) = keys.translate(input) {
                let result = self.app.dispatch_action(action.clone());
                if action == SemanticAction::SetInteractionMode(InteractionMode::Navigate)
                    && self.app.current_semantic_model().active_surface() == SurfaceId::PatchChoice
                {
                    assert_eq!(
                        result.unwrap_err(),
                        EventRejection::ActionUnavailableInContext
                    );
                } else {
                    result.unwrap_or_else(|error| panic!("{action:?}: {error:?}"));
                }
            }
        }
    }
    fn chord(
        &mut self,
        modifier: crest_synth::shell::WindowKey,
        key: crest_synth::shell::WindowKey,
    ) {
        use crest_synth::shell::{KeyboardInputTranslator, WindowInput};
        let mut keys = KeyboardInputTranslator::default();
        for input in [
            WindowInput::key_down(modifier),
            WindowInput::key_down(key),
            WindowInput::key_up(key),
            WindowInput::key_up(modifier),
        ] {
            if let Some(action) = keys.translate(input) {
                let result = self.app.dispatch_action(action.clone());
                if action == SemanticAction::SetInteractionMode(InteractionMode::Navigate)
                    && self.app.current_semantic_model().active_surface() == SurfaceId::PatchChoice
                {
                    assert_eq!(
                        result.unwrap_err(),
                        EventRejection::ActionUnavailableInContext
                    );
                } else {
                    result.unwrap_or_else(|error| panic!("{action:?}: {error:?}"));
                }
            }
        }
    }
    fn capture(&self, name: &str) {
        if let Some(root) = std::env::var_os("CREST_SOUNDFONT_EVIDENCE_DIR") {
            let root = std::path::PathBuf::from(root);
            std::fs::create_dir_all(&root).unwrap();
            let shell = self.app.current_graphical_shell();
            std::fs::write(
                root.join(format!("{name}.json")),
                serde_json::to_vec(shell.semantic_model()).unwrap(),
            )
            .unwrap();
        }
    }
    fn select(&mut self, fixture: &Fixture, source: &std::path::Path) -> AssetImportResult {
        use crest_synth::shell::WindowKey;
        self.key(WindowKey::Return); // Overview to Detail, preset focus.
        self.key(WindowKey::S); // File.
        let origin = self.app.current_semantic_model().focus_path().clone();
        self.key(WindowKey::Return);
        assert_eq!(
            self.app.current_semantic_model().active_surface(),
            SurfaceId::FileBrowser
        );
        assert_eq!(self.app.file_browser().asset_kind(), AssetKind::SoundFont);
        let listing = FileBrowserListing::new(
            FileBrowserFolderId::default(),
            vec![
                FileBrowserRow::new(
                    "file:external",
                    source.file_name().unwrap().to_str().unwrap(),
                    FileBrowserRowKind::File(AssetFileId::new("@home/Bank.sf2").unwrap()),
                    None,
                )
                .unwrap(),
                FileBrowserRow::new("cancel", "Cancel", FileBrowserRowKind::Cancel, None).unwrap(),
            ],
        )
        .unwrap();
        self.app
            .dispatch(AppEvent::FileCatalogRefreshed {
                asset_kind: AssetKind::SoundFont,
                folder: FileBrowserFolderId::default(),
                listing: Ok(listing),
            })
            .unwrap();
        self.capture("file-page");
        assert!(self
            .app
            .dispatch_action(SemanticAction::PreviewStart)
            .is_err());
        self.key(WindowKey::Return);
        assert_eq!(self.app.current_semantic_model().focus_path(), &origin);
        let request = self.app.file_browser().import_request().unwrap().clone();
        let library = fixture.library.clone();
        let path = source.to_owned();
        let (asset, descriptor) = std::thread::spawn(move || library.import_file(&path))
            .join()
            .unwrap()
            .unwrap();
        let selection = AssetImportResult {
            request,
            result: Ok(asset),
            descriptor: Some(descriptor),
        };
        let before = self.app.capture_saved_session();
        self.app
            .dispatch(AppEvent::AssetImported(selection.clone()))
            .unwrap();
        assert_eq!(self.app.capture_saved_session(), before);
        self.capture("loading");
        for _ in 0..4 {
            self.app.advance_structural().unwrap();
        }
        assert!(
            self.worker.advance(),
            "{:?}",
            self.app.engine_selection_status()
        );
        let progress = self.app.advance_structural().unwrap();
        assert!(
            progress.graph_published().is_some(),
            "{:?}",
            self.app.engine_selection_status()
        );
        assert_eq!(
            self.app.capture_saved_session(),
            before,
            "staging does not commit bank/preset"
        );
        self.capture("activating");
        self.render();
        assert!(self
            .app
            .advance_structural()
            .unwrap()
            .activation_acknowledged()
            .is_some());
        self.capture("loaded-detail");
        selection
    }
}

#[test]
fn bank_replacement_rejects_a_valid_but_unrequested_preset() {
    use crest_synth::real_time::{
        GraphPreparationCorrelation, GraphPreparationRequest, GraphPreparationRequestError,
    };
    let fixture = Fixture::new();
    let path = fixture.external("Delta", 7, 120);
    let (asset, descriptor) = fixture.library.import_file(&path).unwrap();
    let registry = fixture
        .registry
        .clone()
        .with_asset_descriptor(descriptor)
        .unwrap();
    let state = fixture.state();
    let source = state.patches()[0].instrument_config();
    let file = ParameterId::new(SOUNDFONT_FILE_PARAMETER_ID).unwrap();
    let preset = ParameterId::new(SOUNDFONT_PRESET_PARAMETER_ID).unwrap();
    let reference = AssetReference::new(AssetKind::SoundFont, asset.as_str()).unwrap();
    let expected = registry
        .replace_asset(source, &file, reference.clone())
        .unwrap();
    assert_eq!(
        expected.value(&preset),
        Some(&ParameterValue::Choice(
            SoundFontPresetId::new(7, 3).unwrap().choice_id()
        ))
    );
    let factory = fixture.factory(registry.clone());
    let tampered = factory
        .replace_structural_choice(
            &expected,
            &preset,
            &SoundFontPresetId::new(7, 11).unwrap().choice_id(),
        )
        .unwrap();
    let correlation = GraphPreparationCorrelation::new_with_intent(
        EngineSelectionRequestId::FIRST,
        state.patches()[0].id(),
        StructuralEditIntent::ReplaceAsset {
            capability_id: source.capability_id().clone(),
            parameter_id: file,
            reference,
        },
        source.capability_id().clone(),
        source.capability_id().clone(),
        GraphRevision::INITIAL,
        GraphRevision::INITIAL.checked_next().unwrap(),
    )
    .unwrap();
    assert!(matches!(
        GraphPreparationRequest::replacement(
            correlation,
            state.patches(),
            tampered,
            state.generation(),
            *state.global(),
            state.mixer().clone(),
            AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 256).unwrap(),
            &registry,
        ),
        Err(GraphPreparationRequestError::ConfigDeltaMismatch)
    ));
}

#[test]
fn file_page_loads_independent_banks_and_restores_exact_presets_with_safe_callbacks() {
    use crest_synth::shell::WindowKey;
    let fixture = Fixture::new();
    let alpha = fixture.external("Alpha", 7, 120);
    let beta = fixture.external("Beta", 9, 240);
    let mut rig = Rig::new(&fixture);
    let default_audio = rig.play(1);
    let selection = rig.select(&fixture, &alpha);
    let saved_alpha = rig.app.capture_saved_session();
    assert!(rig
        .app
        .dispatch(AppEvent::AssetImported(selection))
        .is_err());
    assert_eq!(rig.app.capture_saved_session(), saved_alpha);
    let alpha_audio = rig.play(1);
    assert_ne!(default_audio, alpha_audio);
    rig.key(WindowKey::W); // Preset.
    rig.chord(WindowKey::K, WindowKey::W);
    rig.capture("preset-page");
    let choices = rig.app.current_graphical_shell();
    let controls = choices
        .semantic_model()
        .surface(SurfaceId::PatchChoice)
        .unwrap()
        .controls();
    assert_eq!(
        controls
            .iter()
            .map(|control| control.label())
            .collect::<Vec<_>>(),
        ["Alpha One", "Alpha Two"]
    );
    rig.key(WindowKey::S);
    rig.key(WindowKey::Return);
    rig.finish();
    let preset = ParameterId::new(SOUNDFONT_PRESET_PARAMETER_ID).unwrap();
    assert_eq!(
        rig.app.patches()[0].instrument_config().value(&preset),
        Some(&ParameterValue::Choice(
            SoundFontPresetId::new(7, 11).unwrap().choice_id()
        ))
    );
    assert_ne!(rig.play(1), alpha_audio);
    rig.app.dispatch_action(SemanticAction::Return).unwrap();
    rig.app
        .dispatch_action(SemanticAction::SelectPatch(Direction::Right))
        .unwrap();
    rig.select(&fixture, &beta);
    let beta_audio = rig.play(2);
    assert_ne!(alpha_audio, beta_audio);
    for (index, label) in [(0, "Alpha Two"), (1, "Beta One")] {
        let config = rig.app.patches()[index].instrument_config();
        let descriptor = rig
            .app
            .capabilities()
            .descriptor_for_config(config)
            .unwrap();
        let ParameterValue::Choice(choice) = config.value(&preset).unwrap() else {
            panic!()
        };
        assert_eq!(
            descriptor
                .parameter(&preset)
                .unwrap()
                .choices()
                .iter()
                .find(|item| item.id() == choice)
                .unwrap()
                .label(),
            label
        );
    }
    let saved = rig.app.capture_saved_session();
    let json = saved.to_json().unwrap();
    assert!(!json.contains("assetDescriptors"));
    assert!(!json.contains(fixture.root.to_str().unwrap()));
    std::fs::remove_file(alpha).unwrap();
    std::fs::remove_file(beta).unwrap();
    let decoded = SavedSession::from_json(&json, &fixture.registry).unwrap();
    let restored = decoded
        .prepare_restore(
            fixture.registry.clone(),
            EffectCapabilityRegistry::default(),
            &fixture.preparers(),
            &[],
            GraphRevision::new(12).unwrap(),
            48_000.0,
            256,
        )
        .unwrap();
    assert_eq!(SavedSession::capture(restored.state()), saved);
    let (replacement, graph) = restored.into_replacement();
    let mut fresh = fixture.state();
    fresh
        .apply(AppEvent::ReplacePersistedSession(Box::new(replacement)))
        .unwrap();
    assert_eq!(SavedSession::capture(&fresh), saved);
    assert_eq!(fresh.capabilities().asset_descriptors().len(), 2);
    let revision = graph.revision();
    let (control, callback) =
        LockFreeAudioBoundary::new(128, graph.initial_parameters().clone()).into_handles();
    let (_, structural_audio) =
        LockFreeStructuralGraphBoundary::new(1, 1, GraphHandoffStatus::with_active(revision))
            .unwrap()
            .into_handles();
    let (writer, _) = AtomicAudioObservation::default().into_handles();
    let mut renderer = AudioRenderer::with_observation(callback, structural_audio, graph, writer);
    let mut restored_app =
        AppLoop::new(fresh, StateProjector::for_graph(revision), control).unwrap();
    restored_app
        .dispatch(AppEvent::Midi {
            patch_id: PatchId::new(1).unwrap(),
            message: MidiMessage::try_new(
                MidiChannel::new(0).unwrap(),
                MidiMessageKind::NoteOn,
                60,
                110,
            )
            .unwrap(),
        })
        .unwrap();
    let mut restored_audio = [0.0; 512];
    assert_eq!(counted_render(&mut renderer, &mut restored_audio), (0, 0));
    assert!(restored_audio.iter().all(|sample| sample.is_finite()));
    assert!(restored_audio.iter().any(|sample| sample.abs() > 0.0001));
    let mut bad: serde_json::Value = serde_json::from_str(&json).unwrap();
    bad["patches"][0]["instrument"]["values"][0]["value"]["value"] =
        serde_json::json!("sf2.bank-7.program-99");
    let bad = SavedSession::from_json(&bad.to_string(), &fixture.registry).unwrap();
    assert!(bad
        .prepare_restore(
            fixture.registry.clone(),
            EffectCapabilityRegistry::default(),
            &fixture.preparers(),
            &[],
            GraphRevision::new(13).unwrap(),
            48_000.0,
            256
        )
        .is_err());
    assert_eq!(rig.app.capture_saved_session(), saved);
    let bundled = SavedSession::capture(&fixture.state())
        .prepare_restore(
            fixture.registry.clone(),
            EffectCapabilityRegistry::default(),
            &fixture.preparers(),
            &[],
            GraphRevision::new(14).unwrap(),
            48_000.0,
            256,
        )
        .unwrap();
    assert_eq!(
        bundled.state().patches()[0]
            .instrument_config()
            .asset_references()[0]
            .reference()
            .locator(),
        HIDEF_SOUNDFONT_PATH
    );
}

#[test]
fn file_cancel_failed_import_and_wrong_kind_listing_preserve_active_soundfont() {
    use crest_synth::shell::WindowKey;
    let fixture = Fixture::new();
    let bad = fixture.root.join("Library/Bad.sf2");
    std::fs::write(&bad, b"not an SF2").unwrap();
    let mut rig = Rig::new(&fixture);
    let saved = rig.app.capture_saved_session();
    let audio = rig.play(1);
    rig.key(WindowKey::Return);
    rig.key(WindowKey::S);
    let origin = rig.app.current_semantic_model().focus_path().clone();
    rig.key(WindowKey::Return);
    let listing = fixture
        .library
        .list(&FileBrowserFolderId::default())
        .unwrap();
    rig.app
        .dispatch(AppEvent::FileCatalogRefreshed {
            asset_kind: AssetKind::SoundFont,
            folder: FileBrowserFolderId::default(),
            listing: Ok(listing.clone()),
        })
        .unwrap();
    let focus = rig.app.current_semantic_model().focus_path().clone();
    rig.app
        .dispatch(AppEvent::FileCatalogRefreshed {
            asset_kind: AssetKind::Sample,
            folder: FileBrowserFolderId::default(),
            listing: Err(SampleAssetError::Unavailable),
        })
        .unwrap();
    assert_eq!(rig.app.file_browser().rows(), listing.rows());
    assert_eq!(rig.app.current_semantic_model().focus_path(), &focus);
    rig.chord(WindowKey::Shift, WindowKey::S);
    assert_eq!(rig.app.current_semantic_model().focus_path(), &origin);
    assert_eq!(rig.app.capture_saved_session(), saved);
    rig.key(WindowKey::Return);
    rig.key(WindowKey::Return);
    let request = rig.app.file_browser().import_request().unwrap().clone();
    let failure = fixture.library.import_file(&bad).unwrap_err();
    let selection = AssetImportResult {
        request,
        descriptor: None,
        result: Err(failure),
    };
    rig.app
        .dispatch(AppEvent::AssetImported(selection.clone()))
        .unwrap();
    assert_eq!(rig.app.capture_saved_session(), saved);
    assert_eq!(rig.app.current_semantic_model().focus_path(), &origin);
    let shell = rig.app.current_semantic_model();
    let file = shell
        .surface(SurfaceId::PatchDetail)
        .unwrap()
        .controls()
        .iter()
        .find(|row| row.label() == "SoundFont File")
        .unwrap();
    assert_eq!(file.error().unwrap().label(), failure.to_string());
    rig.capture("failed-detail");
    assert_eq!(rig.play(1), audio);
    rig.key(WindowKey::Return);
    rig.key(WindowKey::Return);
    let retry = rig.app.file_browser().import_request().unwrap().clone();
    assert!(rig
        .app
        .dispatch(AppEvent::AssetImported(selection))
        .is_err());
    assert_eq!(rig.app.file_browser().import_request(), Some(&retry));
    rig.app
        .dispatch(AppEvent::AssetImported(AssetImportResult {
            request: retry,
            descriptor: None,
            result: Err(SampleAssetError::Cancelled),
        }))
        .unwrap();
    assert_eq!(rig.app.capture_saved_session(), saved);
}
