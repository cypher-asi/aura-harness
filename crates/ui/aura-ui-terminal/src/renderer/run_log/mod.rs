//! Draw the chat panel's LOG tab: a header of three lines (the run's
//! status, title and `Copy log`; its totals and where the log is stored;
//! a rule), the heads of the tokens, cost and time columns, the run's
//! panels that fold open and shut, and the keys at the bottom.
//!
//! Until the log arrives the tab is blank: no title, no loading line, no
//! keys. The log, or a notice saying why there is none, then fades in
//! ([`crate::app::run_log::FADE_IN`]). Only the log of the agent on screen
//! is ever drawn.

mod body;
mod header;
mod rows;

use super::chrome::body_block;
use crate::animation::fade_in;
use crate::app::run_log::{LogPanel, LogState};
use crate::{App, Theme};
use body::prose_wrap;
use header::{column_heads, header_lines, keys_line};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use rows::document;

/// Lines of a section shown before the rest is cut.
const SECTION_LINES: usize = 400;

/// The button on top that copies the whole log.
const COPY_BUTTON: &str = " ⧉ Copy log ";

/// The button a moment after it copied, as wide.
const COPIED_BUTTON: &str = "  ✓ Copied  ";

/// Widths of the tokens, cost and time columns at the right of turn and
/// step rows.
const COLUMN_WIDTHS: [usize; 3] = [8, 7, 7];

/// What the columns hold, over them.
const COLUMN_HEADS: [&str; 3] = ["tokens", "cost", "time"];

/// Narrowest row with room for all three columns; narrower rows keep the
/// time.
const ALL_COLUMNS: usize = 72;

/// Width of a step's kind column, `decision` and a space.
const KIND_WIDTH: usize = 9;

/// Width of the status mark at the very left of every row, so what went
/// wrong lines up down the tab.
const GUTTER: usize = 2;

/// Lines above the panels: the header's three and the column heads.
const TOP_LINES: u16 = 4;

/// The keys at the bottom, each with what it does.
const KEYS: [(&str, &str); 9] = [
    ("↑↓", "move"),
    ("⏎", "fold"),
    ("e/E", "all"),
    ("J", "json"),
    ("y", "copy"),
    ("c", "copy log"),
    ("o", "open"),
    ("r", "reload"),
    ("esc", "back"),
];

/// Draw the LOG tab in `area`, the chat panel below its header bar.
pub(super) fn render_run_log(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    theme: &Theme,
    is_focused: bool,
) {
    let padded = reset_chat(frame, area, app, theme, is_focused);
    if padded.width < 16 || padded.height < TOP_LINES + 3 {
        app.run_log_mut().set_hits(Vec::new(), None);
        return;
    }
    let width = usize::from(padded.width);
    let spinner = app.spinner_char();
    let state = app.run_log();
    let fade = state.fading();
    let Some(view) = state.shown(app.active_agent_id()) else {
        // Nothing before the log: no title, no loading line, no keys.
        // Only a notice, once one came, says why there is none.
        let mut lines = notice_lines(state, width, theme);
        if let Some(progress) = fade {
            fade_in(&mut lines, progress, theme);
        }
        frame.render_widget(Paragraph::new(lines), padded);
        app.set_chat_rows(None);
        app.run_log_mut().set_hits(Vec::new(), None);
        return;
    };

    let (mut top, button) = header_lines(state, view, padded, spinner, theme);
    top.push(column_heads(width, theme));
    let (mut lines, owners, selected_line) = document(state, width, spinner, theme);
    if let Some(progress) = fade {
        fade_in(&mut top, progress, theme);
        fade_in(&mut lines, progress, theme);
    }
    let body = draw_top_and_keys(frame, padded, top, theme);
    let scroll = follow_scroll(state, selected_line, lines.len(), usize::from(body.height));
    let hits = visible_hits(owners, scroll, body);
    let offset = u16::try_from(scroll).unwrap_or(u16::MAX);
    frame.render_widget(Paragraph::new(lines).scroll((offset, 0)), body);

    app.set_chat_rows(Some((body, scroll)));
    let state = app.run_log_mut();
    state.scroll = scroll;
    state.follow = false;
    state.set_hits(hits, button);
}

/// Draw the panel's border and clear what the chat tab left in `app`;
/// returns the inner area less a column each side.
fn reset_chat(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    theme: &Theme,
    is_focused: bool,
) -> Rect {
    let block = body_block((is_focused, app.host_chrome()), theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.set_chat_layout(Vec::new(), 0);
    app.set_chat_images(Vec::new());
    app.set_chat_code_copies(Vec::new());
    app.set_chat_width(inner.width);
    app.set_chat_text_area(inner);
    Rect {
        x: inner.x.saturating_add(1),
        width: inner.width.saturating_sub(2),
        ..inner
    }
}

/// Draw the header and column heads on top and the keys at the bottom of
/// `padded`; returns the rect between them, for the panels.
fn draw_top_and_keys(
    frame: &mut Frame,
    padded: Rect,
    top: Vec<Line<'static>>,
    theme: &Theme,
) -> Rect {
    frame.render_widget(
        Paragraph::new(top),
        Rect {
            height: TOP_LINES,
            ..padded
        },
    );
    let footer = Rect {
        y: padded.bottom() - 1,
        height: 1,
        ..padded
    };
    let width = usize::from(padded.width);
    frame.render_widget(Paragraph::new(keys_line(width, theme)), footer);
    Rect {
        y: padded.y + TOP_LINES,
        height: padded.height.saturating_sub(TOP_LINES + 1),
        ..padded
    }
}

/// The scroll that keeps the selected panel's header in view when the
/// selection moved, within the `total` lines in `height` rows.
fn follow_scroll(
    state: &LogState,
    selected_line: Option<usize>,
    total: usize,
    height: usize,
) -> usize {
    let mut scroll = state.scroll;
    if state.follow {
        if let Some(line) = selected_line {
            if line < scroll {
                scroll = line;
            } else if line >= scroll + height {
                scroll = line + 1 - height;
            }
        }
    }
    scroll.min(total.saturating_sub(height))
}

/// The screen row of each panel header shown in `body` from `scroll`.
fn visible_hits(owners: Vec<Option<LogPanel>>, scroll: usize, body: Rect) -> Vec<(u16, LogPanel)> {
    owners
        .into_iter()
        .enumerate()
        .skip(scroll)
        .take(usize::from(body.height))
        .filter_map(|(i, owner)| {
            let y = body.y + u16::try_from(i - scroll).ok()?;
            Some((y, owner?))
        })
        .collect()
}

/// Lines and owners of a panel list, and the line of the selected
/// panel's header.
type Document = (Vec<Line<'static>>, Vec<Option<LogPanel>>, Option<usize>);

/// What the tab shows with no log of the agent on screen: why there is
/// none, once a notice came, else nothing at all. A notice from an
/// earlier load is not shown while the log is asked for again: it may no
/// longer hold.
fn notice_lines(state: &LogState, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let Some(notice) = state.notice.as_ref().filter(|_| !state.is_loading()) else {
        return Vec::new();
    };
    let muted = Style::default().fg(theme.colors.muted);
    let faint = Style::default().fg(theme.colors.faint);
    let room = width.saturating_sub(GUTTER);
    let indent = || Span::raw(" ".repeat(GUTTER));
    let mut lines = vec![Line::default()];
    for line in prose_wrap(notice, room) {
        lines.push(Line::from(vec![indent(), Span::styled(line, muted)]));
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![
        indent(),
        Span::styled("r", muted),
        Span::styled(" to retry", faint),
    ]));
    lines
}

#[cfg(test)]
mod tests;
