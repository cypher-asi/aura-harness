//! Reviews against the acceptance criteria written down before the work
//! started.

use super::*;

/// Every step allowed, reviews off, reviews against criteria only
/// logged.
const LOGGED: Allowed = Allowed {
    criteria: ReviewMode::Log,
    ..ALL
};

/// Every step allowed, reviews off, reviews against criteria acted on.
const ACTING: Allowed = Allowed {
    criteria: ReviewMode::On,
    ..ALL
};

/// Code work with criteria whose check passed, its reply vouching for
/// it.
fn held() -> Ended<'static> {
    Ended {
        criteria: true,
        ..passed("All tests pass.")
    }
}

/// Code work with criteria whose check passed, its reply disowning it.
fn disowned() -> Ended<'static> {
    Ended {
        criteria: true,
        ..passed("I rewrote it, but it isn't verified.")
    }
}

#[test]
fn logged_criteria_have_work_about_to_stand_reviewed_for_the_log() {
    assert_eq!(
        next(&held(), Taken::default(), LOGGED),
        review(&[Why::Criteria], false)
    );
    let reviewed = Taken {
        reviews: 1,
        ..Taken::default()
    };
    assert_eq!(next(&held(), reviewed, LOGGED), Step::Stop, "once");
    let unwritten = Ended {
        criteria: false,
        ..held()
    };
    assert_eq!(next(&unwritten, Taken::default(), LOGGED), Step::Stop);
    assert_eq!(
        next(&disowned(), Taken::default(), LOGGED),
        Step::JudgeUnverified(vec![Suspicion::Unverified]),
        "the judge still sees work that looks wrong"
    );
}

#[test]
fn a_logged_review_against_criteria_joins_another_and_yields_to_one_that_acts() {
    let hard = Ended {
        difficulty: Some(Difficulty::Hard),
        ..held()
    };
    let both = Allowed {
        review: ReviewMode::Log,
        ..LOGGED
    };
    assert_eq!(
        next(&hard, Taken::default(), both),
        review(&[Why::Hard, Why::Criteria], false),
        "one review for both"
    );
    let acting_reviews = Allowed {
        review: ReviewMode::On,
        ..LOGGED
    };
    assert_eq!(
        next(&hard, Taken::default(), acting_reviews),
        review(&[Why::Hard], true),
        "a review that acts goes first"
    );
    assert_eq!(
        next(&held(), Taken::default(), acting_reviews),
        review(&[Why::Criteria], false),
        "nothing else called for a review"
    );
}

#[test]
fn criteria_that_act_have_every_attempt_reviewed() {
    assert_eq!(
        next(&held(), Taken::default(), ACTING),
        review(&[Why::Criteria], true)
    );
    let sent_back = Taken {
        reviews: 1,
        repaired: true,
        ..Taken::default()
    };
    assert_eq!(
        next(&held(), sent_back, ACTING),
        review(&[Why::Criteria], true),
        "again after it was sent back"
    );
    let reviewed = Taken {
        reviews: 1,
        ..Taken::default()
    };
    assert_eq!(
        next(&disowned(), reviewed, ACTING),
        Step::Stop,
        "the review stands in for the judge"
    );
    let signs = [Flag::MaskedCheckWithFailures];
    let flagged = Ended {
        acting_signs: &signs,
        ..held()
    };
    assert_eq!(
        next(&flagged, Taken::default(), ACTING),
        review(&[Why::Criteria], true),
        "signs act only when reviews do"
    );
}

#[test]
fn criteria_that_act_have_unchecked_work_reviewed_once_it_was_checked() {
    let unchecked = Ended {
        criteria: true,
        ..ended(2, "Done.")
    };
    let checked = Taken {
        nudged: true,
        checked: true,
        ..Taken::default()
    };
    assert_eq!(
        next(&unchecked, Taken::default(), ACTING),
        Step::Nudge(Vec::new())
    );
    assert_eq!(
        next(&unchecked, checked, ACTING),
        review(&[Why::Criteria], true)
    );
    assert_eq!(Why::Criteria.name(), "criteria");
}
