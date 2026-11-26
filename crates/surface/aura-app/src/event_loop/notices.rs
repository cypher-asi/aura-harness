//! Messages for an agent that don't come from the user: a background
//! command it started has ended, a task it handed off has finished, an
//! agent it spawned replied.
//!
//! A notice goes into the agent's running turn if it has one, else it
//! starts a turn, whether or not the agent is on screen.

use super::agent_events::AgentUi;
use super::handlers::system_message;
use super::processes::ProcessTable;
use super::steering::SteerSource;
use super::{deliver, LoopState};
use aura_agent_kernel::Kernel;
use aura_core_types::{ActionResultPayload, AgentId, Transaction};
use aura_ui::events::ProcessSummary;
use std::path::Path;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

/// Lines of a background command's output included in its notice.
const LOG_TAIL_LINES: usize = 20;

/// A message for an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub to: AgentId,
    /// One line for the chat.
    pub shown: String,
    /// What the agent is told.
    pub prompt: String,
}

/// Where notices are sent, for work that finishes off the event loop.
pub type NoticeSender = mpsc::UnboundedSender<Notice>;

/// Show `notice` in its agent's chat and give it to the agent.
pub(super) async fn handle_notice(state: &mut LoopState<'_>, notice: Notice) {
    info!(agent = %notice.to, shown = %notice.shown, "Delivering a notice");
    AgentUi::new(state.commands.clone(), notice.to)
        .send(system_message(notice.shown))
        .await;
    deliver(state, notice.to, SteerSource::Harness, notice.prompt, false).await;
}

/// The notice for the background command whose completion is `tx`, after
/// noting in `table` how it ended. `None` when the payload is unreadable,
/// the command is unknown, or its own agent stopped it.
pub(crate) fn completion_notice(table: &ProcessTable, tx: &Transaction) -> Option<Notice> {
    let result = serde_json::from_slice::<ActionResultPayload>(&tx.payload).ok()?;
    let row = table.command_finished(result.process_id, result.exit_code, result.success);
    if table.take_stopped_by_owner(result.process_id) {
        info!(process = %result.process_id.to_hex(), "Background command stopped by its agent");
        return None;
    }
    row.map(|row| command_notice(tx.agent_id, &row))
}

/// Append the background command completion `tx` to the record of
/// `kernel`'s agent. Returns whether it was recorded; failures are logged.
pub(crate) async fn record_completion(kernel: &Kernel, tx: &Transaction) -> bool {
    let store = kernel.store();
    if let Err(e) = store.enqueue_tx(tx) {
        error!(error = %e, "Failed to enqueue completion transaction");
        return false;
    }
    let Ok(Some((token, dequeued))) = store.dequeue_tx(tx.agent_id) else {
        return false;
    };
    match kernel.process_dequeued(dequeued, token).await {
        Ok(result) => {
            debug!(
                seq = result.entry.seq,
                "Completion record persisted via kernel"
            );
            true
        }
        Err(e) => {
            error!(error = %e, "Failed to persist completion record via kernel");
            false
        }
    }
}

/// The notice telling an agent that its background command `row` ended.
#[must_use]
pub fn command_notice(to: AgentId, row: &ProcessSummary) -> Notice {
    let how = match (row.status, row.exit_code) {
        (aura_ui::events::ProcessStatus::Killed, _) => {
            "was stopped from the Processes tab".to_string()
        }
        (_, Some(code)) => format!("exited with code {code}"),
        (_, None) => "ended".to_string(),
    };
    let pid = row
        .pid
        .map(|pid| format!(" (pid {pid})"))
        .unwrap_or_default();
    let job = row
        .id
        .strip_prefix("cmd:")
        .map(|id| format!(" (job {id})"))
        .unwrap_or_default();
    let mut prompt = format!("The background command `{}`{pid}{job} {how}.", row.label);
    if let Some(log) = &row.log_path {
        let tail = log_tail(log, LOG_TAIL_LINES);
        prompt.push_str(&format!("\nIts output is in {}.", log.display()));
        if !tail.is_empty() {
            prompt.push_str(&format!(" The last lines:\n```\n{tail}\n```"));
        }
    }
    Notice {
        to,
        shown: format!("Background command {how}: {}", row.label),
        prompt,
    }
}

/// The last `max_lines` lines of `path`.
fn log_tail(path: &Path, max_lines: usize) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(max_lines)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_ui::events::{ProcessKind, ProcessStatus};

    fn row(status: ProcessStatus, exit_code: Option<i32>, log: Option<&Path>) -> ProcessSummary {
        ProcessSummary {
            id: "cmd:1".into(),
            kind: ProcessKind::Command,
            label: "npm run dev".into(),
            status,
            pid: Some(42),
            exit_code,
            log_path: log.map(Path::to_path_buf),
            ..ProcessSummary::default()
        }
    }

    #[test]
    fn command_notice_reports_exit_and_log_tail() {
        let dir = tempfile::TempDir::new().unwrap();
        let log = dir.path().join("1.log");
        let output: Vec<String> = (1..=30).map(|i| format!("line {i}")).collect();
        std::fs::write(&log, output.join("\n")).unwrap();
        let agent = AgentId::generate();

        let notice = command_notice(agent, &row(ProcessStatus::Failed, Some(1), Some(&log)));
        assert_eq!(notice.to, agent);
        assert_eq!(
            notice.shown,
            "Background command exited with code 1: npm run dev"
        );
        assert!(notice.prompt.starts_with(
            "The background command `npm run dev` (pid 42) (job 1) exited with code 1."
        ));
        assert!(notice.prompt.contains(&log.display().to_string()));
        assert!(notice.prompt.contains("line 30"));
        assert!(notice.prompt.contains("line 11\n"));
        assert!(!notice.prompt.contains("line 10\n"));
    }

    #[test]
    fn command_notice_says_when_the_user_stopped_it() {
        let notice = command_notice(AgentId::generate(), &row(ProcessStatus::Killed, None, None));
        assert_eq!(
            notice.prompt,
            "The background command `npm run dev` (pid 42) (job 1) was stopped from the Processes tab."
        );
    }
}
