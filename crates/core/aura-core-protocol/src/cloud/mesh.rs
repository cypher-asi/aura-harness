//! Frames on the mesh socket (`WS /v1/mesh`).
//!
//! The hub keeps one socket open to every running cloud node. Nodes of
//! one project also open sockets to each other when they have mail for
//! a peer, using the addresses the hub sends in [`MeshFrame::Directory`].
//! Every socket starts with a [`MeshFrame::Hello`] from the side that
//! opened it.
//!
//! ## Guarantees
//!
//! - Mail is acknowledged by id ([`MeshFrame::MailAck`]); unacknowledged
//!   mail is resent after a reconnect, and receivers drop ids they have
//!   seen, so delivery is at least once and handling exactly once.
//! - [`MeshFrame::RecordCommitted`] is only a hint: the hub pulls the
//!   entries themselves through `GET /agents/:id/record` and checks them.

use serde::{Deserialize, Serialize};

use super::host::HostStats;

/// Version of the mesh protocol, sent in [`MeshFrame::Hello`].
pub const MESH_PROTOCOL: u32 = 1;

/// An agent a node holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshAgent {
    /// Agent id, hex.
    pub agent_id: String,
    /// Display name.
    pub name: String,
    /// `running`, `idle`, `paused` or `stopped`.
    pub state: String,
    /// Parent agent id, hex, for spawned agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

/// Where to reach an agent held by another node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshPeer {
    /// Agent id, hex.
    pub agent_id: String,
    /// Display name.
    pub name: String,
    /// Node holding it (`hub` for the local app).
    pub node_id: String,
    /// Base URL of that node's API (`https://<machine>.boxd.sh`), or
    /// `None` for agents only the hub can reach (local agents).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Whether a message expects an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeshMailKind {
    /// A new message or task; its handler's final answer goes back.
    Request,
    /// An answer; never answered automatically.
    Reply,
}

/// One message between agents on different nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshMail {
    /// Unique per sending node; with `from_node`, the dedup key.
    pub id: u64,
    /// Node that sent it.
    pub from_node: String,
    /// Thread id, prefixed with the node that created the thread
    /// (`hub:t17`, `aura-re36-3f9c:t2`).
    pub thread: String,
    /// Thread subject.
    pub subject: String,
    /// Sender agent id, hex.
    pub from: String,
    /// Sender's display name.
    pub from_name: String,
    /// Recipient agent id, hex.
    pub to: String,
    /// Request or reply.
    pub kind: MeshMailKind,
    /// Body.
    pub text: String,
    /// When it was sent, Unix milliseconds.
    pub sent_ms: u64,
}

/// Work under way for an agent on a node, for the PROCESSES tab: a turn,
/// or an agent one of its agents spawned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshProcess {
    /// Id unique on the node.
    pub id: String,
    /// Agent it is for, hex: the agent taking the turn, or the spawned
    /// agent itself.
    pub agent_id: String,
    /// What it is: a turn's prompt, or a spawned agent's name and role.
    pub label: String,
    /// `running`, `done` or `failed`; a spawned agent can also be `idle`
    /// or `paused`.
    pub status: String,
    /// When it started, Unix milliseconds.
    pub started_ms: u64,
    /// [`MESH_PROCESS_AGENT`] for a spawned agent; a turn otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The chat run taking a turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// A spawned agent's parent, hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// A spawned agent's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// [`MeshProcess::kind`] of a spawned agent.
pub const MESH_PROCESS_AGENT: &str = "agent";

impl MeshProcess {
    /// Whether it is a spawned agent rather than a turn.
    #[must_use]
    pub fn is_agent(&self) -> bool {
        self.kind.as_deref() == Some(MESH_PROCESS_AGENT)
    }

    /// Whether it is still working.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.status == "running"
    }
}

/// One frame on a mesh socket, as JSON text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MeshFrame {
    /// First frame from the side that opened the socket.
    Hello {
        /// Sender's node id.
        node_id: String,
        /// [`MESH_PROTOCOL`].
        protocol: u32,
    },
    /// The agents a node holds, sent on connect and on change.
    Announce {
        /// Sender's node id.
        node_id: String,
        /// Its agents.
        agents: Vec<MeshAgent>,
    },
    /// From the hub: where every agent of the project can be reached.
    Directory {
        /// Every known agent outside the receiving node.
        peers: Vec<MeshPeer>,
    },
    /// A message for an agent on the receiving node (or, sent to the
    /// hub, a copy of node-to-node mail for the Inbox).
    Mail(MeshMail),
    /// The receiver took `id` from `from_node`.
    MailAck {
        /// Node that sent the mail.
        from_node: String,
        /// Mail id.
        id: u64,
        /// Whether it reached its agent.
        delivered: bool,
        /// Why not, when it didn't.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// An agent's record grew to `head_seq`.
    RecordCommitted {
        /// Agent id, hex.
        agent_id: String,
        /// New head.
        head_seq: u64,
    },
    /// The node's live processes.
    Processes {
        /// Every process it lists.
        processes: Vec<MeshProcess>,
    },
    /// A sample of the node's host.
    HostStats(HostStats),
    /// Keep-alive, answered with [`Self::Pong`] carrying the same `ms`.
    Ping {
        /// Sender's clock, Unix milliseconds.
        ms: u64,
    },
    /// Answer to [`Self::Ping`].
    Pong {
        /// The ping's `ms`, for round-trip time.
        ms: u64,
    },
    /// A frame type this build does not know; skipped.
    #[serde(other)]
    Unknown,
}

impl MeshFrame {
    /// The frame as JSON text.
    #[must_use]
    pub fn to_text(&self) -> String {
        // Strings, numbers and enums always serialize.
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Parse a JSON text frame.
    ///
    /// # Errors
    ///
    /// When the text is not a mesh frame.
    pub fn from_text(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail() -> MeshMail {
        MeshMail {
            id: 7,
            from_node: "aura-re36-3f9c".into(),
            thread: "hub:t17".into(),
            subject: "Parser review".into(),
            from: "aa".into(),
            from_name: "scout".into(),
            to: "bb".into(),
            kind: MeshMailKind::Request,
            text: "Found two issues".into(),
            sent_ms: 1,
        }
    }

    #[test]
    fn frames_round_trip() {
        for frame in [
            MeshFrame::Hello {
                node_id: "hub".into(),
                protocol: MESH_PROTOCOL,
            },
            MeshFrame::Mail(mail()),
            MeshFrame::MailAck {
                from_node: "hub".into(),
                id: 7,
                delivered: false,
                error: Some("unknown agent".into()),
            },
            MeshFrame::RecordCommitted {
                agent_id: "aa".into(),
                head_seq: 12,
            },
            MeshFrame::HostStats(HostStats::default()),
            MeshFrame::Ping { ms: 5 },
            MeshFrame::Processes {
                processes: vec![
                    MeshProcess {
                        id: "turn:r1:3".into(),
                        agent_id: "aa".into(),
                        label: "Fix the parser".into(),
                        status: "running".into(),
                        started_ms: 3,
                        kind: None,
                        run_id: Some("r1".into()),
                        parent: None,
                        name: None,
                    },
                    MeshProcess {
                        id: "agent:bb".into(),
                        agent_id: "bb".into(),
                        label: "scout (explorer)".into(),
                        status: "idle".into(),
                        started_ms: 4,
                        kind: Some(MESH_PROCESS_AGENT.into()),
                        run_id: None,
                        parent: Some("aa".into()),
                        name: Some("scout".into()),
                    },
                ],
            },
        ] {
            let text = frame.to_text();
            assert_eq!(MeshFrame::from_text(&text).expect("parse"), frame, "{text}");
        }
    }

    #[test]
    fn a_process_without_a_kind_is_a_turn() {
        let frame = MeshFrame::from_text(
            r#"{"type":"processes","processes":[{"id":"t","agent_id":"aa","label":"x","status":"running","started_ms":1}]}"#,
        )
        .expect("parse");
        let MeshFrame::Processes { processes } = frame else {
            panic!("not a processes frame: {frame:?}");
        };
        assert!(!processes[0].is_agent());
        assert!(processes[0].is_running());
        assert_eq!(processes[0].parent, None);
    }

    #[test]
    fn frames_are_tagged_by_type() {
        let text = MeshFrame::Mail(mail()).to_text();
        let json: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(json["type"], "mail");
        assert_eq!(json["kind"], "request");
    }

    #[test]
    fn unknown_frames_are_skipped() {
        let frame = MeshFrame::from_text(r#"{"type":"from_the_future","x":1}"#).expect("parse");
        assert_eq!(frame, MeshFrame::Unknown);
    }
}
