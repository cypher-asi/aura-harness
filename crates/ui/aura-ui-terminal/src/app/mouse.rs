//! The mouse: clicks, the wheel and hover, each sent to the panel under
//! the pointer.

use super::selection::Selection;
use super::{
    contains, tab_at, App, AppState, DetailView, HoverTarget, InputView, PanelFocus, RecordsTab,
};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;

impl App {
    /// The wheel over the input box scrolls its text a row per notch
    /// while it has more rows than the box shows. Returns whether it did,
    /// so the event does not also scroll a panel.
    fn scroll_input(&mut self, mouse: MouseEvent) -> bool {
        let up = match mouse.kind {
            MouseEventKind::ScrollUp => true,
            MouseEventKind::ScrollDown => false,
            _ => return false,
        };
        if !contains(self.areas.input, mouse.column, mouse.row) {
            return false;
        }
        let Some(rows) = self
            .areas
            .input_rows
            .filter(|rows| rows.total > rows.visible)
        else {
            return false;
        };
        // Several notches can arrive between frames, so keep counting
        // from the last one rather than from what was drawn.
        let scrolled = self.input_scroll_top();
        let top = scrolled.map_or(rows.top, |top| top.min(rows.max_top()));
        let top = if up {
            top.saturating_sub(1)
        } else {
            (top + 1).min(rows.max_top())
        };
        if let Some(view) = self
            .input
            .input_view
            .as_mut()
            .filter(|_| scrolled.is_some())
        {
            view.top = top;
        } else {
            self.input.input_view = Some(InputView {
                top,
                input: self.input.text.clone(),
                cursor: self.input.cursor_pos,
            });
        }
        true
    }

    pub(super) fn panel_at(&self, column: u16, row: u16) -> Option<PanelFocus> {
        if contains(self.areas.swarm, column, row) {
            Some(PanelFocus::Swarm)
        } else if contains(self.areas.records, column, row) {
            Some(PanelFocus::Records)
        } else if contains(self.areas.sandbox, column, row) {
            Some(PanelFocus::Sandbox)
        } else if contains(self.areas.chat, column, row) {
            Some(PanelFocus::Chat)
        } else {
            None
        }
    }

    /// Clickable item under the pointer, if it is over one.
    #[must_use]
    pub(crate) const fn hover(&self) -> Option<HoverTarget> {
        self.hover
    }

    /// Handle a mouse event: the wheel scrolls the panel under the pointer,
    /// or the input box's text while it has more rows than the box shows,
    /// a left click focuses it, clicking an agent row switches to it,
    /// clicking a record shows it over the chat panel, and clicking a
    /// sandbox row selects it and opens or closes a directory, or shows a
    /// file over the chat panel.
    /// Dragging with the left button selects text inside the panel the
    /// drag started in and copies it when the button is released; in the
    /// chat panel, unless it shows the graph or an image, holding the
    /// pointer past its top or bottom scrolls it (see [`Self::tick`]), and
    /// the wheel scrolls it under the selection. A press on the chat
    /// panel's scrollbar takes hold of it instead, and dragging then
    /// scrolls what the panel shows.
    /// Moving over a clickable item highlights it until the pointer leaves.
    ///
    /// Returns whether the screen may look different: false for a move or
    /// a drag that left the highlight and the selection as they were.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> bool {
        let pointing = matches!(mouse.kind, MouseEventKind::Moved | MouseEventKind::Drag(_));
        let menu = |app: &Self| {
            app.context_menu.as_ref().map(|menu| {
                let submenu = menu.submenu.as_ref().map(|submenu| submenu.selected);
                (menu.selected, submenu)
            })
        };
        let ends = |app: &Self| app.select.selection.as_ref().map(Selection::ends);
        let state = |app: &Self| {
            (
                app.hover,
                ends(app),
                menu(app),
                app.project_list.project_moves,
                app.project_drag_hit(),
                app.queue_pointer_state(),
                app.todo_drawer_pointer_state(),
                (app.scrollbar_held(), app.scroll_offset, app.detail.scroll),
            )
        };
        let before = state(self);
        self.mouse(mouse);
        !pointing || before != state(self)
    }

    pub(super) fn mouse(&mut self, mouse: MouseEvent) {
        if self.mouse_taken_first(mouse) || self.mouse_on_chrome(mouse) {
            return;
        }
        match mouse.kind {
            MouseEventKind::Drag(MouseButton::Left) => self.drag_left(mouse),
            MouseEventKind::Up(MouseButton::Left) => self.release_left(mouse),
            MouseEventKind::Moved => self.hover = self.hover_target(mouse.column, mouse.row),
            _ => self.mouse_on_panel(mouse),
        }
    }

    /// What takes the event before anything on screen: the modal on top,
    /// the stop button, the placement switch, a sign-in prompt, and an
    /// open context menu. Returns whether one took it.
    fn mouse_taken_first(&mut self, mouse: MouseEvent) -> bool {
        // The modal takes every event whole: a press that closes it must
        // not also land on what is under it.
        if let Some(modal) = self.modal() {
            self.modal_mouse(modal, mouse);
            return true;
        }
        // An open context menu takes the click instead, as it may cover
        // the button.
        if self.context_menu.is_none() && self.click_stop_button(mouse) {
            return true;
        }
        if self.context_menu.is_none() && self.click_placement_switch(mouse) {
            return true;
        }
        if self.state() != AppState::Idle && self.state() != AppState::Processing {
            self.release_pointer();
            return true;
        }
        if self.context_menu_mouse(mouse.kind, mouse.column, mouse.row) {
            self.hover = None;
            return true;
        }
        false
    }

    /// The side column's toggle, the queue, the to-do drawer, the bench
    /// panel, right clicks and the chat panel's scrollbar. Returns whether
    /// one took the event.
    fn mouse_on_chrome(&mut self, mouse: MouseEvent) -> bool {
        let (column, row) = (mouse.column, mouse.row);
        if mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && contains(self.areas.side_toggle, column, row)
        {
            self.toggle_side_column();
            self.clear_selection();
            self.hover = self.hover_target(column, row);
            return true;
        }
        if self.queue_mouse(mouse.kind, column, row)
            || self.todo_drawer_mouse(mouse.kind, column, row)
            || self.bench_panel_mouse(mouse.kind, column, row)
        {
            self.hover = None;
            return true;
        }
        if mouse.kind == MouseEventKind::Down(MouseButton::Right) && self.right_click(column, row) {
            return true;
        }
        if self.scrollbar_mouse(mouse.kind, column, row) {
            self.hover = self.hover_target(column, row);
            return true;
        }
        false
    }

    /// A right click opens the agents panel's or the side panel's menu,
    /// or pastes in the input box. Returns whether it did any of them.
    fn right_click(&mut self, column: u16, row: u16) -> bool {
        let menu = (self.panel_at(column, row) == Some(PanelFocus::Swarm)
            && self.open_context_menu(column, row))
            || (self.panel_at(column, row) == Some(PanelFocus::Sandbox)
                && self.open_channel_side_menu(column, row));
        // Right-click in the input box pastes, as in a terminal.
        let paste = !menu && contains(self.areas.input, column, row);
        if !menu && !paste {
            return false;
        }
        self.clear_selection();
        self.hover = None;
        if paste {
            self.request_paste();
        }
        true
    }

    /// Dragging with the left button moves a project or the flow view, or
    /// extends the selection.
    fn drag_left(&mut self, mouse: MouseEvent) {
        self.hover = None;
        if self.drag_project(mouse.row) || self.drag_flow(mouse.column, mouse.row) {
            return;
        }
        let at = Position::new(mouse.column, mouse.row);
        if let Some(selection) = self
            .select
            .selection
            .as_mut()
            .filter(|_| self.select.selecting.is_some())
        {
            selection.extend_to(at);
            self.select.selecting = Some(at);
        }
    }

    /// Releasing the left button drops a project or the flow view, or
    /// ends the selection.
    fn release_left(&mut self, mouse: MouseEvent) {
        if self.release_project() {
            self.hover = self.hover_target(mouse.column, mouse.row);
            return;
        }
        if self.release_flow() {
            return;
        }
        if self.select.selecting.take().is_some() {
            self.finish_selection(mouse.column, mouse.row);
        }
        self.hover = self.hover_target(mouse.column, mouse.row);
    }

    /// End the selection at the pointer: copy it, or treat an empty one
    /// as a click on a link or an activity row. A link opens on release
    /// rather than press, so a drag that starts on one selects its text.
    fn finish_selection(&mut self, column: u16, row: u16) {
        if let Some(selection) = self.select.selection.as_mut() {
            selection.extend_to(Position::new(column, row));
        }
        if self
            .select
            .selection
            .as_ref()
            .is_some_and(|s| !s.is_empty())
        {
            self.select.copy_requested = true;
        } else {
            self.select.selection = None;
            if self.detail.view.is_none() && !self.open_link_at(column, row) {
                self.toggle_activity_at(column, row);
            }
        }
    }

    /// The wheel over the input box or a panel, or a press on a panel.
    fn mouse_on_panel(&mut self, mouse: MouseEvent) {
        if self.scroll_input(mouse) {
            self.hover = self.hover_target(mouse.column, mouse.row);
            return;
        }
        let Some(panel) = self.panel_at(mouse.column, mouse.row) else {
            return;
        };
        match mouse.kind {
            MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => self.wheel_panel(panel, mouse),
            MouseEventKind::Down(MouseButton::Left) => self.press_panel(panel, mouse),
            _ => self.hover = self.hover_target(mouse.column, mouse.row),
        }
    }

    /// The wheel over `panel` scrolls the detail view, the flow view or
    /// the panel.
    fn wheel_panel(&mut self, panel: PanelFocus, mouse: MouseEvent) {
        let step = if panel == PanelFocus::Swarm {
            self.agents_panel_wheel_step()
        } else {
            3
        };
        let over_detail = panel == PanelFocus::Chat && self.detail.view.is_some();
        if matches!(
            mouse.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) && !self.wheel_keeps_selection(panel)
        {
            self.clear_selection();
        }
        match mouse.kind {
            MouseEventKind::ScrollUp if over_detail => self.scroll_detail_up(step),
            MouseEventKind::ScrollDown if over_detail => self.scroll_detail_down(step),
            _ if panel == PanelFocus::Chat && self.flow.is_open() => {
                self.clear_selection();
                self.scroll_flow(mouse.kind, mouse.modifiers, step);
            }
            MouseEventKind::ScrollUp => self.scroll_panel_up(panel, step),
            MouseEventKind::ScrollDown => self.scroll_panel_down(panel, step),
            _ => {}
        }
        self.hover = self.hover_target(mouse.column, mouse.row);
    }

    /// A left press focuses `panel`, opens an image or copies code in the
    /// chat panel, or starts a selection and clicks what is under it.
    fn press_panel(&mut self, panel: PanelFocus, mouse: MouseEvent) {
        let (column, row) = (mouse.column, mouse.row);
        self.focus = panel;
        if panel == PanelFocus::Chat
            && (self.open_image_at(column, row) || self.copy_code_at(column, row))
        {
            self.clear_selection();
            self.hover = None;
            return;
        }
        self.start_selection(panel, Position::new(column, row));
        match panel {
            PanelFocus::Swarm => {
                if !self.press_project(column, row) {
                    self.click_agents_panel(column, row);
                }
            }
            PanelFocus::Sandbox => self.click_side_panel(column, row),
            PanelFocus::Records => self.click_records_panel(column, row),
            PanelFocus::Chat => self.click_chat_panel(column, row),
        }
        self.hover = self.hover_target(column, row);
    }

    /// Start selecting text in `panel` at `at`, if it has selectable text.
    fn start_selection(&mut self, panel: PanelFocus, at: Position) {
        self.select.selection_scrolls = panel == PanelFocus::Chat && self.areas.chat_rows.is_some();
        let top = if self.select.selection_scrolls {
            self.areas.chat_rows_top
        } else {
            0
        };
        self.select.selection = self
            .selectable_area(panel)
            .map(|bounds| Selection::new(bounds, top, at));
        self.select.selecting = self.select.selection.is_some().then_some(at);
    }

    /// A left click in the chat panel: its close, expand and tab buttons,
    /// the graph, a thread, the log, the flow view or the bench.
    fn click_chat_panel(&mut self, column: u16, row: u16) {
        if self.detail_close_at(column, row) {
            self.close_chat_view();
            self.clear_selection();
        } else if contains(self.areas.chat_expand, column, row) {
            self.toggle_chat_expanded();
            self.clear_selection();
        } else if self.click_chat_tabs(column, row) {
            self.clear_selection();
        } else if self.detail.view == Some(DetailView::Graph) {
            self.click_graph(column, row);
        } else if self.click_thread(column, row)
            || (self.detail.view.is_none()
                && ((self.log.is_open() && self.click_log(column, row))
                    || (self.flow.is_open() && self.click_flow(column, row))
                    || (self.bench_draws_center() && self.click_bench(column, row))))
        {
            self.clear_selection();
        }
    }

    /// A left press on the input box's stop button cancels the running
    /// turn, as Ctrl+C does. Returns whether the press was on it.
    fn click_stop_button(&mut self, mouse: MouseEvent) -> bool {
        let pressed = mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && self.is_busy()
            && contains(self.areas.input_stop, mouse.column, mouse.row);
        if !pressed {
            return false;
        }
        self.cancel();
        self.clear_selection();
        self.hover = None;
        true
    }

    /// Clickable item at a screen cell, using the areas from the last frame.
    fn hover_target(&self, column: u16, row: u16) -> Option<HoverTarget> {
        if self.on_agent_tabs(column, row) {
            return self.agent_tab_at(column, row).map(HoverTarget::AgentTab);
        }
        if let Some(target) = self.hovered_chat_tab(column, row) {
            return Some(target);
        }
        if self.detail_close_at(column, row) {
            return Some(HoverTarget::CloseDetail);
        }
        if contains(self.areas.chat_expand, column, row) {
            return Some(HoverTarget::ExpandChat);
        }
        if contains(self.areas.input_stop, column, row) {
            return Some(HoverTarget::StopTurn);
        }
        if self.on_placement_switch(column, row) {
            return Some(HoverTarget::PlacementSwitch);
        }
        if contains(self.areas.side_toggle, column, row) {
            return Some(HoverTarget::SideToggle);
        }
        if let Some(target) = self.hovered_agents_panel(column, row) {
            return Some(target);
        }
        if contains(self.areas.record_tabs, column, row) {
            return tab_at(self.areas.record_tabs, column, row, RecordsTab::layout)
                .map(HoverTarget::RecordTab);
        }
        if let Some(index) = self.record_row_at(column, row) {
            return Some(HoverTarget::RecordRow(index));
        }
        if let Some(index) = self.member_row_at(column, row) {
            return Some(HoverTarget::MemberRow(index));
        }
        if let Some(index) = self.inbox_row_at(column, row) {
            return Some(HoverTarget::InboxRow(index));
        }
        if let Some(target) = self.hovered_side_panel(column, row) {
            return Some(target);
        }
        if let Some(target) = self.image_at(column, row) {
            return Some(target);
        }
        if let Some(hit) = self.code_copy_at(column, row) {
            return Some(HoverTarget::CopyCode(hit.line));
        }
        if let Some(target) = self.hovered_link(column, row) {
            return Some(target);
        }
        self.hovered_activity(column, row)
    }

    fn record_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let rows = self
            .areas
            .record_rows
            .filter(|&rows| contains(Some(rows), column, row))?;
        let index = self.records.scroll + usize::from(row - rows.y);
        (index < self.records.items.len()).then_some(index)
    }

    /// Clicking a record highlights it and shows it over the chat panel.
    pub(super) fn click_record_row(&mut self, column: u16, row: u16) {
        if let Some(index) = self.record_row_at(column, row) {
            self.records.selected = index;
            self.open_detail(DetailView::Record);
        }
    }

    pub(super) fn hovered_activity(&self, column: u16, row: u16) -> Option<HoverTarget> {
        if self.detail.view.is_some() {
            return None;
        }
        self.toggle_row_at(column, row)
            .map(|toggle| HoverTarget::Activity(toggle.target))
    }
}
