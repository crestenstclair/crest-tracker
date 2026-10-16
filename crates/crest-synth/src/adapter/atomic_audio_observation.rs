use crate::mixer::mixer_track_id::MixerTrackId;
use crate::mixer::track_meter::TrackMeter;
use crate::real_time::audio_observation::{
    AudioObservation, CallbackAudioObservation, ControlAudioObservation,
};
use crate::real_time::audio_observation_snapshot::AudioObservationSnapshot;
use core::sync::atomic::{fence, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// Prepared coherent latest-value storage for callback audio observations.
pub struct AtomicAudioObservation {
    shared: Arc<AtomicObservationFields>,
}

impl AtomicAudioObservation {
    #[must_use]
    pub fn new(initial: AudioObservationSnapshot) -> Self {
        Self {
            shared: Arc::new(AtomicObservationFields::new(initial)),
        }
    }
}

impl Default for AtomicAudioObservation {
    fn default() -> Self {
        Self::new(AudioObservationSnapshot::default())
    }
}

impl AudioObservation for AtomicAudioObservation {
    type CallbackHandle = AtomicAudioObservationWriter;
    type ControlHandle = AtomicAudioObservationReader;

    fn into_handles(self) -> (Self::CallbackHandle, Self::ControlHandle) {
        (
            AtomicAudioObservationWriter {
                shared: Arc::clone(&self.shared),
            },
            AtomicAudioObservationReader {
                shared: self.shared,
            },
        )
    }
}

/// Narrow callback-side publisher.
pub struct AtomicAudioObservationWriter {
    shared: Arc<AtomicObservationFields>,
}

impl CallbackAudioObservation for AtomicAudioObservationWriter {
    fn publish_from_callback(&mut self, snapshot: AudioObservationSnapshot) {
        self.shared.publish(snapshot);
    }
}

/// Narrow control-side latest-value reader.
#[derive(Clone)]
pub struct AtomicAudioObservationReader {
    shared: Arc<AtomicObservationFields>,
}

impl ControlAudioObservation for AtomicAudioObservationReader {
    fn read_latest_on_control(&self) -> AudioObservationSnapshot {
        self.shared.read()
    }
}

struct AtomicObservationFields {
    version: AtomicU64,
    sequence: AtomicU64,
    rendered_blocks: AtomicU64,
    rendered_frames: AtomicU64,
    parameter_generation: AtomicU64,
    active_graph_revision: AtomicU64,
    commands_consumed: AtomicU64,
    active_notes: AtomicU32,
    routing_failures: AtomicU64,
    /// WP01's refusal counter. Without this field the callback's count
    /// is published into `AudioObservationSnapshot` and then discarded by
    /// this transport, so every control-side reader sees 0 — a witness
    /// predicate that cannot pass for reasons unrelated to whether the
    /// voice limit works.
    voice_limit_refusals: AtomicU64,
    scheduled_late_events: AtomicU64,
    scheduled_max_lateness_frames: AtomicU64,
    scheduled_queue_high_water: AtomicU64,
    last_unknown_patch_id: AtomicU32,
    primary_patch_id: AtomicU32,
    primary_patch_rms: AtomicU32,
    primary_active_notes: AtomicU32,
    effect_patch_id: AtomicU32,
    effect_input_rms: AtomicU32,
    effect_output_rms: AtomicU32,
    effect_difference_rms: AtomicU32,
    effect_side_rms: AtomicU32,
    preview_identity: AtomicU64,
    preview_patch_id: AtomicU32,
    preview_playing: AtomicU32,
    preview_playhead: AtomicU32,
    track_left_peaks: [AtomicU32; MixerTrackId::COUNT],
    track_right_peaks: [AtomicU32; MixerTrackId::COUNT],
    track_rms: [AtomicU32; MixerTrackId::COUNT],
    left_peak: AtomicU32,
    right_peak: AtomicU32,
    output_rms: AtomicU32,
    reverb_input_rms: AtomicU32,
    delay_input_rms: AtomicU32,
    wet_output_rms: AtomicU32,
    non_finite_samples: AtomicU64,
    clipped_samples: AtomicU64,
}

impl AtomicObservationFields {
    fn new(initial: AudioObservationSnapshot) -> Self {
        Self {
            version: AtomicU64::new(0),
            sequence: AtomicU64::new(initial.sequence()),
            rendered_blocks: AtomicU64::new(initial.rendered_blocks()),
            rendered_frames: AtomicU64::new(initial.rendered_frames()),
            parameter_generation: AtomicU64::new(initial.parameter_generation()),
            active_graph_revision: AtomicU64::new(initial.active_graph_revision().value()),
            commands_consumed: AtomicU64::new(initial.commands_consumed()),
            active_notes: AtomicU32::new(initial.active_notes()),
            routing_failures: AtomicU64::new(initial.routing_failures()),
            voice_limit_refusals: AtomicU64::new(initial.voice_limit_refusals()),
            scheduled_late_events: AtomicU64::new(initial.scheduled_late_events()),
            scheduled_max_lateness_frames: AtomicU64::new(initial.scheduled_max_lateness_frames()),
            scheduled_queue_high_water: AtomicU64::new(initial.scheduled_queue_high_water()),
            last_unknown_patch_id: AtomicU32::new(
                initial
                    .last_unknown_patch_id()
                    .map_or(0, |patch_id| patch_id.value()),
            ),
            primary_patch_id: AtomicU32::new(
                initial
                    .primary_patch_id()
                    .map_or(0, |patch_id| patch_id.value()),
            ),
            primary_patch_rms: AtomicU32::new(initial.primary_patch_rms().to_bits()),
            primary_active_notes: AtomicU32::new(initial.primary_active_notes()),
            effect_patch_id: AtomicU32::new(
                initial
                    .patch_effect()
                    .patch_id()
                    .map_or(0, |patch_id| patch_id.value()),
            ),
            effect_input_rms: AtomicU32::new(initial.patch_effect().input_rms().to_bits()),
            effect_output_rms: AtomicU32::new(initial.patch_effect().output_rms().to_bits()),
            effect_difference_rms: AtomicU32::new(
                initial.patch_effect().difference_rms().to_bits(),
            ),
            effect_side_rms: AtomicU32::new(initial.patch_effect().side_rms().to_bits()),
            preview_identity: AtomicU64::new(initial.preview_identity()),
            preview_patch_id: AtomicU32::new(
                initial
                    .preview_patch_id()
                    .map_or(0, |patch_id| patch_id.value()),
            ),
            preview_playing: AtomicU32::new(u32::from(initial.preview_playing())),
            preview_playhead: AtomicU32::new(initial.preview_playhead().to_bits()),
            track_left_peaks: std::array::from_fn(|index| {
                AtomicU32::new(initial.tracks()[index].left_peak().to_bits())
            }),
            track_right_peaks: std::array::from_fn(|index| {
                AtomicU32::new(initial.tracks()[index].right_peak().to_bits())
            }),
            track_rms: std::array::from_fn(|index| {
                AtomicU32::new(initial.tracks()[index].rms().to_bits())
            }),
            left_peak: AtomicU32::new(initial.left_peak().to_bits()),
            right_peak: AtomicU32::new(initial.right_peak().to_bits()),
            output_rms: AtomicU32::new(initial.output_rms().to_bits()),
            reverb_input_rms: AtomicU32::new(initial.reverb_input_rms().to_bits()),
            delay_input_rms: AtomicU32::new(initial.delay_input_rms().to_bits()),
            wet_output_rms: AtomicU32::new(initial.wet_output_rms().to_bits()),
            non_finite_samples: AtomicU64::new(initial.non_finite_samples()),
            clipped_samples: AtomicU64::new(initial.clipped_samples()),
        }
    }

    fn publish(&self, snapshot: AudioObservationSnapshot) {
        // Only the unique callback handle publishes. Pair this release fence
        // with the reader's acquire fence so partially observed publications
        // cannot pass the version check, including on weakly ordered CPUs.
        let version = self.version.load(Ordering::Relaxed);
        self.version
            .store(version.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        self.sequence.store(snapshot.sequence(), Ordering::Relaxed);
        self.rendered_blocks
            .store(snapshot.rendered_blocks(), Ordering::Relaxed);
        self.rendered_frames
            .store(snapshot.rendered_frames(), Ordering::Relaxed);
        self.parameter_generation
            .store(snapshot.parameter_generation(), Ordering::Relaxed);
        self.active_graph_revision
            .store(snapshot.active_graph_revision().value(), Ordering::Relaxed);
        self.commands_consumed
            .store(snapshot.commands_consumed(), Ordering::Relaxed);
        self.active_notes
            .store(snapshot.active_notes(), Ordering::Relaxed);
        self.routing_failures
            .store(snapshot.routing_failures(), Ordering::Relaxed);
        self.scheduled_late_events
            .store(snapshot.scheduled_late_events(), Ordering::Relaxed);
        self.scheduled_max_lateness_frames
            .store(snapshot.scheduled_max_lateness_frames(), Ordering::Relaxed);
        self.scheduled_queue_high_water
            .store(snapshot.scheduled_queue_high_water(), Ordering::Relaxed);
        self.voice_limit_refusals
            .store(snapshot.voice_limit_refusals(), Ordering::Relaxed);
        self.last_unknown_patch_id.store(
            snapshot
                .last_unknown_patch_id()
                .map_or(0, |patch_id| patch_id.value()),
            Ordering::Relaxed,
        );
        self.primary_patch_id.store(
            snapshot
                .primary_patch_id()
                .map_or(0, |patch_id| patch_id.value()),
            Ordering::Relaxed,
        );
        self.primary_patch_rms
            .store(snapshot.primary_patch_rms().to_bits(), Ordering::Relaxed);
        self.primary_active_notes
            .store(snapshot.primary_active_notes(), Ordering::Relaxed);
        self.effect_patch_id.store(
            snapshot
                .patch_effect()
                .patch_id()
                .map_or(0, |patch_id| patch_id.value()),
            Ordering::Relaxed,
        );
        self.effect_input_rms.store(
            snapshot.patch_effect().input_rms().to_bits(),
            Ordering::Relaxed,
        );
        self.effect_output_rms.store(
            snapshot.patch_effect().output_rms().to_bits(),
            Ordering::Relaxed,
        );
        self.effect_difference_rms.store(
            snapshot.patch_effect().difference_rms().to_bits(),
            Ordering::Relaxed,
        );
        self.effect_side_rms.store(
            snapshot.patch_effect().side_rms().to_bits(),
            Ordering::Relaxed,
        );
        self.preview_identity
            .store(snapshot.preview_identity(), Ordering::Relaxed);
        self.preview_patch_id.store(
            snapshot
                .preview_patch_id()
                .map_or(0, |patch_id| patch_id.value()),
            Ordering::Relaxed,
        );
        self.preview_playing
            .store(u32::from(snapshot.preview_playing()), Ordering::Relaxed);
        self.preview_playhead
            .store(snapshot.preview_playhead().to_bits(), Ordering::Relaxed);
        for index in 0..MixerTrackId::COUNT {
            let meter = snapshot.tracks()[index];
            self.track_left_peaks[index].store(meter.left_peak().to_bits(), Ordering::Relaxed);
            self.track_right_peaks[index].store(meter.right_peak().to_bits(), Ordering::Relaxed);
            self.track_rms[index].store(meter.rms().to_bits(), Ordering::Relaxed);
        }
        self.left_peak
            .store(snapshot.left_peak().to_bits(), Ordering::Relaxed);
        self.right_peak
            .store(snapshot.right_peak().to_bits(), Ordering::Relaxed);
        self.output_rms
            .store(snapshot.output_rms().to_bits(), Ordering::Relaxed);
        self.reverb_input_rms
            .store(snapshot.reverb_input_rms().to_bits(), Ordering::Relaxed);
        self.delay_input_rms
            .store(snapshot.delay_input_rms().to_bits(), Ordering::Relaxed);
        self.wet_output_rms
            .store(snapshot.wet_output_rms().to_bits(), Ordering::Relaxed);
        self.non_finite_samples
            .store(snapshot.non_finite_samples(), Ordering::Relaxed);
        self.clipped_samples
            .store(snapshot.clipped_samples(), Ordering::Relaxed);
        self.version
            .store(version.wrapping_add(2), Ordering::Release);
    }

    fn read(&self) -> AudioObservationSnapshot {
        loop {
            let before = self.version.load(Ordering::Acquire);
            if before & 1 != 0 {
                core::hint::spin_loop();
                continue;
            }

            let tracks = std::array::from_fn(|index| {
                TrackMeter::new(
                    f32::from_bits(self.track_left_peaks[index].load(Ordering::Relaxed)),
                    f32::from_bits(self.track_right_peaks[index].load(Ordering::Relaxed)),
                    f32::from_bits(self.track_rms[index].load(Ordering::Relaxed)),
                )
                .unwrap_or_default()
            });
            let snapshot =
                AudioObservationSnapshot::from_parts_with_graph_routing_primary_effect_and_tracks(
                    self.sequence.load(Ordering::Relaxed),
                    self.rendered_blocks.load(Ordering::Relaxed),
                    self.rendered_frames.load(Ordering::Relaxed),
                    self.parameter_generation.load(Ordering::Relaxed),
                    crate::real_time::GraphRevision::new(
                        self.active_graph_revision.load(Ordering::Relaxed),
                    )
                    .expect("published observations always carry a valid graph revision"),
                    self.commands_consumed.load(Ordering::Relaxed),
                    self.active_notes.load(Ordering::Relaxed),
                    self.routing_failures.load(Ordering::Relaxed),
                    crate::kernel::patch_id::PatchId::new(
                        self.last_unknown_patch_id.load(Ordering::Relaxed),
                    )
                    .ok(),
                    crate::kernel::patch_id::PatchId::new(
                        self.primary_patch_id.load(Ordering::Relaxed),
                    )
                    .ok(),
                    f32::from_bits(self.primary_patch_rms.load(Ordering::Relaxed)),
                    self.primary_active_notes.load(Ordering::Relaxed),
                    crate::real_time::PatchEffectObservation::from_parts(
                        crate::kernel::patch_id::PatchId::new(
                            self.effect_patch_id.load(Ordering::Relaxed),
                        )
                        .ok(),
                        f32::from_bits(self.effect_input_rms.load(Ordering::Relaxed)),
                        f32::from_bits(self.effect_output_rms.load(Ordering::Relaxed)),
                        f32::from_bits(self.effect_difference_rms.load(Ordering::Relaxed)),
                        f32::from_bits(self.effect_side_rms.load(Ordering::Relaxed)),
                    ),
                    tracks,
                    f32::from_bits(self.left_peak.load(Ordering::Relaxed)),
                    f32::from_bits(self.right_peak.load(Ordering::Relaxed)),
                    f32::from_bits(self.output_rms.load(Ordering::Relaxed)),
                    f32::from_bits(self.reverb_input_rms.load(Ordering::Relaxed)),
                    f32::from_bits(self.delay_input_rms.load(Ordering::Relaxed)),
                    f32::from_bits(self.wet_output_rms.load(Ordering::Relaxed)),
                    self.non_finite_samples.load(Ordering::Relaxed),
                    self.clipped_samples.load(Ordering::Relaxed),
                )
                .with_voice_limit_refusals(self.voice_limit_refusals.load(Ordering::Relaxed))
                .with_schedule_metrics(
                    self.scheduled_late_events.load(Ordering::Relaxed),
                    self.scheduled_max_lateness_frames.load(Ordering::Relaxed),
                    self.scheduled_queue_high_water.load(Ordering::Relaxed),
                )
                .with_preview_observation(
                    crate::real_time::PreviewAudioObservation::from_parts(
                        self.preview_identity.load(Ordering::Relaxed),
                        crate::kernel::patch_id::PatchId::new(
                            self.preview_patch_id.load(Ordering::Relaxed),
                        )
                        .ok(),
                        self.preview_playing.load(Ordering::Relaxed) != 0,
                        f32::from_bits(self.preview_playhead.load(Ordering::Relaxed)),
                    ),
                );
            fence(Ordering::Acquire);
            let after = self.version.load(Ordering::Relaxed);
            if before == after {
                return snapshot;
            }
            core::hint::spin_loop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AtomicAudioObservation;
    use crate::mixer::mix_observation::MixObservation;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::track_meter::TrackMeter;
    use crate::real_time::audio_observation::{
        AudioObservation, CallbackAudioObservation, ControlAudioObservation,
    };
    use crate::real_time::audio_observation_snapshot::AudioObservationSnapshot;

    fn snapshot(sequence: u64) -> AudioObservationSnapshot {
        let value = sequence as f32;
        AudioObservationSnapshot::from_mix_with_routing(
            sequence,
            sequence,
            sequence * 64,
            sequence,
            sequence,
            sequence as u32,
            sequence,
            crate::kernel::patch_id::PatchId::new(sequence as u32).ok(),
            MixObservation::new(
                [TrackMeter::new(value, value, value).unwrap(); MixerTrackId::COUNT],
                value,
                value,
                value,
                value,
                value,
                value,
                sequence,
                sequence,
            ),
        )
        .with_schedule_metrics(sequence * 3, sequence * 4, sequence * 5)
        .with_voice_limit_refusals(sequence * 2)
        .with_preview_observation(crate::real_time::PreviewAudioObservation::from_parts(
            sequence,
            crate::kernel::patch_id::PatchId::new(sequence as u32).ok(),
            sequence != 0,
            0.5,
        ))
    }

    #[test]
    fn the_transport_carries_the_voice_limit_refusal_count() {
        // Falsification for T035 step 0: before this field existed the
        // callback's refusal count was published into the snapshot and then
        // dropped here, so this assertion read 0 for any input.
        let observation = AtomicAudioObservation::default();
        let (mut writer, reader) = observation.into_handles();
        assert_eq!(reader.read_latest_on_control().voice_limit_refusals(), 0);
        writer.publish_from_callback(snapshot(9));
        assert_eq!(reader.read_latest_on_control().voice_limit_refusals(), 18);
    }

    #[test]
    fn atomic_audio_observation_is_latest_wins_and_coherent() {
        let observation = AtomicAudioObservation::default();
        let (mut writer, reader) = observation.into_handles();

        writer.publish_from_callback(snapshot(1));
        writer.publish_from_callback(snapshot(2));

        let latest = reader.read_latest_on_control();
        assert_eq!(latest, snapshot(2));
        assert_eq!(latest.output_rms().to_bits(), 2.0_f32.to_bits());
    }

    #[test]
    fn atomic_audio_observation_never_tears_concurrent_snapshots() {
        let observation = AtomicAudioObservation::default();
        let (mut writer, reader) = observation.into_handles();
        let writer_thread = std::thread::spawn(move || {
            for sequence in 1..=20_000 {
                writer.publish_from_callback(snapshot(sequence));
            }
        });

        while reader.read_latest_on_control().sequence() < 20_000 {
            let latest = reader.read_latest_on_control();
            let sequence = latest.sequence();
            assert_eq!(latest.rendered_blocks(), sequence);
            assert_eq!(latest.parameter_generation(), sequence);
            assert_eq!(
                latest.active_graph_revision(),
                crate::real_time::GraphRevision::INITIAL
            );
            assert_eq!(latest.routing_failures(), sequence);
            assert_eq!(latest.voice_limit_refusals(), sequence * 2);
            assert_eq!(latest.scheduled_late_events(), sequence * 3);
            assert_eq!(latest.scheduled_max_lateness_frames(), sequence * 4);
            assert_eq!(latest.scheduled_queue_high_water(), sequence * 5);
            assert_eq!(latest.preview_identity(), sequence);
            assert_eq!(latest.preview_playing(), sequence != 0);
            assert_eq!(
                latest.preview_playhead().to_bits(),
                if sequence == 0 { 0.0_f32 } else { 0.5_f32 }.to_bits()
            );
            assert_eq!(
                latest
                    .last_unknown_patch_id()
                    .map(|patch_id| patch_id.value()),
                (sequence != 0).then_some(sequence as u32)
            );
            assert_eq!(latest.output_rms().to_bits(), (sequence as f32).to_bits());
            assert_eq!(latest.non_finite_samples(), sequence);
        }
        writer_thread.join().expect("publisher thread completes");
    }

    #[test]
    fn concrete_handles_are_send_and_keep_narrow_capabilities() {
        fn assert_send<T: Send>() {}

        assert_send::<super::AtomicAudioObservationWriter>();
        assert_send::<super::AtomicAudioObservationReader>();
    }
}
