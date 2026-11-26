//! The to-do drawer between the Chat body and the input.
//!
//! The drawer lists the to-do list of the agent on screen while it has
//! an item still open, during a turn and between turns. The list itself
//! is the agent's [`Hud::todos`](super::hud::Hud), which every
//! [`UiCommand::SetTodos`](crate::events::UiCommand) replaces; the drawer
//! only keeps how it is shown.
//!
//! Its state belongs to the drawer, not the agent: whether it is folded
//! to its header carries over turns and agents, and a new empty list
//! opens it again. A forced drawer shows a list with every item closed,
//! which otherwise hides it, until `/todos` puts it away or the list is
//! emptied. `/todos` forces a hidden list back, and otherwise folds or
//! opens the drawer.
//!
//! Mouse: a left press on the header row folds or opens the drawer. A
//! pointer move over the header lights its toggle and is never used up,
//! so the rest of the screen still sees where the pointer went.

use super::App;
use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::layout::Rect;

/// State of the to-do drawer.
#[derive(Debug, Default)]
pub(crate) struct TodoDrawer {
    /// Whether only the header shows
    folded: bool,
    /// Whether the drawer shows a list with every item closed
    forced: bool,
    /// Where the header was drawn in the last frame
    header: Option<Rect>,
    /// Whether the pointer is over the header
    hovered: bool,
}

impl App {
    /// Whether the to-do drawer shows only its header.
    #[must_use]
    pub(crate) const fn todo_drawer_folded(&self) -> bool {
        self.todo_drawer.folded
    }

    /// Whether the pointer is over the drawer's header, which lights its
    /// toggle.
    #[must_use]
    pub(crate) const fn todo_drawer_hovered(&self) -> bool {
        self.todo_drawer.hovered
    }

    /// Whether the drawer has anything to show: an agent's list with an
    /// item still open, or any list while the drawer is forced. In a
    /// channel that is its coordinator's list; screens with no agent,
    /// CHANNELS with no channel open among them, have no to-dos.
    #[must_use]
    pub(crate) fn todo_drawer_shown(&self) -> bool {
        if self.no_agent_on_screen() {
            return false;
        }
        self.hud.has_open_todos() || (self.todo_drawer.forced && !self.hud.todos.is_empty())
    }

    /// Fold the drawer to its header, or open it.
    pub(crate) fn toggle_todo_drawer(&mut self) {
        self.todo_drawer.folded = !self.todo_drawer.folded;
    }

    /// Show the drawer open, even for a list with every item closed,
    /// which otherwise hides it.
    pub(super) fn show_todo_drawer(&mut self) {
        self.todo_drawer.forced = true;
        self.todo_drawer.folded = false;
    }

    /// Stop showing a list with every item closed, leaving the fold as
    /// it is.
    pub(super) fn unforce_todo_drawer(&mut self) {
        self.todo_drawer.forced = false;
    }

    /// What `/todos` does to the drawer: bring a hidden list back open,
    /// put a finished list that it brought back away again, and fold or
    /// open a list still in progress.
    pub(super) fn cycle_todo_drawer(&mut self) {
        if !self.todo_drawer_shown() {
            self.show_todo_drawer();
        } else if self.todo_drawer.forced && !self.hud.has_open_todos() {
            self.todo_drawer.forced = false;
        } else {
            self.toggle_todo_drawer();
        }
    }

    /// Record where the drawer drew its header this frame; `None` when it
    /// was not drawn, which also ends a hover over it.
    pub(crate) fn set_todo_drawer_header(&mut self, header: Option<Rect>) {
        self.todo_drawer.header = header;
        if header.is_none() {
            self.todo_drawer.hovered = false;
        }
    }

    /// Pointer state that changes how the drawer looks, so a move that
    /// leaves it as it was skips a redraw.
    pub(super) const fn todo_drawer_pointer_state(&self) -> bool {
        self.todo_drawer.hovered
    }

    /// Forget the pointer over the drawer, as when the mouse went
    /// somewhere the drawer can't follow.
    pub(super) fn drop_todo_drawer_hover(&mut self) {
        self.todo_drawer.hovered = false;
    }

    /// Open the drawer and stop forcing it, for a list that was emptied:
    /// the next list starts open, whatever the last one did.
    pub(super) fn reset_todo_drawer(&mut self) {
        self.todo_drawer.folded = false;
        self.todo_drawer.forced = false;
    }

    /// Take a mouse event aimed at the drawer. Returns whether the drawer
    /// used it: only a left press on its header.
    pub(super) fn todo_drawer_mouse(
        &mut self,
        kind: MouseEventKind,
        column: u16,
        row: u16,
    ) -> bool {
        let on_header = super::contains(self.todo_drawer.header, column, row);
        match kind {
            MouseEventKind::Down(MouseButton::Left) if on_header => {
                self.clear_selection();
                self.toggle_todo_drawer();
                self.todo_drawer.hovered = true;
                true
            }
            MouseEventKind::Moved | MouseEventKind::Up(_) => {
                self.todo_drawer.hovered = on_header;
                false
            }
            _ => false,
        }
    }
}
