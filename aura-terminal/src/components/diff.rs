//! Diff view component for displaying file changes.

use crate::themes::Theme;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use serde::{Deserialize, Serialize};

/// Type of diff line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffLineType {
    /// Context line (unchanged)
    Context,
    /// Added line
    Addition,
    /// Removed line
    Deletion,
    /// Separator/header
    Header,
}

/// A single line in a diff.
#[derive(Debug, Clone)]
pub struct DiffLine {
    /// Line type
    pub line_type: DiffLineType,
    /// Line number (if applicable)
    pub line_number: Option<u32>,
    /// Line content
    pub content: String,
}

impl DiffLine {
    /// Create a new diff line.
    #[must_use]
    pub fn new(line_type: DiffLineType, content: &str) -> Self {
        Self {
            line_type,
            line_number: None,
            content: content.to_string(),
        }
    }

    /// Set the line number.
    #[must_use]
    pub const fn with_line_number(mut self, num: u32) -> Self {
        self.line_number = Some(num);
        self
    }
}

/// Diff view for displaying file changes.
#[derive(Debug, Clone)]
pub struct DiffView {
    /// File path
    path: String,
    /// Diff lines
    lines: Vec<DiffLine>,
    /// Number of additions
    additions: u32,
    /// Number of deletions
    deletions: u32,
}

impl DiffView {
    /// Create a new diff view.
    #[must_use]
    pub fn new(path: &str) -> Self {
        Self {
            path: path.to_string(),
            lines: Vec::new(),
            additions: 0,
            deletions: 0,
        }
    }

    /// Add a line to the diff.
    pub fn add_line(&mut self, line: DiffLine) {
        match line.line_type {
            DiffLineType::Addition => self.additions += 1,
            DiffLineType::Deletion => self.deletions += 1,
            _ => {}
        }
        self.lines.push(line);
    }

    /// Get the number of additions.
    #[must_use]
    pub const fn additions(&self) -> u32 {
        self.additions
    }

    /// Get the number of deletions.
    #[must_use]
    pub const fn deletions(&self) -> u32 {
        self.deletions
    }

    /// Render the diff view.
    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let title = format!(" DIFF: {} ", self.path);

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

        // Build diff lines
        let mut render_lines: Vec<Line> = Vec::new();

        for line in &self.lines {
            let (prefix, color) = match line.line_type {
                DiffLineType::Context => ("   ", theme.colors.foreground),
                DiffLineType::Addition => (" + ", theme.colors.success),
                DiffLineType::Deletion => (" - ", theme.colors.error),
                DiffLineType::Header => ("", theme.colors.muted),
            };

            let line_num_str = line
                .line_number
                .map_or_else(|| "   ".to_string(), |n| format!("{n:3}"));

            render_lines.push(Line::from(vec![
                Span::styled(
                    format!("{line_num_str} │"),
                    Style::default().fg(theme.colors.muted),
                ),
                Span::styled(prefix, Style::default().fg(color)),
                Span::styled(&line.content, Style::default().fg(color)),
            ]));
        }

        // Add summary line
        render_lines.push(Line::from(""));
        render_lines.push(Line::from(vec![
            Span::styled(
                format!("  +{} additions", self.additions),
                Style::default().fg(theme.colors.success),
            ),
            Span::styled("   ", Style::default()),
            Span::styled(
                format!("-{} deletions", self.deletions),
                Style::default().fg(theme.colors.error),
            ),
        ]));

        let paragraph = Paragraph::new(render_lines)
            .block(block)
            .wrap(Wrap { trim: false });

        frame.render_widget(paragraph, area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_view_creation() {
        let mut diff = DiffView::new("src/main.rs");
        diff.add_line(DiffLine::new(DiffLineType::Context, "fn main() {").with_line_number(1));
        diff.add_line(DiffLine::new(DiffLineType::Deletion, "    old_code();").with_line_number(2));
        diff.add_line(DiffLine::new(DiffLineType::Addition, "    new_code();").with_line_number(2));

        assert_eq!(diff.additions(), 1);
        assert_eq!(diff.deletions(), 1);
    }
}
