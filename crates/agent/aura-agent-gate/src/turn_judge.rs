//! The judge of a failed turn: a strong model sees what the user asked,
//! what the turn changed, its final reply and how it failed, and says
//! whether the model that did the work was at fault.
//!
//! Gate only calls it for work that already failed (an error, or its
//! last test or build failing), so its cost is paid when there is
//! something to learn. A failure the judge puts down to the model counts
//! against it and sends the work on to a stronger one; a failure it puts
//! down to something else (the environment, a check that was failing
//! anyway) counts for nothing and the work stays where it is.
//!
//! The same judge reviews work that ran no check but looks wrong (see
//! [`crate::followup`]), through [`unverified_request`]: there it may
//! also find the work done (`OK`).
//!
//! This module builds the request and reads the verdict; the caller
//! sends the request through the agent's kernel, so the call is recorded
//! like any other model call.

use aura_model::model_catalog::infer_tier;
use aura_model::{
    Message, ModelCallPurpose, ModelRequest, ModelRequestKind, ReasonerError, ThinkingEffort,
    ToolChoice,
};

use crate::followup::Suspicion;
use crate::judge::{cut, MAX_DIFF, MAX_REPLY};
use crate::router::Candidate;

/// Longest request shown to the judge, in characters.
const MAX_TASK: usize = 8_000;

/// Longest account of the failure shown to the judge, in characters; the
/// end of a test run says what failed.
pub const MAX_FAILURE: usize = 3_000;

/// Room for a few sentences and the verdict line.
const MAX_REPLY_TOKENS: u32 = 512;

/// System prompt of every request to the judge of failed turns; tells
/// them apart from the agent's own calls.
pub const SYSTEM_PROMPT: &str = "\
You are reviewing an attempt at a task a user gave a coding agent. The attempt failed: its last \
test or build command failed, or the run ended in an error. You see the user's request, the \
changes the attempt made to files as a unified diff (changes made through shell commands are \
not shown), its final reply, and how it failed. Decide what caused the failure:

- MODEL: the attempt's own work is wrong or incomplete. It misunderstood the request, made a \
wrong or partial change, or gave up. A more capable model would likely have done better.
- OTHER: the failure does not come from the attempt's work. The environment got in the way (a \
missing tool or dependency, no network, a timeout), the check was already failing for reasons \
unrelated to the request, the check ran before the fix and was not run again while the change \
itself is right, or the request cannot be carried out as asked.

When you cannot tell, answer MODEL. Say in one to three sentences what went wrong, concretely \
enough to help whoever takes the work over. Then end with one line that is exactly \
`VERDICT: MODEL` or `VERDICT: OTHER`.";

/// System prompt of every request to the judge of unverified turns:
/// attempts that ran no check and look wrong (see
/// [`crate::followup::suspicions`]).
pub const UNVERIFIED_SYSTEM_PROMPT: &str = "\
You are reviewing an attempt at a task a user gave a coding agent. The attempt ran no test or \
build that checks its work, and something about it looks wrong. You see the user's request, the \
changes the attempt made to files as a unified diff (changes made through shell commands are \
not shown, but the commands it ran are listed), its final reply, and what looks wrong. Decide \
whether the work is done:

- MODEL: the attempt's own work is wrong, incomplete or missing. It misunderstood the request, \
made a wrong or partial change, changed nothing when a change was needed, or gave up. A more \
capable model would likely have done better.
- OK: the work looks complete and correct for what was asked, or no change was needed and the \
reply shows why.
- OTHER: the work could not be done for reasons outside the attempt: the environment got in the \
way, or the request cannot be carried out as asked.

Judge the work itself, not how it was described. Say in one to three sentences what you found, \
concretely enough to help whoever takes the work over. Then end with one line that is exactly \
`VERDICT: MODEL`, `VERDICT: OK` or `VERDICT: OTHER`.";

/// What the judge sees of a failed turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failed<'a> {
    /// What the user asked.
    pub task: &'a str,
    /// The turn's file changes as a unified diff; empty when it changed
    /// none.
    pub diff: &'a str,
    /// The turn's final reply.
    pub reply: &'a str,
    /// How it failed: the failing check and the end of its output, or
    /// the error.
    pub failure: &'a str,
}

/// What the judge found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// Whether the model's own work caused the failure.
    pub at_fault: bool,
    /// The judge's account of what went wrong.
    pub reasons: String,
}

/// The last `max` characters of `text`, saying so when it was cut.
fn tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let kept: String = text.chars().skip(count - max).collect();
    format!("(start cut) ...\n{kept}")
}

/// The judge's message about `failed`. Nothing in it names the model
/// that did the work.
#[must_use]
pub fn prompt(failed: &Failed<'_>) -> String {
    let or = |text: &str, empty: &str, shown: String| {
        if text.trim().is_empty() {
            empty.to_string()
        } else {
            shown
        }
    };
    format!(
        "## The user's request\n\n{}\n\n\
         ## Changes\n```diff\n{}\n```\n\n\
         ## Final reply\n{}\n\n\
         ## How it failed\n{}",
        cut(failed.task, MAX_TASK, "request"),
        or(
            failed.diff,
            "(no file changes recorded)",
            cut(failed.diff, MAX_DIFF, "diff")
        ),
        or(
            failed.reply,
            "(no reply)",
            cut(failed.reply, MAX_REPLY, "reply")
        ),
        or(
            failed.failure,
            "(nothing recorded)",
            tail(failed.failure, MAX_FAILURE)
        ),
    )
}

/// The request that asks `model` to judge `failed`, sent with the user's
/// `auth_token` when the route needs one.
///
/// # Errors
/// Returns the builder's error when `model` is empty.
pub fn request(
    model: &str,
    failed: &Failed<'_>,
    auth_token: Option<String>,
) -> Result<ModelRequest, ReasonerError> {
    ModelRequest::builder(model, SYSTEM_PROMPT)
        .message(Message::user(prompt(failed)))
        .tool_choice(ToolChoice::None)
        .max_tokens(MAX_REPLY_TOKENS)
        .thinking_effort(Some(ThinkingEffort::Off))
        .auth_token(auth_token)
        .request_kind(ModelRequestKind::Auxiliary)
        .purpose(ModelCallPurpose::Judge)
        .try_build()
}

/// What the judge sees of a turn that ran no check and looks wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unverified<'a> {
    /// What the user asked.
    pub task: &'a str,
    /// The turn's file changes as a unified diff; empty when it changed
    /// none.
    pub diff: &'a str,
    /// The turn's final reply.
    pub reply: &'a str,
    /// The commands it ran, one per line, latest last.
    pub commands: &'a str,
    /// What looks wrong.
    pub suspicions: &'a [Suspicion],
}

/// The judge's message about `unverified`. Nothing in it names the model
/// that did the work.
#[must_use]
pub fn unverified_prompt(unverified: &Unverified<'_>) -> String {
    let or = |text: &str, empty: &str, shown: String| {
        if text.trim().is_empty() {
            empty.to_string()
        } else {
            shown
        }
    };
    let wrong: Vec<String> = unverified
        .suspicions
        .iter()
        .map(|s| format!("- {}", s.describe()))
        .collect();
    format!(
        "## The user's request\n\n{}\n\n\
         ## Changes\n```diff\n{}\n```\n\n\
         ## Commands it ran\n{}\n\n\
         ## Final reply\n{}\n\n\
         ## What looks wrong\n{}",
        cut(unverified.task, MAX_TASK, "request"),
        or(
            unverified.diff,
            "(no file changes recorded)",
            cut(unverified.diff, MAX_DIFF, "diff")
        ),
        or(
            unverified.commands,
            "(none)",
            tail(unverified.commands, MAX_FAILURE)
        ),
        or(
            unverified.reply,
            "(no reply)",
            cut(unverified.reply, MAX_REPLY, "reply")
        ),
        wrong.join("\n"),
    )
}

/// The request that asks `model` to judge `unverified`, sent with the
/// user's `auth_token` when the route needs one.
///
/// # Errors
/// Returns the builder's error when `model` is empty.
pub fn unverified_request(
    model: &str,
    unverified: &Unverified<'_>,
    auth_token: Option<String>,
) -> Result<ModelRequest, ReasonerError> {
    ModelRequest::builder(model, UNVERIFIED_SYSTEM_PROMPT)
        .message(Message::user(unverified_prompt(unverified)))
        .tool_choice(ToolChoice::None)
        .max_tokens(MAX_REPLY_TOKENS)
        .thinking_effort(Some(ThinkingEffort::Off))
        .auth_token(auth_token)
        .request_kind(ModelRequestKind::Auxiliary)
        .purpose(ModelCallPurpose::Judge)
        .try_build()
}

/// The verdict in the judge's `reply`: its last `VERDICT:` line, with the
/// text before it as the reasons. `MODEL` puts the fault on the model;
/// `OTHER`, and `OK` from the judge of unverified turns, do not. `None`
/// when the reply has no such line or the line names no answer.
#[must_use]
pub fn parse(reply: &str) -> Option<Verdict> {
    let reply = reply
        .rsplit_once("</think>")
        .map_or(reply, |(_, after)| after);
    let (at, line) = reply
        .lines()
        .enumerate()
        .filter(|(_, line)| line.to_ascii_uppercase().contains("VERDICT:"))
        .last()?;
    let upper = line.to_ascii_uppercase();
    let answer = upper.split("VERDICT:").nth(1)?;
    let answer = answer.trim_matches(|c: char| !c.is_ascii_alphabetic());
    let at_fault = match answer {
        "MODEL" => true,
        "OTHER" | "OK" => false,
        _ => return None,
    };
    let reasons = reply
        .lines()
        .take(at)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    Some(Verdict { at_fault, reasons })
}

/// Whether `judge` reviews failed work done on `model`: only work on a
/// weaker tier than its own. Work on its tier has no stronger model to
/// go on to, and a model judging itself says little. Tiers come from
/// `candidates`, else from the models' names.
#[must_use]
pub fn judges(judge: &str, model: &str, candidates: &[Candidate]) -> bool {
    let tier = |id: &str| {
        candidates
            .iter()
            .find(|c| c.id == id)
            .map_or_else(|| infer_tier(id), |c| c.tier)
    };
    !judge.is_empty() && tier(model) < tier(judge)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_model::model_catalog::{ModelBackend, ModelTier};

    fn failed() -> Failed<'static> {
        Failed {
            task: "fix the parser",
            diff: "--- src/parse.rs\n-old\n+new",
            reply: "Fixed it.",
            failure: "Check `cargo test` failed:\ntest parse ... FAILED",
        }
    }

    #[test]
    fn the_verdict_is_the_last_verdict_line() {
        assert_eq!(
            parse("The loop is off by one.\nIt never handles n.\n\nVERDICT: MODEL"),
            Some(Verdict {
                at_fault: true,
                reasons: "The loop is off by one.\nIt never handles n.".to_string(),
            })
        );
        assert_eq!(
            parse("cargo is not installed.\n`verdict: other`\n"),
            Some(Verdict {
                at_fault: false,
                reasons: "cargo is not installed.".to_string(),
            })
        );
        assert_eq!(
            parse("<think>VERDICT: OTHER?</think>Wrong fix.\n**VERDICT: MODEL**")
                .map(|v| v.at_fault),
            Some(true)
        );
        assert_eq!(
            parse("First VERDICT: OTHER, but then\nVERDICT: MODEL").map(|v| v.at_fault),
            Some(true),
            "the last line decides"
        );
    }

    #[test]
    fn work_found_done_is_not_at_fault() {
        assert_eq!(
            parse("The change is complete.\nVERDICT: OK").map(|v| v.at_fault),
            Some(false)
        );
    }

    #[test]
    fn the_unverified_request_shows_the_commands_and_what_looks_wrong() {
        let unverified = Unverified {
            task: "make ensure_type keep tags",
            diff: "",
            reply: "The code already does this.",
            commands: "rg ensure_type\ncat lib/config.py",
            suspicions: &[Suspicion::NoChanges],
        };
        let built = unverified_request(OPUS, &unverified, None).unwrap();
        assert_eq!(built.system, UNVERIFIED_SYSTEM_PROMPT);
        assert!(built.tools.is_empty());
        let sent = built.messages[0].text_content();
        for part in [
            "make ensure_type keep tags",
            "(no file changes recorded)",
            "cat lib/config.py",
            "The code already does this.",
            Suspicion::NoChanges.describe(),
        ] {
            assert!(sent.contains(part), "{part}: {sent}");
        }
        assert!(unverified_request("", &unverified, None).is_err());
    }

    const OPUS: &str = "claude/claude-opus-4-7";

    #[test]
    fn a_reply_without_a_verdict_decides_nothing() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("It looks wrong to me."), None);
        assert_eq!(parse("VERDICT: MAYBE"), None);
        assert_eq!(parse("VERDICT:"), None);
    }

    #[test]
    fn the_request_shows_the_work_and_the_failure_without_tools() {
        let built = request("claude/claude-opus-4-7", &failed(), Some("jwt".into())).unwrap();
        assert_eq!(built.model.as_str(), "claude/claude-opus-4-7");
        assert!(built.tools.is_empty());
        assert!(matches!(built.tool_choice, ToolChoice::None));
        assert_eq!(built.auth_token.as_deref(), Some("jwt"));
        assert!(built.system.contains("VERDICT: MODEL"));
        let sent = built.messages[0].text_content();
        for part in [
            "fix the parser",
            "+new",
            "Fixed it.",
            "test parse ... FAILED",
        ] {
            assert!(sent.contains(part), "{part}: {sent}");
        }
        assert!(request("", &failed(), None).is_err());
    }

    #[test]
    fn long_parts_are_cut_and_empty_ones_say_so() {
        let output = format!("{}the end", "x".repeat(MAX_FAILURE * 2));
        let long = Failed {
            diff: "",
            reply: " ",
            failure: &output,
            ..failed()
        };
        let sent = prompt(&long);
        assert!(sent.contains("(no file changes recorded)"));
        assert!(sent.contains("(no reply)"));
        assert!(sent.contains("(start cut)") && sent.ends_with("the end"));
        assert!(sent.len() < MAX_FAILURE + 500, "{}", sent.len());
    }

    #[test]
    fn only_work_below_the_judges_tier_is_judged() {
        const HAIKU: &str = "claude/claude-haiku-4-5";
        const SONNET: &str = "claude/claude-sonnet-4-6";
        const OPUS: &str = "claude/claude-opus-4-7";
        assert!(judges(OPUS, HAIKU, &[]));
        assert!(judges(OPUS, SONNET, &[]));
        assert!(!judges(OPUS, OPUS, &[]), "nothing is stronger");
        assert!(!judges(SONNET, OPUS, &[]));
        assert!(!judges("", HAIKU, &[]), "no judge");
        let listed = [Candidate {
            id: "house-model".to_string(),
            label: "House".to_string(),
            backend: ModelBackend::Router,
            tier: ModelTier::Fast,
            cost_usd: None,
        }];
        assert!(
            judges(OPUS, "house-model", &listed),
            "a candidate's tier wins over its name"
        );
    }
}
