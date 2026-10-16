use core::fmt;
use serde::{Deserialize, Deserializer, Serialize};

/// Stable identity of one track in the fixed sixteen-track mixer bank.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct MixerTrackId(u8);

impl MixerTrackId {
    pub const COUNT: usize = 16;
    pub const MIN: u8 = 0;
    pub const MAX: u8 = 15;
    pub const ALL: [Self; Self::COUNT] = [
        Self(0),
        Self(1),
        Self(2),
        Self(3),
        Self(4),
        Self(5),
        Self(6),
        Self(7),
        Self(8),
        Self(9),
        Self(10),
        Self(11),
        Self(12),
        Self(13),
        Self(14),
        Self(15),
    ];

    /// Creates a track identity without clamping, wrapping, or substitution.
    pub const fn new(value: u8) -> Result<Self, MixerTrackIdError> {
        if value <= Self::MAX {
            Ok(Self(value))
        } else {
            Err(MixerTrackIdError { value })
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }

    pub const fn index(self) -> usize {
        self.0 as usize
    }

    /// Selects a neighboring track and rejects either boundary.
    pub const fn adjacent(self, increase: bool) -> Result<Self, MixerTrackBoundaryError> {
        if increase {
            if self.0 == Self::MAX {
                Err(MixerTrackBoundaryError)
            } else {
                Ok(Self(self.0 + 1))
            }
        } else if self.0 == Self::MIN {
            Err(MixerTrackBoundaryError)
        } else {
            Ok(Self(self.0 - 1))
        }
    }
}

impl Default for MixerTrackId {
    fn default() -> Self {
        Self(Self::MIN)
    }
}

impl fmt::Display for MixerTrackId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "T{:02X}", self.0)
    }
}

impl TryFrom<u8> for MixerTrackId {
    type Error = MixerTrackIdError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for MixerTrackId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u8::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl From<MixerTrackId> for u8 {
    fn from(value: MixerTrackId) -> Self {
        value.value()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MixerTrackIdError {
    value: u8,
}

impl MixerTrackIdError {
    pub const fn value(self) -> u8 {
        self.value
    }
}

impl fmt::Display for MixerTrackIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "mixer track id must be in {}..={}, got {}",
            MixerTrackId::MIN,
            MixerTrackId::MAX,
            self.value
        )
    }
}

impl std::error::Error for MixerTrackIdError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MixerTrackBoundaryError;

impl fmt::Display for MixerTrackBoundaryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("mixer track selection is already at the requested boundary")
    }
}

impl std::error::Error for MixerTrackBoundaryError {}

#[cfg(test)]
mod tests {
    use super::{MixerTrackId, MixerTrackIdError};

    #[test]
    fn fixed_bank_is_exact_and_formats_t00_through_t0f() {
        assert_eq!(MixerTrackId::ALL.len(), 16);
        let labels = MixerTrackId::ALL.map(|track| track.to_string());
        assert_eq!(labels.first().map(String::as_str), Some("T00"));
        assert_eq!(labels.last().map(String::as_str), Some("T0F"));
        for (index, track) in MixerTrackId::ALL.into_iter().enumerate() {
            assert_eq!(track.index(), index);
            assert_eq!(track.value(), index as u8);
        }
    }

    #[test]
    fn invalid_identity_is_rejected_without_clamping() {
        assert_eq!(MixerTrackId::new(16), Err(MixerTrackIdError { value: 16 }));
        assert_eq!(MixerTrackId::new(u8::MAX).unwrap_err().value(), u8::MAX);
    }

    #[test]
    fn adjacent_choice_is_nonwrapping() {
        assert_eq!(
            MixerTrackId::new(7).unwrap().adjacent(true).unwrap(),
            MixerTrackId::new(8).unwrap()
        );
        assert!(MixerTrackId::default().adjacent(false).is_err());
        assert!(MixerTrackId::new(15).unwrap().adjacent(true).is_err());
    }

    #[test]
    fn serde_round_trip_preserves_numeric_identity() {
        let track = MixerTrackId::new(10).unwrap();
        assert_eq!(serde_json::to_string(&track).unwrap(), "10");
        assert_eq!(serde_json::from_str::<MixerTrackId>("10").unwrap(), track);
        assert!(serde_json::from_str::<MixerTrackId>("16").is_err());
    }
}
