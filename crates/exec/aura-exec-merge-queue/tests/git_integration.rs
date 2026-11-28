//! The queue landing real pushes with git: a bare origin, the hub's
//! clone the queue works from, and an agent's clone that pushes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use aura_exec_merge_queue::processor::Processor;
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

/// The hub's queue for `world` and its processor.
async fn queue(world: &World, config: QueueConfig) -> (MergeQueue, Processor) {
    let repo = RepoPaths::of(&world.hub).await.unwrap();
    let integrator = Arc::new(GitIntegrator::new(repo.clone(), config));
    let queue = MergeQueue::open(repo, integrator, Limits::default())
        .await
        .unwrap();
    let processor = queue.processor().await.unwrap().unwrap();
    (queue, processor)
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
    let (queue, processor) = queue(&world, QueueConfig::unchecked("main")).await;
    let request = agent_pushes(&world, "aura/a", "a.txt", "a\n");
    let head = request.push.head.clone();
    queue.enqueue(request).await.unwrap();
    let done = processor.process_next().await.unwrap().unwrap();
    assert_eq!(done.status, MergeStatus::Merged, "{:?}", done.detail);
    assert_eq!(origin_main(&world), head, "the exact pushed commit lands");
}

#[tokio::test]
async fn entries_are_rebased_onto_what_merged_before_them() {
    if !has_git() {
        return;
    }
    let world = world();
    let (queue, processor) = queue(&world, QueueConfig::unchecked("main")).await;
    queue
        .enqueue(agent_pushes(&world, "aura/a", "a.txt", "a\n"))
        .await
        .unwrap();
    queue
        .enqueue(agent_pushes(&world, "aura/b", "b.txt", "b\n"))
        .await
        .unwrap();
    while processor.process_next().await.unwrap().is_some() {}
    let entries = queue.entries().await;
    assert!(
        entries.iter().all(|e| e.status == MergeStatus::Merged),
        "{entries:?}"
    );
    assert!(entries[1]
        .detail
        .as_deref()
        .unwrap_or_default()
        .contains("merged with"));
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
    let (queue, processor) = queue(&world, QueueConfig::unchecked("main")).await;
    queue
        .enqueue(agent_pushes(&world, "aura/a", "README", "one\n"))
        .await
        .unwrap();
    queue
        .enqueue(agent_pushes(&world, "aura/b", "README", "two\n"))
        .await
        .unwrap();
    processor.process_next().await.unwrap();
    let main_after_first = origin_main(&world);
    let second = processor.process_next().await.unwrap().unwrap();
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
    let (queue, processor) = queue(&world, config).await;
    queue
        .enqueue(agent_pushes(&world, "aura/a", "broken.txt", "x\n"))
        .await
        .unwrap();
    let done = processor.process_next().await.unwrap().unwrap();
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
    let (queue, processor) = queue(&world, config).await;
    queue
        .enqueue(agent_pushes(&world, "aura/a", "ok.txt", "x\n"))
        .await
        .unwrap();
    let done = processor.process_next().await.unwrap().unwrap();
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
    let (queue, processor) = queue(&world, QueueConfig::unchecked("main")).await;
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
    let done = processor.process_next().await.unwrap().unwrap();
    assert_eq!(done.status, MergeStatus::Merged, "{:?}", done.detail);
    assert_eq!(git(&world.hub, &["rev-parse", "main"]), head);
    assert!(
        world.hub.join("l.txt").exists(),
        "the checkout moved with main"
    );
}

/// The agent commits on top of `branch` as it is and pushes it.
fn agent_commits_and_pushes(world: &World, branch: &str, message: &str) -> MergeRequest {
    git(&world.agent, &["add", "-A"]);
    git(&world.agent, &["commit", "-qm", message]);
    git(&world.agent, &["push", "-q", "origin", branch]);
    MergeRequest {
        push: QueuedPush {
            branch: branch.to_string(),
            head: git(&world.agent, &["rev-parse", "HEAD"]),
            remote: Some("origin".to_string()),
        },
        origin: Origin::default(),
    }
}

#[tokio::test]
async fn a_conflict_the_agent_resolved_by_merging_main_does_not_come_back() {
    if !has_git() {
        return;
    }
    let world = world();
    let (queue, processor) = queue(&world, QueueConfig::unchecked("main")).await;
    // The agent's branch and main both change README.
    let _mine = agent_pushes(&world, "aura/a", "README", "mine\n");
    let theirs = agent_pushes(&world, "aura/other", "README", "theirs\n");
    queue.enqueue(theirs).await.unwrap();
    processor.process_next().await.unwrap();
    // The agent merges main and resolves, as git_ship has it do.
    git(&world.agent, &["checkout", "-q", "aura/a"]);
    git(&world.agent, &["fetch", "-q", "origin", "main"]);
    let merge = Command::new("git")
        .args(["merge", "-q", "FETCH_HEAD"])
        .current_dir(&world.agent)
        .output()
        .unwrap();
    assert!(!merge.status.success(), "the README conflicts");
    std::fs::write(world.agent.join("README"), "both\n").unwrap();
    let resolved = agent_commits_and_pushes(&world, "aura/a", "merge main");
    // main moves on once more, elsewhere, before the queue gets to it.
    let later = agent_pushes(&world, "aura/c", "c.txt", "c\n");
    queue.enqueue(later).await.unwrap();
    processor.process_next().await.unwrap();
    queue.enqueue(resolved).await.unwrap();
    let done = processor.process_next().await.unwrap().unwrap();
    assert_eq!(done.status, MergeStatus::Merged, "{:?}", done.detail);
    assert_eq!(git(&world.origin, &["show", "main:README"]), "both");
    let files = git(&world.origin, &["ls-tree", "--name-only", "main"]);
    assert!(files.contains("c.txt"), "{files}");
}

#[tokio::test]
async fn the_checks_are_the_target_branchs_not_the_pushed_ones() {
    if !has_git() {
        return;
    }
    let world = world();
    let (queue, processor) = queue(&world, QueueConfig::unchecked("main")).await;
    let config = ".aura/merge-queue.toml";
    // main says what must pass.
    git(
        &world.agent,
        &["checkout", "-q", "-B", "aura/cfg", "origin/main"],
    );
    std::fs::create_dir_all(world.agent.join(".aura")).unwrap();
    std::fs::write(world.agent.join(config), "test = \"test ! -f bad.txt\"\n").unwrap();
    let rules = agent_commits_and_pushes(&world, "aura/cfg", "checks");
    queue.enqueue(rules).await.unwrap();
    let first = processor.process_next().await.unwrap().unwrap();
    assert_eq!(first.status, MergeStatus::Merged, "{:?}", first.detail);
    // A push turning the check off doesn't turn it off for itself.
    git(&world.agent, &["fetch", "-q", "origin"]);
    git(
        &world.agent,
        &["checkout", "-q", "-B", "aura/sneaky", "origin/main"],
    );
    std::fs::write(world.agent.join(config), "test = \"\"\n").unwrap();
    std::fs::write(world.agent.join("bad.txt"), "x\n").unwrap();
    let sneaky = agent_commits_and_pushes(&world, "aura/sneaky", "sneak");
    queue.enqueue(sneaky).await.unwrap();
    let done = processor.process_next().await.unwrap().unwrap();
    assert_eq!(done.status, MergeStatus::Failed, "{:?}", done.detail);
}

#[tokio::test]
async fn a_target_that_moves_during_the_checks_is_merged_again_and_landed() {
    if !has_git() {
        return;
    }
    let world = world();
    let root = world.origin.parent().unwrap().to_path_buf();
    let mover = root.join("mover");
    let (origin, moved) = (world.origin.display(), mover.display());
    git(
        &root,
        &["clone", "-q", &origin.to_string(), &moved.to_string()],
    );
    // The first time the check runs, someone else pushes to main.
    let once = root.join("moved-once");
    let check = format!(
        "if [ ! -f {once} ]; then touch {once}; cd {moved} && echo m > m.txt && git add -A && \
         git -c user.name=t -c user.email=t@t commit -qm move && git push -q origin HEAD:main; fi",
        once = once.display(),
    );
    let config = QueueConfig {
        test: Some(check),
        ..QueueConfig::unchecked("main")
    };
    let (queue, processor) = queue(&world, config).await;
    let request = agent_pushes(&world, "aura/a", "a.txt", "a\n");
    queue.enqueue(request).await.unwrap();
    let done = processor.process_next().await.unwrap().unwrap();
    assert_eq!(done.status, MergeStatus::Merged, "{:?}", done.detail);
    let files = git(&world.origin, &["ls-tree", "--name-only", "main"]);
    assert!(
        files.contains("a.txt") && files.contains("m.txt"),
        "{files}"
    );
}
