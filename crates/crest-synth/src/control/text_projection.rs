use crate::control::top_level_context::TopLevelContext;
use serde::{Serialize, Serializer};
use std::sync::Arc;

/// The immutable text view derived from one accepted state snapshot.
///
/// Formatting and selection are calculated by the state projector. This value
/// keeps the rendered body, its selected line, and the originating snapshot
/// hash together so view adapters cannot observe mismatched projection parts.
#[derive(Clone, Debug, Eq)]
pub struct TextProjection {
    context: TopLevelContext,
    body: Arc<str>,
    selected_line: usize,
    state_hash: Arc<str>,
}

impl PartialEq for TextProjection {
    fn eq(&self, other: &Self) -> bool {
        self.context == other.context
            && self.selected_line == other.selected_line
            && (Arc::ptr_eq(&self.state_hash, &other.state_hash)
                || self.state_hash == other.state_hash)
            && (Arc::ptr_eq(&self.body, &other.body) || self.body == other.body)
    }
}

impl Serialize for TextProjection {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct SerializableTextProjection<'a> {
            context: TopLevelContext,
            body: &'a str,
            selected_line: usize,
            state_hash: &'a str,
        }

        SerializableTextProjection {
            context: self.context,
            body: self.body(),
            selected_line: self.selected_line,
            state_hash: self.state_hash(),
        }
        .serialize(serializer)
    }
}

impl TextProjection {
    pub const SERIALIZED_PROPERTY_DESCRIPTOR: &'static [&'static str] =
        &["context", "body", "selectedLine", "stateHash"];

    /// Returns the production-owned serialized projection property surface.
    pub const fn serialized_property_descriptor() -> &'static [&'static str] {
        Self::SERIALIZED_PROPERTY_DESCRIPTOR
    }

    /// Collects the complete deterministic projection derived by the state
    /// projector from a single snapshot.
    pub fn new(body: String, selected_line: usize, state_hash: String) -> Self {
        Self::for_context(TopLevelContext::Mixer, body, selected_line, state_hash)
    }

    /// Collects a projection for one explicit reducer-owned context.
    pub fn for_context(
        context: TopLevelContext,
        body: String,
        selected_line: usize,
        state_hash: String,
    ) -> Self {
        Self {
            context,
            body: Arc::from(body),
            selected_line,
            state_hash: Arc::from(state_hash),
        }
    }

    /// Reuses an unchanged body for a MIDI-only accepted generation.
    pub(crate) fn with_state_hash(&self, state_hash: String) -> Self {
        Self {
            context: self.context,
            body: Arc::clone(&self.body),
            selected_line: self.selected_line,
            state_hash: Arc::from(state_hash),
        }
    }

    /// Returns the reducer-owned semantic context represented by this body.
    pub const fn context(&self) -> TopLevelContext {
        self.context
    }

    /// Returns the complete single-screen wall of text.
    pub fn body(&self) -> &str {
        &self.body
    }

    /// Returns the zero-based body line carrying the selection marker.
    pub const fn selected_line(&self) -> usize {
        self.selected_line
    }

    /// Returns the hash of the state snapshot used to derive this projection.
    pub fn state_hash(&self) -> &str {
        &self.state_hash
    }
}

#[cfg(test)]
mod tests {
    use super::TextProjection;
    use crate::control::TopLevelContext;

    const HEADER: &str = "KEYS: W/S parameters | A/D tracks | K+direction edit";
    const SEPARATOR: &str = "------------------------------------------------------------";

    #[test]
    fn projection_keeps_one_snapshot_body_selection_and_hash_together() {
        let body = format!(
            "{HEADER}\nTRACK T00 routedPatches=[01:Lead]\n> levelDb=-6 pan=0 sends[0]=0.2 sends[1]=0.1\n{SEPARATOR}\nRETURNS\nRETURN B0 occupancy=empty\n returnLevel=0.5\n{SEPARATOR}\nGLOBAL\n masterGainDb=-3"
        );
        let projection = TextProjection::for_context(
            TopLevelContext::Mixer,
            body.clone(),
            2,
            "accepted-state-hash".to_owned(),
        );

        assert_eq!(projection.body(), body);
        assert_eq!(projection.selected_line(), 2);
        assert_eq!(
            projection.body().lines().nth(2),
            Some("> levelDb=-6 pan=0 sends[0]=0.2 sends[1]=0.1")
        );
        assert_eq!(projection.state_hash(), "accepted-state-hash");
    }

    #[test]
    fn cloning_preserves_the_atomic_projection_value() {
        let projection = TextProjection::for_context(
            TopLevelContext::Mixer,
            format!("{HEADER}\nGLOBAL\n> masterGainDb=0"),
            2,
            "snapshot-42".to_owned(),
        );

        assert_eq!(projection.clone(), projection);
    }

    #[test]
    fn equality_checks_content_and_identity_for_shared_and_independent_bodies() {
        let projection = TextProjection::new("body".into(), 0, "generation-1".into());
        let independent = TextProjection::new("body".into(), 0, "generation-1".into());
        assert_eq!(projection, independent);
        assert_eq!(projection, projection.clone());
        assert_ne!(
            projection,
            projection.with_state_hash("generation-2".into())
        );
        assert_ne!(
            projection,
            TextProjection::new("other".into(), 0, "generation-1".into())
        );
        assert_ne!(
            projection,
            TextProjection::new("body".into(), 1, "generation-1".into())
        );
        assert_ne!(
            projection,
            TextProjection::for_context(
                TopLevelContext::Patch,
                "body".into(),
                0,
                "generation-1".into()
            )
        );
    }

    #[test]
    fn serialized_property_descriptor_exactly_matches_the_projection() {
        let projection = TextProjection::for_context(
            TopLevelContext::Patch,
            "PATCH".to_owned(),
            0,
            "snapshot-7".to_owned(),
        );
        let value = serde_json::to_value(projection).unwrap();
        let mut discovered = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        discovered.sort_unstable();
        let mut described = TextProjection::serialized_property_descriptor().to_vec();
        described.sort_unstable();

        assert_eq!(described, discovered);
    }
}
