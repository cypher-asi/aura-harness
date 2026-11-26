//! What the reflogs say: how `HEAD` and the remote-tracking branches
//! moved.

use super::super::parse::{self, Entry, HeadMove, Moved, RemoteMove};

fn entry(sha: &str, at: i64, message: &str) -> Entry {
    Entry {
        sha: sha.into(),
        at,
        message: message.into(),
    }
}

/// The moves of `HEAD` in `entries`: what, from where and to where.
fn head_moves_in(entries: &[Entry]) -> Vec<(HeadMove, Option<String>, String)> {
    parse::head_moves(entries)
        .into_iter()
        .map(|m| (m.what, m.from, m.to))
        .collect()
}

/// A move from `from` to `to`, as [`head_moves_in`] lists it.
fn moved(what: HeadMove, from: &str, to: &str) -> (HeadMove, Option<String>, String) {
    (what, Some(from.to_string()), to.to_string())
}

#[test]
fn reflog_entries_read_their_sha_time_and_message() {
    let out = "ffff\x1fHEAD@{1795663637}\x1fmerge channel-projects: Fast-forward\0";
    assert_eq!(
        parse::reflog(out),
        [entry(
            "ffff",
            1_795_663_637,
            "merge channel-projects: Fast-forward"
        )]
    );
}

#[test]
fn head_moves_fold_a_rebase_and_leave_commits_to_the_log() {
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
    assert_eq!(
        head_moves_in(&entries),
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
    let merged = |branch: &str, from: &str, to: &str| {
        let what = HeadMove::Merge {
            branch: branch.into(),
        };
        moved(what, from, to)
    };
    assert_eq!(
        head_moves_in(&entries),
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
