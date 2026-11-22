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

    /// Break the input line at the cursor.
    pub(super) fn insert_newline(&mut self) {
        let pos = self.cursor_pos.min(self.input.len());
        self.input.insert(pos, '\n');
        self.cursor_pos = pos + 1;
    }

    /// Move the cursor to the same column on the line above (`up`) or
    /// below in a multi-line input. Returns `false`, leaving the cursor
    /// where it was, when there is no line that way, so the key can recall
    /// history instead.
    pub(super) fn move_cursor_line(&mut self, up: bool) -> bool {
        let pos = self.cursor_pos.min(self.input.len());
        let target = if up {
            line_above(&self.input, pos)
        } else {
            line_below(&self.input, pos)
        };
        match target {
            Some(target) => {
                self.cursor_pos = target;
                true
            }
            None => false,
        }
    }
}

/// Whether `key` asks for a line break rather than a send: Shift+Enter
/// where the terminal reports it, and Alt+Enter and Ctrl+J for those that
/// cannot tell Shift+Enter from Enter (in raw mode a typed line feed
/// arrives as Ctrl+J). Ctrl+Enter is left alone; it sends.
pub(super) fn is_newline_key(key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Enter => {
            key.modifiers
                .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT)
                && !key.modifiers.contains(KeyModifiers::CONTROL)
        }
        KeyCode::Char('j') => key.modifiers == KeyModifiers::CONTROL,
        _ => false,
    }
}

/// Byte offset of the start of the line holding `pos`.
fn line_start(text: &str, pos: usize) -> usize {
    text[..pos].rfind('\n').map_or(0, |i| i + 1)
}

/// Byte offset of the `col`-th char of `line`, or its end when it is
/// shorter.
fn column_offset(line: &str, col: usize) -> usize {
    line.char_indices().nth(col).map_or(line.len(), |(i, _)| i)
}

/// Cursor on the line above `pos`, at the same char column or the end of
/// that line when it is shorter. `None` on the first line.
fn line_above(text: &str, pos: usize) -> Option<usize> {
    let start = line_start(text, pos);
    let above_end = start.checked_sub(1)?;
    let col = text[start..pos].chars().count();
    let above_start = line_start(text, above_end);
    Some(above_start + column_offset(&text[above_start..above_end], col))
}

/// Cursor on the line below `pos`, as [`line_above`]. `None` on the last
/// line.
fn line_below(text: &str, pos: usize) -> Option<usize> {
    let below_start = pos + text[pos..].find('\n')? + 1;
    let col = text[line_start(text, pos)..pos].chars().count();
    let below_end = text[below_start..]
        .find('\n')
        .map_or(text.len(), |i| below_start + i);
    Some(below_start + column_offset(&text[below_start..below_end], col))
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
    fn newline_keys_are_shift_or_alt_enter_and_ctrl_j() {
        let cases = [
            (KeyCode::Enter, KeyModifiers::SHIFT, true),
            (KeyCode::Enter, KeyModifiers::ALT, true),
            (KeyCode::Char('j'), KeyModifiers::CONTROL, true),
            (KeyCode::Enter, KeyModifiers::NONE, false),
            (KeyCode::Enter, KeyModifiers::CONTROL, false),
            (KeyCode::Char('j'), KeyModifiers::NONE, false),
            (
                KeyCode::Char('j'),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
                false,
            ),
        ];
        for (code, modifiers, expected) in cases {
            assert_eq!(
                is_newline_key(KeyEvent::new(code, modifiers)),
                expected,
                "{code:?} {modifiers:?}"
            );
        }
    }

    #[test]
    fn cursor_moves_between_lines_at_the_same_column() {
        // "abc\nx\nhello": lines start at bytes 0, 4 and 6.
        let text = "abc\nx\nhello";
        assert_eq!(line_above(text, 9), Some(5), "clamped to the short line");
        assert_eq!(line_above(text, 5), Some(1));
        assert_eq!(line_above(text, 2), None, "first line");
        assert_eq!(line_below(text, 2), Some(5), "clamped to the short line");
        assert_eq!(line_below(text, 4), Some(6));
        assert_eq!(line_below(text, 8), None, "last line");
        assert_eq!(line_below("ab\n", 1), Some(3), "onto an empty last line");
        assert_eq!(line_above("é\nab", 4), Some(2), "columns count chars");
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
