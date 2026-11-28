//! The side panel's REPO tab (see [`crate::app::repo_tab`]): a strip of
//! filters over the branch's sync, the uncommitted changes and the
//! timeline, newest first, or over the repo's branches, tags or
//! worktrees. A strip too wide for the panel shows the filters around the
//! one picked, with `‹` and `›` where it is cut.
//!
//! ```text
//!  All  Changes  Commits  Pushes │ Branches  Tags
//!  ↑  main  2 ahead of origin/main         a1b2c3d4
//!  ▾  Uncommitted changes                   2 files
//!    M  src/lib.rs                           staged
//!    ?  notes.txt
//!  ○  Add the REPO tab  a1b2c3d4 · main          now
//!  ↑  Pushed 2 commits to origin/main             5m
//!  ●  Fix the parser  e5f6a7b8                    2h
//! ```

mod rows;
mod strip;
#[cfg(test)]
mod tests;

use super::chrome::{Body, ScrollView};
use super::side_list::render_empty;
use crate::app::repo_tab::RepoFilter;
use crate::app::{HoverTarget, PanelFocus};
use crate::components::CellState;
use crate::{App, Theme};
use aura_core_types::time::now_ms;
use ratatui::{layout::Rect, text::Line, Frame};
use rows::{row_line, Look};

/// What the tab says until the first view of the agent on screen comes.
const READING: &str = "Reading the repo…";

/// Draw the REPO tab into `inner`, the body below the tab bar of the
/// panel bordered by `area`.
pub(super) fn render_repo(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    app: &mut App,
    theme: &Theme,
) {
    let problem = match app.repo_view() {
        None => Some(READING.to_string()),
        Some(view) => view.problem.clone(),
    };
    if let Some(problem) = problem {
        app.set_repo_strip(Vec::new(), (None, None));
        app.set_repo_rows_area(None, 0);
        render_empty(frame, inner, &problem, theme);
        return;
    }
    let strip = Rect {
        height: inner.height.min(1),
        ..inner
    };
    strip::render_strip(frame, strip, app, theme);

    let list = Rect {
        y: inner.y + strip.height,
        height: inner.height - strip.height,
        ..inner
    };
    render_list(frame, area, list, app, theme);
}

/// Draw the rows into `list`, the body below the strip of the panel
/// bordered by `area`, and record where they went.
fn render_list(frame: &mut Frame, area: Rect, list: Rect, app: &mut App, theme: &Theme) {
    let is_focused = app.focus() == PanelFocus::Sandbox;
    let body = Body::list(app, area, list, is_focused);
    let visible = body.visible();
    let scroll = app.sync_repo_scroll(visible);
    let hovered = match app.hover() {
        Some(HoverTarget::RepoRow(index)) => Some(index),
        _ => None,
    };
    let selected = app.repo_selected();
    let look = Look {
        width: body.width(),
        now: now_ms(),
        folded: app.repo_changes_folded(),
        nested: app.repo_filter() == RepoFilter::All,
    };
    let Some(view) = app.repo_view() else {
        return;
    };
    let rows = app.repo_rows();
    let lines: Vec<Line<'static>> = rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible)
        .map(|(i, &row)| {
            let state = CellState::of(is_focused && i == selected, hovered == Some(i));
            row_line(view, row, state, look, theme)
        })
        .collect();
    let shown = ScrollView {
        total: rows.len(),
        visible,
        first: scroll,
    };
    app.set_repo_rows_area(Some(body.rows()), scroll);
    body.draw(frame, lines, shown, theme);
}
