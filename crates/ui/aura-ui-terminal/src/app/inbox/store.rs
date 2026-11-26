//! The INBOX tab's store: the messages kept and the rows listed.

use std::collections::{HashMap, HashSet};

use super::{Inbox, InboxRow, MAX_MAIL, PREVIEW_CHARS};
use crate::events::MailView;

impl Inbox {
    /// Keep `mail`, in place of a message with its id already kept. When
    /// the cap drops old messages, the fold, read and subject state of
    /// threads left with none is forgotten too. Only those threads: a
    /// subject can come before its thread's first message, so a thread
    /// with no messages yet keeps it.
    pub(super) fn add(&mut self, mail: MailView) {
        match self.messages.iter_mut().find(|kept| kept.id == mail.id) {
            Some(kept) => *kept = mail,
            None => self.messages.push(mail),
        }
        let over = self.messages.len().saturating_sub(MAX_MAIL);
        if over == 0 {
            return;
        }
        let dropped: HashSet<String> = self
            .messages
            .drain(..over)
            .map(|mail| mail.thread_id)
            .collect();
        let kept: HashSet<&str> = self
            .messages
            .iter()
            .map(|mail| mail.thread_id.as_str())
            .collect();
        self.expanded
            .retain(|thread| kept.contains(thread.as_str()));
        self.read.retain(|thread, _| kept.contains(thread.as_str()));
        self.subjects
            .retain(|thread, _| !dropped.contains(thread) || kept.contains(thread.as_str()));
    }

    /// The subject of `thread_id`: the title model's when it wrote one,
    /// else `first`'s, the subject of the thread's first message.
    pub(super) fn subject<'a>(&'a self, thread_id: &str, first: &'a str) -> &'a str {
        self.subjects.get(thread_id).map_or(first, String::as_str)
    }

    /// Newest message id of `thread_id`, if any message of it is kept.
    pub(super) fn newest_id(&self, thread_id: &str) -> Option<u64> {
        self.messages
            .iter()
            .filter(|mail| mail.thread_id == thread_id)
            .map(|mail| mail.id)
            .max()
    }

    /// List the threads with a message from or to one of `agents` (every
    /// thread when `None`), keeping the highlight on the same row when it
    /// is still listed.
    pub(super) fn rebuild(&mut self, agents: Option<&HashSet<&str>>) {
        let kept = self.rows.get(self.selected).map(InboxRow::identity);
        self.rows = self.rows_for(agents);
        self.selected = kept
            .and_then(|kept| self.rows.iter().position(|row| row.identity() == kept))
            .unwrap_or(self.selected)
            .min(self.rows.len().saturating_sub(1));
    }

    pub(super) fn rows_for(&self, agents: Option<&HashSet<&str>>) -> Vec<InboxRow> {
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
    pub(super) fn thread_row(&self, thread: &[&MailView]) -> Option<InboxRow> {
        let first = thread.first()?;
        let last = thread.last()?;
        let read = self.read.get(&first.thread_id).copied().unwrap_or(0);
        Some(InboxRow::Thread {
            thread_id: first.thread_id.clone(),
            subject: self.subject(&first.thread_id, &first.subject).to_string(),
            count: thread.len(),
            participants: participants(thread),
            last_ms: thread.iter().map(|mail| mail.sent_ms).max().unwrap_or(0),
            unread: last.id > read,
            expanded: self.expanded.contains(&first.thread_id),
        })
    }

    pub(super) fn select(&mut self, index: usize) {
        self.selected = index.min(self.rows.len().saturating_sub(1));
    }

    pub(super) fn move_by(&mut self, delta: isize) {
        self.select(self.selected.saturating_add_signed(delta));
    }
}

/// Who took part in `thread`, its messages in the order they were sent.
pub(super) fn participants(thread: &[&MailView]) -> String {
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

pub(super) fn message_row(mail: &MailView) -> InboxRow {
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
