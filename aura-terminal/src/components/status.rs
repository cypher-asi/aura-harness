//! Status bar component.

use crate::themes::Theme;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

/// Status bar displaying current state and statistics.
#[derive(Debug, Clone)]
pub struct StatusBar {
    /// Current status message
    status: String,
    /// Token count (if available)
    tokens: Option<(u32, u32)>,
    /// Tool call count
    tool_calls: Option<u32>,
    /// Elapsed time (if processing)
    elapsed_ms: Option<u64>,
}

impl StatusBar {
    /// Create a new status bar.
    #[must_use]
    pub fn new(status: &str) -> Self {
        Self {
            status: status.to_string(),
            tokens: None,
            tool_calls: None,
            elapsed_ms: None,
        }
    }

    /// Set the token counts.
    #[must_use]
    pub const fn with_tokens(mut self, used: u32, total: u32) -> Self {
        self.tokens = Some((used, total));
        self
    }

    /// Set the tool call count.
    #[must_use]
    pub const fn with_tool_calls(mut self, count: u32) -> Self {
        self.tool_calls = Some(count);
        self
    }

    /// Set the elapsed time.
    #[must_use]
    pub const fn with_elapsed(mut self, ms: u64) -> Self {
        self.elapsed_ms = Some(ms);
        self
    }

    /// Render the status bar.
    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let is_ready = self.status == "Ready";
        let status_color = if is_ready {
            theme.colors.success
        } else {
            theme.colors.warning
        };

        let status_icon = if is_ready { "●" } else { "◐" };

        let mut spans = vec![
            Span::styled(
                format!("  {status_icon} "),
                Style::default().fg(status_color),
            ),
            Span::styled(
                &self.status,
                Style::default()
                    .fg(status_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ];

        // Token count - format as "X.Yk/Zk" using integer math
        if let Some((used, total)) = self.tokens {
            spans.push(Span::styled(
                "  │  Tokens: ",
                Style::default().fg(theme.colors.muted),
            ));
            let used_k = used / 1000;
            let used_decimal = (used % 1000) / 100;
            let total_k = total / 1000;
            spans.push(Span::styled(
                format!("{used_k}.{used_decimal}k/{total_k}k"),
                Style::default().fg(theme.colors.foreground),
            ));
        }

        // Tool calls
        if let Some(count) = self.tool_calls {
            spans.push(Span::styled(
                "  │  Tools: ",
                Style::default().fg(theme.colors.muted),
            ));
            spans.push(Span::styled(
                format!("{count} used"),
                Style::default().fg(theme.colors.foreground),
            ));
        }

        // Elapsed time - format as "X.Ys" using integer math
        if let Some(ms) = self.elapsed_ms {
            spans.push(Span::styled(
                "  │  ⏱ ",
                Style::default().fg(theme.colors.muted),
            ));
            let secs = ms / 1000;
            let decimal = (ms % 1000) / 100;
            spans.push(Span::styled(
                format!("{secs}.{decimal}s"),
                Style::default().fg(theme.colors.foreground),
            ));
        }

        let line = Line::from(spans);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(theme.border_style.to_border_type())
            .border_style(Style::default().fg(theme.colors.muted));

        let paragraph = Paragraph::new(line).block(block);
        frame.render_widget(paragraph, area);
    }
}

impl Default for StatusBar {
    fn default() -> Self {
        Self::new("Ready")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_bar_creation() {
        let status = StatusBar::new("Processing...")
            .with_tokens(12400, 100_000)
            .with_tool_calls(3)
            .with_elapsed(2300);

        assert_eq!(status.status, "Processing...");
        assert!(status.tokens.is_some());
    }
}
