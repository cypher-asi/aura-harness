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
