//! Actions on a queued entry: move it, remove it, send it now.

use crate::app::{App, NotificationType};
use crate::events::UiEvent;

impl App {
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
        self.notifications.current = Some((
            "Sending the queued message now".to_string(),
            NotificationType::Success,
        ));
    }
}
