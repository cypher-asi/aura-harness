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
    let runs = style_runs(line, indent);
    let (cell, code) = pack_runs(buffer, runs, (row, indent), measure, theme);
    for x in cell..row.width {
        if let Some(slot) = buffer.cell_mut((row.x + x, row.y)) {
            slot.reset();
        }
    }
    (cell, code)
}

/// The graphemes of `line` after its first `indent` columns, gathered
/// into runs of one style.
fn style_runs<'a>(line: &'a Line<'a>, indent: u16) -> Vec<(Vec<&'a str>, Style)> {
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
    runs
}

/// Write `runs` into `row` from column `indent` on, each into the cells
/// its width rounds to. Returns the cell after the last one written and
/// where the inline code falls.
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
    let mut code: Vec<Range<u16>> = Vec::new();
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
        if is_inline_code(style, theme) {
            code.push(row.x + start..row.x + end);
        }
        last = Some(row.x + end - 1);
        cell = end;
        position = position.max(f32::from(cell) - 0.5);
    }
    (cell, code)
}
