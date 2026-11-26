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

use super::{images::PendingImage, App, AppState, NotificationType, PanelFocus};
use crate::events::{QueuedItemView, UiEvent};
use crate::terminal::KeyResult;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
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
        if let Some(tx) = &self.event_tx {
            // A full or closed channel means the event loop is behind or
            // gone; its next `SetQueue` (or none) corrects the panel.
            let _ = tx.try_send(event);
        }
    }

    /// Take a mouse event aimed at the panel. Returns whether the panel
    /// used it; a pointer move is never used up, so the rest of the screen
    /// still sees where the pointer went.
    pub(super) fn queue_mouse(&mut self, kind: MouseEventKind, column: u16, row: u16) -> bool {
        match kind {
            MouseEventKind::Down(button) => {
                // A press left over from a release we never saw ends here.
                self.queue_panel.press = None;
                if button != MouseButton::Left {
                    return false;
                }
                let Some(spot) = self.queue_spot_at(column, row) else {
                    return false;
                };
                self.clear_selection();
                match spot {
                    QueueSpot::Header => self.toggle_queue_collapsed(),
                    QueueSpot::Row { id, part } => self.press_queue_row(id, part, row),
                }
                self.queue_panel.hover = self.queue_spot_at(column, row);
                true
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(press) = self.queue_panel.press.as_mut() else {
                    return false;
                };
                press.moved |= row != press.row;
                press.pointer = row;
                self.queue_panel.hover = None;
                true
            }
            MouseEventKind::Up(button) => {
                let press = self.queue_panel.press.take();
                let used = match press.filter(|_| button == MouseButton::Left) {
                    Some(press) => {
                        self.release_queue_row(&press);
                        true
                    }
                    None => false,
                };
                self.queue_panel.hover = self.queue_spot_at(column, row);
                used
            }
            MouseEventKind::Moved => {
                // A move reports no button down, so the release was lost.
                self.queue_panel.press = None;
                self.queue_panel.hover = self.queue_spot_at(column, row);
                false
            }
            MouseEventKind::Drag(_) => {
                self.queue_panel.press = None;
                false
            }
            _ => false,
        }
    }

    fn press_queue_row(&mut self, id: u64, part: QueuePart, row: u16) {
        match part {
            QueuePart::SendNow => self.send_queued_item_now(id),
            QueuePart::Edit => self.start_queue_edit(id),
            QueuePart::Remove => self.remove_queued(id),
            QueuePart::Body => {
                self.queue_panel.press = Some(QueuePress {
                    id,
                    row,
                    pointer: row,
                    moved: false,
                });
            }
        }
    }

    /// The button came up after a press on an entry: a drag drops it at
    /// the row under the pointer. A click does nothing more than the
    /// hover did; the keyboard stays with the input.
    fn release_queue_row(&mut self, press: &QueuePress) {
        if !press.moved {
            return;
        }
        if let Some(to) = self.queue_index_at_row(press.pointer) {
            self.move_queued(press.id, to);
        }
    }

    /// Move entry `id` to index `to`, here and in the event loop.
    pub(crate) fn move_queued(&mut self, id: u64, to: usize) {
        let Some(from) = self.queue_index_of(id) else {
            return;
        };
        let to = to.min(self.queue().len().saturating_sub(1));
        if from == to {
            return;
        }
        let item = self.hud.queue.remove(from);
        self.hud.queue.insert(to, item);
        self.settle_queue_panel();
        self.send_queue_event(UiEvent::MoveQueued {
            agent_id: self.active_agent_id.clone(),
            id,
            to,
        });
    }

    /// Drop entry `id`, here and in the event loop.
    pub(crate) fn remove_queued(&mut self, id: u64) {
        let Some(index) = self.queue_index_of(id) else {
            return;
        };
        let item = self.hud.queue.remove(index);
        if self.queue_panel.editing.as_ref().map(|e| e.id) == Some(id) {
            self.cancel_queue_edit();
        }
        self.forget_queued_previews_of(id, &item.text);
        self.settle_queue_panel();
        self.send_queue_event(UiEvent::RemoveQueued {
            agent_id: self.active_agent_id.clone(),
            id,
        });
    }

    /// Run entry `id` now: it goes to the front of the queue and the
    /// running turn is cancelled for it.
    pub(crate) fn send_queued_item_now(&mut self, id: u64) {
        let Some(index) = self.queue_index_of(id) else {
            return;
        };
        let item = self.hud.queue.remove(index);
        if self.queue_panel.editing.as_ref().map(|e| e.id) == Some(id) {
            self.cancel_queue_edit();
        }
        self.hud.queue.insert(0, item);
        self.queue_panel.selected = None;
        self.settle_queue_panel();
        self.send_queue_event(UiEvent::SendQueuedItemNow {
            agent_id: self.active_agent_id.clone(),
            id,
        });
        self.notification = Some((
            "Sending the queued message now".to_string(),
            NotificationType::Success,
        ));
    }

    /// Load the text of queued message `id` into the input for editing,
    /// keeping what was typed there for afterwards.
    pub(crate) fn start_queue_edit(&mut self, id: u64) {
        let Some(item) = self.queue().iter().find(|item| item.id == id) else {
            return;
        };
        if !item.editable {
            self.notification = Some((
                "Only queued messages can be edited".to_string(),
                NotificationType::Warning,
            ));
            return;
        }
        let text = item.text.clone();
        self.cancel_queue_edit();
        let draft = self.take_draft(text);
        self.queue_panel.editing = Some(QueueEdit { id, draft });
        self.queue_panel.selected = None;
        self.focus = PanelFocus::Chat;
        self.input.slash_dismissed = true;
    }

    /// Put `text` in the input and return what it held.
    pub(super) fn take_draft(&mut self, text: String) -> Draft {
        let text = std::mem::replace(&mut self.input.text, text);
        let cursor = std::mem::replace(&mut self.input.cursor_pos, self.input.text.len());
        let images = std::mem::take(&mut self.input.images);
        Draft {
            text,
            cursor,
            images,
        }
    }

    /// Put back the draft the edit set aside, dropping the edited text.
    pub(super) fn cancel_queue_edit(&mut self) {
        let Some(edit) = self.queue_panel.editing.take() else {
            return;
        };
        self.restore_draft(edit.draft);
    }

    pub(super) fn restore_draft(&mut self, draft: Draft) {
        self.input.text = draft.text;
        self.input.cursor_pos = draft.cursor.min(self.input.text.len());
        self.input.images = draft.images;
    }

    /// Give up on sending the edited text of a message that already ran:
    /// it goes into the input history, where Up finds it, and the draft
    /// set aside comes back.
    fn drop_parked_edit(&mut self) {
        let Some(draft) = self.queue_panel.parked.take() else {
            return;
        };
        let edited = std::mem::take(&mut self.input.text);
        if !edited.trim().is_empty() {
            self.history().add(&edited);
        }
        self.restore_draft(draft);
    }

    /// Save the input back into the queued message being edited and put
    /// back the draft. A message that already left the queue can't take
    /// the text, so the text stays in the input to send as a new one and
    /// the draft waits until then.
    fn finish_queue_edit(&mut self) {
        if self.input.text.trim().is_empty() {
            self.notification = Some((
                "A queued message can't be empty; Esc cancels the edit".to_string(),
                NotificationType::Warning,
            ));
            return;
        }
        let Some(edit) = self.queue_panel.editing.take() else {
            return;
        };
        let Some(item) = self.hud.queue.iter_mut().find(|item| item.id == edit.id) else {
            self.queue_panel.parked = Some(edit.draft);
            self.notification = Some((
                "That message already left the queue; Enter sends this as a new one, \
                 then your draft comes back (Esc brings it back now)"
                    .to_string(),
                NotificationType::Warning,
            ));
            return;
        };
        let text = std::mem::take(&mut self.input.text);
        let old = std::mem::replace(&mut item.text, text.clone());
        self.note_queued_rename(edit.id, old, &text);
        self.restore_draft(edit.draft);
        self.send_queue_event(UiEvent::EditQueued {
            agent_id: self.active_agent_id.clone(),
            id: edit.id,
            text,
        });
    }

    /// Remember that entry `id` was edited from `old` to `new`, for when
    /// the event loop confirms it. A second edit before that keeps the
    /// first one's old text, which is what the previews are under.
    fn note_queued_rename(&mut self, id: u64, old: String, new: &str) {
        let agent_id = &self.active_agent_id;
        let renames = &mut self.queue_panel.renames;
        if let Some(rename) = renames
            .iter_mut()
            .find(|r| r.id == id && &r.agent_id == agent_id)
        {
            new.clone_into(&mut rename.new);
            return;
        }
        renames.push(PendingRename {
            agent_id: agent_id.clone(),
            id,
            old,
            new: new.to_string(),
        });
        if renames.len() > MAX_PENDING_RENAMES {
            renames.remove(0);
        }
    }

    /// Keep the previews of a queued message under its new text, so the
    /// chat still finds them when the message runs.
    fn rename_queued_previews(&mut self, old: &str, new: &str) {
        if let Some(entry) = self.queued_images.iter_mut().find(|(text, _)| text == old) {
            new.clone_into(&mut entry.0);
        }
    }

    /// Drop the previews of queued message `id`, shown as `text`, that
    /// will never run. An unconfirmed edit means they are still under the
    /// text before it.
    fn forget_queued_previews_of(&mut self, id: u64, text: &str) {
        let agent_id = &self.active_agent_id;
        let pending = self
            .queue_panel
            .renames
            .iter()
            .position(|r| r.id == id && &r.agent_id == agent_id);
        let text = match pending {
            Some(at) => self.queue_panel.renames.remove(at).old,
            None => text.to_string(),
        };
        if let Some(at) = self.queued_images.iter().position(|(t, _)| *t == text) {
            self.queued_images.remove(at);
        }
    }

    /// Handle `key` for the panel: the editing keys while a queued message
    /// is in the input, the panel's keys while it has the keys, and
    /// Shift+Up, which gives them to it. Returns `None` for keys the panel
    /// leaves to the rest of the screen.
    pub(super) fn handle_queue_key(&mut self, key: KeyEvent) -> Option<KeyResult> {
        if self.queue_panel.editing.is_some() {
            return match key.code {
                KeyCode::Esc => {
                    self.cancel_queue_edit();
                    Some(KeyResult::continue_running())
                }
                KeyCode::Enter if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                    self.finish_queue_edit();
                    Some(KeyResult::continue_running())
                }
                _ => None,
            };
        }
        if self.queue_panel.parked.is_some() {
            if let Some(result) = self.handle_parked_key(key) {
                return Some(result);
            }
        }
        if self.queue_panel.selected.is_some() {
            return self.handle_focused_queue_key(key);
        }
        let enters = key.code == KeyCode::Up
            && key.modifiers == KeyModifiers::SHIFT
            && self.focus == PanelFocus::Chat
            && matches!(self.state, AppState::Idle | AppState::Processing)
            && !self.queue().is_empty();
        if !enters {
            return None;
        }
        self.queue_panel.collapsed = false;
        self.select_queued(self.queue().len().saturating_sub(1));
        Some(KeyResult::continue_running())
    }

    /// Keys while the input holds the edited text of a message that
    /// already ran: Enter sends it and brings the draft back, Esc brings
    /// the draft back at once.
    fn handle_parked_key(&mut self, key: KeyEvent) -> Option<KeyResult> {
        match key.code {
            KeyCode::Esc => {
                self.drop_parked_edit();
                Some(KeyResult::continue_running())
            }
            KeyCode::Enter
                if !key.modifiers.contains(KeyModifiers::SHIFT)
                    && !self.input.text.trim().is_empty() =>
            {
                let result = self.submit_input();
                // A send that was refused puts the text back; the draft
                // waits for the next try.
                if self.input.text.is_empty() {
                    if let Some(draft) = self.queue_panel.parked.take() {
                        self.restore_draft(draft);
                    }
                }
                Some(result)
            }
            _ => None,
        }
    }

    fn handle_focused_queue_key(&mut self, key: KeyEvent) -> Option<KeyResult> {
        let Some(selected) = self.queue_selected() else {
            self.queue_panel.selected = None;
            return None;
        };
        let Some(id) = self.queue().get(selected).map(|item| item.id) else {
            self.queue_panel.selected = None;
            return None;
        };
        let last = self.queue().len().saturating_sub(1);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Up if alt => self.move_queued(id, selected.saturating_sub(1)),
            KeyCode::Down if alt => self.move_queued(id, (selected + 1).min(last)),
            KeyCode::Up => self.select_queued(selected.saturating_sub(1)),
            KeyCode::Down => self.select_queued((selected + 1).min(last)),
            KeyCode::Home => self.select_queued(0),
            KeyCode::End => self.select_queued(last),
            KeyCode::Delete => self.remove_queued(id),
            KeyCode::Enter if ctrl => self.send_queued_item_now(id),
            KeyCode::Enter => self.start_queue_edit(id),
            KeyCode::Char('e') if !ctrl && !alt => self.start_queue_edit(id),
            KeyCode::Esc => self.queue_panel.selected = None,
            _ => {
                self.queue_panel.selected = None;
                return None;
            }
        }
        Some(KeyResult::continue_running())
    }
}
