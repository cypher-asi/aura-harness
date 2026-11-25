//! Bodies of the calls the hub makes to a cloud node to move an agent's
//! single writer, mirror its record and pull its work back.
//!
//! | Call | Body | Answer |
//! |---|---|---|
//! | `POST /v1/sync/import` | [`SyncImport`] | [`SyncImported`] |
//! | `POST /v1/sync/seal/:agent_id` | none | [`SealResponse`] |
//! | `GET /v1/sync/traces?since=` | none | [`TracePage`] |
//! | `GET /v1/workspace/patch?base=` | none | [`WorkspacePatch`] |
//! | `aura node claim --stdin` (over `Exec`) | [`NodeClaim`] | [`NodeClaimed`] |
//!
//! Record pages themselves come from the existing
//! `GET /agents/:id/record`; [`RecordPage`] is its answer's shape.

use aura_core_types::RecordEntry;
use serde::{Deserialize, Serialize};

/// Seed a node with an agent's record so it can take over as the
/// agent's writer. The node appends the entries verbatim after checking
/// that they continue its own head (none, for a new agent) and that the
/// hash chain holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncImport {
    /// Agent id, hex.
    pub agent_id: String,
    /// Display name, for the node's host.
    pub name: String,
    /// Entries from the node's head + 1, in order.
    pub entries: Vec<RecordEntry>,
    /// Whether this is the last page: the node may then start the
    /// agent's turns.
    pub done: bool,
}

/// Answer to [`SyncImport`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncImported {
    /// The node's head for the agent after the import.
    pub head_seq: u64,
}

/// Answer to a seal: the node will not append to the agent again, and
/// this is its final head.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealResponse {
    /// Final head.
    pub head_seq: u64,
}

/// One page of an agent's record, as `GET /agents/:id/record` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordPage {
    /// Entries, in seq order.
    pub entries: Vec<RecordEntry>,
    /// The agent's head on the node when the page was read.
    #[serde(default)]
    pub head_seq: u64,
}

/// Trace runs written on a node since a point, opaque to the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TracePage {
    /// Serialized trace runs, oldest first.
    pub runs: Vec<serde_json::Value>,
    /// Pass as `since` for the next page.
    pub next_since: u64,
}

/// The agent's work as a patch against the snapshot's commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacePatch {
    /// Commit the patch applies to.
    pub base_commit: String,
    /// `git diff --binary` of tracked changes plus untracked files.
    pub patch: String,
    /// Paths it touches.
    pub files: Vec<String>,
}

/// How a cloud node calls one plan provider for its owner. The bearer is
/// a boxd host-bound placeholder (`bxds_...`) the node reads from its own
/// environment variable `env`; boxd's egress proxy puts the owner's real
/// token in on the way to the provider. Never a real token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanTemplate {
    /// Provider id (`claude`, `chatgpt`, `openrouter`).
    pub provider: String,
    /// Environment variable holding the placeholder (the boxd secret's
    /// name).
    pub env: String,
    /// Extra headers the provider requires.
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// Text the provider requires at the start of the system prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_preamble: Option<String>,
    /// The placeholder, filled in on the machine by `aura node claim`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

impl std::fmt::Debug for PlanTemplate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlanTemplate")
            .field("provider", &self.provider)
            .field("env", &self.env)
            .field("headers", &self.headers.len())
            .field("placeholder", &self.placeholder.is_some())
            .finish()
    }
}

/// Prefix of every boxd placeholder for a host-bound secret.
pub const PLACEHOLDER_PREFIX: &str = "bxds_";

/// What the hub tells a node when it takes it over (`claim`, on a
/// machine made from a snapshot) or after a fork (`rebind`). Sent over
/// boxd's authenticated `Exec`, never over the public domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeClaim {
    /// The node's id from now on (its machine name).
    pub node_id: String,
    /// Bearer token the hub calls the node with.
    pub node_token: String,
    /// Token nodes of the project present to each other.
    pub mesh_token: String,
    /// Agents on the node to seal (a fork's copies of its parent's
    /// agents), hex ids.
    #[serde(default)]
    pub seal: Vec<String>,
    /// Owner tag of the user the machine belongs to; never a user id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// How the node calls its owner's plans; placeholders are filled on
    /// the machine.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plans: Vec<PlanTemplate>,
}

impl std::fmt::Display for NodeClaim {
    /// Never shows the tokens.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let plans: Vec<&str> = self.plans.iter().map(|p| p.provider.as_str()).collect();
        write!(
            f,
            "NodeClaim({}, owner {}, plans [{}], seal {})",
            self.node_id,
            self.owner.as_deref().unwrap_or("-"),
            plans.join(","),
            self.seal.len()
        )
    }
}

/// Answer to [`NodeClaim`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeClaimed {
    /// The node's id now.
    pub node_id: String,
    /// Port the node's API listens on.
    pub port: u16,
    /// Agents it sealed.
    pub sealed: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_display_hides_tokens() {
        let claim = NodeClaim {
            node_id: "aura-re36-3f9c".into(),
            node_token: "node-secret-123".into(),
            mesh_token: "mesh-secret-456".into(),
            seal: vec!["aa".into()],
            owner: Some("a11ce000".into()),
            plans: vec![PlanTemplate {
                provider: "claude".into(),
                env: "AURA_A11CE000_INST01_CLAUDE".into(),
                headers: vec![("anthropic-beta".into(), "oauth-2025-04-20".into())],
                system_preamble: None,
                placeholder: Some("bxds_secret-placeholder".into()),
            }],
        };
        let text = claim.to_string();
        assert!(!text.contains("secret"), "{text}");
        assert!(
            text.contains("owner a11ce000") && text.contains("plans [claude]"),
            "{text}"
        );
        let debug = format!("{:?}", claim.plans[0]);
        assert!(!debug.contains("bxds_"), "{debug}");
        let json = serde_json::to_string(&claim).expect("json");
        assert_eq!(
            serde_json::from_str::<NodeClaim>(&json).expect("parse"),
            claim
        );
    }
}
