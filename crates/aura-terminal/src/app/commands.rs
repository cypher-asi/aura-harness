//! Slash command handling and UI command processing for the App.

use super::{App, AppState, NotificationType, PanelFocus, PendingApproval};
use crate::{
    components::{Message, MessageRole, ToolCard, ToolStatus},
    events::{UiCommand, UiEvent},
    terminal::KeyResult,
};
use tracing::debug;

impl App {
    /// Handle a slash command.
    pub(super) fn handle_command(&mut self, text: &str) -> KeyResult {
        let parts: Vec<&str> = text[1..].splitn(2, ' ').collect();
        let cmd = parts[0].to_lowercase();
        let _arg = parts.get(1).unwrap_or(&"");

        match cmd.as_str() {
            "quit" | "exit" | "q" => {
                if let Some(tx) = &self.event_tx {
                    let _ = tx.try_send(UiEvent::Quit);
                }
                return KeyResult::quit();
            }
            "help" | "?" => {
                self.state = AppState::ShowingHelp;
            }
            "clear" => {
                self.messages.clear();
                self.tools.clear();
                self.scroll_offset = 0;
                self.thinking_content.clear();
                self.is_thinking = false;
            }
            "status" | "s" => {
                if let Some(tx) = &self.event_tx {
                    let _ = tx.try_send(UiEvent::ShowStatus);
                }
            }
            "history" | "h" => {
                if let Some(tx) = &self.event_tx {
                    let _ = tx.try_send(UiEvent::ShowHistory(None));
                }
            }
            "record" | "r" => {
                self.record_panel_visible = !self.record_panel_visible;
                if !self.record_panel_visible && self.focus == PanelFocus::Records {
                    self.focus = PanelFocus::Chat;
                }
            }
            "swarm" | "sw" => {
                self.swarm_panel_visible = !self.swarm_panel_visible;
                if !self.swarm_panel_visible && self.focus == PanelFocus::Swarm {
                    self.focus = PanelFocus::Chat;
                }
                if self.swarm_panel_visible {
                    if let Some(tx) = &self.event_tx {
                        let _ = tx.try_send(UiEvent::RefreshAgents);
                    }
                }
            }
            "login" => {
                self.state = AppState::LoginEmail;
                self.login_email.clear();
                self.status = "Login — enter email".to_string();
            }
            "logout" => {
                if let Some(tx) = &self.event_tx {
                    let _ = tx.try_send(UiEvent::Logout);
                }
            }
            "whoami" | "me" => {
                if let Some(tx) = &self.event_tx {
                    let _ = tx.try_send(UiEvent::Whoami);
                }
            }
            "new" | "n" => {
                self.messages.clear();
                self.tools.clear();
                self.scroll_offset = 0;
                self.streaming_content.clear();
                self.thinking_content.clear();
                self.is_thinking = false;
                if let Some(tx) = &self.event_tx {
                    let _ = tx.try_send(UiEvent::NewSession);
                }
                self.notification =
                    Some(("New session started".to_string(), NotificationType::Success));
            }
            _ => {
                self.notification = Some((
                    format!("Unknown command: /{cmd}. Type /help for available commands."),
                    NotificationType::Warning,
                ));
            }
        }
        KeyResult::continue_running()
    }

    /// Process a UI command from the kernel.
    pub fn process_command(&mut self, cmd: UiCommand) {
        debug!(?cmd, "Processing UI command");
        match cmd {
            UiCommand::SetStatus(status) => self.status = status,
            UiCommand::StartStreaming => self.cmd_start_streaming(),
            UiCommand::AppendText(text) => self.cmd_append_text(&text),
            UiCommand::FinishStreaming => self.cmd_finish_streaming(),
            UiCommand::StartThinking => {
                self.thinking_content.clear();
                self.is_thinking = true;
            }
            UiCommand::AppendThinking(thinking) => self.thinking_content.push_str(&thinking),
            UiCommand::FinishThinking => self.is_thinking = false,
            UiCommand::ShowMessage(data) => self.cmd_show_message(data),
            UiCommand::ShowTool(data) => self.cmd_show_tool(data),
            UiCommand::CompleteTool {
                id,
                result,
                success,
            } => {
                self.cmd_complete_tool(&id, result, success);
            }
            UiCommand::RequestApproval {
                id,
                tool,
                description,
            } => {
                self.pending_approval = Some(PendingApproval {
                    id,
                    tool,
                    description,
                });
                self.state = AppState::AwaitingApproval;
            }
            UiCommand::ShowError(msg) => {
                self.cmd_notification(&msg, "⛔ Error: ", NotificationType::Error)
            }
            UiCommand::ShowSuccess(msg) => {
                self.cmd_notification(&msg, "✓ ", NotificationType::Success)
            }
            UiCommand::ShowWarning(msg) => {
                self.cmd_notification(&msg, "⚠ Warning: ", NotificationType::Warning)
            }
            UiCommand::Complete => self.cmd_complete(),
            UiCommand::ClearConversation => self.cmd_clear_conversation(),
            UiCommand::NewRecord(record) => self.cmd_new_record(record),
            UiCommand::SetAgents(agents) => self.cmd_set_agents(agents),
            UiCommand::SetActiveAgent(agent_id) => self.cmd_set_active_agent(agent_id),
            UiCommand::ClearRecords => {
                self.records.clear();
                self.selected_record = 0;
            }
            UiCommand::SetApiStatus { url, active } => {
                self.api_url = url;
                self.api_active = active;
            }
        }
    }

    fn cmd_start_streaming(&mut self) {
        self.streaming_content.clear();
        self.thinking_content.clear();
        self.is_thinking = false;
        let mut msg = Message::new(MessageRole::Assistant, "");
        msg.set_streaming(true);
        self.add_message(msg);
    }

    fn cmd_append_text(&mut self, text: &str) {
        self.streaming_content.push_str(text);
        if let Some(last_msg) = self.messages.back_mut() {
            if last_msg.is_streaming() {
                last_msg.set_content(&self.streaming_content);
            }
        }
    }

    fn cmd_finish_streaming(&mut self) {
        if let Some(last_msg) = self.messages.back_mut() {
            if last_msg.is_streaming() {
                last_msg.set_streaming(false);
                if self.streaming_content.is_empty() {
                    self.messages.pop_back();
                } else {
                    last_msg.set_content(&self.streaming_content);
                }
            }
        }
        self.streaming_content.clear();
    }

    fn cmd_show_message(&mut self, data: crate::events::MessageData) {
        let mut msg = Message::new(data.role, &data.content);
        if data.is_streaming {
            msg.set_streaming(true);
        }
        self.add_message(msg);
    }

    fn cmd_show_tool(&mut self, data: crate::events::ToolData) {
        let tool = ToolCard::new(&data.id, &data.name).with_args(&data.args);
        self.add_tool(tool);
        let tool_summary = super::format::format_tool_summary(&data.name, &data.args);
        self.add_message(Message::new(
            MessageRole::System,
            &format!("🔧 {} : {}", data.name, tool_summary),
        ));
    }

    fn cmd_complete_tool(&mut self, id: &str, result: String, success: bool) {
        let mut tool_name = String::new();
        for tool in &mut self.tools {
            if tool.id() == id {
                tool_name = tool.name().to_string();
                tool.set_status(if success {
                    ToolStatus::Success
                } else {
                    ToolStatus::Error
                });
                tool.set_result(&result);
            }
        }
        if !result.is_empty() {
            let (icon, prefix) = if success {
                ("✓", "")
            } else {
                ("✗", "Error: ")
            };
            let display_result = if result.len() > 200 {
                format!("{}...", &result[..197])
            } else {
                result
            };
            self.add_message(Message::new(
                MessageRole::System,
                &format!("   {icon} {tool_name}: {prefix}{display_result}"),
            ));
        }
    }

    fn cmd_notification(&mut self, msg: &str, prefix: &str, kind: NotificationType) {
        self.add_message(Message::new(MessageRole::System, &format!("{prefix}{msg}")));
        self.notification = Some((msg.to_string(), kind));
    }

    fn cmd_complete(&mut self) {
        if let Some(last_msg) = self.messages.back_mut() {
            if last_msg.is_streaming() {
                last_msg.set_streaming(false);
                if !self.streaming_content.is_empty() {
                    last_msg.set_content(&self.streaming_content);
                } else if last_msg.content().is_empty() {
                    self.messages.pop_back();
                }
            }
        }
        self.streaming_content.clear();
        self.is_thinking = false;
        self.state = AppState::Idle;
        self.status = "Ready".to_string();
        self.tools.clear();
    }

    fn cmd_clear_conversation(&mut self) {
        self.messages.clear();
        self.tools.clear();
        self.thinking_content.clear();
        self.is_thinking = false;
    }

    fn cmd_new_record(&mut self, record: crate::events::RecordSummary) {
        self.records.push_front(record);
        while self.records.len() > super::MAX_RECORDS {
            self.records.pop_back();
        }
        if self.selected_record >= self.records.len() && !self.records.is_empty() {
            self.selected_record = self.records.len() - 1;
        }
    }

    fn cmd_set_agents(&mut self, agents: Vec<crate::events::AgentSummary>) {
        self.agents = agents;
        if let Some(idx) = self
            .agents
            .iter()
            .position(|a| a.id == self.active_agent_id)
        {
            self.selected_agent = idx;
        }
        if self.selected_agent >= self.agents.len() && !self.agents.is_empty() {
            self.selected_agent = self.agents.len() - 1;
        }
    }

    fn cmd_set_active_agent(&mut self, agent_id: String) {
        self.active_agent_id = agent_id;
        for agent in &mut self.agents {
            agent.is_active = agent.id == self.active_agent_id;
        }
        if let Some(idx) = self.agents.iter().position(|a| a.is_active) {
            self.selected_agent = idx;
        }
    }
}
