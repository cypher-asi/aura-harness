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
//! [`UiCommand::AddMail`]: crate::events::UiCommand::AddMail

use std::collections::{HashMap, HashSet};

use super::{
    contains, follow_selection, processes::SIDE_PAGE, App, DetailView, PanelFocus, RecordsTab,
};
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
    /// Rows listed for the run on screen
    rows: Vec<InboxRow>,
    /// Highlighted row
    selected: usize,
    /// First row on screen
    scroll: usize,
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

/// Source lines message `mail` takes in a thread view: its header, plus
/// its body when `unfolded`. The renderer lays messages out the same way,
/// with one blank line between them.
#[must_use]
fn thread_message_lines(mail: &MailView, unfolded: bool) -> usize {
    if unfolded {
        1 + mail.text.lines().count()
    } else {
        1
    }
}

impl Inbox {
    /// Keep `mail`, in place of a message with its id already kept. When
    /// the cap drops old messages, the fold and read state of threads
    /// left with none is forgotten too.
    fn add(&mut self, mail: MailView) {
        match self.messages.iter_mut().find(|kept| kept.id == mail.id) {
            Some(kept) => *kept = mail,
            None => self.messages.push(mail),
        }
        let over = self.messages.len().saturating_sub(MAX_MAIL);
        if over == 0 {
            return;
        }
        self.messages.drain(..over);
        let kept: HashSet<&str> = self
            .messages
            .iter()
            .map(|mail| mail.thread_id.as_str())
            .collect();
        self.expanded
            .retain(|thread| kept.contains(thread.as_str()));
        self.read.retain(|thread, _| kept.contains(thread.as_str()));
    }

    /// Newest message id of `thread_id`, if any message of it is kept.
    fn newest_id(&self, thread_id: &str) -> Option<u64> {
        self.messages
            .iter()
            .filter(|mail| mail.thread_id == thread_id)
            .map(|mail| mail.id)
            .max()
    }

    /// List the threads with a message from or to one of `agents` (every
    /// thread when `None`), keeping the highlight on the same row when it
    /// is still listed.
    fn rebuild(&mut self, agents: Option<&HashSet<&str>>) {
        let kept = self.rows.get(self.selected).map(InboxRow::identity);
        self.rows = self.rows_for(agents);
        self.selected = kept
            .and_then(|kept| self.rows.iter().position(|row| row.identity() == kept))
            .unwrap_or(self.selected)
            .min(self.rows.len().saturating_sub(1));
    }

    fn rows_for(&self, agents: Option<&HashSet<&str>>) -> Vec<InboxRow> {
        let mut threads: HashMap<&str, Vec<&MailView>> = HashMap::new();
        for mail in &self.messages {
            threads.entry(&mail.thread_id).or_default().push(mail);
        }
        let mut threads: Vec<Vec<&MailView>> = threads
            .into_values()
            .filter(|thread| {
                agents.is_none_or(|agents| {
                    thread.iter().any(|mail| {
                        agents.contains(mail.from_id.as_str())
                            || agents.contains(mail.to_id.as_str())
                    })
                })
            })
            .collect();
        for thread in &mut threads {
            thread.sort_by_key(|mail| mail.id);
        }
        threads.sort_by_key(|thread| {
            let last_ms = thread.iter().map(|mail| mail.sent_ms).max().unwrap_or(0);
            let last_id = thread.last().map_or(0, |mail| mail.id);
            std::cmp::Reverse((last_ms, last_id))
        });

        let mut rows = Vec::new();
        for thread in threads {
            let Some(row) = self.thread_row(&thread) else {
                continue;
            };
            let expanded = matches!(row, InboxRow::Thread { expanded: true, .. });
            rows.push(row);
            if expanded {
                rows.extend(thread.iter().map(|mail| message_row(mail)));
            }
        }
        rows
    }

    /// The row of `thread`, its messages in the order they were sent.
    fn thread_row(&self, thread: &[&MailView]) -> Option<InboxRow> {
        let first = thread.first()?;
        let last = thread.last()?;
        let read = self.read.get(&first.thread_id).copied().unwrap_or(0);
        Some(InboxRow::Thread {
            thread_id: first.thread_id.clone(),
            subject: first.subject.clone(),
            count: thread.len(),
            participants: participants(thread),
            last_ms: thread.iter().map(|mail| mail.sent_ms).max().unwrap_or(0),
            unread: last.id > read,
            expanded: self.expanded.contains(&first.thread_id),
        })
    }

    fn select(&mut self, index: usize) {
        self.selected = index.min(self.rows.len().saturating_sub(1));
    }

    fn move_by(&mut self, delta: isize) {
        self.select(self.selected.saturating_add_signed(delta));
    }
}

/// Who took part in `thread`, its messages in the order they were sent.
fn participants(thread: &[&MailView]) -> String {
    let mut names: Vec<(&str, &str)> = Vec::new();
    for mail in thread {
        for (id, name) in [
            (&mail.from_id, &mail.from_name),
            (&mail.to_id, &mail.to_name),
        ] {
            if !names.iter().any(|(seen, _)| *seen == id.as_str()) {
                names.push((id, name));
            }
        }
    }
    match names.as_slice() {
        [(first_id, first), (_, second)] => {
            let both_ways = thread.iter().any(|mail| mail.from_id != *first_id);
            let arrow = if both_ways { "↔" } else { "→" };
            format!("{first} {arrow} {second}")
        }
        _ => names
            .iter()
            .map(|(_, name)| *name)
            .collect::<Vec<_>>()
            .join(", "),
    }
}

fn message_row(mail: &MailView) -> InboxRow {
    let preview: String = mail
        .text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(PREVIEW_CHARS)
        .collect();
    InboxRow::Message {
        thread_id: mail.thread_id.clone(),
        message_id: mail.id,
        from_name: mail.from_name.clone(),
        to_name: mail.to_name.clone(),
        preview,
        sent_ms: mail.sent_ms,
        reply: mail.reply,
    }
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
        self.inbox.scroll = follow_selection(
            self.inbox.scroll,
            self.inbox.selected,
            visible_rows,
            self.inbox.rows.len(),
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

    /// Show the Kernel panel's INBOX tab, focused, as `/inbox` does. While
    /// a channel is open its panel owns the column, so this only says so:
    /// changing the layout flags here would leave a different layout behind
    /// when the channel closes.
    pub(super) fn show_inbox(&mut self) {
        if self.channel_open() {
            self.notify_channel_owns_column();
            return;
        }
        self.record_panel_visible = true;
        self.side_expanded = false;
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

    /// Show `thread_id` over the chat with `focus_message` (when it is of
    /// the thread) and its newest message unfolded, `focus_message`
    /// highlighted or else the newest, and mark the thread read.
    pub(super) fn open_thread(&mut self, thread_id: String, focus_message: Option<u64>) {
        let messages = self.thread_messages(&thread_id);
        let subject = messages
            .first()
            .map(|mail| mail.subject.clone())
            .unwrap_or_default();
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
        self.detail_scroll = self.selected_thread_line().unwrap_or(0);
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

    fn mark_thread_read(&mut self, thread_id: &str) {
        if let Some(newest) = self.inbox.newest_id(thread_id) {
            let read = self.inbox.read.entry(thread_id.to_string()).or_default();
            *read = (*read).max(newest);
        }
    }

    fn thread_view_mut(&mut self) -> Option<&mut ThreadView> {
        match self.detail.as_mut() {
            Some(DetailView::Thread(view)) => Some(view.as_mut()),
            _ => None,
        }
    }

    fn thread_view(&self) -> Option<&ThreadView> {
        match self.detail.as_ref() {
            Some(DetailView::Thread(view)) => Some(view.as_ref()),
            _ => None,
        }
    }

    /// Source line of the highlighted message's header in the thread view.
    fn selected_thread_line(&self) -> Option<usize> {
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
    pub(super) fn handle_thread_key(&mut self, key: KeyEvent) -> bool {
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

    fn move_thread_selection(&mut self, delta: isize) {
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
    fn reveal_selected_message(&mut self) {
        let Some(line) = self.selected_thread_line() else {
            return;
        };
        if line < self.detail_scroll || line >= self.detail_scroll + self.detail_shown {
            self.detail_scroll = line;
        }
    }

    fn toggle_selected_message(&mut self) {
        let Some(view) = self.thread_view_mut() else {
            return;
        };
        if let Some(id) = view.selected {
            toggle_fold(view, id);
        }
    }

    fn fold_all_messages(&mut self, fold: bool) {
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
    pub(super) fn click_thread(&mut self, column: u16, row: u16) -> bool {
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

fn toggle_fold(view: &mut ThreadView, id: u64) {
    if !view.unfolded.remove(&id) {
        view.unfolded.insert(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{AgentSummary, ProcessKind, ProcessStatus, ProcessSummary, UiCommand};
    use crossterm::event::KeyModifiers;

    const LEAD: &str = "aa";

    fn mail(id: u64, thread: &str, from: &str, to: &str, sent_ms: u64) -> MailView {
        MailView {
            id,
            thread_id: thread.into(),
            subject: format!("About {thread}"),
            from_id: from.into(),
            from_name: from.to_uppercase(),
            to_id: to.into(),
            to_name: to.to_uppercase(),
            reply: false,
            text: format!("message {id}\nsecond line"),
            sent_ms,
        }
    }

    fn task(id: &str, owner: &str) -> ProcessSummary {
        ProcessSummary {
            id: format!("task:{id}"),
            kind: ProcessKind::Task,
            label: id.into(),
            status: ProcessStatus::Running,
            owner_agent_id: owner.into(),
            started_ms: 10,
            ..ProcessSummary::default()
        }
    }

    fn app_with_lead() -> App {
        let mut app = App::new();
        app.process_command(UiCommand::SetAgents(vec![AgentSummary {
            id: LEAD.into(),
            name: "Lead".into(),
            ..AgentSummary::default()
        }]));
        app.process_command(UiCommand::SetActiveAgent(LEAD.into()));
        app.process_command(UiCommand::SetProcesses(vec![task("bb", LEAD)]));
        app
    }

    fn send(app: &mut App, mail: MailView) {
        app.process_command(UiCommand::AddMail(mail));
    }

    fn threads(app: &App) -> Vec<String> {
        app.inbox_rows()
            .iter()
            .filter_map(|row| match row {
                InboxRow::Thread { thread_id, .. } => Some(thread_id.clone()),
                InboxRow::Message { .. } => None,
            })
            .collect()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    /// The thread shown over the chat and its highlighted message.
    fn open_thread(app: &App) -> Option<(String, Option<u64>)> {
        let view = app.thread_view()?;
        Some((view.thread_id.clone(), view.selected))
    }

    fn unfolded(app: &App) -> Vec<u64> {
        let mut ids: Vec<u64> = app
            .thread_view()
            .map(|view| view.unfolded.iter().copied().collect())
            .unwrap_or_default();
        ids.sort_unstable();
        ids
    }

    /// Lead's run with thread t1 of messages 1, 2 and 3 on screen,
    /// opened at message 1.
    fn app_with_open_thread() -> App {
        let mut app = app_with_lead();
        send(&mut app, mail(1, "t1", LEAD, "bb", 100));
        send(&mut app, mail(2, "t1", "bb", LEAD, 200));
        send(&mut app, mail(3, "t1", LEAD, "bb", 300));
        app.open_thread("t1".into(), Some(1));
        app
    }

    #[test]
    fn messages_group_by_thread_and_threads_order_by_last_activity() {
        let mut app = app_with_lead();
        send(&mut app, mail(1, "t1", LEAD, "bb", 100));
        send(&mut app, mail(2, "t2", LEAD, "bb", 200));
        send(&mut app, mail(3, "t1", "bb", LEAD, 300));
        assert_eq!(threads(&app), ["t1", "t2"]);
        assert!(matches!(
            &app.inbox_rows()[0],
            InboxRow::Thread { count: 2, participants, last_ms: 300, expanded: false, .. }
                if participants == "AA ↔ BB"
        ));
        assert!(matches!(
            &app.inbox_rows()[1],
            InboxRow::Thread { count: 1, participants, .. } if participants == "AA → BB"
        ));
    }

    #[test]
    fn only_threads_of_the_run_on_screen_are_listed() {
        let mut app = app_with_lead();
        send(&mut app, mail(1, "ours", "bb", LEAD, 100));
        send(&mut app, mail(2, "theirs", "ff", "gg", 200));
        assert_eq!(threads(&app), ["ours"]);

        // A member joining brings its threads in.
        app.process_command(UiCommand::SetProcesses(vec![
            task("bb", LEAD),
            task("ff", LEAD),
        ]));
        assert_eq!(threads(&app), ["theirs", "ours"]);

        // Another agent on screen lists its own run's threads.
        app.process_command(UiCommand::SetActiveAgent("zz".into()));
        assert!(threads(&app).is_empty());
    }

    #[test]
    fn every_thread_is_listed_with_no_agent_on_screen() {
        let mut app = App::new();
        send(&mut app, mail(1, "t1", "ff", "gg", 100));
        assert_eq!(threads(&app), ["t1"]);
    }

    #[test]
    fn right_unfolds_a_thread_and_left_folds_it_from_a_message() {
        let mut app = app_with_lead();
        send(&mut app, mail(1, "t1", LEAD, "bb", 100));
        send(&mut app, mail(2, "t1", "bb", LEAD, 200));
        app.show_inbox();
        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.inbox_rows().len(), 3);
        assert!(matches!(
            &app.inbox_rows()[1],
            InboxRow::Message { message_id: 1, preview, .. } if preview == "message 1 second line"
        ));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.selected_inbox_row(), 2);
        app.handle_key(key(KeyCode::Left));
        assert_eq!(app.inbox_rows().len(), 1);
        assert_eq!(app.selected_inbox_row(), 0);
        assert!(matches!(
            &app.inbox_rows()[0],
            InboxRow::Thread {
                expanded: false,
                ..
            }
        ));
    }

    #[test]
    fn enter_on_a_message_opens_its_thread_there_and_marks_it_read() {
        let mut app = app_with_lead();
        send(&mut app, mail(1, "t1", LEAD, "bb", 100));
        send(&mut app, mail(2, "t1", "bb", LEAD, 200));
        send(&mut app, mail(3, "t2", "bb", LEAD, 50));
        assert_eq!(app.inbox_header_counts(), (2, 2));
        app.show_inbox();
        app.handle_key(key(KeyCode::Right));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(open_thread(&app), Some(("t1".into(), Some(1))));
        assert_eq!(app.inbox_header_counts(), (1, 2));

        // A thread row opens at its newest message.
        app.handle_key(key(KeyCode::Esc));
        assert!(app.handle_records_shortcut(key(KeyCode::End)));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(open_thread(&app), Some(("t2".into(), Some(3))));
        assert_eq!(app.inbox_header_counts(), (0, 2));
    }

    #[test]
    fn new_mail_makes_a_read_thread_unread() {
        let mut app = app_with_lead();
        send(&mut app, mail(1, "t1", LEAD, "bb", 100));
        app.open_thread("t1".into(), None);
        assert_eq!(app.inbox_header_counts(), (0, 1));
        app.handle_key(key(KeyCode::Esc));
        assert!(app.detail_view().is_none());
        send(&mut app, mail(2, "t1", "bb", LEAD, 200));
        assert_eq!(app.inbox_header_counts(), (1, 1));
    }

    #[test]
    fn clicking_a_row_selects_and_opens_it() {
        let mut app = app_with_lead();
        send(&mut app, mail(1, "t1", LEAD, "bb", 100));
        send(&mut app, mail(2, "t2", LEAD, "bb", 200));
        app.show_inbox();
        app.set_inbox_rows_area(Rect::new(80, 3, 40, 10), 0);
        app.click_records_panel(85, 4);
        assert_eq!(app.selected_inbox_row(), 1);
        assert_eq!(open_thread(&app), Some(("t1".into(), Some(1))));
    }

    #[test]
    fn the_highlight_stays_on_its_thread_as_mail_arrives() {
        let mut app = app_with_lead();
        send(&mut app, mail(1, "t1", LEAD, "bb", 100));
        send(&mut app, mail(2, "t2", LEAD, "bb", 200));
        app.show_inbox();
        app.handle_key(key(KeyCode::Down));
        send(&mut app, mail(3, "t3", LEAD, "bb", 300));
        let selected = &app.inbox_rows()[app.selected_inbox_row()];
        assert_eq!(selected.thread_id(), "t1");
    }

    #[test]
    fn shift_right_cycles_through_all_four_tabs() {
        let mut app = app_with_lead();
        let right = KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT);
        let left = KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT);
        for tab in [
            RecordsTab::Members,
            RecordsTab::Policies,
            RecordsTab::Inbox,
            RecordsTab::Kernel,
        ] {
            assert!(app.handle_records_shortcut(right));
            assert_eq!(app.records_tab(), tab);
        }
        assert!(app.handle_records_shortcut(left));
        assert_eq!(app.records_tab(), RecordsTab::Inbox);
    }

    #[test]
    fn the_inbox_command_shows_the_tab_focused() {
        for command in ["/inbox", "/mail"] {
            let mut app = app_with_lead();
            app.handle_command("/kernel");
            assert!(!app.record_panel_visible());
            app.handle_command(command);
            assert!(app.record_panel_visible());
            assert_eq!(app.records_tab(), RecordsTab::Inbox);
            assert_eq!(app.focus(), PanelFocus::Records);
        }
    }

    #[test]
    fn mail_is_deduplicated_by_id_and_capped() {
        let mut app = App::new();
        send(&mut app, mail(1, "t1", "ff", "gg", 100));
        send(&mut app, mail(1, "t1", "ff", "gg", 100));
        assert!(matches!(
            &app.inbox_rows()[0],
            InboxRow::Thread { count: 1, .. }
        ));

        for id in 2..=u64::try_from(MAX_MAIL).unwrap_or(u64::MAX) + 1 {
            send(&mut app, mail(id, "t2", "ff", "gg", 200 + id));
        }
        assert_eq!(app.inbox.messages.len(), MAX_MAIL);
        assert_eq!(threads(&app), ["t2"], "the oldest message went first");
    }

    #[test]
    fn capped_mail_forgets_the_state_of_threads_it_dropped() {
        let mut app = App::new();
        send(&mut app, mail(1, "old", "ff", "gg", 100));
        send(&mut app, mail(2, "kept", "ff", "gg", 100));
        app.open_thread("old".into(), None);
        app.open_thread("kept".into(), None);
        app.inbox.expanded.insert("old".into());
        app.inbox.expanded.insert("kept".into());

        for id in 3..=u64::try_from(MAX_MAIL).unwrap_or(u64::MAX) + 1 {
            send(&mut app, mail(id, "new", "ff", "gg", 200 + id));
        }
        assert_eq!(threads(&app), ["new", "kept"]);
        assert!(!app.inbox.expanded.contains("old"));
        assert!(!app.inbox.read.contains_key("old"));
        assert!(app.inbox.expanded.contains("kept"));
        assert!(app.inbox.read.contains_key("kept"));
    }

    #[test]
    fn opening_a_thread_unfolds_and_selects_the_focused_and_newest_message() {
        let app = app_with_open_thread();
        let Some(DetailView::Thread(view)) = app.detail_view() else {
            panic!("no thread view");
        };
        assert_eq!(view.subject, "About t1");
        assert_eq!(view.selected, Some(1));
        assert_eq!(unfolded(&app), [1, 3]);
        assert_eq!(
            app.thread_messages("t1")
                .iter()
                .map(|mail| mail.id)
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
    }

    #[test]
    fn opening_a_thread_without_focus_selects_its_newest_message() {
        let mut app = app_with_open_thread();
        app.open_thread("t1".into(), Some(99));
        assert_eq!(open_thread(&app), Some(("t1".into(), Some(3))));
        assert_eq!(unfolded(&app), [3]);
    }

    #[test]
    fn thread_keys_move_the_selection_and_fold_messages() {
        let mut app = app_with_open_thread();
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(unfolded(&app), [3], "Enter folds the selected message");
        assert!(app.detail_view().is_some(), "Enter does not close the view");
        app.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(unfolded(&app), [1, 3], "Space unfolds it again");

        app.handle_key(key(KeyCode::Char('n')));
        assert_eq!(open_thread(&app), Some(("t1".into(), Some(2))));
        app.handle_key(key(KeyCode::Char('n')));
        app.handle_key(key(KeyCode::Char('n')));
        assert_eq!(
            open_thread(&app),
            Some(("t1".into(), Some(3))),
            "n stops at the last"
        );
        app.handle_key(key(KeyCode::Char('p')));
        assert_eq!(open_thread(&app), Some(("t1".into(), Some(2))));

        app.handle_key(key(KeyCode::Char('e')));
        assert_eq!(unfolded(&app), [1, 2, 3]);
        app.handle_key(key(KeyCode::Char('c')));
        assert!(unfolded(&app).is_empty());
        assert_eq!(app.input(), "", "thread keys stay out of the input");
    }

    #[test]
    fn thread_selection_scrolls_to_the_selected_header() {
        let mut app = app_with_open_thread();
        app.set_detail_bounds(100, 1);
        // Message 1 unfolded: header + 2 body lines, then a blank line.
        app.handle_key(key(KeyCode::Char('n')));
        assert_eq!(app.detail_scroll(), 4);
        app.handle_key(key(KeyCode::Char('n')));
        assert_eq!(app.detail_scroll(), 6);
    }

    #[test]
    fn new_mail_in_the_open_thread_shows_unfolded_and_read() {
        let mut app = app_with_open_thread();
        send(&mut app, mail(4, "t1", "bb", LEAD, 400));
        send(&mut app, mail(5, "t2", "bb", LEAD, 500));
        assert_eq!(unfolded(&app), [1, 3, 4]);
        assert_eq!(app.inbox_header_counts(), (1, 2), "only t2 is unread");
    }

    #[test]
    fn clicking_a_thread_header_toggles_its_message() {
        let mut app = app_with_open_thread();
        app.set_chat_rows(Some((Rect::new(2, 1, 40, 20), 0)));
        app.set_thread_header_rows(vec![(0, 1), (4, 2), (6, 3)]);
        assert!(app.click_thread(5, 5));
        assert_eq!(open_thread(&app), Some(("t1".into(), Some(2))));
        assert_eq!(unfolded(&app), [1, 2, 3]);
        assert!(app.click_thread(5, 1));
        assert_eq!(unfolded(&app), [2, 3]);
        assert!(!app.click_thread(5, 2), "a body line is not a header");
    }
}
