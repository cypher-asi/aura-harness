//! Gate's latest decisions.

use super::{learned_view, line};

#[test]
fn the_latest_decisions_are_listed_newest_last() {
    let text = learned_view();
    assert!(text.contains("Last 1 decisions"), "{text}");
    assert!(line(&text, "turn     question").ends_with("claude-haiku-4-5 (claude plan) proven"));
}
