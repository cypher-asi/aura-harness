//! Window input as the crossterm events the terminal UI handles, so keys
//! behave exactly as they do in a terminal.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use iced::keyboard::key::Named;
use iced::keyboard::{Key, Modifiers};

pub fn modifiers(m: Modifiers) -> KeyModifiers {
    let mut out = KeyModifiers::NONE;
    if m.shift() {
        out |= KeyModifiers::SHIFT;
    }
    if m.control() {
        out |= KeyModifiers::CONTROL;
    }
    if m.alt() {
        out |= KeyModifiers::ALT;
    }
    if m.logo() {
        out |= KeyModifiers::SUPER;
    }
    out
}

/// The key events a terminal would report for one key press. Usually one;
/// text from an input method can be several characters.
pub fn key_events(key: &Key, text: Option<&str>, m: Modifiers) -> Vec<KeyEvent> {
    let mods = modifiers(m);
    let code = match key.as_ref() {
        Key::Named(named) => match named {
            Named::Enter => KeyCode::Enter,
            Named::Tab if m.shift() => KeyCode::BackTab,
            Named::Tab => KeyCode::Tab,
            Named::Backspace => KeyCode::Backspace,
            Named::Delete => KeyCode::Delete,
            Named::Escape => KeyCode::Esc,
            Named::ArrowUp => KeyCode::Up,
            Named::ArrowDown => KeyCode::Down,
            Named::ArrowLeft => KeyCode::Left,
            Named::ArrowRight => KeyCode::Right,
            Named::Home => KeyCode::Home,
            Named::End => KeyCode::End,
            Named::PageUp => KeyCode::PageUp,
            Named::PageDown => KeyCode::PageDown,
            Named::Insert => KeyCode::Insert,
            Named::Space => KeyCode::Char(' '),
            Named::F1 => KeyCode::F(1),
            Named::F2 => KeyCode::F(2),
            Named::F3 => KeyCode::F(3),
            Named::F4 => KeyCode::F(4),
            Named::F5 => KeyCode::F(5),
            Named::F6 => KeyCode::F(6),
            Named::F7 => KeyCode::F(7),
            Named::F8 => KeyCode::F(8),
            Named::F9 => KeyCode::F(9),
            Named::F10 => KeyCode::F(10),
            Named::F11 => KeyCode::F(11),
            Named::F12 => KeyCode::F(12),
            _ => return Vec::new(),
        },
        // With Ctrl or Alt held a terminal reports the key itself
        // (Ctrl+C is `c` + CONTROL), not the control character.
        Key::Character(c) if m.control() || m.alt() => match c.chars().next() {
            Some(c) => KeyCode::Char(c.to_ascii_lowercase()),
            None => return Vec::new(),
        },
        Key::Character(c) => {
            let typed = text
                .filter(|t| !t.chars().any(char::is_control))
                .unwrap_or(c);
            return typed
                .chars()
                .map(|ch| KeyEvent::new(KeyCode::Char(ch), mods))
                .collect();
        }
        Key::Unidentified => {
            let Some(typed) = text.filter(|t| !t.chars().any(char::is_control)) else {
                return Vec::new();
            };
            return typed
                .chars()
                .map(|ch| KeyEvent::new(KeyCode::Char(ch), mods))
                .collect();
        }
    };
    vec![KeyEvent::new(code, mods)]
}

/// Pasted text as typed characters. The input line is single-line, so
/// line breaks and tabs become spaces (a CRLF pair is one space).
pub fn paste_events(text: &str) -> Vec<KeyEvent> {
    text.replace("\r\n", "\n")
        .chars()
        .filter_map(|c| match c {
            '\n' | '\r' | '\t' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .map(|c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::needless_pass_by_value)]
    fn press(key: Key, text: Option<&str>, m: Modifiers) -> Vec<(KeyCode, KeyModifiers)> {
        key_events(&key, text, m)
            .into_iter()
            .map(|e| (e.code, e.modifiers))
            .collect()
    }

    #[test]
    fn typed_text_uses_the_layout_character() {
        let got = press(Key::Character("a".into()), Some("A"), Modifiers::SHIFT);
        assert_eq!(got, vec![(KeyCode::Char('A'), KeyModifiers::SHIFT)]);
    }

    #[test]
    fn control_chords_report_the_key() {
        let got = press(Key::Character("c".into()), Some("\u{3}"), Modifiers::CTRL);
        assert_eq!(got, vec![(KeyCode::Char('c'), KeyModifiers::CONTROL)]);
    }

    #[test]
    fn named_keys_map_to_terminal_codes() {
        assert_eq!(
            press(Key::Named(Named::Enter), Some("\r"), Modifiers::empty()),
            vec![(KeyCode::Enter, KeyModifiers::NONE)]
        );
        assert_eq!(
            press(Key::Named(Named::Tab), None, Modifiers::SHIFT),
            vec![(KeyCode::BackTab, KeyModifiers::SHIFT)]
        );
        assert_eq!(
            press(Key::Named(Named::Space), Some(" "), Modifiers::empty()),
            vec![(KeyCode::Char(' '), KeyModifiers::NONE)]
        );
        assert!(press(Key::Named(Named::Shift), None, Modifiers::SHIFT).is_empty());
    }

    #[test]
    fn paste_flattens_lines() {
        let codes: String = paste_events("a\r\nb\tc\u{7}")
            .into_iter()
            .map(|e| match e.code {
                KeyCode::Char(c) => c,
                _ => '?',
            })
            .collect();
        assert_eq!(codes, "a b c");
    }
}
