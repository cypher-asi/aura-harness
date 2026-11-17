//! Slash-command menu shown above the input while a `/command` is typed.
//! After `/agent ` it lists permanent agents to add to the project instead.

use super::{App, AppState};
use crate::{events::AgentDefinitionSummary, terminal::KeyResult};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A command offered by the slash menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlashCommand {
    /// Name without the leading `/`
    pub name: &'static str,
    /// Argument hint shown after the name (empty when it takes none)
    pub args: &'static str,
    /// One-line description
    pub description: &'static str,
    /// Whether picking the command leaves the input open for its arguments
    /// instead of running it
    pub needs_args: bool,
}

const fn command(name: &'static str, description: &'static str) -> SlashCommand {
    SlashCommand {
        name,
        args: "",
        description,
        needs_args: false,
    }
}

/// Commands offered by the menu, in display order.
pub const SLASH_COMMANDS: &[SlashCommand] = &[
    command("help", "Show help"),
    command("new", "New session (reset context)"),
    SlashCommand {
        name: "resume",
        args: "[n]",
        description: "List earlier sessions, or resume one",
        needs_args: false,
    },
    SlashCommand {
        name: "rewind",
        args: "[n] [code|chat|both]",
        description: "List turns, or undo back to before one",
        needs_args: false,
    },
    SlashCommand {
        name: "diff",
        args: "[session]",
        description: "Show file changes of the last turn or session",
        needs_args: false,
    },
    command("good", "Rate the last turn good (teaches Gate)"),
    command("bad", "Rate the last turn bad (teaches Gate)"),
    SlashCommand {
        name: "name",
        args: "[name]",
        description: "Rename the current project",
        needs_args: false,
    },
    command("clear", "Clear messages"),
    command("context", "Show what fills the context window"),
    SlashCommand {
        name: "compact",
        args: "[focus]",
        description: "Summarize older history now",
        needs_args: false,
    },
    command("init", "Have the agent write or improve AGENTS.md"),
    command("todos", "Toggle the agent's todo list"),
    command("cost", "Show tokens and cost of the session"),
    SlashCommand {
        name: "agent",
        args: "new [name] | <n|name>",
        description: "Add an agent to the project, create or switch",
        needs_args: true,
    },
    SlashCommand {
        name: "spawn",
        args: "<permanent agent>",
        description: "Start an instance of a permanent agent",
        needs_args: true,
    },
    command("agents", "Toggle Agents panel"),
    command("kernel", "Toggle Kernel panel"),
    command("files", "Toggle Sandbox panel"),
    SlashCommand {
        name: "model",
        args: "[id]",
        description: "Choose model (local or cloud)",
        needs_args: false,
    },
    command(
        "plan",
        "Plan mode: research and write a plan, no code edits",
    ),
    SlashCommand {
        name: "build",
        args: "[plan path]",
        description: "Leave plan mode and implement the latest plan",
        needs_args: false,
    },
    SlashCommand {
        name: "mode",
        args: "[default|accept-edits|plan|unrestricted]",
        description: "Show or set what runs without asking",
        needs_args: false,
    },
    SlashCommand {
        name: "graph",
        args: "[run id]",
        description: "Run graph: turns, steps, decisions and links",
        needs_args: false,
    },
    command("runs", "List recorded runs"),
    command("status", "Scorecard of this conversation's run"),
    command("login", "Login for cloud models"),
    command("logout", "Clear credentials"),
    command("whoami", "Show auth status"),
    command("quit", "Exit"),
];

/// Items whose name contains `query`, ignoring case: prefix matches first,
/// then the rest, each in their original order.
fn prefix_first<'a, T>(
    items: impl IntoIterator<Item = &'a T>,
    query: &str,
    name: impl Fn(&T) -> &str,
) -> Vec<&'a T> {
    let query = query.to_lowercase();
    let (mut prefix, rest): (Vec<_>, Vec<_>) = items
        .into_iter()
        .filter(|item| name(item).to_lowercase().contains(&query))
        .partition(|item| name(item).to_lowercase().starts_with(&query));
    prefix.extend(rest);
    prefix
}

impl App {
    /// The menu was not dismissed with Esc, the line is not at the terminal
    /// prompt (where `/` starts a path), and no modal state owns the
    /// keyboard.
    fn slash_menu_allowed(&self) -> bool {
        !self.slash_dismissed
            && !self.terminal_mode
            && matches!(self.state, AppState::Idle | AppState::Processing)
    }

    /// Commands matching the `/word` being typed. Empty while the menu is
    /// closed: the input is not a lone `/word`, the menu was dismissed with
    /// Esc, or a modal state owns the keyboard.
    #[must_use]
    pub fn slash_matches(&self) -> Vec<&'static SlashCommand> {
        if !self.slash_menu_allowed() {
            return Vec::new();
        }
        match self.input.strip_prefix('/') {
            Some(query) if !query.contains(char::is_whitespace) => {
                prefix_first(SLASH_COMMANDS, query, |c| c.name)
            }
            _ => Vec::new(),
        }
    }

    /// Permanent agents matching the name typed after `/agent `, offered
    /// for adding to the active agent's project. Empty while the menu is
    /// closed, and for `/agent new`, which creates a blank agent instead.
    #[must_use]
    pub fn agent_picks(&self) -> Vec<&AgentDefinitionSummary> {
        if !self.slash_menu_allowed() {
            return Vec::new();
        }
        let Some((command, query)) = self
            .input
            .strip_prefix('/')
            .and_then(|rest| rest.split_once(' '))
        else {
            return Vec::new();
        };
        let query = query.trim();
        let creating = query
            .split_whitespace()
            .next()
            .is_some_and(|word| word.eq_ignore_ascii_case("new"));
        if !(command.eq_ignore_ascii_case("agent") || command.eq_ignore_ascii_case("a")) || creating
        {
            return Vec::new();
        }
        prefix_first(&self.definitions, query, |d| d.name.as_str())
    }

    /// Rows in the open menu: matching commands, or else agents to add.
    fn slash_menu_len(&self) -> usize {
        match self.slash_matches().len() {
            0 => self.agent_picks().len(),
            commands => commands,
        }
    }

    /// Highlighted row in the slash menu.
    #[must_use]
    pub fn slash_cursor(&self) -> usize {
        self.slash_cursor
            .min(self.slash_menu_len().saturating_sub(1))
    }

    /// Reopen the menu at its first row after the input was edited.
    pub(super) fn reset_slash_menu(&mut self) {
        self.slash_cursor = 0;
        self.slash_dismissed = false;
    }

    /// Handle a key while the slash menu is open. `None` when the menu is
    /// closed or the key is not one it uses.
    pub(super) fn handle_slash_menu_key(&mut self, key: KeyEvent) -> Option<KeyResult> {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return None;
        }
        let last = self.slash_menu_len().checked_sub(1)?;
        let cursor = self.slash_cursor();
        match key.code {
            KeyCode::Up => self.slash_cursor = cursor.checked_sub(1).unwrap_or(last),
            KeyCode::Down => self.slash_cursor = if cursor >= last { 0 } else { cursor + 1 },
            KeyCode::Tab | KeyCode::Enter => {
                if let Some(&command) = self.slash_matches().get(cursor) {
                    self.complete_slash_command(command);
                    if key.code == KeyCode::Enter && !command.needs_args {
                        return Some(self.handle_chat_key(key));
                    }
                } else if let Some(name) = self.agent_picks().get(cursor).map(|d| d.name.clone()) {
                    if key.code == KeyCode::Tab {
                        self.set_slash_input(format!("/agent {name}"));
                    } else {
                        self.add_definition_to_project(&name);
                        self.set_slash_input(String::new());
                    }
                }
            }
            KeyCode::Esc => self.slash_dismissed = true,
            _ => return None,
        }
        Some(KeyResult::continue_running())
    }

    /// Replace the input with `/name`, plus a space when arguments follow.
    fn complete_slash_command(&mut self, command: &SlashCommand) {
        let space = if command.needs_args { " " } else { "" };
        self.set_slash_input(format!("/{}{space}", command.name));
    }

    /// Replace the input with `text`, cursor at its end, menu at its top.
    fn set_slash_input(&mut self, text: String) {
        self.input = text;
        self.cursor_pos = self.input.len();
        self.slash_cursor = 0;
    }
}
