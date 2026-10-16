//! Control-side operations available to an embedding application.
//!
//! The host borrows this facade for one control callback. Canonical Synth
//! state, real-time queues, native workers, and graph ownership remain private.

use crate::control::app_event::AppEvent;
use crate::control::app_loop::{AppLoop, ScheduledEventDispatchError};
use crate::control::app_state::EventRejection;
use crate::control::engine_selection::EngineSelectionStatusKind;
use crate::control::event_record::EventSource;
use crate::control::saved_session::{SavedSessionError, SoundDefinition, SoundDefinitionError};
use crate::control::{
    GraphicalShellProjection, InteractionMode, MidiActiveInputIdentity, MidiActivitySnapshot,
    MidiInputDescriptor, SavedSession, SemanticAction, SendAction, SurfaceId, TopLevelContext,
};
use crate::kernel::midi_channel::MidiChannel;
use crate::kernel::midi_message::MidiMessage;
use crate::kernel::patch_id::PatchId;
use crate::real_time::audio_boundary::{BoundaryFull, ControlAudioBoundary};
use crate::real_time::scheduled_event::{ScheduledEvent, ScheduledEventEpochExhausted};
use crate::real_time::GraphRevision;
use crate::shell::{SessionLifecycleCoordinator, SessionLifecycleError, SessionLifecycleStage};
use crate::synth::{MidiSource, MidiVoiceId, OwnedVoiceInitialization};
use serde::Serialize;

/// Read-only target identity and channel subscription for one installed Patch.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostedPatch {
    id: PatchId,
    name: String,
    channel: MidiChannel,
}

/// Global destinations an embedding application can reveal. Subordinate
/// editors and settings tabs remain owned by Synth's semantic navigation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostedSynthPage {
    Instruments,
    Mixer,
    Sends,
    MidiSettings,
}

impl HostedSynthPage {
    fn surface(self) -> SurfaceId {
        match self {
            Self::Instruments => SurfaceId::PatchMain,
            Self::Mixer => SurfaceId::MixerMain,
            Self::Sends => SurfaceId::Sends,
            Self::MidiSettings => SurfaceId::MidiDeviceSettings,
        }
    }

    fn action(self) -> SemanticAction {
        match self {
            Self::Instruments => SemanticAction::SelectContext(TopLevelContext::Patch),
            Self::Mixer => SemanticAction::SelectContext(TopLevelContext::Mixer),
            Self::Sends => SemanticAction::Send(SendAction::Open),
            Self::MidiSettings => SemanticAction::OpenMidiSettings,
        }
    }
}

impl HostedPatch {
    pub const fn id(&self) -> PatchId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn channel(&self) -> MidiChannel {
        self.channel
    }
}

/// Proven semantics for a tagged attack. This is not an MPE zone/profile claim;
/// the embedding host still owns master/member interpretation and event order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OwnedExpressionControl {
    Raw14PitchBend,
    Rpn0BendRange,
    AbsoluteCc74Timbre,
    /// Overrides the addressed dimension's pedal. Either member or master
    /// pedal holds the attack; pedal-down cannot revive an existing release.
    Cc64SustainOverride,
    /// Channel pressure addressed to an attack scales that attack's gain.
    ChannelPressureGain,
    /// Polyphonic key pressure scales gain only for the addressed attack/key.
    KeyPressureGain,
}

/// Read-only support witnessed for this exact Patch and graph revision. Query
/// again after a graph change; engine names and renderer state remain private.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedExpressionProfile {
    patch_id: PatchId,
    graph_revision: GraphRevision,
    controls: &'static [OwnedExpressionControl],
    master_controls: &'static [OwnedExpressionControl],
}

impl OwnedExpressionProfile {
    pub const fn patch_id(self) -> PatchId {
        self.patch_id
    }
    pub const fn graph_revision(self) -> GraphRevision {
        self.graph_revision
    }

    /// Supported member controls. Kept as the original single-dimension API.
    pub const fn controls(self) -> &'static [OwnedExpressionControl] {
        self.controls
    }

    pub const fn member_controls(self) -> &'static [OwnedExpressionControl] {
        self.controls
    }

    /// Independent controls addressed to the same attack, not channel-wide
    /// broadcasts. Master/member bends add before native quantization;
    /// channel-pressure gains multiply and sustain pedals OR. Master timbre,
    /// key pressure, attacks, releases and other controllers are unsupported.
    /// Both dimensions start centered with a 2-semitone bend range. The host
    /// uses an atomic initialized attack to configure both dimensions before audio.
    pub const fn master_controls(self) -> &'static [OwnedExpressionControl] {
        self.master_controls
    }

    /// Uses the same exact admission rule as native rendering: normalized MIDI,
    /// RPN0 range 0..127 semitones plus 0..99 cents, resulting pitch 0 through
    /// 127.9921875 semitones. Check each event, including range changes at a
    /// noncentral bend. Validation precedes native 1/128-semitone quantization.
    /// Initial per-attack range is 2 semitones; raw14 center is 8192.
    pub fn validate_pitch(
        self,
        note: u8,
        raw14: u16,
        semitones: u8,
        cents: u8,
    ) -> Result<(), OwnedExpressionPitchError> {
        crate::adapter::braids_preparer::owned_pitch_bend_units(note, raw14, semitones, cents)
            .map(|_| ())
            .ok_or(OwnedExpressionPitchError {
                note,
                raw14,
                semitones,
                cents,
            })
    }

    /// Validates the exact combined native pitch. Each tuple is
    /// `(raw14_bend, range_semitones, range_cents)`. Validate after each change,
    /// including RPN0 range edits at noncentral bends; dimensions may offset
    /// each other even when one alone would leave the native pitch interval.
    pub fn validate_combined_pitch(
        self,
        note: u8,
        member: (u16, u8, u8),
        master: (u16, u8, u8),
    ) -> Result<(), OwnedCombinedExpressionPitchError> {
        crate::adapter::braids_preparer::owned_combined_pitch_bend_units(note, member, master)
            .map(|_| ())
            .ok_or(OwnedCombinedExpressionPitchError {
                note,
                member,
                master,
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("owned pitch is outside this profile: note {note}, raw14 bend {raw14}, range {semitones} semitones {cents} cents; require MIDI note 0..127, raw14 0..16383, range 0..127 plus 0..99 cents, and resulting pitch 0..127.9921875")]
pub struct OwnedExpressionPitchError {
    note: u8,
    raw14: u16,
    semitones: u8,
    cents: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("combined owned pitch is outside this profile: note {note}, member {member:?}, master {master:?} (raw14, semitones, cents); require MIDI note 0..127, each raw14 0..16383, each range 0..127 plus 0..99 cents, and resulting pitch 0..127.9921875")]
pub struct OwnedCombinedExpressionPitchError {
    note: u8,
    member: (u16, u8, u8),
    master: (u16, u8, u8),
}

#[derive(Debug, thiserror::Error)]
pub enum HostedSynthError {
    #[error("saved sound edits are blocked by a pending document operation")]
    EditsBlocked,
    #[error("sound state cannot be captured while a structural change is pending")]
    CaptureBusy,
    #[error("Patch {patch_id} has no proven owned-expression profile")]
    OwnedExpressionUnsupported { patch_id: PatchId },
    #[error(
        "Patch {patch_id} owned-expression support is unavailable until the sound graph is ready"
    )]
    OwnedExpressionNotReady { patch_id: PatchId },
    #[error(transparent)]
    OwnedCombinedPitch(#[from] OwnedCombinedExpressionPitchError),
    #[error(transparent)]
    Control(#[from] EventRejection),
    #[error(transparent)]
    AudioBoundary(#[from] BoundaryFull),
    #[error(transparent)]
    Scheduled(#[from] ScheduledEventDispatchError),
    #[error(transparent)]
    ScheduleEpoch(#[from] ScheduledEventEpochExhausted),
    #[error("this control callback has no session lifecycle")]
    LifecycleUnavailable,
    #[error(transparent)]
    Lifecycle(Box<SessionLifecycleError>),
    #[error(transparent)]
    SoundDefinition(#[from] SoundDefinitionError),
    #[error(transparent)]
    SavedSession(#[from] SavedSessionError),
}

/// Temporary access to validated Synth operations, never mutable product state.
pub struct HostedSynthControl<'a, Boundary: ControlAudioBoundary> {
    app: &'a mut AppLoop<Boundary>,
    persisted_edits_blocked: bool,
    lifecycle: Option<&'a mut SessionLifecycleCoordinator>,
}

impl<'a, Boundary: ControlAudioBoundary> HostedSynthControl<'a, Boundary> {
    #[cfg(test)]
    pub(crate) fn new(app: &'a mut AppLoop<Boundary>, persisted_edits_blocked: bool) -> Self {
        Self {
            app,
            persisted_edits_blocked,
            lifecycle: None,
        }
    }

    pub(crate) fn with_lifecycle(
        app: &'a mut AppLoop<Boundary>,
        lifecycle: &'a mut SessionLifecycleCoordinator,
    ) -> Self {
        Self {
            app,
            persisted_edits_blocked: lifecycle.persisted_edits_blocked(),
            lifecycle: Some(lifecycle),
        }
    }

    /// Returns installation order and canonical IDs without exposing sound internals.
    pub fn installed_patches(&self) -> Vec<HostedPatch> {
        self.app
            .patches()
            .iter()
            .map(|patch| HostedPatch {
                id: patch.id(),
                name: patch.name().to_owned(),
                channel: patch.channel(),
            })
            .collect()
    }

    /// Reports only proven semantics of the actual installed engine. A pending
    /// structural/document operation cannot advertise its candidate or old graph
    /// as ready. Unsupported engines never inherit support from UI selection.
    pub fn owned_expression_profile(
        &self,
        patch_id: PatchId,
    ) -> Result<OwnedExpressionProfile, HostedSynthError> {
        let patch = self
            .app
            .patches()
            .iter()
            .find(|patch| patch.id() == patch_id)
            .ok_or(EventRejection::UnknownPatch)?;
        if self.saved_edits_blocked()
            || self.app.session_replacement_pending()
            || self.app.engine_selection_status().kind() != EngineSelectionStatusKind::Ready
            || self.app.staged_graph_revision().is_some()
            || self.app.in_flight_graph_revision().is_some()
            || self.app.active_audio_graph_revision() != self.app.graph_revision()
            || self
                .app
                .engine_graph_handoff_status()
                .is_some_and(|status| status.active_revision().is_none())
            || self.app.current_parameters().patch(patch_id).is_none()
        {
            return Err(HostedSynthError::OwnedExpressionNotReady { patch_id });
        }
        let config = patch.instrument_config();
        if self.app.capabilities().validate_config(config).is_err() {
            return Err(HostedSynthError::OwnedExpressionNotReady { patch_id });
        }
        if config.capability_id().as_str()
            != crate::adapter::braids_capability::BRAIDS_CAPABILITY_ID
        {
            return Err(HostedSynthError::OwnedExpressionUnsupported { patch_id });
        }
        Ok(OwnedExpressionProfile {
            patch_id,
            graph_revision: self.app.active_audio_graph_revision(),
            controls: &[
                OwnedExpressionControl::Raw14PitchBend,
                OwnedExpressionControl::Rpn0BendRange,
                OwnedExpressionControl::AbsoluteCc74Timbre,
                OwnedExpressionControl::Cc64SustainOverride,
                OwnedExpressionControl::ChannelPressureGain,
                OwnedExpressionControl::KeyPressureGain,
            ],
            master_controls: &[
                OwnedExpressionControl::Raw14PitchBend,
                OwnedExpressionControl::Rpn0BendRange,
                OwnedExpressionControl::Cc64SustainOverride,
                OwnedExpressionControl::ChannelPressureGain,
            ],
        })
    }

    pub fn projection(&self) -> GraphicalShellProjection {
        self.app.current_graphical_shell()
    }

    /// Exact reducer-acknowledged device identity for physical MIDI capture.
    pub fn active_midi_input_identity(&self) -> Option<&MidiActiveInputIdentity> {
        self.app.active_midi_input_identity()
    }

    /// The active device's canonical discovery metadata, never the saved
    /// preference or a pending connection candidate.
    pub fn active_midi_input_descriptor(&self) -> Option<&MidiInputDescriptor> {
        let input = self.app.state().midi_input();
        input
            .entry(input.active()?.identity())
            .map(|entry| entry.descriptor())
    }

    /// Reveals a global page even when an embedded workspace was hidden with
    /// settings or a modal editor open. Repeated navigation preserves focus.
    /// All exits use native actions, including preview and capture cleanup.
    pub fn show_page(
        &mut self,
        page: HostedSynthPage,
        source: EventSource,
    ) -> Result<(), HostedSynthError> {
        if self.app.state().interaction().active_surface() == page.surface() {
            return Ok(());
        }
        if self.app.state().interaction().active_surface().is_system() {
            self.dispatch_action(SemanticAction::Return, source)?;
            // The first Return cancels controller capture when it is active;
            // the second leaves its settings session.
            if self.app.state().interaction().active_surface().is_system() {
                self.dispatch_action(SemanticAction::Return, source)?;
            }
        }
        if self.app.state().interaction().mode() == InteractionMode::Modal {
            self.dispatch_action(SemanticAction::Return, source)?;
        }
        if page == HostedSynthPage::MidiSettings
            && self.app.state().interaction().mode() != InteractionMode::Navigate
        {
            self.dispatch_action(
                SemanticAction::SetInteractionMode(InteractionMode::Navigate),
                source,
            )?;
        }
        self.dispatch_action(page.action(), source)
    }

    /// Latest activity only while its connection revision remains canonical.
    pub fn midi_activity(&self) -> Option<MidiActivitySnapshot> {
        self.app.current_midi_activity()
    }

    /// Runs an existing Synth UI operation through its reducer and edit gate.
    pub fn dispatch_action(
        &mut self,
        action: SemanticAction,
        source: EventSource,
    ) -> Result<(), HostedSynthError> {
        if self.saved_edits_blocked() && action.may_change_saved_session() {
            return Err(HostedSynthError::EditsBlocked);
        }
        let result = self.app.dispatch_action_from(action, source)?;
        if let Some(error) = result.boundary_full() {
            return Err(error.into());
        }
        Ok(())
    }

    /// Delivers immediate MIDI to one exact installed Patch, without channel fanout.
    pub fn send_midi(
        &mut self,
        patch_id: PatchId,
        message: MidiMessage,
    ) -> Result<(), HostedSynthError> {
        let result = self.app.dispatch_from(
            AppEvent::Midi { patch_id, message },
            EventSource::AutomaticMidi,
        )?;
        if let Some(error) = result.boundary_full() {
            return Err(error.into());
        }
        Ok(())
    }

    /// Delivers immediate MIDI to every current subscriber; returns recipient count.
    pub fn send_channel_midi(&mut self, message: MidiMessage) -> Result<usize, HostedSynthError> {
        let result = self
            .app
            .dispatch_midi_from(message, EventSource::AutomaticMidi)?;
        if let Some(error) = result.boundary_full() {
            return Err(error.into());
        }
        Ok(result.subscriber_count())
    }

    /// Delivers one exact host-owned monitored articulation through the reducer
    /// and bounded immediate audio queue. Master controls remain attack-scoped.
    pub fn send_owned_monitor_midi(
        &mut self,
        patch_id: PatchId,
        voice: crate::synth::MidiVoiceId,
        scope: crate::synth::MidiExpressionScope,
        message: MidiMessage,
    ) -> Result<(), HostedSynthError> {
        let result = self.app.dispatch_voice_for_source(
            patch_id,
            message,
            MidiSource::Monitor,
            voice,
            scope,
        )?;
        if let Some(error) = result.boundary_full() {
            return Err(error.into());
        }
        Ok(())
    }

    /// One bounded command initializes both expression dimensions and starts the attack.
    pub fn send_initialized_monitor_voice(
        &mut self,
        patch: PatchId,
        voice: MidiVoiceId,
        initial: OwnedVoiceInitialization,
    ) -> Result<(), HostedSynthError> {
        self.validate_initialized_voice(patch, initial)?;
        let result = self.app.dispatch_initialized_voice_for_source(
            patch,
            MidiSource::Monitor,
            voice,
            initial,
        )?;
        if let Some(error) = result.boundary_full() {
            return Err(error.into());
        }
        Ok(())
    }

    /// Schedules one atomic attack after validating its complete initial pitch.
    pub fn schedule_initialized_voice(
        &mut self,
        frame: u64,
        patch: PatchId,
        voice: MidiVoiceId,
        initial: OwnedVoiceInitialization,
    ) -> Result<(), HostedSynthError> {
        self.validate_initialized_voice(patch, initial)?;
        let event = ScheduledEvent::initialized_voice(
            frame,
            self.schedule_epoch(),
            self.active_graph_revision(),
            patch,
            voice,
            initial,
        );
        self.app
            .dispatch_scheduled_midi_from(event, EventSource::AutomaticMidi)?;
        Ok(())
    }

    fn validate_initialized_voice(
        &self,
        patch: PatchId,
        initial: OwnedVoiceInitialization,
    ) -> Result<(), HostedSynthError> {
        let member = initial.member();
        let master = initial.master();
        self.owned_expression_profile(patch)?
            .validate_combined_pitch(
                initial.message().data1(),
                (member.raw14(), member.semitones(), member.cents()),
                (master.raw14(), master.semitones(), master.cents()),
            )?;
        Ok(())
    }

    /// Delivers host monitoring MIDI with ownership separate from live input and playback.
    pub fn send_monitor_midi(
        &mut self,
        patch_id: PatchId,
        message: MidiMessage,
    ) -> Result<(), HostedSynthError> {
        let result = self
            .app
            .dispatch_midi_for_source(patch_id, message, MidiSource::Monitor)?;
        if let Some(error) = result.boundary_full() {
            return Err(error.into());
        }
        Ok(())
    }

    /// Releases only monitoring voices for one installed Patch.
    pub fn cancel_monitor(&mut self, patch_id: PatchId) -> Result<(), HostedSynthError> {
        Ok(self.app.cancel_midi_source(patch_id, MidiSource::Monitor)?)
    }

    pub fn active_graph_revision(&self) -> GraphRevision {
        self.app.active_audio_graph_revision()
    }

    pub fn schedule_epoch(&self) -> u64 {
        self.app.scheduled_epoch()
    }

    /// Queues one absolute renderer-frame delivery in the current graph and epoch.
    /// Frames must be nondecreasing across all producers within that epoch.
    pub fn schedule_midi(
        &mut self,
        absolute_frame: u64,
        patch_id: PatchId,
        message: MidiMessage,
    ) -> Result<(), HostedSynthError> {
        self.schedule_voice_midi(absolute_frame, patch_id, message, None)
    }
    pub fn schedule_voice_midi(
        &mut self,
        absolute_frame: u64,
        patch_id: PatchId,
        message: MidiMessage,
        voice: Option<crate::synth::MidiVoiceId>,
    ) -> Result<(), HostedSynthError> {
        self.schedule_scoped_voice_midi(
            absolute_frame,
            patch_id,
            message,
            voice,
            crate::synth::MidiExpressionScope::Member,
        )
    }
    /// Schedules raw member/master expression for one owned articulation.
    pub fn schedule_scoped_voice_midi(
        &mut self,
        absolute_frame: u64,
        patch_id: PatchId,
        message: MidiMessage,
        voice: Option<crate::synth::MidiVoiceId>,
        scope: crate::synth::MidiExpressionScope,
    ) -> Result<(), HostedSynthError> {
        let event = ScheduledEvent::new(
            absolute_frame,
            self.schedule_epoch(),
            self.active_graph_revision(),
            patch_id,
            message,
        )
        .with_voice(voice)
        .with_expression_scope(scope);
        self.app
            .dispatch_scheduled_midi_from(event, EventSource::AutomaticMidi)?;
        Ok(())
    }

    /// Begins a temporary send at an exact sample. The lease never edits saved
    /// state; its end is either the supplied frame or an explicit scheduled end.
    pub fn schedule_send_lease(
        &mut self,
        absolute_frame: u64,
        patch: PatchId,
        lease: crate::real_time::SendLease,
    ) -> Result<(), HostedSynthError> {
        let event = ScheduledEvent::send_lease(
            absolute_frame,
            self.schedule_epoch(),
            self.active_graph_revision(),
            patch,
            lease,
        );
        self.app.dispatch_scheduled_send(event)?;
        Ok(())
    }
    pub fn schedule_end_send_lease(
        &mut self,
        absolute_frame: u64,
        patch: PatchId,
        id: crate::real_time::SendLeaseId,
    ) -> Result<(), HostedSynthError> {
        let event = ScheduledEvent::end_send_lease(
            absolute_frame,
            self.schedule_epoch(),
            self.active_graph_revision(),
            patch,
            id,
        );
        self.app.dispatch_scheduled_send(event)?;
        Ok(())
    }

    /// Cancels pending playback and releases notes owned by that schedule.
    pub fn cancel_scheduled(&mut self) -> Result<u64, HostedSynthError> {
        Ok(self.app.cancel_scheduled_midi()?)
    }

    /// Captures a complete settled sound state for a composite host document.
    pub fn capture_saved_session(&self) -> Result<SavedSession, HostedSynthError> {
        if self
            .lifecycle
            .as_ref()
            .is_some_and(|lifecycle| lifecycle.persisted_edits_blocked())
            || self.app.session_replacement_pending()
            || matches!(
                self.app.engine_selection_status().kind(),
                EngineSelectionStatusKind::Loading
                    | EngineSelectionStatusKind::Validating
                    | EngineSelectionStatusKind::Preparing
                    | EngineSelectionStatusKind::Activating
            )
        {
            return Err(HostedSynthError::CaptureBusy);
        }
        Ok(self.app.capture_saved_session())
    }

    /// Captures portable sound content without installation or mixer settings.
    pub fn capture_sound_definition(
        &self,
        patch_id: PatchId,
    ) -> Result<SoundDefinition, HostedSynthError> {
        Ok(self.capture_saved_session()?.sound_definition(patch_id)?)
    }

    /// Replaces only an installed Patch's sound content through the existing
    /// worker and prepared-graph activation path. Routing and mix remain local.
    /// Completion and typed failure are reported by `replacement_status`.
    pub fn request_install_sound_definition(
        &mut self,
        patch_id: PatchId,
        definition: SoundDefinition,
    ) -> Result<(), HostedSynthError> {
        if self.saved_edits_blocked() {
            return Err(HostedSynthError::EditsBlocked);
        }
        let candidate = self
            .capture_saved_session()?
            .with_sound_definition(patch_id, definition)?;
        self.request_restore_json_requiring_patches(
            candidate.to_json()?.into_bytes(),
            vec![patch_id],
        )
    }

    fn saved_edits_blocked(&self) -> bool {
        self.lifecycle
            .as_ref()
            .map_or(self.persisted_edits_blocked, |lifecycle| {
                lifecycle.persisted_edits_blocked()
            })
    }

    /// Requests restoration through the existing candidate worker and graph owner.
    /// The host publishes its matching document only after `replacement_status`
    /// reports completion; failure preserves the prior sound session.
    pub fn request_restore(&mut self, saved: SavedSession) -> Result<(), HostedSynthError> {
        let lifecycle = self
            .lifecycle
            .as_mut()
            .ok_or(HostedSynthError::LifecycleUnavailable)?;
        lifecycle
            .request_host_restore(saved, self.app)
            .map_err(|error| HostedSynthError::Lifecycle(Box::new(error)))
    }

    /// Sends encoded sound content to the existing worker for parsing,
    /// capability validation, asset admission, and complete graph preparation.
    pub fn request_restore_json(&mut self, bytes: Vec<u8>) -> Result<(), HostedSynthError> {
        self.request_restore_json_requiring_patches(bytes, Vec::new())
    }

    /// Restores sound only if every host-referenced Patch exists in the prepared
    /// candidate. Missing references fail before graph activation or replacement.
    pub fn request_restore_json_requiring_patches(
        &mut self,
        bytes: Vec<u8>,
        required_patches: Vec<PatchId>,
    ) -> Result<(), HostedSynthError> {
        let lifecycle = self
            .lifecycle
            .as_mut()
            .ok_or(HostedSynthError::LifecycleUnavailable)?;
        lifecycle
            .request_host_restore_json_requiring_patches(bytes, required_patches, self.app)
            .map_err(|error| HostedSynthError::Lifecycle(Box::new(error)))
    }

    /// Reports completion of an outstanding host restore after audio activation.
    /// The host owns whether it has issued a request; an idle Ready lifecycle
    /// also returns true. Failures retain the coordinator's typed cause.
    pub fn replacement_status(&self) -> Result<bool, HostedSynthError> {
        let lifecycle = self
            .lifecycle
            .as_ref()
            .ok_or(HostedSynthError::LifecycleUnavailable)?;
        if let Some(error) = lifecycle.failure() {
            return Err(HostedSynthError::Lifecycle(Box::new(error.clone())));
        }
        Ok(lifecycle.stage() == SessionLifecycleStage::Ready
            && !self.app.session_replacement_pending())
    }

    pub fn session_replacement_pending(&self) -> bool {
        self.app.session_replacement_pending()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::braids_capability::BraidsCapability;
    use crate::adapter::lock_free_audio_boundary::{
        LockFreeAudioBoundary, LockFreeAudioHandle, LockFreeControlHandle,
    };
    use crate::control::app_state::AppState;
    use crate::control::state_projector::StateProjector;
    use crate::control::Direction;
    use crate::kernel::midi_message::MidiMessageKind;
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_state::MixerState;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::audio_command::AudioCommand;
    use crate::real_time::scheduled_event::{ScheduledEventPoll, ScheduledEventRejectionReason};
    use crate::real_time::{AudioBoundary, AudioThreadBoundary, ParameterSnapshot};
    use crate::synth::{CapabilityRegistry, InstrumentCapabilityProvider, Patch};

    fn app() -> (AppLoop<LockFreeControlHandle>, LockFreeAudioHandle) {
        let provider = BraidsCapability::new().unwrap();
        let global = GlobalParameters::new(0.0).unwrap();
        let registry = CapabilityRegistry::new(vec![provider.descriptor()]).unwrap();
        let mut state = AppState::new(registry, global);
        let patches = [1, 7]
            .into_iter()
            .map(|id| {
                Patch::new(
                    PatchId::new(id).unwrap(),
                    format!("Sound {id}"),
                    provider.default_config().unwrap(),
                    MidiChannel::new(0).unwrap(),
                    PatchOutput::default(),
                )
            })
            .collect();
        state.apply(AppEvent::InstallPatches(patches)).unwrap();
        let initial = ParameterSnapshot::new(0, global, MixerState::default(), &[]).unwrap();
        let (control, audio) = LockFreeAudioBoundary::new(8, initial).into_handles();
        (
            AppLoop::new(state, StateProjector::new(), control).unwrap(),
            audio,
        )
    }

    fn note() -> MidiMessage {
        MidiMessage::try_new(
            MidiChannel::new(0).unwrap(),
            MidiMessageKind::NoteOn,
            60,
            100,
        )
        .unwrap()
    }

    #[test]
    fn hosted_global_navigation_reopens_pages_without_losing_focus_or_sound() {
        let (mut app, _audio) = app();
        let mut host = HostedSynthControl::new(&mut app, true);
        let saved = host.capture_saved_session().unwrap();
        for page in [
            HostedSynthPage::MidiSettings,
            HostedSynthPage::Mixer,
            HostedSynthPage::MidiSettings,
            HostedSynthPage::Sends,
            HostedSynthPage::MidiSettings,
            HostedSynthPage::Instruments,
        ] {
            host.show_page(page, EventSource::System).unwrap();
            assert_eq!(
                host.app.state().interaction().active_surface(),
                page.surface()
            );
            let focus = host.app.state().interaction().focus_path().clone();
            let generation = host.projection().generation();
            host.show_page(page, EventSource::System).unwrap();
            assert_eq!(host.app.state().interaction().focus_path(), &focus);
            assert_eq!(host.projection().generation(), generation);
            assert_eq!(host.capture_saved_session().unwrap(), saved);
        }
        host.show_page(HostedSynthPage::MidiSettings, EventSource::System)
            .unwrap();
        host.dispatch_action(
            SemanticAction::Navigate(Direction::Left),
            EventSource::Keyboard,
        )
        .unwrap();
        assert_eq!(
            host.app.state().interaction().active_surface(),
            SurfaceId::SaveLoadSettings
        );
        host.show_page(HostedSynthPage::MidiSettings, EventSource::System)
            .unwrap();
        assert_eq!(
            host.app.state().interaction().active_surface(),
            SurfaceId::MidiDeviceSettings
        );
        assert_eq!(host.capture_saved_session().unwrap(), saved);
    }

    #[test]
    fn hosted_device_metadata_follows_only_the_acknowledged_active_input() {
        use crate::control::{
            MidiConnectionRequestId, MidiConnectionRevision, MidiInputDeviceId, MidiInputScanId,
        };
        let (mut app, _audio) = app();
        let identity = MidiInputDeviceId::new("fixture-v1", "opaque-17").unwrap();
        let descriptor =
            MidiInputDescriptor::new(identity.clone(), "Performance keyboard", None).unwrap();
        let request_id = MidiConnectionRequestId::FIRST;
        let revision = MidiConnectionRevision::FIRST;
        let host = HostedSynthControl::new(&mut app, false);
        assert!(host.active_midi_input_descriptor().is_none());
        for event in [
            AppEvent::MidiInputScanStarted,
            AppEvent::MidiInputScanSucceeded {
                scan_id: MidiInputScanId::FIRST,
                descriptors: vec![descriptor.clone()],
            },
            AppEvent::MidiInputConnectRequested {
                identity: identity.clone(),
            },
            AppEvent::MidiInputConnectionPrepared {
                request_id,
                revision,
            },
        ] {
            host.app.dispatch_from(event, EventSource::System).unwrap();
            assert!(host.active_midi_input_descriptor().is_none());
        }
        host.app
            .dispatch_from(
                AppEvent::MidiInputActivationAcknowledged {
                    request_id,
                    revision,
                },
                EventSource::System,
            )
            .unwrap();
        assert_eq!(host.active_midi_input_descriptor(), Some(&descriptor));
        host.app
            .dispatch_from(
                AppEvent::MidiInputDisconnectRequested { identity },
                EventSource::System,
            )
            .unwrap();
        assert!(host.active_midi_input_descriptor().is_none());
    }

    #[test]
    fn owned_expression_query_is_exact_read_only_and_enforces_native_pitch_admission() {
        let (mut app, mut audio) = app();
        let host = HostedSynthControl::new(&mut app, false);
        let before = host.capture_saved_session().unwrap();
        let generation = host.projection().generation();
        let id = PatchId::new(7).unwrap();
        let profile = host.owned_expression_profile(id).unwrap();
        assert_eq!(profile.patch_id(), id);
        assert_eq!(profile.graph_revision(), host.active_graph_revision());
        assert_eq!(
            profile.controls(),
            &[
                OwnedExpressionControl::Raw14PitchBend,
                OwnedExpressionControl::Rpn0BendRange,
                OwnedExpressionControl::AbsoluteCc74Timbre,
                OwnedExpressionControl::Cc64SustainOverride,
                OwnedExpressionControl::ChannelPressureGain,
                OwnedExpressionControl::KeyPressureGain,
            ]
        );
        for (note, raw, semitones, cents) in [
            (0, 8192, 2, 0),
            (127, 8192, 2, 0),
            (60, 16383, 48, 99),
            (60, 12288, 14, 50),
        ] {
            profile.validate_pitch(note, raw, semitones, cents).unwrap();
        }
        for (note, raw, semitones, cents) in [
            (128, 8192, 2, 0),
            (60, 16384, 2, 0),
            (60, 8192, 128, 0),
            (60, 8192, 2, 100),
            (0, 8191, 2, 0),
            (127, 16383, 2, 0),
        ] {
            assert!(profile.validate_pitch(note, raw, semitones, cents).is_err());
        }
        assert!(matches!(
            host.owned_expression_profile(PatchId::new(99).unwrap()),
            Err(HostedSynthError::Control(EventRejection::UnknownPatch))
        ));
        assert_eq!(host.projection().generation(), generation);
        assert_eq!(host.capture_saved_session().unwrap(), before);
        assert_eq!(audio.pop_command(), None);
    }

    #[test]
    fn owned_expression_query_rejects_a_document_gate_without_claiming_readiness() {
        let (mut app, _) = app();
        let host = HostedSynthControl::new(&mut app, true);
        let id = PatchId::new(7).unwrap();
        assert!(matches!(host.owned_expression_profile(id),
            Err(HostedSynthError::OwnedExpressionNotReady { patch_id }) if patch_id == id));
    }

    #[test]
    fn owned_expression_profile_separates_master_semantics_and_validates_exact_combined_pitch() {
        let (mut app, _) = app();
        let host = HostedSynthControl::new(&mut app, false);
        let profile = host
            .owned_expression_profile(PatchId::new(7).unwrap())
            .unwrap();
        assert_eq!(profile.member_controls(), profile.controls());
        assert_eq!(
            profile.master_controls(),
            &[
                OwnedExpressionControl::Raw14PitchBend,
                OwnedExpressionControl::Rpn0BendRange,
                OwnedExpressionControl::Cc64SustainOverride,
                OwnedExpressionControl::ChannelPressureGain,
            ]
        );
        let serialized = serde_json::to_value(profile).unwrap();
        assert_eq!(
            serialized["masterControls"],
            serde_json::json!([
                "raw14PitchBend",
                "rpn0BendRange",
                "cc64SustainOverride",
                "channelPressureGain"
            ])
        );
        for (note, member, master) in [
            (60, (12_288, 48, 50), (4_096, 12, 0)),
            (0, (0, 2, 0), (12_288, 4, 0)),
            (60, (8_209, 2, 0), (8_209, 2, 0)),
            (127, (12_288, 1, 0), (12_224, 1, 0)),
        ] {
            profile
                .validate_combined_pitch(note, member, master)
                .unwrap();
        }
        for (note, member, master) in [
            (60, (12_288, 100, 0), (12_288, 100, 0)),
            (0, (8_191, 2, 0), (8_192, 2, 0)),
            (127, (12_288, 1, 0), (12_225, 1, 0)),
            (60, (8_192, 2, 0), (16_384, 2, 0)),
            (60, (8_192, 2, 0), (8_192, 128, 0)),
            (60, (8_192, 2, 0), (8_192, 2, 100)),
        ] {
            assert!(profile
                .validate_combined_pitch(note, member, master)
                .is_err());
        }
    }

    #[test]
    fn initialized_attacks_reject_combined_pitch_before_reducer_or_queue_mutation() {
        use crate::synth::OwnedExpressionState;

        let (mut app, mut audio) = app();
        let mut host = HostedSynthControl::new(&mut app, false);
        let patch = PatchId::new(7).unwrap();
        let voice = MidiVoiceId::new(1).unwrap();
        let saved = host.capture_saved_session().unwrap();
        let generation = host.projection().generation();
        let records = host.app.event_log_ref().records().len();
        for (pitch, member, master) in [
            (60, (14_336, 120, 0), (14_336, 120, 0)),
            (0, (8_191, 2, 0), (8_192, 2, 0)),
            (127, (12_288, 1, 0), (12_225, 1, 0)),
        ] {
            let attack = MidiMessage::try_new(
                MidiChannel::new(0).unwrap(),
                MidiMessageKind::NoteOn,
                pitch,
                100,
            )
            .unwrap();
            let initial = OwnedVoiceInitialization::new(
                attack,
                OwnedExpressionState::new(member.0, member.1, member.2, Some(96), Some(25), 0)
                    .unwrap(),
                OwnedExpressionState::new(master.0, master.1, master.2, Some(64), None, 127)
                    .unwrap(),
            )
            .unwrap();
            for result in [
                host.send_initialized_monitor_voice(patch, voice, initial),
                host.schedule_initialized_voice(100, patch, voice, initial),
            ] {
                assert!(matches!(
                    result,
                    Err(HostedSynthError::OwnedCombinedPitch(_))
                ));
            }
        }
        assert_eq!(host.projection().generation(), generation);
        assert_eq!(host.app.event_log_ref().records().len(), records);
        assert_eq!(host.capture_saved_session().unwrap(), saved);
        assert_eq!(audio.pop_command(), None);
        assert_eq!(
            audio.poll_scheduled_before(u64::MAX),
            ScheduledEventPoll::Empty
        );

        // Rejected frame 100 never advances the ordered queue's admission fence.
        let valid = OwnedVoiceInitialization::new(
            note(),
            OwnedExpressionState::default(),
            OwnedExpressionState::default(),
        )
        .unwrap();
        host.schedule_initialized_voice(5, patch, voice, valid)
            .unwrap();
        assert!(matches!(
            audio.poll_scheduled_before(6),
            ScheduledEventPoll::Ready(_)
        ));
    }

    #[test]
    fn initialized_opposing_bends_publish_one_complete_command_per_source() {
        use crate::synth::OwnedExpressionState;

        let (mut app, mut audio) = app();
        let mut host = HostedSynthControl::new(&mut app, false);
        let patch = PatchId::new(7).unwrap();
        let voice = MidiVoiceId::new(1).unwrap();
        let saved = host.capture_saved_session().unwrap();
        let generation = host.projection().generation();
        let profile = host.owned_expression_profile(patch).unwrap();
        let member = OwnedExpressionState::new(14_336, 120, 0, Some(96), Some(25), 0).unwrap();
        let master = OwnedExpressionState::new(2_048, 120, 0, Some(64), None, 127).unwrap();
        assert!(profile
            .validate_pitch(60, member.raw14(), member.semitones(), member.cents())
            .is_err());
        assert!(profile
            .validate_pitch(60, master.raw14(), master.semitones(), master.cents())
            .is_err());
        let initial = OwnedVoiceInitialization::new(note(), member, master).unwrap();
        host.send_initialized_monitor_voice(patch, voice, initial)
            .unwrap();
        host.schedule_initialized_voice(5, patch, voice, initial)
            .unwrap();
        assert_eq!(
            audio.pop_command(),
            Some(AudioCommand::InitializedVoice {
                source: MidiSource::Monitor,
                patch_id: patch,
                voice,
                initial
            })
        );
        assert_eq!(audio.pop_command(), None);
        assert_eq!(
            audio.poll_scheduled_before(6),
            ScheduledEventPoll::Ready(ScheduledEvent::initialized_voice(
                5,
                host.schedule_epoch(),
                host.active_graph_revision(),
                patch,
                voice,
                initial
            ))
        );
        assert_eq!(
            audio.poll_scheduled_before(u64::MAX),
            ScheduledEventPoll::Empty
        );
        assert_eq!(host.projection().generation(), generation + 2);
        assert_eq!(host.capture_saved_session().unwrap(), saved);
    }

    struct ExpressionRig {
        app: AppLoop<LockFreeControlHandle>,
        renderer: crate::real_time::AudioRenderer<LockFreeAudioHandle,
            crate::adapter::lock_free_structural_graph_boundary::LockFreeStructuralAudioHandle>,
        worker: crate::testing::deterministic_graph_preparation_worker::DeterministicGraphPreparationHandle,
    }

    impl ExpressionRig {
        fn new() -> Self {
            use crate::adapter::braids_preparer::BraidsPreparer;
            use crate::adapter::lock_free_structural_graph_boundary::LockFreeStructuralGraphBoundary;
            use crate::real_time::structural_graph_boundary::StructuralGraphBoundary;
            use crate::real_time::{GraphHandoffStatus, PreparedGraphBuilder};
            use crate::shell::audio_output::{AudioDeviceConfig, AudioSampleFormat};
            use crate::synth::{DescriptorDefaultConfigFactory, InstrumentPreparer};
            use crate::testing::deterministic_graph_preparation_worker::DeterministicGraphPreparationWorker;
            let braids = BraidsCapability::new().unwrap();
            let other = crate::adapter::upstream_audio::instrument_ports()
                .unwrap()
                .into_iter()
                .find(|provider| {
                    provider.descriptor().id().as_str() == "instrument.daisy.analogbassdrum"
                })
                .unwrap();
            let registry =
                CapabilityRegistry::new(vec![braids.descriptor(), other.descriptor()]).unwrap();
            let mut state = AppState::new(registry.clone(), GlobalParameters::new(0.0).unwrap());
            state
                .apply(AppEvent::InstallPatches(vec![Patch::new(
                    PatchId::new(7).unwrap(),
                    "Profile witness".into(),
                    braids.default_config().unwrap(),
                    MidiChannel::new(0).unwrap(),
                    PatchOutput::default(),
                )]))
                .unwrap();
            let initial = StateProjector::new().project(&state).unwrap().2;
            let (control, audio) = LockFreeAudioBoundary::new(8, initial).into_handles();
            let mut app = AppLoop::new(state, StateProjector::new(), control).unwrap();
            let preparers = || -> Vec<Box<dyn InstrumentPreparer>> {
                vec![
                    Box::new(BraidsPreparer::new().unwrap()),
                    Box::new(other.clone()),
                ]
            };
            let graph = PreparedGraphBuilder::new(&registry, &preparers())
                .build(
                    GraphRevision::INITIAL,
                    app.patches(),
                    app.current_parameters().clone(),
                    48_000.0,
                    64,
                )
                .unwrap();
            let config = AudioDeviceConfig::new(48_000.0, 2, AudioSampleFormat::F32, 64).unwrap();
            let structural = LockFreeStructuralGraphBoundary::new(
                1,
                1,
                GraphHandoffStatus::with_active(GraphRevision::INITIAL),
            )
            .unwrap();
            let (structural_control, structural_audio) = structural.into_handles();
            let worker =
                DeterministicGraphPreparationWorker::new(registry.clone(), preparers(), config);
            let handle = worker.advance_handle();
            app.configure_engine_selection(
                DescriptorDefaultConfigFactory::new(
                    registry,
                    vec![Box::new(braids), Box::new(other)],
                ),
                worker,
                structural_control,
                &graph,
                config,
            )
            .unwrap();
            let renderer = crate::real_time::AudioRenderer::new(audio, structural_audio, graph);
            Self {
                app,
                renderer,
                worker: handle,
            }
        }

        fn query(&mut self) -> Result<OwnedExpressionProfile, HostedSynthError> {
            HostedSynthControl::new(&mut self.app, false)
                .owned_expression_profile(PatchId::new(7).unwrap())
        }

        fn request(&mut self, direction: Direction) {
            self.app
                .dispatch_from(
                    AppEvent::SelectContext(TopLevelContext::Patch),
                    EventSource::Keyboard,
                )
                .unwrap();
            self.app
                .dispatch_from(AppEvent::Adjust(direction), EventSource::Keyboard)
                .unwrap();
            assert_eq!(
                self.app.engine_selection_status().kind(),
                EngineSelectionStatusKind::Loading
            );
        }

        fn prepare(&mut self) {
            for expected in [
                EngineSelectionStatusKind::Validating,
                EngineSelectionStatusKind::Preparing,
            ] {
                self.app.advance_structural().unwrap();
                assert_eq!(self.app.engine_selection_status().kind(), expected);
                assert!(matches!(
                    self.query(),
                    Err(HostedSynthError::OwnedExpressionNotReady { .. })
                ));
            }
            assert!(self.worker.advance());
            self.app.advance_structural().unwrap();
        }

        fn activate(&mut self) {
            let mut output = [0.0; 128];
            self.renderer.render(&mut output);
            assert!(matches!(
                self.query(),
                Err(HostedSynthError::OwnedExpressionNotReady { .. })
            ));
            self.app.advance_structural().unwrap();
            assert_eq!(
                self.app.engine_selection_status().kind(),
                EngineSelectionStatusKind::Ready
            );
        }
    }

    #[test]
    fn owned_expression_profile_tracks_actual_graph_activation_and_failed_selection() {
        let mut rig = ExpressionRig::new();
        let original = rig.query().unwrap();
        rig.request(Direction::Right);
        assert!(matches!(
            rig.query(),
            Err(HostedSynthError::OwnedExpressionNotReady { .. })
        ));
        rig.prepare();
        assert_eq!(
            rig.app.engine_selection_status().kind(),
            EngineSelectionStatusKind::Activating
        );
        assert!(matches!(
            rig.query(),
            Err(HostedSynthError::OwnedExpressionNotReady { .. })
        ));
        rig.activate();
        assert!(
            matches!(rig.query(), Err(HostedSynthError::OwnedExpressionUnsupported { patch_id }) if patch_id == original.patch_id())
        );
        assert_eq!(
            rig.app.patches()[0]
                .instrument_config()
                .capability_id()
                .as_str(),
            "instrument.daisy.analogbassdrum"
        );
        rig.request(Direction::Left);
        rig.prepare();
        rig.activate();
        let restored = rig.query().unwrap();
        assert_eq!(restored.patch_id(), original.patch_id());
        assert_ne!(restored.graph_revision(), original.graph_revision());
        assert_eq!(restored.graph_revision(), rig.renderer.active_revision());
        rig.request(Direction::Right);
        rig.worker
            .fail_next(crate::control::EngineSelectionFailure::AssetUnavailable);
        rig.prepare();
        assert_eq!(
            rig.app.engine_selection_status().kind(),
            EngineSelectionStatusKind::Unavailable
        );
        assert!(matches!(
            rig.query(),
            Err(HostedSynthError::OwnedExpressionNotReady { .. })
        ));
    }

    #[test]
    fn scheduled_send_start_end_and_cancellation_preserve_saved_session() {
        use crate::mixer::bus_id::BusId;
        use crate::real_time::{SendLease, SendLeaseId};

        let (mut app, mut audio) = app();
        let mut host = HostedSynthControl::new(&mut app, true);
        let before = host.capture_saved_session().unwrap();
        let generation = host.projection().generation();
        let patch = PatchId::new(7).unwrap();
        let id = SendLeaseId::new(1).unwrap();
        let lease = SendLease::new(id, BusId::default(), 0.6, u64::MAX).unwrap();
        host.schedule_send_lease(5, patch, lease).unwrap();
        host.schedule_end_send_lease(11, patch, id).unwrap();
        assert_eq!(audio.pop_command(), None);
        assert_eq!(
            audio.poll_scheduled_before(5),
            ScheduledEventPoll::Future { absolute_frame: 5 }
        );
        assert_eq!(
            audio.poll_scheduled_before(6),
            ScheduledEventPoll::Ready(ScheduledEvent::send_lease(
                5,
                host.schedule_epoch(),
                host.active_graph_revision(),
                patch,
                lease
            ))
        );
        assert_eq!(
            audio.poll_scheduled_before(12),
            ScheduledEventPoll::Ready(ScheduledEvent::end_send_lease(
                11,
                host.schedule_epoch(),
                host.active_graph_revision(),
                patch,
                id
            ))
        );
        assert_eq!(host.projection().generation(), generation);
        assert_eq!(host.capture_saved_session().unwrap(), before);

        host.schedule_send_lease(16, patch, lease).unwrap();
        host.cancel_scheduled().unwrap();
        assert_eq!(audio.poll_scheduled_before(17), ScheduledEventPoll::Stale);
        assert_eq!(host.capture_saved_session().unwrap(), before);
    }

    #[test]
    fn invalid_scheduled_send_targets_intervals_and_queue_pressure_fail_without_saved_edits() {
        use crate::mixer::bus_id::BusId;
        use crate::real_time::{SendLease, SendLeaseError, SendLeaseId};

        let (mut app, mut audio) = app();
        let mut host = HostedSynthControl::new(&mut app, false);
        let before = host.capture_saved_session().unwrap();
        let patch = PatchId::new(1).unwrap();
        let id = SendLeaseId::new(1).unwrap();
        let valid = SendLease::new(id, BusId::default(), 0.6, u64::MAX).unwrap();
        assert!(matches!(
            host.schedule_send_lease(0, PatchId::new(99).unwrap(), valid),
            Err(HostedSynthError::Scheduled(
                ScheduledEventDispatchError::Control(EventRejection::UnknownPatch)
            ))
        ));
        let bad_bus = SendLease::new(id, BusId::new(u16::MAX).unwrap(), 0.6, u64::MAX).unwrap();
        assert!(matches!(
            host.schedule_send_lease(0, patch, bad_bus),
            Err(HostedSynthError::Scheduled(
                ScheduledEventDispatchError::UnknownSendBus
            ))
        ));
        let expired = SendLease::new(id, BusId::default(), 0.6, 1).unwrap();
        assert!(matches!(
            host.schedule_send_lease(1, patch, expired),
            Err(HostedSynthError::Scheduled(
                ScheduledEventDispatchError::Lease(SendLeaseError::AlreadyExpired { .. })
            ))
        ));
        assert_eq!(
            audio.poll_scheduled_before(u64::MAX),
            ScheduledEventPoll::Empty
        );
        for frame in 0..8 {
            host.schedule_send_lease(frame, patch, valid).unwrap();
        }
        assert!(
            matches!(host.schedule_end_send_lease(8, patch, id), Err(HostedSynthError::Scheduled(ScheduledEventDispatchError::Publication(error))) if error.reason() == ScheduledEventRejectionReason::Full)
        );
        host.cancel_scheduled().unwrap();
        assert_eq!(host.capture_saved_session().unwrap(), before);
        assert_eq!(audio.pop_command(), None);
    }

    #[test]
    fn hosted_targets_preserve_identity_and_explicit_channel_fanout() {
        let (mut app, mut audio) = app();
        let mut host = HostedSynthControl::new(&mut app, false);
        let targets = host.installed_patches();
        assert_eq!(
            targets.iter().map(HostedPatch::name).collect::<Vec<_>>(),
            ["Sound 1", "Sound 7"]
        );
        assert_eq!(targets[1].id(), PatchId::new(7).unwrap());
        assert_eq!(targets[1].channel(), MidiChannel::new(0).unwrap());
        host.send_midi(targets[1].id(), note()).unwrap();
        assert_eq!(
            audio.pop_command(),
            Some(AudioCommand::patch_midi(targets[1].id(), note()))
        );
        assert_eq!(audio.pop_command(), None);
        assert_eq!(host.send_channel_midi(note()).unwrap(), 2);
        assert_eq!(
            audio.pop_command(),
            Some(AudioCommand::patch_midi(targets[0].id(), note()))
        );
        assert_eq!(
            audio.pop_command(),
            Some(AudioCommand::patch_midi(targets[1].id(), note()))
        );
        assert_eq!(audio.pop_command(), None);
    }

    #[test]
    fn hosted_monitoring_publishes_owned_midi_and_scoped_release() {
        let (mut app, mut audio) = app();
        let mut host = HostedSynthControl::new(&mut app, false);
        let before = host.capture_saved_session().unwrap();
        let patch_id = PatchId::new(7).unwrap();
        host.send_monitor_midi(patch_id, note()).unwrap();
        assert_eq!(
            audio.pop_command(),
            Some(AudioCommand::OwnedMidi {
                source: MidiSource::Monitor,
                patch_id,
                message: note(),
            })
        );
        host.cancel_monitor(patch_id).unwrap();
        assert_eq!(
            audio.pop_command(),
            Some(AudioCommand::ReleaseSource {
                source: MidiSource::Monitor,
                patch_id,
            })
        );
        assert!(matches!(
            host.send_monitor_midi(PatchId::new(99).unwrap(), note()),
            Err(HostedSynthError::Control(EventRejection::UnknownPatch))
        ));
        assert_eq!(audio.pop_command(), None);
        assert_eq!(host.capture_saved_session().unwrap(), before);
    }

    #[test]
    fn owned_monitor_and_scheduled_expression_keep_raw_scope_and_identity() {
        let (mut app, mut audio) = app();
        let mut host = HostedSynthControl::new(&mut app, false);
        let before = host.capture_saved_session().unwrap();
        let patch = PatchId::new(7).unwrap();
        let voice = crate::synth::MidiVoiceId::new(91).unwrap();
        let scope = crate::synth::MidiExpressionScope::Master;
        let bend = MidiMessage::try_new(
            MidiChannel::new(2).unwrap(),
            crate::kernel::midi_message::MidiMessageKind::PitchBend,
            37,
            71,
        )
        .unwrap();
        host.send_owned_monitor_midi(patch, voice, scope, bend)
            .unwrap();
        assert_eq!(
            audio.pop_command(),
            Some(AudioCommand::VoiceMidi {
                source: MidiSource::Monitor,
                patch_id: patch,
                voice,
                scope,
                message: bend,
            })
        );
        host.schedule_scoped_voice_midi(128, patch, bend, Some(voice), scope)
            .unwrap();
        assert_eq!(
            audio.poll_scheduled_before(129),
            ScheduledEventPoll::Ready(
                ScheduledEvent::new(
                    128,
                    host.schedule_epoch(),
                    host.active_graph_revision(),
                    patch,
                    bend
                )
                .with_voice(Some(voice))
                .with_expression_scope(scope)
            )
        );
        assert_eq!(audio.pop_command(), None);
        assert_eq!(host.capture_saved_session().unwrap(), before);
    }

    #[test]
    fn scheduled_midi_crosses_reducer_and_timed_queue_without_immediate_delivery() {
        let (mut app, mut audio) = app();
        let mut host = HostedSynthControl::new(&mut app, false);
        let before = host.capture_saved_session().unwrap();
        let generation = host.projection().generation();
        let id = PatchId::new(7).unwrap();
        host.schedule_midi(4096, id, note()).unwrap();
        assert_eq!(host.projection().generation(), generation + 1);
        assert_eq!(host.capture_saved_session().unwrap(), before);
        assert_eq!(audio.pop_command(), None);
        assert_eq!(
            audio.poll_scheduled_before(4096),
            ScheduledEventPoll::Future {
                absolute_frame: 4096
            }
        );
        assert_eq!(
            audio.poll_scheduled_before(4097),
            ScheduledEventPoll::Ready(ScheduledEvent::new(
                4096,
                host.schedule_epoch(),
                host.active_graph_revision(),
                id,
                note(),
            ))
        );
    }

    #[test]
    fn invalid_target_graph_and_epoch_do_not_change_state_or_publish() {
        let (mut app, mut audio) = app();
        let before = app.current_graphical_shell();
        let epoch = app.scheduled_epoch();
        let revision = app.active_audio_graph_revision();
        let invalid_target =
            ScheduledEvent::new(100, epoch, revision, PatchId::new(99).unwrap(), note());
        assert_eq!(
            app.dispatch_scheduled_midi_from(invalid_target, EventSource::AutomaticMidi),
            Err(ScheduledEventDispatchError::Control(
                EventRejection::UnknownPatch
            ))
        );
        let invalid_graph = ScheduledEvent::new(
            100,
            epoch,
            GraphRevision::new(99).unwrap(),
            PatchId::new(1).unwrap(),
            note(),
        );
        assert!(matches!(
            app.dispatch_scheduled_midi_from(invalid_graph, EventSource::AutomaticMidi),
            Err(ScheduledEventDispatchError::GraphRevision { .. })
        ));
        let invalid_epoch =
            ScheduledEvent::new(100, epoch + 1, revision, PatchId::new(1).unwrap(), note());
        assert!(
            matches!(app.dispatch_scheduled_midi_from(invalid_epoch, EventSource::AutomaticMidi),
            Err(ScheduledEventDispatchError::Publication(error))
                if matches!(error.reason(), ScheduledEventRejectionReason::StaleEpoch { .. }))
        );
        assert_eq!(app.current_graphical_shell(), before);
        assert_eq!(audio.pop_command(), None);
        assert_eq!(
            audio.poll_scheduled_before(u64::MAX),
            ScheduledEventPoll::Empty
        );
    }

    #[test]
    fn rejected_ordering_stays_typed_and_cancellation_changes_epoch() {
        let (mut app, mut audio) = app();
        let mut host = HostedSynthControl::new(&mut app, false);
        let id = PatchId::new(1).unwrap();
        host.schedule_midi(1000, id, note()).unwrap();
        assert!(matches!(host.schedule_midi(999, id, note()),
            Err(HostedSynthError::Scheduled(ScheduledEventDispatchError::Publication(error)))
                if matches!(error.reason(), ScheduledEventRejectionReason::OutOfOrder { last_frame: 1000 })));
        let epoch = host.schedule_epoch();
        assert_eq!(host.cancel_scheduled().unwrap(), epoch + 1);
        assert_eq!(
            audio.pop_command(),
            None,
            "cancel must preserve unrelated live voices"
        );
        assert_eq!(audio.poll_scheduled_before(1001), ScheduledEventPoll::Stale);
        host.schedule_midi(900, id, note()).unwrap();
        assert!(
            matches!(audio.poll_scheduled_before(901), ScheduledEventPoll::Ready(event) if event.epoch() == epoch + 1)
        );
    }

    #[test]
    fn document_edit_gate_keeps_navigation_and_performance_available() {
        let (mut app, mut audio) = app();
        let mut host = HostedSynthControl::new(&mut app, true);
        let before = host.capture_saved_session().unwrap();
        assert!(matches!(
            host.dispatch_action(
                SemanticAction::Adjust(Direction::Right),
                EventSource::Keyboard
            ),
            Err(HostedSynthError::EditsBlocked)
        ));
        host.dispatch_action(
            SemanticAction::SelectContext(TopLevelContext::Patch),
            EventSource::Keyboard,
        )
        .unwrap();
        host.send_midi(PatchId::new(1).unwrap(), note()).unwrap();
        assert_eq!(host.capture_saved_session().unwrap(), before);
        assert_eq!(
            audio.pop_command(),
            Some(AudioCommand::patch_midi(PatchId::new(1).unwrap(), note()))
        );
    }

    #[test]
    fn sound_capture_and_failed_install_leave_the_active_session_untouched() {
        let (mut app, mut audio) = app();
        let mut host = HostedSynthControl::new(&mut app, false);
        let before = host.capture_saved_session().unwrap();
        let target = PatchId::new(7).unwrap();
        let missing = PatchId::new(99).unwrap();
        let definition = host.capture_sound_definition(target).unwrap();
        assert_eq!(definition, before.sound_definition(target).unwrap());
        assert!(matches!(host.capture_sound_definition(missing),
            Err(HostedSynthError::SoundDefinition(SoundDefinitionError::UnknownPatch(id))) if id == missing));
        assert!(
            matches!(host.request_install_sound_definition(missing, definition.clone()),
            Err(HostedSynthError::SoundDefinition(SoundDefinitionError::UnknownPatch(id))) if id == missing)
        );
        assert!(matches!(
            host.request_install_sound_definition(target, definition),
            Err(HostedSynthError::LifecycleUnavailable)
        ));
        assert_eq!(host.capture_saved_session().unwrap(), before);
        assert_eq!(audio.pop_command(), None);
    }

    #[test]
    fn sound_install_obeys_the_existing_persisted_edit_gate() {
        let (mut app, mut audio) = app();
        let mut host = HostedSynthControl::new(&mut app, true);
        let before = host.capture_saved_session().unwrap();
        let target = PatchId::new(1).unwrap();
        let definition = host.capture_sound_definition(target).unwrap();
        assert!(matches!(
            host.request_install_sound_definition(target, definition),
            Err(HostedSynthError::EditsBlocked)
        ));
        assert_eq!(host.capture_saved_session().unwrap(), before);
        assert_eq!(audio.pop_command(), None);
    }

    #[test]
    fn restore_without_owned_lifecycle_fails_without_changing_saved_state() {
        let (mut app, _) = app();
        let mut host = HostedSynthControl::new(&mut app, false);
        let before = host.capture_saved_session().unwrap();
        assert!(matches!(
            host.request_restore(before.clone()),
            Err(HostedSynthError::LifecycleUnavailable)
        ));
        assert!(matches!(
            host.replacement_status(),
            Err(HostedSynthError::LifecycleUnavailable)
        ));
        assert_eq!(host.capture_saved_session().unwrap(), before);
    }
}
