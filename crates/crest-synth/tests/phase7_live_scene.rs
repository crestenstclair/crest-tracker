//! Deterministic construction proof for the retained Phase 7 live scene.

use crest_synth::adapter::atomic_audio_observation::AtomicAudioObservation;
use crest_synth::adapter::braids_capability::BraidsCapability;
use crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
use crest_synth::adapter::lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary;
use crest_synth::adapter::production_effects::{
    production_chorus_config, production_effect_preparers, production_effect_providers,
    production_effect_registry,
};
use crest_synth::adapter::production_instruments::production_soundfont_capability;
use crest_synth::adapter::sample_capability::SampleCapability;
use crest_synth::adapter::sample_preparer::SamplePreparer;
use crest_synth::control::{
    AppEvent, AppLoop, AppState, EventLog, FocusCapabilityId, InteractionMode, PatchControlId,
    StateProjector, SurfaceId, TopLevelContext,
};
use crest_synth::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crest_synth::kernel::{MidiChannel, PatchId};
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mixer_state::MixerState;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::{
    AudioBoundary, AudioObservation, AudioRenderer, GraphHandoffStatus, GraphRevision,
    ParameterSnapshot, PreparedGraphBuilder, RtPatchParameters, StructuralGraphBoundary,
};
use crest_synth::shell::audio_output::{AudioDeviceConfig, AudioSampleFormat};
use crest_synth::shell::{
    ShellFrameObservation, ShellRegionId, ShellRegionObservation, ShellRegionRect,
};
use crest_synth::synth::effect_slot_id::EffectSlotIndex;
use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
use crest_synth::synth::{AssetFileId, CapabilityRegistry, InstrumentCapabilityProvider, Patch};
use crest_synth::synth::{
    CapabilityId, DecodedSample, DescriptorDefaultConfigFactory, FileBrowserFolderId,
    FileBrowserListing, FileBrowserRow, FileBrowserRowKind, InstrumentPreparationError,
    InstrumentPreparer, PreparedInstrument, PreparedInstrumentError, SampleAssetCatalogPort,
    SampleAssetError, SampleDecoderPort, SampleEncoding, SampleMetadata,
};
use crest_synth::testing::automatic_midi_test::{create_soundfont_config, AutomaticMidiTest};
use crest_synth::testing::instrument_part::InstrumentPart;
use crest_synth::testing::live_demo_scene::LiveDemoScene;
use crest_synth::testing::midi_event_source::{FixedEventBatch, MidiEventSource, MidiSourceError};
use crest_synth::testing::{
    DeterministicGraphPreparationWorker, DeterministicSampleCatalog, DeterministicSampleDecoder,
    LiveDemoRunner, RuntimeAudioWitness,
};
use std::sync::Arc;
use std::time::Duration;

const SAMPLE_RATE: f32 = 48_000.0;
const FRAME_COUNT: usize = 64;

fn first_json_difference(
    left: &serde_json::Value,
    right: &serde_json::Value,
    path: &str,
) -> Option<String> {
    match (left, right) {
        (serde_json::Value::Object(left), serde_json::Value::Object(right)) => {
            for key in left.keys().chain(right.keys()) {
                match (left.get(key), right.get(key)) {
                    (Some(left), Some(right)) if left != right => {
                        return first_json_difference(left, right, &format!("{path}.{key}"));
                    }
                    (Some(_), None) | (None, Some(_)) => {
                        return Some(format!("{path}.{key}: key presence differs"));
                    }
                    _ => {}
                }
            }
            None
        }
        (serde_json::Value::Array(left), serde_json::Value::Array(right)) => {
            for (index, (left, right)) in left.iter().zip(right).enumerate() {
                if left != right {
                    return first_json_difference(left, right, &format!("{path}[{index}]"));
                }
            }
            (left.len() != right.len())
                .then(|| format!("{path}: array lengths {} != {}", left.len(), right.len()))
        }
        _ => Some(format!("{path}: {left} != {right}")),
    }
}

struct TonePreparer {
    capability_id: CapabilityId,
    shared_asset: bool,
}

impl TonePreparer {
    fn new(capability_id: &CapabilityId, shared_asset: bool) -> Self {
        Self {
            capability_id: capability_id.clone(),
            shared_asset,
        }
    }
}

impl InstrumentPreparer for TonePreparer {
    fn capability_id(&self) -> &CapabilityId {
        &self.capability_id
    }

    fn prepared_shared_asset_count(&self) -> usize {
        usize::from(self.shared_asset)
    }

    fn prepare(
        &self,
        patch: &Patch,
        _sample_rate: f32,
        _max_frames: usize,
    ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
        Ok(Box::new(ToneInstrument {
            patch_id: patch.id(),
            phase: 0.0,
            increment: 0.007 + patch.id().value() as f32 * 0.003,
        }))
    }
}

struct ToneInstrument {
    patch_id: PatchId,
    phase: f32,
    increment: f32,
}

impl PreparedInstrument for ToneInstrument {
    fn patch_id(&self) -> PatchId {
        self.patch_id
    }

    fn dispatch(
        &mut self,
        _message: MidiMessage,
        _parameters: &RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        Ok(())
    }

    fn render(
        &mut self,
        output: &mut [f32],
        frame_count: usize,
        _parameters: &RtPatchParameters,
    ) -> Result<(), crest_synth::synth::PreparedInstrumentError> {
        for frame in output[..frame_count * 2].chunks_exact_mut(2) {
            self.phase += self.increment;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
            let sample = (self.phase * 2.0 - 1.0) * 0.2;
            frame[0] = sample;
            frame[1] = sample * 0.9;
        }

        Ok(())
    }

    fn all_notes_off(&mut self) {}
}

fn shell_frame(
    projection: &crest_synth::control::GraphicalShellProjection,
) -> ShellFrameObservation {
    ShellFrameObservation::try_new_semantic(
        1_920.0,
        1_080.0,
        projection.semantic_model(),
        [
            ShellRegionObservation::new(
                ShellRegionId::ContextLine,
                ShellRegionRect::new(0.0, 0.0, 1_920.0, 48.0),
                projection.context_line().context_label(),
            ),
            ShellRegionObservation::new(
                ShellRegionId::IdentityHeader,
                ShellRegionRect::new(0.0, 48.0, 1_920.0, 120.0),
                projection.identity_header().primary_label(),
            ),
            ShellRegionObservation::new(
                ShellRegionId::MainWorkspace,
                ShellRegionRect::new(0.0, 120.0, 1_500.0, 1_016.0),
                projection.workspace().main_label(),
            ),
            ShellRegionObservation::new(
                ShellRegionId::PersistentSideRegion,
                ShellRegionRect::new(1_500.0, 120.0, 1_920.0, 1_016.0),
                projection.workspace().side_label(),
            ),
            ShellRegionObservation::new(
                ShellRegionId::Footer,
                ShellRegionRect::new(0.0, 1_016.0, 1_920.0, 1_080.0),
                projection.footer().path_label(),
            ),
        ],
    )
    .unwrap()
}

struct Phase7MidiSource {
    parts: Vec<InstrumentPart>,
    started: bool,
    emitted: bool,
}

impl Phase7MidiSource {
    fn new() -> Self {
        Self {
            parts: vec![
                InstrumentPart::new(
                    0,
                    "Fixture Lead".to_owned(),
                    SoundFontInstrument::new(0, 8, false).unwrap(),
                ),
                InstrumentPart::new(
                    1,
                    "Fixture Pad".to_owned(),
                    SoundFontInstrument::new(0, 48, false).unwrap(),
                ),
                InstrumentPart::new(
                    2,
                    "Fixture Sample".to_owned(),
                    SoundFontInstrument::new(0, 80, false).unwrap(),
                ),
            ],
            started: false,
            emitted: false,
        }
    }
}

impl MidiEventSource for Phase7MidiSource {
    fn prepare(&mut self) -> Result<Vec<InstrumentPart>, MidiSourceError> {
        Ok(self.parts.clone())
    }

    fn start(&mut self) {
        self.started = true;
    }

    fn poll(
        &mut self,
        elapsed: Duration,
        output: &mut FixedEventBatch,
    ) -> Result<(), MidiSourceError> {
        if self.started && !self.emitted && !elapsed.is_zero() {
            for part in &self.parts {
                let message = MidiMessage::try_new(
                    part.assigned_channel(),
                    MidiMessageKind::NoteOn,
                    60 + part.index() as u8,
                    96,
                )
                .unwrap();
                output.try_push(message)?;
            }
            self.emitted = true;
        }
        Ok(())
    }

    fn finished(&self) -> bool {
        self.emitted
    }
}

fn decoded_asset(asset: &str, frequency: f32) -> DecodedSample {
    let frames = 4_096_usize;
    let samples = (0..frames)
        .map(|frame| {
            let phase = frame as f32 * frequency * core::f32::consts::TAU / SAMPLE_RATE;
            phase.sin() * 0.4
        })
        .collect::<Vec<_>>();
    DecodedSample::new(
        SampleMetadata::new(
            AssetFileId::new(asset).unwrap(),
            samples.len() as u64 * 4,
            SAMPLE_RATE as u32,
            1,
            32,
            SampleEncoding::Float,
            frames as u64,
        )
        .unwrap(),
        samples,
    )
    .unwrap()
}

fn sample_listing() -> FileBrowserListing {
    let folder = FileBrowserFolderId::default();
    FileBrowserListing::new(
        folder,
        [
            ("A-valid.wav", "A-valid.wav"),
            ("B-alternate.wav", "B-alternate.wav"),
            ("Z-invalid.wav", "Z-invalid.wav"),
        ]
        .into_iter()
        .map(|(id, label)| {
            FileBrowserRow::new(
                format!("file:{id}"),
                label,
                FileBrowserRowKind::File(AssetFileId::new(id).unwrap()),
                Some(4_096),
            )
            .unwrap()
        })
        .chain([FileBrowserRow::new(
            "cancel:",
            "CANCEL — UNCHANGED",
            FileBrowserRowKind::Cancel,
            None,
        )
        .unwrap()])
        .collect(),
    )
    .unwrap()
}

fn tree() -> crest_synth::control::StateTree {
    let soundfont = production_soundfont_capability().unwrap();
    let braids = BraidsCapability::new().unwrap();
    let sample = SampleCapability::new(AssetFileId::new("A-valid.wav").unwrap()).unwrap();
    let registry = CapabilityRegistry::new(vec![
        soundfont.descriptor(),
        braids.descriptor(),
        sample.descriptor(),
    ])
    .unwrap();
    let effects = production_effect_registry().unwrap();
    let first_slot = EffectSlotIndex::ALL[0];
    let patches = vec![
        Patch::new(
            PatchId::new(1).unwrap(),
            "Fixture Lead".to_owned(),
            create_soundfont_config(&soundfont, SoundFontInstrument::new(0, 8, false).unwrap())
                .unwrap(),
            MidiChannel::new(0).unwrap(),
            PatchOutput::to_track(MixerTrackId::new(0).unwrap()),
        )
        .with_effect_slot(
            first_slot,
            production_chorus_config(first_slot.instance_identity()).unwrap(),
        ),
        Patch::new(
            PatchId::new(2).unwrap(),
            "Fixture Pad".to_owned(),
            braids.default_config().unwrap(),
            MidiChannel::new(1).unwrap(),
            PatchOutput::to_track(MixerTrackId::new(1).unwrap()),
        ),
        Patch::new(
            PatchId::new(3).unwrap(),
            "Fixture Sample".to_owned(),
            sample.default_config().unwrap(),
            MidiChannel::new(2).unwrap(),
            PatchOutput::to_track(MixerTrackId::new(2).unwrap()),
        ),
    ];
    let mut state = AppState::new_with_effects(
        registry,
        effects.clone(),
        GlobalParameters::new(0.0).unwrap(),
    )
    .with_initial_returns(crest_synth::adapter::production_effects::startup_bus_returns(&effects));
    state.apply(AppEvent::InstallPatches(patches)).unwrap();
    StateProjector::new()
        .project_with_shell_tree(&state)
        .unwrap()
        .5
}

#[test]
fn phase7_live_scene_is_cumulative_semantic_bounded_and_has_a_falsifying_negative() {
    let tree = tree();
    let base = LiveDemoScene::from_installed_state(&tree).unwrap();
    let scene = LiveDemoScene::detail_and_assets_from_installed_state(&tree, false).unwrap();
    let defeated = LiveDemoScene::detail_and_assets_from_installed_state(&tree, true).unwrap();

    assert_eq!(scene.name(), "detail-and-assets-live-demo");
    assert!(scene.steps().len() > base.steps().len());
    assert_eq!(
        scene.expected_editable_parameters(),
        base.expected_editable_parameters(),
        "the cumulative Phase 7 tail retains the earlier complete scalar surface"
    );
    assert_eq!(
        scene.expected_engine_transitions(),
        base.expected_engine_transitions(),
        "the retained engine lifecycle remains byte-for-byte cumulative"
    );
    assert_eq!(
        scene.detail_assets_subject(),
        Some((
            PatchId::new(3).unwrap(),
            MixerTrackId::new(2).unwrap(),
            "A-valid.wav"
        ))
    );

    let events = scene
        .steps()
        .iter()
        .map(|step| step.event())
        .collect::<Vec<_>>();
    assert!(events.contains(&&AppEvent::SelectContext(TopLevelContext::Patch)));
    assert!(events.contains(&&AppEvent::EnterSurface(SurfaceId::PatchDetail)));
    assert!(events.contains(&&AppEvent::OpenRelated));
    assert!(events.contains(&&AppEvent::PreviewStart));
    assert!(events.contains(&&AppEvent::PreviewStop));
    assert!(events.contains(&&AppEvent::Return));
    assert!(
        events
            .iter()
            .filter(|event| matches!(event, AppEvent::Activate))
            .count()
            >= 2
    );
    assert!(
        events
            .iter()
            .filter(|event| matches!(event, AppEvent::Midi { .. }))
            .count()
            >= 80
    );
    assert!(scene.required_event_log_capacity(256) < 4_096);
    let choice_exits = scene
        .steps()
        .windows(3)
        .filter_map(|steps| {
            (steps[0].event() == &AppEvent::SetInteractionMode(InteractionMode::Adjust)
                && steps[1].event() == &AppEvent::Adjust(crest_synth::control::Direction::Up))
                .then_some(steps[2].event())
        })
        .collect::<Vec<_>>();
    assert_eq!(
        choice_exits
            .iter()
            .filter(|event| matches!(event, AppEvent::Return))
            .count(),
        4,
        "engine, descriptor, effect-slot, and route modals each cancel"
    );
    assert_eq!(
        choice_exits
            .iter()
            .filter(|event| matches!(event, AppEvent::Activate))
            .count(),
        4,
        "engine, descriptor, effect-slot, and route modals each confirm their current stable identity"
    );

    assert!(!defeated
        .steps()
        .iter()
        .any(|step| matches!(step.event(), AppEvent::PreviewStart | AppEvent::PreviewStop)));
    assert_eq!(
        defeated.steps().len() + 2,
        scene.steps().len(),
        "the controlled negative removes only the required preview press/release"
    );
}

fn run_phase7_headless(defeat_preview: bool) -> crest_synth::testing::LiveDemoReport {
    let soundfont = production_soundfont_capability().unwrap();
    let braids = BraidsCapability::new().unwrap();
    let sample = SampleCapability::new(AssetFileId::new("A-valid.wav").unwrap()).unwrap();
    let providers: Vec<Box<dyn InstrumentCapabilityProvider>> =
        vec![Box::new(soundfont), Box::new(braids), Box::new(sample)];
    let registry = CapabilityRegistry::new(
        providers
            .iter()
            .map(|provider| provider.descriptor())
            .collect(),
    )
    .unwrap();
    let effect_providers = production_effect_providers().unwrap();
    let effects = production_effect_registry().unwrap();
    let listing = sample_listing();
    let global = GlobalParameters::new(0.0).unwrap();
    let initial = ParameterSnapshot::new(0, global, MixerState::default(), &[]).unwrap();
    let boundary = LockFreeAudioBoundary::new(4_096, initial);
    let (control, audio) = boundary.into_handles();
    let mut app_loop = AppLoop::with_event_log(
        AppState::new_with_effects(registry.clone(), effects.clone(), global)
            .with_initial_returns(
                crest_synth::adapter::production_effects::startup_bus_returns(&effects),
            )
            .with_sample_catalog([(FileBrowserFolderId::default(), Ok(listing))]),
        StateProjector::for_graph(GraphRevision::INITIAL),
        control,
        EventLog::new(4_096).unwrap(),
    )
    .unwrap();
    let mut automatic = AutomaticMidiTest::new(Phase7MidiSource::new());
    automatic
        .initialize_with_effects(&providers, &effect_providers, &mut app_loop)
        .unwrap();
    let scene = LiveDemoScene::detail_and_assets_from_installed_state(
        &app_loop.current_state_tree(),
        defeat_preview,
    )
    .unwrap();

    let a = AssetFileId::new("A-valid.wav").unwrap();
    let b = AssetFileId::new("B-alternate.wav").unwrap();
    let z = AssetFileId::new("Z-invalid.wav").unwrap();
    let catalog: Arc<dyn SampleAssetCatalogPort> = Arc::new(DeterministicSampleCatalog::new(
        [],
        [
            (a.clone(), Ok(vec![1])),
            (b.clone(), Ok(vec![1])),
            (z.clone(), Ok(vec![1])),
        ],
    ));
    let decoder: Arc<dyn SampleDecoderPort> = Arc::new(DeterministicSampleDecoder::new([
        (a.clone(), Ok(decoded_asset(a.as_str(), 220.0))),
        (b.clone(), Ok(decoded_asset(b.as_str(), 330.0))),
        (z, Err(SampleAssetError::MalformedWave)),
    ]));
    let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![
        Box::new(TonePreparer::new(providers[0].descriptor().id(), true)),
        Box::new(TonePreparer::new(providers[1].descriptor().id(), false)),
        Box::new(SamplePreparer::new(Arc::clone(&catalog), Arc::clone(&decoder)).unwrap()),
    ];
    let effect_preparers = production_effect_preparers().unwrap();
    let audio_config =
        AudioDeviceConfig::new(SAMPLE_RATE, 2, AudioSampleFormat::F32, FRAME_COUNT).unwrap();
    let graph = PreparedGraphBuilder::new(&registry, &preparers)
        .with_effects(&effects, &effect_preparers)
        .with_returns(app_loop.bus_returns())
        .build(
            GraphRevision::INITIAL,
            app_loop.patches(),
            app_loop.current_parameters().clone(),
            SAMPLE_RATE,
            FRAME_COUNT,
        )
        .unwrap();
    let prepared_shared_assets = preparers
        .iter()
        .map(|preparer| preparer.prepared_shared_asset_count())
        .sum();
    let structural = LockFreeStructuralGraphBoundary::new(
        1,
        1,
        GraphHandoffStatus::with_active(GraphRevision::INITIAL),
    )
    .unwrap();
    let (structural_control, structural_audio) = structural.into_handles();
    let worker_preparers: Vec<Box<dyn InstrumentPreparer>> = vec![
        Box::new(TonePreparer::new(providers[0].descriptor().id(), true)),
        Box::new(TonePreparer::new(providers[1].descriptor().id(), false)),
        Box::new(SamplePreparer::new(catalog, decoder).unwrap()),
    ];
    let worker = DeterministicGraphPreparationWorker::new_with_effects(
        registry.clone(),
        worker_preparers,
        effects.clone(),
        production_effect_preparers().unwrap(),
        audio_config,
    );
    let worker_handle = worker.advance_handle();
    app_loop
        .configure_engine_selection(
            DescriptorDefaultConfigFactory::new(registry, providers),
            worker,
            structural_control,
            &graph,
            audio_config,
        )
        .unwrap();
    let observation = AtomicAudioObservation::default();
    let (writer, reader) = observation.into_handles();
    let mut renderer = AudioRenderer::with_observation(audio, structural_audio, graph, writer);
    automatic.start().unwrap();
    let runtime = RuntimeAudioWitness::new(
        prepared_shared_assets,
        app_loop.patches().len(),
        app_loop.patches().len(),
        0,
        true,
        GraphRevision::INITIAL,
        0,
        0,
    );
    let mut runner = LiveDemoRunner::start(scene, automatic, reader, runtime);
    let mut output = [0.0_f32; FRAME_COUNT * 2];

    for _ in 0..3_600 {
        let _ = worker_handle.advance();
        app_loop.advance_structural().unwrap();
        if let Err(error) = runner.advance(Duration::from_millis(25), &mut app_loop) {
            let tree = app_loop.current_state_tree();
            let tree_value: serde_json::Value = serde_json::from_str(tree.json()).unwrap();
            let shell = app_loop.current_graphical_shell();
            let shell_value = serde_json::to_value(&shell).unwrap();
            let shell_difference = first_json_difference(
                tree_value.get("graphicalShell").unwrap(),
                &shell_value,
                "graphicalShell",
            );
            panic!(
                "Phase 7 runner failed: {error:?}; recent={:?}; status={:?}; treeGen={}; shellGen={}; treeHash={}; shellHash={}; shellEqual={}; difference={shell_difference:?}",
                app_loop.event_log().records().iter().rev().take(16).collect::<Vec<_>>(),
                app_loop.engine_selection_status(),
                tree.generation(),
                shell.generation(),
                tree.state_hash(),
                shell.state_hash(),
                tree_value.get("graphicalShell") == Some(&shell_value),
            );
        }
        renderer.render(&mut output);
        if runner.completed_report().is_none() {
            runner
                .observe_shell_frame(shell_frame(&app_loop.current_graphical_shell()))
                .unwrap();
        }
        if runner.completed_report().is_some() {
            break;
        }
    }

    let report = runner
        .completed_report()
        .expect("the bounded Phase 7 scene completes headlessly")
        .clone();
    drop(renderer);
    app_loop.shutdown_engine_selection_on_control().unwrap();
    assert_eq!(app_loop.owned_structural_graphs_on_control(), 0);
    report
}

#[test]
fn phase7_headless_scene_correlates_worker_renderer_projection_and_lossless_report() {
    let report = run_phase7_headless(false);
    #[path = "support/performance_evidence.rs"]
    mod performance_evidence;
    performance_evidence::retain(&report);
    let evidence = report
        .detail_and_assets()
        .expect("the Phase 7 report carries typed detail/assets evidence");
    assert!(evidence.is_complete(), "{evidence:?}");
    assert!(
        report.complete(),
        "{}; coverage={:?}; shell={:?}; routing={:?}; runtime={:?}",
        report.summary(),
        report.coverage(),
        report.shell_coverage(),
        report.mixer_routing(),
        report.runtime_audio(),
    );
    assert!(evidence.exact_return_observed());
    assert!(evidence.preview_revision_compatible());
    assert!(evidence.preview_release_observed());
    assert!(evidence.waveform_correlated());
    assert!(!evidence.checkpoints().is_empty());
    assert!(evidence
        .checkpoints()
        .iter()
        .all(|checkpoint| checkpoint.visible_focus_matches()));
    assert!(evidence.checkpoints().iter().any(|checkpoint| {
        checkpoint.focus().surface() == SurfaceId::PatchDetail
            && matches!(
                checkpoint.focus().capability_id(),
                Some(FocusCapabilityId::Instrument(_))
            )
    }));
    assert!(evidence.checkpoints().iter().any(|checkpoint| {
        checkpoint.focus().surface() == SurfaceId::PatchDetail
            && matches!(
                checkpoint.focus().capability_id(),
                Some(FocusCapabilityId::Effect(_))
            )
    }));
    let modal_ids = evidence
        .checkpoints()
        .iter()
        .filter_map(|checkpoint| checkpoint.focus().modal_id())
        .collect::<Vec<_>>();
    for control in [
        PatchControlId::Engine,
        PatchControlId::EffectSlot(EffectSlotIndex::new(0).unwrap()),
        PatchControlId::Output(crest_synth::mixer::patch_output::PatchOutputParameter::OutputTrack),
    ] {
        assert!(
            modal_ids
                .iter()
                .any(|modal_id| modal_id.ends_with(control.as_str().as_ref())),
            "missing painted modal correlation for {control:?}: {modal_ids:?}"
        );
    }
    assert!(
        modal_ids
            .iter()
            .any(|modal_id| modal_id.contains(".choice.patch.capability.")),
        "missing painted descriptor-choice correlation: {modal_ids:?}"
    );
    assert!(evidence
        .checkpoints()
        .iter()
        .any(|checkpoint| checkpoint.focus().surface() == SurfaceId::FileBrowser));
    assert!(
        evidence.checkpoints().iter().any(|checkpoint| {
            checkpoint.preview_revision_compatible()
                // `primaryPatchId` names the concurrent MIDI command target,
                // while preview compatibility was correlated independently
                // to Sample Patch 3. Seeing both non-zero proves the audition
                // was routed to its origin track without silencing the other
                // installed capability.
                && checkpoint.primary_patch_id() == Some(PatchId::new(1).unwrap())
                && checkpoint.primary_patch_rms() > 1.0e-5
                && checkpoint.origin_track_rms() > checkpoint.primary_patch_rms()
        }),
        "preview checkpoints: {:?}",
        evidence
            .checkpoints()
            .iter()
            .filter(|checkpoint| checkpoint.preview_revision_compatible())
            .collect::<Vec<_>>()
    );
    assert!(report.mixer_routing().is_complete());
    assert_eq!(report.runtime_audio().prepared_instruments(), 3);
    assert_eq!(
        report.runtime_audio().engine_managed_patches(),
        report.runtime_audio().prepared_instruments()
    );
    assert_eq!(report.runtime_audio().fixed_per_patch_patches(), 0);
    assert!(report.runtime_audio().adjacent_capabilities_distinct());
    assert_eq!(report.event_log().dropped_records(), 0);
    assert_eq!(
        report.event_log().records().len() as u64,
        report.event_log().total_observed()
    );
    assert_eq!(report.final_audio_observation().active_notes(), 0);
    assert!(!report.final_audio_observation().preview_playing());
}

#[test]
fn phase7_controlled_negative_reaches_teardown_but_fails_named_preview_predicates() {
    let report = run_phase7_headless(true);
    let evidence = report
        .detail_and_assets()
        .expect("the defeated scene still emits typed Phase 7 evidence");
    assert!(!evidence.preview_revision_compatible());
    assert!(!evidence.preview_release_observed());
    assert!(!evidence.is_complete());
    assert!(
        !report.complete(),
        "the same completeness gate used by the optimized command must reject the controlled negative"
    );
    assert_eq!(report.final_audio_observation().active_notes(), 0);
    assert!(!report.final_audio_observation().preview_playing());
    assert_eq!(report.event_log().dropped_records(), 0);
}
