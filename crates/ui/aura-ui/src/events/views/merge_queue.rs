//! A channel's merge queue, as its QUEUE tab shows it.

use serde::{Deserialize, Serialize};

/// Where a merge is in its life.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeStateView {
    /// Waiting for its turn
    #[default]
    Queued,
    /// Being rebased, checked and landed
    Running,
    /// On the target branch
    Merged,
    /// Not merged; the entry's detail says why
    Failed,
}

impl MergeStateView {
    /// Upper-case name, as the QUEUE tab shows it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Queued => "QUEUED",
            Self::Running => "RUNNING",
            Self::Merged => "MERGED",
            Self::Failed => "FAILED",
        }
    }
}

/// One merge in a repo's queue.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeEntryView {
    /// Entry id in its queue (`q17`)
    pub id: String,
    /// Branch it merges
    pub branch: String,
    /// Short sha of the pushed commit
    pub head: String,
    /// Agent that pushed it: its code or name, when known
    #[serde(default)]
    pub agent: Option<String>,
    /// Where it is in its life
    pub state: MergeStateView,
    /// When it was queued, RFC 3339
    pub queued_at: String,
    /// When it was merged or failed, RFC 3339
    #[serde(default)]
    pub finished_at: Option<String>,
    /// How many times it started running
    #[serde(default)]
    pub attempts: u32,
    /// What happened: why it failed, or how it merged
    #[serde(default)]
    pub detail: Option<String>,
    /// Short sha the target branch moved to
    #[serde(default)]
    pub merged: Option<String>,
    /// Other work in flight that changes the same files, one line each:
    /// `q12 aura/x (RUS1): src/a.rs, src/b.rs`
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlaps: Vec<String>,
}

/// A repo's merge queue.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeQueueView {
    /// Branch merges land on
    pub target: String,
    /// Folder of the repo
    pub repo: String,
    /// The entries shown, newest first: every waiting and running one,
    /// and the newest finished ones
    pub entries: Vec<MergeEntryView>,
    /// Finished merges older than those shown, which
    /// `UiEvent::LoadOlderMerges` asks for a page at a time
    #[serde(default)]
    pub older: u64,
}
