//! `/compare`: run the user's own task (a prompt, a plan, or a session
//! replayed prompt by prompt) on two arms side by side, each Gate or a
//! pinned model, then apply the better run. The BENCH tab drives
//! the same actions (see [`super::bench`]).
//!
//! `/compare [prompt]` prepares a comparison and shows what it should
//! cost; `/compare go` starts it (see [`run`]). Each
//! comparison is an agent (`Compare #n`) under the agent that asked,
//! with its runs under it; its state is the latest `compare` system
//! record on its own record, which is also where `/compare` lists past
//! comparisons from. Copies are removed by `/compare apply` and
//! `/compare dismiss`.
//!
//! - `handle`: `/compare`'s subcommands, and the list and the view of
//!   one comparison they show
//! - `prepare`: the comparison a request asks for, and what it should
//!   cost
//! - `lifecycle`: starting a comparison, watching its task and
//!   cancelling it
//! - `ended`: applying, showing and dismissing the runs of a comparison
//!   that ended
//! - `record`: each comparison's latest state, read from its record
//! - [`run`]: running a comparison: its runs in their copies, their
//!   checks, the judge, and the record of how it ended
//! - [`view`]: what `/compare` shows

mod ended;
mod handle;
mod lifecycle;
mod prepare;
mod record;
pub(super) mod run;
pub(super) mod view;

pub(super) use ended::{apply, diff, dismiss};
pub(super) use handle::handle;
pub(super) use lifecycle::{cancel, start};
pub(super) use prepare::prepare;
pub(super) use record::comparisons;

use super::LoopState;
use aura_agent_kernel::Kernel;
use aura_core_types::AgentId;
use aura_gate::compare::DEFAULT_CONCURRENCY;
use run::ComparePlan;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;

/// A comparison under way.
struct Running {
    host: AgentId,
    name: String,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

/// `/compare`'s state in the event loop: where copies go, the
/// comparison waiting for `/compare go`, and the one running.
pub(crate) struct CompareState {
    root: PathBuf,
    concurrency: usize,
    pending: Option<ComparePlan>,
    running: Option<Running>,
}

impl CompareState {
    /// Copies under `root`, at most `concurrency` runs at once.
    pub(crate) fn new(root: PathBuf, concurrency: usize) -> Self {
        Self {
            root,
            concurrency,
            pending: None,
            running: None,
        }
    }

    /// Copies under `AURA_HOME/compare`, else under `data_dir`.
    pub(crate) fn in_aura_home(data_dir: &Path) -> Self {
        let root = aura_config::AuraHome::resolve().map_or_else(
            |e| {
                warn!(error = %e, "AURA_HOME unresolved; /compare copies go in the data directory");
                data_dir.join("compare")
            },
            |home| home.path.join("compare"),
        );
        Self::new(root, DEFAULT_CONCURRENCY)
    }

    fn running(&self) -> Option<&Running> {
        self.running.as_ref().filter(|r| !r.task.is_finished())
    }

    /// The agent holding the comparison under way.
    pub(super) fn running_host(&self) -> Option<AgentId> {
        self.running().map(|r| r.host)
    }

    /// Wait for the running comparison to end (for tests).
    #[cfg(test)]
    pub(super) async fn finished(&mut self) {
        if let Some(running) = self.running.take() {
            let _ = running.task.await;
        }
    }
}

/// The kernel of `agent_id`, opening its session when nothing has.
pub(super) fn kernel_of(
    state: &mut LoopState<'_>,
    agent_id: AgentId,
) -> Result<Arc<Kernel>, String> {
    if let Some(kernel) = state.kernel_for(agent_id) {
        return Ok(kernel);
    }
    let session = super::agents::open_session(state, agent_id)?;
    let kernel = Arc::clone(&session.kernel);
    state.idle.insert(agent_id, session);
    Ok(kernel)
}
