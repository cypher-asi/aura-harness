//! `aura eval run --model gate`: each run of a task is an agent of its
//! own under the Headless agent, working in the task's workspace, and its
//! prompts are `aura -p` turns. Gate classifies and routes them with the
//! table as configured (exploration included), the agent leads the turns
//! that call for a lead and its `task` subtasks are routed and escalated,
//! and every prompt ends with its `Outcome`. The task's verify command then runs as a `run_command`
//! call through the agent's kernel, and its verdict is recorded as a
//! later `Outcome` of the last prompt, so eval results count in the Gate
//! table of the data directory.

use super::output::Output;
use super::run::{Headless, RunLimits};
use super::OutputFormat;
use crate::event_loop::subagents::lock;
use crate::event_loop::task_check::{run_script, CheckRun};
use aura_agent_trace::RunRecorder;
use aura_core_types::ModelTokens;
use aura_gate::live::{self, Judged, RunOf};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::warn;

/// `ToolProposal::source` of a task's verify command.
const VERIFY_SOURCE: &str = "eval_verify";

/// One run of an eval task.
pub struct EvalTurn<'a> {
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
pub struct EvalRan {
    /// Why the turns stopped early, if they did
    pub error: Option<String>,
    /// The verify command's run
    pub check: CheckRun,
    /// How long it took, in milliseconds
    pub check_ms: u64,
    pub gate: GateUse,
}

/// Runs eval tasks on a headless harness.
pub struct EvalTurns {
    headless: Headless,
    output: Output,
}

impl EvalTurns {
    pub fn new(headless: Headless) -> Self {
        Self {
            headless,
            output: Output::new(OutputFormat::Text, Box::new(std::io::sink())),
        }
    }

    /// What to show before the first run (see [`Headless::setup_lines`]).
    #[must_use]
    pub fn setup_lines(&self) -> Vec<String> {
        self.headless.setup_lines()
    }

    /// Run `turn`'s prompts, then its verify command.
    ///
    /// # Errors
    /// When the run's agent can't be registered or opened.
    pub async fn run(&mut self, turn: EvalTurn<'_>) -> anyhow::Result<EvalRan> {
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
        crate::event_loop::channels::join_or_warn(&hub, &agent);
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
        self.headless.judged.clear();
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
            copy_dir(checks, turn.workspace)?;
        }
        let check_started = Instant::now();
        let check = run_script(&session, &verify_script(turn.verify), VERIFY_SOURCE).await;
        let check_ms = u64::try_from(check_started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match (check.signal(), &last) {
            (Some(signal), Some(ended)) => {
                if let Some(passed) = signal.success() {
                    let of = RunOf {
                        compare: "aura eval",
                        run: turn.name,
                        task: None,
                        copy: None,
                    };
                    hub.gate.log_truth(agent_id, ended.prompt_tx, passed, of);
                }
                live::record(&session.kernel, agent_id, &ended.with_signal(signal)).await;
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
                judged: std::mem::take(&mut self.headless.judged),
            },
        })
    }
}

/// What Gate did in one run of a task.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GateUse {
    /// Models Gate routed the run's turns and subtasks to, first used
    /// first.
    pub routed: Vec<String>,
    /// Model calls of the run and its subtasks per model and purpose,
    /// classifier and judge calls included.
    pub tokens: Vec<ModelTokens>,
    /// What Gate's judge found on the turns that failed, oldest first.
    pub judged: Vec<Judged>,
}

impl GateUse {
    /// What the judge's calls cost in USD: paid to learn from failed
    /// turns, and part of `tokens` too.
    #[must_use]
    pub fn judge_cost_usd(&self) -> f64 {
        self.judged
            .iter()
            .filter_map(|judged| judged.cost_usd)
            .sum()
    }

    /// The judge's calls whose price is unknown, which
    /// [`Self::judge_cost_usd`] leaves out.
    #[must_use]
    pub fn judge_unpriced_calls(&self) -> usize {
        self.judged.iter().filter(|j| j.cost_usd.is_none()).count()
    }
}

/// Copy the files under `from` into `to`, keeping relative paths.
///
/// # Errors
/// Fails on an I/O error.
pub fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            std::fs::create_dir_all(&target)?;
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
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
