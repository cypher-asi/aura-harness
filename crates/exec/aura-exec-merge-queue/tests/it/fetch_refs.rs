//! The queue's fetches write only its private refs. A fetch that also
//! moved the hub's `refs/remotes/origin/<branch>` raced every other fetch
//! of that branch for the ref's lock: the processor's fetch and the
//! overlap scan's, started together when a push was queued, failed the
//! entry with "cannot lock ref".

#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::support;

use aura_exec_merge_queue::overlap::changed_files;
use std::time::Duration;
use support::*;

const TRACKING: &str = "refs/remotes/origin/aura/a";

#[tokio::test]
async fn landing_and_scanning_leave_the_remote_tracking_refs_alone() {
    if !has_git() {
        return;
    }
    let world = world();
    let (queue, processor) = queue(&world, QueueConfig::unchecked("main")).await;
    // The hub tracks the branch at an older commit, as when the agent
    // pushed again since the hub last fetched.
    let stale = git(&world.hub, &["rev-parse", "HEAD"]);
    git(&world.hub, &["update-ref", TRACKING, &stale]);

    queue
        .enqueue(agent_pushes(&world, "aura/a", "a.txt", "a\n"))
        .await
        .unwrap();
    let entry = queue.entries().await.remove(0);
    let files = changed_files(&repo(&world).await, "main", &entry, Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(files, ["a.txt"]);
    let done = processor.process_next().await.unwrap().unwrap();
    assert_eq!(done.status, MergeStatus::Merged, "{:?}", done.detail);

    assert_eq!(
        git(&world.hub, &["rev-parse", TRACKING]),
        stale,
        "the queue's fetches never touch the hub's remote-tracking refs"
    );
}
