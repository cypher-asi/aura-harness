//! Progress bar component.

use crate::themes::Theme;
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use serde::{Deserialize, Serialize};

/// Progress bar style.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProgressStyle {
    /// Block characters: [████████░░░░]
    #[default]
    Blocks,
    /// Arrow style: ◇━━━━━━━━━━▸
    Arrow,
    /// Gradient blocks: ▓▓▓▓▓░░░░░
    Gradient,
}

/// Progress bar for showing completion status.
#[derive(Debug, Clone)]
pub struct ProgressBar {
    /// Progress value (0.0 to 1.0)
    progress: f32,
    /// Display style
    style: ProgressStyle,
    /// Width in characters
    width: u16,
    /// Whether to show percentage
    show_percentage: bool,
}

impl ProgressBar {
    /// Create a new progress bar.
    #[must_use]
    pub fn new(width: u16) -> Self {
        Self {
            progress: 0.0,
            style: ProgressStyle::default(),
            width,
            show_percentage: true,
        }
    }

    /// Set the progress value.
    #[must_use]
    pub fn with_progress(mut self, progress: f32) -> Self {
        self.progress = progress.clamp(0.0, 1.0);
        self
    }

    /// Set the display style.
    #[must_use]
    pub const fn with_style(mut self, style: ProgressStyle) -> Self {
        self.style = style;
        self
    }

    /// Set whether to show percentage.
    #[must_use]
    pub const fn with_show_percentage(mut self, show: bool) -> Self {
        self.show_percentage = show;
        self
    }

    /// Update the progress value.
    pub fn set_progress(&mut self, progress: f32) {
        self.progress = progress.clamp(0.0, 1.0);
    }

    /// Render the progress bar to a string.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    pub fn render_string(&self) -> String {
        let inner_width = self.width.saturating_sub(2) as usize; // Account for brackets
        let filled = (self.progress * inner_width as f32) as usize;
        let empty = inner_width.saturating_sub(filled);

        match self.style {
            ProgressStyle::Blocks => {
                let bar = format!("[{}{}]", "█".repeat(filled), "░".repeat(empty));
                if self.show_percentage {
                    format!("{bar} {:.0}%", self.progress * 100.0)
                } else {
                    bar
                }
            }
            ProgressStyle::Arrow => {
                if filled == 0 {
                    format!("◇{}▸", "─".repeat(inner_width.saturating_sub(1)))
                } else {
                    format!(
                        "◇{}▸{}",
                        "━".repeat(filled.saturating_sub(1)),
                        " ".repeat(empty)
                    )
                }
            }
            ProgressStyle::Gradient => {
                let bar = format!("▓{}░", "▓".repeat(filled.saturating_sub(1)));
                if self.show_percentage {
                    format!("{bar} {:.0}%", self.progress * 100.0)
                } else {
                    bar
                }
            }
        }
    }

    /// Render the progress bar to the frame.
    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let text = self.render_string();
        let color = if self.progress >= 1.0 {
            theme.colors.success
        } else {
            theme.colors.primary
        };

        let line = Line::from(Span::styled(text, Style::default().fg(color)));
        let paragraph = Paragraph::new(line);
        frame.render_widget(paragraph, area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_bar_creation() {
        let bar = ProgressBar::new(20).with_progress(0.5);
        assert!((bar.progress - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_progress_bar_clamping() {
        let bar = ProgressBar::new(20).with_progress(1.5);
        assert!((bar.progress - 1.0).abs() < f32::EPSILON);

        let bar = ProgressBar::new(20).with_progress(-0.5);
        assert!(bar.progress.abs() < f32::EPSILON);
    }

    #[test]
    fn test_progress_bar_render_string() {
        let bar = ProgressBar::new(12)
            .with_progress(0.5)
            .with_style(ProgressStyle::Blocks);
        let rendered = bar.render_string();
        assert!(rendered.contains("█"));
        assert!(rendered.contains("░"));
    }
}
