//! A real repo read into the REPO tab's view.

use super::super::load;
use super::support::{agent, find, viewed};
use aura_core_types::AgentId;
use aura_ui::repo::{ChangeKind, EventKind, EventState, PrState};

#[tokio::test]
async fn a_repo_reads_as_its_branch_and_its_changes() {
    let (_dir, _work, view) = viewed().await;
    assert_eq!(view.problem, None);
    assert_eq!(view.agent_id, agent().to_hex());
    assert_eq!(view.branch.as_deref(), Some("main"));
    assert_eq!(view.upstream.as_deref(), Some("origin/main"));
    assert_eq!(
        (view.ahead, view.behind),
        (3, 0),
        "Third, Feature and the merge"
    );
    assert!(!view.worktree);
    assert_eq!(view.prs, PrState::NotGitHub);
    let changes: Vec<(&str, ChangeKind)> = view
        .changes
        .iter()
        .map(|c| (c.path.as_str(), c.kind))
        .collect();
    assert_eq!(
        changes,
        [
            ("a.txt", ChangeKind::Modified),
            ("new.txt", ChangeKind::Untracked)
        ]
    );
}

#[tokio::test]
async fn a_repos_timeline_shows_what_happened_in_it_newest_first() {
    let (_dir, _work, view) = viewed().await;
    assert_eq!(
        find(&view, EventKind::Commit, "Third").state,
        EventState::Pending
    );
    assert_eq!(
        find(&view, EventKind::Merge, "Merge feature").state,
        EventState::Pending
    );
    assert_eq!(
        find(&view, EventKind::Commit, "Second").state,
        EventState::Done
    );
    find(&view, EventKind::Push, "Pushed 1 commit to origin/main");
    find(&view, EventKind::Push, "Pushed new branch origin/main");
    find(&view, EventKind::Checkout, "Switched from main to feature");
    find(&view, EventKind::Checkout, "Switched from feature to main");
    find(&view, EventKind::Reset, "Reset to HEAD~1");
    let stash = find(&view, EventKind::Stash, "On main: Try it");
    assert_eq!(stash.detail, "stash@{0}");
    assert_eq!(
        view.events
            .iter()
            .filter(|e| e.kind == EventKind::Merge)
            .count(),
        1,
        "the merge's reflog entry is its commit's"
    );
    let times: Vec<i64> = view.events.iter().map(|e| e.at).collect();
    assert!(
        times.windows(2).all(|w| w[0] >= w[1]),
        "newest first: {times:?}"
    );
    assert_eq!(view.span_start, None, "every commit was read");
}

#[tokio::test]
async fn a_repo_lists_its_branches_and_worktrees() {
    let (_dir, _work, view) = viewed().await;
    let names: Vec<(&str, bool)> = view
        .branches
        .iter()
        .map(|b| (b.name.as_str(), b.current))
        .collect();
    assert!(
        names.contains(&("main", true)) && names.contains(&("feature", false)),
        "{names:?}"
    );
    assert_eq!(view.worktrees.len(), 2);
    assert!(view.worktrees[0].current && !view.worktrees[1].current);
    assert_eq!(view.worktrees[1].branch.as_deref(), Some("side"));
}

#[tokio::test]
async fn a_folder_outside_any_repo_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let (view, _) = load::view(dir.path(), AgentId::new([4; 32]), false).await;
    assert_eq!(view.problem.as_deref(), Some("Not a git repository."));
}
