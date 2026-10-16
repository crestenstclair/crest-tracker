use crest_synth::adapter::atomic_audio_observation::AtomicAudioObservation;
use crest_synth::adapter::braids_capability::{BraidsCapability, BRAIDS_CAPABILITY_ID};
use crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
use crest_synth::adapter::lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary;
use crest_synth::adapter::production_effects::{
    production_chorus_config, production_effect_preparers, production_effect_providers,
    production_effect_registry,
};
use crest_synth::adapter::production_instruments::{
    production_capability_registry, production_instrument_providers,
};
use crest_synth::control::app_event::{AppEvent, Direction};
use crest_synth::control::app_loop::AppLoop;
use crest_synth::control::app_state::AppState;
use crest_synth::control::event_log::EventLog;
use crest_synth::control::state_projector::StateProjector;
use crest_synth::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crest_synth::mixer::global_parameters::GlobalParameters;
use crest_synth::mixer::mixer_state::MixerState;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::audio_boundary::AudioBoundary;
use crest_synth::real_time::audio_observation::AudioObservation;
use crest_synth::real_time::audio_renderer::AudioRenderer;
use crest_synth::real_time::graph_revision::GraphRevision;
use crest_synth::real_time::parameter_snapshot::ParameterSnapshot;
use crest_synth::real_time::prepared_graph_builder::PreparedGraphBuilder;
use crest_synth::real_time::{GraphHandoffStatus, StructuralGraphBoundary};
use crest_synth::shell::audio_output::{AudioDeviceConfig, AudioSampleFormat};
use crest_synth::synth::effect_slot_id::EffectSlotIndex;
use crest_synth::synth::patch::Patch;
use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
use crest_synth::synth::{
    CapabilityId, DescriptorDefaultConfigFactory, InstrumentPreparationError, InstrumentPreparer,
    PreparedInstrument, PreparedInstrumentError,
};
use crest_synth::testing::automatic_midi_test::{create_soundfont_config, AutomaticMidiTest};
use crest_synth::testing::demo_scene::DemoScene;
use crest_synth::testing::demo_scene_report::DemoSceneReport;
use crest_synth::testing::instrument_part::InstrumentPart;
use crest_synth::testing::midi_event_source::{FixedEventBatch, MidiEventSource, MidiSourceError};
use crest_synth::testing::{DeterministicGraphPreparationWorker, ExhaustiveGuiDemo};
use serde_json::Value;
use std::time::Duration;

pub const FRAME_COUNT: usize = 32;
pub const SAMPLE_RATE: f32 = 48_000.0;

pub struct DemoRun {
    pub report: DemoSceneReport,
    #[allow(dead_code)]
    pub baseline: Value,
    #[allow(dead_code)]
    pub expected_coverage: Vec<String>,
}

pub fn globals() -> GlobalParameters {
    GlobalParameters::new(0.0).expect("fixture global parameters are valid")
}

fn parts() -> Vec<InstrumentPart> {
    vec![
        InstrumentPart::new(
            0,
            "Fixture Lead".to_owned(),
            SoundFontInstrument::new(0, 8, false).expect("fixture instrument is valid"),
        ),
        InstrumentPart::new(
            1,
            "Fixture Pad".to_owned(),
            SoundFontInstrument::new(0, 48, false).expect("fixture instrument is valid"),
        ),
    ]
}

fn scene_patches() -> Vec<Patch> {
    let soundfont = crest_synth::adapter::production_instruments::production_soundfont_capability()
        .expect("fixture capability is valid");
    let braids = BraidsCapability::new().expect("fixture capability is valid");
    parts()
        .into_iter()
        .enumerate()
        .map(|(index, part)| {
            let patch_id = crest_synth::kernel::patch_id::PatchId::new(index as u32 + 1)
                .expect("fixture PatchId is valid");
            let mut patch = Patch::new(
                patch_id,
                part.name().to_owned(),
                if index % 2 == 0 {
                    create_soundfont_config(&soundfont, part.instrument())
                        .expect("fixture config matches the SoundFont descriptor")
                } else {
                    braids
                        .default_config()
                        .expect("fixture config matches the Braids descriptor")
                },
                part.assigned_channel(),
                PatchOutput::to_track(
                    MixerTrackId::new(index as u8).expect("fixture mixer track is valid"),
                ),
            );
            if index == 0 {
                // Position-explicit: the fixture states the address it
                // occupies, and the occupant's identity is the one that
                // address derives, so a round trip lands it back in place.
                let address = EffectSlotIndex::ALL[0];
                patch = patch.with_effect_slot(
                    address,
                    production_chorus_config(address.instance_identity())
                        .expect("fixture Chorus config is valid"),
                );
            }
            patch
        })
        .collect()
}

pub struct FixtureMidiSource {
    parts: Vec<InstrumentPart>,
    emitted: bool,
    started: bool,
}

impl FixtureMidiSource {
    pub fn new() -> Self {
        Self {
            parts: parts(),
            emitted: false,
            started: false,
        }
    }
}

impl MidiEventSource for FixtureMidiSource {
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
            let channel = self.parts[0].assigned_channel();
            let message = MidiMessage::try_new(channel, MidiMessageKind::NoteOn, 60, 96)
                .expect("fixture MIDI bytes are valid");
            output.try_push(message)?;
            self.emitted = true;
        }
        Ok(())
    }

    fn finished(&self) -> bool {
        self.emitted
    }
}

pub struct FixturePreparer {
    capability_id: CapabilityId,
}

impl FixturePreparer {
    pub fn new() -> Self {
        Self::for_capability("instrument.soundfont.hidef")
    }

    pub fn for_capability(capability_id: &str) -> Self {
        Self {
            capability_id: CapabilityId::new(capability_id)
                .expect("fixture capability identity is valid"),
        }
    }
}

impl InstrumentPreparer for FixturePreparer {
    fn capability_id(&self) -> &CapabilityId {
        &self.capability_id
    }

    fn prepared_shared_asset_count(&self) -> usize {
        usize::from(self.capability_id.as_str() == "instrument.soundfont.hidef")
    }

    fn prepare(
        &self,
        patch: &Patch,
        _sample_rate: f32,
        _max_frames: usize,
    ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
        if patch.id().value() % 2 == 1 {
            Ok(Box::new(FixtureLeadInstrument {
                patch_id: patch.id(),
            }))
        } else {
            Ok(Box::new(FixturePadInstrument {
                patch_id: patch.id(),
            }))
        }
    }
}

struct FixtureLeadInstrument {
    patch_id: crest_synth::kernel::patch_id::PatchId,
}

impl PreparedInstrument for FixtureLeadInstrument {
    fn patch_id(&self) -> crest_synth::kernel::patch_id::PatchId {
        self.patch_id
    }

    fn dispatch(
        &mut self,
        _message: MidiMessage,
        _parameters: &crest_synth::real_time::RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        Ok(())
    }

    fn render(
        &mut self,
        output: &mut [f32],
        _frame_count: usize,
        _parameters: &crest_synth::real_time::RtPatchParameters,
    ) -> Result<(), crest_synth::synth::PreparedInstrumentError> {
        for frame in output.chunks_exact_mut(2) {
            frame[0] = 0.15;
            frame[1] = 0.15;
        }

        Ok(())
    }

    fn all_notes_off(&mut self) {}
}

struct FixturePadInstrument {
    patch_id: crest_synth::kernel::patch_id::PatchId,
}

impl PreparedInstrument for FixturePadInstrument {
    fn patch_id(&self) -> crest_synth::kernel::patch_id::PatchId {
        self.patch_id
    }

    fn dispatch(
        &mut self,
        _message: MidiMessage,
        _parameters: &crest_synth::real_time::RtPatchParameters,
    ) -> Result<(), PreparedInstrumentError> {
        Ok(())
    }

    fn render(
        &mut self,
        output: &mut [f32],
        _frame_count: usize,
        _parameters: &crest_synth::real_time::RtPatchParameters,
    ) -> Result<(), crest_synth::synth::PreparedInstrumentError> {
        for frame in output.chunks_exact_mut(2) {
            frame[0] = 0.26;
            frame[1] = 0.2782;
        }

        Ok(())
    }

    fn all_notes_off(&mut self) {}
}

pub fn run_demo() -> DemoRun {
    let providers = production_instrument_providers().expect("production providers are valid");
    let registry = production_capability_registry().expect("production registry is valid");
    let effect_providers =
        production_effect_providers().expect("production effect providers are valid");
    let effects = production_effect_registry().expect("production effect registry is valid");
    let patches = scene_patches();
    let global_parameters = globals();
    // The exhaustive scene measures the legacy return parameters as well as
    // topology changes, so it needs explicit occupants before building coverage.
    let startup_returns =
        crest_synth::adapter::production_effects::production_default_bus_returns(&effects)
            .expect("the exhaustive fixture composes its audible returns");
    let scene = DemoScene::exhaustive_with_effects(
        &registry,
        &effects,
        &patches,
        &global_parameters,
        &startup_returns,
    )
    .expect("the fixture contains two discriminating Patches");
    let expected_coverage = scene.expected_coverage().to_vec();
    let initial_parameters =
        ParameterSnapshot::new(0, global_parameters, MixerState::default(), &[])
            .expect("initial parameters are valid");
    let boundary = LockFreeAudioBoundary::new(16, initial_parameters);
    let (control, audio) = boundary.into_handles();
    let event_log = EventLog::new(scene.event_log_capacity().saturating_add(16))
        .expect("fixture EventLog capacity is valid");
    let mut app_loop = AppLoop::with_event_log(
        AppState::new_with_effects(registry.clone(), effects.clone(), global_parameters)
            .with_initial_returns(startup_returns.clone()),
        StateProjector::for_graph(GraphRevision::INITIAL),
        control,
        event_log,
    )
    .expect("initial state projects");

    let mut automatic = AutomaticMidiTest::new(FixtureMidiSource::new());
    automatic
        .initialize_with_effects(&providers, &effect_providers, &mut app_loop)
        .expect("automatic fixture initializes through AppLoop");
    let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![
        Box::new(FixturePreparer::new()),
        Box::new(FixturePreparer::for_capability(BRAIDS_CAPABILITY_ID)),
    ];
    let effect_preparers =
        production_effect_preparers().expect("production effect preparers are valid");
    let graph = PreparedGraphBuilder::new(app_loop.capabilities(), &preparers)
        .with_effects(app_loop.effects(), &effect_preparers)
        .with_returns(app_loop.bus_returns())
        .build(
            GraphRevision::INITIAL,
            app_loop.patches(),
            app_loop.current_parameters().clone(),
            SAMPLE_RATE,
            FRAME_COUNT,
        )
        .expect("fixture graph prepares atomically");
    let structural = LockFreeStructuralGraphBoundary::new(
        1,
        1,
        GraphHandoffStatus::with_active(GraphRevision::INITIAL),
    )
    .expect("fixture structural boundary is valid");
    let (structural_control, structural_audio) = structural.into_handles();
    let worker_preparers: Vec<Box<dyn InstrumentPreparer>> = vec![
        Box::new(FixturePreparer::new()),
        Box::new(FixturePreparer::for_capability(BRAIDS_CAPABILITY_ID)),
    ];
    let worker_effect_preparers =
        production_effect_preparers().expect("production worker effect preparers are valid");
    let audio_config = AudioDeviceConfig::new(SAMPLE_RATE, 2, AudioSampleFormat::F32, FRAME_COUNT)
        .expect("fixture audio config is valid");
    let worker = DeterministicGraphPreparationWorker::new_with_effects(
        registry.clone(),
        worker_preparers,
        effects,
        worker_effect_preparers,
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
        .expect("fixture engine-selection orchestration configures");
    let (observation_writer, observation_reader) = AtomicAudioObservation::default().into_handles();
    let mut renderer =
        AudioRenderer::with_observation(audio, structural_audio, graph, observation_writer);
    automatic
        .start()
        .expect("automatic fixture starts after graph preparation");
    automatic
        .tick(Duration::from_millis(10), &mut app_loop)
        .expect("automatic fixture dispatches its due MIDI event");

    app_loop
        .dispatch(AppEvent::Navigate(Direction::Down))
        .expect("System-source navigation is accepted");
    app_loop
        .dispatch(AppEvent::Navigate(Direction::Up))
        .expect("inverse System-source navigation is accepted");

    let baseline: Value = serde_json::from_str(app_loop.current_state_tree().json())
        .expect("baseline StateTree is valid JSON");
    let mut audio_buffer = vec![0.0_f32; FRAME_COUNT * 2];
    let mut demo = ExhaustiveGuiDemo::new_with_worker_and_observation(
        &mut app_loop,
        &mut renderer,
        &mut audio_buffer,
        worker_handle,
        &observation_reader,
    );
    let report = demo
        .run(scene)
        .expect("the deterministic demo returns a diagnostic report");

    DemoRun {
        report,
        baseline,
        expected_coverage,
    }
}

/// Simulated worker/activation messages for reducer-only structural tests.
/// Actual graph construction and callback activation are exercised separately.
#[allow(dead_code)]
pub fn topology_activation_events(
    status: &crest_synth::control::EngineSelectionStatus,
) -> Vec<crest_synth::control::app_event::AppEvent> {
    use crest_synth::control::{app_event::AppEvent, EngineSelectionStatusKind};
    if status.kind() != EngineSelectionStatusKind::Loading {
        return Vec::new();
    }
    let correlation = status
        .correlation()
        .expect("pending structural correlation");
    let request_id = correlation.request_id();
    let intent = correlation.intent().clone();
    let source = correlation.source_graph_revision();
    let target = source.checked_next().unwrap();
    vec![
        AppEvent::EngineSelectionLifecycleAdvanced {
            request_id,
            lifecycle: EngineSelectionStatusKind::Validating,
        },
        AppEvent::EngineSelectionLifecycleAdvanced {
            request_id,
            lifecycle: EngineSelectionStatusKind::Preparing,
        },
        AppEvent::TopologyPrepared {
            request_id,
            intent: intent.clone(),
            source_graph_revision: source,
            target_graph_revision: target,
            prepared_visualization: None,
        },
        AppEvent::EngineActivationAcknowledged {
            request_id,
            intent,
            target_graph_revision: target,
            retired_graph_revision: source,
            collected: true,
        },
    ]
}
