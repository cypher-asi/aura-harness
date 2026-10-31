//! Message component for chat bubbles.

use crate::themes::Theme;
use chrono::{DateTime, Local, Utc};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::components::Activity;
use crate::events::MessageRole;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_MESSAGE_ID: AtomicU64 = AtomicU64::new(0);

fn next_message_id() -> u64 {
    NEXT_MESSAGE_ID.fetch_add(1, Ordering::Relaxed)
}

/// A chat message.
#[derive(Debug)]
pub struct Message {
    /// Message role
    role: MessageRole,
    /// Message content
    content: String,
    /// Timestamp
    timestamp: DateTime<Utc>,
    /// Whether the message is still streaming
    is_streaming: bool,
    /// Set when the message is a shell command the user ran
    shell: Option<ShellRun>,
    /// Set when the message is a step the agent took (thinking, a tool call)
    activity: Option<Activity>,
    /// Whether an activity block shows its full detail
    expanded: bool,
    /// Unique to this message, clones included
    id: u64,
    /// Bumped on every change
    revision: u64,
}

impl Clone for Message {
    fn clone(&self) -> Self {
        Self {
            role: self.role,
            content: self.content.clone(),
            timestamp: self.timestamp,
            is_streaming: self.is_streaming,
            shell: self.shell.clone(),
            activity: self.activity.clone(),
            expanded: self.expanded,
            id: next_message_id(),
            revision: 0,
        }
    }
}

/// A shell command typed by the user and its terminal output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellRun {
    /// Command line as typed
    pub command: String,
    /// Terminal output (may contain ANSI styling); `None` while running
    pub output: Option<String>,
    /// Exit code of a failed command
    pub exit_code: Option<i32>,
}

impl Message {
    /// Create a new message.
    #[must_use]
    pub fn new(role: MessageRole, content: &str) -> Self {
        Self {
            role,
            content: content.to_string(),
            timestamp: Utc::now(),
            is_streaming: false,
            shell: None,
            activity: None,
            expanded: false,
            id: next_message_id(),
            revision: 0,
        }
    }

    /// A step the agent took, drawn as a collapsible block.
    #[must_use]
    pub fn activity(activity: Activity, expanded: bool) -> Self {
        let mut message = Self::new(MessageRole::Assistant, "");
        message.activity = Some(activity);
        message.expanded = expanded;
        message
    }

    /// The agent step this message shows, if it is one.
    #[must_use]
    pub const fn activity_step(&self) -> Option<&Activity> {
        self.activity.as_ref()
    }

    /// Change the agent step this message shows.
    pub fn update_activity(&mut self, update: impl FnOnce(&mut Activity)) {
        if let Some(activity) = &mut self.activity {
            update(activity);
            self.touch();
        }
    }

    /// Whether this is an agent step still in progress (drawn with a
    /// spinner and a running clock).
    #[must_use]
    pub fn is_live_activity(&self) -> bool {
        self.activity.as_ref().is_some_and(Activity::is_running)
    }

    /// Whether an activity block shows its full detail.
    #[must_use]
    pub const fn is_expanded(&self) -> bool {
        self.expanded
    }

    /// Show or hide an activity block's full detail.
    pub fn set_expanded(&mut self, expanded: bool) {
        if self.activity.is_some() && self.expanded != expanded {
            self.expanded = expanded;
            self.touch();
        }
    }

    /// `(id, revision)`: differs from every other message's, and changes
    /// whenever this message does.
    #[must_use]
    pub const fn version(&self) -> (u64, u64) {
        (self.id, self.revision)
    }

    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// A shell command the user ran; output is filled in by [`Self::finish_shell`].
    #[must_use]
    pub fn shell(command: &str) -> Self {
        let mut message = Self::new(MessageRole::User, command);
        message.shell = Some(ShellRun {
            command: command.to_string(),
            output: None,
            exit_code: None,
        });
        message
    }

    /// The shell command this message shows, if it is one.
    #[must_use]
    pub const fn shell_run(&self) -> Option<&ShellRun> {
        self.shell.as_ref()
    }

    /// Whether this is a shell command still waiting for its output.
    #[must_use]
    pub fn is_running_shell(&self) -> bool {
        self.shell.as_ref().is_some_and(|run| run.output.is_none())
    }

    /// Record the output of a shell command message.
    pub fn finish_shell(&mut self, output: &str, exit_code: Option<i32>) {
        if let Some(run) = &mut self.shell {
            run.output = Some(output.to_string());
            run.exit_code = exit_code;
            self.touch();
        }
    }

    /// Override the timestamp (e.g. when restoring history from the record).
    #[must_use]
    pub fn with_timestamp(mut self, timestamp: DateTime<Utc>) -> Self {
        self.timestamp = timestamp;
        self.touch();
        self
    }

    /// Get the message role.
    #[must_use]
    pub const fn role(&self) -> MessageRole {
        self.role
    }

    /// Get the message content.
    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Get the message timestamp formatted as local time (HH:MM:SS).
    #[must_use]
    pub fn timestamp_local(&self) -> String {
        let local_time: DateTime<Local> = DateTime::from(self.timestamp);
        local_time.format("%H:%M:%S").to_string()
    }

    /// Set whether the message is streaming.
    pub fn set_streaming(&mut self, streaming: bool) {
        self.is_streaming = streaming;
        self.touch();
    }

    /// Check if the message is still streaming.
    #[must_use]
    pub const fn is_streaming(&self) -> bool {
        self.is_streaming
    }

    /// Append content to the message.
    pub fn append(&mut self, text: &str) {
        self.content.push_str(text);
        self.touch();
    }

    /// Set the message content (replaces existing content).
    pub fn set_content(&mut self, content: &str) {
        content.clone_into(&mut self.content);
        self.touch();
    }

    /// Render the message.
    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let (label, label_color, border_style) = match self.role {
            MessageRole::User => ("YOU", theme.colors.foreground, Borders::ALL),
            MessageRole::Assistant => ("AURA", theme.colors.primary, Borders::ALL),
            MessageRole::System => ("SYSTEM", theme.colors.secondary, Borders::NONE),
        };

        // Format timestamp
        let local_time: DateTime<Local> = DateTime::from(self.timestamp);
        let timestamp = local_time.format("%H:%M:%S").to_string();

        // Build header line
        let mut header_spans = vec![
            Span::styled(
                format!("[{timestamp}] "),
                Style::default().fg(theme.colors.muted),
            ),
            Span::styled(
                label,
                Style::default()
                    .fg(label_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ];

        // Streaming indicator
        if self.is_streaming && self.role == MessageRole::Assistant {
            header_spans.push(Span::styled(
                " ◇━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━▸",
                Style::default().fg(theme.colors.primary),
            ));
        }

        let header = Line::from(header_spans);

        // Build content lines
        let mut lines = vec![header, Line::from("")];
        for line in self.content.lines() {
            lines.push(Line::from(Span::styled(
                line,
                Style::default().fg(if self.role == MessageRole::Assistant {
                    theme.colors.primary
                } else {
                    theme.colors.foreground
                }),
            )));
        }

        let block = Block::default()
            .borders(border_style)
            .border_type(theme.border_style.to_border_type())
            .border_style(Style::default().fg(label_color));

        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false });

        frame.render_widget(paragraph, area);
    }

    /// Calculate the height needed to render this message.
    #[must_use]
    pub fn height(&self, width: u16) -> u16 {
        let content_lines = self.content.lines().count();
        let wrapped_lines = self
            .content
            .lines()
            .map(|line| {
                let line_len = u16::try_from(line.len()).unwrap_or(u16::MAX);
                let inner_width = width.saturating_sub(4); // Account for borders and padding
                if inner_width == 0 {
                    1
                } else {
                    (line_len / inner_width).max(1)
                }
            })
            .sum::<u16>();

        // Header + content + border padding
        let content_lines_u16 = u16::try_from(content_lines).unwrap_or(u16::MAX);
        2 + wrapped_lines.max(content_lines_u16) + 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_creation() {
        let msg = Message::new(MessageRole::User, "Hello, world!");
        assert_eq!(msg.role(), MessageRole::User);
        assert_eq!(msg.content(), "Hello, world!");
    }

    #[test]
    fn test_message_streaming() {
        let mut msg = Message::new(MessageRole::Assistant, "");
        assert!(!msg.is_streaming);
        msg.set_streaming(true);
        assert!(msg.is_streaming);
    }

    #[test]
    fn test_version_changes_with_content_and_differs_per_clone() {
        let mut msg = Message::new(MessageRole::Assistant, "Hello");
        let before = msg.version();
        msg.append("!");
        assert_ne!(msg.version(), before);
        assert_ne!(msg.clone().version().0, msg.version().0);
    }

    #[test]
    fn test_message_append() {
        let mut msg = Message::new(MessageRole::Assistant, "Hello");
        msg.append(", world!");
        assert_eq!(msg.content(), "Hello, world!");
    }
}
