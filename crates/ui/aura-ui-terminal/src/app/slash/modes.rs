//! Mode commands: plan mode, building plans, work modes and approval levels.

use crate::{
    app::{App, AppState, NotificationType},
    components::{Message, MessageRole},
    events::UiEvent,
    mode_command::{parse_approval_command, ApprovalLevel, PermissionMode, SlashModeCommand},
};

impl App {
    /// Run `cmd` when it is a mode command; whether it was one.
    pub(super) fn mode_command(&mut self, cmd: &str, arg: &str) -> bool {
        match cmd {
            "plan" => self.request_plan_toggle(),
            "build" => self.request_build(arg),
            "mode" => self.handle_mode_command(arg),
            "approval" | "approvals" => self.handle_approval_command(arg),
            _ => return false,
        }
        true
    }

    /// Whether a turn owns the session, so the mode can't change under it.
    fn turn_running(&self) -> bool {
        matches!(
            self.state,
            AppState::Processing | AppState::AwaitingApproval
        )
    }

    /// `/plan`: enter plan mode, or leave it when the agent is already there.
    /// The event loop knows the mode from before plan mode, so the same
    /// command restores it.
    pub(in crate::app) fn request_plan_toggle(&mut self) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::TogglePlan(self.active_agent_id.clone()));
        }
    }

    /// `/plan <message>`: put the active agent in plan mode, staying there
    /// when it already is, so the message that follows is sent under it.
    /// The event loop handles events in order, so the switch is recorded
    /// before the message starts a turn.
    pub(in crate::app) fn request_plan_mode(&mut self) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::SetMode {
                agent_id: self.active_agent_id.clone(),
                mode: PermissionMode::Plan,
            });
        }
    }

    /// Ask the event loop to switch the active agent to `mode`. The badge
    /// changes once the switch is recorded and [`UiCommand::SetMode`] comes
    /// back.
    pub(in crate::app) fn request_mode(&mut self, mode: PermissionMode) {
        if self.hud.mode == Some(mode) {
            self.notifications.current =
                Some((format!("Already in {mode} mode"), NotificationType::Warning));
            return;
        }
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::SetMode {
                agent_id: self.active_agent_id.clone(),
                mode,
            });
        }
    }

    /// Handle `/build [path]`: run the given plan, or the latest one.
    pub(in crate::app) fn request_build(&mut self, arg: &str) {
        if self.turn_running() {
            self.notifications.current = Some((
                "Wait for the current turn to finish before building".to_string(),
                NotificationType::Warning,
            ));
            return;
        }
        let path = (!arg.is_empty()).then(|| arg.to_string());
        if path.is_none() && self.account.active_plan.is_none() {
            self.notifications.current = Some((
                "No plan to build yet. Switch to plan mode with /plan and ask for one.".to_string(),
                NotificationType::Warning,
            ));
            return;
        }
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::BuildPlan {
                agent_id: self.active_agent_id.clone(),
                path,
            });
        }
    }

    /// Whether `text` asks a plan-mode agent with a plan to build it, the
    /// way `/build` does: a bare "implement", "build it", "go ahead"...
    /// Anything longer is feedback on the plan and goes to the agent.
    pub(in crate::app) fn wants_build(&self, text: &str) -> bool {
        const PHRASES: &[&str] = &[
            "implement",
            "implement it",
            "implement this",
            "implement plan",
            "implement the plan",
            "build",
            "build it",
            "build the plan",
            "go ahead",
            "go ahead and implement",
            "go ahead and implement it",
            "do it",
            "proceed",
        ];
        if self.hud.mode != Some(PermissionMode::Plan) || self.account.active_plan.is_none() {
            return false;
        }
        let text = text
            .trim()
            .trim_end_matches(['.', '!'])
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        PHRASES.contains(&text.as_str())
    }

    /// `/mode`: show the agent's work mode and the others; `/mode
    /// <name>`: switch to one for the agent's next tool calls.
    fn handle_mode_command(&mut self, arg: &str) {
        if arg.is_empty() {
            let current = self.hud.mode.unwrap_or_default();
            let mut report = format!("Mode: {current} — {}", current.description());
            for mode in [
                PermissionMode::Agent,
                PermissionMode::Plan,
                PermissionMode::ReadOnly,
                PermissionMode::Debug,
            ] {
                report.push_str(&format!("\n  {:<14}{}", mode.name(), mode.description()));
            }
            report.push_str("\n/plan enters plan mode, or leaves it.");
            report.push_str("\n/approval picks how much asks first.");
            self.add_message(Message::new(MessageRole::System, &report));
            return;
        }
        match SlashModeCommand::parse(arg) {
            Ok(command) => self.request_mode(command.requested),
            Err(e) => {
                self.notifications.current = Some((e.to_string(), NotificationType::Warning));
            }
        }
    }

    /// `/approval`: show the agent's approval level and the others;
    /// `/approval <level>`: switch to one for the agent's next tool calls.
    fn handle_approval_command(&mut self, arg: &str) {
        if arg.is_empty() {
            let current = self.hud.approval.unwrap_or_default();
            let mut report = format!("Approval: {current} — {}", current.description());
            for level in [
                ApprovalLevel::Yolo,
                ApprovalLevel::Ask,
                ApprovalLevel::Strict,
            ] {
                report.push_str(&format!("\n  {:<14}{}", level.name(), level.description()));
            }
            report.push_str("\nShift+Tab cycles ask → strict → yolo.");
            self.add_message(Message::new(MessageRole::System, &report));
            return;
        }
        match parse_approval_command(arg) {
            Ok(level) => self.request_approval(level),
            Err(e) => {
                self.notifications.current = Some((e.to_string(), NotificationType::Warning));
            }
        }
    }

    /// Ask the event loop to switch the active agent to `level`. The badge
    /// changes once the switch is recorded and [`UiCommand::SetApproval`]
    /// comes back.
    pub(in crate::app) fn request_approval(&mut self, level: ApprovalLevel) {
        if self.hud.approval == Some(level) {
            self.notifications.current = Some((
                format!("Already at approval level {level}"),
                NotificationType::Warning,
            ));
            return;
        }
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::SetApproval {
                agent_id: self.active_agent_id.clone(),
                level,
            });
        }
    }
}
