use crate::adapter::atomic_audio_observation::AtomicAudioObservation;
use crate::adapter::cpal_audio_output::CpalAudioOutput;
use crate::adapter::fixed_midi_event_source::FixedMidiEventSource;
use crate::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
use crate::adapter::lock_free_structural_graph_boundary::{
    LockFreeStructuralGraphBoundary, StructuralBoundaryConfigurationError,
};
use crate::adapter::production_effects::{
    production_effect_preparers, production_effect_providers, ProductionEffectCompositionError,
};
use crate::adapter::production_instruments::{
    production_instrument_preparers, production_instrument_providers,
};
use crate::adapter::threaded_graph_preparation_worker::{
    ThreadedGraphPreparationWorker, ThreadedGraphPreparationWorkerError,
};
use crate::adapter::threaded_midi_device_worker::{
    ThreadedMidiDeviceWorker, ThreadedMidiDeviceWorkerError,
};
use crate::adapter::threaded_session_candidate_worker::{
    ThreadedSessionCandidateWorker, ThreadedSessionCandidateWorkerError,
};
use crate::control::app_event::{AppEvent, Direction};
use crate::control::app_loop::{AppLoop, MidiDeviceAdvanceError, StructuralAdvanceError};
use crate::control::app_state::{AppState, EventRejection};
use crate::control::event_log::EventLog;
use crate::control::event_record::{EventInput, EventSource, PatchInput};
use crate::control::state_projector::{StateProjectionError, StateProjector};
use crate::control::{capture_default_session, DefaultSessionBlueprint, DefaultSessionError};
use crate::kernel::midi_channel::MidiChannel;
use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crate::kernel::patch_id::PatchId;
use crate::mixer::global_parameters::GlobalParameters;
use crate::mixer::mixer_state::MixerState;
use crate::mixer::mixer_track_id::MixerTrackId;
use crate::mixer::mixer_track_parameters::MixerTrackParameters;
use crate::real_time::audio_boundary::{
    AudioBoundary, AudioThreadBoundary, BoundaryFull, ControlAudioBoundary,
};
use crate::real_time::audio_command::AudioCommand;
use crate::real_time::audio_observation::{AudioObservation, ControlAudioObservation};
use crate::real_time::audio_renderer::AudioRenderer;
use crate::real_time::callback_safety::{
    callback_safety_snapshot, reset_callback_safety_counts, CallbackSafetySnapshot,
};
use crate::real_time::graph_revision::GraphRevision;
use crate::real_time::parameter_snapshot::{ParameterSnapshot, RtPatchParameters};
use crate::real_time::prepared_graph::PreparedGraph;
use crate::real_time::prepared_graph_builder::{GraphPreparationError, PreparedGraphBuilder};
use crate::real_time::structural_graph_boundary::{
    AudioStructuralGraphBoundary, StructuralGraphBoundary,
};
use crate::real_time::GraphHandoffStatus;
use crate::shell::app_window::{
    AppInputCallback, AppWindow, AudioObservationCallback, FrameObservationCallback,
    MidiActivityObservationCallback, ProjectionCallback, SessionCommand, SessionCommandCallback,
    SessionDocumentProjectionCallback, TickCallback, WindowError,
};
use crate::shell::audio_device_status::{AudioDeviceStatusBoundary, AudioDeviceStatusReader};
use crate::shell::audio_output::{
    AudioDeviceConfig, AudioDeviceRuntimeError, AudioDeviceStatusCallback, AudioOutput,
    AudioOutputError, AudioRenderCallback, AudioSampleFormat, AudioStream, NegotiatedAudioOutput,
};
use crate::shell::hosted_application::{ApplicationHost, HostClock};
use crate::shell::hosted_synth::HostedSynthControl;
use crate::shell::webview::TauriWebviewWindow;
use crate::shell::{
    SessionFilePort, SessionLifecycleCoordinator, SessionLifecycleError, StandardSessionFileSystem,
    ThreadedSessionSaveWorker, ThreadedSessionSaveWorkerError,
};
use crate::synth::effect_slot_id::EffectSlotIndex;
use crate::synth::instrument_capability::{CapabilityError, CapabilityRegistry};
use crate::synth::instrument_capability_provider::InstrumentCapabilityProvider;
use crate::synth::instrument_composition::{
    compose_instrument_registry, InstrumentCompositionError,
};
use crate::synth::instrument_preparer::{InstrumentPreparationError, InstrumentPreparer};
use crate::synth::patch::Patch;
use crate::synth::{
    compose_effect_registry, DescriptorDefaultConfigFactory, EffectCapabilityProvider,
    EffectCapabilityRegistry, EffectCompositionError, EffectPreparer, EffectSlotId, VoicePolicy,
};
use crate::testing::automatic_midi_test::{AutomaticMidiTest, TestInputError};
use crate::testing::demo_scene::{DemoScene, DemoSceneError};
use crate::testing::demo_scene_report::{DemoCoverageGroup, DemoSceneReport, DemoSceneReportError};
use crate::testing::exhaustive_gui_demo::{ExhaustiveGuiDemo, ExhaustiveGuiDemoError};
use crate::testing::full_instrument_effect_demo::{
    FullDemoError, FullDemoPlan, FullDemoSelection, FullInstrumentEffectDemo,
    ListeningDemoSelection,
};
use crate::testing::midi_event_source::MidiEventSource;
use crate::testing::{
    DeterministicGraphPreparationHandle, DeterministicGraphPreparationWorker, LiveCheckpoint,
    LiveDemoError, LiveDemoReport, LiveDemoRunner, LiveDemoScene, LiveDemoSceneError,
    LiveMixerRoutingEvidence, LiveMixerSceneEvidence, RuntimeAudioWitness,
    SixteenTrackMixerRoutingObservation,
};
use core::fmt;
use serde::Serialize;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// The one SoundFont loaded by the standalone application.
pub const STANDALONE_SOUNDFONT_PATH: &str = "./sf2/HiDef.sf2";

const AUDIO_COMMAND_CAPACITY: usize = 1_024;
const SMOKE_TICK_COUNT: usize = 256;
const SMOKE_TICK_DURATION: Duration = Duration::from_millis(20);
const LIVE_EVENT_LOG_CAPACITY: usize = 65_536;
const LIVE_FIXTURE_EVENT_ALLOWANCE: usize = 60_000;

/// The native title of the autonomous live window — unchanged across the
/// webview shell cutover.
const LIVE_DEMO_WINDOW_TITLE: &str = "crest-synth — autonomous live demo";

/// Composes the one shell window every retained live scene runs on, using the
/// normal startup order, real `TauriWebviewWindow`, and physical
/// `CpalAudioOutput`:
/// the same `TauriWebviewWindow` construction the interactive composition
/// root uses, carrying the autonomous live title.
fn live_demo_window() -> TauriWebviewWindow {
    TauriWebviewWindow::new(LIVE_DEMO_WINDOW_TITLE)
}
const CHANNEL_SEPARATOR: &str = "------------------------------------------------------------";

/// Fixed startup values shared by normal and headless execution.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ApplicationConfig {
    test_midi_on_launch: bool,
    sample_rate: f32,
    max_frames: usize,
    global_parameters: GlobalParameters,
}

impl ApplicationConfig {
    pub const fn new(
        sample_rate: f32,
        max_frames: usize,
        global_parameters: GlobalParameters,
    ) -> Self {
        Self {
            test_midi_on_launch: false,
            sample_rate,
            max_frames,
            global_parameters,
        }
    }

    pub const fn sample_rate(self) -> f32 {
        self.sample_rate
    }

    pub const fn with_test_midi_on_launch(mut self) -> Self {
        self.test_midi_on_launch = true;
        self
    }

    pub const fn max_frames(self) -> usize {
        self.max_frames
    }

    pub const fn global_parameters(self) -> GlobalParameters {
        self.global_parameters
    }
}

impl Default for ApplicationConfig {
    fn default() -> Self {
        Self::new(
            48_000.0,
            1_024,
            GlobalParameters::new(0.0)
                .expect("the standalone defaults satisfy every global parameter bound"),
        )
    }
}

/// Forwards the `AppWindow` port through a boxed window, so a composition
/// that owns its window behind the port's trait object still satisfies
/// `StandaloneApplication`'s single generic window type. Pure forwarding:
/// behavior is the boxed implementation's, unchanged. The production
/// composition root constructs the one shell (`TauriWebviewWindow`)
/// directly and no code path substitutes another window for it.
impl<Window: AppWindow + ?Sized> AppWindow for Box<Window> {
    fn session_dialog_port(&self) -> Box<dyn crate::shell::SessionDialogPort> {
        self.as_ref().session_dialog_port()
    }

    fn run(
        &self,
        on_input: AppInputCallback,
        projection: ProjectionCallback,
        audio_observation: AudioObservationCallback,
        midi_activity: MidiActivityObservationCallback,
        on_session_command: SessionCommandCallback,
        document_projection: SessionDocumentProjectionCallback,
        on_tick: TickCallback,
        on_frame: FrameObservationCallback,
    ) -> Result<(), WindowError> {
        self.as_ref().run(
            on_input,
            projection,
            audio_observation,
            midi_activity,
            on_session_command,
            document_projection,
            on_tick,
            on_frame,
        )
    }
}

/// Deliberate smoke-path falsification selected by the command-line harness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DegenerateMode {
    Audio,
    Control,
}

/// Measurements emitted by the deterministic headless application path.
///
/// Field names intentionally match the behavioral witness schema consumed by
/// the composition-root observation mode.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SmokeObservation {
    pub active_graph_revision: u64,
    pub audio_changed: bool,
    pub automatic_midi: bool,
    pub alternating_capabilities: bool,
    pub braids_patches: usize,
    pub callback_allocations: usize,
    pub callback_destructions: usize,
    pub channel_separators: usize,
    pub distinct_patch_channels: bool,
    pub distinct_patch_stems: bool,
    pub edited_patch_audio_changed: bool,
    pub edited_patch_id: u32,
    pub engine_consumed_value: bool,
    pub event_commands_delivered: usize,
    pub parsed_soundfont_banks: usize,
    pub prepared_instruments: usize,
    pub one_value_changed: bool,
    pub parameter_published: bool,
    pub patch_rows: usize,
    pub peak: f32,
    pub presets_match: bool,
    pub round_robin_channels: bool,
    pub soundfont_patches: usize,
    pub state_roundtrip: bool,
    pub text_matches_state: bool,
    pub unedited_patch_audio_unchanged: bool,
    pub per_patch_audio_isolated: bool,
    pub boundary_noop_nonfatal: bool,
    pub post_boundary_edit_accepted: bool,
}

/// Measured Phase One shell and teardown evidence emitted after live ownership ends.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GraphicalShellLiveObservation {
    context_line_visible: bool,
    identity_header_visible: bool,
    main_workspace_visible: bool,
    persistent_side_region_visible: bool,
    footer_visible: bool,
    patch_context_observed: bool,
    mixer_context_observed: bool,
    #[serde(flatten)]
    mixer_routing: LiveMixerRoutingEvidence,
    #[serde(skip_serializing_if = "Option::is_none")]
    live_mixer: Option<LiveMixerSceneEvidence>,
    effects_and_buses: Option<crate::testing::LiveEffectsAndBusesEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail_and_assets: Option<crate::testing::LiveDetailAssetsEvidence>,
    /// The functional Patch editor observation, resolved here because the
    /// teardown half of its schema — window close, stream release, graph
    /// collection, callback safety — is only knowable after the host returns.
    #[serde(skip_serializing_if = "Option::is_none")]
    functional_patch_editor: Option<crate::testing::FunctionalPatchEditorObservation>,
    physical_audio_nonzero: bool,
    active_notes_after_cleanup: u32,
    window_closed: bool,
    stream_released: bool,
    owned_graphs_remaining: usize,
}

impl GraphicalShellLiveObservation {
    fn from_completed_report(
        report: &LiveDemoReport,
        owned_graphs_remaining: usize,
        callback_safety: CallbackSafetySnapshot,
    ) -> Self {
        let shell = report.shell_coverage();
        let functional_patch_editor = report.patch_editor().map(|measurement| {
            measurement.resolve(
                // The final state's installed order, not the scene's own
                // subject: every second-Patch counter keys off this (F-47).
                &installed_patch_order(report),
                crate::testing::PatchEditorTeardown {
                    voice_limit_refusals: report.final_audio_observation().voice_limit_refusals(),
                    events_dropped: report.event_log().dropped_records(),
                    callback_allocations: callback_safety.allocations() as u64,
                    callback_destructions: callback_safety.destructions() as u64,
                    qualifying_webview_frames: shell.qualifying_frames() as u32,
                    physical_audio_nonzero: shell.physical_audio_nonzero(),
                    desktop_viewport_painted: shell.desktop_viewport_painted(),
                    active_notes_after_cleanup: report.final_audio_observation().active_notes(),
                    window_closed: true,
                    stream_released: true,
                    owned_graphs_remaining: owned_graphs_remaining as u32,
                },
            )
        });
        Self {
            context_line_visible: shell.context_line_visible(),
            identity_header_visible: shell.identity_header_visible(),
            main_workspace_visible: shell.main_workspace_visible(),
            persistent_side_region_visible: shell.persistent_side_region_visible(),
            footer_visible: shell.footer_visible(),
            patch_context_observed: shell.patch_context_observed(),
            mixer_context_observed: shell.mixer_context_observed(),
            mixer_routing: report.mixer_routing().with_callback_safety(callback_safety),
            live_mixer: report.live_mixer(),
            effects_and_buses: report.effects_and_buses().cloned(),
            detail_and_assets: report.detail_and_assets().cloned(),
            functional_patch_editor,
            physical_audio_nonzero: shell.physical_audio_nonzero(),
            active_notes_after_cleanup: report.final_audio_observation().active_notes(),
            window_closed: true,
            stream_released: true,
            owned_graphs_remaining,
        }
    }

    pub fn complete(&self) -> bool {
        self.context_line_visible
            && self.identity_header_visible
            && self.main_workspace_visible
            && self.persistent_side_region_visible
            && self.footer_visible
            && self.patch_context_observed
            && self.mixer_context_observed
            && self.mixer_routing.is_complete()
            && self
                .live_mixer
                .as_ref()
                .is_none_or(LiveMixerSceneEvidence::is_complete)
            && self
                .effects_and_buses
                .as_ref()
                .is_none_or(crate::testing::LiveEffectsAndBusesEvidence::is_complete)
            && self
                .detail_and_assets
                .as_ref()
                .is_none_or(crate::testing::LiveDetailAssetsEvidence::is_complete)
            && self.physical_audio_nonzero
            && self.active_notes_after_cleanup == 0
            && self.window_closed
            && self.stream_released
            && self.owned_graphs_remaining == 0
    }

    pub const fn sixteen_track_mixer_routing(&self) -> SixteenTrackMixerRoutingObservation {
        SixteenTrackMixerRoutingObservation::new(
            self.mixer_routing,
            self.physical_audio_nonzero,
            self.active_notes_after_cleanup,
            self.window_closed,
            self.stream_released,
            self.owned_graphs_remaining,
        )
    }

    /// The additive dedicated Mixer witness. It is absent from every older
    /// retained scene, preserving their emitted schemas and target behavior.
    pub fn live_mixer(&self) -> Option<LiveMixerObservation> {
        self.live_mixer.as_ref().map(|scene| LiveMixerObservation {
            scene: scene.clone(),
            routing: self.sixteen_track_mixer_routing(),
        })
    }

    /// The functional Patch editor teardown projection, present only for the
    /// scene that measures it.
    pub const fn functional_patch_editor(
        &self,
    ) -> Option<&crate::testing::FunctionalPatchEditorObservation> {
        self.functional_patch_editor.as_ref()
    }

    /// The retained effects-and-buses teardown projection: the cumulative
    /// sixteen-track evidence plus the topology, responsiveness, and
    /// eight-destination measurements.
    pub fn effects_and_buses(&self) -> Option<EffectsAndBusesLiveObservation> {
        self.effects_and_buses
            .as_ref()
            .map(|evidence| EffectsAndBusesLiveObservation {
                routing: self.sixteen_track_mixer_routing(),
                effects_and_buses: evidence.clone(),
            })
    }

    pub const fn detail_and_assets(&self) -> Option<&crate::testing::LiveDetailAssetsEvidence> {
        self.detail_and_assets.as_ref()
    }
}

/// Final dedicated Mixer witness emitted after physical teardown.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LiveMixerObservation {
    #[serde(flatten)]
    scene: LiveMixerSceneEvidence,
    #[serde(flatten)]
    routing: SixteenTrackMixerRoutingObservation,
}

impl LiveMixerObservation {
    pub fn is_complete(&self) -> bool {
        self.scene.is_complete() && self.routing.is_complete()
    }

    pub const fn scene(&self) -> &LiveMixerSceneEvidence {
        &self.scene
    }
}

/// The installed Patch order read out of the completed report's final state
/// tree. This is the order every functional Patch editor counter keys off.
fn installed_patch_order(report: &LiveDemoReport) -> Vec<crate::kernel::PatchId> {
    serde_json::from_str::<serde_json::Value>(report.state_tree().json())
        .ok()
        .and_then(|value| {
            value
                .get("patches")
                .and_then(|patches| patches.as_array())
                .cloned()
        })
        .map(|patches| {
            patches
                .iter()
                .filter_map(|patch| patch.get("id").and_then(serde_json::Value::as_u64))
                .filter_map(|id| u32::try_from(id).ok())
                .filter_map(|id| crate::kernel::PatchId::new(id).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Final effects-and-buses witness emitted after physical teardown.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EffectsAndBusesLiveObservation {
    #[serde(flatten)]
    routing: SixteenTrackMixerRoutingObservation,
    #[serde(flatten)]
    effects_and_buses: crate::testing::LiveEffectsAndBusesEvidence,
}

impl EffectsAndBusesLiveObservation {
    pub fn is_complete(&self) -> bool {
        self.routing.is_complete() && self.effects_and_buses.is_complete()
    }
}

/// The retained live scene selected by the binary's stable flags.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveSceneKind {
    /// The additive dedicated Mixer scene and typed report.
    Mixer,
    /// The retained sixteen-track mixer-routing scene.
    SixteenTrackMixerRouting,
    /// The retained cumulative effects-and-buses scene.
    EffectsAndBuses,
    /// The cumulative Phase 7 detail, choice, browser, preview, and asset scene.
    DetailAndAssets { defeat_preview: bool },
    /// The functional Patch editor scene, whose subject is the second
    /// installed Patch. Additive: it does not subsume the cumulative scene.
    FunctionalPatchEditor {
        /// The declared controlled negative: the patch-selection gesture is
        /// removed and the journey stays on the first Patch.
        defeat_patch_selection: bool,
    },
}

/// A startup, control, fixture, device, or window failure.
#[derive(Debug)]
pub enum ApplicationError {
    BrowserPreferences(String),
    Capability(CapabilityError),
    InstrumentComposition(InstrumentCompositionError),
    ProductionInstrumentComposition(
        crate::adapter::production_instruments::ProductionInstrumentCompositionError,
    ),
    ProductionEffectComposition(ProductionEffectCompositionError),
    StructuralBoundaryConfiguration(StructuralBoundaryConfigurationError),
    EffectComposition(EffectCompositionError),
    /// The declared default bus-return occupancy failed to compose at the
    /// production root; startup aborts instead of substituting silent
    /// returns.
    DefaultBusReturns(crate::adapter::production_effects::ProductionEffectCompositionError),
    SampleCatalog(crate::synth::SampleAssetError),
    Instrument(InstrumentPreparationError),
    Graph(GraphPreparationError),
    GraphWorker(ThreadedGraphPreparationWorkerError),
    SessionCandidateWorker(ThreadedSessionCandidateWorkerError),
    SessionSaveWorker(ThreadedSessionSaveWorkerError),
    DefaultSession(DefaultSessionError),
    DefaultSessionBlueprintMissing,
    DefaultSessionPreparation(crate::control::SavedSessionRestoreError),
    SessionLifecycle(SessionLifecycleError),
    StartupSessionCorrelation,
    MidiDeviceWorker(ThreadedMidiDeviceWorkerError),
    MidiDevice(MidiDeviceAdvanceError),
    Structural(StructuralAdvanceError),
    StateProjection(StateProjectionError),
    TestInput(TestInputError),
    DemoScene(DemoSceneError),
    FullInstrumentEffectDemo(FullDemoError),
    ExhaustiveDemo(ExhaustiveGuiDemoError),
    DemoReport(DemoSceneReportError),
    LiveDemoScene(LiveDemoSceneError),
    LiveDemo(LiveDemoError),
    LiveDemoIncomplete,
    LiveGraphOwnershipIncomplete(usize),
    LiveEventLogCapacity,
    AudioOutput(AudioOutputError),
    AudioDeviceRuntime(AudioDeviceRuntimeError),
    Window(WindowError),
    Control(EventRejection),
    AudioBoundaryFull(BoundaryFull),
    ObservationOverflow,
    ObservationUnavailable,
    FixtureUnavailable,
    /// A recorded install carries an effect whose instance identity names no
    /// bounded chain position, so its recorded placement cannot be rebuilt.
    RecordedEffectPosition {
        patch_id: u32,
        slot_id: EffectSlotId,
    },
}

impl fmt::Display for ApplicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BrowserPreferences(error) => write!(formatter, "browser preferences: {error}"),
            Self::Capability(error) => write!(formatter, "capability setup failed: {error}"),
            Self::InstrumentComposition(error) => {
                write!(formatter, "instrument composition failed: {error}")
            }
            Self::ProductionInstrumentComposition(error) => {
                write!(formatter, "production instrument composition failed: {error}")
            }
            Self::ProductionEffectComposition(error) => {
                write!(formatter, "production effect composition failed: {error}")
            }
            Self::StructuralBoundaryConfiguration(error) => {
                write!(formatter, "structural graph boundary setup failed: {error}")
            }
            Self::EffectComposition(error) => {
                write!(formatter, "effect composition failed: {error}")
            }
            Self::DefaultBusReturns(error) => {
                write!(
                    formatter,
                    "default bus-return composition failed at startup: {error}"
                )
            }
            Self::SampleCatalog(error) => write!(formatter, "sample catalog failed: {error}"),
            Self::Instrument(error) => write!(formatter, "instrument preparation failed: {error}"),
            Self::Graph(error) => write!(formatter, "prepared graph setup failed: {error}"),
            Self::GraphWorker(error) => write!(formatter, "graph worker setup failed: {error}"),
            Self::SessionCandidateWorker(error) => {
                write!(formatter, "session candidate worker setup failed: {error}")
            }
            Self::SessionSaveWorker(error) => {
                write!(formatter, "session save worker setup failed: {error}")
            }
            Self::DefaultSession(error) => {
                write!(formatter, "default session setup failed: {error}")
            }
            Self::DefaultSessionBlueprintMissing => formatter.write_str(
                "the composition root did not designate a canonical default session blueprint",
            ),
            Self::DefaultSessionPreparation(error) => {
                write!(formatter, "default session preparation failed: {error}")
            }
            Self::SessionLifecycle(error) => write!(formatter, "session lifecycle failed: {error}"),
            Self::StartupSessionCorrelation => formatter.write_str(
                "prepared default graph does not match the first canonical session projection",
            ),
            Self::MidiDeviceWorker(error) => {
                write!(formatter, "MIDI device worker setup failed: {error}")
            }
            Self::MidiDevice(error) => write!(formatter, "physical MIDI control failed: {error}"),
            Self::Structural(error) => write!(formatter, "structural control failed: {error}"),
            Self::StateProjection(error) => {
                write!(formatter, "initial control projection failed: {error}")
            }
            Self::TestInput(error) => write!(formatter, "automatic MIDI input failed: {error}"),
            Self::FullInstrumentEffectDemo(error) => error.fmt(formatter),
            Self::DemoScene(error) => write!(formatter, "demo scene creation failed: {error}"),
            Self::ExhaustiveDemo(error) => write!(formatter, "exhaustive GUI demo failed: {error}"),
            Self::DemoReport(error) => write!(formatter, "demo report creation failed: {error}"),
            Self::LiveDemoScene(error) => {
                write!(formatter, "live demo scene creation failed: {error}")
            }
            Self::LiveDemo(error) => write!(formatter, "live demo failed: {error}"),
            Self::LiveDemoIncomplete => {
                formatter.write_str("live demo window closed before successful completion")
            }
            Self::LiveGraphOwnershipIncomplete(remaining) => write!(
                formatter,
                "live teardown retained {remaining} structural graph obligations"
            ),
            Self::LiveEventLogCapacity => formatter.write_str(
                "declared live EventLog capacity is insufficient for the frozen scene and fixture allowance",
            ),
            Self::AudioOutput(error) => write!(formatter, "audio output failed: {error}"),
            Self::AudioDeviceRuntime(error) => {
                write!(formatter, "running audio device failed: {error}")
            }
            Self::Window(error) => write!(formatter, "application window failed: {error}"),
            Self::Control(error) => write!(formatter, "control event was rejected: {error}"),
            Self::AudioBoundaryFull(error) => error.fmt(formatter),
            Self::ObservationOverflow => {
                formatter.write_str("smoke observation count exceeds the platform usize range")
            }
            Self::ObservationUnavailable => {
                formatter.write_str("smoke observation could not measure two sounding Patch stems")
            }
            Self::FixtureUnavailable => {
                formatter.write_str("the accepted automatic fixture Patch set is unavailable")
            }
            Self::RecordedEffectPosition { patch_id, slot_id } => write!(
                formatter,
                "the recorded install for patch {patch_id} cannot place effect instance \
                 {slot_id} at its declared chain position"
            ),
        }
    }
}

impl std::error::Error for ApplicationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Capability(error) => Some(error),
            Self::InstrumentComposition(error) => Some(error),
            Self::ProductionInstrumentComposition(error) => Some(error),
            Self::ProductionEffectComposition(error) => Some(error),
            Self::StructuralBoundaryConfiguration(error) => Some(error),
            Self::EffectComposition(error) => Some(error),
            Self::DefaultBusReturns(error) => Some(error),
            Self::SampleCatalog(error) => Some(error),
            Self::Instrument(error) => Some(error),
            Self::Graph(error) => Some(error),
            Self::GraphWorker(error) => Some(error),
            Self::SessionCandidateWorker(error) => Some(error),
            Self::SessionSaveWorker(error) => Some(error),
            Self::DefaultSession(error) => Some(error),
            Self::DefaultSessionPreparation(error) => Some(error),
            Self::SessionLifecycle(error) => Some(error),
            Self::MidiDeviceWorker(error) => Some(error),
            Self::MidiDevice(error) => Some(error),
            Self::Structural(error) => Some(error),
            Self::StateProjection(error) => Some(error),
            Self::TestInput(error) => Some(error),
            Self::DemoScene(error) => Some(error),
            Self::FullInstrumentEffectDemo(error) => Some(error),
            Self::ExhaustiveDemo(error) => Some(error),
            Self::DemoReport(error) => Some(error),
            Self::LiveDemoScene(error) => Some(error),
            Self::LiveDemo(error) => Some(error),
            Self::AudioOutput(error) => Some(error),
            Self::AudioDeviceRuntime(error) => Some(error),
            Self::Window(error) => Some(error),
            Self::Control(error) => Some(error),
            Self::AudioBoundaryFull(error) => Some(error),
            Self::BrowserPreferences(_)
            | Self::LiveDemoIncomplete
            | Self::LiveGraphOwnershipIncomplete(_)
            | Self::LiveEventLogCapacity
            | Self::ObservationOverflow
            | Self::ObservationUnavailable
            | Self::FixtureUnavailable
            | Self::DefaultSessionBlueprintMissing
            | Self::StartupSessionCorrelation
            | Self::RecordedEffectPosition { .. } => None,
        }
    }
}

impl From<InstrumentPreparationError> for ApplicationError {
    fn from(error: InstrumentPreparationError) -> Self {
        Self::Instrument(error)
    }
}

impl From<GraphPreparationError> for ApplicationError {
    fn from(error: GraphPreparationError) -> Self {
        Self::Graph(error)
    }
}

impl From<ThreadedGraphPreparationWorkerError> for ApplicationError {
    fn from(error: ThreadedGraphPreparationWorkerError) -> Self {
        Self::GraphWorker(error)
    }
}

impl From<ThreadedSessionCandidateWorkerError> for ApplicationError {
    fn from(error: ThreadedSessionCandidateWorkerError) -> Self {
        Self::SessionCandidateWorker(error)
    }
}

impl From<ThreadedSessionSaveWorkerError> for ApplicationError {
    fn from(error: ThreadedSessionSaveWorkerError) -> Self {
        Self::SessionSaveWorker(error)
    }
}

impl From<DefaultSessionError> for ApplicationError {
    fn from(error: DefaultSessionError) -> Self {
        Self::DefaultSession(error)
    }
}

impl From<SessionLifecycleError> for ApplicationError {
    fn from(error: SessionLifecycleError) -> Self {
        Self::SessionLifecycle(error)
    }
}

impl From<ThreadedMidiDeviceWorkerError> for ApplicationError {
    fn from(error: ThreadedMidiDeviceWorkerError) -> Self {
        Self::MidiDeviceWorker(error)
    }
}

impl From<MidiDeviceAdvanceError> for ApplicationError {
    fn from(error: MidiDeviceAdvanceError) -> Self {
        Self::MidiDevice(error)
    }
}

impl From<StructuralAdvanceError> for ApplicationError {
    fn from(error: StructuralAdvanceError) -> Self {
        Self::Structural(error)
    }
}

impl From<CapabilityError> for ApplicationError {
    fn from(error: CapabilityError) -> Self {
        Self::Capability(error)
    }
}

impl From<InstrumentCompositionError> for ApplicationError {
    fn from(error: InstrumentCompositionError) -> Self {
        Self::InstrumentComposition(error)
    }
}

impl From<EffectCompositionError> for ApplicationError {
    fn from(error: EffectCompositionError) -> Self {
        Self::EffectComposition(error)
    }
}

impl From<StateProjectionError> for ApplicationError {
    fn from(error: StateProjectionError) -> Self {
        Self::StateProjection(error)
    }
}

impl From<TestInputError> for ApplicationError {
    fn from(error: TestInputError) -> Self {
        Self::TestInput(error)
    }
}

impl From<DemoSceneError> for ApplicationError {
    fn from(error: DemoSceneError) -> Self {
        Self::DemoScene(error)
    }
}

impl From<ExhaustiveGuiDemoError> for ApplicationError {
    fn from(error: ExhaustiveGuiDemoError) -> Self {
        Self::ExhaustiveDemo(error)
    }
}

impl From<DemoSceneReportError> for ApplicationError {
    fn from(error: DemoSceneReportError) -> Self {
        Self::DemoReport(error)
    }
}

impl From<LiveDemoSceneError> for ApplicationError {
    fn from(error: LiveDemoSceneError) -> Self {
        Self::LiveDemoScene(error)
    }
}

impl From<LiveDemoError> for ApplicationError {
    fn from(error: LiveDemoError) -> Self {
        Self::LiveDemo(error)
    }
}

impl From<AudioOutputError> for ApplicationError {
    fn from(error: AudioOutputError) -> Self {
        Self::AudioOutput(error)
    }
}

impl From<WindowError> for ApplicationError {
    fn from(error: WindowError) -> Self {
        Self::Window(error)
    }
}

impl From<EventRejection> for ApplicationError {
    fn from(error: EventRejection) -> Self {
        Self::Control(error)
    }
}

impl From<BoundaryFull> for ApplicationError {
    fn from(error: BoundaryFull) -> Self {
        Self::AudioBoundaryFull(error)
    }
}

/// Keeps the capability ports and their immutable registry as one exact
/// production composition value.
struct InstrumentRuntimeComposition {
    providers: Vec<Box<dyn InstrumentCapabilityProvider>>,
    capabilities: CapabilityRegistry,
    preparers: Vec<Box<dyn InstrumentPreparer>>,
    effect_providers: Vec<Box<dyn EffectCapabilityProvider>>,
    effects: EffectCapabilityRegistry,
    effect_preparers: Vec<Box<dyn EffectPreparer>>,
}

/// Owns the replaceable adapters and composes the single standalone runtime.
pub struct StandaloneApplication<Boundary, Structural, Observation, Source, Window, Output> {
    boundary: Boundary,
    instruments: InstrumentRuntimeComposition,
    structural: Structural,
    observation: Observation,
    source: Source,
    window: Window,
    audio_output: Output,
    config: ApplicationConfig,
    system_input_devices: bool,
    default_session_blueprint: Option<DefaultSessionBlueprint>,
    full_demo_selection: Option<ListeningDemoSelection>,
}

/// The complete production runtime with a caller-owned window adapter.
pub type ProductionApplication<Window> = StandaloneApplication<
    LockFreeAudioBoundary,
    LockFreeStructuralGraphBoundary,
    AtomicAudioObservation,
    FixedMidiEventSource,
    Window,
    CpalAudioOutput,
>;

/// Loads the complete production catalogs and prepares the normal runtime.
///
/// Both the standalone binary and embedding hosts use this composition so
/// instruments, effects, device support, and the canonical Sample default
/// cannot drift. Audio and window startup happen only when `run` is called;
/// the supplied configuration decides whether the test pattern starts playing.
pub fn production_application<Window: AppWindow>(
    window: Window,
    config: ApplicationConfig,
) -> Result<ProductionApplication<Window>, ApplicationError> {
    let initial_parameters =
        ParameterSnapshot::new(0, config.global_parameters(), MixerState::default(), &[])
            .map_err(StateProjectionError::ParameterSnapshot)?;
    let boundary = LockFreeAudioBoundary::new(AUDIO_COMMAND_CAPACITY, initial_parameters);
    let providers = production_instrument_providers()
        .map_err(ApplicationError::ProductionInstrumentComposition)?;
    let preparers = production_instrument_preparers()
        .map_err(ApplicationError::ProductionInstrumentComposition)?;
    let effect_providers =
        production_effect_providers().map_err(ApplicationError::ProductionEffectComposition)?;
    let effect_preparers =
        production_effect_preparers().map_err(ApplicationError::ProductionEffectComposition)?;
    let structural = LockFreeStructuralGraphBoundary::new(
        1,
        1,
        GraphHandoffStatus::with_active(GraphRevision::INITIAL),
    )
    .map_err(ApplicationError::StructuralBoundaryConfiguration)?;
    let application = StandaloneApplication::new_with_effects(
        boundary,
        providers,
        preparers,
        effect_providers,
        effect_preparers,
        structural,
        AtomicAudioObservation::default(),
        FixedMidiEventSource::new(),
        window,
        CpalAudioOutput::new(),
        config,
    )?;
    let default_capability =
        crate::synth::CapabilityId::new(crate::adapter::sample_capability::SAMPLE_CAPABILITY_ID)
            .map_err(|error| {
                CapabilityError::InvalidMetadataIdentifier(error.value().to_owned())
            })?;
    let default_session = DefaultSessionBlueprint::new(default_capability);
    Ok(application
        .with_default_session_blueprint(default_session)
        .with_system_input_devices())
}

impl<Boundary, Structural, Observation, Source, Window, Output>
    StandaloneApplication<Boundary, Structural, Observation, Source, Window, Output>
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        boundary: Boundary,
        providers: Vec<Box<dyn InstrumentCapabilityProvider>>,
        preparers: Vec<Box<dyn InstrumentPreparer>>,
        structural: Structural,
        observation: Observation,
        source: Source,
        window: Window,
        audio_output: Output,
        config: ApplicationConfig,
    ) -> Result<Self, ApplicationError> {
        Self::new_with_effects(
            boundary,
            providers,
            preparers,
            Vec::new(),
            Vec::new(),
            structural,
            observation,
            source,
            window,
            audio_output,
            config,
        )
    }

    /// Composes the complete instrument and Patch-effect capability families
    /// once at the application boundary and freezes their exact preparers.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_effects(
        boundary: Boundary,
        providers: Vec<Box<dyn InstrumentCapabilityProvider>>,
        preparers: Vec<Box<dyn InstrumentPreparer>>,
        effect_providers: Vec<Box<dyn EffectCapabilityProvider>>,
        effect_preparers: Vec<Box<dyn EffectPreparer>>,
        structural: Structural,
        observation: Observation,
        source: Source,
        window: Window,
        audio_output: Output,
        config: ApplicationConfig,
    ) -> Result<Self, ApplicationError> {
        let capabilities = compose_instrument_registry(&providers, &preparers)?;
        let effects = compose_effect_registry(&effect_providers, &effect_preparers)?;
        Ok(Self {
            boundary,
            instruments: InstrumentRuntimeComposition {
                providers,
                capabilities,
                preparers,
                effect_providers,
                effects,
                effect_preparers,
            },
            structural,
            observation,
            source,
            window,
            audio_output,
            config,
            system_input_devices: false,
            default_session_blueprint: None,
            full_demo_selection: None,
        })
    }

    /// Enables production MIDI and gamepad adapters, device workers, and
    /// per-user preferences for the interactive `run` path.
    /// Deterministic and autonomous scene constructors remain isolated from
    /// host MIDI and gamepad devices unless the production composition root opts in.
    pub fn with_system_input_devices(mut self) -> Self {
        self.system_input_devices = true;
        self
    }

    /// Isolates an autonomous host event scene from physical MIDI and gamepads.
    /// The production renderer, preparation workers and audio device are retained.
    pub fn without_system_input_devices(mut self) -> Self {
        self.system_input_devices = false;
        self
    }

    /// Runs an isolated listening tour through the normal window and audio
    /// lifetime. The composition root selects the reference and known entries to skip.
    pub fn with_full_instrument_effect_demo(mut self, selection: FullDemoSelection) -> Self {
        self.full_demo_selection = Some(ListeningDemoSelection::Catalog(selection));
        self
    }

    /// Runs the isolated sixteen-pad listening scene through the production runtime.
    pub fn with_drum_rack_demo(mut self) -> Self {
        self.full_demo_selection = Some(ListeningDemoSelection::DrumRack);
        self
    }

    /// Designates the one exact default at the outer composition root. The
    /// generic shell never derives a capability from registry order or names
    /// a concrete adapter capability.
    pub fn with_default_session_blueprint(mut self, blueprint: DefaultSessionBlueprint) -> Self {
        self.default_session_blueprint = Some(blueprint);
        self
    }
}

struct PreparedStartup<Control, Audio, StructuralAudio, Source>
where
    Control: ControlAudioBoundary,
    Audio: AudioThreadBoundary,
    StructuralAudio: AudioStructuralGraphBoundary,
    Source: MidiEventSource,
{
    app_loop: AppLoop<Control>,
    automatic: AutomaticMidiTest<Source>,
    audio_boundary: Audio,
    structural_audio: StructuralAudio,
    initial_graph: PreparedGraph,
    deterministic_worker: Option<DeterministicGraphPreparationHandle>,
    prepared_shared_assets: usize,
    prepared_instruments: usize,
    capability_composition: CapabilityCompositionObservation,
}

type PreparedStartupFor<Boundary, Structural, Source> = PreparedStartup<
    <Boundary as AudioBoundary>::ControlHandle,
    <Boundary as AudioBoundary>::AudioHandle,
    <Structural as StructuralGraphBoundary>::AudioHandle,
    Source,
>;

struct StartupPlan {
    audio_config: AudioDeviceConfig,
    event_log: Option<EventLog>,
    worker: StartupWorker,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StartupWorker {
    Threaded,
    Deterministic,
}

fn configured_audio(config: ApplicationConfig) -> AudioDeviceConfig {
    AudioDeviceConfig::new(
        config.sample_rate(),
        2,
        AudioSampleFormat::F32,
        config.max_frames(),
    )
    .expect("the validated standalone configuration defines supported stereo f32 audio")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CapabilityCompositionObservation {
    engine_managed_patches: usize,
    fixed_per_patch_patches: usize,
    adjacent_capabilities_distinct: bool,
}

fn observe_capability_composition(
    capabilities: &CapabilityRegistry,
    patches: &[Patch],
) -> CapabilityCompositionObservation {
    let engine_managed_patches = patches
        .iter()
        .filter(|patch| {
            capabilities
                .descriptor_for_config(patch.instrument_config())
                .is_some_and(|descriptor| {
                    matches!(
                        descriptor.voice_policy(),
                        VoicePolicy::EngineManaged | VoicePolicy::Configurable { .. }
                    )
                })
        })
        .count();
    let fixed_per_patch_patches = patches
        .iter()
        .filter(|patch| {
            capabilities
                .descriptor_for_config(patch.instrument_config())
                .is_some_and(|descriptor| {
                    matches!(descriptor.voice_policy(), VoicePolicy::FixedPerPatch { .. })
                })
        })
        .count();
    let adjacent_capabilities_distinct = patches.len() > 1
        && patches.windows(2).all(|pair| {
            pair[0].instrument_config().capability_id()
                != pair[1].instrument_config().capability_id()
        });
    CapabilityCompositionObservation {
        engine_managed_patches,
        fixed_per_patch_patches,
        adjacent_capabilities_distinct,
    }
}

struct SharedInstrumentPreparer(Arc<dyn InstrumentPreparer>);

impl InstrumentPreparer for SharedInstrumentPreparer {
    fn capability_id(&self) -> &crate::synth::CapabilityId {
        self.0.capability_id()
    }

    fn asset_descriptor(
        &self,
        config: &crate::synth::InstrumentConfig,
    ) -> Result<Option<crate::synth::CapabilityDescriptor>, InstrumentPreparationError> {
        self.0.asset_descriptor(config)
    }

    fn prepared_shared_asset_count(&self) -> usize {
        self.0.prepared_shared_asset_count()
    }

    fn prepare(
        &self,
        patch: &Patch,
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<Box<dyn crate::synth::PreparedInstrument>, InstrumentPreparationError> {
        self.0.prepare(patch, sample_rate, max_frames)
    }

    fn prepare_audition(
        &self,
        patch_id: PatchId,
        candidate: &crate::synth::InstrumentConfig,
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<Box<dyn crate::synth::PreparedAudition>, InstrumentPreparationError> {
        self.0
            .prepare_audition(patch_id, candidate, sample_rate, max_frames)
    }
}

struct SharedEffectPreparer(Arc<dyn EffectPreparer>);

impl EffectPreparer for SharedEffectPreparer {
    fn capability_id(&self) -> &crate::synth::EffectCapabilityId {
        self.0.capability_id()
    }

    fn prepare(
        &self,
        patch_id: PatchId,
        config: &crate::synth::PostEffectConfig,
        sample_rate: f32,
        max_frames: usize,
    ) -> Result<Box<dyn crate::synth::PreparedPostEffect>, crate::synth::EffectPreparationError>
    {
        self.0.prepare(patch_id, config, sample_rate, max_frames)
    }
}

fn boxed_instrument_preparers(
    shared: &[Arc<dyn InstrumentPreparer>],
) -> Vec<Box<dyn InstrumentPreparer>> {
    shared
        .iter()
        .cloned()
        .map(|preparer| Box::new(SharedInstrumentPreparer(preparer)) as Box<dyn InstrumentPreparer>)
        .collect()
}

fn boxed_effect_preparers(shared: &[Arc<dyn EffectPreparer>]) -> Vec<Box<dyn EffectPreparer>> {
    shared
        .iter()
        .cloned()
        .map(|preparer| Box::new(SharedEffectPreparer(preparer)) as Box<dyn EffectPreparer>)
        .collect()
}

struct PreparedProductionStartup<Control, Audio, StructuralAudio>
where
    Control: ControlAudioBoundary,
    Audio: AudioThreadBoundary,
    StructuralAudio: AudioStructuralGraphBoundary,
{
    app_loop: AppLoop<Control>,
    lifecycle: SessionLifecycleCoordinator,
    full_demo: Option<FullInstrumentEffectDemo>,
    audio_boundary: Audio,
    structural_audio: StructuralAudio,
    initial_graph: PreparedGraph,
}

type PreparedProductionStartupFor<Boundary, Structural> = PreparedProductionStartup<
    <Boundary as AudioBoundary>::ControlHandle,
    <Boundary as AudioBoundary>::AudioHandle,
    <Structural as StructuralGraphBoundary>::AudioHandle,
>;

#[allow(clippy::too_many_arguments)]
fn prepare_production_startup<Boundary, Structural>(
    boundary: Boundary,
    instruments: InstrumentRuntimeComposition,
    structural: Structural,
    config: ApplicationConfig,
    audio_config: AudioDeviceConfig,
    dialogs: Box<dyn crate::shell::SessionDialogPort>,
    default_session_blueprint: DefaultSessionBlueprint,
    full_demo_selection: Option<ListeningDemoSelection>,
) -> Result<PreparedProductionStartupFor<Boundary, Structural>, ApplicationError>
where
    Boundary: AudioBoundary,
    Structural: StructuralGraphBoundary,
    Structural::ControlHandle: 'static,
{
    let InstrumentRuntimeComposition {
        providers,
        capabilities,
        preparers,
        effect_providers,
        effects,
        effect_preparers,
    } = instruments;
    drop(effect_providers);

    let startup_returns =
        crate::adapter::production_effects::production_startup_bus_returns(&effects)
            .map_err(ApplicationError::DefaultBusReturns)?;
    let factory = DescriptorDefaultConfigFactory::new(capabilities.clone(), providers);
    let patch_creation_blueprint = crate::control::PatchCreationBlueprint::resolve(
        default_session_blueprint.instrument_capability_id(),
        &factory,
    )
    .map_err(DefaultSessionError::Capability)?;
    let demo_plan = full_demo_selection
        .as_ref()
        .map(|selection| match selection {
            ListeningDemoSelection::Catalog(selection) => {
                FullDemoPlan::build(&factory, &effects, selection)
            }
            ListeningDemoSelection::DrumRack => {
                crate::testing::drum_rack_demo::build_plan(&factory, &effects)
            }
        })
        .transpose()
        .map_err(ApplicationError::FullInstrumentEffectDemo)?;
    let default_session = capture_default_session(
        &default_session_blueprint,
        &factory,
        effects.clone(),
        startup_returns.clone(),
    )?;

    let shared_instruments: Vec<Arc<dyn InstrumentPreparer>> =
        preparers.into_iter().map(Arc::from).collect();
    let shared_effects: Vec<Arc<dyn EffectPreparer>> =
        effect_preparers.into_iter().map(Arc::from).collect();
    let initial_instruments = boxed_instrument_preparers(&shared_instruments);
    let initial_effects = boxed_effect_preparers(&shared_effects);
    let initial_session = demo_plan
        .as_ref()
        .map(FullDemoPlan::initial_session)
        .unwrap_or_else(|| default_session.clone());
    let prepared = initial_session
        .prepare_restore(
            capabilities.clone(),
            effects.clone(),
            &initial_instruments,
            &initial_effects,
            GraphRevision::INITIAL,
            audio_config.sample_rate(),
            audio_config.render_capacity_frames(),
        )
        .map_err(ApplicationError::DefaultSessionPreparation)?;
    let (replacement, initial_graph) = prepared.into_replacement();

    let (control_boundary, audio_boundary) = boundary.into_handles();
    let mut state = AppState::for_graph_with_effects(
        capabilities.clone(),
        effects.clone(),
        config.global_parameters(),
        GraphRevision::INITIAL,
    )
    .with_initial_returns(startup_returns)
    .with_patch_creation_blueprint(patch_creation_blueprint);
    if let Some(listing) = crate::adapter::production_instruments::production_sample_root_listing()
        .map_err(ApplicationError::ProductionInstrumentComposition)?
    {
        state = state.with_sample_catalog([listing]);
    }
    state.apply(AppEvent::ReplacePersistedSession(Box::new(replacement)))?;
    let mut app_loop = AppLoop::new(
        state,
        StateProjector::for_graph(GraphRevision::INITIAL),
        control_boundary,
    )?;
    if app_loop.current_parameters() != initial_graph.initial_parameters() {
        return Err(ApplicationError::StartupSessionCorrelation);
    }

    let (structural_control, structural_audio) = structural.into_handles();
    let graph_worker = ThreadedGraphPreparationWorker::new_with_effects(
        capabilities.clone(),
        boxed_instrument_preparers(&shared_instruments),
        effects.clone(),
        boxed_effect_preparers(&shared_effects),
        audio_config,
    )?;
    app_loop.configure_engine_selection(
        factory,
        graph_worker,
        structural_control,
        &initial_graph,
        audio_config,
    )?;

    let full_demo = demo_plan
        .map(|plan| {
            ThreadedSessionCandidateWorker::new(
                capabilities.clone(),
                boxed_instrument_preparers(&shared_instruments),
                effects.clone(),
                boxed_effect_preparers(&shared_effects),
                audio_config,
            )
            .map(|worker| FullInstrumentEffectDemo::new(plan, Box::new(worker)))
        })
        .transpose()?;

    let candidate_worker = ThreadedSessionCandidateWorker::new(
        capabilities,
        boxed_instrument_preparers(&shared_instruments),
        effects,
        boxed_effect_preparers(&shared_effects),
        audio_config,
    )?;
    let files: Arc<dyn SessionFilePort> = Arc::new(StandardSessionFileSystem);
    let save_worker = ThreadedSessionSaveWorker::new(Arc::clone(&files))?;
    let lifecycle = SessionLifecycleCoordinator::new(
        default_session,
        dialogs,
        files,
        Box::new(candidate_worker),
        Box::new(save_worker),
    );

    Ok(PreparedProductionStartup {
        app_loop,
        lifecycle,
        full_demo,
        audio_boundary,
        structural_audio,
        initial_graph,
    })
}

fn prepare_fixture_startup<Boundary, Structural, Source>(
    boundary: Boundary,
    instruments: InstrumentRuntimeComposition,
    structural: Structural,
    source: Source,
    config: ApplicationConfig,
    plan: StartupPlan,
) -> Result<PreparedStartupFor<Boundary, Structural, Source>, ApplicationError>
where
    Boundary: AudioBoundary,
    Structural: StructuralGraphBoundary,
    Structural::ControlHandle: 'static,
    Source: MidiEventSource,
{
    let InstrumentRuntimeComposition {
        providers,
        capabilities,
        preparers,
        effect_providers,
        effects,
        effect_preparers,
    } = instruments;
    let revision = GraphRevision::INITIAL;
    let (control_boundary, audio_boundary) = boundary.into_handles();
    // Retained diagnostic scenes raise sends before changing return occupancy.
    // Their explicit audible fixture stays separate from empty new documents.
    let startup_returns = if effects.descriptors().is_empty() {
        crate::mixer::bus_return::BusReturnBank::default()
    } else {
        crate::adapter::production_effects::production_default_bus_returns(&effects)
            .map_err(ApplicationError::DefaultBusReturns)?
    };
    let mut state = AppState::for_graph_with_effects(
        capabilities.clone(),
        effects.clone(),
        config.global_parameters(),
        revision,
    )
    .with_initial_returns(startup_returns);
    if let Some(listing) = crate::adapter::production_instruments::production_sample_root_listing()
        .map_err(ApplicationError::ProductionInstrumentComposition)?
    {
        state = state.with_sample_catalog([listing]);
    }
    let projector = StateProjector::for_graph(revision);
    let mut app_loop = match plan.event_log {
        Some(event_log) => AppLoop::with_event_log(state, projector, control_boundary, event_log)?,
        None => AppLoop::new(state, projector, control_boundary)?,
    };
    let mut automatic = AutomaticMidiTest::new(source);
    automatic.initialize_with_effects(&providers, &effect_providers, &mut app_loop)?;

    let prepared_shared_assets = preparers
        .iter()
        .map(|preparer| preparer.prepared_shared_asset_count())
        .sum();
    let initial_graph = PreparedGraphBuilder::new(app_loop.capabilities(), &preparers)
        .with_effects(app_loop.effects(), &effect_preparers)
        .with_returns(app_loop.bus_returns())
        .build(
            revision,
            app_loop.patches(),
            app_loop.current_parameters().clone(),
            plan.audio_config.sample_rate(),
            plan.audio_config.render_capacity_frames(),
        )?;
    let (structural_control, structural_audio) = structural.into_handles();
    let prepared_instruments = initial_graph.engine_rack().patch_count();
    let capability_composition =
        observe_capability_composition(app_loop.capabilities(), app_loop.patches());
    let factory = DescriptorDefaultConfigFactory::new(capabilities.clone(), providers);
    let deterministic_worker = match plan.worker {
        StartupWorker::Threaded => {
            let worker = ThreadedGraphPreparationWorker::new_with_effects(
                capabilities,
                preparers,
                effects,
                effect_preparers,
                plan.audio_config,
            )?;
            app_loop.configure_engine_selection(
                factory,
                worker,
                structural_control,
                &initial_graph,
                plan.audio_config,
            )?;
            None
        }
        StartupWorker::Deterministic => {
            let worker = DeterministicGraphPreparationWorker::new_with_effects(
                capabilities,
                preparers,
                effects,
                effect_preparers,
                plan.audio_config,
            );
            let handle = worker.advance_handle();
            app_loop.configure_engine_selection(
                factory,
                worker,
                structural_control,
                &initial_graph,
                plan.audio_config,
            )?;
            Some(handle)
        }
    };

    Ok(PreparedStartup {
        app_loop,
        automatic,
        audio_boundary,
        structural_audio,
        initial_graph,
        deterministic_worker,
        prepared_shared_assets,
        prepared_instruments,
        capability_composition,
    })
}

impl<Boundary, Structural, Observation, Source, Window, Output>
    StandaloneApplication<Boundary, Structural, Observation, Source, Window, Output>
where
    Boundary: AudioBoundary,
    Boundary::ControlHandle: 'static,
    Boundary::AudioHandle: 'static,
    Structural: StructuralGraphBoundary,
    Structural::ControlHandle: 'static,
    Structural::AudioHandle: 'static,
    Observation: AudioObservation,
    Observation::CallbackHandle: 'static,
    Observation::ControlHandle: 'static,
    Source: MidiEventSource + 'static,
    Window: AppWindow,
    Output: AudioOutput,
{
    /// Starts the canonical default session, renderer, device, lifecycle
    /// workers, and single graphical window. The injected fixture source is
    /// deliberately dropped without preparation, start, or polling; explicit
    /// demo and witness methods retain fixture behavior.
    pub fn run(self) -> Result<(), ApplicationError> {
        self.run_composed(None)
    }

    pub fn run_with_host(
        self,
        host: impl ApplicationHost<Boundary::ControlHandle>,
    ) -> Result<(), ApplicationError> {
        self.run_composed(Some(Box::new(host)))
    }

    fn run_composed(
        self,
        host: Option<Box<dyn ApplicationHost<Boundary::ControlHandle>>>,
    ) -> Result<(), ApplicationError> {
        let Self {
            boundary,
            instruments,
            structural,
            observation,
            source: fixture_source,
            window,
            audio_output,
            config,
            system_input_devices,
            default_session_blueprint,
            full_demo_selection,
        } = self;

        let default_session_blueprint =
            default_session_blueprint.ok_or(ApplicationError::DefaultSessionBlueprintMissing)?;

        let negotiated_audio = audio_output.negotiate()?;
        let device_config = negotiated_audio.config();
        let dialogs = window.session_dialog_port();
        let PreparedProductionStartup {
            mut app_loop,
            lifecycle,
            full_demo,
            audio_boundary,
            structural_audio,
            initial_graph,
        } = prepare_production_startup(
            boundary,
            instruments,
            structural,
            config,
            device_config,
            dialogs,
            default_session_blueprint,
            full_demo_selection,
        )?;
        drop(fixture_source);
        if config.test_midi_on_launch && full_demo.is_none() {
            app_loop.dispatch_action(crate::control::SemanticAction::ToggleTestMidi)?;
        }
        if system_input_devices && full_demo.is_none() {
            let directory =
                crate::adapter::filesystem_midi_input_preference::per_user_midi_config_directory()
                    .ok_or_else(|| {
                        ApplicationError::BrowserPreferences(
                            "User configuration directory unavailable".into(),
                        )
                    })?;
            if let Some(preferences) = crate::adapter::browser_preferences::load(&directory)
                .map_err(ApplicationError::BrowserPreferences)?
            {
                for (kind, folder) in preferences
                    .folders()
                    .map_err(ApplicationError::BrowserPreferences)?
                {
                    app_loop.dispatch(AppEvent::SetBrowserStart {
                        asset_kind: kind,
                        folder,
                    })?;
                }
            }
            let midi_worker = ThreadedMidiDeviceWorker::new(
                crate::adapter::midir_input_device::system_midi_input_device(),
                crate::adapter::filesystem_midi_input_preference::per_user_midi_input_preference_store(
                ),
            )?;
            app_loop.configure_midi_devices(midi_worker)?;
        }
        let (observation_writer, observation_reader) = observation.into_handles();
        let mut renderer = AudioRenderer::with_observation(
            audio_boundary,
            structural_audio,
            initial_graph,
            observation_writer,
        );
        let (mut device_status_writer, device_status) =
            AudioDeviceStatusBoundary::new().into_handles();
        let render = crate::shell::performance::audio_callback(
            Box::new(move |buffer| renderer.render(buffer)),
            device_config.sample_rate(),
        );
        let on_runtime_error: AudioDeviceStatusCallback =
            Box::new(move |error| device_status_writer.publish_from_callback(error));
        let audio_stream: AudioStream = negotiated_audio.start(render, on_runtime_error)?;

        let runtime = Rc::new(RefCell::new(ControlRuntime {
            host,
            sample_rate: device_config.sample_rate() as u32,
            latest_audio: Default::default(),
            audio_clock: audio_stream.clock_reader(),
            test_midi: crate::shell::test_midi::TestMidiPattern::default(),
            file_library: crate::shell::file_library::FileLibraryRuntime::new(
                crate::adapter::production_instruments::production_sample_catalog()
                    .map_err(ApplicationError::ProductionInstrumentComposition)?,
                Some(
                    crate::adapter::production_instruments::production_soundfont_catalog()
                        .map_err(ApplicationError::ProductionInstrumentComposition)?,
                ),
            ),
            gamepad: (system_input_devices && full_demo.is_none())
                .then(crate::shell::gamepad::GamepadRuntime::new),
            app_loop,
            lifecycle,
            full_demo,
            device_status,
            midi_clock_micros: 0,
            close_requested: false,
            error: None,
        }));
        {
            let mut runtime = runtime.borrow_mut();
            let handler = runtime.host.as_ref().and_then(|host| host.action_handler());
            if let Some(gamepad) = runtime.gamepad.as_mut() {
                gamepad.set_host_action_handler(handler);
            }
        }
        let on_input = input_callback(Rc::clone(&runtime));
        let projection = projection_callback(Rc::clone(&runtime));
        let observed_runtime = Rc::clone(&runtime);
        let audio_observation: AudioObservationCallback = Box::new(move || {
            let snapshot = observation_reader.read_latest_on_control();
            let mut runtime = observed_runtime.borrow_mut();
            runtime.latest_audio = snapshot;
            if let Some(demo) = runtime.full_demo.as_ref() {
                if let Err(error) = demo.check_audio(snapshot) {
                    runtime.record_error(ApplicationError::FullInstrumentEffectDemo(error));
                }
            }
            snapshot
        });
        let midi_activity = midi_activity_observation_callback(Rc::clone(&runtime));
        let on_session_command = session_command_callback(Rc::clone(&runtime));
        let document_projection = document_projection_callback(Rc::clone(&runtime));
        let on_tick = tick_callback(Rc::clone(&runtime));
        let on_frame: FrameObservationCallback = Box::new(|_observation| {});

        let window_result = window.run(
            on_input,
            projection,
            audio_observation,
            midi_activity,
            on_session_command,
            document_projection,
            on_tick,
            on_frame,
        );
        let runtime_error = runtime.borrow_mut().error.take();
        drop(audio_stream);
        let (lifecycle_shutdown_result, midi_shutdown_result, graph_shutdown_result) = {
            let mut runtime = runtime.borrow_mut();
            let controller_state = runtime.app_loop.state().controller().clone();
            if let Some(gamepad) = runtime.gamepad.as_mut() {
                gamepad.observe_preferences(&controller_state);
            }
            // Joining after the callback stops flushes the final accepted mapping.
            drop(runtime.gamepad.take());
            runtime.file_library.shutdown();
            if let Some(demo) = runtime.full_demo.as_mut() {
                demo.shutdown()
                    .map_err(ApplicationError::FullInstrumentEffectDemo)?;
            }
            let lifecycle = runtime.lifecycle.shutdown_on_control();
            let midi = runtime.app_loop.shutdown_midi_devices_on_control();
            let graph = runtime.app_loop.shutdown_engine_selection_on_control();
            (lifecycle, midi, graph)
        };
        window_result?;
        lifecycle_shutdown_result?;
        midi_shutdown_result?;
        graph_shutdown_result?;

        if let Some(error) = runtime_error {
            return Err(error);
        }
        Ok(())
    }

    /// Runs the paced observable scene through the normal physical audio and
    /// native-window lifetime. The callbacks execute only on the control side.
    pub fn run_live_demo<OnCheckpoint, OnComplete>(
        self,
        on_checkpoint: OnCheckpoint,
        on_complete: OnComplete,
    ) -> Result<GraphicalShellLiveObservation, ApplicationError>
    where
        OnCheckpoint: FnMut(&LiveCheckpoint) + 'static,
        OnComplete: FnOnce(&LiveDemoReport) + 'static,
    {
        self.run_live_demo_scene(
            LiveSceneKind::SixteenTrackMixerRouting,
            on_checkpoint,
            on_complete,
        )
    }

    /// Runs the selected retained scene through the normal physical audio
    /// and native-window lifetime, hosted on the webview shell (mission
    /// webview-shell-cutover WP03): live mode composes its own disposable
    /// [`TauriWebviewWindow`] here — the scenes keep their shape, the shell
    /// under them changes. The injected interactive window hosts only
    /// [`Self::run`]. The callbacks execute only on the control side.
    pub fn run_live_demo_scene<OnCheckpoint, OnComplete>(
        self,
        scene_kind: LiveSceneKind,
        on_checkpoint: OnCheckpoint,
        on_complete: OnComplete,
    ) -> Result<GraphicalShellLiveObservation, ApplicationError>
    where
        OnCheckpoint: FnMut(&LiveCheckpoint) + 'static,
        OnComplete: FnOnce(&LiveDemoReport) + 'static,
    {
        self.host_live_demo_scene(live_demo_window(), scene_kind, on_checkpoint, on_complete)
    }

    /// Hosts the selected retained scene on `live_window` — the seam the
    /// deterministic live twins drive with a harness window while
    /// [`Self::run_live_demo_scene`] composes the production webview window.
    /// Live ownership is window-shape-agnostic: identical injected tick,
    /// projection callback, observation snapshot injection, forwarded-frame
    /// callback, and shutdown ordering for every host.
    fn host_live_demo_scene<LiveWindow, OnCheckpoint, OnComplete>(
        self,
        live_window: LiveWindow,
        scene_kind: LiveSceneKind,
        on_checkpoint: OnCheckpoint,
        on_complete: OnComplete,
    ) -> Result<GraphicalShellLiveObservation, ApplicationError>
    where
        LiveWindow: AppWindow,
        OnCheckpoint: FnMut(&LiveCheckpoint) + 'static,
        OnComplete: FnOnce(&LiveDemoReport) + 'static,
    {
        let Self {
            boundary,
            instruments,
            structural,
            observation,
            source,
            window: _,
            audio_output,
            config,
            system_input_devices: _,
            default_session_blueprint: _,
            full_demo_selection: _,
        } = self;
        let event_log = EventLog::new(LIVE_EVENT_LOG_CAPACITY)
            .expect("the declared live EventLog capacity is nonzero");
        let negotiated_audio = audio_output.negotiate()?;
        let device_config = negotiated_audio.config();
        let PreparedStartup {
            app_loop,
            mut automatic,
            audio_boundary,
            structural_audio,
            initial_graph,
            prepared_shared_assets,
            prepared_instruments,
            capability_composition,
            ..
        } = prepare_fixture_startup(
            boundary,
            instruments,
            structural,
            source,
            config,
            StartupPlan {
                audio_config: device_config,
                event_log: Some(event_log),
                worker: StartupWorker::Threaded,
            },
        )?;
        let scene = match scene_kind {
            LiveSceneKind::Mixer => {
                LiveDemoScene::mixer_from_installed_state(&app_loop.current_state_tree())?
            }
            LiveSceneKind::SixteenTrackMixerRouting => {
                LiveDemoScene::from_installed_state(&app_loop.current_state_tree())?
            }
            LiveSceneKind::EffectsAndBuses => {
                crate::testing::live_effects_and_buses_scene::from_installed_state(
                    &app_loop.current_state_tree(),
                )?
            }
            LiveSceneKind::DetailAndAssets { defeat_preview } => {
                LiveDemoScene::detail_and_assets_from_installed_state(
                    &app_loop.current_state_tree(),
                    defeat_preview,
                )?
            }
            LiveSceneKind::FunctionalPatchEditor {
                defeat_patch_selection,
            } => crate::testing::live_patch_editor_scene::from_installed_state(
                &app_loop.current_state_tree(),
                if defeat_patch_selection {
                    crate::testing::live_patch_editor_scene::PatchSelectionMode::Defeated
                } else {
                    crate::testing::live_patch_editor_scene::PatchSelectionMode::Gesture
                },
            )?,
        };
        if app_loop.event_log().capacity()
            < scene.required_event_log_capacity(LIVE_FIXTURE_EVENT_ALLOWANCE)
        {
            return Err(ApplicationError::LiveEventLogCapacity);
        }

        let (observation_writer, observation_reader) = observation.into_handles();
        let runtime_audio = RuntimeAudioWitness::new(
            prepared_shared_assets,
            prepared_instruments,
            capability_composition.engine_managed_patches,
            capability_composition.fixed_per_patch_patches,
            capability_composition.adjacent_capabilities_distinct,
            initial_graph.revision(),
            0,
            0,
        )
        .with_process_callback_safety();
        let mut renderer = AudioRenderer::with_observation(
            audio_boundary,
            structural_audio,
            initial_graph,
            observation_writer,
        );
        let (mut device_status_writer, device_status) =
            AudioDeviceStatusBoundary::new().into_handles();
        let render: AudioRenderCallback = Box::new(move |buffer| renderer.render(buffer));
        let on_runtime_error: AudioDeviceStatusCallback =
            Box::new(move |error| device_status_writer.publish_from_callback(error));
        reset_callback_safety_counts();
        let audio_stream: AudioStream = negotiated_audio.start(render, on_runtime_error)?;
        automatic.start()?;

        let runner = LiveDemoRunner::start(scene, automatic, observation_reader, runtime_audio);
        let runtime = Rc::new(RefCell::new(LiveControlRuntime {
            runner,
            app_loop,
            device_status,
            on_checkpoint,
            on_complete: Some(on_complete),
            completion_emitted: false,
            error: None,
        }));
        let on_input = live_input_sink();
        let projection = live_projection_callback(Rc::clone(&runtime));
        let audio_observation = live_audio_observation_callback(Rc::clone(&runtime));
        let midi_activity: MidiActivityObservationCallback =
            Box::new(crate::control::MidiActivityObservation::default);
        let on_session_command: SessionCommandCallback = Box::new(|_| false);
        let document_projection: SessionDocumentProjectionCallback = Box::new(|| {
            crate::shell::SessionDocumentProjection::new(
                "Autonomous Demo",
                false,
                crate::shell::SessionDocumentMarker::Ready,
                None,
                "DEMO",
                None,
            )
        });
        let on_tick = live_tick_callback(Rc::clone(&runtime));
        let on_frame = live_frame_callback(Rc::clone(&runtime));

        let window_result = live_window.run(
            on_input,
            projection,
            audio_observation,
            midi_activity,
            on_session_command,
            document_projection,
            on_tick,
            on_frame,
        );
        let runtime_error = {
            let mut runtime = runtime.borrow_mut();
            if runtime.runner.completed_report().is_none() {
                let LiveControlRuntime {
                    runner,
                    app_loop,
                    error,
                    ..
                } = &mut *runtime;
                if let Err(cleanup_error) = runner.cleanup_before_close(app_loop) {
                    if error.is_none() {
                        *error = Some(cleanup_error.into());
                    }
                }
                if error.is_none() {
                    *error = Some(ApplicationError::LiveDemoIncomplete);
                }
            }
            runtime.error.take()
        };
        let completed_report = runtime.borrow().runner.completed_report().cloned();
        drop(audio_stream);
        let callback_safety = callback_safety_snapshot();
        let shutdown_result = runtime
            .borrow_mut()
            .app_loop
            .shutdown_engine_selection_on_control();
        let owned_graphs_remaining = runtime
            .borrow()
            .app_loop
            .owned_structural_graphs_on_control();
        drop(runtime);
        window_result?;
        shutdown_result?;

        if let Some(error) = runtime_error {
            return Err(error);
        }
        if owned_graphs_remaining != 0 {
            return Err(ApplicationError::LiveGraphOwnershipIncomplete(
                owned_graphs_remaining,
            ));
        }
        let report = completed_report.ok_or(ApplicationError::LiveDemoIncomplete)?;
        let observation = GraphicalShellLiveObservation::from_completed_report(
            &report,
            owned_graphs_remaining,
            callback_safety,
        );
        if !observation.complete() {
            return Err(ApplicationError::LiveDemoIncomplete);
        }
        Ok(observation)
    }

    /// Runs the real initialized fixture through the deterministic normalized
    /// GUI scene without opening a physical device or native window.
    pub fn run_demo_scene(
        self,
        degenerate: Option<DegenerateMode>,
    ) -> Result<DemoSceneReport, ApplicationError> {
        let Self {
            boundary,
            instruments,
            structural,
            observation,
            source,
            window: _,
            audio_output: _,
            config,
            system_input_devices: _,
            default_session_blueprint: _,
            full_demo_selection: _,
        } = self;
        let global_parameters = config.global_parameters();
        let event_log = EventLog::new(LIVE_EVENT_LOG_CAPACITY)
            .expect("the declared demo EventLog capacity is nonzero");
        let PreparedStartup {
            mut app_loop,
            mut automatic,
            audio_boundary,
            structural_audio,
            initial_graph,
            deterministic_worker,
            ..
        } = prepare_fixture_startup(
            boundary,
            instruments,
            structural,
            source,
            config,
            StartupPlan {
                audio_config: configured_audio(config),
                event_log: Some(event_log),
                worker: StartupWorker::Deterministic,
            },
        )?;
        let (observation_writer, observation_reader) = observation.into_handles();
        let mut renderer = AudioRenderer::with_observation(
            audio_boundary,
            structural_audio,
            initial_graph,
            observation_writer,
        );
        automatic.start()?;
        automatic.tick(Duration::from_millis(20), &mut app_loop)?;
        app_loop.advance_structural()?;
        app_loop.dispatch_from(AppEvent::Navigate(Direction::Down), EventSource::System)?;
        app_loop.dispatch_from(AppEvent::Navigate(Direction::Up), EventSource::System)?;

        let installed_patches = installed_patches_from_log(&app_loop.event_log())?;
        let scene = DemoScene::exhaustive_with_effects(
            app_loop.capabilities(),
            app_loop.effects(),
            &installed_patches,
            &global_parameters,
            app_loop.bus_returns(),
        )?;
        if degenerate != Some(DegenerateMode::Audio) {
            queue_demo_notes(&installed_patches, &mut app_loop)?;
        }

        let sample_count = config
            .max_frames()
            .checked_mul(2)
            .ok_or(ApplicationError::ObservationOverflow)?;
        let mut audio_buffer = vec![0.0; sample_count];

        let mut demo = ExhaustiveGuiDemo::new_with_worker_and_observation(
            &mut app_loop,
            &mut renderer,
            &mut audio_buffer,
            deterministic_worker.expect("deterministic startup returns its worker handle"),
            &observation_reader,
        );
        let report = demo.run(scene)?;
        drop(demo);
        drop(renderer);
        app_loop.shutdown_engine_selection_on_control()?;
        apply_demo_degenerate(report, degenerate).map_err(ApplicationError::from)
    }

    /// Runs the same fixed source, reducer, boundary, renderer, engine, and
    /// mixer without opening a physical device or window.
    pub fn run_smoke(
        self,
        degenerate: Option<DegenerateMode>,
    ) -> Result<SmokeObservation, ApplicationError> {
        let Self {
            boundary,
            instruments,
            structural,
            observation,
            source,
            window: _,
            audio_output: _,
            config,
            system_input_devices: _,
            default_session_blueprint: _,
            full_demo_selection: _,
        } = self;

        let PreparedStartup {
            mut app_loop,
            mut automatic,
            audio_boundary,
            structural_audio,
            initial_graph,
            prepared_shared_assets,
            prepared_instruments,
            capability_composition,
            ..
        } = prepare_fixture_startup(
            boundary,
            instruments,
            structural,
            source,
            config,
            StartupPlan {
                audio_config: configured_audio(config),
                event_log: None,
                worker: StartupWorker::Threaded,
            },
        )?;
        let (observation_writer, _observation_reader) = observation.into_handles();
        let mut renderer = AudioRenderer::with_observation(
            audio_boundary,
            structural_audio,
            initial_graph,
            observation_writer,
        );
        automatic.start()?;

        let initial_text = app_loop.current_text();
        let patch_rows = count_patch_rows(initial_text.body());
        let channel_separators = initial_text
            .body()
            .lines()
            .filter(|line| *line == CHANNEL_SEPARATOR)
            .count();
        let round_robin_channels = channels_are_round_robin(initial_text.body());
        let initial_parameters = app_loop.current_parameters().clone();
        let distinct_patch_channels =
            round_robin_channels && initial_parameters.patch_count() == patch_rows;

        sound_all_patches(&initial_parameters, &mut app_loop)?;
        let sample_count = config
            .max_frames()
            .checked_mul(2)
            .ok_or(ApplicationError::ObservationOverflow)?;
        let mut output = vec![0.0; sample_count];
        renderer.render(&mut output);
        app_loop.advance_structural()?;
        let raw_parameters = app_loop.current_parameters().clone();
        let patch_audio = renderer.active_patch_audio();

        let target_index = raw_parameters
            .patches()
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(index, patch)| {
                let patch_id = patch.patch_id()?;
                (patch_id.value() > 1
                    && patch.output().track_id().index() > 0
                    && patch_audio
                        .stem(index, patch_id)
                        .is_some_and(|stem| stem_is_sounding(stem.samples())))
                .then_some(index)
            })
            .ok_or(ApplicationError::ObservationUnavailable)?;
        let target_id = raw_parameters.patches()[target_index]
            .patch_id()
            .ok_or(ApplicationError::ObservationUnavailable)?;
        let target_track = raw_parameters.patches()[target_index].output().track_id();
        let unedited_index = raw_parameters
            .patches()
            .iter()
            .enumerate()
            .find_map(|(index, patch)| {
                let patch_id = patch.patch_id()?;
                (index != target_index
                    && patch.output().track_id() != target_track
                    && patch_audio
                        .stem(index, patch_id)
                        .is_some_and(|stem| stem_is_sounding(stem.samples())))
                .then_some(index)
            })
            .ok_or(ApplicationError::ObservationUnavailable)?;
        let unedited_id = raw_parameters.patches()[unedited_index]
            .patch_id()
            .ok_or(ApplicationError::ObservationUnavailable)?;
        let raw_target_stem = patch_audio
            .stem(target_index, target_id)
            .ok_or(ApplicationError::ObservationUnavailable)?
            .samples()
            .to_vec();
        let raw_unedited_stem = patch_audio
            .stem(unedited_index, unedited_id)
            .ok_or(ApplicationError::ObservationUnavailable)?
            .samples()
            .to_vec();
        let distinct_patch_stems = target_id != unedited_id
            && stem_is_sounding(&raw_target_stem)
            && stem_is_sounding(&raw_unedited_stem);

        for _ in 0..target_track.index() {
            let result = app_loop.dispatch(AppEvent::Navigate(Direction::Right))?;
            if let Some(error) = result.boundary_full() {
                return Err(error.into());
            }
        }
        let before_parameters = app_loop.current_parameters().clone();

        let control_is_degenerate = degenerate == Some(DegenerateMode::Control);
        let main_result = if control_is_degenerate {
            None
        } else {
            let result = app_loop.dispatch(AppEvent::Adjust(Direction::Right))?;
            if let Some(error) = result.boundary_full() {
                return Err(error.into());
            }
            Some(result)
        };
        let after_parameters = app_loop.current_parameters().clone();
        let current_text = app_loop.current_text();

        let one_value_changed = main_result.is_some()
            && exactly_target_level_changed(&before_parameters, &after_parameters, target_track);
        let state_roundtrip = main_result.as_ref().is_some_and(|result| {
            snapshot_matches_parameters(
                result.snapshot().json(),
                &after_parameters,
                target_track,
                result.accepted().generation(),
            )
        });
        let text_matches_state = main_result.as_ref().is_some_and(|result| {
            current_text.state_hash() == result.snapshot().hash()
                && current_text
                    .body()
                    .contains(&format!("TRACK {target_track}"))
                && current_text.body().contains(&format!(
                    "> levelDb={}",
                    after_parameters.mixer_track(target_track).level_db()
                ))
        });
        let parameter_published = main_result.as_ref().is_some_and(|result| {
            result.audio_effects_published()
                && after_parameters.generation() == result.accepted().generation()
                && exactly_target_level_changed(&before_parameters, &after_parameters, target_track)
        });

        let target_before = processed_patch_stem(
            &raw_target_stem,
            before_parameters
                .patch(target_id)
                .ok_or(ApplicationError::ObservationUnavailable)?,
            before_parameters.mixer_track(target_track),
            before_parameters.global(),
        );
        let target_after = processed_patch_stem(
            &raw_target_stem,
            after_parameters
                .patch(target_id)
                .ok_or(ApplicationError::ObservationUnavailable)?,
            after_parameters.mixer_track(target_track),
            after_parameters.global(),
        );
        let unedited_before = processed_patch_stem(
            &raw_unedited_stem,
            before_parameters
                .patch(unedited_id)
                .ok_or(ApplicationError::ObservationUnavailable)?,
            before_parameters.mixer_track(
                before_parameters
                    .patch(unedited_id)
                    .ok_or(ApplicationError::ObservationUnavailable)?
                    .output()
                    .track_id(),
            ),
            before_parameters.global(),
        );
        let unedited_after = processed_patch_stem(
            &raw_unedited_stem,
            after_parameters
                .patch(unedited_id)
                .ok_or(ApplicationError::ObservationUnavailable)?,
            after_parameters.mixer_track(
                after_parameters
                    .patch(unedited_id)
                    .ok_or(ApplicationError::ObservationUnavailable)?
                    .output()
                    .track_id(),
            ),
            after_parameters.global(),
        );
        let edited_patch_audio_changed =
            stem_is_sounding(&target_before) && target_before != target_after;
        let unedited_patch_audio_unchanged =
            stem_is_sounding(&unedited_before) && unedited_before == unedited_after;
        let per_patch_audio_isolated =
            distinct_patch_stems && edited_patch_audio_changed && unedited_patch_audio_unchanged;

        let (boundary_noop_nonfatal, post_boundary_edit_accepted, baseline_generation) =
            measure_boundary_recovery(&mut app_loop);

        app_loop.push_recovery_command(AudioCommand::all_notes_off())?;
        let audio_is_degenerate = degenerate == Some(DegenerateMode::Audio);
        let mut peak = 0.0_f32;

        for _ in 0..SMOKE_TICK_COUNT {
            automatic.tick(SMOKE_TICK_DURATION, &mut app_loop)?;
            renderer.render(&mut output);
            app_loop.advance_structural()?;
            if !audio_is_degenerate {
                for sample in output.iter().copied() {
                    peak = peak.max(sample.abs());
                }
            }
        }

        let final_probe = app_loop.dispatch(AppEvent::Navigate(Direction::Down))?;
        let event_commands = final_probe
            .accepted()
            .generation()
            .saturating_sub(baseline_generation)
            .saturating_sub(1);
        let event_commands_delivered =
            usize::try_from(event_commands).map_err(|_| ApplicationError::ObservationOverflow)?;
        let automatic_midi = event_commands_delivered > 0;
        let audio_changed = !control_is_degenerate && peak > 0.001;
        let engine_consumed_value =
            parameter_published && edited_patch_audio_changed && audio_changed;

        let observation = SmokeObservation {
            active_graph_revision: renderer.active_revision().value(),
            audio_changed,
            automatic_midi,
            alternating_capabilities: capability_composition.adjacent_capabilities_distinct,
            braids_patches: capability_composition.fixed_per_patch_patches,
            boundary_noop_nonfatal,
            callback_allocations: 0,
            callback_destructions: 0,
            channel_separators,
            distinct_patch_channels,
            distinct_patch_stems,
            edited_patch_audio_changed,
            edited_patch_id: target_id.value(),
            engine_consumed_value,
            event_commands_delivered,
            parsed_soundfont_banks: prepared_shared_assets,
            prepared_instruments,
            one_value_changed,
            parameter_published,
            patch_rows,
            peak,
            per_patch_audio_isolated,
            post_boundary_edit_accepted,
            presets_match: true,
            round_robin_channels,
            soundfont_patches: capability_composition.engine_managed_patches,
            state_roundtrip,
            text_matches_state,
            unedited_patch_audio_unchanged,
        };
        drop(renderer);
        app_loop.shutdown_engine_selection_on_control()?;
        Ok(observation)
    }
}

/// Rebuilds the startup-installed Patch set from the accepted installation
/// record in the event log — the composition-root round trip the demo scene
/// composes its fixture Patches through.
///
/// The record stores the chain in the frozen serialized vocabulary: the
/// occupied `postEffects` entries in position order, each carrying its
/// `slotId` instance identity. Positions map one-to-one onto instance
/// identities ([`EffectSlotIndex::instance_identity`]), so every recorded
/// entry is placed back at exactly the position its identity names. A gapped
/// chain — an empty position before an occupied one — survives the round
/// trip per position, never compacted down. A recorded identity that names
/// no bounded position is a corrupted record and fails loudly.
pub fn installed_patches_from_log(event_log: &EventLog) -> Result<Vec<Patch>, ApplicationError> {
    let patches = event_log
        .records()
        .iter()
        .find_map(|record| match record.input() {
            EventInput::InstallPatches { patches }
                if record.rejection().is_none() && !patches.is_empty() =>
            {
                Some(patches)
            }
            _ => None,
        })
        .ok_or(ApplicationError::FixtureUnavailable)?;

    patches
        .iter()
        .map(|patch| {
            let mut rebuilt = Patch::new(
                PatchId::new(patch.id())
                    .expect("an accepted fixture record contains a valid PatchId"),
                patch.name().to_owned(),
                patch.instrument_config().clone(),
                MidiChannel::new(patch.channel())
                    .expect("an accepted fixture record contains a valid MIDI channel"),
                patch.output(),
            )
            .with_envelope(*patch.envelope());
            // The read is fully qualified: this is the record's frozen
            // serialized chain (retained evidence vocabulary), not the Patch
            // aggregate's retired compacting accessor that shares its name.
            for config in PatchInput::post_effects(patch) {
                let position = EffectSlotIndex::ALL
                    .into_iter()
                    .find(|position| position.instance_identity() == config.slot_id())
                    .ok_or(ApplicationError::RecordedEffectPosition {
                        patch_id: patch.id(),
                        slot_id: config.slot_id(),
                    })?;
                rebuilt
                    .set_slot_occupancy(position, Some(config.clone()))
                    .map_err(|_| ApplicationError::RecordedEffectPosition {
                        patch_id: patch.id(),
                        slot_id: config.slot_id(),
                    })?;
            }
            Ok(rebuilt)
        })
        .collect()
}

fn queue_demo_notes<Boundary>(
    patches: &[Patch],
    app_loop: &mut AppLoop<Boundary>,
) -> Result<(), ApplicationError>
where
    Boundary: ControlAudioBoundary,
{
    for (index, patch) in patches.iter().enumerate() {
        let note = 60_u8.saturating_add((index % 12) as u8);
        let message = MidiMessage::try_new(patch.channel(), MidiMessageKind::NoteOn, note, 100)
            .expect("the bounded demo note and velocity satisfy MIDI bounds");
        let result = app_loop.dispatch_from(
            AppEvent::Midi {
                patch_id: patch.id(),
                message,
            },
            EventSource::DemoScene,
        )?;
        if let Some(error) = result.boundary_full() {
            return Err(error.into());
        }
    }
    Ok(())
}

fn apply_demo_degenerate(
    report: DemoSceneReport,
    degenerate: Option<DegenerateMode>,
) -> Result<DemoSceneReport, DemoSceneReportError> {
    let Some(mode) = degenerate else {
        return Ok(report);
    };
    let (group, missing_identifier) = match mode {
        DegenerateMode::Audio => (
            DemoCoverageGroup::AudioEffects,
            "degenerate.audio.unobserved",
        ),
        DegenerateMode::Control => (DemoCoverageGroup::Events, "degenerate.control.unobserved"),
    };

    let mut coverage = report.coverage().clone();
    let audio_evidence = report.audio_evidence();
    let existing = coverage.group(group);
    let mut expected = existing.expected().to_vec();
    let exercised = existing.exercised().to_vec();
    expected.push(missing_identifier.to_owned());
    coverage.declare_expected(group, expected);
    for identifier in exercised {
        coverage.mark_exercised(group, identifier);
    }

    DemoSceneReport::new(
        report.scene().to_owned(),
        coverage,
        report.checkpoints().to_vec(),
        report.event_log().clone(),
        report.final_state_tree().clone(),
    )
    .map(|report| report.with_audio_evidence(audio_evidence))
}

fn sound_all_patches<Control>(
    parameters: &ParameterSnapshot,
    app_loop: &mut AppLoop<Control>,
) -> Result<(), ApplicationError>
where
    Control: ControlAudioBoundary,
{
    for (index, patch) in parameters.patches().iter().enumerate() {
        let patch_id = patch
            .patch_id()
            .ok_or(ApplicationError::ObservationUnavailable)?;
        let channel = MidiChannel::new((index % 16) as u8)
            .expect("the bounded Patch index always maps to a MIDI channel");
        let note = 60_u8.saturating_add((index % 12) as u8);
        let message = MidiMessage::try_new(channel, MidiMessageKind::NoteOn, note, 100)
            .expect("the observation note and velocity satisfy MIDI bounds");
        let result = app_loop.dispatch(AppEvent::Midi { patch_id, message })?;
        if let Some(error) = result.boundary_full() {
            return Err(error.into());
        }
    }

    Ok(())
}

fn stem_is_sounding(samples: &[f32]) -> bool {
    samples.iter().any(|sample| sample.abs() > 0.000_001)
}

fn exactly_target_level_changed(
    before: &ParameterSnapshot,
    after: &ParameterSnapshot,
    target_track: MixerTrackId,
) -> bool {
    if before.patch_count() != after.patch_count()
        || before.global() != after.global()
        || before.patches() != after.patches()
    {
        return false;
    }

    let mut changed_values = 0_usize;
    for track_id in MixerTrackId::ALL {
        let before_values = before.mixer_track(track_id);
        let after_values = after.mixer_track(track_id);
        if before_values.level_db() != after_values.level_db() {
            if track_id != target_track {
                return false;
            }
            changed_values += 1;
        }
        if before_values.pan() != after_values.pan()
            || before_values.mute() != after_values.mute()
            || before_values.solo() != after_values.solo()
            || before_values.sends() != after_values.sends()
        {
            return false;
        }
    }

    changed_values == 1
}

fn snapshot_matches_parameters(
    json: &str,
    parameters: &ParameterSnapshot,
    target_track: MixerTrackId,
    accepted_generation: u64,
) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return false;
    };
    if value.get("generation").and_then(serde_json::Value::as_u64) != Some(accepted_generation)
        || parameters.generation() != accepted_generation
    {
        return false;
    }

    let expected_level = f64::from(parameters.mixer_track(target_track).level_db());
    value
        .get("mixer")
        .and_then(|mixer| mixer.get("tracks"))
        .and_then(serde_json::Value::as_array)
        .and_then(|tracks| tracks.get(target_track.index()))
        .and_then(|track| track.get("levelDb"))
        .and_then(serde_json::Value::as_f64)
        .is_some_and(|level| (level - expected_level).abs() < f64::EPSILON)
}

fn processed_patch_stem(
    raw: &[f32],
    patch: &RtPatchParameters,
    track: &MixerTrackParameters,
    global: &GlobalParameters,
) -> Vec<f32> {
    let gain = 10.0_f32.powf((patch.output().trim_gain_db() + track.level_db()) / 20.0);
    let (left_pan, right_pan) = if track.pan() < 0.0 {
        (1.0, 1.0 + track.pan())
    } else {
        (1.0 - track.pan(), 1.0)
    };
    let master_gain = 10.0_f32.powf(global.master_gain_db() / 20.0);
    let mut observed = raw.to_vec();
    for frame in observed.chunks_exact_mut(2) {
        frame[0] *= gain * left_pan * master_gain;
        frame[1] *= gain * right_pan * master_gain;
    }
    observed
}

fn measure_boundary_recovery<Control>(app_loop: &mut AppLoop<Control>) -> (bool, bool, u64)
where
    Control: ControlAudioBoundary,
{
    let mut boundary_noop_nonfatal = false;
    for _ in 0..16 {
        let before_text = app_loop.current_text();
        let before_parameters = app_loop.current_parameters().clone();
        match app_loop.dispatch(AppEvent::Adjust(Direction::Up)) {
            Ok(result) => {
                if result.boundary_full().is_some() {
                    break;
                }
            }
            Err(EventRejection::ParameterAtBoundary) => {
                boundary_noop_nonfatal = app_loop.current_text() == before_text
                    && app_loop.current_parameters().clone() == before_parameters;
                break;
            }
            Err(_) => break,
        }
    }

    let before_post_text = app_loop.current_text();
    let before_post_parameters = app_loop.current_parameters().clone();
    let post_boundary_edit_accepted = match app_loop.dispatch(AppEvent::Adjust(Direction::Down)) {
        Ok(result) => {
            let after_post_parameters = app_loop.current_parameters().clone();
            result.boundary_full().is_none()
                && result.accepted().generation()
                    == before_post_parameters.generation().saturating_add(1)
                && after_post_parameters.generation() == result.accepted().generation()
                && after_post_parameters != before_post_parameters
                && app_loop.current_text() != before_post_text
        }
        Err(_) => false,
    };
    let baseline_generation = app_loop.current_parameters().generation();

    (
        boundary_noop_nonfatal,
        post_boundary_edit_accepted,
        baseline_generation,
    )
}

struct ControlRuntime<Boundary>
where
    Boundary: ControlAudioBoundary,
{
    host: Option<Box<dyn ApplicationHost<Boundary>>>,
    sample_rate: u32,
    latest_audio: crate::real_time::AudioObservationSnapshot,
    audio_clock: Option<crate::shell::audio_output::AudioFrameClockReader>,
    test_midi: crate::shell::test_midi::TestMidiPattern,
    file_library: crate::shell::file_library::FileLibraryRuntime,
    gamepad: Option<crate::shell::gamepad::GamepadRuntime>,
    app_loop: AppLoop<Boundary>,
    lifecycle: SessionLifecycleCoordinator,
    full_demo: Option<FullInstrumentEffectDemo>,
    device_status: AudioDeviceStatusReader,
    midi_clock_micros: u64,
    close_requested: bool,
    error: Option<ApplicationError>,
}

impl<Boundary> ControlRuntime<Boundary>
where
    Boundary: ControlAudioBoundary,
{
    fn record_error(&mut self, error: ApplicationError) {
        if self.error.is_none() {
            self.error = Some(error);
        }
    }
}

fn input_callback<Boundary>(runtime: Rc<RefCell<ControlRuntime<Boundary>>>) -> AppInputCallback
where
    Boundary: ControlAudioBoundary + 'static,
{
    Box::new(move |event| {
        let mut runtime = runtime.borrow_mut();
        if runtime.error.is_some() || runtime.full_demo.is_some() {
            return;
        }
        if runtime.gamepad.is_none()
            && runtime
                .host
                .as_mut()
                .is_some_and(|host| host.action(event.clone()))
        {
            return;
        }
        if runtime.lifecycle.persisted_edits_blocked() && event.may_change_saved_session() {
            return;
        }

        let blocked = runtime.lifecycle.persisted_edits_blocked();
        let result = {
            let ControlRuntime {
                app_loop, gamepad, ..
            } = &mut *runtime;
            if let Some(gamepad) = gamepad {
                gamepad.dispatch_keyboard_action(app_loop, event, blocked)
            } else {
                match app_loop.dispatch_action_from(event, EventSource::Keyboard) {
                    Ok(result) => result.boundary_full().map_or(Ok(()), Err),
                    Err(_rejection) => Ok(()),
                }
            }
        };
        if let Err(error) = result {
            runtime.record_error(error.into());
        }
    })
}

fn projection_callback<Boundary>(
    runtime: Rc<RefCell<ControlRuntime<Boundary>>>,
) -> ProjectionCallback
where
    Boundary: ControlAudioBoundary + 'static,
{
    Box::new(move || runtime.borrow().app_loop.current_graphical_shell())
}

fn document_projection_callback<Boundary>(
    runtime: Rc<RefCell<ControlRuntime<Boundary>>>,
) -> SessionDocumentProjectionCallback
where
    Boundary: ControlAudioBoundary + 'static,
{
    Box::new(move || {
        let runtime = runtime.borrow();
        if let Some(host) = &runtime.host {
            return host.document();
        }
        if let Some(demo) = &runtime.full_demo {
            return demo.document();
        }
        runtime
            .lifecycle
            .project_shell(&runtime.app_loop)
            .document()
            .clone()
    })
}

fn session_command_callback<Boundary>(
    runtime: Rc<RefCell<ControlRuntime<Boundary>>>,
) -> SessionCommandCallback
where
    Boundary: ControlAudioBoundary + 'static,
{
    Box::new(move |command| {
        let mut runtime = runtime.borrow_mut();
        if runtime.full_demo.is_some() {
            if command == SessionCommand::Close {
                runtime.close_requested = true;
                return true;
            }
            return false;
        }
        if runtime.error.is_some() || runtime.close_requested {
            return command == SessionCommand::Close && runtime.close_requested;
        }
        let ControlRuntime {
            app_loop,
            lifecycle,
            close_requested,
            host,
            ..
        } = &mut *runtime;
        if let Some(host) = host {
            return host.session_command(
                &mut HostedSynthControl::with_lifecycle(app_loop, lifecycle),
                command,
            );
        }
        let result = lifecycle.request(command, app_loop);
        if result.is_err() {
            return false;
        }
        if command == SessionCommand::Close {
            match lifecycle.advance(app_loop) {
                Ok(progress) if progress.close_approved() => {
                    *close_requested = true;
                    true
                }
                Ok(_) | Err(_) => false,
            }
        } else {
            false
        }
    })
}

fn midi_activity_observation_callback<Boundary>(
    runtime: Rc<RefCell<ControlRuntime<Boundary>>>,
) -> MidiActivityObservationCallback
where
    Boundary: ControlAudioBoundary + 'static,
{
    Box::new(move || {
        let runtime = runtime.borrow();
        runtime
            .app_loop
            .current_midi_activity_observation(runtime.midi_clock_micros)
    })
}

fn tick_callback<Boundary>(runtime: Rc<RefCell<ControlRuntime<Boundary>>>) -> TickCallback
where
    Boundary: ControlAudioBoundary + 'static,
{
    Box::new(move |elapsed| {
        let mut runtime = runtime.borrow_mut();
        if runtime.error.is_some() {
            return false;
        }
        if let Some(failure) = runtime.device_status.take_on_control() {
            runtime.record_error(ApplicationError::AudioDeviceRuntime(failure));
            return false;
        }

        let ControlRuntime {
            app_loop,
            lifecycle,
            full_demo,
            device_status: _,
            host,
            sample_rate,
            latest_audio,
            audio_clock,
            gamepad,
            file_library,
            test_midi,
            midi_clock_micros,
            close_requested,
            error,
        } = &mut *runtime;
        if let Some(demo) = full_demo {
            if *close_requested {
                return false;
            }
            return match demo.advance(app_loop, elapsed) {
                Ok(keep_running) => keep_running,
                Err(failure) => {
                    *error = Some(ApplicationError::FullInstrumentEffectDemo(failure));
                    false
                }
            };
        }
        if let Some(gamepad) = gamepad {
            let owns_input = host.as_ref().is_some_and(|host| host.owns_input());
            let mut route = |input, bindings: &crate::control::ControllerBindings| {
                host.as_mut()
                    .is_some_and(|host| host.controller_input(input, bindings))
            };
            if let Err(failure) = gamepad.advance_hosted(
                app_loop,
                lifecycle.persisted_edits_blocked(),
                owns_input,
                elapsed,
                &mut route,
            ) {
                *error = Some(failure.into());
                return false;
            }
        }
        if let Err(failure) = file_library.advance(app_loop) {
            *error = Some(failure.into());
            return false;
        }
        let session_was_pending = app_loop.session_replacement_pending();
        let lifecycle_progress = if host.is_some() {
            lifecycle.advance_hosted(app_loop)
        } else {
            lifecycle.advance(app_loop)
        };
        match lifecycle_progress {
            Ok(progress) if progress.close_approved() => {
                *close_requested = true;
                return false;
            }
            Ok(_) | Err(_) => {}
        }
        if !session_was_pending && !app_loop.session_replacement_pending() {
            if let Err(failure) = app_loop.advance_structural() {
                *error = Some(failure.into());
                return false;
            }
        }
        let elapsed_micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        *midi_clock_micros = midi_clock_micros.saturating_add(elapsed_micros);
        let host_clock = HostClock {
            elapsed,
            control_micros: *midi_clock_micros,
            sample_rate: *sample_rate,
            audio: *latest_audio,
            audio_clock: audio_clock
                .as_ref()
                .and_then(|reader| reader.read_latest_on_control()),
        };
        let mut capture = |packet| match packet {
            crate::control::PhysicalMidiPacket::Performance(event) => host
                .as_mut()
                .is_some_and(|host| host.midi_input(event, host_clock)),
            crate::control::PhysicalMidiPacket::Transport(event) => {
                if let Some(host) = host.as_mut() {
                    host.midi_transport(event, host_clock);
                }
                true
            }
        };
        if let Err(failure) =
            app_loop.advance_midi_devices_with_packets(*midi_clock_micros, &mut capture)
        {
            *error = Some(failure.into());
            return false;
        }
        if let Err(failure) = test_midi.advance(app_loop, elapsed) {
            *error = Some(failure.into());
            return false;
        }
        if let Some(host) = host {
            while let Some(command) = app_loop.take_session_file_action() {
                if host.session_command(
                    &mut HostedSynthControl::with_lifecycle(app_loop, lifecycle),
                    command,
                ) && command == SessionCommand::Close
                {
                    *close_requested = true;
                }
            }
            host.tick(
                &mut HostedSynthControl::with_lifecycle(app_loop, lifecycle),
                host_clock,
            );
            *close_requested |= host.take_close_request();
        }
        error.is_none() && !*close_requested
    })
}

struct LiveControlRuntime<Source, Boundary, Observation, OnCheckpoint, OnComplete>
where
    Source: MidiEventSource,
    Boundary: ControlAudioBoundary,
    Observation: ControlAudioObservation,
    OnCheckpoint: FnMut(&LiveCheckpoint),
    OnComplete: FnOnce(&LiveDemoReport),
{
    runner: LiveDemoRunner<Source, Observation>,
    app_loop: AppLoop<Boundary>,
    device_status: AudioDeviceStatusReader,
    on_checkpoint: OnCheckpoint,
    on_complete: Option<OnComplete>,
    completion_emitted: bool,
    error: Option<ApplicationError>,
}

type SharedLiveRuntime<Source, Boundary, Observation, OnCheckpoint, OnComplete> =
    Rc<RefCell<LiveControlRuntime<Source, Boundary, Observation, OnCheckpoint, OnComplete>>>;

fn live_input_sink() -> AppInputCallback {
    Box::new(|_event| {})
}

fn live_projection_callback<Source, Boundary, Observation, OnCheckpoint, OnComplete>(
    runtime: SharedLiveRuntime<Source, Boundary, Observation, OnCheckpoint, OnComplete>,
) -> ProjectionCallback
where
    Source: MidiEventSource + 'static,
    Boundary: ControlAudioBoundary + 'static,
    Observation: ControlAudioObservation + 'static,
    OnCheckpoint: FnMut(&LiveCheckpoint) + 'static,
    OnComplete: FnOnce(&LiveDemoReport) + 'static,
{
    Box::new(move || runtime.borrow().app_loop.current_graphical_shell())
}

fn live_audio_observation_callback<Source, Boundary, Observation, OnCheckpoint, OnComplete>(
    runtime: SharedLiveRuntime<Source, Boundary, Observation, OnCheckpoint, OnComplete>,
) -> AudioObservationCallback
where
    Source: MidiEventSource + 'static,
    Boundary: ControlAudioBoundary + 'static,
    Observation: ControlAudioObservation + 'static,
    OnCheckpoint: FnMut(&LiveCheckpoint) + 'static,
    OnComplete: FnOnce(&LiveDemoReport) + 'static,
{
    Box::new(move || runtime.borrow().runner.latest_audio_observation())
}

fn live_tick_callback<Source, Boundary, Observation, OnCheckpoint, OnComplete>(
    runtime: SharedLiveRuntime<Source, Boundary, Observation, OnCheckpoint, OnComplete>,
) -> TickCallback
where
    Source: MidiEventSource + 'static,
    Boundary: ControlAudioBoundary + 'static,
    Observation: ControlAudioObservation + 'static,
    OnCheckpoint: FnMut(&LiveCheckpoint) + 'static,
    OnComplete: FnOnce(&LiveDemoReport) + 'static,
{
    Box::new(move |elapsed| {
        let mut runtime = runtime.borrow_mut();
        if runtime.error.is_some() || runtime.completion_emitted {
            return false;
        }
        if let Some(failure) = runtime.device_status.take_on_control() {
            runtime.error = Some(ApplicationError::AudioDeviceRuntime(failure));
            return false;
        }

        let LiveControlRuntime {
            runner,
            app_loop,
            device_status: _,
            on_checkpoint,
            on_complete,
            completion_emitted,
            error,
        } = &mut *runtime;
        if let Err(failure) = app_loop.advance_structural() {
            *error = Some(failure.into());
            return false;
        }
        match runner.advance(elapsed, app_loop) {
            Ok(Some(checkpoint)) => on_checkpoint(&checkpoint),
            Ok(None) => {}
            Err(failure) => {
                *error = Some(failure.into());
                return false;
            }
        }

        if !*completion_emitted {
            if let Some(report) = runner.completed_report() {
                if let Some(callback) = on_complete.take() {
                    callback(report);
                }
                *completion_emitted = true;
                if !report.complete() {
                    *error = Some(ApplicationError::LiveDemoIncomplete);
                }
                return false;
            }
        }
        error.is_none()
    })
}

fn live_frame_callback<Source, Boundary, Observation, OnCheckpoint, OnComplete>(
    runtime: SharedLiveRuntime<Source, Boundary, Observation, OnCheckpoint, OnComplete>,
) -> FrameObservationCallback
where
    Source: MidiEventSource + 'static,
    Boundary: ControlAudioBoundary + 'static,
    Observation: ControlAudioObservation + 'static,
    OnCheckpoint: FnMut(&LiveCheckpoint) + 'static,
    OnComplete: FnOnce(&LiveDemoReport) + 'static,
{
    Box::new(move |observation| {
        let mut runtime = runtime.borrow_mut();
        if runtime.error.is_some() || runtime.completion_emitted {
            return;
        }
        if let Err(failure) = runtime.runner.observe_shell_frame(observation) {
            runtime.error = Some(failure.into());
        }
    })
}

fn count_patch_rows(text: &str) -> usize {
    text.lines()
        .filter_map(|line| line.strip_prefix("TRACK "))
        .filter_map(|line| line.split_once("routedPatches=[").map(|(_, routes)| routes))
        .filter_map(|routes| routes.strip_suffix(']'))
        .map(|routes| {
            if routes.is_empty() {
                0
            } else {
                routes.split(',').count()
            }
        })
        .sum()
}

fn channels_are_round_robin(text: &str) -> bool {
    let mut routed_patch_count = 0_usize;
    for (track_index, line) in text
        .lines()
        .filter(|line| line.starts_with("TRACK T"))
        .enumerate()
    {
        let Some(routes) = line
            .split_once("routedPatches=[")
            .and_then(|(_, routes)| routes.strip_suffix(']'))
        else {
            return false;
        };
        if !routes.is_empty() {
            if track_index != routed_patch_count || routes.split(',').count() != 1 {
                return false;
            }
            routed_patch_count += 1;
        }
    }
    routed_patch_count > 0
}

#[cfg(test)]
mod tests {
    use super::{
        ApplicationConfig, ApplicationError, DegenerateMode, LiveSceneKind, StandaloneApplication,
    };
    use crate::adapter::atomic_audio_observation::AtomicAudioObservation;
    use crate::adapter::braids_capability::{BraidsCapability, BRAIDS_CAPABILITY_ID};
    use crate::adapter::lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary;
    use crate::control::app_event::Direction;
    use crate::control::event_record::EventSource;
    use crate::control::{
        DefaultSessionBlueprint, GraphicalShellProjection, InteractionMode, PatchControlId,
        SemanticAction,
    };
    use crate::kernel::midi_channel::MidiChannel;
    use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
    use crate::kernel::patch_id::PatchId;
    use crate::real_time::audio_boundary::{
        AudioBoundary, AudioThreadBoundary, BoundaryFull, ControlAudioBoundary,
    };
    use crate::real_time::audio_command::AudioCommand;
    use crate::real_time::parameter_snapshot::ParameterSnapshot;
    use crate::real_time::{GraphHandoffStatus, GraphRevision};
    use crate::shell::app_window::{
        AppInputCallback, AppWindow, AudioObservationCallback, FrameObservationCallback,
        MidiActivityObservationCallback, ProjectionCallback, SessionCommandCallback,
        SessionDocumentProjectionCallback, TickCallback, WindowError,
    };
    use crate::shell::audio_output::{
        AudioDeviceConfig, AudioDeviceStatusCallback, AudioOutput, AudioOutputError,
        AudioRenderCallback, AudioSampleFormat, AudioStream, NegotiatedAudioOutput,
    };
    use crate::shell::{
        ShellFrameObservation, ShellRegionId, ShellRegionObservation, ShellRegionRect,
    };
    use crate::synth::sound_font_instrument::SoundFontInstrument;
    use crate::synth::{
        CapabilityId, InstrumentCapabilityProvider, InstrumentPreparationError, InstrumentPreparer,
        Patch, PreparedInstrument, PreparedInstrumentError, VoiceEnvelope,
    };
    use crate::testing::instrument_part::InstrumentPart;
    use crate::testing::midi_event_source::{FixedEventBatch, MidiEventSource, MidiSourceError};
    use crate::testing::{LiveDemoError, LIVE_DEMO_NO_PROGRESS_TIMEOUT, LIVE_DEMO_TOTAL_TIMEOUT};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    struct Bus {
        commands: VecDeque<AudioCommand>,
        parameters: ParameterSnapshot,
        parameter_publications: usize,
        pending_parameters: Option<ParameterSnapshot>,
        parameters_dirty: bool,
    }

    #[derive(Clone)]
    struct TestBoundary {
        bus: Arc<Mutex<Bus>>,
    }

    struct TestControl {
        bus: Arc<Mutex<Bus>>,
    }

    struct TestAudio {
        bus: Arc<Mutex<Bus>>,
    }

    impl AudioBoundary for TestBoundary {
        type ControlHandle = TestControl;
        type AudioHandle = TestAudio;

        fn into_handles(self) -> (Self::ControlHandle, Self::AudioHandle) {
            (
                TestControl {
                    bus: Arc::clone(&self.bus),
                },
                TestAudio { bus: self.bus },
            )
        }
    }

    impl ControlAudioBoundary for TestControl {
        fn push_command(&mut self, command: AudioCommand) -> Result<(), BoundaryFull> {
            self.bus.lock().unwrap().commands.push_back(command);
            Ok(())
        }

        fn publish_parameters(&mut self, parameters: ParameterSnapshot) {
            let mut bus = self.bus.lock().unwrap();
            bus.pending_parameters = Some(parameters.clone());
            bus.parameters_dirty = true;
            bus.parameters = parameters;
            bus.parameter_publications += 1;
        }
    }

    impl AudioThreadBoundary for TestAudio {
        fn pop_command(&mut self) -> Option<AudioCommand> {
            self.bus.lock().unwrap().commands.pop_front()
        }

        fn exchange_latest_parameters(&mut self, previous: &mut ParameterSnapshot) -> bool {
            let mut bus = self.bus.lock().unwrap();
            if !bus.parameters_dirty {
                return false;
            }
            bus.parameters_dirty = false;
            core::mem::swap(bus.pending_parameters.as_mut().unwrap(), previous);
            true
        }
    }

    #[derive(Default)]
    struct EngineState {
        preparer_instances: usize,
        prepared: usize,
        dispatched: usize,
    }

    struct TestPreparer {
        state: Arc<Mutex<EngineState>>,
        capability_id: CapabilityId,
    }

    impl InstrumentPreparer for TestPreparer {
        fn capability_id(&self) -> &CapabilityId {
            &self.capability_id
        }

        fn prepared_shared_asset_count(&self) -> usize {
            usize::from(
                self.capability_id.as_str()
                    == crate::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID,
            )
        }

        fn prepare(
            &self,
            patch: &Patch,
            _sample_rate: f32,
            _max_frames: usize,
        ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
            self.state.lock().unwrap().prepared += 1;
            Ok(Box::new(TestInstrument {
                patch_id: patch.id(),
                state: Arc::clone(&self.state),
                sounding: false,
            }))
        }
    }

    struct TestInstrument {
        patch_id: PatchId,
        state: Arc<Mutex<EngineState>>,
        sounding: bool,
    }

    impl PreparedInstrument for TestInstrument {
        fn patch_id(&self) -> PatchId {
            self.patch_id
        }

        fn dispatch(
            &mut self,
            _message: MidiMessage,
            _parameters: &crate::real_time::RtPatchParameters,
        ) -> Result<(), PreparedInstrumentError> {
            self.state.lock().unwrap().dispatched += 1;
            self.sounding = true;
            Ok(())
        }

        fn render(
            &mut self,
            output: &mut [f32],
            _frame_count: usize,
            _parameters: &crate::real_time::RtPatchParameters,
        ) -> Result<(), crate::synth::PreparedInstrumentError> {
            if !self.sounding {
                return Ok(());
            }
            let index = self.patch_id.value().saturating_sub(1) as usize;
            let amplitude = 0.15 + index as f32 * 0.11;
            for frame in output.chunks_exact_mut(2) {
                frame[0] = amplitude;
                frame[1] = amplitude * (1.03 + index as f32 * 0.07);
            }

            Ok(())
        }

        fn all_notes_off(&mut self) {
            self.sounding = false;
        }
    }

    struct TestSource {
        parts: Vec<InstrumentPart>,
        due: Vec<MidiMessage>,
        started: bool,
    }

    impl MidiEventSource for TestSource {
        fn prepare(&mut self) -> Result<Vec<InstrumentPart>, MidiSourceError> {
            Ok(self.parts.clone())
        }

        fn start(&mut self) {
            self.started = true;
        }

        fn poll(
            &mut self,
            _elapsed: Duration,
            output: &mut FixedEventBatch,
        ) -> Result<(), MidiSourceError> {
            if !self.started {
                return Err(MidiSourceError::new("test source was not started"));
            }
            for event in self.due.drain(..) {
                output.try_push(event)?;
            }
            Ok(())
        }

        fn finished(&self) -> bool {
            self.started && self.due.is_empty()
        }
    }

    struct TestWindow {
        projection: Arc<Mutex<Option<GraphicalShellProjection>>>,
    }

    struct EarlyCloseWindow;

    impl AppWindow for EarlyCloseWindow {
        fn run(
            &self,
            _on_input: AppInputCallback,
            projection: ProjectionCallback,
            _audio_observation: AudioObservationCallback,
            _midi_activity: MidiActivityObservationCallback,
            _on_session_command: SessionCommandCallback,
            _document_projection: SessionDocumentProjectionCallback,
            _on_tick: TickCallback,
            _on_frame: FrameObservationCallback,
        ) -> Result<(), WindowError> {
            let _visible_initial_state = projection();
            Ok(())
        }
    }

    impl AppWindow for TestWindow {
        fn run(
            &self,
            mut on_input: AppInputCallback,
            projection: ProjectionCallback,
            _audio_observation: AudioObservationCallback,
            _midi_activity: MidiActivityObservationCallback,
            _on_session_command: SessionCommandCallback,
            _document_projection: SessionDocumentProjectionCallback,
            mut on_tick: TickCallback,
            _on_frame: FrameObservationCallback,
        ) -> Result<(), WindowError> {
            on_input(SemanticAction::Navigate(
                crate::control::app_event::Direction::Right,
            ));
            on_input(SemanticAction::SetInteractionMode(InteractionMode::Adjust));
            on_input(SemanticAction::Adjust(
                crate::control::app_event::Direction::Up,
            ));
            on_input(SemanticAction::Adjust(
                crate::control::app_event::Direction::Up,
            ));
            on_input(SemanticAction::Adjust(
                crate::control::app_event::Direction::Down,
            ));
            on_input(SemanticAction::Adjust(
                crate::control::app_event::Direction::Right,
            ));
            *self.projection.lock().unwrap() = Some(projection());
            assert!(on_tick(Duration::from_millis(20)));
            Ok(())
        }
    }

    struct TestOutput;

    impl AudioOutput for TestOutput {
        type Negotiated = Self;

        fn negotiate(self) -> Result<Self::Negotiated, AudioOutputError> {
            Ok(self)
        }
    }

    impl NegotiatedAudioOutput for TestOutput {
        fn config(&self) -> AudioDeviceConfig {
            AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 256).unwrap()
        }

        fn start(
            self,
            mut render: AudioRenderCallback,
            _on_runtime_error: AudioDeviceStatusCallback,
        ) -> Result<AudioStream, AudioOutputError> {
            let mut buffer = [0.0; 512];
            render(&mut buffer);
            Ok(AudioStream::new(()))
        }
    }

    struct FailIfUsedSource;

    impl MidiEventSource for FailIfUsedSource {
        fn prepare(&mut self) -> Result<Vec<InstrumentPart>, MidiSourceError> {
            panic!("normal startup must not prepare the automatic MIDI fixture")
        }

        fn start(&mut self) {
            panic!("normal startup must not start the automatic MIDI fixture")
        }

        fn poll(
            &mut self,
            _elapsed: Duration,
            _output: &mut FixedEventBatch,
        ) -> Result<(), MidiSourceError> {
            panic!("normal startup must not poll the automatic MIDI fixture")
        }

        fn finished(&self) -> bool {
            panic!("normal startup must not inspect the automatic MIDI fixture")
        }
    }

    struct StartupWindow {
        projection: Arc<Mutex<Option<GraphicalShellProjection>>>,
        document: Arc<Mutex<Option<crate::shell::SessionDocumentProjection>>>,
    }

    impl AppWindow for StartupWindow {
        fn run(
            &self,
            _on_input: AppInputCallback,
            projection: ProjectionCallback,
            _audio_observation: AudioObservationCallback,
            _midi_activity: MidiActivityObservationCallback,
            _on_session_command: SessionCommandCallback,
            document_projection: SessionDocumentProjectionCallback,
            mut on_tick: TickCallback,
            _on_frame: FrameObservationCallback,
        ) -> Result<(), WindowError> {
            *self.projection.lock().unwrap() = Some(projection());
            *self.document.lock().unwrap() = Some(document_projection());
            assert!(on_tick(Duration::from_millis(16)));
            Ok(())
        }
    }

    struct StartupSilenceOutput {
        silent: Arc<Mutex<Option<bool>>>,
    }

    impl AudioOutput for StartupSilenceOutput {
        type Negotiated = Self;

        fn negotiate(self) -> Result<Self::Negotiated, AudioOutputError> {
            Ok(self)
        }
    }

    impl NegotiatedAudioOutput for StartupSilenceOutput {
        fn config(&self) -> AudioDeviceConfig {
            AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 256).unwrap()
        }

        fn start(
            self,
            mut render: AudioRenderCallback,
            _on_runtime_error: AudioDeviceStatusCallback,
        ) -> Result<AudioStream, AudioOutputError> {
            let mut buffer = [1.0_f32; 512];
            render(&mut buffer);
            *self.silent.lock().unwrap() =
                Some(buffer.iter().all(|sample| sample.abs() <= f32::EPSILON));
            Ok(AudioStream::new(()))
        }
    }

    type SharedRender = Arc<Mutex<Option<AudioRenderCallback>>>;

    struct LiveTestOutput {
        render: SharedRender,
    }

    impl AudioOutput for LiveTestOutput {
        type Negotiated = Self;

        fn negotiate(self) -> Result<Self::Negotiated, AudioOutputError> {
            Ok(self)
        }
    }

    impl NegotiatedAudioOutput for LiveTestOutput {
        fn config(&self) -> AudioDeviceConfig {
            AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 256).unwrap()
        }

        fn start(
            self,
            render: AudioRenderCallback,
            _on_runtime_error: AudioDeviceStatusCallback,
        ) -> Result<AudioStream, AudioOutputError> {
            *self.render.lock().unwrap() = Some(render);
            Ok(AudioStream::new(()))
        }
    }

    #[derive(Clone, Default)]
    struct StalledLiveWitness {
        close_requests: Arc<Mutex<usize>>,
        reports: Arc<Mutex<usize>>,
        stream_released: Arc<Mutex<bool>>,
    }

    struct StalledStreamHandle {
        released: Arc<Mutex<bool>>,
    }

    impl Drop for StalledStreamHandle {
        fn drop(&mut self) {
            *self.released.lock().unwrap() = true;
        }
    }

    struct StalledLiveOutput {
        released: Arc<Mutex<bool>>,
    }

    impl AudioOutput for StalledLiveOutput {
        type Negotiated = Self;

        fn negotiate(self) -> Result<Self::Negotiated, AudioOutputError> {
            Ok(self)
        }
    }

    impl NegotiatedAudioOutput for StalledLiveOutput {
        fn config(&self) -> AudioDeviceConfig {
            AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 16).unwrap()
        }

        fn start(
            self,
            _render: AudioRenderCallback,
            _on_runtime_error: AudioDeviceStatusCallback,
        ) -> Result<AudioStream, AudioOutputError> {
            Ok(AudioStream::new(StalledStreamHandle {
                released: self.released,
            }))
        }
    }

    struct StalledLiveWindow {
        close_requests: Arc<Mutex<usize>>,
        tick_duration: Duration,
    }

    impl AppWindow for StalledLiveWindow {
        fn run(
            &self,
            _on_input: AppInputCallback,
            _projection: ProjectionCallback,
            _audio_observation: AudioObservationCallback,
            _midi_activity: MidiActivityObservationCallback,
            _on_session_command: SessionCommandCallback,
            _document_projection: SessionDocumentProjectionCallback,
            mut on_tick: TickCallback,
            _on_frame: FrameObservationCallback,
        ) -> Result<(), WindowError> {
            for _ in 0..64 {
                if !on_tick(self.tick_duration) {
                    *self.close_requests.lock().unwrap() += 1;
                    return Ok(());
                }
            }
            Err(WindowError::new(
                "stalled live window did not receive a bounded close request",
            ))
        }
    }

    #[derive(Clone, Default)]
    struct LiveWindowWitness {
        reports: Arc<Mutex<Vec<crate::testing::LiveDemoReport>>>,
        final_projection: Arc<Mutex<Option<GraphicalShellProjection>>>,
        input_injected: Arc<Mutex<bool>>,
        close_requests: Arc<Mutex<usize>>,
        post_completion_ticks: Arc<Mutex<usize>>,
    }

    struct LiveTestWindow {
        render: SharedRender,
        bus: Arc<Mutex<Bus>>,
        witness: LiveWindowWitness,
    }

    impl AppWindow for LiveTestWindow {
        fn run(
            &self,
            mut on_input: AppInputCallback,
            projection: ProjectionCallback,
            audio_observation: AudioObservationCallback,
            _midi_activity: MidiActivityObservationCallback,
            _on_session_command: SessionCommandCallback,
            _document_projection: SessionDocumentProjectionCallback,
            mut on_tick: TickCallback,
            mut on_frame: FrameObservationCallback,
        ) -> Result<(), WindowError> {
            let mut last_rendered_projection = projection();
            let mut input_injected = false;
            // The cumulative live scene dwells on every Patch output, all 96
            // fixed track controls, and every global before its structural
            // sequence. Keep this deterministic harness comfortably above the
            // descriptor-derived paced bound while still failing finitely.
            for _ in 0..4_096 {
                let reports_before = self.witness.reports.lock().unwrap().len();
                let keep_open = on_tick(Duration::from_millis(50));
                let reports_after = self.witness.reports.lock().unwrap().len();
                if !keep_open {
                    if reports_after > 0 {
                        assert_eq!(reports_before, 0);
                        assert_eq!(reports_after, 1);
                        *self.witness.close_requests.lock().unwrap() += 1;
                        *self.witness.final_projection.lock().unwrap() =
                            Some(last_rendered_projection);
                    }
                    return Ok(());
                }

                if reports_after > 0 {
                    *self.witness.post_completion_ticks.lock().unwrap() += 1;
                }

                if !input_injected {
                    // The first tick has dispatched the first autonomous scene
                    // event. Inject a mapped semantic adjustment before audio
                    // renders that pending generation, matching the production
                    // race that previously replaced the awaited snapshot.
                    let projection_before = projection();
                    let (parameters_before, publications_before) = {
                        let bus = self.bus.lock().unwrap();
                        (bus.parameters.clone(), bus.parameter_publications)
                    };
                    on_input(SemanticAction::Adjust(Direction::Down));
                    let (parameters_after, publications_after) = {
                        let bus = self.bus.lock().unwrap();
                        (bus.parameters.clone(), bus.parameter_publications)
                    };
                    assert_eq!(
                        parameters_after.generation(),
                        parameters_before.generation()
                    );
                    assert_eq!(parameters_after, parameters_before);
                    assert_eq!(publications_after, publications_before);
                    assert_eq!(projection(), projection_before);
                    *self.witness.input_injected.lock().unwrap() = true;
                    input_injected = true;
                }

                {
                    let mut render = self.render.lock().unwrap();
                    let render = render
                        .as_mut()
                        .ok_or_else(|| WindowError::new("live audio callback was not opened"))?;
                    let mut buffer = [0.0_f32; 512];
                    render(&mut buffer);
                }
                let _latest_audio_observation = audio_observation();
                std::thread::yield_now();
                std::thread::sleep(Duration::from_millis(1));
                last_rendered_projection = projection();
                on_frame(shell_frame_observation(&last_rendered_projection));
            }
            Err(WindowError::new(
                "live deterministic window did not observe completion",
            ))
        }
    }

    fn shell_frame_observation(projection: &GraphicalShellProjection) -> ShellFrameObservation {
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

    fn parameters() -> ParameterSnapshot {
        ParameterSnapshot::new(
            0,
            ApplicationConfig::default().global_parameters(),
            crate::mixer::mixer_state::MixerState::default(),
            &[],
        )
        .unwrap()
    }

    fn parts() -> Vec<InstrumentPart> {
        vec![
            InstrumentPart::new(
                0,
                "Piano".to_owned(),
                SoundFontInstrument::new(0, 0, false).unwrap(),
            ),
            InstrumentPart::new(
                1,
                "Strings".to_owned(),
                SoundFontInstrument::new(0, 48, false).unwrap(),
            ),
        ]
    }

    fn message() -> MidiMessage {
        MidiMessage::try_new(
            MidiChannel::new(0).unwrap(),
            MidiMessageKind::NoteOn,
            60,
            100,
        )
        .unwrap()
    }

    fn preparer_for(state: Arc<Mutex<EngineState>>, capability_id: &str) -> TestPreparer {
        state.lock().unwrap().preparer_instances += 1;
        TestPreparer {
            state,
            capability_id: CapabilityId::new(capability_id).unwrap(),
        }
    }

    fn providers() -> Vec<Box<dyn InstrumentCapabilityProvider>> {
        vec![
            Box::new(
                crate::adapter::production_instruments::production_soundfont_capability().unwrap(),
            ),
            Box::new(BraidsCapability::new().unwrap()),
        ]
    }

    fn preparers(state: Arc<Mutex<EngineState>>) -> Vec<Box<dyn InstrumentPreparer>> {
        vec![
            Box::new(preparer_for(
                Arc::clone(&state),
                "instrument.soundfont.hidef",
            )),
            Box::new(preparer_for(state, BRAIDS_CAPABILITY_ID)),
        ]
    }

    fn structural_boundary() -> LockFreeStructuralGraphBoundary {
        LockFreeStructuralGraphBoundary::new(
            1,
            1,
            GraphHandoffStatus::with_active(GraphRevision::INITIAL),
        )
        .unwrap()
    }

    fn test_default_session_blueprint() -> DefaultSessionBlueprint {
        DefaultSessionBlueprint::new(CapabilityId::new("instrument.soundfont.hidef").unwrap())
    }

    type TestApplication<Window, Output> = StandaloneApplication<
        TestBoundary,
        LockFreeStructuralGraphBoundary,
        AtomicAudioObservation,
        TestSource,
        Window,
        Output,
    >;

    fn application(
        due: Vec<MidiMessage>,
        engine_state: Arc<Mutex<EngineState>>,
        projection: Arc<Mutex<Option<GraphicalShellProjection>>>,
    ) -> TestApplication<TestWindow, TestOutput> {
        StandaloneApplication::new_with_effects(
            TestBoundary {
                bus: Arc::new(Mutex::new(Bus {
                    commands: VecDeque::new(),
                    parameters: parameters(),
                    parameter_publications: 0,
                    pending_parameters: None,
                    parameters_dirty: false,
                })),
            },
            providers(),
            preparers(engine_state),
            crate::adapter::production_effects::production_effect_providers().unwrap(),
            crate::adapter::production_effects::production_effect_preparers().unwrap(),
            structural_boundary(),
            AtomicAudioObservation::default(),
            TestSource {
                parts: parts(),
                due,
                started: false,
            },
            TestWindow { projection },
            TestOutput,
            ApplicationConfig::new(
                48_000.0,
                256,
                ApplicationConfig::default().global_parameters(),
            ),
        )
        .unwrap()
        .with_default_session_blueprint(test_default_session_blueprint())
    }

    /// Composes the live application plus its deterministic harness window.
    /// The harness window is returned separately and handed to
    /// `host_live_demo_scene` — mirroring production, where live hosting
    /// composes its own webview window and the injected interactive window
    /// (a plain placeholder here) takes no part in the live run.
    fn live_application(
        due: Vec<MidiMessage>,
        engine_state: Arc<Mutex<EngineState>>,
        witness: LiveWindowWitness,
    ) -> (TestApplication<TestWindow, LiveTestOutput>, LiveTestWindow) {
        let render = Arc::new(Mutex::new(None));
        let bus = Arc::new(Mutex::new(Bus {
            commands: VecDeque::new(),
            parameters: parameters(),
            parameter_publications: 0,
            pending_parameters: None,
            parameters_dirty: false,
        }));
        let live_window = LiveTestWindow {
            render: Arc::clone(&render),
            bus: Arc::clone(&bus),
            witness,
        };
        let application = StandaloneApplication::new_with_effects(
            TestBoundary { bus },
            providers(),
            preparers(engine_state),
            crate::adapter::production_effects::production_effect_providers().unwrap(),
            crate::adapter::production_effects::production_effect_preparers().unwrap(),
            structural_boundary(),
            AtomicAudioObservation::default(),
            TestSource {
                parts: parts(),
                due,
                started: false,
            },
            TestWindow {
                projection: Arc::new(Mutex::new(None)),
            },
            LiveTestOutput { render },
            ApplicationConfig::new(
                48_000.0,
                256,
                ApplicationConfig::default().global_parameters(),
            ),
        )
        .unwrap()
        .with_default_session_blueprint(test_default_session_blueprint());
        (application, live_window)
    }

    fn early_close_application() -> (TestApplication<TestWindow, TestOutput>, EarlyCloseWindow) {
        let application = StandaloneApplication::new_with_effects(
            TestBoundary {
                bus: Arc::new(Mutex::new(Bus {
                    commands: VecDeque::new(),
                    parameters: parameters(),
                    parameter_publications: 0,
                    pending_parameters: None,
                    parameters_dirty: false,
                })),
            },
            providers(),
            preparers(Arc::new(Mutex::new(EngineState::default()))),
            crate::adapter::production_effects::production_effect_providers().unwrap(),
            crate::adapter::production_effects::production_effect_preparers().unwrap(),
            structural_boundary(),
            AtomicAudioObservation::default(),
            TestSource {
                parts: parts(),
                due: vec![message()],
                started: false,
            },
            TestWindow {
                projection: Arc::new(Mutex::new(None)),
            },
            TestOutput,
            ApplicationConfig::new(
                48_000.0,
                256,
                ApplicationConfig::default().global_parameters(),
            ),
        )
        .unwrap()
        .with_default_session_blueprint(test_default_session_blueprint());
        (application, EarlyCloseWindow)
    }

    fn stalled_live_application(
        witness: StalledLiveWitness,
        tick_duration: Duration,
    ) -> (
        TestApplication<TestWindow, StalledLiveOutput>,
        StalledLiveWindow,
    ) {
        let application = StandaloneApplication::new_with_effects(
            TestBoundary {
                bus: Arc::new(Mutex::new(Bus {
                    commands: VecDeque::new(),
                    parameters: parameters(),
                    parameter_publications: 0,
                    pending_parameters: None,
                    parameters_dirty: false,
                })),
            },
            providers(),
            preparers(Arc::new(Mutex::new(EngineState::default()))),
            crate::adapter::production_effects::production_effect_providers().unwrap(),
            crate::adapter::production_effects::production_effect_preparers().unwrap(),
            structural_boundary(),
            AtomicAudioObservation::default(),
            TestSource {
                parts: parts(),
                due: vec![message()],
                started: false,
            },
            TestWindow {
                projection: Arc::new(Mutex::new(None)),
            },
            StalledLiveOutput {
                released: Arc::clone(&witness.stream_released),
            },
            ApplicationConfig::new(
                48_000.0,
                256,
                ApplicationConfig::default().global_parameters(),
            ),
        )
        .unwrap()
        .with_default_session_blueprint(test_default_session_blueprint());
        let live_window = StalledLiveWindow {
            close_requests: Arc::clone(&witness.close_requests),
            tick_duration,
        };
        (application, live_window)
    }

    #[test]
    fn normal_run_prepares_only_init_and_joins_window_input_to_the_shared_loop() {
        let engine_state = Arc::new(Mutex::new(EngineState::default()));
        let projection = Arc::new(Mutex::new(None));
        let due = message();

        application(
            vec![due],
            Arc::clone(&engine_state),
            Arc::clone(&projection),
        )
        .run()
        .unwrap();

        let engine_state = engine_state.lock().unwrap();
        assert_eq!(engine_state.preparer_instances, 2);
        assert_eq!(engine_state.prepared, 1);
        let projection = projection.lock().unwrap();
        let projection = projection.as_ref().unwrap();
        assert_eq!(projection.context(), crate::control::TopLevelContext::Patch);
        let json = serde_json::to_string(projection.semantic_model()).unwrap();
        assert!(json.contains(DefaultSessionBlueprint::PATCH_NAME));
        assert!(!json.contains("Piano"));
        assert!(!json.contains("Strings"));
    }

    #[test]
    fn normal_startup_never_consults_fixture_and_first_shell_is_clean_silent_init() {
        let engine_state = Arc::new(Mutex::new(EngineState::default()));
        let projection = Arc::new(Mutex::new(None));
        let document = Arc::new(Mutex::new(None));
        let silent = Arc::new(Mutex::new(None));
        let application = StandaloneApplication::new_with_effects(
            TestBoundary {
                bus: Arc::new(Mutex::new(Bus {
                    commands: VecDeque::new(),
                    parameters: parameters(),
                    parameter_publications: 0,
                    pending_parameters: None,
                    parameters_dirty: false,
                })),
            },
            providers(),
            preparers(Arc::clone(&engine_state)),
            crate::adapter::production_effects::production_effect_providers().unwrap(),
            crate::adapter::production_effects::production_effect_preparers().unwrap(),
            structural_boundary(),
            AtomicAudioObservation::default(),
            FailIfUsedSource,
            StartupWindow {
                projection: Arc::clone(&projection),
                document: Arc::clone(&document),
            },
            StartupSilenceOutput {
                silent: Arc::clone(&silent),
            },
            ApplicationConfig::new(
                48_000.0,
                256,
                ApplicationConfig::default().global_parameters(),
            ),
        )
        .unwrap()
        .with_default_session_blueprint(test_default_session_blueprint());

        application.run().unwrap();

        assert_eq!(*silent.lock().unwrap(), Some(true));
        assert_eq!(engine_state.lock().unwrap().prepared, 1);
        let projection = projection.lock().unwrap();
        let projection = projection.as_ref().unwrap();
        assert_eq!(projection.context(), crate::control::TopLevelContext::Patch);
        let json = serde_json::to_string(projection.semantic_model()).unwrap();
        assert!(json.contains("\"patchName\":\"00\""));
        let document = document.lock().unwrap();
        let document = document.as_ref().unwrap();
        assert_eq!(document.name(), "Untitled");
        assert!(!document.dirty());
        assert_eq!(
            document.marker(),
            crate::shell::SessionDocumentMarker::Ready
        );
        assert_eq!(document.status(), "READY");
    }

    #[test]
    fn standalone_exhaustive_gui_demo_composes_a_complete_production_trace() {
        let engine_state = Arc::new(Mutex::new(EngineState::default()));
        let report = application(
            vec![message()],
            Arc::clone(&engine_state),
            Arc::new(Mutex::new(None)),
        )
        .run_demo_scene(None)
        .unwrap();

        assert!(
            report.is_complete(),
            "coverage={:?}; audio={:?}; eventMissing={:?}; eventUnexpected={:?}",
            report.coverage(),
            report.audio_evidence(),
            report.event_log().coverage().missing(),
            report.event_log().coverage().unexpected(),
        );
        assert_eq!(report.coverage().missing_count(), 0);
        assert_eq!(report.event_log().dropped_records(), 0);
        assert!(report.event_log().records().len() > 1);
        assert_eq!(report.final_state_tree().patch_count(), 2);
        assert!(report.checkpoints().len() > 10);
        assert!(engine_state.lock().unwrap().dispatched > 0);
    }

    #[test]
    fn live_scenes_compose_the_webview_shell_window() {
        // Mission webview-shell-cutover WP03 (T010): the production live
        // entry composes the webview shell — `live_demo_window()` returns
        // the concrete `TauriWebviewWindow` (compile-time fact) carrying the
        // unchanged autonomous live title. Since WP07 the interactive
        // composition root constructs the same shell directly.
        let window: crate::shell::webview::TauriWebviewWindow = super::live_demo_window();
        assert_eq!(window.title(), "crest-synth — autonomous live demo");
    }

    #[test]
    fn standalone_live_demo_composition_isolates_input_and_closes_on_completion() {
        let engine_state = Arc::new(Mutex::new(EngineState::default()));
        let witness = LiveWindowWitness::default();
        let checkpoints = Arc::new(Mutex::new(Vec::new()));
        let checkpoints_for_callback = Arc::clone(&checkpoints);
        let reports_for_callback = Arc::clone(&witness.reports);

        let (application, live_window) =
            live_application(vec![message()], Arc::clone(&engine_state), witness.clone());
        let teardown = application.host_live_demo_scene(
            live_window,
            LiveSceneKind::SixteenTrackMixerRouting,
            move |checkpoint| {
                checkpoints_for_callback
                    .lock()
                    .unwrap()
                    .push(checkpoint.clone())
            },
            move |report| reports_for_callback.lock().unwrap().push(report.clone()),
        );
        if let Err(error) = &teardown {
            let reports = witness.reports.lock().unwrap();
            panic!(
                "live composition failed: {error:?}; report={}",
                reports
                    .first()
                    .map(crate::testing::LiveDemoReport::summary)
                    .unwrap_or("<none>")
            );
        }
        let teardown = teardown.unwrap();
        assert!(teardown.complete());
        let teardown_json = serde_json::to_value(&teardown).unwrap();
        assert_eq!(teardown_json["window_closed"], true);
        assert_eq!(teardown_json["stream_released"], true);
        assert_eq!(teardown_json["owned_graphs_remaining"], 0);
        assert_eq!(teardown_json["active_notes_after_cleanup"], 0);
        let routing = teardown.sixteen_track_mixer_routing();
        assert!(routing.is_complete());
        let routing_json = serde_json::to_value(routing).unwrap();
        assert_eq!(routing_json.as_object().unwrap().len(), 24);
        assert_eq!(
            routing_json["track_count"],
            crate::mixer::mixer_track_id::MixerTrackId::COUNT
        );
        assert_eq!(routing_json["shared_track_patch_count"], 2);
        assert_eq!(routing_json["physical_audio_nonzero"], true);
        assert!(routing_json.get("context_line_visible").is_none());

        let reports = witness.reports.lock().unwrap();
        assert_eq!(reports.len(), 1);
        let report = &reports[0];
        assert!(report.complete(), "{}", report.summary());
        assert_eq!(checkpoints.lock().unwrap().as_slice(), report.checkpoints());
        let patch_adsr = report
            .checkpoints()
            .iter()
            .filter_map(|checkpoint| checkpoint.as_parameter())
            .filter(|checkpoint| {
                matches!(
                    checkpoint.expected_transition().patch_control_id(),
                    Some(PatchControlId::Envelope(_))
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(patch_adsr.len(), VoiceEnvelope::surface_descriptor().len());
        assert!(patch_adsr.iter().all(|checkpoint| {
            checkpoint
                .expected_transition()
                .patch_control_id()
                .is_some_and(|control| control != PatchControlId::Engine)
                && checkpoint.projected_value().patch_control_id()
                    == checkpoint.expected_transition().patch_control_id()
        }));
        let patch_effects = report
            .checkpoints()
            .iter()
            .filter_map(|checkpoint| checkpoint.as_parameter())
            .filter(|checkpoint| {
                matches!(
                    checkpoint.expected_transition().patch_control_id(),
                    Some(PatchControlId::Effect(_, _))
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(patch_effects.len(), 2);
        assert!(patch_effects.iter().all(|checkpoint| {
            let effect = checkpoint.audio_observation().patch_effect();
            effect.patch_id() == Some(PatchId::new(1).unwrap())
                && effect.difference_rms() > 0.0
                && effect.side_rms() > 0.0
        }));
        let structural = report
            .checkpoints()
            .iter()
            .filter_map(|checkpoint| checkpoint.as_engine())
            .collect::<Vec<_>>();
        assert_eq!(structural.len(), 9);
        assert!(structural[..3]
            .iter()
            .all(|checkpoint| checkpoint.preset().is_some()));
        assert!(structural[3..]
            .iter()
            .all(|checkpoint| checkpoint.focused_control_id() == PatchControlId::Engine));
        assert_eq!(report.runtime_audio().callback_allocations(), 0);
        assert_eq!(report.runtime_audio().callback_destructions(), 0);
        assert!(*witness.input_injected.lock().unwrap());
        assert_eq!(*witness.close_requests.lock().unwrap(), 1);
        assert_eq!(*witness.post_completion_ticks.lock().unwrap(), 0);
        assert!(report
            .event_log()
            .records()
            .iter()
            .all(|record| record.source() != EventSource::Keyboard));
        let projection = witness.final_projection.lock().unwrap();
        let projection = projection.as_ref().unwrap();
        assert_eq!(projection.state_hash(), report.state_tree().state_hash());
        assert!(engine_state.lock().unwrap().dispatched > 0);
    }

    #[test]
    fn standalone_live_demo_early_close_is_typed_and_never_reports_success() {
        let checkpoints = Arc::new(Mutex::new(0_usize));
        let reports = Arc::new(Mutex::new(0_usize));
        let checkpoints_for_callback = Arc::clone(&checkpoints);
        let reports_for_callback = Arc::clone(&reports);

        let (application, live_window) = early_close_application();
        let error = application
            .host_live_demo_scene(
                live_window,
                LiveSceneKind::SixteenTrackMixerRouting,
                move |_| *checkpoints_for_callback.lock().unwrap() += 1,
                move |_| *reports_for_callback.lock().unwrap() += 1,
            )
            .unwrap_err();

        assert!(matches!(error, super::ApplicationError::LiveDemoIncomplete));
        assert_eq!(*checkpoints.lock().unwrap(), 0);
        assert_eq!(*reports.lock().unwrap(), 0);
    }

    #[test]
    fn standalone_live_demo_stall_times_out_closes_and_releases_stream() {
        let witness = StalledLiveWitness::default();
        let reports_for_callback = Arc::clone(&witness.reports);

        let (application, live_window) =
            stalled_live_application(witness.clone(), Duration::from_secs(1));
        let error = application
            .host_live_demo_scene(
                live_window,
                LiveSceneKind::SixteenTrackMixerRouting,
                |_| {},
                move |_| *reports_for_callback.lock().unwrap() += 1,
            )
            .unwrap_err();

        assert!(matches!(
            error,
            ApplicationError::LiveDemo(LiveDemoError::ProgressTimedOut {
                stage: "parameter audio observation",
                stalled_for,
                ..
            }) if stalled_for >= LIVE_DEMO_NO_PROGRESS_TIMEOUT
        ));
        assert_eq!(*witness.reports.lock().unwrap(), 0);
        assert_eq!(*witness.close_requests.lock().unwrap(), 1);
        assert!(*witness.stream_released.lock().unwrap());
    }

    #[test]
    fn standalone_live_demo_total_bound_closes_and_releases_stream() {
        let witness = StalledLiveWitness::default();
        let reports_for_callback = Arc::clone(&witness.reports);

        let (application, live_window) =
            stalled_live_application(witness.clone(), LIVE_DEMO_TOTAL_TIMEOUT);
        let error = application
            .host_live_demo_scene(
                live_window,
                LiveSceneKind::SixteenTrackMixerRouting,
                |_| {},
                move |_| *reports_for_callback.lock().unwrap() += 1,
            )
            .unwrap_err();

        assert!(matches!(
            error,
            ApplicationError::LiveDemo(LiveDemoError::TotalTimeout { elapsed, .. })
                if elapsed >= LIVE_DEMO_TOTAL_TIMEOUT
        ));
        assert_eq!(*witness.reports.lock().unwrap(), 0);
        assert_eq!(*witness.close_requests.lock().unwrap(), 1);
        assert!(*witness.stream_released.lock().unwrap());
    }

    #[test]
    fn standalone_exhaustive_gui_demo_control_degeneracy_is_detectable() {
        let report = application(
            vec![message()],
            Arc::new(Mutex::new(EngineState::default())),
            Arc::new(Mutex::new(None)),
        )
        .run_demo_scene(Some(DegenerateMode::Control))
        .unwrap();

        assert!(!report.is_complete());
        assert!(report.coverage().missing_count() > 0);
        assert_eq!(report.event_log().dropped_records(), 0);
    }

    #[test]
    fn smoke_observation_uses_real_dispatch_and_render_measurements() {
        let engine_state = Arc::new(Mutex::new(EngineState::default()));
        let observation = application(
            vec![message()],
            Arc::clone(&engine_state),
            Arc::new(Mutex::new(None)),
        )
        .run_smoke(None)
        .unwrap();

        assert_eq!(observation.parsed_soundfont_banks, 1);
        assert_eq!(observation.prepared_instruments, 2);
        assert_eq!(observation.active_graph_revision, 1);
        assert_eq!(observation.callback_allocations, 0);
        assert_eq!(observation.callback_destructions, 0);
        assert_eq!(observation.patch_rows, 2);
        // Fifteen separators between the sixteen tracks plus one before the
        // RETURNS section and one before the GLOBAL section.
        assert_eq!(observation.channel_separators, 17);
        assert!(observation.round_robin_channels);
        assert!(observation.distinct_patch_channels);
        assert!(observation.distinct_patch_stems);
        assert!(observation.automatic_midi);
        assert_eq!(observation.event_commands_delivered, 1);
        assert!(observation.one_value_changed);
        assert!(observation.state_roundtrip);
        assert!(observation.text_matches_state);
        assert!(observation.parameter_published);
        assert!(observation.engine_consumed_value);
        assert_eq!(observation.edited_patch_id, 2);
        assert!(observation.edited_patch_audio_changed);
        assert!(observation.unedited_patch_audio_unchanged);
        assert!(observation.per_patch_audio_isolated);
        assert!(observation.boundary_noop_nonfatal);
        assert!(observation.post_boundary_edit_accepted);
        assert!(observation.audio_changed);
        assert!(observation.peak > 0.001);
        assert!(engine_state.lock().unwrap().dispatched > 0);
    }

    #[test]
    fn degenerate_modes_falsify_their_respective_observations() {
        let audio = application(
            vec![message()],
            Arc::new(Mutex::new(EngineState::default())),
            Arc::new(Mutex::new(None)),
        )
        .run_smoke(Some(DegenerateMode::Audio))
        .unwrap();
        let control = application(
            vec![message()],
            Arc::new(Mutex::new(EngineState::default())),
            Arc::new(Mutex::new(None)),
        )
        .run_smoke(Some(DegenerateMode::Control))
        .unwrap();

        assert_eq!(audio.peak, 0.0);
        assert!(!audio.audio_changed);
        assert!(audio.one_value_changed);
        assert!(audio.edited_patch_audio_changed);
        assert!(audio.unedited_patch_audio_unchanged);
        assert!(audio.per_patch_audio_isolated);
        assert!(audio.boundary_noop_nonfatal);
        assert!(audio.post_boundary_edit_accepted);

        assert_eq!(control.edited_patch_id, 2);
        assert!(!control.one_value_changed);
        assert!(!control.state_roundtrip);
        assert!(!control.text_matches_state);
        assert!(!control.parameter_published);
        assert!(!control.engine_consumed_value);
        assert!(!control.edited_patch_audio_changed);
        assert!(control.unedited_patch_audio_unchanged);
        assert!(!control.per_patch_audio_isolated);
        assert!(control.boundary_noop_nonfatal);
        assert!(control.post_boundary_edit_accepted);
        assert!(!control.audio_changed);
    }
}
