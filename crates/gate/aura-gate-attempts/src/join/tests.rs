//! Which of an attempt's lines and truths count, and the attempts that
//! stood.

use aura_core_types::OutcomeSignal;
use aura_gate_evidence::Flag;

use super::stood;
use crate::line::{Attempt, Line, TruthSource};
use crate::samples::{attempt, clean, masked, truth, OPUS, SONNET};
use crate::{below_strongest, evaluate};

#[test]
fn an_attempt_logged_twice_counts_once_and_a_task_check_outranks_the_user() {
    let mut rebuilt = attempt("p1", OutcomeSignal::ChecksPassed, clean());
    rebuilt.backfilled = true;
    let lines = [
        Line::Attempt(rebuilt.clone()),
        Line::Attempt(attempt("p1", OutcomeSignal::Unavailable, clean())),
        Line::Attempt(attempt("p1", OutcomeSignal::ChecksPassed, masked())),
        Line::Attempt(rebuilt),
        Line::Truth(truth("p1", TruthSource::TaskCheck, false)),
        Line::Truth(truth("p1", TruthSource::Picked, true)),
    ];
    let report = evaluate(&lines);
    assert_eq!(report.attempts, 1);
    assert_eq!(
        (
            report.verdicts.false_passes,
            report.verdicts.unjudged_failures
        ),
        (1, 0),
        "the task's check counts, against what Gate last made of the attempt"
    );
    assert_eq!(
        report.missed().next().unwrap().flags.first(),
        Some(&Flag::MaskedCheck),
        "the last line written while the app ran counts"
    );
}

#[test]
fn the_attempts_that_stood_are_listed_once_with_what_proved_them() {
    let failed = Attempt {
        success: Some(false),
        ..attempt("p3", OutcomeSignal::ChecksFailed, clean())
    };
    let lines = [
        Line::Attempt(attempt("p1", OutcomeSignal::ChecksPassed, clean())),
        Line::Attempt(attempt("p1", OutcomeSignal::ChecksPassed, masked())),
        Line::Truth(truth("p1", TruthSource::TaskCheck, false)),
        Line::Attempt(attempt("p2", OutcomeSignal::ChecksPassed, clean())),
        Line::Attempt(failed),
        Line::Truth(truth("p3", TruthSource::TaskCheck, false)),
    ];
    let stood = stood(&lines);
    assert_eq!(stood.len(), 1, "p2 has no truth and p3 did not stand");
    assert_eq!(stood[0].attempt.prompt_tx, "p1");
    assert_eq!(stood[0].attempt.evidence, masked(), "as last logged");
    assert!(!stood[0].truth.passed);
    assert_eq!(stood[0].attempts_by_agent, 3);
    assert!(below_strongest(SONNET));
    assert!(!below_strongest(OPUS));
}
