//! `git_ship`'s changelog guard against a real origin.

use super::*;
use crate::git_tool::{git_ship_impl, ShipRequest};

fn request(message: &str) -> ShipRequest<'_> {
    ShipRequest {
        message,
        target: "main",
        push: PushTarget::default(),
        timeout: Duration::from_secs(20),
        push_policy: PushPolicy::single(Duration::from_secs(20)),
        task: None,
    }
}

/// [`repo_with_bare_origin`] whose `main` on origin has a CHANGELOG.md,
/// and a `changelog.d/` folder when `fragments` is set.
async fn repo_with_changelog(fragments: bool) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let (parent, repo, origin) = repo_with_bare_origin().await;
    std::fs::write(
        repo.join("CHANGELOG.md"),
        "# Changelog\n\n## [Unreleased]\n",
    )
    .unwrap();
    if fragments {
        std::fs::create_dir(repo.join("changelog.d")).unwrap();
        std::fs::write(repo.join("changelog.d/README.md"), "How\n").unwrap();
        std::fs::write(repo.join("changelog.d/old.md"), "### Old\n").unwrap();
    }
    git(&repo, &["add", "-A"]).await;
    git(&repo, &["commit", "-qm", "changelog"]).await;
    git(&repo, &["push", "-q", "origin", "HEAD:refs/heads/main"]).await;
    (parent, repo, origin)
}

fn edit_changelog(repo: &Path) {
    std::fs::write(
        repo.join("CHANGELOG.md"),
        "# Changelog\n\n## [Unreleased]\n\n### Mine\n",
    )
    .unwrap();
}

async fn origin_has_work(origin: &Path) -> bool {
    let out = Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", "refs/heads/work"])
        .current_dir(origin)
        .output()
        .await
        .unwrap();
    out.status.success()
}

#[tokio::test]
async fn an_edit_to_the_changelog_is_not_pushed_where_fragments_are_kept() {
    if !git_available() {
        return;
    }
    let (_parent, repo, origin) = repo_with_changelog(true).await;
    edit_changelog(&repo);
    let err = git_ship_impl(&repo, request("mine")).await.unwrap_err();
    match &err {
        GitToolError::ChangelogEdited { fragment } => {
            assert_eq!(fragment, "changelog.d/work.md");
        }
        other => panic!("expected ChangelogEdited, got {other:?}"),
    }
    assert!(err.to_string().contains("changelog.d/work.md"), "{err}");
    assert!(!origin_has_work(&origin).await, "nothing is pushed");
}

#[tokio::test]
async fn a_fragment_ships() {
    if !git_available() {
        return;
    }
    let (_parent, repo, origin) = repo_with_changelog(true).await;
    std::fs::write(repo.join("changelog.d/work.md"), "### Mine\n").unwrap();
    git_ship_impl(&repo, request("mine")).await.unwrap();
    assert!(origin_has_work(&origin).await);
}

#[tokio::test]
async fn a_release_that_deletes_fragments_ships() {
    if !git_available() {
        return;
    }
    let (_parent, repo, origin) = repo_with_changelog(true).await;
    edit_changelog(&repo);
    std::fs::remove_file(repo.join("changelog.d/old.md")).unwrap();
    git_ship_impl(&repo, request("release")).await.unwrap();
    assert!(origin_has_work(&origin).await);
}

#[tokio::test]
async fn a_repo_without_fragments_ships_changelog_edits() {
    if !git_available() {
        return;
    }
    let (_parent, repo, origin) = repo_with_changelog(false).await;
    edit_changelog(&repo);
    git_ship_impl(&repo, request("mine")).await.unwrap();
    assert!(origin_has_work(&origin).await);
}
