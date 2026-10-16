use crate::adapter::{
    filesystem_sample_catalog::FilesystemSampleCatalog,
    filesystem_soundfont_catalog::FilesystemSoundFontCatalog,
};
use crate::control::{
    AppEvent, AppLoop, AssetImportRequest, AssetImportResult, SamplePreviewState, SurfaceId,
};
use crate::real_time::{BoundaryFull, ControlAudioBoundary};
use crate::synth::{
    AssetFileId, AssetKind, CapabilityDescriptor, FileBrowserFolderId, FileBrowserListing,
    SampleAssetCatalogPort, SampleAssetError,
};
use std::sync::Arc;
use std::thread::JoinHandle;

type ImportedFile = Result<(AssetFileId, Option<CapabilityDescriptor>), SampleAssetError>;
type ListingKey = (crate::control::FocusPath, AssetKind, FileBrowserFolderId);

enum LibraryWork {
    Import(AssetImportRequest, JoinHandle<ImportedFile>),
    List(
        ListingKey,
        JoinHandle<Result<FileBrowserListing, SampleAssetError>>,
    ),
}
impl LibraryWork {
    fn is_finished(&self) -> bool {
        match self {
            Self::Import(_, worker) => worker.is_finished(),
            Self::List(_, worker) => worker.is_finished(),
        }
    }
}

/// Capacity-one filesystem work shared by asset kinds. All state commits stay in AppState.
pub(crate) struct FileLibraryRuntime {
    samples: Option<Arc<FilesystemSampleCatalog>>,
    soundfonts: Option<Arc<FilesystemSoundFontCatalog>>,
    work: Option<LibraryWork>,
    visited: Option<ListingKey>,
}
impl FileLibraryRuntime {
    pub fn new(
        samples: Option<Arc<FilesystemSampleCatalog>>,
        soundfonts: Option<Arc<FilesystemSoundFontCatalog>>,
    ) -> Self {
        Self {
            samples,
            soundfonts,
            work: None,
            visited: None,
        }
    }

    pub fn advance<B: ControlAudioBoundary>(
        &mut self,
        app: &mut AppLoop<B>,
    ) -> Result<(), BoundaryFull> {
        if self.work.as_ref().is_some_and(LibraryWork::is_finished) {
            let preview_held = app.state().file_browser().preview() != &SamplePreviewState::Idle
                || app.state().engine_selection().is_in_flight();
            if !matches!(self.work, Some(LibraryWork::List(..))) || !preview_held {
                match self.work.take().expect("finished work exists") {
                    LibraryWork::Import(request, worker) => {
                        let imported = worker.join().unwrap_or(Err(SampleAssetError::Unavailable));
                        let (result, descriptor) = match imported {
                            Ok((id, descriptor)) => (Ok(id), descriptor),
                            Err(error) => (Err(error), None),
                        };
                        Self::complete(
                            app,
                            AssetImportResult {
                                request,
                                result,
                                descriptor,
                            },
                        )?;
                    }
                    LibraryWork::List((origin, asset_kind, folder), worker) => {
                        let listing = worker.join().unwrap_or(Err(SampleAssetError::Unavailable));
                        let browser = app.state().file_browser();
                        if app.state().interaction().active_surface() == SurfaceId::FileBrowser
                            && app
                                .state()
                                .interaction()
                                .return_path()
                                .is_some_and(|path| path.origin() == &origin)
                            && browser.asset_kind() == asset_kind
                            && browser.folder() == &folder
                        {
                            if let Ok(result) = app.dispatch(AppEvent::FileCatalogRefreshed {
                                asset_kind,
                                folder,
                                listing,
                            }) {
                                if let Some(error) = result.boundary_full() {
                                    return Err(error);
                                }
                            }
                        }
                    }
                }
            }
        }
        if self.work.is_some() {
            return Ok(());
        }
        if let Some(request) = app.state().file_browser().import_request().cloned() {
            let samples = self.samples.clone();
            let soundfonts = self.soundfonts.clone();
            let asset = request.asset_id.clone();
            let kind = request.asset_kind;
            match std::thread::Builder::new()
                .name("asset-import".into())
                .spawn(move || match kind {
                    AssetKind::Sample => samples
                        .ok_or(SampleAssetError::Unavailable)?
                        .import_browser_asset(&asset)
                        .map(|id| (id, None)),
                    AssetKind::SoundFont => soundfonts
                        .ok_or(SampleAssetError::Unavailable)?
                        .import_browser_asset(&asset)
                        .map(|(id, descriptor)| (id, Some(descriptor))),
                    AssetKind::Sfz => {
                        crate::adapter::sfz_library::import(&asset).map(|id| (id, None))
                    }
                    AssetKind::SysEx => crate::adapter::dx7_library::import(&asset)
                        .map(|(id, descriptor)| (id, Some(descriptor))),
                    AssetKind::NeuralModel | AssetKind::ImpulseResponse => {
                        crate::adapter::model_assets::import(kind, &asset).map(|id| (id, None))
                    }
                    AssetKind::Other => Err(SampleAssetError::Unavailable),
                }) {
                Ok(worker) => self.work = Some(LibraryWork::Import(request, worker)),
                Err(_) => Self::complete(
                    app,
                    AssetImportResult {
                        request,
                        descriptor: None,
                        result: Err(SampleAssetError::Unavailable),
                    },
                )?,
            }
            return Ok(());
        }
        if app.state().interaction().active_surface() != SurfaceId::FileBrowser {
            self.visited = None;
            return Ok(());
        }
        let browser = app.state().file_browser();
        let Some(origin) = app
            .state()
            .interaction()
            .return_path()
            .map(|path| path.origin().clone())
        else {
            return Ok(());
        };
        let kind = browser.asset_kind();
        let folder = browser.folder().clone();
        let key = (origin, kind, folder.clone());
        if self.visited.as_ref() == Some(&key) {
            return Ok(());
        }
        self.visited = Some(key.clone());
        let samples = self.samples.clone();
        let soundfonts = self.soundfonts.clone();
        let requested = folder.clone();
        match std::thread::Builder::new()
            .name("asset-catalog".into())
            .spawn(move || match kind {
                AssetKind::Sample => samples
                    .ok_or(SampleAssetError::Unavailable)?
                    .list(&requested),
                AssetKind::SoundFont => soundfonts
                    .ok_or(SampleAssetError::Unavailable)?
                    .list(&requested),
                AssetKind::SysEx => crate::adapter::dx7_library::browser()?.list(&requested, kind),
                AssetKind::Sfz | AssetKind::NeuralModel | AssetKind::ImpulseResponse => {
                    crate::adapter::model_assets::browser(kind)?.list(&requested, kind)
                }
                AssetKind::Other => Err(SampleAssetError::Unavailable),
            }) {
            Ok(worker) => self.work = Some(LibraryWork::List(key, worker)),
            Err(_) => {
                let _ = app.dispatch(AppEvent::FileCatalogRefreshed {
                    asset_kind: kind,
                    folder,
                    listing: Err(SampleAssetError::Unavailable),
                });
            }
        }
        Ok(())
    }
    fn complete<B: ControlAudioBoundary>(
        app: &mut AppLoop<B>,
        selection: AssetImportResult,
    ) -> Result<(), BoundaryFull> {
        if let Ok(result) = app.dispatch(AppEvent::AssetImported(selection)) {
            if let Some(error) = result.boundary_full() {
                return Err(error);
            }
        }
        Ok(())
    }
    pub fn shutdown(&mut self) {
        if let Some(work) = self.work.take() {
            match work {
                LibraryWork::Import(_, worker) => {
                    let _ = worker.join();
                }
                LibraryWork::List(_, worker) => {
                    let _ = worker.join();
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "../../tests/support/soundfont_fixture.rs"]
mod soundfont_fixture;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
    use crate::adapter::sample_capability::SampleCapability;
    use crate::control::{
        AppState, Direction, SavedSession, SemanticAction, StateProjector, TopLevelContext,
    };
    use crate::kernel::{MidiChannel, PatchId};
    use crate::mixer::{
        global_parameters::GlobalParameters, mixer_state::MixerState, patch_output::PatchOutput,
    };
    use crate::real_time::{AudioBoundary, ParameterSnapshot};
    use crate::synth::{
        CapabilityRegistry, FileBrowserRowKind, InstrumentCapabilityProvider, Patch,
    };

    #[test]
    fn browser_worker_loads_nested_files_and_preserves_cancelled_assignment() {
        let root = std::env::temp_dir().join(format!("crest-nested-sample-{}", std::process::id()));
        std::fs::create_dir_all(root.join("Drums/Kicks")).unwrap();
        std::fs::write(
            root.join("Drums/Kicks/Test.wav"),
            include_bytes!("../../assets/sample-test.wav"),
        )
        .unwrap();
        let catalog = Arc::new(FilesystemSampleCatalog::new(&root).unwrap());
        let sample =
            SampleCapability::new(AssetFileId::new("Drums/Kicks/Test.wav").unwrap()).unwrap();
        let global = GlobalParameters::new(0.0).unwrap();
        let mut state = AppState::new(
            CapabilityRegistry::new(vec![sample.descriptor()]).unwrap(),
            global,
        );
        state
            .apply(AppEvent::InstallPatches(vec![Patch::new(
                PatchId::new(1).unwrap(),
                "Sample".into(),
                sample.default_config().unwrap(),
                MidiChannel::new(0).unwrap(),
                PatchOutput::default(),
            )]))
            .unwrap();
        state
            .apply_semantic_action(SemanticAction::SelectContext(TopLevelContext::Patch))
            .unwrap();
        let saved = SavedSession::capture(&state);
        let (control, _audio) = LockFreeAudioBoundary::new(
            32,
            ParameterSnapshot::new(0, global, MixerState::default(), &[]).unwrap(),
        )
        .into_handles();
        let mut app = AppLoop::new(state, StateProjector::new(), control).unwrap();
        let mut runtime = FileLibraryRuntime::new(Some(catalog), None);
        app.dispatch_action(SemanticAction::Activate).unwrap();
        let origin = app.state().interaction().focus_path().clone();
        app.dispatch_action(SemanticAction::OpenRelated).unwrap();
        for (folder, target) in [
            ("", "Drums"),
            ("Drums", "Kicks"),
            ("Drums/Kicks", "Test.wav"),
        ] {
            runtime.advance(&mut app).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while runtime.work.is_some() && std::time::Instant::now() < deadline {
                std::thread::yield_now();
                runtime.advance(&mut app).unwrap();
            }
            assert!(runtime.work.is_none(), "catalog worker completes");
            assert_eq!(app.state().file_browser().folder().as_str(), folder);
            let rows = app.state().file_browser().rows();
            let index = rows.iter().position(|row| row.label() == target).unwrap();
            for _ in 0..index {
                app.dispatch_action(SemanticAction::Navigate(Direction::Down))
                    .unwrap();
            }
            if target != "Test.wav" {
                app.dispatch_action(SemanticAction::Activate).unwrap();
            }
        }
        assert!(app.state().file_browser().rows().iter().any(|row| matches!(row.kind(), FileBrowserRowKind::File(asset) if asset.as_str() == "Drums/Kicks/Test.wav")));
        app.dispatch_action(SemanticAction::Return).unwrap();
        assert_eq!(app.state().interaction().focus_path(), &origin);
        assert_eq!(SavedSession::capture(app.state()), saved);
        runtime.shutdown();
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn soundfont_worker_uses_sf2_filter_and_imports_metadata_without_committing() {
        use crate::adapter::hidef_soundfont_asset::HiDefSoundFontAsset;
        use crate::adapter::hidef_soundfont_capability::HiDefSoundFontCapability;
        let root = std::env::temp_dir().join(format!("crest-sf2-worker-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let bytes = super::soundfont_fixture::bank("Worker", 4, 120);
        std::fs::write(root.join("Bank.sf2"), &bytes).unwrap();
        std::fs::write(
            root.join("Sample.wav"),
            include_bytes!("../../assets/sample-test.wav"),
        )
        .unwrap();
        let asset = HiDefSoundFontAsset::from_bytes(&bytes).unwrap();
        let provider = HiDefSoundFontCapability::new(asset.catalog()).unwrap();
        let registry = CapabilityRegistry::new(vec![provider.descriptor()]).unwrap();
        let config = crate::synth::DescriptorDefaultConfigFactory::new(
            registry.clone(),
            vec![Box::new(provider)],
        )
        .create(
            &crate::synth::CapabilityId::new(
                crate::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID,
            )
            .unwrap(),
        )
        .unwrap();
        let global = GlobalParameters::new(0.0).unwrap();
        let mut state = AppState::new(registry, global);
        state
            .apply(AppEvent::InstallPatches(vec![Patch::new(
                PatchId::new(1).unwrap(),
                "SoundFont".into(),
                config,
                MidiChannel::new(0).unwrap(),
                PatchOutput::default(),
            )]))
            .unwrap();
        state
            .apply_semantic_action(SemanticAction::SelectContext(TopLevelContext::Patch))
            .unwrap();
        let (control, _audio) = LockFreeAudioBoundary::new(
            32,
            ParameterSnapshot::new(0, global, MixerState::default(), &[]).unwrap(),
        )
        .into_handles();
        let mut app = AppLoop::new(state, StateProjector::new(), control).unwrap();
        let library = Arc::new(FilesystemSoundFontCatalog::new(&root, asset).unwrap());
        let mut runtime = FileLibraryRuntime::new(None, Some(library));
        app.dispatch_action(SemanticAction::Activate).unwrap();
        app.dispatch_action(SemanticAction::Navigate(Direction::Down))
            .unwrap();
        let origin = app.current_semantic_model().focus_path().clone();
        app.dispatch_action(SemanticAction::Activate).unwrap();
        let wait = |runtime: &mut FileLibraryRuntime, app: &mut AppLoop<_>| {
            runtime.advance(app).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while runtime.work.is_some() && std::time::Instant::now() < deadline {
                std::thread::yield_now();
                runtime.advance(app).unwrap();
            }
            assert!(runtime.work.is_none());
        };
        wait(&mut runtime, &mut app);
        assert!(app
            .file_browser()
            .rows()
            .iter()
            .any(|row| row.label() == "Bank.sf2"));
        assert!(!app
            .file_browser()
            .rows()
            .iter()
            .any(|row| row.label() == "Sample.wav"));
        let saved = app.capture_saved_session();
        app.dispatch_action(SemanticAction::Activate).unwrap();
        assert!(app.file_browser().import_request().is_some());
        wait(&mut runtime, &mut app);
        assert!(app.file_browser().import_request().is_none());
        assert_eq!(app.current_semantic_model().focus_path(), &origin);
        assert_eq!(app.capture_saved_session(), saved);
        assert_eq!(
            app.capabilities().asset_descriptors()[0].default_assets()[0]
                .reference()
                .locator(),
            "Bank.sf2"
        );
        runtime.shutdown();
        std::fs::remove_dir_all(root).unwrap();
    }
}
