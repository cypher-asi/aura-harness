//! The blind judge's request, and what is read out of its reply.

use aura_core_types::{CompareArm, RefVerdict};

use super::prompt::prompt;
use super::{
    a_first, parse, reference_verdicts, request, unblind, winner, Attempt, Verdict, MAX_DIFF,
    MAX_REPLY, SYSTEM_PROMPT,
};

fn attempt(diff: &str, reply: &str) -> Attempt {
    Attempt {
        diff: diff.to_string(),
        reply: reply.to_string(),
    }
}

#[test]
fn verdicts_parse_from_the_last_winner_line() {
    assert_eq!(parse("Better tests.\nWINNER: A"), Some(Verdict::A));
    assert_eq!(parse("winner: b\n"), Some(Verdict::B));
    assert_eq!(parse("**WINNER: TIE**"), Some(Verdict::Tie));
    assert_eq!(parse("WINNER: `B`."), Some(Verdict::B));
    assert_eq!(
        parse("At first WINNER: A seemed right.\nWINNER: A\nOn reflection:\nWINNER: B"),
        Some(Verdict::B),
        "the last verdict line counts"
    );
    assert_eq!(parse("A is better"), None);
    assert_eq!(parse("WINNER: C"), None);
    assert_eq!(parse(""), None);
}

#[test]
fn a_winner_line_opens_its_line_and_the_last_readable_one_counts() {
    assert_eq!(parse("- WINNER: A"), None, "a list item is no winner line");
    assert_eq!(parse("Overall WINNER: B"), None);
    assert_eq!(parse("WINNER: A\nWINNER: C"), Some(Verdict::A));
    assert_eq!(parse("WINNER:B"), Some(Verdict::B));
    assert_eq!(parse("Winner: tie."), Some(Verdict::Tie));
}

#[test]
fn the_last_rating_line_counts_even_when_unreadable() {
    let reply = "A_VS_REFERENCE: BETTER\nA_VS_REFERENCE: maybe\nB_VS_REFERENCE: as-good";
    assert_eq!(
        reference_verdicts(reply, true),
        (None, Some(RefVerdict::AsGood))
    );
}

#[test]
fn the_winner_follows_the_order_the_judge_saw() {
    assert_eq!(winner(Verdict::A, true), Some(CompareArm::A));
    assert_eq!(winner(Verdict::A, false), Some(CompareArm::B));
    assert_eq!(winner(Verdict::B, true), Some(CompareArm::B));
    assert_eq!(winner(Verdict::B, false), Some(CompareArm::A));
    assert_eq!(winner(Verdict::Tie, true), None);
}

#[test]
fn the_order_is_random_per_pair_and_repeatable() {
    let orders: Vec<bool> = (0..64).map(|pair| a_first(42, pair)).collect();
    let first = orders.iter().filter(|g| **g).count();
    assert!(
        (16..=48).contains(&first),
        "about half the pairs show arm A first: {first}/64"
    );
    let again: Vec<bool> = (0..64).map(|pair| a_first(42, pair)).collect();
    assert_eq!(orders, again, "the same seed gives the same order");
    let other_seed: Vec<bool> = (0..64).map(|pair| a_first(43, pair)).collect();
    assert_ne!(orders, other_seed);
}

#[test]
fn the_judge_sees_no_arm_or_model_names() {
    let gate = attempt("+fn parse() {}", "Fixed the parser.");
    let other = attempt("+fn parse_all() {}", "Rewrote parsing.");
    for a_first in [true, false] {
        let built = request(
            "claude/claude-opus-4-7",
            "fix the parser",
            (&gate, &other),
            a_first,
            None,
            None,
        )
        .unwrap();
        let sent = format!("{}\n{}", built.system, built.messages[0].text_content());
        for name in ["gate", "Gate", "other", "opus", "sonnet", "claude"] {
            assert!(!sent.contains(name), "{name:?} leaked into:\n{sent}");
        }
        let (a, b) = sent.split_once("## Attempt B").unwrap();
        assert!(!sent.contains("Reference"));
        let (shown_a, shown_b) = if a_first {
            (&gate, &other)
        } else {
            (&other, &gate)
        };
        assert!(a.contains(&shown_a.diff) && a.contains(&shown_a.reply));
        assert!(b.contains(&shown_b.diff) && b.contains(&shown_b.reply));
        assert!(built.tools.is_empty());
    }
}

#[test]
fn the_reasoning_names_the_runs_not_the_order_they_were_shown_in() {
    let reply = "Attempt A wins; attempt B's reply overstates it. Attempts differ.\n\
                 Attempt AB is not a label, nor is reattempt A.\nWINNER: A";
    assert_eq!(
        unblind(reply, false, ("a-2", "b-2")),
        "b-2 wins; a-2's reply overstates it. Attempts differ.\n\
         Attempt AB is not a label, nor is reattempt A.\nWINNER: A"
    );
    assert!(unblind(reply, true, ("a-2", "b-2")).starts_with("a-2 wins; b-2's"));
    assert_eq!(
        unblind("é ttempt A — Attempt B", true, ("x", "y")),
        "é ttempt A — y"
    );
    assert!(
        SYSTEM_PROMPT.contains("\"Attempt A\" and \"Attempt B\" in full"),
        "the judge is asked for the labels unblind finds"
    );
}

#[test]
fn long_diffs_and_replies_are_cut_and_empty_ones_named() {
    let long = attempt(&"+x\n".repeat(MAX_DIFF), &"y".repeat(MAX_REPLY * 2));
    let text = prompt("task", &long, &attempt("", " "), None);
    assert!(text.contains("... (diff cut)"));
    assert!(text.contains("... (reply cut)"));
    assert!(text.contains("(no changes)"));
    assert!(text.contains("(no reply)"));
    assert!(text.len() < 2 * (MAX_DIFF + MAX_REPLY) + 1_000);
}

#[test]
fn a_reference_goes_before_the_attempts_and_asks_for_ratings() {
    let kept = attempt("+fn kept() {}", "Kept this.");
    let text = prompt(
        "fix the parser",
        &attempt("+a", "A"),
        &attempt("+b", "B"),
        Some(&kept),
    );
    let reference = text.find("## Reference (what the user kept)").unwrap();
    assert!(reference < text.find("## Attempt A").unwrap());
    assert!(text.contains("+fn kept() {}") && text.contains("Kept this."));
    assert!(text.contains("A_VS_REFERENCE") && text.contains("B_VS_REFERENCE"));
    let empty = prompt("t", &kept, &kept, Some(&attempt("", "")));
    assert!(empty.contains("(no changes)"), "{empty}");
}

#[test]
fn reference_ratings_follow_the_order_the_judge_saw() {
    let reply =
        "A matches what was kept.\nA_VS_REFERENCE: AS_GOOD\n**B_VS_REFERENCE: worse**\nWINNER: A";
    assert_eq!(parse(reply), Some(Verdict::A));
    assert_eq!(
        reference_verdicts(reply, true),
        (Some(RefVerdict::AsGood), Some(RefVerdict::Worse))
    );
    assert_eq!(
        reference_verdicts(reply, false),
        (Some(RefVerdict::Worse), Some(RefVerdict::AsGood)),
        "arm A was shown as B"
    );
    assert_eq!(
        reference_verdicts("A_VS_REFERENCE: as good\nB_VS_REFERENCE: better", true),
        (Some(RefVerdict::AsGood), Some(RefVerdict::Better))
    );
}

#[test]
fn a_reply_without_reference_lines_still_has_a_winner() {
    let reply = "B is cleaner.\nWINNER: B";
    assert_eq!(parse(reply), Some(Verdict::B));
    assert_eq!(reference_verdicts(reply, true), (None, None));
    assert_eq!(
        reference_verdicts("A_VS_REFERENCE: maybe\nWINNER: TIE", true),
        (None, None)
    );
}
