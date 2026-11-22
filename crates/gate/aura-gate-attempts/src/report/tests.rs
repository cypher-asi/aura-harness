//! The report as text.

use crate::evaluate;
use crate::samples::{flagged, tiered, RENDERED};

#[test]
fn a_report_renders_as_pinned() {
    assert_eq!(evaluate(&tiered()).render(), RENDERED);
}

#[test]
fn a_report_says_what_gate_let_stand() {
    let text = evaluate(&flagged()).render();
    assert!(
        text.contains(
            "6 attempts logged (6 routed by Gate), 5 later shown right or wrong: 3 right, 2 wrong"
        ),
        "{text}"
    );
    assert!(text.contains("Gate let stand 4 of them: 2 right, 2 wrong (1 it counted as passed, 1 it had nothing to judge by)"), "{text}");
    assert!(text.contains("masked_check_with_failures"), "{text}");
    assert!(text.contains("Compare #22 a-4 · ansible-d33bedc4 · claude/claude-sonnet-5-5 · gate: checks_passed · masked_check"), "{text}");
}

#[test]
fn an_empty_log_says_what_fills_it() {
    assert_eq!(
        evaluate(&[]).render(),
        "0 attempts logged (0 routed by Gate), 0 later shown right or wrong: 0 right, 0 wrong\n\
         Nothing to score yet: run /compare on benchmark tasks, whose own checks say whether each run's work was right.\n"
    );
}
