//! Tables and rules the benchmark detail is drawn with.

use super::format::{fit, under_way};
use crate::renderer::text::display_width;
use crate::Theme;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// How a column of a [`table`] lines up.
#[derive(Debug, Clone, Copy)]
pub(super) struct Col {
    right: bool,
    /// Gives up width, cutting its cells, when the table is too wide
    flex: bool,
    /// Drawn after a ` │ ` rather than a plain gap
    sep: bool,
}

impl Col {
    pub(super) const L: Self = Self {
        right: false,
        flex: false,
        sep: false,
    };
    pub(super) const R: Self = Self {
        right: true,
        ..Self::L
    };

    pub(super) const fn flex(self) -> Self {
        Self { flex: true, ..self }
    }

    pub(super) const fn sep(self) -> Self {
        Self { sep: true, ..self }
    }
}

pub(super) type Cell = (String, Style);

pub(super) enum Row {
    Cells(Vec<Cell>),
    /// Cells drawn in a selection style, gaps included
    Picked(Vec<Cell>, Style),
    /// Cells of something under way up to column `status_at`, with
    /// `status` drawn from there in the room left on the line. The status
    /// never widens a column, so the table holds still as it changes. On a
    /// frame, a band of light sweeps across the status.
    Live {
        cells: Vec<Cell>,
        status_at: usize,
        status: Cell,
        frame: Option<usize>,
    },
    /// A rule across the table, as between groups of rows
    Rule,
}

/// `rows` in columns as wide as their widest cell, at most `width`
/// together: flex columns give up width first. Columns empty in every row
/// are left out. `header` gets a rule under it.
pub(super) fn table(
    cols: &[Col],
    header: Option<&[&str]>,
    rows: &[Row],
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let layout = Layout::new(cols, header, rows, width, theme);
    let mut lines = Vec::new();
    if let Some(header) = header {
        let muted = Style::default().fg(theme.colors.muted);
        let cells: Vec<Cell> = header.iter().map(|h| ((*h).to_string(), muted)).collect();
        lines.push(layout.line(&cells, None, None));
        lines.push(layout.rule());
    }
    lines.extend(rows.iter().map(|row| layout.row(row)));
    lines
}

type Status<'r> = (usize, &'r Cell, Option<usize>);

/// The widths a [`table`]'s columns get.
struct Layout<'c> {
    cols: &'c [Col],
    widths: Vec<usize>,
    shown: Vec<usize>,
    width: usize,
    faint: Style,
    shine: (Color, Color),
}

fn cell_width(row: &Row, i: usize) -> usize {
    match row {
        Row::Cells(cells) | Row::Picked(cells, _) | Row::Live { cells, .. } => {
            cells.get(i).map_or(0, |(text, _)| display_width(text))
        }
        Row::Rule => 0,
    }
}

impl<'c> Layout<'c> {
    fn new(
        cols: &'c [Col],
        header: Option<&[&str]>,
        rows: &[Row],
        width: usize,
        theme: &Theme,
    ) -> Self {
        let widths: Vec<usize> = (0..cols.len())
            .map(|i| {
                let head = header
                    .and_then(|h| h.get(i))
                    .map_or(0, |text| display_width(text));
                rows.iter()
                    .map(|row| cell_width(row, i))
                    .max()
                    .unwrap_or(0)
                    .max(head)
            })
            .collect();
        let shown: Vec<usize> = (0..cols.len()).filter(|&i| widths[i] > 0).collect();
        let mut layout = Self {
            cols,
            widths,
            shown,
            width,
            faint: Style::default().fg(theme.colors.faint),
            shine: (theme.colors.muted, theme.colors.foreground),
        };
        layout.shrink();
        layout
    }

    /// Flex columns give up width until the table fits.
    fn shrink(&mut self) {
        let total = self.across(&self.shown);
        let mut over = total.saturating_sub(self.width);
        while over > 0 {
            let Some(&i) = self
                .shown
                .iter()
                .filter(|&&i| self.cols[i].flex && self.widths[i] > 1)
                .max_by_key(|&&i| self.widths[i])
            else {
                break;
            };
            self.widths[i] -= 1;
            over -= 1;
        }
    }

    fn gap(&self, i: usize) -> usize {
        if self.cols[i].sep {
            3
        } else {
            2
        }
    }

    /// Columns `open` take together, gaps between them included.
    fn across(&self, open: &[usize]) -> usize {
        open.iter()
            .enumerate()
            .map(|(n, &i)| self.widths[i] + if n == 0 { 0 } else { self.gap(i) })
            .sum()
    }

    fn row(&self, row: &Row) -> Line<'static> {
        match row {
            Row::Cells(cells) => self.line(cells, None, None),
            Row::Picked(cells, style) => self.line(cells, Some(*style), None),
            Row::Live {
                cells,
                status_at,
                status,
                frame,
            } => self.line(cells, None, Some((*status_at, status, *frame))),
            Row::Rule => self.rule(),
        }
    }

    fn line(
        &self,
        cells: &[Cell],
        picked: Option<Style>,
        live: Option<Status<'_>>,
    ) -> Line<'static> {
        let cols_in: Vec<usize> = self
            .shown
            .iter()
            .copied()
            .filter(|&i| live.is_none_or(|(at, ..)| i < at))
            .collect();
        let mut spans = self.cell_spans(cells, picked, live.is_some(), &cols_in);
        if let Some((_, (text, style), frame)) = live.filter(|(_, (text, _), _)| !text.is_empty()) {
            self.push_status(&mut spans, &cols_in, (text, *style), frame);
        }
        Line::from(spans)
    }

    fn cell_spans(
        &self,
        cells: &[Cell],
        picked: Option<Style>,
        live: bool,
        cols_in: &[usize],
    ) -> Vec<Span<'static>> {
        let filled = |i: usize| cells.get(i).is_some_and(|(text, _)| !text.is_empty());
        let padded = picked.is_some() || live;
        let last = if padded {
            cols_in.last().copied()
        } else {
            cols_in.iter().rev().find(|&&i| filled(i)).copied()
        };
        let mut spans = Vec::new();
        for (n, &i) in cols_in.iter().enumerate() {
            if last.is_some_and(|last| i > last) {
                break;
            }
            if n > 0 {
                let (text, style) = if self.cols[i].sep {
                    (" │ ", self.faint)
                } else {
                    ("  ", Style::default())
                };
                spans.push(Span::styled(text, picked.unwrap_or(style)));
            }
            let (text, style) = cells.get(i).cloned().unwrap_or_default();
            let text = fit(&text, self.widths[i]);
            let pad = " ".repeat(self.widths[i] - display_width(&text));
            let text = if self.cols[i].right {
                pad + &text
            } else if Some(i) == last && !padded {
                text
            } else {
                text + &pad
            };
            let style = picked.map_or(style, |p| style.patch(p));
            spans.push(Span::styled(text, style));
        }
        spans
    }

    /// The status of a row under way, in the room left after `cols_in`.
    fn push_status(
        &self,
        spans: &mut Vec<Span<'static>>,
        cols_in: &[usize],
        (text, style): (&str, Style),
        frame: Option<usize>,
    ) {
        let gap = if cols_in.is_empty() { 0 } else { 2 };
        let room = self.width.saturating_sub(self.across(cols_in) + gap);
        if room > 0 {
            spans.push(Span::raw(" ".repeat(gap)));
            let text = fit(text, room);
            match frame {
                Some(frame) => spans.extend(under_way(&text, style, Some(frame), self.shine)),
                None => spans.push(Span::styled(text, style)),
            }
        }
    }

    fn rule(&self) -> Line<'static> {
        let text: String = self
            .shown
            .iter()
            .enumerate()
            .map(|(n, &i)| {
                let lead = match (n, self.cols[i].sep) {
                    (0, _) => "",
                    (_, true) => "─┼─",
                    (_, false) => "──",
                };
                format!("{lead}{}", "─".repeat(self.widths[i]))
            })
            .collect();
        Line::from(Span::styled(text, self.faint))
    }
}

/// `── title ─────…` across `width`.
pub(super) fn section_rule(title: &str, width: usize, theme: &Theme) -> Line<'static> {
    let faint = Style::default().fg(theme.colors.faint);
    let title = fit(title, width.saturating_sub(4));
    let rest = width.saturating_sub(display_width(&title) + 4);
    Line::from(vec![
        Span::styled("── ", faint),
        Span::styled(
            title,
            Style::default()
                .fg(theme.colors.text)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {}", "─".repeat(rest)), faint),
    ])
}

pub(super) fn muted_line<'a>(text: impl Into<String>, theme: &Theme) -> Line<'a> {
    Line::from(Span::styled(
        text.into(),
        Style::default().fg(theme.colors.muted),
    ))
}
