//! Draw what BENCH shows in the chat panel: one benchmark's numbers side
//! by side, or the form that sets one up. Its list is in the agents panel
//! (`agents::bench`).

mod detail;
mod form;
mod format;
mod runs;
mod summary;
mod table;

use super::chrome::{body_block, pad_sides, pad_top};
use crate::app::{BenchMode, FormField};
use crate::{App, Theme};
use detail::detail_lines;
use form::form_layout;
use ratatui::{
    layout::Rect,
    widgets::{Paragraph, Wrap},
    Frame,
};
use table::muted_line;

pub(super) use format::{fit, money, setup_counts, short_model, task_prefix, under_way};

/// Draw what BENCH shows in `area`, the chat panel below its header bar.
pub(super) fn render_benchmarks(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    theme: &Theme,
    is_focused: bool,
) {
    let block = body_block((is_focused, app.host_chrome()), theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    // Nothing of the conversation is on screen to click or select.
    app.set_chat_layout(Vec::new(), 0);
    app.set_chat_images(Vec::new());
    app.set_chat_code_copies(Vec::new());
    app.set_chat_width(inner.width);
    app.set_chat_text_area(inner);
    let rows = pad_top(inner);
    let padded = pad_sides(rows);
    if padded.width < 12 || padded.height == 0 {
        return;
    }
    let width = usize::from(padded.width);
    let frame_no = theme.animate_spinners.then(|| app.animation_frame());
    let state = app.bench();
    // The form wraps; the detail's tables fit the width, and wrapping
    // would only break them.
    let wrap = state.mode == Some(BenchMode::Form);
    let mut form_rows = None;
    let (lines, scroll) = if wrap {
        let (lines, rows) = form_layout(app, theme, width);
        form_rows = Some(rows);
        (lines, 0)
    } else {
        match state.shown() {
            Some(view) => {
                let lines = detail_lines(view, state.run, theme, width, frame_no);
                let last = u16::try_from(lines.len().saturating_sub(1)).unwrap_or(u16::MAX);
                (lines, state.scroll.min(last))
            }
            None => (vec![muted_line("That benchmark is gone.", theme)], 0),
        }
    };
    set_form_rows(app, padded, form_rows.unwrap_or_default());
    app.clamp_bench_scroll(scroll);
    app.set_chat_rows(Some((rows, usize::from(scroll))));
    let mut paragraph = Paragraph::new(lines).scroll((scroll, 0));
    if wrap {
        paragraph = paragraph.wrap(Wrap { trim: false });
    }
    frame.render_widget(paragraph, padded);
}

/// Hand the pointer the form's field `rows`, each the line of `padded`
/// it is drawn on. The form is drawn from its top, so a row's line is its
/// screen row.
fn set_form_rows(app: &mut App, padded: Rect, rows: Vec<(usize, FormField)>) {
    let rows: Vec<(u16, FormField)> = rows
        .into_iter()
        .filter(|(line, _)| *line < usize::from(padded.height))
        .filter_map(|(line, field)| Some((padded.y.checked_add(u16::try_from(line).ok()?)?, field)))
        .collect();
    let area = (!rows.is_empty()).then_some(padded);
    app.set_bench_form_rows(area, rows);
}

#[cfg(test)]
mod tests;
