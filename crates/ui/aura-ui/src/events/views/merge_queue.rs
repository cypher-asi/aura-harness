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
    /// The tests that failed it, shown first
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failing_tests: Vec<String>,
    /// What the failure printed: a test's assertion or snapshot diff, or
    /// the conflicting files
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_excerpt: Option<String>,
    /// The PR it lands through
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<String>,
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
    /// Why the target is broken, when the queue found it failing its own
    /// checks: shown above every entry
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broken: Option<String>,
    /// The repo's page on github.com (`https://github.com/owner/repo`),
    /// when its remote is there: QUEUE links each branch under it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web: Option<String>,
}

impl MergeQueueView {
    /// The page of `branch` on github.com, under [`Self::web`]: `None`
    /// without a repo page, for an empty branch, or for one with a part
    /// that is no plain path segment.
    #[must_use]
    pub fn branch_url(&self, branch: &str) -> Option<String> {
        let web = self.web.as_deref()?.trim_end_matches('/');
        if !web.starts_with("https://github.com/") || branch.trim().is_empty() {
            return None;
        }
        let mut path = String::new();
        for segment in branch.trim().split('/') {
            if segment.is_empty() || segment == "." || segment == ".." {
                return None;
            }
            path.push('/');
            path.push_str(&encode_segment(segment));
        }
        Some(format!("{web}/tree{path}"))
    }
}

/// `segment` with every byte but an unreserved one percent-encoded.
fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(web: Option<&str>) -> MergeQueueView {
        MergeQueueView {
            web: web.map(str::to_string),
            ..MergeQueueView::default()
        }
    }

    #[test]
    fn a_branch_links_to_its_tree_on_github() {
        let queue = queue(Some("https://github.com/o/r"));
        assert_eq!(
            queue.branch_url("aura/x").as_deref(),
            Some("https://github.com/o/r/tree/aura/x")
        );
        assert_eq!(
            queue.branch_url("feat/a b#c").as_deref(),
            Some("https://github.com/o/r/tree/feat/a%20b%23c")
        );
    }

    #[test]
    fn no_link_without_a_github_page_or_a_plain_branch() {
        assert_eq!(queue(None).branch_url("aura/x"), None);
        let elsewhere = queue(Some("https://gitlab.com/o/r"));
        assert_eq!(elsewhere.branch_url("aura/x"), None);
        let github = queue(Some("https://github.com/o/r/"));
        for branch in ["", "a//b", "../x", "a/./b"] {
            assert_eq!(github.branch_url(branch), None, "{branch:?}");
        }
    }
}
