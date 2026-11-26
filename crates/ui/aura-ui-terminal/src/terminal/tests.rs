use super::*;
use crossterm::event::KeyEvent;

#[test]
fn test_key_result() {
    let result = KeyResult::continue_running();
    assert!(!result.should_quit());

    let result = KeyResult::quit();
    assert!(result.should_quit());
}

#[test]
fn kitty_control_letters_map_back_to_their_legacy_keys() {
    let ctrl = KeyModifiers::CONTROL;
    let cases = [
        (
            (KeyCode::Char('['), ctrl),
            (KeyCode::Esc, KeyModifiers::NONE),
        ),
        (
            (KeyCode::Char('m'), ctrl),
            (KeyCode::Enter, KeyModifiers::NONE),
        ),
        (
            (KeyCode::Char('i'), ctrl),
            (KeyCode::Tab, KeyModifiers::NONE),
        ),
        // Anything else, or with more than Control held, is left alone.
        ((KeyCode::Char('j'), ctrl), (KeyCode::Char('j'), ctrl)),
        (
            (KeyCode::Char('m'), ctrl | KeyModifiers::SHIFT),
            (KeyCode::Char('m'), ctrl | KeyModifiers::SHIFT),
        ),
        (
            (KeyCode::Char('i'), ctrl | KeyModifiers::ALT),
            (KeyCode::Char('i'), ctrl | KeyModifiers::ALT),
        ),
        (
            (KeyCode::Char('m'), KeyModifiers::NONE),
            (KeyCode::Char('m'), KeyModifiers::NONE),
        ),
        ((KeyCode::Esc, ctrl), (KeyCode::Esc, ctrl)),
    ];
    for ((code, modifiers), (want_code, want_modifiers)) in cases {
        let key = legacy_control_key(KeyEvent::new(code, modifiers));
        assert_eq!(
            (key.code, key.modifiers),
            (want_code, want_modifiers),
            "{code:?} {modifiers:?}"
        );
        assert_eq!(key.kind, KeyEventKind::Press);
    }
}
