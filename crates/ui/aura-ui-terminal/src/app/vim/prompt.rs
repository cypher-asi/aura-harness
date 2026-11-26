//! The bottom line: typing a search or a `:` command and running it.

use super::{Align, Outcome, Prompt, Search, Vim};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl Vim {
    pub(super) fn prompt_key(&mut self, key: KeyEvent, lines: &[&str]) -> Outcome {
        let Some(prompt) = self.prompt.as_mut() else {
            return Outcome::Stay;
        };
        let text = match prompt {
            Prompt::Search { text, .. } | Prompt::Command(text) => text,
        };
        match key.code {
            KeyCode::Esc => self.prompt = None,
            KeyCode::Backspace if text.is_empty() => self.prompt = None,
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => text.clear(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => text.push(c),
            KeyCode::Enter => {
                self.align = Align::Cursor;
                match self.prompt.take() {
                    Some(Prompt::Search { forward, text }) => {
                        if !text.is_empty() {
                            self.search = Some(Search {
                                pattern: text,
                                forward,
                                whole_word: false,
                            });
                        } else if let Some(search) = self.search.as_mut() {
                            search.forward = forward;
                        }
                        self.search_again(1, false, lines);
                    }
                    Some(Prompt::Command(command)) => return self.run_command(&command, lines),
                    None => {}
                }
            }
            _ => {}
        }
        Outcome::Stay
    }

    fn run_command(&mut self, command: &str, lines: &[&str]) -> Outcome {
        let command = command.trim();
        match command {
            "" => {}
            "q" | "q!" | "qa" | "qa!" | "quit" | "x" | "wq" | "close" => return Outcome::Close,
            "noh" | "nohl" | "nohlsearch" => self.highlight = false,
            "$" => {
                self.set_line(usize::MAX, lines);
                self.go_first_non_blank(lines);
            }
            _ => match command.parse::<usize>() {
                Ok(n) => {
                    self.set_line(n.saturating_sub(1), lines);
                    self.go_first_non_blank(lines);
                }
                Err(_) => self.message = Some(format!("Not an editor command: {command}")),
            },
        }
        Outcome::Stay
    }
}
