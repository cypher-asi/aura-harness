//! Editing a queued message in the input, and keeping its image previews
//! under the text that will run.

use super::{Draft, PendingRename, QueueEdit, MAX_PENDING_RENAMES};
use crate::app::{App, NotificationType, PanelFocus};
use crate::events::UiEvent;

impl App {
    /// Load the text of queued message `id` into the input for editing,
    /// keeping what was typed there for afterwards.
    pub(crate) fn start_queue_edit(&mut self, id: u64) {
        let Some(item) = self.queue().iter().find(|item| item.id == id) else {
            return;
        };
        if !item.editable {
            self.notifications.current = Some((
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
    pub(in crate::app) fn take_draft(&mut self, text: String) -> Draft {
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
    pub(in crate::app) fn cancel_queue_edit(&mut self) {
        let Some(edit) = self.queue_panel.editing.take() else {
            return;
        };
        self.restore_draft(edit.draft);
    }

    pub(in crate::app) fn restore_draft(&mut self, draft: Draft) {
        self.input.text = draft.text;
        self.input.cursor_pos = draft.cursor.min(self.input.text.len());
        self.input.images = draft.images;
    }

    /// Give up on sending the edited text of a message that already ran:
    /// it goes into the input history, where Up finds it, and the draft
    /// set aside comes back.
    pub(super) fn drop_parked_edit(&mut self) {
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
    pub(super) fn finish_queue_edit(&mut self) {
        if self.input.text.trim().is_empty() {
            self.notifications.current = Some((
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
            self.notifications.current = Some((
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
    pub(super) fn note_queued_rename(&mut self, id: u64, old: String, new: &str) {
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
    pub(super) fn rename_queued_previews(&mut self, old: &str, new: &str) {
        if let Some(entry) = self
            .chat
            .queued_images
            .iter_mut()
            .find(|(text, _)| text == old)
        {
            new.clone_into(&mut entry.0);
        }
    }

    /// Drop the previews of queued message `id`, shown as `text`, that
    /// will never run. An unconfirmed edit means they are still under the
    /// text before it.
    pub(super) fn forget_queued_previews_of(&mut self, id: u64, text: &str) {
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
        if let Some(at) = self.chat.queued_images.iter().position(|(t, _)| *t == text) {
            self.chat.queued_images.remove(at);
        }
    }
}
