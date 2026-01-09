//! Input field component.

use crate::themes::Theme;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

/// Input field for user text entry.
#[derive(Debug, Clone)]
pub struct InputField {
    /// Input text
    text: String,
    /// Cursor position
    cursor: usize,
    /// Placeholder text
    placeholder: String,
    /// Whether the field is focused
    focused: bool,
}

impl InputField {
    /// Create a new input field.
    #[must_use]
    pub fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            placeholder: "Type your message... (Tab: autocomplete │ /help │ Ctrl+C: cancel)"
                .to_string(),
            focused: true,
        }
    }

    /// Set the current text.
    #[must_use]
    pub fn with_text(mut self, text: &str) -> Self {
        self.text = text.to_string();
        self.cursor = text.len();
        self
    }

    /// Set the cursor position.
    #[must_use]
    pub fn with_cursor(mut self, pos: usize) -> Self {
        self.cursor = pos.min(self.text.len());
        self
    }

    /// Set the placeholder text.
    #[must_use]
    pub fn with_placeholder(mut self, placeholder: &str) -> Self {
        self.placeholder = placeholder.to_string();
        self
    }

    /// Set whether the field is focused.
    #[must_use]
    pub const fn with_focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Render the input field.
    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let border_color = if self.focused {
            theme.colors.primary
        } else {
            theme.colors.muted
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(theme.border_style.to_border_type())
            .border_style(Style::default().fg(border_color));

        let inner_area = block.inner(area);

        // Build the text with cursor
        let content = if self.text.is_empty() {
            Line::from(Span::styled(
                format!("  ▸ {}", self.placeholder),
                Style::default().fg(theme.colors.muted),
            ))
        } else {
            let before = &self.text[..self.cursor];
            let cursor_char = self.text.chars().nth(self.cursor).unwrap_or(' ');
            let after = if self.cursor < self.text.len() {
                &self.text[self.cursor + cursor_char.len_utf8()..]
            } else {
                ""
            };

            Line::from(vec![
                Span::styled("  ▸ ", Style::default().fg(theme.colors.primary)),
                Span::styled(before, Style::default().fg(theme.colors.foreground)),
                Span::styled(
                    cursor_char.to_string(),
                    Style::default()
                        .fg(theme.colors.background)
                        .bg(theme.colors.foreground)
                        .add_modifier(Modifier::SLOW_BLINK),
                ),
                Span::styled(after, Style::default().fg(theme.colors.foreground)),
            ])
        };

        frame.render_widget(block, area);

        let paragraph = Paragraph::new(content);
        frame.render_widget(paragraph, inner_area);
    }
}

impl Default for InputField {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_field_creation() {
        let field = InputField::new().with_text("hello").with_cursor(3);

        assert_eq!(field.text, "hello");
        assert_eq!(field.cursor, 3);
    }
}
