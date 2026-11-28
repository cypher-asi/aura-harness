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
    RepoPaths::new(dir.to_path_buf(), dir.join(".git"), &dir.join("work"))
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

async fn open(dir: &Path, integrator: Arc<dyn Integrator>) -> MergeQueue {
    MergeQueue::open(repo(dir), integrator, Limits::default())
        .await
        .unwrap()
}

/// Whether `queue`'s processor lock can be taken within a few seconds.
/// It is free the moment its processor drops it, but a child process
/// another test starts holds a copy of every open file until it execs,
/// the lock's too, so under load it can look held for a moment after.
async fn lock_frees(queue: &MergeQueue) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if queue.processor().await.unwrap().is_some() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// `queue`'s processor, recovered; panics when another holds it.
async fn processor(queue: &MergeQueue) -> (Processor, Vec<MergeEntry>) {
    let processor = queue.processor().await.unwrap().expect("the lock is free");
    let failed = processor.recover().await.unwrap();
    (processor, failed)
}

#[tokio::test]
async fn entries_are_merged_in_the_order_they_were_pushed() {
    let dir = tempfile::tempdir().unwrap();
    let recorder = Arc::new(Recorder::default());
    let queue = open(dir.path(), recorder.clone()).await;
    for branch in ["a", "bad-b", "c"] {
        queue.enqueue(request(branch)).await.unwrap();
    }
    let (processor, _) = processor(&queue).await;
    while processor.process_next().await.unwrap().is_some() {}
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
    let queue = open(dir.path(), Arc::new(Recorder::default())).await;
    let mut events = queue.subscribe();
    queue.enqueue(request("bad")).await.unwrap();
    processor(&queue).await.0.process_next().await.unwrap();
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
        let queue = open(dir.path(), Arc::new(Recorder::default())).await;
        queue.enqueue(request("a")).await.unwrap();
        queue.enqueue(request("b")).await.unwrap();
    }
    let recorder = Arc::new(Recorder::default());
    let queue = open(dir.path(), recorder.clone()).await;
    let (processor, failed) = processor(&queue).await;
    assert!(failed.is_empty());
    assert_eq!(queue.entries().await.len(), 2);
    while processor.process_next().await.unwrap().is_some() {}
    assert_eq!(*recorder.seen.lock().unwrap(), ["a", "b"]);
}

#[tokio::test]
async fn a_merge_cut_off_by_a_crash_runs_again_in_its_place() {
    let dir = tempfile::tempdir().unwrap();
    {
        let queue = open(dir.path(), Arc::new(Hangs)).await;
        queue.enqueue(request("a")).await.unwrap();
        queue.enqueue(request("b")).await.unwrap();
        let (processor, _) = processor(&queue).await;
        let cut = tokio::time::timeout(Duration::from_millis(50), processor.process_next()).await;
        assert!(cut.is_err(), "the merge was cut off");
        assert_eq!(queue.entries().await[0].status, MergeStatus::Running);
    }
    let recorder = Arc::new(Recorder::default());
    let queue = open(dir.path(), recorder.clone()).await;
    assert_eq!(
        queue.entries().await[0].status,
        MergeStatus::Running,
        "opening alone recovers nothing"
    );
    let (processor, _) = processor(&queue).await;
    let entries = queue.entries().await;
    assert_eq!(entries[0].status, MergeStatus::Queued);
    assert_eq!(entries[0].attempts, 1);
    while processor.process_next().await.unwrap().is_some() {}
    assert_eq!(*recorder.seen.lock().unwrap(), ["a", "b"]);
}

#[tokio::test]
async fn an_entry_that_crashes_every_time_fails_instead_of_looping() {
    let dir = tempfile::tempdir().unwrap();
    for _ in 0..3 {
        let queue = open(dir.path(), Arc::new(Hangs)).await;
        if queue.entries().await.is_empty() {
            queue.enqueue(request("a")).await.unwrap();
        }
        let (processor, _) = processor(&queue).await;
        let cut = tokio::time::timeout(Duration::from_millis(20), processor.process_next()).await;
        assert!(cut.is_err());
    }
    let queue = open(dir.path(), Arc::new(Recorder::default())).await;
    let (_processor, failed) = processor(&queue).await;
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
    let queue = MergeQueue::open(repo(dir.path()), Arc::new(Hangs), limits)
        .await
        .unwrap();
    queue.enqueue(request("a")).await.unwrap();
    let done = processor(&queue)
        .await
        .0
        .process_next()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(done.status, MergeStatus::Failed);
    assert!(done.detail.unwrap_or_default().contains("took longer"));
}

#[tokio::test]
async fn the_processor_merges_pushes_as_they_arrive_until_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let recorder = Arc::new(Recorder::default());
    let queue = open(dir.path(), recorder.clone()).await;
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
    assert!(lock_frees(&queue).await, "the lock goes with the processor");
}

/// Never finishes, but has landed: the merge was pushed when time ran out.
struct LandedThenHung;

#[async_trait]
impl Integrator for LandedThenHung {
    async fn integrate(&self, _entry: &MergeEntry) -> RunOutcome {
        std::future::pending().await
    }

    async fn landed(&self, _entry: &MergeEntry) -> Option<String> {
        Some("tip".to_string())
    }
}

#[tokio::test]
async fn an_entry_that_landed_before_its_time_ran_out_is_merged() {
    let dir = tempfile::tempdir().unwrap();
    let limits = Limits {
        entry_timeout: Duration::from_millis(20),
        ..Limits::default()
    };
    let queue = MergeQueue::open(repo(dir.path()), Arc::new(LandedThenHung), limits)
        .await
        .unwrap();
    queue.enqueue(request("a")).await.unwrap();
    let done = processor(&queue)
        .await
        .0
        .process_next()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(done.status, MergeStatus::Merged);
    assert_eq!(done.merged.as_deref(), Some("tip"));
}

#[tokio::test]
async fn two_queues_on_one_repo_share_one_file_and_one_processor() {
    let dir = tempfile::tempdir().unwrap();
    let recorder = Arc::new(Recorder::default());
    let first = open(dir.path(), recorder.clone()).await;
    let second = open(dir.path(), recorder.clone()).await;
    first.enqueue(request("a")).await.unwrap();
    second.enqueue(request("b")).await.unwrap();
    first.enqueue(request("c")).await.unwrap();
    let branches = |entries: Vec<MergeEntry>| -> Vec<String> {
        entries.into_iter().map(|entry| entry.branch).collect()
    };
    assert_eq!(branches(first.entries().await), ["a", "b", "c"]);
    assert_eq!(branches(second.entries().await), ["a", "b", "c"]);

    let (processor, _) = processor(&first).await;
    assert!(
        second.processor().await.unwrap().is_none(),
        "one processor per repo"
    );
    while processor.process_next().await.unwrap().is_some() {}
    assert_eq!(*recorder.seen.lock().unwrap(), ["a", "b", "c"]);
    let merged = second.entries().await;
    assert!(merged
        .iter()
        .all(|entry| entry.status == MergeStatus::Merged));
}

#[tokio::test]
async fn a_running_entry_is_left_alone_while_its_processor_lives() {
    let dir = tempfile::tempdir().unwrap();
    let first = open(dir.path(), Arc::new(Hangs)).await;
    first.enqueue(request("a")).await.unwrap();
    let (processor, _) = processor(&first).await;
    let cut = tokio::time::timeout(Duration::from_millis(20), processor.process_next()).await;
    assert!(cut.is_err());
    // Another process opens the queue and queues a push meanwhile.
    let second = open(dir.path(), Arc::new(Recorder::default())).await;
    assert!(second.processor().await.unwrap().is_none());
    second.enqueue(request("b")).await.unwrap();
    let entries = first.entries().await;
    assert_eq!(entries[0].status, MergeStatus::Running);
    assert_eq!(entries.len(), 2);
}

#[tokio::test]
async fn every_finished_entry_is_kept_and_paged_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let limits = Limits {
        keep_finished: 100,
        ..Limits::default()
    };
    let queue = MergeQueue::open(repo(dir.path()), Arc::new(Recorder::default()), limits)
        .await
        .unwrap();
    let (processor, _) = processor(&queue).await;
    for n in 0..250 {
        queue.enqueue(request(&format!("b{n:03}"))).await.unwrap();
        processor.process_next().await.unwrap();
    }
    queue.enqueue(request("waiting")).await.unwrap();
    assert_eq!(queue.entries().await.len(), 101, "the queue file keeps 100");

    let branches = |page: &QueuePage| -> Vec<String> {
        page.entries.iter().map(|e| e.branch.clone()).collect()
    };
    let first = queue.page(100).await;
    assert_eq!(first.older, 150);
    assert_eq!(first.entries.len(), 101);
    assert_eq!(branches(&first)[0], "b150");
    assert_eq!(branches(&first)[100], "waiting");

    let all = queue.page(400).await;
    assert_eq!(all.older, 0);
    let mut expected: Vec<String> = (0..250).map(|n| format!("b{n:03}")).collect();
    expected.push("waiting".to_string());
    assert_eq!(branches(&all), expected, "every merge, oldest first");

    let few = queue.page(10).await;
    assert_eq!((few.entries.len(), few.older), (11, 240));
    assert_eq!(branches(&few)[0], "b240");
}
