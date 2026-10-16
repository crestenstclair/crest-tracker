use crate::real_time::graph_handoff_status::GraphHandoffStatus;
use crate::real_time::graph_revision::GraphRevision;
use crate::real_time::prepared_graph::PreparedGraph;
use core::fmt;

/// Control/worker-only operations for complete structural graph ownership.
pub trait ControlStructuralGraphBoundary: Send {
    fn publish_prepared_on_control(
        &mut self,
        graph: PreparedGraph,
    ) -> Result<(), StructuralBoundaryFull>;

    /// Pops and destroys at most one returned graph on control ownership and
    /// reports its revision. Repeated calls drain the bounded return queue.
    fn collect_retired_on_control(&mut self) -> Option<GraphRevision>;

    fn read_status_on_control(&self) -> GraphHandoffStatus;
}

impl<Boundary> ControlStructuralGraphBoundary for Box<Boundary>
where
    Boundary: ControlStructuralGraphBoundary + ?Sized,
{
    fn publish_prepared_on_control(
        &mut self,
        graph: PreparedGraph,
    ) -> Result<(), StructuralBoundaryFull> {
        (**self).publish_prepared_on_control(graph)
    }

    fn collect_retired_on_control(&mut self) -> Option<GraphRevision> {
        (**self).collect_retired_on_control()
    }

    fn read_status_on_control(&self) -> GraphHandoffStatus {
        (**self).read_status_on_control()
    }
}

/// Callback-only operations for complete structural graph ownership.
pub trait AudioStructuralGraphBoundary: Send {
    fn take_prepared_on_audio(&mut self) -> Option<PreparedGraph>;

    fn return_retired_on_audio(&mut self, graph: PreparedGraph) -> Result<(), RetiredBoundaryFull>;

    fn publish_status_on_audio(&mut self, status: GraphHandoffStatus);
}

/// Factory for distinct control/worker and callback structural handles.
pub trait StructuralGraphBoundary: Send + Sized {
    type ControlHandle: ControlStructuralGraphBoundary;
    type AudioHandle: AudioStructuralGraphBoundary;

    fn into_handles(self) -> (Self::ControlHandle, Self::AudioHandle);
}

/// Callback-side structural handle for compositions that deliberately publish
/// no graph replacements. It retains status locally and preserves any
/// unexpected retired owner by returning it to the caller.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoStructuralGraphChanges {
    status: GraphHandoffStatus,
}

impl NoStructuralGraphChanges {
    pub fn new() -> Self {
        Self {
            status: GraphHandoffStatus::default(),
        }
    }

    pub const fn status(self) -> GraphHandoffStatus {
        self.status
    }
}

impl AudioStructuralGraphBoundary for NoStructuralGraphChanges {
    fn take_prepared_on_audio(&mut self) -> Option<PreparedGraph> {
        None
    }

    fn return_retired_on_audio(&mut self, graph: PreparedGraph) -> Result<(), RetiredBoundaryFull> {
        Err(RetiredBoundaryFull::new(graph))
    }

    fn publish_status_on_audio(&mut self, status: GraphHandoffStatus) {
        self.status = status;
    }
}

/// A full control-to-audio queue preserving the untouched candidate graph.
#[must_use = "the prepared graph remains owned by the control side"]
#[derive(Debug)]
pub struct StructuralBoundaryFull {
    graph: PreparedGraph,
}

impl StructuralBoundaryFull {
    pub const fn new(graph: PreparedGraph) -> Self {
        Self { graph }
    }

    pub const fn graph(&self) -> &PreparedGraph {
        &self.graph
    }

    pub fn into_graph(self) -> PreparedGraph {
        self.graph
    }
}

impl fmt::Display for StructuralBoundaryFull {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the prepared structural graph queue is full")
    }
}

impl std::error::Error for StructuralBoundaryFull {}

/// A full audio-to-control queue preserving the untouched retired graph.
#[must_use = "the retired graph must remain in callback-owned fixed storage"]
#[derive(Debug)]
pub struct RetiredBoundaryFull {
    graph: PreparedGraph,
}

impl RetiredBoundaryFull {
    pub const fn new(graph: PreparedGraph) -> Self {
        Self { graph }
    }

    pub const fn graph(&self) -> &PreparedGraph {
        &self.graph
    }

    pub fn into_graph(self) -> PreparedGraph {
        self.graph
    }
}

impl fmt::Display for RetiredBoundaryFull {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the retired structural graph queue is full")
    }
}

impl std::error::Error for RetiredBoundaryFull {}
