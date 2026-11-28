//! The queue landing pushes through their PRs, on a fake GitHub over a
//! bare origin: a ship that auto-merges, a conflict and a failing test
//! that leave the PR open with a comment, a head that moved under the
//! test, branch cleanup, and an unreachable GitHub.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use aura_exec_merge_queue::forge::{CheckState, PrState};
use aura_exec_merge_queue::{FailureKind, HoldKind, QueueEvent};
use std::sync::atomic::Ordering;
use support::forge::FakeForge;
use support::*;

fn forge(world: &World) -> Arc<FakeForge> {
    Arc::new(FakeForge::new(world.origin.clone()))
}

fn parents(world: &World, commit: &str) -> Vec<String> {
    git(&world.origin, &["rev-list", "--parents", "-n", "1", commit])
        .split_whitespace()
        .skip(1)
        .map(str::to_string)
        .collect()
}

#[tokio::test]
async fn a_ship_opens_a_pr_that_merges_at_the_tested_head_and_its_branch_goes() {
    if !has_git() {
        return;
    }
    let world = world();
    let forge = forge(&world);
    let (queue, processor) =
        queue_with_forge(&world, QueueConfig::unchecked("main"), Arc::clone(&forge)).await;
    let main_before = origin_main(&world);
    let request = agent_pushes(&world, "aura/a-rus1", "a.txt", "a\n");
    let head = request.push.head.clone();
    queue.enqueue(request).await.unwrap();

    let done = processor.process_next().await.unwrap().unwrap();

    assert_eq!(done.status, MergeStatus::Merged, "{:?}", done.detail);
    let pr = forge.pr_of("aura/a-rus1");
    assert_eq!(pr.state, PrState::Merged);
    assert_eq!(pr.title, "a.txt", "the PR is titled by its first commit");
    assert_eq!(done.pr.as_ref().map(|pr| pr.number), Some(pr.number));
    let merge = origin_main(&world);
    assert_eq!(done.merged.as_deref(), Some(merge.as_str()));
    assert_eq!(
        parents(&world, &merge),
        vec![main_before, head.clone()],
        "a normal merge commit of the tested head"
    );
    assert_eq!(
        forge.status_of(&merge),
        Some(CheckState::Success),
        "main is the tested commit"
    );
    assert_eq!(
        forge.tip("aura/a-rus1"),
        None,
        "the merged branch is deleted"
    );
    assert!(forge.comments_on(pr.number).is_empty());
}

#[tokio::test]
async fn a_branch_behind_main_gets_the_tested_merge_on_its_pr_first() {
    if !has_git() {
        return;
    }
    let world = world();
    let forge = forge(&world);
    let (queue, processor) =
        queue_with_forge(&world, QueueConfig::unchecked("main"), Arc::clone(&forge)).await;
    queue
        .enqueue(agent_pushes(&world, "aura/a-rus1", "a.txt", "a\n"))
        .await
        .unwrap();
    queue
        .enqueue(agent_pushes(&world, "aura/b-rus1", "b.txt", "b\n"))
        .await
        .unwrap();
    while processor.process_next().await.unwrap().is_some() {}

    let entries = queue.entries().await;
    assert!(
        entries.iter().all(|e| e.status == MergeStatus::Merged),
        "{entries:?}"
    );
    let files = git(&world.origin, &["ls-tree", "--name-only", "main"]);
    assert!(
        files.contains("a.txt") && files.contains("b.txt"),
        "{files}"
    );
    let second = forge.pr_of("aura/b-rus1");
    assert_eq!(second.state, PrState::Merged);
    assert_eq!(
        forge.status_of(&second.head),
        Some(CheckState::Success),
        "the PR's head is the queue's tested merge"
    );
    assert_eq!(forge.tip("aura/b-rus1"), None);
}

#[tokio::test]
async fn a_conflict_leaves_the_pr_open_with_a_comment_naming_the_files() {
    if !has_git() {
        return;
    }
    let world = world();
    let forge = forge(&world);
    let (queue, processor) =
        queue_with_forge(&world, QueueConfig::unchecked("main"), Arc::clone(&forge)).await;
    queue
        .enqueue(agent_pushes(&world, "aura/a-rus1", "README", "one\n"))
        .await
        .unwrap();
    queue
        .enqueue(agent_pushes(&world, "aura/b-rus2", "README", "two\n"))
        .await
        .unwrap();
    processor.process_next().await.unwrap();
    let main_after_first = origin_main(&world);

    let second = processor.process_next().await.unwrap().unwrap();

    assert_eq!(second.status, MergeStatus::Failed);
    let failure = second.failure.clone().unwrap();
    assert_eq!(failure.kind, FailureKind::Conflict);
    assert_eq!(failure.files, vec!["README".to_string()]);
    assert!(failure.pr.is_some(), "the agent hears which PR");
    assert_eq!(origin_main(&world), main_after_first, "nothing merged");
    let pr = forge.pr_of("aura/b-rus2");
    assert_eq!(pr.state, PrState::Open, "the PR stays open");
    let comments = forge.comments_on(pr.number);
    assert_eq!(comments.len(), 1);
    assert!(
        comments[0].contains("Conflicting files") && comments[0].contains("`README`"),
        "{}",
        comments[0]
    );
    assert!(
        comments[0].contains("aura/b-rus2"),
        "fix it on the same branch"
    );
    assert_eq!(forge.status_of(&pr.head), Some(CheckState::Failure));
    assert!(forge.tip("aura/b-rus2").is_some(), "the branch stays");
}

#[tokio::test]
async fn a_failing_test_blocks_the_merge() {
    if !has_git() {
        return;
    }
    let world = world();
    let forge = forge(&world);
    let config = QueueConfig {
        test: Some("echo 'test broken::case ... FAILED'; exit 101".to_string()),
        ..QueueConfig::unchecked("main")
    };
    let (queue, processor) = queue_with_forge(&world, config, Arc::clone(&forge)).await;
    let main_before = origin_main(&world);
    queue
        .enqueue(agent_pushes(&world, "aura/a-rus1", "a.txt", "a\n"))
        .await
        .unwrap();

    let done = processor.process_next().await.unwrap().unwrap();

    assert_eq!(done.status, MergeStatus::Failed);
    let failure = done.failure.unwrap();
    assert_eq!(failure.kind, FailureKind::Check);
    assert!(failure.excerpt.unwrap_or_default().contains("FAILED"));
    assert_eq!(origin_main(&world), main_before);
    let pr = forge.pr_of("aura/a-rus1");
    assert_eq!(pr.state, PrState::Open);
    assert_eq!(forge.status_of(&pr.head), Some(CheckState::Failure));
    let comments = forge.comments_on(pr.number);
    assert!(comments[0].contains("a check failed"), "{}", comments[0]);
    assert!(comments[0].contains("test broken::case ... FAILED"));
}

#[tokio::test]
async fn a_head_that_moves_after_the_test_is_tested_again_before_it_merges() {
    if !has_git() {
        return;
    }
    let world = world();
    let forge = forge(&world);
    let (queue, processor) =
        queue_with_forge(&world, QueueConfig::unchecked("main"), Arc::clone(&forge)).await;
    let request = agent_pushes(&world, "aura/a-rus1", "a.txt", "a\n");
    let first_head = request.push.head.clone();
    queue.enqueue(request).await.unwrap();
    let agent = world.agent.clone();
    forge.before_next_merge(move || {
        // As git_ship does: pull its own branch, then push on top.
        git(
            &agent,
            &["pull", "-q", "--no-rebase", "origin", "aura/a-rus1"],
        );
        commit(&agent, "late.txt", "late\n");
        git(&agent, &["push", "-q", "origin", "aura/a-rus1"]);
    });

    let done = processor.process_next().await.unwrap().unwrap();

    assert_eq!(done.status, MergeStatus::Merged, "{:?}", done.detail);
    let files = git(&world.origin, &["ls-tree", "--name-only", "main"]);
    assert!(files.contains("late.txt"), "the newer head merged: {files}");
    let pr = forge.pr_of("aura/a-rus1");
    let statuses = forge.statuses.lock().unwrap().clone();
    let tested: Vec<_> = statuses
        .iter()
        .filter(|(_, state, _)| *state == CheckState::Success)
        .map(|(sha, _, _)| sha.clone())
        .collect();
    assert_eq!(tested.len(), 2, "tested twice: {statuses:?}");
    assert_eq!(parents(&world, &tested[0])[1], first_head);
    assert_eq!(
        origin_main(&world),
        tested[1],
        "main is the second tested commit"
    );
    assert_eq!(pr.state, PrState::Merged);
}

#[tokio::test]
async fn an_abandoned_pr_fails_its_entry_and_keeps_its_branch() {
    if !has_git() {
        return;
    }
    let world = world();
    let forge = forge(&world);
    let (queue, processor) =
        queue_with_forge(&world, QueueConfig::unchecked("main"), Arc::clone(&forge)).await;
    let main_before = origin_main(&world);
    queue
        .enqueue(agent_pushes(&world, "aura/a-rus1", "a.txt", "a\n"))
        .await
        .unwrap();
    let closing = Arc::clone(&forge);
    forge.before_next_merge(move || closing.close(1));

    let done = processor.process_next().await.unwrap().unwrap();

    assert_eq!(done.status, MergeStatus::Failed);
    assert!(
        done.detail
            .as_deref()
            .unwrap_or_default()
            .contains("closed"),
        "{:?}",
        done.detail
    );
    assert_eq!(origin_main(&world), main_before);
    assert!(
        forge.tip("aura/a-rus1").is_some(),
        "a closed PR's branch is kept"
    );
}

#[tokio::test]
async fn an_unreachable_github_holds_the_queue_and_charges_no_one() {
    if !has_git() {
        return;
    }
    let world = world();
    let forge = forge(&world);
    let (queue, processor) =
        queue_with_forge(&world, QueueConfig::unchecked("main"), Arc::clone(&forge)).await;
    let mut events = queue.subscribe();
    forge.unavailable.store(true, Ordering::SeqCst);
    queue
        .enqueue(agent_pushes(&world, "aura/a-rus1", "a.txt", "a\n"))
        .await
        .unwrap();

    let back = processor.process_next().await.unwrap().unwrap();

    assert_eq!(back.status, MergeStatus::Queued, "{:?}", back.detail);
    assert_eq!(back.attempts, 0, "not charged");
    assert!(
        processor.process_next().await.unwrap().is_none(),
        "nothing starts while held"
    );
    let mut held = None;
    while let Ok(event) = events.try_recv() {
        if let QueueEvent::Held { hold, .. } = event {
            held = Some(hold);
        }
    }
    assert_eq!(held.map(|hold| hold.kind), Some(HoldKind::ForgeUnavailable));
}

#[tokio::test]
async fn a_repo_set_to_push_lands_without_a_pr() {
    if !has_git() {
        return;
    }
    let world = world();
    let forge = forge(&world);
    let config = QueueConfig {
        land: aura_exec_merge_queue::Landing::Push,
        ..QueueConfig::unchecked("main")
    };
    // The integrator's own config stands in for the target's file here.
    let (queue, processor) = queue_with_forge(&world, config, Arc::clone(&forge)).await;
    let request = agent_pushes(&world, "aura/a-rus1", "a.txt", "a\n");
    let head = request.push.head.clone();
    queue.enqueue(request).await.unwrap();
    let done = processor.process_next().await.unwrap().unwrap();
    assert_eq!(done.status, MergeStatus::Merged, "{:?}", done.detail);
    assert_eq!(origin_main(&world), head);
    assert!(forge.prs.lock().unwrap().is_empty());
}
