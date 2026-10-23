//! Host-neutral workspace projections painted by the existing Synth renderer.
//! Hosts supply immutable data and semantic events, never HTML or physical input.
use serde::{Deserialize, Serialize};
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRow {
    pub id: String,
    pub label: String,
    pub value: String,
    pub enabled: bool,
    pub span: usize,
    pub selected: bool,
    pub playing: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorkspaceGridRow {
    pub id: String,
    pub label: String,
    pub cells: Vec<WorkspaceRow>,
    pub playing: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTextEntry {
    pub id: String,
    pub label: String,
    pub value: String,
    pub submit_label: String,
}

/// Host-neutral duration marks over a labeled time/lane grid. Coordinates are
/// ten-thousandths of the visible time window; document timing stays host-owned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSpan {
    pub id: String,
    pub level_id: String,
    pub label: String,
    pub lane: usize,
    pub start: u32,
    pub end: u32,
    pub level: u8,
    pub selected: bool,
}
/// A host-owned value trace aligned to the timeline. Both axes use 0–10000.
/// The host supplies every segment; the renderer never interpolates data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorkspaceCurve {
    pub label: String,
    pub caption: String,
    pub maximum: u32,
    pub path: Vec<[u32; 2]>,
    pub points: Vec<WorkspaceCurvePoint>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorkspaceCurvePoint {
    pub label: String,
    pub position: u32,
    pub value: u32,
    pub selected: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceTimeline {
    pub label: String,
    pub caption: String,
    pub view_label: String,
    /// Exact editing position, independent of the audible playhead.
    pub cursor: Option<u32>,
    /// Host-authored lane emphasis (for example, the raised keys of a keyboard).
    pub secondary_lanes: Vec<usize>,
    pub muted_lanes: Vec<usize>,
    pub spans: Vec<WorkspaceSpan>,
    /// Original timing outlines while the host projects an uncommitted edit.
    pub originals: Vec<WorkspaceSpan>,
    pub beat_columns: Vec<usize>,
    pub cursor_lane: Option<usize>,
    pub level_action: WorkspaceRow,
    pub level_focused: bool,
    pub curve: Option<WorkspaceCurve>,
    pub cursor_width: u32,
    pub playhead: Option<u32>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorkspacePreview {
    pub title: String,
    pub caption: String,
    pub values: Vec<String>,
    pub keys: Vec<WorkspaceRow>,
    pub controls: Vec<WorkspaceRow>,
    pub footer: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceProjection {
    pub overlay: bool,
    /// A non-interactive progress modal; the host also gates input admission.
    pub busy: bool,
    /// Compact contextual fields alongside an underlying timeline.
    pub panel_columns: usize,
    pub panel_caption: String,
    pub summary: String,
    pub playing: bool,
    pub timeline: Option<WorkspaceTimeline>,
    pub preview: Option<WorkspacePreview>,
    pub context: String,
    pub title: String,
    pub subtitle: String,
    pub status: String,
    pub focus: String,
    pub adjusting: bool,
    pub columns: Vec<String>,
    pub compact_grid: bool,
    pub grid: Vec<WorkspaceGridRow>,
    pub rows: Vec<WorkspaceRow>,
    pub tabs: Vec<WorkspaceRow>,
    pub inspector_title: String,
    pub inspector: Vec<WorkspaceRow>,
    /// Focus-local controls, grouped by newline. The footer wraps each group.
    pub hints: String,
    pub text_entry: Option<WorkspaceTextEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostView {
    pub name: String,
    pub entry: WorkspaceRow,
    /// Host-consumed semantic actions replace the matching native footer hint.
    pub action_overrides: Vec<crate::control::ValidAction>,
    pub workspace: Option<WorkspaceProjection>,
}
impl HostView {
    pub fn synth_visible(&self) -> bool {
        self.workspace.is_none()
    }
    pub fn text_entry(&self) -> bool {
        self.workspace
            .as_ref()
            .is_some_and(|workspace| workspace.text_entry.is_some())
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum WorkspaceEvent {
    /// Measured list capacity; `id` identifies the first projected row.
    ListPageSize {
        id: String,
        rows: usize,
    },
    Focus {
        id: String,
    },
    Activate {
        id: String,
    },
    Submit {
        id: String,
        value: String,
    },
    TextChanged {
        id: String,
        value: String,
    },
}

#[derive(Clone)]
pub struct HostPresentation {
    pub projection: Rc<dyn Fn() -> HostView>,
    pub event: Rc<dyn Fn(WorkspaceEvent)>,
}
impl std::fmt::Debug for HostPresentation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostPresentation")
            .finish_non_exhaustive()
    }
}
