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
//!   Queued    5 h ago
//!   Finished  2 minutes ago
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
//! Times read as people say them (`components::relative_time`): `just
//! now`, `2 minutes ago`, `5 h ago`, then `yesterday 14:32` and dates.
//! The tab records when the soonest of them next reads differently, and
//! the app redraws then, not every frame. The pointer over a time shows
//! it in full, to the second; over a branch it underlines it, as the
//! chat's links are.
//!
//! Only the dots are colored, and nothing is bold: the text is
//! `foreground`, or `muted` for ids, shas, output and what happened.
//!
//! Without a queue, or with nothing in it, a note says where merges come
//! from. The session sends the newest merges a page at a time: under the
//! last one shown, `N older merges` says how many more there are, and
//! scrolling to it asks for the next page.

use super::chrome::{pad_sides, Body};
use crate::app::{HoverTarget, PanelFocus};
use crate::components::{
    exact_time, parse_time, relative_time, relative_time_changes, truncate_line, Info, StatusDot,
    Value,
};
use crate::events::{MergeEntryView, MergeQueueView, MergeStateView};
use crate::{App, Theme};
use chrono::{DateTime, Local};
use ratatui::{layout::Rect, style::Style, text::Line, Frame};
use std::time::{Duration, Instant};

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

/// What the QUEUE document is drawn for: the time it is, and the branch
/// link and the time under the pointer, each by its place in the
/// document.
#[derive(Debug, Clone, Copy)]
pub(super) struct QueueLook {
    pub now: DateTime<Local>,
    pub hovered_link: Option<usize>,
    pub hovered_time: Option<usize>,
}

impl QueueLook {
    /// Drawn at `now` with nothing under the pointer.
    pub(super) const fn at(now: DateTime<Local>) -> Self {
        Self {
            now,
            hovered_link: None,
            hovered_time: None,
        }
    }
}

/// The QUEUE tab's document: its lines, the lines that open a branch's
/// page with the page, the lines with a time, and how long until the
/// soonest time reads differently.
pub(super) struct QueueDoc {
    pub lines: Vec<Line<'static>>,
    pub links: Vec<(usize, String)>,
    pub times: Vec<usize>,
    pub due: Option<Duration>,
}

/// What the entries add to the document as it is built.
struct Marks {
    look: QueueLook,
    links: Vec<(usize, String)>,
    times: Vec<usize>,
    due: Option<Duration>,
}

/// Draw the open channel's merge queue into `inner`, the body below the
/// tab bar of the panel bordered by `area`, and record where its branch
/// links and times went, and when a time next changes. Without an open
/// channel there is nothing to draw.
pub(super) fn render_channel_queue(
    frame: &mut Frame,
    area: Rect,
    inner: Rect,
    app: &mut App,
    theme: &Theme,
) {
    app.set_channel_queue_marks(Vec::new(), Vec::new(), None);
    if app.open_channel().is_none() {
        return;
    }
    let body = Body::new(area, inner, app.focus() == PanelFocus::Sandbox);
    let look = QueueLook {
        hovered_link: match app.hover() {
            Some(HoverTarget::ChannelQueueLink(at)) => Some(at),
            _ => None,
        },
        hovered_time: match app.hover() {
            Some(HoverTarget::ChannelQueueTime(at)) => Some(at),
            _ => None,
        },
        ..QueueLook::at(Local::now())
    };
    let doc = queue_doc(app.open_channel_queue(), body.text_width(), theme, look);
    let (total, visible) = (doc.lines.len(), body.visible());
    let first = app.sync_channel_queue_scroll(total, visible);
    let rows = pad_sides(body.rows());
    let links = doc
        .links
        .iter()
        .enumerate()
        .filter_map(|(at, (line, url))| Some((at, line_area(*line, rows, first)?, url.clone())))
        .collect();
    let times = doc
        .times
        .iter()
        .enumerate()
        .filter_map(|(at, line)| Some((at, line_area(*line, rows, first)?)))
        .collect();
    app.set_channel_queue_marks(links, times, doc.due.map(|due| Instant::now() + due));
    body.draw_document(frame, doc.lines, first, theme);
    if first + visible >= total {
        app.queue_end_reached();
    }
}

/// Where document line `line` is on screen, the document drawn in `rows`
/// from line `first`; `None` once it is scrolled out of them.
fn line_area(line: usize, rows: Rect, first: usize) -> Option<Rect> {
    let offset = u16::try_from(line.checked_sub(first)?).ok()?;
    (offset < rows.height).then(|| Rect {
        y: rows.y + offset,
        height: 1,
        ..rows
    })
}

/// The QUEUE tab's lines, `width` columns wide.
#[cfg(test)]
pub(super) fn queue_lines(
    queue: Option<&MergeQueueView>,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    queue_doc(queue, width, theme, QueueLook::at(Local::now())).lines
}

/// The QUEUE tab's document, `width` columns wide.
pub(super) fn queue_doc(
    queue: Option<&MergeQueueView>,
    width: usize,
    theme: &Theme,
    look: QueueLook,
) -> QueueDoc {
    let mut marks = Marks {
        look,
        links: Vec::new(),
        times: Vec::new(),
        due: None,
    };
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
            times: Vec::new(),
            due: None,
        };
    }
    for entry in entries {
        entry_lines(&mut info, queue, entry, theme, &mut marks);
    }
    if let Some(older) = queue.map(|queue| queue.older).filter(|&older| older > 0) {
        info.blank();
        info.note(&older_merges(older));
    }
    QueueDoc {
        lines: info.into_lines(),
        links: marks.links,
        times: marks.times,
        due: marks.due,
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
/// own, then its facts. The branch's line and page go in `marks` when the
/// branch opens one.
fn entry_lines(
    info: &mut Info<'_>,
    queue: Option<&MergeQueueView>,
    entry: &MergeEntryView,
    theme: &Theme,
    marks: &mut Marks,
) {
    let head = entry.head.trim();
    let note = (!head.is_empty()).then_some(head);
    info.marked_section(state_dot(entry.state), entry.state.label(), note);
    let branch = entry.branch.trim();
    match queue.and_then(|queue| queue.branch_url(branch)) {
        Some(url) => {
            let hovered = marks.look.hovered_link == Some(marks.links.len());
            marks.links.push((info.link(branch, hovered), url));
        }
        None if !branch.is_empty() => info.item(branch, None),
        None => {}
    }
    failure_first(info, entry, theme);
    entry_facts(info, entry, marks);
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
fn entry_facts(info: &mut Info<'_>, entry: &MergeEntryView, marks: &mut Marks) {
    if let Some(agent) = entry.agent.as_deref().filter(|agent| !agent.is_empty()) {
        info.fact("Agent", agent, Value::Plain);
    }
    time_fact(info, "Queued", &entry.queued_at, marks);
    if let Some(finished) = entry.finished_at.as_deref() {
        time_fact(info, "Finished", finished, marks);
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

/// A time fact: `Queued  5 h ago`, or the time in full while the pointer
/// is on it. Its line goes in `marks`, and when it next reads
/// differently. A time that does not parse is shown as sent, cut to its
/// minutes.
fn time_fact(info: &mut Info<'_>, label: &str, at: &str, marks: &mut Marks) {
    let index = marks.times.len();
    marks.times.push(info.line_count());
    let Some(time) = parse_time(at) else {
        let sent = at.trim().chars().take(16).collect::<String>();
        info.fact(label, &sent.replace('T', " "), Value::Plain);
        return;
    };
    let now = marks.look.now;
    if let Some(next) = relative_time_changes(&time, &now) {
        marks.due = Some(marks.due.map_or(next, |due| due.min(next)));
    }
    let text = if marks.look.hovered_time == Some(index) {
        exact_time(&time)
    } else {
        relative_time(&time, &now)
    };
    info.fact(label, &text, Value::Plain);
}
