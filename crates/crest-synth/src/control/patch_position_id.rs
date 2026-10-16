use crate::kernel::PatchId;
use core::fmt;
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};

/// Stable interaction identity for a created Patch or the one trailing empty
/// Patch position.
///
/// `TrailingEmpty` is deliberately not a `PatchId`: it is never persisted,
/// projected into the active parameter snapshot, or installed in an audio
/// graph. Created positions retain their historic numeric JSON shape so event
/// and focus records remain compatible.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PatchPositionId {
    Created(PatchId),
    TrailingEmpty,
}

impl PatchPositionId {
    pub const fn created(patch_id: PatchId) -> Self {
        Self::Created(patch_id)
    }

    pub const fn patch_id(self) -> Option<PatchId> {
        match self {
            Self::Created(patch_id) => Some(patch_id),
            Self::TrailingEmpty => None,
        }
    }

    pub const fn is_trailing_empty(self) -> bool {
        matches!(self, Self::TrailingEmpty)
    }
}

impl From<PatchId> for PatchPositionId {
    fn from(patch_id: PatchId) -> Self {
        Self::Created(patch_id)
    }
}

impl Serialize for PatchPositionId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Created(patch_id) => patch_id.serialize(serializer),
            Self::TrailingEmpty => serializer.serialize_str("trailingEmpty"),
        }
    }
}

impl<'de> Deserialize<'de> for PatchPositionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Representation {
            Created(PatchId),
            Named(String),
        }

        match Representation::deserialize(deserializer)? {
            Representation::Created(patch_id) => Ok(Self::Created(patch_id)),
            Representation::Named(name) if name == "trailingEmpty" => Ok(Self::TrailingEmpty),
            Representation::Named(name) => Err(D::Error::custom(format_args!(
                "unknown Patch position identity {name:?}"
            ))),
        }
    }
}

impl fmt::Display for PatchPositionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Created(patch_id) => patch_id.fmt(formatter),
            Self::TrailingEmpty => formatter.write_str("trailing empty Patch"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PatchPositionId;
    use crate::kernel::PatchId;

    #[test]
    fn created_identity_keeps_the_numeric_wire_shape() {
        let position = PatchPositionId::created(PatchId::new(7).unwrap());

        assert_eq!(serde_json::to_string(&position).unwrap(), "7");
        assert_eq!(
            serde_json::from_str::<PatchPositionId>("7").unwrap(),
            position
        );
    }

    #[test]
    fn trailing_empty_has_an_explicit_non_numeric_wire_shape() {
        assert_eq!(
            serde_json::to_string(&PatchPositionId::TrailingEmpty).unwrap(),
            "\"trailingEmpty\""
        );
        assert_eq!(
            serde_json::from_str::<PatchPositionId>("\"trailingEmpty\"").unwrap(),
            PatchPositionId::TrailingEmpty
        );
    }
}
