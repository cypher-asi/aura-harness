//! Header bar component.

use crate::themes::Theme;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

/// Header bar displaying app info and connection status.
#[derive(Debug, Clone)]
pub struct HeaderBar {
    /// Application name
    app_name: String,
    /// Agent identifier
    agent_id: Option<String>,
    /// Session identifier
    session_id: Option<String>,
    /// Connection status
    connected: bool,
}

impl HeaderBar {
    /// Create a new header bar.
    #[must_use]
    pub fn new() -> Self {
        Self {
            app_name: "AURA CLI".to_string(),
            agent_id: None,
            session_id: None,
            connected: true,
        }
    }

    /// Set the agent ID.
    #[must_use]
    pub fn with_agent_id(mut self, id: impl Into<String>) -> Self {
        self.agent_id = Some(id.into());
        self
    }

    /// Set the session ID.
    #[must_use]
    pub fn with_session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    /// Set the connection status.
    #[must_use]
    pub const fn with_connected(mut self, connected: bool) -> Self {
        self.connected = connected;
        self
    }

    /// Render the header bar.
    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let mut spans = vec![
            Span::styled("  ◈ ", Style::default().fg(theme.colors.primary)),
            Span::styled(
                &self.app_name,
                Style::default()
                    .fg(theme.colors.primary)
                    .add_modifier(Modifier::BOLD),
            ),
        ];

        if let Some(agent) = &self.agent_id {
            spans.push(Span::styled(
                "  │  Agent: ",
                Style::default().fg(theme.colors.muted),
            ));
            spans.push(Span::styled(
                agent,
                Style::default().fg(theme.colors.foreground),
            ));
        }

        if let Some(session) = &self.session_id {
            spans.push(Span::styled(
                "  │  Session: ",
                Style::default().fg(theme.colors.muted),
            ));
            spans.push(Span::styled(
                format!("#{session}"),
                Style::default().fg(theme.colors.foreground),
            ));
        }

        // Connection status
        spans.push(Span::styled(
            "  │  ",
            Style::default().fg(theme.colors.muted),
        ));
        if self.connected {
            spans.push(Span::styled(
                "● Connected",
                Style::default().fg(theme.colors.success),
            ));
        } else {
            spans.push(Span::styled(
                "○ Disconnected",
                Style::default().fg(theme.colors.error),
            ));
        }

        let line = Line::from(spans);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(theme.border_style.to_border_type())
            .border_style(Style::default().fg(theme.colors.primary));

        let paragraph = Paragraph::new(line).block(block);
        frame.render_widget(paragraph, area);
    }
}

impl Default for HeaderBar {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_bar_creation() {
        let header = HeaderBar::new()
            .with_agent_id("agent-01")
            .with_session_id("a7f3")
            .with_connected(true);

        assert!(header.agent_id.is_some());
        assert!(header.session_id.is_some());
        assert!(header.connected);
    }
}
