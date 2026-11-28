//! One branch per task: `git_start_task`, the pin that lets a machine
//! push only its own agent's branches, and follow-up work after a merge
//! moving to a new branch.

use super::*;
use crate::git_tool::{git_ship_impl, start_task_branch, ShipRequest};

const PIN: &str = "aura/first-task-rus207";

/// A repo whose origin has `main`, checked out on `main`.
async fn on_main() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let (parent, repo, origin) = repo_with_bare_origin().await;
    git(&repo, &["push", "-q", "origin", "HEAD:refs/heads/main"]).await;
    git(&repo, &["checkout", "-q", "-B", "main", "origin/main"]).await;
    (parent, repo, origin)
}

fn pinned_ship(message: &str) -> ShipRequest<'_> {
    ShipRequest {
        message,
        target: "main",
        push: PushTarget {
            only_branch: Some(PIN),
            ..PushTarget::default()
        },
        timeout: Duration::from_secs(20),
        push_policy: PushPolicy::single(Duration::from_secs(20)),
        task: None,
    }
}

async fn on_origin(origin: &Path, branch: &str) -> bool {
    let out = tokio::process::Command::new("git")
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .current_dir(origin)
        .output()
        .await
        .unwrap();
    out.status.success()
}

#[tokio::test]
async fn two_tasks_from_one_agent_ship_on_separate_fresh_branches() {
    if !git_available() {
        return;
    }
    let (_parent, repo, origin) = on_main().await;
    let timeout = Duration::from_secs(20);
    let first = start_task_branch(&repo, PIN, "main", timeout)
        .await
        .unwrap();
    assert_eq!(first.branch, PIN);
    assert_eq!(first.from, "origin/main");
    std::fs::write(repo.join("one.txt"), "1").unwrap();
    git_ship_impl(&repo, pinned_ship("task one")).await.unwrap();

    let second = start_task_branch(&repo, "aura/second-task-rus207", "main", timeout)
        .await
        .unwrap();
    assert_eq!(second.branch, "aura/second-task-rus207");
    assert!(
        !repo.join("one.txt").exists(),
        "the second starts from main, not the first"
    );
    std::fs::write(repo.join("two.txt"), "2").unwrap();
    let shipped = git_ship_impl(&repo, pinned_ship("task two")).await.unwrap();

    assert_eq!(shipped.branch, "aura/second-task-rus207");
    assert!(on_origin(&origin, PIN).await && on_origin(&origin, &shipped.branch).await);
    let files = git(
        &origin,
        &[
            "ls-tree",
            "--name-only",
            "refs/heads/aura/second-task-rus207",
        ],
    )
    .await;
    assert!(
        files.contains("two.txt") && !files.contains("one.txt"),
        "{files}"
    );
}

#[tokio::test]
async fn a_taken_name_is_never_reused() {
    if !git_available() {
        return;
    }
    let (_parent, repo, _origin) = on_main().await;
    let timeout = Duration::from_secs(20);
    start_task_branch(&repo, PIN, "main", timeout)
        .await
        .unwrap();
    std::fs::write(repo.join("one.txt"), "1").unwrap();
    git_ship_impl(&repo, pinned_ship("task one")).await.unwrap();
    let again = start_task_branch(&repo, PIN, "main", timeout)
        .await
        .unwrap();
    assert_eq!(again.branch, "aura/first-task-2-rus207");
}

#[tokio::test]
async fn a_pinned_machine_is_refused_another_agents_branch_and_main() {
    if !git_available() {
        return;
    }
    let (_parent, repo, origin) = on_main().await;
    for refused in ["aura/their-work-rus204", "main", "feature/x-rus207"] {
        git(&repo, &["checkout", "-q", "-B", refused, "origin/main"]).await;
        std::fs::write(repo.join("x.txt"), refused).unwrap();
        let shipped = git_ship_impl(&repo, pinned_ship("not mine")).await;
        assert!(
            matches!(
                shipped,
                Err(GitToolError::BranchNotAllowed { .. } | GitToolError::ProtectedBranch { .. })
            ),
            "{refused}: {shipped:?}"
        );
        if refused != "main" {
            assert!(
                !on_origin(&origin, refused).await,
                "{refused} never reached origin"
            );
        }
        git(&repo, &["checkout", "-q", "--", "."]).await;
        let _ = std::fs::remove_file(repo.join("x.txt"));
    }
}

#[tokio::test]
async fn starting_a_task_refuses_to_leave_work_behind() {
    if !git_available() {
        return;
    }
    let (_parent, repo, _origin) = on_main().await;
    let timeout = Duration::from_secs(20);
    start_task_branch(&repo, PIN, "main", timeout)
        .await
        .unwrap();
    std::fs::write(repo.join("draft.txt"), "wip").unwrap();
    let dirty = start_task_branch(&repo, "aura/next-rus207", "main", timeout).await;
    assert!(
        matches!(&dirty, Err(GitToolError::WorkLeftBehind(why)) if why.contains("draft.txt")),
        "{dirty:?}"
    );
    git(&repo, &["add", "-A"]).await;
    git(&repo, &["commit", "-qm", "local only"]).await;
    let unpushed = start_task_branch(&repo, "aura/next-rus207", "main", timeout).await;
    assert!(
        matches!(&unpushed, Err(GitToolError::WorkLeftBehind(why)) if why.contains("local only")),
        "{unpushed:?}"
    );
}

#[tokio::test]
async fn follow_up_work_after_a_merge_ships_on_a_new_branch() {
    if !git_available() {
        return;
    }
    let (_parent, repo, origin) = on_main().await;
    let timeout = Duration::from_secs(20);
    start_task_branch(&repo, PIN, "main", timeout)
        .await
        .unwrap();
    std::fs::write(repo.join("one.txt"), "1").unwrap();
    let shipped = git_ship_impl(&repo, pinned_ship("task one")).await.unwrap();
    // The queue merged it and deleted the branch.
    git(&origin, &["update-ref", "refs/heads/main", &shipped.head]).await;
    git(&origin, &["update-ref", "-d", &format!("refs/heads/{PIN}")]).await;

    std::fs::write(repo.join("more.txt"), "more").unwrap();
    let request = ShipRequest {
        task: Some("polish the first task"),
        ..pinned_ship("more work")
    };
    let follow_up = git_ship_impl(&repo, request).await.unwrap();

    assert_eq!(follow_up.branch, "aura/polish-the-first-task-rus207");
    assert!(
        !on_origin(&origin, PIN).await,
        "the merged branch is not pushed again"
    );
    assert!(on_origin(&origin, &follow_up.branch).await);
}

#[tokio::test]
async fn a_commit_list_that_cant_be_read_refuses_to_start() {
    if !git_available() {
        return;
    }
    let (_parent, repo, _origin) = on_main().await;
    // An unborn branch: `rev-list HEAD` fails.
    git(&repo, &["checkout", "-q", "--orphan", "empty"]).await;
    git(&repo, &["rm", "-rqf", "."]).await;
    let started =
        start_task_branch(&repo, "aura/next-rus207", "main", Duration::from_secs(20)).await;
    assert!(
        matches!(&started, Err(GitToolError::WorkLeftBehind(why)) if why.contains("could not tell")),
        "{started:?}"
    );
}

#[tokio::test]
async fn a_follow_up_name_the_pin_refuses_is_never_checked_out() {
    if !git_available() {
        return;
    }
    let (_parent, repo, origin) = on_main().await;
    let timeout = Duration::from_secs(20);
    start_task_branch(&repo, PIN, "main", timeout)
        .await
        .unwrap();
    std::fs::write(repo.join("one.txt"), "1").unwrap();
    let shipped = git_ship_impl(&repo, pinned_ship("task one")).await.unwrap();
    git(&origin, &["update-ref", "refs/heads/main", &shipped.head]).await;
    std::fs::write(repo.join("more.txt"), "more").unwrap();
    git(&repo, &["add", "-A"]).await;
    git(&repo, &["commit", "-qm", "more"]).await;
    let refuse = |name: &str| {
        Err(GitToolError::BranchNotAllowed {
            branch: name.to_string(),
            allowed: PIN.to_string(),
        })
    };
    let moved = crate::git_tool::branches::follow_up_branch(
        &repo,
        PIN,
        "main",
        Some("polish"),
        refuse,
        timeout,
    )
    .await;
    assert!(
        matches!(moved, Err(GitToolError::BranchNotAllowed { .. })),
        "{moved:?}"
    );
    let current = git(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]).await;
    assert_eq!(current, PIN, "still on its branch");
    let branches = git(&repo, &["branch", "--list", "aura/polish*"]).await;
    assert!(branches.is_empty(), "no branch made: {branches}");
}
