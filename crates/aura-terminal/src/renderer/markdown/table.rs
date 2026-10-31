//! GitHub-style pipe tables drawn with box-drawing borders.

use super::inline::parse_inline;
use super::wrap::wrap_spans;
use crate::Theme;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

/// Narrowest a column is squeezed to when the table is wider than the panel.
const MIN_COLUMN_WIDTH: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Align {
    Left,
    Center,
    Right,
}

/// Whether a table (header row plus `|---|` separator) starts at `lines[i]`.
pub(super) fn starts_at(lines: &[&str], i: usize) -> bool {
    lines[i].contains('|')
        && lines
            .get(i + 1)
            .and_then(|next| alignments(next))
            .is_some_and(|aligns| aligns.len() == split_cells(lines[i]).len())
}

/// Index just past the last body row of the table starting at `start`.
pub(super) fn end(lines: &[&str], start: usize) -> usize {
    let body_start = start + 2;
    body_start
        + lines[body_start..]
            .iter()
            .take_while(|line| line.contains('|') && !line.trim().is_empty())
            .count()
}

/// Render `rows` (header, separator, body rows) to fit `width` columns.
pub(super) fn render(
    rows: &[&str],
    width: usize,
    base: Style,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let aligns = alignments(rows[1]).unwrap_or_default();
    let columns = aligns.len();
    let header_style = base.add_modifier(Modifier::BOLD);
    let parse_row = |row: &str, style: Style| -> Vec<Vec<Span<'static>>> {
        let mut cells: Vec<Vec<Span<'static>>> = split_cells(row)
            .into_iter()
            .map(|cell| parse_inline(cell, style, theme))
            .collect();
        cells.resize_with(columns, Vec::new);
        cells
    };
    let header = parse_row(rows[0], header_style);
    let body: Vec<_> = rows[2..].iter().map(|row| parse_row(row, base)).collect();

    let natural: Vec<usize> = (0..columns)
        .map(|col| {
            std::iter::once(&header)
                .chain(&body)
                .map(|row| row[col].iter().map(Span::width).sum::<usize>())
                .max()
                .unwrap_or(0)
                .max(1)
        })
        .collect();
    let widths = fit_columns(&natural, width.saturating_sub(3 * columns + 1), width == 0);

    let border = Style::default().fg(theme.colors.muted);
    let mut lines = vec![rule(&widths, ['┌', '┬', '┐'], border)];
    lines.extend(render_row(&header, &widths, &aligns, border));
    lines.push(rule(&widths, ['├', '┼', '┤'], border));
    for row in &body {
        lines.extend(render_row(row, &widths, &aligns, border));
    }
    lines.push(rule(&widths, ['└', '┴', '┘'], border));
    lines
}

/// Column widths that fit `available` content columns: columns narrower
/// than an even share keep their natural width and the rest split what
/// remains.
fn fit_columns(natural: &[usize], available: usize, unbounded: bool) -> Vec<usize> {
    if unbounded || natural.iter().sum::<usize>() <= available {
        return natural.to_vec();
    }
    let mut widths = vec![0; natural.len()];
    let mut open: Vec<usize> = (0..natural.len()).collect();
    let mut remaining = available;
    while !open.is_empty() {
        let share = remaining / open.len();
        let (fits, wide): (Vec<usize>, Vec<usize>) =
            open.iter().partition(|&&col| natural[col] <= share);
        if fits.is_empty() {
            let extra = remaining % open.len();
            for (k, &col) in wide.iter().enumerate() {
                widths[col] = (share + usize::from(k < extra)).max(MIN_COLUMN_WIDTH);
            }
            break;
        }
        for &col in &fits {
            widths[col] = natural[col];
            remaining -= natural[col];
        }
        open = wide;
    }
    widths
}

fn rule(widths: &[usize], [left, mid, right]: [char; 3], style: Style) -> Line<'static> {
    let segments: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
    let text = format!("{left}{}{right}", segments.join(&mid.to_string()));
    Line::from(Span::styled(text, style))
}

/// One table row, wrapped inside its cells, as one or more lines.
fn render_row(
    cells: &[Vec<Span<'static>>],
    widths: &[usize],
    aligns: &[Align],
    border: Style,
) -> Vec<Line<'static>> {
    let wrapped: Vec<Vec<Line<'static>>> = cells
        .iter()
        .zip(widths)
        .map(|(cell, &width)| wrap_spans(cell, width, &[], &[]))
        .collect();
    let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);

    (0..height)
        .map(|k| {
            let mut spans = vec![Span::styled("│", border)];
            for ((cell_lines, &width), &align) in wrapped.iter().zip(widths).zip(aligns) {
                let content = cell_lines
                    .get(k)
                    .map(|line| line.spans.clone())
                    .unwrap_or_default();
                let used: usize = content.iter().map(Span::width).sum();
                let gap = width.saturating_sub(used);
                let (left, right) = match align {
                    Align::Left => (0, gap),
                    Align::Right => (gap, 0),
                    Align::Center => (gap / 2, gap - gap / 2),
                };
                spans.push(Span::raw(" ".repeat(left + 1)));
                spans.extend(content);
                spans.push(Span::raw(" ".repeat(right + 1)));
                spans.push(Span::styled("│", border));
            }
            Line::from(spans)
        })
        .collect()
}

/// Column alignments if `line` is a separator row like `|:--|--:|`.
fn alignments(line: &str) -> Option<Vec<Align>> {
    let cells = split_cells(line);
    if cells.is_empty() {
        return None;
    }
    cells
        .into_iter()
        .map(|cell| {
            let dashes = cell.trim_start_matches(':').trim_end_matches(':');
            if dashes.is_empty() || !dashes.bytes().all(|b| b == b'-') {
                return None;
            }
            Some(match (cell.starts_with(':'), cell.ends_with(':')) {
                (true, true) => Align::Center,
                (false, true) => Align::Right,
                _ => Align::Left,
            })
        })
        .collect()
}

/// Split a row on unescaped pipes outside code spans, dropping the
/// optional outer pipes.
fn split_cells(line: &str) -> Vec<&str> {
    let row = line.trim();
    let row = row.strip_prefix('|').unwrap_or(row);
    let row = match row.strip_suffix('|') {
        Some(stripped) if !stripped.ends_with('\\') => stripped,
        _ => row,
    };
    if row.trim().is_empty() {
        return Vec::new();
    }

    let bytes = row.as_bytes();
    let mut cells = Vec::new();
    let mut cell_start = 0;
    let mut in_code = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1,
            b'`' => in_code = !in_code,
            b'|' if !in_code => {
                cells.push(row[cell_start..i].trim());
                cell_start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    cells.push(row[cell_start..].trim());
    cells
}
