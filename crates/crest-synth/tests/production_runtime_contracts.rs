use crest_synth::adapter::atomic_audio_observation::AtomicAudioObservation;
use crest_synth::adapter::braids_capability::{BraidsCapability, BRAIDS_CAPABILITY_ID};
use crest_synth::adapter::chorus_capability::ChorusCapability;
use crest_synth::adapter::chorus_preparer::ChorusPreparer;
use crest_synth::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID;
use crest_synth::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
use crest_synth::adapter::lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary;
use crest_synth::control::app_event::AppEvent;
use crest_synth::control::app_loop::AppLoop;
use crest_synth::control::app_state::AppState;
use crest_synth::control::event_log::EventLog;
use crest_synth::control::event_record::EventSource;
use crest_synth::control::state_projector::StateProjector;
use crest_synth::kernel::midi_channel::MidiChannel;
use crest_synth::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crest_synth::kernel::patch_id::PatchId;
use crest_synth::mixer::mixer_state::MixerState;
use crest_synth::mixer::mixer_track_id::MixerTrackId;
use crest_synth::mixer::patch_output::PatchOutput;
use crest_synth::real_time::audio_boundary::{AudioBoundary, ControlAudioBoundary};
use crest_synth::real_time::audio_command::AudioCommand;
use crest_synth::real_time::audio_observation::{
    AudioObservation, CallbackAudioObservation, ControlAudioObservation,
};
use crest_synth::real_time::audio_observation_snapshot::AudioObservationSnapshot;
use crest_synth::real_time::audio_renderer::AudioRenderer;
use crest_synth::real_time::graph_handoff_status::GraphHandoffStatus;
use crest_synth::real_time::graph_revision::GraphRevision;
use crest_synth::real_time::parameter_snapshot::{ParameterSnapshot, RtPatchParameters};
use crest_synth::real_time::prepared_graph::PreparedGraph;
use crest_synth::real_time::prepared_graph_builder::PreparedGraphBuilder;
use crest_synth::real_time::structural_graph_boundary::{
    AudioStructuralGraphBoundary, ControlStructuralGraphBoundary, RetiredBoundaryFull,
    StructuralBoundaryFull, StructuralGraphBoundary,
};
use crest_synth::real_time::structural_graph_coordinator::StructuralGraphCoordinator;
use crest_synth::shell::app_window::{
    AppInputCallback, AppWindow, AudioObservationCallback, FrameObservationCallback,
    MidiActivityObservationCallback, ProjectionCallback, SessionCommandCallback,
    SessionDocumentProjectionCallback, TickCallback, WindowError,
};
use crest_synth::shell::audio_output::{
    AudioDeviceConfig, AudioDeviceRuntimeError, AudioDeviceStatusCallback, AudioOutput,
    AudioOutputError, AudioRenderCallback, AudioSampleFormat, AudioStream, NegotiatedAudioOutput,
};
use crest_synth::shell::standalone_application::installed_patches_from_log;
use crest_synth::shell::{ApplicationConfig, ApplicationError, StandaloneApplication};
use crest_synth::synth::effect_slot_id::{EffectSlotIndex, MAX_EFFECT_SLOTS};
use crest_synth::synth::sound_font_instrument::SoundFontInstrument;
use crest_synth::synth::{
    CapabilityId, EffectCapabilityId, EffectCapabilityProvider, EffectPreparer,
    InstrumentCapabilityProvider, InstrumentCompositionError, InstrumentPreparationError,
    InstrumentPreparer, Patch, PreparedInstrument, PreparedInstrumentError,
};
use crest_synth::testing::automatic_midi_test::create_soundfont_config;
use crest_synth::testing::instrument_part::InstrumentPart;
use crest_synth::testing::midi_event_source::{FixedEventBatch, MidiEventSource, MidiSourceError};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

const TEST_MAX_FRAMES: usize = 7;
const TEST_SAMPLE_RATE: f32 = 44_100.0;

#[derive(Default)]
struct PreparationProbe {
    prepared: AtomicUsize,
    sample_rate_bits: AtomicU32,
    max_frames: AtomicUsize,
    oversized_render: AtomicBool,
    dispatches: AtomicUsize,
    drops: AtomicUsize,
}

struct TonePreparer {
    capability_id: CapabilityId,
    probe: Arc<PreparationProbe>,
}

impl TonePreparer {
    fn boxed(capability_id: &str, probe: &Arc<PreparationProbe>) -> Box<dyn InstrumentPreparer> {
        Box::new(Self {
            capability_id: CapabilityId::new(capability_id).unwrap(),
            probe: Arc::clone(probe),
        })
    }
}

impl InstrumentPreparer for TonePreparer {
    fn capability_id(&self) -> &CapabilityId {
        &self.capability_id
    }

    fn prepare(
        &self,
        patch: &Patch,
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
        self.probe.prepared.fetch_add(1, Ordering::Relaxed);
        self.probe
            .sample_rate_bits
            .store(sample_rate.to_bits(), Ordering::Relaxed);
        self.probe.max_frames.store(max_frames, Ordering::Relaxed);
        Ok(Box::new(ToneInstrument {
            patch_id: patch.id(),
            max_frames,
            probe: Arc::clone(&self.probe),
        }))
    }
}

struct ToneInstrument {
    patch_id: PatchId,
    max_frames: usize,
    probe: Arc<PreparationProbe>,
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
        self.probe.dispatches.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn render(
        &mut self,
        output: &mut [f32],
        frame_count: usize,
        _parameters: &RtPatchParameters,
    ) -> Result<(), crest_synth::synth::PreparedInstrumentError> {
        if frame_count > self.max_frames {
            self.probe.oversized_render.store(true, Ordering::Relaxed);
            return Ok(());
        }
        for sample in output.iter_mut().take(frame_count.saturating_mul(2)) {
            *sample = 0.25;
        }

        Ok(())
    }

    fn all_notes_off(&mut self) {}
}

impl Drop for ToneInstrument {
    fn drop(&mut self) {
        self.probe.drops.fetch_add(1, Ordering::Relaxed);
    }
}

struct FixtureMidiSource;

impl MidiEventSource for FixtureMidiSource {
    fn prepare(&mut self) -> Result<Vec<InstrumentPart>, MidiSourceError> {
        Ok(vec![InstrumentPart::new(
            0,
            "Negotiated fixture".to_owned(),
            SoundFontInstrument::new(0, 0, false).unwrap(),
        )])
    }

    fn start(&mut self) {}

    fn poll(
        &mut self,
        _elapsed: Duration,
        _output: &mut FixedEventBatch,
    ) -> Result<(), MidiSourceError> {
        Ok(())
    }

    fn finished(&self) -> bool {
        true
    }
}

#[derive(Default)]
struct StructuralProbe {
    splits: AtomicUsize,
    status_publications: AtomicUsize,
}

struct ReplaceableStructuralBoundary {
    probe: Arc<StructuralProbe>,
}

struct ReplaceableStructuralControl;

struct ReplaceableStructuralAudio {
    probe: Arc<StructuralProbe>,
}

impl StructuralGraphBoundary for ReplaceableStructuralBoundary {
    type ControlHandle = ReplaceableStructuralControl;
    type AudioHandle = ReplaceableStructuralAudio;

    fn into_handles(self) -> (Self::ControlHandle, Self::AudioHandle) {
        self.probe.splits.fetch_add(1, Ordering::Relaxed);
        (
            ReplaceableStructuralControl,
            ReplaceableStructuralAudio { probe: self.probe },
        )
    }
}

impl ControlStructuralGraphBoundary for ReplaceableStructuralControl {
    fn publish_prepared_on_control(
        &mut self,
        graph: PreparedGraph,
    ) -> Result<(), StructuralBoundaryFull> {
        Err(StructuralBoundaryFull::new(graph))
    }

    fn collect_retired_on_control(&mut self) -> Option<GraphRevision> {
        None
    }

    fn read_status_on_control(&self) -> GraphHandoffStatus {
        GraphHandoffStatus::with_active(GraphRevision::INITIAL)
    }
}

impl AudioStructuralGraphBoundary for ReplaceableStructuralAudio {
    fn take_prepared_on_audio(&mut self) -> Option<PreparedGraph> {
        None
    }

    fn return_retired_on_audio(&mut self, graph: PreparedGraph) -> Result<(), RetiredBoundaryFull> {
        Err(RetiredBoundaryFull::new(graph))
    }

    fn publish_status_on_audio(&mut self, _status: GraphHandoffStatus) {
        self.probe
            .status_publications
            .fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct ObservationProbe {
    publications: AtomicUsize,
    rendered_blocks: AtomicU64,
    rendered_frames: AtomicU64,
}

struct ProbeObservation {
    probe: Arc<ObservationProbe>,
}

struct ProbeObservationWriter {
    probe: Arc<ObservationProbe>,
}

struct ProbeObservationReader;

impl AudioObservation for ProbeObservation {
    type CallbackHandle = ProbeObservationWriter;
    type ControlHandle = ProbeObservationReader;

    fn into_handles(self) -> (Self::CallbackHandle, Self::ControlHandle) {
        (
            ProbeObservationWriter { probe: self.probe },
            ProbeObservationReader,
        )
    }
}

impl CallbackAudioObservation for ProbeObservationWriter {
    fn publish_from_callback(&mut self, snapshot: AudioObservationSnapshot) {
        self.probe.publications.fetch_add(1, Ordering::Relaxed);
        self.probe
            .rendered_blocks
            .store(snapshot.rendered_blocks(), Ordering::Relaxed);
        self.probe
            .rendered_frames
            .store(snapshot.rendered_frames(), Ordering::Relaxed);
    }
}

impl ControlAudioObservation for ProbeObservationReader {
    fn read_latest_on_control(&self) -> AudioObservationSnapshot {
        AudioObservationSnapshot::default()
    }
}

#[derive(Default)]
struct OutputProbe {
    started: AtomicBool,
    exact_callback_nonzero: AtomicBool,
    oversized_callback_nonzero: AtomicBool,
    rendered_samples: AtomicUsize,
}

struct ProbeAudioOutput {
    config: Option<AudioDeviceConfig>,
    runtime_error: Option<AudioDeviceRuntimeError>,
    probe: Arc<OutputProbe>,
}

struct ProbeNegotiatedAudioOutput {
    config: AudioDeviceConfig,
    runtime_error: Option<AudioDeviceRuntimeError>,
    probe: Arc<OutputProbe>,
}

impl AudioOutput for ProbeAudioOutput {
    type Negotiated = ProbeNegotiatedAudioOutput;

    fn negotiate(self) -> Result<Self::Negotiated, AudioOutputError> {
        let config = self
            .config
            .ok_or_else(|| AudioOutputError::new("controlled unsupported configuration"))?;
        Ok(ProbeNegotiatedAudioOutput {
            config,
            runtime_error: self.runtime_error,
            probe: self.probe,
        })
    }
}

impl NegotiatedAudioOutput for ProbeNegotiatedAudioOutput {
    fn config(&self) -> AudioDeviceConfig {
        self.config
    }

    fn start(
        self,
        mut render: AudioRenderCallback,
        mut on_runtime_error: AudioDeviceStatusCallback,
    ) -> Result<AudioStream, AudioOutputError> {
        self.probe.started.store(true, Ordering::Release);
        let capacity = self.config.render_capacity_frames();

        let mut exact = vec![0.0; capacity * 2];
        render(&mut exact);
        self.probe.exact_callback_nonzero.store(
            exact.iter().all(|sample| sample.abs() > 0.000_001),
            Ordering::Relaxed,
        );

        let mut oversized = vec![0.0; capacity * 3 * 2];
        render(&mut oversized);
        self.probe.oversized_callback_nonzero.store(
            oversized.iter().all(|sample| sample.abs() > 0.000_001),
            Ordering::Relaxed,
        );
        self.probe
            .rendered_samples
            .store(exact.len() + oversized.len(), Ordering::Relaxed);

        if let Some(error) = self.runtime_error {
            on_runtime_error(error);
        }
        Ok(AudioStream::new(()))
    }
}

struct OneTickWindow {
    close_requested: Arc<AtomicBool>,
}

impl AppWindow for OneTickWindow {
    fn run(
        &self,
        _on_input: AppInputCallback,
        projection: ProjectionCallback,
        _audio_observation: AudioObservationCallback,
        _midi_activity: MidiActivityObservationCallback,
        _on_session_command: SessionCommandCallback,
        _document_projection: SessionDocumentProjectionCallback,
        mut on_tick: TickCallback,
        _on_frame: FrameObservationCallback,
    ) -> Result<(), WindowError> {
        let _visible_state = projection();
        self.close_requested
            .store(!on_tick(Duration::from_millis(16)), Ordering::Relaxed);
        Ok(())
    }
}

type RuntimeApplication = StandaloneApplication<
    LockFreeAudioBoundary,
    ReplaceableStructuralBoundary,
    ProbeObservation,
    FixtureMidiSource,
    OneTickWindow,
    ProbeAudioOutput,
>;

fn globals() -> crest_synth::mixer::global_parameters::GlobalParameters {
    ApplicationConfig::default().global_parameters()
}

fn initial_audio_boundary() -> LockFreeAudioBoundary {
    let initial = ParameterSnapshot::new(0, globals(), MixerState::default(), &[]).unwrap();
    LockFreeAudioBoundary::new(16, initial)
}

fn hidef_provider() -> Box<dyn InstrumentCapabilityProvider> {
    Box::new(
        crest_synth::adapter::production_instruments::production_soundfont_capability().unwrap(),
    )
}

fn braids_provider() -> Box<dyn InstrumentCapabilityProvider> {
    Box::new(BraidsCapability::new().unwrap())
}

#[allow(clippy::too_many_arguments)]
fn runtime_application(
    providers: Vec<Box<dyn InstrumentCapabilityProvider>>,
    preparers: Vec<Box<dyn InstrumentPreparer>>,
    structural_probe: Arc<StructuralProbe>,
    observation_probe: Arc<ObservationProbe>,
    output_probe: Arc<OutputProbe>,
    close_requested: Arc<AtomicBool>,
    config: Option<AudioDeviceConfig>,
    runtime_error: Option<AudioDeviceRuntimeError>,
) -> Result<RuntimeApplication, ApplicationError> {
    StandaloneApplication::new(
        initial_audio_boundary(),
        providers,
        preparers,
        ReplaceableStructuralBoundary {
            probe: structural_probe,
        },
        ProbeObservation {
            probe: observation_probe,
        },
        FixtureMidiSource,
        OneTickWindow { close_requested },
        ProbeAudioOutput {
            config,
            runtime_error,
            probe: output_probe,
        },
        ApplicationConfig::default(),
    )
    .map(|application| {
        application.with_default_session_blueprint(
            crest_synth::control::DefaultSessionBlueprint::new(
                CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap(),
            ),
        )
    })
}

fn supported_config() -> AudioDeviceConfig {
    AudioDeviceConfig::new(TEST_SAMPLE_RATE, 6, AudioSampleFormat::I16, TEST_MAX_FRAMES).unwrap()
}

#[test]
fn production_composition_validates_every_injected_registration_before_publication() {
    fn outcome(
        providers: Vec<Box<dyn InstrumentCapabilityProvider>>,
        preparers: Vec<Box<dyn InstrumentPreparer>>,
    ) -> (
        Result<RuntimeApplication, ApplicationError>,
        Arc<StructuralProbe>,
    ) {
        let structural = Arc::new(StructuralProbe::default());
        let result = runtime_application(
            providers,
            preparers,
            Arc::clone(&structural),
            Arc::new(ObservationProbe::default()),
            Arc::new(OutputProbe::default()),
            Arc::new(AtomicBool::new(false)),
            Some(supported_config()),
            None,
        );
        (result, structural)
    }

    let probe = Arc::new(PreparationProbe::default());
    let (matching, structural) = outcome(
        vec![hidef_provider()],
        vec![TonePreparer::boxed(HIDEF_CAPABILITY_ID, &probe)],
    );
    assert!(matching.is_ok());
    assert_eq!(structural.splits.load(Ordering::Relaxed), 0);

    let cases = [
        outcome(
            vec![hidef_provider(), hidef_provider()],
            vec![TonePreparer::boxed(HIDEF_CAPABILITY_ID, &probe)],
        ),
        outcome(
            vec![hidef_provider()],
            vec![
                TonePreparer::boxed(HIDEF_CAPABILITY_ID, &probe),
                TonePreparer::boxed(HIDEF_CAPABILITY_ID, &probe),
            ],
        ),
        outcome(
            vec![hidef_provider(), braids_provider()],
            vec![TonePreparer::boxed(HIDEF_CAPABILITY_ID, &probe)],
        ),
        outcome(
            vec![hidef_provider()],
            vec![
                TonePreparer::boxed(HIDEF_CAPABILITY_ID, &probe),
                TonePreparer::boxed("instrument.test.unknown", &probe),
            ],
        ),
        outcome(
            vec![hidef_provider()],
            vec![TonePreparer::boxed(BRAIDS_CAPABILITY_ID, &probe)],
        ),
    ];

    let expected = [
        "duplicate-provider",
        "duplicate-preparer",
        "missing-preparer",
        "unknown-preparer",
        "mismatched-registration",
    ];
    for ((result, structural), expected) in cases.into_iter().zip(expected) {
        let error = match result {
            Ok(_) => panic!("invalid composition was accepted"),
            Err(error) => error,
        };
        let actual = match error {
            ApplicationError::InstrumentComposition(
                InstrumentCompositionError::DuplicateProvider { .. },
            ) => "duplicate-provider",
            ApplicationError::InstrumentComposition(
                InstrumentCompositionError::DuplicatePreparer { .. },
            ) => "duplicate-preparer",
            ApplicationError::InstrumentComposition(
                InstrumentCompositionError::MissingPreparer { .. },
            ) => "missing-preparer",
            ApplicationError::InstrumentComposition(
                InstrumentCompositionError::UnknownPreparer { .. },
            ) => "unknown-preparer",
            ApplicationError::InstrumentComposition(
                InstrumentCompositionError::MismatchedRegistration { .. },
            ) => "mismatched-registration",
            other => panic!("unexpected composition result: {other}"),
        };
        assert_eq!(actual, expected);
        assert_eq!(structural.splits.load(Ordering::Relaxed), 0);
    }
}

#[test]
fn audio_renderer_realtime_contract() {
    let preparation = Arc::new(PreparationProbe::default());
    let structural = Arc::new(StructuralProbe::default());
    let observation = Arc::new(ObservationProbe::default());
    let output = Arc::new(OutputProbe::default());
    let close_requested = Arc::new(AtomicBool::new(false));
    let application = runtime_application(
        vec![hidef_provider()],
        vec![TonePreparer::boxed(HIDEF_CAPABILITY_ID, &preparation)],
        Arc::clone(&structural),
        Arc::clone(&observation),
        Arc::clone(&output),
        Arc::clone(&close_requested),
        Some(supported_config()),
        None,
    )
    .unwrap();

    application.run().unwrap();

    assert_eq!(preparation.prepared.load(Ordering::Relaxed), 1);
    assert_eq!(
        f32::from_bits(preparation.sample_rate_bits.load(Ordering::Relaxed)),
        TEST_SAMPLE_RATE
    );
    assert_eq!(
        preparation.max_frames.load(Ordering::Relaxed),
        TEST_MAX_FRAMES
    );
    assert!(!preparation.oversized_render.load(Ordering::Relaxed));
    assert!(output.started.load(Ordering::Acquire));
    assert!(output.exact_callback_nonzero.load(Ordering::Relaxed));
    assert!(output.oversized_callback_nonzero.load(Ordering::Relaxed));
    assert_eq!(
        output.rendered_samples.load(Ordering::Relaxed),
        TEST_MAX_FRAMES * 8
    );
    assert_eq!(observation.publications.load(Ordering::Relaxed), 4);
    assert_eq!(observation.rendered_blocks.load(Ordering::Relaxed), 4);
    assert_eq!(
        observation.rendered_frames.load(Ordering::Relaxed),
        (TEST_MAX_FRAMES * 4) as u64
    );
    assert_eq!(structural.splits.load(Ordering::Relaxed), 1);
    assert!(structural.status_publications.load(Ordering::Relaxed) >= 5);
    assert!(!close_requested.load(Ordering::Relaxed));
    println!("CREST_RT_VALIDATION audio_renderer_realtime_contract passed");
}

#[test]
fn unsupported_device_configuration_is_rejected_before_graph_preparation() {
    let preparation = Arc::new(PreparationProbe::default());
    let structural = Arc::new(StructuralProbe::default());
    let output = Arc::new(OutputProbe::default());
    let application = runtime_application(
        vec![hidef_provider()],
        vec![TonePreparer::boxed(HIDEF_CAPABILITY_ID, &preparation)],
        Arc::clone(&structural),
        Arc::new(ObservationProbe::default()),
        Arc::clone(&output),
        Arc::new(AtomicBool::new(false)),
        None,
        None,
    )
    .unwrap();

    let error = application.run().unwrap_err();
    assert!(matches!(error, ApplicationError::AudioOutput(_)));
    assert_eq!(preparation.prepared.load(Ordering::Relaxed), 0);
    assert_eq!(structural.splits.load(Ordering::Relaxed), 0);
    assert!(!output.started.load(Ordering::Relaxed));
}

#[test]
fn post_start_device_failure_becomes_an_exact_application_error() {
    let preparation = Arc::new(PreparationProbe::default());
    let close_requested = Arc::new(AtomicBool::new(false));
    let application = runtime_application(
        vec![hidef_provider()],
        vec![TonePreparer::boxed(HIDEF_CAPABILITY_ID, &preparation)],
        Arc::new(StructuralProbe::default()),
        Arc::new(ObservationProbe::default()),
        Arc::new(OutputProbe::default()),
        Arc::clone(&close_requested),
        Some(supported_config()),
        Some(AudioDeviceRuntimeError::DeviceUnavailable),
    )
    .unwrap();

    let error = application.run().unwrap_err();
    assert!(matches!(
        error,
        ApplicationError::AudioDeviceRuntime(AudioDeviceRuntimeError::DeviceUnavailable)
    ));
    assert!(close_requested.load(Ordering::Relaxed));
}

struct GraphFixture {
    graph: PreparedGraph,
    parameters: ParameterSnapshot,
    patch: Patch,
}

fn graph_fixture(
    revision: u64,
    generation: u64,
    sample_rate: f32,
    max_frames: usize,
    probe: &Arc<PreparationProbe>,
) -> GraphFixture {
    let provider =
        crest_synth::adapter::production_instruments::production_soundfont_capability().unwrap();
    let registry = provider.registry().unwrap();
    let patch = Patch::new(
        PatchId::new(1).unwrap(),
        "Runtime contract".to_owned(),
        create_soundfont_config(&provider, SoundFontInstrument::new(0, 0, false).unwrap()).unwrap(),
        MidiChannel::new(0).unwrap(),
        PatchOutput::to_track(MixerTrackId::new(0).unwrap()),
    );
    let revision = GraphRevision::new(revision).unwrap();
    let parameters = ParameterSnapshot::for_graph(
        generation,
        revision,
        globals(),
        MixerState::default(),
        &[RtPatchParameters::new(patch.id(), patch.output())],
    )
    .unwrap();
    let preparers = vec![TonePreparer::boxed(HIDEF_CAPABILITY_ID, probe)];
    let graph = PreparedGraphBuilder::new(&registry, &preparers)
        .build(
            revision,
            std::slice::from_ref(&patch),
            parameters.clone(),
            sample_rate,
            max_frames,
        )
        .unwrap();
    GraphFixture {
        graph,
        parameters,
        patch,
    }
}

#[test]
fn prepared_graph_handoff() {
    let probe = Arc::new(PreparationProbe::default());
    let first = graph_fixture(1, 10, 48_000.0, 8, &probe);
    let second = graph_fixture(2, 20, 48_000.0, 8, &probe);
    let boundary = LockFreeAudioBoundary::new(8, first.parameters);
    let (_control, audio) = boundary.into_handles();
    let structural = LockFreeStructuralGraphBoundary::new(
        1,
        1,
        GraphHandoffStatus::with_active(GraphRevision::INITIAL),
    )
    .unwrap();
    let (structural_control, structural_audio) = structural.into_handles();
    let mut coordinator = StructuralGraphCoordinator::new(structural_control, &first.graph);
    coordinator.publish(second.graph).unwrap();
    let mut renderer = AudioRenderer::new(audio, structural_audio, first.graph);
    let mut output = [0.0; 16];

    renderer.render(&mut output);
    assert_eq!(renderer.active_revision(), GraphRevision::new(2).unwrap());
    assert!(output.iter().all(|sample| sample.abs() > 0.000_001));
    assert_eq!(probe.drops.load(Ordering::Relaxed), 0);

    let progress = coordinator.poll();
    assert_eq!(progress.collected_count(), 1);
    assert_eq!(
        progress.completed_revision(),
        Some(GraphRevision::new(2).unwrap())
    );
    assert_eq!(probe.drops.load(Ordering::Relaxed), 1);
    println!("CREST_RT_VALIDATION prepared_graph_handoff passed");
}

#[test]
fn audio_observation_realtime_contract() {
    let probe = Arc::new(PreparationProbe::default());
    let fixture = graph_fixture(1, 1, 48_000.0, 8, &probe);
    let boundary = LockFreeAudioBoundary::new(8, fixture.parameters);
    let (mut control, audio) = boundary.into_handles();
    let unknown_patch = PatchId::new(99).unwrap();
    control
        .push_command(AudioCommand::patch_midi(
            unknown_patch,
            MidiMessage::try_new(
                MidiChannel::new(0).unwrap(),
                MidiMessageKind::NoteOn,
                60,
                100,
            )
            .unwrap(),
        ))
        .unwrap();
    let structural = LockFreeStructuralGraphBoundary::new(
        1,
        1,
        GraphHandoffStatus::with_active(GraphRevision::INITIAL),
    )
    .unwrap();
    let (_structural_control, structural_audio) = structural.into_handles();
    let (observation_writer, observation_reader) = AtomicAudioObservation::default().into_handles();
    let mut renderer =
        AudioRenderer::with_observation(audio, structural_audio, fixture.graph, observation_writer);
    let mut output = [0.0; 16];

    renderer.render(&mut output);

    let snapshot = observation_reader.read_latest_on_control();
    assert_eq!(probe.dispatches.load(Ordering::Relaxed), 0);
    assert_eq!(snapshot.commands_consumed(), 1);
    assert_eq!(snapshot.routing_failures(), 1);
    assert_eq!(snapshot.last_unknown_patch_id(), Some(unknown_patch));
    assert_eq!(snapshot.rendered_blocks(), 1);
    assert_eq!(snapshot.rendered_frames(), 8);
    assert!(output.iter().all(|sample| sample.abs() > 0.000_001));
    assert_eq!(fixture.patch.id(), PatchId::new(1).unwrap());
    println!("CREST_RT_VALIDATION audio_observation_realtime_contract passed");
}

fn runtime_application_with_effects(
    effect_providers: Vec<Box<dyn EffectCapabilityProvider>>,
    effect_preparers: Vec<Box<dyn EffectPreparer>>,
    device_config: AudioDeviceConfig,
    preparation_probe: &Arc<PreparationProbe>,
    output_probe: &Arc<OutputProbe>,
) -> Result<RuntimeApplication, ApplicationError> {
    StandaloneApplication::new_with_effects(
        initial_audio_boundary(),
        vec![hidef_provider()],
        vec![TonePreparer::boxed(HIDEF_CAPABILITY_ID, preparation_probe)],
        effect_providers,
        effect_preparers,
        ReplaceableStructuralBoundary {
            probe: Arc::new(StructuralProbe::default()),
        },
        ProbeObservation {
            probe: Arc::new(ObservationProbe::default()),
        },
        FixtureMidiSource,
        OneTickWindow {
            close_requested: Arc::new(AtomicBool::new(false)),
        },
        ProbeAudioOutput {
            config: Some(device_config),
            runtime_error: None,
            probe: Arc::clone(output_probe),
        },
        ApplicationConfig::default(),
    )
    .map(|application| {
        application.with_default_session_blueprint(
            crest_synth::control::DefaultSessionBlueprint::new(
                CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap(),
            ),
        )
    })
}

/// Empty startup send chains do not require any specific effect provider.
/// A partial registry still boots through the complete production root.
#[test]
fn empty_startup_sends_accept_a_partial_effect_registry() {
    let preparation = Arc::new(PreparationProbe::default());
    let output = Arc::new(OutputProbe::default());
    let application = runtime_application_with_effects(
        vec![Box::new(ChorusCapability::new().unwrap())],
        vec![Box::new(ChorusPreparer::new().unwrap())],
        supported_config(),
        &preparation,
        &output,
    )
    .unwrap();

    application.run().unwrap();
    assert!(output.started.load(Ordering::Acquire));
    assert!(preparation.prepared.load(Ordering::Relaxed) > 0);
    println!("CREST_RT_VALIDATION empty_startup_sends_accept_a_partial_effect_registry passed");
}

/// The complete production effect registry also boots with empty startup sends.
#[test]
fn declared_default_return_composition_boots_the_production_root() {
    let preparation = Arc::new(PreparationProbe::default());
    let output = Arc::new(OutputProbe::default());
    // The production chorus prepares at exactly 48 kHz, so the healthy boot
    // negotiates the production sample rate.
    let device_config =
        AudioDeviceConfig::new(48_000.0, 6, AudioSampleFormat::I16, TEST_MAX_FRAMES).unwrap();
    let application = runtime_application_with_effects(
        crest_synth::adapter::production_effects::production_effect_providers().unwrap(),
        crest_synth::adapter::production_effects::production_effect_preparers().unwrap(),
        device_config,
        &preparation,
        &output,
    )
    .unwrap();

    application.run().unwrap();
    assert!(output.started.load(Ordering::Acquire));
    println!(
        "CREST_RT_VALIDATION declared_default_return_composition_boots_the_production_root passed"
    );
}

/// Drives one accepted occupancy request through TopologyPrepared and the
/// activation acknowledgement, exactly as the production orchestration does.
fn commit_pending_topology(state: &mut AppState) {
    let correlation = state
        .engine_selection()
        .correlation()
        .expect("an accepted occupancy request owns its correlation")
        .clone();
    for lifecycle in [
        crest_synth::control::EngineSelectionStatusKind::Validating,
        crest_synth::control::EngineSelectionStatusKind::Preparing,
    ] {
        state
            .apply(AppEvent::EngineSelectionLifecycleAdvanced {
                request_id: correlation.request_id(),
                lifecycle,
            })
            .unwrap();
    }
    let source = correlation.source_graph_revision();
    let target = source
        .checked_next()
        .expect("the fixture revision space is not exhausted");
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

/// A chain with slot 0 empty and slot 1 occupied must come back from the
/// composition-root round trip (event-log install record -> reconstructed
/// Patch) with the occupant still at slot 1 and slot 0 still empty — the
/// never-compacted per-position contract of the Patch effect chain.
#[test]
fn gapped_chain_survives_the_composition_root_round_trip() {
    let registry =
        crest_synth::adapter::production_instruments::production_capability_registry().unwrap();
    let effects = crest_synth::adapter::production_effects::production_effect_registry().unwrap();
    let provider =
        crest_synth::adapter::production_instruments::production_soundfont_capability().unwrap();
    let patch = Patch::new(
        PatchId::new(1).unwrap(),
        "Gapped chain".to_owned(),
        create_soundfont_config(&provider, SoundFontInstrument::new(0, 0, false).unwrap()).unwrap(),
        MidiChannel::new(0).unwrap(),
        PatchOutput::to_track(MixerTrackId::new(0).unwrap()),
    );

    // Build the gapped chain through the production reducer: install the
    // chain-less Patch, then occupy position 1 through the structural
    // occupancy handshake. Slot 0 stays empty by construction.
    let mut state = AppState::new_with_effects(registry.clone(), effects.clone(), globals());
    state.apply(AppEvent::InstallPatches(vec![patch])).unwrap();
    state
        .apply(AppEvent::SetSlotOccupancy {
            patch_id: PatchId::new(1).unwrap(),
            slot: EffectSlotIndex::ALL[1],
            entry: Some(EffectCapabilityId::new("effect.chorus").unwrap()),
        })
        .unwrap();
    commit_pending_topology(&mut state);
    let gapped = state.patches()[0].clone();
    assert!(gapped.effect_slots()[0].is_none());
    assert!(gapped.effect_slots()[1].is_some());
    assert!(gapped.effect_slots()[2].is_none());

    // Record the gapped Patch's installation through the production event
    // log, exactly as the composition root records the startup fixture.
    let initial = ParameterSnapshot::new(0, globals(), MixerState::default(), &[]).unwrap();
    let (control, _audio) = LockFreeAudioBoundary::new(16, initial).into_handles();
    let mut app_loop = AppLoop::with_event_log(
        AppState::new_with_effects(registry, effects, globals()),
        StateProjector::for_graph(GraphRevision::INITIAL),
        control,
        EventLog::new(8).unwrap(),
    )
    .unwrap();
    app_loop
        .dispatch_from(
            AppEvent::InstallPatches(vec![gapped.clone()]),
            EventSource::Startup,
        )
        .unwrap();

    let rebuilt = installed_patches_from_log(&app_loop.event_log()).unwrap();
    assert_eq!(rebuilt.len(), 1);
    for position in 0..MAX_EFFECT_SLOTS {
        assert_eq!(
            rebuilt[0].effect_slots()[position],
            gapped.effect_slots()[position],
            "position {position} occupancy must survive the composition-root round trip exactly"
        );
    }
    assert_eq!(
        rebuilt[0], gapped,
        "the reconstructed Patch must equal the installed Patch"
    );
    println!("CREST_RT_VALIDATION gapped_chain_survives_the_composition_root_round_trip passed");
}

#[test]
fn zero_selection_validation_is_rejected_even_if_broad_output_looks_successful() {
    let status = Command::new("bash")
        .arg("scripts/run_exact_test_validation.sh")
        .arg("--self-test")
        .status()
        .expect("the exact-test validation guard runs");
    assert!(status.success());
    println!("CREST_ACCEPTANCE production_runtime_contracts passed");
}
