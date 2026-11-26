use super::*;

#[test]
fn clicking_remove_drops_the_entry_and_tells_the_event_loop() {
    let (mut app, mut rx) = app_with_queue(three());
    let remove = middle(hover_row(&mut app, 2).remove);
    click(&mut app, remove);

    assert_eq!(texts(&app), ["first", "/compact"], "removed at once");
    assert!(matches!(
        events(&mut rx).as_slice(),
        [UiEvent::RemoveQueued { agent_id, id: 2 }] if agent_id == "aa"
    ));
}

#[test]
fn clicking_send_now_moves_the_entry_to_the_front_and_runs_it() {
    let (mut app, mut rx) = app_with_queue(three());
    let send = middle(hover_row(&mut app, 3).send_now);
    click(&mut app, send);

    assert_eq!(texts(&app), ["/compact", "first", "second"]);
    assert!(matches!(
        events(&mut rx).as_slice(),
        [UiEvent::SendQueuedItemNow { agent_id, id: 3 }] if agent_id == "aa"
    ));
}

#[test]
fn a_press_where_hidden_actions_would_be_is_a_press_on_the_row() {
    let (mut app, mut rx) = app_with_queue(three());
    let shown = hover_row(&mut app, 2);
    let remove = middle(shown.remove);
    // The pointer moves off the panel and the frame drops the actions.
    mouse(&mut app, MouseEventKind::Moved, (0, 0));
    draw(&mut app);
    assert!(row_area(&app, 2).remove.is_none());

    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), remove);
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), remove);
    assert_eq!(texts(&app), ["first", "second", "/compact"]);
    assert!(events(&mut rx).is_empty(), "nothing was removed");
}

#[test]
fn a_click_acts_on_the_entry_drawn_there_even_if_the_queue_changed() {
    let (mut app, mut rx) = app_with_queue(three());
    let remove = middle(hover_row(&mut app, 2).remove);
    // A new entry lands ahead of it before the next frame.
    app.process_command(UiCommand::SetQueue(vec![
        item(1, "first", true),
        item(9, "newcomer", true),
        item(2, "second", true),
        item(3, "/compact", false),
    ]));
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), remove);

    assert_eq!(texts(&app), ["first", "newcomer", "/compact"]);
    assert!(matches!(
        events(&mut rx).as_slice(),
        [UiEvent::RemoveQueued { id: 2, .. }]
    ));
}

#[test]
fn dragging_the_second_row_onto_the_first_moves_it_to_the_front() {
    let (mut app, mut rx) = app_with_queue(three());
    let from = row_area(&app, 2).row;
    let onto = row_area(&app, 1).row;
    let grab = (from.x + 4, from.y);
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), grab);
    mouse(
        &mut app,
        MouseEventKind::Drag(MouseButton::Left),
        (grab.0, onto.y),
    );
    let drag = app.queue_drag().expect("a drag is on");
    assert_eq!((drag.from, drag.to), (1, 0));
    assert!(
        events(&mut rx).is_empty(),
        "nothing moves before the release"
    );
    mouse(
        &mut app,
        MouseEventKind::Up(MouseButton::Left),
        (grab.0, onto.y),
    );

    assert_eq!(texts(&app), ["second", "first", "/compact"]);
    assert!(matches!(
        events(&mut rx).as_slice(),
        [UiEvent::MoveQueued { id: 2, to: 0, .. }]
    ));
    assert!(app.queue_drag().is_none());
}

#[test]
fn a_click_on_a_row_leaves_the_keys_in_the_input() {
    let (mut app, mut rx) = app_with_queue(three());
    let row = row_area(&app, 2).row;
    click(&mut app, (row.x + 4, row.y));

    assert!(events(&mut rx).is_empty());
    assert_eq!(texts(&app), ["first", "second", "/compact"]);
    assert_eq!(app.queue_selected(), None, "a click does not take the keys");

    // Letters that are shortcuts in the panel are typed into the input.
    type_text(&mut app, "send e");
    key(&mut app, KeyCode::Backspace, KeyModifiers::empty());
    key(&mut app, KeyCode::Delete, KeyModifiers::empty());
    assert_eq!(app.input(), "send ");
    assert_eq!(texts(&app), ["first", "second", "/compact"]);
    assert!(events(&mut rx).is_empty(), "no queue action ran");
}

#[test]
fn a_lost_release_does_not_turn_a_later_click_into_a_move() {
    let (mut app, mut rx) = app_with_queue(three());
    let second = row_area(&app, 2).row;
    let first = row_area(&app, 1).row;
    mouse(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        (second.x + 4, second.y),
    );
    // The release happened outside the terminal; the next press is in
    // the input, then the pointer drags over the first row.
    let input = app.areas.input.expect("input drawn");
    mouse(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        (input.x + 2, input.y + 1),
    );
    mouse(
        &mut app,
        MouseEventKind::Drag(MouseButton::Left),
        (first.x + 4, first.y),
    );
    assert!(app.queue_drag().is_none());
    mouse(
        &mut app,
        MouseEventKind::Up(MouseButton::Left),
        (first.x + 4, first.y),
    );
    assert_eq!(texts(&app), ["first", "second", "/compact"]);
    assert!(events(&mut rx).is_empty());
}

#[test]
fn a_press_ends_on_a_plain_move_a_dropped_entry_or_an_agent_switch() {
    let drag_onto_first = |app: &mut App| {
        let first = row_area(app, 1).row;
        mouse(
            app,
            MouseEventKind::Drag(MouseButton::Left),
            (first.x + 4, first.y),
        );
        app.queue_drag()
    };
    let press_second = |app: &mut App| {
        let second = row_area(app, 2).row;
        mouse(
            app,
            MouseEventKind::Down(MouseButton::Left),
            (second.x + 4, second.y),
        );
    };

    let (mut app, _rx) = app_with_queue(three());
    press_second(&mut app);
    mouse(&mut app, MouseEventKind::Moved, (0, 0));
    assert!(drag_onto_first(&mut app).is_none(), "a move has no button");

    press_second(&mut app);
    mouse(&mut app, MouseEventKind::Drag(MouseButton::Right), (0, 0));
    assert!(drag_onto_first(&mut app).is_none(), "only the left drags");

    press_second(&mut app);
    app.process_command(UiCommand::SetQueue(vec![
        item(1, "first", true),
        item(3, "/compact", false),
    ]));
    assert!(drag_onto_first(&mut app).is_none(), "the entry is gone");

    let (mut app, _rx) = app_with_queue(three());
    press_second(&mut app);
    app.process_command(UiCommand::SetActiveAgent("bb".to_string()));
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    draw(&mut app);
    assert!(drag_onto_first(&mut app).is_none(), "the switch ended it");
}

#[test]
fn clicking_the_header_folds_and_opens_the_panel() {
    let (mut app, _rx) = app_with_queue(three());
    let header = app.queue_areas().header.expect("header drawn");
    click(&mut app, (header.x + 2, header.y));
    assert!(app.queue_collapsed());
    draw(&mut app);
    assert!(app.queue_areas().rows.is_empty(), "folded shows no rows");

    // The folded panel is shorter, so its header moved down.
    let header = app.queue_areas().header.expect("header drawn");
    click(&mut app, (header.x + 2, header.y));
    assert!(!app.queue_collapsed());
}
