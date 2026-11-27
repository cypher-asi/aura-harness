//! The queue landing real pushes with git: a bare origin, the hub's
//! clone the queue works from, and an agent's clone that pushes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use aura_exec_merge_queue::{
    GitIntegrator, Limits, MergeQueue, MergeRequest, MergeStatus, Origin, QueueConfig, QueuedPush,
    RepoPaths,
};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str, text: &str) -> String {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", file]);
    git(dir, &["rev-parse", "HEAD"])
}

/// A bare origin with `main`, the hub's clone, and an agent's clone.
struct World {
    _root: tempfile::TempDir,
    origin: std::path::PathBuf,
    hub: std::path::PathBuf,
    agent: std::path::PathBuf,
}

fn world() -> World {
    let root = tempfile::tempdir().unwrap();
    let origin = root.path().join("origin.git");
    let seed = root.path().join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git(
        root.path(),
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    git(&seed, &["init", "-q", "-b", "main"]);
    commit(&seed, "README", "hello\n");
    git(
        &seed,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&seed, &["push", "-q", "origin", "main"]);
    let hub = root.path().join("hub");
    let agent = root.path().join("agent");
    for clone in [&hub, &agent] {
        git(
            root.path(),
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
    }
    World {
        _root: root,
        origin,
        hub,
        agent,
    }
}

/// The agent commits `file` on `branch` and pushes it; the push to queue.
fn agent_pushes(world: &World, branch: &str, file: &str, text: &str) -> MergeRequest {
    git(
        &world.agent,
        &["checkout", "-q", "-B", branch, "origin/main"],
    );
    let head = commit(&world.agent, file, text);
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

fn origin_main(world: &World) -> String {
    git(&world.origin, &["rev-parse", "main"])
}

async fn queue(world: &World, config: QueueConfig) -> MergeQueue {
    let repo = RepoPaths::of(&world.hub).await.unwrap();
    let integrator = Arc::new(GitIntegrator::new(repo.clone(), config));
    MergeQueue::open(repo, integrator, Limits::default())
        .await
        .unwrap()
        .0
}

fn has_git() -> bool {
    Command::new("git").arg("--version").output().is_ok()
}

#[tokio::test]
async fn a_push_based_on_main_fast_forwards_main() {
    if !has_git() {
        return;
    }
    let world = world();
    let queue = queue(&world, QueueConfig::unchecked("main")).await;
    let request = agent_pushes(&world, "aura/a", "a.txt", "a\n");
    let head = request.push.head.clone();
    queue.enqueue(request).await.unwrap();
    let done = queue.process_next().await.unwrap().unwrap();
    assert_eq!(done.status, MergeStatus::Merged, "{:?}", done.detail);
    assert_eq!(origin_main(&world), head, "the exact pushed commit lands");
}

#[tokio::test]
async fn entries_are_rebased_onto_what_merged_before_them() {
    if !has_git() {
        return;
    }
    let world = world();
    let queue = queue(&world, QueueConfig::unchecked("main")).await;
    queue
        .enqueue(agent_pushes(&world, "aura/a", "a.txt", "a\n"))
        .await
        .unwrap();
    queue
        .enqueue(agent_pushes(&world, "aura/b", "b.txt", "b\n"))
        .await
        .unwrap();
    while queue.process_next().await.unwrap().is_some() {}
    let entries = queue.entries().await;
    assert!(
        entries.iter().all(|e| e.status == MergeStatus::Merged),
        "{entries:?}"
    );
    assert!(entries[1]
        .detail
        .as_deref()
        .unwrap_or_default()
        .contains("rebased"));
    let files = git(&world.origin, &["ls-tree", "--name-only", "main"]);
    assert!(
        files.contains("a.txt") && files.contains("b.txt"),
        "{files}"
    );
}

#[tokio::test]
async fn a_conflict_fails_the_entry_and_leaves_main_alone() {
    if !has_git() {
        return;
    }
    let world = world();
    let queue = queue(&world, QueueConfig::unchecked("main")).await;
    queue
        .enqueue(agent_pushes(&world, "aura/a", "README", "one\n"))
        .await
        .unwrap();
    queue
        .enqueue(agent_pushes(&world, "aura/b", "README", "two\n"))
        .await
        .unwrap();
    queue.process_next().await.unwrap();
    let main_after_first = origin_main(&world);
    let second = queue.process_next().await.unwrap().unwrap();
    assert_eq!(second.status, MergeStatus::Failed);
    assert!(second.detail.unwrap_or_default().contains("README"));
    assert_eq!(origin_main(&world), main_after_first);
}

#[tokio::test]
async fn a_failing_test_keeps_the_push_off_main() {
    if !has_git() {
        return;
    }
    let world = world();
    let before = origin_main(&world);
    let config = QueueConfig {
        test: Some("test ! -f broken.txt".to_string()),
        ..QueueConfig::unchecked("main")
    };
    let queue = queue(&world, config).await;
    queue
        .enqueue(agent_pushes(&world, "aura/a", "broken.txt", "x\n"))
        .await
        .unwrap();
    let done = queue.process_next().await.unwrap().unwrap();
    assert_eq!(done.status, MergeStatus::Failed);
    assert!(done.detail.unwrap_or_default().starts_with("test "));
    assert_eq!(origin_main(&world), before);
}

#[tokio::test]
async fn a_passing_build_and_test_let_the_push_land() {
    if !has_git() {
        return;
    }
    let world = world();
    let config = QueueConfig {
        build: Some("test -f ok.txt".to_string()),
        test: Some("true".to_string()),
        ..QueueConfig::unchecked("main")
    };
    let queue = queue(&world, config).await;
    queue
        .enqueue(agent_pushes(&world, "aura/a", "ok.txt", "x\n"))
        .await
        .unwrap();
    let done = queue.process_next().await.unwrap().unwrap();
    assert_eq!(done.status, MergeStatus::Merged, "{:?}", done.detail);
    assert!(done
        .detail
        .unwrap_or_default()
        .contains("build and test passed"));
}

#[tokio::test]
async fn a_local_branch_fast_forwards_a_clean_checkout_of_main() {
    if !has_git() {
        return;
    }
    let world = world();
    git(&world.hub, &["checkout", "-q", "-b", "aura/local"]);
    let head = commit(&world.hub, "l.txt", "l\n");
    git(&world.hub, &["checkout", "-q", "main"]);
    let queue = queue(&world, QueueConfig::unchecked("main")).await;
    queue
        .enqueue(MergeRequest {
            push: QueuedPush {
                branch: "aura/local".to_string(),
                head: head.clone(),
                remote: None,
            },
            origin: Origin::default(),
        })
        .await
        .unwrap();
    let done = queue.process_next().await.unwrap().unwrap();
    assert_eq!(done.status, MergeStatus::Merged, "{:?}", done.detail);
    assert_eq!(git(&world.hub, &["rev-parse", "main"]), head);
    assert!(
        world.hub.join("l.txt").exists(),
        "the checkout moved with main"
    );
}
