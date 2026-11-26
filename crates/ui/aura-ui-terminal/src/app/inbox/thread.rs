//! The thread view over the chat panel: opening a thread, its keys
//! and clicks.

use std::collections::HashSet;

use super::ThreadView;
use crate::app::{contains, App, DetailView};
use crate::events::MailView;
use crossterm::event::{KeyCode, KeyEvent};

/// Source lines message `mail` takes in a thread view: its header, plus
/// its body when `unfolded`. The renderer lays messages out the same way,
/// with one blank line between them.
#[must_use]
pub(super) fn thread_message_lines(mail: &MailView, unfolded: bool) -> usize {
    if unfolded {
        1 + mail.text.lines().count()
    } else {
        1
    }
}

impl App {
    /// Show `thread_id` over the chat with `focus_message` (when it is of
    /// the thread) and its newest message unfolded, `focus_message`
    /// highlighted or else the newest, and mark the thread read.
    pub(in crate::app) fn open_thread(&mut self, thread_id: String, focus_message: Option<u64>) {
        let messages = self.thread_messages(&thread_id);
        let first = messages.first().map_or("", |mail| mail.subject.as_str());
        let subject = self.inbox.subject(&thread_id, first).to_string();
        let newest = messages.last().map(|mail| mail.id);
        let focus = focus_message.filter(|id| messages.iter().any(|mail| mail.id == *id));
        let view = ThreadView {
            unfolded: focus.into_iter().chain(newest).collect(),
            selected: focus.or(newest),
            thread_id,
            subject,
        };
        self.mark_thread_read(&view.thread_id);
        self.inbox.thread_headers.clear();
        self.open_detail(DetailView::Thread(Box::new(view)));
        self.detail.scroll = self.selected_thread_line().unwrap_or(0);
        self.refresh_inbox();
    }

    /// Messages of `thread_id`, in the order they were sent.
    #[must_use]
    pub fn thread_messages(&self, thread_id: &str) -> Vec<&MailView> {
        let mut messages: Vec<&MailView> = self
            .inbox
            .messages
            .iter()
            .filter(|mail| mail.thread_id == thread_id)
            .collect();
        messages.sort_by_key(|mail| mail.id);
        messages
    }

    /// Record the wrapped row, from the top of the thread view's content,
    /// of each message header drawn this frame.
    pub fn set_thread_header_rows(&mut self, rows: Vec<(usize, u64)>) {
        self.inbox.thread_headers = rows;
    }

    pub(super) fn mark_thread_read(&mut self, thread_id: &str) {
        if let Some(newest) = self.inbox.newest_id(thread_id) {
            let read = self.inbox.read.entry(thread_id.to_string()).or_default();
            *read = (*read).max(newest);
        }
    }

    pub(super) fn thread_view_mut(&mut self) -> Option<&mut ThreadView> {
        match self.detail.view.as_mut() {
            Some(DetailView::Thread(view)) => Some(view.as_mut()),
            _ => None,
        }
    }

    pub(super) fn thread_view(&self) -> Option<&ThreadView> {
        match self.detail.view.as_ref() {
            Some(DetailView::Thread(view)) => Some(view.as_ref()),
            _ => None,
        }
    }

    /// Source line of the highlighted message's header in the thread view.
    pub(super) fn selected_thread_line(&self) -> Option<usize> {
        let view = self.thread_view()?;
        let selected = view.selected?;
        let mut line = 0;
        for mail in self.thread_messages(&view.thread_id) {
            if mail.id == selected {
                return Some(line);
            }
            line += thread_message_lines(mail, view.unfolded.contains(&mail.id)) + 1;
        }
        None
    }

    /// Keys of the thread view: `n`/`p` highlight the next or previous
    /// message, Enter or Space folds or unfolds it, `e`/`c` unfold or fold
    /// every message. Returns whether the key was one of them; the rest
    /// scroll or close the view like any other.
    pub(in crate::app) fn handle_thread_key(&mut self, key: KeyEvent) -> bool {
        if self.thread_view().is_none() {
            return false;
        }
        match key.code {
            KeyCode::Char('n') => self.move_thread_selection(1),
            KeyCode::Char('p') => self.move_thread_selection(-1),
            KeyCode::Enter | KeyCode::Char(' ') => self.toggle_selected_message(),
            KeyCode::Char('e') => self.fold_all_messages(false),
            KeyCode::Char('c') => self.fold_all_messages(true),
            _ => return false,
        }
        true
    }

    pub(super) fn move_thread_selection(&mut self, delta: isize) {
        let Some(view) = self.thread_view() else {
            return;
        };
        let ids: Vec<u64> = self
            .thread_messages(&view.thread_id)
            .iter()
            .map(|mail| mail.id)
            .collect();
        let Some(last) = ids.len().checked_sub(1) else {
            return;
        };
        let index = view
            .selected
            .and_then(|selected| ids.iter().position(|id| *id == selected))
            .map_or(0, |index| index.saturating_add_signed(delta).min(last));
        if let Some(view) = self.thread_view_mut() {
            view.selected = ids.get(index).copied();
        }
        self.reveal_selected_message();
    }

    /// Scroll the thread view to the highlighted message's header when the
    /// last frame did not show it.
    pub(super) fn reveal_selected_message(&mut self) {
        let Some(line) = self.selected_thread_line() else {
            return;
        };
        if line < self.detail.scroll || line >= self.detail.scroll + self.detail.shown {
            self.detail.scroll = line;
        }
    }

    pub(super) fn toggle_selected_message(&mut self) {
        let Some(view) = self.thread_view_mut() else {
            return;
        };
        if let Some(id) = view.selected {
            toggle_fold(view, id);
        }
    }

    pub(super) fn fold_all_messages(&mut self, fold: bool) {
        let Some(thread_id) = self.thread_view().map(|view| view.thread_id.clone()) else {
            return;
        };
        let ids: HashSet<u64> = if fold {
            HashSet::new()
        } else {
            self.thread_messages(&thread_id)
                .iter()
                .map(|mail| mail.id)
                .collect()
        };
        if let Some(view) = self.thread_view_mut() {
            view.unfolded = ids;
        }
        self.reveal_selected_message();
    }

    /// A click on a message header of the thread view highlights the
    /// message and folds or unfolds it. Returns whether it was on one.
    pub(in crate::app) fn click_thread(&mut self, column: u16, row: u16) -> bool {
        if self.thread_view().is_none() {
            return false;
        }
        let Some(rows) = self
            .areas
            .chat_rows
            .filter(|&rows| contains(Some(rows), column, row))
        else {
            return false;
        };
        let wrapped = self.areas.chat_rows_top + usize::from(row - rows.y);
        let Some(id) = self
            .inbox
            .thread_headers
            .iter()
            .find(|(header_row, _)| *header_row == wrapped)
            .map(|(_, id)| *id)
        else {
            return false;
        };
        let Some(view) = self.thread_view_mut() else {
            return false;
        };
        view.selected = Some(id);
        toggle_fold(view, id);
        true
    }
}

pub(super) fn toggle_fold(view: &mut ThreadView, id: u64) {
    if !view.unfolded.remove(&id) {
        view.unfolded.insert(id);
    }
}
