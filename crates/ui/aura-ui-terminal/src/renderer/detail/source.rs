//! The lines of a detail view before wrapping, and which of them a view
//! starts at.

use super::super::text::display_width;
use super::highlight::{highlight_view_line, mark};
use super::wrap::{hard_wrap, sanitize};
use crate::app::file_view::FileBody;
use crate::app::vim::{Align, Vim};
use crate::Theme;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

/// First line to show so the cursor line sits where `align` asks,
/// starting from the current `scroll`.
pub(super) fn fit(
    source: &Source<'_>,
    align: Align,
    cursor: usize,
    scroll: usize,
    width: usize,
    visible: usize,
) -> usize {
    let cursor = cursor.min(source.len().saturating_sub(1));
    let text_width = source.text_width(width);
    let center = || {
        let own = source.rows(cursor, text_width);
        start_ending_at(source, cursor, width, own + visible.saturating_sub(own) / 2)
    };
    let half = visible / 2;
    match align {
        Align::View => scroll,
        Align::Top => cursor,
        Align::Center => center(),
        Align::Bottom => start_ending_at(source, cursor, width, visible),
        Align::Cursor if cursor < scroll => {
            if scroll - cursor > half {
                center()
            } else {
                cursor
            }
        }
        Align::Cursor => {
            let needed = start_ending_at(source, cursor, width, visible);
            if needed <= scroll {
                scroll
            } else if needed - scroll > half {
                center()
            } else {
                needed
            }
        }
    }
}

/// How many lines from `first` fit whole in `visible` rows, at least 1.
pub(super) fn fully_shown(
    source: &Source<'_>,
    first: usize,
    width: usize,
    visible: usize,
) -> usize {
    let text_width = source.text_width(width);
    let mut used = 0;
    let mut count = 0;
    for index in first..source.len() {
        used += source.rows(index, text_width);
        if used > visible {
            break;
        }
        count += 1;
    }
    count.max(1)
}

/// The lines of a detail view, before wrapping.
pub(super) enum Source<'a> {
    /// Already styled lines (a record).
    Styled(Vec<Line<'a>>),
    /// Source text highlighted on demand, with a line-number gutter.
    Code {
        language: String,
        lines: Vec<&'a str>,
        gutter: usize,
        /// Number of the first line in the gutter.
        first_line: usize,
        /// Lines already highlighted in the context of the whole file;
        /// others are highlighted on their own when drawn.
        highlighted: Highlighted,
    },
}

/// Highlighted spans of consecutive source lines, from line `first`.
#[derive(Default)]
pub(super) struct Highlighted {
    pub(super) first: usize,
    pub(super) lines: Vec<Vec<Span<'static>>>,
}

impl Highlighted {
    pub(super) fn get(&self, index: usize) -> Option<Vec<Span<'static>>> {
        self.lines.get(index.checked_sub(self.first)?).cloned()
    }
}

pub(super) fn file_source<'a>(
    body: &'a FileBody,
    language: String,
    first_line: usize,
    theme: &Theme,
) -> Source<'a> {
    match body {
        FileBody::Text(text) if text.is_empty() => Source::Styled(vec![Line::from(Span::styled(
            "Empty file",
            Style::default().fg(theme.colors.muted),
        ))]),
        FileBody::Text(_) => {
            let lines = body.lines();
            let last = first_line.max(1) + lines.len().max(1) - 1;
            Source::Code {
                language,
                gutter: last.to_string().len(),
                lines,
                first_line: first_line.max(1),
                highlighted: Highlighted::default(),
            }
        }
        FileBody::Note(note) => Source::Styled(vec![Line::from(Span::styled(
            note.clone(),
            Style::default().fg(theme.colors.muted),
        ))]),
    }
}

impl<'a> Source<'a> {
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Styled(lines) => lines.len(),
            Self::Code { lines, .. } => lines.len(),
        }
    }

    /// Columns taken by the line-number gutter, including its separator.
    pub(super) fn gutter_width(&self) -> usize {
        match self {
            Self::Styled(_) => 0,
            Self::Code { gutter, .. } => gutter + 3,
        }
    }

    /// Columns left for text in each row of a view `width` wide.
    pub(super) fn text_width(&self, width: usize) -> usize {
        width.saturating_sub(self.gutter_width()).max(1)
    }

    /// Display width of line `index`, not counting the gutter.
    pub(super) fn line_width(&self, index: usize) -> usize {
        match self {
            Self::Styled(lines) => lines[index].width(),
            Self::Code { lines, .. } => display_width(&sanitize(lines[index])),
        }
    }

    /// Rows line `index` wraps to with `text_width` columns per row.
    pub(super) fn rows(&self, index: usize, text_width: usize) -> usize {
        self.line_width(index).div_ceil(text_width).max(1)
    }

    /// Wrapped rows of the lines before `first`, in a view `width` wide.
    pub(super) fn rows_before(&self, first: usize, width: usize) -> usize {
        let text_width = self.text_width(width);
        (0..first.min(self.len()))
            .map(|index| self.rows(index, text_width))
            .sum()
    }

    /// Wrapped rows for lines `first..` that fit in `visible` rows, and how
    /// many source lines they came from. With `vim`, code shows its cursor
    /// and search matches.
    pub(super) fn visible(
        &self,
        first: usize,
        width: usize,
        visible: usize,
        theme: &Theme,
        vim: Option<&Vim>,
    ) -> (Vec<Line<'a>>, usize) {
        let indent = " ".repeat(self.gutter_width());
        let mut out: Vec<Line<'a>> = Vec::new();
        let mut shown = 0;
        for index in first..self.len() {
            if out.len() >= visible {
                break;
            }
            let line = match self {
                Self::Styled(lines) => lines[index].clone(),
                Self::Code {
                    language,
                    lines,
                    gutter,
                    first_line,
                    highlighted,
                } => {
                    let on_cursor = vim.is_some_and(|vim| vim.line() == index);
                    let number = if on_cursor {
                        Style::default()
                            .fg(theme.colors.primary)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(theme.colors.muted)
                    };
                    let mut spans = vec![Span::styled(
                        format!("{:>gutter$} │ ", index + first_line),
                        number,
                    )];
                    let code = highlighted.get(index).unwrap_or_else(|| {
                        highlight_view_line(language, &sanitize(lines[index]), theme)
                    });
                    match vim {
                        Some(vim) => spans.extend(mark(code, lines[index], vim, on_cursor, theme)),
                        None => spans.extend(code),
                    }
                    Line::from(spans)
                }
            };
            out.extend(hard_wrap(line, width, &indent));
            shown += 1;
        }
        out.truncate(visible);
        (out, shown)
    }
}

/// First line to show so the last line sits at the bottom of `visible`
/// rows, measuring backwards from the end.
pub(super) fn last_page_start(source: &Source<'_>, width: usize, visible: usize) -> usize {
    match source.len() {
        0 => 0,
        len => start_ending_at(source, len - 1, width, visible),
    }
}

/// First line to show so line `last` ends at the bottom of `rows` rows.
fn start_ending_at(source: &Source<'_>, last: usize, width: usize, rows: usize) -> usize {
    let text_width = source.text_width(width);
    let mut used = 0;
    let mut start = last + 1;
    while start > 0 {
        let needed = source.rows(start - 1, text_width);
        if used + needed > rows {
            break;
        }
        used += needed;
        start -= 1;
    }
    start.min(last)
}
