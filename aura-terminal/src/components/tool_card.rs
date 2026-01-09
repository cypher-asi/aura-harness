//! Tool card component for displaying tool execution.

use crate::{animation::Spinner, themes::Theme};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use serde::{Deserialize, Serialize};

/// Tool execution status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ToolStatus {
    /// Tool is executing
    #[default]
    Running,
    /// Tool completed successfully
    Success,
    /// Tool failed
    Error,
    /// Tool was denied
    Denied,
}

/// A tool execution card.
#[derive(Debug, Clone)]
pub struct ToolCard {
    /// Tool use ID
    id: String,
    /// Tool name
    name: String,
    /// Tool arguments (JSON string)
    args: String,
    /// Execution status
    status: ToolStatus,
    /// Result content (if completed)
    result: Option<String>,
    /// Spinner for running state
    spinner: Spinner,
}

impl ToolCard {
    /// Create a new tool card.
    #[must_use]
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            args: String::new(),
            status: ToolStatus::Running,
            result: None,
            spinner: Spinner::new(),
        }
    }

    /// Get the tool ID.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Get the tool name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Set the tool arguments.
    #[must_use]
    pub fn with_args(mut self, args: &str) -> Self {
        self.args = args.to_string();
        self
    }

    /// Set the tool status.
    pub fn set_status(&mut self, status: ToolStatus) {
        self.status = status;
    }

    /// Set the result.
    pub fn set_result(&mut self, result: &str) {
        self.result = Some(result.to_string());
    }

    /// Get the current status.
    #[must_use]
    pub const fn status(&self) -> ToolStatus {
        self.status
    }

    /// Render the tool card.
    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let (status_icon, status_color) = match self.status {
            ToolStatus::Running => (self.spinner.tick(), theme.colors.secondary),
            ToolStatus::Success => ("✓", theme.colors.success),
            ToolStatus::Error => ("✗", theme.colors.error),
            ToolStatus::Denied => ("⊘", theme.colors.warning),
        };

        let title = format!(" TOOL: {} ", self.name);

        let block = Block::default()
            .title(Span::styled(
                &title,
                Style::default()
                    .fg(theme.colors.secondary)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_type(theme.border_style.to_border_type())
            .border_style(Style::default().fg(theme.colors.secondary));

        // Build content
        let mut lines = vec![];

        // Status line
        lines.push(Line::from(vec![
            Span::styled(format!("{status_icon} "), Style::default().fg(status_color)),
            Span::styled(
                match self.status {
                    ToolStatus::Running => "Executing...",
                    ToolStatus::Success => "Complete",
                    ToolStatus::Error => "Failed",
                    ToolStatus::Denied => "Denied",
                },
                Style::default().fg(status_color),
            ),
        ]));

        // Args preview (if any)
        if !self.args.is_empty() {
            let preview = if self.args.len() > 60 {
                format!("{}...", &self.args[..60])
            } else {
                self.args.clone()
            };
            lines.push(Line::from(Span::styled(
                preview,
                Style::default().fg(theme.colors.muted),
            )));
        }

        // Result preview (if any)
        if let Some(result) = &self.result {
            lines.push(Line::from(""));
            let preview = if result.len() > 100 {
                format!("{}...", &result[..100])
            } else {
                result.clone()
            };
            let preview_lines: Vec<&str> = preview.lines().take(3).collect();
            for line in preview_lines {
                lines.push(Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(theme.colors.muted),
                )));
            }
        }

        let paragraph = Paragraph::new(lines).block(block);
        frame.render_widget(paragraph, area);
    }

    /// Calculate the height needed to render this card.
    #[must_use]
    pub fn height(&self) -> u16 {
        let mut height = 4; // Border + status line
        if !self.args.is_empty() {
            height += 1;
        }
        if self.result.is_some() {
            height += 4; // Blank + up to 3 lines
        }
        height
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_card_creation() {
        let card = ToolCard::new("tool-1", "fs.read").with_args(r#"{"path": "main.rs"}"#);

        assert_eq!(card.id(), "tool-1");
        assert_eq!(card.name(), "fs.read");
        assert_eq!(card.status(), ToolStatus::Running);
    }

    #[test]
    fn test_tool_card_status_change() {
        let mut card = ToolCard::new("tool-1", "fs.read");
        card.set_status(ToolStatus::Success);
        card.set_result("file contents here");

        assert_eq!(card.status(), ToolStatus::Success);
    }
}
