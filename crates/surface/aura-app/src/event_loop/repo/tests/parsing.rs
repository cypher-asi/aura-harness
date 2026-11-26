//! What `status`, `log`, `for-each-ref`, `worktree list` and `stash
//! list` print, read.

use super::super::parse;
use aura_ui::repo::ChangeKind;

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
