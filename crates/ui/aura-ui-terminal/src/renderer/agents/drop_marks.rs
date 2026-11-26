//! The line marking where a dragged project or agent goes in the
//! PROJECTS tab.

use crate::app::ProjectListRow;
use crate::components::ROW_INSET;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// Draw the drop line in `color` on `line`, list row `row_index`, when it
/// goes there.
pub(super) fn mark_drop(
    line: &mut Line<'_>,
    drop_line: Option<DropLine>,
    row_index: usize,
    width: usize,
    color: Color,
) {
    match drop_line {
        Some(DropLine::Rule(at)) if at == row_index => {
            *line = drop_rule(width, color);
        }
        Some(DropLine::Under(at)) if at == row_index => {
            underline(line, color);
        }
        _ => {}
    }
}

/// How the edge a drag's rows go to is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DropLine {
    /// A rule across this gap row, between two projects
    Rule(usize),
    /// An underline along this row, the one just above the edge
    Under(usize),
}

/// How to draw the edge just above list row `edge`: across the gap row
/// beside it when there is one, else under the row above it. `None` above
/// the first row, where there is no row to draw on.
pub(super) fn drop_line_at(rows: &[ProjectListRow], edge: usize) -> Option<DropLine> {
    let above = edge.checked_sub(1);
    if let Some(above) = above.filter(|&above| rows.get(above) == Some(&ProjectListRow::Gap)) {
        return Some(DropLine::Rule(above));
    }
    if rows.get(edge) == Some(&ProjectListRow::Gap) {
        return Some(DropLine::Rule(edge));
    }
    above.map(DropLine::Under)
}

/// A horizontal rule `width` columns wide, inset like the list's rows.
fn drop_rule(width: usize, color: Color) -> Line<'static> {
    let inset = " ".repeat(ROW_INSET);
    let rule = DROP_RULE.repeat(width.saturating_sub(2 * ROW_INSET));
    Line::from(vec![
        Span::raw(inset.clone()),
        Span::styled(rule, Style::default().fg(color)),
        Span::raw(inset),
    ])
}

/// Underline all of `line` in `color`, keeping its text colors.
fn underline(line: &mut Line<'_>, color: Color) {
    for span in &mut line.spans {
        span.style = span
            .style
            .add_modifier(Modifier::UNDERLINED)
            .underline_color(color);
    }
}

/// Glyph of the rule drawn where a dragged project or agent goes.
const DROP_RULE: &str = "─";

/// Glyph of the rail beside a drag's hit area.
pub(in crate::renderer) const DROP_RAIL: &str = "▎";
