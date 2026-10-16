use crate::mixer::bus_id::{BusId, MAX_BUS_RETURNS};
use crate::mixer::mixer_track_id::{MixerTrackId, MixerTrackId as TrackId};
use crate::mixer::track_meter::TrackMeter;

/// Fixed-size measurements from one completed mixer block.
///
/// The value contains only numeric callback-local data. It never owns or
/// borrows mixer buffers and cannot influence rendering decisions.
///
/// Bus measurements are indexed by `BusId`: `bus_input_rms` measures only the
/// post-gate track-send sums and `bus_output_rms` measures each return's
/// contribution before final master gain; an unoccupied return measures zero.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MixObservation {
    tracks: [TrackMeter; MixerTrackId::COUNT],
    left_peak: f32,
    right_peak: f32,
    output_rms: f32,
    bus_input_rms: [f32; MAX_BUS_RETURNS],
    bus_output_rms: [f32; MAX_BUS_RETURNS],
    wet_output_rms: f32,
    non_finite_samples: u64,
    clipped_samples: u64,
}

impl MixObservation {
    /// Transitional seam: the legacy two-named-input constructor kept for the
    /// retained in-crate observation tests. The named inputs are
    /// `bus_input_rms[0]` and `bus_input_rms[1]`; return contributions
    /// measure zero.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn new(
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
        let mut bus_input_rms = [0.0; MAX_BUS_RETURNS];
        bus_input_rms[0] = reverb_input_rms;
        bus_input_rms[1] = delay_input_rms;
        Self {
            tracks,
            left_peak,
            right_peak,
            output_rms,
            bus_input_rms,
            bus_output_rms: [0.0; MAX_BUS_RETURNS],
            wet_output_rms,
            non_finite_samples,
            clipped_samples,
        }
    }

    /// Complete per-bus measurement from callback-owned buffers.
    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn measured(
        tracks: [TrackMeter; MixerTrackId::COUNT],
        left_peak: f32,
        right_peak: f32,
        output_rms: f32,
        bus_input_rms: [f32; MAX_BUS_RETURNS],
        bus_output_rms: [f32; MAX_BUS_RETURNS],
        wet_output_rms: f32,
        non_finite_samples: u64,
        clipped_samples: u64,
    ) -> Self {
        Self {
            tracks,
            left_peak,
            right_peak,
            output_rms,
            bus_input_rms,
            bus_output_rms,
            wet_output_rms,
            non_finite_samples,
            clipped_samples,
        }
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

    /// Returns the post-gate send RMS, or None outside the observation bank.
    pub fn bus_input_rms(self, bus: BusId) -> Option<f32> {
        self.bus_input_rms.get(bus.index()).copied()
    }

    /// Returns every bus input RMS in ascending `BusId` order.
    pub const fn bus_input_rms_all(self) -> [f32; MAX_BUS_RETURNS] {
        self.bus_input_rms
    }

    /// Returns pre-master output RMS, or None outside the observation bank.
    pub fn bus_output_rms(self, bus: BusId) -> Option<f32> {
        self.bus_output_rms.get(bus.index()).copied()
    }

    /// Returns every return contribution RMS in ascending `BusId` order.
    pub const fn bus_output_rms_all(self) -> [f32; MAX_BUS_RETURNS] {
        self.bus_output_rms
    }

    /// Transitional seam for the retained observation surfaces: `bus_input_rms[0]`.
    pub const fn reverb_input_rms(self) -> f32 {
        self.bus_input_rms[0]
    }

    /// Transitional seam for the retained observation surfaces: `bus_input_rms[1]`.
    pub const fn delay_input_rms(self) -> f32 {
        self.bus_input_rms[1]
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

#[cfg(test)]
mod tests {
    use super::MixObservation;
    use crate::mixer::bus_id::{BusId, MAX_BUS_RETURNS};
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::track_meter::TrackMeter;

    #[test]
    fn bus_outside_observation_bank_is_explicitly_unavailable() {
        let observation = MixObservation::default();
        let bus = BusId::new(MAX_BUS_RETURNS as u16).unwrap();
        assert_eq!(observation.bus_input_rms(bus), None);
        assert_eq!(observation.bus_output_rms(bus), None);
    }

    #[test]
    fn mix_observation_is_fixed_size_copyable_numeric_data() {
        fn assert_copy<T: Copy>() {}

        assert_copy::<MixObservation>();
        assert!(!core::mem::needs_drop::<MixObservation>());
        assert_eq!(MixObservation::default().non_finite_samples(), 0);
    }

    #[test]
    fn legacy_constructor_maps_named_inputs_onto_the_first_two_buses() {
        let observation = MixObservation::new(
            [TrackMeter::default(); MixerTrackId::COUNT],
            0.0,
            0.0,
            0.0,
            0.25,
            0.5,
            0.0,
            0,
            0,
        );
        assert_eq!(observation.reverb_input_rms(), 0.25);
        assert_eq!(observation.delay_input_rms(), 0.5);
        assert_eq!(
            observation
                .bus_input_rms(BusId::new(0).unwrap())
                .expect("bus belongs to observed default bank"),
            0.25
        );
        assert_eq!(
            observation
                .bus_input_rms(BusId::new(1).unwrap())
                .expect("bus belongs to observed default bank"),
            0.5
        );
        for bus in &BusId::ALL[2..] {
            assert_eq!(
                observation
                    .bus_input_rms(*bus)
                    .expect("bus belongs to observed default bank"),
                0.0
            );
        }
        assert_eq!(
            observation.bus_output_rms_all(),
            [0.0; crate::mixer::bus_id::MAX_BUS_RETURNS]
        );
    }
}
