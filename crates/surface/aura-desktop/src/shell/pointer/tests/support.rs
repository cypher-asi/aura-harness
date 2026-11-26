use super::*;

impl Stage {
    /// Draw a frame, so what is pointed at is where it was drawn.
    pub(super) fn draw(&mut self) {
        self.shell.frame(self.now);
    }

    /// Press and let go of `button` on a box of the tab bar.
    pub fn click(&mut self, hit: Hit, button: MouseButton) {
        self.draw();
        let at = self.shell.bar.spot(hit).expect("the box is drawn");
        self.shell.pointer_moved(at);
        self.shell.button(button, true, self.now);
        self.shell.button(button, false, self.now);
        self.draw();
    }

    /// Press the left button on a box of the tab bar and let go over
    /// another.
    pub fn drag_tab(&mut self, from: Hit, to: Hit) {
        self.draw();
        let from = self.shell.bar.spot(from).expect("the box is drawn");
        let to = self.shell.bar.spot(to).expect("the box is drawn");
        self.shell.pointer_moved(from);
        self.shell.button(MouseButton::Left, true, self.now);
        self.shell.pointer_moved(to);
        self.shell.button(MouseButton::Left, false, self.now);
        self.draw();
    }

    /// Press the left button at `from` on the grid's part of the
    /// window and let go at `to`.
    pub fn drag(&mut self, from: Point, to: Point) {
        self.draw();
        let bar = tab_bar::height(&self.shell.metrics);
        let on_window = |at: Point| Point::new(at.x, at.y + bar);
        self.shell.pointer_moved(on_window(from));
        self.shell.button(MouseButton::Left, true, self.now);
        self.shell.pointer_moved(on_window(to));
        self.shell.button(MouseButton::Left, false, self.now);
        self.draw();
    }

    pub(super) fn chat(&self) -> PaneChrome {
        let tui = &self.shell.tabs.front().tui;
        let pane = tui.pane_chrome().iter().find(|c| c.pane == Pane::Chat);
        pane.expect("the chat has a bar").clone()
    }
}

/// A record shown over the chat.
pub(super) fn record_over_the_chat() -> Stage {
    use aura_ui::events::{RecordStatus, RecordSummary};
    use crossterm::event::{KeyCode, KeyEvent};

    let mut stage = stage();
    stage.seed(false);
    stage.shell.commands([UiCommand::NewRecord(RecordSummary {
        seq: 60,
        timestamp: "06:16:19".to_string(),
        full_hash: "355d52f6".repeat(8),
        hash_suffix: String::new(),
        tx_kind: "Policy".to_string(),
        sender: "KERNEL".to_string(),
        message: "approved".to_string(),
        action_count: 0,
        effect_status: "-".to_string(),
        status: RecordStatus::Ok,
        info: String::new(),
        what: String::new(),
        group: String::new(),
        detail: String::new(),
        error_details: String::new(),
        tx_id: "a71e37b3".repeat(8),
        agent_id: String::new(),
        ts_ms: 0,
        policy: None,
    })]);
    stage.draw();
    assert!(!stage.chat().closable, "the chat's own tabs don't close");
    let tui = &mut stage.shell.tabs.front_mut().tui;
    tui.focus_pane(Pane::Records);
    tui.handle_key(KeyEvent::from(KeyCode::Enter));
    stage.draw();
    stage
}
