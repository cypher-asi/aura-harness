//! A repo's merge queue as the QUEUE tab shows it.

use aura_exec_merge_queue::{short_sha, MergeEntry, MergeStatus};
use aura_ui::events::{MergeEntryView, MergeQueueView, MergeStateView};

/// The view of `entries` (oldest first) of the queue of the repo at
/// `repo`, merging into `target`, with `older` finished merges not in
/// them: newest first, each open entry with the work it overlaps
/// ([`super::overlap::entry_overlaps`]).
pub(crate) fn queue_view(
    target: &str,
    repo: &str,
    entries: &[MergeEntry],
    older: u64,
) -> MergeQueueView {
    let mut overlaps = super::overlap::entry_overlaps(entries, repo);
    let entries = entries
        .iter()
        .rev()
        .map(|entry| {
            let mut view = entry_view(entry);
            view.overlaps = overlaps.remove(&entry.id).unwrap_or_default();
            view
        })
        .collect();
    MergeQueueView {
        target: target.to_string(),
        repo: repo.to_string(),
        entries,
        older,
    }
}

fn entry_view(entry: &MergeEntry) -> MergeEntryView {
    MergeEntryView {
        id: entry.id.to_string(),
        branch: entry.branch.clone(),
        head: entry.short_head().to_string(),
        agent: entry.origin.agent_name.clone(),
        state: state_view(entry.status),
        queued_at: entry.enqueued_at.to_rfc3339(),
        finished_at: entry.finished_at.map(|at| at.to_rfc3339()),
        attempts: entry.attempts,
        detail: entry.detail.clone(),
        merged: entry
            .merged
            .as_deref()
            .map(|sha| short_sha(sha).to_string()),
        overlaps: Vec::new(),
    }
}

const fn state_view(status: MergeStatus) -> MergeStateView {
    match status {
        MergeStatus::Queued => MergeStateView::Queued,
        MergeStatus::Running => MergeStateView::Running,
        MergeStatus::Merged => MergeStateView::Merged,
        MergeStatus::Failed => MergeStateView::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_exec_merge_queue::{MergeRequest, Origin, QueueState, QueuedPush};

    #[test]
    fn the_view_lists_the_newest_merge_first_with_its_details() {
        let mut state = QueueState::default();
        for branch in ["aura/old", "aura/new"] {
            state.enqueue(
                MergeRequest {
                    push: QueuedPush {
                        branch: branch.to_string(),
                        head: "0123456789abcdef".to_string(),
                        remote: Some("origin".to_string()),
                    },
                    origin: Origin {
                        agent_name: Some("RUS1".to_string()),
                        ..Origin::default()
                    },
                },
                chrono::Utc::now(),
            );
        }
        let view = queue_view("main", "/repo", &state.entries, 7);
        assert_eq!(view.older, 7);
        assert_eq!(view.target, "main");
        assert_eq!(view.entries[0].branch, "aura/new");
        assert_eq!(view.entries[1].branch, "aura/old");
        assert_eq!(view.entries[0].head, "01234567");
        assert_eq!(view.entries[0].agent.as_deref(), Some("RUS1"));
        assert_eq!(view.entries[0].state, MergeStateView::Queued);
    }

    #[test]
    fn open_entries_that_change_the_same_files_show_each_other() {
        let mut state = QueueState::default();
        for (branch, agent) in [("aura/a", "RUS1"), ("aura/b", "RUS2")] {
            let head = format!("{branch}-head");
            let id = state
                .enqueue(
                    MergeRequest {
                        push: QueuedPush {
                            branch: branch.to_string(),
                            head: head.clone(),
                            remote: None,
                        },
                        origin: Origin {
                            agent_name: Some(agent.to_string()),
                            ..Origin::default()
                        },
                    },
                    chrono::Utc::now(),
                )
                .id()
                .clone();
            state.record_files(&id, &head, vec!["src/shared.rs".to_string()]);
        }
        let view = queue_view("main", "/repo", &state.entries, 0);
        assert_eq!(view.entries[0].overlaps, ["q1 aura/a (RUS1): src/shared.rs"]);
        assert_eq!(view.entries[1].overlaps, ["q2 aura/b (RUS2): src/shared.rs"]);
    }
}
