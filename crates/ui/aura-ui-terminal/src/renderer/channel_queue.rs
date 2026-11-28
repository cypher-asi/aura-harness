//! The QUEUE tab of an open channel's side panel: every merge of the
//! merge queue of the channel's repo, newest first, as an info document
//! laid out like INFO.
//!
//! A section names the branch merges land on, with the repo's folder
//! under it; then each merge is a section of its own, headed by a status
//! dot in its state's color, its state and the short sha pushed. Under
//! the header come its branch on a line of its own, which opens the
//! branch on GitHub when the repo is there, then who pushed it, when it
//! was queued and finished, how many times it ran when more than once,
//! the sha the target moved to, and what happened, wrapped to the column:
//!
//! ```text
//! MERGES INTO  main
//!   ~/code/zode
//!
//! ● FAILED  a1b2c3d
//!   feat/parser
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
//! files that conflicted) right under its branch, before its other facts.
//! When the queue found the target failing its own checks, a `BROKEN`
//! section with a failed dot says why, above every merge.
//!
//! Only the dots are colored, and nothing is bold: the text is
//! `foreground`, or `muted` for ids, shas, output and what happened.
//!
//! Without a queue, or with nothing in it, a note says where merges come
//! from. The session sends the newest merges a page at a time: under the
//! last one shown, `N older merges` says how many more there are, and
//! scrolling to it asks for the next page.

use super::chrome::{pad_sides, Body};
use crate::app::PanelFocus;
use crate::components::{truncate_line, Info, StatusDot, Value};
use crate::events::{MergeEntryView, MergeQueueView, MergeStateView};
use crate::{App, Theme};
use chrono::{DateTime, Local};
use ratatui::{layout::Rect, style::Style, text::Line, Frame};

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

/// The QUEUE tab's document: its lines, and the lines that open a
/// branch's page, by index, with the page.
pub(super) struct QueueDoc {
    pub lines: Vec<Line<'static>>,
    pub links: Vec<(usize, String)>,
}

/// Draw the open channel's merge queue into `inner`, the body below the
/// tab bar of the panel bordered by `area`, and record where its branch
/// links went. Without an open channel there is nothing to draw.
pub(super) fn render_channel_queue(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    app: &mut App,
    theme: &Theme,
) {
    app.set_channel_queue_links(Vec::new());
    if app.open_channel().is_none() {
        return;
    }
    let body = Body::new(area, inner, app.focus() == PanelFocus::Sandbox);
    let doc = queue_doc(app.open_channel_queue(), body.text_width(), theme);
    let (total, visible) = (doc.lines.len(), body.visible());
    let first = app.sync_channel_queue_scroll(total, visible);
    app.set_channel_queue_links(link_areas(&doc.links, pad_sides(body.rows()), first));
    body.draw_document(frame, doc.lines, first, theme);
    if first + visible >= total {
        app.queue_end_reached();
    }
}

/// Where each of `links` is on screen, its document drawn in `rows` from
/// line `first`: the links scrolled out of `rows` are left out.
fn link_areas(links: &[(usize, String)], rows: Rect, first: usize) -> Vec<(Rect, String)> {
    links
        .iter()
        .filter_map(|(line, url)| {
            let offset = u16::try_from(line.checked_sub(first)?).ok()?;
            (offset < rows.height).then(|| {
                let area = Rect {
                    y: rows.y + offset,
                    height: 1,
                    ..rows
                };
                (area, url.clone())
            })
        })
        .collect()
}

/// The QUEUE tab's lines, `width` columns wide.
#[cfg(test)]
pub(super) fn queue_lines(
    queue: Option<&MergeQueueView>,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    queue_doc(queue, width, theme).lines
}

/// The QUEUE tab's document, `width` columns wide.
pub(super) fn queue_doc(queue: Option<&MergeQueueView>, width: usize, theme: &Theme) -> QueueDoc {
    let mut info = Info::new(width, theme).wrap_values();
    if let Some(queue) = queue {
        target_section(&mut info, queue);
    }
    let entries = queue.map_or(&[][..], |queue| queue.entries.as_slice());
    if entries.is_empty() {
        if queue.is_some_and(|queue| !queue.target.trim().is_empty()) {
            info.blank();
        }
        info.note(&no_merges(queue.map(|queue| queue.target.as_str())));
        return QueueDoc {
            lines: info.into_lines(),
            links: Vec::new(),
        };
    }
    let mut links = Vec::new();
    for entry in entries {
        if let Some(link) = entry_lines(&mut info, queue, entry, theme) {
            links.push(link);
        }
    }
    if let Some(older) = queue.map(|queue| queue.older).filter(|&older| older > 0) {
        info.blank();
        info.note(&older_merges(older));
    }
    QueueDoc {
        lines: info.into_lines(),
        links,
    }
}

/// `MERGES INTO  main`, then the repo's folder under it; nothing for a
/// queue whose target is not known yet.
fn target_section(info: &mut Info<'_>, queue: &MergeQueueView) {
    let target = queue.target.trim();
    if target.is_empty() {
        return;
    }
    info.section("MERGES INTO", Some(target));
    if !queue.repo.trim().is_empty() {
        info.item(queue.repo.trim(), None);
    }
    if let Some(broken) = queue.broken.as_deref().filter(|b| !b.trim().is_empty()) {
        info.marked_section(StatusDot::Failed, "BROKEN", Some(target));
        info.text(&format!("{target} is broken: {broken}"), Value::Plain);
    }
}

/// The line under the merges shown while older ones are not: scrolling
/// to it loads the next page.
fn older_merges(older: u64) -> String {
    match older {
        1 => "1 older merge".to_string(),
        n => format!("{n} older merges"),
    }
}

/// An entry as its own section: its header, its branch on a line of its
/// own, then its facts. Returns the branch's line and page when the
/// branch opens one.
fn entry_lines(
    info: &mut Info<'_>,
    queue: Option<&MergeQueueView>,
    entry: &MergeEntryView,
    theme: &Theme,
) -> Option<(usize, String)> {
    let head = entry.head.trim();
    let note = (!head.is_empty()).then_some(head);
    info.marked_section(state_dot(entry.state), entry.state.label(), note);
    let branch = entry.branch.trim();
    let link = match queue.and_then(|queue| queue.branch_url(branch)) {
        Some(url) => Some((info.link(branch), url)),
        None => {
            if !branch.is_empty() {
                info.item(branch, None);
            }
            None
        }
    };
    failure_first(info, entry, theme);
    entry_facts(info, entry);
    link
}

/// A failed merge's failing tests and their output, before anything else
/// about it: what to fix is what its agent and the user look for first.
fn failure_first(info: &mut Info<'_>, entry: &MergeEntryView, theme: &Theme) {
    if entry.state != MergeStateView::Failed {
        return;
    }
    for (at, test) in entry.failing_tests.iter().enumerate() {
        let label = if at == 0 { "Failing" } else { "" };
        info.fact(label, test, Value::Plain);
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

/// An entry's facts under its branch: who pushed it, when it was queued
/// and finished, its attempts when it ran more than once, what the
/// target moved to, the other work that changes the same files, and what
/// happened. None of it is colored.
fn entry_facts(info: &mut Info<'_>, entry: &MergeEntryView) {
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
        info.fact(label, overlap, Value::Plain);
    }
    if let Some(detail) = entry.detail.as_deref().filter(|d| !d.trim().is_empty()) {
        info.text(detail, Value::Quiet);
    }
}

/// The dot of a merge's state: landed `Ok`, failed `Failed`, running
/// `Busy`, so it stands apart from a landed one, and waiting `Waiting`.
pub(super) const fn state_dot(state: MergeStateView) -> StatusDot {
    match state {
        MergeStateView::Merged => StatusDot::Ok,
        MergeStateView::Failed => StatusDot::Failed,
        MergeStateView::Running => StatusDot::Busy,
        MergeStateView::Queued => StatusDot::Waiting,
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
