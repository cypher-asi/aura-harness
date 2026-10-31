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
        let arg = parts.get(1).map_or("", |a| a.trim());

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
                if let Some(tx) = &self.event_tx {
                    let _ = tx.try_send(UiEvent::Clear);
                }
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
            "records" | "record" | "r" => {
                self.record_panel_visible = !self.record_panel_visible;
                if !self.record_panel_visible && self.focus == PanelFocus::Records {
                    self.focus = PanelFocus::Chat;
                }
            }
            "agents" | "swarm" | "sw" => {
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
            "files" | "sandbox" | "f" => {
                self.sandbox_panel_visible = !self.sandbox_panel_visible;
                if self.sandbox_panel_visible {
                    self.sandbox.refresh();
                } else if self.focus == PanelFocus::Sandbox {
                    self.focus = PanelFocus::Chat;
                }
            }
            "agent" | "a" => self.handle_agent_command(arg),
            "login" => self.start_login(),
            "model" | "models" | "m" => {
                if arg.is_empty() {
                    self.open_model_selector();
                } else {
                    let id = self
                        .models
                        .iter()
                        .find(|m| {
                            m.id.eq_ignore_ascii_case(arg) || m.label.eq_ignore_ascii_case(arg)
                        })
                        .map_or_else(|| arg.to_string(), |m| m.id.clone());
                    self.choose_model(&id);
                }
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

    /// Re-read the sandbox tree when the panel is on screen, so files the
    /// agent writes show up.
    fn refresh_sandbox(&mut self) {
        if self.sandbox_panel_visible {
            self.sandbox.refresh();
        }
    }

    /// Enter the email/password login flow.
    pub(super) fn start_login(&mut self) {
        self.state = AppState::LoginEmail;
        self.login_email.clear();
        self.input.clear();
        self.cursor_pos = 0;
        self.status = "Login — enter email".to_string();
    }

    /// Open the model selector with the current model highlighted.
    pub(super) fn open_model_selector(&mut self) {
        if self.models.is_empty() {
            self.notification = Some((
                "Model list not loaded yet".to_string(),
                NotificationType::Warning,
            ));
            return;
        }
        self.model_cursor = self
            .models
            .iter()
            .position(|m| m.id == self.current_model)
            .unwrap_or(0);
        self.state = AppState::SelectingModel;
    }

    /// Select `id` for the next turn. Cloud models start the login flow
    /// first when browsing as guest.
    pub(super) fn choose_model(&mut self, id: &str) {
        let needs_login = self
            .models
            .iter()
            .find(|m| m.id == id)
            .is_some_and(|m| !m.local)
            && self.user.is_none();
        if needs_login {
            self.pending_model = Some(id.to_string());
            self.start_login();
            self.notification = Some((
                "Cloud models need a login — enter email (Esc to stay local)".to_string(),
                NotificationType::Warning,
            ));
            return;
        }
        self.state = AppState::Idle;
        if id != self.current_model {
            if let Some(tx) = &self.event_tx {
                let _ = tx.try_send(UiEvent::SelectModel(id.to_string()));
            }
        }
    }

    fn cmd_set_models(
        &mut self,
        options: Vec<crate::events::ModelOptionView>,
        current: String,
        user: Option<String>,
    ) {
        self.models = options;
        self.current_model = current;
        self.user = user;
        if self.model_cursor >= self.models.len() {
            self.model_cursor = self.models.len().saturating_sub(1);
        }
        if self.user.is_some() {
            if let Some(id) = self.pending_model.take() {
                self.choose_model(&id);
            }
        }
    }

    /// Handle `/agent new [name]` and `/agent <number|name>`.
    fn handle_agent_command(&mut self, arg: &str) {
        let mut parts = arg.splitn(2, ' ');
        let first = parts.next().unwrap_or("");
        if first.eq_ignore_ascii_case("new") {
            if matches!(
                self.state,
                AppState::Processing | AppState::AwaitingApproval
            ) {
                self.notification = Some((
                    "Wait for the current turn to finish before creating an agent".to_string(),
                    NotificationType::Warning,
                ));
                return;
            }
            let name = parts
                .next()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(str::to_string);
            if let Some(tx) = &self.event_tx {
                let _ = tx.try_send(UiEvent::CreateAgent(name));
            }
            return;
        }
        if arg.is_empty() {
            self.notification = Some((
                "Usage: /agent new [name] | /agent <number|name>".to_string(),
                NotificationType::Warning,
            ));
            return;
        }
        let target = arg
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|idx| self.agents.get(idx))
            .or_else(|| {
                self.agents
                    .iter()
                    .find(|a| a.name.eq_ignore_ascii_case(arg))
            })
            .map(|a| a.id.clone());
        match target {
            Some(agent_id) => self.request_agent_switch(agent_id),
            None => {
                self.notification = Some((
                    format!("No agent matching '{arg}'"),
                    NotificationType::Warning,
                ));
            }
        }
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
            UiCommand::ShowMessage(data) => self.cmd_show_message(&data),
            UiCommand::ShowTool(data) => self.cmd_show_tool(&data),
            UiCommand::CompleteTool {
                id,
                result,
                success,
            } => {
                self.cmd_complete_tool(&id, &result, success);
                self.refresh_sandbox();
            }
            UiCommand::CommandFinished { output, exit_code } => {
                if let Some(message) = self
                    .messages
                    .iter_mut()
                    .rev()
                    .find(|m| m.is_running_shell())
                {
                    message.finish_shell(&output, exit_code);
                }
                self.refresh_sandbox();
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
                self.cmd_notification(&msg, "error: ", NotificationType::Error);
            }
            UiCommand::ShowSuccess(msg) => {
                self.cmd_notification(&msg, "✓ ", NotificationType::Success);
            }
            UiCommand::ShowWarning(msg) => {
                self.cmd_notification(&msg, "warning: ", NotificationType::Warning);
            }
            UiCommand::Complete => {
                self.cmd_complete();
                self.refresh_sandbox();
            }
            UiCommand::ClearConversation => self.cmd_clear_conversation(),
            UiCommand::NewRecord(record) => self.cmd_new_record(record),
            UiCommand::SetAgents(agents) => self.cmd_set_agents(agents),
            UiCommand::SetActiveAgent(agent_id) => self.cmd_set_active_agent(agent_id),
            UiCommand::ClearRecords => {
                self.records.clear();
                self.selected_record = 0;
                self.showing_record_detail = false;
            }
            UiCommand::LoadConversation(history) => self.cmd_load_conversation(history),
            UiCommand::SetSandboxRoot(root) => self.sandbox.set_root(root),
            UiCommand::SetApiStatus { url, active } => {
                self.api_url = url;
                self.api_active = active;
            }
            UiCommand::SetModels {
                options,
                current,
                user,
            } => self.cmd_set_models(options, current, user),
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

    fn cmd_show_message(&mut self, data: &crate::events::MessageData) {
        let mut msg = Message::new(data.role, &data.content);
        if data.is_streaming {
            msg.set_streaming(true);
        }
        self.add_message(msg);
    }

    fn cmd_show_tool(&mut self, data: &crate::events::ToolData) {
        let tool = ToolCard::new(&data.id, &data.name).with_args(&data.args);
        self.add_tool(tool);
        let tool_summary = super::format::format_tool_summary(&data.name, &data.args);
        self.add_message(Message::new(
            MessageRole::System,
            &format!("$ {} {}", data.name, tool_summary),
        ));
    }

    fn cmd_complete_tool(&mut self, id: &str, result: &str, success: bool) {
        let tool_name = self
            .tools
            .iter()
            .find(|tool| tool.id() == id)
            .map(|tool| tool.name().to_string())
            .unwrap_or_default();
        let result = super::format::format_tool_result(&tool_name, result);
        for tool in &mut self.tools {
            if tool.id() == id {
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
                ("✗", "error: ")
            };
            let display_result = if result.chars().count() > 200 {
                let head: String = result.chars().take(197).collect();
                format!("{head}...")
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

    fn cmd_load_conversation(&mut self, history: Vec<crate::events::MessageData>) {
        self.cmd_clear_conversation();
        self.streaming_content.clear();
        for data in history {
            let mut msg = data.command.as_deref().map_or_else(
                || Message::new(data.role, &data.content),
                |command| {
                    let mut msg = Message::shell(&self.shell_prompt, command);
                    msg.finish_shell("", None);
                    msg
                },
            );
            if let Some(ts) = data
                .timestamp_ms
                .and_then(|ms| i64::try_from(ms).ok())
                .and_then(chrono::DateTime::from_timestamp_millis)
            {
                msg = msg.with_timestamp(ts);
            }
            self.messages.push_back(msg);
        }
        while self.messages.len() > super::MAX_MESSAGES {
            self.messages.pop_front();
        }
        self.scroll_offset = 0;
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
        self.selected_agent = self.selected_agent.min(self.agents.len());
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
