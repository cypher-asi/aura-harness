//! Slash-command menu shown above the input while a `/command` is typed.

use super::{App, AppState};
use crate::terminal::KeyResult;
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
    command("clear", "Clear messages"),
    SlashCommand {
        name: "agent",
        args: "new [name] | <n|name>",
        description: "Create or switch agent",
        needs_args: true,
    },
    command("agents", "Toggle Agents panel"),
    command("record", "Toggle Record panel"),
    command("files", "Toggle Sandbox panel"),
    SlashCommand {
        name: "model",
        args: "[id]",
        description: "Choose model (local or cloud)",
        needs_args: false,
    },
    command("login", "Login for cloud models"),
    command("logout", "Clear credentials"),
    command("whoami", "Show auth status"),
    command("quit", "Exit"),
];

/// Commands whose name contains `query`: prefix matches first, then the
/// rest, each in table order.
fn matching_commands(query: &str) -> Vec<&'static SlashCommand> {
    let query = query.to_lowercase();
    let (mut prefix, rest): (Vec<_>, Vec<_>) = SLASH_COMMANDS
        .iter()
        .filter(|c| c.name.contains(&query))
        .partition(|c| c.name.starts_with(&query));
    prefix.extend(rest);
    prefix
}

impl App {
    /// Commands matching the `/word` being typed. Empty while the menu is
    /// closed: the input is not a lone `/word`, the menu was dismissed with
    /// Esc, or a modal state owns the keyboard.
    #[must_use]
    pub fn slash_matches(&self) -> Vec<&'static SlashCommand> {
        if self.slash_dismissed || !matches!(self.state, AppState::Idle | AppState::Processing) {
            return Vec::new();
        }
        match self.input.strip_prefix('/') {
            Some(query) if !query.contains(char::is_whitespace) => matching_commands(query),
            _ => Vec::new(),
        }
    }

    /// Highlighted row in the slash menu.
    #[must_use]
    pub fn slash_cursor(&self) -> usize {
        self.slash_cursor
            .min(self.slash_matches().len().saturating_sub(1))
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
        let matches = self.slash_matches();
        let last = matches.len().checked_sub(1)?;
        let selected = matches[self.slash_cursor()];
        match key.code {
            KeyCode::Up => {
                self.slash_cursor = self.slash_cursor().checked_sub(1).unwrap_or(last);
            }
            KeyCode::Down => {
                self.slash_cursor = if self.slash_cursor() >= last {
                    0
                } else {
                    self.slash_cursor() + 1
                };
            }
            KeyCode::Tab => self.complete_slash_command(selected),
            KeyCode::Enter => {
                self.complete_slash_command(selected);
                if !selected.needs_args {
                    return Some(self.handle_chat_key(key));
                }
            }
            KeyCode::Esc => self.slash_dismissed = true,
            _ => return None,
        }
        Some(KeyResult::continue_running())
    }

    /// Replace the input with `/name`, plus a space when arguments follow.
    fn complete_slash_command(&mut self, command: &SlashCommand) {
        self.input = format!("/{}", command.name);
        if command.needs_args {
            self.input.push(' ');
        }
        self.cursor_pos = self.input.len();
        self.slash_cursor = 0;
    }
}
