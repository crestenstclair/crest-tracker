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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceProjection {
    pub context: String,
    pub title: String,
    pub subtitle: String,
    pub status: String,
    pub focus: String,
    pub adjusting: bool,
    pub columns: Vec<String>,
    pub grid: Vec<WorkspaceGridRow>,
    pub rows: Vec<WorkspaceRow>,
    pub inspector_title: String,
    pub inspector: Vec<WorkspaceRow>,
    pub hints: String,
    pub text_entry: Option<WorkspaceTextEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostView {
    pub name: String,
    pub entry: WorkspaceRow,
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
    Focus { id: String },
    Activate { id: String },
    Submit { id: String, value: String },
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
