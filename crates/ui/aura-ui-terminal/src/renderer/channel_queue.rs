//! The QUEUE tab of an open channel's side panel: every merge of the
//! merge queue of the channel's repo, newest first, as an info document.
//!
//! A header names the branch merges land on and the repo's folder; then
//! each merge is a block: its state, coloured, with its branch and the
//! short sha pushed, then who pushed it, when it was queued and finished,
//! how many times it ran when more than once, the sha the target moved
//! to, and what happened, wrapped to the column:
//!
//! ```text
//! Merges into main
//! ~/code/zode
//!
//! FAILED  feat/parser  a1b2c3d
//!   Failing   parser::escapes
//!   ---- parser::escapes stdout ----
//!   assertion `left == right` failed
//!   Agent     RE36
//!   Queued    11-27 20:31
//!   Finished  11-27 20:33
//!   Attempts  2
//!   Overlaps  q4 feat/lexer (RE37): src/parser.rs
//!   cargo test failed: parser::escapes
//! ```
//!
//! A failed merge shows the tests that failed it and their output (or the
//! files that conflicted) first, before its other facts. When the queue
//! found the target failing its own checks, that comes first of all,
//! above every merge.
//!
//! Without a queue, or with nothing in it, a note says where merges come
//! from. The session sends the newest merges a page at a time: under the
//! last one shown, `N older merges` says how many more there are, and
//! scrolling to it asks for the next page.

use super::chrome::Body;
use crate::app::PanelFocus;
use crate::components::{truncate_line, Info, Value};
use crate::events::{MergeEntryView, MergeQueueView, MergeStateView};
use crate::{App, Theme};
use chrono::{DateTime, Local};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    Frame,
};
use unicode_width::UnicodeWidthStr;

/// What QUEUE says while no merge has been queued into `target`, the
/// queue's target branch; the branch goes unnamed while unknown.
pub(super) fn no_merges(target: Option<&str>) -> String {
    let into = target
        .map(str::trim)
        .filter(|target| !target.is_empty())
        .map_or_else(
            || "its target branch".to_string(),
            |target| format!("`{target}`"),
        );
    format!(
        "No merges yet. Every push from an agent of this channel lands here and merges into \
         {into} one at a time."
    )
}

/// Draw the open channel's merge queue into `inner`, the body below the
/// tab bar of the panel bordered by `area`. Without an open channel there
/// is nothing to draw.
pub(super) fn render_channel_queue(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    app: &mut App,
    theme: &Theme,
) {
    if app.open_channel().is_none() {
        return;
    }
    let body = Body::new(area, inner, app.focus() == PanelFocus::Sandbox);
    let lines = queue_lines(app.open_channel_queue(), body.text_width(), theme);
    let (total, visible) = (lines.len(), body.visible());
    let first = app.sync_channel_queue_scroll(total, visible);
    body.draw_document(frame, lines, first, theme);
    if first + visible >= total {
        app.queue_end_reached();
    }
}

/// The QUEUE tab's lines, `width` columns wide.
pub(super) fn queue_lines(
    queue: Option<&MergeQueueView>,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut info = Info::new(width, theme);
    if let Some(queue) = queue {
        if !queue.target.trim().is_empty() {
            info.title(&format!("Merges into {}", queue.target));
        }
        if !queue.repo.trim().is_empty() {
            info.note(&queue.repo);
        }
        if let Some(broken) = queue.broken.as_deref().filter(|b| !b.trim().is_empty()) {
            info.blank();
            info.text(
                &format!("{} is broken: {broken}", queue.target),
                Value::Error,
            );
        }
    }
    let entries = queue.map_or(&[][..], |queue| queue.entries.as_slice());
    if entries.is_empty() {
        if queue.is_some() {
            info.blank();
        }
        info.note(&no_merges(queue.map(|queue| queue.target.as_str())));
        return info.into_lines();
    }
    let mut lines = info.into_lines();
    for entry in entries {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(entry_head(entry, width, theme));
        lines.extend(entry_facts(entry, width, theme));
    }
    if let Some(older) = queue.map(|queue| queue.older).filter(|&older| older > 0) {
        lines.push(Line::default());
        let mut info = Info::new(width, theme);
        info.note(&older_merges(older));
        lines.extend(info.into_lines());
    }
    lines
}

/// The line under the merges shown while older ones are not: scrolling
/// to it loads the next page.
fn older_merges(older: u64) -> String {
    match older {
        1 => "1 older merge".to_string(),
        n => format!("{n} older merges"),
    }
}

/// An entry's first line: its state in its colour, its branch, then the
/// short sha pushed, muted. The branch is cut first.
fn entry_head(entry: &MergeEntryView, width: usize, theme: &Theme) -> Line<'static> {
    let state = entry.state.label();
    let head = entry.head.trim();
    let head_room = if head.is_empty() { 0 } else { head.width() + 2 };
    let branch_room = width.saturating_sub(state.width() + 2 + head_room);
    let mut spans = vec![Span::styled(
        state,
        Style::default()
            .fg(state_color(entry.state, theme))
            .add_modifier(Modifier::BOLD),
    )];
    let branch = truncate_line(entry.branch.trim(), branch_room);
    if !branch.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            branch,
            Style::default().fg(theme.colors.foreground),
        ));
    }
    if !head.is_empty() && state.width() + 2 + head.width() <= width {
        spans.push(Span::styled(
            format!("  {head}"),
            Style::default().fg(theme.colors.muted),
        ));
    }
    Line::from(spans)
}

/// An entry's facts under its first line: who pushed it, when it was
/// queued and finished, its attempts when it ran more than once, what the
/// target moved to, the other work that changes the same files, and what
/// happened.
fn entry_facts(entry: &MergeEntryView, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let mut info = Info::new(width, theme).wrap_values();
    failure_first(&mut info, entry, theme);
    if let Some(agent) = entry.agent.as_deref().filter(|agent| !agent.is_empty()) {
        info.fact("Agent", agent, Value::Plain);
    }
    info.fact("Queued", &compact_time(&entry.queued_at), Value::Plain);
    if let Some(finished) = entry.finished_at.as_deref() {
        info.fact("Finished", &compact_time(finished), Value::Plain);
    }
    if entry.attempts > 1 {
        info.fact("Attempts", &entry.attempts.to_string(), Value::Plain);
    }
    if let Some(merged) = entry.merged.as_deref().filter(|sha| !sha.is_empty()) {
        info.fact("Merged", merged, Value::Quiet);
    }
    if let Some(pr) = entry.pr.as_deref().filter(|pr| !pr.is_empty()) {
        info.fact("PR", pr, Value::Quiet);
    }
    for (at, overlap) in entry.overlaps.iter().enumerate() {
        let label = if at == 0 { "Overlaps" } else { "" };
        info.fact(label, overlap, Value::Flag);
    }
    if let Some(detail) = entry.detail.as_deref().filter(|d| !d.trim().is_empty()) {
        let kind = if entry.state == MergeStateView::Failed {
            Value::Error
        } else {
            Value::Quiet
        };
        info.text(detail, kind);
    }
    info.into_lines()
}

/// A failed merge's failing tests and their output, before anything else
/// about it: what to fix is what its agent and the user look for first.
fn failure_first(info: &mut Info, entry: &MergeEntryView, theme: &Theme) {
    if entry.state != MergeStateView::Failed {
        return;
    }
    for (at, test) in entry.failing_tests.iter().enumerate() {
        let label = if at == 0 { "Failing" } else { "" };
        info.fact(label, test, Value::Error);
    }
    if let Some(excerpt) = entry.failure_excerpt.as_deref() {
        // Kept as printed, spaces and all: a snapshot diff reads by its
        // columns. Cut to the column, not wrapped.
        let room = info.indented_width();
        let style = Style::default().fg(theme.colors.muted);
        let lines = excerpt
            .lines()
            .filter(|line| !line.trim().is_empty())
            .take(EXCERPT_LINES)
            .map(|line| Line::styled(truncate_line(line, room), style));
        info.indented(lines);
    }
}

/// Most lines of a failure's output a QUEUE block shows.
const EXCERPT_LINES: usize = 24;

/// The colour of a merge's state: landed in the accent, failed in the
/// error colour, running in the info colour, so it stands apart from a
/// landed one, and waiting muted.
const fn state_color(state: MergeStateView, theme: &Theme) -> Color {
    match state {
        MergeStateView::Merged => theme.colors.success,
        MergeStateView::Failed => theme.colors.error,
        MergeStateView::Running => theme.colors.warning,
        MergeStateView::Queued => theme.colors.muted,
    }
}

/// An RFC 3339 time as `MM-DD HH:MM` in local time. A time that does not
/// parse is shown as sent, cut to its minutes.
pub(super) fn compact_time(at: &str) -> String {
    let at = at.trim();
    match DateTime::parse_from_rfc3339(at) {
        Ok(time) => time.with_timezone(&Local).format("%m-%d %H:%M").to_string(),
        Err(_) => at.chars().take(16).collect::<String>().replace('T', " "),
    }
}
