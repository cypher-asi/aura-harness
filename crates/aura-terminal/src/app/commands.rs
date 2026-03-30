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
    #[allow(clippy::too_many_lines)]
    pub fn process_command(&mut self, cmd: UiCommand) {
        debug!(?cmd, "Processing UI command");
        match cmd {
            UiCommand::SetStatus(status) => {
                self.status = status;
            }
            UiCommand::StartStreaming => {
                self.streaming_content.clear();
                self.thinking_content.clear();
                self.is_thinking = false;
                let mut msg = Message::new(MessageRole::Assistant, "");
                msg.set_streaming(true);
                self.add_message(msg);
            }
            UiCommand::AppendText(text) => {
                self.streaming_content.push_str(&text);
                if let Some(last_msg) = self.messages.back_mut() {
                    if last_msg.is_streaming() {
                        last_msg.set_content(&self.streaming_content);
                    }
                }
            }
            UiCommand::FinishStreaming => {
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
            UiCommand::StartThinking => {
                self.thinking_content.clear();
                self.is_thinking = true;
            }
            UiCommand::AppendThinking(thinking) => {
                self.thinking_content.push_str(&thinking);
            }
            UiCommand::FinishThinking => {
                self.is_thinking = false;
            }
            UiCommand::ShowMessage(data) => {
                let mut msg = Message::new(data.role, &data.content);
                if data.is_streaming {
                    msg.set_streaming(true);
                }
                self.add_message(msg);
            }
            UiCommand::ShowTool(data) => {
                let tool = ToolCard::new(&data.id, &data.name).with_args(&data.args);
                self.add_tool(tool);

                let tool_summary = super::format::format_tool_summary(&data.name, &data.args);
                self.add_message(Message::new(
                    MessageRole::System,
                    &format!("🔧 {} : {}", data.name, tool_summary),
                ));
            }
            UiCommand::CompleteTool {
                id,
                result,
                success,
            } => {
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
                self.add_message(Message::new(
                    MessageRole::System,
                    &format!("⛔ Error: {msg}"),
                ));
                self.notification = Some((msg, NotificationType::Error));
            }
            UiCommand::ShowSuccess(msg) => {
                self.add_message(Message::new(MessageRole::System, &format!("✓ {msg}")));
                self.notification = Some((msg, NotificationType::Success));
            }
            UiCommand::ShowWarning(msg) => {
                self.add_message(Message::new(
                    MessageRole::System,
                    &format!("⚠ Warning: {msg}"),
                ));
                self.notification = Some((msg, NotificationType::Warning));
            }
            UiCommand::Complete => {
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
            UiCommand::ClearConversation => {
                self.messages.clear();
                self.tools.clear();
                self.thinking_content.clear();
                self.is_thinking = false;
            }
            UiCommand::NewRecord(record) => {
                self.records.push_front(record);
                while self.records.len() > super::MAX_RECORDS {
                    self.records.pop_back();
                }
                if self.selected_record >= self.records.len() && !self.records.is_empty() {
                    self.selected_record = self.records.len() - 1;
                }
            }
            UiCommand::SetAgents(agents) => {
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
            UiCommand::SetActiveAgent(agent_id) => {
                self.active_agent_id = agent_id;
                for agent in &mut self.agents {
                    agent.is_active = agent.id == self.active_agent_id;
                }
                if let Some(idx) = self.agents.iter().position(|a| a.is_active) {
                    self.selected_agent = idx;
                }
            }
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
}
