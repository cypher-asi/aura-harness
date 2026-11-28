//! The queue settling conflicts with real git: a CHANGELOG-only conflict
//! and a Cargo.lock conflict are resolved and land; a real code conflict
//! goes back with its files listed.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use aura_exec_merge_queue::processor::Processor;
use aura_exec_merge_queue::{
    GitIntegrator, Limits, MergeEntry, MergeQueue, MergeRequest, MergeStatus, Origin, QueueConfig,
    QueuedPush, RepoPaths,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

const CHANGELOG: &str = "# Changelog\n\n## [Unreleased]\n\n### Old\n\n- old\n";

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

fn write(dir: &Path, file: &str, text: &str) {
    let path = dir.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn commit_all(dir: &Path, message: &str) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", message]);
    git(dir, &["rev-parse", "HEAD"])
}

/// A bare origin whose `main` has `files`, the hub's clone and an
/// agent's clone.
struct World {
    _root: tempfile::TempDir,
    origin: PathBuf,
    hub: PathBuf,
    agent: PathBuf,
}

fn world(files: &[(&str, &str)]) -> World {
    let root = tempfile::tempdir().unwrap();
    let origin = root.path().join("origin.git");
    let seed = root.path().join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    let bare = origin.to_str().unwrap();
    git(root.path(), &["init", "-q", "--bare", "-b", "main", bare]);
    git(&seed, &["init", "-q", "-b", "main"]);
    for (file, text) in files {
        write(&seed, file, text);
    }
    commit_all(&seed, "seed");
    git(&seed, &["remote", "add", "origin", bare]);
    git(&seed, &["push", "-q", "origin", "main"]);
    let hub = root.path().join("hub");
    let agent = root.path().join("agent");
    for clone in [&hub, &agent] {
        git(
            root.path(),
            &["clone", "-q", bare, clone.to_str().unwrap()],
        );
    }
    World {
        _root: root,
        origin,
        hub,
        agent,
    }
}

/// The agent branches `branch` off origin's main, writes `files`, commits
/// and pushes; the push to queue.
fn agent_pushes(world: &World, branch: &str, files: &[(&str, &str)]) -> MergeRequest {
    git(&world.agent, &["fetch", "-q", "origin"]);
    git(
        &world.agent,
        &["checkout", "-q", "-B", branch, "origin/main"],
    );
    for (file, text) in files {
        write(&world.agent, file, text);
    }
    let head = commit_all(&world.agent, branch);
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

async fn queue(world: &World, config: QueueConfig) -> (MergeQueue, Processor) {
    let repo = RepoPaths::of(&world.hub).await.unwrap();
    let integrator = Arc::new(GitIntegrator::new(repo.clone(), config));
    let queue = MergeQueue::open(repo, integrator, Limits::default())
        .await
        .unwrap();
    let processor = queue.processor().await.unwrap().unwrap();
    (queue, processor)
}

/// Queue both requests, made off the same main, and run them in turn;
/// returns the second, finished.
async fn land_both(
    queue: &MergeQueue,
    processor: &Processor,
    first: MergeRequest,
    second: MergeRequest,
) -> MergeEntry {
    queue.enqueue(first).await.unwrap();
    queue.enqueue(second).await.unwrap();
    let first = processor.process_next().await.unwrap().unwrap();
    assert_eq!(first.status, MergeStatus::Merged, "{:?}", first.detail);
    processor.process_next().await.unwrap().unwrap()
}

fn on_main(world: &World, file: &str) -> String {
    git(&world.origin, &["show", &format!("main:{file}")])
}

fn has(program: &str) -> bool {
    Command::new(program).arg("--version").output().is_ok()
}

fn with_entry(title: &str) -> String {
    CHANGELOG.replace(
        "## [Unreleased]\n\n",
        &format!("## [Unreleased]\n\n### {title}\n\n- {title}\n\n"),
    )
}

#[tokio::test]
async fn a_changelog_only_conflict_is_resolved_and_lands_checked() {
    if !has("git") {
        return;
    }
    let world = world(&[("CHANGELOG.md", CHANGELOG), ("README", "hello\n")]);
    // The check fails on any conflict marker, so it proves the checks ran
    // on the resolved commit.
    let config = QueueConfig {
        build: Some("! grep -q '<<<<<<<' CHANGELOG.md".to_string()),
        ..QueueConfig::unchecked("main")
    };
    let (queue, processor) = queue(&world, config).await;
    let first = agent_pushes(
        &world,
        "aura/a",
        &[("CHANGELOG.md", &with_entry("A")), ("a.rs", "a\n")],
    );
    let second = agent_pushes(
        &world,
        "aura/b",
        &[("CHANGELOG.md", &with_entry("B")), ("b.rs", "b\n")],
    );
    let done = land_both(&queue, &processor, first, second).await;
    assert_eq!(done.status, MergeStatus::Merged, "{:?}", done.detail);
    let detail = done.detail.unwrap_or_default();
    assert!(
        detail.contains("resolved CHANGELOG.md mechanically") && detail.contains("build passed"),
        "{detail}"
    );
    let changelog = on_main(&world, "CHANGELOG.md");
    assert!(
        changelog.contains("### A") && changelog.contains("### B") && changelog.contains("### Old"),
        "{changelog}"
    );
    assert!(!changelog.contains("<<<<<<<"), "{changelog}");
    let files = git(&world.origin, &["ls-tree", "--name-only", "main"]);
    assert!(files.contains("a.rs") && files.contains("b.rs"), "{files}");
}

#[tokio::test]
async fn a_code_conflict_goes_back_with_its_files_listed() {
    if !has("git") {
        return;
    }
    let world = world(&[("CHANGELOG.md", CHANGELOG), ("src/lib.rs", "fn a() {}\n")]);
    let (queue, processor) = queue(&world, QueueConfig::unchecked("main")).await;
    let first = agent_pushes(
        &world,
        "aura/a",
        &[
            ("CHANGELOG.md", &with_entry("A")),
            ("src/lib.rs", "fn a() { 1 }\n"),
        ],
    );
    let second = agent_pushes(
        &world,
        "aura/b",
        &[
            ("CHANGELOG.md", &with_entry("B")),
            ("src/lib.rs", "fn a() { 2 }\n"),
        ],
    );
    let done = land_both(&queue, &processor, first, second).await;
    assert_eq!(done.status, MergeStatus::Failed);
    let detail = done.detail.unwrap_or_default();
    assert!(detail.contains("in src/lib.rs;"), "{detail}");
    assert!(
        detail.contains("CHANGELOG.md conflicted too"),
        "the generated file is named apart: {detail}"
    );
    assert!(!detail.contains("in CHANGELOG.md"), "{detail}");
    assert_eq!(
        on_main(&world, "src/lib.rs"),
        "fn a() { 1 }",
        "main keeps the first entry only"
    );

    // The aborted merge leaves the worktree ready for the next entry.
    let next = agent_pushes(&world, "aura/c", &[("c.rs", "c\n")]);
    queue.enqueue(next).await.unwrap();
    let next = processor.process_next().await.unwrap().unwrap();
    assert_eq!(next.status, MergeStatus::Merged, "{:?}", next.detail);
}

/// A crate with room for a dependency of `a` and one of `b` far enough
/// apart in its manifest that they merge cleanly.
const MANIFEST: &str = "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
[dependencies]\n# a's\n\n# -\n# -\n# -\n# -\n\n[dev-dependencies]\n# b's\n";

fn with_dependency(after: &str, name: &str) -> String {
    MANIFEST.replace(
        after,
        &format!("{after}\n{name} = {{ path = \"{name}\" }}"),
    )
}

fn path_crate(name: &str) -> [(String, String); 2] {
    [
        (
            format!("{name}/Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        ),
        (format!("{name}/src/lib.rs"), String::new()),
    ]
}

/// The agent's push adding path dependency `name` after `after`, with
/// the lockfile cargo writes for it.
fn adds_dependency(world: &World, branch: &str, after: &str, name: &str) -> MergeRequest {
    git(&world.agent, &["fetch", "-q", "origin"]);
    git(
        &world.agent,
        &["checkout", "-q", "-B", branch, "origin/main"],
    );
    write(&world.agent, "Cargo.toml", &with_dependency(after, name));
    for (file, text) in path_crate(name) {
        write(&world.agent, &file, &text);
    }
    lock(&world.agent);
    let head = commit_all(&world.agent, branch);
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

fn lock(dir: &Path) {
    let out = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--offline"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[tokio::test]
async fn a_cargo_lock_conflict_is_regenerated_from_the_merged_manifest() {
    if !has("git") || !has("cargo") {
        return;
    }
    let seed = tempfile::tempdir().unwrap();
    write(seed.path(), "Cargo.toml", MANIFEST);
    write(seed.path(), "src/lib.rs", "");
    lock(seed.path());
    let seeded_lock = std::fs::read_to_string(seed.path().join("Cargo.lock")).unwrap();
    let world = world(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", ""),
        ("Cargo.lock", &seeded_lock),
    ]);
    let (queue, processor) = queue(&world, QueueConfig::unchecked("main")).await;
    let first = adds_dependency(&world, "aura/a", "# a's", "dep_a");
    let second = adds_dependency(&world, "aura/b", "# b's", "dep_b");
    let done = land_both(&queue, &processor, first, second).await;
    assert_eq!(done.status, MergeStatus::Merged, "{:?}", done.detail);
    let detail = done.detail.unwrap_or_default();
    assert!(
        detail.contains("resolved Cargo.lock mechanically"),
        "{detail}"
    );
    let lockfile = on_main(&world, "Cargo.lock");
    assert!(
        lockfile.contains("name = \"dep_a\"") && lockfile.contains("name = \"dep_b\""),
        "{lockfile}"
    );
    assert!(!lockfile.contains("<<<<<<<"), "{lockfile}");
}
