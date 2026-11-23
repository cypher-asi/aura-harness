//! Tests of the queued-messages panel: mouse, keyboard and editing.

use super::queue_panel::QueueRowArea;
use super::App;
use crate::events::{AgentSummary, QueuedItemView, UiCommand, UiEvent};
use crate::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use tokio::sync::mpsc;

/// Enough of a PNG for the input to take it as one.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nimage";

fn item(id: u64, text: &str, editable: bool) -> QueuedItemView {
    QueuedItemView {
        id,
        text: text.to_string(),
        editable,
    }
}

fn agent(id: &str) -> AgentSummary {
    AgentSummary {
        id: id.to_string(),
        name: id.to_uppercase(),
        ..Default::default()
    }
}

/// An app showing agent `aa` with `items` queued, drawn once so the
/// panel's areas are known.
fn app_with_queue(items: Vec<QueuedItemView>) -> (App, mpsc::Receiver<UiEvent>) {
    let (tx, rx) = mpsc::channel(16);
    let mut app = App::new().with_event_sender(tx);
    app.process_command(UiCommand::SetAgents(vec![agent("aa"), agent("bb")]));
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app.process_command(UiCommand::SetQueue(items));
    draw(&mut app);
    (app, rx)
}

fn three() -> Vec<QueuedItemView> {
    vec![
        item(1, "first", true),
        item(2, "second", true),
        item(3, "/compact", false),
    ]
}

fn draw(app: &mut App) {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
    terminal
        .draw(|frame| crate::renderer::render(frame, app, &Theme::cyber()))
        .unwrap();
}

fn mouse(app: &mut App, kind: MouseEventKind, (column, row): (u16, u16)) {
    app.handle_mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::empty(),
    });
}

fn click(app: &mut App, at: (u16, u16)) {
    mouse(app, MouseEventKind::Moved, at);
    mouse(app, MouseEventKind::Down(MouseButton::Left), at);
    mouse(app, MouseEventKind::Up(MouseButton::Left), at);
}

fn key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    app.handle_key(KeyEvent::new(code, modifiers));
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        key(app, KeyCode::Char(c), KeyModifiers::empty());
    }
}

/// Where entry `id` was drawn in the last frame.
fn row_area(app: &App, id: u64) -> QueueRowArea {
    *app.queue_areas()
        .rows
        .iter()
        .find(|area| area.id == id)
        .unwrap_or_else(|| panic!("entry {id} is not on screen"))
}

/// Point at entry `id` and draw, so its actions are on screen.
fn hover_row(app: &mut App, id: u64) -> QueueRowArea {
    let row = row_area(app, id).row;
    mouse(app, MouseEventKind::Moved, (row.x + 2, row.y));
    draw(app);
    row_area(app, id)
}

fn middle(rect: Option<Rect>) -> (u16, u16) {
    let rect = rect.expect("the part is drawn");
    (rect.x + rect.width / 2, rect.y)
}

fn texts(app: &App) -> Vec<&str> {
    app.queue().iter().map(|item| item.text.as_str()).collect()
}

fn events(rx: &mut mpsc::Receiver<UiEvent>) -> Vec<UiEvent> {
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

fn preview_texts(app: &App) -> Vec<&str> {
    app.queued_images.iter().map(|(t, _)| t.as_str()).collect()
}

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
    app.input.clear();
    app.cursor_pos = 0;
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
