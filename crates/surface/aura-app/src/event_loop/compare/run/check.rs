//! The check of a run's work once its last turn ended: the check command
//! runs in the copy as a `run_command` call through the run's kernel,
//! with an example task's hidden checks in place and the task records
//! out of its way, and what it found is the outcome of the run's trace.

use super::turn::RunParts;
use crate::bench_pack;
use crate::event_loop::agents::AgentSession;
use crate::event_loop::handlers::system_message;
use crate::event_loop::processes::now_ms;
use crate::event_loop::task_check::{self, CheckRun};
use crate::event_loop::tasks::TASKS_DIR;
use aura_agent_kernel::Kernel;
use aura_core_types::trace::{Reward, RewardKind, RewardTarget};
use aura_core_types::{CompareCopy, OutcomePayload};
use aura_ui_terminal::UiCommand;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::warn;

/// `ToolProposal::source` of a run's check command.
const CHECK_SOURCE: &str = "compare_check";

/// What a run's check found.
#[derive(Default)]
pub(super) struct Checked {
    /// Whether the check passed; `None` when there was none, or it gave
    /// no verdict
    pub passed: Option<bool>,
    /// The run's last turn's `Outcome` as the check judged it, recorded on
    /// its kernel once every run has ended, so no run is routed by
    /// another's hidden checks
    pub outcome: Option<(Arc<Kernel>, OutcomePayload)>,
}

/// Run the check of `parts`' run on `session`, which works in `copy`,
/// unless the user stopped the run or it has no check. `last` is its last
/// turn's `Outcome`, which the check's verdict follows.
pub(super) async fn check(
    parts: &RunParts<'_>,
    session: &AgentSession,
    copy: &CompareCopy,
    last: Option<&OutcomePayload>,
) -> Checked {
    let (plan, run) = (&parts.ctx.plan, parts.run);
    let check = run
        .example
        .map_or(plan.check.as_deref(), |task| Some(task.verify.as_str()));
    let Some(check) = check.filter(|_| !parts.cancel.is_cancelled()) else {
        return Checked::default();
    };
    let workdir = session.kernel.agent_workspace();
    let hidden = match run.example {
        Some(task) => task
            .place_checks(&workdir)
            .map_err(|e| format!("putting the hidden checks of {} in place: {e}", task.id)),
        None => Ok(Vec::new()),
    };
    let hidden = match hidden {
        Ok(hidden) => hidden,
        Err(problem) => {
            warn!(agent_id = %session.agent_id(), %problem, "Could not check an example run");
            parts.ui.send(UiCommand::ShowWarning(problem)).await;
            return Checked::default();
        }
    };
    let records = RecordsAside::take(&workdir, &copy.root, &run.name);
    let checked = task_check::run_script(session, check, CHECK_SOURCE).await;
    records.put_back();
    bench_pack::remove_checks(&hidden);
    parts.ui.send(system_message(checked.describe())).await;
    if let Some(recorder) = &parts.recorder {
        recorder.add_rewards(&[check_reward(&checked)]);
    }
    let outcome = (checked.signal().zip(last))
        .map(|(signal, ended)| (Arc::clone(&session.kernel), ended.with_signal(signal)));
    Checked {
        passed: checked.passed,
        outcome,
    }
}

/// What the run's check found, as the outcome of its trace.
fn check_reward(run: &CheckRun) -> Reward {
    Reward {
        target: RewardTarget::Run,
        kind: RewardKind::Outcome,
        name: "verify".into(),
        value: if run.passed == Some(true) { 1.0 } else { 0.0 },
        source: "bench".into(),
        at_ms: now_ms(),
        evidence: serde_json::json!({
            "cmd": run.command,
            "exit_code": run.exit_code,
            "output_tail": run.tail,
        }),
    }
}

/// The task records a run's subtasks left in its copy, moved out of it
/// while its check runs: they quote the prompts, so a check that greps the
/// copy would find them.
pub(super) struct RecordsAside {
    /// Where they were, and where they are meanwhile
    moved: Option<(PathBuf, PathBuf)>,
}

impl RecordsAside {
    /// Move the records under `workdir` beside the copy at `root`, which
    /// the run `name` works in.
    pub(super) fn take(workdir: &Path, root: &Path, name: &str) -> Self {
        let records = workdir.join(TASKS_DIR);
        let aside = root.with_file_name(format!("{name}.tasks"));
        let moved = records.is_dir() && !aside.exists();
        let moved = match moved.then(|| std::fs::rename(&records, &aside)) {
            Some(Ok(())) => Some((records, aside)),
            Some(Err(e)) => {
                warn!(run = name, error = %e, "Could not move the task records out of the check's way");
                None
            }
            None => None,
        };
        Self { moved }
    }

    /// Put the records back where they were.
    pub(super) fn put_back(self) {
        if let Some((records, aside)) = self.moved {
            if let Err(e) = std::fs::rename(&aside, &records) {
                warn!(error = %e, from = %aside.display(), "Could not put the task records back");
            }
        }
    }
}
