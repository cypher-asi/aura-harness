//! How each kind of line is written and read back, and what it is about.

use aura_core_types::OutcomeSignal;

use super::{Kind, Line, TruthSource};
use crate::samples::{attempt, clean, truth, written, LOG};

#[test]
fn each_kind_of_line_is_written_as_the_sample_log_holds_it() {
    let written = written();
    let json: Vec<String> = written
        .iter()
        .map(|line| serde_json::to_string(line).unwrap())
        .collect();
    let held: Vec<&str> = LOG.lines().take(written.len()).collect();
    assert_eq!(json, held);
}

#[test]
fn a_line_is_keyed_by_the_attempt_it_is_about_and_its_kind() {
    let written = written();
    let keys: Vec<(&str, &str, Kind)> = written.iter().map(Line::key).collect();
    assert_eq!(
        keys,
        [
            ("a1b2", "c3d4", Kind::Attempt),
            ("aa", "p2", Kind::Attempt),
            ("aa", "p1", Kind::Truth),
            ("aa", "p1", Kind::Truth),
            ("aa", "p1", Kind::Truth),
            ("aa", "p1", Kind::Review),
            ("aa", "p1", Kind::Review),
        ]
    );
}

#[test]
fn the_users_signals_are_truths() {
    assert_eq!(
        TruthSource::of_user(OutcomeSignal::Rewind),
        Some(TruthSource::Rewind)
    );
    assert_eq!(
        TruthSource::of_user(OutcomeSignal::RetryStronger),
        Some(TruthSource::RetryStronger)
    );
    assert_eq!(TruthSource::of_user(OutcomeSignal::ChecksPassed), None);
    assert_eq!(TruthSource::of_user(OutcomeSignal::JudgedFail), None);
}

#[test]
fn older_lines_still_read() {
    let mut line = serde_json::to_value(Line::Attempt(attempt(
        "p1",
        OutcomeSignal::ChecksPassed,
        clean(),
    )))
    .unwrap();
    line["signal"] = "none".into();
    let Line::Attempt(read) = serde_json::from_value(line).unwrap() else {
        panic!("an attempt");
    };
    assert_eq!(read.signal, OutcomeSignal::Unavailable);
    let mut picked =
        serde_json::to_value(Line::Truth(truth("p1", TruthSource::Rewind, true))).unwrap();
    picked["source"] = "picked".into();
    assert!(serde_json::from_value::<Line>(picked).is_ok());
}

#[test]
fn a_review_keeps_an_unknown_price_and_older_ones_read_as_priced() {
    use crate::samples::review;
    use aura_gate_judge::turn::Answer;
    let mut unpriced = review("p1", Answer::Ok, "plain");
    unpriced.cost_usd = None;
    let json = serde_json::to_value(Line::Review(unpriced.clone())).unwrap();
    assert!(json["cost_usd"].is_null(), "{json}");
    assert_eq!(
        serde_json::from_value::<Line>(json).unwrap(),
        Line::Review(unpriced)
    );
    let older = serde_json::to_value(Line::Review(review("p1", Answer::Ok, "plain"))).unwrap();
    assert_eq!(older["cost_usd"], 0.05);
    let Line::Review(read) = serde_json::from_value(older).unwrap() else {
        panic!("a review");
    };
    assert_eq!(read.cost_usd, Some(0.05));
}
