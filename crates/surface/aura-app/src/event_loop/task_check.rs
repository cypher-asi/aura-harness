//! A `task` call's `check`, and what its caller gets back.
//!
//! The check runs after the subagent finishes, as a `run_command` call
//! through the subagent's kernel (so it is policy-checked and recorded
//! in the child's log like any command), in the child's workspace. Its
//! exit status is the subtask's verdict. The caller then gets a compact
//! result instead of the transcript: the subagent's summary (capped),
//! the files it changed, the check and how it went, and whether the
//! subtask was retried on a stronger model.
//!
//! `/compare` runs its check on a finished run the same way, through the
//! run's kernel, and so does `aura eval run` under `gate` with a task's
//! verify command.

use super::agents::AgentSession;
use super::run_command::{process_user_proposal, terminal_output, RUN_COMMAND_TOOL};
use super::turns::drain_stale_inbox;
use aura_core_types::{OutcomeSignal, ToolProposal};
use aura_ui_terminal::ansi::strip_ansi;
use std::fmt::Write;

/// `ToolProposal::source` of check commands.
const CHECK_SOURCE: &str = "task_check";

/// Longest a check may run.
const CHECK_TIMEOUT_SECS: u64 = 20 * 60;

/// Characters of the subagent's summary the caller gets.
const SUMMARY_CHARS: usize = 4_000;

/// Lines of a failed check's output the caller gets.
const TAIL_LINES: usize = 20;

/// How a check went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRun {
    pub command: String,
    /// Whether it passed; `None` when it could not run (denied, or no
    /// exit status)
    pub passed: Option<bool>,
    pub exit_code: Option<i32>,
    /// The end of its output, or why it could not run
    pub tail: String,
}

impl CheckRun {
    /// The subtask's verdict, when the check gave one.
    pub(crate) fn signal(&self) -> Option<OutcomeSignal> {
        self.passed.map(|passed| {
            if passed {
                OutcomeSignal::ChecksPassed
            } else {
                OutcomeSignal::ChecksFailed
            }
        })
    }

    /// One line saying how it went, then the output's tail when it
    /// failed or could not run.
    pub(crate) fn describe(&self) -> String {
        let mut out = format!("Check `{}`: ", self.command);
        match (self.passed, self.exit_code) {
            (Some(true), _) => out.push_str("passed"),
            (Some(false), Some(code)) => {
                let _ = write!(out, "failed with exit code {code}");
            }
            (Some(false), None) => out.push_str("failed"),
            (None, _) => out.push_str("could not run"),
        }
        if self.passed != Some(true) && !self.tail.trim().is_empty() {
            out.push_str(". Last lines:\n");
            out.push_str(&self.tail);
        }
        out
    }
}

/// The last [`TAIL_LINES`] lines of `text`, without terminal styling.
fn tail(text: &str) -> String {
    let text = strip_ansi(text);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n")
}

/// Whether a command's error text says it ran out of time; the tool
/// reports a timeout only in its error text.
fn timed_out(text: &str) -> bool {
    text.contains("Command timed out")
}

/// Run `command` in `session`'s workspace through its kernel, with the
/// workspace's tasks set aside: they quote the prompts, so a check that
/// searches for what the task should have removed would find them.
pub(super) async fn run_check(session: &AgentSession, command: &str) -> CheckRun {
    let _aside = session.tasks.set_aside(&session.kernel.agent_workspace());
    run_script(session, command, CHECK_SOURCE).await
}

/// Run the check `command` in `session`'s workspace through its kernel,
/// proposed with `source`. A check that runs out of time failed.
pub(crate) async fn run_script(session: &AgentSession, command: &str, source: &str) -> CheckRun {
    run_script_within(session, command, source, CHECK_TIMEOUT_SECS).await
}

/// [`run_script`] stopped after `timeout_secs`. A check that ran out of
/// time failed with no exit code.
pub(crate) async fn run_script_within(
    session: &AgentSession,
    command: &str,
    source: &str,
    timeout_secs: u64,
) -> CheckRun {
    drain_stale_inbox(session).await;
    let proposal = ToolProposal::new(
        format!("check_{}", uuid::Uuid::new_v4().simple()),
        RUN_COMMAND_TOOL,
        serde_json::json!({
            "shell_script": command,
            "allow_shell": true,
            "timeout_secs": timeout_secs,
        }),
    )
    .with_source(source);
    let unrun = |why: String| CheckRun {
        command: command.to_string(),
        passed: None,
        exit_code: None,
        tail: why,
    };
    let output = match process_user_proposal(&session.kernel, &proposal).await {
        Ok((_, result)) => match result.tool_output {
            Some(output) => output,
            None => return unrun("the kernel returned no output".to_string()),
        },
        Err(e) => return unrun(format!("kernel error: {e}")),
    };
    let (text, exit_code) = terminal_output(&output.content, output.is_error);
    let passed = if !output.is_error {
        Some(true)
    } else if exit_code.is_some() || timed_out(&text) {
        Some(false)
    } else {
        None
    };
    CheckRun {
        command: command.to_string(),
        passed,
        exit_code: if passed == Some(true) {
            Some(0)
        } else {
            exit_code
        },
        tail: tail(&text),
    }
}

/// One attempt at a subtask, as its caller hears about it.
pub(super) struct Attempted<'a> {
    pub model: &'a str,
    pub reply: &'a Result<String, String>,
    pub files: &'a [String],
    pub check: Option<&'a CheckRun>,
}

impl Attempted<'_> {
    /// Why the attempt failed, for the retry and the caller; `None`
    /// when it did not.
    pub(super) fn failure(&self) -> Option<String> {
        if let Err(reason) = self.reply {
            return Some(format!("it stopped with an error: {reason}"));
        }
        let check = self.check.filter(|check| check.passed == Some(false))?;
        Some(format!("its check failed. {}", check.describe()))
    }
}

/// `text` cut to `max` characters, marked when cut.
fn capped(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}… [summary cut at {max} characters]")
}

/// What the caller gets back from `last`, its final attempt; `retried`
/// names the first attempt's model and why it failed when this one is
/// its retry. Plain replies with no check, no files and no retry come
/// back as they are.
pub(super) fn compact(last: &Attempted<'_>, retried: Option<(&str, &str)>) -> String {
    let mut parts = Vec::new();
    if let Ok(reply) = last.reply {
        if !reply.trim().is_empty() {
            parts.push(capped(reply, SUMMARY_CHARS));
        }
    }
    if !last.files.is_empty() {
        parts.push(format!("Files changed: {}", last.files.join(", ")));
    }
    if let Some(check) = last.check {
        parts.push(check.describe());
    }
    if let Some((model, why)) = retried {
        parts.push(format!(
            "Retried once: the first attempt on {model} failed ({why}); its file changes were \
             undone and the task ran again on {}.",
            last.model
        ));
    }
    parts.join("\n\n")
}

/// The prompt of the retry of `prompt`, whose first attempt on `model`
/// failed for `why`.
pub(super) fn retry_prompt(prompt: &str, model: &str, why: &str) -> String {
    format!(
        "{prompt}\n\n---\nA first attempt at this task on {model} failed: {why}\n\
         Its file changes were undone, so start again from the files as they are now."
    )
}

/// What the stronger model is told when it takes over a turn whose work
/// on `model` ended with `signal`, with what the judge found wrong when
/// it reviewed the work. The conversation and the files stay as that
/// work left them.
pub(crate) fn takeover_prompt(model: &str, signal: OutcomeSignal, review: Option<&str>) -> String {
    let how = match signal {
        OutcomeSignal::ChecksFailed => "with its last test or build command failing",
        OutcomeSignal::JudgedFail => "without checking its work, which looked unfinished",
        _ => "in an error",
    };
    let review = review
        .map(str::trim)
        .filter(|review| !review.is_empty())
        .map(|review| format!(" A reviewer of that work found: {review}"))
        .unwrap_or_default();
    format!(
        "The work above was done on {model} and ended {how}.{review} You are a stronger model \
         taking over this turn. The files are as that work left them: check what it changed, \
         find what is wrong and finish the request."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(passed: Option<bool>, exit_code: Option<i32>, tail: &str) -> CheckRun {
        CheckRun {
            command: "cargo test -p parser".to_string(),
            passed,
            exit_code,
            tail: tail.to_string(),
        }
    }

    #[test]
    fn a_check_says_how_it_went() {
        let passed = check(Some(true), Some(0), "ok");
        assert_eq!(passed.describe(), "Check `cargo test -p parser`: passed");
        assert_eq!(passed.signal(), Some(OutcomeSignal::ChecksPassed));
        let failed = check(Some(false), Some(101), "test parse ... FAILED");
        assert_eq!(
            failed.describe(),
            "Check `cargo test -p parser`: failed with exit code 101. Last lines:\n\
             test parse ... FAILED"
        );
        assert_eq!(failed.signal(), Some(OutcomeSignal::ChecksFailed));
        let unrun = check(None, None, "denied");
        assert!(unrun.describe().contains("could not run"));
        assert_eq!(unrun.signal(), None);
    }

    #[test]
    fn a_timeout_is_told_apart_from_a_denial() {
        assert!(timed_out(
            "aura: Command failed: Command timed out: Process timed out\n"
        ));
        assert!(!timed_out("aura: Policy denied run_command\n"));
    }

    #[test]
    fn the_compact_result_has_the_summary_files_check_and_retry() {
        let reply = Ok("Fixed the parser.".to_string());
        let files = ["src/parser.rs".to_string()];
        let passed = check(Some(true), Some(0), "");
        let last = Attempted {
            model: "claude/claude-opus-4-7",
            reply: &reply,
            files: &files,
            check: Some(&passed),
        };
        assert_eq!(last.failure(), None);
        let text = compact(
            &last,
            Some(("claude/claude-sonnet-4-6", "its check failed")),
        );
        assert_eq!(
            text,
            "Fixed the parser.\n\nFiles changed: src/parser.rs\n\n\
             Check `cargo test -p parser`: passed\n\n\
             Retried once: the first attempt on claude/claude-sonnet-4-6 failed (its check \
             failed); its file changes were undone and the task ran again on \
             claude/claude-opus-4-7."
        );

        let plain = Attempted {
            model: "m",
            reply: &reply,
            files: &[],
            check: None,
        };
        assert_eq!(compact(&plain, None), "Fixed the parser.");
    }

    #[test]
    fn long_summaries_are_capped_and_failures_explained() {
        let reply = Ok("x".repeat(SUMMARY_CHARS + 10));
        let failed = check(Some(false), Some(1), "boom");
        let attempt = Attempted {
            model: "m",
            reply: &reply,
            files: &[],
            check: Some(&failed),
        };
        let text = compact(&attempt, None);
        assert!(text.contains("[summary cut at 4000 characters]"));
        let why = attempt.failure().unwrap();
        assert!(why.starts_with("its check failed. Check `cargo test -p parser`: failed"));
        assert!(why.ends_with("boom"));

        let errored: Result<String, String> = Err("the model fell over".to_string());
        let attempt = Attempted {
            model: "m",
            reply: &errored,
            files: &[],
            check: None,
        };
        assert_eq!(
            attempt.failure().as_deref(),
            Some("it stopped with an error: the model fell over")
        );
        assert!(retry_prompt("Fix it", "m", "boom").starts_with("Fix it\n\n---\nA first attempt"));
    }
}
