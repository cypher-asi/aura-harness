//! The queued-messages panel above the input box.
//!
//! While an agent is busy, what the user sends waits in its queue. The
//! panel lists that queue in run order and lets the user remove an entry,
//! edit a queued message, move an entry, or send one now, with the mouse
//! or the keyboard.
//!
//! The queue itself is the agent's [`Hud::queue`](super::hud::Hud), which
//! the event loop replaces with every [`UiCommand::SetQueue`]
//! (crate::events::UiCommand). Actions change the local list at once so
//! the panel answers without waiting, then tell the event loop; its next
//! `SetQueue` is authoritative and overwrites the local guess.
//!
//! The panel's own state (folded, keyboard selection, pointer, the entry
//! being edited) belongs to the panel, not the agent: switching agents
//! drops the selection and the pointer state and cancels an edit.
//!
//! Invariant: entries are named by id everywhere (the selection, the
//! pressed row, the hovered row, the rows on screen), never by index,
//! because the queue can change under the panel between two frames.
//! Indexes are looked up again at the time of each action, and an id that
//! is gone does nothing. A selected entry that leaves the queue hands the
//! selection to the entry now at its place.
//!
//! Mouse: a left press on the header folds or opens the panel; on an
//! action shown on screen it runs the action; on a row it waits to see
//! whether the pointer moves. Moved to another row, it is a drag, and the
//! release drops the entry at the row under the pointer. A click never
//! gives the panel the keyboard, so typing after a click still goes to the
//! input. Any other press, a pointer move with no button, or a release
//! anywhere ends a press, so a lost release can't turn into a move later.
//!
//! Keyboard: Shift+Up in the input selects the newest entry and gives the
//! panel the keys. Up/Down select, Alt+Up/Down move the entry, Delete
//! removes it, Enter or `e` edits it, Ctrl+Enter sends it now and Esc goes
//! back to the input. Any other key goes back to the input and is handled
//! there.
//!
//! Editing loads the message into the input. Enter saves it back to the
//! same entry; Esc cancels. Either way the draft that was in the input
//! comes back, with its images. If the message started before the edit
//! was saved, the edited text stays in the input to send as a new message
//! and the draft is set aside: it comes back after that send, or on Esc.
//!
//! Previews of a queued message's images are found by its text when it
//! runs, so an edit renames them only once the event loop's queue shows
//! the new text; until then they stay under the text that will run.

mod actions;
mod edit;
mod keys;
mod mouse;

use super::{images::PendingImage, App};
use crate::events::{QueuedItemView, UiEvent};
use ratatui::layout::Rect;

/// Rows of entries the panel shows at most; past this a `+N more` row
/// says how many are hidden.
pub(crate) const MAX_VISIBLE_ROWS: usize = 5;

/// Edits waiting for the event loop to confirm them, at most; older ones
/// are dropped, leaving their previews under the old text.
const MAX_PENDING_RENAMES: usize = 32;

/// A clickable part of the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueSpot {
    /// The header row, which folds or opens the panel
    Header,
    /// The entry with id `id`, or one of its actions
    Row {
        /// Id of the entry
        id: u64,
        /// Which part of the row
        part: QueuePart,
    },
}

/// A part of an entry's row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueuePart {
    /// The text: a handle to drag the entry by
    Body,
    /// `Send Now ⏎`
    SendNow,
    /// `✎`, on messages only
    Edit,
    /// `✕`
    Remove,
}

/// Where the renderer drew one entry and its actions in the last frame.
/// An action has a rect only when it was drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QueueRowArea {
    /// Id of the entry
    pub(crate) id: u64,
    /// The whole row
    pub(crate) row: Rect,
    /// `Send Now ⏎`
    pub(crate) send_now: Option<Rect>,
    /// `✎`
    pub(crate) edit: Option<Rect>,
    /// `✕`
    pub(crate) remove: Option<Rect>,
}

/// Where the renderer drew the panel in the last frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct QueueAreas {
    /// The header row
    pub(crate) header: Option<Rect>,
    /// The entries on screen, top to bottom
    pub(crate) rows: Vec<QueueRowArea>,
}

/// The left button is down on an entry's row.
#[derive(Debug, Clone, PartialEq, Eq)]
struct QueuePress {
    /// Entry pressed
    id: u64,
    /// Screen row it went down on
    row: u16,
    /// Screen row the pointer is on now
    pointer: u16,
    /// The pointer has left that row
    moved: bool,
}

/// The entry the keyboard is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QueueSelection {
    /// Entry selected
    id: u64,
    /// Where it was last seen, for handing the selection on if it leaves
    at: usize,
}

/// What the input held, set aside: during a queue edit, or while another
/// agent is on screen.
#[derive(Debug, Default)]
pub(crate) struct Draft {
    text: String,
    cursor: usize,
    images: Vec<PendingImage>,
}

impl Draft {
    /// Whether nothing was typed or pasted.
    pub(super) fn is_empty(&self) -> bool {
        self.text.is_empty() && self.images.is_empty()
    }
}

/// A queued message loaded into the input for editing, and what the input
/// held before.
#[derive(Debug)]
struct QueueEdit {
    /// Entry being edited
    id: u64,
    /// The input before the edit
    draft: Draft,
}

/// An edit sent to the event loop, whose previews move to the new text
/// once its queue shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingRename {
    agent_id: String,
    id: u64,
    old: String,
    new: String,
}

/// State of the queued-messages panel.
#[derive(Debug, Default)]
pub(crate) struct QueuePanel {
    /// Whether only the header shows
    collapsed: bool,
    /// The entry the keyboard is on; `Some` while the panel has the keys
    selected: Option<QueueSelection>,
    /// What the pointer is over
    hover: Option<QueueSpot>,
    /// The left button is down on an entry
    press: Option<QueuePress>,
    /// The message being edited in the input
    editing: Option<QueueEdit>,
    /// The draft set aside by an edit whose message already ran; the
    /// input holds the edited text to send as a new message
    parked: Option<Draft>,
    /// Edits the event loop has not confirmed yet
    renames: Vec<PendingRename>,
    /// Where the panel was drawn in the last frame
    areas: QueueAreas,
}

/// Where a drag stands, for drawing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QueueDrag {
    /// Index of the entry being dragged
    pub(crate) from: usize,
    /// Index it lands at if released now
    pub(crate) to: usize,
}

impl App {
    /// The active agent's queue, oldest first.
    #[must_use]
    pub(crate) fn queue(&self) -> &[QueuedItemView] {
        &self.hud.queue
    }

    /// Whether the queued-messages panel shows only its header.
    #[must_use]
    pub(crate) const fn queue_collapsed(&self) -> bool {
        self.queue_panel.collapsed
    }

    /// Index of the entry highlighted for the keyboard, while the panel
    /// has the keys.
    #[must_use]
    pub(crate) fn queue_selected(&self) -> Option<usize> {
        let selection = self.queue_panel.selected?;
        self.queue_index_of(selection.id)
    }

    /// What the pointer is over in the panel.
    #[must_use]
    pub(crate) const fn queue_hover(&self) -> Option<QueueSpot> {
        self.queue_panel.hover
    }

    /// Index of the entry whose text is in the input for editing.
    #[must_use]
    pub(crate) fn queue_editing_index(&self) -> Option<usize> {
        let id = self.queue_panel.editing.as_ref()?.id;
        self.queue_index_of(id)
    }

    /// Whether the input holds a queued message being edited.
    #[must_use]
    pub fn editing_queued(&self) -> bool {
        self.queue_panel.editing.is_some()
    }

    /// The entry being dragged and where it would land, during a drag.
    #[must_use]
    pub(crate) fn queue_drag(&self) -> Option<QueueDrag> {
        let press = self.queue_panel.press.as_ref().filter(|p| p.moved)?;
        let from = self.queue_index_of(press.id)?;
        let to = self.queue_index_at_row(press.pointer)?;
        Some(QueueDrag { from, to })
    }

    /// First entry the panel shows when it has room for `visible` rows:
    /// the window follows the keyboard selection and the edited entry.
    #[must_use]
    pub(crate) fn queue_first_visible(&self, visible: usize) -> usize {
        let len = self.queue().len();
        let Some(focus) = self.queue_selected().or_else(|| self.queue_editing_index()) else {
            return 0;
        };
        (focus + 1)
            .saturating_sub(visible)
            .min(len.saturating_sub(visible))
    }

    /// Record where the panel was drawn this frame.
    pub(crate) fn set_queue_areas(&mut self, areas: QueueAreas) {
        self.queue_panel.areas = areas;
    }

    /// Where the panel was drawn in the last frame.
    #[cfg(test)]
    pub(crate) const fn queue_areas(&self) -> &QueueAreas {
        &self.queue_panel.areas
    }

    /// Pointer state that changes how the panel looks, so a move that
    /// leaves it as it was skips a redraw.
    pub(super) fn queue_pointer_state(&self) -> (Option<QueueSpot>, Option<QueueDrag>) {
        (self.queue_panel.hover, self.queue_drag())
    }

    /// Fold the panel to its header, or open it.
    pub(crate) fn toggle_queue_collapsed(&mut self) {
        self.queue_panel.collapsed = !self.queue_panel.collapsed;
        if self.queue_panel.collapsed {
            self.queue_panel.selected = None;
            self.queue_panel.press = None;
        }
    }

    /// Drop the panel's selection and pointer state and cancel an edit,
    /// as when another agent's chat comes on screen.
    pub(super) fn leave_queue_panel(&mut self) {
        self.cancel_queue_edit();
        self.drop_parked_edit();
        self.queue_panel.selected = None;
        self.queue_panel.hover = None;
        self.queue_panel.press = None;
    }

    /// Forget a press on an entry, as when the mouse went somewhere the
    /// panel can't follow.
    pub(super) fn drop_queue_press(&mut self) {
        self.queue_panel.press = None;
    }

    /// Take the event loop's queue for the agent on screen.
    pub(super) fn set_queue(&mut self, queue: Vec<QueuedItemView>) {
        let agent_id = self.active_agent_id.clone();
        self.confirm_queued_renames(&agent_id, &queue);
        self.hud.queue = queue;
        self.settle_queue_panel();
    }

    /// Rename the previews of edits that `queue`, the event loop's queue
    /// for `agent_id`, shows were applied. Edits of entries that left that
    /// queue are dropped: the message ran, or was removed, with the text
    /// the previews are still under.
    pub(super) fn confirm_queued_renames(&mut self, agent_id: &str, queue: &[QueuedItemView]) {
        let renames = std::mem::take(&mut self.queue_panel.renames);
        for rename in renames {
            if rename.agent_id != agent_id {
                self.queue_panel.renames.push(rename);
                continue;
            }
            match queue.iter().find(|item| item.id == rename.id) {
                Some(item) if item.text == rename.new => {
                    self.rename_queued_previews(&rename.old, &rename.new);
                }
                // A queue sent before the edit reached the event loop.
                Some(_) => self.queue_panel.renames.push(rename),
                None => {}
            }
        }
    }

    /// Bring the panel's state back in line after the queue changed: the
    /// selection moves to the entry now at its place, and a press or
    /// hover on an entry that is gone ends.
    fn settle_queue_panel(&mut self) {
        if let Some(selection) = self.queue_panel.selected {
            self.queue_panel.selected = match self.queue_index_of(selection.id) {
                Some(at) => Some(QueueSelection { at, ..selection }),
                None => self.queue().len().checked_sub(1).and_then(|last| {
                    let at = selection.at.min(last);
                    self.queue()
                        .get(at)
                        .map(|item| QueueSelection { id: item.id, at })
                }),
            };
        }
        if let Some(press) = &self.queue_panel.press {
            if self.queue_index_of(press.id).is_none() {
                self.queue_panel.press = None;
            }
        }
        if let Some(QueueSpot::Row { id, .. }) = self.queue_panel.hover {
            if self.queue_index_of(id).is_none() {
                self.queue_panel.hover = None;
            }
        }
    }

    fn queue_index_of(&self, id: u64) -> Option<usize> {
        self.queue().iter().position(|item| item.id == id)
    }

    /// Put the keyboard on entry `index`, if there is one.
    fn select_queued(&mut self, index: usize) {
        if let Some(item) = self.queue().get(index) {
            self.queue_panel.selected = Some(QueueSelection {
                id: item.id,
                at: index,
            });
        }
    }

    /// Index of the entry drawn at screen `row`; above the first row on
    /// screen is the first, below the last is the last.
    fn queue_index_at_row(&self, row: u16) -> Option<usize> {
        let rows = &self.queue_panel.areas.rows;
        let first = rows.first()?;
        let last = rows.last()?;
        let id = if row < first.row.y {
            first.id
        } else {
            rows.iter()
                .find(|area| area.row.y == row)
                .map_or(last.id, |area| area.id)
        };
        self.queue_index_of(id)
    }

    /// The part of the panel drawn at a screen cell.
    fn queue_spot_at(&self, column: u16, row: u16) -> Option<QueueSpot> {
        let areas = &self.queue_panel.areas;
        if super::contains(areas.header, column, row) {
            return Some(QueueSpot::Header);
        }
        let area = areas
            .rows
            .iter()
            .find(|area| super::contains(Some(area.row), column, row))?;
        let part = if super::contains(area.send_now, column, row) {
            QueuePart::SendNow
        } else if super::contains(area.edit, column, row) {
            QueuePart::Edit
        } else if super::contains(area.remove, column, row) {
            QueuePart::Remove
        } else {
            QueuePart::Body
        };
        Some(QueueSpot::Row { id: area.id, part })
    }

    fn send_queue_event(&self, event: UiEvent) {
        if let Some(tx) = &self.link.event_tx {
            // A full or closed channel means the event loop is behind or
            // gone; its next `SetQueue` (or none) corrects the panel.
            let _ = tx.try_send(event);
        }
    }
}
