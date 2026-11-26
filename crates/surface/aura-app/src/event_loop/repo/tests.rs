use super::github::{self, Failure, Pr, PrStatus, Slug, Source};
use super::parse::{self, Entry, HeadMove, Moved, RemoteMove};
use super::{load, lock, open, prs, CACHE, PRS_FOR};
use async_trait::async_trait;
use aura_core_types::AgentId;
use aura_ui_terminal::repo::{
    ChangeKind, EventKind, EventState, PrState, RepoEvent, RepoTarget, RepoView,
};
use aura_ui_terminal::UiCommand;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

#[test]
fn status_reads_the_branch_and_its_changes() {
    let out = [
        "# branch.oid 9984d116aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "# branch.head main",
        "# branch.upstream origin/main",
        "# branch.ab +2 -1",
        "1 .M N... 100644 100644 100644 aaaa bbbb src/main.rs",
        "1 A. N... 000000 100644 100644 0000 cccc new file.rs",
        "1 D. N... 100644 000000 000000 dddd 0000 gone.rs",
        "2 R. N... 100644 100644 100644 eeee eeee R100 lib/new.rs",
        "lib/old.rs",
        "u UU N... 100644 100644 100644 100644 ffff ffff ffff both.rs",
        "? notes/",
        "",
    ]
    .join("\0");
    let status = parse::status(&out);
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(status.upstream.as_deref(), Some("origin/main"));
    assert_eq!((status.ahead, status.behind), (2, 1));
    let changes: Vec<(&str, ChangeKind, bool, bool)> = status
        .changes
        .iter()
        .map(|c| (c.path.as_str(), c.kind, c.staged, c.unstaged))
        .collect();
    assert_eq!(
        changes,
        [
            ("src/main.rs", ChangeKind::Modified, false, true),
            ("new file.rs", ChangeKind::Added, true, false),
            ("gone.rs", ChangeKind::Deleted, true, false),
            ("lib/new.rs", ChangeKind::Renamed, true, false),
            ("both.rs", ChangeKind::Conflicted, false, true),
            ("notes/", ChangeKind::Untracked, false, true),
        ]
    );
    assert_eq!(status.changes[3].from.as_deref(), Some("lib/old.rs"));

    let empty = parse::status("# branch.oid (initial)\0# branch.head (detached)\0");
    assert_eq!((empty.oid, empty.branch), (None, None));
}

#[test]
fn commits_keep_their_refs_but_head() {
    let out = "9984d116aa\x1f9984d116\x1fa b\x1f1795665123\x1fHEAD -> main, origin/main, tag: v1, origin/HEAD\x1fMerge it\0\
               f5bc5cd4aa\x1ff5bc5cd4\x1fa\x1f1795665000\x1f\x1fFirst\0";
    let commits = parse::commits(out);
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].parents, 2);
    assert_eq!(commits[0].refs, ["main", "origin/main", "v1"]);
    assert_eq!(
        (commits[1].at, commits[1].subject.as_str()),
        (1_795_665_000, "First")
    );
    assert!(commits[1].refs.is_empty());
}

fn entry(sha: &str, at: i64, message: &str) -> Entry {
    Entry {
        sha: sha.into(),
        at,
        message: message.into(),
    }
}

#[test]
fn head_moves_fold_a_rebase_and_leave_commits_to_the_log() {
    let out = "ffff\x1fHEAD@{1795663637}\x1fmerge channel-projects: Fast-forward\0";
    assert_eq!(
        parse::reflog(out),
        [entry(
            "ffff",
            1_795_663_637,
            "merge channel-projects: Fast-forward"
        )]
    );

    let entries = [
        entry("e", 90, "reset: moving to HEAD~1"),
        entry("d", 80, "rebase (finish): returning to refs/heads/main"),
        entry("d", 70, "rebase (pick): Second"),
        entry("c", 60, "rebase (start): checkout origin/main"),
        entry("b", 50, "commit (amend): First"),
        entry("a", 40, "checkout: moving from feature to main"),
        entry("a", 30, "reset: moving to HEAD"),
        entry("a", 20, "pull --ff-only: Fast-forward"),
        entry("z", 10, "commit (initial): Start"),
    ];
    let moves: Vec<(HeadMove, Option<String>, String)> = parse::head_moves(&entries)
        .into_iter()
        .map(|m| (m.what, m.from, m.to))
        .collect();
    let moved =
        |what: HeadMove, from: &str, to: &str| (what, Some(from.to_string()), to.to_string());
    assert_eq!(
        moves,
        [
            moved(
                HeadMove::Reset {
                    to: "HEAD~1".into()
                },
                "d",
                "e"
            ),
            moved(
                HeadMove::Rebase {
                    branch: Some("main".into()),
                    onto: Some("origin/main".into()),
                },
                "b",
                "d"
            ),
            moved(
                HeadMove::Checkout {
                    from: "feature".into(),
                    to: "main".into(),
                },
                "a",
                "a"
            ),
            moved(HeadMove::Pull, "z", "a"),
        ],
        "the amend is a commit, and the reset to where HEAD was moved nothing"
    );
}

#[test]
fn merges_move_head_with_a_commit_or_without() {
    let entries = [
        entry("c", 30, "merge feature: Merge made by the 'ort' strategy."),
        entry("b", 20, "merge fix: Fast-forward"),
        entry("a", 10, "commit (initial): Start"),
    ];
    let moves: Vec<(HeadMove, Option<String>, String)> = parse::head_moves(&entries)
        .into_iter()
        .map(|m| (m.what, m.from, m.to))
        .collect();
    let merged = |branch: &str, from: &str, to: &str| {
        let what = HeadMove::Merge {
            branch: branch.into(),
        };
        (what, Some(from.to_string()), to.to_string())
    };
    assert_eq!(
        moves,
        [merged("feature", "b", "c"), merged("fix", "a", "b")]
    );
}

#[test]
fn remote_moves_are_pushes_and_fetches() {
    let entries = [
        entry("c", 30, "update by push"),
        entry("b", 20, "fetch origin: fast-forward"),
        entry("a", 10, "pull: fast-forward"),
    ];
    let whats = |moves: Vec<Moved<RemoteMove>>| -> Vec<(RemoteMove, Option<String>)> {
        moves.into_iter().map(|m| (m.what, m.from)).collect()
    };
    assert_eq!(
        whats(parse::remote_moves(&entries, true)),
        [
            (RemoteMove::Push, Some("b".into())),
            (RemoteMove::Fetch, Some("a".into())),
            (RemoteMove::Pull, None),
        ],
        "a whole reflog's oldest entry made the branch"
    );
    assert_eq!(
        parse::remote_moves(&entries, false).len(),
        2,
        "a cut one's started before it"
    );
}

#[test]
fn refs_read_their_tracking() {
    let out = "refs/heads/main\x1f9984d116aa\x1f9984d116\x1forigin/main\x1fahead 10, behind 2\x1forigin\x1f1795665123\x1f*\x1fSplit it\n\
               refs/heads/old\x1f1111\x1f1111\x1forigin/old\x1fgone\x1forigin\x1f1795000000\x1f \x1fOld\n";
    let refs = parse::refs(out);
    assert_eq!(refs.len(), 2);
    assert!(refs[0].head && !refs[1].head);
    assert_eq!((refs[0].ahead, refs[0].behind), (10, 2));
    assert_eq!(refs[0].remote.as_deref(), Some("origin"));
    assert!(refs[1].gone);
}

#[test]
fn worktrees_and_stashes_read_as_listed() {
    let worktrees = parse::worktrees(
        "worktree /src/aura\nHEAD 9984d116aa\nbranch refs/heads/main\n\n\
         worktree /src/aura-wt\nHEAD f5bc5cd4aa\ndetached\nlocked\nprunable gitdir file points to non-existent location\n\n",
    );
    assert_eq!(worktrees.len(), 2);
    assert_eq!(worktrees[0].branch.as_deref(), Some("main"));
    assert!(worktrees[1].branch.is_none() && worktrees[1].locked && worktrees[1].prunable);

    let stashes = parse::stashes("stash@{0}\x1f1795665123\x1fOn main: try it\0stash@{1}\x1f1795665000\x1fWIP on main: 9984d11 x\0");
    assert_eq!(stashes.len(), 2);
    assert_eq!(
        (stashes[1].name.as_str(), stashes[1].at),
        ("stash@{1}", 1_795_665_000)
    );
    assert!(parse::is_stash("stash@{12}"));
    assert!(!parse::is_stash("stash@{-1}") && !parse::is_stash("HEAD@{0}"));
}

/// `git <args>` in `dir` at Unix time `at`, with no config but the repo's.
fn git_at(dir: &Path, at: i64, args: &[&str]) -> String {
    let date = format!("@{at} +0000");
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
        .env("GIT_AUTHOR_NAME", "A")
        .env("GIT_AUTHOR_EMAIL", "a@example.com")
        .env("GIT_COMMITTER_NAME", "A")
        .env("GIT_COMMITTER_EMAIL", "a@example.com")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn commit(dir: &Path, at: i64, file: &str, text: &str, subject: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    git_at(dir, at, &["add", file]);
    git_at(dir, at, &["commit", "-q", "-m", subject]);
}

fn find<'a>(view: &'a RepoView, kind: EventKind, title: &str) -> &'a RepoEvent {
    view.events
        .iter()
        .find(|e| e.kind == kind && e.title == title)
        .unwrap_or_else(|| panic!("no {kind:?} {title:?} in {:#?}", view.events))
}

#[tokio::test]
async fn a_repo_reads_as_its_timeline_and_opens_its_diffs() {
    let dir = tempfile::tempdir().unwrap();
    let (origin, work) = (dir.path().join("origin.git"), dir.path().join("work"));
    std::fs::create_dir_all(&work).unwrap();
    git_at(
        dir.path(),
        1000,
        &["init", "-q", "--bare", "-b", "main", "origin.git"],
    );
    git_at(&work, 1000, &["init", "-q", "-b", "main"]);
    git_at(
        &work,
        1000,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    commit(&work, 1001, "a.txt", "one\n", "First");
    git_at(&work, 1002, &["push", "-q", "-u", "origin", "main"]);
    commit(&work, 1003, "a.txt", "two\n", "Second");
    git_at(&work, 1004, &["push", "-q"]);
    commit(&work, 1005, "a.txt", "three\n", "Third");
    git_at(&work, 1006, &["checkout", "-q", "-b", "feature"]);
    commit(&work, 1007, "b.txt", "feature\n", "Feature");
    git_at(&work, 1008, &["checkout", "-q", "main"]);
    git_at(
        &work,
        1009,
        &["merge", "-q", "--no-ff", "feature", "-m", "Merge feature"],
    );
    commit(&work, 1010, "a.txt", "oops\n", "Oops");
    git_at(&work, 1011, &["reset", "-q", "--hard", "HEAD~1"]);
    std::fs::write(work.join("a.txt"), "stashed\n").unwrap();
    git_at(&work, 1012, &["stash", "push", "-q", "-m", "Try it"]);
    let linked = dir.path().join("linked");
    git_at(
        &work,
        1013,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            linked.to_str().unwrap(),
        ],
    );
    std::fs::write(work.join("a.txt"), "changed\n").unwrap();
    std::fs::write(work.join("new.txt"), "new\n").unwrap();

    let agent = AgentId::new([3; 32]);
    let (view, reading) = load::view(&work, agent, false).await;
    assert!(reading.is_none(), "no GitHub, no PRs to read");
    assert_eq!(view.problem, None);
    assert_eq!(view.agent_id, agent.to_hex());
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
    let reset = find(&view, EventKind::Reset, "Reset to HEAD~1");
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

#[tokio::test]
async fn a_folder_outside_any_repo_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let (view, _) = load::view(dir.path(), AgentId::new([4; 32]), false).await;
    assert_eq!(view.problem.as_deref(), Some("Not a git repository."));
}

/// How many times [`Counted`] was asked for PRs.
static ASKED: AtomicUsize = AtomicUsize::new(0);

/// A source that lists one PR and counts how often it is asked.
struct Counted;

#[async_trait]
impl Source for Counted {
    async fn list(&self, _: &Slug) -> Result<Vec<Pr>, Failure> {
        ASKED.fetch_add(1, Ordering::SeqCst);
        Ok(vec![one_pr()])
    }
    async fn diff(&self, _: &Slug, _: u64) -> Result<String, Failure> {
        Err(Failure::Unavailable)
    }
}

fn counted() -> Vec<Box<dyn Source>> {
    vec![Box::new(Counted)]
}

fn one_pr() -> Pr {
    Pr {
        number: 1,
        title: "One".into(),
        status: PrStatus::Open,
        head: "one".into(),
        base: "main".into(),
        created: 1,
        merged: None,
        closed: None,
        url: String::new(),
    }
}

#[tokio::test]
async fn prs_are_asked_for_a_minute_at_a_time_unless_forced() {
    let slug = github::slug("git@github.com:acme/cached").unwrap();
    let asked = || ASKED.load(Ordering::SeqCst);

    let (answer, reading) = prs(&slug, false, counted).await;
    assert_eq!(answer, None, "a load doesn't wait for GitHub");
    let (_, again) = prs(&slug, false, counted).await;
    assert!(again.is_none(), "one ask at a time");
    reading.expect("GitHub is asked").await.unwrap();
    assert_eq!(asked(), 1);

    let (answer, reading) = prs(&slug, false, counted).await;
    assert_eq!(answer, Some(Ok(vec![one_pr()])));
    assert!(reading.is_none(), "a minute hasn't gone by");

    let (answer, reading) = prs(&slug, true, counted).await;
    assert_eq!(answer, Some(Ok(vec![one_pr()])));
    assert!(reading.is_none(), "forced, it waits for the answer");
    assert_eq!(asked(), 2);

    lock(&CACHE.prs).get_mut(&slug).unwrap().asked = Instant::now().checked_sub(PRS_FOR);
    let (_, reading) = prs(&slug, false, counted).await;
    reading.expect("a minute on, it asks again").await.unwrap();
    assert_eq!(asked(), 3);
}
