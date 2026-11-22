//! What a sign earns, and which attempts count towards it.

use aura_core_types::OutcomeSignal;
use aura_gate_evidence::Flag;
use aura_gate_judge::turn::Answer;

use super::{tiers, Tier};
use crate::evaluate;
use crate::line::{Attempt, Line, Review, TruthSource};
use crate::samples::{attempt, clean, masked, proven, review, stale, truth, OPUS};

#[test]
fn a_sign_earns_a_review_by_being_wrong_more_often_than_its_attempts_are() {
    // Ten hard attempts, three wrong, all three among the five whose
    // check is stale: twice what chance gives those five.
    let mut lines = proven("s", 5, 3, &stale());
    lines.extend(proven("c", 5, 0, &clean()));
    let found = tiers(&lines);
    assert_eq!((found.known, found.wrong), (10, 3));
    let sign = found
        .signs
        .iter()
        .find(|sign| sign.flag == Flag::StaleCheck)
        .unwrap();
    assert_eq!((sign.raised, sign.wrong), (5, 3));
    assert!((sign.expected - 1.5).abs() < 1e-9, "{}", sign.expected);
    assert_eq!(sign.tier, Tier::Review);
    assert_eq!(found.of(Flag::StaleCheck), Tier::Review);
    assert_eq!(found.of(Flag::NoCheck), Tier::Log);
    assert_eq!(
        found.in_tier(&[Flag::NoCheck, Flag::StaleCheck], Tier::Review),
        [Flag::StaleCheck]
    );

    // One wrong attempt more among the clean ones and the sign is no
    // better than chance.
    let mut diluted = proven("s", 5, 3, &stale());
    diluted.extend(proven("c", 5, 2, &clean()));
    assert_eq!(tiers(&diluted).of(Flag::StaleCheck), Tier::Log);

    // Too few raises earn nothing, however wrong.
    let mut few = proven("s", 4, 4, &stale());
    few.extend(proven("c", 20, 0, &clean()));
    let found = tiers(&few);
    assert_eq!(found.of(Flag::StaleCheck), Tier::Log);
    assert_eq!(
        found.signs[Flag::ALL
            .iter()
            .position(|flag| *flag == Flag::StaleCheck)
            .unwrap()]
        .lacks(),
        "1 more raises"
    );
}

#[test]
fn a_sign_acts_once_most_of_what_it_is_raised_on_is_wrong() {
    let mut lines = proven("m", 10, 8, &masked());
    lines.extend(proven("c", 10, 0, &clean()));
    let found = tiers(&lines);
    assert_eq!(found.of(Flag::MaskedCheckWithFailures), Tier::Act);
    assert_eq!(found.of(Flag::ReplyUnverified), Tier::Act);
    let mut fewer = proven("m", 10, 7, &masked());
    fewer.extend(proven("c", 10, 0, &clean()));
    assert_eq!(
        tiers(&fewer).of(Flag::MaskedCheckWithFailures),
        Tier::Review
    );
    let rendered = evaluate(&lines).render();
    assert!(
        rendered.contains(
            "What each sign has earned, on the 20 routed attempts below the strongest tier whose work is known (8 wrong):"
        ),
        "{rendered}"
    );
    assert!(
        rendered.contains("masked_check_with_failures      10      8        4.0  acts"),
        "{rendered}"
    );
}

#[test]
fn tiers_count_routed_work_below_the_strongest_tier_and_what_reviews_sent_back() {
    let mut lines = proven("s", 5, 3, &stale());
    lines.extend(proven("c", 5, 0, &clean()));
    // Work on the strongest model, work Gate did not route and work
    // nothing proved say nothing of the signs.
    let opus = Attempt {
        model: OPUS.to_string(),
        ..attempt("o1", OutcomeSignal::ChecksPassed, stale())
    };
    let pinned = Attempt {
        routed: false,
        ..attempt("u1", OutcomeSignal::ChecksPassed, stale())
    };
    lines.extend([
        Line::Attempt(opus),
        Line::Truth(truth("o1", TruthSource::TaskCheck, true)),
        Line::Attempt(pinned),
        Line::Truth(truth("u1", TruthSource::TaskCheck, true)),
        Line::Attempt(attempt("n1", OutcomeSignal::ChecksPassed, stale())),
    ]);
    let found = tiers(&lines);
    assert_eq!((found.known, found.wrong), (10, 3));

    // An attempt a review sent back counts as wrong with the signs it
    // showed then, whatever became of it; a replayed review or one
    // only logged changes nothing.
    let sent_back = Review {
        acted: true,
        replayed: false,
        signs: vec![Flag::StaleCheck],
        ..review("n1", Answer::Model, "plain")
    };
    lines.push(Line::Review(sent_back));
    lines.push(Line::Review(review("c0", Answer::Model, "plain")));
    let found = tiers(&lines);
    assert_eq!((found.known, found.wrong), (11, 4));
    let sign = found
        .signs
        .iter()
        .find(|sign| sign.flag == Flag::StaleCheck)
        .unwrap();
    assert_eq!((sign.raised, sign.wrong), (6, 4));
}
