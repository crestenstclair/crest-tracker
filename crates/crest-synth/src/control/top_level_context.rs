use serde::{Deserialize, Serialize};

/// The complete semantic set of top-level product contexts.
///
/// Physical bindings and window tabs are deliberately absent: adapters emit a
/// `SelectContext` event and the reducer owns the selected value.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TopLevelContext {
    Patch,
    #[default]
    Mixer,
}

impl TopLevelContext {
    pub const ALL: [Self; 2] = [Self::Patch, Self::Mixer];

    /// Returns both top-level contexts exactly once.
    pub const fn surface_descriptor() -> &'static [Self] {
        &Self::ALL
    }

    /// Returns the stable uppercase presentation identity used by the basic shell.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Patch => "PATCH",
            Self::Mixer => "MIXER",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TopLevelContext;

    #[test]
    fn top_level_context_surface_is_closed_unique_and_defaults_to_mixer() {
        let descriptor = TopLevelContext::surface_descriptor();
        assert_eq!(
            descriptor,
            &[TopLevelContext::Patch, TopLevelContext::Mixer]
        );
        assert_eq!(TopLevelContext::default(), TopLevelContext::Mixer);
        assert_eq!(TopLevelContext::Patch.label(), "PATCH");
        assert_eq!(TopLevelContext::Mixer.label(), "MIXER");
    }
}
