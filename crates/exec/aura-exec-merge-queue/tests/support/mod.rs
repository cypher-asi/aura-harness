//! A bare origin, the hub's clone the queue works from, and an agent's
//! clone that pushes, shared by the integration tests.

#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

pub use aura_exec_merge_queue::{
    GitIntegrator, Limits, MergeQueue, MergeRequest, MergeStatus, Origin, Processor, QueueConfig,
    QueuedPush, RepoPaths,
};
pub use std::path::Path;
pub use std::process::Command;
pub use std::sync::Arc;

pub fn git(dir: &Path, args: &[&str]) -> String {
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

pub fn commit(dir: &Path, file: &str, text: &str) -> String {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", file]);
    git(dir, &["rev-parse", "HEAD"])
}

/// A bare origin with `main`, the hub's clone, and an agent's clone.
pub struct World {
    pub root: tempfile::TempDir,
    pub origin: std::path::PathBuf,
    pub hub: std::path::PathBuf,
    pub agent: std::path::PathBuf,
}

pub fn world() -> World {
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
        root,
        origin,
        hub,
        agent,
    }
}

/// The agent commits `file` on `branch` and pushes it; the push to queue.
pub fn agent_pushes(world: &World, branch: &str, file: &str, text: &str) -> MergeRequest {
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

pub fn origin_main(world: &World) -> String {
    git(&world.origin, &["rev-parse", "main"])
}

/// The hub's queue for `world` and its processor.
pub async fn queue(world: &World, config: QueueConfig) -> (MergeQueue, Processor) {
    let repo = repo(world).await;
    let integrator = Arc::new(GitIntegrator::new(repo.clone(), config));
    let queue = MergeQueue::open(repo, integrator, Limits::default())
        .await
        .unwrap();
    let processor = queue.processor().await.unwrap().unwrap();
    (queue, processor)
}

pub fn has_git() -> bool {
    Command::new("git").arg("--version").output().is_ok()
}

/// The hub's repo, its work folder kept in the test's folder.
pub async fn repo(world: &World) -> RepoPaths {
    RepoPaths::of(&world.hub)
        .await
        .unwrap()
        .with_work_root(&world.root.path().join("work"))
}
