//! Clickable rows and copy buttons drawn in the chat.

use super::*;

impl App {
    /// Record where activity rows were drawn: `rows` holds the chat line
    /// of each one, and `first_line` is the chat line at the top of the
    /// view.
    pub(crate) fn set_chat_layout(&mut self, rows: Vec<ToggleRow>, first_line: usize) {
        self.toggle_rows = rows;
        self.chat_first_line = first_line;
    }

    /// The activity row at a screen cell, using the layout of the last frame.
    pub(super) fn toggle_row_at(&self, column: u16, row: u16) -> Option<ToggleRow> {
        let area = self
            .areas
            .chat_text
            .filter(|area| contains(Some(*area), column, row))?;
        let line = self.chat_first_line + usize::from(row - area.y);
        self.toggle_rows.iter().find(|r| r.line == line).copied()
    }

    /// Record where the chat drew code blocks' copy buttons this frame.
    pub(crate) fn set_chat_code_copies(&mut self, hits: Vec<CodeCopyHit>) {
        self.code_copy_hits = hits;
    }

    /// The code block copy button at a screen cell, using the last frame.
    /// A view over the chat hides the buttons.
    pub(super) fn code_copy_at(&self, column: u16, row: u16) -> Option<&CodeCopyHit> {
        if self.detail.is_some() {
            return None;
        }
        let area = self
            .areas
            .chat_text
            .filter(|area| contains(Some(*area), column, row))?;
        let line = self.chat_first_line + usize::from(row - area.y);
        let x = column - area.x;
        self.code_copy_hits
            .iter()
            .find(|hit| hit.line == line && (hit.x..hit.x + hit.width).contains(&x))
    }

    /// Forget the copy buttons a menu or overlay was drawn over, so a click
    /// on what covers one does not copy.
    pub(crate) fn drop_covered_code_copies(&mut self, buffer: &Buffer) {
        let Some(area) = self.areas.chat_text else {
            self.code_copy_hits.clear();
            return;
        };
        let first = self.chat_first_line;
        self.code_copy_hits.retain(|hit| {
            let Some(y) = hit
                .line
                .checked_sub(first)
                .and_then(|offset| u16::try_from(offset).ok())
                .map(|offset| area.y.saturating_add(offset))
            else {
                return false;
            };
            let x = area.x.saturating_add(hit.x);
            let drawn: String = (x..x.saturating_add(hit.width))
                .filter_map(|x| buffer.cell((x, y)).map(ratatui::buffer::Cell::symbol))
                .collect();
            drawn == crate::components::COPY_BUTTON
        });
    }

    /// Copy what the copy button at a screen cell copies after the next
    /// frame. Returns whether there was a button there.
    pub(super) fn copy_code_at(&mut self, column: u16, row: u16) -> bool {
        let Some(text) = self.code_copy_at(column, row).map(|hit| hit.text.clone()) else {
            return false;
        };
        let (text, what) = match text {
            CopyText::Code(code) => (code.to_string(), "the code"),
            CopyText::BootLog(id) => {
                let Some(console) = self
                    .messages
                    .iter()
                    .rev()
                    .filter_map(|m| m.boot_console())
                    .find(|console| console.id == id)
                else {
                    return true;
                };
                (console.transcript(), "the boot log")
            }
        };
        self.select.copy_pending_text = Some(text);
        self.select.copy_label = Some(what.to_string());
        true
    }

    /// Open or close the group of steps, the step or the to-do checklist
    /// at screen `row`, or open a turn's records. Returns whether anything
    /// was toggled.
    pub(super) fn toggle_activity_at(&mut self, column: u16, row: u16) -> bool {
        let Some(toggle) = self.toggle_row_at(column, row) else {
            return false;
        };
        match toggle.target {
            ToggleTarget::Group(id) => {
                self.group_open.insert(id, !toggle.open);
            }
            ToggleTarget::Step(id) => {
                if self.open_tool_step(id) {
                    return true;
                }
                let Some(message) = self.messages.iter_mut().find(|m| m.version().0 == id) else {
                    return false;
                };
                let expanded = !message.is_expanded();
                message.set_expanded(expanded);
            }
            ToggleTarget::TurnRecords(started_ms) => self.show_turn_records(started_ms),
            ToggleTarget::TodoChecklist => self.todos_expanded = !self.todos_expanded,
            ToggleTarget::More(id, section) => {
                let Some(message) = self.messages.iter_mut().find(|m| m.version().0 == id) else {
                    return false;
                };
                message.show_more(section);
            }
        }
        true
    }
}
