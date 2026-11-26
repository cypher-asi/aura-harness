//! What the rows of a real repo's REPO tab open: their diffs, or why not.

use super::super::open;
use super::support::{find, viewed};
use aura_ui::repo::{EventKind, RepoTarget};
use aura_ui::UiCommand;

#[tokio::test]
async fn a_commit_and_a_change_open_as_their_diffs() {
    let (_dir, work, view) = viewed().await;
    let opened = |target: RepoTarget| open::command(&work, target, "Title");
    let second = find(&view, EventKind::Commit, "Second").target.clone();
    match opened(second).await {
        UiCommand::ShowDiff { title, files } => {
            assert_eq!(title, "Title");
            assert_eq!(files.len(), 1);
            assert!(files[0].diff.contains("-one\n+two"), "{files:?}");
        }
        other => panic!("{other:?}"),
    }
    let new = RepoTarget::Change {
        path: "new.txt".into(),
        from: None,
    };
    match opened(new).await {
        UiCommand::ShowDiff { files, .. } => assert!(files[0].diff.contains("+new")),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn moves_stashes_and_unpushed_work_open_as_diffs_but_a_fresh_worktree_has_none() {
    let (_dir, work, view) = viewed().await;
    let opened = |target: RepoTarget| open::command(&work, target, "Title");
    let reset = find(&view, EventKind::Reset, "Reset to HEAD~1");
    let stash = find(&view, EventKind::Stash, "On main: Try it");
    for target in [
        reset.target.clone(),
        stash.target.clone(),
        RepoTarget::Unpushed,
    ] {
        assert!(
            matches!(opened(target.clone()).await, UiCommand::ShowDiff { .. }),
            "{target:?}"
        );
    }
    let worktree = RepoTarget::Worktree {
        path: view.worktrees[1].path.clone(),
    };
    assert!(
        matches!(opened(worktree).await, UiCommand::ShowMessage(_)),
        "a fresh worktree has nothing to show"
    );
}

#[tokio::test]
async fn targets_that_are_not_the_repos_own_are_refused() {
    let (dir, work, _view) = viewed().await;
    let opened = |target: RepoTarget| open::command(&work, target, "Title");
    for refused in [
        RepoTarget::Commit {
            sha: "--output=x".into(),
        },
        RepoTarget::Stash {
            name: "HEAD".into(),
        },
        RepoTarget::Branch {
            name: "nope".into(),
        },
        RepoTarget::Change {
            path: "../outside".into(),
            from: None,
        },
        RepoTarget::Worktree {
            path: dir.path().to_path_buf(),
        },
    ] {
        assert!(
            matches!(opened(refused.clone()).await, UiCommand::ShowWarning(_)),
            "{refused:?}"
        );
    }
    assert!(!work.join("x").exists());
}
