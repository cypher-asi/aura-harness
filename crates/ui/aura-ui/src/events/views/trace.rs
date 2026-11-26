//! Nodes of the run graph.

use serde::{Deserialize, Serialize};

/// Health of a run-graph node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TraceNodeStatus {
    /// Still running or waiting for a result
    #[default]
    Pending,
    /// Finished normally
    Ok,
    /// Needed a nudge, retry or block
    Warn,
    /// Failed
    Error,
}

/// One node of the run graph: a run, turn, iteration, tool call,
/// harness decision, reward, subagent or branch.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TraceNode {
    /// Stable id within the run
    pub id: String,
    /// Kind label (`run`, `turn`, `iteration`, `tool`, `decision`, ...)
    pub kind: String,
    /// Containing node
    pub parent: Option<String>,
    /// Short label
    pub label: String,
    /// One-line detail shown after the label
    pub detail: String,
    /// Health
    pub status: TraceNodeStatus,
    /// Trajectory step the node came from
    pub seq: Option<u64>,
    /// Metadata shown in the side pane, in display order
    pub meta: Vec<(String, String)>,
    /// Non-containment edges from this node: (edge kind, other node id)
    pub links: Vec<(String, String)>,
}
