//! Chat lines for each message, kept between frames.

use super::activity::activity_lines;
use super::command::shell_lines;
use super::markdown::{parse_content_segments, ContentSegment};
use super::segments::{message_header, message_style, render_code_segment, render_text_segment};
use crate::components::Message;
use crate::Theme;
use ratatui::text::Line;
use std::collections::{HashMap, HashSet, VecDeque};

/// The lines each chat message is drawn as, so a frame only formats the
/// messages that are new or changed since the last one.
#[derive(Debug, Default)]
pub struct ChatLineCache {
    /// Text width the cached lines were wrapped to
    width: usize,
    /// Theme the cached lines were styled with
    theme: Option<Theme>,
    /// Lines per message id, with the revision they were made from
    entries: HashMap<u64, (u64, Vec<Line<'static>>)>,
}

impl ChatLineCache {
    /// Bring the cache up to date with `messages` drawn `width` columns wide.
    /// Activity still in progress is redrawn every frame, with `spinner`.
    ///
    /// Returns `true` when every message was reflowed because the width or
    /// theme changed, so line positions from the last frame no longer apply.
    pub fn update(
        &mut self,
        messages: &VecDeque<Message>,
        width: usize,
        theme: &Theme,
        spinner: &str,
    ) -> bool {
        let reflowed = self.width != width || self.theme.as_ref() != Some(theme);
        if reflowed {
            self.width = width;
            self.theme = Some(theme.clone());
            self.entries.clear();
        }
        for message in messages {
            let (id, revision) = message.version();
            if message.is_live_activity()
                || self
                    .entries
                    .get(&id)
                    .is_none_or(|(cached, _)| *cached != revision)
            {
                self.entries.insert(
                    id,
                    (revision, message_lines(message, width, theme, spinner)),
                );
            }
        }
        if self.entries.len() > messages.len() {
            let live: HashSet<u64> = messages.iter().map(|m| m.version().0).collect();
            self.entries.retain(|id, _| live.contains(id));
        }
        reflowed
    }

    /// Lines for `message` as of the last [`Self::update`].
    pub fn lines(&self, message: &Message) -> &[Line<'static>] {
        self.entries
            .get(&message.version().0)
            .map_or(&[], |(_, lines)| lines.as_slice())
    }
}

/// Whether a blank line goes above message `index`: between every two
/// messages, except that consecutive activity blocks sit together.
pub(super) fn gap_before(messages: &VecDeque<Message>, index: usize) -> bool {
    let is_activity = |i: usize| messages.get(i).is_some_and(|m| m.activity_step().is_some());
    index > 0 && !(is_activity(index) && is_activity(index - 1))
}

/// The header and body lines of one chat message.
pub(super) fn message_lines(
    message: &Message,
    width: usize,
    theme: &Theme,
    spinner: &str,
) -> Vec<Line<'static>> {
    if let Some(activity) = message.activity_step() {
        return activity_lines(activity, message.is_expanded(), spinner, width, theme);
    }
    let timestamp = message.timestamp_local();
    if let Some(run) = message.shell_run() {
        return shell_lines(run, &timestamp, width, theme);
    }
    let (nick, nick_color, msg_color) = message_style(message.role(), theme);
    let mut lines = vec![message_header(nick, nick_color, &timestamp, theme)];
    for segment in parse_content_segments(message.content()) {
        match segment {
            ContentSegment::Text(text) => {
                render_text_segment(&mut lines, &text, msg_color, width, message.role(), theme);
            }
            ContentSegment::CodeBlock { language, code } => {
                render_code_segment(&mut lines, &language, &code, width, theme);
            }
        }
    }
    lines
}
