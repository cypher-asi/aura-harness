//! Text utility functions: display width, word wrapping, centered rect.

use ratatui::layout::Rect;

/// Calculate the display width of a string (accounting for Unicode characters).
pub fn display_width(s: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    UnicodeWidthStr::width(s)
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
            if word_width > max_width {
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
                if !chunk.is_empty() {
                    current_line = chunk;
                    current_width = display_width(&current_line);
                }
            } else {
                current_line = word.to_string();
                current_width = word_width;
            }
        } else if current_width + 1 + word_width <= max_width {
            current_line.push(' ');
            current_line.push_str(word);
            current_width += 1 + word_width;
        } else {
            lines.push(std::mem::take(&mut current_line));
            current_width = 0;

            if word_width > max_width {
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
                if !chunk.is_empty() {
                    current_line = chunk;
                    current_width = display_width(&current_line);
                }
            } else {
                current_line = word.to_string();
                current_width = word_width;
            }
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
