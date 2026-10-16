use crate::real_time::audio_observation_snapshot::AudioObservationSnapshot;

/// Callback-only half of the latest-value audio-observation seam.
pub trait CallbackAudioObservation: Send {
    /// Publishes one complete snapshot without allocation or backpressure.
    fn publish_from_callback(&mut self, snapshot: AudioObservationSnapshot);
}

/// Control-only half of the latest-value audio-observation seam.
pub trait ControlAudioObservation: Send {
    /// Returns one coherent complete latest snapshot.
    fn read_latest_on_control(&self) -> AudioObservationSnapshot;
}

/// Factory for separate callback-write and control-read observation handles.
pub trait AudioObservation: Send + Sized {
    type CallbackHandle: CallbackAudioObservation;
    type ControlHandle: ControlAudioObservation;

    fn into_handles(self) -> (Self::CallbackHandle, Self::ControlHandle);
}

/// Callback publisher used by non-observing deterministic seams.
#[derive(Clone, Copy, Debug, Default)]
pub struct DiscardAudioObservation;

impl CallbackAudioObservation for DiscardAudioObservation {
    fn publish_from_callback(&mut self, _snapshot: AudioObservationSnapshot) {}
}
