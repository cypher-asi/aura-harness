//! The Kernel panel's INBOX tab: the messages agents of the run on screen
//! sent each other, grouped into threads the way an email subject groups
//! mail.
//!
//! Messages come one at a time with [`UiCommand::AddMail`], for every
//! agent, and are kept here whichever agent is on screen. The tab lists
//! the threads with a message from or to an agent of the run on screen
//! (the agents of its MEMBERS tab), the most recently active first; an
//! unfolded thread lists its messages under it in the order they were
//! sent. A thread is unread while it has a message newer than the newest
//! one it had when it was last opened.
//!
//! A thread's subject is its first message's, until the title model
//! names it with [`UiCommand::SetThreadSubject`]. That name may come
//! before the first message does, so it is kept until the cap drops the
//! thread's messages.
//!
//! [`UiCommand::AddMail`]: crate::events::UiCommand::AddMail
//! [`UiCommand::SetThreadSubject`]: crate::events::UiCommand::SetThreadSubject

mod store;
mod thread;

use crate::app::wheel_views::WheelList;
use std::collections::{HashMap, HashSet};

use super::{contains, processes::SIDE_PAGE, App, PanelFocus, RecordsTab};
use crate::events::MailView;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;

/// Messages kept, across every agent; the oldest go first.
const MAX_MAIL: usize = 2000;

/// Characters of a message kept for its row's preview.
const PREVIEW_CHARS: usize = 200;

/// One row of the INBOX tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboxRow {
    /// A conversation, folded or unfolded.
    Thread {
        thread_id: String,
        subject: String,
        /// Messages in the thread
        count: usize,
        /// `Lead ↔ scout` when both wrote, `Lead → scout` when one did,
        /// the names joined by commas when more than two took part
        participants: String,
        /// When its newest message was sent, in Unix milliseconds
        last_ms: u64,
        /// Whether it has a message newer than when it was last opened
        unread: bool,
        /// Whether its messages are listed under it
        expanded: bool,
    },
    /// A message of an unfolded thread.
    Message {
        thread_id: String,
        message_id: u64,
        from_name: String,
        to_name: String,
        /// The text on one line, cut short
        preview: String,
        /// When it was sent, in Unix milliseconds
        sent_ms: u64,
        /// Whether it answers a message the recipient sent
        reply: bool,
    },
}

impl InboxRow {
    /// The thread the row is, or belongs to.
    #[must_use]
    pub fn thread_id(&self) -> &str {
        match self {
            Self::Thread { thread_id, .. } | Self::Message { thread_id, .. } => thread_id,
        }
    }

    /// What stays the same about the row across rebuilds, to keep the
    /// highlight on it.
    fn identity(&self) -> RowIdentity {
        match self {
            Self::Thread { thread_id, .. } => RowIdentity::Thread(thread_id.clone()),
            Self::Message { message_id, .. } => RowIdentity::Message(*message_id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RowIdentity {
    Thread(String),
    Message(u64),
}

/// State of the INBOX tab.
#[derive(Debug, Default)]
pub(crate) struct Inbox {
    /// Every message kept, in the order they came
    messages: Vec<MailView>,
    /// Threads whose messages are listed under them
    expanded: HashSet<String>,
    /// Newest message id of each thread when it was last opened
    read: HashMap<String, u64>,
    /// Subject the title model wrote for each thread, in place of its
    /// first message's
    subjects: HashMap<String, String>,
    /// Rows listed for the run on screen
    rows: Vec<InboxRow>,
    /// Highlighted row
    selected: usize,
    /// First row on screen
    pub(in crate::app) scroll: usize,
    /// Wrapped row of each message header the open thread view drew last
    /// frame, counted from the top of its content, to map clicks
    thread_headers: Vec<(usize, u64)>,
}

/// A thread shown over the chat panel (`DetailView::Thread`): its
/// messages stacked in the order they were sent, each folded to one line
/// or unfolded to its whole text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadView {
    /// The thread shown
    pub thread_id: String,
    /// Its subject, for the header
    pub subject: String,
    /// Ids of the messages shown whole; the rest show one line
    pub unfolded: HashSet<u64>,
    /// Id of the highlighted message, which `n`/`p` move and Enter folds
    pub selected: Option<u64>,
}

impl App {
    /// Rows of the INBOX tab: the threads of the run on screen, the most
    /// recently active first, each unfolded one followed by its messages.
    #[must_use]
    pub fn inbox_rows(&self) -> &[InboxRow] {
        &self.inbox.rows
    }

    /// Highlighted row of the INBOX tab.
    #[must_use]
    pub const fn selected_inbox_row(&self) -> usize {
        self.inbox.selected
    }

    /// Unread threads and all threads the INBOX tab lists, for its header.
    #[must_use]
    pub fn inbox_header_counts(&self) -> (usize, usize) {
        self.inbox
            .rows
            .iter()
            .fold((0, 0), |(unread, threads), row| match row {
                InboxRow::Thread { unread: true, .. } => (unread + 1, threads + 1),
                InboxRow::Thread { .. } => (unread, threads + 1),
                InboxRow::Message { .. } => (unread, threads),
            })
    }

    /// Keep the highlighted row inside a list `visible_rows` tall,
    /// returning the first row to show.
    pub fn sync_inbox_scroll(&mut self, visible_rows: usize) -> usize {
        self.inbox.scroll = self.panels.wheel.place(
            WheelList::Inbox,
            (self.inbox.scroll, self.inbox.selected),
            (visible_rows, self.inbox.rows.len()),
        );
        self.inbox.scroll
    }

    /// Record where the INBOX rows were drawn this frame and which row is
    /// at the top.
    pub fn set_inbox_rows_area(&mut self, area: Rect, scroll: usize) {
        self.areas.inbox_rows = Some(area);
        self.areas.inbox_scroll = scroll;
    }

    pub(super) fn inbox_row_at(&self, column: u16, row: u16) -> Option<usize> {
        let rows = self
            .areas
            .inbox_rows
            .filter(|&rows| contains(Some(rows), column, row))?;
        let index = self.areas.inbox_scroll + usize::from(row - rows.y);
        (index < self.inbox.rows.len()).then_some(index)
    }

    /// Show the Kernel panel's INBOX tab, focused, as `/inbox` does.
    /// CHANNELS has no Kernel panel, nor does BENCH but on an agent of a
    /// benchmark, so there this only says so: changing the layout flags
    /// here would leave a different layout behind when the user goes back
    /// to the agents.
    pub(super) fn show_inbox(&mut self) {
        if self.agent_panels_hidden() {
            self.notify_no_agent_panels();
            return;
        }
        self.panels.record_visible = true;
        self.panels.side_expanded = false;
        self.show_records_tab(RecordsTab::Inbox);
        self.focus = PanelFocus::Records;
    }

    /// File a message sent between agents under its thread. A message of
    /// the thread on screen shows unfolded at its bottom, already read.
    pub(super) fn add_mail(&mut self, mail: MailView) {
        let (id, thread_id) = (mail.id, mail.thread_id.clone());
        self.inbox.add(mail);
        if let Some(view) = self
            .thread_view_mut()
            .filter(|view| view.thread_id == thread_id)
        {
            view.unfolded.insert(id);
            self.mark_thread_read(&thread_id);
        }
        self.refresh_inbox();
    }

    /// Name `thread_id` with the subject the title model wrote, in the
    /// INBOX tab and in its thread view when that is open. A blank one is
    /// ignored, keeping whatever subject the thread showed.
    pub(super) fn set_thread_subject(&mut self, thread_id: String, subject: &str) {
        let subject = subject.trim();
        if subject.is_empty() {
            return;
        }
        if let Some(view) = self
            .thread_view_mut()
            .filter(|view| view.thread_id == thread_id)
        {
            view.subject = subject.to_string();
        }
        self.inbox.subjects.insert(thread_id, subject.to_string());
        self.refresh_inbox();
    }

    /// List the threads of the run on screen again, after the mail, the
    /// members or the agent on screen changed.
    pub(super) fn refresh_inbox(&mut self) {
        if self.active_agent_id.is_empty() {
            self.inbox.rebuild(None);
            return;
        }
        let agents: HashSet<&str> = self
            .members
            .rows()
            .iter()
            .map(|member| member.agent_id.as_str())
            .chain(std::iter::once(self.active_agent_id.as_str()))
            .collect();
        self.inbox.rebuild(Some(&agents));
    }

    /// Move the INBOX tab's highlight by `delta` rows, as the wheel does.
    pub(super) fn scroll_inbox(&mut self, delta: isize) {
        self.inbox.move_by(delta);
    }

    /// Click on row `index` of the INBOX tab: highlight it and open it.
    pub(super) fn click_inbox_row(&mut self, index: usize) {
        self.inbox.select(index);
        self.open_selected_inbox_row();
    }

    /// Navigation keys of the INBOX tab: arrows and pages move, Left
    /// folds the thread, Right unfolds it and Enter opens it.
    pub(super) fn handle_inbox_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.inbox.move_by(-1),
            KeyCode::Down => self.inbox.move_by(1),
            KeyCode::PageUp => self.inbox.move_by(-SIDE_PAGE),
            KeyCode::PageDown => self.inbox.move_by(SIDE_PAGE),
            KeyCode::Left => self.fold_selected_thread(),
            KeyCode::Right => self.unfold_selected_thread(),
            KeyCode::Enter => self.open_selected_inbox_row(),
            _ => {}
        }
    }

    /// Home/End of the INBOX tab, which would otherwise go to the empty
    /// input.
    pub(super) fn handle_inbox_shortcut(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Home => self.inbox.select(0),
            KeyCode::End => self.inbox.select(usize::MAX),
            _ => return false,
        }
        true
    }

    /// Fold the highlighted thread, or the thread of the highlighted
    /// message, which moves the highlight to the thread's row.
    fn fold_selected_thread(&mut self) {
        let Some(row) = self.inbox.rows.get(self.inbox.selected) else {
            return;
        };
        let thread_id = row.thread_id().to_string();
        if let Some(index) = self.inbox.rows.iter().position(
            |row| matches!(row, InboxRow::Thread { thread_id: id, .. } if *id == thread_id),
        ) {
            self.inbox.select(index);
        }
        if self.inbox.expanded.remove(&thread_id) {
            self.refresh_inbox();
        }
    }

    /// Unfold the highlighted thread.
    fn unfold_selected_thread(&mut self) {
        let Some(InboxRow::Thread { thread_id, .. }) = self.inbox.rows.get(self.inbox.selected)
        else {
            return;
        };
        if self.inbox.expanded.insert(thread_id.clone()) {
            self.refresh_inbox();
        }
    }

    fn open_selected_inbox_row(&mut self) {
        let Some(row) = self.inbox.rows.get(self.inbox.selected) else {
            return;
        };
        let (thread_id, message) = match row {
            InboxRow::Thread { thread_id, .. } => (thread_id.clone(), None),
            InboxRow::Message {
                thread_id,
                message_id,
                ..
            } => (thread_id.clone(), Some(*message_id)),
        };
        self.open_thread(thread_id, message);
    }
}

#[cfg(test)]
mod tests;
