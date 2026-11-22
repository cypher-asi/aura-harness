//! Reviews of work about to stand, and repairs a sign sends back.

use super::*;

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
        signal: Some(OutcomeSignal::ChecksFailed),
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
    let gave_up = ended(0, "I was unable to find the cause.");
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
        Step::JudgeUnverified(vec![Suspicion::NoChanges, Suspicion::GaveUp]),
        "a review that gave no verdict leaves it to the judge"
    );
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
