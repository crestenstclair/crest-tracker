use core::fmt;
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};

/// A stable, namespaced identity for one installed instrument capability.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct CapabilityId(String);

impl CapabilityId {
    /// Constructs an identifier from lowercase kebab-case segments separated by dots.
    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        validate_namespaced_identifier(&value)?;
        Ok(Self(value))
    }

    /// Returns the stable serialized value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CapabilityId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// The reason a stable semantic identifier could not be constructed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentifierError {
    value: String,
}

impl IdentifierError {
    pub(crate) fn new(value: &str) -> Self {
        Self {
            value: value.to_owned(),
        }
    }

    /// Returns the rejected identifier text.
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Display for IdentifierError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "identifier {:?} must contain lowercase kebab-case segments separated by dots",
            self.value
        )
    }
}

impl std::error::Error for IdentifierError {}

pub(crate) fn validate_namespaced_identifier(value: &str) -> Result<(), IdentifierError> {
    if value.is_empty()
        || value.split('.').any(|segment| {
            segment.is_empty()
                || segment.starts_with('-')
                || segment.ends_with('-')
                || segment.bytes().any(|byte| {
                    !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                })
        })
    {
        return Err(IdentifierError::new(value));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::CapabilityId;

    #[test]
    fn capability_id_accepts_stable_namespaced_kebab_case() {
        let id = CapabilityId::new("instrument.soundfont.hi-def").unwrap();
        assert_eq!(id.as_str(), "instrument.soundfont.hi-def");
        assert_eq!(
            serde_json::to_string(&id).unwrap(),
            "\"instrument.soundfont.hi-def\""
        );
        assert_eq!(
            serde_json::from_str::<CapabilityId>("\"instrument.soundfont.hi-def\"").unwrap(),
            id
        );
    }

    #[test]
    fn capability_id_rejects_unstable_or_malformed_values() {
        for value in [
            "",
            "SoundFont",
            "sound_font",
            ".soundfont",
            "soundfont.",
            "soundfont..file",
            "soundfont.-file",
        ] {
            assert!(CapabilityId::new(value).is_err(), "accepted {value:?}");
        }
    }
}
