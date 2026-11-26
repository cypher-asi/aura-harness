use super::*;

pub(super) fn item(id: u64, text: &str, editable: bool) -> QueuedItemView {
    QueuedItemView {
        id,
        text: text.to_string(),
        editable,
    }
}

pub(super) fn agent(id: &str) -> AgentSummary {
    AgentSummary {
        id: id.to_string(),
        name: id.to_uppercase(),
        ..Default::default()
    }
}

/// An app showing agent `aa` with `items` queued, drawn once so the
/// panel's areas are known.
pub(super) fn app_with_queue(items: Vec<QueuedItemView>) -> (App, mpsc::Receiver<UiEvent>) {
    let (tx, rx) = mpsc::channel(16);
    let mut app = App::new().with_event_sender(tx);
    app.process_command(UiCommand::SetAgents(vec![agent("aa"), agent("bb")]));
    app.process_command(UiCommand::SetActiveAgent("aa".to_string()));
    app.process_command(UiCommand::SetQueue(items));
    draw(&mut app);
    (app, rx)
}

pub(super) fn three() -> Vec<QueuedItemView> {
    vec![
        item(1, "first", true),
        item(2, "second", true),
        item(3, "/compact", false),
    ]
}

pub(super) fn draw(app: &mut App) {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
    terminal
        .draw(|frame| crate::renderer::render(frame, app, &Theme::cyber()))
        .unwrap();
}

pub(super) fn mouse(app: &mut App, kind: MouseEventKind, (column, row): (u16, u16)) {
    app.handle_mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::empty(),
    });
}

pub(super) fn click(app: &mut App, at: (u16, u16)) {
    mouse(app, MouseEventKind::Moved, at);
    mouse(app, MouseEventKind::Down(MouseButton::Left), at);
    mouse(app, MouseEventKind::Up(MouseButton::Left), at);
}

pub(super) fn key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    app.handle_key(KeyEvent::new(code, modifiers));
}

pub(super) fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        key(app, KeyCode::Char(c), KeyModifiers::empty());
    }
}

/// Where entry `id` was drawn in the last frame.
pub(super) fn row_area(app: &App, id: u64) -> QueueRowArea {
    *app.queue_areas()
        .rows
        .iter()
        .find(|area| area.id == id)
        .unwrap_or_else(|| panic!("entry {id} is not on screen"))
}

/// Point at entry `id` and draw, so its actions are on screen.
pub(super) fn hover_row(app: &mut App, id: u64) -> QueueRowArea {
    let row = row_area(app, id).row;
    mouse(app, MouseEventKind::Moved, (row.x + 2, row.y));
    draw(app);
    row_area(app, id)
}

pub(super) fn middle(rect: Option<Rect>) -> (u16, u16) {
    let rect = rect.expect("the part is drawn");
    (rect.x + rect.width / 2, rect.y)
}

pub(super) fn texts(app: &App) -> Vec<&str> {
    app.queue().iter().map(|item| item.text.as_str()).collect()
}

pub(super) fn events(rx: &mut mpsc::Receiver<UiEvent>) -> Vec<UiEvent> {
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

pub(super) fn preview_texts(app: &App) -> Vec<&str> {
    app.chat
        .queued_images
        .iter()
        .map(|(t, _)| t.as_str())
        .collect()
}
