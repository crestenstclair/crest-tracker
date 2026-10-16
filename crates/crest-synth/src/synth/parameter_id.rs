use crate::synth::capability_id::{validate_namespaced_identifier, IdentifierError};
use core::fmt;
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};

/// A stable, capability-scoped identity for one instrument parameter.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ParameterId(String);

impl ParameterId {
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

impl fmt::Display for ParameterId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ParameterId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::ParameterId;

    #[test]
    fn parameter_id_round_trips_its_stable_value() {
        let id = ParameterId::new("soundfont.program").unwrap();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<ParameterId>(&json).unwrap(), id);
    }

    #[test]
    fn parameter_id_rejects_labels_paths_and_malformed_segments() {
        for value in [
            "Program",
            "soundfont/program",
            "soundfont. program",
            "-program",
        ] {
            assert!(ParameterId::new(value).is_err(), "accepted {value:?}");
        }
    }
}
