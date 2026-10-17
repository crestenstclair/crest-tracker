//! Runtime ownership for a cancellable set of scheduled native voices. A group
//! has no knowledge of clips, tracks, documents or musical time.
use core::{fmt, num::NonZeroU64};

/// Issued by one audio boundary. Never serialized or reused within the process;
/// canceled groups cannot become current when their storage slot is recycled.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct VoiceGroupId(NonZeroU64);
impl VoiceGroupId {
    pub(crate) const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VoiceGroupError {
    Unsupported,
    InvalidCapacity,
    StorageAllocationFailed,
    Full,
    Unknown,
    IdentitiesExhausted,
}
impl fmt::Display for VoiceGroupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "this boundary does not support scheduled voice groups",
            Self::InvalidCapacity => "scheduled voice group capacity must be nonzero",
            Self::StorageAllocationFailed => "scheduled voice group storage allocation failed",
            Self::Full => "scheduled voice group capacity reached",
            Self::Unknown => "scheduled voice group is canceled or belongs to another boundary",
            Self::IdentitiesExhausted => "scheduled voice group identities are exhausted",
        })
    }
}
impl std::error::Error for VoiceGroupError {}
