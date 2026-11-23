//! A sign in an attempt's evidence that it may have failed.

use serde::{Deserialize, Serialize};

/// A sign in an attempt's [`Evidence`](super::Evidence) that it may have
/// failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Flag {
    /// Its last check counted as passed, but the command's exit status
    /// was not the check's: its output was piped on, or a command after
    /// it ran whatever it did.
    MaskedCheck,
    /// Its last check counted as passed, but what it printed shows
    /// failures or errors.
    CheckOutputFailures,
    /// Both of the above: nothing says the check passed, and its output
    /// says it did not.
    MaskedCheckWithFailures,
    /// Its reply says the work was not verified
    /// ([`says_unverified`](crate::transcript::reply::says_unverified)).
    /// After a check that counted as passed, Gate has the judge review
    /// such an attempt (see `aura-gate-followup`).
    ReplyUnverified,
    /// Its reply says tests still fail.
    ReplyFailures,
    /// It ran no check to its end.
    NoCheck,
    /// Work that changes code changed no file.
    NoChanges,
    /// Its reply says it could not do the work.
    GaveUp,
    /// It stopped on a limit.
    HitLimit,
    /// It changed files with commands it ran rather than with the file
    /// tools: the workspace holds changes its file tools did not make.
    ShellEdit,
    /// It changed a file after the last check it ran.
    StaleCheck,
    /// Its reply says the tests it ran do not cover its change.
    ReplyNotCovered,
    /// Work that changes code added or changed no test.
    NoTestChanged,
    /// Its last check counted as passed, but every test it ran is in a
    /// file the attempt wrote itself
    /// ([`Evidence::files_written`](super::Evidence::files_written)), so
    /// no test it did not write checked the work.
    OwnTestsOnly,
}

impl Flag {
    /// Every flag, in the order reports list them.
    pub const ALL: [Self; 14] = [
        Self::MaskedCheck,
        Self::CheckOutputFailures,
        Self::MaskedCheckWithFailures,
        Self::ReplyUnverified,
        Self::ReplyFailures,
        Self::NoCheck,
        Self::NoChanges,
        Self::GaveUp,
        Self::HitLimit,
        Self::ShellEdit,
        Self::StaleCheck,
        Self::ReplyNotCovered,
        Self::NoTestChanged,
        Self::OwnTestsOnly,
    ];

    /// The flag's name as logged, e.g. `masked_check`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::MaskedCheck => "masked_check",
            Self::CheckOutputFailures => "check_output_failures",
            Self::MaskedCheckWithFailures => "masked_check_with_failures",
            Self::ReplyUnverified => "reply_unverified",
            Self::ReplyFailures => "reply_failures",
            Self::NoCheck => "no_check",
            Self::NoChanges => "no_changes",
            Self::GaveUp => "gave_up",
            Self::HitLimit => "hit_limit",
            Self::ShellEdit => "shell_edit",
            Self::StaleCheck => "stale_check",
            Self::ReplyNotCovered => "reply_not_covered",
            Self::NoTestChanged => "no_test_changed",
            Self::OwnTestsOnly => "own_tests_only",
        }
    }

    /// The flag in words, for whoever reviews or repairs the work.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::MaskedCheck => "the last test or build ran through a pipe or a chain of commands, so its exit status is not the check's own",
            Self::CheckOutputFailures => "the last test or build exited with success, but what it printed shows failures or errors",
            Self::MaskedCheckWithFailures => "the last test or build printed failures or errors, and its exit status was another command's",
            Self::ReplyUnverified => "the final reply says the work was not verified or not tested",
            Self::ReplyFailures => "the final reply says tests still fail",
            Self::NoCheck => "no test or build was run to its end",
            Self::NoChanges => "the request asks for a code change, but no file was changed",
            Self::GaveUp => "the final reply says the work could not be done or was left unfinished",
            Self::HitLimit => "the attempt stopped on a limit (iterations, budget or a stall) instead of finishing",
            Self::ShellEdit => "files were changed by shell commands rather than the file tools",
            Self::StaleCheck => "a file was changed after the last test or build ran",
            Self::ReplyNotCovered => "the final reply says the tests that were run do not cover the change",
            Self::NoTestChanged => "no test was added or changed",
            Self::OwnTestsOnly => "the last test or build ran only tests in files the attempt wrote itself, so no test it did not write checked the work",
        }
    }
}
