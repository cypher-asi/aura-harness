//! The repo's own changelog: every `changelog.d/` fragment is well formed
//! and none repeats an entry already under `[Unreleased]` in
//! CHANGELOG.md, which a release would then list twice.
//!
//! `git_ship` keeps agents from editing CHANGELOG.md, but a direct merge
//! or a hand-resolved conflict can still put an entry back there. This
//! test runs with the workspace's tests, so the merge queue's checks and
//! CI catch it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use aura_exec_merge_queue::changelog::{check, Fragment, CHANGELOG_FILE, FRAGMENT_DIR};
use std::path::Path;

#[test]
fn no_changelog_entry_is_both_in_a_fragment_and_in_the_changelog() {
    let top = Path::new(env!("CARGO_MANIFEST_DIR"));
    let changelog = std::fs::read_to_string(top.join(CHANGELOG_FILE)).unwrap();
    let fragments = Fragment::read_dir(&top.join(FRAGMENT_DIR)).unwrap();
    if let Err(err) = check(&changelog, &fragments) {
        panic!("{err}\n\nRun `aura changelog --check` after fixing it.");
    }
}
