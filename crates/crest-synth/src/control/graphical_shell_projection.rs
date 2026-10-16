use crate::control::{SemanticGraphicalViewModel, TextProjection, TopLevelContext};
use core::fmt;
use serde::{Serialize, Serializer};
use std::sync::{Arc, OnceLock};

/// The semantic identity of the main shell workspace.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShellMainRegion {
    Patch,
    Mixer,
}

impl ShellMainRegion {
    pub const fn for_context(context: TopLevelContext) -> Self {
        match context {
            TopLevelContext::Patch => Self::Patch,
            TopLevelContext::Mixer => Self::Mixer,
        }
    }
}

/// The persistent side region paired with one top-level context.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShellSideRegion {
    Utility,
    Inspector,
}

impl ShellSideRegion {
    pub const fn for_context(context: TopLevelContext) -> Self {
        match context {
            TopLevelContext::Patch => Self::Utility,
            TopLevelContext::Mixer => Self::Inspector,
        }
    }
}

/// Presentation labels for the shallow product/context/status line.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellContextLine {
    product_label: String,
    context_label: String,
    status_label: String,
}

impl ShellContextLine {
    pub(crate) fn new(
        product_label: impl Into<String>,
        context_label: impl Into<String>,
        status_label: impl Into<String>,
    ) -> Self {
        Self {
            product_label: product_label.into(),
            context_label: context_label.into(),
            status_label: status_label.into(),
        }
    }

    pub fn product_label(&self) -> &str {
        &self.product_label
    }

    pub fn context_label(&self) -> &str {
        &self.context_label
    }

    pub fn status_label(&self) -> &str {
        &self.status_label
    }
}

/// Canonical identity labels shown directly below the context line.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellIdentityHeader {
    primary_label: String,
    secondary_label: String,
}

impl ShellIdentityHeader {
    pub(crate) fn new(
        primary_label: impl Into<String>,
        secondary_label: impl Into<String>,
    ) -> Self {
        Self {
            primary_label: primary_label.into(),
            secondary_label: secondary_label.into(),
        }
    }

    pub fn primary_label(&self) -> &str {
        &self.primary_label
    }

    pub fn secondary_label(&self) -> &str {
        &self.secondary_label
    }
}

/// Main and persistent-side workspace identities plus the retained diagnostic.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellWorkspace {
    main_region: ShellMainRegion,
    main_label: String,
    side_region: ShellSideRegion,
    side_label: String,
    diagnostic: TextProjection,
}

impl ShellWorkspace {
    pub(crate) fn new(
        main_region: ShellMainRegion,
        main_label: impl Into<String>,
        side_region: ShellSideRegion,
        side_label: impl Into<String>,
        diagnostic: TextProjection,
    ) -> Self {
        Self {
            main_region,
            main_label: main_label.into(),
            side_region,
            side_label: side_label.into(),
            diagnostic,
        }
    }

    pub const fn main_region(&self) -> ShellMainRegion {
        self.main_region
    }

    pub fn main_label(&self) -> &str {
        &self.main_label
    }

    pub const fn side_region(&self) -> ShellSideRegion {
        self.side_region
    }

    pub fn side_label(&self) -> &str {
        &self.side_label
    }

    pub const fn diagnostic(&self) -> &TextProjection {
        &self.diagnostic
    }
}

/// Current path and the existing Phase 1 keyboard vocabulary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellFooter {
    path_label: String,
    action_hints: Vec<String>,
}

impl ShellFooter {
    pub(crate) fn new(path_label: impl Into<String>, action_hints: Vec<String>) -> Self {
        Self {
            path_label: path_label.into(),
            action_hints,
        }
    }

    pub fn path_label(&self) -> &str {
        &self.path_label
    }

    pub fn action_hints(&self) -> &[String] {
        &self.action_hints
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphicalShellData {
    generation: u64,
    state_hash: String,
    context: TopLevelContext,
    semantic_model: SemanticGraphicalViewModel,
    context_line: ShellContextLine,
    identity_header: ShellIdentityHeader,
    workspace: ShellWorkspace,
    footer: ShellFooter,
}

/// A coherence failure while collecting one immutable graphical shell value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphicalShellProjectionError {
    DiagnosticContextMismatch,
    DiagnosticHashMismatch,
    MainRegionMismatch,
    SideRegionMismatch,
    SemanticGenerationMismatch,
    SemanticHashMismatch,
    SemanticContextMismatch,
}

impl fmt::Display for GraphicalShellProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::DiagnosticContextMismatch => {
                "graphical shell and retained diagnostic contexts differ"
            }
            Self::DiagnosticHashMismatch => {
                "graphical shell and retained diagnostic state hashes differ"
            }
            Self::MainRegionMismatch => {
                "graphical shell main region does not match its top-level context"
            }
            Self::SideRegionMismatch => {
                "graphical shell side region does not match its top-level context"
            }
            Self::SemanticGenerationMismatch => {
                "graphical shell and semantic model generations differ"
            }
            Self::SemanticHashMismatch => "graphical shell and semantic model state hashes differ",
            Self::SemanticContextMismatch => "graphical shell and semantic model contexts differ",
        })
    }
}

impl std::error::Error for GraphicalShellProjectionError {}

/// The immutable host-neutral structural projection consumed by the window.
///
/// Cloning shares one already-derived data set, so a frame request does not
/// create another application-state or projection authority.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphicalShellProjection {
    data: Arc<GraphicalShellData>,
}

impl Serialize for GraphicalShellProjection {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.data.as_ref().serialize(serializer)
    }
}

impl GraphicalShellProjection {
    pub const SERIALIZED_PROPERTY_DESCRIPTOR: &'static [&'static str] = &[
        "generation",
        "stateHash",
        "context",
        "semanticModel",
        "contextLine",
        "identityHeader",
        "workspace",
        "footer",
    ];

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        generation: u64,
        state_hash: impl Into<String>,
        semantic_model: SemanticGraphicalViewModel,
        context_line: ShellContextLine,
        identity_header: ShellIdentityHeader,
        main_label: impl Into<String>,
        side_label: impl Into<String>,
        diagnostic: TextProjection,
        footer: ShellFooter,
    ) -> Result<Self, GraphicalShellProjectionError> {
        let state_hash = state_hash.into();
        let context = semantic_model.context();
        if semantic_model.generation() != generation {
            return Err(GraphicalShellProjectionError::SemanticGenerationMismatch);
        }
        if semantic_model.state_hash() != state_hash {
            return Err(GraphicalShellProjectionError::SemanticHashMismatch);
        }
        if diagnostic.context() != context {
            return Err(GraphicalShellProjectionError::DiagnosticContextMismatch);
        }
        if diagnostic.state_hash() != state_hash {
            return Err(GraphicalShellProjectionError::DiagnosticHashMismatch);
        }
        let main_region = ShellMainRegion::for_context(context);
        let side_region = ShellSideRegion::for_context(context);
        let workspace =
            ShellWorkspace::new(main_region, main_label, side_region, side_label, diagnostic);
        Self::from_data(GraphicalShellData {
            generation,
            state_hash,
            context,
            semantic_model,
            context_line,
            identity_header,
            workspace,
            footer,
        })
    }

    fn from_data(data: GraphicalShellData) -> Result<Self, GraphicalShellProjectionError> {
        if data.workspace.main_region != ShellMainRegion::for_context(data.context) {
            return Err(GraphicalShellProjectionError::MainRegionMismatch);
        }
        if data.workspace.side_region != ShellSideRegion::for_context(data.context) {
            return Err(GraphicalShellProjectionError::SideRegionMismatch);
        }
        if data.workspace.diagnostic.context() != data.context {
            return Err(GraphicalShellProjectionError::DiagnosticContextMismatch);
        }
        if data.workspace.diagnostic.state_hash() != data.state_hash {
            return Err(GraphicalShellProjectionError::DiagnosticHashMismatch);
        }
        if data.semantic_model.generation() != data.generation {
            return Err(GraphicalShellProjectionError::SemanticGenerationMismatch);
        }
        if data.semantic_model.state_hash() != data.state_hash {
            return Err(GraphicalShellProjectionError::SemanticHashMismatch);
        }
        if data.semantic_model.context() != data.context {
            return Err(GraphicalShellProjectionError::SemanticContextMismatch);
        }
        Ok(Self {
            data: Arc::new(data),
        })
    }

    /// Reuses stable presentation content after a MIDI event changes only the
    /// accepted generation and state-snapshot identity.
    pub(crate) fn with_generation(
        &self,
        generation: u64,
        state_hash: String,
        diagnostic: TextProjection,
    ) -> Result<Self, GraphicalShellProjectionError> {
        let semantic_model = self
            .data
            .semantic_model
            .with_generation(generation, state_hash.clone());
        Self::from_data(GraphicalShellData {
            generation,
            state_hash,
            context: self.data.context,
            semantic_model,
            context_line: self.data.context_line.clone(),
            identity_header: self.data.identity_header.clone(),
            workspace: ShellWorkspace {
                diagnostic,
                ..self.data.workspace.clone()
            },
            footer: self.data.footer.clone(),
        })
    }

    pub const fn serialized_property_descriptor() -> &'static [&'static str] {
        Self::SERIALIZED_PROPERTY_DESCRIPTOR
    }

    pub fn serialized_leaf_descriptor() -> &'static [&'static str] {
        static DESCRIPTOR: OnceLock<Vec<&'static str>> = OnceLock::new();
        DESCRIPTOR.get_or_init(|| {
            let mut descriptor = vec![
                "context",
                "contextLine.contextLabel",
                "contextLine.productLabel",
                "contextLine.statusLabel",
                "footer.actionHints[]",
                "footer.pathLabel",
                "generation",
                "identityHeader.primaryLabel",
                "identityHeader.secondaryLabel",
                "stateHash",
                "workspace.diagnostic.body",
                "workspace.diagnostic.context",
                "workspace.diagnostic.selectedLine",
                "workspace.diagnostic.stateHash",
                "workspace.mainLabel",
                "workspace.mainRegion",
                "workspace.sideLabel",
                "workspace.sideRegion",
            ];
            descriptor.extend(
                SemanticGraphicalViewModel::serialized_leaf_descriptor()
                    .iter()
                    .map(|path| {
                        Box::leak(format!("semanticModel.{path}").into_boxed_str()) as &'static str
                    }),
            );
            descriptor.sort_unstable();
            descriptor.dedup();
            descriptor
        })
    }

    pub fn generation(&self) -> u64 {
        self.data.generation
    }

    pub fn state_hash(&self) -> &str {
        &self.data.state_hash
    }

    pub fn context(&self) -> TopLevelContext {
        self.data.context
    }

    pub fn semantic_model(&self) -> &SemanticGraphicalViewModel {
        &self.data.semantic_model
    }

    /// The one Patch identity this shell speaks for, or `None` in MIXER.
    ///
    /// Paired with [`Self::generation`] this is what lets a consumer assert
    /// NFR-005 from the outside: a patch-selection gesture must advance the
    /// generation by exactly one, and the projection at that generation must
    /// name the destination Patch and nothing else. Both halves are needed —
    /// a single advanced generation naming the wrong Patch and two generations
    /// naming the right one are different defects, and neither is visible from
    /// the other's evidence alone.
    pub fn patch_identity(&self) -> Option<crate::kernel::PatchId> {
        self.data.semantic_model.patch_identity()
    }

    pub fn context_line(&self) -> &ShellContextLine {
        &self.data.context_line
    }

    pub fn identity_header(&self) -> &ShellIdentityHeader {
        &self.data.identity_header
    }

    pub fn workspace(&self) -> &ShellWorkspace {
        &self.data.workspace
    }

    pub fn footer(&self) -> &ShellFooter {
        &self.data.footer
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GraphicalShellProjection, GraphicalShellProjectionError, ShellContextLine, ShellFooter,
        ShellIdentityHeader, ShellMainRegion, ShellSideRegion,
    };
    use crate::control::{TextProjection, TopLevelContext};
    use serde_json::Value;
    use std::collections::BTreeSet;

    fn projection(context: TopLevelContext) -> GraphicalShellProjection {
        let context_label = context.label();
        GraphicalShellProjection::new(
            7,
            "state-7",
            crate::control::SemanticGraphicalViewModel::fixture(7, "state-7", context),
            ShellContextLine::new("CREST SYNTH", context_label, "READY"),
            ShellIdentityHeader::new(context_label, "Fixture identity"),
            format!("{context_label} WORKSPACE"),
            match context {
                TopLevelContext::Patch => "UTILITY",
                TopLevelContext::Mixer => "INSPECTOR",
            },
            TextProjection::for_context(
                context,
                format!("{context_label} diagnostic"),
                0,
                "state-7".to_owned(),
            ),
            ShellFooter::new(
                context_label,
                vec!["1 MIXER".to_owned(), "2 PATCH".to_owned()],
            ),
        )
        .unwrap()
    }

    fn discover_leaves(value: &Value, prefix: &str, leaves: &mut BTreeSet<String>) {
        match value {
            Value::Object(object) => {
                for (name, child) in object {
                    let path = if prefix.is_empty() {
                        name.to_owned()
                    } else {
                        format!("{prefix}.{name}")
                    };
                    discover_leaves(child, &path, leaves);
                }
            }
            Value::Array(values) => {
                for child in values {
                    discover_leaves(child, &format!("{prefix}[]"), leaves);
                }
            }
            _ => {
                leaves.insert(prefix.to_owned());
            }
        }
    }

    #[test]
    fn graphical_shell_projection_maps_patch_and_mixer_exactly() {
        let patch = projection(TopLevelContext::Patch);
        assert_eq!(patch.generation(), 7);
        assert_eq!(patch.context(), TopLevelContext::Patch);
        assert_eq!(patch.state_hash(), "state-7");
        assert_eq!(patch.context_line().context_label(), "PATCH");
        assert_eq!(patch.workspace().main_region(), ShellMainRegion::Patch);
        assert_eq!(patch.workspace().side_region(), ShellSideRegion::Utility);
        assert_eq!(patch.workspace().diagnostic().context(), patch.context());
        assert_eq!(
            patch.workspace().diagnostic().state_hash(),
            patch.state_hash()
        );

        let mixer = projection(TopLevelContext::Mixer);
        assert_eq!(mixer.context(), TopLevelContext::Mixer);
        assert_eq!(mixer.context_line().context_label(), "MIXER");
        assert_eq!(mixer.workspace().main_region(), ShellMainRegion::Mixer);
        assert_eq!(mixer.workspace().side_region(), ShellSideRegion::Inspector);
    }

    #[test]
    fn graphical_shell_projection_rejects_incoherent_diagnostic_identity() {
        let result = GraphicalShellProjection::new(
            1,
            "state-1",
            crate::control::SemanticGraphicalViewModel::fixture(
                1,
                "state-1",
                TopLevelContext::Patch,
            ),
            ShellContextLine::new("CREST SYNTH", "PATCH", "READY"),
            ShellIdentityHeader::new("Patch", "Identity"),
            "PATCH WORKSPACE",
            "UTILITY",
            TextProjection::for_context(
                TopLevelContext::Mixer,
                "MIXER".to_owned(),
                0,
                "state-1".to_owned(),
            ),
            ShellFooter::new("PATCH", Vec::new()),
        );
        assert_eq!(
            result,
            Err(GraphicalShellProjectionError::DiagnosticContextMismatch)
        );
    }

    #[test]
    fn graphical_shell_projection_leaf_descriptor_is_bidirectionally_exact() {
        let value = serde_json::to_value(projection(TopLevelContext::Patch)).unwrap();
        let mut discovered = BTreeSet::new();
        discover_leaves(&value, "", &mut discovered);
        let descriptor = GraphicalShellProjection::serialized_leaf_descriptor();
        let described = descriptor
            .iter()
            .map(|path| (*path).to_owned())
            .collect::<BTreeSet<_>>();

        assert_eq!(descriptor.len(), described.len());
        assert!(descriptor.windows(2).all(|pair| pair[0] < pair[1]));
        let described_shell = described
            .iter()
            .filter(|path| !path.starts_with("semanticModel."))
            .cloned()
            .collect::<BTreeSet<_>>();
        let discovered_shell = discovered
            .iter()
            .filter(|path| !path.starts_with("semanticModel."))
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(described_shell, discovered_shell);
        assert!(discovered.is_subset(&described));
        for leaf in crate::control::SemanticGraphicalViewModel::serialized_leaf_descriptor() {
            assert!(described.contains(&format!("semanticModel.{leaf}")));
        }
    }
}
