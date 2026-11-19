//! The agent flow shown by the chat panel's GRAPH tab: what one agent and
//! every agent it spawned did, as the kernel recorded it.
//!
//! Each agent is a [`FlowLane`]; each prompt, model call, tool call, reply,
//! spawn, message and finished process is a [`FlowNode`] in its agent's
//! lane. Links between nodes say which call started which agent and which
//! reply went back to whom.

use crate::events::TraceNodeStatus;
use serde::{Deserialize, Serialize};

/// What a [`FlowNode`] stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowKind {
    /// A prompt: typed by the user, or written by the agent or harness
    /// that gave this agent work. Starts a turn.
    Prompt,
    /// A model call
    Model,
    /// Gate asking a model what a prompt is, to pick the model that takes
    /// it; recorded before that prompt
    Gate,
    /// Gate's judge reviewing failed work
    Judge,
    /// A model naming the chat
    Title,
    /// A tool call, with its policy decision and result once they exist
    Tool,
    /// The agent's reply
    Reply,
    /// A background agent was started
    Spawn,
    /// A message from another agent
    Message,
    /// A background process ended
    Process,
    /// What a task given to another agent came back with
    Result,
    /// Anything else the kernel recorded (session start, mode change, ...)
    #[default]
    Event,
}

impl FlowKind {
    /// Lower-case name shown on the node's box.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Prompt => "prompt",
            Self::Model => "model",
            Self::Gate => "gate",
            Self::Judge => "judge",
            Self::Title => "title",
            Self::Tool => "tool",
            Self::Reply => "reply",
            Self::Spawn => "spawn",
            Self::Message => "message",
            Self::Process => "process",
            Self::Result => "result",
            Self::Event => "event",
        }
    }

    /// Whether it is a model call: the agent's own, or one the harness
    /// made around its work.
    #[must_use]
    pub const fn is_model_call(self) -> bool {
        matches!(self, Self::Model | Self::Gate | Self::Judge | Self::Title)
    }
}

/// One agent in the flow.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowLane {
    /// Hex id of the agent
    pub agent_id: String,
    /// Display name
    pub name: String,
    /// Hex id of the agent that spawned it; `None` for the agent the flow
    /// was asked for
    pub parent: Option<String>,
}

/// One box of the flow.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FlowNode {
    /// Stable id within the flow
    pub id: String,
    /// Hex id of the agent whose record it comes from
    pub agent_id: String,
    pub kind: FlowKind,
    /// What it did, on one line
    pub label: String,
    /// Short facts shown beside the kind: model, tokens, duration
    pub detail: String,
    /// Health
    pub status: TraceNodeStatus,
    /// Sequence number, in its agent's record, of the entry it comes from
    pub seq: u64,
    /// When that entry was recorded (Unix ms)
    pub at_ms: u64,
    /// Metadata shown when the node is opened, in display order
    pub meta: Vec<(String, String)>,
    /// Edges from this node: (edge kind, other node id)
    pub links: Vec<(String, String)>,
}

impl FlowNode {
    /// Apply `update`, a later report about the same node: what it says
    /// replaces what was known, what it leaves empty is kept, and its
    /// metadata and links are added to the node's.
    pub fn merge(&mut self, update: Self) {
        if !update.label.is_empty() {
            self.label = update.label;
        }
        if !update.detail.is_empty() {
            self.detail = update.detail;
        }
        if update.status != TraceNodeStatus::Pending {
            self.status = update.status;
        }
        for (key, value) in update.meta {
            match self.meta.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 = value,
                None => self.meta.push((key, value)),
            }
        }
        for link in update.links {
            if !self.links.contains(&link) {
                self.links.push(link);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merging_keeps_what_an_update_leaves_out() {
        let mut node = FlowNode {
            id: "a:1".into(),
            kind: FlowKind::Tool,
            label: "read_file src/main.rs".into(),
            meta: vec![("tool".into(), "read_file".into())],
            links: vec![("spawn".into(), "b:1".into())],
            ..FlowNode::default()
        };
        node.merge(FlowNode {
            id: "a:1".into(),
            detail: "12ms".into(),
            status: TraceNodeStatus::Ok,
            meta: vec![
                ("tool".into(), "read_file".into()),
                ("result".into(), "fn main() {}".into()),
            ],
            links: vec![
                ("spawn".into(), "b:1".into()),
                ("spawn".into(), "c:1".into()),
            ],
            ..FlowNode::default()
        });
        assert_eq!(node.label, "read_file src/main.rs");
        assert_eq!(node.detail, "12ms");
        assert_eq!(node.status, TraceNodeStatus::Ok);
        assert_eq!(node.meta.len(), 2);
        assert_eq!(node.links.len(), 2);

        // A status that is still pending says nothing new.
        node.merge(FlowNode::default());
        assert_eq!(node.status, TraceNodeStatus::Ok);
    }
}
