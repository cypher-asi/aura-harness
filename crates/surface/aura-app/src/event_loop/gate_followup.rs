//! What a turn Gate routed does when it ends with no check of its own
//! (see [`aura_agent_gate::followup`]), shared by the terminal's turns,
//! `/compare` runs and `aura -p`.
//!
//! Each of them asks [`FollowUp::next`] once an attempt ends and carries
//! the step out: a nudge goes on in the same conversation on the same
//! model, told [`nudge_prompt`]; the harness check runs here
//! ([`harness_check`]); and so does the judge of unchecked work
//! ([`judge_unchecked`]). What either finds wrong sends the work on to a
//! stronger model as a failed check of the turn's own does.

use super::agents::AgentSession;
use super::gate_live::GateLive;
use super::gate_record::{self, Judged};
use super::steering::SteerSource;
use super::task_check::{run_script_within, CheckRun};
use aura_agent::AgentLoopResult;
use aura_agent_gate::checks::{command_line, exit_status_masked, last_check_seen, CheckSeen};
use aura_agent_gate::followup::{self, Allowed, Ended, Step, Suspicion, Taken, NUDGE_PROMPT};
use aura_agent_gate::turn_judge::Unverified;
use aura_agent_gate::Candidate;
use aura_core_types::{OutcomePayload, OutcomeSignal, PromptCategory, RouteLevel};
use aura_model::{ContentBlock, Message, Role};
use std::path::Path;

/// `ToolProposal::source` of the harness's checks.
const CHECK_SOURCE: &str = "gate_check";

/// Longest the harness's check may run. One that takes longer says
/// nothing, so a slow suite costs the user at most this long.
const CHECK_TIMEOUT_SECS: u64 = 5 * 60;

/// Exit codes of a command the shell could not run (126: not
/// executable, 127: not found).
const NOT_RUN_EXIT_CODES: [i32; 2] = [126, 127];

/// Exit codes of pytest that say nothing about the change: 4 a usage
/// error, 5 no tests collected.
const PYTEST_NOT_RUN_EXIT_CODES: [i32; 2] = [4, 5];

/// What a check printed when its tools are missing, not its tests
/// failing.
const NOT_RUN_OUTPUT: &[&str] = &[
    "command not found",
    "No module named pytest",
    "Missing script",
    "is not recognized as",
];

/// Commands shown to the judge of unchecked work, latest last.
const SHOWN_COMMANDS: usize = 15;

/// The steps one attempt at a routed turn may still take.
#[derive(Debug, Clone, Default)]
pub(crate) struct FollowUp {
    allowed: Allowed,
    taken: Taken,
    /// Test command the harness runs
    check: Option<String>,
    /// Model that judges unchecked work
    judge: Option<String>,
    /// Files changed before the nudge, whose run ended there
    changed_before: usize,
}

impl FollowUp {
    /// The steps for an attempt on `model`, routed among `candidates`,
    /// working in `workspace`: none when no candidate is stronger, as
    /// there is nowhere for the work to go, else those Gate's settings
    /// allow.
    pub(crate) fn new(
        live: &GateLive,
        candidates: &[Candidate],
        model: &str,
        workspace: &Path,
    ) -> Self {
        let stronger =
            aura_agent_gate::escalate(&live.stats(), RouteLevel::Turn, model, candidates).is_some();
        if !stronger {
            return Self::default();
        }
        let settings = live.settings();
        let check = settings
            .harness_check
            .then(|| check_command(workspace))
            .flatten();
        let judge = gate_record::unverified_judge_for(live, candidates, model);
        Self {
            allowed: Allowed {
                nudge: settings.nudge,
                harness_check: check.is_some(),
                judge: judge.is_some(),
            },
            taken: Taken::default(),
            check,
            judge,
            changed_before: 0,
        }
    }

    /// The next step for an attempt that ended with `result` and
    /// `signal`, asking for `category`; the step counts as taken.
    pub(crate) fn next(
        &mut self,
        result: &AgentLoopResult,
        category: PromptCategory,
        signal: OutcomeSignal,
    ) -> Step {
        let reply = final_reply(result);
        let files_changed = self.changed_before + result.file_changes.len();
        let ended = Ended {
            signal,
            category,
            files_changed,
            reply: &reply,
            hit_limit: result.budget_exhausted || result.stalled || result.output_truncated,
        };
        let step = followup::next(&ended, self.taken, self.allowed);
        match step {
            Step::Nudge => {
                self.taken.nudged = true;
                self.changed_before = files_changed;
            }
            Step::HarnessCheck => self.taken.checked = true,
            Step::JudgeUnverified(_) => self.taken.judged = true,
            Step::Stop => {}
        }
        step
    }

    /// The test command the harness runs, when it may run one.
    pub(crate) fn check(&self) -> Option<&str> {
        self.check.as_deref()
    }

    /// The model that judges unchecked work, when one may.
    pub(crate) fn judge(&self) -> Option<&str> {
        self.judge.as_deref()
    }
}

/// The text of the last assistant message of `result`: its final reply,
/// without what the model narrated along the way.
fn final_reply(result: &AgentLoopResult) -> String {
    result
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::Assistant && !m.text_content().trim().is_empty())
        .map(Message::text_content)
        .unwrap_or_default()
}

/// The message that asks the model to check its change, marked as the
/// harness's.
pub(crate) fn nudge_prompt() -> String {
    format!("{}{NUDGE_PROMPT}", SteerSource::Harness.prefix())
}

/// The test command the harness runs in `workspace`: the operator's
/// (`AURA_DOD_TEST_COMMAND`), else the one its manifest implies.
fn check_command(workspace: &Path) -> Option<String> {
    aura_config::agent()
        .verify
        .test_command_override
        .map(|command| command.trim().to_string())
        .filter(|command| !command.is_empty())
        .or_else(|| aura_agent::infer_default_test_command(workspace))
}

/// Run `command` in `session`'s workspace as the harness's check.
pub(crate) async fn harness_check(session: &AgentSession, command: &str) -> CheckRun {
    run_script_within(session, command, CHECK_SOURCE, CHECK_TIMEOUT_SECS).await
}

/// What `check` says of the change: passed or failed, `None` when it
/// could not run, ran out of time, or failed for want of its tools.
pub(crate) fn check_signal(check: &CheckRun) -> Option<OutcomeSignal> {
    if could_not_run(check) {
        return None;
    }
    check.signal()
}

fn could_not_run(check: &CheckRun) -> bool {
    match (check.passed, check.exit_code) {
        (None, _) | (Some(false), None) => true,
        (Some(true), _) => false,
        (Some(false), Some(code)) => {
            NOT_RUN_EXIT_CODES.contains(&code)
                || (check.command.contains("pytest") && PYTEST_NOT_RUN_EXIT_CODES.contains(&code))
                || NOT_RUN_OUTPUT.iter().any(|text| check.tail.contains(text))
        }
    }
}

/// The commands the `run_command` calls in `messages` ran, one per line,
/// the last [`SHOWN_COMMANDS`] of them.
fn commands_run(messages: &[Message]) -> String {
    let commands: Vec<String> = messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|block| match block {
            ContentBlock::ToolUse { name, input, .. } if name == "run_command" => {
                Some(command_line(input))
            }
            _ => None,
        })
        .collect();
    commands[commands.len().saturating_sub(SHOWN_COMMANDS)..].join("\n")
}

/// The attempt's last check as the judge of unchecked work sees it: its
/// command, how it exited, and what it printed. A command whose exit
/// status is not the check's is pointed out, since its exiting with
/// success then says nothing of the check.
fn check_shown(check: &CheckSeen) -> String {
    let exited = if check.passed {
        "exited with success"
    } else {
        "failed"
    };
    let masked = if check.passed && exit_status_masked(&check.command) {
        " The exit status is that of the command the check's output was piped or chained into, \
         not the check's own, so only the output says how the check went."
    } else {
        ""
    };
    format!(
        "`{}` {exited}.{masked} It printed:\n{}",
        check.command, check.output
    )
}

/// What the judge of unchecked work is shown of an attempt.
pub(crate) struct Unchecked<'a> {
    /// What the user asked
    pub task: &'a str,
    /// The attempt's file changes as a unified diff
    pub diff: &'a str,
    /// How it ended
    pub result: &'a AgentLoopResult,
    /// Where its own messages start in `result`
    pub turn_start: usize,
    pub suspicions: &'a [Suspicion],
}

/// Ask `judge` through `session`'s kernel whether the work of `ended`,
/// shown as `unchecked`, is done (see [`gate_record::judge_unverified`]).
pub(crate) async fn judge_unchecked(
    session: &AgentSession,
    judge: &str,
    ended: &OutcomePayload,
    unchecked: &Unchecked<'_>,
    auth_token: Option<String>,
) -> Option<Judged> {
    let reply = final_reply(unchecked.result);
    let messages = unchecked
        .result
        .messages
        .get(unchecked.turn_start..)
        .unwrap_or_default();
    let commands = commands_run(messages);
    let last_check = last_check_seen(messages)
        .map(|check| check_shown(&check))
        .unwrap_or_default();
    let shown = Unverified {
        task: unchecked.task,
        diff: unchecked.diff,
        reply: &reply,
        commands: &commands,
        last_check: &last_check,
        suspicions: unchecked.suspicions,
    };
    gate_record::judge_unverified(&session.kernel, judge, ended, &shown, auth_token).await
}

/// What the stronger model is told of a harness check that failed.
pub(crate) fn check_review(check: &CheckRun) -> String {
    format!("The harness ran the project's tests. {}", check.describe())
}

/// `first`, then `second`, on lines of their own; whichever is present.
pub(crate) fn joined(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(first), Some(second)) => Some(format!("{first}\n{second}")),
        (first, second) => first.or(second),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn check(passed: Option<bool>, exit_code: Option<i32>, command: &str, tail: &str) -> CheckRun {
        CheckRun {
            command: command.to_string(),
            passed,
            exit_code,
            tail: tail.to_string(),
        }
    }

    #[test]
    fn a_check_that_ran_says_how_the_change_went() {
        let passed = check(Some(true), Some(0), "cargo test", "ok");
        assert_eq!(check_signal(&passed), Some(OutcomeSignal::ChecksPassed));
        let failed = check(
            Some(false),
            Some(101),
            "cargo test",
            "test parse ... FAILED",
        );
        assert_eq!(check_signal(&failed), Some(OutcomeSignal::ChecksFailed));
    }

    #[test]
    fn a_check_that_could_not_run_says_nothing() {
        for unrun in [
            check(None, None, "cargo test", "denied"),
            check(Some(false), None, "cargo test", "Command timed out"),
            check(Some(false), Some(127), "go test ./...", "sh: go: not found"),
            check(
                Some(false),
                Some(1),
                "python -m pytest -q",
                "No module named pytest",
            ),
            check(Some(false), Some(5), "python -m pytest -q", "no tests ran"),
            check(
                Some(false),
                Some(1),
                "npm test --silent",
                "npm error Missing script: \"test\"",
            ),
        ] {
            assert_eq!(check_signal(&unrun), None, "{unrun:?}");
        }
    }

    #[test]
    fn the_judge_is_told_when_a_checks_exit_status_is_not_its_own() {
        let piped = CheckSeen {
            command: "pytest -q 2>&1 | head -20".to_string(),
            passed: true,
            output: "TypeError: boom\n9 passed".to_string(),
        };
        let shown = check_shown(&piped);
        assert!(
            shown.starts_with(
                "`pytest -q 2>&1 | head -20` exited with success. The exit status is that of"
            ),
            "{shown}"
        );
        assert!(
            shown.ends_with("It printed:\nTypeError: boom\n9 passed"),
            "{shown}"
        );

        let plain = CheckSeen {
            command: "pytest -q".to_string(),
            ..piped
        };
        assert_eq!(
            check_shown(&plain),
            "`pytest -q` exited with success. It printed:\nTypeError: boom\n9 passed"
        );
    }

    #[test]
    fn the_judge_sees_the_last_commands_run() {
        let call = |n: usize| {
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: n.to_string(),
                    name: "run_command".to_string(),
                    input: json!({"command": format!("cmd {n}")}),
                }],
            )
        };
        let messages: Vec<Message> = (0..20).map(call).collect();
        let shown = commands_run(&messages);
        assert_eq!(shown.lines().count(), SHOWN_COMMANDS);
        assert!(
            shown.starts_with("cmd 5\n") && shown.ends_with("cmd 19"),
            "{shown}"
        );
        assert_eq!(commands_run(&[]), "");
    }

    #[test]
    fn the_final_reply_is_the_last_assistant_text() {
        let result = AgentLoopResult {
            messages: vec![
                Message::new(
                    Role::Assistant,
                    vec![ContentBlock::Text {
                        text: "I couldn't find it at first.".to_string(),
                    }],
                ),
                Message::user("ok"),
                Message::new(
                    Role::Assistant,
                    vec![ContentBlock::Text {
                        text: "Done: fixed the parser.".to_string(),
                    }],
                ),
            ],
            ..AgentLoopResult::default()
        };
        assert_eq!(final_reply(&result), "Done: fixed the parser.");
    }

    #[test]
    fn reviews_join_on_lines() {
        let some = |s: &str| Some(s.to_string());
        assert_eq!(joined(some("a"), some("b")), some("a\nb"));
        assert_eq!(joined(None, some("b")), some("b"));
        assert_eq!(joined(None, None), None);
    }
}
