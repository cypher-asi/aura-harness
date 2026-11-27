use super::*;
use crate::entry::{MergeStatus, Origin, QueuedPush};
use async_trait::async_trait;
use std::path::Path;
use std::sync::Mutex as StdMutex;

/// Lands every entry, failing branches whose name starts with `bad`, and
/// remembers the order it saw them in.
#[derive(Default)]
struct Recorder {
    seen: StdMutex<Vec<String>>,
}

#[async_trait]
impl Integrator for Recorder {
    async fn integrate(&self, entry: &MergeEntry) -> RunOutcome {
        self.seen
            .lock()
            .map(|mut seen| seen.push(entry.branch.clone()))
            .ok();
        if entry.branch.starts_with("bad") {
            RunOutcome::Failed("tests failed".to_string())
        } else {
            RunOutcome::Merged {
                merged: format!("m-{}", entry.branch),
                note: "fast-forward".to_string(),
            }
        }
    }
}

/// Never finishes, like a merge cut off by a crash.
struct Hangs;

#[async_trait]
impl Integrator for Hangs {
    async fn integrate(&self, _entry: &MergeEntry) -> RunOutcome {
        std::future::pending().await
    }
}

fn repo(dir: &Path) -> RepoPaths {
    RepoPaths {
        top: dir.to_path_buf(),
        common: dir.join(".git"),
    }
}

fn request(branch: &str) -> MergeRequest {
    MergeRequest {
        push: QueuedPush {
            branch: branch.to_string(),
            head: format!("h-{branch}"),
            remote: Some("origin".to_string()),
        },
        origin: Origin {
            agent: Some("ab12".to_string()),
            agent_name: Some("RUS1".to_string()),
            channel: Some("c1".to_string()),
        },
    }
}

async fn open(dir: &Path, integrator: Arc<dyn Integrator>) -> (MergeQueue, Vec<MergeEntry>) {
    MergeQueue::open(repo(dir), integrator, Limits::default())
        .await
        .unwrap()
}

#[tokio::test]
async fn entries_are_merged_in_the_order_they_were_pushed() {
    let dir = tempfile::tempdir().unwrap();
    let recorder = Arc::new(Recorder::default());
    let (queue, _) = open(dir.path(), recorder.clone()).await;
    for branch in ["a", "bad-b", "c"] {
        queue.enqueue(request(branch)).await.unwrap();
    }
    while queue.process_next().await.unwrap().is_some() {}
    assert_eq!(*recorder.seen.lock().unwrap(), ["a", "bad-b", "c"]);
    let statuses: Vec<MergeStatus> = queue.entries().await.iter().map(|e| e.status).collect();
    assert_eq!(
        statuses,
        [
            MergeStatus::Merged,
            MergeStatus::Failed,
            MergeStatus::Merged
        ]
    );
}

#[tokio::test]
async fn a_failure_is_announced_with_its_reason_and_owner() {
    let dir = tempfile::tempdir().unwrap();
    let (queue, _) = open(dir.path(), Arc::new(Recorder::default())).await;
    let mut events = queue.subscribe();
    queue.enqueue(request("bad")).await.unwrap();
    queue.process_next().await.unwrap();
    let mut finished = None;
    while let Ok(event) = events.try_recv() {
        if let QueueEvent::Finished(entry) = event {
            finished = Some(*entry);
        }
    }
    let finished = finished.unwrap();
    assert_eq!(finished.status, MergeStatus::Failed);
    assert_eq!(finished.detail.as_deref(), Some("tests failed"));
    assert_eq!(finished.origin.agent_name.as_deref(), Some("RUS1"));
}

#[tokio::test]
async fn the_queue_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    {
        let (queue, _) = open(dir.path(), Arc::new(Recorder::default())).await;
        queue.enqueue(request("a")).await.unwrap();
        queue.enqueue(request("b")).await.unwrap();
    }
    let recorder = Arc::new(Recorder::default());
    let (queue, failed) = open(dir.path(), recorder.clone()).await;
    assert!(failed.is_empty());
    assert_eq!(queue.entries().await.len(), 2);
    while queue.process_next().await.unwrap().is_some() {}
    assert_eq!(*recorder.seen.lock().unwrap(), ["a", "b"]);
}

#[tokio::test]
async fn a_merge_cut_off_by_a_crash_runs_again_in_its_place() {
    let dir = tempfile::tempdir().unwrap();
    {
        let (queue, _) = open(dir.path(), Arc::new(Hangs)).await;
        queue.enqueue(request("a")).await.unwrap();
        queue.enqueue(request("b")).await.unwrap();
        let cut = tokio::time::timeout(Duration::from_millis(50), queue.process_next()).await;
        assert!(cut.is_err(), "the merge was cut off");
        assert_eq!(queue.entries().await[0].status, MergeStatus::Running);
    }
    let recorder = Arc::new(Recorder::default());
    let (queue, _) = open(dir.path(), recorder.clone()).await;
    let entries = queue.entries().await;
    assert_eq!(entries[0].status, MergeStatus::Queued);
    assert_eq!(entries[0].attempts, 1);
    while queue.process_next().await.unwrap().is_some() {}
    assert_eq!(*recorder.seen.lock().unwrap(), ["a", "b"]);
}

#[tokio::test]
async fn an_entry_that_crashes_every_time_fails_instead_of_looping() {
    let dir = tempfile::tempdir().unwrap();
    for _ in 0..3 {
        let (queue, _) = open(dir.path(), Arc::new(Hangs)).await;
        if queue.entries().await.is_empty() {
            queue.enqueue(request("a")).await.unwrap();
        }
        let _ = tokio::time::timeout(Duration::from_millis(20), queue.process_next()).await;
    }
    let (queue, failed) = open(dir.path(), Arc::new(Recorder::default())).await;
    assert_eq!(failed.len(), 1);
    assert_eq!(queue.entries().await[0].status, MergeStatus::Failed);
}

#[tokio::test]
async fn an_entry_running_too_long_fails() {
    let dir = tempfile::tempdir().unwrap();
    let limits = Limits {
        entry_timeout: Duration::from_millis(20),
        ..Limits::default()
    };
    let (queue, _) = MergeQueue::open(repo(dir.path()), Arc::new(Hangs), limits)
        .await
        .unwrap();
    queue.enqueue(request("a")).await.unwrap();
    let done = queue.process_next().await.unwrap().unwrap();
    assert_eq!(done.status, MergeStatus::Failed);
    assert!(done.detail.unwrap_or_default().contains("took longer"));
}

#[tokio::test]
async fn the_processor_merges_pushes_as_they_arrive_until_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let recorder = Arc::new(Recorder::default());
    let (queue, _) = open(dir.path(), recorder.clone()).await;
    let mut events = queue.subscribe();
    let cancel = CancellationToken::new();
    let processor = tokio::spawn({
        let queue = queue.clone();
        let cancel = cancel.clone();
        async move { queue.run(cancel).await }
    });
    queue.enqueue(request("a")).await.unwrap();
    queue.enqueue(request("b")).await.unwrap();
    let mut finished = Vec::new();
    while finished.len() < 2 {
        if let Ok(QueueEvent::Finished(entry)) = events.recv().await {
            finished.push(entry.branch);
        }
    }
    assert_eq!(finished, ["a", "b"]);
    cancel.cancel();
    processor.await.unwrap();
    assert!(
        !repo(dir.path())
            .queue_dir()
            .join(crate::store::LOCK_FILE)
            .exists(),
        "the lock goes with the processor"
    );
}
