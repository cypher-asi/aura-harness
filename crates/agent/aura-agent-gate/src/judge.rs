//! The blind judge of `/compare` and the BENCH tab: a frontier model sees
//! one run of arm A and one of arm B (the task, each run's diff and final
//! reply) in random order, without arm or model names, and picks the
//! better one or calls a tie. Ported from `evals/gate_cost/judge.py`.
//!
//! When the work replays an earlier session, the judge also sees what the
//! user kept from it (its diff and last reply) as a reference, and rates
//! each attempt against it: better, as good, or worse.
//!
//! This module builds the request, decides the order and reads the
//! verdict; the caller sends the request through an agent's kernel, so
//! the call is recorded like any other model call.

use aura_core_types::{CompareArm, RefVerdict};
use aura_model::{
    Message, ModelCallPurpose, ModelRequest, ModelRequestKind, ModelResponse, ReasonerError,
    ThinkingEffort, ToolChoice,
};

use crate::router::{Random, SplitMix64};

/// Longest diff the judge sees per attempt, in characters.
pub const MAX_DIFF: usize = 12_000;

/// Longest final reply the judge sees per attempt, in characters.
pub const MAX_REPLY: usize = 4_000;

/// System prompt of every judge request; tells them apart from the
/// agent's own calls.
pub const SYSTEM_PROMPT: &str = "\
You are reviewing two attempts at the same coding task, made by different agents in identical \
copies of the same repository. Both may have passed the task's automated checks, so judge what \
the checks miss:

- correctness and completeness against everything the user asked, including edge cases;
- scope: no changes nobody asked for;
- code quality: clear, idiomatic, consistent with the surrounding code;
- the final reply: accurate about what was done, no false claims, useful to the user.

Explain your judgment in two to four sentences, naming the concrete differences that decided \
it. Always call them \"Attempt A\" and \"Attempt B\" in full, never a bare A or B. Then end with one line that is exactly `WINNER: A`, `WINNER: B` or `WINNER: TIE` (TIE only \
when neither is meaningfully better).";

/// What the judge sees of one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    /// The run's changes as a unified diff; empty when it changed nothing.
    pub diff: String,
    /// The run's final reply.
    pub reply: String,
}

/// `text` cut to `max` characters, saying so when it was cut.
pub(crate) fn cut(text: &str, max: usize, what: &str) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max).collect();
    format!("{kept}\n... ({what} cut)")
}

/// What the judge is told when there is a reference.
const REFERENCE_INSTRUCTIONS: &str = "\
The reference below is what the user kept when this work was first done: a known-acceptable \
result, not necessarily the only correct one. Besides the winner, rate each attempt against it. \
Before the WINNER line, add two lines, each exactly `A_VS_REFERENCE: BETTER`, `AS_GOOD` or \
`WORSE`, and the same for `B_VS_REFERENCE`.";

/// The judge's message for the attempts at `task`, `first` shown as A,
/// with what the user kept from the original work when there is a
/// `reference`. Nothing in it says which arm or model made either
/// attempt.
#[must_use]
pub fn prompt(
    task: &str,
    first: &Attempt,
    second: &Attempt,
    reference: Option<&Attempt>,
) -> String {
    let diff = |attempt: &Attempt| {
        if attempt.diff.trim().is_empty() {
            "(no changes)".to_string()
        } else {
            cut(&attempt.diff, MAX_DIFF, "diff")
        }
    };
    let reply = |attempt: &Attempt| {
        if attempt.reply.trim().is_empty() {
            "(no reply)".to_string()
        } else {
            cut(&attempt.reply, MAX_REPLY, "reply")
        }
    };
    let reference = reference.map_or_else(String::new, |reference| {
        format!(
            "{REFERENCE_INSTRUCTIONS}\n\n## Reference (what the user kept)\n\n\
             ### Changes\n```diff\n{}\n```\n\n### Final reply\n{}\n\n",
            diff(reference),
            reply(reference),
        )
    });
    format!(
        "## The user's message\n\n{task}\n\n{reference}\
         ## Attempt A\n\n### Changes\n```diff\n{}\n```\n\n### Final reply\n{}\n\n\
         ## Attempt B\n\n### Changes\n```diff\n{}\n```\n\n### Final reply\n{}",
        diff(first),
        reply(first),
        diff(second),
        reply(second),
    )
}

/// Whether the arm A run of pair `pair` is shown as attempt A. The order
/// is random but the same for the same `seed` and pair, so a judgment can
/// be repeated.
#[must_use]
pub fn a_first(seed: u64, pair: usize) -> bool {
    let pair = u64::try_from(pair).unwrap_or(u64::MAX);
    let mut rng = SplitMix64::seeded(seed ^ pair.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    rng.unit() < 0.5
}

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

/// The judge's `reply` with the attempts it saw named as the runs they
/// were, `names` being the arm A run's and the arm B run's: "Attempt A"
/// (or "attempt A") is the arm A run when it was shown first, else the
/// arm B run, and so on. The order is random, so its own labels say
/// nothing about the arms.
#[must_use]
pub fn unblind(reply: &str, a_first: bool, names: (&str, &str)) -> String {
    let (shown_a, shown_b) = if a_first { names } else { (names.1, names.0) };
    let mut out = String::with_capacity(reply.len());
    let mut rest = reply;
    while let Some(at) = rest.find("ttempt ") {
        let start = at.saturating_sub(1);
        let word = if at > 0 && rest.is_char_boundary(start) {
            &rest[start..]
        } else {
            ""
        };
        let label = word
            .strip_prefix("Attempt ")
            .or_else(|| word.strip_prefix("attempt "))
            .and_then(|after| {
                let mut chars = after.chars();
                let label = chars.next()?;
                let ends = chars.next().is_none_or(|c| !c.is_alphanumeric());
                let before = rest[..start].chars().next_back();
                let starts = before.is_none_or(|c| !c.is_alphanumeric());
                (starts && ends).then_some(label)
            });
        let name = match label {
            Some('A') => Some(shown_a),
            Some('B') => Some(shown_b),
            _ => None,
        };
        match name {
            Some(name) => {
                out.push_str(&rest[..start]);
                out.push_str(name);
                rest = &rest[start + "Attempt A".len()..];
            }
            _ => {
                let end = at + "ttempt ".len();
                out.push_str(&rest[..end]);
                rest = &rest[end..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The value of the last line of `reply` labelled `label` (`LABEL: value`,
/// in any case, optionally in bold or backticks).
fn labelled<'a>(reply: &'a str, label: &str) -> Option<&'a str> {
    reply.lines().rev().find_map(|line| {
        let line = line.trim().trim_matches('*').trim();
        let (name, value) = line.split_once(':')?;
        name.trim().eq_ignore_ascii_case(label).then(|| {
            value
                .trim()
                .trim_matches(|c: char| c == '*' || c == '`' || c == '.')
        })
    })
}

fn ref_verdict(value: &str) -> Option<RefVerdict> {
    match value.to_ascii_uppercase().replace([' ', '-'], "_").as_str() {
        "BETTER" => Some(RefVerdict::Better),
        "AS_GOOD" => Some(RefVerdict::AsGood),
        "WORSE" => Some(RefVerdict::Worse),
        _ => None,
    }
}

/// How the judge rated each arm against the reference, as `(arm A, arm
/// B)`, given whether the arm A run was shown first. Either is `None`
/// when its line is missing or unreadable.
#[must_use]
pub fn reference_verdicts(reply: &str, a_first: bool) -> (Option<RefVerdict>, Option<RefVerdict>) {
    let shown_a = labelled(reply, "A_VS_REFERENCE").and_then(ref_verdict);
    let shown_b = labelled(reply, "B_VS_REFERENCE").and_then(ref_verdict);
    if a_first {
        (shown_a, shown_b)
    } else {
        (shown_b, shown_a)
    }
}

/// The request asking `model` to judge `a` against `b` on `task`, in the
/// order `a_first` says, against `reference` when there is one, sent with
/// the user's `auth_token` when the route needs one.
///
/// # Errors
/// Returns the builder's error when `model` is empty.
pub fn request(
    model: &str,
    task: &str,
    (a, b): (&Attempt, &Attempt),
    a_first: bool,
    reference: Option<&Attempt>,
    auth_token: Option<String>,
) -> Result<ModelRequest, ReasonerError> {
    let (first, second) = if a_first { (a, b) } else { (b, a) };
    ModelRequest::builder(model, SYSTEM_PROMPT)
        .message(Message::user(prompt(task, first, second, reference)))
        .tool_choice(ToolChoice::None)
        .thinking_effort(Some(ThinkingEffort::Off))
        .auth_token(auth_token)
        .request_kind(ModelRequestKind::Auxiliary)
        .purpose(ModelCallPurpose::Judge)
        .try_build()
}

/// The judge's reply text.
#[must_use]
pub fn reply_text(response: &ModelResponse) -> String {
    response.message.text_content()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let reply = "A matches what was kept.\nA_VS_REFERENCE: AS_GOOD\n**B_VS_REFERENCE: worse**\nWINNER: A";
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
}
