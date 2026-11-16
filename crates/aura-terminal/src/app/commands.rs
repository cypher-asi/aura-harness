//! Slash command handling and UI command processing for the App.

use super::{App, AppState, DetailView, NotificationType, PanelFocus, PendingApproval};
use crate::{
    components::{elapsed_ms, Activity, Message, MessageRole, ToolStatus, ToolStep},
    events::{CallUsage, RewindScope, UiCommand, UiEvent},
    mode_command::{PermissionMode, SlashModeCommand},
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
                self.scroll_offset = 0;
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
            "kernel" | "record" | "k" | "r" => {
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
            "name" => {
                if arg.is_empty() {
                    self.prompt_rename_project();
                } else if let Some(tx) = &self.event_tx {
                    let _ = tx.try_send(UiEvent::RenameProject(arg.to_string()));
                }
            }
            "spawn" => self.handle_spawn_command(arg),
            "plan" => self.request_mode(PermissionMode::Plan),
            "build" => self.request_build(arg),
            "login" => self.handle_login_command(arg),
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
                let event = match plan_provider(arg) {
                    Some(provider) => UiEvent::Unlink(provider.to_string()),
                    None if arg.is_empty() || arg.eq_ignore_ascii_case("zos") => UiEvent::Logout,
                    None => {
                        self.notification = Some((
                            format!("Unknown provider: {arg}. Use zos, claude or chatgpt."),
                            NotificationType::Warning,
                        ));
                        return KeyResult::continue_running();
                    }
                };
                if let Some(tx) = &self.event_tx {
                    let _ = tx.try_send(event);
                }
            }
            "graph" | "g" => self.request_graph(arg),
            "runs" => {
                if let Some(tx) = &self.event_tx {
                    let _ = tx.try_send(UiEvent::ListRuns);
                }
            }
            "whoami" | "me" => {
                if let Some(tx) = &self.event_tx {
                    let _ = tx.try_send(UiEvent::Whoami);
                }
            }
            "context" | "ctx" => {
                let report = self.hud.context_report();
                self.add_message(Message::new(MessageRole::System, &report));
            }
            "cost" | "usage" => {
                let report = self.hud.cost_report();
                self.add_message(Message::new(MessageRole::System, &report));
            }
            "todos" | "todo" => {
                if self.hud.todos.is_empty() {
                    self.todos_open = false;
                    self.notification = Some((
                        "The agent has no todo list".to_string(),
                        NotificationType::Warning,
                    ));
                } else {
                    self.todos_open = !self.todos_open;
                }
            }
            "compact" => self.handle_compact_command(arg),
            "mode" => self.handle_mode_command(arg),
            "init" => self.handle_init_command(),
            "resume" => self.handle_resume_command(arg),
            "rewind" => self.handle_rewind_command(arg),
            "diff" => self.handle_diff_command(arg),
            "new" | "n" => {
                self.messages.clear();
                self.scroll_offset = 0;
                self.streaming_content.clear();
                let jobs = std::mem::take(&mut self.hud.jobs);
                let instructions = self.hud.instructions.take();
                self.hud = super::hud::Hud {
                    jobs,
                    mode: self.hud.mode,
                    instructions,
                    ..Default::default()
                };
                self.todos_open = false;
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

    /// `/login` (zOS), `/login zos`, or `/login <plan> [paste]`.
    fn handle_login_command(&mut self, arg: &str) {
        let mut words = arg.split_whitespace();
        let target = words.next().unwrap_or("");
        let paste = words
            .next()
            .is_some_and(|w| w.eq_ignore_ascii_case("paste"));
        if target.is_empty() || target.eq_ignore_ascii_case("zos") {
            self.start_login();
        } else if let Some(provider) = plan_provider(target) {
            self.start_link(provider, paste);
        } else {
            self.notification = Some((
                format!("Unknown provider: {target}. Use zos, claude or chatgpt."),
                NotificationType::Warning,
            ));
        }
    }

    /// Ask the event loop to link a personal plan.
    fn start_link(&mut self, provider: &str, paste: bool) {
        self.state = AppState::Idle;
        self.status = format!("Linking {provider}...");
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::LinkSubscription {
                provider: provider.to_string(),
                paste,
            });
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

    /// Select `id` for the next turn. Models whose sign-in is missing
    /// start that sign-in first and are selected once it succeeds.
    pub(super) fn choose_model(&mut self, id: &str) {
        let missing = self
            .models
            .iter()
            .find(|m| m.id == id && !m.available)
            .map(|m| m.provider.clone().unwrap_or_default());
        if let Some(provider) = missing {
            self.pending_model = Some(id.to_string());
            match plan_provider(&provider) {
                Some(plan) => self.start_link(plan, false),
                None => {
                    self.start_login();
                    self.notification = Some((
                        "Cloud models need a login — enter email (Esc to stay local)".to_string(),
                        NotificationType::Warning,
                    ));
                }
            }
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
        plans: Vec<String>,
    ) {
        self.models = options;
        self.current_model = current;
        self.user = user;
        self.plans = plans;
        if self.model_cursor >= self.models.len() {
            self.model_cursor = self.models.len().saturating_sub(1);
        }
        let pending_ready = self
            .pending_model
            .as_deref()
            .is_some_and(|id| self.models.iter().any(|m| m.id == id && m.available));
        if pending_ready {
            if let Some(id) = self.pending_model.take() {
                self.choose_model(&id);
            }
        }
    }

    fn cmd_prompt_link_code(&mut self, provider: &str) {
        self.state = AppState::LinkCode;
        self.input.clear();
        self.cursor_pos = 0;
        self.status = format!("{provider} — paste the code from the sign-in page");
    }

    fn cmd_end_link(&mut self) {
        self.pending_model = None;
        if self.state == AppState::LinkCode {
            self.state = AppState::Idle;
            self.input.clear();
            self.cursor_pos = 0;
        }
    }

    /// Handle `/agent new [name]` and `/agent <number|name>`.
    fn handle_agent_command(&mut self, arg: &str) {
        let mut parts = arg.splitn(2, ' ');
        let first = parts.next().unwrap_or("");
        if first.eq_ignore_ascii_case("new") {
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

    /// Whether a turn owns the session, so the mode can't change under it.
    fn turn_running(&self) -> bool {
        matches!(
            self.state,
            AppState::Processing | AppState::AwaitingApproval
        )
    }

    /// Ask the event loop to switch the active agent to `mode`. The badge
    /// changes once the switch is recorded and [`UiCommand::SetMode`] comes
    /// back.
    pub(super) fn request_mode(&mut self, mode: PermissionMode) {
        if self.hud.mode == Some(mode) {
            self.notification =
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
    fn request_build(&mut self, arg: &str) {
        if self.turn_running() {
            self.notification = Some((
                "Wait for the current turn to finish before building".to_string(),
                NotificationType::Warning,
            ));
            return;
        }
        let path = (!arg.is_empty()).then(|| arg.to_string());
        if path.is_none() && self.active_plan.is_none() {
            self.notification = Some((
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

    /// Handle `/spawn <permanent agent>`.
    fn handle_spawn_command(&mut self, arg: &str) {
        let name = self
            .definitions
            .iter()
            .find(|d| d.name.eq_ignore_ascii_case(arg))
            .map(|d| d.name.clone());
        match name {
            Some(name) => self.instantiate_definition(&name),
            None => {
                let available: Vec<&str> =
                    self.definitions.iter().map(|d| d.name.as_str()).collect();
                self.notification = Some((
                    format!("Usage: /spawn <permanent agent> ({})", available.join(", ")),
                    NotificationType::Warning,
                ));
            }
        }
    }

    /// `/resume [n]`: list the agent's earlier sessions, or resume number `n`.
    fn handle_resume_command(&mut self, arg: &str) {
        let index = if arg.is_empty() {
            None
        } else if let Some(n) = arg.parse::<usize>().ok().filter(|n| *n > 0) {
            Some(n)
        } else {
            self.notification = Some((
                "Usage: /resume [n] (n from the list /resume shows)".to_string(),
                NotificationType::Warning,
            ));
            return;
        };
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::Resume {
                agent_id: self.active_agent_id.clone(),
                index,
            });
        }
    }

    /// `/mode`: show the agent's permission mode and the others; `/mode
    /// <name>`: switch to one for the agent's next tool calls.
    fn handle_mode_command(&mut self, arg: &str) {
        if arg.is_empty() {
            let current = self.hud.mode.unwrap_or_default();
            let mut report = format!("Mode: {current} — {}", current.description());
            for mode in [
                PermissionMode::Default,
                PermissionMode::AcceptEdits,
                PermissionMode::Plan,
                PermissionMode::Unrestricted,
                PermissionMode::Ask,
                PermissionMode::Debug,
            ] {
                report.push_str(&format!("\n  {:<14}{}", mode.name(), mode.description()));
            }
            report.push_str("\nShift+Tab cycles default → accept-edits → plan → unrestricted.");
            self.add_message(Message::new(MessageRole::System, &report));
            return;
        }
        match SlashModeCommand::parse(arg) {
            Ok(command) => self.request_mode(command.requested),
            Err(e) => {
                self.notification = Some((e.to_string(), NotificationType::Warning));
            }
        }
    }

    /// `/rewind [n [code|chat|both]]`: list the agent's turns, or put its
    /// files and/or conversation back to before turn `n`.
    fn handle_rewind_command(&mut self, arg: &str) {
        let mut words = arg.split_whitespace();
        let turn = words
            .next()
            .map(|w| w.parse::<usize>().ok().filter(|n| *n > 0));
        let scope = words.next().map(str::parse::<RewindScope>);
        let (turn, scope) = match (turn, scope, words.next()) {
            (None, None, None) => (None, RewindScope::default()),
            (Some(Some(n)), None, None) => (Some(n), RewindScope::default()),
            (Some(Some(n)), Some(Ok(scope)), None) => (Some(n), scope),
            _ => {
                self.notification = Some((
                    "Usage: /rewind [n] [code|chat|both] (n from the list /rewind shows)"
                        .to_string(),
                    NotificationType::Warning,
                ));
                return;
            }
        };
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::Rewind {
                agent_id: self.active_agent_id.clone(),
                turn,
                scope,
            });
        }
    }

    /// `/diff [session]`: what the agent's latest turn, or its whole
    /// session, changed in files.
    fn handle_diff_command(&mut self, arg: &str) {
        let session = match arg.to_ascii_lowercase().as_str() {
            "" | "turn" | "last" => false,
            "session" | "all" => true,
            _ => {
                self.notification = Some((
                    "Usage: /diff [session]".to_string(),
                    NotificationType::Warning,
                ));
                return;
            }
        };
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::Diff {
                agent_id: self.active_agent_id.clone(),
                session,
            });
        }
    }

    /// `/compact [focus]`: summarize the agent's older history now. A busy
    /// agent does it once its current work ends.
    fn handle_compact_command(&mut self, focus: &str) {
        let Some(tx) = &self.event_tx else {
            return;
        };
        let event = UiEvent::Compact {
            agent_id: self.active_agent_id.clone(),
            focus: (!focus.is_empty()).then(|| focus.to_string()),
        };
        if tx.try_send(event).is_err() {
            return;
        }
        if self.agent_busy(&self.active_agent_id) {
            self.notification = Some((
                "Queued: the history is summarized when the current work ends".to_string(),
                NotificationType::Success,
            ));
        } else {
            self.state = AppState::Processing;
            self.status = "Summarizing history...".to_string();
        }
    }

    /// `/init`: have the agent study the project and write or improve its
    /// `AGENTS.md`. The chat shows the request once the turn starts; a busy
    /// agent starts it when its current work ends.
    fn handle_init_command(&mut self) {
        let Some(tx) = &self.event_tx else {
            return;
        };
        let event = UiEvent::InitProject {
            agent_id: self.active_agent_id.clone(),
        };
        if tx.try_send(event).is_err() {
            return;
        }
        if self.agent_busy(&self.active_agent_id) {
            self.notification = Some((
                "Queued: AGENTS.md is written when the current work ends".to_string(),
                NotificationType::Success,
            ));
        } else {
            self.state = AppState::Processing;
            self.status = "Studying the project...".to_string();
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
            UiCommand::SetReplyModel { model, usage } => self.cmd_set_reply_model(&model, usage),
            UiCommand::StartThinking => self.add_activity(Activity::thinking()),
            UiCommand::AppendThinking(thinking) => self.cmd_append_thinking(&thinking),
            UiCommand::FinishThinking => self.cmd_finish_thinking(),
            UiCommand::ShowMessage(data) => self.cmd_show_message(&data),
            UiCommand::ShowTool(data) => {
                self.add_activity(Activity::tool(&data.id, &data.name, &data.args));
            }
            UiCommand::UpdateToolArgs { id, args } => {
                self.update_tool(&id, |tool| tool.args = args);
            }
            UiCommand::ToolPolicy { id, tool, decision } => {
                self.cmd_tool_policy(&id, &tool, decision);
            }
            UiCommand::ToolRetrying {
                id,
                attempt,
                max_attempts,
            } => {
                self.update_tool(&id, |tool| tool.retry = Some((attempt, max_attempts)));
            }
            UiCommand::CompleteTool {
                id,
                result,
                success,
            } => {
                self.cmd_complete_tool(&id, &result, success);
                self.refresh_sandbox();
            }
            UiCommand::ToolDiff { id, diff } => {
                self.update_tool(&id, |tool| tool.diff = Some(diff));
            }
            UiCommand::ShowDiff { title, files } => {
                self.add_message(Message::diff(&title, files));
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
                always,
            } => {
                if self.pending_approvals.is_empty() {
                    self.status = format!("Waiting for approval to run {tool}");
                }
                if !self.pending_approvals.iter().any(|a| a.id == id) {
                    self.mark_awaiting_approval(&id, true);
                    self.pending_approvals.push_back(PendingApproval {
                        id,
                        tool,
                        description,
                        always,
                    });
                }
                self.state = AppState::AwaitingApproval;
            }
            UiCommand::CloseApproval(id) => self.cmd_close_approval(&id),
            UiCommand::SetMode(mode) => self.hud.mode = Some(mode),
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
            UiCommand::TurnStarted { prompt } => self.cmd_turn_started(prompt),
            UiCommand::UserSteered(text) => self.cmd_user_steered(&text),
            UiCommand::CallStats(call) => self.hud.stats.add(&call),
            UiCommand::Compacted(text) => {
                self.hud.stats.compactions = self.hud.stats.compactions.saturating_add(1);
                self.add_message(Message::new(MessageRole::System, &text));
            }
            UiCommand::SetTodos(todos) => {
                if todos.is_empty() {
                    self.todos_open = false;
                }
                self.hud.todos = todos;
            }
            UiCommand::SetJobs(jobs) => self.hud.jobs = jobs,
            UiCommand::SetQueued(queued) => self.hud.queued = queued,
            UiCommand::SetSessionTotals(totals) => self.hud.stats.restore(&totals),
            UiCommand::SetInstructions(files) => self.hud.instructions = Some(files),
            UiCommand::SetContext(context) => {
                if context.limit_tokens.is_some() {
                    self.hud.stats.context_limit = context.limit_tokens;
                }
                self.hud.context = Some(context);
            }
            UiCommand::ForAgent { agent_id, command } => {
                self.process_agent_command(&agent_id, *command);
            }
            UiCommand::ClearConversation => self.cmd_clear_conversation(),
            UiCommand::NewRecord(record) => self.cmd_new_record(record),
            UiCommand::SetAgents(agents) => self.cmd_set_agents(agents),
            UiCommand::SetActiveAgent(agent_id) => self.cmd_set_active_agent(&agent_id),
            UiCommand::SetAgentDefinitions(definitions) => self.set_definitions(definitions),
            UiCommand::OpenEditor(path) => self.editor_request = Some(path),
            UiCommand::ClearRecords => {
                self.records.clear();
                self.selected_record = 0;
                if self.detail == Some(DetailView::Record) {
                    self.close_detail();
                }
            }
            UiCommand::LoadConversation(history) => self.cmd_load_conversation(history),
            UiCommand::SetSandboxRoot(root) => self.sandbox.set_root(root),
            UiCommand::SetProcesses(processes) => self.set_processes(processes),
            UiCommand::GraphLoad { run_id, nodes } => self.load_graph(run_id, nodes),
            UiCommand::GraphUpsert { run_id, nodes } => {
                self.trace_feed.push(&run_id, &nodes);
                self.upsert_graph(&run_id, nodes);
            }
            UiCommand::SetApiStatus { url, active } => {
                self.api_url = url;
                self.api_active = active;
            }
            UiCommand::SetModels {
                options,
                current,
                user,
                plans,
            } => self.cmd_set_models(options, current, user, plans),
            UiCommand::PromptLinkCode { provider } => self.cmd_prompt_link_code(&provider),
            UiCommand::EndLink => self.cmd_end_link(),
            UiCommand::SetPlan(plan) => self.active_plan = plan,
        }
    }

    fn cmd_start_streaming(&mut self) {
        self.streaming_content.clear();
        if self
            .messages
            .back()
            .is_some_and(|m| m.is_streaming() && m.content().is_empty())
        {
            return;
        }
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

    fn cmd_set_reply_model(&mut self, model: &str, usage: Option<CallUsage>) {
        let replies = self
            .messages
            .iter_mut()
            .rev()
            .take_while(|m| m.role() != MessageRole::User)
            .filter(|m| {
                m.role() == MessageRole::Assistant
                    && m.activity_step().is_none()
                    && m.model().is_none()
            });
        for message in replies {
            message.set_model(model);
            message.set_usage(usage);
        }
    }

    fn cmd_show_message(&mut self, data: &crate::events::MessageData) {
        let mut msg = Message::new(data.role, &data.content);
        if data.is_streaming {
            msg.set_streaming(true);
        }
        if let Some(model) = &data.model {
            msg.set_model(model);
        }
        msg.set_usage(data.usage);
        self.add_message(msg);
    }

    fn add_activity(&mut self, activity: Activity) {
        self.add_message(Message::activity(activity, self.activity_expanded));
    }

    /// The last thinking block still streaming.
    fn live_thinking(&mut self) -> Option<&mut Message> {
        self.messages
            .iter_mut()
            .rev()
            .find(|m| matches!(m.activity_step(), Some(Activity::Thinking(step)) if step.running))
    }

    fn cmd_append_thinking(&mut self, text: &str) {
        if self.live_thinking().is_none() {
            self.add_activity(Activity::thinking());
        }
        if let Some(message) = self.live_thinking() {
            message.update_activity(|activity| {
                if let Activity::Thinking(step) = activity {
                    step.text.push_str(text);
                }
            });
        }
    }

    fn cmd_finish_thinking(&mut self) {
        if let Some(message) = self.live_thinking() {
            message.update_activity(|activity| activity.finish(""));
        }
    }

    /// Apply `update` to the tool call with use id `id`; `false` when no
    /// such call is shown.
    fn update_tool(&mut self, id: &str, update: impl FnOnce(&mut ToolStep)) -> bool {
        let found = self
            .messages
            .iter_mut()
            .rev()
            .find(|m| matches!(m.activity_step(), Some(Activity::Tool(tool)) if tool.id == id));
        let Some(message) = found else {
            return false;
        };
        message.update_activity(|activity| {
            if let Activity::Tool(tool) = activity {
                update(tool);
            }
        });
        true
    }

    /// Keep the policy decision with its tool call, so a run of calls
    /// stays one block; a call that isn't shown gets a `policy:` line.
    fn cmd_tool_policy(&mut self, id: &str, tool: &str, decision: String) {
        let line = format!("policy: {tool} {decision}");
        if !self.update_tool(id, |step| step.policy = Some(decision)) {
            self.add_message(Message::new(MessageRole::System, &line));
        }
    }

    fn cmd_complete_tool(&mut self, id: &str, result: &str, success: bool) {
        self.update_tool(id, |tool| {
            tool.result = Some(super::format::format_tool_result(&tool.name, result));
            tool.status = if success {
                ToolStatus::Success
            } else {
                ToolStatus::Error
            };
            tool.retry = None;
            tool.duration_ms = elapsed_ms(tool.started);
        });
    }

    /// End every activity block still in progress, e.g. when the turn ends.
    fn finish_live_activity(&mut self) {
        for message in &mut self.messages {
            if message.is_live_activity() {
                message.update_activity(|activity| activity.finish("interrupted"));
            }
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
        self.finish_live_activity();
        self.state = AppState::Idle;
        self.status = "Ready".to_string();
    }

    /// Drop the prompt for approval `id`, which stopped waiting; the turn
    /// goes on without it.
    fn cmd_close_approval(&mut self, id: &str) {
        let before = self.pending_approvals.len();
        self.pending_approvals.retain(|a| a.id != id);
        if self.pending_approvals.len() < before {
            self.mark_awaiting_approval(id, false);
            self.after_approval_closed();
        }
    }

    /// Mark the tool call behind approval `request_id` as waiting for the
    /// user, or no longer waiting. The kernel's request id is
    /// `<agent hex>:<tool use id>`.
    pub(super) fn mark_awaiting_approval(&mut self, request_id: &str, waiting: bool) {
        let tool_use_id = request_id
            .split_once(':')
            .map_or(request_id, |(_, tool_use_id)| tool_use_id);
        self.update_tool(tool_use_id, |tool| tool.awaiting_approval = waiting);
    }

    /// Show the next waiting prompt, or go back to the turn when none is
    /// left.
    pub(super) fn after_approval_closed(&mut self) {
        if let Some(next) = self.pending_approvals.front() {
            self.status = format!("Waiting for approval to run {}", next.tool);
        } else if self.state == AppState::AwaitingApproval {
            self.state = AppState::Processing;
        }
    }

    fn cmd_turn_started(&mut self, prompt: Option<String>) {
        if let Some(prompt) = prompt {
            self.add_message(Message::new(MessageRole::User, &prompt));
        }
        // A turn the user did not start (a background completion notice)
        // must not hide a login prompt or an approval on screen.
        if self.state == AppState::Idle {
            self.state = AppState::Processing;
            self.status = "Thinking...".to_string();
        }
    }

    /// Show a message sent into the running turn. A reply still streaming
    /// is split around it, so the rest of the reply keeps streaming below.
    fn cmd_user_steered(&mut self, text: &str) {
        let was_streaming = self.messages.back().is_some_and(Message::is_streaming);
        if was_streaming {
            self.cmd_finish_streaming();
        }
        self.add_message(Message::new(MessageRole::User, text));
        if was_streaming {
            self.cmd_start_streaming();
        }
    }

    fn cmd_clear_conversation(&mut self) {
        self.messages.clear();
    }

    fn cmd_load_conversation(&mut self, history: Vec<crate::events::MessageData>) {
        self.cmd_clear_conversation();
        self.streaming_content.clear();
        for data in history {
            let mut msg = if let Some(activity) = &data.activity {
                Message::activity(Activity::from(activity), self.activity_expanded)
            } else if let Some(command) = data.command.as_deref() {
                let mut msg = Message::shell(command);
                msg.finish_shell("", None);
                msg
            } else {
                Message::new(data.role, &data.content)
            };
            if let Some(ts) = data
                .timestamp_ms
                .and_then(|ms| i64::try_from(ms).ok())
                .and_then(chrono::DateTime::from_timestamp_millis)
            {
                msg = msg.with_timestamp(ts);
            }
            if let Some(model) = &data.model {
                msg.set_model(model);
            }
            msg.set_usage(data.usage);
            self.messages.push_back(msg);
        }
        while self.messages.len() > super::MAX_MESSAGES {
            self.messages.pop_front();
        }
        self.scroll_offset = 0;
    }

    /// Append a record, oldest first. A selection on the newest record
    /// follows new arrivals; one moved up stays on its record.
    fn cmd_new_record(&mut self, record: crate::events::RecordSummary) {
        // Agents running in the background write records too; the panel
        // lists the active agent's.
        if !record.agent_id.is_empty()
            && !self.active_agent_id.is_empty()
            && record.agent_id != self.active_agent_id
        {
            return;
        }
        let follow = self.detail != Some(DetailView::Record)
            && self.selected_record + 1 >= self.records.len();
        self.records.push_back(record);
        while self.records.len() > super::MAX_RECORDS {
            self.records.pop_front();
            self.selected_record = self.selected_record.saturating_sub(1);
        }
        if follow {
            self.selected_record = self.records.len() - 1;
        }
    }

    fn cmd_set_agents(&mut self, agents: Vec<crate::events::AgentSummary>) {
        self.agents = agents;
        self.collapsed_projects
            .retain(|id| self.agents.iter().any(|agent| agent.id == *id));
        self.select_visible_active();
    }

    fn cmd_set_active_agent(&mut self, agent_id: &str) {
        self.show_agent_chat(agent_id);
        for agent in &mut self.agents {
            agent.is_active = agent.id == self.active_agent_id;
        }
        self.agents_scroll_detached = false;
        self.select_visible_active();
    }
}

/// Canonical plan id for `/login` and `/logout` arguments.
fn plan_provider(arg: &str) -> Option<&'static str> {
    match arg.trim().to_ascii_lowercase().as_str() {
        "claude" | "anthropic" => Some("claude"),
        "chatgpt" | "openai" => Some("chatgpt"),
        _ => None,
    }
}
