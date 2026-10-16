use crate::mixer::mixer_track_id::MixerTrackId;
use crate::mixer::mixer_track_parameters::MixerTrackParameters;
use serde::{Deserialize, Serialize};

/// Canonical fixed mixer bank owned by control state.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MixerState {
    tracks: [MixerTrackParameters; MixerTrackId::COUNT],
}

impl MixerState {
    pub const fn new(tracks: [MixerTrackParameters; MixerTrackId::COUNT]) -> Self {
        Self { tracks }
    }

    pub const fn tracks(&self) -> &[MixerTrackParameters; MixerTrackId::COUNT] {
        &self.tracks
    }

    pub const fn track(&self, id: MixerTrackId) -> &MixerTrackParameters {
        &self.tracks[id.index()]
    }

    pub fn set_track(&mut self, id: MixerTrackId, parameters: MixerTrackParameters) {
        self.tracks[id.index()] = parameters;
    }

    pub fn with_track(mut self, id: MixerTrackId, parameters: MixerTrackParameters) -> Self {
        self.set_track(id, parameters);
        self
    }
}

impl Default for MixerState {
    fn default() -> Self {
        Self {
            tracks: std::array::from_fn(|_| MixerTrackParameters::default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MixerState;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::mixer_track_parameters::{MixerTrackParameter, MixerTrackParameters};

    #[test]
    fn bank_always_contains_exactly_sixteen_independent_tracks() {
        let mut state = MixerState::default();
        assert_eq!(state.tracks().len(), 16);
        let edited = MixerTrackParameters::default()
            .with_scalar_value(MixerTrackParameter::Level, -12.0)
            .unwrap();
        state.set_track(MixerTrackId::new(15).unwrap(), edited);
        assert_eq!(
            state.track(MixerTrackId::new(15).unwrap()).level_db(),
            -12.0
        );
        assert_eq!(state.track(MixerTrackId::new(14).unwrap()).level_db(), 0.0);
    }

    #[test]
    fn serde_round_trip_retains_fixed_order_and_count() {
        let state = MixerState::default();
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(serde_json::from_str::<MixerState>(&json).unwrap(), state);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["tracks"].as_array().unwrap().len(), 16);
    }
}
