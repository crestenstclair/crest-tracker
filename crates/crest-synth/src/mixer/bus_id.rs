use core::fmt;
use serde::{Deserialize, Deserializer, Serialize};

/// Initial number of sends. This is a startup default, not a product limit.
pub const DEFAULT_BUS_RETURNS: usize = 16;
/// Compatibility name for the default storage size, not an identity limit.
pub const MAX_BUS_RETURNS: usize = DEFAULT_BUS_RETURNS;

/// Stable positional identity of one bus return, independent of its contents.
///
/// Every representable identity is valid. A return bank determines which
/// identities are installed; the default bank does not restrict this type.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct BusId(u16);

impl BusId {
    /// Number of returns installed by default, not an identity limit.
    pub const COUNT: usize = DEFAULT_BUS_RETURNS;
    pub const MIN: u16 = 0;
    pub const MAX: u16 = u16::MAX;
    /// Identities installed by default. Larger banks use additional identities.
    pub const ALL: [Self; Self::COUNT] = {
        let mut all = [Self(0); Self::COUNT];
        let mut index = 0;
        while index < Self::COUNT {
            all[index] = Self(index as u16);
            index += 1;
        }
        all
    };

    /// Creates an identity without imposing a bank size or substituting values.
    /// The result signature is retained for callers of the prior bounded type.
    pub const fn new(value: u16) -> Result<Self, BusIdError> {
        Ok(Self(value))
    }

    pub const fn value(self) -> u16 {
        self.0
    }

    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

impl Default for BusId {
    fn default() -> Self {
        Self(Self::MIN)
    }
}

impl fmt::Display for BusId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "B{}", self.0)
    }
}

impl TryFrom<u16> for BusId {
    type Error = BusIdError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<u8> for BusId {
    type Error = BusIdError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::new(u16::from(value))
    }
}

impl<'de> Deserialize<'de> for BusId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u16::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl From<BusId> for u16 {
    fn from(value: BusId) -> Self {
        value.value()
    }
}

/// Compatibility error type retained by the identity constructor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BusIdError {
    value: u16,
}

impl BusIdError {
    pub const fn value(self) -> u16 {
        self.value
    }
}

impl fmt::Display for BusIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "bus id must be in {}..={}, got {}",
            BusId::MIN,
            BusId::MAX,
            self.value
        )
    }
}

impl std::error::Error for BusIdError {}

#[cfg(test)]
mod tests {
    use super::{BusId, DEFAULT_BUS_RETURNS, MAX_BUS_RETURNS};

    #[test]
    fn default_bank_is_positional_without_limiting_identity() {
        assert_eq!(DEFAULT_BUS_RETURNS, 16);
        assert_eq!(MAX_BUS_RETURNS, DEFAULT_BUS_RETURNS);
        assert_eq!(BusId::ALL.len(), DEFAULT_BUS_RETURNS);
        for (index, bus) in BusId::ALL.into_iter().enumerate() {
            assert_eq!(bus.index(), index);
            assert_eq!(bus.value(), index as u16);
            assert_eq!(BusId::new(index as u16), Ok(bus));
        }
        assert_eq!(BusId::ALL[3].to_string(), "B3");
        assert_eq!(BusId::new(256).unwrap().value(), 256);
        assert_eq!(BusId::new(u16::MAX).unwrap().value(), u16::MAX);
    }

    #[test]
    fn serde_preserves_identity_and_rejects_unrepresentable_values() {
        for value in [6, 16, 256, u16::MAX] {
            let bus = BusId::new(value).unwrap();
            let encoded = serde_json::to_string(&bus).unwrap();
            assert_eq!(encoded, value.to_string());
            assert_eq!(serde_json::from_str::<BusId>(&encoded).unwrap(), bus);
        }
        assert!(serde_json::from_str::<BusId>("65536").is_err());
        assert!(serde_json::from_str::<BusId>("-1").is_err());
    }
}
