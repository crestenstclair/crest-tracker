use crate::mixer::mixer_track_id::{MixerTrackBoundaryError, MixerTrackId};
use core::fmt;
use serde::{Deserialize, Deserializer, Serialize};

/// Stable identity of one Patch-owned output field.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PatchOutputParameter {
    TrimGain,
    OutputTrack,
}

impl PatchOutputParameter {
    pub const ALL: [Self; 2] = [Self::TrimGain, Self::OutputTrack];

    pub const fn name(self) -> &'static str {
        match self {
            Self::TrimGain => "trimGainDb",
            Self::OutputTrack => "outputTrack",
        }
    }

    pub const fn descriptor(self) -> &'static PatchOutputParameterDescriptor {
        match self {
            Self::TrimGain => &PATCH_OUTPUT_SURFACE_DESCRIPTOR[0],
            Self::OutputTrack => &PATCH_OUTPUT_SURFACE_DESCRIPTOR[1],
        }
    }
}

impl fmt::Display for PatchOutputParameter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PatchOutputParameterKind {
    Continuous,
    TrackChoice,
}

/// Production-owned presentation and adjustment contract for a Patch output field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PatchOutputParameterDescriptor {
    parameter: PatchOutputParameter,
    label: &'static str,
    kind: PatchOutputParameterKind,
    minimum: Option<f32>,
    maximum: Option<f32>,
    fine_step: Option<f32>,
    coarse_step: Option<f32>,
    unit: Option<&'static str>,
}

impl PatchOutputParameterDescriptor {
    const fn continuous(
        parameter: PatchOutputParameter,
        label: &'static str,
        minimum: f32,
        maximum: f32,
        fine_step: f32,
        coarse_step: f32,
        unit: &'static str,
    ) -> Self {
        Self {
            parameter,
            label,
            kind: PatchOutputParameterKind::Continuous,
            minimum: Some(minimum),
            maximum: Some(maximum),
            fine_step: Some(fine_step),
            coarse_step: Some(coarse_step),
            unit: Some(unit),
        }
    }

    const fn track_choice(parameter: PatchOutputParameter, label: &'static str) -> Self {
        Self {
            parameter,
            label,
            kind: PatchOutputParameterKind::TrackChoice,
            minimum: None,
            maximum: None,
            fine_step: None,
            coarse_step: None,
            unit: None,
        }
    }

    pub const fn parameter(self) -> PatchOutputParameter {
        self.parameter
    }

    pub const fn name(self) -> &'static str {
        self.parameter.name()
    }

    pub const fn label(self) -> &'static str {
        self.label
    }

    pub const fn kind(self) -> PatchOutputParameterKind {
        self.kind
    }

    pub const fn minimum(self) -> Option<f32> {
        self.minimum
    }

    pub const fn maximum(self) -> Option<f32> {
        self.maximum
    }

    pub const fn fine_step(self) -> Option<f32> {
        self.fine_step
    }

    pub const fn coarse_step(self) -> Option<f32> {
        self.coarse_step
    }

    pub const fn unit(self) -> Option<&'static str> {
        self.unit
    }
}

const PATCH_OUTPUT_SURFACE_DESCRIPTOR: [PatchOutputParameterDescriptor; 2] = [
    PatchOutputParameterDescriptor::continuous(
        PatchOutputParameter::TrimGain,
        "Trim Gain",
        -60.0,
        6.0,
        1.0,
        6.0,
        "dB",
    ),
    PatchOutputParameterDescriptor::track_choice(PatchOutputParameter::OutputTrack, "Output Track"),
];

/// Patch-local output routing before fixed track accumulation.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchOutput {
    track_id: MixerTrackId,
    trim_gain_db: f32,
}

impl PatchOutput {
    pub const MIN_TRIM_GAIN_DB: f32 = -60.0;
    pub const MAX_TRIM_GAIN_DB: f32 = 6.0;

    pub fn new(track_id: MixerTrackId, trim_gain_db: f32) -> Result<Self, PatchOutputError> {
        validate_trim(trim_gain_db)?;
        Ok(Self {
            track_id,
            trim_gain_db,
        })
    }

    /// Creates a neutral-trim output for an explicitly selected track.
    pub const fn to_track(track_id: MixerTrackId) -> Self {
        Self {
            track_id,
            trim_gain_db: 0.0,
        }
    }

    pub const fn surface_descriptor() -> &'static [PatchOutputParameterDescriptor] {
        &PATCH_OUTPUT_SURFACE_DESCRIPTOR
    }

    pub const fn track_id(self) -> MixerTrackId {
        self.track_id
    }

    pub const fn trim_gain_db(self) -> f32 {
        self.trim_gain_db
    }

    pub fn with_trim_gain_db(mut self, value: f32) -> Result<Self, PatchOutputError> {
        validate_trim(value)?;
        self.trim_gain_db = value;
        Ok(self)
    }

    pub const fn with_track_id(mut self, track_id: MixerTrackId) -> Self {
        self.track_id = track_id;
        self
    }

    pub fn with_adjacent_track(self, increase: bool) -> Result<Self, MixerTrackBoundaryError> {
        self.track_id
            .adjacent(increase)
            .map(|track_id| self.with_track_id(track_id))
    }
}

impl Default for PatchOutput {
    fn default() -> Self {
        Self {
            track_id: MixerTrackId::default(),
            trim_gain_db: 0.0,
        }
    }
}

impl<'de> Deserialize<'de> for PatchOutput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Value {
            track_id: MixerTrackId,
            trim_gain_db: f32,
        }

        let value = Value::deserialize(deserializer)?;
        Self::new(value.track_id, value.trim_gain_db).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PatchOutputError {
    NonFiniteTrimGain,
    TrimGainOutOfRange { value: f32 },
}

impl fmt::Display for PatchOutputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::NonFiniteTrimGain => formatter.write_str("trimGainDb must be finite"),
            Self::TrimGainOutOfRange { value } => write!(
                formatter,
                "trimGainDb must be in {}..={}, got {value}",
                PatchOutput::MIN_TRIM_GAIN_DB,
                PatchOutput::MAX_TRIM_GAIN_DB
            ),
        }
    }
}

impl std::error::Error for PatchOutputError {}

fn validate_trim(value: f32) -> Result<(), PatchOutputError> {
    if !value.is_finite() {
        return Err(PatchOutputError::NonFiniteTrimGain);
    }
    if !(PatchOutput::MIN_TRIM_GAIN_DB..=PatchOutput::MAX_TRIM_GAIN_DB).contains(&value) {
        return Err(PatchOutputError::TrimGainOutOfRange { value });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{PatchOutput, PatchOutputError, PatchOutputParameter, PatchOutputParameterKind};
    use crate::mixer::mixer_track_id::MixerTrackId;

    #[test]
    fn descriptor_is_complete_stable_and_typed() {
        let descriptors = PatchOutput::surface_descriptor();
        assert_eq!(descriptors.len(), 2);
        assert_eq!(descriptors[0].parameter(), PatchOutputParameter::TrimGain);
        assert_eq!(descriptors[0].kind(), PatchOutputParameterKind::Continuous);
        assert_eq!(
            descriptors[1].parameter(),
            PatchOutputParameter::OutputTrack
        );
        assert_eq!(descriptors[1].kind(), PatchOutputParameterKind::TrackChoice);
    }

    #[test]
    fn trim_accepts_inclusive_bounds_and_rejects_every_invalid_class() {
        let track = MixerTrackId::new(15).unwrap();
        assert_eq!(
            PatchOutput::new(track, -60.0).unwrap().trim_gain_db(),
            -60.0
        );
        assert_eq!(PatchOutput::new(track, 6.0).unwrap().trim_gain_db(), 6.0);
        assert_eq!(
            PatchOutput::new(track, f32::NAN),
            Err(PatchOutputError::NonFiniteTrimGain)
        );
        assert!(matches!(
            PatchOutput::new(track, 6.1),
            Err(PatchOutputError::TrimGainOutOfRange { .. })
        ));
    }

    #[test]
    fn route_choice_changes_only_the_destination_and_never_wraps() {
        let output = PatchOutput::new(MixerTrackId::new(14).unwrap(), -3.0).unwrap();
        let moved = output.with_adjacent_track(true).unwrap();
        assert_eq!(moved.track_id(), MixerTrackId::new(15).unwrap());
        assert_eq!(moved.trim_gain_db(), -3.0);
        assert!(moved.with_adjacent_track(true).is_err());
    }

    #[test]
    fn serde_rejects_an_invalid_nested_track_identity() {
        let output = PatchOutput::new(MixerTrackId::new(10).unwrap(), -3.0).unwrap();
        let json = serde_json::to_string(&output).unwrap();
        assert_eq!(serde_json::from_str::<PatchOutput>(&json).unwrap(), output);
        assert!(serde_json::from_str::<PatchOutput>(r#"{"trackId":16,"trimGainDb":0.0}"#).is_err());
        assert!(serde_json::from_str::<PatchOutput>(r#"{"trackId":0,"trimGainDb":7.0}"#).is_err());
    }
}
