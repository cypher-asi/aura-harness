use super::*;

#[test]
fn the_selection_stays_on_its_entry_when_the_queue_changes() {
    let (mut app, mut rx) = app_with_queue(three());
    key(&mut app, KeyCode::Up, KeyModifiers::SHIFT);
    key(&mut app, KeyCode::Up, KeyModifiers::empty());
    assert_eq!(app.queue_selected(), Some(1), "on `second`");

    // `first` started running.
    app.process_command(UiCommand::SetQueue(vec![
        item(2, "second", true),
        item(3, "/compact", false),
    ]));
    assert_eq!(app.queue_selected(), Some(0), "still on `second`");
    key(&mut app, KeyCode::Delete, KeyModifiers::empty());
    assert!(matches!(
        events(&mut rx).as_slice(),
        [UiEvent::RemoveQueued { id: 2, .. }]
    ));
}

#[test]
fn a_selected_entry_that_leaves_hands_the_selection_to_its_place() {
    let (mut app, mut rx) = app_with_queue(three());
    key(&mut app, KeyCode::Up, KeyModifiers::SHIFT);
    key(&mut app, KeyCode::Up, KeyModifiers::empty());

    app.process_command(UiCommand::SetQueue(vec![
        item(1, "first", true),
        item(3, "/compact", false),
    ]));
    assert_eq!(app.queue_selected(), Some(1), "now on `/compact`");
    key(&mut app, KeyCode::Enter, KeyModifiers::CONTROL);
    assert!(matches!(
        events(&mut rx).as_slice(),
        [UiEvent::SendQueuedItemNow { id: 3, .. }]
    ));

    key(&mut app, KeyCode::Up, KeyModifiers::SHIFT);
    app.process_command(UiCommand::SetQueue(Vec::new()));
    assert_eq!(app.queue_selected(), None, "nothing left to select");
    type_text(&mut app, "x");
    assert_eq!(app.input(), "x");
}

#[test]
fn the_event_loops_queue_replaces_the_local_one() {
    let (mut app, _rx) = app_with_queue(three());
    let remove = middle(hover_row(&mut app, 1).remove);
    click(&mut app, remove);
    assert_eq!(texts(&app), ["second", "/compact"]);

    app.process_command(UiCommand::SetQueue(vec![item(2, "second", true)]));
    assert_eq!(texts(&app), ["second"]);
}

#[test]
fn queued_image_previews_move_to_an_edit_once_the_event_loop_has_it() {
    let (mut app, _rx) = app_with_queue(vec![
        item(1, "look [image 1]", true),
        item(2, "and [image 1]", true),
    ]);
    app.queued_images
        .push_back(("look [image 1]".to_string(), Vec::new()));
    app.queued_images
        .push_back(("and [image 1]".to_string(), Vec::new()));

    app.start_queue_edit(1);
    type_text(&mut app, " here");
    key(&mut app, KeyCode::Enter, KeyModifiers::empty());
    assert_eq!(
        preview_texts(&app),
        ["look [image 1]", "and [image 1]"],
        "not renamed before the event loop confirms"
    );
    // Another agent's queue says nothing about this edit.
    app.process_command(UiCommand::ForAgent {
        agent_id: "bb".to_string(),
        command: Box::new(UiCommand::SetQueue(Vec::new())),
    });
    // A queue sent before the edit arrived still has the old text.
    app.process_command(UiCommand::SetQueue(vec![
        item(1, "look [image 1]", true),
        item(2, "and [image 1]", true),
    ]));
    assert_eq!(preview_texts(&app), ["look [image 1]", "and [image 1]"]);
    app.process_command(UiCommand::SetQueue(vec![
        item(1, "look [image 1] here", true),
        item(2, "and [image 1]", true),
    ]));
    assert_eq!(
        preview_texts(&app),
        ["look [image 1] here", "and [image 1]"]
    );

    app.remove_queued(2);
    assert_eq!(preview_texts(&app), ["look [image 1] here"]);
}

#[test]
fn an_edit_the_event_loop_never_applied_keeps_the_previews_for_the_text_that_runs() {
    let (mut app, _rx) = app_with_queue(vec![item(1, "look [image 1]", true)]);
    app.queued_images
        .push_back(("look [image 1]".to_string(), Vec::new()));

    app.start_queue_edit(1);
    type_text(&mut app, " here");
    key(&mut app, KeyCode::Enter, KeyModifiers::empty());
    // The message had already started; its turn shows the old text.
    app.process_command(UiCommand::SetQueue(Vec::new()));
    assert_eq!(preview_texts(&app), ["look [image 1]"]);
    app.process_command(UiCommand::TurnStarted {
        prompt: Some("look [image 1]".to_string()),
    });
    assert!(preview_texts(&app).is_empty(), "the chat took them");
}

#[test]
fn removing_an_unconfirmed_edit_forgets_the_previews_under_the_old_text() {
    let (mut app, _rx) = app_with_queue(vec![item(1, "look [image 1]", true)]);
    app.queued_images
        .push_back(("look [image 1]".to_string(), Vec::new()));
    app.start_queue_edit(1);
    type_text(&mut app, " here");
    key(&mut app, KeyCode::Enter, KeyModifiers::empty());
    app.remove_queued(1);
    assert!(preview_texts(&app).is_empty());
}

#[test]
fn an_edit_whose_message_already_ran_is_sent_as_new_and_then_the_draft_returns() {
    let (mut app, mut rx) = app_with_queue(three());
    type_text(&mut app, "draft ");
    app.attach_image(PNG).expect("image attached");
    let draft = app.input().to_string();
    assert_eq!(app.attached_images(), 1);

    app.start_queue_edit(1);
    type_text(&mut app, "!");
    app.process_command(UiCommand::SetQueue(vec![item(2, "second", true)]));
    key(&mut app, KeyCode::Enter, KeyModifiers::empty());

    assert!(!app.editing_queued());
    assert_eq!(app.input(), "first!", "the edited text waits to be sent");
    assert_eq!(app.attached_images(), 0, "the draft's image is not on it");
    assert!(app.notification().is_some());
    assert!(events(&mut rx).is_empty());

    key(&mut app, KeyCode::Enter, KeyModifiers::empty());
    let sent = events(&mut rx);
    assert!(
        matches!(
            sent.as_slice(),
            [UiEvent::UserMessage { text, images, .. }] if text == "first!" && images.is_empty()
        ),
        "{sent:?}"
    );
    assert_eq!(app.input(), draft, "the draft comes back after the send");
    assert_eq!(app.attached_images(), 1, "with its image");
}

#[test]
fn esc_after_an_edit_of_a_message_that_ran_brings_the_draft_back_at_once() {
    let (mut app, mut rx) = app_with_queue(three());
    type_text(&mut app, "draft ");
    app.attach_image(PNG).expect("image attached");
    let draft = app.input().to_string();

    app.start_queue_edit(1);
    type_text(&mut app, "!");
    app.process_command(UiCommand::SetQueue(Vec::new()));
    key(&mut app, KeyCode::Enter, KeyModifiers::empty());
    key(&mut app, KeyCode::Esc, KeyModifiers::empty());

    assert_eq!(app.input(), draft);
    assert_eq!(app.attached_images(), 1);
    assert!(events(&mut rx).is_empty());
    // The edited text went into the history rather than away.
    app.input.text.clear();
    app.input.cursor_pos = 0;
    key(&mut app, KeyCode::Up, KeyModifiers::empty());
    assert_eq!(app.input(), "first!");
}

#[test]
fn switching_agents_cancels_an_edit() {
    let (mut app, _rx) = app_with_queue(three());
    type_text(&mut app, "draft");
    app.start_queue_edit(1);
    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));

    assert!(!app.editing_queued());
    assert_eq!(app.input(), "", "the draft stays with aa");
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    assert_eq!(
        app.input(),
        "draft",
        "aa gets back the draft the edit set aside, not the queued text"
    );
}
