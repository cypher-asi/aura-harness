//! A failing test is told apart before an entry is blamed: a target
//! that fails the same test on its own parks the entry and is reported
//! with its suspects; a test that passes when run again lets the entry
//! merge as flaky; a test only the entry breaks fails it, by name.
//!
//! The repo's `sh ci/cargo test` stands in for `cargo test`: it prints
//! libtest's lines and fails while the tree has `broken.txt` (the
//! target's bug), `entry-breaks.txt` (the entry's), or the first time it
//! runs with `flaky.txt`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::support;

use aura_exec_merge_queue::{HoldKind, QueueEvent};
use support::*;

const FAKE_CARGO: &str = r#"#!/bin/sh
[ -f runs.txt ] && echo run >> "$(cat runs.txt)"
fail() { echo "test $1 ... FAILED"; echo; echo "failures:"; echo; echo "---- $1 stdout ----"; echo "assertion failed: $2"; echo; echo "failures:"; echo "    $1"; exit 101; }
[ -f broken.txt ] && fail broken_on_main "main is broken"
[ -f entry-breaks.txt ] && fail entry_case "the entry broke it"
if [ -f flaky.txt ]; then
  mark="$(cat flaky.txt)"
  if [ ! -f "$mark" ]; then touch "$mark"; fail sometimes "flaked"; fi
fi
echo "test result: ok. 3 passed"
"#;

fn config() -> QueueConfig {
    QueueConfig {
        test: Some("sh ci/cargo test".to_string()),
        ..QueueConfig::unchecked("main")
    }
}

/// Main gets the fake cargo and `extra` files, pushed straight to it.
fn main_with(world: &World, extra: &[(&str, &str)]) {
    let agent = &world.agent;
    git(agent, &["checkout", "-q", "-B", "setup", "origin/main"]);
    std::fs::create_dir_all(agent.join("ci")).unwrap();
    std::fs::write(agent.join("ci/cargo"), FAKE_CARGO).unwrap();
    for (file, text) in extra {
        std::fs::write(agent.join(file), text).unwrap();
    }
    git(agent, &["add", "-A"]);
    git(agent, &["commit", "-qm", "main breaks"]);
    git(agent, &["push", "-q", "origin", "HEAD:main"]);
    git(agent, &["fetch", "-q", "origin"]);
}

#[tokio::test]
async fn a_broken_main_is_reported_with_suspects_and_not_blamed_on_the_entry() {
    if !has_git() {
        return;
    }
    let world = world();
    main_with(&world, &[("broken.txt", "bug\n")]);
    let (queue, processor) = queue(&world, config()).await;
    let mut events = queue.subscribe();
    let request = agent_pushes(&world, "aura/innocent-rus203", "a.txt", "a\n");
    queue.enqueue(request).await.unwrap();

    let back = processor.process_next().await.unwrap().unwrap();

    assert_eq!(back.status, MergeStatus::Queued, "{:?}", back.detail);
    assert_eq!(back.attempts, 0, "not charged");
    assert!(back.failure.is_none(), "not blamed");
    let mut held = None;
    while let Ok(event) = events.try_recv() {
        if let QueueEvent::Held { hold, .. } = event {
            held = Some(hold);
        }
    }
    let hold = held.expect("the broken main is announced");
    assert_eq!(hold.kind, HoldKind::TargetBroken);
    assert_eq!(hold.tests, vec!["broken_on_main".to_string()]);
    assert!(hold.excerpt.unwrap_or_default().contains("main is broken"));
    assert_eq!(
        hold.suspects.first().map(|s| s.subject.as_str()),
        Some("main breaks")
    );

    // The fix still lands, and then the parked entry runs and merges.
    queue.enqueue(push_fix(&world)).await.unwrap();
    let fix = processor.process_next().await.unwrap().unwrap();
    assert_eq!(fix.branch, "aura/fix-main-rus207");
    assert_eq!(fix.status, MergeStatus::Merged, "{:?}", fix.detail);
    processor.release_hold().await;
    let retried = processor.process_next().await.unwrap().unwrap();
    assert_eq!(retried.branch, "aura/innocent-rus203");
    assert_eq!(retried.status, MergeStatus::Merged, "{:?}", retried.detail);
}

/// An agent's branch that removes the target's bug, pushed.
fn push_fix(world: &World) -> MergeRequest {
    let agent = &world.agent;
    git(
        agent,
        &[
            "checkout",
            "-q",
            "-B",
            "aura/fix-main-rus207",
            "origin/main",
        ],
    );
    git(agent, &["rm", "-q", "broken.txt"]);
    git(agent, &["commit", "-qm", "fix main"]);
    git(agent, &["push", "-q", "origin", "aura/fix-main-rus207"]);
    MergeRequest {
        push: QueuedPush {
            branch: "aura/fix-main-rus207".to_string(),
            head: git(agent, &["rev-parse", "HEAD"]),
            remote: Some("origin".to_string()),
        },
        origin: Origin::default(),
    }
}

#[tokio::test]
async fn a_test_that_fails_once_then_passes_merges_as_flaky() {
    if !has_git() {
        return;
    }
    let world = world();
    let mark = world.root.path().join("flaked-once");
    main_with(&world, &[("flaky.txt", mark.to_str().unwrap())]);
    let (queue, processor) = queue(&world, config()).await;
    queue
        .enqueue(agent_pushes(&world, "aura/a-rus1", "a.txt", "a\n"))
        .await
        .unwrap();
    let done = processor.process_next().await.unwrap().unwrap();
    assert_eq!(done.status, MergeStatus::Merged, "{:?}", done.detail);
    assert!(
        done.detail
            .unwrap_or_default()
            .contains("flaky, passed when run again: sometimes"),
        "the note names the flaky test"
    );
}

#[tokio::test]
async fn a_test_only_the_entry_breaks_fails_it_by_name() {
    if !has_git() {
        return;
    }
    let world = world();
    main_with(&world, &[]);
    let (queue, processor) = queue(&world, config()).await;
    queue
        .enqueue(agent_pushes(
            &world,
            "aura/b-rus1",
            "entry-breaks.txt",
            "x\n",
        ))
        .await
        .unwrap();
    let done = processor.process_next().await.unwrap().unwrap();
    assert_eq!(done.status, MergeStatus::Failed);
    let failure = done.failure.unwrap();
    assert_eq!(failure.tests, vec!["entry_case".to_string()]);
    assert!(
        failure.reason.contains("pass on main"),
        "{}",
        failure.reason
    );
    assert!(failure
        .excerpt
        .unwrap_or_default()
        .contains("the entry broke it"));
}

#[tokio::test]
async fn a_second_entry_on_a_known_broken_tip_parks_without_running_again() {
    if !has_git() {
        return;
    }
    let world = world();
    let runs = world.root.path().join("runs");
    main_with(
        &world,
        &[
            ("broken.txt", "bug\n"),
            ("runs.txt", runs.to_str().unwrap()),
        ],
    );
    let (queue, processor) = queue(&world, config()).await;
    for branch in ["aura/a-rus1", "aura/b-rus2"] {
        let file = format!("{}.txt", &branch[5..6]);
        queue
            .enqueue(agent_pushes(&world, branch, &file, "x\n"))
            .await
            .unwrap();
    }
    let count = || {
        std::fs::read_to_string(&runs)
            .unwrap_or_default()
            .lines()
            .count()
    };

    let first = processor.process_next().await.unwrap().unwrap();
    assert_eq!(first.status, MergeStatus::Queued);
    let after_first = count();
    assert_eq!(after_first, 3, "the merge, the whole rerun, the target");
    let second = processor.process_next().await.unwrap().unwrap();

    assert_eq!(second.branch, "aura/b-rus2");
    assert_eq!(second.status, MergeStatus::Queued, "{:?}", second.detail);
    assert_eq!(count(), after_first + 1, "only its own merge ran");
}
