//! Slash commands: what `/name args` typed in the input does.

use super::{App, AppState, NotificationType, PanelFocus};
use crate::{
    components::{Message, MessageRole},
    events::{RewindScope, UiEvent},
    mode_command::{parse_approval_command, ApprovalLevel, PermissionMode, SlashModeCommand},
    terminal::KeyResult,
};

/// The word that sends a message in plan mode, wherever it is typed.
const PLAN_WORD: &str = "/plan";

/// Byte ranges of every `/plan` word in `text` (any case): bounded by
/// whitespace or the ends of the text, so `/planner` and `src/plan` are
/// not it. None when `text` is another slash command (`/build x /plan`),
/// which keeps its own meaning. The input draws them in the accent so it
/// is clear the message goes in plan mode.
pub(crate) fn plan_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let first_word = text.split(char::is_whitespace).next().unwrap_or("");
    if first_word.starts_with('/') && !first_word.eq_ignore_ascii_case(PLAN_WORD) {
        return Vec::new();
    }
    text.char_indices()
        .filter(|&(i, c)| {
            c == '/'
                && text[..i]
                    .chars()
                    .next_back()
                    .is_none_or(char::is_whitespace)
                && text
                    .get(i..i + PLAN_WORD.len())
                    .is_some_and(|word| word.eq_ignore_ascii_case(PLAN_WORD))
                && text[i + PLAN_WORD.len()..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace)
        })
        .map(|(i, _)| i..i + PLAN_WORD.len())
        .collect()
}

/// `text` without its `/plan` words, trimmed, when it has any: the
/// message to send in plan mode, or empty for a bare `/plan` (the
/// toggle). `None` when there is no `/plan` word.
pub(crate) fn strip_plan(text: &str) -> Option<String> {
    let ranges = plan_ranges(text);
    if ranges.is_empty() {
        return None;
    }
    let mut message = String::with_capacity(text.len());
    let mut last = 0;
    for range in ranges {
        message.push_str(&text[last..range.start]);
        last = range.end;
        // Drop the space after the word too, so "fix /plan it" reads
        // "fix it" rather than "fix  it".
        if text[last..].starts_with(' ')
            && message.chars().next_back().is_none_or(char::is_whitespace)
        {
            last += 1;
        }
    }
    message.push_str(&text[last..]);
    Some(message.trim().to_string())
}

impl App {
    /// Handle a slash command.
    pub(super) fn handle_command(&mut self, text: &str) -> KeyResult {
        // A line break or tab ends the name as a space does, so a recalled
        // or pasted "/model\n" is still `/model`.
        let body = text.strip_prefix('/').unwrap_or(text).trim();
        let (cmd, arg) = body.split_once(char::is_whitespace).unwrap_or((body, ""));
        let cmd = cmd.to_lowercase();
        let arg = arg.trim();

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
            "kernel" | "record" | "k" | "r" if self.agent_panels_hidden() => {
                self.notify_no_agent_panels();
            }
            "kernel" | "record" | "k" | "r" => {
                if self.side_maximized() {
                    // Hidden under the side panel, it comes back.
                    self.panels.side_expanded = false;
                    self.panels.record_visible = true;
                } else {
                    self.panels.record_visible = !self.panels.record_visible;
                }
                if !self.panels.record_visible && self.focus == PanelFocus::Records {
                    self.focus = PanelFocus::Chat;
                }
            }
            "cloud" | "local" | "host" => self.handle_cloud_command(&cmd, arg),
            "members" | "team" => self.show_members(),
            "policies" | "policy" | "perms" | "permissions" => self.show_policies(),
            "inbox" | "mail" => self.show_inbox(),
            "agents" | "swarm" | "sw" => {
                self.panels.swarm_visible = !self.panels.swarm_visible;
                if !self.panels.swarm_visible && self.focus == PanelFocus::Swarm {
                    self.focus = PanelFocus::Chat;
                }
                if self.panels.swarm_visible {
                    if let Some(tx) = &self.event_tx {
                        let _ = tx.try_send(UiEvent::RefreshAgents);
                    }
                }
            }
            "files" | "sandbox" | "f" if self.agent_panels_hidden() => {
                self.notify_no_agent_panels();
            }
            "files" | "sandbox" | "f" => {
                self.panels.sandbox_visible = !self.panels.sandbox_visible;
                if self.panels.sandbox_visible {
                    self.sandbox.refresh();
                } else {
                    // The Record panel gets the column back, and the
                    // panel comes back to its own slot next time.
                    self.panels.side_expanded = false;
                    if self.focus == PanelFocus::Sandbox {
                        self.focus = PanelFocus::Chat;
                    }
                }
            }
            "expand" | "zoom" | "fullscreen" => self.toggle_chat_expanded(),
            "agent" | "a" => self.handle_agent_command(arg),
            "project" => self.show_active_project_info(),
            "spawn" => self.handle_spawn_command(arg),
            "archive" => {
                let id = self.active_agent_id.clone();
                self.archive_agent(id);
            }
            "unarchive" => self.unarchive_active_agent(),
            "remove-project" => {
                let id = self.active_agent_id.clone();
                self.request_remove_project_of(&id);
            }
            "plan" => self.request_plan_toggle(),
            "build" => self.request_build(arg),
            "login" => self.handle_login_command(arg),
            "model" | "models" | "m" => {
                if arg.is_empty() {
                    self.open_model_selector();
                } else {
                    // An exact id or label maps to its id here; any other
                    // name (`opus`, `deepseek`) goes as typed and the event
                    // loop resolves it as an alias.
                    let id = super::model_picker::exact_match(&self.models, arg)
                        .map_or_else(|| arg.to_string(), |m| m.id.clone());
                    self.choose_model(&id);
                }
            }
            "thinking" | "think" | "effort" => self.handle_thinking_command(arg),
            "logout" => {
                let event = match plan_provider(arg) {
                    Some(provider) => UiEvent::Unlink(provider.to_string()),
                    None if arg.is_empty() || arg.eq_ignore_ascii_case("zos") => UiEvent::Logout,
                    None => {
                        self.notifications.current = Some((
                            format!(
                                "Unknown provider: {arg}. Use zos, claude, chatgpt or openrouter."
                            ),
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
                let mut report = self.hud.cost_report();
                report.push_str(&super::hud::sources_report(self.turn_sources()));
                self.add_message(Message::new(MessageRole::System, &report));
            }
            "todos" | "todo" => {
                if self.hud.todos.is_empty() {
                    self.todos_open = false;
                    self.notifications.current = Some((
                        "The agent has no todo list".to_string(),
                        NotificationType::Warning,
                    ));
                } else {
                    self.todos_open = !self.todos_open;
                }
            }
            "compact" => self.handle_compact_command(arg),
            "mode" => self.handle_mode_command(arg),
            "approval" | "approvals" => self.handle_approval_command(arg),
            "init" => self.handle_init_command(),
            "resume" => self.handle_resume_command(arg),
            "rewind" => self.handle_rewind_command(arg),
            "diff" => self.handle_diff_command(arg),
            "gate" => {
                if let Some(tx) = &self.event_tx {
                    let _ = tx.try_send(UiEvent::ShowGate);
                }
            }
            "compare" => {
                if let Some(tx) = &self.event_tx {
                    let _ = tx.try_send(UiEvent::Compare {
                        agent_id: self.active_agent_id.clone(),
                        args: arg.to_string(),
                    });
                }
            }
            "log" | "logs" => self.show_chat_tab(super::ChatTab::Log),
            "bench" | "benchmarks" => {
                self.show_agents_tab(super::AgentsTab::Bench);
                if arg.trim() == "new" {
                    self.show_bench(super::BenchMode::Form);
                    self.focus = PanelFocus::Chat;
                }
            }
            "new" | "n" => {
                self.messages.clear();
                self.scroll_offset = 0;
                self.streaming_content.clear();
                let jobs = std::mem::take(&mut self.hud.jobs);
                let instructions = self.hud.instructions.take();
                self.hud = super::hud::Hud {
                    jobs,
                    mode: self.hud.mode,
                    approval: self.hud.approval,
                    instructions,
                    ..Default::default()
                };
                self.todos_open = false;
                // The new conversation is a new run, which no agent has
                // joined yet.
                self.start_members_run();
                if let Some(tx) = &self.event_tx {
                    let _ = tx.try_send(UiEvent::NewSession);
                }
                self.notifications.current =
                    Some(("New session started".to_string(), NotificationType::Success));
            }
            _ => {
                self.notifications.current = Some((
                    format!("Unknown command: /{cmd}. Type /help for available commands."),
                    NotificationType::Warning,
                ));
            }
        }
        KeyResult::continue_running()
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
            self.notifications.current = Some((
                format!("Unknown provider: {target}. Use zos, claude, chatgpt or openrouter."),
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
        self.input.text.clear();
        self.input.cursor_pos = 0;
        self.status = "Login — enter email".to_string();
    }

    /// Open the model selector with the current model highlighted.
    pub(super) fn open_model_selector(&mut self) {
        if self.models.is_empty() {
            self.notifications.current = Some((
                "Model list not loaded yet".to_string(),
                NotificationType::Warning,
            ));
            return;
        }
        self.model_picker.open(&self.models, &self.current_model);
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
                    self.notifications.current = Some((
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

    /// `/thinking`: show the thinking level and the others;
    /// `/thinking <level>`: pin it for every turn, or `auto` to let each
    /// model pick its own.
    fn handle_thinking_command(&mut self, arg: &str) {
        if arg.is_empty() {
            let current = self.thinking.as_deref().unwrap_or("auto");
            let mut report = format!("Thinking: {current}");
            for (level, description) in THINKING_LEVELS {
                report.push_str(&format!("\n  {level:<10}{description}"));
            }
            report.push_str("\nAlt+T cycles through them.");
            self.add_message(Message::new(MessageRole::System, &report));
            return;
        }
        let Some((level, _)) = THINKING_LEVELS
            .iter()
            .find(|(level, _)| level.eq_ignore_ascii_case(arg))
        else {
            let names: Vec<&str> = THINKING_LEVELS.iter().map(|(level, _)| *level).collect();
            self.notifications.current = Some((
                format!("Unknown thinking level: {arg}. Use {}.", names.join(", ")),
                NotificationType::Warning,
            ));
            return;
        };
        let level = (*level != "auto").then(|| (*level).to_string());
        if level == self.thinking {
            self.notifications.current = Some((
                format!("Thinking is already {}", level.as_deref().unwrap_or("auto")),
                NotificationType::Warning,
            ));
            return;
        }
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::SetThinking(level));
        }
    }

    /// Ask for the thinking level after the current one (Alt+T).
    pub(super) fn cycle_thinking(&mut self) {
        let current = self.thinking.as_deref().unwrap_or("auto");
        let at = THINKING_LEVELS
            .iter()
            .position(|(level, _)| *level == current)
            .unwrap_or(0);
        let (next, _) = THINKING_LEVELS[(at + 1) % THINKING_LEVELS.len()];
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::SetThinking(
                (next != "auto").then(|| next.to_string()),
            ));
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
            self.notifications.current = Some((
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
                self.notifications.current = Some((
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

    /// `/plan`: enter plan mode, or leave it when the agent is already there.
    /// The event loop knows the mode from before plan mode, so the same
    /// command restores it.
    pub(super) fn request_plan_toggle(&mut self) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.try_send(UiEvent::TogglePlan(self.active_agent_id.clone()));
        }
    }

    /// `/plan <message>`: put the active agent in plan mode, staying there
    /// when it already is, so the message that follows is sent under it.
    /// The event loop handles events in order, so the switch is recorded
    /// before the message starts a turn.
    pub(super) fn request_plan_mode(&mut self) {
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
    pub(super) fn request_mode(&mut self, mode: PermissionMode) {
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
    pub(super) fn request_build(&mut self, arg: &str) {
        if self.turn_running() {
            self.notifications.current = Some((
                "Wait for the current turn to finish before building".to_string(),
                NotificationType::Warning,
            ));
            return;
        }
        let path = (!arg.is_empty()).then(|| arg.to_string());
        if path.is_none() && self.active_plan.is_none() {
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
    pub(super) fn wants_build(&self, text: &str) -> bool {
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
        if self.hud.mode != Some(PermissionMode::Plan) || self.active_plan.is_none() {
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

    /// Handle `/spawn <permanent agent>`.
    fn handle_spawn_command(&mut self, arg: &str) {
        let name = self
            .definitions
            .items
            .iter()
            .find(|d| d.name.eq_ignore_ascii_case(arg))
            .map(|d| d.name.clone());
        match name {
            Some(name) => self.instantiate_definition(&name),
            None => {
                let available: Vec<&str> = self
                    .definitions
                    .items
                    .iter()
                    .map(|d| d.name.as_str())
                    .collect();
                self.notifications.current = Some((
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
            self.notifications.current = Some((
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
    pub(super) fn request_approval(&mut self, level: ApprovalLevel) {
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
                self.notifications.current = Some((
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
                self.notifications.current = Some((
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
            self.notifications.current = Some((
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
            self.notifications.current = Some((
                "Queued: AGENTS.md is written when the current work ends".to_string(),
                NotificationType::Success,
            ));
        } else {
            self.state = AppState::Processing;
            self.status = "Studying the project...".to_string();
        }
    }
}

/// What `/thinking` takes, in the order Alt+T cycles through them. The
/// names past `auto` are the wire names of `aura_model::ThinkingEffort`.
const THINKING_LEVELS: [(&str, &str); 8] = [
    ("auto", "Each model picks its own (the default)"),
    ("off", "No extended thinking"),
    ("minimal", "The least the model does"),
    ("low", "Quick, for simple edits"),
    ("medium", "Balanced"),
    ("high", "More for harder problems"),
    ("xhigh", "Even more, on models that have it"),
    ("max", "The most the model does"),
];

/// Canonical plan id for `/login` and `/logout` arguments.
fn plan_provider(arg: &str) -> Option<&'static str> {
    match arg.trim().to_ascii_lowercase().as_str() {
        "claude" | "anthropic" => Some("claude"),
        "chatgpt" | "openai" => Some("chatgpt"),
        "openrouter" | "open-router" | "or" => Some("openrouter"),
        _ => None,
    }
}
