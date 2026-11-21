//! What Gate does about a turn it routed that ended with nothing to judge
//! it by: no test or build of its own, and no error; or with a check
//! that passed while its own reply says the work was not verified.
//!
//! A turn with no check may have failed without anyone noticing, so
//! before it counts as done Gate looks for evidence, cheapest first:
//!
//! 1. [`Step::Nudge`]: a turn that changed files is asked once, on the
//!    same model, to run the tests that cover its change. Its own check
//!    then says how it went.
//! 2. [`Step::HarnessCheck`]: when it still ran none, the harness runs
//!    the project's tests itself.
//! 3. [`Step::JudgeUnverified`]: when nothing could be run and the turn
//!    looks wrong ([`suspicions`]), the judge reviews it without a check.
//!
//! A turn whose check passed is not thereby right: the check may cover
//! little of what was asked. On the benchmark runs in Gate's attempt log
//! ([`crate::attempt_log`]), a fifth of the hard attempts by models below
//! the strongest tier that Gate let stand had failed the task's own
//! check, most of them after a check of their own had passed. So before
//! such work stands it may be reviewed:
//!
//! 4. [`Step::Review`]: the judge goes through the request against
//!    everything the attempt changed. It is asked when the work was
//!    classified hard, when its evidence raises a sign the attempt log
//!    backs ([`crate::attempt_log::tiers`]), or when it looks wrong as in
//!    step 3 or its reply says the work was not verified
//!    ([`says_unverified`]). Work it finds unfinished goes back to the
//!    same model once with the findings ([`repair_prompt`]) and is
//!    reviewed again; found unfinished a second time, it fails. A turn
//!    is sent back once: work that has moved on to a stronger model and
//!    is found unfinished there fails at once.
//! 5. [`Step::Repair`]: a sign that has proven itself outright in the
//!    log skips the review and sends the work back at once.
//!
//! The review is governed by [`ReviewMode`]: `on` as above; `off` does
//! not ask, which leaves steps 1 to 3 and the one case the judge always
//! took, a passed check the reply disowns; `log` is `off` plus a review
//! whose verdict is only logged, of the work the judge would not see.
//!
//! A failure any of them turns up goes on to a stronger model as a failed
//! check or a judged failure does. Each step runs at most once per
//! attempt, the review twice when the work was sent back between; the
//! caller only asks while a stronger model is left.

use aura_config::ReviewMode;
use aura_core_types::{Difficulty, OutcomeSignal, PromptCategory};

use crate::evidence::Flag;
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
    /// How hard the classifier judged the work.
    pub difficulty: Option<Difficulty>,
    /// The signs its evidence raises that have earned a review of the
    /// attempts they are raised on ([`crate::attempt_log::Tier::Review`]).
    pub review_signs: &'a [Flag],
    /// The signs its evidence raises that act on their own
    /// ([`crate::attempt_log::Tier::Act`]).
    pub acting_signs: &'a [Flag],
}

/// The steps already taken for the attempt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Taken {
    pub nudged: bool,
    pub checked: bool,
    pub judged: bool,
    /// Times it was reviewed
    pub reviews: u8,
    /// Whether the turn's work was sent back to its model with what was
    /// found, in this attempt or one before it
    pub repaired: bool,
}

/// The steps the caller can take.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Allowed {
    pub nudge: bool,
    /// A test command the harness can run is known.
    pub harness_check: bool,
    pub judge: bool,
    /// Whether work about to stand is reviewed; needs `judge`.
    pub review: ReviewMode,
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
    /// A check it ran passed, but its reply says the work was not
    /// verified.
    Unverified,
}

impl Suspicion {
    /// The suspicion in words, for the judge.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::NoChanges => "the request asks for a code change, but no file was changed",
            Self::GaveUp => "the final reply says the work could not be done or was left unfinished",
            Self::HitLimit => "the attempt stopped on a limit (iterations, budget or a stall) instead of finishing",
            Self::Unverified => "a test or build it ran exited with success, but the final reply says the work was not verified or not tested",
        }
    }

    /// The sign of failure that says the same of an attempt's evidence.
    #[must_use]
    pub const fn flag(self) -> Flag {
        match self {
            Self::NoChanges => Flag::NoChanges,
            Self::GaveUp => Flag::GaveUp,
            Self::HitLimit => Flag::HitLimit,
            Self::Unverified => Flag::ReplyUnverified,
        }
    }
}

/// Why work about to stand is reviewed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// The work was classified hard.
    Hard,
    /// Its evidence raises this sign.
    Sign(Flag),
}

impl Why {
    /// The reason's name as logged: `hard`, or the sign's.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Hard => "hard",
            Self::Sign(flag) => flag.name(),
        }
    }

    /// The reason in words, for the reviewer.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Hard => {
                "the task was rated hard, and hard work is reviewed before it is accepted"
            }
            Self::Sign(flag) => flag.describe(),
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
    /// Have the judge go through the work against the request before it
    /// stands, for these reasons. Unless `act`, its verdict is only
    /// logged.
    Review { why: Vec<Why>, act: bool },
    /// Send the work back to the same model, told these signs.
    Repair(Vec<Flag>),
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
];

/// Whether `reply` says the work, or part of it, was not verified.
#[must_use]
pub fn says_unverified(reply: &str) -> bool {
    let reply = reply.to_lowercase().replace('’', "'");
    UNVERIFIED.iter().any(|phrase| reply.contains(phrase))
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

/// Why `ended`, whose check did not fail, looks wrong without a review:
/// a passed check its reply disowns (unless the harness ran the check),
/// or, with no check, its [`suspicions`].
fn looks_wrong(ended: &Ended<'_>, taken: Taken) -> Vec<Suspicion> {
    if ended.signal == OutcomeSignal::ChecksPassed {
        if !taken.checked && says_unverified(ended.reply) {
            vec![Suspicion::Unverified]
        } else {
            Vec::new()
        }
    } else {
        suspicions(ended)
    }
}

/// Why `ended` is reviewed before it stands; empty when nothing calls
/// for it.
fn reasons(ended: &Ended<'_>, looks: &[Suspicion]) -> Vec<Why> {
    let mut why = Vec::new();
    if ended.difficulty == Some(Difficulty::Hard) {
        why.push(Why::Hard);
    }
    let signs = ended
        .acting_signs
        .iter()
        .chain(ended.review_signs)
        .copied()
        .chain(looks.iter().map(|suspicion| suspicion.flag()));
    for sign in signs {
        if !why.contains(&Why::Sign(sign)) {
            why.push(Why::Sign(sign));
        }
    }
    why
}

/// The next step for `ended`, given the steps `taken` and those
/// `allowed`. Only a turn that ended with [`OutcomeSignal::None`] or
/// [`OutcomeSignal::ChecksPassed`] gets any: other signals already say
/// how it went. One with no check is nudged, then checked by the
/// harness. Then, where [`Allowed::review`] is on, work that is hard,
/// shows a sign or looks wrong is reviewed, twice at most and the second
/// time only after it was sent back. Otherwise a turn that looks wrong
/// goes to the judge: with no check, for its [`suspicions`]; with a
/// passed check of its own, when its reply says the work was not
/// verified.
#[must_use]
pub fn next(ended: &Ended<'_>, taken: Taken, allowed: Allowed) -> Step {
    if !checkable(ended.category) {
        return Step::Stop;
    }
    if !matches!(
        ended.signal,
        OutcomeSignal::None | OutcomeSignal::ChecksPassed
    ) {
        return Step::Stop;
    }
    if ended.signal == OutcomeSignal::None {
        let changed = ended.files_changed > 0;
        if changed && allowed.nudge && !taken.nudged && !ended.hit_limit {
            return Step::Nudge;
        }
        if changed && allowed.harness_check && !taken.checked {
            return Step::HarnessCheck;
        }
    }
    if !allowed.judge {
        return Step::Stop;
    }
    let looks = looks_wrong(ended, taken);
    match allowed.review {
        ReviewMode::On => {
            let fresh = taken.reviews == 0 && !taken.repaired;
            if fresh && !ended.hit_limit && !ended.acting_signs.is_empty() {
                return Step::Repair(ended.acting_signs.to_vec());
            }
            if taken.reviews == 0 || (taken.reviews == 1 && taken.repaired) {
                let why = reasons(ended, &looks);
                if !why.is_empty() {
                    return Step::Review { why, act: true };
                }
            }
            // The review stands in for the judge of work that looks
            // wrong.
            return Step::Stop;
        }
        // Work that looks wrong goes to the judge below; the rest is
        // reviewed for the log.
        ReviewMode::Log if looks.is_empty() && taken.reviews == 0 => {
            let why = reasons(ended, &[]);
            if !why.is_empty() {
                return Step::Review { why, act: false };
            }
        }
        ReviewMode::Log | ReviewMode::Off => {}
    }
    if !taken.judged && !looks.is_empty() {
        return Step::JudgeUnverified(looks);
    }
    Step::Stop
}

/// How every message that sends work back to its model starts; tells
/// them apart from the user's prompts.
pub const REPAIR_OPENING: &str = "Before your work was accepted, ";

/// What the model is told when a review found its work unfinished.
#[must_use]
pub fn repair_prompt(findings: &str) -> String {
    format!(
        "{REPAIR_OPENING}a reviewer went through your change against the request and found it \
         unfinished:\n\n{}\n\nFix what it found. If a finding is wrong, say why and leave that \
         part as it is. Then run the tests that cover the change (the way this project or task \
         says commands run) and reply with what you changed and how it went.",
        findings.trim()
    )
}

/// What the model is told when `signs` alone send its work back.
#[must_use]
pub fn sign_repair_prompt(signs: &[Flag]) -> String {
    let listed: Vec<String> = signs
        .iter()
        .map(|sign| format!("- {}", sign.describe()))
        .collect();
    format!(
        "{REPAIR_OPENING}the harness found signs that it is not finished:\n{}\n\nGo back over \
         the request and your change, and fix what is missing or wrong. Then run the tests that \
         cover the change (the way this project or task says commands run) and reply with what \
         you changed and how it went.",
        listed.join("\n")
    )
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
        review: ReviewMode::Off,
    };

    const REVIEWING: Allowed = Allowed {
        review: ReviewMode::On,
        ..ALL
    };

    const LOGGING: Allowed = Allowed {
        review: ReviewMode::Log,
        ..ALL
    };

    fn ended(files_changed: usize, reply: &str) -> Ended<'_> {
        Ended {
            signal: OutcomeSignal::None,
            category: PromptCategory::MultiFileChange,
            files_changed,
            reply,
            hit_limit: false,
            difficulty: None,
            review_signs: &[],
            acting_signs: &[],
        }
    }

    fn passed(reply: &str) -> Ended<'_> {
        Ended {
            signal: OutcomeSignal::ChecksPassed,
            ..ended(2, reply)
        }
    }

    fn review(why: &[Why], act: bool) -> Step {
        Step::Review {
            why: why.to_vec(),
            act,
        }
    }

    #[test]
    fn hard_work_whose_check_passed_is_reviewed_before_it_stands() {
        let hard = Ended {
            difficulty: Some(Difficulty::Hard),
            ..passed("All 286 tests passed.")
        };
        assert_eq!(
            next(&hard, Taken::default(), REVIEWING),
            review(&[Why::Hard], true)
        );
        let reviewed = Taken {
            reviews: 1,
            ..Taken::default()
        };
        assert_eq!(next(&hard, reviewed, REVIEWING), Step::Stop, "once");
        let sent_back = Taken {
            repaired: true,
            ..reviewed
        };
        assert_eq!(
            next(&hard, sent_back, REVIEWING),
            review(&[Why::Hard], true),
            "again after it was sent back"
        );
        let twice = Taken {
            reviews: 2,
            ..sent_back
        };
        assert_eq!(next(&hard, twice, REVIEWING), Step::Stop);

        let easy = Ended {
            difficulty: Some(Difficulty::Medium),
            ..hard
        };
        assert_eq!(next(&easy, Taken::default(), REVIEWING), Step::Stop);
        assert_eq!(next(&hard, Taken::default(), ALL), Step::Stop, "off");
        let no_judge = Allowed {
            judge: false,
            ..REVIEWING
        };
        assert_eq!(next(&hard, Taken::default(), no_judge), Step::Stop);
        let failed = Ended {
            signal: OutcomeSignal::ChecksFailed,
            ..hard
        };
        assert_eq!(next(&failed, Taken::default(), REVIEWING), Step::Stop);
    }

    #[test]
    fn a_sign_the_log_backs_calls_for_a_review() {
        let signs = [Flag::ShellEdit];
        let flagged = Ended {
            review_signs: &signs,
            ..passed("Done; the tests pass.")
        };
        assert_eq!(
            next(&flagged, Taken::default(), REVIEWING),
            review(&[Why::Sign(Flag::ShellEdit)], true)
        );
        // What used to go to the judge is a reason too, named once.
        let unverified = [Flag::ReplyUnverified];
        let disowned = Ended {
            difficulty: Some(Difficulty::Hard),
            review_signs: &unverified,
            ..passed("I rewrote it, but it isn't verified.")
        };
        assert_eq!(
            next(&disowned, Taken::default(), REVIEWING),
            review(&[Why::Hard, Why::Sign(Flag::ReplyUnverified)], true)
        );
        let gave_up = Ended {
            signal: OutcomeSignal::None,
            ..ended(0, "I was unable to find the cause.")
        };
        assert_eq!(
            next(&gave_up, Taken::default(), REVIEWING),
            review(&[Why::Sign(Flag::NoChanges), Why::Sign(Flag::GaveUp)], true)
        );
        let reviewed = Taken {
            reviews: 1,
            ..Taken::default()
        };
        assert_eq!(
            next(&gave_up, reviewed, REVIEWING),
            Step::Stop,
            "the review stands in for the judge of unchecked work"
        );
    }

    #[test]
    fn work_with_no_check_is_nudged_and_checked_before_it_is_reviewed() {
        let hard = Ended {
            difficulty: Some(Difficulty::Hard),
            ..ended(2, "Done.")
        };
        assert_eq!(next(&hard, Taken::default(), REVIEWING), Step::Nudge);
        let nudged = Taken {
            nudged: true,
            ..Taken::default()
        };
        assert_eq!(next(&hard, nudged, REVIEWING), Step::HarnessCheck);
        let checked = Taken {
            checked: true,
            ..nudged
        };
        assert_eq!(next(&hard, checked, REVIEWING), review(&[Why::Hard], true));
    }

    #[test]
    fn a_logged_review_is_asked_of_work_the_judge_would_not_see() {
        let vouched = Ended {
            difficulty: Some(Difficulty::Hard),
            ..passed("All tests pass.")
        };
        assert_eq!(
            next(&vouched, Taken::default(), LOGGING),
            review(&[Why::Hard], false)
        );
        let reviewed = Taken {
            reviews: 1,
            ..Taken::default()
        };
        assert_eq!(next(&vouched, reviewed, LOGGING), Step::Stop, "once");

        // Work that looks wrong goes to the judge as with no review, and
        // is not paid for twice.
        let disowned = Ended {
            difficulty: Some(Difficulty::Hard),
            ..passed("I rewrote it, but it isn't verified.")
        };
        assert_eq!(
            next(&disowned, Taken::default(), LOGGING),
            Step::JudgeUnverified(vec![Suspicion::Unverified])
        );
        let judged = Taken {
            judged: true,
            ..Taken::default()
        };
        assert_eq!(next(&disowned, judged, LOGGING), Step::Stop);
    }

    #[test]
    fn a_sign_that_acts_sends_the_work_back_without_a_review() {
        let signs = [Flag::MaskedCheckWithFailures];
        let flagged = Ended {
            acting_signs: &signs,
            ..passed("Done.")
        };
        assert_eq!(
            next(&flagged, Taken::default(), REVIEWING),
            Step::Repair(signs.to_vec())
        );
        let sent_back = Taken {
            repaired: true,
            ..Taken::default()
        };
        assert_eq!(
            next(&flagged, sent_back, REVIEWING),
            review(&[Why::Sign(Flag::MaskedCheckWithFailures)], true),
            "what comes back is reviewed"
        );
        assert_eq!(
            next(&flagged, Taken::default(), LOGGING),
            review(&[Why::Sign(Flag::MaskedCheckWithFailures)], false),
            "a logged review sends nothing back"
        );
        let limited = Ended {
            hit_limit: true,
            ..flagged
        };
        assert_eq!(
            next(&limited, Taken::default(), REVIEWING),
            review(&[Why::Sign(Flag::MaskedCheckWithFailures)], true),
            "a turn that ran out of room is not sent back"
        );
    }

    #[test]
    fn what_goes_back_to_the_model_says_what_was_found() {
        let told = repair_prompt("1. `int` accepts `'1.1'`.\n");
        assert!(told.starts_with(REPAIR_OPENING), "{told}");
        assert!(
            told.contains("1. `int` accepts `'1.1'`.\n\nFix what it found"),
            "{told}"
        );
        let signs = sign_repair_prompt(&[Flag::StaleCheck, Flag::ShellEdit]);
        assert!(signs.starts_with(REPAIR_OPENING), "{signs}");
        assert!(signs.contains(Flag::StaleCheck.describe()), "{signs}");
        assert!(signs.contains(Flag::ShellEdit.describe()), "{signs}");
        assert_eq!(Why::Hard.name(), "hard");
        assert_eq!(Why::Sign(Flag::ShellEdit).name(), "shell_edit");
        assert_eq!(Suspicion::Unverified.flag(), Flag::ReplyUnverified);
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
            ..Taken::default()
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
    fn a_passed_check_the_reply_disowns_goes_to_the_judge() {
        let disowned = Ended {
            signal: OutcomeSignal::ChecksPassed,
            ..ended(
                2,
                "I rewrote `ensure_type()`, but it isn't verified. The only test run was cut short.",
            )
        };
        assert_eq!(
            next(&disowned, Taken::default(), ALL),
            Step::JudgeUnverified(vec![Suspicion::Unverified])
        );
        let judged = Taken {
            judged: true,
            ..Taken::default()
        };
        assert_eq!(next(&disowned, judged, ALL), Step::Stop, "once");
        let no_judge = Allowed {
            judge: false,
            ..ALL
        };
        assert_eq!(next(&disowned, Taken::default(), no_judge), Step::Stop);
        let harness_ran = Taken {
            checked: true,
            ..Taken::default()
        };
        assert_eq!(
            next(&disowned, harness_ran, ALL),
            Step::Stop,
            "the harness's own check passed"
        );

        let vouched = Ended {
            signal: OutcomeSignal::ChecksPassed,
            ..ended(2, "Fixed the parser; `cargo test` passes.")
        };
        assert_eq!(next(&vouched, Taken::default(), ALL), Step::Stop);
        let failed = Ended {
            signal: OutcomeSignal::ChecksFailed,
            ..disowned
        };
        assert_eq!(
            next(&failed, Taken::default(), ALL),
            Step::Stop,
            "a failed check already goes on"
        );
        let answer = Ended {
            category: PromptCategory::Question,
            ..disowned
        };
        assert_eq!(next(&answer, Taken::default(), ALL), Step::Stop);
    }

    #[test]
    fn not_verifying_is_read_from_the_reply() {
        assert!(says_unverified(
            "The change is in place but I couldn’t run the tests."
        ));
        assert!(says_unverified("It builds; the Python 2 path is UNTESTED."));
        assert!(!says_unverified("All 286 tests passed."));
        assert!(!says_unverified(""));
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
