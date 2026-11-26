//! Account commands: logging in and out, the model and its thinking level.

use crate::{
    app::{model_picker, App, AppState, NotificationType},
    components::{Message, MessageRole},
    events::UiEvent,
};

impl App {
    /// Run `cmd` when it is an account command; whether it was one.
    pub(super) fn account_command(&mut self, cmd: &str, arg: &str) -> bool {
        match cmd {
            "login" => self.handle_login_command(arg),
            "model" | "models" | "m" => self.model_command(arg),
            "thinking" | "think" | "effort" => self.handle_thinking_command(arg),
            "logout" => self.logout_command(arg),
            _ => return false,
        }
        true
    }

    /// `/model`: open the selector; `/model <name>`: pick that model.
    fn model_command(&mut self, arg: &str) {
        if arg.is_empty() {
            self.open_model_selector();
        } else {
            // An exact id or label maps to its id here; any other
            // name (`opus`, `deepseek`) goes as typed and the event
            // loop resolves it as an alias.
            let id = model_picker::exact_match(&self.account.models, arg)
                .map_or_else(|| arg.to_string(), |m| m.id.clone());
            self.choose_model(&id);
        }
    }

    /// `/logout` (zOS), `/logout zos`, or `/logout <plan>`.
    fn logout_command(&mut self, arg: &str) {
        let event = match plan_provider(arg) {
            Some(provider) => UiEvent::Unlink(provider.to_string()),
            None if arg.is_empty() || arg.eq_ignore_ascii_case("zos") => UiEvent::Logout,
            None => {
                self.notifications.current = Some((
                    format!("Unknown provider: {arg}. Use zos, claude, chatgpt or openrouter."),
                    NotificationType::Warning,
                ));
                return;
            }
        };
        self.send_command_event(event);
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
        if let Some(tx) = &self.link.event_tx {
            let _ = tx.try_send(UiEvent::LinkSubscription {
                provider: provider.to_string(),
                paste,
            });
        }
    }

    /// Enter the email/password login flow.
    pub(in crate::app) fn start_login(&mut self) {
        self.state = AppState::LoginEmail;
        self.account.login_email.clear();
        self.input.text.clear();
        self.input.cursor_pos = 0;
        self.status = "Login — enter email".to_string();
    }

    /// Open the model selector with the current model highlighted.
    pub(in crate::app) fn open_model_selector(&mut self) {
        if self.account.models.is_empty() {
            self.notifications.current = Some((
                "Model list not loaded yet".to_string(),
                NotificationType::Warning,
            ));
            return;
        }
        self.account
            .model_picker
            .open(&self.account.models, &self.account.current_model);
        self.state = AppState::SelectingModel;
    }

    /// Select `id` for the next turn. Models whose sign-in is missing
    /// start that sign-in first and are selected once it succeeds.
    pub(in crate::app) fn choose_model(&mut self, id: &str) {
        let missing = self
            .account
            .models
            .iter()
            .find(|m| m.id == id && !m.available)
            .map(|m| m.provider.clone().unwrap_or_default());
        if let Some(provider) = missing {
            self.pending.model = Some(id.to_string());
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
        if id != self.account.current_model {
            if let Some(tx) = &self.link.event_tx {
                let _ = tx.try_send(UiEvent::SelectModel(id.to_string()));
            }
        }
    }

    /// `/thinking`: show the thinking level and the others;
    /// `/thinking <level>`: pin it for every turn, or `auto` to let each
    /// model pick its own.
    fn handle_thinking_command(&mut self, arg: &str) {
        if arg.is_empty() {
            let current = self.account.thinking.as_deref().unwrap_or("auto");
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
        if level == self.account.thinking {
            self.notifications.current = Some((
                format!("Thinking is already {}", level.as_deref().unwrap_or("auto")),
                NotificationType::Warning,
            ));
            return;
        }
        if let Some(tx) = &self.link.event_tx {
            let _ = tx.try_send(UiEvent::SetThinking(level));
        }
    }

    /// Ask for the thinking level after the current one (Alt+T).
    pub(in crate::app) fn cycle_thinking(&mut self) {
        let current = self.account.thinking.as_deref().unwrap_or("auto");
        let at = THINKING_LEVELS
            .iter()
            .position(|(level, _)| *level == current)
            .unwrap_or(0);
        let (next, _) = THINKING_LEVELS[(at + 1) % THINKING_LEVELS.len()];
        if let Some(tx) = &self.link.event_tx {
            let _ = tx.try_send(UiEvent::SetThinking(
                (next != "auto").then(|| next.to_string()),
            ));
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
