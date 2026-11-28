//! Keys: the window's own shortcuts, and everything else as the
//! crossterm events the terminal UI handles, so keys behave exactly as
//! they do in a terminal.

use super::{Effect, Shell};
use crate::grid::Metrics;
use crate::theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Instant;
use winit::keyboard::{Key, ModifiersState, NamedKey, SmolStr};

/// A key going down.
#[derive(Debug, Clone)]
pub struct Press {
    /// The key as it is with no modifier held: `2` where Shift makes `@`
    pub plain: Key,
    /// The key as the layout has it with the modifiers held
    pub key: Key,
    /// What it types, if anything
    pub text: Option<SmolStr>,
}

#[cfg(test)]
impl Press {
    /// A key with a name, which is the same whatever is held.
    pub fn of(named: NamedKey) -> Self {
        Self {
            plain: Key::Named(named),
            key: Key::Named(named),
            text: None,
        }
    }

    /// A key that types `text`.
    pub fn typing(text: &str) -> Self {
        Self::chord(text, text, text)
    }

    /// The key `plain` that the modifiers held make `key`, typing `text`.
    pub fn chord(plain: &str, key: &str, text: &str) -> Self {
        Self {
            plain: Key::Character(plain.into()),
            key: Key::Character(key.into()),
            text: Some(text.into()),
        }
    }
}

/// What a key typed, without the characters systems keep for keys that
/// type nothing (the arrows, on macOS).
pub fn typed(text: Option<&str>) -> Option<&str> {
    let private = |c: char| ('\u{E000}'..='\u{F8FF}').contains(&c);
    text.filter(|text| !text.chars().any(private))
}

pub fn modifiers(m: ModifiersState) -> KeyModifiers {
    let mut out = KeyModifiers::NONE;
    if m.shift_key() {
        out |= KeyModifiers::SHIFT;
    }
    if m.control_key() {
        out |= KeyModifiers::CONTROL;
    }
    if m.alt_key() {
        out |= KeyModifiers::ALT;
    }
    if m.super_key() {
        out |= KeyModifiers::SUPER;
    }
    out
}

/// The key events a terminal would report for one key press. Usually one;
/// text from an input method can be several characters.
pub fn key_events(key: &Key, text: Option<&str>, m: ModifiersState) -> Vec<KeyEvent> {
    let mods = modifiers(m);
    let typed = |typed: &str| {
        typed
            .chars()
            .map(|ch| KeyEvent::new(KeyCode::Char(ch), mods))
            .collect()
    };
    let printable = |text: &&str| !text.chars().any(char::is_control);
    let code = match key.as_ref() {
        Key::Named(named) => match named {
            NamedKey::Enter => KeyCode::Enter,
            NamedKey::Tab if m.shift_key() => KeyCode::BackTab,
            NamedKey::Tab => KeyCode::Tab,
            NamedKey::Backspace => KeyCode::Backspace,
            NamedKey::Delete => KeyCode::Delete,
            NamedKey::Escape => KeyCode::Esc,
            NamedKey::ArrowUp => KeyCode::Up,
            NamedKey::ArrowDown => KeyCode::Down,
            NamedKey::ArrowLeft => KeyCode::Left,
            NamedKey::ArrowRight => KeyCode::Right,
            NamedKey::Home => KeyCode::Home,
            NamedKey::End => KeyCode::End,
            NamedKey::PageUp => KeyCode::PageUp,
            NamedKey::PageDown => KeyCode::PageDown,
            NamedKey::Insert => KeyCode::Insert,
            NamedKey::Space => KeyCode::Char(' '),
            NamedKey::F1 => KeyCode::F(1),
            NamedKey::F2 => KeyCode::F(2),
            NamedKey::F3 => KeyCode::F(3),
            NamedKey::F4 => KeyCode::F(4),
            NamedKey::F5 => KeyCode::F(5),
            NamedKey::F6 => KeyCode::F(6),
            NamedKey::F7 => KeyCode::F(7),
            NamedKey::F8 => KeyCode::F(8),
            NamedKey::F9 => KeyCode::F(9),
            NamedKey::F10 => KeyCode::F(10),
            NamedKey::F11 => KeyCode::F(11),
            NamedKey::F12 => KeyCode::F(12),
            _ => return Vec::new(),
        },
        // With Ctrl or Alt held a terminal reports the key itself
        // (Ctrl+C is `c` + CONTROL), not the control character.
        Key::Character(c) if m.control_key() || m.alt_key() => match c.chars().next() {
            Some(c) => KeyCode::Char(c.to_ascii_lowercase()),
            None => return Vec::new(),
        },
        Key::Character(c) => return typed(text.filter(printable).unwrap_or(c)),
        // A key the layout has no name for, or the second of a pair
        // that composes a character: only what it typed is known.
        Key::Unidentified(_) | Key::Dead(_) => {
            return text.filter(printable).map(typed).unwrap_or_default();
        }
    };
    vec![KeyEvent::new(code, mods)]
}

/// The tab a number key picks, counting from 1.
fn tab_number(key: &Key) -> Option<usize> {
    match key.as_ref() {
        Key::Character(digit) => digit.parse().ok().filter(|number| (1..=9).contains(number)),
        _ => None,
    }
}

impl Shell {
    /// The modifier keys held changed.
    pub const fn modifiers_changed(&mut self, modifiers: ModifiersState) {
        self.modifiers = modifiers;
    }

    /// A key went down, or repeats.
    pub fn key_pressed(&mut self, press: &Press, now: Instant) {
        let started = Instant::now();
        // The cursor shows while typing.
        self.blink_from = now;
        if !self.shortcut(press) {
            let text = typed(press.text.as_deref());
            for event in key_events(&press.plain, text, self.modifiers) {
                if self.key_press(event) {
                    self.effects.push(Effect::Exit);
                    break;
                }
            }
        }
        self.settle(started);
    }

    /// Window shortcuts a terminal emulator would handle itself: tabs,
    /// zoom, copy and paste. Whether `press` was one.
    fn shortcut(&mut self, press: &Press) -> bool {
        let m = self.modifiers;
        if m.control_key() && press.key == Key::Named(NamedKey::Tab) {
            if m.shift_key() {
                self.tabs.previous();
            } else {
                self.tabs.next();
            }
            return true;
        }
        let chord = m.super_key() || (!cfg!(target_os = "macos") && m.control_key());
        // Tabs by number, as window managers switch workspaces. Option
        // types symbols on macOS, so Alt is for the other systems.
        if chord || (m.alt_key() && !cfg!(target_os = "macos")) {
            if let Some(number) = tab_number(&press.plain) {
                self.tabs.focus(number - 1);
                return true;
            }
        }
        if !chord {
            if m.shift_key() && press.key == Key::Named(NamedKey::Insert) {
                self.tabs.front_mut().tui.paste_from_clipboard();
                return true;
            }
            return false;
        }
        self.chord(press)
    }

    /// A shortcut held with the window's chord modifier: zoom, paste,
    /// copy, and opening and closing tabs. Whether `press` was one.
    fn chord(&mut self, press: &Press) -> bool {
        let m = self.modifiers;
        // Ctrl+C, Ctrl+V, Ctrl+T and Ctrl+W on their own belong to the
        // UI, as in a terminal; the window takes them with Shift.
        let shifted = m.shift_key() || m.super_key() || cfg!(target_os = "macos");
        let size = self.font_size;
        let zoom = |size: f32| size.clamp(theme::MIN_FONT_SIZE, theme::MAX_FONT_SIZE);
        let new_size = match press.key.as_ref() {
            Key::Character("=" | "+") => zoom(size + 1.0),
            Key::Character("-" | "_") => zoom(size - 1.0),
            Key::Character("0" | ")") => theme::DEFAULT_FONT_SIZE,
            Key::Character("v" | "V") if shifted => {
                self.tabs.front_mut().tui.paste_from_clipboard();
                return true;
            }
            Key::Character("c" | "C") if shifted => {
                self.tabs.front_mut().tui.request_copy();
                return true;
            }
            Key::Character("t" | "T") if shifted => {
                self.tabs.new_tab();
                return true;
            }
            Key::Character("w" | "W") if shifted => {
                self.tabs.close(self.tabs.front_index());
                return true;
            }
            _ => return false,
        };
        if (new_size - size).abs() > f32::EPSILON {
            self.font_size = new_size;
            self.metrics = Metrics::on_display(new_size, self.scale);
            self.clear_selections();
        }
        true
    }

    /// One key, with the terminal's Ctrl+C and Ctrl+D rules. `true` quits.
    fn key_press(&mut self, key: KeyEvent) -> bool {
        let tui = &mut self.tabs.front_mut().tui;
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => {
                    if tui.request_copy() {
                        return false;
                    }
                    if !tui.is_busy() {
                        return true;
                    }
                    tui.cancel();
                    return false;
                }
                KeyCode::Char('d') => return true,
                _ => {}
            }
        }
        tui.handle_key(key).should_quit()
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{stage, Stage};
    use super::*;
    use aura_ui::events::UiEvent;

    const NONE: ModifiersState = ModifiersState::empty();
    const CTRL: ModifiersState = ModifiersState::CONTROL;
    const SHIFT: ModifiersState = ModifiersState::SHIFT;

    #[allow(clippy::needless_pass_by_value)]
    fn press(key: Key, text: Option<&str>, m: ModifiersState) -> Vec<(KeyCode, KeyModifiers)> {
        key_events(&key, text, m)
            .into_iter()
            .map(|e| (e.code, e.modifiers))
            .collect()
    }

    impl Stage {
        /// Press `press` with `held` down.
        #[allow(clippy::needless_pass_by_value)]
        pub fn key(&mut self, held: ModifiersState, press: Press) {
            self.shell.modifiers_changed(held);
            self.shell.key_pressed(&press, self.now);
            self.shell.modifiers_changed(NONE);
        }

        pub fn type_text(&mut self, text: &str) {
            let mut buffer = [0; 4];
            for ch in text.chars() {
                self.key(NONE, Press::typing(ch.encode_utf8(&mut buffer)));
            }
        }

        /// Pick the folder highlighted in the folder browser, as Space
        /// does, which asks the session to open it as a project.
        pub fn pick_folder(&mut self) {
            assert!(
                self.shell.tabs.front().tui.folder_picker().is_some(),
                "a folder browser is open"
            );
            self.key(NONE, Press::typing(" "));
            let asked = self.asked();
            assert!(
                asked.iter().any(|event| event.starts_with("CreateProject")),
                "{asked:?}"
            );
        }
    }

    #[test]
    fn typed_text_uses_the_layout_character() {
        let got = press(Key::Character("a".into()), Some("A"), SHIFT);
        assert_eq!(got, vec![(KeyCode::Char('A'), KeyModifiers::SHIFT)]);
    }

    #[test]
    fn control_chords_report_the_key() {
        let got = press(Key::Character("c".into()), Some("\u{3}"), CTRL);
        assert_eq!(got, vec![(KeyCode::Char('c'), KeyModifiers::CONTROL)]);
    }

    #[test]
    fn named_keys_map_to_terminal_codes() {
        assert_eq!(
            press(Key::Named(NamedKey::Enter), Some("\r"), NONE),
            vec![(KeyCode::Enter, KeyModifiers::NONE)]
        );
        assert_eq!(
            press(Key::Named(NamedKey::Tab), None, SHIFT),
            vec![(KeyCode::BackTab, KeyModifiers::SHIFT)]
        );
        assert_eq!(
            press(Key::Named(NamedKey::Space), Some(" "), NONE),
            vec![(KeyCode::Char(' '), KeyModifiers::NONE)]
        );
        assert!(press(Key::Named(NamedKey::Shift), None, SHIFT).is_empty());
    }

    #[test]
    fn composed_and_private_text() {
        // The second key of a dead-key pair types the composed character.
        assert_eq!(
            press(Key::Dead(None), Some("é"), NONE),
            vec![(KeyCode::Char('é'), KeyModifiers::NONE)]
        );
        assert!(press(Key::Dead(Some('´')), None, NONE).is_empty());
        // macOS gives the arrows text from the private use area.
        assert_eq!(typed(Some("\u{F700}")), None);
        assert_eq!(typed(Some("a")), Some("a"));
    }

    #[test]
    fn typing_reaches_the_input_and_enter_sends_it() {
        let mut stage = stage();
        stage.seed(false);
        stage.shell.frame(stage.now);
        stage.type_text("hello there");
        assert!(stage.shell.dirty());
        stage.shell.frame(stage.now);
        assert!(stage.shell.text().contains("hello there"));

        stage.key(NONE, Press::of(NamedKey::Enter));
        stage.shell.frame(stage.now);
        let mut sent = Vec::new();
        while let Ok(event) = stage.events.try_recv() {
            sent.push(event);
        }
        assert!(
            sent.iter().any(
                |event| matches!(event, UiEvent::UserMessage { text, .. } if text == "hello there")
            ),
            "{sent:?}"
        );
        assert!(stage.shell.take_effects().is_empty());
    }

    #[test]
    fn ctrl_up_and_down_reach_the_tui_input_ends() {
        assert_eq!(
            press(Key::Named(NamedKey::ArrowUp), None, CTRL),
            [(KeyCode::Up, KeyModifiers::CONTROL)]
        );
        let mut stage = stage();
        stage.seed(false);
        stage.shell.frame(stage.now);
        stage.type_text("first");
        stage.key(SHIFT, Press::of(NamedKey::Enter));
        stage.type_text("last");
        stage.key(CTRL, Press::of(NamedKey::ArrowUp));
        stage.type_text(">");
        stage.key(CTRL, Press::of(NamedKey::ArrowDown));
        stage.type_text("<");
        assert_eq!(stage.shell.tabs.front_mut().tui.input(), ">first\nlast<");
    }

    #[test]
    fn a_key_brings_the_cursor_back() {
        let mut stage = stage();
        stage.seed(false);
        stage.pass(super::super::BLINK);
        stage.shell.frame(stage.now);
        assert!(!stage.shell.cursor_shown, "half a blink in");
        stage.type_text("a");
        stage.shell.frame(stage.now);
        assert!(stage.shell.cursor_shown);
    }

    #[test]
    fn control_c_cancels_a_turn_then_quits_and_control_d_quits() {
        let mut stage = stage();
        stage.seed(false);
        stage.type_text("go");
        stage.key(NONE, Press::of(NamedKey::Enter));
        let _ = stage.asked();
        let ctrl_c = Press::chord("c", "c", "\u{3}");
        stage.key(CTRL, ctrl_c.clone());
        assert_eq!(stage.asked(), [r#"Cancel("a1")"#]);
        assert!(
            stage.shell.take_effects().is_empty(),
            "the agent was working"
        );
        stage.key(CTRL, ctrl_c);
        assert_eq!(stage.shell.take_effects(), [Effect::Exit]);

        let mut other = self::stage();
        other.key(CTRL, Press::chord("d", "d", "\u{4}"));
        assert_eq!(other.shell.take_effects(), [Effect::Exit]);
    }

    #[test]
    fn the_window_zooms_and_keeps_the_display_scale() {
        let mut stage = stage();
        stage
            .shell
            .resized(crate::geom::Size::new(2640.0, 1720.0), 2.0);
        stage.shell.frame(stage.now);
        let (size, cell, cols) = (
            stage.shell.font_size,
            stage.shell.metrics.cell,
            stage.shell.cols,
        );

        stage.key(CTRL, Press::typing("="));
        assert_eq!(stage.shell.font_size, size + 1.0);
        assert!(stage.shell.metrics.cell.height > cell.height);
        stage.shell.frame(stage.now);
        assert!(stage.shell.cols < cols, "bigger cells, fewer of them");

        stage.key(CTRL, Press::typing("-"));
        assert_eq!(stage.shell.metrics.cell, cell);
        for _ in 0..40 {
            stage.key(CTRL, Press::typing("-"));
        }
        assert_eq!(stage.shell.font_size, theme::MIN_FONT_SIZE);
        stage.key(CTRL, Press::typing("0"));
        assert_eq!(stage.shell.font_size, theme::DEFAULT_FONT_SIZE);
        // Nothing of it reached the UI's input.
        stage.shell.frame(stage.now);
        assert!(!stage.shell.text().contains('='));
    }

    #[test]
    fn tabs_open_close_and_switch_from_the_keyboard() {
        let mut stage = stage();
        stage.seed(false);
        let _ = stage.asked();
        let held = CTRL | SHIFT;
        stage.key(held, Press::typing("T"));
        assert!(stage.asked().is_empty(), "nothing until a folder is picked");
        stage.pick_folder();
        assert_eq!(stage.shell.tabs.len(), 1, "the tab opens with its project");
        stage.created("a3", "agent03");
        assert_eq!(stage.shell.tabs.len(), 2);
        assert_eq!(stage.shell.tabs.front_index(), 1);

        // Shift makes `1` a `!`: the number is the key's, not what it types.
        stage.key(held, Press::chord("1", "!", "!"));
        assert_eq!(stage.shell.tabs.front_index(), 0);
        stage.key(CTRL, Press::of(NamedKey::Tab));
        assert_eq!(stage.shell.tabs.front_index(), 1);
        stage.key(held, Press::of(NamedKey::Tab));
        assert_eq!(stage.shell.tabs.front_index(), 0);
        let _ = stage.asked();

        // Ctrl+W without Shift is the UI's.
        stage.key(CTRL, Press::chord("w", "w", "\u{17}"));
        assert_eq!(stage.shell.tabs.len(), 2);
        // Closing a tab hands its projects to the one before it.
        stage.key(held, Press::typing("W"));
        assert_eq!(stage.shell.tabs.len(), 1);
        stage.key(held, Press::typing("W"));
        assert_eq!(stage.shell.tabs.len(), 1, "the last tab stays");
        assert!(stage.shell.take_effects().is_empty());
    }
}
