use super::*;

#[test]
fn finds_the_checkout_of_a_branch() {
    let listed = "worktree /repo\nHEAD 1\nbranch refs/heads/main\n\n\
                  worktree /wt\nHEAD 2\ndetached\n";
    assert_eq!(
        checkout_of(listed, "refs/heads/main"),
        Some(PathBuf::from("/repo"))
    );
    assert_eq!(checkout_of(listed, "refs/heads/dev"), None);
}

#[test]
fn a_conflict_names_its_files() {
    let reason = conflict_reason("main", "0123456789", "a.rs\nb.rs\n", "");
    assert!(reason.contains("a.rs, b.rs"), "{reason}");
    assert!(reason.contains("01234567"), "{reason}");
}
