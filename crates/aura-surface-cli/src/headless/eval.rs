//! `aura eval run --model gate`: each run of a task is an agent of its
//! own under the Headless agent, working in the task's workspace, and its
//! prompts are `aura -p` turns. Gate classifies and routes them with the
//! table as configured (exploration included), the agent leads and its
//! `task` subtasks are routed and escalated, and every prompt ends with
//! its `Outcome`. The task's verify command then runs as a `run_command`
//! call through the agent's kernel, and its verdict is recorded as a
//! later `Outcome` of the last prompt, so eval results count in the Gate
//! table of the data directory.

use super::output::Output;
use super::run::{Headless, RunLimits};
use crate::cli::OutputFormat;
use crate::evals::GateUse;
use crate::event_loop::gate_record;
use crate::event_loop::subagents::lock;
use crate::event_loop::task_check::{run_script, CheckRun};
use aura_agent_trace::RunRecorder;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::warn;

/// `ToolProposal::source` of a task's verify command.
const VERIFY_SOURCE: &str = "eval_verify";

/// One run of an eval task.
pub(crate) struct EvalTurn<'a> {
    /// Name of the run's agent
    pub name: &'a str,
    /// Where it works
    pub workspace: &'a Path,
    /// One user prompt per turn
    pub prompts: &'a [String],
    /// Iteration cap of each turn
    pub max_iterations: Option<usize>,
    /// Hidden checks copied into the workspace before `verify`
    pub checks: Option<&'a Path>,
    pub verify: &'a str,
    /// The eval run's trace
    pub trace: Arc<RunRecorder>,
}

/// How a run went.
pub(crate) struct EvalRan {
    /// Why the turns stopped early, if they did
    pub error: Option<String>,
    /// The verify command's run
    pub check: CheckRun,
    /// How long it took, in milliseconds
    pub check_ms: u64,
    pub gate: GateUse,
}

/// Runs eval tasks on a headless harness.
pub(crate) struct EvalTurns {
    headless: Headless,
    output: Output,
}

impl EvalTurns {
    pub(crate) fn new(headless: Headless) -> Self {
        Self {
            headless,
            output: Output::new(OutputFormat::Text, Box::new(std::io::sink())),
        }
    }

    /// Run `turn`'s prompts, then its verify command.
    ///
    /// # Errors
    /// When the run's agent can't be registered or opened.
    pub(crate) async fn run(&mut self, turn: EvalTurn<'_>) -> anyhow::Result<EvalRan> {
        let hub = Arc::clone(&self.headless.hub);
        let agent = {
            let mut registry = lock(&hub.registry);
            let parent = registry.headless()?;
            registry.create_child(
                turn.name,
                &parent.zns_id,
                Some(turn.workspace.to_path_buf()),
            )?
        };
        let mut session =
            hub.factory
                .open(&agent, Vec::new(), Some(turn.workspace.to_path_buf()))?;
        let agent_id = session.agent_id();

        let limits = RunLimits {
            max_turns: None,
            max_time: hub.long_run.turn_timeout,
            max_cost_usd: hub.long_run.max_turn_cost_usd,
            max_iterations: turn.max_iterations,
        };
        let interrupt = CancellationToken::new();
        let started = Instant::now();
        self.headless.trace = Some(Arc::clone(&turn.trace));
        let mut error = None;
        let mut last = None;
        for prompt in turn.prompts {
            let (report, ended) = self
                .headless
                .drive(
                    &mut session,
                    prompt.clone(),
                    limits,
                    started,
                    &self.output,
                    &interrupt,
                )
                .await;
            if ended.is_some() {
                last = ended;
            }
            if report.is_error {
                error = Some(
                    report
                        .error
                        .unwrap_or_else(|| format!("stopped ({:?})", report.exit_reason)),
                );
                break;
            }
        }
        self.headless.trace = None;

        if let Some(checks) = turn.checks {
            crate::evals::copy_dir(checks, turn.workspace)?;
        }
        let check_started = Instant::now();
        let check = run_script(&session, &verify_script(turn.verify), VERIFY_SOURCE).await;
        let check_ms = u64::try_from(check_started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match (check.signal(), &last) {
            (Some(signal), Some(ended)) => {
                gate_record::record(&session.kernel, agent_id, &ended.with_signal(signal)).await;
            }
            (None, _) => warn!(%agent_id, "{}", check.describe()),
            (Some(_), None) => {}
        }
        Ok(EvalRan {
            error,
            check,
            check_ms,
            gate: GateUse {
                routed: hub.routed_models(agent_id),
                tokens: hub.tokens_by_model(agent_id),
            },
        })
    }
}

/// `verify` as one `sh -c` call, so the command policy sees an allowed
/// program whatever the command starts with (`[`, `!`, `VAR=…`), with
/// cargo building into the workspace's own `target` so workspaces of
/// crates with the same name never share build results.
fn verify_script(verify: &str) -> String {
    let script = format!("export CARGO_TARGET_DIR=\"$PWD/target\"\n{verify}");
    format!("sh -c '{}'", script.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::verify_script;

    #[test]
    fn the_verify_command_runs_as_one_sh_call() {
        assert_eq!(
            verify_script("[ \"$(cat a)\" = 'b' ]"),
            "sh -c 'export CARGO_TARGET_DIR=\"$PWD/target\"\n[ \"$(cat a)\" = '\\''b'\\'' ]'"
        );
    }
}
