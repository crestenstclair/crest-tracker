use crate::control::app_event::{AppEvent, Direction};
use crate::control::app_state::{EventRejection, StateAccepted};
use crate::control::engine_selection::{
    EngineSelectionEffect, EngineSelectionFailure, EngineSelectionRequestId, StructuralEditIntent,
};
use crate::control::state_snapshot::StateSnapshot;
use crate::control::text_projection::TextProjection;
use crate::control::top_level_context::TopLevelContext;
use crate::control::{
    InteractionMode, MidiConnectionRequestId, MidiConnectionRevision, MidiDeviceFailure,
    MidiInputDescriptor, MidiInputDeviceId, MidiInputPreference, MidiInputScanId, SurfaceId,
};
use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crate::real_time::audio_command::AudioCommand;
use crate::real_time::GraphRevision;
use crate::synth::instrument_capability::InstrumentConfig;
use crate::synth::patch::Patch;
use crate::synth::voice_envelope::VoiceEnvelope;
use crate::synth::PostEffectConfig;
use core::fmt;
use serde::{Serialize, Serializer};

/// The stable origin of an application event.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EventSource {
    Startup,
    Keyboard,
    Controller,
    AutomaticMidi,
    PhysicalMidi,
    DemoScene,
    Worker,
    System,
}

impl EventSource {
    pub const ALL: [Self; 8] = [
        Self::Startup,
        Self::Keyboard,
        Self::Controller,
        Self::AutomaticMidi,
        Self::PhysicalMidi,
        Self::DemoScene,
        Self::Worker,
        Self::System,
    ];

    pub const fn surface_descriptor() -> &'static [Self] {
        &Self::ALL
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Keyboard => "keyboard",
            Self::Controller => "controller",
            Self::AutomaticMidi => "automaticMidi",
            Self::PhysicalMidi => "physicalMidi",
            Self::DemoScene => "demoScene",
            Self::Worker => "worker",
            Self::System => "system",
        }
    }
}

/// Whether the reducer accepted or rejected an input.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EventOutcome {
    Accepted,
    Rejected,
}

/// A stable direction value used by the serialized input descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EventDirection {
    Up,
    Down,
    Left,
    Right,
}

impl From<Direction> for EventDirection {
    fn from(direction: Direction) -> Self {
        match direction {
            Direction::Up => Self::Up,
            Direction::Down => Self::Down,
            Direction::Left => Self::Left,
            Direction::Right => Self::Right,
        }
    }
}

/// The stable kind of a normalized MIDI message.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MidiKind {
    NoteOn,
    NoteOff,
    ControlChange,
    ProgramChange,
    ChannelPressure,
    PitchBend,
    AllNotesOff,
    PolyPressure,
}

impl From<MidiMessageKind> for MidiKind {
    fn from(kind: MidiMessageKind) -> Self {
        match kind {
            MidiMessageKind::NoteOn => Self::NoteOn,
            MidiMessageKind::NoteOff => Self::NoteOff,
            MidiMessageKind::ControlChange => Self::ControlChange,
            MidiMessageKind::ProgramChange => Self::ProgramChange,
            MidiMessageKind::ChannelPressure => Self::ChannelPressure,
            MidiMessageKind::PitchBend => Self::PitchBend,
            MidiMessageKind::AllNotesOff => Self::AllNotesOff,
            MidiMessageKind::PolyPressure => Self::PolyPressure,
        }
    }
}

/// A complete, bounded MIDI payload suitable for deterministic observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MidiInput {
    channel: u8,
    kind: MidiKind,
    data1: u8,
    data2: u8,
}

impl MidiInput {
    pub const fn channel(&self) -> u8 {
        self.channel
    }

    pub const fn kind(&self) -> MidiKind {
        self.kind
    }

    pub const fn data1(&self) -> u8 {
        self.data1
    }

    pub const fn data2(&self) -> u8 {
        self.data2
    }
}

impl From<MidiMessage> for MidiInput {
    fn from(message: MidiMessage) -> Self {
        Self {
            channel: message.channel().value(),
            kind: message.kind().into(),
            data1: message.data1(),
            data2: message.data2(),
        }
    }
}

/// One patch payload recorded from the startup installation event.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchInput {
    id: u32,
    name: String,
    channel: u8,
    instrument: InstrumentConfig,
    post_effects: Vec<PostEffectConfig>,
    envelope: VoiceEnvelope,
    output: crate::mixer::patch_output::PatchOutput,
}

impl PatchInput {
    pub const fn id(&self) -> u32 {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn channel(&self) -> u8 {
        self.channel
    }

    pub const fn instrument_config(&self) -> &InstrumentConfig {
        &self.instrument
    }

    /// Returns the recorded occupied effect configurations.
    ///
    /// The record's `postEffects` payload is frozen serialized vocabulary — a
    /// dense list of the occupied configurations in position order, each
    /// carrying its stable slot identity — and this accessor keeps that
    /// vocabulary name. It is an output shape, never an addressable chain:
    /// positions are recovered from slot identities, not from list indices.
    pub fn post_effects(&self) -> &[PostEffectConfig] {
        &self.post_effects
    }

    pub const fn envelope(&self) -> &VoiceEnvelope {
        &self.envelope
    }

    pub const fn output(&self) -> crate::mixer::patch_output::PatchOutput {
        self.output
    }
}

impl From<&Patch> for PatchInput {
    fn from(patch: &Patch) -> Self {
        Self {
            id: patch.id().value(),
            name: patch.name().to_owned(),
            channel: patch.channel().value(),
            instrument: patch.instrument_config().clone(),
            // The frozen `postEffects` payload shape: occupied positions of
            // the per-position chain in position order, identified by their
            // stable slot ids. Derived once for output; never indexed back
            // into by dense position.
            post_effects: patch.effect_slots().iter().flatten().cloned().collect(),
            envelope: *patch.envelope(),
            output: patch.output(),
        }
    }
}

/// Bounded, replayable worker visualization payload recorded for an accepted
/// prepared result. It copies only the at-most-2,048 min/max pairs and exact
/// landmarks; decoded PCM and prepared graph ownership never enter the log.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedSampleVisualizationInput {
    asset_id: crate::synth::AssetFileId,
    sample_rate: u32,
    channels: u16,
    frames: usize,
    waveform: Vec<crate::synth::WaveformPair>,
    playback_start: usize,
    playback_end: usize,
    loop_start: usize,
    loop_end: usize,
    crossfade_frames: usize,
    loop_mode: crate::synth::SampleLoopMode,
}

impl PreparedSampleVisualizationInput {
    pub const fn asset_id(&self) -> &crate::synth::AssetFileId {
        &self.asset_id
    }

    pub fn waveform(&self) -> &[crate::synth::WaveformPair] {
        &self.waveform
    }
}

impl From<&crate::synth::PreparedSampleVisualization> for PreparedSampleVisualizationInput {
    fn from(value: &crate::synth::PreparedSampleVisualization) -> Self {
        let landmarks = value.landmarks();
        Self {
            asset_id: value.asset_id().clone(),
            sample_rate: value.sample_rate(),
            channels: value.channels(),
            frames: value.frames(),
            waveform: value.waveform().to_vec(),
            playback_start: landmarks.start,
            playback_end: landmarks.end,
            loop_start: landmarks.loop_start,
            loop_end: landmarks.loop_end,
            crossfade_frames: landmarks.crossfade_frames,
            loop_mode: landmarks.loop_mode,
        }
    }
}

/// A stable tagged representation of every current AppEvent variant.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EventInput {
    Send {
        action: crate::control::SendAction,
    },
    Controller {
        event: crate::control::ControllerEvent,
    },
    SelectContext {
        context: TopLevelContext,
    },
    SelectPatch {
        direction: EventDirection,
    },
    NavigatePage {
        direction: EventDirection,
    },
    Navigate {
        direction: EventDirection,
    },
    Adjust {
        direction: EventDirection,
    },
    SetInteractionMode {
        mode: InteractionMode,
    },
    OpenRelated,
    OpenMidiSettings,
    Activate,
    AssetImported {
        selection: crate::control::AssetImportResult,
    },
    PreviewStart,
    ToggleTestMidi,
    PreviewStop,
    EnterSurface {
        surface: SurfaceId,
    },
    Return,
    InstallPatches {
        patches: Vec<PatchInput>,
    },
    ReplacePersistedSession {
        #[serde(rename = "targetGraphRevision")]
        target_graph_revision: GraphRevision,
        #[serde(rename = "patchIds")]
        patch_ids: Vec<u32>,
    },
    Midi {
        #[serde(rename = "patchId")]
        patch_id: u32,
        message: MidiInput,
    },
    SetPatchOverviewOriginEnabled {
        #[serde(rename = "patchId")]
        patch_id: u32,
        control: crate::control::PatchControlId,
        enabled: bool,
    },
    EngineSelectionLifecycleAdvanced {
        #[serde(rename = "requestId")]
        request_id: EngineSelectionRequestId,
        lifecycle: crate::control::EngineSelectionStatusKind,
    },
    EnginePrepared {
        #[serde(rename = "requestId")]
        request_id: EngineSelectionRequestId,
        #[serde(rename = "patchId")]
        patch_id: u32,
        intent: StructuralEditIntent,
        #[serde(rename = "sourceCapabilityId")]
        source_capability_id: crate::synth::CapabilityId,
        #[serde(rename = "targetCapabilityId")]
        target_capability_id: crate::synth::CapabilityId,
        #[serde(rename = "sourceGraphRevision")]
        source_graph_revision: GraphRevision,
        #[serde(rename = "targetGraphRevision")]
        target_graph_revision: GraphRevision,
        #[serde(rename = "candidateConfig")]
        candidate_config: InstrumentConfig,
        #[serde(rename = "preparedVisualization")]
        prepared_visualization: Option<Vec<PreparedSampleVisualizationInput>>,
    },
    SampleAssetLifecycleAdvanced {
        #[serde(rename = "requestId")]
        request_id: EngineSelectionRequestId,
        lifecycle: crate::control::SampleAssetLifecycle,
    },
    FileCatalogRefreshed {
        #[serde(rename = "assetKind")]
        asset_kind: crate::synth::AssetKind,
        folder: crate::synth::FileBrowserFolderId,
        listing: Option<crate::synth::FileBrowserListing>,
        failure: Option<crate::synth::SampleAssetError>,
    },
    EnginePreparationFailed {
        #[serde(rename = "requestId")]
        request_id: EngineSelectionRequestId,
        intent: StructuralEditIntent,
        #[serde(rename = "patchId")]
        patch_id: u32,
        #[serde(rename = "sourceCapabilityId")]
        source_capability_id: crate::synth::CapabilityId,
        #[serde(rename = "targetCapabilityId")]
        target_capability_id: crate::synth::CapabilityId,
        #[serde(rename = "sourceGraphRevision")]
        source_graph_revision: GraphRevision,
        #[serde(rename = "targetGraphRevision")]
        target_graph_revision: GraphRevision,
        failure: EngineSelectionFailure,
    },
    EngineActivationAcknowledged {
        #[serde(rename = "requestId")]
        request_id: EngineSelectionRequestId,
        intent: StructuralEditIntent,
        #[serde(rename = "targetGraphRevision")]
        target_graph_revision: GraphRevision,
        #[serde(rename = "retiredGraphRevision")]
        retired_graph_revision: GraphRevision,
        collected: bool,
    },
    SetSlotOccupancy {
        #[serde(rename = "patchId")]
        patch_id: u32,
        slot: crate::synth::effect_slot_id::EffectSlotIndex,
        entry: Option<crate::synth::EffectCapabilityId>,
    },
    SetReturnOccupancy {
        bus: crate::mixer::bus_id::BusId,
        entry: Option<crate::synth::EffectCapabilityId>,
    },
    TopologyPrepared {
        #[serde(rename = "requestId")]
        request_id: EngineSelectionRequestId,
        intent: StructuralEditIntent,
        #[serde(rename = "sourceGraphRevision")]
        source_graph_revision: GraphRevision,
        #[serde(rename = "targetGraphRevision")]
        target_graph_revision: GraphRevision,
        prepared_visualization: Option<Vec<PreparedSampleVisualizationInput>>,
    },
    TopologyPreparationFailed {
        #[serde(rename = "requestId")]
        request_id: EngineSelectionRequestId,
        intent: StructuralEditIntent,
        #[serde(rename = "sourceGraphRevision")]
        source_graph_revision: GraphRevision,
        #[serde(rename = "targetGraphRevision")]
        target_graph_revision: GraphRevision,
        failure: EngineSelectionFailure,
    },
    MidiInputPreferenceRestored {
        preference: Option<MidiInputPreference>,
        failure: Option<MidiDeviceFailure>,
    },
    MidiInputPreferenceStoreFailed {
        failure: MidiDeviceFailure,
    },
    MidiInputScanStarted,
    MidiInputScanSucceeded {
        #[serde(rename = "scanId")]
        scan_id: MidiInputScanId,
        descriptors: Vec<MidiInputDescriptor>,
    },
    MidiInputScanFailed {
        #[serde(rename = "scanId")]
        scan_id: MidiInputScanId,
        failure: MidiDeviceFailure,
    },
    MidiInputConnectRequested {
        identity: MidiInputDeviceId,
    },
    MidiInputConnectionPrepared {
        #[serde(rename = "requestId")]
        request_id: MidiConnectionRequestId,
        revision: MidiConnectionRevision,
    },
    MidiInputActivationAcknowledged {
        #[serde(rename = "requestId")]
        request_id: MidiConnectionRequestId,
        revision: MidiConnectionRevision,
    },
    MidiInputDisconnectRequested {
        identity: MidiInputDeviceId,
    },
    MidiInputConnectionLost {
        identity: MidiInputDeviceId,
        revision: MidiConnectionRevision,
    },
    MidiInputOperationFailed {
        identity: MidiInputDeviceId,
        #[serde(rename = "requestId")]
        request_id: Option<MidiConnectionRequestId>,
        revision: Option<MidiConnectionRevision>,
        failure: MidiDeviceFailure,
    },
    MidiInputShutdownRequested,
}

impl From<&AppEvent> for EventInput {
    fn from(event: &AppEvent) -> Self {
        match event {
            AppEvent::SelectContext(context) => Self::SelectContext { context: *context },
            AppEvent::SelectPatch(direction) => Self::SelectPatch {
                direction: (*direction).into(),
            },
            AppEvent::NavigatePage(direction) => Self::NavigatePage {
                direction: (*direction).into(),
            },
            AppEvent::Navigate(direction) => Self::Navigate {
                direction: (*direction).into(),
            },
            AppEvent::Adjust(direction) => Self::Adjust {
                direction: (*direction).into(),
            },
            AppEvent::SetInteractionMode(mode) => Self::SetInteractionMode { mode: *mode },
            AppEvent::OpenRelated => Self::OpenRelated,
            AppEvent::Controller(event) => Self::Controller {
                event: event.clone(),
            },
            AppEvent::OpenMidiSettings => Self::OpenMidiSettings,
            AppEvent::Activate => Self::Activate,
            AppEvent::AssetImported(selection) => Self::AssetImported {
                selection: selection.clone(),
            },
            AppEvent::PreviewStart => Self::PreviewStart,
            AppEvent::ToggleTestMidi => Self::ToggleTestMidi,
            AppEvent::PreviewStop => Self::PreviewStop,
            AppEvent::EnterSurface(surface) => Self::EnterSurface { surface: *surface },
            AppEvent::Return => Self::Return,
            AppEvent::InstallPatches(patches) => Self::InstallPatches {
                patches: patches.iter().map(PatchInput::from).collect(),
            },
            AppEvent::ReplacePersistedSession(replacement) => Self::ReplacePersistedSession {
                target_graph_revision: replacement.target_graph_revision(),
                patch_ids: replacement
                    .patch_ids()
                    .map(|patch_id| patch_id.value())
                    .collect(),
            },
            AppEvent::Midi { patch_id, message } => Self::Midi {
                patch_id: patch_id.value(),
                message: (*message).into(),
            },
            AppEvent::SetPatchOverviewOriginEnabled {
                patch_id,
                control,
                enabled,
            } => Self::SetPatchOverviewOriginEnabled {
                patch_id: patch_id.value(),
                control: control.clone(),
                enabled: *enabled,
            },
            AppEvent::EngineSelectionLifecycleAdvanced {
                request_id,
                lifecycle,
            } => Self::EngineSelectionLifecycleAdvanced {
                request_id: *request_id,
                lifecycle: *lifecycle,
            },
            AppEvent::EnginePrepared {
                request_id,
                patch_id,
                intent,
                source_capability_id,
                target_capability_id,
                source_graph_revision,
                target_graph_revision,
                candidate_config,
                prepared_visualization,
            } => Self::EnginePrepared {
                request_id: *request_id,
                patch_id: patch_id.value(),
                intent: intent.clone(),
                source_capability_id: source_capability_id.clone(),
                target_capability_id: target_capability_id.clone(),
                source_graph_revision: *source_graph_revision,
                target_graph_revision: *target_graph_revision,
                candidate_config: candidate_config.clone(),
                prepared_visualization: prepared_visualization.as_ref().map(|values| {
                    values
                        .iter()
                        .map(PreparedSampleVisualizationInput::from)
                        .collect()
                }),
            },
            AppEvent::SampleAssetLifecycleAdvanced {
                request_id,
                lifecycle,
            } => Self::SampleAssetLifecycleAdvanced {
                request_id: *request_id,
                lifecycle: *lifecycle,
            },
            AppEvent::FileCatalogRefreshed {
                asset_kind,
                folder,
                listing,
            } => {
                let (listing, failure) = match listing {
                    Ok(listing) => (Some(listing.clone()), None),
                    Err(failure) => (None, Some(*failure)),
                };
                Self::FileCatalogRefreshed {
                    asset_kind: *asset_kind,
                    folder: folder.clone(),
                    listing,
                    failure,
                }
            }
            AppEvent::EnginePreparationFailed {
                request_id,
                patch_id,
                intent,
                source_capability_id,
                target_capability_id,
                source_graph_revision,
                target_graph_revision,
                failure,
            } => Self::EnginePreparationFailed {
                request_id: *request_id,
                patch_id: patch_id.value(),
                intent: intent.clone(),
                source_capability_id: source_capability_id.clone(),
                target_capability_id: target_capability_id.clone(),
                source_graph_revision: *source_graph_revision,
                target_graph_revision: *target_graph_revision,
                failure: *failure,
            },
            AppEvent::EngineActivationAcknowledged {
                request_id,
                intent,
                target_graph_revision,
                retired_graph_revision,
                collected,
            } => Self::EngineActivationAcknowledged {
                request_id: *request_id,
                intent: intent.clone(),
                target_graph_revision: *target_graph_revision,
                retired_graph_revision: *retired_graph_revision,
                collected: *collected,
            },
            AppEvent::SetSlotOccupancy {
                patch_id,
                slot,
                entry,
            } => Self::SetSlotOccupancy {
                patch_id: patch_id.value(),
                slot: *slot,
                entry: entry.clone(),
            },
            AppEvent::Send(action) => Self::Send {
                action: action.clone(),
            },
            AppEvent::SetReturnOccupancy { bus, entry } => Self::SetReturnOccupancy {
                bus: *bus,
                entry: entry.clone(),
            },
            AppEvent::TopologyPrepared {
                request_id,
                intent,
                source_graph_revision,
                target_graph_revision,
                prepared_visualization,
            } => Self::TopologyPrepared {
                request_id: *request_id,
                intent: intent.clone(),
                source_graph_revision: *source_graph_revision,
                target_graph_revision: *target_graph_revision,
                prepared_visualization: prepared_visualization.as_ref().map(|values| {
                    values
                        .iter()
                        .map(PreparedSampleVisualizationInput::from)
                        .collect()
                }),
            },
            AppEvent::TopologyPreparationFailed {
                request_id,
                intent,
                source_graph_revision,
                target_graph_revision,
                failure,
            } => Self::TopologyPreparationFailed {
                request_id: *request_id,
                intent: intent.clone(),
                source_graph_revision: *source_graph_revision,
                target_graph_revision: *target_graph_revision,
                failure: *failure,
            },
            AppEvent::MidiInputPreferenceRestored {
                preference,
                failure,
            } => Self::MidiInputPreferenceRestored {
                preference: preference.clone(),
                failure: failure.clone(),
            },
            AppEvent::MidiInputPreferenceStoreFailed { failure } => {
                Self::MidiInputPreferenceStoreFailed {
                    failure: failure.clone(),
                }
            }
            AppEvent::MidiInputScanStarted => Self::MidiInputScanStarted,
            AppEvent::MidiInputScanSucceeded {
                scan_id,
                descriptors,
            } => Self::MidiInputScanSucceeded {
                scan_id: *scan_id,
                descriptors: descriptors.clone(),
            },
            AppEvent::MidiInputScanFailed { scan_id, failure } => Self::MidiInputScanFailed {
                scan_id: *scan_id,
                failure: failure.clone(),
            },
            AppEvent::MidiInputConnectRequested { identity } => Self::MidiInputConnectRequested {
                identity: identity.clone(),
            },
            AppEvent::MidiInputConnectionPrepared {
                request_id,
                revision,
            } => Self::MidiInputConnectionPrepared {
                request_id: *request_id,
                revision: *revision,
            },
            AppEvent::MidiInputActivationAcknowledged {
                request_id,
                revision,
            } => Self::MidiInputActivationAcknowledged {
                request_id: *request_id,
                revision: *revision,
            },
            AppEvent::MidiInputDisconnectRequested { identity } => {
                Self::MidiInputDisconnectRequested {
                    identity: identity.clone(),
                }
            }
            AppEvent::MidiInputConnectionLost { identity, revision } => {
                Self::MidiInputConnectionLost {
                    identity: identity.clone(),
                    revision: *revision,
                }
            }
            AppEvent::MidiInputOperationFailed {
                identity,
                request_id,
                revision,
                failure,
            } => Self::MidiInputOperationFailed {
                identity: identity.clone(),
                request_id: *request_id,
                revision: *revision,
                failure: failure.clone(),
            },
            AppEvent::MidiInputShutdownRequested => Self::MidiInputShutdownRequested,
        }
    }
}

impl EventInput {
    /// Mirrors the production event publication contract in serialized
    /// behavioral evidence.
    pub const fn publishes_parameters_on_acceptance(&self) -> bool {
        !matches!(
            self,
            Self::Controller { .. }
                | Self::SelectContext { .. }
                | Self::SelectPatch { .. }
                | Self::NavigatePage { .. }
                | Self::Navigate { .. }
                | Self::SetInteractionMode { .. }
                | Self::OpenRelated
                | Self::OpenMidiSettings
                | Self::ToggleTestMidi
                | Self::PreviewStart
                | Self::PreviewStop
                | Self::SetPatchOverviewOriginEnabled { .. }
                | Self::EngineSelectionLifecycleAdvanced { .. }
                | Self::SampleAssetLifecycleAdvanced { .. }
                | Self::FileCatalogRefreshed { .. }
                | Self::EnterSurface { .. }
                | Self::Return
                | Self::MidiInputPreferenceRestored { .. }
                | Self::MidiInputPreferenceStoreFailed { .. }
                | Self::MidiInputScanStarted
                | Self::MidiInputScanSucceeded { .. }
                | Self::MidiInputScanFailed { .. }
                | Self::MidiInputConnectRequested { .. }
                | Self::MidiInputConnectionPrepared { .. }
                | Self::MidiInputActivationAcknowledged { .. }
                | Self::MidiInputDisconnectRequested { .. }
                | Self::MidiInputConnectionLost { .. }
                | Self::MidiInputOperationFailed { .. }
                | Self::MidiInputShutdownRequested
        )
    }
}

/// A stable, allocation-free description of one command sent toward audio.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AudioEffect {
    ReleaseImmediateNotes,
    InitializedVoice {
        source: crate::synth::MidiSource,
        patch_id: u32,
        voice: u64,
        initial: crate::synth::OwnedVoiceInitialization,
    },
    VoiceMidi {
        source: crate::synth::MidiSource,
        patch_id: u32,
        voice: u64,
        scope: crate::synth::MidiExpressionScope,
        message: MidiInput,
    },
    OwnedMidi {
        source: crate::synth::MidiSource,
        patch_id: u32,
        message: MidiInput,
    },
    ReleaseSource {
        source: crate::synth::MidiSource,
        patch_id: u32,
    },
    PatchMidi {
        #[serde(rename = "patchId")]
        patch_id: u32,
        message: MidiInput,
    },
    PreviewStart {
        #[serde(rename = "patchId")]
        patch_id: u32,
        #[serde(rename = "auditionId")]
        audition_id: u64,
    },
    PreviewStop {
        #[serde(rename = "patchId")]
        patch_id: u32,
        #[serde(rename = "auditionId")]
        audition_id: u64,
    },
    AllNotesOff,
}

impl From<AudioCommand> for AudioEffect {
    fn from(command: AudioCommand) -> Self {
        match command {
            AudioCommand::ReleaseImmediateNotes => Self::ReleaseImmediateNotes,
            AudioCommand::InitializedVoice {
                source,
                patch_id,
                voice,
                initial,
            } => Self::InitializedVoice {
                source,
                patch_id: patch_id.value(),
                voice: voice.get(),
                initial,
            },
            AudioCommand::VoiceMidi {
                source,
                patch_id,
                voice,
                scope,
                message,
            } => Self::VoiceMidi {
                source,
                patch_id: patch_id.value(),
                voice: voice.get(),
                scope,
                message: message.into(),
            },
            AudioCommand::OwnedMidi {
                source,
                patch_id,
                message,
            } => Self::OwnedMidi {
                source,
                patch_id: patch_id.value(),
                message: message.into(),
            },
            AudioCommand::ReleaseSource { source, patch_id } => Self::ReleaseSource {
                source,
                patch_id: patch_id.value(),
            },
            AudioCommand::PatchMidi { patch_id, message } => Self::PatchMidi {
                patch_id: patch_id.value(),
                message: message.into(),
            },
            AudioCommand::PreviewStart {
                patch_id,
                audition_id,
            } => Self::PreviewStart {
                patch_id: patch_id.value(),
                audition_id,
            },
            AudioCommand::PreviewStop {
                patch_id,
                audition_id,
            } => Self::PreviewStop {
                patch_id: patch_id.value(),
                audition_id,
            },
            AudioCommand::AllNotesOff => Self::AllNotesOff,
        }
    }
}

/// One deterministic effect derived after an input has been accepted.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EmittedEvent {
    StateAccepted {
        generation: u64,
    },
    ParameterSnapshotPublished {
        generation: u64,
        #[serde(rename = "graphRevision")]
        graph_revision: GraphRevision,
    },
    AudioCommand {
        effect: AudioEffect,
    },
    EngineSelection {
        effect: EngineSelectionEffect,
    },
}

/// A coherence failure detected while assembling an EventRecord.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventRecordError {
    GenerationOverflow,
    GenerationDidNotAdvance { expected: u64, actual: u64 },
    StateHashDidNotChange,
    ParameterGenerationMismatch { expected: u64, actual: u64 },
    ProjectionStateHashMismatch { expected: String, actual: String },
}

impl fmt::Display for EventRecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GenerationOverflow => {
                formatter.write_str("event record generation cannot advance beyond u64::MAX")
            }
            Self::GenerationDidNotAdvance { expected, actual } => write!(
                formatter,
                "accepted generation must advance exactly once: expected {expected}, got {actual}"
            ),
            Self::StateHashDidNotChange => {
                formatter.write_str("accepted event must produce a new state hash")
            }
            Self::ParameterGenerationMismatch { expected, actual } => write!(
                formatter,
                "parameter generation must equal the recorded state generation: expected {expected}, got {actual}"
            ),
            Self::ProjectionStateHashMismatch { expected, actual } => write!(
                formatter,
                "projection state hash must match the recorded state hash: expected {expected}, got {actual}"
            ),
        }
    }
}

impl std::error::Error for EventRecordError {}

/// One deterministic control-side record of an input and its complete outcome.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRecord {
    sequence: u64,
    source: EventSource,
    input: EventInput,
    outcome: EventOutcome,
    generation_before: u64,
    generation_after: u64,
    state_hash_before: String,
    state_hash_after: String,
    parameter_generation: u64,
    selected_line: usize,
    projection_state_hash: String,
    emitted_events: Vec<EmittedEvent>,
    #[serde(serialize_with = "serialize_rejection")]
    rejection: Option<EventRejection>,
}

impl EventRecord {
    pub const SERIALIZED_PROPERTY_DESCRIPTOR: &'static [&'static str] = &[
        "sequence",
        "source",
        "input",
        "outcome",
        "rejection",
        "generationBefore",
        "generationAfter",
        "stateHashBefore",
        "stateHashAfter",
        "emittedEvents",
        "parameterGeneration",
        "selectedLine",
        "projectionStateHash",
    ];

    /// Returns the stable top-level properties of every serialized event record.
    pub const fn serialized_property_descriptor() -> &'static [&'static str] {
        Self::SERIALIZED_PROPERTY_DESCRIPTOR
    }

    /// Every normalized leaf path emitted by the EventRecord serializer.
    ///
    /// Object components are separated by `.`, and `[]` denotes any element
    /// of a serialized array. The descriptor is sorted, contains no duplicates,
    /// and must be compared with the union of discriminating accepted and
    /// rejected records rather than with one convenient record.
    pub const SERIALIZED_LEAF_DESCRIPTOR: &[&str] = &[
        "emittedEvents[].effect.intent.capabilityId",
        "emittedEvents[].effect.intent.choiceId",
        "emittedEvents[].effect.intent.kind",
        "emittedEvents[].effect.intent.parameterId",
        "emittedEvents[].effect.intent.targetCapabilityId",
        "emittedEvents[].effect.kind",
        "emittedEvents[].effect.message.channel",
        "emittedEvents[].effect.message.data1",
        "emittedEvents[].effect.message.data2",
        "emittedEvents[].effect.message.kind",
        "emittedEvents[].effect.patchId",
        "emittedEvents[].effect.requestId",
        "emittedEvents[].effect.sourceCapabilityId",
        "emittedEvents[].effect.sourceGraphRevision",
        "emittedEvents[].effect.targetCapabilityId",
        "emittedEvents[].effect.targetGraphRevision",
        "emittedEvents[].generation",
        "emittedEvents[].graphRevision",
        "emittedEvents[].kind",
        "generationAfter",
        "generationBefore",
        "input.action.bus",
        "input.action.entry",
        "input.action.kind",
        "input.action.name",
        "input.action.slotId",
        "input.assetKind",
        "input.candidateConfig.assetReferences[].parameterId",
        "input.candidateConfig.assetReferences[].reference.kind",
        "input.candidateConfig.assetReferences[].reference.locator",
        "input.candidateConfig.capabilityId",
        "input.candidateConfig.values[].parameterId",
        "input.candidateConfig.values[].value.kind",
        "input.candidateConfig.values[].value.value",
        "input.collected",
        "input.context",
        "input.direction",
        "input.failure",
        "input.folder",
        "input.intent.capabilityId",
        "input.intent.choiceId",
        "input.intent.kind",
        "input.intent.parameterId",
        "input.intent.targetCapabilityId",
        "input.kind",
        "input.lifecycle",
        "input.listing",
        "input.listing.folder",
        "input.listing.rows[].id",
        "input.listing.rows[].kind.kind",
        "input.listing.rows[].kind.target",
        "input.listing.rows[].label",
        "input.listing.rows[].metadata",
        "input.listing.rows[].metadata.Err",
        "input.listing.rows[].metadata.Ok.assetId",
        "input.listing.rows[].metadata.Ok.bitsPerSample",
        "input.listing.rows[].metadata.Ok.channels",
        "input.listing.rows[].metadata.Ok.durationMilliseconds",
        "input.listing.rows[].metadata.Ok.encoding",
        "input.listing.rows[].metadata.Ok.frames",
        "input.listing.rows[].metadata.Ok.sampleRate",
        "input.listing.rows[].metadata.Ok.sourceBytes",
        "input.listing.rows[].sourceBytes",
        "input.message.channel",
        "input.message.data1",
        "input.message.data2",
        "input.message.kind",
        "input.mode",
        "input.patchId",
        "input.patches[].channel",
        "input.patches[].envelope.attackMilliseconds",
        "input.patches[].envelope.decayMilliseconds",
        "input.patches[].envelope.releaseMilliseconds",
        "input.patches[].envelope.sustain",
        "input.patches[].id",
        "input.patches[].instrument.assetReferences[].parameterId",
        "input.patches[].instrument.assetReferences[].reference.kind",
        "input.patches[].instrument.assetReferences[].reference.locator",
        "input.patches[].instrument.capabilityId",
        "input.patches[].instrument.values[].parameterId",
        "input.patches[].instrument.values[].value.kind",
        "input.patches[].instrument.values[].value.value",
        "input.patches[].name",
        "input.patches[].output.trackId",
        "input.patches[].output.trimGainDb",
        "input.patches[].postEffects[].assetReferences[].parameterId",
        "input.patches[].postEffects[].assetReferences[].reference.kind",
        "input.patches[].postEffects[].assetReferences[].reference.locator",
        "input.patches[].postEffects[].capabilityId",
        "input.patches[].postEffects[].slotId",
        "input.patches[].postEffects[].values[].parameterId",
        "input.patches[].postEffects[].values[].value.kind",
        "input.patches[].postEffects[].values[].value.value",
        "input.preparedVisualization",
        "input.preparedVisualization[].assetId",
        "input.preparedVisualization[].channels",
        "input.preparedVisualization[].crossfadeFrames",
        "input.preparedVisualization[].frames",
        "input.preparedVisualization[].loopEnd",
        "input.preparedVisualization[].loopMode",
        "input.preparedVisualization[].loopStart",
        "input.preparedVisualization[].playbackEnd",
        "input.preparedVisualization[].playbackStart",
        "input.preparedVisualization[].sampleRate",
        "input.preparedVisualization[].waveform[].leftMax",
        "input.preparedVisualization[].waveform[].leftMin",
        "input.preparedVisualization[].waveform[].rightMax",
        "input.preparedVisualization[].waveform[].rightMin",
        "input.requestId",
        "input.retiredGraphRevision",
        "input.selection.descriptor",
        "input.selection.descriptor.assetRequirements[].parameterId",
        "input.selection.descriptor.assetRequirements[].required",
        "input.selection.descriptor.assetScopedChoices",
        "input.selection.descriptor.availability.kind",
        "input.selection.descriptor.id",
        "input.selection.descriptor.instrumentCategory",
        "input.selection.descriptor.label",
        "input.selection.descriptor.sections[].id",
        "input.selection.descriptor.sections[].label",
        "input.selection.descriptor.sections[].leadingControls",
        "input.selection.descriptor.sections[].parameters[].choices[].id",
        "input.selection.descriptor.sections[].parameters[].choices[].label",
        "input.selection.descriptor.sections[].parameters[].coarseStep",
        "input.selection.descriptor.sections[].parameters[].continuousLabels[].label",
        "input.selection.descriptor.sections[].parameters[].continuousLabels[].range.maximum",
        "input.selection.descriptor.sections[].parameters[].continuousLabels[].range.minimum",
        "input.selection.descriptor.sections[].parameters[].continuousLabels[].valueOffset",
        "input.selection.descriptor.sections[].parameters[].continuousLabels[].valueScale",
        "input.selection.descriptor.sections[].parameters[].continuousLabels[].valueUnit",
        "input.selection.descriptor.sections[].parameters[].defaultValue.kind",
        "input.selection.descriptor.sections[].parameters[].defaultValue.value.kind",
        "input.selection.descriptor.sections[].parameters[].defaultValue.value.locator",
        "input.selection.descriptor.sections[].parameters[].defaultValue.value.value",
        "input.selection.descriptor.sections[].parameters[].enabledWhen",
        "input.selection.descriptor.sections[].parameters[].fineStep",
        "input.selection.descriptor.sections[].parameters[].formatter",
        "input.selection.descriptor.sections[].parameters[].id",
        "input.selection.descriptor.sections[].parameters[].kind",
        "input.selection.descriptor.sections[].parameters[].label",
        "input.selection.descriptor.sections[].parameters[].patchInteraction",
        "input.selection.descriptor.sections[].parameters[].range",
        "input.selection.descriptor.sections[].parameters[].range.maximum",
        "input.selection.descriptor.sections[].parameters[].range.minimum",
        "input.selection.descriptor.sections[].parameters[].steppedLabels[]",
        "input.selection.descriptor.sections[].parameters[].unit",
        "input.selection.descriptor.sections[].parameters[].update",
        "input.selection.descriptor.sections[].parameters[].visibleWhen",
        "input.selection.descriptor.semanticAccent",
        "input.selection.descriptor.supportedMidiKinds[]",
        "input.selection.descriptor.visualizations[].id",
        "input.selection.descriptor.visualizations[].kind",
        "input.selection.descriptor.visualizations[].label",
        "input.selection.descriptor.voicePolicy.defaultVoices",
        "input.selection.descriptor.voicePolicy.kind",
        "input.selection.request.assetId",
        "input.selection.request.assetKind",
        "input.selection.request.generation",
        "input.selection.request.graphRevision",
        "input.selection.request.origin.capabilityId.id",
        "input.selection.request.origin.capabilityId.kind",
        "input.selection.request.origin.context",
        "input.selection.request.origin.controlId.id",
        "input.selection.request.origin.controlId.kind",
        "input.selection.request.origin.modalId",
        "input.selection.request.origin.patchId",
        "input.selection.request.origin.surface",
        "input.selection.result.Err",
        "input.selection.result.Ok",
        "input.sourceCapabilityId",
        "input.sourceGraphRevision",
        "input.surface",
        "input.targetCapabilityId",
        "input.targetGraphRevision",
        "outcome",
        "parameterGeneration",
        "projectionStateHash",
        "rejection",
        "selectedLine",
        "sequence",
        "source",
        "stateHashAfter",
        "stateHashBefore",
    ];

    /// Returns the production-owned serialized leaf descriptor.
    #[must_use]
    pub const fn serialized_leaf_descriptor() -> &'static [&'static str] {
        Self::SERIALIZED_LEAF_DESCRIPTOR
    }

    /// Records an accepted transition after all coherent projections exist.
    #[allow(clippy::too_many_arguments)]
    pub fn accepted(
        sequence: u64,
        source: EventSource,
        input: &AppEvent,
        generation_before: u64,
        state_hash_before: impl Into<String>,
        accepted: StateAccepted,
        snapshot: &StateSnapshot,
        parameter_generation: u64,
        parameter_graph_revision: GraphRevision,
        parameters_published: bool,
        projection: &TextProjection,
        audio_command: Option<AudioCommand>,
        engine_selection_effect: Option<EngineSelectionEffect>,
    ) -> Result<Self, EventRecordError> {
        let generation_after = generation_before
            .checked_add(1)
            .ok_or(EventRecordError::GenerationOverflow)?;
        if accepted.generation() != generation_after {
            return Err(EventRecordError::GenerationDidNotAdvance {
                expected: generation_after,
                actual: accepted.generation(),
            });
        }

        let state_hash_before = state_hash_before.into();
        if state_hash_before == snapshot.hash() {
            return Err(EventRecordError::StateHashDidNotChange);
        }
        validate_parameter_generation(generation_after, parameter_generation)?;
        validate_projection_hash(snapshot.hash(), projection.state_hash())?;

        let mut emitted_events = vec![EmittedEvent::StateAccepted {
            generation: accepted.generation(),
        }];
        if parameters_published {
            emitted_events.push(EmittedEvent::ParameterSnapshotPublished {
                generation: parameter_generation,
                graph_revision: parameter_graph_revision,
            });
        }
        if let Some(command) = audio_command {
            emitted_events.push(EmittedEvent::AudioCommand {
                effect: command.into(),
            });
        }
        if let Some(effect) = engine_selection_effect {
            emitted_events.push(EmittedEvent::EngineSelection { effect });
        }

        Ok(Self {
            sequence,
            source,
            input: EventInput::from(input),
            outcome: EventOutcome::Accepted,
            generation_before,
            generation_after,
            state_hash_before,
            state_hash_after: snapshot.hash().to_owned(),
            parameter_generation,
            selected_line: projection.selected_line(),
            projection_state_hash: projection.state_hash().to_owned(),
            emitted_events,
            rejection: None,
        })
    }

    /// Records a rejected input without deriving or publishing new effects.
    #[allow(clippy::too_many_arguments)]
    pub fn rejected(
        sequence: u64,
        source: EventSource,
        input: &AppEvent,
        generation: u64,
        state_hash: impl Into<String>,
        parameter_generation: u64,
        projection: &TextProjection,
        rejection: EventRejection,
    ) -> Result<Self, EventRecordError> {
        let state_hash = state_hash.into();
        validate_parameter_generation(generation, parameter_generation)?;
        validate_projection_hash(&state_hash, projection.state_hash())?;

        Ok(Self {
            sequence,
            source,
            input: EventInput::from(input),
            outcome: EventOutcome::Rejected,
            generation_before: generation,
            generation_after: generation,
            state_hash_before: state_hash.clone(),
            state_hash_after: state_hash,
            parameter_generation,
            selected_line: projection.selected_line(),
            projection_state_hash: projection.state_hash().to_owned(),
            emitted_events: Vec::new(),
            rejection: Some(rejection),
        })
    }

    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    pub const fn source(&self) -> EventSource {
        self.source
    }

    pub const fn input(&self) -> &EventInput {
        &self.input
    }

    pub const fn outcome(&self) -> EventOutcome {
        self.outcome
    }

    pub const fn generation_before(&self) -> u64 {
        self.generation_before
    }

    pub const fn generation_after(&self) -> u64 {
        self.generation_after
    }

    pub fn state_hash_before(&self) -> &str {
        &self.state_hash_before
    }

    pub fn state_hash_after(&self) -> &str {
        &self.state_hash_after
    }

    pub const fn parameter_generation(&self) -> u64 {
        self.parameter_generation
    }

    pub const fn selected_line(&self) -> usize {
        self.selected_line
    }

    pub fn projection_state_hash(&self) -> &str {
        &self.projection_state_hash
    }

    pub fn emitted_events(&self) -> &[EmittedEvent] {
        &self.emitted_events
    }

    pub(crate) fn append_engine_selection_effect(&mut self, effect: EngineSelectionEffect) -> bool {
        if self.outcome != EventOutcome::Accepted
            || self.emitted_events.iter().any(|emitted| {
                matches!(
                    emitted,
                    EmittedEvent::EngineSelection { effect: existing }
                        if existing.kind() == effect.kind()
                            && existing.request_id() == effect.request_id()
                )
            })
        {
            return false;
        }
        self.emitted_events
            .push(EmittedEvent::EngineSelection { effect });
        true
    }

    pub const fn rejection(&self) -> Option<EventRejection> {
        self.rejection
    }

    /// Serializes the complete record with stable camelCase field names.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

fn validate_parameter_generation(expected: u64, actual: u64) -> Result<(), EventRecordError> {
    if actual == expected {
        Ok(())
    } else {
        Err(EventRecordError::ParameterGenerationMismatch { expected, actual })
    }
}

fn validate_projection_hash(expected: &str, actual: &str) -> Result<(), EventRecordError> {
    if actual == expected {
        Ok(())
    } else {
        Err(EventRecordError::ProjectionStateHashMismatch {
            expected: expected.to_owned(),
            actual: actual.to_owned(),
        })
    }
}

fn serialize_rejection<S>(
    rejection: &Option<EventRejection>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    rejection.map(rejection_name).serialize(serializer)
}

const fn rejection_name(rejection: EventRejection) -> &'static str {
    match rejection {
        EventRejection::InstallationClosed => "installationClosed",
        EventRejection::TooManyPatches => "tooManyPatches",
        EventRejection::InvalidInstrumentConfig => "invalidInstrumentConfig",
        EventRejection::InvalidEffectConfig => "invalidEffectConfig",
        EventRejection::NoPatchesInstalled => "noPatchesInstalled",
        EventRejection::UnknownPatch => "unknownPatch",
        EventRejection::InvalidSelection => "invalidSelection",
        EventRejection::ParameterAtBoundary => "parameterAtBoundary",
        EventRejection::InvalidParameterValue => "invalidParameterValue",
        EventRejection::ActionUnavailableInContext => "actionUnavailableInContext",
        EventRejection::EngineSelectionUnavailable => "engineSelectionUnavailable",
        EventRejection::StructuralEditBusy => "structuralEditBusy",
        EventRejection::StaleEngineSelection => "staleEngineSelection",
        EventRejection::MismatchedEngineSelection => "mismatchedEngineSelection",
        EventRejection::RequestIdOverflow => "requestIdOverflow",
        EventRejection::GenerationOverflow => "generationOverflow",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AudioEffect, EmittedEvent, EventDirection, EventInput, EventOutcome, EventRecord,
        EventRecordError, EventSource, MidiInput, MidiKind, PatchInput,
        PreparedSampleVisualizationInput,
    };
    use crate::adapter::hidef_soundfont_capability::{
        HIDEF_CAPABILITY_ID, SOUNDFONT_PRESET_PARAMETER_ID,
    };
    use crate::control::app_event::{AppEvent, Direction};
    use crate::control::app_state::{AppState, EventRejection};
    use crate::control::state_snapshot::StateSnapshot;
    use crate::control::text_projection::TextProjection;
    use crate::control::{
        EngineSelectionEffect, EngineSelectionEffectKind, EngineSelectionFailure,
        EngineSelectionRequestId, EngineSelectionStatus, InteractionMode, SendAction,
        StructuralEditIntent, SurfaceId, TopLevelContext,
    };
    use crate::kernel::midi_channel::MidiChannel;
    use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
    use crate::kernel::patch_id::PatchId;
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::real_time::GraphRevision;
    use crate::synth::patch::Patch;
    use crate::synth::sound_font_instrument::SoundFontInstrument;
    use crate::synth::voice_envelope::VoiceEnvelope;
    use crate::synth::InstrumentCapabilityProvider;
    use crate::synth::{
        AssetFileId, FileBrowserFolderId, FileBrowserListing, FileBrowserRow, FileBrowserRowKind,
        ParameterId, ParameterValue, PreparedSampleLandmarks, PreparedSamplePcm,
        PreparedSampleVisualization, SampleAssetError, SampleEncoding, SampleLoopMode,
        SampleMetadata, WaveformPair,
    };
    use crate::testing::automatic_midi_test::create_soundfont_config;
    use serde_json::Value;
    use std::collections::BTreeSet;
    use std::sync::Arc;

    fn patch(id: u32) -> Patch {
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        Patch::new(
            PatchId::new(id).unwrap(),
            format!("Patch {id}"),
            create_soundfont_config(
                &provider,
                SoundFontInstrument::new(128, (id - 1) as u8, false).unwrap(),
            )
            .unwrap(),
            MidiChannel::new((id - 1) as u8).unwrap(),
            crate::mixer::patch_output::PatchOutput::new(
                crate::mixer::mixer_track_id::MixerTrackId::new((id - 1) as u8).unwrap(),
                -6.0,
            )
            .unwrap(),
        )
    }

    fn installed_state() -> AppState {
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        let mut state = AppState::new(
            provider.registry().unwrap(),
            GlobalParameters::new(-3.0).unwrap(),
        );
        state
            .apply(AppEvent::InstallPatches(vec![patch(1)]))
            .unwrap();
        state
    }

    fn snapshot_and_projection(generation: u64) -> (StateSnapshot, TextProjection) {
        let snapshot = StateSnapshot::new(format!("{{\"generation\":{generation}}}"));
        let projection = TextProjection::new(
            "GLOBAL\n> masterGainDb=-3".to_owned(),
            1,
            snapshot.hash().to_owned(),
        );
        (snapshot, projection)
    }

    fn schema_record(
        source: EventSource,
        input: EventInput,
        outcome: EventOutcome,
        emitted_events: Vec<EmittedEvent>,
        rejection: Option<EventRejection>,
    ) -> EventRecord {
        EventRecord {
            sequence: 1,
            source,
            input,
            outcome,
            generation_before: 4,
            generation_after: 5,
            state_hash_before: "before".to_owned(),
            state_hash_after: "after".to_owned(),
            parameter_generation: 5,
            selected_line: 2,
            projection_state_hash: "after".to_owned(),
            emitted_events,
            rejection,
        }
    }

    fn discriminating_schema_records() -> Vec<EventRecord> {
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        let patch = PatchInput {
            id: 7,
            name: "Schema Patch".to_owned(),
            channel: 3,
            instrument: create_soundfont_config(
                &provider,
                SoundFontInstrument::new(128, 11, false).unwrap(),
            )
            .unwrap(),
            post_effects: vec![{
                let chorus = crate::adapter::production_effects::production_chorus_config(
                    crate::synth::EffectSlotId::new(1).unwrap(),
                )
                .unwrap();
                crate::synth::PostEffectConfig::from_parts(
                    chorus.slot_id(),
                    chorus.capability_id().clone(),
                    chorus.values().to_vec(),
                    vec![crate::synth::AssetAssignment::new(
                        crate::synth::ParameterId::new("chorus.schema.asset").unwrap(),
                        crate::synth::AssetReference::new(
                            crate::synth::AssetKind::Other,
                            "schema://chorus",
                        )
                        .unwrap(),
                    )],
                )
            }],
            envelope: VoiceEnvelope::default(),
            output: crate::mixer::patch_output::PatchOutput::new(
                crate::mixer::mixer_track_id::MixerTrackId::new(3).unwrap(),
                -4.0,
            )
            .unwrap(),
        };
        let message = MidiInput {
            channel: 3,
            kind: MidiKind::PitchBend,
            data1: 1,
            data2: 65,
        };
        let request_id = EngineSelectionRequestId::FIRST;
        let source_capability_id = crate::synth::CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap();
        let target_capability_id =
            crate::synth::CapabilityId::new("instrument.schema-target").unwrap();
        let target_graph_revision = GraphRevision::INITIAL.checked_next().unwrap();
        let candidate_config = patch.instrument.clone();
        let effect_status = EngineSelectionStatus::preparing(
            GraphRevision::INITIAL,
            request_id,
            PatchId::new(7).unwrap(),
            source_capability_id.clone(),
            target_capability_id.clone(),
        )
        .unwrap()
        .advance_admission(crate::control::EngineSelectionStatusKind::Validating)
        .unwrap()
        .advance_admission(crate::control::EngineSelectionStatusKind::Preparing)
        .unwrap()
        .activating(target_graph_revision)
        .unwrap();
        let engine_effect = EngineSelectionEffect::from_correlation(
            EngineSelectionEffectKind::CandidatePrepared,
            effect_status.correlation().unwrap(),
        )
        .unwrap();
        let preset_status = EngineSelectionStatus::preparing_with_intent(
            GraphRevision::INITIAL,
            request_id,
            PatchId::new(7).unwrap(),
            StructuralEditIntent::ReplaceParameterChoice {
                capability_id: source_capability_id.clone(),
                parameter_id: ParameterId::new(SOUNDFONT_PRESET_PARAMETER_ID).unwrap(),
                choice_id: "sf2.bank-128.program-0".to_owned(),
            },
            source_capability_id.clone(),
            source_capability_id.clone(),
        )
        .unwrap()
        .advance_admission(crate::control::EngineSelectionStatusKind::Validating)
        .unwrap()
        .advance_admission(crate::control::EngineSelectionStatusKind::Preparing)
        .unwrap()
        .activating(target_graph_revision)
        .unwrap();
        let preset_effect = EngineSelectionEffect::from_correlation(
            EngineSelectionEffectKind::CandidatePrepared,
            preset_status.correlation().unwrap(),
        )
        .unwrap();
        let catalog_folder = FileBrowserFolderId::default();
        let catalog_asset = AssetFileId::new("catalog-ready.wav").unwrap();
        let catalog_listing = FileBrowserListing::new(
            catalog_folder.clone(),
            vec![
                FileBrowserRow::new(
                    "file:catalog-ready.wav",
                    "catalog-ready.wav",
                    FileBrowserRowKind::File(catalog_asset.clone()),
                    Some(256),
                )
                .unwrap()
                .with_metadata(Ok(SampleMetadata::new(
                    catalog_asset,
                    256,
                    48_000,
                    2,
                    24,
                    SampleEncoding::SignedPcm,
                    48_000,
                )
                .unwrap()))
                .unwrap(),
                FileBrowserRow::new(
                    "file:catalog-invalid.wav",
                    "catalog-invalid.wav",
                    FileBrowserRowKind::File(AssetFileId::new("catalog-invalid.wav").unwrap()),
                    Some(12),
                )
                .unwrap()
                .with_metadata(Err(SampleAssetError::MalformedWave))
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

        let mut records = vec![
            schema_record(
                EventSource::Keyboard,
                EventInput::SelectContext {
                    context: TopLevelContext::Patch,
                },
                EventOutcome::Accepted,
                Vec::new(),
                None,
            ),
            schema_record(
                EventSource::Startup,
                EventInput::InstallPatches {
                    patches: vec![patch],
                },
                EventOutcome::Accepted,
                vec![
                    EmittedEvent::StateAccepted { generation: 5 },
                    EmittedEvent::ParameterSnapshotPublished {
                        generation: 5,
                        graph_revision: GraphRevision::INITIAL,
                    },
                ],
                None,
            ),
            schema_record(
                EventSource::Keyboard,
                EventInput::Navigate {
                    direction: EventDirection::Up,
                },
                EventOutcome::Rejected,
                Vec::new(),
                Some(EventRejection::ParameterAtBoundary),
            ),
            schema_record(
                EventSource::AutomaticMidi,
                EventInput::Midi {
                    patch_id: 7,
                    message,
                },
                EventOutcome::Accepted,
                vec![EmittedEvent::AudioCommand {
                    effect: AudioEffect::PatchMidi {
                        patch_id: 7,
                        message,
                    },
                }],
                None,
            ),
            schema_record(
                EventSource::DemoScene,
                EventInput::Adjust {
                    direction: EventDirection::Right,
                },
                EventOutcome::Accepted,
                vec![EmittedEvent::AudioCommand {
                    effect: AudioEffect::AllNotesOff,
                }],
                None,
            ),
            schema_record(
                EventSource::System,
                EventInput::Navigate {
                    direction: EventDirection::Down,
                },
                EventOutcome::Accepted,
                Vec::new(),
                None,
            ),
            schema_record(
                EventSource::Keyboard,
                EventInput::SetInteractionMode {
                    mode: InteractionMode::Adjust,
                },
                EventOutcome::Accepted,
                Vec::new(),
                None,
            ),
            schema_record(
                EventSource::Keyboard,
                EventInput::EnterSurface {
                    surface: SurfaceId::MixerInspector,
                },
                EventOutcome::Accepted,
                Vec::new(),
                None,
            ),
            schema_record(
                EventSource::Keyboard,
                EventInput::Return,
                EventOutcome::Accepted,
                Vec::new(),
                None,
            ),
            schema_record(
                EventSource::Worker,
                EventInput::EnginePrepared {
                    request_id,
                    patch_id: 7,
                    intent: effect_status.correlation().unwrap().intent().clone(),
                    source_capability_id: source_capability_id.clone(),
                    target_capability_id: target_capability_id.clone(),
                    source_graph_revision: GraphRevision::INITIAL,
                    target_graph_revision,
                    candidate_config: candidate_config.clone(),
                    prepared_visualization: Some(vec![PreparedSampleVisualizationInput {
                        asset_id: AssetFileId::new("event-log.wav").unwrap(),
                        sample_rate: 48_000,
                        channels: 2,
                        frames: 128,
                        waveform: vec![WaveformPair {
                            left_min: -0.5,
                            left_max: 0.5,
                            right_min: -0.25,
                            right_max: 0.25,
                        }],
                        playback_start: 0,
                        playback_end: 128,
                        loop_start: 16,
                        loop_end: 112,
                        crossfade_frames: 8,
                        loop_mode: SampleLoopMode::Forward,
                    }]),
                },
                EventOutcome::Accepted,
                vec![EmittedEvent::EngineSelection {
                    effect: engine_effect,
                }],
                None,
            ),
            schema_record(
                EventSource::Worker,
                EventInput::EnginePreparationFailed {
                    request_id,
                    patch_id: 7,
                    intent: effect_status.correlation().unwrap().intent().clone(),
                    source_capability_id: source_capability_id.clone(),
                    target_capability_id,
                    source_graph_revision: GraphRevision::INITIAL,
                    target_graph_revision,
                    failure: EngineSelectionFailure::AssetUnavailable,
                },
                EventOutcome::Accepted,
                Vec::new(),
                None,
            ),
            schema_record(
                EventSource::Worker,
                EventInput::EngineActivationAcknowledged {
                    request_id,
                    intent: effect_status.correlation().unwrap().intent().clone(),
                    target_graph_revision,
                    retired_graph_revision: GraphRevision::INITIAL,
                    collected: true,
                },
                EventOutcome::Accepted,
                Vec::new(),
                None,
            ),
            schema_record(
                EventSource::Worker,
                EventInput::EnginePrepared {
                    request_id,
                    patch_id: 7,
                    intent: preset_status.correlation().unwrap().intent().clone(),
                    source_capability_id: source_capability_id.clone(),
                    target_capability_id: source_capability_id,
                    source_graph_revision: GraphRevision::INITIAL,
                    target_graph_revision,
                    candidate_config,
                    prepared_visualization: None,
                },
                EventOutcome::Accepted,
                vec![EmittedEvent::EngineSelection {
                    effect: preset_effect,
                }],
                None,
            ),
            schema_record(
                EventSource::Worker,
                EventInput::AssetImported {
                    selection: crate::control::AssetImportResult {
                        request: crate::control::AssetImportRequest {
                            generation: 7,
                            asset_kind: crate::synth::AssetKind::SoundFont,
                            asset_id: AssetFileId::new("Imported/bank.sf2").unwrap(),
                            origin: crate::control::FocusPath::patch_detail(
                                crate::kernel::PatchId::new(7).unwrap(),
                                crate::control::FocusCapabilityId::Instrument(
                                    provider.descriptor().id().clone(),
                                ),
                                crate::control::PatchControlId::Capability(
                                    crate::synth::ParameterId::new("soundfont.file").unwrap(),
                                ),
                            ),
                            graph_revision: GraphRevision::INITIAL,
                        },
                        descriptor: Some(provider.descriptor()),
                        result: Ok(AssetFileId::new("Imported/bank.sf2").unwrap()),
                    },
                },
                EventOutcome::Accepted,
                Vec::new(),
                None,
            ),
            schema_record(
                EventSource::Worker,
                EventInput::SampleAssetLifecycleAdvanced {
                    request_id,
                    lifecycle: crate::control::SampleAssetLifecycle::Validating,
                },
                EventOutcome::Accepted,
                Vec::new(),
                None,
            ),
            schema_record(
                EventSource::Worker,
                EventInput::FileCatalogRefreshed {
                    asset_kind: crate::synth::AssetKind::Sample,
                    folder: catalog_folder,
                    listing: Some(catalog_listing),
                    failure: None,
                },
                EventOutcome::Accepted,
                Vec::new(),
                None,
            ),
            schema_record(
                EventSource::Worker,
                EventInput::FileCatalogRefreshed {
                    asset_kind: crate::synth::AssetKind::Sample,
                    folder: FileBrowserFolderId::default(),
                    listing: None,
                    failure: Some(SampleAssetError::Unavailable),
                },
                EventOutcome::Accepted,
                Vec::new(),
                None,
            ),
        ];
        let mut failed = records
            .iter()
            .find_map(|record| match &record.input {
                EventInput::AssetImported { selection } => Some(selection.clone()),
                _ => None,
            })
            .unwrap();
        // SoundFont alone has choice metadata, but cannot witness numeric
        // ranges or the catalog's stepped/continuous display-label payloads.
        let registry =
            crate::adapter::production_instruments::production_capability_registry().unwrap();
        for id in ["instrument.mutable.tides", "instrument.mda.jx10"] {
            let mut labelled = failed.clone();
            labelled.descriptor = Some(
                registry
                    .descriptor(&crate::synth::CapabilityId::new(id).unwrap())
                    .unwrap()
                    .clone(),
            );
            records.push(schema_record(
                EventSource::Worker,
                EventInput::AssetImported {
                    selection: labelled,
                },
                EventOutcome::Accepted,
                Vec::new(),
                None,
            ));
        }
        failed.descriptor = None;
        failed.result = Err(SampleAssetError::MalformedSoundFont);
        records.push(schema_record(
            EventSource::Worker,
            EventInput::AssetImported { selection: failed },
            EventOutcome::Accepted,
            Vec::new(),
            None,
        ));
        let mut labeled = records
            .iter()
            .find_map(|record| match &record.input {
                EventInput::AssetImported { selection } => Some(selection.clone()),
                _ => None,
            })
            .unwrap();
        labeled.descriptor = Some(labeled_schema_descriptor());
        records.push(schema_record(
            EventSource::Worker,
            EventInput::AssetImported { selection: labeled },
            EventOutcome::Accepted,
            Vec::new(),
            None,
        ));
        records.extend(send_action_witnesses().into_iter().map(|(action, _)| {
            schema_record(
                EventSource::Keyboard,
                EventInput::from(&AppEvent::Send(action)),
                EventOutcome::Accepted,
                Vec::new(),
                None,
            )
        }));
        records
    }

    fn labeled_schema_descriptor() -> crate::synth::CapabilityDescriptor {
        use crate::synth::{
            CapabilityDescriptor, CapabilityId, CapabilitySection, ContinuousValueLabel,
            ParameterDefault, ParameterKind, ParameterRange, ParameterSpec, ParameterUpdate,
            VoicePolicy,
        };
        let range = ParameterRange::new(0.0, 1.0).unwrap();
        let mut parameters = Vec::new();
        for (name, kind, default) in [
            ("step", ParameterKind::Stepped, ParameterValue::Stepped(0)),
            (
                "continuous",
                ParameterKind::Continuous,
                ParameterValue::continuous(0.5).unwrap(),
            ),
        ] {
            let parameter = ParameterSpec::new(
                ParameterId::new(format!("schema.{name}")).unwrap(),
                name,
                kind,
                ParameterUpdate::Scalar,
                ParameterDefault::Value(default),
                Some(range),
                Vec::new(),
                Some(1.0),
                Some(1.0),
                None,
                "number",
                None,
                None,
            )
            .unwrap();
            parameters.push(if kind == ParameterKind::Stepped {
                parameter
                    .with_stepped_labels(vec!["First".into(), "Second".into()])
                    .unwrap()
            } else {
                parameter
                    .with_continuous_labels(vec![
                        ContinuousValueLabel::new(range, "Amount").with_amount(100.0, 0.0, "%")
                    ])
                    .unwrap()
            });
        }
        CapabilityDescriptor::new(
            CapabilityId::new("instrument.schema-labels").unwrap(),
            "Schema Labels",
            "schema",
            vec![CapabilitySection::new("schema.labels", "Labels", parameters).unwrap()],
            Vec::new(),
            VoicePolicy::Configurable { default_voices: 1 },
            vec![MidiMessageKind::NoteOn],
        )
        .unwrap()
    }

    fn send_action_witnesses() -> Vec<(SendAction, Value)> {
        let bus = crate::mixer::bus_id::BusId::new(15).unwrap();
        let slot_id = crate::synth::EffectSlotId::new(3).unwrap();
        vec![
            (SendAction::Open, serde_json::json!({ "kind": "open" })),
            (
                SendAction::Rename {
                    bus,
                    name: "Long Hall".to_owned(),
                },
                serde_json::json!({ "kind": "rename", "bus": 15, "name": "Long Hall" }),
            ),
            (
                SendAction::CancelRename,
                serde_json::json!({ "kind": "cancelRename" }),
            ),
            (
                SendAction::SetEffect {
                    bus,
                    slot_id,
                    entry: Some(crate::synth::EffectCapabilityId::new("effect.reverb").unwrap()),
                },
                serde_json::json!({
                    "kind": "setEffect", "bus": 15, "slotId": 3, "entry": "effect.reverb"
                }),
            ),
            (
                SendAction::SetEffect {
                    bus,
                    slot_id,
                    entry: None,
                },
                serde_json::json!({
                    "kind": "setEffect", "bus": 15, "slotId": 3, "entry": null
                }),
            ),
        ]
    }

    #[test]
    fn send_event_inputs_preserve_event_kind_and_every_typed_action_payload() {
        for (action, expected) in send_action_witnesses() {
            let input = EventInput::from(&AppEvent::Send(action.clone()));
            let serialized = serde_json::to_value(input).unwrap();
            assert_eq!(
                serialized,
                serde_json::json!({ "kind": "send", "action": expected })
            );
            assert_eq!(
                serde_json::from_value::<SendAction>(serialized["action"].clone()).unwrap(),
                action
            );
        }
    }

    fn collect_leaf_paths(value: &Value, prefix: &str, paths: &mut BTreeSet<String>) {
        match value {
            Value::Object(fields) => {
                for (name, value) in fields {
                    let path = if prefix.is_empty() {
                        name.to_owned()
                    } else {
                        format!("{prefix}.{name}")
                    };
                    collect_leaf_paths(value, &path, paths);
                }
            }
            Value::Array(values) => {
                let path = format!("{prefix}[]");
                for value in values {
                    collect_leaf_paths(value, &path, paths);
                }
            }
            _ => {
                assert!(!prefix.is_empty());
                paths.insert(prefix.to_owned());
            }
        }
    }

    #[test]
    fn serialized_leaf_descriptor_exactly_matches_discriminating_record_union() {
        let descriptor = EventRecord::serialized_leaf_descriptor();
        assert!(descriptor.windows(2).all(|pair| pair[0] < pair[1]));

        let described = descriptor
            .iter()
            .copied()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        assert_eq!(described.len(), descriptor.len());

        let mut discovered = BTreeSet::new();
        for record in discriminating_schema_records() {
            let value = serde_json::to_value(record).expect("schema record serializes");
            collect_leaf_paths(&value, "", &mut discovered);
        }

        assert_eq!(described, discovered);
    }

    #[test]
    fn engine_prepared_event_input_keeps_the_bounded_visualization_without_pcm() {
        let asset = AssetFileId::new("event-input.wav").unwrap();
        let pair = WaveformPair {
            left_min: -0.75,
            left_max: 0.5,
            right_min: -0.25,
            right_max: 0.125,
        };
        let pcm = PreparedSamplePcm::new(
            asset.clone(),
            48_000,
            1,
            Arc::from([0.0_f32, 0.25, -0.5, 0.75]),
            Arc::from([pair]),
        )
        .unwrap();
        let visualization = PreparedSampleVisualization::new(
            &pcm,
            PreparedSampleLandmarks {
                start: 0,
                end: 4,
                loop_start: 1,
                loop_end: 3,
                crossfade_frames: 1,
                loop_mode: SampleLoopMode::Forward,
            },
        );
        let state = installed_state();
        let config = state.patches()[0].instrument_config().clone();
        let capability_id = config.capability_id().clone();
        let event = AppEvent::EnginePrepared {
            request_id: EngineSelectionRequestId::FIRST,
            patch_id: state.patches()[0].id(),
            intent: StructuralEditIntent::ReplaceCapability {
                target_capability_id: capability_id.clone(),
            },
            source_capability_id: capability_id.clone(),
            target_capability_id: capability_id,
            source_graph_revision: GraphRevision::INITIAL,
            target_graph_revision: GraphRevision::new(2).unwrap(),
            candidate_config: config,
            prepared_visualization: Some(vec![visualization]),
        };
        let input = EventInput::from(&event);
        let EventInput::EnginePrepared {
            prepared_visualization: Some(recorded),
            ..
        } = &input
        else {
            panic!("EnginePrepared records its visualization");
        };
        assert_eq!(recorded[0].asset_id(), &asset);
        assert_eq!(recorded[0].waveform(), &[pair]);
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("preparedVisualization"));
        assert!(!json.contains("interleaved"));
        assert!(!json.contains("pcm"));
    }

    #[test]
    fn event_source_surface_includes_physical_midi_with_stable_serialized_names() {
        let descriptor = EventSource::surface_descriptor();
        assert_eq!(descriptor.len(), 8);
        for (index, source) in descriptor.iter().enumerate() {
            assert!(!descriptor[..index].contains(source));
            assert_eq!(
                serde_json::to_string(source).unwrap(),
                format!("\"{}\"", source.name())
            );
        }
        assert!(descriptor.contains(&EventSource::Worker));
        assert!(descriptor.contains(&EventSource::PhysicalMidi));
        assert!(descriptor.contains(&EventSource::System));
        assert!(descriptor.contains(&EventSource::Controller));
        assert_ne!(EventSource::PhysicalMidi, EventSource::AutomaticMidi);
    }

    #[test]
    fn accepted_record_keeps_one_coherent_hash_generation_and_effect_chain() {
        let mut state = installed_state();
        let message = MidiMessage::try_new(
            MidiChannel::new(0).unwrap(),
            MidiMessageKind::NoteOn,
            60,
            100,
        )
        .unwrap();
        let event = AppEvent::Midi {
            patch_id: PatchId::new(1).unwrap(),
            message,
        };
        let generation_before = state.generation();
        let outcome = state.apply(event.clone()).unwrap();
        let (snapshot, projection) = snapshot_and_projection(state.generation());

        let record = EventRecord::accepted(
            7,
            EventSource::AutomaticMidi,
            &event,
            generation_before,
            "previous-state-hash",
            outcome.accepted(),
            &snapshot,
            state.generation(),
            GraphRevision::INITIAL,
            true,
            &projection,
            outcome.audio_command().copied(),
            outcome.engine_selection_effect().cloned(),
        )
        .unwrap();

        assert_eq!(record.sequence(), 7);
        assert_eq!(record.outcome(), EventOutcome::Accepted);
        assert_eq!(record.generation_before(), 1);
        assert_eq!(record.generation_after(), 2);
        assert_eq!(record.state_hash_before(), "previous-state-hash");
        assert_eq!(record.state_hash_after(), snapshot.hash());
        assert_eq!(record.projection_state_hash(), snapshot.hash());
        assert_eq!(record.parameter_generation(), 2);
        assert_eq!(record.selected_line(), 1);
        assert_eq!(record.rejection(), None);
        assert_eq!(record.emitted_events().len(), 3);
        assert_eq!(
            record.emitted_events()[0],
            EmittedEvent::StateAccepted { generation: 2 }
        );
        assert_eq!(
            record.emitted_events()[1],
            EmittedEvent::ParameterSnapshotPublished {
                generation: 2,
                graph_revision: GraphRevision::INITIAL,
            }
        );
        assert_eq!(
            record.emitted_events()[2],
            EmittedEvent::AudioCommand {
                effect: AudioEffect::PatchMidi {
                    patch_id: 1,
                    message: message.into(),
                },
            }
        );

        let json: serde_json::Value = serde_json::from_str(&record.to_json().unwrap()).unwrap();
        assert_eq!(json["source"], "automaticMidi");
        assert_eq!(json["input"]["kind"], "midi");
        assert_eq!(json["input"]["patchId"], 1);
        assert_eq!(json["input"]["message"]["kind"], "noteOn");
        assert_eq!(json["rejection"], serde_json::Value::Null);

        let physical_record = EventRecord::accepted(
            8,
            EventSource::PhysicalMidi,
            &event,
            generation_before,
            "previous-state-hash",
            outcome.accepted(),
            &snapshot,
            state.generation(),
            GraphRevision::INITIAL,
            true,
            &projection,
            outcome.audio_command().copied(),
            outcome.engine_selection_effect().cloned(),
        )
        .unwrap();
        let physical_json: serde_json::Value =
            serde_json::from_str(&physical_record.to_json().unwrap()).unwrap();
        assert_eq!(physical_record.source(), EventSource::PhysicalMidi);
        assert_eq!(physical_json["source"], "physicalMidi");
    }

    #[test]
    fn rejected_record_preserves_state_and_emits_nothing() {
        let state = installed_state();
        let event = AppEvent::Adjust(Direction::Right);
        let (snapshot, projection) = snapshot_and_projection(state.generation());

        let record = EventRecord::rejected(
            8,
            EventSource::Keyboard,
            &event,
            state.generation(),
            snapshot.hash(),
            state.generation(),
            &projection,
            EventRejection::ParameterAtBoundary,
        )
        .unwrap();

        assert_eq!(record.outcome(), EventOutcome::Rejected);
        assert_eq!(record.generation_before(), record.generation_after());
        assert_eq!(record.state_hash_before(), record.state_hash_after());
        assert!(record.emitted_events().is_empty());
        assert_eq!(
            record.rejection(),
            Some(EventRejection::ParameterAtBoundary)
        );

        let json: serde_json::Value = serde_json::from_str(&record.to_json().unwrap()).unwrap();
        assert_eq!(json["input"]["kind"], "adjust");
        assert_eq!(json["input"]["direction"], "right");
        assert_eq!(json["rejection"], "parameterAtBoundary");
    }

    #[test]
    fn install_input_records_every_domain_payload() {
        let event = AppEvent::InstallPatches(vec![patch(3)]);
        let input = EventInput::from(&event);

        let EventInput::InstallPatches { patches } = input else {
            panic!("expected installation descriptor");
        };
        let installed = &patches[0];

        assert_eq!(installed.id(), 3);
        assert_eq!(installed.name(), "Patch 3");
        assert_eq!(installed.channel(), 2);
        assert_eq!(
            installed.instrument_config().capability_id().as_str(),
            HIDEF_CAPABILITY_ID
        );
        assert_eq!(
            installed
                .instrument_config()
                .value(&ParameterId::new(SOUNDFONT_PRESET_PARAMETER_ID).unwrap()),
            Some(&ParameterValue::Choice(
                SoundFontInstrument::new(128, 2, false)
                    .unwrap()
                    .preset_id()
                    .choice_id()
            ))
        );
        assert_eq!(installed.output().trim_gain_db(), -6.0);
        assert_eq!(installed.output().track_id().value(), 2);
        // The recorded payload field is read directly: the same-module test
        // asserts the stored dense payload, not an accessor round-trip.
        assert!(installed.post_effects.is_empty());
    }

    #[test]
    fn install_input_preserves_slot_identities_of_a_gapped_chain() {
        // Slot 0 empty, slot 1 occupied: the shape a compacting accessor
        // used to silently squeeze down to position 0.
        let mut gapped = patch(1);
        gapped
            .set_slot_occupancy(
                crate::synth::effect_slot_id::EffectSlotIndex::new(1).unwrap(),
                Some(
                    crate::adapter::production_effects::production_chorus_config(
                        crate::synth::EffectSlotId::new(2).unwrap(),
                    )
                    .unwrap(),
                ),
            )
            .unwrap();
        assert!(gapped.effect_slots()[0].is_none());

        let input = PatchInput::from(&gapped);

        // The frozen dense payload carries exactly the occupied
        // configuration, and its stable slot identity still names position
        // 1 — an identity of 1 here would mean the record renumbered the
        // chain.
        assert_eq!(input.post_effects.len(), 1);
        assert_eq!(input.post_effects[0].slot_id().value(), 2);
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["postEffects"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn accepted_record_rejects_incoherent_projection_generation() {
        let mut state = installed_state();
        let event = AppEvent::Navigate(Direction::Down);
        let generation_before = state.generation();
        let outcome = state.apply(event.clone()).unwrap();
        let (snapshot, projection) = snapshot_and_projection(state.generation());

        let error = EventRecord::accepted(
            1,
            EventSource::DemoScene,
            &event,
            generation_before,
            "previous-state-hash",
            outcome.accepted(),
            &snapshot,
            state.generation() + 1,
            GraphRevision::INITIAL,
            false,
            &projection,
            None,
            None,
        )
        .unwrap_err();

        assert_eq!(
            error,
            EventRecordError::ParameterGenerationMismatch {
                expected: state.generation(),
                actual: state.generation() + 1,
            }
        );
    }
}
