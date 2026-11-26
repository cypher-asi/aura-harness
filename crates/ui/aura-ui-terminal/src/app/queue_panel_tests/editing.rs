use super::*;

#[test]
fn editing_with_the_pencil_saves_the_text_back_and_restores_the_draft() {
    let (mut app, mut rx) = app_with_queue(three());
    type_text(&mut app, "draft");
    let edit = middle(hover_row(&mut app, 2).edit);
    click(&mut app, edit);

    assert!(app.editing_queued());
    assert_eq!(app.input(), "second");
    type_text(&mut app, " again");
    key(&mut app, KeyCode::Enter, KeyModifiers::empty());

    assert!(!app.editing_queued());
    assert_eq!(app.input(), "draft", "the draft comes back");
    assert_eq!(texts(&app), ["first", "second again", "/compact"]);
    assert!(matches!(
        events(&mut rx).as_slice(),
        [UiEvent::EditQueued { agent_id, id: 2, text }]
            if agent_id == "aa" && text == "second again"
    ));
}

#[test]
fn esc_cancels_an_edit_and_restores_the_draft() {
    let (mut app, mut rx) = app_with_queue(three());
    type_text(&mut app, "draft");
    let edit = middle(hover_row(&mut app, 1).edit);
    click(&mut app, edit);
    type_text(&mut app, " changed");
    key(&mut app, KeyCode::Esc, KeyModifiers::empty());

    assert!(!app.editing_queued());
    assert_eq!(app.input(), "draft");
    assert_eq!(texts(&app), ["first", "second", "/compact"]);
    assert!(events(&mut rx).is_empty());
}

#[test]
fn work_that_is_not_a_message_has_no_pencil_and_cannot_be_edited() {
    let (mut app, mut rx) = app_with_queue(three());
    let label = hover_row(&mut app, 3);
    assert!(label.send_now.is_some());
    assert!(label.edit.is_none());

    key(&mut app, KeyCode::Up, KeyModifiers::SHIFT);
    assert_eq!(app.queue_selected(), Some(2));
    key(&mut app, KeyCode::Enter, KeyModifiers::empty());
    assert!(!app.editing_queued());
    assert!(app.notification().is_some());
    assert!(events(&mut rx).is_empty());
}
