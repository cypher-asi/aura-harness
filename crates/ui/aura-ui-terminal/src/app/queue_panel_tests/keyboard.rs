use super::*;

#[test]
fn keyboard_selects_moves_removes_and_edits_entries() {
    let (mut app, mut rx) = app_with_queue(three());

    key(&mut app, KeyCode::Up, KeyModifiers::SHIFT);
    assert_eq!(
        app.queue_selected(),
        Some(2),
        "Shift+Up lands on the newest"
    );
    key(&mut app, KeyCode::Up, KeyModifiers::empty());
    key(&mut app, KeyCode::Up, KeyModifiers::empty());
    key(&mut app, KeyCode::Up, KeyModifiers::empty());
    assert_eq!(app.queue_selected(), Some(0));

    key(&mut app, KeyCode::Down, KeyModifiers::ALT);
    assert_eq!(texts(&app), ["second", "first", "/compact"]);
    assert_eq!(app.queue_selected(), Some(1), "the selection follows");
    assert!(matches!(
        events(&mut rx).as_slice(),
        [UiEvent::MoveQueued { id: 1, to: 1, .. }]
    ));

    key(&mut app, KeyCode::Delete, KeyModifiers::empty());
    assert_eq!(texts(&app), ["second", "/compact"]);
    assert!(matches!(
        events(&mut rx).as_slice(),
        [UiEvent::RemoveQueued { id: 1, .. }]
    ));
    assert_eq!(
        app.queue_selected(),
        Some(1),
        "the next entry takes its place"
    );

    key(&mut app, KeyCode::Up, KeyModifiers::empty());
    key(&mut app, KeyCode::Enter, KeyModifiers::empty());
    assert!(app.editing_queued());
    assert_eq!(app.input(), "second");
    key(&mut app, KeyCode::Esc, KeyModifiers::empty());
    assert!(!app.editing_queued());
    assert_eq!(app.queue_selected(), None, "back in the input");
}

#[test]
fn keyboard_sends_an_entry_now_and_esc_goes_back_to_the_input() {
    let (mut app, mut rx) = app_with_queue(three());
    key(&mut app, KeyCode::Up, KeyModifiers::SHIFT);
    key(&mut app, KeyCode::Up, KeyModifiers::empty());
    key(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);
    assert!(matches!(
        events(&mut rx).as_slice(),
        [UiEvent::SendQueuedItemNow { id: 2, .. }]
    ));
    assert_eq!(app.queue_selected(), None);

    key(&mut app, KeyCode::Up, KeyModifiers::SHIFT);
    key(&mut app, KeyCode::Esc, KeyModifiers::empty());
    assert_eq!(app.queue_selected(), None);
    type_text(&mut app, "x");
    assert_eq!(app.input(), "x", "typing goes to the input again");
}

#[test]
fn bare_s_and_backspace_are_not_shortcuts_even_with_the_keys_on_the_panel() {
    let (mut app, mut rx) = app_with_queue(three());
    key(&mut app, KeyCode::Up, KeyModifiers::SHIFT);
    type_text(&mut app, "s");
    assert_eq!(app.input(), "s", "typed into the input");
    assert_eq!(app.queue_selected(), None);

    key(&mut app, KeyCode::Up, KeyModifiers::SHIFT);
    key(&mut app, KeyCode::Backspace, KeyModifiers::empty());
    assert_eq!(app.input(), "", "the input took the Backspace");
    assert_eq!(texts(&app), ["first", "second", "/compact"]);
    assert!(events(&mut rx).is_empty());
}

#[test]
fn typing_while_the_panel_has_the_keys_goes_back_to_the_input() {
    let (mut app, _rx) = app_with_queue(three());
    key(&mut app, KeyCode::Up, KeyModifiers::SHIFT);
    type_text(&mut app, "hi");
    assert_eq!(app.queue_selected(), None);
    assert_eq!(app.input(), "hi");
}

#[test]
fn shift_up_with_an_empty_queue_leaves_the_keys_in_the_input() {
    let (mut app, _rx) = app_with_queue(Vec::new());
    key(&mut app, KeyCode::Up, KeyModifiers::SHIFT);
    assert_eq!(app.queue_selected(), None);
}
