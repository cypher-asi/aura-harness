//! What a judge of a turn answered, read out of its reply.

use aura_core_types::OutcomeSignal;
use serde::{Deserialize, Serialize};

use crate::request::after_thinking;

/// What a judge's verdict line holds, in upper case, before its answer.
const VERDICT: &str = "VERDICT:";

/// What the judge found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// Whether the model's own work caused the failure.
    pub at_fault: bool,
    /// The judge's account of what went wrong.
    pub reasons: String,
}

/// What a judge answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    /// The model's work is wrong, incomplete or missing.
    Model,
    /// The work is done.
    Ok,
    /// Something outside the work got in the way.
    Other,
}

impl Answer {
    /// The outcome of work no check vouched for that the judge answered
    /// this of.
    #[must_use]
    pub const fn verdict(self) -> OutcomeSignal {
        match self {
            Self::Model => OutcomeSignal::JudgedFail,
            Self::Ok => OutcomeSignal::JudgedPass,
            Self::Other => OutcomeSignal::JudgedExcused,
        }
    }
}

/// The answer in the judge's `reply`: its last line holding `VERDICT:`
/// (in any case), with the text before it as the reasons. `None` when
/// the reply has no such line or the line names no answer.
#[must_use]
pub fn parse_answer(reply: &str) -> Option<(Answer, String)> {
    let lines: Vec<&str> = after_thinking(reply).lines().collect();
    let at = lines
        .iter()
        .rposition(|line| line.to_ascii_uppercase().contains(VERDICT))?;
    let line = lines[at].to_ascii_uppercase();
    let answer = match line
        .split(VERDICT)
        .nth(1)?
        .trim_matches(|c: char| !c.is_ascii_alphabetic())
    {
        "MODEL" => Answer::Model,
        "OK" => Answer::Ok,
        "OTHER" => Answer::Other,
        _ => return None,
    };
    Some((answer, lines[..at].join("\n").trim().to_string()))
}
