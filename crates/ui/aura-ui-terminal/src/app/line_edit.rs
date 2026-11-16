//! Readline-style editing of the input line.
//!
//! `cursor_pos` is a byte offset into `input` and always sits on a char
//! boundary.

use super::App;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What an editing key did to the input line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LineEdit {
    /// Only the cursor moved.
    Moved,
    /// The text changed.
    Changed,
}

impl App {
    /// Apply `key` to the input line if it is an editing key:
    ///
    /// - Left/Right, Ctrl+B/F: move by a character
    /// - Ctrl or Alt + Left/Right, Alt+B/F: move by a word
    /// - Home/End, Ctrl+A/E: jump to the start or end
    /// - Backspace/Delete: delete a character
    /// - Ctrl or Alt + Backspace, Ctrl+W, Ctrl+H: delete the word before the
    ///   cursor (terminals send Ctrl+Backspace as Ctrl+H)
    /// - Ctrl or Alt + Delete, Alt+D: delete the word after the cursor
    /// - Ctrl+U/K: delete to the start or end
    pub(super) fn edit_input(&mut self, key: KeyEvent) -> Option<LineEdit> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let text = self.input.as_str();
        let len = text.len();
        let pos = self.cursor_pos.min(len);
        let (target, delete) = match key.code {
            // Ctrl+Alt is AltGr on Windows, which types characters.
            KeyCode::Char(c) if ctrl == alt => {
                self.input.insert(pos, c);
                self.cursor_pos = pos + c.len_utf8();
                return Some(LineEdit::Changed);
            }
            KeyCode::Char('b') if ctrl => (prev_char(text, pos), false),
            KeyCode::Char('f') if ctrl => (next_char(text, pos), false),
            KeyCode::Left | KeyCode::Char('b') if ctrl || alt => (prev_word(text, pos), false),
            KeyCode::Right | KeyCode::Char('f') if ctrl || alt => (next_word(text, pos), false),
            KeyCode::Left => (prev_char(text, pos), false),
            KeyCode::Right => (next_char(text, pos), false),
            KeyCode::Home => (0, false),
            KeyCode::End => (len, false),
            KeyCode::Char('a') if ctrl => (0, false),
            KeyCode::Char('e') if ctrl => (len, false),
            KeyCode::Backspace if ctrl || alt => (prev_word(text, pos), true),
            KeyCode::Char('w' | 'h') if ctrl => (prev_word(text, pos), true),
            KeyCode::Backspace => (prev_char(text, pos), true),
            KeyCode::Delete if ctrl || alt => (next_word(text, pos), true),
            KeyCode::Char('d') if alt => (next_word(text, pos), true),
            KeyCode::Delete => (next_char(text, pos), true),
            KeyCode::Char('u') if ctrl => (0, true),
            KeyCode::Char('k') if ctrl => (len, true),
            _ => return None,
        };
        let (start, end) = (target.min(pos), target.max(pos));
        if !delete || start == end {
            self.cursor_pos = target;
            return Some(LineEdit::Moved);
        }
        self.input.replace_range(start..end, "");
        self.cursor_pos = start;
        Some(LineEdit::Changed)
    }
}

fn prev_char(text: &str, pos: usize) -> usize {
    text[..pos].char_indices().next_back().map_or(0, |(i, _)| i)
}

fn next_char(text: &str, pos: usize) -> usize {
    text[pos..]
        .chars()
        .next()
        .map_or(pos, |c| pos + c.len_utf8())
}

/// Letters, digits and `_` form words; other non-blank characters form
/// runs of their own, so `src/main.rs` is five stops.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn same_class(a: char, b: char) -> bool {
    is_word_char(a) == is_word_char(b)
}

/// Start of the word before `pos`, skipping blanks first.
fn prev_word(text: &str, pos: usize) -> usize {
    let mut chars = text[..pos].char_indices().rev().peekable();
    while chars.next_if(|(_, c)| c.is_whitespace()).is_some() {}
    let Some(&(mut start, first)) = chars.peek() else {
        return 0;
    };
    while let Some((i, _)) = chars.next_if(|(_, c)| !c.is_whitespace() && same_class(*c, first)) {
        start = i;
    }
    start
}

/// End of the word after `pos`, skipping blanks first.
fn next_word(text: &str, pos: usize) -> usize {
    let mut chars = text[pos..].char_indices().peekable();
    while chars.next_if(|(_, c)| c.is_whitespace()).is_some() {}
    let Some(&(_, first)) = chars.peek() else {
        return text.len();
    };
    let mut end = text.len();
    for (i, c) in chars {
        if c.is_whitespace() || !same_class(c, first) {
            end = pos + i;
            break;
        }
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with(input: &str, cursor: usize) -> App {
        let mut app = App::new();
        app.input = input.to_string();
        app.cursor_pos = cursor;
        app
    }

    fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        app.edit_input(KeyEvent::new(code, modifiers));
    }

    #[test]
    fn ctrl_backspace_deletes_word_by_word() {
        let text = "git commit -m fix";
        let mut app = app_with(text, text.len());
        let mut seen = Vec::new();
        while !app.input.is_empty() {
            press(&mut app, KeyCode::Backspace, KeyModifiers::CONTROL);
            seen.push(app.input.clone());
        }
        assert_eq!(
            seen,
            ["git commit -m ", "git commit -", "git commit ", "git ", ""]
        );
    }

    #[test]
    fn terminal_variants_of_word_delete() {
        for (code, modifiers) in [
            (KeyCode::Char('h'), KeyModifiers::CONTROL),
            (KeyCode::Char('w'), KeyModifiers::CONTROL),
            (KeyCode::Backspace, KeyModifiers::ALT),
        ] {
            let mut app = app_with("cd src/main.rs", 14);
            press(&mut app, code, modifiers);
            assert_eq!(app.input, "cd src/main.", "{code:?} {modifiers:?}");
            assert_eq!(app.cursor_pos, 12);
        }
    }

    #[test]
    fn word_jumps_stop_at_word_edges() {
        let text = "echo hello  world";
        let mut app = app_with(text, text.len());
        let mut stops = Vec::new();
        for _ in 0..4 {
            press(&mut app, KeyCode::Left, KeyModifiers::CONTROL);
            stops.push(app.cursor_pos);
        }
        assert_eq!(stops, [12, 5, 0, 0]);

        stops.clear();
        for _ in 0..4 {
            press(&mut app, KeyCode::Right, KeyModifiers::ALT);
            stops.push(app.cursor_pos);
        }
        assert_eq!(stops, [4, 10, 17, 17]);
    }

    #[test]
    fn line_kills_and_jumps() {
        let mut app = app_with("hello world", 5);
        press(&mut app, KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert_eq!((app.input.as_str(), app.cursor_pos), ("hello", 5));
        press(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert_eq!(app.cursor_pos, 0);
        press(&mut app, KeyCode::Char('e'), KeyModifiers::CONTROL);
        assert_eq!(app.cursor_pos, 5);
        press(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!((app.input.as_str(), app.cursor_pos), ("", 0));
    }

    #[test]
    fn forward_word_delete() {
        let mut app = app_with("one two three", 3);
        press(&mut app, KeyCode::Delete, KeyModifiers::CONTROL);
        assert_eq!(app.input, "one three");
        press(&mut app, KeyCode::Char('d'), KeyModifiers::ALT);
        assert_eq!(app.input, "one");
    }

    #[test]
    fn multibyte_text_keeps_char_boundaries() {
        let mut app = App::new();
        for c in "héllo wörld".chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        assert_eq!(app.input, "héllo wörld");
        press(&mut app, KeyCode::Left, KeyModifiers::NONE);
        press(&mut app, KeyCode::Left, KeyModifiers::NONE);
        press(&mut app, KeyCode::Left, KeyModifiers::NONE);
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(app.input, "héllo wrld");
        press(&mut app, KeyCode::Backspace, KeyModifiers::CONTROL);
        assert_eq!(app.input, "héllo rld");
    }

    #[test]
    fn other_control_keys_are_not_typed() {
        let mut app = App::new();
        assert_eq!(
            app.edit_input(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)),
            None
        );
        assert!(app.input.is_empty());
    }
}
