//! A repo's merge queue as the QUEUE tab shows it.

use aura_exec_merge_queue::{short_sha, HistoryPage, MergeEntry, MergeStatus};
use aura_ui::events::{MergeEntryView, MergeQueueView, MergeStateView};

/// The queue of the repo at `repo`, merging into `target`, as the QUEUE
/// tab first shows it: its waiting and running entries of `live` (the
/// queue file's, oldest first), newest first, then the newest finished
/// merges, from the history's newest `page`. Without a page (the history
/// couldn't be read) the finished entries the queue file keeps stand in.
pub(crate) fn queue_view(
    target: &str,
    repo: &str,
    live: &[MergeEntry],
    page: Option<&HistoryPage>,
) -> MergeQueueView {
    let waiting = live.iter().rev().filter(|e| !e.status.is_finished());
    let (finished, older): (Vec<&MergeEntry>, _) = match page {
        Some(page) => (page.entries.iter().collect(), page.older),
        None => {
            let kept = live.iter().rev().filter(|e| e.status.is_finished());
            (kept.collect(), None)
        }
    };
    MergeQueueView {
        target: target.to_string(),
        repo: repo.to_string(),
        entries: waiting.chain(finished).map(entry_view).collect(),
        older,
        page_before: None,
    }
}

/// `page`, the finished merges before cursor `before`, as a view whose
/// entries go after those the QUEUE tab shows.
pub(crate) fn history_view(
    target: &str,
    repo: &str,
    page: &HistoryPage,
    before: u64,
) -> MergeQueueView {
    MergeQueueView {
        target: target.to_string(),
        repo: repo.to_string(),
        entries: page.entries.iter().map(entry_view).collect(),
        older: page.older,
        page_before: Some(before),
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
        let view = queue_view("main", "/repo", &state.entries, None);
        assert_eq!(view.target, "main");
        assert_eq!(view.entries[0].branch, "aura/new");
        assert_eq!(view.entries[1].branch, "aura/old");
        assert_eq!(view.entries[0].head, "01234567");
        assert_eq!(view.entries[0].agent.as_deref(), Some("RUS1"));
        assert_eq!(view.entries[0].state, MergeStateView::Queued);
    }

    #[test]
    fn finished_merges_come_from_the_history_page_with_its_cursor() {
        let mut state = QueueState::default();
        let now = chrono::Utc::now();
        let push = |branch: &str| MergeRequest {
            push: QueuedPush {
                branch: branch.to_string(),
                head: "h".to_string(),
                remote: None,
            },
            origin: Origin::default(),
        };
        state.enqueue(push("aura/done"), now);
        state.claim_next(now);
        let id = state.entries[0].id.clone();
        state.finish(
            &id,
            aura_exec_merge_queue::RunOutcome::Failed("no".into()),
            now,
        );
        state.enqueue(push("aura/wait"), now);
        let page = HistoryPage {
            entries: vec![state.entries[0].clone()],
            older: Some(42),
        };
        let view = queue_view("main", "/repo", &state.entries, Some(&page));
        let branches: Vec<&str> = view.entries.iter().map(|e| e.branch.as_str()).collect();
        assert_eq!(branches, ["aura/wait", "aura/done"]);
        assert_eq!(view.older, Some(42));

        let older = history_view("main", "/repo", &page, 900);
        assert_eq!(older.page_before, Some(900));
        assert_eq!(older.entries.len(), 1);
        assert_eq!(older.older, Some(42));
    }
}
