use crate::kernel::patch_id::PatchId;
use crate::mixer::mix_observation::MixObservation;
use crate::mixer::mixer_track_id::{MixerTrackId, MixerTrackId as TrackId};
use crate::mixer::track_meter::TrackMeter;
use crate::real_time::{GraphRevision, PatchEffectObservation, PreviewAudioObservation};
use serde::Serialize;

/// Fixed-size numeric evidence from one completed real-time render block.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioObservationSnapshot {
    sequence: u64,
    rendered_blocks: u64,
    rendered_frames: u64,
    parameter_generation: u64,
    active_graph_revision: GraphRevision,
    commands_consumed: u64,
    active_notes: u32,
    routing_failures: u64,
    /// How many note-ons the Patch's snapshot-carried voice limit refused.
    ///
    /// This is what makes the limit falsifiable: a limit defeated inside the
    /// callback shows zero refusals under a fixture that must exceed it, which
    /// is a failing predicate rather than an unnoticed absence of silence.
    voice_limit_refusals: u64,
    scheduled_late_events: u64,
    scheduled_max_lateness_frames: u64,
    scheduled_queue_high_water: u64,
    last_unknown_patch_id: Option<PatchId>,
    primary_patch_id: Option<PatchId>,
    primary_patch_rms: f32,
    primary_active_notes: u32,
    patch_effect: PatchEffectObservation,
    preview_identity: u64,
    preview_patch_id: Option<PatchId>,
    preview_playing: bool,
    preview_playhead: f32,
    tracks: [TrackMeter; MixerTrackId::COUNT],
    left_peak: f32,
    right_peak: f32,
    output_rms: f32,
    reverb_input_rms: f32,
    delay_input_rms: f32,
    wet_output_rms: f32,
    non_finite_samples: u64,
    clipped_samples: u64,
}

impl AudioObservationSnapshot {
    pub const fn from_mix(
        sequence: u64,
        rendered_blocks: u64,
        rendered_frames: u64,
        parameter_generation: u64,
        commands_consumed: u64,
        active_notes: u32,
        mix: MixObservation,
    ) -> Self {
        Self::from_mix_with_routing(
            sequence,
            rendered_blocks,
            rendered_frames,
            parameter_generation,
            commands_consumed,
            active_notes,
            0,
            None,
            mix,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub const fn from_mix_with_routing(
        sequence: u64,
        rendered_blocks: u64,
        rendered_frames: u64,
        parameter_generation: u64,
        commands_consumed: u64,
        active_notes: u32,
        routing_failures: u64,
        last_unknown_patch_id: Option<PatchId>,
        mix: MixObservation,
    ) -> Self {
        Self::from_mix_with_graph_and_routing(
            sequence,
            rendered_blocks,
            rendered_frames,
            parameter_generation,
            GraphRevision::INITIAL,
            commands_consumed,
            active_notes,
            routing_failures,
            last_unknown_patch_id,
            mix,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub const fn from_mix_with_graph_and_routing(
        sequence: u64,
        rendered_blocks: u64,
        rendered_frames: u64,
        parameter_generation: u64,
        active_graph_revision: GraphRevision,
        commands_consumed: u64,
        active_notes: u32,
        routing_failures: u64,
        last_unknown_patch_id: Option<PatchId>,
        mix: MixObservation,
    ) -> Self {
        Self::from_mix_with_graph_routing_and_primary(
            sequence,
            rendered_blocks,
            rendered_frames,
            parameter_generation,
            active_graph_revision,
            commands_consumed,
            active_notes,
            routing_failures,
            last_unknown_patch_id,
            None,
            0.0,
            0,
            mix,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub const fn from_mix_with_graph_routing_and_primary(
        sequence: u64,
        rendered_blocks: u64,
        rendered_frames: u64,
        parameter_generation: u64,
        active_graph_revision: GraphRevision,
        commands_consumed: u64,
        active_notes: u32,
        routing_failures: u64,
        last_unknown_patch_id: Option<PatchId>,
        primary_patch_id: Option<PatchId>,
        primary_patch_rms: f32,
        primary_active_notes: u32,
        mix: MixObservation,
    ) -> Self {
        Self::from_mix_with_graph_routing_primary_and_effect(
            sequence,
            rendered_blocks,
            rendered_frames,
            parameter_generation,
            active_graph_revision,
            commands_consumed,
            active_notes,
            routing_failures,
            last_unknown_patch_id,
            primary_patch_id,
            primary_patch_rms,
            primary_active_notes,
            PatchEffectObservation::EMPTY,
            mix,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub const fn from_mix_with_graph_routing_primary_and_effect(
        sequence: u64,
        rendered_blocks: u64,
        rendered_frames: u64,
        parameter_generation: u64,
        active_graph_revision: GraphRevision,
        commands_consumed: u64,
        active_notes: u32,
        routing_failures: u64,
        last_unknown_patch_id: Option<PatchId>,
        primary_patch_id: Option<PatchId>,
        primary_patch_rms: f32,
        primary_active_notes: u32,
        patch_effect: PatchEffectObservation,
        mix: MixObservation,
    ) -> Self {
        Self {
            sequence,
            rendered_blocks,
            rendered_frames,
            parameter_generation,
            active_graph_revision,
            commands_consumed,
            active_notes,
            routing_failures,
            voice_limit_refusals: 0,
            scheduled_late_events: 0,
            scheduled_max_lateness_frames: 0,
            scheduled_queue_high_water: 0,
            last_unknown_patch_id,
            primary_patch_id,
            primary_patch_rms,
            primary_active_notes,
            patch_effect,
            preview_identity: 0,
            preview_patch_id: None,
            preview_playing: false,
            preview_playhead: 0.0,
            tracks: mix.tracks(),
            left_peak: mix.left_peak(),
            right_peak: mix.right_peak(),
            output_rms: mix.output_rms(),
            reverb_input_rms: mix.reverb_input_rms(),
            delay_input_rms: mix.delay_input_rms(),
            wet_output_rms: mix.wet_output_rms(),
            non_finite_samples: mix.non_finite_samples(),
            clipped_samples: mix.clipped_samples(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[cfg(test)]
    pub(crate) const fn from_parts(
        sequence: u64,
        rendered_blocks: u64,
        rendered_frames: u64,
        parameter_generation: u64,
        commands_consumed: u64,
        active_notes: u32,
        left_peak: f32,
        right_peak: f32,
        output_rms: f32,
        reverb_input_rms: f32,
        delay_input_rms: f32,
        wet_output_rms: f32,
        non_finite_samples: u64,
        clipped_samples: u64,
    ) -> Self {
        Self::from_parts_with_routing(
            sequence,
            rendered_blocks,
            rendered_frames,
            parameter_generation,
            commands_consumed,
            active_notes,
            0,
            None,
            left_peak,
            right_peak,
            output_rms,
            reverb_input_rms,
            delay_input_rms,
            wet_output_rms,
            non_finite_samples,
            clipped_samples,
        )
    }

    #[allow(clippy::too_many_arguments)]
    #[cfg(test)]
    pub(crate) const fn from_parts_with_routing(
        sequence: u64,
        rendered_blocks: u64,
        rendered_frames: u64,
        parameter_generation: u64,
        commands_consumed: u64,
        active_notes: u32,
        routing_failures: u64,
        last_unknown_patch_id: Option<PatchId>,
        left_peak: f32,
        right_peak: f32,
        output_rms: f32,
        reverb_input_rms: f32,
        delay_input_rms: f32,
        wet_output_rms: f32,
        non_finite_samples: u64,
        clipped_samples: u64,
    ) -> Self {
        Self::from_parts_with_graph_and_routing(
            sequence,
            rendered_blocks,
            rendered_frames,
            parameter_generation,
            GraphRevision::INITIAL,
            commands_consumed,
            active_notes,
            routing_failures,
            last_unknown_patch_id,
            left_peak,
            right_peak,
            output_rms,
            reverb_input_rms,
            delay_input_rms,
            wet_output_rms,
            non_finite_samples,
            clipped_samples,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn from_parts_with_graph_and_routing(
        sequence: u64,
        rendered_blocks: u64,
        rendered_frames: u64,
        parameter_generation: u64,
        active_graph_revision: GraphRevision,
        commands_consumed: u64,
        active_notes: u32,
        routing_failures: u64,
        last_unknown_patch_id: Option<PatchId>,
        left_peak: f32,
        right_peak: f32,
        output_rms: f32,
        reverb_input_rms: f32,
        delay_input_rms: f32,
        wet_output_rms: f32,
        non_finite_samples: u64,
        clipped_samples: u64,
    ) -> Self {
        Self::from_parts_with_graph_routing_and_primary(
            sequence,
            rendered_blocks,
            rendered_frames,
            parameter_generation,
            active_graph_revision,
            commands_consumed,
            active_notes,
            routing_failures,
            last_unknown_patch_id,
            None,
            0.0,
            0,
            left_peak,
            right_peak,
            output_rms,
            reverb_input_rms,
            delay_input_rms,
            wet_output_rms,
            non_finite_samples,
            clipped_samples,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn from_parts_with_graph_routing_and_primary(
        sequence: u64,
        rendered_blocks: u64,
        rendered_frames: u64,
        parameter_generation: u64,
        active_graph_revision: GraphRevision,
        commands_consumed: u64,
        active_notes: u32,
        routing_failures: u64,
        last_unknown_patch_id: Option<PatchId>,
        primary_patch_id: Option<PatchId>,
        primary_patch_rms: f32,
        primary_active_notes: u32,
        left_peak: f32,
        right_peak: f32,
        output_rms: f32,
        reverb_input_rms: f32,
        delay_input_rms: f32,
        wet_output_rms: f32,
        non_finite_samples: u64,
        clipped_samples: u64,
    ) -> Self {
        Self::from_parts_with_graph_routing_primary_and_effect(
            sequence,
            rendered_blocks,
            rendered_frames,
            parameter_generation,
            active_graph_revision,
            commands_consumed,
            active_notes,
            routing_failures,
            last_unknown_patch_id,
            primary_patch_id,
            primary_patch_rms,
            primary_active_notes,
            PatchEffectObservation::EMPTY,
            left_peak,
            right_peak,
            output_rms,
            reverb_input_rms,
            delay_input_rms,
            wet_output_rms,
            non_finite_samples,
            clipped_samples,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn from_parts_with_graph_routing_primary_and_effect(
        sequence: u64,
        rendered_blocks: u64,
        rendered_frames: u64,
        parameter_generation: u64,
        active_graph_revision: GraphRevision,
        commands_consumed: u64,
        active_notes: u32,
        routing_failures: u64,
        last_unknown_patch_id: Option<PatchId>,
        primary_patch_id: Option<PatchId>,
        primary_patch_rms: f32,
        primary_active_notes: u32,
        patch_effect: PatchEffectObservation,
        left_peak: f32,
        right_peak: f32,
        output_rms: f32,
        reverb_input_rms: f32,
        delay_input_rms: f32,
        wet_output_rms: f32,
        non_finite_samples: u64,
        clipped_samples: u64,
    ) -> Self {
        Self::from_parts_with_graph_routing_primary_effect_and_tracks(
            sequence,
            rendered_blocks,
            rendered_frames,
            parameter_generation,
            active_graph_revision,
            commands_consumed,
            active_notes,
            routing_failures,
            last_unknown_patch_id,
            primary_patch_id,
            primary_patch_rms,
            primary_active_notes,
            patch_effect,
            [TrackMeter::ZERO; MixerTrackId::COUNT],
            left_peak,
            right_peak,
            output_rms,
            reverb_input_rms,
            delay_input_rms,
            wet_output_rms,
            non_finite_samples,
            clipped_samples,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn from_parts_with_graph_routing_primary_effect_and_tracks(
        sequence: u64,
        rendered_blocks: u64,
        rendered_frames: u64,
        parameter_generation: u64,
        active_graph_revision: GraphRevision,
        commands_consumed: u64,
        active_notes: u32,
        routing_failures: u64,
        last_unknown_patch_id: Option<PatchId>,
        primary_patch_id: Option<PatchId>,
        primary_patch_rms: f32,
        primary_active_notes: u32,
        patch_effect: PatchEffectObservation,
        tracks: [TrackMeter; MixerTrackId::COUNT],
        left_peak: f32,
        right_peak: f32,
        output_rms: f32,
        reverb_input_rms: f32,
        delay_input_rms: f32,
        wet_output_rms: f32,
        non_finite_samples: u64,
        clipped_samples: u64,
    ) -> Self {
        Self {
            sequence,
            rendered_blocks,
            rendered_frames,
            parameter_generation,
            active_graph_revision,
            commands_consumed,
            active_notes,
            routing_failures,
            voice_limit_refusals: 0,
            scheduled_late_events: 0,
            scheduled_max_lateness_frames: 0,
            scheduled_queue_high_water: 0,
            last_unknown_patch_id,
            primary_patch_id,
            primary_patch_rms,
            primary_active_notes,
            patch_effect,
            preview_identity: 0,
            preview_patch_id: None,
            preview_playing: false,
            preview_playhead: 0.0,
            tracks,
            left_peak,
            right_peak,
            output_rms,
            reverb_input_rms,
            delay_input_rms,
            wet_output_rms,
            non_finite_samples,
            clipped_samples,
        }
    }

    /// Cumulative schedule timing and producer-sampled queue occupancy.
    /// Lateness measures delivery frames, not device or acoustic latency.
    #[must_use]
    pub const fn with_schedule_metrics(
        mut self,
        late_events: u64,
        max_lateness_frames: u64,
        queue_high_water: u64,
    ) -> Self {
        self.scheduled_late_events = late_events;
        self.scheduled_max_lateness_frames = max_lateness_frames;
        self.scheduled_queue_high_water = queue_high_water;
        self
    }

    pub const fn scheduled_late_events(self) -> u64 {
        self.scheduled_late_events
    }
    pub const fn scheduled_max_lateness_frames(self) -> u64 {
        self.scheduled_max_lateness_frames
    }
    pub const fn scheduled_queue_high_water(self) -> u64 {
        self.scheduled_queue_high_water
    }

    /// Carries the callback's saturating refusal count without allocation,
    /// formatting, logging, or backpressure at the publishing site.
    #[must_use]
    pub const fn with_voice_limit_refusals(mut self, voice_limit_refusals: u64) -> Self {
        self.voice_limit_refusals = voice_limit_refusals;
        self
    }

    #[must_use]
    pub const fn with_preview_observation(mut self, preview: PreviewAudioObservation) -> Self {
        self.preview_identity = preview.identity();
        self.preview_patch_id = preview.patch_id();
        self.preview_playing = preview.playing();
        self.preview_playhead = preview.playhead();
        self
    }

    pub const fn sequence(self) -> u64 {
        self.sequence
    }
    pub const fn rendered_blocks(self) -> u64 {
        self.rendered_blocks
    }
    pub const fn rendered_frames(self) -> u64 {
        self.rendered_frames
    }
    pub const fn parameter_generation(self) -> u64 {
        self.parameter_generation
    }
    pub const fn active_graph_revision(self) -> GraphRevision {
        self.active_graph_revision
    }
    pub const fn commands_consumed(self) -> u64 {
        self.commands_consumed
    }
    pub const fn active_notes(self) -> u32 {
        self.active_notes
    }
    pub const fn routing_failures(self) -> u64 {
        self.routing_failures
    }
    /// Returns how many note-ons the snapshot-carried voice limit refused.
    pub const fn voice_limit_refusals(self) -> u64 {
        self.voice_limit_refusals
    }
    pub const fn last_unknown_patch_id(self) -> Option<PatchId> {
        self.last_unknown_patch_id
    }
    pub const fn primary_patch_id(self) -> Option<PatchId> {
        self.primary_patch_id
    }
    pub const fn primary_patch_rms(self) -> f32 {
        self.primary_patch_rms
    }
    pub const fn primary_active_notes(self) -> u32 {
        self.primary_active_notes
    }
    pub const fn patch_effect(self) -> PatchEffectObservation {
        self.patch_effect
    }
    pub const fn preview_identity(self) -> u64 {
        self.preview_identity
    }
    pub const fn preview_patch_id(self) -> Option<PatchId> {
        self.preview_patch_id
    }
    pub const fn preview_playing(self) -> bool {
        self.preview_playing
    }
    pub const fn preview_playhead(self) -> f32 {
        self.preview_playhead
    }
    /// Returns the passive audition observation only when it belongs to the
    /// exact canonical document/request that is asking for it.
    ///
    /// This is a read-only filter over copied fixed-size data. Hosts may
    /// repaint from the returned value, but cannot mistake a stale,
    /// non-finite, or out-of-range playhead for compatible evidence.
    pub fn compatible_preview(
        self,
        parameter_generation: u64,
        graph_revision: GraphRevision,
        preview_identity: u64,
        patch_id: PatchId,
    ) -> Option<PreviewAudioObservation> {
        (self.parameter_generation == parameter_generation
            && self.active_graph_revision == graph_revision
            && preview_identity != 0
            && self.preview_identity == preview_identity
            && self.preview_patch_id == Some(patch_id)
            && self.preview_playhead.is_finite()
            && (0.0..=1.0).contains(&self.preview_playhead))
        .then(|| {
            PreviewAudioObservation::from_parts(
                self.preview_identity,
                self.preview_patch_id,
                self.preview_playing,
                self.preview_playhead,
            )
        })
    }
    pub const fn tracks(self) -> [TrackMeter; MixerTrackId::COUNT] {
        self.tracks
    }
    pub const fn track(self, id: TrackId) -> TrackMeter {
        self.tracks[id.index()]
    }
    pub const fn left_peak(self) -> f32 {
        self.left_peak
    }
    pub const fn right_peak(self) -> f32 {
        self.right_peak
    }
    pub const fn output_rms(self) -> f32 {
        self.output_rms
    }
    pub const fn reverb_input_rms(self) -> f32 {
        self.reverb_input_rms
    }
    pub const fn delay_input_rms(self) -> f32 {
        self.delay_input_rms
    }
    pub const fn wet_output_rms(self) -> f32 {
        self.wet_output_rms
    }
    pub const fn non_finite_samples(self) -> u64 {
        self.non_finite_samples
    }
    pub const fn clipped_samples(self) -> u64 {
        self.clipped_samples
    }
}

impl Default for AudioObservationSnapshot {
    fn default() -> Self {
        Self::from_parts_with_graph_and_routing(
            0,
            0,
            0,
            0,
            GraphRevision::INITIAL,
            0,
            0,
            0,
            None,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0,
            0,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::AudioObservationSnapshot;

    #[test]
    fn snapshot_is_copyable_fixed_size_numeric_data() {
        fn assert_copy<T: Copy>() {}

        assert_copy::<AudioObservationSnapshot>();
        assert!(!core::mem::needs_drop::<AudioObservationSnapshot>());
        assert_eq!(AudioObservationSnapshot::default().sequence(), 0);
    }

    /// The refusal count is one more fixed numeric field: it starts at zero,
    /// carries verbatim, and adding it destroys neither the `Copy` nor the
    /// destructor-free character of the observation.
    #[test]
    fn the_refusal_counter_defaults_to_zero_and_carries_verbatim() {
        let empty = AudioObservationSnapshot::default();
        assert_eq!(empty.voice_limit_refusals(), 0);

        let counted = empty.with_voice_limit_refusals(7);
        assert_eq!(counted.voice_limit_refusals(), 7);
        assert_eq!(
            counted.routing_failures(),
            empty.routing_failures(),
            "the refusal counter is independent of the routing counter"
        );
        assert!(!core::mem::needs_drop::<AudioObservationSnapshot>());
        assert_eq!(
            core::mem::size_of_val(&counted),
            core::mem::size_of::<AudioObservationSnapshot>()
        );
        assert_eq!(
            empty
                .with_voice_limit_refusals(u64::MAX)
                .voice_limit_refusals(),
            u64::MAX
        );
    }

    #[test]
    fn preview_compatibility_rejects_stale_identity_and_invalid_playheads() {
        let patch_id = crate::kernel::PatchId::new(7).unwrap();
        let revision = crate::real_time::GraphRevision::new(3).unwrap();
        let base = AudioObservationSnapshot::from_mix_with_graph_and_routing(
            1,
            1,
            64,
            11,
            revision,
            0,
            0,
            0,
            None,
            crate::mixer::mix_observation::MixObservation::default(),
        )
        .with_preview_observation(crate::real_time::PreviewAudioObservation::from_parts(
            23,
            Some(patch_id),
            true,
            0.375,
        ));
        let compatible = base
            .compatible_preview(11, revision, 23, patch_id)
            .expect("every correlation identity matches");
        assert!(compatible.playing());
        assert_eq!(compatible.playhead(), 0.375);

        assert!(base
            .compatible_preview(12, revision, 23, patch_id)
            .is_none());
        assert!(base
            .compatible_preview(
                11,
                crate::real_time::GraphRevision::new(4).unwrap(),
                23,
                patch_id,
            )
            .is_none());
        assert!(base
            .compatible_preview(11, revision, 24, patch_id)
            .is_none());
        assert!(base
            .compatible_preview(11, revision, 23, crate::kernel::PatchId::new(8).unwrap())
            .is_none());
        for playhead in [f32::NAN, f32::INFINITY, -0.001, 1.001] {
            let invalid = base.with_preview_observation(
                crate::real_time::PreviewAudioObservation::from_parts(
                    23,
                    Some(patch_id),
                    true,
                    playhead,
                ),
            );
            assert!(invalid
                .compatible_preview(11, revision, 23, patch_id)
                .is_none());
        }
    }
}
