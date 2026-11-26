//! Text utility functions: display width, word wrapping, centered rect.

use crate::app::ProseMeasure;
use crate::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

/// Calculate the display width of a string (accounting for Unicode characters).
pub fn display_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    UnicodeWidthStr::width(s)
}

/// How many cells text takes: one per column on the grid, or what the
/// front end's proportional face makes of prose. Inline code, set apart
/// by its background, stays on the grid.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Measure {
    /// The face's measure, and the background of inline code
    prose: Option<(ProseMeasure, Color)>,
}

impl Measure {
    pub(crate) const GRID: Self = Self { prose: None };

    /// Prose in `measure`'s face, or on the grid without one.
    pub(crate) fn prose(measure: Option<ProseMeasure>, theme: &Theme) -> Self {
        Self {
            prose: measure.map(|measure| (measure, theme.colors.surface)),
        }
    }

    pub(crate) const fn is_grid(self) -> bool {
        self.prose.is_none()
    }

    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn width(self, text: &str, style: Style) -> f32 {
        match self.prose {
            Some((measure, code)) if style.bg != Some(code) => measure(text, style.add_modifier),
            _ => display_width(text) as f32,
        }
    }
}

/// Wrap text at word boundaries to fit within `max_width` (display width).
/// Returns a vector of lines, each fitting within the width.
pub fn wrap_words(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut current_width = 0;

    for word in text.split_whitespace() {
        let word_width = display_width(word);

        if current_width == 0 {
            (current_line, current_width) = start_line(word, word_width, max_width, &mut lines);
        } else if current_width + 1 + word_width <= max_width {
            current_line.push(' ');
            current_line.push_str(word);
            current_width += 1 + word_width;
        } else {
            lines.push(std::mem::take(&mut current_line));
            (current_line, current_width) = start_line(word, word_width, max_width, &mut lines);
        }
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

/// Start a line with `word`, `word_width` wide. A word wider than
/// `max_width` is broken into chunks that fit, all but the last pushed to
/// `lines`. Returns the line begun and its width.
fn start_line(
    word: &str,
    word_width: usize,
    max_width: usize,
    lines: &mut Vec<String>,
) -> (String, usize) {
    if word_width <= max_width {
        return (word.to_string(), word_width);
    }
    let mut chunk = String::new();
    let mut chunk_width = 0;
    for c in word.chars() {
        use unicode_width::UnicodeWidthChar;
        let char_width = c.width().unwrap_or(1);
        if chunk_width + char_width > max_width && !chunk.is_empty() {
            lines.push(std::mem::take(&mut chunk));
            chunk_width = 0;
        }
        chunk.push(c);
        chunk_width += char_width;
    }
    let width = display_width(&chunk);
    (chunk, width)
}

/// Helper to create a centered rect.
pub fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}
