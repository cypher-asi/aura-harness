//! Slash commands: what `/name args` typed in the input does.

mod account;
mod agents;
mod modes;
mod panels;
mod session;

use super::{App, NotificationType};
use crate::{events::UiEvent, terminal::KeyResult};

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
    pub(in crate::app) fn handle_command(&mut self, text: &str) -> KeyResult {
        // A line break or tab ends the name as a space does, so a recalled
        // or pasted "/model\n" is still `/model`.
        let body = text.strip_prefix('/').unwrap_or(text).trim();
        let (cmd, arg) = body.split_once(char::is_whitespace).unwrap_or((body, ""));
        let cmd = cmd.to_lowercase();
        let arg = arg.trim();

        if matches!(cmd.as_str(), "quit" | "exit" | "q") {
            return self.quit_command();
        }
        // Each domain handles its own commands and reports whether `cmd`
        // was one of them; the names never overlap.
        let handled = self.session_command(&cmd, arg)
            || self.panel_command(&cmd, arg)
            || self.agent_command(&cmd, arg)
            || self.account_command(&cmd, arg)
            || self.mode_command(&cmd, arg);
        if !handled {
            self.unknown_command(&cmd);
        }
        KeyResult::continue_running()
    }

    /// `/quit`: tell the event loop, then stop.
    fn quit_command(&self) -> KeyResult {
        self.send_command_event(UiEvent::Quit);
        KeyResult::quit()
    }

    /// Send `event` to the event loop, if there is one.
    pub(super) fn send_command_event(&self, event: UiEvent) {
        let _ = self.try_send_event(event);
    }

    fn unknown_command(&mut self, cmd: &str) {
        self.notifications.current = Some((
            format!("Unknown command: /{cmd}. Type /help for available commands."),
            NotificationType::Warning,
        ));
    }
}
