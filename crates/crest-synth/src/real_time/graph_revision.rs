use core::fmt;
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};

/// Monotonic identity for one completely prepared structural audio graph.
///
/// Zero is reserved by structural handoff status to mean that no graph has
/// been published or retired. A `GraphRevision` is therefore always nonzero,
/// copyable, fixed-size, and owns no destructible state.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct GraphRevision(u64);

impl GraphRevision {
    /// The first valid graph revision.
    pub const INITIAL: Self = Self(1);

    /// Creates a graph revision when `value` is nonzero.
    pub const fn new(value: u64) -> Result<Self, GraphRevisionError> {
        if value == 0 {
            Err(GraphRevisionError::Zero)
        } else {
            Ok(Self(value))
        }
    }

    /// Returns the validated numeric identity.
    pub const fn value(self) -> u64 {
        self.0
    }

    /// Returns the next monotonic revision when the numeric range is not
    /// exhausted.
    pub const fn checked_next(self) -> Result<Self, GraphRevisionError> {
        match self.0.checked_add(1) {
            Some(value) => Ok(Self(value)),
            None => Err(GraphRevisionError::Exhausted),
        }
    }
}

impl TryFrom<u64> for GraphRevision {
    type Error = GraphRevisionError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<GraphRevision> for u64 {
    fn from(revision: GraphRevision) -> Self {
        revision.value()
    }
}

impl<'de> Deserialize<'de> for GraphRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

impl fmt::Display for GraphRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// The reason a numeric value could not identify a prepared graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphRevisionError {
    Zero,
    Exhausted,
}

impl fmt::Display for GraphRevisionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zero => formatter.write_str("graph revision must be nonzero"),
            Self::Exhausted => formatter.write_str("graph revision range is exhausted"),
        }
    }
}

impl std::error::Error for GraphRevisionError {}

#[cfg(test)]
mod tests {
    use super::{GraphRevision, GraphRevisionError};

    #[test]
    fn revision_is_nonzero_copyable_and_monotonic() {
        fn assert_copy<T: Copy>() {}

        let first = GraphRevision::INITIAL;
        let second = first.checked_next().unwrap();

        assert_copy::<GraphRevision>();
        assert!(!core::mem::needs_drop::<GraphRevision>());
        assert_eq!(first.value(), 1);
        assert_eq!(second.value(), 2);
        assert!(first < second);
    }

    #[test]
    fn rejects_the_reserved_zero_and_numeric_exhaustion() {
        assert_eq!(GraphRevision::new(0), Err(GraphRevisionError::Zero));
        assert_eq!(
            GraphRevision::new(u64::MAX).unwrap().checked_next(),
            Err(GraphRevisionError::Exhausted)
        );
    }
}
