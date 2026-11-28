//! What HISTORY shows of each message: the message itself when it is
//! short, else a one-line summary the session has a small model write
//! once and keep ([`UiEvent::SummarizeMessages`]).
//!
//! A message is long past [`SUMMARY_THRESHOLD_CHARS`] characters, counted
//! on one line. Until its summary comes, and for good when none comes
//! (the call failed or timed out), it shows cut at a word, with `…`, to
//! that many characters. Summaries are asked for only for the rows drawn,
//! each once a run; the session answers from its cache when it can.

use super::HistoryEntry;
use crate::app::App;
use crate::events::{MessageSummaryRequest, MessageSummaryView, UiEvent};
use std::borrow::Cow;
use std::ops::Range;

/// Longest message, in characters on one line, HISTORY shows as it is.
pub(crate) const SUMMARY_THRESHOLD_CHARS: usize = 80;

/// What marks a message cut short.
const ELLIPSIS: char = '…';

/// Whether `entry` is too long to show as it is.
fn is_long(entry: &HistoryEntry) -> bool {
    entry.line.chars().count() > SUMMARY_THRESHOLD_CHARS
}

/// The key a long message's summary is filed under.
fn summary_key(entry: &HistoryEntry) -> String {
    format!("{:016x}", super::text_digest(&entry.text))
}

/// `line` cut at a word, then `…`, to at most [`SUMMARY_THRESHOLD_CHARS`]
/// characters in all; a word longer than half of that is cut inside.
pub(crate) fn cut_at_word(line: &str) -> String {
    let max = SUMMARY_THRESHOLD_CHARS - 1;
    let mut cut: String = line.chars().take(max).collect();
    let mid_word = line.chars().nth(max).is_some_and(|c| !c.is_whitespace());
    if mid_word {
        if let Some(space) = cut
            .rfind(' ')
            .filter(|&at| cut[..at].chars().count() >= max / 2)
        {
            cut.truncate(space);
        }
    }
    let mut out = cut
        .trim_end_matches([' ', ',', ';', ':', '-', '.'])
        .to_string();
    out.push(ELLIPSIS);
    out
}

/// What HISTORY shows of `entry`: itself when short, else its summary in
/// `summaries`, else its start cut at a word.
pub(crate) fn history_line<'a>(
    entry: &'a HistoryEntry,
    summaries: &'a std::collections::HashMap<String, String>,
) -> Cow<'a, str> {
    if !is_long(entry) {
        return Cow::Borrowed(&entry.line);
    }
    summaries.get(&summary_key(entry)).map_or_else(
        || Cow::Owned(cut_at_word(&entry.line)),
        |s| Cow::Borrowed(s.as_str()),
    )
}

impl App {
    /// What HISTORY shows of `entry`.
    #[must_use]
    pub(crate) fn channel_history_line<'a>(&'a self, entry: &'a HistoryEntry) -> Cow<'a, str> {
        history_line(entry, &self.channels.history.summaries)
    }

    /// Ask the session for the summaries of the long messages among the
    /// entries `rows` of HISTORY that have none and were not asked for
    /// yet, through the open channel's coordinator.
    pub(crate) fn request_channel_history_summaries(&mut self, rows: Range<usize>) {
        let Some(coordinator) = self.open_channel().map(|c| c.coordinator_id.clone()) else {
            return;
        };
        let history = &self.channels.history;
        let requests: Vec<MessageSummaryRequest> = history
            .list
            .items
            .get(rows.start..rows.end.min(history.list.len()))
            .unwrap_or_default()
            .iter()
            .filter(|entry| is_long(entry))
            .map(|entry| MessageSummaryRequest {
                key: summary_key(entry),
                text: entry.text.clone(),
            })
            .filter(|request| {
                !history.summaries.contains_key(&request.key)
                    && !history.requested.contains(&request.key)
            })
            .collect();
        if requests.is_empty() || coordinator.is_empty() {
            return;
        }
        let keys: Vec<String> = requests.iter().map(|r| r.key.clone()).collect();
        let event = UiEvent::SummarizeMessages {
            agent_id: coordinator,
            requests,
        };
        if self.try_send_event(event) {
            self.channels.history.requested.extend(keys);
        }
    }

    /// Keep the summary the session sent. None (the call failed) leaves
    /// the message cut, and it is not asked for again this run.
    pub(in crate::app) fn set_message_summary(&mut self, view: MessageSummaryView) {
        if let Some(summary) = view.summary.filter(|s| !s.trim().is_empty()) {
            self.channels.history.summaries.insert(view.key, summary);
        }
    }
}
