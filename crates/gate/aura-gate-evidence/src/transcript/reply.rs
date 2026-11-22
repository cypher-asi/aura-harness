//! What a turn's final reply says of its own work.

/// Phrases of a reply that gives up, lowercase.
const GAVE_UP: &[&str] = &[
    "i was unable to",
    "i wasn't able to",
    "i was not able to",
    "i'm unable to",
    "i am unable to",
    "i'm not able to",
    "i am not able to",
    "i couldn't",
    "i could not",
    "i didn't manage",
    "i did not manage",
    "i gave up",
    "unable to complete",
    "could not complete",
    "couldn't complete",
    "left as a todo",
    "not yet implemented",
];

/// Whether `reply` says the work could not be done.
#[must_use]
pub fn gave_up(reply: &str) -> bool {
    says_any(reply, GAVE_UP)
}

/// Words that mark a sentence of a reply as about linting or formatting,
/// lowercase.
const LINTING: &[&str] = &[
    "lint",
    "pyflakes",
    "pycodestyle",
    "flake8",
    "ruff",
    "rustfmt",
    "clippy",
    "isort",
    "formatter",
    "format check",
    "style check",
];

/// Words that mark a sentence of a reply as about the work's tests or
/// build too, lowercase.
const TESTING: &[&str] = &["test", "build", "compile", "type check", "typecheck"];

/// Whether a sentence of `reply`, lowercase, holds one of `phrases`. A
/// sentence about linting or formatting alone does not count: that such
/// a step could not run says nothing of whether the work does what was
/// asked (`I couldn't run a lint check because pyflakes isn't installed`
/// after the tests passed).
fn says_any(reply: &str, phrases: &[&str]) -> bool {
    let reply = reply.to_lowercase().replace('’', "'");
    reply
        .split(['\n', '!', '?', ';'])
        .flat_map(|line| line.split(". "))
        .filter(|sentence| {
            !LINTING.iter().any(|word| sentence.contains(word))
                || TESTING.iter().any(|word| sentence.contains(word))
        })
        .any(|sentence| phrases.iter().any(|phrase| sentence.contains(phrase)))
}

/// Phrases of a reply that says the work was not verified, lowercase.
const UNVERIFIED: &[&str] = &[
    "isn't verified",
    "is not verified",
    "not verified",
    "unverified",
    "couldn't verify",
    "could not verify",
    "didn't verify",
    "did not verify",
    "wasn't able to run",
    "was not able to run",
    "unable to run",
    "couldn't run",
    "could not run",
    "haven't run",
    "have not run",
    "not been tested",
    "haven't tested",
    "have not tested",
    "didn't test",
    "did not test",
    "untested",
    "cut short",
    "can't say whether",
    "cannot say whether",
    "can't confirm",
    "cannot confirm",
    "couldn't confirm",
    "could not confirm",
    "still unchecked",
    "is unchecked",
    "are unchecked",
    "remains unchecked",
    "remain unchecked",
    "left unchecked",
    "haven't checked",
    "have not checked",
    "not checked against",
    "did not compare",
    "didn't compare",
    "do not compare against",
    "don't compare against",
    "does not compare against",
    "doesn't compare against",
    "not compared",
    "not pinned down",
    "open question",
    "did not run it",
    "didn't run it",
    "did not run them",
    "didn't run them",
    "did not run the",
    "didn't run the",
    "did not run any",
    "didn't run any",
];

/// Whether `reply` says the work, or part of it, was not verified.
#[must_use]
pub fn says_unverified(reply: &str) -> bool {
    says_any(reply, UNVERIFIED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_verifying_is_read_from_the_reply() {
        assert!(says_unverified(
            "The change is in place but I couldn’t run the tests."
        ));
        assert!(says_unverified("It builds; the Python 2 path is UNTESTED."));
        assert!(!says_unverified("All 286 tests passed."));
        assert!(!says_unverified(""));
    }

    /// The end of the final reply of run a-2 of Compare #32: Claude Haiku
    /// 5.5 on `ansible-b748edea`, whose own 14 tests passed and which
    /// Gate let stand. The task's check then failed.
    const A2_GAPS: &str = "**Gaps that remain:**\n\
        - The tests do not compare against `email.mime.multipart.MIMEMultipart('form-data')` \
        serialized with `email.policy.HTTP`, and the `multipart.txt` fixture the request \
        mentions does not exist in the repo. So byte-identity with that output is still \
        unchecked.\n\
        - I quote the boundary in `Content-Type` (`boundary=\"...\"`), which I chose to match \
        stdlib output. The tests accept both quoted and unquoted forms, so this choice is not \
        pinned down by a test.\n\
        - The test file uses `bytes` and `rb''` literals, so it is Python 3 only. I did not \
        run it under Python 2.\n\nNo code changes were needed after the test run.";

    #[test]
    fn a_gap_the_reply_owns_up_to_counts_as_unverified() {
        assert!(says_unverified(A2_GAPS));
        for gap in A2_GAPS.lines().filter(|line| line.starts_with("- ")) {
            assert!(says_unverified(gap), "{gap}");
        }
        for reply in [
            "I did not compare the output byte-for-byte, so that comparison is an open question.",
            "I haven't checked it against Python 2.",
        ] {
            assert!(says_unverified(reply), "{reply}");
        }
        for reply in [
            "I did not run into any issues; all 14 tests pass.",
            "Replaced `get_unchecked` with `get`; cargo test passes.",
            "The old code did not check for `None`; the new test covers it and passes.",
        ] {
            assert!(!says_unverified(reply), "{reply}");
        }
    }

    #[test]
    fn a_lint_step_that_could_not_run_leaves_the_tests_standing() {
        let reply = "All 39 unit tests pass. I couldn't run a lint check because pyflakes and \
                     pycodestyle aren't installed in the container.\n\nThe sample data is \
                     written by hand.";
        assert!(!says_unverified(reply));
        assert!(!gave_up(reply));
        assert!(!says_unverified("Couldn't run clippy; rustfmt is clean."));
        for reply in [
            "I couldn't run the tests or the linter.",
            "The linter is clean, but the new parser is untested.",
            "Lint passes. I couldn't run the build.",
        ] {
            assert!(says_unverified(reply), "{reply}");
        }
        assert!(gave_up("Lint passes. I could not reproduce the failure."));
    }

    #[test]
    fn giving_up_is_read_from_the_reply() {
        assert!(gave_up("Sorry, I could not reproduce the failure."));
        assert!(gave_up("I'M UNABLE TO run the tests here."));
        assert!(!gave_up("Fixed the parser and added a test."));
        assert!(!gave_up(""));
    }
}
