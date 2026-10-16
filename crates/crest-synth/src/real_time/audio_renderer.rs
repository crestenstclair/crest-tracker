use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crate::kernel::patch_id::PatchId;
use crate::mixer::mix_observation::MixObservation;
use crate::mixer::track_meter::TrackMeter;
use crate::real_time::audio_boundary::AudioThreadBoundary;
use crate::real_time::audio_command::AudioCommand;
use crate::real_time::audio_observation::{CallbackAudioObservation, DiscardAudioObservation};
use crate::real_time::audio_observation_snapshot::AudioObservationSnapshot;
use crate::real_time::graph_handoff_status::GraphHandoffStatus;
use crate::real_time::graph_revision::GraphRevision;
use crate::real_time::parameter_snapshot::MAX_ACTIVE_PATCHES;
use crate::real_time::prepared_engine_rack::RackDispatchError;
use crate::real_time::prepared_graph::{PreparedGraph, PreviewAudioObservation};
use crate::real_time::scheduled_event::{ScheduledEvent, ScheduledEventPoll, ScheduledPayload};
use crate::real_time::structural_graph_boundary::AudioStructuralGraphBoundary;
use crate::real_time::PatchEffectObservation;
use crate::real_time::{SendLeaseError, SendLeases};
use crate::synth::prepared_instrument::{MidiSource, PreparedInstrumentError};

/// Default ordered-command budget per audio block. A producer can
/// refill an SPSC queue while it is drained, so queue capacity alone does not
/// bound callback work. Excess commands remain queued in order, including
/// note-offs and the reserved all-notes-off recovery command.
const DEFAULT_AUDIO_COMMANDS_PER_BLOCK: usize = 1024;
/// Hard-real-time owner of one active complete prepared graph.
///
/// Construction receives every engine, effect, stem, route, and scratch owner
/// already prepared. `render` performs only bounded graph handoff, command
/// dispatch, compatible scalar selection, synthesis, mixing, and observation.
pub struct AudioRenderer<Boundary, Structural, Observation = DiscardAudioObservation> {
    boundary: Boundary,
    structural: Structural,
    observation: Observation,
    active_graph: PreparedGraph,
    pending_retirement: Option<PreparedGraph>,
    parameters: crate::real_time::parameter_snapshot::ParameterSnapshot,
    candidate_parameters: crate::real_time::parameter_snapshot::ParameterSnapshot,
    candidate_pending: bool,
    handoff_status: GraphHandoffStatus,
    active_notes: SourceNoteObservation,
    scheduled_patches: [Option<PatchId>; MAX_ACTIVE_PATCHES],
    scheduled_epoch: u64,
    send_leases: SendLeases,
    pending_scheduled: Option<ScheduledEvent>,
    rendered_blocks: u64,
    rendered_frames: u64,
    max_commands_per_block: usize,
    commands_consumed: u64,
    routing_failures: u64,
    voice_limit_refusals: u64,
    scheduled_late_events: u64,
    scheduled_max_lateness_frames: u64,
    last_unknown_patch_id: Option<PatchId>,
}

/// Whether this message starts a note.
///
/// MIDI spells a note-off two ways, and only the sounding one is subject to the
/// voice limit: a `NoteOn` at zero velocity releases a note and must never be
/// refused, exactly like an explicit `NoteOff`.
fn starts_a_note(message: crate::kernel::midi_message::MidiMessage) -> bool {
    message.kind() == MidiMessageKind::NoteOn && message.data2() > 0
}

impl<Boundary, Structural> AudioRenderer<Boundary, Structural, DiscardAudioObservation>
where
    Boundary: AudioThreadBoundary,
    Structural: AudioStructuralGraphBoundary,
{
    #[must_use]
    pub fn new(boundary: Boundary, structural: Structural, initial_graph: PreparedGraph) -> Self {
        Self::with_observation(boundary, structural, initial_graph, DiscardAudioObservation)
    }
}

impl<Boundary, Structural, Observation> AudioRenderer<Boundary, Structural, Observation>
where
    Boundary: AudioThreadBoundary,
    Structural: AudioStructuralGraphBoundary,
    Observation: CallbackAudioObservation,
{
    #[must_use]
    pub fn with_observation(
        boundary: Boundary,
        mut structural: Structural,
        initial_graph: PreparedGraph,
        observation: Observation,
    ) -> Self {
        let parameters = initial_graph.initial_parameters().clone();
        let handoff_status = GraphHandoffStatus::with_active(initial_graph.revision());
        let scheduled_epoch = boundary.scheduled_epoch();
        structural.publish_status_on_audio(handoff_status);
        Self {
            boundary,
            structural,
            observation,
            active_graph: initial_graph,
            pending_retirement: None,
            candidate_parameters: parameters.clone(),
            candidate_pending: false,
            parameters,
            handoff_status,
            active_notes: SourceNoteObservation::new(),
            scheduled_patches: [None; MAX_ACTIVE_PATCHES],
            scheduled_epoch,
            send_leases: SendLeases::new(
                core::num::NonZeroUsize::new(DEFAULT_AUDIO_COMMANDS_PER_BLOCK)
                    .expect("positive default budget"),
            )
            .expect("send lease storage allocation"),
            pending_scheduled: None,
            rendered_blocks: 0,
            rendered_frames: 0,
            max_commands_per_block: DEFAULT_AUDIO_COMMANDS_PER_BLOCK,
            commands_consumed: 0,
            routing_failures: 0,
            voice_limit_refusals: 0,
            scheduled_late_events: 0,
            scheduled_max_lateness_frames: 0,
            last_unknown_patch_id: None,
        }
    }

    /// Sets the per-block event budget on the control side before device start.
    /// Tune against measured block deadlines; excess events stay queued in order.
    #[must_use]
    pub fn with_command_budget(mut self, commands: core::num::NonZeroUsize) -> Self {
        self.max_commands_per_block = commands.get();
        self
    }

    /// Configures overlap admission before starting the device. Storage never grows in render.
    pub fn with_send_lease_capacity(
        mut self,
        capacity: core::num::NonZeroUsize,
    ) -> Result<Self, SendLeaseError> {
        self.send_leases = SendLeases::new(capacity)?;
        Ok(self)
    }

    /// Applies structural ownership only at block start, drains ready commands,
    /// consumes at most one compatible latest scalar snapshot, and renders the
    /// active graph into interleaved stereo output.
    pub fn render(&mut self, interleaved_stereo: &mut [f32]) {
        interleaved_stereo.fill(0.0);
        let capacity_samples = self.active_graph.max_frames().saturating_mul(2);
        if capacity_samples == 0 {
            return;
        }
        let usable_samples = interleaved_stereo.len() - interleaved_stereo.len() % 2;
        for block in interleaved_stereo[..usable_samples].chunks_mut(capacity_samples) {
            self.render_bounded_block(block);
        }
    }

    fn render_bounded_block(&mut self, interleaved_stereo: &mut [f32]) {
        self.apply_structural_handoff();
        self.select_latest_parameters();
        let epoch = self.boundary.scheduled_epoch();
        if epoch != self.scheduled_epoch {
            self.release_scheduled_notes();
            self.scheduled_epoch = epoch;
        }
        let remaining_commands = self.drain_immediate_commands();
        let frame_count = interleaved_stereo.len() / 2;
        if frame_count == 0 {
            return;
        }
        let mut observation = None;
        if !self.render_scheduled_block(interleaved_stereo, remaining_commands, &mut observation) {
            interleaved_stereo.fill(0.0);
            self.publish_failed_block(frame_count);
            return;
        }
        if let Some(observation) = observation {
            self.publish_rendered_block(observation);
        }
    }

    fn select_latest_parameters(&mut self) {
        // Select the newest compatible complete projection before command
        // delivery so note-on/note-off latching observes this generation.
        self.candidate_pending |= self
            .boundary
            .exchange_latest_parameters(&mut self.candidate_parameters);
        let latest = &self.candidate_parameters;
        let compatible = latest.graph_revision() == self.active_graph.revision()
            && self.active_graph.engine_rack().matches_parameters(latest)
            && self.active_graph.effect_rack().matches_parameters(latest)
            && self
                .active_graph
                .bus_return_rack()
                .matches_parameters(latest);
        if self.candidate_pending && compatible {
            core::mem::swap(&mut self.parameters, &mut self.candidate_parameters);
            self.candidate_pending = false;
        } else if self.candidate_pending {
            self.handoff_status.record_incompatible_snapshot();
        }
        self.structural.publish_status_on_audio(self.handoff_status);
    }

    fn drain_immediate_commands(&mut self) -> usize {
        let mut remaining = self.max_commands_per_block;
        while remaining > 0 {
            let Some(command) = self.boundary.pop_command() else {
                break;
            };
            remaining -= 1;
            self.commands_consumed = self.commands_consumed.saturating_add(1);
            match command {
                AudioCommand::PatchMidi { patch_id, message } => {
                    self.dispatch_patch_midi(MidiSource::Immediate, patch_id, message);
                }
                AudioCommand::InitializedVoice {
                    source,
                    patch_id,
                    voice,
                    initial,
                } => {
                    self.dispatch_initialized_voice(source, patch_id, voice, initial);
                }
                AudioCommand::VoiceMidi {
                    source,
                    patch_id,
                    voice,
                    scope,
                    message,
                } => {
                    self.dispatch_scoped_voice_midi(source, Some(voice), scope, patch_id, message);
                }
                AudioCommand::OwnedMidi {
                    source,
                    patch_id,
                    message,
                } => {
                    self.dispatch_patch_midi(source, patch_id, message);
                }
                AudioCommand::ReleaseSource { source, patch_id } => {
                    if self
                        .active_graph
                        .callback_parts_mut()
                        .0
                        .all_notes_off_for_source(source, patch_id)
                        .is_ok()
                    {
                        self.active_notes.clear_source_patch(source, patch_id);
                    } else {
                        self.routing_failures = self.routing_failures.saturating_add(1);
                    }
                }
                AudioCommand::ReleaseImmediateNotes => self.release_immediate_notes(),
                AudioCommand::AllNotesOff => {
                    self.active_graph.callback_parts_mut().0.all_notes_off();
                    self.active_notes.clear_all();
                    self.scheduled_patches.fill(None);
                    self.send_leases.clear();
                }
                AudioCommand::PreviewStart {
                    patch_id,
                    audition_id,
                } => {
                    if !self.active_graph.preview_start(audition_id, patch_id) {
                        self.routing_failures = self.routing_failures.saturating_add(1);
                    }
                }
                AudioCommand::PreviewStop {
                    patch_id,
                    audition_id,
                } => {
                    if !self.active_graph.preview_stop(audition_id, patch_id) {
                        self.routing_failures = self.routing_failures.saturating_add(1);
                    }
                }
            }
        }
        remaining
    }

    /// Both delivery lanes share the exact same admission and routing rules.
    fn dispatch_patch_midi(
        &mut self,
        source: MidiSource,
        patch_id: PatchId,
        message: MidiMessage,
    ) -> bool {
        self.dispatch_voice_midi(source, None, patch_id, message)
    }
    fn dispatch_voice_midi(
        &mut self,
        source: MidiSource,
        voice: Option<crate::synth::MidiVoiceId>,
        patch_id: PatchId,
        message: MidiMessage,
    ) -> bool {
        self.dispatch_scoped_voice_midi(
            source,
            voice,
            crate::synth::MidiExpressionScope::Member,
            patch_id,
            message,
        )
    }
    fn dispatch_scoped_voice_midi(
        &mut self,
        source: MidiSource,
        voice: Option<crate::synth::MidiVoiceId>,
        scope: crate::synth::MidiExpressionScope,
        patch_id: PatchId,
        message: MidiMessage,
    ) -> bool {
        self.dispatch_owned_delivery(source, voice, scope, patch_id, message, None)
    }
    fn dispatch_initialized_voice(
        &mut self,
        source: MidiSource,
        patch_id: PatchId,
        voice: crate::synth::MidiVoiceId,
        initial: crate::synth::OwnedVoiceInitialization,
    ) -> bool {
        self.dispatch_owned_delivery(
            source,
            Some(voice),
            crate::synth::MidiExpressionScope::Member,
            patch_id,
            initial.message(),
            Some(initial),
        )
    }
    fn dispatch_owned_delivery(
        &mut self,
        source: MidiSource,
        voice: Option<crate::synth::MidiVoiceId>,
        scope: crate::synth::MidiExpressionScope,
        patch_id: PatchId,
        message: MidiMessage,
        initial: Option<crate::synth::OwnedVoiceInitialization>,
    ) -> bool {
        let matching_parameters = self.parameters.patch(patch_id);
        if matches!(
            message.kind(),
            MidiMessageKind::NoteOn | MidiMessageKind::NoteOff
        ) && !self
            .active_graph
            .callback_parts_mut()
            .0
            .accepts_note(patch_id, message.data1())
        {
            return false;
        }
        // A zero-velocity note-on is a release and must never be refused by
        // note admission. Refusing a new voice never steals an existing one.
        if let Some(parameters) = matching_parameters {
            if starts_a_note(message)
                && self.observed_note_count(patch_id)
                    >= parameters.voice_limit().active_note_ceiling()
            {
                self.voice_limit_refusals = self.voice_limit_refusals.saturating_add(1);
                return false;
            }
        }
        let result = matching_parameters.map(|parameters| {
            let rack = self.active_graph.callback_parts_mut().0;
            match (voice, initial) {
                (Some(voice), Some(initial)) => rack
                    .dispatch_initialized_voice_from(source, voice, patch_id, initial, parameters),
                _ => rack.dispatch_scoped_voice_from(
                    source, voice, scope, patch_id, message, parameters,
                ),
            }
        });
        match result {
            Some(Ok(())) => {
                self.active_notes
                    .observe_patch_message(source, patch_id, message);
                if source == MidiSource::Scheduled {
                    self.mark_scheduled_patch(patch_id);
                }
                true
            }
            None | Some(Err(RackDispatchError::UnknownPatch { .. })) => {
                self.routing_failures = self.routing_failures.saturating_add(1);
                self.last_unknown_patch_id = Some(patch_id);
                false
            }
            Some(Err(RackDispatchError::Instrument {
                source: PreparedInstrumentError::UnsupportedMidiSource { .. },
                ..
            })) => {
                self.routing_failures = self.routing_failures.saturating_add(1);
                false
            }
            Some(Err(
                RackDispatchError::Instrument { .. }
                | RackDispatchError::ParameterLayoutMismatch { .. },
            )) => {
                self.routing_failures = self.routing_failures.saturating_add(1);
                false
            }
        }
    }

    fn observed_note_count(&self, patch_id: PatchId) -> u32 {
        self.active_graph
            .engine_rack()
            .active_note_count(patch_id)
            .unwrap_or_else(|| self.active_notes.count_patch(patch_id))
    }
    fn observed_note_total(&self) -> u32 {
        self.parameters
            .patches()
            .iter()
            .filter_map(|p| p.patch_id())
            .map(|id| self.observed_note_count(id))
            .fold(0, u32::saturating_add)
    }
    fn render_scheduled_block(
        &mut self,
        output: &mut [f32],
        command_budget: usize,
        observation: &mut Option<RenderedSegment>,
    ) -> bool {
        let end_frame = self
            .rendered_frames
            .saturating_add((output.len() / 2) as u64);
        let mut rendered_samples = 0;
        for _ in 0..command_budget {
            let polled = self.pending_scheduled.take().map_or_else(
                || self.boundary.poll_scheduled_before(end_frame),
                ScheduledEventPoll::Ready,
            );
            let event = match polled {
                ScheduledEventPoll::Empty | ScheduledEventPoll::Future { .. } => break,
                ScheduledEventPoll::Stale => {
                    self.commands_consumed = self.commands_consumed.saturating_add(1);
                    continue;
                }
                ScheduledEventPoll::Ready(event) => event,
            };
            // If cancellation races a block, preserve a newly admitted event
            // until the next block performs old-epoch note cleanup.
            let epoch = self.boundary.scheduled_epoch();
            if epoch != self.scheduled_epoch && event.epoch() == epoch {
                self.pending_scheduled = Some(event);
                break;
            }
            self.commands_consumed = self.commands_consumed.saturating_add(1);
            if event.epoch() != self.scheduled_epoch || event.epoch() != epoch {
                continue;
            }
            if event.graph_revision() != self.active_graph.revision() {
                self.routing_failures = self.routing_failures.saturating_add(1);
                continue;
            }
            let offset = event
                .absolute_frame()
                .saturating_sub(self.rendered_frames)
                .min((output.len() / 2) as u64) as usize
                * 2;
            let offset = offset.max(rendered_samples);
            if !self.append_rendered_segment(
                &mut output[rendered_samples..offset],
                self.rendered_frames + (rendered_samples / 2) as u64,
                observation,
            ) {
                return false;
            }
            rendered_samples = offset;
            let delivered_frame = self.rendered_frames.saturating_add((offset / 2) as u64);
            let lateness = delivered_frame.saturating_sub(event.absolute_frame());
            if lateness > 0 {
                self.scheduled_late_events = self.scheduled_late_events.saturating_add(1);
                self.scheduled_max_lateness_frames =
                    self.scheduled_max_lateness_frames.max(lateness);
            }
            match event.payload() {
                ScheduledPayload::InitializedVoice { voice, initial } => {
                    self.dispatch_initialized_voice(
                        MidiSource::Scheduled,
                        event.patch_id(),
                        voice,
                        initial,
                    );
                }
                ScheduledPayload::Midi {
                    message,
                    voice,
                    scope,
                } => {
                    self.dispatch_scoped_voice_midi(
                        MidiSource::Scheduled,
                        voice,
                        scope,
                        event.patch_id(),
                        message,
                    );
                }
                ScheduledPayload::EndSendLease(id) => {
                    self.send_leases.end(event.patch_id(), id);
                }
                ScheduledPayload::SendLease(lease) => {
                    let target = self
                        .parameters
                        .patches()
                        .iter()
                        .find(|p| p.patch_id() == Some(event.patch_id()));
                    let at = self.rendered_frames + (offset / 2) as u64;
                    if target.is_none_or(|p| lease.bus().index() >= p.sends().len())
                        || self.send_leases.begin(event.patch_id(), lease, at).is_err()
                    {
                        self.routing_failures = self.routing_failures.saturating_add(1);
                    }
                }
            }
        }
        self.append_rendered_segment(
            &mut output[rendered_samples..],
            self.rendered_frames + (rendered_samples / 2) as u64,
            observation,
        )
    }

    fn append_rendered_segment(
        &mut self,
        output: &mut [f32],
        mut at: u64,
        observation: &mut Option<RenderedSegment>,
    ) -> bool {
        let mut offset = 0;
        // Every extra segment expires at least one admitted lease. Cleanup is
        // independent of the incoming command budget and cannot be starved.
        while offset < output.len() {
            self.send_leases.expire(at);
            let remaining = (output.len() - offset) / 2;
            let frames = self.send_leases.next_expiry().map_or(remaining, |end| {
                remaining.min(usize::try_from(end.saturating_sub(at)).unwrap_or(usize::MAX))
            });
            if frames == 0 {
                return false;
            }
            let end = offset + frames * 2;
            let Some(segment) = self.render_segment(&mut output[offset..end]) else {
                return false;
            };
            match observation {
                Some(previous) => previous.append(segment),
                None => *observation = Some(segment),
            }
            offset = end;
            at = at.saturating_add(frames as u64);
        }
        self.send_leases.expire(at);
        true
    }

    fn render_segment(&mut self, output: &mut [f32]) -> Option<RenderedSegment> {
        let frame_count = output.len() / 2;
        let parameters = &self.parameters;
        let mut effect_observations = [PatchEffectObservation::EMPTY; MAX_ACTIVE_PATCHES];
        {
            let (rack, patch_audio, _) = self.active_graph.callback_parts_mut();
            patch_audio.begin_render(parameters, frame_count).ok()?;
            rack.render(patch_audio, parameters).ok()?;
        }
        let preview = self.active_graph.render_audition(frame_count);
        let (_, effect_rack, patch_audio, mixer) =
            self.active_graph.callback_parts_with_effects_mut();
        effect_rack
            .process(patch_audio, parameters, &mut effect_observations)
            .ok()?;
        let primary = patch_audio.stems().first();
        let primary_patch_id = primary.and_then(|stem| stem.patch_id());
        let primary_patch_rms = primary.map_or(0.0, |stem| {
            let samples = stem.samples();
            let energy = samples.iter().fold(0.0_f64, |sum, sample| {
                sum + f64::from(*sample) * f64::from(*sample)
            });
            (energy / samples.len().max(1) as f64).sqrt() as f32
        });
        let patch_effect = effect_observations
            .iter()
            .copied()
            .find(|value| value.patch_id().is_some())
            .unwrap_or(PatchEffectObservation::EMPTY);
        Some(RenderedSegment {
            frames: frame_count,
            primary_patch_id,
            primary_patch_rms,
            patch_effect,
            mix: mixer.mix_with_send_leases(
                patch_audio,
                parameters,
                output,
                Some(&self.send_leases),
            ),
            preview,
        })
    }

    fn publish_rendered_block(&mut self, observation: RenderedSegment) {
        self.rendered_blocks = self.rendered_blocks.saturating_add(1);
        self.rendered_frames = self
            .rendered_frames
            .saturating_add(observation.frames as u64);
        let primary_active_notes = observation
            .primary_patch_id
            .map_or(0, |patch_id| self.observed_note_count(patch_id));
        self.observation.publish_from_callback(
            AudioObservationSnapshot::from_mix_with_graph_routing_primary_and_effect(
                self.rendered_blocks,
                self.rendered_blocks,
                self.rendered_frames,
                self.parameters.generation(),
                self.active_graph.revision(),
                self.commands_consumed,
                self.observed_note_total(),
                self.routing_failures,
                self.last_unknown_patch_id,
                observation.primary_patch_id,
                observation.primary_patch_rms,
                primary_active_notes,
                observation.patch_effect,
                observation.mix,
            )
            .with_voice_limit_refusals(self.voice_limit_refusals)
            .with_schedule_metrics(
                self.scheduled_late_events,
                self.scheduled_max_lateness_frames,
                self.boundary.scheduled_queue_high_water(),
            )
            .with_preview_observation(observation.preview),
        );
    }

    fn mark_scheduled_patch(&mut self, patch_id: PatchId) {
        if self.scheduled_patches.contains(&Some(patch_id)) {
            return;
        }
        if let Some(slot) = self
            .scheduled_patches
            .iter_mut()
            .find(|slot| slot.is_none())
        {
            *slot = Some(patch_id);
        }
    }

    /// Physical input loss must not invalidate the host's queued owned expression.
    fn release_immediate_notes(&mut self) {
        for patch in self.parameters.patches() {
            let Some(patch_id) = patch.patch_id() else {
                continue;
            };
            if self
                .active_graph
                .callback_parts_mut()
                .0
                .all_notes_off_for_source(MidiSource::Immediate, patch_id)
                .is_ok()
            {
                self.active_notes
                    .clear_source_patch(MidiSource::Immediate, patch_id);
            } else {
                self.routing_failures = self.routing_failures.saturating_add(1);
            }
        }
    }

    /// Source-aware cleanup reaches held and sustained voices directly. Only
    /// touched Patches are visited; unrelated legacy instruments never receive
    /// an unsupported source operation. Prepared voice capacity bounds the work.
    fn release_scheduled_notes(&mut self) {
        self.send_leases.clear();
        for index in 0..MAX_ACTIVE_PATCHES {
            let Some(patch_id) = self.scheduled_patches[index].take() else {
                continue;
            };
            let result = self
                .active_graph
                .callback_parts_mut()
                .0
                .all_notes_off_for_source(MidiSource::Scheduled, patch_id);
            match result {
                Ok(()) => self
                    .active_notes
                    .clear_source_patch(MidiSource::Scheduled, patch_id),
                Err(_) => self.routing_failures = self.routing_failures.saturating_add(1),
            }
        }
    }

    fn publish_failed_block(&mut self, frame_count: usize) {
        self.routing_failures = self.routing_failures.saturating_add(1);
        self.rendered_blocks = self.rendered_blocks.saturating_add(1);
        self.rendered_frames = self.rendered_frames.saturating_add(frame_count as u64);
        self.observation.publish_from_callback(
            AudioObservationSnapshot::from_mix_with_graph_and_routing(
                self.rendered_blocks,
                self.rendered_blocks,
                self.rendered_frames,
                self.parameters.generation(),
                self.active_graph.revision(),
                self.commands_consumed,
                self.observed_note_total(),
                self.routing_failures,
                self.last_unknown_patch_id,
                crate::mixer::mix_observation::MixObservation::default(),
            )
            .with_voice_limit_refusals(self.voice_limit_refusals)
            .with_schedule_metrics(
                self.scheduled_late_events,
                self.scheduled_max_lateness_frames,
                self.boundary.scheduled_queue_high_water(),
            ),
        );
    }

    /// Places the refusal counter at an arbitrary point so a test can drive it
    /// against its ceiling. Saturation is unreachable by rendering alone.
    #[cfg(test)]
    fn seed_voice_limit_refusals(&mut self, refusals: u64) {
        self.voice_limit_refusals = refusals;
    }

    pub const fn active_revision(&self) -> GraphRevision {
        self.active_graph.revision()
    }

    pub fn pending_retirement_revision(&self) -> Option<GraphRevision> {
        self.pending_retirement
            .as_ref()
            .map(PreparedGraph::revision)
    }

    pub const fn handoff_status(&self) -> GraphHandoffStatus {
        self.handoff_status
    }

    pub const fn parameters(&self) -> &crate::real_time::parameter_snapshot::ParameterSnapshot {
        &self.parameters
    }

    /// Returns the active graph's caller-owned Patch stems for control-side
    /// deterministic verification when no physical callback owns the renderer.
    pub fn active_patch_audio(&self) -> &crate::real_time::patch_audio_block::PatchAudioBlock {
        self.active_graph.patch_audio()
    }

    fn apply_structural_handoff(&mut self) {
        if let Some(retired) = self.pending_retirement.take() {
            let retired_revision = retired.revision();
            match self.structural.return_retired_on_audio(retired) {
                Ok(()) => self.handoff_status.record_retired(retired_revision),
                Err(full) => {
                    self.pending_retirement = Some(full.into_graph());
                    self.handoff_status.record_retirement_retry();
                    self.structural.publish_status_on_audio(self.handoff_status);
                    return;
                }
            }
        }

        let Some(mut replacement) = self.structural.take_prepared_on_audio() else {
            self.structural.publish_status_on_audio(self.handoff_status);
            return;
        };

        let replacement_revision = replacement.revision();
        // Old-revision note-offs will not be delivered into the new graph.
        // Release their owned notes before any live instances carry over.
        self.release_scheduled_notes();
        replacement.exchange_initial_parameters_on_audio(&mut self.parameters);
        // Voice carry-over: before the graphs exchange, move every
        // still-live prepared instance the declared delta leaves unchanged
        // from the active graph into the replacement — bounded pointer swaps,
        // no allocation, no destruction. The fresh never-sounded instances
        // ride into the superseded graph and retire off-callback as before.
        let carry_over = replacement.carry_over_scope();
        replacement.carry_live_state_from(&mut self.active_graph);
        let retired = core::mem::replace(&mut self.active_graph, replacement);
        let retired_revision = retired.revision();
        match carry_over {
            // No declared delta: the full-reset swap semantics remain.
            None => self.active_notes.clear_all(),
            // Only the selected Patch's engine restarted; every other
            // sounding voice carried over with its engine.
            Some(crate::real_time::GraphReplacementScope::SelectedEngine(patch_id)) => {
                self.active_notes.clear_patch(patch_id);
            }
            // Occupancy deltas leave every engine identity unchanged: every
            // sounding voice carried over.
            Some(
                crate::real_time::GraphReplacementScope::PatchSlot { .. }
                | crate::real_time::GraphReplacementScope::BusReturn(_)
                | crate::real_time::GraphReplacementScope::AppendPatch(_)
                | crate::real_time::GraphReplacementScope::Audition,
            ) => {}
            // A whole-session activation never carries live voices across.
            Some(crate::real_time::GraphReplacementScope::WholeSession) => {
                self.active_notes.clear_all();
            }
        }
        self.handoff_status.record_swap(replacement_revision);

        match self.structural.return_retired_on_audio(retired) {
            Ok(()) => self.handoff_status.record_retired(retired_revision),
            Err(full) => {
                self.pending_retirement = Some(full.into_graph());
                self.handoff_status.record_retirement_retry();
            }
        }
        self.structural.publish_status_on_audio(self.handoff_status);
    }
}

/// Segment measurements combine into one device-block observation. Peaks use
/// maxima; RMS values combine by frame-weighted energy, never by averaging RMS.
struct RenderedSegment {
    frames: usize,
    primary_patch_id: Option<PatchId>,
    primary_patch_rms: f32,
    patch_effect: PatchEffectObservation,
    mix: MixObservation,
    preview: PreviewAudioObservation,
}

impl RenderedSegment {
    fn append(&mut self, next: Self) {
        let weighted = |first, second| weighted_rms(first, self.frames, second, next.frames);
        self.primary_patch_rms = weighted(self.primary_patch_rms, next.primary_patch_rms);
        self.patch_effect = PatchEffectObservation::from_parts(
            self.patch_effect.patch_id(),
            weighted(self.patch_effect.input_rms(), next.patch_effect.input_rms()),
            weighted(
                self.patch_effect.output_rms(),
                next.patch_effect.output_rms(),
            ),
            weighted(
                self.patch_effect.difference_rms(),
                next.patch_effect.difference_rms(),
            ),
            weighted(self.patch_effect.side_rms(), next.patch_effect.side_rms()),
        );
        self.mix = merge_mix_observations(self.mix, self.frames, next.mix, next.frames);
        self.preview = next.preview;
        self.frames += next.frames;
    }
}

fn weighted_rms(first: f32, first_frames: usize, second: f32, second_frames: usize) -> f32 {
    let first_energy = f64::from(first).powi(2) * first_frames as f64;
    let second_energy = f64::from(second).powi(2) * second_frames as f64;
    ((first_energy + second_energy) / (first_frames + second_frames) as f64).sqrt() as f32
}

fn merge_mix_observations(
    first: MixObservation,
    first_frames: usize,
    second: MixObservation,
    second_frames: usize,
) -> MixObservation {
    let weighted = |a, b| weighted_rms(a, first_frames, b, second_frames);
    let first_tracks = first.tracks();
    let second_tracks = second.tracks();
    let tracks = std::array::from_fn(|index| {
        let a = first_tracks[index];
        let b = second_tracks[index];
        TrackMeter::new(
            a.left_peak().max(b.left_peak()),
            a.right_peak().max(b.right_peak()),
            weighted(a.rms(), b.rms()),
        )
        .unwrap_or_default()
    });
    let first_inputs = first.bus_input_rms_all();
    let second_inputs = second.bus_input_rms_all();
    let inputs = std::array::from_fn(|index| weighted(first_inputs[index], second_inputs[index]));
    let first_outputs = first.bus_output_rms_all();
    let second_outputs = second.bus_output_rms_all();
    let outputs =
        std::array::from_fn(|index| weighted(first_outputs[index], second_outputs[index]));
    MixObservation::measured(
        tracks,
        first.left_peak().max(second.left_peak()),
        first.right_peak().max(second.right_peak()),
        weighted(first.output_rms(), second.output_rms()),
        inputs,
        outputs,
        weighted(first.wet_output_rms(), second.wet_output_rms()),
        first
            .non_finite_samples()
            .saturating_add(second.non_finite_samples()),
        first
            .clipped_samples()
            .saturating_add(second.clipped_samples()),
    )
}

/// Active note observations and admission count each transport lane separately,
/// so a scheduled release of a shared pitch cannot clear a live note's bit.
struct SourceNoteObservation {
    sources: [ActiveNoteObservation; MidiSource::COUNT],
}

impl SourceNoteObservation {
    const fn new() -> Self {
        Self {
            sources: [const { ActiveNoteObservation::new() }; MidiSource::COUNT],
        }
    }

    fn observe_patch_message(
        &mut self,
        source: MidiSource,
        patch_id: PatchId,
        message: MidiMessage,
    ) {
        self.sources[source.index()].observe_patch_message(patch_id, message);
    }

    fn clear_all(&mut self) {
        for source in &mut self.sources {
            source.clear_all();
        }
    }

    fn clear_patch(&mut self, patch_id: PatchId) {
        for source in &mut self.sources {
            source.clear_patch(patch_id);
        }
    }

    fn clear_source_patch(&mut self, source: MidiSource, patch_id: PatchId) {
        self.sources[source.index()].clear_patch(patch_id);
    }

    #[cfg(test)]
    fn count(&self) -> u32 {
        self.sources
            .iter()
            .fold(0, |sum: u32, source| sum.saturating_add(source.count()))
    }

    fn count_patch(&self, patch_id: PatchId) -> u32 {
        self.sources.iter().fold(0, |sum: u32, source| {
            sum.saturating_add(source.count_patch(patch_id))
        })
    }
}

#[derive(Clone, Copy)]
struct PatchNoteBits {
    patch_id: Option<PatchId>,
    low: [u64; 16],
    high: [u64; 16],
}

impl PatchNoteBits {
    const EMPTY: Self = Self {
        patch_id: None,
        low: [0; 16],
        high: [0; 16],
    };

    fn set(&mut self, channel: usize, note: u8) {
        if note < 64 {
            self.low[channel] |= 1_u64 << note;
        } else {
            self.high[channel] |= 1_u64 << (note - 64);
        }
    }

    fn clear(&mut self, channel: usize, note: u8) {
        if note < 64 {
            self.low[channel] &= !(1_u64 << note);
        } else {
            self.high[channel] &= !(1_u64 << (note - 64));
        }
    }

    fn count(self) -> u32 {
        self.low
            .iter()
            .chain(&self.high)
            .map(|bits| bits.count_ones())
            .sum()
    }
}

struct ActiveNoteObservation {
    patches: [PatchNoteBits; MAX_ACTIVE_PATCHES],
}

impl ActiveNoteObservation {
    const fn new() -> Self {
        Self {
            patches: [PatchNoteBits::EMPTY; MAX_ACTIVE_PATCHES],
        }
    }

    fn observe_patch_message(
        &mut self,
        patch_id: PatchId,
        message: crate::kernel::midi_message::MidiMessage,
    ) {
        if message.kind() == MidiMessageKind::AllNotesOff
            || (message.kind() == MidiMessageKind::ControlChange
                && matches!(message.data1(), 120 | 123))
        {
            if let Some(patch) = self.patch_mut(patch_id, false) {
                let channel = usize::from(message.channel().value());
                patch.low[channel] = 0;
                patch.high[channel] = 0;
            }
            return;
        }

        let note = message.data1();
        match message.kind() {
            MidiMessageKind::NoteOn if message.data2() > 0 => {
                if let Some(patch) = self.patch_mut(patch_id, true) {
                    patch.set(usize::from(message.channel().value()), note);
                }
            }
            MidiMessageKind::NoteOn | MidiMessageKind::NoteOff => {
                if let Some(patch) = self.patch_mut(patch_id, false) {
                    patch.clear(usize::from(message.channel().value()), note);
                    if patch.count() == 0 {
                        *patch = PatchNoteBits::EMPTY;
                    }
                }
            }
            MidiMessageKind::ControlChange
            | MidiMessageKind::ProgramChange
            | MidiMessageKind::ChannelPressure
            | MidiMessageKind::PolyPressure
            | MidiMessageKind::PitchBend
            | MidiMessageKind::AllNotesOff => {}
        }
    }

    fn patch_mut(&mut self, patch_id: PatchId, create: bool) -> Option<&mut PatchNoteBits> {
        if let Some(index) = self
            .patches
            .iter()
            .position(|patch| patch.patch_id == Some(patch_id))
        {
            return self.patches.get_mut(index);
        }
        if !create {
            return None;
        }
        let slot = self
            .patches
            .iter_mut()
            .find(|patch| patch.patch_id.is_none())?;
        slot.patch_id = Some(patch_id);
        Some(slot)
    }

    fn clear_all(&mut self) {
        self.patches.fill(PatchNoteBits::EMPTY);
    }

    /// Clears only one Patch's observed notes with bounded work — the
    /// carry-over swap path uses this when exactly one engine restarted.
    fn clear_patch(&mut self, patch_id: PatchId) {
        if let Some(patch) = self.patch_mut(patch_id, false) {
            *patch = PatchNoteBits::EMPTY;
        }
    }

    #[cfg(test)]
    fn count(&self) -> u32 {
        self.patches
            .iter()
            .fold(0_u32, |count, patch| count.saturating_add(patch.count()))
    }

    fn count_patch(&self, patch_id: PatchId) -> u32 {
        self.patches
            .iter()
            .find(|patch| patch.patch_id == Some(patch_id))
            .map_or(0, |patch| patch.count())
    }
}

#[cfg(test)]
mod tests {
    use super::{ActiveNoteObservation, AudioRenderer, SourceNoteObservation};
    use crate::adapter::braids_capability::BRAIDS_CAPABILITY_ID;
    use crate::adapter::hidef_soundfont_capability::{
        HiDefSoundFontCapability, HIDEF_CAPABILITY_ID,
    };
    use crate::adapter::hidef_soundfont_preparer::HiDefSoundFontPreparer;
    use crate::adapter::production_instruments::{
        production_capability_registry, production_instrument_preparers,
        production_instrument_providers,
    };
    use crate::kernel::midi_channel::MidiChannel;
    use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
    use crate::kernel::patch_id::PatchId;
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_state::MixerState;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::patch_output::PatchOutput;
    use crate::real_time::audio_boundary::AudioThreadBoundary;
    use crate::real_time::audio_command::AudioCommand;
    use crate::real_time::audio_observation::CallbackAudioObservation;
    use crate::real_time::audio_observation_snapshot::AudioObservationSnapshot;
    use crate::real_time::graph_handoff_status::GraphHandoffStatus;
    use crate::real_time::graph_revision::GraphRevision;
    use crate::real_time::parameter_snapshot::{ParameterSnapshot, RtPatchParameters};
    use crate::real_time::prepared_graph::PreparedGraph;
    use crate::real_time::prepared_graph_builder::{
        AuditionPreparationRequest, PreparedGraphBuilder,
    };
    use crate::real_time::structural_graph_boundary::{
        AudioStructuralGraphBoundary, RetiredBoundaryFull,
    };
    use crate::real_time::MAX_ACTIVE_PATCHES;
    use crate::synth::capability_id::CapabilityId;
    use crate::synth::instrument_preparer::{InstrumentPreparationError, InstrumentPreparer};
    use crate::synth::patch::Patch;
    use crate::synth::prepared_instrument::MidiSource;
    use crate::synth::prepared_instrument::{PreparedInstrument, PreparedInstrumentError};
    use crate::synth::sound_font_instrument::SoundFontInstrument;
    use crate::synth::{DescriptorDefaultConfigFactory, PreparedAudition};
    use crate::testing::automatic_midi_test::create_soundfont_config;
    use core::alloc::{GlobalAlloc, Layout};
    use core::cell::Cell;
    use std::alloc::System;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    thread_local! {
        static COUNT_MEMORY: Cell<bool> = const { Cell::new(false) };
        static ALLOCATION_COUNT: Cell<usize> = const { Cell::new(0) };
        static DEALLOCATION_COUNT: Cell<usize> = const { Cell::new(0) };
    }

    struct TestAllocator;

    #[global_allocator]
    static TEST_ALLOCATOR: TestAllocator = TestAllocator;

    unsafe impl GlobalAlloc for TestAllocator {
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

        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            record_allocation();
            unsafe { System.realloc(pointer, layout, new_size) }
        }
    }

    fn record_allocation() {
        let _ = COUNT_MEMORY.try_with(|enabled| {
            if enabled.get() {
                let _ = ALLOCATION_COUNT.try_with(|count| count.set(count.get() + 1));
            }
        });
    }

    fn record_deallocation() {
        let _ = COUNT_MEMORY.try_with(|enabled| {
            if enabled.get() {
                let _ = DEALLOCATION_COUNT.try_with(|count| count.set(count.get() + 1));
            }
        });
    }

    fn begin_memory_count() {
        ALLOCATION_COUNT.with(|count| count.set(0));
        DEALLOCATION_COUNT.with(|count| count.set(0));
        COUNT_MEMORY.with(|enabled| enabled.set(true));
    }

    fn finish_memory_count() -> (usize, usize) {
        COUNT_MEMORY.with(|enabled| enabled.set(false));
        (
            ALLOCATION_COUNT.with(Cell::get),
            DEALLOCATION_COUNT.with(Cell::get),
        )
    }

    struct TestBoundary {
        commands: [Option<AudioCommand>; 4],
        next_command: usize,
        latest: ParameterSnapshot,
        parameter_reads: usize,
    }

    #[test]
    fn command_flood_is_bounded_and_keeps_ordered_recovery_for_the_next_block() {
        use super::DEFAULT_AUDIO_COMMANDS_PER_BLOCK;
        struct Flood {
            popped: usize,
            budget: usize,
            parameters: ParameterSnapshot,
        }
        impl AudioThreadBoundary for Flood {
            fn pop_command(&mut self) -> Option<AudioCommand> {
                let command = if self.popped < self.budget * 2 {
                    AudioCommand::patch_midi(
                        PatchId::new(1).unwrap(),
                        MidiMessage::try_new(
                            MidiChannel::new(0).unwrap(),
                            MidiMessageKind::ControlChange,
                            11,
                            100,
                        )
                        .unwrap(),
                    )
                } else if self.popped == self.budget * 2 {
                    AudioCommand::all_notes_off()
                } else {
                    return None;
                };
                self.popped += 1;
                Some(command)
            }
            fn exchange_latest_parameters(&mut self, previous: &mut ParameterSnapshot) -> bool {
                if self.parameters.generation() < previous.generation() {
                    return false;
                }
                core::mem::swap(&mut self.parameters, previous);
                true
            }
        }
        for budget in [7, DEFAULT_AUDIO_COMMANDS_PER_BLOCK] {
            let fixture = Fixture::new();
            let graph = fixture.graph(1, 1);
            let boundary = Flood {
                popped: 0,
                budget,
                parameters: fixture.parameters(1, 1),
            };
            let mut renderer = AudioRenderer::new(boundary, TestStructural::new(), graph)
                .with_command_budget(core::num::NonZeroUsize::new(budget).unwrap());
            let mut output = [0.0; 8];
            for block in 1..=2 {
                begin_memory_count();
                renderer.render(&mut output);
                let memory = finish_memory_count();
                assert_eq!(memory, (0, 0));
                assert_eq!(renderer.commands_consumed, (budget * block) as u64);
                assert!(output.iter().any(|sample| *sample != 0.0));
            }
            renderer.render(&mut output);
            assert_eq!(renderer.commands_consumed, (budget * 2 + 1) as u64);
            assert_eq!(fixture.first_dispatches.load(Ordering::Relaxed), 0);
            renderer.render(&mut output);
            assert_eq!(renderer.commands_consumed, (budget * 2 + 1) as u64);
        }
    }

    impl AudioThreadBoundary for TestBoundary {
        fn pop_command(&mut self) -> Option<AudioCommand> {
            let command = self.commands.get_mut(self.next_command)?.take();
            if command.is_some() {
                self.next_command += 1;
            }
            command
        }

        fn exchange_latest_parameters(&mut self, previous: &mut ParameterSnapshot) -> bool {
            self.parameter_reads += 1;
            if self.latest.generation() < previous.generation() {
                return false;
            }
            core::mem::swap(&mut self.latest, previous);
            true
        }
    }

    struct TestStructural {
        prepared: [Option<PreparedGraph>; 2],
        prepared_index: usize,
        retired: [Option<PreparedGraph>; 2],
        retired_count: usize,
        retirement_blocked: bool,
        latest_status: GraphHandoffStatus,
    }

    impl TestStructural {
        fn new() -> Self {
            Self {
                prepared: std::array::from_fn(|_| None),
                prepared_index: 0,
                retired: std::array::from_fn(|_| None),
                retired_count: 0,
                retirement_blocked: false,
                latest_status: GraphHandoffStatus::default(),
            }
        }

        fn with_prepared(graphs: [Option<PreparedGraph>; 2]) -> Self {
            Self {
                prepared: graphs,
                prepared_index: 0,
                retired: std::array::from_fn(|_| None),
                retired_count: 0,
                retirement_blocked: false,
                latest_status: GraphHandoffStatus::default(),
            }
        }
    }

    impl AudioStructuralGraphBoundary for TestStructural {
        fn take_prepared_on_audio(&mut self) -> Option<PreparedGraph> {
            let graph = self.prepared.get_mut(self.prepared_index)?.take();
            if graph.is_some() {
                self.prepared_index += 1;
            }
            graph
        }

        fn return_retired_on_audio(
            &mut self,
            graph: PreparedGraph,
        ) -> Result<(), RetiredBoundaryFull> {
            if self.retirement_blocked || self.retired_count == self.retired.len() {
                return Err(RetiredBoundaryFull::new(graph));
            }
            self.retired[self.retired_count] = Some(graph);
            self.retired_count += 1;
            Ok(())
        }

        fn publish_status_on_audio(&mut self, status: GraphHandoffStatus) {
            self.latest_status = status;
        }
    }

    #[derive(Default)]
    struct TestObservation {
        latest: AudioObservationSnapshot,
        publications: usize,
    }

    impl CallbackAudioObservation for TestObservation {
        fn publish_from_callback(&mut self, snapshot: AudioObservationSnapshot) {
            self.latest = snapshot;
            self.publications += 1;
        }
    }

    struct FixturePreparer {
        capability_id: CapabilityId,
        first_dispatches: Arc<AtomicUsize>,
        render_failure: Arc<AtomicBool>,
        second_dispatches: Arc<AtomicUsize>,
        drops: Arc<AtomicUsize>,
    }

    impl InstrumentPreparer for FixturePreparer {
        fn capability_id(&self) -> &CapabilityId {
            &self.capability_id
        }

        fn prepare(
            &self,
            patch: &Patch,
            _sample_rate: f32,
            _max_frames: usize,
        ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
            if patch.id().value() % 2 == 1 {
                Ok(Box::new(FirstInstrument {
                    patch_id: patch.id(),
                    dispatches: Arc::clone(&self.first_dispatches),
                    render_failure: Arc::clone(&self.render_failure),
                    drops: Arc::clone(&self.drops),
                }))
            } else {
                Ok(Box::new(SecondInstrument {
                    patch_id: patch.id(),
                    dispatches: Arc::clone(&self.second_dispatches),
                    drops: Arc::clone(&self.drops),
                }))
            }
        }

        fn prepare_audition(
            &self,
            patch_id: PatchId,
            _candidate: &crate::synth::InstrumentConfig,
            _sample_rate: f32,
            _max_frames: usize,
        ) -> Result<Box<dyn PreparedAudition>, InstrumentPreparationError> {
            Ok(Box::new(FixtureAudition {
                patch_id,
                playing: false,
            }))
        }
    }

    struct FixtureAudition {
        patch_id: PatchId,
        playing: bool,
    }

    impl PreparedAudition for FixtureAudition {
        fn patch_id(&self) -> PatchId {
            self.patch_id
        }

        fn start(&mut self) {
            self.playing = true;
        }

        fn stop(&mut self) {
            self.playing = false;
        }

        fn render(&mut self, output: &mut [f32], frame_count: usize) {
            if !self.playing {
                return;
            }
            for sample in output.iter_mut().take(frame_count.saturating_mul(2)) {
                *sample += 0.125;
            }
        }

        fn is_playing(&self) -> bool {
            self.playing
        }

        fn playhead(&self) -> f32 {
            if self.playing {
                0.5
            } else {
                0.0
            }
        }
    }

    struct FirstInstrument {
        patch_id: PatchId,
        render_failure: Arc<AtomicBool>,
        dispatches: Arc<AtomicUsize>,
        drops: Arc<AtomicUsize>,
    }

    impl PreparedInstrument for FirstInstrument {
        fn patch_id(&self) -> PatchId {
            self.patch_id
        }

        fn dispatch(
            &mut self,
            _message: MidiMessage,
            _parameters: &crate::real_time::RtPatchParameters,
        ) -> Result<(), PreparedInstrumentError> {
            self.dispatches.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn render(
            &mut self,
            output: &mut [f32],
            _frame_count: usize,
            _parameters: &crate::real_time::RtPatchParameters,
        ) -> Result<(), crate::synth::PreparedInstrumentError> {
            output.fill(0.25);
            if self.render_failure.load(Ordering::Relaxed) {
                return Err(PreparedInstrumentError::RenderRejected);
            }
            Ok(())
        }

        fn all_notes_off(&mut self) {
            self.dispatches.store(0, Ordering::Relaxed);
        }
    }

    impl Drop for FirstInstrument {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }

    struct SecondInstrument {
        patch_id: PatchId,
        dispatches: Arc<AtomicUsize>,
        drops: Arc<AtomicUsize>,
    }

    impl PreparedInstrument for SecondInstrument {
        fn patch_id(&self) -> PatchId {
            self.patch_id
        }

        fn dispatch(
            &mut self,
            _message: MidiMessage,
            _parameters: &crate::real_time::RtPatchParameters,
        ) -> Result<(), PreparedInstrumentError> {
            self.dispatches.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn render(
            &mut self,
            output: &mut [f32],
            _frame_count: usize,
            _parameters: &crate::real_time::RtPatchParameters,
        ) -> Result<(), crate::synth::PreparedInstrumentError> {
            output.fill(0.5);

            Ok(())
        }

        fn all_notes_off(&mut self) {
            self.dispatches.store(0, Ordering::Relaxed);
        }
    }

    impl Drop for SecondInstrument {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }

    struct Fixture {
        provider: HiDefSoundFontCapability,
        patches: [Patch; 2],
        first_dispatches: Arc<AtomicUsize>,
        render_failure: Arc<AtomicBool>,
        second_dispatches: Arc<AtomicUsize>,
        drops: Arc<AtomicUsize>,
    }

    impl Fixture {
        fn new() -> Self {
            let provider =
                crate::adapter::production_instruments::production_soundfont_capability().unwrap();
            let patch = |id: u32| {
                Patch::new(
                    PatchId::new(id).unwrap(),
                    format!("Patch {id}"),
                    create_soundfont_config(
                        &provider,
                        SoundFontInstrument::new(0, (id - 1) as u8, false).unwrap(),
                    )
                    .unwrap(),
                    MidiChannel::new((id - 1) as u8).unwrap(),
                    PatchOutput::to_track(MixerTrackId::new((id - 1) as u8).unwrap()),
                )
            };
            Self {
                patches: [patch(1), patch(2)],
                provider,
                first_dispatches: Arc::new(AtomicUsize::new(0)),
                render_failure: Arc::new(AtomicBool::new(false)),
                second_dispatches: Arc::new(AtomicUsize::new(0)),
                drops: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn parameters(&self, revision: u64, generation: u64) -> ParameterSnapshot {
            ParameterSnapshot::for_graph(
                generation,
                GraphRevision::new(revision).unwrap(),
                GlobalParameters::new(0.0).unwrap(),
                MixerState::default(),
                &self
                    .patches
                    .each_ref()
                    .map(|patch| RtPatchParameters::new(patch.id(), patch.output())),
            )
            .unwrap()
        }

        fn graph(&self, revision: u64, generation: u64) -> PreparedGraph {
            let registry = self.provider.registry().unwrap();
            let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(FixturePreparer {
                capability_id: CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap(),
                first_dispatches: Arc::clone(&self.first_dispatches),
                render_failure: Arc::clone(&self.render_failure),
                second_dispatches: Arc::clone(&self.second_dispatches),
                drops: Arc::clone(&self.drops),
            })];
            PreparedGraphBuilder::new(&registry, &preparers)
                .build(
                    GraphRevision::new(revision).unwrap(),
                    &self.patches,
                    self.parameters(revision, generation),
                    48_000.0,
                    4,
                )
                .unwrap()
        }

        fn graph_with_audition(
            &self,
            revision: u64,
            generation: u64,
            identity: u64,
            patch_id: PatchId,
        ) -> PreparedGraph {
            let registry = self.provider.registry().unwrap();
            let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(FixturePreparer {
                capability_id: CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap(),
                first_dispatches: Arc::clone(&self.first_dispatches),
                render_failure: Arc::clone(&self.render_failure),
                second_dispatches: Arc::clone(&self.second_dispatches),
                drops: Arc::clone(&self.drops),
            })];
            let candidate = self
                .patches
                .iter()
                .find(|patch| patch.id() == patch_id)
                .unwrap()
                .instrument_config()
                .clone();
            let audition = AuditionPreparationRequest::new(identity, patch_id, candidate).unwrap();
            PreparedGraphBuilder::new(&registry, &preparers)
                .with_audition(&audition)
                .build(
                    GraphRevision::new(revision).unwrap(),
                    &self.patches,
                    self.parameters(revision, generation),
                    48_000.0,
                    4,
                )
                .unwrap()
        }

        fn boundary(&self, latest: ParameterSnapshot, commands: &[AudioCommand]) -> TestBoundary {
            let mut storage = [None; 4];
            for (slot, command) in storage.iter_mut().zip(commands) {
                *slot = Some(*command);
            }
            TestBoundary {
                commands: storage,
                next_command: 0,
                latest,
                parameter_reads: 0,
            }
        }
    }

    fn midi(kind: MidiMessageKind) -> MidiMessage {
        MidiMessage::try_new(MidiChannel::new(1).unwrap(), kind, 60, 100).unwrap()
    }

    #[test]
    fn renderer_routes_heterogeneous_patches_and_publishes_finite_mix_observation() {
        let fixture = Fixture::new();
        let boundary = fixture.boundary(
            fixture.parameters(1, 9),
            &[
                AudioCommand::patch_midi(fixture.patches[1].id(), midi(MidiMessageKind::NoteOn)),
                AudioCommand::patch_midi(PatchId::new(99).unwrap(), midi(MidiMessageKind::NoteOn)),
            ],
        );
        let structural = TestStructural::new();
        let mut renderer = AudioRenderer::with_observation(
            boundary,
            structural,
            fixture.graph(1, 1),
            TestObservation::default(),
        );
        let mut output = [0.0; 8];

        renderer.render(&mut output);

        assert_eq!(fixture.first_dispatches.load(Ordering::Relaxed), 0);
        assert_eq!(fixture.second_dispatches.load(Ordering::Relaxed), 1);
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(output
            .iter()
            .all(|sample| (*sample - 0.75).abs() < 0.000_001));
        assert_eq!(renderer.observation.publications, 1);
        assert_eq!(renderer.observation.latest.parameter_generation(), 9);
        assert_eq!(renderer.observation.latest.commands_consumed(), 2);
        assert_eq!(renderer.observation.latest.active_notes(), 1);
    }

    #[test]
    fn failed_instrument_render_silences_partial_output_and_publishes_fresh_failure() {
        let fixture = Fixture::new();
        use crate::real_time::audio_boundary::{AudioBoundary, ControlAudioBoundary};
        let (mut control, boundary) =
            crate::adapter::lock_free_audio_boundary::LockFreeAudioBoundary::new(
                4,
                fixture.parameters(1, 1),
            )
            .into_handles();
        control.publish_parameters(fixture.parameters(1, 9));
        let mut renderer = AudioRenderer::with_observation(
            boundary,
            TestStructural::new(),
            fixture.graph(1, 1),
            TestObservation::default(),
        );
        let mut output = [0.0; 8];
        renderer.render(&mut output);
        assert!(output.iter().any(|sample| *sample != 0.0));
        let before = renderer.observation.latest;
        fixture.render_failure.store(true, Ordering::Relaxed);
        begin_memory_count();
        renderer.render(&mut output);
        let memory = finish_memory_count();
        assert_eq!(memory, (0, 0));
        assert_eq!(output, [0.0; 8]);
        let failed = renderer.observation.latest;
        assert_eq!(failed.sequence(), before.sequence() + 1);
        assert_eq!(failed.routing_failures(), before.routing_failures() + 1);
        assert_eq!(failed.parameter_generation(), 9);
        assert_eq!(failed.output_rms(), 0.0);
        fixture.render_failure.store(false, Ordering::Relaxed);
        renderer.render(&mut output);
        assert!(output.iter().any(|sample| *sample != 0.0));
        assert_eq!(
            renderer.observation.latest.sequence(),
            failed.sequence() + 1
        );
    }

    #[test]
    fn audition_commands_mix_only_into_the_correlated_origin_track_and_publish_playhead() {
        let fixture = Fixture::new();
        let patch_id = fixture.patches[0].id();
        let identity = 41;

        let baseline_boundary = fixture.boundary(fixture.parameters(1, 1), &[]);
        let mut baseline = AudioRenderer::with_observation(
            baseline_boundary,
            TestStructural::new(),
            fixture.graph(1, 1),
            TestObservation::default(),
        );
        let mut baseline_output = [0.0; 8];
        baseline.render(&mut baseline_output);
        let baseline_observation = baseline.observation.latest;

        let boundary = fixture.boundary(
            fixture.parameters(1, 1),
            &[AudioCommand::preview_start(patch_id, identity)],
        );
        let mut renderer = AudioRenderer::with_observation(
            boundary,
            TestStructural::new(),
            fixture.graph_with_audition(1, 1, identity, patch_id),
            TestObservation::default(),
        );
        let mut output = [0.0; 8];
        begin_memory_count();
        renderer.render(&mut output);
        let start_memory = finish_memory_count();

        let observation = renderer.observation.latest;
        assert_eq!(start_memory, (0, 0));
        assert_eq!(observation.preview_identity(), identity);
        assert_eq!(observation.preview_patch_id(), Some(patch_id));
        assert!(observation.preview_playing());
        assert_eq!(observation.preview_playhead(), 0.5);
        assert!(
            observation.track(MixerTrackId::new(0).unwrap()).rms()
                > baseline_observation
                    .track(MixerTrackId::new(0).unwrap())
                    .rms()
        );
        assert_eq!(
            observation
                .track(MixerTrackId::new(1).unwrap())
                .rms()
                .to_bits(),
            baseline_observation
                .track(MixerTrackId::new(1).unwrap())
                .rms()
                .to_bits(),
            "the audition must not enter another Patch/track stem"
        );

        renderer.boundary.commands[1] = Some(AudioCommand::preview_stop(patch_id, identity));
        begin_memory_count();
        renderer.render(&mut output);
        let stop_memory = finish_memory_count();
        assert_eq!(stop_memory, (0, 0));
        assert!(!renderer.observation.latest.preview_playing());
        assert_eq!(renderer.observation.latest.preview_playhead(), 0.0);
    }

    #[test]
    fn graph_swaps_apply_once_at_block_start_and_return_the_old_owner() {
        let fixture = Fixture::new();
        let boundary = fixture.boundary(fixture.parameters(2, 20), &[]);
        let structural =
            TestStructural::with_prepared([Some(fixture.graph(2, 20)), Some(fixture.graph(3, 30))]);
        let mut renderer = AudioRenderer::new(boundary, structural, fixture.graph(1, 10));
        let mut output = [0.0; 8];

        renderer.render(&mut output);

        assert_eq!(renderer.active_revision(), GraphRevision::new(2).unwrap());
        assert_eq!(renderer.structural.prepared_index, 1);
        assert_eq!(renderer.structural.retired_count, 1);
        assert_eq!(
            renderer.structural.retired[0].as_ref().unwrap().revision(),
            GraphRevision::new(1).unwrap()
        );
        assert_eq!(renderer.handoff_status().swaps_applied(), 1);
        assert_eq!(
            renderer.handoff_status().retired_revision(),
            Some(GraphRevision::new(1).unwrap())
        );
    }

    #[test]
    fn retirement_pressure_retains_and_retries_without_taking_another_graph() {
        let fixture = Fixture::new();
        let boundary = fixture.boundary(fixture.parameters(2, 20), &[]);
        let mut structural =
            TestStructural::with_prepared([Some(fixture.graph(2, 20)), Some(fixture.graph(3, 30))]);
        structural.retirement_blocked = true;
        let mut renderer = AudioRenderer::new(boundary, structural, fixture.graph(1, 10));
        let mut output = [0.0; 8];

        begin_memory_count();
        renderer.render(&mut output);
        renderer.render(&mut output);
        let (allocations, deallocations) = finish_memory_count();

        assert_eq!(allocations, 0);
        assert_eq!(deallocations, 0);
        assert_eq!(fixture.drops.load(Ordering::Relaxed), 0);
        assert_eq!(renderer.active_revision(), GraphRevision::new(2).unwrap());
        assert_eq!(
            renderer.pending_retirement_revision(),
            Some(GraphRevision::new(1).unwrap())
        );
        assert_eq!(renderer.structural.prepared_index, 1);
        assert_eq!(renderer.handoff_status().swaps_applied(), 1);
        assert_eq!(renderer.handoff_status().retirement_retries(), 2);

        renderer.structural.retirement_blocked = false;
        renderer.render(&mut output);
        assert_eq!(renderer.active_revision(), GraphRevision::new(3).unwrap());
        assert_eq!(renderer.structural.prepared_index, 2);
        assert_eq!(renderer.structural.retired_count, 2);
    }

    #[test]
    fn incompatible_scalar_races_keep_each_graphs_last_compatible_snapshot() {
        let fixture = Fixture::new();
        let boundary = fixture.boundary(fixture.parameters(2, 99), &[]);
        let structural = TestStructural::new();
        let mut renderer = AudioRenderer::with_observation(
            boundary,
            structural,
            fixture.graph(1, 10),
            TestObservation::default(),
        );
        let mut output = [0.0; 8];

        renderer.render(&mut output);
        assert_eq!(renderer.parameters().generation(), 10);
        assert_eq!(renderer.observation.latest.parameter_generation(), 10);
        assert_eq!(renderer.handoff_status().incompatible_snapshots(), 1);

        renderer.structural.prepared[0] = Some(fixture.graph(2, 20));
        renderer.render(&mut output);
        assert_eq!(renderer.active_revision(), GraphRevision::new(2).unwrap());
        assert_eq!(renderer.parameters().generation(), 99);
        assert_eq!(renderer.observation.latest.parameter_generation(), 99);

        renderer.boundary.latest = fixture.parameters(1, 100);
        renderer.render(&mut output);
        assert_eq!(renderer.parameters().generation(), 99);
        assert_eq!(renderer.handoff_status().incompatible_snapshots(), 2);
    }

    #[test]
    fn production_layout_changing_swap_is_bounded_finite_and_allocation_free() {
        let registry = production_capability_registry().unwrap();
        let factory = DescriptorDefaultConfigFactory::new(
            registry.clone(),
            production_instrument_providers().unwrap(),
        );
        let patch_id = PatchId::new(1).unwrap();
        let soundfont = factory
            .create(&CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap())
            .unwrap();
        let mut patch = Patch::new(
            patch_id,
            "Layout-changing Patch".to_owned(),
            soundfont,
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        );
        let global = GlobalParameters::new(0.0).unwrap();
        let preparers = production_instrument_preparers().unwrap();
        let builder = PreparedGraphBuilder::new(&registry, &preparers);
        let source_parameters = ParameterSnapshot::project_patches(
            1,
            GraphRevision::INITIAL,
            global,
            MixerState::default(),
            std::slice::from_ref(&patch),
            &registry,
        )
        .unwrap();
        let source = builder
            .build(
                GraphRevision::INITIAL,
                std::slice::from_ref(&patch),
                source_parameters,
                48_000.0,
                512,
            )
            .unwrap();
        assert_eq!(source.engine_rack().scalar_count(0), Some(0));

        patch.set_instrument_config(
            factory
                .create(&CapabilityId::new(BRAIDS_CAPABILITY_ID).unwrap())
                .unwrap(),
        );
        let target_revision = GraphRevision::new(2).unwrap();
        let target_parameters = ParameterSnapshot::project_patches(
            2,
            target_revision,
            global,
            MixerState::default(),
            std::slice::from_ref(&patch),
            &registry,
        )
        .unwrap();
        let target = builder
            .build(
                target_revision,
                std::slice::from_ref(&patch),
                target_parameters.clone(),
                48_000.0,
                512,
            )
            .unwrap();
        assert_eq!(target.engine_rack().scalar_count(0), Some(3));

        let message =
            MidiMessage::try_new(patch.channel(), MidiMessageKind::NoteOn, 60, 100).unwrap();
        let boundary = TestBoundary {
            commands: [
                Some(AudioCommand::patch_midi(patch_id, message)),
                None,
                None,
                None,
            ],
            next_command: 0,
            latest: target_parameters,
            parameter_reads: 0,
        };
        let structural = TestStructural::with_prepared([Some(target), None]);
        let mut renderer = AudioRenderer::new(boundary, structural, source);
        let mut output = [0.0; 1_024];

        begin_memory_count();
        renderer.render(&mut output);
        let (allocations, deallocations) = finish_memory_count();

        assert_eq!(allocations, 0);
        assert_eq!(deallocations, 0);
        assert_eq!(renderer.active_revision(), target_revision);
        assert_eq!(renderer.active_graph.engine_rack().scalar_count(0), Some(3));
        assert_eq!(renderer.structural.retired_count, 1);
        assert_eq!(renderer.handoff_status().swaps_applied(), 1);
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(output.iter().any(|sample| sample.abs() > f32::EPSILON));
    }

    #[test]
    fn normal_renderer_callback_allocates_deallocates_and_destroys_nothing() {
        let fixture = Fixture::new();
        let boundary = fixture.boundary(
            fixture.parameters(1, 2),
            &[AudioCommand::patch_midi(
                fixture.patches[0].id(),
                midi(MidiMessageKind::NoteOn),
            )],
        );
        let structural = TestStructural::new();
        let mut renderer = AudioRenderer::new(boundary, structural, fixture.graph(1, 1));
        let mut output = [0.0; 8];

        begin_memory_count();
        renderer.render(&mut output);
        let (allocations, deallocations) = finish_memory_count();

        assert_eq!(allocations, 0);
        assert_eq!(deallocations, 0);
        assert_eq!(fixture.drops.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn hidef_soundfont_preparer_callback_operations_allocate_and_deallocate_nothing() {
        let provider =
            crate::adapter::production_instruments::production_soundfont_capability().unwrap();
        let patch = Patch::new(
            PatchId::new(1).unwrap(),
            "Prepared piano".to_owned(),
            create_soundfont_config(&provider, SoundFontInstrument::new(0, 0, false).unwrap())
                .unwrap(),
            MidiChannel::new(0).unwrap(),
            PatchOutput::default(),
        );
        let preparer = HiDefSoundFontPreparer::new(
            crate::adapter::production_instruments::production_soundfont_asset().unwrap(),
        )
        .unwrap();
        let mut instrument = preparer.prepare(&patch, 48_000.0, 512).unwrap();
        let message =
            MidiMessage::try_new(patch.channel(), MidiMessageKind::NoteOn, 60, 100).unwrap();
        let mut output = [0.0; 1_024];
        let parameters = RtPatchParameters::new(patch.id(), patch.output());

        begin_memory_count();
        let dispatch = instrument.dispatch(message, &parameters);
        instrument.render(&mut output, 512, &parameters).unwrap();
        instrument.all_notes_off();
        let (allocations, deallocations) = finish_memory_count();

        assert_eq!(dispatch, Ok(()));
        assert_eq!(allocations, 0);
        assert_eq!(deallocations, 0);
        assert!(output.iter().all(|sample| sample.is_finite()));
    }

    // ---- Voice carry-over across topology activation ----------------------

    /// Preparer whose instruments render a distinct serial-numbered level, so
    /// a test can observe which prepared instance (live or fresh) is sounding
    /// after an activation without any engine-identity branch.
    struct SerialPreparer {
        capability_id: CapabilityId,
        serials: Arc<AtomicUsize>,
        drops: Arc<AtomicUsize>,
    }

    impl InstrumentPreparer for SerialPreparer {
        fn capability_id(&self) -> &CapabilityId {
            &self.capability_id
        }

        fn prepare(
            &self,
            patch: &Patch,
            _sample_rate: f32,
            _max_frames: usize,
        ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
            let serial = self.serials.fetch_add(1, Ordering::Relaxed) + 1;
            Ok(Box::new(SerialInstrument {
                patch_id: patch.id(),
                level: serial as f32 * 0.125,
                drops: Arc::clone(&self.drops),
            }))
        }
    }

    struct SerialInstrument {
        patch_id: PatchId,
        level: f32,
        drops: Arc<AtomicUsize>,
    }

    impl PreparedInstrument for SerialInstrument {
        fn patch_id(&self) -> PatchId {
            self.patch_id
        }

        fn dispatch(
            &mut self,
            _message: MidiMessage,
            _parameters: &crate::real_time::RtPatchParameters,
        ) -> Result<(), PreparedInstrumentError> {
            Ok(())
        }

        fn render(
            &mut self,
            output: &mut [f32],
            _frame_count: usize,
            _parameters: &crate::real_time::RtPatchParameters,
        ) -> Result<(), crate::synth::PreparedInstrumentError> {
            output.fill(self.level);

            Ok(())
        }

        fn all_notes_off(&mut self) {}
    }

    impl Drop for SerialInstrument {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }

    struct CarryOverFixture {
        patch: Patch,
        provider: HiDefSoundFontCapability,
        serials: Arc<AtomicUsize>,
        drops: Arc<AtomicUsize>,
    }

    impl CarryOverFixture {
        fn new() -> Self {
            let provider =
                crate::adapter::production_instruments::production_soundfont_capability().unwrap();
            let patch = Patch::new(
                PatchId::new(1).unwrap(),
                "Carry-over Patch".to_owned(),
                create_soundfont_config(&provider, SoundFontInstrument::new(0, 0, false).unwrap())
                    .unwrap(),
                MidiChannel::new(0).unwrap(),
                PatchOutput::to_track(MixerTrackId::new(0).unwrap()),
            );
            Self {
                patch,
                provider,
                serials: Arc::new(AtomicUsize::new(0)),
                drops: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn parameters(&self, revision: u64, generation: u64) -> ParameterSnapshot {
            ParameterSnapshot::for_graph(
                generation,
                GraphRevision::new(revision).unwrap(),
                GlobalParameters::new(0.0).unwrap(),
                MixerState::default(),
                &[RtPatchParameters::new(self.patch.id(), self.patch.output())],
            )
            .unwrap()
        }

        /// Builds one graph whose single instrument sounds the next serial
        /// level, so instance movement is audible.
        fn graph(&self, revision: u64, generation: u64) -> PreparedGraph {
            let registry = self.provider.registry().unwrap();
            let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(SerialPreparer {
                capability_id: CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap(),
                serials: Arc::clone(&self.serials),
                drops: Arc::clone(&self.drops),
            })];
            PreparedGraphBuilder::new(&registry, &preparers)
                .build(
                    GraphRevision::new(revision).unwrap(),
                    std::slice::from_ref(&self.patch),
                    self.parameters(revision, generation),
                    48_000.0,
                    4,
                )
                .unwrap()
        }
    }

    /// An occupancy-scoped activation exchanges the live prepared
    /// instrument into the replacement — the sounding instance survives, the
    /// observed notes survive, and the callback allocates, deallocates, and
    /// destroys nothing.
    #[test]
    fn occupancy_scoped_activation_carries_the_live_instrument_and_held_notes() {
        let fixture = CarryOverFixture::new();
        let source = fixture.graph(1, 10);
        let mut target = fixture.graph(2, 20);
        target.set_carry_over_scope(crate::real_time::GraphReplacementScope::PatchSlot {
            patch_id: fixture.patch.id(),
            slot: crate::synth::effect_slot_id::EffectSlotIndex::ALL[0],
        });

        let boundary = TestBoundary {
            commands: [
                Some(AudioCommand::patch_midi(
                    fixture.patch.id(),
                    midi(MidiMessageKind::NoteOn),
                )),
                None,
                None,
                None,
            ],
            next_command: 0,
            latest: fixture.parameters(2, 20),
            parameter_reads: 0,
        };
        let structural = TestStructural::new();
        let mut renderer = AudioRenderer::with_observation(
            boundary,
            structural,
            source,
            TestObservation::default(),
        );
        let mut output = [0.0_f32; 8];

        // The live instrument (serial 1) sounds before activation.
        renderer.render(&mut output);
        let live_block = output;
        assert_eq!(renderer.observation.latest.active_notes(), 1);

        renderer.structural.prepared[0] = Some(target);
        begin_memory_count();
        renderer.render(&mut output);
        let (allocations, deallocations) = finish_memory_count();

        assert_eq!(allocations, 0, "carry-over activation must not allocate");
        assert_eq!(
            deallocations, 0,
            "carry-over activation must not deallocate"
        );
        assert_eq!(
            fixture.drops.load(Ordering::Relaxed),
            0,
            "no prepared instrument may be destroyed by the callback"
        );
        assert_eq!(renderer.active_revision(), GraphRevision::new(2).unwrap());
        assert_eq!(
            output[..],
            live_block[..],
            "the live instance carried across the swap — no restart, no fresh instance"
        );
        assert_eq!(
            renderer.observation.latest.active_notes(),
            1,
            "held notes survive an occupancy-scoped activation"
        );
        // The superseded graph carries the fresh never-sounded instance and
        // retires off-callback intact.
        assert_eq!(renderer.structural.retired_count, 1);
    }

    /// Negative space: a `SelectedEngine` scope restarts exactly the
    /// selected Patch — the fresh instance sounds and its observed notes
    /// clear — and an unscoped replacement keeps the full-reset semantics.
    #[test]
    fn selected_engine_scope_restarts_only_the_selected_patch() {
        let fixture = CarryOverFixture::new();
        let source = fixture.graph(1, 10);
        let mut target = fixture.graph(2, 20);
        target.set_carry_over_scope(crate::real_time::GraphReplacementScope::SelectedEngine(
            fixture.patch.id(),
        ));

        let boundary = TestBoundary {
            commands: [
                Some(AudioCommand::patch_midi(
                    fixture.patch.id(),
                    midi(MidiMessageKind::NoteOn),
                )),
                None,
                None,
                None,
            ],
            next_command: 0,
            latest: fixture.parameters(2, 20),
            parameter_reads: 0,
        };
        let structural = TestStructural::new();
        let mut renderer = AudioRenderer::with_observation(
            boundary,
            structural,
            source,
            TestObservation::default(),
        );
        let mut output = [0.0_f32; 8];

        renderer.render(&mut output);
        let live_block = output;
        assert_eq!(renderer.observation.latest.active_notes(), 1);

        renderer.structural.prepared[0] = Some(target);
        renderer.render(&mut output);

        assert_eq!(renderer.active_revision(), GraphRevision::new(2).unwrap());
        assert_ne!(
            output[..],
            live_block[..],
            "the selected Patch's engine restarted with its fresh instance"
        );
        assert_eq!(
            renderer.observation.latest.active_notes(),
            0,
            "the restarted Patch's observed notes clear"
        );
    }

    #[test]
    fn active_note_observation_tracks_patch_lifecycle_with_bounded_bits() {
        let mut notes = ActiveNoteObservation::new();
        let patch = PatchId::new(3).unwrap();

        notes.observe_patch_message(patch, midi(MidiMessageKind::NoteOn));
        assert_eq!(notes.count(), 1);
        notes.observe_patch_message(patch, midi(MidiMessageKind::NoteOff));
        assert_eq!(notes.count(), 0);
        notes.observe_patch_message(patch, midi(MidiMessageKind::NoteOn));
        notes.observe_patch_message(patch, midi(MidiMessageKind::AllNotesOff));
        assert_eq!(notes.count(), 0);
    }

    #[test]
    fn source_observations_count_shared_pitches_and_recycle_graph_identities() {
        let mut notes = SourceNoteObservation::new();
        let patch = PatchId::new(3).unwrap();
        for source in [MidiSource::Immediate, MidiSource::Scheduled] {
            notes.observe_patch_message(source, patch, midi(MidiMessageKind::NoteOn));
        }
        assert_eq!(notes.count_patch(patch), 2);
        notes.observe_patch_message(MidiSource::Scheduled, patch, midi(MidiMessageKind::NoteOff));
        assert_eq!(notes.count_patch(patch), 1);
        notes.clear_source_patch(MidiSource::Scheduled, patch);
        assert_eq!(notes.count(), 1);
        notes.clear_all();
        for id in 1..=(MAX_ACTIVE_PATCHES as u32 * 2) {
            let patch = PatchId::new(id).unwrap();
            notes.observe_patch_message(
                MidiSource::Scheduled,
                patch,
                midi(MidiMessageKind::NoteOn),
            );
            assert_eq!(notes.count(), 1);
            notes.clear_source_patch(MidiSource::Scheduled, patch);
        }
        assert_eq!(notes.count(), 0);
    }

    #[test]
    fn unsupported_scheduled_source_reports_failure_without_dispatch_or_allocation() {
        let fixture = Fixture::new();
        let patch = fixture.patches[0].id();
        let boundary = fixture.boundary(fixture.parameters(1, 1), &[]);
        let mut renderer = AudioRenderer::new(boundary, TestStructural::new(), fixture.graph(1, 1));
        begin_memory_count();
        let accepted = renderer.dispatch_patch_midi(
            MidiSource::Scheduled,
            patch,
            midi(MidiMessageKind::NoteOn),
        );
        let memory = finish_memory_count();
        assert!(!accepted);
        assert_eq!(memory, (0, 0));
        assert_eq!(renderer.routing_failures, 1);
        assert_eq!(renderer.active_notes.count(), 0);
        assert_eq!(fixture.first_dispatches.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn prepared_graph_exposes_fixed_patch_stem_storage() {
        let fixture = Fixture::new();
        let graph = fixture.graph(1, 1);
        let stems: &[crate::real_time::patch_audio_block::PatchStereoStem] =
            graph.patch_audio().stems();

        assert!(stems.is_empty());
        assert_eq!(graph.patch_audio().storage().len(), MAX_ACTIVE_PATCHES);
    }

    // ---- Canonical voice limit enforcement (FR-009) -----------------------

    /// Enforcement measured through the production render path: the Patch's own
    /// canonical limit is projected by the production `ParameterSnapshot`
    /// projection, published on the latest-scalar transport, and read by the
    /// real `AudioRenderer::render`. Nothing here writes a limit straight into a
    /// snapshot, so a limit that never left the aggregate would fail rather than
    /// pass.
    mod voice_limit {
        use super::*;

        struct LimitFixture {
            provider: HiDefSoundFontCapability,
            patches: Vec<Patch>,
            dispatches: Arc<AtomicUsize>,
            unused_dispatches: Arc<AtomicUsize>,
            drops: Arc<AtomicUsize>,
        }

        impl LimitFixture {
            fn new(limit: u16) -> Self {
                let provider =
                    crate::adapter::production_instruments::production_soundfont_capability()
                        .unwrap();
                let mut patch = Patch::new(
                    PatchId::new(1).unwrap(),
                    "Limited Patch".to_owned(),
                    create_soundfont_config(
                        &provider,
                        SoundFontInstrument::new(0, 0, false).unwrap(),
                    )
                    .unwrap(),
                    MidiChannel::new(0).unwrap(),
                    PatchOutput::to_track(MixerTrackId::new(0).unwrap()),
                );
                patch.set_voice_limit(limit).unwrap();
                Self {
                    provider,
                    patches: vec![patch],
                    dispatches: Arc::new(AtomicUsize::new(0)),
                    unused_dispatches: Arc::new(AtomicUsize::new(0)),
                    drops: Arc::new(AtomicUsize::new(0)),
                }
            }

            fn patch_id(&self) -> PatchId {
                self.patches[0].id()
            }

            fn dispatched(&self) -> usize {
                self.dispatches.load(Ordering::Relaxed)
            }

            /// Projects canonical Patch state through the production
            /// projection, optionally re-stating the Patch's limit first.
            fn parameters(&self, generation: u64, limit: Option<u16>) -> ParameterSnapshot {
                let registry = self.provider.registry().unwrap();
                let patches: Vec<Patch> = self
                    .patches
                    .iter()
                    .cloned()
                    .map(|mut patch| {
                        if let Some(value) = limit {
                            patch.set_voice_limit(value).unwrap();
                        }
                        patch
                    })
                    .collect();
                ParameterSnapshot::project_patches(
                    generation,
                    GraphRevision::new(1).unwrap(),
                    GlobalParameters::new(0.0).unwrap(),
                    MixerState::default(),
                    &patches,
                    &registry,
                )
                .unwrap()
            }

            fn graph(&self) -> PreparedGraph {
                let registry = self.provider.registry().unwrap();
                let preparers: Vec<Box<dyn InstrumentPreparer>> = vec![Box::new(FixturePreparer {
                    capability_id: CapabilityId::new(HIDEF_CAPABILITY_ID).unwrap(),
                    first_dispatches: Arc::clone(&self.dispatches),
                    render_failure: Arc::new(AtomicBool::new(false)),
                    second_dispatches: Arc::clone(&self.unused_dispatches),
                    drops: Arc::clone(&self.drops),
                })];
                PreparedGraphBuilder::new(&registry, &preparers)
                    .build(
                        GraphRevision::new(1).unwrap(),
                        &self.patches,
                        self.parameters(1, None),
                        48_000.0,
                        4,
                    )
                    .unwrap()
            }

            fn note_on(&self, note: u8) -> AudioCommand {
                self.command(MidiMessageKind::NoteOn, note, 100)
            }

            fn command(&self, kind: MidiMessageKind, data1: u8, data2: u8) -> AudioCommand {
                AudioCommand::patch_midi(
                    self.patch_id(),
                    MidiMessage::try_new(MidiChannel::new(0).unwrap(), kind, data1, data2).unwrap(),
                )
            }
        }

        /// A wider command bank than the shared `TestBoundary`, so one block can
        /// carry more note-ons than any limit under test.
        struct LimitBoundary {
            commands: [Option<AudioCommand>; 24],
            next_command: usize,
            latest: ParameterSnapshot,
        }

        impl LimitBoundary {
            fn new(latest: ParameterSnapshot, commands: &[AudioCommand]) -> Self {
                let mut boundary = Self {
                    commands: [None; 24],
                    next_command: 0,
                    latest: latest.clone(),
                };
                boundary.reload(latest, commands);
                boundary
            }

            fn reload(&mut self, latest: ParameterSnapshot, commands: &[AudioCommand]) {
                assert!(commands.len() <= self.commands.len());
                self.commands = [None; 24];
                for (slot, command) in self.commands.iter_mut().zip(commands) {
                    *slot = Some(*command);
                }
                self.next_command = 0;
                self.latest = latest;
            }
        }

        impl AudioThreadBoundary for LimitBoundary {
            fn pop_command(&mut self) -> Option<AudioCommand> {
                let command = self.commands.get_mut(self.next_command)?.take();
                if command.is_some() {
                    self.next_command += 1;
                }
                command
            }

            fn exchange_latest_parameters(&mut self, previous: &mut ParameterSnapshot) -> bool {
                if self.latest.generation() < previous.generation() {
                    return false;
                }
                core::mem::swap(&mut self.latest, previous);
                true
            }
        }

        fn renderer(
            fixture: &LimitFixture,
            commands: &[AudioCommand],
            limit: Option<u16>,
        ) -> AudioRenderer<LimitBoundary, TestStructural, TestObservation> {
            let boundary = LimitBoundary::new(fixture.parameters(2, limit), commands);
            AudioRenderer::with_observation(
                boundary,
                TestStructural::new(),
                fixture.graph(),
                TestObservation::default(),
            )
        }

        /// The load-bearing claim. A Patch already sounding its limit does not
        /// start the arriving note: the instrument is never told, no note bit is
        /// set, every sounding voice stays latched, and the refusal is counted.
        ///
        /// This is the test T006 falsifies. Removing the refusal branch in
        /// `render_bounded_block` makes it fail on the dispatch count and on the
        /// refusal count together.
        #[test]
        fn a_patch_already_sounding_its_limit_does_not_start_the_note() {
            let fixture = LimitFixture::new(3);
            let commands: Vec<AudioCommand> =
                (60_u8..65).map(|note| fixture.note_on(note)).collect();
            let mut renderer = renderer(&fixture, &commands, None);
            let mut output = [0.0_f32; 8];

            renderer.render(&mut output);

            assert_eq!(
                fixture.dispatched(),
                3,
                "only the first three note-ons may reach the prepared instrument"
            );
            assert_eq!(
                renderer.active_notes.count_patch(fixture.patch_id()),
                3,
                "a refused note-on must not set a note bit"
            );
            assert_eq!(renderer.observation.latest.active_notes(), 3);
            assert_eq!(
                renderer.observation.latest.voice_limit_refusals(),
                2,
                "both note-ons beyond the limit are counted, once each"
            );
            assert_eq!(
                renderer.observation.latest.commands_consumed(),
                5,
                "a refused note-on is still a consumed command"
            );
            assert_eq!(renderer.observation.latest.routing_failures(), 0);
            assert_eq!(
                fixture.drops.load(Ordering::Relaxed),
                0,
                "refusing a start must never destroy a prepared instrument"
            );
        }

        /// Both halves of the lowered-limit edge case: voices already sounding
        /// are untouched by a limit lowered beneath them, and the new limit
        /// applies at the very next note-on.
        #[test]
        fn a_limit_lowered_beneath_the_sounding_count_leaves_those_voices_latched() {
            let fixture = LimitFixture::new(4);
            let commands: Vec<AudioCommand> = [60_u8, 62, 64, 65]
                .iter()
                .map(|note| fixture.note_on(*note))
                .collect();
            let mut renderer = renderer(&fixture, &commands, None);
            let mut output = [0.0_f32; 8];

            renderer.render(&mut output);
            assert_eq!(fixture.dispatched(), 4);
            assert_eq!(renderer.active_notes.count_patch(fixture.patch_id()), 4);
            assert_eq!(renderer.observation.latest.voice_limit_refusals(), 0);

            renderer
                .boundary
                .reload(fixture.parameters(3, Some(2)), &[fixture.note_on(67)]);
            renderer.render(&mut output);

            assert_eq!(
                renderer.active_notes.count_patch(fixture.patch_id()),
                4,
                "a lowered limit never reaches into the callback to end latched voices"
            );
            assert_eq!(
                fixture.dispatched(),
                4,
                "the next note-on is refused under the lowered limit"
            );
            assert_eq!(renderer.observation.latest.voice_limit_refusals(), 1);
            assert_eq!(fixture.drops.load(Ordering::Relaxed), 0);
        }

        /// A limit that could swallow a note-off would strand a latched voice,
        /// so every message that is not a sounding note-on falls straight
        /// through — including a zero-velocity note-on, which is a release.
        #[test]
        fn the_limit_never_refuses_a_release_or_any_non_note_message() {
            let fixture = LimitFixture::new(1);
            let commands = [
                fixture.note_on(60),
                fixture.command(MidiMessageKind::ControlChange, 7, 90),
                fixture.command(MidiMessageKind::ProgramChange, 3, 0),
                fixture.command(MidiMessageKind::ChannelPressure, 40, 0),
                fixture.command(MidiMessageKind::PitchBend, 0, 96),
                fixture.command(MidiMessageKind::NoteOn, 61, 0),
                fixture.command(MidiMessageKind::NoteOff, 60, 0),
                fixture.command(MidiMessageKind::AllNotesOff, 0, 0),
            ];
            let mut renderer = renderer(&fixture, &commands, None);
            let mut output = [0.0_f32; 8];

            renderer.render(&mut output);

            assert_eq!(
                fixture.dispatched(),
                commands.len(),
                "every message reached the instrument; the limit refused none of them"
            );
            assert_eq!(
                renderer.observation.latest.voice_limit_refusals(),
                0,
                "nothing but a sounding note-on beyond the limit may be refused"
            );
            assert_eq!(
                renderer.active_notes.count_patch(fixture.patch_id()),
                0,
                "the release and the all-notes-off both landed"
            );
        }

        /// The limit check must not shadow the routing-failure path: an identity
        /// the snapshot does not carry is still one preserved routing failure.
        #[test]
        fn an_unknown_patch_identity_still_reports_a_routing_failure() {
            let fixture = LimitFixture::new(1);
            let unknown = PatchId::new(99).unwrap();
            let commands = [
                fixture.note_on(60),
                AudioCommand::patch_midi(
                    unknown,
                    MidiMessage::try_new(
                        MidiChannel::new(0).unwrap(),
                        MidiMessageKind::NoteOn,
                        62,
                        100,
                    )
                    .unwrap(),
                ),
                fixture.note_on(64),
            ];
            let mut renderer = renderer(&fixture, &commands, None);
            let mut output = [0.0_f32; 8];

            renderer.render(&mut output);

            assert_eq!(
                renderer.observation.latest.routing_failures(),
                1,
                "the unknown identity is reported as a routing failure, not as a refusal"
            );
            assert_eq!(
                renderer.observation.latest.last_unknown_patch_id(),
                Some(unknown)
            );
            assert_eq!(
                renderer.observation.latest.voice_limit_refusals(),
                1,
                "the known Patch's own second note-on is the one refusal"
            );
            assert_eq!(fixture.dispatched(), 1);
        }

        /// The callback contract holds while the limit is biting: no allocation,
        /// no deallocation, and no prepared instrument destroyed.
        #[test]
        fn enforcing_the_limit_allocates_nothing_and_destroys_nothing() {
            let fixture = LimitFixture::new(2);
            let mut renderer = renderer(&fixture, &[], None);
            let mut output = [0.0_f32; 8];

            // Warm the block once so the measurement covers steady-state
            // rendering with enforcement, not first-block preparation.
            renderer.render(&mut output);

            let commands: Vec<AudioCommand> =
                (60_u8..66).map(|note| fixture.note_on(note)).collect();
            renderer
                .boundary
                .reload(fixture.parameters(3, None), &commands);

            begin_memory_count();
            renderer.render(&mut output);
            let (allocations, deallocations) = finish_memory_count();

            assert_eq!(
                renderer.observation.latest.voice_limit_refusals(),
                4,
                "the measured block actually refused notes"
            );
            assert_eq!(allocations, 0, "enforcement must not allocate");
            assert_eq!(deallocations, 0, "enforcement must not deallocate");
            assert_eq!(
                fixture.drops.load(Ordering::Relaxed),
                0,
                "enforcement must destroy nothing"
            );
            assert!(output.iter().all(|sample| sample.is_finite()));
        }

        /// The refusal counter saturates like every other callback counter,
        /// rather than wrapping back through zero and reporting a limit that
        /// never bit.
        ///
        /// The counter is placed one below its ceiling and then driven past it
        /// by three real refusals, so the assertion discriminates three ways at
        /// once: wrapping would land on 1, an uncounted refusal would leave it
        /// one short, and a defeated limit would leave it two short.
        #[test]
        fn the_refusal_counter_saturates_rather_than_wrapping() {
            let fixture = LimitFixture::new(1);
            let mut renderer = renderer(&fixture, &[fixture.note_on(60)], None);
            let mut output = [0.0_f32; 8];

            renderer.render(&mut output);
            assert_eq!(renderer.observation.latest.voice_limit_refusals(), 0);
            assert_eq!(renderer.active_notes.count_patch(fixture.patch_id()), 1);

            renderer.seed_voice_limit_refusals(u64::MAX - 1);
            let beyond: Vec<AudioCommand> = [62_u8, 64, 65]
                .iter()
                .map(|n| fixture.note_on(*n))
                .collect();
            renderer
                .boundary
                .reload(fixture.parameters(3, None), &beyond);
            renderer.render(&mut output);

            assert_eq!(
                renderer.observation.latest.voice_limit_refusals(),
                u64::MAX,
                "the counter holds at its ceiling instead of wrapping to zero"
            );
        }
    }

    // ---- Full-occupancy measurements (C-RT-1/3/7, NFR-001/002/003) --------

    mod full_occupancy {
        use super::{
            begin_memory_count, finish_memory_count, midi, AudioCommand, AudioRenderer,
            MidiMessageKind, TestBoundary, TestStructural,
        };
        use crate::adapter::lock_free_audio_boundary::LockFreeAudioBoundary;
        use crate::adapter::production_effects::{
            production_effect_preparers, production_effect_registry,
        };
        use crate::kernel::midi_channel::MidiChannel;
        use crate::kernel::midi_message::MidiMessage;
        use crate::kernel::patch_id::PatchId;
        use crate::mixer::bus_id::{BusId, MAX_BUS_RETURNS};
        use crate::mixer::global_parameters::GlobalParameters;
        use crate::mixer::mixer_state::MixerState;
        use crate::mixer::mixer_track_id::MixerTrackId;
        use crate::mixer::mixer_track_parameters::MixerTrackParameters;
        use crate::mixer::patch_output::PatchOutput;
        use crate::real_time::audio_boundary::{
            AudioBoundary, AudioThreadBoundary, ControlAudioBoundary,
        };
        use crate::real_time::graph_revision::GraphRevision;
        use crate::real_time::parameter_snapshot::{ParameterSnapshot, MAX_ACTIVE_PATCHES};
        use crate::real_time::prepared_graph::PreparedGraph;
        use crate::real_time::prepared_graph_builder::PreparedGraphBuilder;
        use crate::synth::capability_id::CapabilityId;
        use crate::synth::effect_slot_id::{EffectSlotIndex, MAX_EFFECT_SLOTS};
        use crate::synth::instrument_preparer::{InstrumentPreparationError, InstrumentPreparer};
        use crate::synth::patch::Patch;
        use crate::synth::prepared_instrument::{PreparedInstrument, PreparedInstrumentError};
        use crate::synth::sound_font_instrument::SoundFontInstrument;
        use crate::synth::{
            EffectCapabilityRegistry, EffectPreparer, EffectSlotId, PostEffectConfig,
        };
        use crate::testing::automatic_midi_test::create_soundfont_config;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        const SAMPLE_RATE: f32 = 48_000.0;
        const MAX_FRAMES: usize = 64;

        struct CountingPreparer {
            capability_id: CapabilityId,
            drops: Arc<AtomicUsize>,
        }

        impl InstrumentPreparer for CountingPreparer {
            fn capability_id(&self) -> &CapabilityId {
                &self.capability_id
            }

            fn prepare(
                &self,
                patch: &Patch,
                _sample_rate: f32,
                _max_frames: usize,
            ) -> Result<Box<dyn PreparedInstrument>, InstrumentPreparationError> {
                Ok(Box::new(CountingInstrument {
                    patch_id: patch.id(),
                    drops: Arc::clone(&self.drops),
                }))
            }
        }

        struct CountingInstrument {
            patch_id: PatchId,
            drops: Arc<AtomicUsize>,
        }

        impl PreparedInstrument for CountingInstrument {
            fn patch_id(&self) -> PatchId {
                self.patch_id
            }

            fn dispatch(
                &mut self,
                _message: MidiMessage,
                _parameters: &crate::real_time::RtPatchParameters,
            ) -> Result<(), PreparedInstrumentError> {
                Ok(())
            }

            fn render(
                &mut self,
                output: &mut [f32],
                _frame_count: usize,
                _parameters: &crate::real_time::RtPatchParameters,
            ) -> Result<(), crate::synth::PreparedInstrumentError> {
                output.fill(0.05);

                Ok(())
            }

            fn all_notes_off(&mut self) {}
        }

        impl Drop for CountingInstrument {
            fn drop(&mut self) {
                self.drops.fetch_add(1, Ordering::Relaxed);
            }
        }

        /// The complete fully occupied configuration this mission must carry:
        /// 16 Patches x 3 occupied effect positions, 16 tracks x 8 non-zero
        /// sends, and all 8 bus returns occupied.
        struct FullOccupancyFixture {
            provider: crate::adapter::hidef_soundfont_capability::HiDefSoundFontCapability,
            patches: Vec<Patch>,
            effect_registry: EffectCapabilityRegistry,
            effect_preparers: Vec<Box<dyn EffectPreparer>>,
            drops: Arc<AtomicUsize>,
        }

        impl FullOccupancyFixture {
            fn new() -> Self {
                let provider =
                    crate::adapter::production_instruments::production_soundfont_capability()
                        .unwrap();
                let effect_registry = production_effect_registry().unwrap();
                let capabilities = ["effect.chorus", "effect.reverb", "effect.delay"];
                let patches = (1..=MAX_ACTIVE_PATCHES as u32)
                    .map(|id| {
                        let occupants: Vec<(EffectSlotIndex, PostEffectConfig)> =
                            EffectSlotIndex::ALL
                                .into_iter()
                                .map(|address| {
                                    let position = address.index();
                                    let slot = EffectSlotId::new(
                                        ((id - 1) as u16) * MAX_EFFECT_SLOTS as u16
                                            + position as u16
                                            + 1,
                                    )
                                    .unwrap();
                                    let config = effect_registry
                                        .descriptors()
                                        .iter()
                                        .find(|descriptor| {
                                            descriptor.id().as_str() == capabilities[position]
                                        })
                                        .unwrap()
                                        .default_config(slot)
                                        .unwrap();
                                    (address, config)
                                })
                                .collect();
                        let patch = Patch::new(
                            PatchId::new(id).unwrap(),
                            format!("Full {id}"),
                            create_soundfont_config(
                                &provider,
                                SoundFontInstrument::new(0, (id - 1) as u8, false).unwrap(),
                            )
                            .unwrap(),
                            MidiChannel::new(((id - 1) % 16) as u8).unwrap(),
                            PatchOutput::to_track(
                                MixerTrackId::new(((id - 1) % 16) as u8).unwrap(),
                            ),
                        );
                        occupants
                            .into_iter()
                            .fold(patch, |patch, (address, config)| {
                                patch.with_effect_slot(address, config)
                            })
                    })
                    .collect();
                Self {
                    provider,
                    patches,
                    effect_registry,
                    effect_preparers: production_effect_preparers().unwrap(),
                    drops: Arc::new(AtomicUsize::new(0)),
                }
            }

            fn mixer_state(&self) -> MixerState {
                let mut mixer = MixerState::default();
                for track_id in MixerTrackId::ALL {
                    let mut sends = [0.0; MAX_BUS_RETURNS];
                    for (index, send) in sends.iter_mut().enumerate() {
                        *send = 0.1 + index as f32 * 0.05;
                    }
                    mixer.set_track(
                        track_id,
                        MixerTrackParameters::from_values(-6.0, 0.0, false, false, sends).unwrap(),
                    );
                }
                mixer
            }

            fn global() -> GlobalParameters {
                GlobalParameters::new(0.0).unwrap()
            }

            /// The canonical fully occupied bank: the production defaults on
            /// buses 0 and 1 plus a chorus occupant on every remaining bus.
            fn full_bank(&self) -> crate::mixer::bus_return::BusReturnBank {
                let chorus = crate::synth::EffectCapabilityId::new("effect.chorus").unwrap();
                let mut bank = crate::adapter::production_effects::production_default_bus_returns(
                    &self.effect_registry,
                )
                .unwrap();
                for bus_value in 2..MAX_BUS_RETURNS as u16 {
                    let bus = BusId::new(bus_value).unwrap();
                    bank.set_return_occupancy(&self.effect_registry, bus, Some(&chorus))
                        .unwrap();
                    bank.set_return_level(bus, 0.4).unwrap();
                }
                bank
            }

            /// Projects the complete fully occupied snapshot: every effect
            /// position live, every send non-zero, all eight returns active.
            fn full_snapshot(&self, revision: u64, generation: u64) -> ParameterSnapshot {
                let registry = self.provider.registry().unwrap();
                ParameterSnapshot::project_patches_with_effects_and_returns(
                    generation,
                    GraphRevision::new(revision).unwrap(),
                    Self::global(),
                    self.mixer_state(),
                    &self.patches,
                    &registry,
                    &self.effect_registry,
                    &self.full_bank(),
                )
                .unwrap()
            }

            /// Builds one callback-ready graph whose racks occupy the complete
            /// widened capacity, with its initial parameters refreshed to the
            /// matching fully occupied snapshot.
            fn full_graph(
                &self,
                revision: u64,
                generation: u64,
            ) -> (PreparedGraph, ParameterSnapshot) {
                let registry = self.provider.registry().unwrap();
                let preparers: Vec<Box<dyn InstrumentPreparer>> =
                    vec![Box::new(CountingPreparer {
                        capability_id: CapabilityId::new(
                            crate::adapter::hidef_soundfont_capability::HIDEF_CAPABILITY_ID,
                        )
                        .unwrap(),
                        drops: Arc::clone(&self.drops),
                    })];
                let snapshot = self.full_snapshot(revision, generation);
                let bank = self.full_bank();
                let graph = PreparedGraphBuilder::new(&registry, &preparers)
                    .with_effects(&self.effect_registry, &self.effect_preparers)
                    .with_returns(&bank)
                    .build(
                        GraphRevision::new(revision).unwrap(),
                        &self.patches,
                        snapshot.clone(),
                        SAMPLE_RATE,
                        MAX_FRAMES,
                    )
                    .unwrap();
                (graph, snapshot)
            }
        }

        fn assert_fully_occupied(snapshot: &ParameterSnapshot) {
            assert_eq!(snapshot.patch_count(), MAX_ACTIVE_PATCHES);
            assert!(snapshot
                .patches()
                .iter()
                .all(|patch| patch.effects().iter().all(|effect| effect.is_active())));
            assert!(snapshot
                .mixer_tracks()
                .iter()
                .all(|track| track.sends().iter().all(|send| *send > 0.0)));
            assert!(snapshot.returns().iter().all(|entry| entry.is_active()));
        }

        /// C-RT-3/NFR-002: a steady-state block and a topology-activation
        /// block at the complete widened occupancy perform zero allocation,
        /// zero deallocation, and zero destruction; the superseded graph is
        /// returned off-callback intact; activation is atomic at block start.
        #[test]
        fn fully_occupied_render_and_activation_stay_allocation_and_destruction_free() {
            let fixture = FullOccupancyFixture::new();
            let (source, source_snapshot) = fixture.full_graph(1, 10);
            let (target, target_snapshot) = fixture.full_graph(2, 20);
            assert_fully_occupied(&source_snapshot);
            assert_fully_occupied(&target_snapshot);
            assert_eq!(source.effect_rack().patch_count(), MAX_ACTIVE_PATCHES);
            for index in 0..MAX_ACTIVE_PATCHES {
                assert_eq!(
                    source.effect_rack().occupied_slot_count(index),
                    MAX_EFFECT_SLOTS
                );
            }
            assert_eq!(source.bus_return_rack().occupied_count(), MAX_BUS_RETURNS);

            let patch_id = fixture.patches[0].id();
            let boundary = TestBoundary {
                commands: [
                    Some(AudioCommand::patch_midi(
                        patch_id,
                        midi(MidiMessageKind::NoteOn),
                    )),
                    None,
                    None,
                    None,
                ],
                next_command: 0,
                latest: target_snapshot,
                parameter_reads: 0,
            };
            let structural = TestStructural::new();
            let mut renderer = AudioRenderer::new(boundary, structural, source);
            let mut output = [0.0_f32; MAX_FRAMES * 2];

            // Steady state at full occupancy: enough blocks for the wet-only
            // delay stage (250 ms) to reach the output, all measured.
            begin_memory_count();
            let mut any_audible = false;
            for _ in 0..256 {
                renderer.render(&mut output);
                assert!(output.iter().all(|sample| sample.is_finite()));
                any_audible |= output.iter().any(|sample| sample.abs() > 0.0);
            }
            let (allocations, deallocations) = finish_memory_count();
            assert_eq!(allocations, 0, "steady-state blocks must not allocate");
            assert_eq!(deallocations, 0, "steady-state blocks must not deallocate");
            assert!(any_audible, "the fully occupied graph must produce audio");
            assert_eq!(renderer.parameters().generation(), 10);

            // Topology activation at full occupancy, exactly at block start.
            renderer.structural.prepared[0] = Some(target);
            begin_memory_count();
            renderer.render(&mut output);
            let (allocations, deallocations) = finish_memory_count();
            assert_eq!(allocations, 0, "activation block must not allocate");
            assert_eq!(deallocations, 0, "activation block must not deallocate");
            assert_eq!(
                fixture.drops.load(Ordering::Relaxed),
                0,
                "no prepared instrument may be destroyed by the callback"
            );
            assert_eq!(renderer.active_revision(), GraphRevision::new(2).unwrap());
            // The rendered block observes the replacement's complete snapshot:
            // no partially applied topology is observable (NFR-003, C-RT-9).
            assert_eq!(renderer.parameters().generation(), 20);
            assert!(renderer
                .parameters()
                .audio_values_equal(&fixture.full_snapshot(2, 20)));
            assert_eq!(renderer.structural.retired_count, 1);
            assert_eq!(
                renderer.structural.retired[0].as_ref().unwrap().revision(),
                GraphRevision::new(1).unwrap()
            );
            // The replacement graph starts with empty wet stages, so the
            // activation block is finite but may legitimately be silent.
            assert!(output.iter().all(|sample| sample.is_finite()));
        }

        /// Snapshot storage is prepared by the producer. The callback only
        /// exchanges ownership; superseded allocations return to that producer.
        #[test]
        fn snapshot_exchange_is_allocation_and_destruction_free() {
            let fixture = FullOccupancyFixture::new();
            let snapshot = fixture.full_snapshot(1, 1);
            assert_fully_occupied(&snapshot);
            let boundary = LockFreeAudioBoundary::new(4, snapshot.clone());
            let (mut control, mut audio) = boundary.into_handles();
            let mut active = snapshot.clone();
            for generation in 2..128 {
                control.publish_parameters(snapshot.clone().with_generation(generation));
                begin_memory_count();
                let updated = audio.exchange_latest_parameters(&mut active);
                let repeated = audio.exchange_latest_parameters(&mut active);
                let (allocations, deallocations) = finish_memory_count();
                assert!(updated);
                assert!(!repeated);
                assert_eq!(active.generation(), generation);
                assert_eq!((allocations, deallocations), (0, 0));
            }
        }
    }
}
