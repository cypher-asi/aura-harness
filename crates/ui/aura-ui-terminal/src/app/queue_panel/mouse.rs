//! Mouse handling for the queued-messages panel: presses, drags and hover.

use super::{QueuePart, QueuePress, QueueSpot};
use crate::app::App;
use crossterm::event::{MouseButton, MouseEventKind};

impl App {
    /// Take a mouse event aimed at the panel. Returns whether the panel
    /// used it; a pointer move is never used up, so the rest of the screen
    /// still sees where the pointer went.
    pub(in crate::app) fn queue_mouse(
        &mut self,
        kind: MouseEventKind,
        column: u16,
        row: u16,
    ) -> bool {
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

    pub(super) fn press_queue_row(&mut self, id: u64, part: QueuePart, row: u16) {
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
    pub(super) fn release_queue_row(&mut self, press: &QueuePress) {
        if !press.moved {
            return;
        }
        if let Some(to) = self.queue_index_at_row(press.pointer) {
            self.move_queued(press.id, to);
        }
    }
}
