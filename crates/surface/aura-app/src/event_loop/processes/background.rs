//! Hands background commands to the process manager.

use super::ProcessTable;
use aura_agent::{NewProcess, ProcessManager, ProcessMeta};
use aura_core_types::{ActionId, AgentId, Hash, ProcessId};
use aura_exec_tools::{BackgroundJob, BackgroundProcess, BackgroundProcessHook};
use aura_store_record::ReadStore;
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use tracing::warn;

/// Hands `run_command`'s background commands to the [`ProcessManager`],
/// which reports each one's exit to the agent that started it, and lists
/// them in `table`.
pub struct BackgroundCommands {
    manager: Arc<ProcessManager>,
    store: Arc<dyn ReadStore>,
    log_dir: PathBuf,
    /// Weak: the table's agent host reaches this hook through the kernels
    /// it builds for spawned agents.
    table: Weak<ProcessTable>,
}

impl BackgroundCommands {
    #[must_use]
    pub fn new(
        manager: Arc<ProcessManager>,
        store: Arc<dyn ReadStore>,
        log_dir: PathBuf,
        table: &Arc<ProcessTable>,
    ) -> Self {
        Self {
            manager,
            store,
            log_dir,
            table: Arc::downgrade(table),
        }
    }

    /// The agent's latest recorded transaction, which the completion
    /// refers back to.
    fn latest_tx(&self, agent: AgentId) -> Option<Hash> {
        let seq = self.store.get_head_seq(agent).ok()?;
        self.store
            .get_record_entry(agent, seq)
            .ok()
            .map(|entry| entry.tx.hash)
    }
}

impl BackgroundProcessHook for BackgroundCommands {
    fn log_dir(&self) -> PathBuf {
        self.log_dir.clone()
    }

    fn adopt(&self, agent_id: AgentId, process: BackgroundProcess) -> Result<(), String> {
        let reference = self
            .latest_tx(agent_id)
            .unwrap_or_else(|| Hash::from_content(process.command.as_bytes()));
        if tokio::runtime::Handle::try_current().is_err() {
            let mut child = process.child;
            let _ = child.kill();
            let _ = child.wait();
            return Err("no async runtime to watch the process".to_string());
        }
        let process_id = process.process_id;
        self.manager.register_with(NewProcess {
            agent_id,
            reference_tx_hash: reference,
            action_id: process.action_id.unwrap_or_else(ActionId::generate),
            process_id,
            child: process.child,
            command: process.command,
            meta: ProcessMeta {
                cwd: Some(process.cwd),
                log_path: Some(process.log_path),
                tool_use_id: process.tool_use_id,
            },
        });
        let info = self
            .manager
            .snapshot()
            .into_iter()
            .find(|info| info.process_id == process_id);
        if let (Some(info), Some(table)) = (info, self.table.upgrade()) {
            table.command_started(&info);
        }
        Ok(())
    }

    fn running(&self, agent_id: Option<AgentId>) -> Vec<BackgroundJob> {
        let mut jobs: Vec<BackgroundJob> = self
            .manager
            .snapshot()
            .into_iter()
            .filter(|info| agent_id.is_none_or(|agent| info.agent_id == agent))
            .map(|info| BackgroundJob {
                id: info.process_id.to_hex(),
                agent_id: info.agent_id,
                command: info.command,
                pid: Some(info.pid),
                log_path: info.meta.log_path.unwrap_or_default(),
                started_ms: info.started_ms,
            })
            .collect();
        jobs.sort_by_key(|job| job.started_ms);
        jobs
    }

    fn kill(&self, id: &str) -> Result<(), String> {
        let process = ProcessId::from_hex(id).map_err(|e| format!("bad job id {id}: {e}"))?;
        let table = self.table.upgrade();
        if let Some(table) = &table {
            table.stopped_by_owner(process);
        }
        if self.manager.cancel(&process) {
            return Ok(());
        }
        if let Some(table) = &table {
            table.take_stopped_by_owner(process);
        }
        Err(format!("job {id} is not running"))
    }
}

/// Stop every background command, and everything each one started. They
/// lead their own process groups, so nothing else stops them when the
/// harness exits.
pub(crate) fn stop_background_commands(hook: &dyn BackgroundProcessHook) {
    for job in hook.running(None) {
        if let Err(problem) = hook.kill(&job.id) {
            warn!(job = %job.id, %problem, "Could not stop a background command");
        }
    }
}
