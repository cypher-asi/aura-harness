//! Work that ran no check, or whose check names nothing it added: the
//! nudge, the harness's check and the judge.

use super::*;

#[test]
fn work_with_no_check_is_nudged_and_checked_before_it_is_reviewed() {
    let hard = Ended {
        difficulty: Some(Difficulty::Hard),
        ..ended(2, "Done.")
    };
    assert_eq!(
        next(&hard, Taken::default(), REVIEWING),
        Step::Nudge(Vec::new())
    );
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
fn work_that_ran_its_change_itself_is_not_nudged_but_still_checked_and_judged() {
    let ran = Ended {
        ran_own_check: true,
        ..ended(2, "Done; a script that imports it printed the right pages.")
    };
    assert_eq!(next(&ran, Taken::default(), ALL), Step::HarnessCheck);
    let checked = Taken {
        checked: true,
        ..Taken::default()
    };
    assert_eq!(next(&ran, checked, ALL), Step::JudgeUnverified(Vec::new()));

    let names = ["page".to_string()];
    let uncovered = Ended {
        uncovered: &names,
        ran_own_check: true,
        ..passed("Done; the tests pass.")
    };
    assert_eq!(next(&uncovered, Taken::default(), ALL), Step::HarnessCheck);
}

#[test]
fn a_passed_check_no_test_of_which_names_the_change_counts_as_none() {
    let names = ["prepare_multipart".to_string()];
    let uncovered = Ended {
        uncovered: &names,
        ..passed("Implemented prepare_multipart; the urls tests pass.")
    };
    let step = next(&uncovered, Taken::default(), ALL);
    assert_eq!(step, Step::Nudge(names.to_vec()));
    let nudged = Taken {
        nudged: true,
        ..Taken::default()
    };
    assert_eq!(next(&uncovered, nudged, ALL), Step::HarnessCheck);
    let checked = Taken {
        checked: true,
        ..nudged
    };
    assert_eq!(
        next(&uncovered, checked, ALL),
        Step::JudgeUnverified(vec![Suspicion::Uncovered])
    );
    assert_eq!(
        next(&uncovered, checked, REVIEWING),
        review(&[Why::Uncovered], true),
        "with reviews on, the review stands in for the judge"
    );
    assert_eq!(
        next(&passed("Done."), Taken::default(), ALL),
        Step::Stop,
        "a test names what it added"
    );
}

#[test]
fn a_change_nobody_checked_is_nudged_then_checked_by_the_harness() {
    let changed = ended(2, "Done.");
    assert_eq!(
        next(&changed, Taken::default(), ALL),
        Step::Nudge(Vec::new())
    );
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
        Step::JudgeUnverified(Vec::new()),
        "nothing looks wrong, but it still needs a verdict"
    );
    let judged = Taken {
        judged: true,
        ..checked
    };
    assert_eq!(next(&changed, judged, ALL), Step::Stop);
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
fn a_turn_with_a_signal_gets_no_steps() {
    for signal in [
        OutcomeSignal::ChecksPassed,
        OutcomeSignal::ChecksFailed,
        OutcomeSignal::Error,
        OutcomeSignal::Unavailable,
    ] {
        let with_signal = Ended {
            signal: Some(signal),
            category: PromptCategory::Question,
            ..ended(0, "")
        };
        assert_eq!(
            next(&with_signal, Taken::default(), ALL),
            Step::Stop,
            "{signal}"
        );
    }
}

/// Code work whose check passed while its reply says it is not verified.
fn disowned() -> Ended<'static> {
    passed("I rewrote `ensure_type()`, but it isn't verified. The only test run was cut short.")
}

#[test]
fn a_passed_check_the_reply_disowns_goes_to_the_judge() {
    assert_eq!(
        next(&disowned(), Taken::default(), ALL),
        Step::JudgeUnverified(vec![Suspicion::Unverified])
    );
    let judged = Taken {
        judged: true,
        ..Taken::default()
    };
    assert_eq!(next(&disowned(), judged, ALL), Step::Stop, "once");
    let no_judge = Allowed {
        judge: false,
        ..ALL
    };
    assert_eq!(next(&disowned(), Taken::default(), no_judge), Step::Stop);
    let harness_ran = Taken {
        checked: true,
        ..Taken::default()
    };
    assert_eq!(
        next(&disowned(), harness_ran, ALL),
        Step::Stop,
        "the harness's own check passed"
    );
}

#[test]
fn only_code_work_whose_check_passed_and_whose_reply_disowns_it_goes_to_the_judge() {
    let vouched = passed("Fixed the parser; `cargo test` passes.");
    assert_eq!(next(&vouched, Taken::default(), ALL), Step::Stop);
    let failed = Ended {
        signal: Some(OutcomeSignal::ChecksFailed),
        ..disowned()
    };
    assert_eq!(
        next(&failed, Taken::default(), ALL),
        Step::Stop,
        "a failed check already goes on"
    );
    let answer = Ended {
        category: PromptCategory::Question,
        ..disowned()
    };
    assert_eq!(next(&answer, Taken::default(), ALL), Step::Stop);
}

#[test]
fn answers_and_writing_go_straight_to_the_judge() {
    for category in [
        PromptCategory::Question,
        PromptCategory::Research,
        PromptCategory::Planning,
        PromptCategory::Writing,
        PromptCategory::Other,
    ] {
        let answer = Ended {
            category,
            ..ended(2, "I couldn't find it.")
        };
        assert_eq!(
            next(&answer, Taken::default(), REVIEWING),
            Step::JudgeUnverified(vec![Suspicion::GaveUp]),
            "{category:?}"
        );
        let plain = Ended {
            category,
            ..ended(0, "Here you go.")
        };
        assert_eq!(
            next(&plain, Taken::default(), ALL),
            Step::JudgeUnverified(Vec::new()),
            "{category:?}"
        );
    }
}
