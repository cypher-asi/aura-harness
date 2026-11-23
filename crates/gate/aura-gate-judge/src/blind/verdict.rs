//! What the blind judge said: the winner, how each attempt compares with
//! the reference, and the arm each favours.

use aura_core_types::{CompareArm, RefVerdict};
use aura_model::ModelResponse;

/// What the judge said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Attempt A is better.
    A,
    /// Attempt B is better.
    B,
    /// Neither is meaningfully better.
    Tie,
}

/// The judge's reply text.
#[must_use]
pub fn reply_text(response: &ModelResponse) -> String {
    response.message.text_content()
}

/// The verdict in the judge's `reply`: its last line of the form
/// `WINNER: A|B|TIE` (in any case, optionally in bold). `None` when no
/// line says.
#[must_use]
pub fn parse(reply: &str) -> Option<Verdict> {
    reply.lines().rev().find_map(|line| {
        match labelled(line, "winner")?.to_ascii_uppercase().as_str() {
            "A" => Some(Verdict::A),
            "B" => Some(Verdict::B),
            "TIE" => Some(Verdict::Tie),
            _ => None,
        }
    })
}

/// The arm `verdict` favours when the arm A run was shown first or not;
/// `None` for a tie.
#[must_use]
pub const fn winner(verdict: Verdict, a_first: bool) -> Option<CompareArm> {
    match (verdict, a_first) {
        (Verdict::Tie, _) => None,
        (Verdict::A, true) | (Verdict::B, false) => Some(CompareArm::A),
        (Verdict::A, false) | (Verdict::B, true) => Some(CompareArm::B),
    }
}

/// How the judge rated each arm against the reference, as `(arm A, arm
/// B)`, given whether the arm A run was shown first. Either is `None`
/// when its line is missing or unreadable: the last line labelled for
/// an attempt is its rating.
#[must_use]
pub fn reference_verdicts(reply: &str, a_first: bool) -> (Option<RefVerdict>, Option<RefVerdict>) {
    let rated = |label: &str| {
        reply
            .lines()
            .rev()
            .find_map(|line| labelled(line, label))
            .and_then(ref_verdict)
    };
    let (shown_a, shown_b) = (rated("A_VS_REFERENCE"), rated("B_VS_REFERENCE"));
    if a_first {
        (shown_a, shown_b)
    } else {
        (shown_b, shown_a)
    }
}

/// The value of `line` when it is labelled `label`: `LABEL: value`, in
/// any case, optionally in bold, with bold, backticks or a full stop
/// around the value dropped.
fn labelled<'a>(line: &'a str, label: &str) -> Option<&'a str> {
    let line = line.trim().trim_matches('*').trim();
    let (name, value) = line.split_once(':')?;
    name.trim()
        .eq_ignore_ascii_case(label)
        .then(|| value.trim().trim_matches(['*', '`', '.']))
}

/// The rating `value` names, in any case, with spaces or hyphens for
/// underscores.
fn ref_verdict(value: &str) -> Option<RefVerdict> {
    match value.to_ascii_uppercase().replace([' ', '-'], "_").as_str() {
        "BETTER" => Some(RefVerdict::Better),
        "AS_GOOD" => Some(RefVerdict::AsGood),
        "WORSE" => Some(RefVerdict::Worse),
        _ => None,
    }
}
