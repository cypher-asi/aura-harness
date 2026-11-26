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

use super::chrome::{Body, ScrollView};
use super::header::tab_style;
use super::side_list::{name_style, render_empty, Tone, REPO_HINT, REPO_PR_HINT};
use crate::app::repo_tab::{RepoFilter, RepoRow};
use crate::app::{HoverTarget, PanelFocus};
use crate::components::{
    cell_style, format_ago, list_row_with_detail, now_ms, CellState, StatusIcon,
};
use crate::repo::{ChangeKind, EventKind, EventState, PrState, RepoChange, RepoView};
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::Line,
    Frame,
};
use std::ops::Range;

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
    render_strip(frame, strip, app, theme);

    let is_focused = app.focus() == PanelFocus::Sandbox;
    let list = Rect {
        y: inner.y + strip.height,
        height: inner.height - strip.height,
        ..inner
    };
    let body = Body::new(area, list, is_focused);
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
    let on_pr = match rows.get(selected) {
        Some(&RepoRow::Event(i)) => view.events.get(i).is_some_and(|e| e.url.is_some()),
        _ => false,
    };
    let hint = if on_pr { REPO_PR_HINT } else { REPO_HINT };
    let shown = ScrollView {
        total: rows.len(),
        visible,
        first: scroll,
    };
    app.set_repo_rows_area(Some(body.rows()), scroll);
    body.draw(frame, lines, shown, hint, theme);
}

/// A piece of the strip: a filter, or the bar that sets the lists apart
/// from the timeline's filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Piece {
    Filter(RepoFilter),
    Bar,
}

impl Piece {
    /// Columns it takes: a filter's label with a space on each side.
    fn width(self) -> u16 {
        match self {
            Self::Filter(filter) => u16::try_from(filter.label().len() + 2).unwrap_or(u16::MAX),
            Self::Bar => 1,
        }
    }
}

/// The strip's pieces for `filters`, in order.
fn pieces(filters: &[RepoFilter]) -> Vec<Piece> {
    let mut pieces = Vec::new();
    for &filter in filters {
        let first_list = filter.is_list() && !pieces.iter().any(|&p| is_list_piece(p));
        if first_list && !pieces.is_empty() {
            pieces.push(Piece::Bar);
        }
        pieces.push(Piece::Filter(filter));
    }
    pieces
}

const fn is_list_piece(piece: Piece) -> bool {
    matches!(piece, Piece::Filter(filter) if filter.is_list())
}

/// The pieces that fit in `room` columns with the one at `active` among
/// them. When they don't all fit, a column at each end is kept for `‹`
/// and `›`, and the pieces shown start as early as they can.
fn window(widths: &[u16], active: usize, room: u16) -> Range<usize> {
    let sum = |range: Range<usize>| -> u32 { widths[range].iter().map(|&w| u32::from(w)).sum() };
    if sum(0..widths.len()) <= u32::from(room) {
        return 0..widths.len();
    }
    let room = u32::from(room.saturating_sub(2));
    let active = active.min(widths.len().saturating_sub(1));
    let mut first = 0;
    while first < active && sum(first..active + 1) > room {
        first += 1;
    }
    let mut end = (active + 1).min(widths.len());
    while end < widths.len() && sum(first..end + 1) <= room {
        end += 1;
    }
    first..end
}

/// Draw the strip of filters into the one-row `strip`, the picked one lit
/// as a shown tab is, and record where each filter went.
fn render_strip(frame: &mut Frame, strip: Rect, app: &mut App, theme: &Theme) {
    if strip.height == 0 {
        app.set_repo_strip(Vec::new(), (None, None));
        return;
    }
    let filters = app.repo_filters();
    let active = app.repo_filter();
    let hovered = match app.hover() {
        Some(HoverTarget::RepoFilter(filter)) => Some(filter),
        _ => None,
    };
    let focused = app.focus() == PanelFocus::Sandbox;
    let pieces = pieces(&filters);
    let widths: Vec<u16> = pieces.iter().map(|piece| piece.width()).collect();
    let at = pieces
        .iter()
        .position(|&piece| piece == Piece::Filter(active))
        .unwrap_or(0);
    let shown = window(&widths, at, strip.width);
    let cut = shown.len() < pieces.len();
    // The last column left to the pieces, past which `›` goes.
    let end = strip.right().saturating_sub(u16::from(cut));
    let buf = frame.buffer_mut();
    let mut x = strip.x + u16::from(cut);
    let mut chips = Vec::new();
    for (&piece, &width) in pieces[shown.clone()].iter().zip(&widths[shown.clone()]) {
        let width = width.min(end.saturating_sub(x));
        if width == 0 {
            break;
        }
        let cell = Rect::new(x, strip.y, width, 1);
        match piece {
            Piece::Filter(filter) => {
                let style = tab_style(filter == active, hovered == Some(filter), focused, theme);
                buf.set_style(cell, style);
                let room = usize::from(width.saturating_sub(1));
                buf.set_stringn(x + 1, strip.y, filter.label(), room, style);
                chips.push((cell, filter));
            }
            Piece::Bar => {
                buf.set_string(x, strip.y, "│", Style::default().fg(theme.colors.faint));
            }
        }
        x += width;
    }
    let back = (cut && shown.start > 0).then(|| Rect::new(strip.x, strip.y, 1, 1));
    let on = (cut && shown.end < pieces.len()).then(|| Rect::new(end, strip.y, 1, 1));
    let muted = Style::default().fg(theme.colors.muted);
    for (step, glyph) in [(back, "‹"), (on, "›")] {
        if let Some(cell) = step {
            buf.set_string(cell.x, cell.y, glyph, muted);
        }
    }
    app.set_repo_strip(chips, (back, on));
}

/// What every row is drawn with.
#[derive(Debug, Clone, Copy)]
struct Look {
    width: usize,
    /// Now, in ms since the epoch, for each event's age
    now: u64,
    /// Whether the changes are folded under their header
    folded: bool,
    /// Whether the changes hang from a header, one level in
    nested: bool,
}

fn row_line(
    view: &RepoView,
    row: RepoRow,
    state: CellState,
    look: Look,
    theme: &Theme,
) -> Line<'static> {
    let colors = &theme.colors;
    let line =
        |depth, (glyph, color), (name, detail): (&str, Option<&str>), tone, end: Option<&str>| {
            let cell = cell_style(theme, state, name_style(tone, theme));
            let icon = StatusIcon::new(glyph, color);
            list_row_with_detail(look.width, depth, icon, (name, detail), cell, end)
        };
    let depth = usize::from(look.nested);
    let ago = |at: i64| {
        let then = u64::try_from(at).unwrap_or(0).saturating_mul(1000);
        (then > 0).then(|| format_ago(then, look.now))
    };
    match row {
        RepoRow::Summary => {
            let name = view.branch.as_deref().unwrap_or("HEAD");
            let detail = sync_text(view);
            let end = Some(view.head.as_str()).filter(|head| !head.is_empty());
            let label = (name, Some(detail.as_str()));
            line(0, sync_icon(view, theme), label, Tone::Active, end)
        }
        RepoRow::Changes => {
            let fold = if look.folded { "▸" } else { "▾" };
            let count = view.changes.len() + view.changes_more;
            let end = format!("{count} file{}", if count == 1 { "" } else { "s" });
            let name = ("Uncommitted changes", None);
            line(
                0,
                (fold, colors.muted),
                name,
                Tone::Active,
                Some(end.as_str()),
            )
        }
        RepoRow::Change(i) => view.changes.get(i).map_or_else(Line::default, |change| {
            let name = change.from.as_ref().map_or_else(
                || change.path.clone(),
                |from| format!("{from} → {}", change.path),
            );
            let end = match (change.staged, change.unstaged) {
                (true, true) => Some("partly staged"),
                (true, false) => Some("staged"),
                _ => None,
            };
            let label = (name.as_str(), None);
            line(depth, change_icon(change, theme), label, Tone::Active, end)
        }),
        RepoRow::MoreChanges => {
            let name = format!("{} more", view.changes_more);
            let label = (name.as_str(), None);
            line(depth, ("", colors.muted), label, Tone::Settled, None)
        }
        RepoRow::PrNote => match &view.prs {
            PrState::Note(note) => {
                let label = (note.as_str(), None);
                line(0, ("○", colors.muted), label, Tone::Settled, None)
            }
            _ => Line::default(),
        },
        RepoRow::Event(i) => view.events.get(i).map_or_else(Line::default, |event| {
            let finished = matches!(event.state, EventState::Merged | EventState::Closed);
            let icon = event_icon(event.kind, event.state, theme);
            let label = (event.title.as_str(), Some(event.detail.as_str()));
            line(
                0,
                icon,
                label,
                Tone::active_if(!finished),
                ago(event.at).as_deref(),
            )
        }),
        RepoRow::Branch(i) => view.branches.get(i).map_or_else(Line::default, |branch| {
            let icon = if branch.current {
                ("●", colors.success)
            } else if branch.gone {
                ("✕", colors.muted)
            } else {
                ("○", colors.muted)
            };
            let track = if branch.gone {
                "gone".to_string()
            } else {
                track_text(branch.ahead, branch.behind)
            };
            let end = [Some(track), ago(branch.at)]
                .into_iter()
                .flatten()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("  ");
            let label = (branch.name.as_str(), Some(branch.subject.as_str()));
            line(
                0,
                icon,
                label,
                Tone::active_if(!branch.gone),
                Some(end.as_str()),
            )
        }),
        RepoRow::Tag(i) => view.tags.get(i).map_or_else(Line::default, |tag| {
            let label = (tag.name.as_str(), Some(tag.subject.as_str()));
            line(
                0,
                ("#", colors.muted),
                label,
                Tone::Active,
                ago(tag.at).as_deref(),
            )
        }),
        RepoRow::Worktree(i) => view
            .worktrees
            .get(i)
            .map_or_else(Line::default, |worktree| {
                let icon = if worktree.current {
                    ("●", colors.success)
                } else if worktree.prunable {
                    ("✕", colors.muted)
                } else {
                    ("○", colors.muted)
                };
                // Worktrees sit side by side, so the folder they share is
                // left out.
                let path = view
                    .root
                    .parent()
                    .and_then(|parent| worktree.path.strip_prefix(parent).ok())
                    .unwrap_or(&worktree.path)
                    .display()
                    .to_string();
                let detail = worktree
                    .branch
                    .clone()
                    .unwrap_or_else(|| format!("detached at {}", worktree.head));
                let end = if worktree.locked {
                    Some("locked")
                } else if worktree.prunable {
                    Some("prunable")
                } else {
                    None
                };
                let tone = Tone::active_if(!worktree.prunable);
                line(0, icon, (path.as_str(), Some(detail.as_str())), tone, end)
            }),
    }
}

/// `↑2 ↓1`, `↑2`, `↓1`, or nothing when in sync.
fn track_text(ahead: usize, behind: usize) -> String {
    let parts = [(ahead, "↑"), (behind, "↓")]
        .into_iter()
        .filter(|&(n, _)| n > 0)
        .map(|(n, arrow)| format!("{arrow}{n}"));
    parts.collect::<Vec<_>>().join(" ")
}

/// How the branch stands with its upstream.
fn sync_text(view: &RepoView) -> String {
    let text = match (&view.branch, &view.upstream) {
        (None, _) => "detached".to_string(),
        (Some(_), None) => "no upstream".to_string(),
        (Some(_), Some(upstream)) => match (view.ahead, view.behind) {
            (0, 0) => format!("in sync with {upstream}"),
            (ahead, 0) => format!("{ahead} ahead of {upstream}"),
            (0, behind) => format!("{behind} behind {upstream}"),
            (ahead, behind) => format!("{ahead} ahead, {behind} behind {upstream}"),
        },
    };
    if view.worktree {
        format!("{text} · worktree")
    } else {
        text
    }
}

/// The summary's icon: which way the branch and its upstream are apart,
/// a check when they aren't.
const fn sync_icon(view: &RepoView, theme: &Theme) -> (&'static str, Color) {
    let colors = &theme.colors;
    if view.branch.is_none() {
        return ("◌", colors.muted);
    }
    if view.upstream.is_none() {
        return ("○", colors.muted);
    }
    match (view.ahead > 0, view.behind > 0) {
        (true, true) => ("↕", colors.warning),
        (true, false) => ("↑", colors.warning),
        (false, true) => ("↓", colors.warning),
        (false, false) => ("✓", colors.success),
    }
}

/// A change's status letter: in the accent once wholly staged, in
/// `warning` while some of it isn't, `error` on a conflict.
const fn change_icon(change: &RepoChange, theme: &Theme) -> (&'static str, Color) {
    let colors = &theme.colors;
    let color = match change.kind {
        ChangeKind::Conflicted => colors.error,
        ChangeKind::Untracked => colors.muted,
        _ if change.staged && !change.unstaged => colors.success,
        _ => colors.warning,
    };
    (change.kind.letter(), color)
}

/// An event's icon: unpushed commits are hollow, pushes and pulls point
/// the way the commits went, and a PR shows whether it is open, merged or
/// closed.
const fn event_icon(kind: EventKind, state: EventState, theme: &Theme) -> (&'static str, Color) {
    let colors = &theme.colors;
    match (kind, state) {
        (EventKind::Commit, EventState::Pending) => ("○", colors.warning),
        (EventKind::Commit, _) => ("●", colors.muted),
        (EventKind::Merge, _) => ("◆", colors.muted),
        (EventKind::Rebase, _) => ("◇", colors.muted),
        (EventKind::Push, _) => ("↑", colors.success),
        (EventKind::Pull, _) => ("↓", colors.success),
        (EventKind::Fetch, _) => ("↓", colors.muted),
        (EventKind::Reset, _) => ("↩", colors.warning),
        (EventKind::Checkout, _) => ("→", colors.muted),
        (EventKind::Stash, _) => ("▪", colors.muted),
        (EventKind::Pr, EventState::Merged) => ("✓", colors.success),
        (EventKind::Pr, EventState::Closed) => ("✕", colors.muted),
        (EventKind::Pr, EventState::Draft) => ("↪", colors.muted),
        (EventKind::Pr, _) => ("↪", colors.success),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{PaneTab, SidePanelTab};
    use crate::events::UiCommand;
    use crate::repo::{RepoBranch, RepoEvent, RepoTarget};
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{backend::TestBackend, Terminal};

    fn screen(app: &mut App, width: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, 30)).unwrap();
        terminal
            .draw(|frame| crate::renderer::render(frame, app, &Theme::cyber()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect()
            })
            .collect()
    }

    fn row_with(screen: &[String], text: &str) -> usize {
        screen
            .iter()
            .position(|row| row.contains(text))
            .unwrap_or_else(|| panic!("no {text} in:\n{}", screen.join("\n")))
    }

    /// The column `text` starts at in `row`, whose cells are one char each.
    fn column_of(row: &str, text: &str) -> u16 {
        let at = row
            .find(text)
            .unwrap_or_else(|| panic!("no {text} in {row}"));
        u16::try_from(row[..at].chars().count()).unwrap()
    }

    fn click(app: &mut App, column: u16, row: usize) {
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.handle_mouse(MouseEvent {
                kind,
                column,
                row: u16::try_from(row).unwrap(),
                modifiers: KeyModifiers::empty(),
            });
        }
    }

    fn repo() -> RepoView {
        let event = |kind, state, at, title: &str, target| RepoEvent {
            kind,
            state,
            at,
            title: title.into(),
            detail: String::new(),
            target,
            url: None,
        };
        let commit = |sha: &str| RepoTarget::Commit { sha: sha.into() };
        let pushed = RepoTarget::Range {
            from: None,
            to: "aaaa1111".into(),
        };
        RepoView {
            agent_id: "aa".into(),
            branch: Some("main".into()),
            head: "bbbb2222".into(),
            upstream: Some("origin/main".into()),
            ahead: 1,
            changes: vec![RepoChange {
                path: "src/lib.rs".into(),
                from: None,
                kind: ChangeKind::Modified,
                staged: true,
                unstaged: false,
            }],
            events: vec![
                event(
                    EventKind::Commit,
                    EventState::Pending,
                    50,
                    "Add the REPO tab",
                    commit("b"),
                ),
                event(
                    EventKind::Push,
                    EventState::Done,
                    40,
                    "Pushed 2 commits",
                    pushed,
                ),
                event(
                    EventKind::Commit,
                    EventState::Done,
                    30,
                    "Start",
                    commit("a"),
                ),
            ],
            branches: vec![RepoBranch {
                name: "main".into(),
                current: true,
                upstream: Some("origin/main".into()),
                ahead: 1,
                behind: 0,
                gone: false,
                at: 50,
                subject: "Add the REPO tab".into(),
            }],
            ..RepoView::default()
        }
    }

    fn repo_app() -> App {
        let mut app = App::new();
        app.process_command(UiCommand::SetActiveAgent("aa".into()));
        app.process_command(UiCommand::SetRepo(Box::new(repo())));
        app.select_pane_tab(PaneTab::Side(SidePanelTab::Repo));
        app
    }

    #[test]
    fn the_tab_lists_the_sync_the_changes_and_the_timeline_under_its_strip() {
        let mut app = repo_app();
        let rows = screen(&mut app, 220);
        let strip = row_with(&rows, " All  Changes  Commits  Pushes │ Branches ");
        for (offset, parts) in [
            (1, &["↑", "main", "1 ahead of origin/main", "bbbb2222"][..]),
            (2, &["▾", "Uncommitted changes", "1 file"]),
            (3, &["M  src/lib.rs", "staged"]),
            (4, &["○", "Add the REPO tab"]),
            (5, &["↑", "Pushed 2 commits"]),
            (6, &["●", "Start"]),
        ] {
            let row = &rows[strip + offset];
            assert!(
                parts.iter().all(|part| row.contains(part)),
                "{parts:?} in {row}"
            );
        }

        let changes = column_of(&rows[strip + 2], "▾");
        click(&mut app, changes, strip + 2);
        let rows = screen(&mut app, 220);
        assert!(!rows.join("\n").contains("src/lib.rs"), "folded");
        assert!(rows[strip + 2].contains('▸'), "{}", rows[strip + 2]);

        click(&mut app, column_of(&rows[strip], "Commits"), strip);
        assert_eq!(app.repo_filter(), RepoFilter::Commits);
        let rows = screen(&mut app, 220);
        assert!(
            rows[strip + 2].contains("Add the REPO tab"),
            "{}",
            rows[strip + 2]
        );
        assert!(rows[strip + 3].contains("Start"), "{}", rows[strip + 3]);
        assert!(!rows.join("\n").contains("Pushed"));
    }

    #[test]
    fn a_narrow_strip_is_cut_around_the_picked_filter() {
        let mut app = repo_app();
        let rows = screen(&mut app, 150);
        let strip = row_with(&rows, " All  Changes ");
        assert!(rows[strip].contains('›'), "{}", rows[strip]);
        assert!(!rows[strip].contains("Branches"), "{}", rows[strip]);

        for _ in 0..4 {
            let _ = app.handle_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
        }
        assert_eq!(app.repo_filter(), RepoFilter::Branches);
        let rows = screen(&mut app, 150);
        assert!(
            rows[strip].contains("‹") && rows[strip].contains("Branches"),
            "{}",
            rows[strip]
        );
        assert!(
            rows[strip + 1].contains("main"),
            "the branches alone: {}",
            rows[strip + 1]
        );
        click(&mut app, column_of(&rows[strip], "‹"), strip);
        assert_eq!(app.repo_filter(), RepoFilter::Pushes);
    }

    const LISTED: [RepoFilter; 6] = [
        RepoFilter::All,
        RepoFilter::Changes,
        RepoFilter::Commits,
        RepoFilter::Pushes,
        RepoFilter::Branches,
        RepoFilter::Tags,
    ];

    #[test]
    fn the_lists_sit_past_a_bar() {
        let pieces = pieces(&LISTED);
        assert_eq!(pieces[4], Piece::Bar);
        assert_eq!(pieces[5], Piece::Filter(RepoFilter::Branches));
        assert_eq!(pieces.len(), 7);
        assert!(!super::pieces(&LISTED[..4]).contains(&Piece::Bar));
        assert_eq!(
            super::pieces(&[RepoFilter::Branches]),
            [Piece::Filter(RepoFilter::Branches)],
            "no bar before the first piece"
        );
    }

    #[test]
    fn a_strip_too_wide_keeps_the_picked_filter_in_view() {
        // " All ", " Changes ", " Commits ", " Pushes ", "│", " Branches ",
        // " Tags ": 48 columns in all.
        let widths: Vec<u16> = pieces(&LISTED).iter().map(|p| p.width()).collect();
        assert_eq!(widths.iter().sum::<u16>(), 48);
        assert_eq!(window(&widths, 0, 48), 0..7);
        assert_eq!(window(&widths, 0, 30), 0..3, "28 columns left");
        assert_eq!(window(&widths, 6, 30), 3..7);
        assert_eq!(window(&widths, 3, 20), 2..5, "from Commits to the bar");
        assert_eq!(window(&widths, 1, 5), 1..2, "one too wide still shows");
    }
}
