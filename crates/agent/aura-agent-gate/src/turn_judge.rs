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
//! The same judge gives the verdict on work that ran no check (see
//! [`crate::followup`]), through [`unverified_request`]: there it may
//! also find the work done (`OK`). And it reviews work that is about to
//! stand, through [`review_request`]: hard work, or work that shows a
//! sign of failure the attempt log backs, is gone through against the
//! request before it counts as done.
//!
//! This module builds the request and reads the verdict; the caller
//! sends the request through the agent's kernel, so the call is recorded
//! like any other model call.

use aura_core_types::OutcomeSignal;
use aura_model::model_catalog::infer_tier;
use aura_model::{
    Message, ModelCallPurpose, ModelRequest, ModelRequestKind, ReasonerError, ThinkingEffort,
    ToolChoice,
};
use serde::{Deserialize, Serialize};

use crate::criteria::{self, Criteria};
use crate::followup::Suspicion;
use crate::judge::{cut, MAX_DIFF, MAX_REPLY};
use crate::router::Candidate;

/// Longest request shown to the judge, in characters.
const MAX_TASK: usize = 8_000;

/// Longest account of the failure shown to the judge, in characters; the
/// end of a test run says what failed.
pub const MAX_FAILURE: usize = 3_000;

/// System prompt of every request to the judge of failed turns; tells
/// them apart from the agent's own calls.
pub const SYSTEM_PROMPT: &str = "\
You are reviewing an attempt at a task a user gave a coding agent. The attempt failed: its last \
test or build command failed, or the run ended in an error. You see the user's request, the \
changes the attempt made to files as a unified diff (it may lack changes made through shell \
commands), its final reply, and how it failed. Decide what caused the failure:

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
/// attempts that no check vouches for, because they ran none or because
/// their own reply says the work was not verified (see
/// [`crate::followup`]). Its verdict is the attempt's outcome.
pub const UNVERIFIED_SYSTEM_PROMPT: &str = "\
You are reviewing an attempt at a task a user gave a coding agent. Nothing shows that its work \
is right: it ran no test or build that checks the work, or it ran one and its own final reply \
says the work was not verified. The task may ask for a code change, an answer, research, a plan \
or a piece of writing. You see the user's request, the changes the attempt made to files as a \
unified diff (it may lack changes made through shell commands; the commands it ran are listed), \
the last test or build it ran with what that printed when it ran one, its final reply, and \
anything that looks wrong about it. Decide whether the work is done:

- MODEL: the attempt's own work is wrong, incomplete or missing. It misunderstood the request, \
made a wrong or partial change, changed nothing when a change was needed, answered something \
other than what was asked, got facts wrong, or gave up. A more capable model would likely have \
done better.
- OK: the work looks complete and correct for what was asked, or no change was needed and the \
reply shows why. A reply that only names something beside the point it left untested (another \
platform, an optional style check, tests the request did not ask for) is no reason to doubt \
work that is otherwise right.
- OTHER: the work could not be done for reasons outside the attempt: the environment got in the \
way, or the request cannot be carried out as asked.

Judge the work itself, not how it was described. Say in one to three sentences what you found, \
concretely enough to help whoever takes the work over. Then end with one line that is exactly \
`VERDICT: MODEL`, `VERDICT: OK` or `VERDICT: OTHER`.";

/// Longest diff shown to the reviewer of work about to stand, in
/// characters: it has to see the whole change to say what is missing.
pub const MAX_REVIEW_DIFF: usize = 60_000;

/// System prompt of every review of work about to stand (see
/// [`crate::followup`]).
pub const REVIEW_SYSTEM_PROMPT: &str = "\
You are reviewing an attempt at a task a user gave a coding agent, before the attempt is accepted. \
You see the user's request, what the attempt changed as a unified diff, the commands it ran, the \
last tests or builds it ran with what they printed, its final reply, and why it is being \
reviewed. Decide whether the work does what was asked.

Go through the request one requirement at a time: every behaviour, name, signature, message \
text, default, edge case and file it asks for. For each, find where the diff does it. A \
requirement the diff does not meet, meets only in part, or meets differently from what the \
request says (another name, message or default; a case left out; a file not touched that has to \
change) is a finding. Then read the diff for mistakes of its own: code that cannot work as \
written, a call to something that does not exist, a change that breaks what was there.

Do not take the attempt's word for it. Its reply says what it meant to do. A check that exited \
with success proves only what that check covers, and a test run that does not exercise the \
change says nothing about it. Failures the checks printed count against the work unless the diff \
plainly cannot have caused them.

- MODEL: there is at least one finding: the work is wrong or incomplete in a way that matters to \
what was asked. A more capable model would likely have done better.
- OK: every requirement is met as far as the diff shows and nothing in it looks broken. Missing \
tests, style, and things the request did not ask for are no reason to answer MODEL.
- OTHER: the work could not be done for reasons outside the attempt: the environment got in the \
way, or the request cannot be carried out as asked.

A finding has to name what the request asks for and what the diff does instead; when you have \
none, answer OK. List the findings, at most the five that matter most, as numbered points of a \
few lines each, concrete enough to act on without rereading the request: the file, the \
function, what is asked, what is there. Do not write up the requirements that are met or how \
you checked them. Then end with one line that is exactly `VERDICT: MODEL`, `VERDICT: OK` or \
`VERDICT: OTHER`.";

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
        .thinking_effort(Some(ThinkingEffort::Off))
        .auth_token(auth_token)
        .request_kind(ModelRequestKind::Auxiliary)
        .purpose(ModelCallPurpose::Judge)
        .try_build()
}

/// What the judge sees of a turn no check vouches for.
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
    /// The last test or build it ran and what that printed; empty when
    /// it ran none.
    pub last_check: &'a str,
    /// What looks wrong; empty when nothing does.
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
    let wrong = if wrong.is_empty() {
        "(nothing in particular)".to_string()
    } else {
        wrong.join("\n")
    };
    format!(
        "## The user's request\n\n{}\n\n\
         ## Changes\n```diff\n{}\n```\n\n\
         ## Commands it ran\n{}\n\n\
         ## Its last test or build\n{}\n\n\
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
            unverified.last_check,
            "(it ran none)",
            tail(unverified.last_check, MAX_FAILURE)
        ),
        or(
            unverified.reply,
            "(no reply)",
            cut(unverified.reply, MAX_REPLY, "reply")
        ),
        wrong,
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
        .thinking_effort(Some(ThinkingEffort::Off))
        .auth_token(auth_token)
        .request_kind(ModelRequestKind::Auxiliary)
        .purpose(ModelCallPurpose::Judge)
        .try_build()
}

/// What the reviewer of work about to stand sees of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Standing<'a> {
    /// What the user asked.
    pub task: &'a str,
    /// What the attempt changed, as a unified diff; empty when it
    /// changed nothing.
    pub diff: &'a str,
    /// Whether `diff` holds every change: those made by commands it ran
    /// as well as those made with the file tools.
    pub whole: bool,
    /// The commands it ran, one per line, latest last.
    pub commands: &'a str,
    /// The last tests or builds it ran and what they printed; empty when
    /// it ran none.
    pub checks: &'a str,
    /// Its final reply.
    pub reply: &'a str,
    /// Why it is reviewed, one reason per entry.
    pub why: &'a [String],
    /// What it is checked against, written down before it started; the
    /// reviewer says how the work stands against each.
    pub criteria: Option<&'a Criteria>,
}

/// How the reviewer of work about to stand is asked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ReviewVariant {
    /// With thinking off: one pass over the request and the diff.
    #[default]
    Plain,
    /// With room to reason before it answers.
    Thinking,
}

impl ReviewVariant {
    /// The variant's name as logged, e.g. `plain`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Thinking => "thinking",
        }
    }
}

/// The reviewer's message about `standing`. Nothing in it names the
/// model that did the work.
#[must_use]
pub fn review_prompt(standing: &Standing<'_>) -> String {
    let or = |text: &str, empty: &str, shown: String| {
        if text.trim().is_empty() {
            empty.to_string()
        } else {
            shown
        }
    };
    let changes = if standing.whole {
        "## Changes (everything the attempt changed)"
    } else {
        "## Changes (made with the file tools; changes made by the commands it ran are not shown)"
    };
    let why: Vec<String> = standing.why.iter().map(|why| format!("- {why}")).collect();
    let criteria = standing.criteria.map_or_else(String::new, |c| {
        format!("{}\n\n", criteria::review_section(c))
    });
    format!(
        "## The user's request\n\n{}\n\n\
         {criteria}\
         {changes}\n```diff\n{}\n```\n\n\
         ## Commands it ran\n{}\n\n\
         ## Its last tests or builds\n{}\n\n\
         ## Final reply\n{}\n\n\
         ## Why it is reviewed\n{}",
        cut(standing.task, MAX_TASK, "request"),
        or(
            standing.diff,
            "(no file changes recorded)",
            cut(standing.diff, MAX_REVIEW_DIFF, "diff")
        ),
        or(
            standing.commands,
            "(none)",
            tail(standing.commands, MAX_FAILURE)
        ),
        or(
            standing.checks,
            "(it ran none)",
            tail(standing.checks, 2 * MAX_FAILURE)
        ),
        or(
            standing.reply,
            "(no reply)",
            cut(standing.reply, MAX_REPLY, "reply")
        ),
        why.join("\n"),
    )
}

/// The request that asks `model` to review `standing`, asked as
/// `variant` says and sent with the user's `auth_token` when the route
/// needs one.
///
/// # Errors
/// Returns the builder's error when `model` is empty.
pub fn review_request(
    model: &str,
    standing: &Standing<'_>,
    variant: ReviewVariant,
    auth_token: Option<String>,
) -> Result<ModelRequest, ReasonerError> {
    let effort = match variant {
        ReviewVariant::Plain => ThinkingEffort::Off,
        ReviewVariant::Thinking => ThinkingEffort::High,
    };
    ModelRequest::builder(model, REVIEW_SYSTEM_PROMPT)
        .message(Message::user(review_prompt(standing)))
        .tool_choice(ToolChoice::None)
        .thinking_effort(Some(effort))
        .auth_token(auth_token)
        .request_kind(ModelRequestKind::Auxiliary)
        .purpose(ModelCallPurpose::Judge)
        .try_build()
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

/// The answer in the judge's `reply`: its last `VERDICT:` line, with the
/// text before it as the reasons. `None` when the reply has no such line
/// or the line names no answer.
#[must_use]
pub fn parse_answer(reply: &str) -> Option<(Answer, String)> {
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
    let answer = match answer.trim_matches(|c: char| !c.is_ascii_alphabetic()) {
        "MODEL" => Answer::Model,
        "OK" => Answer::Ok,
        "OTHER" => Answer::Other,
        _ => return None,
    };
    let reasons = reply
        .lines()
        .take(at)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    Some((answer, reasons))
}

/// The verdict in the judge's `reply` ([`parse_answer`]): `MODEL` puts
/// the fault on the model; `OTHER`, and `OK` from the judge of unverified
/// turns, do not.
#[must_use]
pub fn parse(reply: &str) -> Option<Verdict> {
    let (answer, reasons) = parse_answer(reply)?;
    Some(Verdict {
        at_fault: answer == Answer::Model,
        reasons,
    })
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
            last_check: "",
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
            "## Its last test or build\n(it ran none)",
            Suspicion::NoChanges.describe(),
        ] {
            assert!(sent.contains(part), "{part}: {sent}");
        }
        assert!(unverified_request("", &unverified, None).is_err());
    }

    #[test]
    fn the_unverified_request_shows_a_check_the_reply_disowns() {
        let unverified = Unverified {
            task: "make ensure_type keep tags",
            diff: "+    return value",
            reply: "I rewrote it, but it isn't verified.",
            commands: "pytest -q | head -20",
            last_check:
                "`pytest -q | head -20` exited with success. It printed:\nTypeError: boom\n9 passed",
            suspicions: &[Suspicion::Unverified],
        };
        let sent = unverified_prompt(&unverified);
        for part in [
            "## Its last test or build\n`pytest -q | head -20` exited with success.",
            "TypeError: boom",
            "it isn't verified",
            Suspicion::Unverified.describe(),
        ] {
            assert!(sent.contains(part), "{part}: {sent}");
        }
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
    fn the_review_shows_the_criteria_it_is_given() {
        let why = ["the task was rated hard".to_string()];
        let mut standing = Standing {
            task: "add prepare_multipart",
            diff: "--- lib/urls.py\n+def prepare_multipart(fields):",
            whole: true,
            commands: "pytest test/urls",
            checks: "3 passed",
            reply: "Done.",
            why: &why,
            criteria: None,
        };
        let plain = review_prompt(&standing);
        assert!(!plain.contains("Acceptance criteria"), "{plain}");
        let criteria = Criteria {
            items: vec![criteria::Criterion {
                text: "the body is byte-identical to MIMEMultipart's".to_string(),
                kind: criteria::Kind::ReferenceMatch,
                reference: Some("email.mime.multipart".to_string()),
            }],
            strict: true,
        };
        standing.criteria = Some(&criteria);
        let held = review_prompt(&standing);
        let request = held.find("## The user's request").unwrap();
        let listed = held.find(&criteria.listed()).unwrap();
        let changes = held.find("## Changes").unwrap();
        assert!(request < listed && listed < changes, "{held}");
        assert!(held.contains("CRITERION <number>: MET"), "{held}");
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
