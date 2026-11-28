//! The wheel in the pickers' dialogs scrolls their lists and leaves the
//! highlight where it is, as over the panels' lists; keys still move it.

use super::*;
use crate::app::folder_picker::{FolderPicker, FolderRow};
use crossterm::event::MouseEventKind;

/// `/model` over `count` ready local models, the cursor on the first.
fn model_selector(count: usize) -> App {
    let options = (0..count)
        .map(|i| ModelOptionView {
            id: format!("local/m{i:02}"),
            label: format!("Model {i:02}"),
            local: true,
            installed: true,
            group: "Local".to_string(),
            provider: None,
            available: true,
            context_window: None,
        })
        .collect();
    let mut app = App::new();
    app.process_command(UiCommand::SetModels {
        options,
        current: "local/m00".to_string(),
        route: None,
        user: None,
        plans: Vec::new(),
        thinking: None,
    });
    app.handle_command("/model");
    assert_eq!(app.state(), AppState::SelectingModel);
    app
}

/// A notch of the wheel down, or up, in the middle of the dialog drawn.
fn wheel(app: &mut App, down: bool) {
    let frame = app.modal_area().expect("the dialog is drawn").frame;
    let kind = if down {
        MouseEventKind::ScrollDown
    } else {
        MouseEventKind::ScrollUp
    };
    app.handle_mouse(mouse(kind, frame.x + 4, frame.y + frame.height / 2));
}

#[test]
fn the_wheel_scrolls_the_model_selector_without_moving_its_cursor() {
    let mut app = model_selector(60);
    let screen = drawn_text(&mut app);
    assert!(screen.contains("Model 00") && !screen.contains("Model 40"));
    let first = app.model_picker().scroll();

    wheel(&mut app, true);
    wheel(&mut app, true);
    let screen = drawn_text(&mut app);
    assert_eq!(app.model_picker().cursor(), 0, "the cursor stays put");
    assert_eq!(app.model_picker().scroll(), first + 6, "the list scrolled");
    assert!(!screen.contains("Model 00"), "scrolled off: {screen}");
    assert!(screen.contains("Model 06"), "{screen}");
    wheel(&mut app, false);
    drawn_text(&mut app);
    assert_eq!(app.model_picker().scroll(), first + 3);
    assert_eq!(app.model_picker().cursor(), 0);

    // A key moves the cursor, and the list follows it again.
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let screen = drawn_text(&mut app);
    assert_eq!(app.model_picker().cursor(), 1);
    assert!(screen.contains("Model 01"), "back on the cursor: {screen}");
    assert!(screen.contains("Model 00"), "{screen}");
}

#[test]
fn the_wheel_stops_at_the_model_selectors_ends() {
    let mut app = model_selector(60);
    drawn_text(&mut app);
    wheel(&mut app, false);
    drawn_text(&mut app);
    assert_eq!(app.model_picker().scroll(), 0, "never above the top");
    for _ in 0..40 {
        wheel(&mut app, true);
        drawn_text(&mut app);
    }
    let screen = drawn_text(&mut app);
    assert!(screen.contains("Model 59"), "the last row: {screen}");
    let last = app.model_picker().scroll();
    wheel(&mut app, true);
    drawn_text(&mut app);
    assert_eq!(app.model_picker().scroll(), last, "never past the end");
    assert_eq!(app.model_picker().cursor(), 0);
}

#[test]
fn a_click_on_a_wheeled_model_selector_picks_the_model_under_it() {
    let (tx, mut rx) = mpsc::channel(64);
    let mut app = model_selector(60).with_event_sender(tx);
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).expect("backend");
    draw(&mut app, &mut terminal);
    wheel(&mut app, true);
    draw(&mut app, &mut terminal);
    let (column, row) = cell_of(&terminal, "Model 05").expect("scrolled into view");
    app.handle_mouse(mouse(
        MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column,
        row,
    ));
    assert_eq!(app.state(), AppState::Idle, "picked, closed");
    let picked = std::iter::from_fn(|| rx.try_recv().ok()).find_map(|event| match event {
        UiEvent::SelectModel(id) => Some(id),
        _ => None,
    });
    assert_eq!(picked.as_deref(), Some("local/m05"));
}

/// The folder browser over a folder holding `count` folders.
fn folder_browser(count: usize) -> (App, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    for i in 0..count {
        std::fs::create_dir(dir.path().join(format!("dir{i:02}"))).expect("mkdir");
    }
    let mut app = App::new();
    open_folder_picker(&mut app, dir.path());
    (app, dir)
}

fn browser(app: &App) -> &FolderPicker {
    app.folder_picker().expect("the browser is open")
}

#[test]
fn the_wheel_scrolls_the_folder_browser_without_moving_its_highlight() {
    let (mut app, _dir) = folder_browser(60);
    let screen = drawn_text(&mut app);
    assert!(screen.contains("Use this folder"), "{screen}");
    let highlight = browser(&app).selected();

    wheel(&mut app, true);
    wheel(&mut app, true);
    let screen = drawn_text(&mut app);
    assert_eq!(browser(&app).selected(), highlight, "the highlight stays");
    assert_eq!(browser(&app).scroll(), 6, "the list scrolled");
    assert!(
        !screen.contains("Use this folder"),
        "scrolled off: {screen}"
    );

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let screen = drawn_text(&mut app);
    let moved = browser(&app).selected();
    assert_eq!(moved, highlight + 1, "the key moves it");
    assert_eq!(browser(&app).scroll(), moved, "and the list shows it again");
    assert!(screen.contains(".."), "the row moved to is back: {screen}");
}

#[test]
fn keys_still_move_through_and_pick_in_a_wheeled_folder_browser() {
    let (mut app, _dir) = folder_browser(60);
    drawn_text(&mut app);
    wheel(&mut app, true);
    drawn_text(&mut app);
    app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    let screen = drawn_text(&mut app);
    let last = browser(&app).rows().len() - 1;
    assert_eq!(browser(&app).selected(), last);
    assert!(
        matches!(
            browser(&app).rows()[last],
            FolderRow::Folder(_) | FolderRow::More(_)
        ),
        "{:?}",
        browser(&app).rows()[last]
    );
    let shown = browser(&app).scroll();
    assert!(
        shown > 6,
        "the list followed the highlight to the end: {screen}"
    );
}

#[test]
fn a_reopened_picker_shows_its_highlight_whatever_the_wheel_did_before() {
    let mut app = model_selector(60);
    drawn_text(&mut app);
    wheel(&mut app, true);
    wheel(&mut app, true);
    drawn_text(&mut app);
    // Closed by a press off the dialog, which is not a key.
    app.handle_mouse(mouse(
        MouseEventKind::Down(crossterm::event::MouseButton::Left),
        0,
        0,
    ));
    assert_eq!(app.state(), AppState::Idle);
    app.handle_command("/model");
    let screen = drawn_text(&mut app);
    assert!(screen.contains("Model 00"), "opens on its cursor: {screen}");
}
