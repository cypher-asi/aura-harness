//! The open channel's side panel: the MEMBERS, INFO and QUEUE tabs.
//! MEMBERS folds each member over its subagents (see `fold`).

use super::fold;
use crate::animation::AURA_WIDTH;
use crate::app::processes::SIDE_PAGE;
use crate::app::{contains, follow_selection, App};
use crate::components::ROW_INSET;
use crate::events::MergeQueueView;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

/// Rows each member takes on the MEMBERS side tab: its name, then what it
/// works on.
pub(crate) const CHANNEL_MEMBER_ROWS: usize = 2;

impl App {
    /// Highlighted member of the MEMBERS side tab: an index into the open
    /// channel's members, always one that is shown.
    #[must_use]
    pub(crate) const fn selected_channel_member(&self) -> usize {
        self.channels.member_selected
    }

    /// The open channel's members shown on MEMBERS, as indices into them:
    /// none folded under a member.
    #[must_use]
    pub(crate) fn shown_channel_members(&self) -> Vec<usize> {
        self.channels.shown_members()
    }

    /// Whether member `index` of the open channel has subagents, and
    /// whether it is folded over them.
    #[must_use]
    pub(crate) fn channel_member_fold(&self, index: usize) -> Option<bool> {
        let members = self.channels.members();
        fold::has_subagents(members, index)
            .then(|| fold::is_folded(members, &self.channels.unfolded_members, index))
    }

    /// Whether a subagent folded under member `index` is working.
    #[must_use]
    pub(crate) fn channel_member_hides_working(&self, index: usize) -> bool {
        fold::hides_working(
            self.channels.members(),
            &self.channels.unfolded_members,
            index,
            |member| self.agent_thinking(member),
        )
    }

    /// The member shown for member `index`: itself, or the folded member
    /// it is hidden under.
    #[must_use]
    pub(crate) fn shown_channel_member_of(&self, index: usize) -> usize {
        fold::shown_member_of(
            self.channels.members(),
            &self.channels.unfolded_members,
            index,
        )
    }

    /// Unfold every member of every listed channel over its subagents, for
    /// tests about subagent rows.
    #[cfg(test)]
    pub(crate) fn unfold_channel_members(&mut self) {
        let ids: Vec<String> = self
            .channels
            .list
            .iter()
            .flat_map(|channel| {
                let members = &channel.members;
                (0..members.len())
                    .filter(|&i| fold::has_subagents(members, i))
                    .map(|i| members[i].id.clone())
                    .collect::<Vec<_>>()
            })
            .collect();
        self.channels.unfolded_members.extend(ids);
    }

    /// Keep the highlighted member among the `visible_members` that fit,
    /// returning the first shown row to draw.
    pub(crate) fn sync_channel_members_scroll(&mut self, visible_members: usize) -> usize {
        let shown = self.channels.shown_members();
        let selected = self.channels.member_selected;
        let row = shown.iter().position(|&i| i == selected).unwrap_or(0);
        let state = &mut self.channels;
        state.member_scroll =
            follow_selection(state.member_scroll, row, visible_members, shown.len());
        state.member_scroll
    }

    /// Record where the MEMBERS side tab's rows were drawn this frame.
    pub(crate) fn set_channel_member_rows_area(&mut self, area: Rect) {
        self.channels.member_rows_area = Some(area);
    }

    /// Drop where the MEMBERS side tab's rows were drawn, for a frame
    /// that may not draw them.
    pub(in crate::app) fn forget_channel_member_rows(&mut self) {
        self.channels.member_rows_area = None;
    }

    /// Member of the MEMBERS side tab drawn at a screen cell, on either of
    /// its rows.
    pub(in crate::app) fn channel_member_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let state = &self.channels;
        let rows = state
            .member_rows_area
            .filter(|&rows| contains(Some(rows), column, row))?;
        let at = state.member_scroll + usize::from(row - rows.y) / CHANNEL_MEMBER_ROWS;
        state.shown_members().get(at).copied()
    }

    /// Member whose fold marker is at a screen cell: the `▸` or `▾` of a
    /// member with subagents, drawn as `tree_row` lays the icon column
    /// out, after the row's inset and `AURA_WIDTH + 2` columns of guides
    /// per level, on its name row. The rest of the icon column counts too.
    fn channel_fold_marker_at(&self, column: u16, row: u16) -> Option<usize> {
        let index = self.channel_member_row_at(column, row)?;
        let rows = self.channels.member_rows_area?;
        let name_row = usize::from(row - rows.y) % CHANNEL_MEMBER_ROWS == 0;
        let depth = self.channels.members().get(index)?.depth;
        let start = ROW_INSET + depth * (AURA_WIDTH + 2);
        let offset = usize::from(column.checked_sub(rows.x)?);
        (name_row && self.channel_member_fold(index).is_some())
            .then_some(index)
            .filter(|_| (start..=start + AURA_WIDTH).contains(&offset))
    }

    /// Highlight member `index`, unfolding the members above it so it
    /// shows. An index past the members highlights the last one shown.
    /// Opening a member's chat is [`Self::open_channel_member`]'s.
    pub(in crate::app) fn select_channel_member(&mut self, index: usize) {
        if index >= self.channels.members().len() {
            let last = self.channels.shown_members().last().copied().unwrap_or(0);
            self.channels.set_member_selected(last);
            return;
        }
        let above = fold::ids_above(self.channels.members(), index);
        self.channels.unfolded_members.extend(above);
        self.channels.set_member_selected(index);
    }

    /// Move the MEMBERS highlight `delta` rows over the members shown, as
    /// the wheel does.
    pub(in crate::app) fn move_channel_member(&mut self, delta: isize) {
        let shown = self.channels.shown_members();
        let selected = self.channels.member_selected;
        let row = shown.iter().position(|&i| i == selected).unwrap_or(0);
        let row = row
            .saturating_add_signed(delta)
            .min(shown.len().saturating_sub(1));
        if let Some(&index) = shown.get(row) {
            self.channels.set_member_selected(index);
        }
    }

    /// Fold member `index` over its subagents, or unfold it. A highlight
    /// on one of the rows it hides moves to it.
    pub(in crate::app) fn toggle_channel_member(&mut self, index: usize) {
        let Some(folded) = self.channel_member_fold(index) else {
            return;
        };
        self.set_channel_member_folded(index, !folded);
    }

    /// Fold member `index` (`folded`) or unfold it, when it has subagents.
    fn set_channel_member_folded(&mut self, index: usize, folded: bool) {
        let Some(id) = self.channels.members().get(index).map(|m| m.id.clone()) else {
            return;
        };
        if self.channel_member_fold(index).is_none() {
            return;
        }
        if folded {
            self.channels.unfolded_members.remove(&id);
        } else {
            self.channels.unfolded_members.insert(id);
        }
        let selected = self.channels.member_selected;
        self.channels.set_member_selected(selected);
    }

    /// Left on a member: fold it when it is open, else go to the member
    /// that spawned it.
    fn fold_or_leave_channel_member(&mut self) {
        let index = self.channels.member_selected;
        if self.channel_member_fold(index) == Some(false) {
            self.set_channel_member_folded(index, true);
        } else if let Some(parent) = fold::parent_of(self.channels.members(), index) {
            self.channels.set_member_selected(parent);
        }
    }

    /// Clicking a member's fold marker folds or opens it; clicking it
    /// elsewhere highlights it and opens its chat, or the channel's on the
    /// coordinator's row.
    pub(in crate::app) fn click_channel_member(&mut self, column: u16, row: u16) {
        if let Some(index) = self.channel_fold_marker_at(column, row) {
            self.toggle_channel_member(index);
        } else if let Some(index) = self.channel_member_row_at(column, row) {
            self.open_channel_member(index);
        }
    }

    /// Keys of the MEMBERS side tab, which move its highlight over the
    /// members shown; Space folds or opens the highlighted member over its
    /// subagents, Right opens it and Left folds it (or goes to the member
    /// above); Enter opens the highlighted member's chat, as a click does.
    pub(in crate::app) fn handle_channel_members_key(&mut self, key: KeyEvent) {
        let selected = self.channels.member_selected;
        match key.code {
            KeyCode::Up => self.move_channel_member(-1),
            KeyCode::Down => self.move_channel_member(1),
            KeyCode::PageUp => self.move_channel_member(-SIDE_PAGE),
            KeyCode::PageDown => self.move_channel_member(SIDE_PAGE),
            KeyCode::Home => self.select_channel_member(0),
            KeyCode::End => self.select_channel_member(usize::MAX),
            KeyCode::Char(' ') => self.toggle_channel_member(selected),
            KeyCode::Right => self.set_channel_member_folded(selected, false),
            KeyCode::Left => self.fold_or_leave_channel_member(),
            KeyCode::Enter => self.open_channel_member(selected),
            _ => {}
        }
    }

    // ---- The INFO side tab ---------------------------------------------

    /// Keep the INFO side tab's scroll inside a document `total` lines
    /// long shown `visible` lines at a time, returning the first line to
    /// show.
    pub(crate) fn sync_channel_info_scroll(&mut self, total: usize, visible: usize) -> usize {
        self.channels.info_scroll.sync(total, visible)
    }

    /// Scroll the INFO side tab by `delta` lines, as the wheel does.
    pub(in crate::app) fn scroll_channel_info(&mut self, delta: isize) {
        self.channels.info_scroll.by(delta);
    }

    /// Keys of the INFO side tab, which scroll it.
    pub(in crate::app) fn handle_channel_info_key(&mut self, key: KeyEvent) {
        self.channels.info_scroll.key(key.code);
    }

    // ---- The QUEUE side tab --------------------------------------------

    /// The merge queue of the open channel's repo, once the session sent
    /// it. `None` with no channel open.
    #[must_use]
    pub(crate) fn open_channel_queue(&self) -> Option<&MergeQueueView> {
        let id = self.channels.open.as_deref()?;
        self.channels.merge_queues.get(id)
    }

    /// Keep the merge queue of channel `channel_id`'s repo, for its QUEUE
    /// tab, in place of the one before.
    pub(in crate::app) fn set_merge_queue(&mut self, channel_id: String, queue: MergeQueueView) {
        self.channels.merge_queues.insert(channel_id, queue);
    }

    /// Keep the QUEUE side tab's scroll inside a document `total` lines
    /// long shown `visible` lines at a time, returning the first line to
    /// show.
    pub(crate) fn sync_channel_queue_scroll(&mut self, total: usize, visible: usize) -> usize {
        self.channels.queue_scroll.sync(total, visible)
    }

    /// Scroll the QUEUE side tab by `delta` lines, as the wheel does.
    pub(in crate::app) fn scroll_channel_queue(&mut self, delta: isize) {
        self.channels.queue_scroll.by(delta);
    }

    /// Keys of the QUEUE side tab, which scroll it.
    pub(in crate::app) fn handle_channel_queue_key(&mut self, key: KeyEvent) {
        self.channels.queue_scroll.key(key.code);
    }
}
