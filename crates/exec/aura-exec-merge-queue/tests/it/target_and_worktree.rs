//! The queue with a target other than main, read from the config as
//! committed, and its worktree kept outside `.git`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::support;

use aura_exec_merge_queue::resolve_target;
use support::*;

/// `world` with `.aura/merge-queue.toml` naming `trunk` committed on
/// origin's main, and a `trunk` branch on origin; the hub and the agent
/// fetched both.
fn targeting_trunk(world: &World) {
    let seed = &world.agent;
    git(seed, &["checkout", "-q", "main"]);
    std::fs::create_dir_all(seed.join(".aura")).unwrap();
    commit(seed, ".aura/merge-queue.toml", "target = \"trunk\"\n");
    git(seed, &["push", "-q", "origin", "main", "main:trunk"]);
    for clone in [&world.hub, &world.agent] {
        git(clone, &["fetch", "-q", "origin"]);
    }
    git(&world.hub, &["merge", "-q", "--ff-only", "origin/main"]);
}

/// The agent commits `file` on `branch` off `origin/trunk` and pushes it.
fn agent_pushes_onto_trunk(world: &World, branch: &str, file: &str) -> MergeRequest {
    git(
        &world.agent,
        &["checkout", "-q", "-B", branch, "origin/trunk"],
    );
    let head = commit(&world.agent, file, "x\n");
    git(&world.agent, &["push", "-q", "origin", branch]);
    MergeRequest {
        push: QueuedPush {
            branch: branch.to_string(),
            head,
            remote: Some("origin".to_string()),
        },
        origin: Origin::default(),
    }
}

#[tokio::test]
async fn a_committed_non_main_target_gets_the_merges_and_main_stays() {
    if !has_git() {
        return;
    }
    let world = world();
    targeting_trunk(&world);
    let target = resolve_target(&world.hub).await;
    assert_eq!(target, "trunk");
    let main_before = origin_main(&world);
    let (queue, processor) = queue(&world, QueueConfig::unchecked(&target)).await;
    let request = agent_pushes_onto_trunk(&world, "aura/t", "t.txt");
    let head = request.push.head.clone();
    queue.enqueue(request).await.unwrap();
    let done = processor.process_next().await.unwrap().unwrap();
    assert_eq!(done.status, MergeStatus::Merged, "{:?}", done.detail);
    assert_eq!(git(&world.origin, &["rev-parse", "trunk"]), head);
    assert_eq!(origin_main(&world), main_before, "main never moves");
}

#[tokio::test]
async fn the_worktree_is_outside_git_and_the_old_one_is_retired() {
    if !has_git() {
        return;
    }
    let world = world();
    let repo = repo(&world).await;
    // The worktree a queue from before the move left in `.git`.
    let legacy = repo.legacy_worktree();
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    git(
        &world.hub,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            legacy.to_str().unwrap(),
            "HEAD",
        ],
    );
    std::fs::write(repo.legacy_check_log(), "old log").unwrap();

    let (queue, processor) = queue(&world, QueueConfig::unchecked("main")).await;
    queue
        .enqueue(agent_pushes(&world, "aura/a", "a.txt", "a\n"))
        .await
        .unwrap();
    let done = processor.process_next().await.unwrap().unwrap();
    assert_eq!(done.status, MergeStatus::Merged, "{:?}", done.detail);

    let worktree = repo.worktree();
    assert!(worktree.join(".git").exists(), "{}", worktree.display());
    assert!(worktree.starts_with(world.root.path().join("work")));
    assert!(!worktree.starts_with(&repo.common), "outside .git");
    assert!(!legacy.exists(), "the old worktree is gone");
    assert!(!repo.legacy_check_log().exists());
    let listed = git(&world.hub, &["worktree", "list", "--porcelain"]);
    assert!(!listed.contains("merge-worktree"), "{listed}");
    assert!(listed.contains(worktree.to_str().unwrap()), "{listed}");
}

#[tokio::test]
async fn a_worktree_folder_git_forgot_is_made_again() {
    if !has_git() {
        return;
    }
    let world = world();
    let repo = repo(&world).await;
    let (queue, processor) = queue(&world, QueueConfig::unchecked("main")).await;
    queue
        .enqueue(agent_pushes(&world, "aura/a", "a.txt", "a\n"))
        .await
        .unwrap();
    processor.process_next().await.unwrap().unwrap();
    // The cache was cleaned: the folder stays, its `.git` link is gone.
    std::fs::remove_file(repo.worktree().join(".git")).unwrap();
    queue
        .enqueue(agent_pushes(&world, "aura/b", "b.txt", "b\n"))
        .await
        .unwrap();
    let done = processor.process_next().await.unwrap().unwrap();
    assert_eq!(done.status, MergeStatus::Merged, "{:?}", done.detail);
    assert!(repo.worktree().join(".git").exists());
}
