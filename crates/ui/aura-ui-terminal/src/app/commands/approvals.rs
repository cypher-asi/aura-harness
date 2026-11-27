//! Tool calls waiting for the user's approval.

use crate::app::{App, AppState, PendingApproval};
use crate::events::UiCommand;

impl App {
    /// Apply a command that opens or closes an approval prompt; any other
    /// comes back.
    pub(super) fn approval_ui_command(&mut self, cmd: UiCommand) -> Option<UiCommand> {
        match cmd {
            UiCommand::RequestApproval {
                id,
                tool,
                description,
                always,
            } => self.cmd_request_approval(PendingApproval {
                id,
                tool,
                description,
                always,
            }),
            UiCommand::CloseApproval(id) => self.cmd_close_approval(&id),
            other => return Some(other),
        }
        None
    }

    fn cmd_request_approval(&mut self, approval: PendingApproval) {
        if self.pending.approvals.is_empty() {
            self.status = format!("Waiting for approval to run {}", approval.tool);
        }
        if !self.pending.approvals.iter().any(|a| a.id == approval.id) {
            self.mark_awaiting_approval(&approval.id, true);
            self.pending.approvals.push_back(approval);
        }
        // Over any dialog open, which is back once the call is answered.
        self.chat_state = AppState::AwaitingApproval;
    }

    /// Drop the prompt for approval `id`, which stopped waiting; the turn
    /// goes on without it.
    fn cmd_close_approval(&mut self, id: &str) {
        let before = self.pending.approvals.len();
        self.pending.approvals.retain(|a| a.id != id);
        if self.pending.approvals.len() < before {
            self.mark_awaiting_approval(id, false);
            self.after_approval_closed();
        }
    }

    /// Mark the tool call behind approval `request_id` as waiting for the
    /// user, or no longer waiting. The kernel's request id is
    /// `<agent hex>:<tool use id>`.
    pub(in crate::app) fn mark_awaiting_approval(&mut self, request_id: &str, waiting: bool) {
        let tool_use_id = request_id
            .split_once(':')
            .map_or(request_id, |(_, tool_use_id)| tool_use_id);
        self.update_tool(tool_use_id, |tool| tool.awaiting_approval = waiting);
    }

    /// Show the next waiting prompt, or go back to the turn when none is
    /// left.
    pub(in crate::app) fn after_approval_closed(&mut self) {
        if let Some(next) = self.pending.approvals.front() {
            self.status = format!("Waiting for approval to run {}", next.tool);
        } else if self.chat_state == AppState::AwaitingApproval {
            self.chat_state = AppState::Processing;
        }
    }
}
