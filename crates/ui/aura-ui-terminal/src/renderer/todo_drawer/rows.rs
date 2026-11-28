//! The rows of the to-do drawer: its count, the items still open with the
//! agent on each, the "Finished to-dos" row and, opened, the closed items.
//!
//! ```text
//! ⬢ 2 active · 5 done
//! ◐ Fix the redirect             → FEN6 · working 4m
//! ☐ Run the tests                → REV6 · idle
//! ▸ Finished to-dos  5
//! ```
//!
//! An item's agent is never left out for lack of room: its text is cut
//! first, then the agent's status, down to `→ CODE`.

use super::super::hud::truncate;
use super::super::plans::{marker, marker_color, todo_style};
use super::super::text::display_width;
use crate::app::TodoAgent;
use crate::events::{TodoState, TodoView};
use crate::{App, Theme};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use std::time::Duration;

/// Narrowest an item's text is cut to before its agent's status goes.
const MIN_TEXT: usize = 8;
/// What the history row is called.
pub(super) const HISTORY: &str = "Finished to-dos";

/// One row of the drawer under its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Entry<'a> {
    /// An item still open
    Open(&'a TodoView),
    /// The history row, and whether it is open
    History(bool),
    /// A closed item, under the opened history row
    Closed(&'a TodoView),
}

/// Every row the drawer has for `app`'s list, before it is fitted to the
/// screen: the items still open, then the history row when an item is
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

/// Which of `entries` fit in `room` rows, as a range, and whether a
/// `+N more` row takes the last one. The rows start at the item in
/// progress, so it stays in view, and the history row is kept.
pub(super) fn fit(entries: &[Entry<'_>], room: usize) -> (Vec<usize>, bool) {
    if entries.len() <= room {
        return ((0..entries.len()).collect(), false);
    }
    let room = room.saturating_sub(1).max(1);
    let history = entries.iter().position(|e| matches!(e, Entry::History(_)));
    let current = entries
        .iter()
        .position(|e| matches!(e, Entry::Open(t) if t.state == TodoState::InProgress))
        .unwrap_or(0);
    let first = current.min(entries.len() - room);
    let mut shown: Vec<usize> = (first..first + room).collect();
    if let Some(history) = history.filter(|at| !shown.contains(at)) {
        if history > first && room > 1 {
            shown.pop();
            shown.push(history);
        }
    }
    (shown, true)
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
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::styled("⬢ ", Style::default().fg(theme.colors.faint))];
    let count = format!("{active} active");
    if working {
        match frame {
            Some(frame) => spans.extend(crate::animation::shimmer_spans(
                "Working",
                frame,
                bold,
                (theme.colors.muted, theme.colors.foreground),
            )),
            None => spans.push(Span::styled("Working", bold.fg(theme.colors.foreground))),
        }
        spans.push(Span::styled(
            format!(" on {count}"),
            Style::default().fg(theme.colors.secondary),
        ));
    } else {
        spans.push(Span::styled(count, bold.fg(theme.colors.foreground)));
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

/// The line of `entry`, `room` columns wide.
pub(super) fn entry_line(app: &App, entry: Entry<'_>, room: usize, theme: &Theme) -> Line<'static> {
    match entry {
        Entry::Open(todo) => open_line(todo, app.todo_agent(todo).as_ref(), room, theme),
        Entry::Closed(todo) => closed_line(todo, app.todo_agent(todo).as_ref(), room, theme),
        Entry::History(open) => history_line(app, open, theme),
    }
}

/// `◐ Fix the redirect  → FEN6 · working 4m`: an item still open, with
/// the agent it was given to.
pub(super) fn open_line(
    todo: &TodoView,
    agent: Option<&TodoAgent>,
    room: usize,
    theme: &Theme,
) -> Line<'static> {
    let status = agent
        .map(|agent| agent_status(agent, todo.state))
        .unwrap_or_default();
    let suffix = agent
        .map(|agent| suffix(&agent.label, &status, room))
        .unwrap_or_default();
    item_line(todo, suffix, room, theme)
}

/// `✓ Read the parser  → FEN6 · done`: a closed item, with its outcome.
pub(super) fn closed_line(
    todo: &TodoView,
    agent: Option<&TodoAgent>,
    room: usize,
    theme: &Theme,
) -> Line<'static> {
    let outcome = if todo.state == TodoState::Cancelled {
        "cancelled"
    } else {
        "done"
    };
    let suffix = match agent {
        Some(agent) => suffix(&agent.label, outcome, room),
        None => format!("  {outcome}"),
    };
    item_line(todo, suffix, room, theme)
}

/// A marker, the item's text cut to leave room for `suffix`, then
/// `suffix` in `faint`.
fn item_line(todo: &TodoView, suffix: String, room: usize, theme: &Theme) -> Line<'static> {
    let mark = marker(todo.state);
    let text_room = room.saturating_sub(display_width(mark) + 1 + display_width(&suffix));
    Line::from(vec![
        Span::styled(
            format!("{mark} "),
            Style::default().fg(marker_color(todo.state, theme)),
        ),
        Span::styled(
            truncate(&todo.content.replace('\n', " "), text_room.max(1)),
            todo_style(todo.state, theme),
        ),
        Span::styled(suffix, Style::default().fg(theme.colors.faint)),
    ])
}

/// `  → CODE · status` when the item's text keeps [`MIN_TEXT`] columns
/// of `room` beside it, else `  → CODE`.
fn suffix(label: &str, status: &str, room: usize) -> String {
    let short = format!("  → {label}");
    if status.is_empty() {
        return short;
    }
    let full = format!("{short} · {status}");
    if display_width(&full) + MIN_TEXT + 2 <= room {
        full
    } else {
        short
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

    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| &*s.content).collect()
    }

    #[test]
    fn an_item_in_progress_names_its_agent_its_status_and_time() {
        let theme = Theme::default();
        let busy = agent(Some(true), Some(250));
        let line = open_line(&todo(TodoState::InProgress), Some(&busy), 60, &theme);
        assert_eq!(text(&line), "◐ Fix the redirect  → FEN6 · working 4m");
        let idle = agent(Some(false), None);
        let line = open_line(&todo(TodoState::Pending), Some(&idle), 60, &theme);
        assert_eq!(text(&line), "☐ Fix the redirect  → FEN6 · idle");
    }

    #[test]
    fn a_narrow_row_keeps_the_agent_and_cuts_the_text() {
        let theme = Theme::default();
        let busy = agent(Some(true), Some(5));
        let line = open_line(&todo(TodoState::InProgress), Some(&busy), 18, &theme);
        let row = text(&line);
        assert!(row.ends_with("→ FEN6"), "{row}");
        assert!(row.contains('…'), "{row}");
    }

    #[test]
    fn a_closed_item_says_how_it_ended() {
        let theme = Theme::default();
        let by = agent(Some(false), None);
        let done = closed_line(&todo(TodoState::Completed), Some(&by), 60, &theme);
        assert_eq!(text(&done), "✓ Fix the redirect  → FEN6 · done");
        let dropped = closed_line(&todo(TodoState::Cancelled), None, 60, &theme);
        assert_eq!(text(&dropped), "⊘ Fix the redirect  cancelled");
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
        assert_eq!(fit(&entries, 10), ((0..7).collect(), false));
        let (shown, more) = fit(&entries, 4);
        assert!(more);
        assert_eq!(
            shown,
            [3, 4, 6],
            "from the current item, with the history row"
        );
    }
}
