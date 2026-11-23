//! How Gate's verdicts, the flags and the reviews are scored, over the
//! whole log or some of it.

use aura_core_types::OutcomeSignal;
use aura_gate_evidence::{Evidence, Flag};
use aura_gate_judge::turn::Answer;

use super::{evaluate, evaluate_where, Filter, Report, ReviewScore, Scored, Verdicts};
use crate::line::{Line, Review, TruthSource};
use crate::samples::{
    attempt, clean, flagged, masked, review, sample, truth, OPUS, REPORT, SONNET,
};

#[test]
fn a_report_scores_gate_and_each_flag_against_what_happened() {
    let report = evaluate(&flagged());
    assert_eq!((report.attempts, report.routed), (6, 6));
    assert_eq!(
        report.verdicts,
        Verdicts {
            known: 5,
            right: 3,
            false_passes: 1,
            unjudged_failures: 1,
            false_failures: 1,
        }
    );
    assert_eq!((report.stood, report.stood_wrong), (4, 2));
    for (flag, scored) in [
        (Flag::MaskedCheck, (2, 1, 1)),
        (Flag::MaskedCheckWithFailures, (1, 1, 0)),
        (Flag::ReplyUnverified, (1, 1, 0)),
        (Flag::NoCheck, (1, 1, 0)),
        (Flag::HitLimit, (0, 0, 0)),
    ] {
        let score = report.flags.iter().find(|s| s.flag == flag).unwrap();
        let found = (score.raised, score.caught, score.false_alarms);
        assert_eq!(found, scored, "{flag:?}");
    }
    assert_eq!(report.models[SONNET].known, 4);
    assert_eq!(report.scored.len(), 4);
    let missed: Vec<&Scored> = report.missed().collect();
    assert_eq!(missed.len(), 2);
    assert_eq!(missed[0].run, "Compare #22 a-4");
    assert_eq!(
        missed[0].flags,
        [
            Flag::MaskedCheck,
            Flag::CheckOutputFailures,
            Flag::MaskedCheckWithFailures,
            Flag::ReplyUnverified
        ]
    );
}

#[test]
fn a_report_can_be_about_some_attempts_only() {
    let mut pinned = attempt("p2", OutcomeSignal::ChecksPassed, clean());
    pinned.routed = false;
    let mut other = truth("p2", TruthSource::TaskCheck, true);
    other.task = Some("coding/rename-symbol".to_string());
    let lines = [
        Line::Attempt(attempt("p1", OutcomeSignal::ChecksPassed, masked())),
        Line::Truth(truth("p1", TruthSource::TaskCheck, false)),
        Line::Attempt(pinned),
        Line::Truth(other),
        Line::Attempt(attempt("p3", OutcomeSignal::ChecksPassed, clean())),
    ];
    let all = evaluate(&lines);
    assert_eq!((all.attempts, all.stood, all.stood_wrong), (3, 2, 1));
    let routed = evaluate_where(
        &lines,
        &Filter {
            routed: true,
            ..Filter::default()
        },
    );
    assert_eq!(
        (routed.attempts, routed.stood, routed.stood_wrong),
        (2, 1, 1)
    );
    let task = evaluate_where(
        &lines,
        &Filter {
            task: Some("coding/".to_string()),
            ..Filter::default()
        },
    );
    assert_eq!((task.attempts, task.stood, task.stood_wrong), (1, 1, 0));
    let mut unjudged = lines.to_vec();
    unjudged.push(Line::Attempt(attempt(
        "p4",
        OutcomeSignal::Unavailable,
        Evidence::default(),
    )));
    unjudged.push(Line::Truth(truth("p4", TruthSource::Rewind, false)));
    let passed = Filter {
        passed: true,
        ..Filter::default()
    };
    assert_eq!(evaluate(&unjudged).stood, 3);
    assert_eq!(evaluate_where(&unjudged, &passed).stood, 2);
}

/// Three attempts, two of them proved, reviewed plainly and by thinking
/// first, and a review of an attempt the log does not hold.
fn reviewed() -> Vec<Line> {
    vec![
        Line::Attempt(attempt("p1", OutcomeSignal::ChecksPassed, clean())),
        Line::Truth(truth("p1", TruthSource::TaskCheck, false)),
        Line::Attempt(attempt("p2", OutcomeSignal::ChecksPassed, clean())),
        Line::Truth(truth("p2", TruthSource::TaskCheck, true)),
        Line::Attempt(attempt("p3", OutcomeSignal::ChecksPassed, clean())),
        Line::Review(review("p1", Answer::Model, "plain")),
        Line::Review(review("p2", Answer::Ok, "plain")),
        Line::Review(review("p3", Answer::Model, "plain")),
        // Asked twice the same way, the last answer counts.
        Line::Review(review("p1", Answer::Ok, "thinking")),
        Line::Review(review("p1", Answer::Model, "thinking")),
        Line::Review(review("p2", Answer::Model, "thinking")),
        // A review of an attempt the log does not hold is left out.
        Line::Review(review("gone", Answer::Model, "plain")),
    ]
}

/// [`reviewed`] with Sonnet asked plainly about the first attempt too.
fn reviewed_by_both() -> Vec<Line> {
    let mut lines = reviewed();
    lines.push(Line::Review(Review {
        judge: SONNET.to_string(),
        ..review("p1", Answer::Ok, "plain")
    }));
    lines
}

/// The score of the reviews `judge` was asked as `variant` in `report`.
fn score_of<'a>(report: &'a Report, variant: &str, judge: &str) -> &'a ReviewScore {
    report
        .reviews
        .iter()
        .find(|score| score.variant == variant && score.judge == judge)
        .unwrap()
}

#[test]
fn reviews_are_scored_per_way_of_asking_against_what_happened() {
    let report = evaluate(&reviewed());
    assert_eq!(report.reviews.len(), 2);
    let plain = score_of(&report, "plain", OPUS);
    assert!(plain.replayed);
    assert_eq!(
        (plain.reviewed, plain.known, plain.wrong),
        (3, 2, 1),
        "{plain:?}"
    );
    assert_eq!(
        (plain.unfinished, plain.caught, plain.false_alarms),
        (1, 1, 0)
    );
    assert!((plain.cost_usd - 0.15).abs() < 1e-9);
    let thinking = score_of(&report, "thinking", OPUS);
    assert_eq!((thinking.reviewed, thinking.known), (2, 2));
    assert_eq!(
        (thinking.unfinished, thinking.caught, thinking.false_alarms),
        (2, 1, 1)
    );
    let rendered = report.render();
    assert!(
        rendered.contains("Reviews of work that was about to stand:"),
        "{rendered}"
    );
    assert!(rendered.contains("plain (replayed)"), "{rendered}");
}

#[test]
fn another_reviewer_asked_the_same_way_is_scored_on_its_own() {
    let both = evaluate(&reviewed_by_both());
    assert_eq!(both.reviews.len(), 3);
    let alone = evaluate(&reviewed());
    assert_eq!(
        score_of(&both, "plain", OPUS),
        score_of(&alone, "plain", OPUS)
    );
    let sonnet = score_of(&both, "plain", SONNET);
    assert_eq!((sonnet.reviewed, sonnet.wrong, sonnet.caught), (1, 1, 0));
    let rendered = both.render();
    assert!(rendered.contains("claude-sonnet-5-5"), "{rendered}");
}

#[test]
fn a_report_about_some_attempts_scores_the_reviews_of_those_only() {
    let failed = evaluate_where(
        &reviewed_by_both(),
        &Filter {
            task: Some("nothing".to_string()),
            ..Filter::default()
        },
    );
    assert!(failed.reviews.is_empty());
    let passed = evaluate_where(
        &sample(),
        &Filter {
            passed: true,
            ..Filter::default()
        },
    );
    assert_eq!(
        serde_json::to_value(&passed.reviews).unwrap(),
        serde_json::json!([
            {
                "variant": "plain", "judge": OPUS, "replayed": true,
                "reviewed": 2, "known": 2, "wrong": 1,
                "unfinished": 1, "caught": 1, "false_alarms": 0, "cost_usd": 0.1
            },
            {
                "variant": "thinking", "judge": SONNET, "replayed": false,
                "reviewed": 1, "known": 1, "wrong": 1,
                "unfinished": 1, "caught": 1, "false_alarms": 0, "cost_usd": 0.125
            }
        ]),
        "the review of the attempt Gate had nothing to judge by is left out"
    );
}

#[test]
fn a_report_serializes_as_pinned() {
    let json = serde_json::to_string_pretty(&evaluate(&sample())).unwrap();
    assert_eq!(format!("{json}\n"), REPORT);
}
