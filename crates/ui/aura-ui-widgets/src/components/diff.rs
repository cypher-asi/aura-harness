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
use similar::{ChangeTag, TextDiff};
use unicode_width::UnicodeWidthChar;

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

    /// A view of the hunks of a unified diff. `---`/`+++` file headers are
    /// skipped (the path is the view's title); lines that are not part of a
    /// hunk are kept as headers. Deletions are numbered by the old file,
    /// everything else by the new one.
    #[must_use]
    pub fn from_unified(path: &str, text: &str) -> Self {
        let mut view = Self::new(path);
        let (mut old, mut new) = (0u32, 0u32);
        for line in text.lines() {
            if line.starts_with("--- ") || line.starts_with("+++ ") {
                continue;
            }
            if let Some((old_start, new_start)) = hunk_starts(line) {
                old = old_start;
                new = new_start;
                view.add_line(DiffLine::new(DiffLineType::Header, line));
            } else if let Some(rest) = line.strip_prefix('+') {
                view.add_line(DiffLine::new(DiffLineType::Addition, rest).with_line_number(new));
                new = new.saturating_add(1);
            } else if let Some(rest) = line.strip_prefix('-') {
                view.add_line(DiffLine::new(DiffLineType::Deletion, rest).with_line_number(old));
                old = old.saturating_add(1);
            } else if let Some(rest) = line.strip_prefix(' ') {
                view.add_line(DiffLine::new(DiffLineType::Context, rest).with_line_number(new));
                old = old.saturating_add(1);
                new = new.saturating_add(1);
            } else {
                view.add_line(DiffLine::new(DiffLineType::Header, line));
            }
        }
        view
    }

    /// A view of how `new` differs from `old`, line by line. The texts are
    /// snippets rather than whole files, so lines are not numbered.
    #[must_use]
    pub fn from_texts(path: &str, old: &str, new: &str) -> Self {
        let mut view = Self::new(path);
        for change in TextDiff::from_lines(old, new).iter_all_changes() {
            let line_type = match change.tag() {
                ChangeTag::Equal => DiffLineType::Context,
                ChangeTag::Insert => DiffLineType::Addition,
                ChangeTag::Delete => DiffLineType::Deletion,
            };
            let content = change.value();
            let content = content.strip_suffix('\n').unwrap_or(content);
            view.add_line(DiffLine::new(line_type, content));
        }
        view
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

    /// The file the diff is for.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
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

    /// Whether the diff has no lines.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The diff as styled lines: a line-number gutter, a `+`/`-` sign and
    /// the content. Content wider than `width` columns continues on the
    /// next line under the same sign; `0` never wraps.
    #[must_use]
    pub fn styled_lines(&self, theme: &Theme, width: usize) -> Vec<Line<'static>> {
        let digits = self
            .lines
            .iter()
            .filter_map(|l| l.line_number)
            .max()
            .map_or(0, |n| n.to_string().len())
            .max(3);
        let muted = Style::default().fg(theme.colors.muted);
        let mut out = Vec::new();
        for line in &self.lines {
            let (sign, color) = match line.line_type {
                DiffLineType::Context => ("   ", theme.colors.foreground),
                DiffLineType::Addition => (" + ", theme.colors.success),
                DiffLineType::Deletion => (" - ", theme.colors.error),
                DiffLineType::Header => {
                    out.push(Line::from(Span::styled(
                        line.content.clone(),
                        muted.add_modifier(Modifier::DIM),
                    )));
                    continue;
                }
            };
            let gutter = line
                .line_number
                .map_or_else(|| " ".repeat(digits), |n| format!("{n:>digits$}"));
            let gutter = format!("{gutter} │");
            let room = width.saturating_sub(gutter.chars().count() + sign.len());
            let style = Style::default().fg(color);
            for (i, chunk) in wrap_columns(&line.content, room).into_iter().enumerate() {
                let gutter = if i == 0 {
                    gutter.clone()
                } else {
                    format!("{} │", " ".repeat(digits))
                };
                out.push(Line::from(vec![
                    Span::styled(gutter, muted),
                    Span::styled(sign, style),
                    Span::styled(chunk, style),
                ]));
            }
        }
        out
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

        let mut render_lines = self.styled_lines(theme, 0);

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

/// Old and new start lines of a `@@ -a,b +c,d @@` hunk header.
fn hunk_starts(line: &str) -> Option<(u32, u32)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let (new, _) = rest.split_once(" @@")?;
    let start = |range: &str| range.split(',').next()?.parse::<u32>().ok();
    Some((start(old)?, start(new)?))
}

/// `text` cut into pieces at most `width` columns wide; `0` keeps it whole.
/// Tabs count as one column, which is how the terminal draws them here.
fn wrap_columns(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut pieces = vec![String::new()];
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > width && used > 0 {
            pieces.push(String::new());
            used = 0;
        }
        if let Some(last) = pieces.last_mut() {
            last.push(c);
        }
        used += w;
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn test_diff_view_creation() {
        let mut diff = DiffView::new("src/main.rs");
        diff.add_line(DiffLine::new(DiffLineType::Context, "fn main() {").with_line_number(1));
        diff.add_line(DiffLine::new(DiffLineType::Deletion, "    old_code();").with_line_number(2));
        diff.add_line(DiffLine::new(DiffLineType::Addition, "    new_code();").with_line_number(2));

        assert_eq!(diff.additions(), 1);
        assert_eq!(diff.deletions(), 1);
    }

    #[test]
    fn unified_diff_is_numbered_from_its_hunks() {
        let text = "--- a/x\n+++ b/x\n@@ -10,3 +10,3 @@\n keep\n-old\n+new\n tail\n";
        let diff = DiffView::from_unified("x", text);
        assert_eq!((diff.additions(), diff.deletions()), (1, 1));
        let lines = texts(&diff.styled_lines(&Theme::default(), 80));
        assert_eq!(
            lines,
            [
                "@@ -10,3 +10,3 @@",
                " 10 │   keep",
                " 11 │ - old",
                " 11 │ + new",
                " 12 │   tail",
            ]
        );
    }

    #[test]
    fn snippet_diff_has_no_numbers() {
        let diff = DiffView::from_texts("x", "a\nb\n", "a\nc\n");
        assert_eq!((diff.additions(), diff.deletions()), (1, 1));
        let lines = texts(&diff.styled_lines(&Theme::default(), 80));
        assert_eq!(lines, ["    │   a", "    │ - b", "    │ + c"]);
    }

    #[test]
    fn long_lines_wrap_under_their_sign() {
        let diff = DiffView::from_unified("x", "@@ -1,0 +1,1 @@\n+abcdefghij\n");
        let lines = texts(&diff.styled_lines(&Theme::default(), 12));
        assert_eq!(lines[1..], ["  1 │ + abcd", "    │ + efgh", "    │ + ij"]);
    }
}
