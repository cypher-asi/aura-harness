//! Agent commands: switching, creating, spawning and archiving agents.

use crate::{
    app::{App, NotificationType},
    events::UiEvent,
};

impl App {
    /// Run `cmd` when it is an agent command; whether it was one.
    pub(super) fn agent_command(&mut self, cmd: &str, arg: &str) -> bool {
        match cmd {
            "agent" | "a" => self.handle_agent_command(arg),
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
            _ => return false,
        }
        true
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
}
