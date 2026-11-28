//! The rows of the to-do drawer: its count, the items still open with the
//! agent on each, the "Finished to-dos" row and, opened, the closed items.
//!
//! An item takes two rows, as a member does on the side panel's MEMBERS:
//! its marker and its text, then, under the text and in gray, the agent
//! it was given to and how that agent is doing, else the item's state.
//!
//! ```text
//! ⬢ 2 active · 5 done
//! ◐ Fix the redirect
//!   FEN6 · working 4m
//! ☐ Run the tests
//!   Pending
//! ▸ Finished to-dos  5
//! ```
//!
//! Nothing is bold. As on the side panel, only the marker has the item's
//! state color: the text is `foreground` while the item is open and
//! `muted` once it is closed. Each row is cut to the drawer on its own.

use super::super::hud::truncate;
use super::super::plans::{marker, marker_color};
use super::super::side_list::{name_style, Tone};
use super::super::text::display_width;
use crate::app::TodoAgent;
use crate::events::{TodoState, TodoView};
use crate::{App, Theme};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use std::time::Duration;

/// Rows an item takes: its text, then its agent and status.
const ITEM_ROWS: usize = 2;
/// What the history row is called.
pub(super) const HISTORY: &str = "Finished to-dos";

/// One entry of the drawer under its header: an item, on its two rows,
/// or the history row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Entry<'a> {
    /// An item still open
    Open(&'a TodoView),
    /// The history row, and whether it is open
    History(bool),
    /// A closed item, under the opened history row
    Closed(&'a TodoView),
}

impl Entry<'_> {
    /// Rows the entry takes on screen.
    pub(super) const fn rows(self) -> usize {
        match self {
            Self::Open(_) | Self::Closed(_) => ITEM_ROWS,
            Self::History(_) => 1,
        }
    }
}

/// Every entry the drawer has for `app`'s list, before it is fitted to
/// the screen: the items still open, then the history row when an item is
/// closed, then the closed items when the history is open (always, for a
/// list with nothing left open).
pub(super) fn entries(app: &App) -> Vec<Entry<'_>> {
    let todos = &app.hud().todos;
    let mut rows: Vec<Entry<'_>> = todos
        .iter()
        .filter(|todo| !todo.state.is_closed())
        .map(Entry::Open)
        .collect();
    let closed: Vec<&TodoView> = todos.iter().filter(|t| t.state.is_closed()).collect();
    // A finished list `/todos` brought back has only its history to show.
    let open = app.todo_history_open() || rows.is_empty();
    if !closed.is_empty() {
        rows.push(Entry::History(open));
        if open {
            rows.extend(closed.into_iter().map(Entry::Closed));
        }
    }
    rows
}

/// Rows the entries `shown` of `entries` take.
pub(super) fn rows_of(entries: &[Entry<'_>], shown: &[usize]) -> usize {
    shown.iter().map(|&at| entries[at].rows()).sum()
}

/// Which of `entries` fit in `room` rows, and whether a `+N more` row
/// takes the last one. The entries start at the item in progress, so it
/// stays in view, and keep the history row. The item in progress shows
/// even in fewer rows than it takes, cut short.
pub(super) fn fit(entries: &[Entry<'_>], room: usize) -> (Vec<usize>, bool) {
    let all: Vec<usize> = (0..entries.len()).collect();
    if rows_of(entries, &all) <= room {
        return (all, false);
    }
    let room = room.saturating_sub(1);
    let current = entries
        .iter()
        .position(|e| matches!(e, Entry::Open(t) if t.state == TodoState::InProgress))
        .unwrap_or(0);
    let (first, end) = window(entries, current, room);
    if first == end {
        return (vec![current], true);
    }
    let mut shown: Vec<usize> = (first..end).collect();
    let history = entries.iter().position(|e| matches!(e, Entry::History(_)));
    if let Some(history) = history.filter(|&at| at >= end) {
        // The last items give their rows to the history row.
        while shown.len() > 1 && rows_of(entries, &shown) >= room {
            shown.pop();
        }
        if rows_of(entries, &shown) < room {
            shown.push(history);
        }
    }
    (shown, true)
}

/// The entries that fill `room` rows from `current` on, then before it
/// once the list ends, as the range `first..end`; empty when `current`
/// alone takes more.
fn window(entries: &[Entry<'_>], current: usize, room: usize) -> (usize, usize) {
    let (mut first, mut end, mut used) = (current, current, 0);
    while end < entries.len() && used + entries[end].rows() <= room {
        used += entries[end].rows();
        end += 1;
    }
    while end > current && first > 0 && used + entries[first - 1].rows() <= room {
        first -= 1;
        used += entries[first].rows();
    }
    (first, end)
}

/// `2 active · 5 done`, with `Working on` before it during a turn; `dropped`
/// counts cancelled items when there are any.
pub(super) fn count_spans(
    app: &App,
    theme: &Theme,
    working: bool,
    frame: Option<usize>,
) -> Vec<Span<'static>> {
    let todos = &app.hud().todos;
    let active = todos.iter().filter(|t| !t.state.is_closed()).count();
    let done = todos
        .iter()
        .filter(|t| t.state == TodoState::Completed)
        .count();
    let dropped = todos
        .iter()
        .filter(|t| t.state == TodoState::Cancelled)
        .count();
    let lit = Style::default().fg(theme.colors.foreground);
    let mut spans = vec![Span::styled("⬢ ", Style::default().fg(theme.colors.faint))];
    let count = format!("{active} active");
    if working {
        match frame {
            Some(frame) => spans.extend(crate::animation::shimmer_spans(
                "Working",
                frame,
                Style::default(),
                (theme.colors.muted, theme.colors.foreground),
            )),
            None => spans.push(Span::styled("Working", lit)),
        }
        spans.push(Span::styled(
            format!(" on {count}"),
            Style::default().fg(theme.colors.secondary),
        ));
    } else {
        spans.push(Span::styled(count, lit));
    }
    let muted = Style::default().fg(theme.colors.muted);
    if done > 0 {
        spans.push(Span::styled(format!(" · {done} done"), muted));
    }
    if dropped > 0 {
        spans.push(Span::styled(format!(" · {dropped} dropped"), muted));
    }
    spans
}

/// The rows of `entry`, each at most `room` columns wide.
pub(super) fn entry_lines(
    app: &App,
    entry: Entry<'_>,
    room: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    match entry {
        Entry::Open(todo) | Entry::Closed(todo) => {
            item_lines(todo, app.todo_agent(todo).as_ref(), room, theme).into()
        }
        Entry::History(open) => vec![history_line(app, open, theme)],
    }
}

/// An item's two rows: `◐ Fix the redirect`, then under its text and in
/// `muted`, [`detail`]: `FEN6 · working 4m`.
pub(super) fn item_lines(
    todo: &TodoView,
    agent: Option<&TodoAgent>,
    room: usize,
    theme: &Theme,
) -> [Line<'static>; ITEM_ROWS] {
    let mark = marker(todo.state);
    let indent = display_width(mark) + 1;
    let text_room = room.saturating_sub(indent).max(1);
    let text = Line::from(vec![
        Span::styled(
            format!("{mark} "),
            Style::default().fg(marker_color(todo.state, theme)),
        ),
        Span::styled(
            truncate(&todo.content.replace('\n', " "), text_room),
            name_style(Tone::active_if(!todo.state.is_closed()), theme),
        ),
    ]);
    let detail = Line::from(vec![
        Span::raw(" ".repeat(indent)),
        Span::styled(
            truncate(&detail(todo, agent), text_room),
            Style::default().fg(theme.colors.muted),
        ),
    ]);
    [text, detail]
}

/// What an item's second row says: the agent it was given to and how that
/// agent is doing, `FEN6 · working 4m`, or how the item ended,
/// `FEN6 · done`; for an item given to no agent, its state alone.
fn detail(todo: &TodoView, agent: Option<&TodoAgent>) -> String {
    let Some(agent) = agent else {
        return state_name(todo.state).to_string();
    };
    let status = match todo.state {
        TodoState::Completed => "done".to_string(),
        TodoState::Cancelled => "cancelled".to_string(),
        state => agent_status(agent, state),
    };
    if status.is_empty() {
        agent.label.clone()
    } else {
        format!("{} · {status}", agent.label)
    }
}

/// An item's state, said on its own row.
const fn state_name(state: TodoState) -> &'static str {
    match state {
        TodoState::Pending => "Pending",
        TodoState::InProgress => "In progress",
        TodoState::Completed => "Done",
        TodoState::Cancelled => "Cancelled",
    }
}

/// What an agent's row says after its code: `working 4m`, `idle`, or
/// only the time for an agent not listed here.
fn agent_status(agent: &TodoAgent, state: TodoState) -> String {
    let time = agent
        .running_for
        .filter(|_| state == TodoState::InProgress)
        .map(short_duration);
    let doing = agent
        .working
        .map(|working| if working { "working" } else { "idle" });
    match (doing, time) {
        (Some(doing), Some(time)) => format!("{doing} {time}"),
        (Some(doing), None) => doing.to_string(),
        (None, Some(time)) => time,
        (None, None) => String::new(),
    }
}

/// `12s`, `4m` or `1h 5m`.
pub(super) fn short_duration(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3_599 => format!("{}m", secs / 60),
        _ => format!("{}h {}m", secs / 3_600, secs % 3_600 / 60),
    }
}

/// `▸ Finished to-dos  5` folded, `▾ Finished to-dos` `open`, in `muted`.
fn history_line(app: &App, open: bool, theme: &Theme) -> Line<'static> {
    let closed = app
        .hud()
        .todos
        .iter()
        .filter(|t| t.state.is_closed())
        .count();
    let muted = Style::default().fg(theme.colors.muted);
    let (arrow, count) = if open {
        ("▾", String::new())
    } else {
        ("▸", format!("  {closed}"))
    };
    Line::from(vec![
        Span::styled(format!("{arrow} "), muted),
        Span::styled(HISTORY, muted),
        Span::styled(count, Style::default().fg(theme.colors.faint)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    fn todo(state: TodoState) -> TodoView {
        TodoView {
            content: "Fix the redirect".to_string(),
            state,
            assignee: Some("FEN6".to_string()),
            task: None,
        }
    }

    fn agent(working: Option<bool>, secs: Option<u64>) -> TodoAgent {
        TodoAgent {
            label: "FEN6".to_string(),
            working,
            running_for: secs.map(Duration::from_secs),
        }
    }

    fn texts(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| line.spans.iter().map(|s| &*s.content).collect())
            .collect()
    }

    #[test]
    fn an_item_takes_two_rows_its_text_then_its_agent_and_status() {
        let theme = Theme::default();
        let busy = agent(Some(true), Some(250));
        let lines = item_lines(&todo(TodoState::InProgress), Some(&busy), 60, &theme);
        assert_eq!(texts(&lines), ["◐ Fix the redirect", "  FEN6 · working 4m"]);
        let idle = agent(Some(false), None);
        let lines = item_lines(&todo(TodoState::Pending), Some(&idle), 60, &theme);
        assert_eq!(texts(&lines), ["☐ Fix the redirect", "  FEN6 · idle"]);
        let unlisted = agent(None, None);
        let lines = item_lines(&todo(TodoState::Pending), Some(&unlisted), 60, &theme);
        assert_eq!(texts(&lines)[1], "  FEN6");
    }

    #[test]
    fn an_item_given_to_no_agent_says_its_state() {
        let theme = Theme::default();
        let doing = item_lines(&todo(TodoState::InProgress), None, 60, &theme);
        assert_eq!(texts(&doing)[1], "  In progress");
        let waiting = item_lines(&todo(TodoState::Pending), None, 60, &theme);
        assert_eq!(texts(&waiting)[1], "  Pending");
    }

    #[test]
    fn nothing_is_bold_and_only_the_marker_has_the_state_color() {
        let theme = Theme::default();
        let colors = &theme.colors;
        let busy = agent(Some(true), Some(5));
        for state in [TodoState::Pending, TodoState::InProgress] {
            let [text, detail] = item_lines(&todo(state), Some(&busy), 60, &theme);
            let mut spans = text.spans.iter().chain(&detail.spans);
            assert!(spans.all(|s| !s.style.add_modifier.contains(Modifier::BOLD)));
            assert_eq!(text.spans[0].style.fg, Some(marker_color(state, &theme)));
            assert_eq!(text.spans[1].style.fg, Some(colors.foreground), "{state:?}");
            assert_eq!(detail.spans[1].style.fg, Some(colors.muted));
        }
        let [dropped, _] = item_lines(&todo(TodoState::Cancelled), None, 60, &theme);
        assert_eq!(dropped.spans[1].style, name_style(Tone::Settled, &theme));
    }

    #[test]
    fn the_count_is_not_bold_either() {
        let theme = Theme::default();
        let mut app = App::new();
        app.process_command(crate::events::UiCommand::SetTodos(vec![todo(
            TodoState::InProgress,
        )]));
        for (working, frame) in [(false, None), (true, None), (true, Some(3))] {
            let spans = count_spans(&app, &theme, working, frame);
            assert!(
                spans
                    .iter()
                    .all(|s| !s.style.add_modifier.contains(Modifier::BOLD)),
                "{spans:?}"
            );
        }
    }

    #[test]
    fn a_narrow_drawer_cuts_each_row_on_its_own() {
        let theme = Theme::default();
        let busy = agent(Some(true), Some(250));
        let lines = item_lines(&todo(TodoState::InProgress), Some(&busy), 12, &theme);
        assert_eq!(texts(&lines), ["◐ Fix the r…", "  FEN6 · wo…"]);
    }

    #[test]
    fn a_closed_item_says_how_it_ended() {
        let theme = Theme::default();
        let by = agent(Some(false), None);
        let done = item_lines(&todo(TodoState::Completed), Some(&by), 60, &theme);
        assert_eq!(texts(&done), ["✓ Fix the redirect", "  FEN6 · done"]);
        let dropped = item_lines(&todo(TodoState::Cancelled), None, 60, &theme);
        assert_eq!(texts(&dropped), ["⊘ Fix the redirect", "  Cancelled"]);
    }

    #[test]
    fn durations_read_short() {
        assert_eq!(short_duration(Duration::from_secs(12)), "12s");
        assert_eq!(short_duration(Duration::from_secs(250)), "4m");
        assert_eq!(short_duration(Duration::from_secs(3_900)), "1h 5m");
    }

    #[test]
    fn fitting_keeps_the_item_in_progress_and_the_history_row() {
        let pending = todo(TodoState::Pending);
        let current = todo(TodoState::InProgress);
        let mut entries: Vec<Entry<'_>> = vec![Entry::Open(&pending); 6];
        entries[3] = Entry::Open(&current);
        entries.push(Entry::History(false));
        // Six items of two rows, and the history row.
        assert_eq!(fit(&entries, 13), ((0..7).collect(), false));
        assert_eq!(
            fit(&entries, 6),
            (vec![3, 4, 6], true),
            "from the current item, with the history row"
        );
        assert_eq!(
            fit(&entries, 5),
            (vec![3, 6], true),
            "the history row takes the place of an item"
        );
        assert_eq!(
            fit(&entries, 2),
            (vec![3], true),
            "the current item, cut short"
        );

        let mut late: Vec<Entry<'_>> = vec![Entry::Open(&pending); 6];
        late[5] = Entry::Open(&current);
        late.push(Entry::History(false));
        assert_eq!(
            fit(&late, 6),
            (vec![4, 5, 6], true),
            "rows left at the end go to the items before"
        );
    }
}
