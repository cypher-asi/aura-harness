//! What the queue holds: one [`MergeEntry`] per push, and the
//! [`QueuedPush`] marker a push tool's result carries so the hub knows a
//! push happened.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Key of the push marker in a push tool's JSON result.
pub const MERGE_QUEUE_KEY: &str = "merge_queue";

/// Identifier of a queue entry, unique within its repo's queue (`q17`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntryId(String);

impl EntryId {
    /// The id of the `seq`th entry a queue ever took.
    #[must_use]
    pub fn from_seq(seq: u64) -> Self {
        Self(format!("q{seq}"))
    }

    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for EntryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where an entry is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeStatus {
    /// Waiting for its turn.
    Queued,
    /// Being rebased, checked and landed. At most one entry per queue.
    Running,
    /// On the target branch.
    Merged,
    /// Not merged; `detail` says why.
    Failed,
}

impl MergeStatus {
    /// Whether the entry is done, merged or not.
    #[must_use]
    pub const fn is_finished(self) -> bool {
        matches!(self, Self::Merged | Self::Failed)
    }

    /// Lower-case name, as the Queue tab shows it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Merged => "merged",
            Self::Failed => "failed",
        }
    }
}

/// A push as its tool reported it: the branch it updated and the commit
/// it left there. `remote` is `None` for a local branch of a repo with no
/// remote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedPush {
    /// Branch the push updated
    pub branch: String,
    /// Commit the branch points at after the push
    pub head: String,
    /// Remote pushed to; `None` when the branch is only local
    #[serde(default)]
    pub remote: Option<String>,
}

impl QueuedPush {
    /// The push marker in a tool's text result, if it has one: the result
    /// is a JSON object with a [`MERGE_QUEUE_KEY`] object in it.
    #[must_use]
    pub fn from_tool_output(output: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(output.trim()).ok()?;
        let marker = value.get(MERGE_QUEUE_KEY)?.clone();
        let push: Self = serde_json::from_value(marker).ok()?;
        (!push.branch.is_empty() && !push.head.is_empty()).then_some(push)
    }

    /// The marker as the JSON value a push tool puts under
    /// [`MERGE_QUEUE_KEY`].
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "branch": self.branch,
            "head": self.head,
            "remote": self.remote,
        })
    }
}

/// Who asked for a merge, and from where.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    /// Hex id of the agent that pushed
    #[serde(default)]
    pub agent: Option<String>,
    /// The agent's name or code, for the Queue tab
    #[serde(default)]
    pub agent_name: Option<String>,
    /// Channel the agent works in
    #[serde(default)]
    pub channel: Option<String>,
}

/// A request to merge a push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeRequest {
    /// The push to merge
    pub push: QueuedPush,
    /// Who pushed it
    pub origin: Origin,
}

/// One merge in a repo's queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeEntry {
    /// The entry's id in its queue
    pub id: EntryId,
    /// Branch to merge
    pub branch: String,
    /// Commit of `branch` to merge
    pub head: String,
    /// Remote `branch` was pushed to; `None` for a local branch
    #[serde(default)]
    pub remote: Option<String>,
    /// Who pushed it
    #[serde(default)]
    pub origin: Origin,
    /// Where the entry is in its life
    pub status: MergeStatus,
    /// When it was queued (a re-push of a queued branch keeps its place)
    pub enqueued_at: DateTime<Utc>,
    /// When its branch was last pushed, when after `enqueued_at`
    #[serde(default)]
    pub pushed_at: Option<DateTime<Utc>>,
    /// When it last started running
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    /// When it was merged or failed
    #[serde(default)]
    pub finished_at: Option<DateTime<Utc>>,
    /// When anything about it last changed: queued, pushed again,
    /// started, put back after a crash or finished. Waiting is measured
    /// from here ([`crate::QueueState::expire`]); none in files from
    /// before it was kept
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    /// How many times it started running
    #[serde(default)]
    pub attempts: u32,
    /// What happened: the failure's reason, or how it merged
    #[serde(default)]
    pub detail: Option<String>,
    /// Commit the target branch points at after the merge
    #[serde(default)]
    pub merged: Option<String>,
}

impl MergeEntry {
    /// When its wait started: its last change, else its last push, else
    /// when it was queued.
    #[must_use]
    pub fn waiting_since(&self) -> DateTime<Utc> {
        self.updated_at
            .or(self.pushed_at)
            .unwrap_or(self.enqueued_at)
    }

    /// Short form of `head`, for messages.
    #[must_use]
    pub fn short_head(&self) -> &str {
        short_sha(&self.head)
    }
}

/// The first 8 characters of `sha` (all of it when shorter).
#[must_use]
pub fn short_sha(sha: &str) -> &str {
    sha.get(..8).unwrap_or(sha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_round_trips_through_a_tool_result() {
        let push = QueuedPush {
            branch: "aura/fix-1".to_string(),
            head: "abc123".to_string(),
            remote: Some("origin".to_string()),
        };
        let output = serde_json::json!({ "pushed": true, MERGE_QUEUE_KEY: push.to_json() });
        assert_eq!(
            QueuedPush::from_tool_output(&output.to_string()),
            Some(push)
        );
    }

    #[test]
    fn results_without_a_marker_are_not_pushes() {
        for output in [
            "",
            "not json",
            r#"{"pushed":true}"#,
            r#"{"merge_queue":{"branch":"","head":"x"}}"#,
            r#"{"merge_queue":"text"}"#,
        ] {
            assert_eq!(QueuedPush::from_tool_output(output), None, "{output}");
        }
    }

    #[test]
    fn entry_round_trips_through_json() {
        let entry = MergeEntry {
            id: EntryId::from_seq(3),
            branch: "aura/x".to_string(),
            head: "0123456789abcdef".to_string(),
            remote: None,
            origin: Origin::default(),
            status: MergeStatus::Failed,
            enqueued_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap_or_default(),
            pushed_at: None,
            started_at: None,
            finished_at: None,
            updated_at: None,
            attempts: 2,
            detail: Some("tests failed".to_string()),
            merged: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert_eq!(serde_json::from_str::<MergeEntry>(&json).unwrap(), entry);
        assert_eq!(entry.short_head(), "01234567");
    }
}
