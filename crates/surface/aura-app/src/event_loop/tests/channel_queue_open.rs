//! A channel opened after its repo's merge queue has entries gets the
//! queue's current state at once, not only on the queue's next change.

use super::support::open_project;
use super::*;
use crate::event_loop::factory::service::QueueService;
use aura_exec_merge_queue::{
    Integrator, Limits, MergeEntry, MergeQueue, MergeRequest, Origin, QueuedPush, RepoPaths,
    RunOutcome,
};

/// Never asked to land anything: the queue is only written to here.
struct Unused;

#[async_trait]
impl Integrator for Unused {
    async fn integrate(&self, _entry: &MergeEntry) -> RunOutcome {
        RunOutcome::Failed("not in this test".to_string())
    }
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// A repo on main whose queue file holds one push, queued before any
/// channel on it opened.
async fn repo_with_a_queued_push(dir: &std::path::Path, work: &std::path::Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("README"), "x").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", "seed"]);
    let repo = RepoPaths::of(dir).await.unwrap().with_work_root(work);
    let queue = MergeQueue::open(repo, Arc::new(Unused), Limits::default())
        .await
        .unwrap();
    queue
        .enqueue(MergeRequest {
            push: QueuedPush {
                branch: "aura/early".to_string(),
                // No such commit: a processor fails it before any worktree.
                head: "0000000000000000000000000000000000000001".to_string(),
                remote: None,
            },
            origin: Origin::default(),
        })
        .await
        .unwrap();
}

/// The next `SetMergeQueue` for `channel_id` that `commands` gets.
async fn next_queue(
    commands: &mut mpsc::Receiver<UiCommand>,
    channel_id: &str,
) -> aura_ui::events::MergeQueueView {
    loop {
        let command = tokio::time::timeout(Duration::from_secs(10), commands.recv())
            .await
            .expect("the queue was sent")
            .expect("the service is running");
        if let UiCommand::SetMergeQueue {
            channel_id: id,
            queue,
        } = command
        {
            if id == channel_id {
                return queue;
            }
        }
    }
}

#[tokio::test]
async fn a_channel_opened_late_gets_its_queue_as_it_stands() {
    let (fixture, mut commands) = fixture();
    let mut b = borrowed(&fixture);
    let links = b.links.clone();
    let mut state = loop_state(&fixture, &mut b);
    let (dir, work) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    repo_with_a_queued_push(dir.path(), work.path()).await;
    let (project_id, _) = open_project(&fixture.hub, dir.path());
    let create = UiEvent::CreateChannel {
        slug: "late".to_string(),
        title: "Late".to_string(),
        description: String::new(),
        project_id: Some(project_id),
        placement: aura_ui::host::PlacementView::Local,
    };
    handle_ui_event(&mut state, create, &links).await;
    let channel_id = fixture
        .hub
        .channels
        .as_ref()
        .unwrap()
        .by_slug("late")
        .unwrap()
        .unwrap()
        .id;
    let _ = drain(&mut commands);

    let (tx, mut sent) = mpsc::channel(64);
    let quit = tokio_util::sync::CancellationToken::new();
    let service = QueueService::new(Arc::clone(&fixture.hub), tx, quit.clone());
    service.show(&channel_id).await;
    let queue = next_queue(&mut sent, &channel_id).await;
    assert_eq!(queue.target, "main");
    let branches: Vec<&str> = queue.entries.iter().map(|e| e.branch.as_str()).collect();
    assert_eq!(branches, ["aura/early"], "{queue:?}");

    // Opened again with the queue already running: sent again at once.
    service.show(&channel_id).await;
    let again = next_queue(&mut sent, &channel_id).await;
    assert_eq!(again.entries.len(), 1);
    quit.cancel();
}
