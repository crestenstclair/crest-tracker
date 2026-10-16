use core::fmt;
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};

/// Stable identity assigned to one instrument patch.
///
/// Patch identifiers are strictly non-zero, so a successfully constructed value
/// can be passed between control, synthesis, and audio boundaries without
/// repeated validation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct PatchId(u32);

impl PatchId {
    /// The smallest valid patch identifier.
    pub const MIN: u32 = 1;

    /// Creates a patch identifier when `value` is non-zero.
    pub const fn new(value: u32) -> Result<Self, PatchIdError> {
        if value == 0 {
            Err(PatchIdError::Zero)
        } else {
            Ok(Self(value))
        }
    }

    /// Returns the validated identifier value.
    pub const fn value(self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for PatchId {
    type Error = PatchIdError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<PatchId> for u32 {
    fn from(patch_id: PatchId) -> Self {
        patch_id.value()
    }
}

impl<'de> Deserialize<'de> for PatchId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u32::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

impl fmt::Display for PatchId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// The reason a raw value could not be used as a patch identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PatchIdError {
    /// Patch identifiers reserve zero as an invalid sentinel.
    Zero,
}

impl fmt::Display for PatchIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("patch id must be non-zero")
    }
}

impl std::error::Error for PatchIdError {}

#[cfg(test)]
mod tests {
    use super::{PatchId, PatchIdError};

    #[test]
    fn accepts_every_non_zero_boundary_value() {
        assert_eq!(PatchId::new(PatchId::MIN).unwrap().value(), 1);
        assert_eq!(PatchId::new(u32::MAX).unwrap().value(), u32::MAX);
    }

    #[test]
    fn rejects_zero() {
        let error = PatchId::new(0).unwrap_err();

        assert_eq!(error, PatchIdError::Zero);
        assert_eq!(error.to_string(), "patch id must be non-zero");
    }

    #[test]
    fn primitive_conversions_preserve_the_validated_value() {
        let patch_id = PatchId::try_from(42).unwrap();

        assert_eq!(u32::from(patch_id), 42);
        assert_eq!(patch_id.to_string(), "42");
    }

    #[test]
    fn identity_traits_compare_the_underlying_value() {
        let first = PatchId::new(1).unwrap();
        let same = PatchId::new(1).unwrap();
        let second = PatchId::new(2).unwrap();

        assert_eq!(first, same);
        assert!(first < second);
    }
}
