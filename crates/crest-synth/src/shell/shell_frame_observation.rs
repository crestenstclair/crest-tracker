use crate::control::{
    FocusPath, InteractionMode, ReturnPath, SemanticAction, SemanticError,
    SemanticGraphicalViewModel, SemanticLifecycleStatus, SurfaceId, TopLevelContext,
};
use core::fmt;
use serde::Serialize;

/// Stable identity of one required Phase 1 shell region.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShellRegionId {
    ContextLine,
    IdentityHeader,
    MainWorkspace,
    PersistentSideRegion,
    Footer,
}

impl ShellRegionId {
    pub const ALL: [Self; 5] = [
        Self::ContextLine,
        Self::IdentityHeader,
        Self::MainWorkspace,
        Self::PersistentSideRegion,
        Self::Footer,
    ];

    pub const fn surface_descriptor() -> &'static [Self] {
        &Self::ALL
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::ContextLine => "contextLine",
            Self::IdentityHeader => "identityHeader",
            Self::MainWorkspace => "mainWorkspace",
            Self::PersistentSideRegion => "persistentSideRegion",
            Self::Footer => "footer",
        }
    }
}

/// Host-neutral pixel bounds copied from one actually composed shell region.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellRegionRect {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
}

impl ShellRegionRect {
    pub const fn new(min_x: f32, min_y: f32, max_x: f32, max_y: f32) -> Self {
        Self {
            min_x,
            min_y,
            max_x,
            max_y,
        }
    }

    pub const fn min_x(self) -> f32 {
        self.min_x
    }

    pub const fn min_y(self) -> f32 {
        self.min_y
    }

    pub const fn max_x(self) -> f32 {
        self.max_x
    }

    pub const fn max_y(self) -> f32 {
        self.max_y
    }

    pub fn width(self) -> f32 {
        self.max_x - self.min_x
    }

    pub fn height(self) -> f32 {
        self.max_y - self.min_y
    }

    pub fn is_finite_nonempty(self) -> bool {
        self.min_x.is_finite()
            && self.min_y.is_finite()
            && self.max_x.is_finite()
            && self.max_y.is_finite()
            && self.max_x > self.min_x
            && self.max_y > self.min_y
    }

    pub fn overlaps(self, other: Self) -> bool {
        self.min_x < other.max_x
            && other.min_x < self.max_x
            && self.min_y < other.max_y
            && other.min_y < self.max_y
    }
}

/// One painted region's stable identity, bounds, and visible text evidence.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellRegionObservation {
    id: ShellRegionId,
    rect: ShellRegionRect,
    visible_label: String,
}

impl ShellRegionObservation {
    pub fn new(id: ShellRegionId, rect: ShellRegionRect, visible_label: impl Into<String>) -> Self {
        Self {
            id,
            rect,
            visible_label: visible_label.into(),
        }
    }

    pub const fn id(&self) -> ShellRegionId {
        self.id
    }

    pub const fn rect(&self) -> ShellRegionRect {
        self.rect
    }

    pub fn visible_label(&self) -> &str {
        &self.visible_label
    }
}

/// What the painting page reported about the PATCH strip's group structure.
///
/// Grouping is decided by the page (`stripGroups` in `webview-page/page.js`)
/// and nowhere else, so this is *transported*, never recomputed: the page reads
/// its own painted DOM back and this value carries the answer across the
/// boundary. Re-deriving the same fact Rust-side would give one fact two
/// producers, which is agreement between copies rather than proof, and the
/// first divergence between them would be silent (mission finding F-44).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StripPaintObservation {
    groups_painted: u32,
    flat_control_run: bool,
}

impl StripPaintObservation {
    pub const fn new(groups_painted: u32, flat_control_run: bool) -> Self {
        Self {
            groups_painted,
            flat_control_run,
        }
    }

    /// How many groups the strip painted.
    pub const fn groups_painted(self) -> u32 {
        self.groups_painted
    }

    /// Whether the strip painted rows with none of them inside a group — the
    /// flat control run FR-001 replaced. A strip that painted no rows at all
    /// is not a flat run and reports `false`.
    pub const fn flat_control_run(self) -> bool {
        self.flat_control_run
    }
}

/// An invalid adapter-boundary frame observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellFrameObservationError {
    InvalidViewport,
    RegionOrder,
    InvalidRegionBounds { region: ShellRegionId },
    EmptyVisibleLabel { region: ShellRegionId },
}

impl fmt::Display for ShellFrameObservationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidViewport => {
                formatter.write_str("shell viewport must be finite and positive")
            }
            Self::RegionOrder => formatter.write_str(
                "shell frame must contain each required region exactly once in canonical order",
            ),
            Self::InvalidRegionBounds { region } => {
                write!(
                    formatter,
                    "{} bounds are invalid or outside the viewport",
                    region.name()
                )
            }
            Self::EmptyVisibleLabel { region } => {
                write!(formatter, "{} has no visible-label evidence", region.name())
            }
        }
    }
}

impl std::error::Error for ShellFrameObservationError {}

/// One immutable observation emitted only after the graphical adapter records
/// all paint commands for the supplied shell projection.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellFrameObservation {
    viewport_width: f32,
    viewport_height: f32,
    generation: u64,
    state_hash: String,
    context: TopLevelContext,
    active_surface: SurfaceId,
    focus_path: FocusPath,
    interaction_mode: InteractionMode,
    return_path: Option<ReturnPath>,
    valid_actions: Vec<SemanticAction>,
    status: SemanticLifecycleStatus,
    errors: Vec<SemanticError>,
    regions: [ShellRegionObservation; 5],
    /// Absent when the painting host reported no strip evidence — a harness
    /// window, or a page older than the field. Absent is not a measured zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    strip: Option<StripPaintObservation>,
}

impl ShellFrameObservation {
    pub fn try_new(
        viewport_width: f32,
        viewport_height: f32,
        generation: u64,
        state_hash: impl Into<String>,
        context: TopLevelContext,
        regions: [ShellRegionObservation; 5],
    ) -> Result<Self, ShellFrameObservationError> {
        let semantic = SemanticGraphicalViewModel::fixture(generation, state_hash, context);
        Self::try_new_semantic(viewport_width, viewport_height, &semantic, regions)
    }

    /// Correlates one post-paint observation with the exact semantic model
    /// supplied to that frame.
    pub fn try_new_semantic(
        viewport_width: f32,
        viewport_height: f32,
        semantic: &SemanticGraphicalViewModel,
        regions: [ShellRegionObservation; 5],
    ) -> Result<Self, ShellFrameObservationError> {
        if !viewport_width.is_finite()
            || !viewport_height.is_finite()
            || viewport_width <= 0.0
            || viewport_height <= 0.0
        {
            return Err(ShellFrameObservationError::InvalidViewport);
        }
        if regions
            .iter()
            .map(ShellRegionObservation::id)
            .ne(ShellRegionId::surface_descriptor().iter().copied())
        {
            return Err(ShellFrameObservationError::RegionOrder);
        }
        for region in &regions {
            let rect = region.rect();
            if !rect.is_finite_nonempty()
                || rect.min_x() < 0.0
                || rect.min_y() < 0.0
                || rect.max_x() > viewport_width
                || rect.max_y() > viewport_height
            {
                return Err(ShellFrameObservationError::InvalidRegionBounds {
                    region: region.id(),
                });
            }
            if region.visible_label().trim().is_empty() {
                return Err(ShellFrameObservationError::EmptyVisibleLabel {
                    region: region.id(),
                });
            }
        }

        Ok(Self {
            viewport_width,
            viewport_height,
            generation: semantic.generation(),
            state_hash: semantic.state_hash().to_owned(),
            context: semantic.context(),
            active_surface: semantic.active_surface(),
            focus_path: semantic.focus_path().clone(),
            interaction_mode: semantic.interaction_mode(),
            return_path: semantic.return_path().cloned(),
            valid_actions: semantic
                .valid_actions()
                .iter()
                .map(|valid| valid.action().clone())
                .collect(),
            status: semantic.status().clone(),
            errors: semantic.errors().to_vec(),
            regions,
            strip: None,
        })
    }

    /// Attaches the painting page's own strip-paint evidence.
    #[must_use]
    pub fn with_strip_paint(mut self, strip: StripPaintObservation) -> Self {
        self.strip = Some(strip);
        self
    }

    /// The painting page's strip-paint evidence, or `None` when the host
    /// reported none.
    pub const fn strip(&self) -> Option<StripPaintObservation> {
        self.strip
    }

    pub const fn viewport_width(&self) -> f32 {
        self.viewport_width
    }

    pub const fn viewport_height(&self) -> f32 {
        self.viewport_height
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub fn state_hash(&self) -> &str {
        &self.state_hash
    }

    pub const fn context(&self) -> TopLevelContext {
        self.context
    }

    pub const fn active_surface(&self) -> SurfaceId {
        self.active_surface
    }

    pub const fn focus_path(&self) -> &FocusPath {
        &self.focus_path
    }

    pub const fn interaction_mode(&self) -> InteractionMode {
        self.interaction_mode
    }

    pub const fn return_path(&self) -> Option<&ReturnPath> {
        self.return_path.as_ref()
    }

    pub fn valid_actions(&self) -> &[SemanticAction] {
        &self.valid_actions
    }

    pub const fn status(&self) -> &SemanticLifecycleStatus {
        &self.status
    }

    pub fn errors(&self) -> &[SemanticError] {
        &self.errors
    }

    pub const fn regions(&self) -> &[ShellRegionObservation; 5] {
        &self.regions
    }

    pub fn region(&self, id: ShellRegionId) -> &ShellRegionObservation {
        &self.regions[id as usize]
    }

    /// Returns true only when no two required painted rectangles overlap.
    pub fn regions_are_non_overlapping(&self) -> bool {
        self.regions.iter().enumerate().all(|(index, region)| {
            self.regions
                .iter()
                .skip(index + 1)
                .all(|other| !region.rect().overlaps(other.rect()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ShellFrameObservation, ShellFrameObservationError, ShellRegionId, ShellRegionObservation,
        ShellRegionRect,
    };
    use crate::control::TopLevelContext;

    fn regions() -> [ShellRegionObservation; 5] {
        [
            ShellRegionObservation::new(
                ShellRegionId::ContextLine,
                ShellRegionRect::new(0.0, 0.0, 1920.0, 48.0),
                "PATCH",
            ),
            ShellRegionObservation::new(
                ShellRegionId::IdentityHeader,
                ShellRegionRect::new(0.0, 48.0, 1920.0, 120.0),
                "PATCH 01",
            ),
            ShellRegionObservation::new(
                ShellRegionId::MainWorkspace,
                ShellRegionRect::new(0.0, 120.0, 1500.0, 1016.0),
                "PATCH WORKSPACE",
            ),
            ShellRegionObservation::new(
                ShellRegionId::PersistentSideRegion,
                ShellRegionRect::new(1500.0, 120.0, 1920.0, 1016.0),
                "UTILITY",
            ),
            ShellRegionObservation::new(
                ShellRegionId::Footer,
                ShellRegionRect::new(0.0, 1016.0, 1920.0, 1080.0),
                "PATCH / engine",
            ),
        ]
    }

    #[test]
    fn shell_frame_observation_preserves_ordered_post_paint_evidence() {
        let observation = ShellFrameObservation::try_new(
            1920.0,
            1080.0,
            9,
            "state-9",
            TopLevelContext::Patch,
            regions(),
        )
        .unwrap();

        assert_eq!(observation.regions().len(), 5);
        assert_eq!(
            observation
                .region(ShellRegionId::PersistentSideRegion)
                .rect()
                .width(),
            420.0
        );
        assert!(observation.regions_are_non_overlapping());
    }

    #[test]
    fn shell_frame_observation_rejects_reordered_or_out_of_bounds_evidence() {
        let mut reordered = regions();
        reordered.swap(0, 1);
        assert_eq!(
            ShellFrameObservation::try_new(
                1920.0,
                1080.0,
                9,
                "state-9",
                TopLevelContext::Patch,
                reordered,
            ),
            Err(ShellFrameObservationError::RegionOrder)
        );

        let mut outside = regions();
        outside[4] = ShellRegionObservation::new(
            ShellRegionId::Footer,
            ShellRegionRect::new(0.0, 1016.0, 1921.0, 1080.0),
            "PATCH / engine",
        );
        assert_eq!(
            ShellFrameObservation::try_new(
                1920.0,
                1080.0,
                9,
                "state-9",
                TopLevelContext::Patch,
                outside,
            ),
            Err(ShellFrameObservationError::InvalidRegionBounds {
                region: ShellRegionId::Footer
            })
        );
    }
}
