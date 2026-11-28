//! Session commands: help, the conversation, its reports and its history.

use crate::{
    app::{hud, App, AppState, Dialog, NotificationType},
    components::{Message, MessageRole},
    events::{RewindScope, UiEvent},
};

impl App {
    /// Run `cmd` when it is a session command; whether it was one.
    pub(super) fn session_command(&mut self, cmd: &str, arg: &str) -> bool {
        match cmd {
            "help" | "?" => self.open_dialog(Dialog::Help),
            "clear" => self.clear_command(),
            "status" | "s" => self.send_command_event(UiEvent::ShowStatus),
            "history" | "h" => self.send_command_event(UiEvent::ShowHistory(None)),
            "runs" => self.send_command_event(UiEvent::ListRuns),
            "whoami" | "me" => self.send_command_event(UiEvent::Whoami),
            "context" | "ctx" => self.context_command(arg),
            "cost" | "usage" => self.cost_command(),
            "todos" | "todo" => self.todos_command(arg),
            "compact" => self.handle_compact_command(arg),
            "init" => self.handle_init_command(),
            "resume" => self.handle_resume_command(arg),
            "rewind" => self.handle_rewind_command(arg),
            "diff" => self.handle_diff_command(arg),
            "gate" => self.send_command_event(UiEvent::ShowGate),
            "compare" => self.compare_command(arg),
            "new" | "n" => self.new_session_command(),
            _ => return false,
        }
        true
    }

    /// `/clear`: empty the chat.
    fn clear_command(&mut self) {
        self.messages.clear();
        self.scroll_offset = 0;
        self.send_command_event(UiEvent::Clear);
    }

    /// `/context`: open the CONTEXT panel; `/context text` writes the
    /// report into the chat instead.
    fn context_command(&mut self, arg: &str) {
        if arg.trim() == "text" {
            let report = self.hud.context_report();
            self.add_message(Message::new(MessageRole::System, &report));
        } else {
            self.open_context_panel();
        }
    }

    /// `/cost`: what the session cost, and where it went.
    fn cost_command(&mut self) {
        let mut report = self.hud.cost_report();
        report.push_str(&hud::sources_report(self.turn_sources()));
        self.add_message(Message::new(MessageRole::System, &report));
    }

    /// `/todos`: fold or open the to-do drawer, or bring a finished list
    /// back into it.
    fn todos_command(&mut self, arg: &str) {
        if arg.trim() == "history" && !self.hud.todos.is_empty() {
            if !self.toggle_todo_history() {
                self.notifications.current = Some((
                    "No finished to-dos yet".to_string(),
                    NotificationType::Warning,
                ));
            }
            return;
        }
        if self.hud.todos.is_empty() {
            self.notifications.current = Some((
                "The agent has no todo list".to_string(),
                NotificationType::Warning,
            ));
        } else {
            self.cycle_todo_drawer();
        }
    }

    /// `/compare [args]`: compare the active agent's runs.
    fn compare_command(&self, arg: &str) {
        self.send_command_event(UiEvent::Compare {
            agent_id: self.active_agent_id.clone(),
            args: arg.to_string(),
        });
    }

    /// `/new`: start a new conversation, keeping the jobs, modes and
    /// instructions.
    fn new_session_command(&mut self) {
        self.messages.clear();
        self.scroll_offset = 0;
        self.chat.streaming_content.clear();
        self.chat.early_policies.clear();
        let jobs = std::mem::take(&mut self.hud.jobs);
        let instructions = self.hud.instructions.take();
        self.hud = hud::Hud {
            jobs,
            mode: self.hud.mode,
            approval: self.hud.approval,
            instructions,
            ..Default::default()
        };
        self.reset_todo_drawer();
        // The new conversation is a new run, which no agent has
        // joined yet.
        self.start_members_run();
        self.send_command_event(UiEvent::NewSession);
        self.notifications.current =
            Some(("New session started".to_string(), NotificationType::Success));
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
        if let Some(tx) = &self.link.event_tx {
            let _ = tx.try_send(UiEvent::Resume {
                agent_id: self.active_agent_id.clone(),
                index,
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
        if let Some(tx) = &self.link.event_tx {
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
        if let Some(tx) = &self.link.event_tx {
            let _ = tx.try_send(UiEvent::Diff {
                agent_id: self.active_agent_id.clone(),
                session,
            });
        }
    }

    /// `/compact [focus]`: summarize the agent's older history now. A busy
    /// agent does it once its current work ends.
    fn handle_compact_command(&mut self, focus: &str) {
        let Some(tx) = &self.link.event_tx else {
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
            self.chat_state = AppState::Processing;
            self.status = "Summarizing history...".to_string();
        }
    }

    /// `/init`: have the agent study the project and write or improve its
    /// `AGENTS.md`. The chat shows the request once the turn starts; a busy
    /// agent starts it when its current work ends.
    fn handle_init_command(&mut self) {
        let Some(tx) = &self.link.event_tx else {
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
            self.chat_state = AppState::Processing;
            self.status = "Studying the project...".to_string();
        }
    }
}
