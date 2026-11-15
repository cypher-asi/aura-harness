//! Messages for an agent that don't come from the user: a background
//! command it started has ended, a task it handed off has finished.
//!
//! A notice for the agent the UI shows starts a turn right away; one for
//! another agent waits until the user opens that agent.

use super::{handlers, local_model_unavailable, record_prompt, run_turn, LoopState};
use aura_core_types::{AgentId, Transaction, TransactionType};
use aura_model_reasoner::Message;
use aura_terminal::events::ProcessSummary;
use aura_terminal::UiCommand;
use std::path::Path;
use tokio::sync::mpsc;
use tracing::info;

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

/// Deliver `notice`: run a turn now if its agent is on screen, otherwise
/// keep it for when the agent is opened.
pub(super) async fn handle_notice(state: &mut LoopState<'_>, notice: Notice) {
    state.park_finished_spawns();
    if notice.to == state.session.agent_id() {
        run_notice_turn(state, notice).await;
    } else {
        info!(agent = %notice.to, "Queueing notice for an agent that isn't open");
        state.queued_notices.push(notice);
    }
}

/// Deliver the notices that waited for the agent now on screen.
pub(super) async fn deliver_queued(state: &mut LoopState<'_>) {
    let agent = state.session.agent_id();
    let (mine, rest): (Vec<Notice>, Vec<Notice>) = std::mem::take(&mut state.queued_notices)
        .into_iter()
        .partition(|notice| notice.to == agent);
    state.queued_notices = rest;
    if mine.is_empty() {
        return;
    }
    let shown: Vec<String> = mine.iter().map(|n| n.shown.clone()).collect();
    let prompt: Vec<String> = mine.iter().map(|n| n.prompt.clone()).collect();
    run_notice_turn(
        state,
        Notice {
            to: agent,
            shown: shown.join("\n"),
            prompt: prompt.join("\n\n"),
        },
    )
    .await;
}

async fn run_notice_turn(state: &mut LoopState<'_>, notice: Notice) {
    info!(agent = %notice.to, "Running a turn for a notice");
    handlers::show_system_message(state, notice.shown).await;
    let _ = state.commands.send(UiCommand::TurnStarted).await;
    if local_model_unavailable(state).await {
        return;
    }
    let tx = Transaction::new_chained(
        notice.to,
        TransactionType::Trigger,
        notice.prompt.clone().into_bytes(),
        None,
    );
    if record_prompt(state, tx).await {
        state.session.messages.push(Message::user(notice.prompt));
        run_turn(state).await;
    }
}

/// The notice telling an agent that its background command `row` ended.
#[must_use]
pub fn command_notice(to: AgentId, row: &ProcessSummary) -> Notice {
    let how = match (row.status, row.exit_code) {
        (aura_terminal::events::ProcessStatus::Killed, _) => {
            "was stopped from the Processes tab".to_string()
        }
        (_, Some(code)) => format!("exited with code {code}"),
        (_, None) => "ended".to_string(),
    };
    let pid = row
        .pid
        .map(|pid| format!(" (pid {pid})"))
        .unwrap_or_default();
    let mut prompt = format!("The background command `{}`{pid} {how}.", row.label);
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
    use aura_terminal::events::{ProcessKind, ProcessStatus};

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
        assert!(notice
            .prompt
            .starts_with("The background command `npm run dev` (pid 42) exited with code 1."));
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
            "The background command `npm run dev` (pid 42) was stopped from the Processes tab."
        );
    }
}
