//! The modal on top: a dialog outlives the turn under it, an approval
//! stands over a dialog, and a press off the modal backs out of it
//! without reaching what is underneath.

use super::super::channel_form::ChannelField;
use super::*;
use crate::components::ModalArea;

/// A modal's box in the middle of the screen the agent areas assume,
/// clear of the agent rows on its left.
fn box_in_the_middle() -> ModalArea {
    ModalArea {
        frame: Rect::new(30, 4, 40, 8),
        body: Rect::new(33, 6, 34, 4),
    }
}

fn press(app: &mut App, column: u16, row: u16) {
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), column, row));
}

fn release(app: &mut App, column: u16, row: u16) {
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), column, row));
}

/// The slug typed into the open channel form.
fn form_slug(app: &App) -> Option<String> {
    app.channel_form()
        .map(|form| form.text(ChannelField::Slug).to_string())
}

#[test]
fn a_dialog_opened_mid_turn_outlives_the_turn() {
    let (mut app, _rx) = app_with_models(None);
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app.process_command(UiCommand::TurnStarted { prompt: None });
    app.handle_command("/model");
    assert_eq!(app.state(), AppState::SelectingModel);

    app.process_command(UiCommand::Complete);
    assert_eq!(
        app.state(),
        AppState::SelectingModel,
        "the turn's end leaves it be"
    );
    assert!(!app.is_busy());
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.state(), AppState::Idle);
}

#[test]
fn closing_a_dialog_mid_turn_goes_back_to_the_turn() {
    let mut app = App::new();
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app.process_command(UiCommand::TurnStarted { prompt: None });
    app.handle_command("/help");
    assert_eq!(app.state(), AppState::ShowingHelp);
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.state(), AppState::Processing);
    assert!(app.is_busy());
}

#[test]
fn an_approval_stands_over_the_channel_form_which_is_back_once_answered() {
    let (tx, mut rx) = mpsc::channel(10);
    let mut app = App::new().with_event_sender(tx);
    app.start_new_channel();
    type_keys(&mut app, "zode");
    app.process_command(approval_request("req-1", ""));
    assert_eq!(app.modal(), Some(ModalKind::Approval));
    let text = drawn_text(&mut app);
    assert!(text.contains("APPROVAL REQUIRED"), "{text}");
    assert!(
        !text.contains("NEW CHANNEL"),
        "only the one on top shows:\n{text}"
    );

    app.handle_key(key(KeyCode::Char('y')));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::Approve(id)) if id == "req-1"));
    assert_eq!(app.state(), AppState::ChannelForm);
    assert_eq!(form_slug(&app).as_deref(), Some("zode"), "as it was left");
}

#[test]
fn keys_answer_an_approval_over_a_view_on_the_chat() {
    let (tx, mut rx) = mpsc::channel(10);
    let mut app = App::new().with_event_sender(tx);
    app.open_detail(DetailView::Graph);
    app.process_command(approval_request("req-1", ""));
    app.handle_key(key(KeyCode::Char('y')));
    assert!(matches!(rx.try_recv(), Ok(UiEvent::Approve(id)) if id == "req-1"));
    assert!(app.detail_view().is_some(), "the view under it stays");
}

#[test]
fn a_press_off_the_approval_denies_it_and_nothing_under_it_takes_the_click() {
    let (mut app, mut rx) = app_with_agent_areas();
    let selected = app.selected_agent();
    app.process_command(approval_request("req-1", ""));
    app.set_modal_area(Some(box_in_the_middle()));

    // Agent `bb`'s row is on line 8, under the backdrop.
    press(&mut app, 5, 8);
    assert!(matches!(rx.try_recv(), Ok(UiEvent::Deny(id)) if id == "req-1"));
    release(&mut app, 5, 8);
    assert!(rx.try_recv().is_err(), "the row under it is not clicked");
    assert_eq!(app.selected_agent(), selected);
    assert!(app.modal().is_none());

    press(&mut app, 5, 8);
    assert!(
        matches!(rx.try_recv(), Ok(UiEvent::SelectAgent(id)) if id == "bb"),
        "with nothing over it, the row takes the press"
    );
}

#[test]
fn a_press_off_the_modals_closes_one_at_a_time() {
    let (tx, mut rx) = mpsc::channel(10);
    let mut app = App::new().with_event_sender(tx);
    app.start_new_channel();
    type_keys(&mut app, "zode");
    app.process_command(approval_request("req-1", ""));
    app.set_modal_area(Some(box_in_the_middle()));

    press(&mut app, 0, 0);
    release(&mut app, 0, 0);
    assert!(matches!(rx.try_recv(), Ok(UiEvent::Deny(id)) if id == "req-1"));
    assert_eq!(
        app.state(),
        AppState::ChannelForm,
        "the form under it stays"
    );
    assert_eq!(form_slug(&app).as_deref(), Some("zode"));

    app.set_modal_area(Some(box_in_the_middle()));
    press(&mut app, 0, 0);
    assert!(app.channel_form().is_none(), "then the form is cancelled");
    assert_eq!(app.state(), AppState::Processing);
    assert!(rx.try_recv().is_err(), "and sends nothing");
}

#[test]
fn a_press_off_any_dialog_closes_it_and_one_inside_does_not() {
    type Open = fn(&mut App, &std::path::Path);
    let dir = tempfile::tempdir().unwrap();
    let dialogs: [(&str, Open); 5] = [
        ("help", |app, _| {
            app.handle_command("/help");
        }),
        ("model selector", |app, _| {
            app.handle_command("/model");
        }),
        ("folder picker", open_folder_picker),
        ("project info", |app, _| {
            app.process_command(UiCommand::SetProjects(vec![project_summary("p1", "alpha")]));
            app.show_project_info("p1");
        }),
        ("channel form", |app, _| app.start_new_channel()),
    ];
    for (name, open) in dialogs {
        let (mut app, mut rx) = app_with_models(None);
        open(&mut app, dir.path());
        assert!(app.dialog().is_some(), "{name} opens");
        drawn_text(&mut app);
        let frame = app.modal_area().expect("the dialog is drawn").frame;

        press(&mut app, frame.x + 1, frame.y + 1);
        release(&mut app, frame.x + 1, frame.y + 1);
        assert!(app.dialog().is_some(), "{name}: a press inside keeps it");
        press(&mut app, 0, 0);
        assert!(app.dialog().is_none(), "{name}: a press off it closes it");
        assert_eq!(app.state(), AppState::Idle, "{name}");
        assert!(rx.try_recv().is_err(), "{name} sends nothing");
    }
}

#[test]
fn the_wheel_scrolls_the_help_over_it_and_does_nothing_off_it() {
    let mut app = App::new();
    app.handle_command("/help");
    drawn_text(&mut app);
    let frame = app.modal_area().expect("the help is drawn").frame;

    app.handle_mouse(mouse(MouseEventKind::ScrollDown, 0, 0));
    assert_eq!(app.help_scroll(), 0);
    assert_eq!(
        app.state(),
        AppState::ShowingHelp,
        "the wheel does not close it"
    );
    app.handle_mouse(mouse(MouseEventKind::ScrollDown, frame.x + 2, frame.y + 2));
    assert_eq!(app.help_scroll(), 3);
}

#[test]
fn the_help_scrolls_to_its_end_and_back_and_other_keys_close_it() {
    let mut app = App::new();
    app.handle_command("/help");
    let text = drawn_text(&mut app);
    assert!(
        text.contains("COMMANDS") && !text.contains("IN DIALOGS"),
        "{text}"
    );

    app.handle_key(key(KeyCode::End));
    let text = drawn_text(&mut app);
    assert!(
        text.contains("IN DIALOGS") && !text.contains("COMMANDS"),
        "{text}"
    );
    app.handle_key(key(KeyCode::PageUp));
    app.handle_key(key(KeyCode::Home));
    let text = drawn_text(&mut app);
    assert!(text.contains("COMMANDS"), "{text}");
    assert_eq!(
        app.state(),
        AppState::ShowingHelp,
        "scrolling keeps it open"
    );

    app.handle_key(key(KeyCode::Char('q')));
    assert_eq!(app.state(), AppState::Idle);
}

#[test]
fn a_paste_goes_into_the_channel_form_and_no_other_dialog_takes_one() {
    let mut app = App::new();
    app.start_new_channel();
    app.handle_paste("zode");
    assert_eq!(form_slug(&app).as_deref(), Some("zode"));
    assert!(app.input().is_empty(), "not into the input under it");

    app.handle_key(key(KeyCode::Esc));
    app.handle_command("/help");
    app.handle_paste("hello");
    assert!(app.input().is_empty());
    assert_eq!(app.state(), AppState::ShowingHelp);
}
