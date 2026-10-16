//! Exact quarter-note positions. Device timestamps are converted only at the host boundary.
use serde::{Deserialize, Deserializer, Serialize};
use std::cmp::Ordering;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TimeError {
    #[error("a musical time denominator must be positive")]
    ZeroDenominator,
    #[error("musical time exceeds the supported integer representation")]
    Overflow,
    #[error("musical time cannot be negative")]
    Negative,
    #[error("tempo must be positive")]
    InvalidTempo,
    #[error("division must be a power of two from quarter through sixty-fourth")]
    InvalidDivision,
    #[error("tuplet counts must be positive")]
    InvalidTuplet,
    #[error("meter requires a positive numerator and power-of-two denominator")]
    InvalidMeter,
}

/// Nonnegative, reduced rational quarter-note beats; construction and decoding validate it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct Beat {
    numerator: u64,
    denominator: u64,
}

impl Beat {
    pub const ZERO: Self = Self::integer(0);
    pub const ONE: Self = Self::integer(1);

    pub const fn integer(beats: u64) -> Self {
        Self {
            numerator: beats,
            denominator: 1,
        }
    }

    pub fn new(numerator: u64, denominator: u64) -> Result<Self, TimeError> {
        Self::from_wide(u128::from(numerator), u128::from(denominator))
    }

    pub const fn numerator(self) -> u64 {
        self.numerator
    }
    pub const fn denominator(self) -> u64 {
        self.denominator
    }
    pub const fn is_zero(self) -> bool {
        self.numerator == 0
    }

    pub fn checked_add(self, other: Self) -> Result<Self, TimeError> {
        let left = u128::from(self.numerator) * u128::from(other.denominator);
        let right = u128::from(other.numerator) * u128::from(self.denominator);
        let numerator = left.checked_add(right).ok_or(TimeError::Overflow)?;
        Self::from_wide(
            numerator,
            u128::from(self.denominator) * u128::from(other.denominator),
        )
    }

    pub fn checked_sub(self, other: Self) -> Result<Self, TimeError> {
        let left = u128::from(self.numerator) * u128::from(other.denominator);
        let right = u128::from(other.numerator) * u128::from(self.denominator);
        let numerator = left.checked_sub(right).ok_or(TimeError::Negative)?;
        Self::from_wide(
            numerator,
            u128::from(self.denominator) * u128::from(other.denominator),
        )
    }

    pub fn checked_scale(self, numerator: u64, denominator: u64) -> Result<Self, TimeError> {
        Self::from_wide(
            u128::from(self.numerator) * u128::from(numerator),
            u128::from(self.denominator) * u128::from(denominator),
        )
    }

    /// Complete grid units before this position, without floating-point conversion.
    pub fn units(self, unit: Self) -> Result<u64, TimeError> {
        if unit.is_zero() {
            return Err(TimeError::ZeroDenominator);
        }
        let numerator = u128::from(self.numerator) * u128::from(unit.denominator);
        let denominator = u128::from(self.denominator) * u128::from(unit.numerator);
        u64::try_from(numerator / denominator).map_err(|_| TimeError::Overflow)
    }

    /// Nearest grid point; exact midpoints resolve to the later point.
    pub fn quantized(self, grid: Self) -> Result<Self, TimeError> {
        let lower = grid.checked_scale(self.units(grid)?, 1)?;
        let remainder = self.checked_sub(lower)?;
        if remainder.checked_scale(2, 1)? >= grid {
            lower.checked_add(grid)
        } else {
            Ok(lower)
        }
    }

    /// Converts a beat duration at constant tempo to nearest audio frame.
    pub fn frames(self, tempo: Tempo, sample_rate: u32) -> Result<u64, TimeError> {
        let scale = 60_000_u128 * u128::from(sample_rate);
        let numerator = u128::from(self.numerator)
            .checked_mul(scale)
            .ok_or(TimeError::Overflow)?;
        let denominator = u128::from(self.denominator) * u128::from(tempo.milli_bpm());
        let rounded = numerator
            .checked_add(denominator / 2)
            .ok_or(TimeError::Overflow)?
            / denominator;
        u64::try_from(rounded).map_err(|_| TimeError::Overflow)
    }

    pub fn from_elapsed_micros(micros: u64, tempo: Tempo) -> Result<Self, TimeError> {
        Self::from_wide(
            u128::from(micros) * u128::from(tempo.milli_bpm()),
            60_000_000_000,
        )
    }

    fn from_wide(numerator: u128, denominator: u128) -> Result<Self, TimeError> {
        if denominator == 0 {
            return Err(TimeError::ZeroDenominator);
        }
        let divisor = gcd(numerator, denominator);
        Ok(Self {
            numerator: u64::try_from(numerator / divisor).map_err(|_| TimeError::Overflow)?,
            denominator: u64::try_from(denominator / divisor).map_err(|_| TimeError::Overflow)?,
        })
    }
}

impl Ord for Beat {
    fn cmp(&self, other: &Self) -> Ordering {
        (u128::from(self.numerator) * u128::from(other.denominator))
            .cmp(&(u128::from(other.numerator) * u128::from(self.denominator)))
    }
}
impl PartialOrd for Beat {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl std::fmt::Display for Beat {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.denominator == 1 {
            write!(out, "{}", self.numerator)
        } else {
            write!(out, "{}/{}", self.numerator, self.denominator)
        }
    }
}
impl<'de> Deserialize<'de> for Beat {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Parts {
            numerator: u64,
            denominator: u64,
        }
        let parts = Parts::deserialize(deserializer)?;
        Self::new(parts.numerator, parts.denominator).map_err(serde::de::Error::custom)
    }
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Tempo in milli-BPM. Persisting integer units avoids changing music on a JSON round trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct Tempo(u32);
impl Tempo {
    pub const DEFAULT: Self = Self(120_000);
    pub fn new(milli_bpm: u32) -> Result<Self, TimeError> {
        if milli_bpm == 0 {
            return Err(TimeError::InvalidTempo);
        }
        Ok(Self(milli_bpm))
    }
    pub const fn milli_bpm(self) -> u32 {
        self.0
    }
    pub fn bpm(self) -> f64 {
        f64::from(self.0) / 1000.0
    }
}
impl TryFrom<u32> for Tempo {
    type Error = TimeError;
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<Tempo> for u32 {
    fn from(value: Tempo) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "DivisionData", into = "DivisionData")]
pub struct Division {
    denominator: u16,
    notes: u16,
    in_time_of: u16,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DivisionData {
    denominator: u16,
    notes: u16,
    in_time_of: u16,
}
impl Division {
    pub const SIXTEENTH: Self = Self {
        denominator: 16,
        notes: 1,
        in_time_of: 1,
    };
    pub fn new(denominator: u16, notes: u16, in_time_of: u16) -> Result<Self, TimeError> {
        if !(4..=64).contains(&denominator) || !denominator.is_power_of_two() {
            return Err(TimeError::InvalidDivision);
        }
        if notes == 0 || in_time_of == 0 {
            return Err(TimeError::InvalidTuplet);
        }
        Ok(Self {
            denominator,
            notes,
            in_time_of,
        })
    }
    pub fn beat_length(self) -> Beat {
        // Validated u16 inputs keep these products representable and denominator positive.
        Beat::new(
            4 * u64::from(self.in_time_of),
            u64::from(self.denominator) * u64::from(self.notes),
        )
        .expect("validated division always fits exact beat representation")
    }
    pub const fn denominator(self) -> u16 {
        self.denominator
    }
    pub const fn tuplet(self) -> (u16, u16) {
        (self.notes, self.in_time_of)
    }
}
impl TryFrom<DivisionData> for Division {
    type Error = TimeError;
    fn try_from(value: DivisionData) -> Result<Self, Self::Error> {
        Self::new(value.denominator, value.notes, value.in_time_of)
    }
}
impl From<Division> for DivisionData {
    fn from(value: Division) -> Self {
        Self {
            denominator: value.denominator,
            notes: value.notes,
            in_time_of: value.in_time_of,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "[u16; 2]", into = "[u16; 2]")]
pub struct Meter {
    numerator: u16,
    denominator: u16,
}
impl Meter {
    pub const COMMON: Self = Self {
        numerator: 4,
        denominator: 4,
    };
    pub fn new(numerator: u16, denominator: u16) -> Result<Self, TimeError> {
        if numerator == 0 || !denominator.is_power_of_two() {
            return Err(TimeError::InvalidMeter);
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }
    pub fn bar_length(self) -> Beat {
        Beat::new(u64::from(self.numerator) * 4, u64::from(self.denominator))
            .expect("validated meter always fits exact beat representation")
    }
    pub const fn numerator(self) -> u16 {
        self.numerator
    }
    pub const fn denominator(self) -> u16 {
        self.denominator
    }
}
impl TryFrom<[u16; 2]> for Meter {
    type Error = TimeError;
    fn try_from(value: [u16; 2]) -> Result<Self, Self::Error> {
        Self::new(value[0], value[1])
    }
}
impl From<Meter> for [u16; 2] {
    fn from(value: Meter) -> Self {
        [value.numerator, value.denominator]
    }
}
