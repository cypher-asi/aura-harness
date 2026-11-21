//! What Gate does about a turn it routed that ended with nothing to judge
//! it by: no test or build of its own, and no error.
//!
//! Such a turn may have failed without anyone noticing, so before it
//! counts as done Gate looks for evidence, cheapest first:
//!
//! 1. [`Step::Nudge`]: a turn that changed files is asked once, on the
//!    same model, to run the tests that cover its change. Its own check
//!    then says how it went.
//! 2. [`Step::HarnessCheck`]: when it still ran none, the harness runs
//!    the project's tests itself.
//! 3. [`Step::JudgeUnverified`]: when nothing could be run and the turn
//!    looks wrong ([`suspicions`]), the judge reviews it without a check.
//!
//! A failure any of them turns up goes on to a stronger model as a failed
//! check or a judged failure does. Each step runs at most once per
//! attempt; the caller only asks while a stronger model is left.

use aura_core_types::{OutcomeSignal, PromptCategory};

use crate::router::verifiable;

/// How a turn that Gate routed ended, as the steps need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ended<'a> {
    /// Its signal (see [`crate::outcome::turn_signal`]).
    pub signal: OutcomeSignal,
    pub category: PromptCategory,
    /// Files its tools changed.
    pub files_changed: usize,
    /// Its final reply.
    pub reply: &'a str,
    /// Whether it stopped on a limit (iterations, budget, a stall, a cut
    /// off reply) rather than by finishing.
    pub hit_limit: bool,
}

/// The steps already taken for the attempt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Taken {
    pub nudged: bool,
    pub checked: bool,
    pub judged: bool,
}

/// The steps the caller can take.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Allowed {
    pub nudge: bool,
    /// A test command the harness can run is known.
    pub harness_check: bool,
    pub judge: bool,
}

/// Why a turn with no check looks wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suspicion {
    /// Work that changes code changed no file.
    NoChanges,
    /// Its reply says it could not do the work.
    GaveUp,
    /// It stopped on a limit.
    HitLimit,
}

impl Suspicion {
    /// The suspicion in words, for the judge.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::NoChanges => "the request asks for a code change, but no file was changed",
            Self::GaveUp => "the final reply says the work could not be done or was left unfinished",
            Self::HitLimit => "the attempt stopped on a limit (iterations, budget or a stall) instead of finishing",
        }
    }
}

/// What to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Nothing more: the turn stands as it ended.
    Stop,
    /// Ask the same model to verify its change.
    Nudge,
    /// Run the project's tests.
    HarnessCheck,
    /// Have the judge review the work for these reasons.
    JudgeUnverified(Vec<Suspicion>),
}

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
    let reply = reply.to_lowercase().replace('’', "'");
    GAVE_UP.iter().any(|phrase| reply.contains(phrase))
}

/// Whether the steps apply to work asking for `category`: code changes,
/// debugging, and work nothing classified.
const fn checkable(category: PromptCategory) -> bool {
    verifiable(category) || matches!(category, PromptCategory::Unclassified)
}

/// Why `ended`, a turn with no check, looks wrong; empty when nothing
/// does.
#[must_use]
pub fn suspicions(ended: &Ended<'_>) -> Vec<Suspicion> {
    if !checkable(ended.category) {
        return Vec::new();
    }
    let mut found = Vec::new();
    if verifiable(ended.category) && ended.files_changed == 0 {
        found.push(Suspicion::NoChanges);
    }
    if gave_up(ended.reply) {
        found.push(Suspicion::GaveUp);
    }
    if ended.hit_limit {
        found.push(Suspicion::HitLimit);
    }
    found
}

/// The next step for `ended`, given the steps `taken` and those
/// `allowed`. Only a turn that ended with [`OutcomeSignal::None`] gets
/// any: other signals already say how it went.
#[must_use]
pub fn next(ended: &Ended<'_>, taken: Taken, allowed: Allowed) -> Step {
    if ended.signal != OutcomeSignal::None || !checkable(ended.category) {
        return Step::Stop;
    }
    let changed = ended.files_changed > 0;
    if changed && allowed.nudge && !taken.nudged && !ended.hit_limit {
        return Step::Nudge;
    }
    if changed && allowed.harness_check && !taken.checked {
        return Step::HarnessCheck;
    }
    if allowed.judge && !taken.judged {
        let found = suspicions(ended);
        if !found.is_empty() {
            return Step::JudgeUnverified(found);
        }
    }
    Step::Stop
}

/// What the model is told when it ended a turn having changed files
/// without running a test or build.
pub const NUDGE_PROMPT: &str = "\
You changed files but did not run any test or build to check the change. Before you finish, run \
the tests that cover what you changed (run them the way this project or task says commands run, \
for example through its own wrapper script), and fix anything that fails. If no test covers it, \
run the build or type check. Then reply with what you ran and how it went.";

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: Allowed = Allowed {
        nudge: true,
        harness_check: true,
        judge: true,
    };

    fn ended(files_changed: usize, reply: &str) -> Ended<'_> {
        Ended {
            signal: OutcomeSignal::None,
            category: PromptCategory::MultiFileChange,
            files_changed,
            reply,
            hit_limit: false,
        }
    }

    #[test]
    fn a_change_nobody_checked_is_nudged_then_checked_by_the_harness() {
        let changed = ended(2, "Done.");
        assert_eq!(next(&changed, Taken::default(), ALL), Step::Nudge);
        let nudged = Taken {
            nudged: true,
            ..Taken::default()
        };
        assert_eq!(next(&changed, nudged, ALL), Step::HarnessCheck);
        let checked = Taken {
            checked: true,
            ..nudged
        };
        assert_eq!(
            next(&changed, checked, ALL),
            Step::Stop,
            "nothing looks wrong"
        );
    }

    #[test]
    fn steps_that_are_not_allowed_are_skipped() {
        let changed = ended(1, "Done.");
        let check_only = Allowed {
            harness_check: true,
            ..Allowed::default()
        };
        assert_eq!(
            next(&changed, Taken::default(), check_only),
            Step::HarnessCheck
        );
        assert_eq!(
            next(&changed, Taken::default(), Allowed::default()),
            Step::Stop
        );
    }

    #[test]
    fn a_change_that_changed_nothing_goes_to_the_judge() {
        assert_eq!(
            next(
                &ended(0, "The code already does this."),
                Taken::default(),
                ALL
            ),
            Step::JudgeUnverified(vec![Suspicion::NoChanges])
        );
        let judged = Taken {
            judged: true,
            ..Taken::default()
        };
        assert_eq!(next(&ended(0, ""), judged, ALL), Step::Stop);
    }

    #[test]
    fn giving_up_and_limits_are_suspicious_after_the_checks() {
        let gave_up = ended(1, "I wasn’t able to finish the migration.");
        let taken = Taken {
            nudged: true,
            checked: true,
            judged: false,
        };
        assert_eq!(
            next(&gave_up, taken, ALL),
            Step::JudgeUnverified(vec![Suspicion::GaveUp])
        );
        let limited = Ended {
            hit_limit: true,
            ..ended(3, "Working on it")
        };
        assert_eq!(
            next(&limited, Taken::default(), ALL),
            Step::HarnessCheck,
            "a turn that ran out of room is not nudged"
        );
        assert_eq!(
            suspicions(&limited),
            vec![Suspicion::HitLimit],
            "it stopped on its limit"
        );
    }

    #[test]
    fn only_a_turn_without_a_signal_gets_steps() {
        for signal in [
            OutcomeSignal::ChecksPassed,
            OutcomeSignal::ChecksFailed,
            OutcomeSignal::Error,
            OutcomeSignal::Unavailable,
        ] {
            let with_signal = Ended {
                signal,
                ..ended(0, "")
            };
            assert_eq!(
                next(&with_signal, Taken::default(), ALL),
                Step::Stop,
                "{signal}"
            );
        }
    }

    #[test]
    fn answers_and_writing_get_no_steps() {
        for category in [
            PromptCategory::Question,
            PromptCategory::Research,
            PromptCategory::Planning,
            PromptCategory::Writing,
            PromptCategory::Other,
        ] {
            let answer = Ended {
                category,
                ..ended(0, "I couldn't find it.")
            };
            assert_eq!(
                next(&answer, Taken::default(), ALL),
                Step::Stop,
                "{category:?}"
            );
            assert!(suspicions(&answer).is_empty());
        }
        let unclassified = Ended {
            category: PromptCategory::Unclassified,
            ..ended(0, "Here you go.")
        };
        assert_eq!(
            next(&unclassified, Taken::default(), ALL),
            Step::Stop,
            "unclassified work that changed nothing may be an answer"
        );
    }

    #[test]
    fn giving_up_is_read_from_the_reply() {
        assert!(gave_up("Sorry, I could not reproduce the failure."));
        assert!(gave_up("I'M UNABLE TO run the tests here."));
        assert!(!gave_up("Fixed the parser and added a test."));
        assert!(!gave_up(""));
    }
}
