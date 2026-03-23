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

use crate::events::MessageRole;

/// A chat message.
#[derive(Debug, Clone)]
pub struct Message {
    /// Message role
    role: MessageRole,
    /// Message content
    content: String,
    /// Timestamp
    timestamp: DateTime<Utc>,
    /// Whether the message is still streaming
    is_streaming: bool,
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
        }
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
    }

    /// Check if the message is still streaming.
    #[must_use]
    pub const fn is_streaming(&self) -> bool {
        self.is_streaming
    }

    /// Append content to the message.
    pub fn append(&mut self, text: &str) {
        self.content.push_str(text);
    }

    /// Set the message content (replaces existing content).
    pub fn set_content(&mut self, content: &str) {
        self.content = content.to_string();
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
    fn test_message_append() {
        let mut msg = Message::new(MessageRole::Assistant, "Hello");
        msg.append(", world!");
        assert_eq!(msg.content(), "Hello, world!");
    }
}
