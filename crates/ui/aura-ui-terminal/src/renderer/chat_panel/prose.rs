//! Prose rows: how many columns a prose line covers, and how it is packed
//! into the cells it covers in the front end's face.

use crate::renderer::markdown::is_inline_code;
use crate::renderer::text::{display_width, Measure};
use crate::Theme;
use ratatui::{buffer::Buffer, layout::Rect, style::Style, text::Line};
use std::ops::Range;

/// How many of `area`'s columns a prose `line` covers, and where its
/// inline code falls.
pub(in crate::renderer) fn prose_columns(
    line: &Line<'_>,
    area: Rect,
    theme: &Theme,
) -> (u16, Vec<Range<u16>>) {
    let mut code: Vec<Range<u16>> = Vec::new();
    let mut column = area.x;
    let end = area.right();
    for span in &line.spans {
        let width = u16::try_from(span.width()).unwrap_or(u16::MAX);
        let next = column.saturating_add(width).min(end);
        if is_inline_code(span.style, theme) && next > column {
            match code.last_mut() {
                Some(last) if last.end == column => last.end = next,
                _ => code.push(column..next),
            }
        }
        column = next;
    }
    (column - area.x, code)
}

/// Write a prose `line` into `row` with its text after `indent` columns
/// packed into the cells it covers in the front end's face, `measure`
/// wide: each run of one style gets the cells its width rounds to, its
/// characters spread across them. Inline code keeps a cell per column.
/// Returns the columns the line covers and where its inline code falls.
pub(in crate::renderer) fn pack_prose(
    buffer: &mut Buffer,
    line: &Line<'_>,
    (row, indent): (Rect, u16),
    measure: Measure,
    theme: &Theme,
) -> (u16, Vec<Range<u16>>) {
    pack_prose_mapped(buffer, line, (row, indent), measure, theme).0
}

/// [`pack_prose`], and which cells each of the line's columns went to.
pub(in crate::renderer) fn pack_prose_mapped(
    buffer: &mut Buffer,
    line: &Line<'_>,
    (row, indent): (Rect, u16),
    measure: Measure,
    theme: &Theme,
) -> ((u16, Vec<Range<u16>>), ColumnMap) {
    let (runs, skipped) = style_runs(line, indent);
    let grid: Vec<u16> = runs
        .iter()
        .map(|(symbols, _)| {
            let width: usize = symbols.iter().map(|s| display_width(s)).sum();
            u16::try_from(width).unwrap_or(u16::MAX)
        })
        .collect();
    let code_runs: Vec<bool> = runs
        .iter()
        .map(|(_, style)| is_inline_code(*style, theme))
        .collect();
    let (cell, cells) = pack_runs(buffer, runs, (row, indent), measure, theme);
    for x in cell..row.width {
        if let Some(slot) = buffer.cell_mut((row.x + x, row.y)) {
            slot.reset();
        }
    }
    let code = cells
        .iter()
        .zip(code_runs)
        .filter(|(cells, code)| *code && !cells.is_empty())
        .map(|(cells, _)| row.x + cells.start..row.x + cells.end)
        .collect();
    let mut column = skipped;
    let map = grid
        .into_iter()
        .zip(cells)
        .map(|(width, cells)| {
            let start = column;
            column = column.saturating_add(width);
            (start..column, cells)
        })
        .collect();
    ((cell, code), ColumnMap(map))
}

/// Where packing put a prose row's text: for each run of one style, the
/// columns it takes on the grid and the cells it was packed into, both
/// counted from the row's left edge. Columns before the first run, the
/// indent, stay where they are.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::renderer) struct ColumnMap(Vec<(Range<u16>, Range<u16>)>);

impl ColumnMap {
    #[cfg(test)]
    pub(in crate::renderer) fn from_runs(runs: Vec<(Range<u16>, Range<u16>)>) -> Self {
        Self(runs)
    }

    /// The cells the grid's `columns` were packed into. A column inside
    /// a run goes to its share of the run's cells.
    pub(in crate::renderer) fn cells(&self, columns: Range<u16>) -> Range<u16> {
        let start = self.cell_at(columns.start, false);
        let end = self.cell_at(columns.end, true);
        start..end.max(start)
    }

    /// The cell `column` starts at, or with `end` the cell after the one
    /// it ends.
    fn cell_at(&self, column: u16, end: bool) -> u16 {
        for (grid, cells) in &self.0 {
            let inside = if end {
                grid.start < column && column <= grid.end
            } else {
                grid.start <= column && column < grid.end
            };
            if inside {
                let columns = u32::from(grid.end - grid.start);
                let share = u32::from(column - grid.start) * u32::from(cells.end - cells.start);
                let offset = if end {
                    share.div_ceil(columns)
                } else {
                    share / columns
                };
                return cells.start + u16::try_from(offset).unwrap_or(u16::MAX);
            }
        }
        match self.0.last() {
            Some((grid, cells)) if column >= grid.end => cells.end,
            _ => column,
        }
    }
}

/// The graphemes of `line` after its first `indent` columns, gathered
/// into runs of one style, and the columns skipped before them.
fn style_runs<'a>(line: &'a Line<'a>, indent: u16) -> (Vec<(Vec<&'a str>, Style)>, u16) {
    let mut runs: Vec<(Vec<&str>, Style)> = Vec::new();
    let mut skipped = 0;
    for grapheme in line.styled_graphemes(Style::default()) {
        if skipped < usize::from(indent) {
            skipped += display_width(grapheme.symbol);
            continue;
        }
        match runs.last_mut() {
            Some((symbols, style)) if *style == grapheme.style => symbols.push(grapheme.symbol),
            _ => runs.push((vec![grapheme.symbol], grapheme.style)),
        }
    }
    (runs, u16::try_from(skipped).unwrap_or(u16::MAX))
}

/// Write `runs` into `row` from column `indent` on, each into the cells
/// its width rounds to. Returns the cell after the last one written and
/// the cells each run went to, from the row's left edge: none for a run
/// past the edge, which joins the last cell.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn pack_runs(
    buffer: &mut Buffer,
    runs: Vec<(Vec<&str>, Style)>,
    (row, indent): (Rect, u16),
    measure: Measure,
    theme: &Theme,
) -> (u16, Vec<Range<u16>>) {
    let mut packed: Vec<Range<u16>> = Vec::with_capacity(runs.len());
    let mut cell = indent;
    let mut position = f32::from(indent);
    let mut last = None;
    for (symbols, style) in runs {
        let width = if is_inline_code(style, theme) {
            symbols.iter().map(|s| display_width(s)).sum::<usize>() as f32
        } else {
            measure.width(&symbols.concat(), style)
        };
        let start = cell;
        position += width;
        let end = if is_inline_code(style, theme) {
            start.saturating_add(width as u16)
        } else {
            (position.round() as u16).max(start + 1)
        }
        .min(row.width);
        if end <= start {
            // Past the edge: the rest joins the last cell.
            if let Some(cell) = last.and_then(|x| buffer.cell_mut((x, row.y))) {
                let joined = format!("{}{}", cell.symbol(), symbols.concat());
                cell.set_symbol(&joined);
            }
            packed.push(cell..cell);
            continue;
        }
        let cells = usize::from(end - start);
        for i in 0..cells {
            let share = &symbols[i * symbols.len() / cells..(i + 1) * symbols.len() / cells];
            if let Some(slot) = buffer.cell_mut((row.x + start + i as u16, row.y)) {
                slot.reset();
                slot.set_symbol(&share.concat()).set_style(style);
            }
        }
        packed.push(start..end);
        last = Some(row.x + end - 1);
        cell = end;
        position = position.max(f32::from(cell) - 0.5);
    }
    (cell, packed)
}
