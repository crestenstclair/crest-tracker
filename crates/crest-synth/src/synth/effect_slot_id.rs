use core::fmt;
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};

/// The product-level maximum number of ordered post-effect slots per Patch.
///
/// Sourced from the DESIGN.md product maximum of three ordered post-FX slots:
/// no slice, snapshot, rack, or graph may exceed it without changing that
/// declaration.
pub const MAX_EFFECT_SLOTS: usize = 3;

/// Stable non-zero Patch-local identity for one ordered post-effect instance.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct EffectSlotId(u16);

impl EffectSlotId {
    pub const MIN: u16 = 1;

    pub const fn new(value: u16) -> Result<Self, EffectSlotIdError> {
        if value == 0 {
            Err(EffectSlotIdError::Zero)
        } else {
            Ok(Self(value))
        }
    }

    pub const fn value(self) -> u16 {
        self.0
    }
}

impl fmt::Display for EffectSlotId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl<'de> Deserialize<'de> for EffectSlotId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(u16::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectSlotIdError {
    Zero,
}

impl fmt::Display for EffectSlotIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("effect slot id must be non-zero")
    }
}

impl std::error::Error for EffectSlotIdError {}

/// Validated ordered Patch effect position in `0..MAX_EFFECT_SLOTS`.
///
/// A position is a stable address, not an instance: `EffectSlotId` identifies
/// one configured effect instance, `EffectSlotIndex` identifies where on the
/// Patch it sits. Positions are anonymous — no index is named after any
/// effect. Out-of-range construction fails; a value is never clamped.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct EffectSlotIndex(u8);

impl EffectSlotIndex {
    /// Every valid position in ascending render order.
    pub const ALL: [Self; MAX_EFFECT_SLOTS] = {
        let mut all = [Self(0); MAX_EFFECT_SLOTS];
        let mut index = 0;
        while index < MAX_EFFECT_SLOTS {
            all[index] = Self(index as u8);
            index += 1;
        }
        all
    };

    pub const fn new(value: usize) -> Result<Self, EffectSlotIndexError> {
        if value < MAX_EFFECT_SLOTS {
            Ok(Self(value as u8))
        } else {
            Err(EffectSlotIndexError::OutOfRange { value })
        }
    }

    /// Returns the validated zero-based array position.
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    /// Returns the stable instance identity an occupancy change derives for
    /// this position: positions map one-to-one onto non-zero slot ids
    /// (position + 1), exactly as bus returns derive theirs, so the identity
    /// is deterministic and unique per position by construction.
    pub const fn instance_identity(self) -> EffectSlotId {
        match EffectSlotId::new(self.0 as u16 + 1) {
            Ok(slot_id) => slot_id,
            // Unreachable: every position is in 0..MAX_EFFECT_SLOTS, so the
            // derived value is non-zero.
            Err(_) => panic!("slot positions map onto non-zero slot ids"),
        }
    }
}

impl fmt::Display for EffectSlotIndex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl<'de> Deserialize<'de> for EffectSlotIndex {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(usize::from(u8::deserialize(deserializer)?)).map_err(D::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectSlotIndexError {
    OutOfRange { value: usize },
}

impl fmt::Display for EffectSlotIndexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfRange { value } => write!(
                formatter,
                "effect slot index {value} is outside 0..{MAX_EFFECT_SLOTS}"
            ),
        }
    }
}

impl std::error::Error for EffectSlotIndexError {}

#[cfg(test)]
mod tests {
    use super::{EffectSlotIndex, EffectSlotIndexError, MAX_EFFECT_SLOTS};

    #[test]
    fn slot_index_accepts_every_position_and_rejects_out_of_range() {
        for value in 0..MAX_EFFECT_SLOTS {
            assert_eq!(EffectSlotIndex::new(value).unwrap().index(), value);
        }
        assert_eq!(
            EffectSlotIndex::new(MAX_EFFECT_SLOTS),
            Err(EffectSlotIndexError::OutOfRange {
                value: MAX_EFFECT_SLOTS
            })
        );
        assert_eq!(
            EffectSlotIndex::new(usize::MAX),
            Err(EffectSlotIndexError::OutOfRange { value: usize::MAX })
        );
    }

    #[test]
    fn slot_index_all_lists_positions_in_ascending_render_order() {
        assert_eq!(EffectSlotIndex::ALL.len(), MAX_EFFECT_SLOTS);
        for (position, index) in EffectSlotIndex::ALL.iter().enumerate() {
            assert_eq!(index.index(), position);
        }
    }

    #[test]
    fn slot_index_serde_round_trips_and_validates() {
        let position = EffectSlotIndex::new(2).unwrap();
        let encoded = serde_json::to_string(&position).unwrap();
        assert_eq!(encoded, "2");
        assert_eq!(
            serde_json::from_str::<EffectSlotIndex>(&encoded).unwrap(),
            position
        );
        assert!(serde_json::from_str::<EffectSlotIndex>("3").is_err());
    }
}
