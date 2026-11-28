//! A channel's QUEUE tab: what the session sends it when the channel
//! opens, with the finished merges from the repo's history.

use super::*;
use aura_exec_merge_queue::store::QueueStore;
use aura_exec_merge_queue::RunOutcome;
use aura_exec_merge_queue::{EntryId, MergeRequest, Origin, QueueState, QueuedPush, RepoPaths};
use aura_ui::events::{MergeQueueView, MergeStateView};

/// A git repo in a fresh folder.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(status.status.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "start",
    ]);
    dir
}

/// A queue file in `folder`'s repo from before the history: `merged`
/// finished merges. (Nothing waits: the processor would start it.)
async fn old_queue_file(folder: &std::path::Path, merged: u64) {
    let repo = RepoPaths::of(folder).await.unwrap();
    let mut state = QueueState::default();
    let now = chrono::Utc::now();
    let push = |branch: String| MergeRequest {
        push: QueuedPush {
            branch,
            head: "0123456789abcdef".to_string(),
            remote: None,
        },
        origin: Origin::default(),
    };
    for n in 1..=merged {
        state.enqueue(push(format!("aura/done-{n}")), now);
        state.claim_next(now);
        let outcome = RunOutcome::Merged {
            merged: "fedcba9876543210".to_string(),
            note: "merged".to_string(),
        };
        state.finish(&EntryId::from_seq(n), outcome, now);
    }
    QueueStore::new(repo.queue_dir()).save(&state).unwrap();
}

/// The QUEUE views sent to channel `channel_id`, waiting up to a few
/// seconds for the first.
async fn views_to(rx: &mut mpsc::Receiver<UiCommand>, channel_id: &str) -> Vec<MergeQueueView> {
    let first = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match rx.recv().await {
                Some(UiCommand::SetMergeQueue {
                    channel_id: to,
                    queue,
                }) if to == channel_id => {
                    return Some(queue);
                }
                Some(_) => {}
                None => return None,
            }
        }
    })
    .await
    .ok()
    .flatten();
    first.into_iter().collect()
}

#[tokio::test]
async fn a_channel_opened_later_gets_its_queue_at_once_with_every_merge_kept() {
    let (fixture, _commands) = fixture();
    let folder = repo();
    old_queue_file(folder.path(), 3).await;
    let (tx, mut rx) = mpsc::channel(64);
    let service =
        crate::event_loop::factory::service::QueueService::for_tests(Arc::clone(&fixture.hub), tx);

    // No change happens in the queue: opening the channel is what sends it.
    service.show_folder("late", folder.path()).await;
    let views = views_to(&mut rx, "late").await;
    assert_eq!(views.len(), 1, "sent on open");
    let view = &views[0];
    let branches: Vec<&str> = view.entries.iter().map(|e| e.branch.as_str()).collect();
    assert_eq!(
        branches,
        ["aura/done-3", "aura/done-2", "aura/done-1"],
        "every merge, newest first"
    );
    assert!(view
        .entries
        .iter()
        .all(|e| e.state == MergeStateView::Merged));
    assert_eq!(view.older, None, "one page holds them all");
    assert_eq!(view.page_before, None);

    // The old queue file's merges now live in the history too.
    let repo = RepoPaths::of(folder.path()).await.unwrap();
    let page = aura_exec_merge_queue::history::read_page(&repo.queue_dir(), None, 10).unwrap();
    assert_eq!(page.entries.len(), 3);
}

#[tokio::test]
async fn a_channel_without_a_queue_gets_nothing() {
    let (fixture, _commands) = fixture();
    let folder = repo();
    let (tx, mut rx) = mpsc::channel(64);
    let service =
        crate::event_loop::factory::service::QueueService::for_tests(Arc::clone(&fixture.hub), tx);
    service.show_folder("c", folder.path()).await;
    assert!(rx.try_recv().is_err());
}
