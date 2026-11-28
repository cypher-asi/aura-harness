//! `git_ship`: commit, pull the target, push, and the merge queue marker.

use super::*;
use crate::git_tool::{git_ship_impl, ShipRequest, PROTECTED_BRANCHES};
use aura_exec_merge_queue::QueuedPush;

fn request(message: &str) -> ShipRequest<'_> {
    ShipRequest {
        message,
        target: "main",
        push: PushTarget::default(),
        timeout: Duration::from_secs(20),
        push_policy: PushPolicy::single(Duration::from_secs(20)),
    }
}

/// [`repo_with_bare_origin`] whose origin also has a `main` that another
/// clone moved on with `file` = `text` after `work` branched from it.
async fn origin_with_main_ahead(
    file: &str,
    text: &str,
) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let (parent, repo, origin) = repo_with_bare_origin().await;
    git(&repo, &["push", "-q", "origin", "HEAD:refs/heads/main"]).await;
    let other = parent.path().join("other");
    git(
        parent.path(),
        &[
            "clone",
            "-q",
            "-b",
            "main",
            &origin.to_string_lossy(),
            "other",
        ],
    )
    .await;
    for (k, v) in [("user.email", "o@example.com"), ("user.name", "Other")] {
        git(&other, &["config", k, v]).await;
    }
    std::fs::write(other.join(file), text).unwrap();
    git(&other, &["add", "-A"]).await;
    git(&other, &["commit", "-qm", "main moves"]).await;
    git(&other, &["push", "-q", "origin", "main"]).await;
    (parent, repo, origin)
}

#[tokio::test]
async fn ship_commits_pulls_main_pushes_and_queues() {
    if !git_available() {
        return;
    }
    let (_parent, repo, origin) = origin_with_main_ahead("m.txt", "main\n").await;
    std::fs::write(repo.join("b.txt"), b"b").unwrap();
    let shipped = git_ship_impl(&repo, request("my task")).await.unwrap();
    assert!(shipped.commit.is_some());
    assert_eq!(shipped.pulled, "merged origin/main");
    assert_eq!(shipped.branch, "work");
    let pushed = git(&origin, &["rev-parse", "refs/heads/work"]).await;
    assert_eq!(pushed, shipped.head);
    let files = git(&origin, &["ls-tree", "--name-only", "refs/heads/work"]).await;
    assert!(
        files.contains("b.txt") && files.contains("m.txt"),
        "{files}"
    );
    let output = shipped.to_json().to_string();
    let marker = QueuedPush::from_tool_output(&output).unwrap();
    assert_eq!(
        marker,
        QueuedPush {
            branch: "work".to_string(),
            head: shipped.head.clone(),
            remote: Some("origin".to_string()),
        }
    );
}

#[tokio::test]
async fn ship_with_nothing_to_commit_still_pushes_the_branch() {
    if !git_available() {
        return;
    }
    let (_parent, repo, origin) = repo_with_bare_origin().await;
    let shipped = git_ship_impl(&repo, request("nothing new")).await.unwrap();
    assert_eq!(shipped.commit, None);
    assert!(shipped.pulled.starts_with("skipped"), "{}", shipped.pulled);
    let head = git(&repo, &["rev-parse", "HEAD"]).await;
    assert_eq!(git(&origin, &["rev-parse", "refs/heads/work"]).await, head);
}

#[tokio::test]
async fn a_conflicting_pull_names_the_files_and_leaves_no_merge_open() {
    if !git_available() {
        return;
    }
    let (_parent, repo, origin) = origin_with_main_ahead("a.txt", "theirs\n").await;
    std::fs::write(repo.join("a.txt"), b"mine\n").unwrap();
    let err = git_ship_impl(&repo, request("clash")).await.unwrap_err();
    match &err {
        GitToolError::MergeConflict { target, files } => {
            assert_eq!(target, "main");
            assert_eq!(files, &["a.txt".to_string()]);
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
    assert!(
        !repo.join(".git/MERGE_HEAD").exists(),
        "the merge was aborted"
    );
    assert_eq!(git(&repo, &["status", "--porcelain"]).await, "");
    let pushed = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", "refs/heads/work"])
        .current_dir(&origin)
        .output()
        .unwrap();
    assert!(!pushed.status.success(), "nothing was pushed");
}

#[tokio::test]
async fn shipping_from_a_protected_branch_is_refused_before_committing() {
    if !git_available() {
        return;
    }
    let (_parent, repo, _origin) = repo_with_bare_origin().await;
    git(&repo, &["checkout", "-q", "-b", "main"]).await;
    std::fs::write(repo.join("c.txt"), b"c").unwrap();
    let err = git_ship_impl(&repo, request("on main")).await.unwrap_err();
    assert!(
        matches!(&err, GitToolError::ProtectedBranch { branch } if branch == "main"),
        "{err:?}"
    );
    assert!(err.to_string().contains("merge queue"));
    assert!(!git(&repo, &["status", "--porcelain"]).await.is_empty());
}

#[tokio::test]
async fn pushes_never_update_a_protected_branch() {
    if !git_available() {
        return;
    }
    let (_parent, repo, _origin) = repo_with_bare_origin().await;
    for branch in PROTECTED_BRANCHES {
        let err = git_push_impl(
            &repo,
            to_branch(branch),
            PushPolicy::single(Duration::from_secs(5)),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, GitToolError::ProtectedBranch { .. }),
            "{branch}: {err:?}"
        );
    }
}

#[tokio::test]
async fn the_queue_target_a_repo_configures_is_protected_too() {
    if !git_available() {
        return;
    }
    let (_parent, repo, _origin) = repo_with_bare_origin().await;
    std::fs::create_dir_all(repo.join(".aura")).unwrap();
    std::fs::write(repo.join(".aura/merge-queue.toml"), "target = \"trunk\"\n").unwrap();
    let err = git_push_impl(
        &repo,
        to_branch("trunk"),
        PushPolicy::single(Duration::from_secs(5)),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, GitToolError::ProtectedBranch { branch } if branch == "trunk"),
        "{err:?}"
    );
    git(&repo, &["checkout", "-q", "-b", "trunk"]).await;
    let err = git_ship_impl(&repo, request("on trunk")).await.unwrap_err();
    assert!(
        matches!(err, GitToolError::ProtectedBranch { .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_repo_without_origin_queues_its_local_branch() {
    if !git_available() {
        return;
    }
    let dir = TempDir::new().unwrap();
    init_repo(dir.path()).await;
    std::fs::write(dir.path().join("seed.txt"), b"s").unwrap();
    git_commit_impl(dir.path(), "seed", Duration::from_secs(10))
        .await
        .unwrap();
    git(dir.path(), &["checkout", "-q", "-b", "aura/local"]).await;
    std::fs::write(dir.path().join("l.txt"), b"l").unwrap();
    let shipped = git_ship_impl(dir.path(), request("local")).await.unwrap();
    let marker = QueuedPush::from_tool_output(&shipped.to_json().to_string()).unwrap();
    assert_eq!(marker.branch, "aura/local");
    assert_eq!(marker.remote, None);
    assert_eq!(marker.head, git(dir.path(), &["rev-parse", "HEAD"]).await);
}

#[tokio::test]
async fn a_push_result_carries_the_queue_marker() {
    if !git_available() {
        return;
    }
    let (_parent, repo, _origin) = repo_with_bare_origin().await;
    let sandbox = crate::sandbox::Sandbox::new(&repo).unwrap();
    let ctx = ToolContext::new(sandbox, crate::ToolConfig::default());
    let result = GitPushTool
        .execute(&ctx, serde_json::json!({}))
        .await
        .unwrap();
    let output = String::from_utf8_lossy(&result.stdout).to_string();
    let marker = QueuedPush::from_tool_output(&output).unwrap();
    assert_eq!(marker.branch, "work");
    assert_eq!(marker.head, git(&repo, &["rev-parse", "HEAD"]).await);
}

/// [`repo_with_bare_origin`] whose `main`, on origin and fetched into the
/// repo, has a committed queue config naming `trunk`, and whose origin
/// has a `trunk` that moved on with `t.txt`.
async fn repo_targeting_trunk() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let (parent, repo, origin) = repo_with_bare_origin().await;
    git(&repo, &["checkout", "-q", "-b", "main"]).await;
    std::fs::create_dir_all(repo.join(".aura")).unwrap();
    std::fs::write(repo.join(".aura/merge-queue.toml"), "target = \"trunk\"\n").unwrap();
    git(&repo, &["add", "-A"]).await;
    git(&repo, &["commit", "-qm", "queue into trunk"]).await;
    git(&repo, &["push", "-q", "origin", "main", "main:trunk"]).await;
    git(&repo, &["checkout", "-q", "-b", "trunk-ahead"]).await;
    std::fs::write(repo.join("t.txt"), b"t").unwrap();
    git(&repo, &["add", "-A"]).await;
    git(&repo, &["commit", "-qm", "trunk moves"]).await;
    git(&repo, &["push", "-q", "origin", "trunk-ahead:trunk"]).await;
    git(&repo, &["checkout", "-q", "work"]).await;
    git(&repo, &["merge", "-q", "main"]).await;
    (parent, repo, origin)
}

#[tokio::test]
async fn ship_without_a_target_pulls_the_committed_queue_target() {
    if !git_available() {
        return;
    }
    let (_parent, repo, origin) = repo_targeting_trunk().await;
    std::fs::write(repo.join("b.txt"), b"b").unwrap();
    let sandbox = crate::sandbox::Sandbox::new(&repo).unwrap();
    let ctx = ToolContext::new(sandbox, crate::ToolConfig::default());
    let result = crate::git_tool::GitShipTool
        .execute(&ctx, serde_json::json!({"message": "ship to trunk"}))
        .await
        .unwrap();
    let output = String::from_utf8_lossy(&result.stdout).to_string();
    assert!(output.contains("origin/trunk"), "{output}");
    let files = git(&origin, &["ls-tree", "--name-only", "refs/heads/work"]).await;
    assert!(files.contains("t.txt"), "trunk was pulled: {files}");
}

#[tokio::test]
async fn the_committed_queue_target_is_protected_without_a_working_config() {
    if !git_available() {
        return;
    }
    let (_parent, repo, _origin) = repo_targeting_trunk().await;
    std::fs::remove_file(repo.join(".aura/merge-queue.toml")).unwrap();
    let err = git_push_impl(
        &repo,
        to_branch("trunk"),
        PushPolicy::single(Duration::from_secs(5)),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, GitToolError::ProtectedBranch { branch } if branch == "trunk"),
        "{err:?}"
    );
}

#[tokio::test]
async fn ship_takes_the_merge_the_queue_put_on_its_branch_before_pushing() {
    if !git_available() {
        return;
    }
    let (parent, repo, origin) = origin_with_main_ahead("m.txt", "main\n").await;
    std::fs::write(repo.join("a.txt"), b"a").unwrap();
    git_ship_impl(&repo, request("first try")).await.unwrap();
    // The queue tested a merge and put it on the PR's branch.
    let queue = parent.path().join("queue");
    let url = origin.to_string_lossy().to_string();
    git(parent.path(), &["clone", "-q", "-b", "work", &url, "queue"]).await;
    for (k, v) in [("user.email", "q@example.com"), ("user.name", "Queue")] {
        git(&queue, &["config", k, v]).await;
    }
    std::fs::write(queue.join("q.txt"), "queue\n").unwrap();
    git(&queue, &["add", "-A"]).await;
    git(&queue, &["commit", "-qm", "merge through the queue"]).await;
    git(&queue, &["push", "-q", "origin", "work"]).await;

    std::fs::write(repo.join("fix.txt"), b"fix").unwrap();
    let shipped = git_ship_impl(&repo, request("the fix")).await.unwrap();

    assert!(
        shipped.pulled.starts_with("merged origin/work"),
        "{}",
        shipped.pulled
    );
    let files = git(&origin, &["ls-tree", "--name-only", "refs/heads/work"]).await;
    assert!(
        files.contains("q.txt") && files.contains("fix.txt"),
        "{files}"
    );
}
