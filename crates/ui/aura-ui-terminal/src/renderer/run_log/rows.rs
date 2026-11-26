//! The LOG tab's panels: one header row per run, turn and step, with
//! open panels' bodies under them.

use super::super::benchmarks::fit;
use super::super::text::display_width;
use super::body::body_lines;
use super::{Document, ALL_COLUMNS, COLUMN_WIDTHS, GUTTER, KIND_WIDTH};
use crate::app::run_log::{LogPanel, LogRow, LogState};
use crate::events::LogStatus;
use crate::Theme;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

/// The mark in a row's gutter: blank for what simply went well, else
/// what went wrong, or a spinner for what still runs.
fn status_mark(status: LogStatus, spinner: &'static str, theme: &Theme) -> Span<'static> {
    let (mark, color) = match status {
        LogStatus::Ok => ("  ".to_string(), theme.colors.muted),
        LogStatus::Warn => ("! ".to_string(), theme.colors.warning),
        LogStatus::Error => ("✗ ".to_string(), theme.colors.error),
        LogStatus::Running => (format!("{spinner} "), theme.colors.pending),
    };
    Span::styled(mark, Style::default().fg(color))
}

/// How a row's summary is drawn: in the colour of what went wrong.
fn summary_style(status: LogStatus, theme: &Theme) -> Style {
    Style::default().fg(match status {
        LogStatus::Error => theme.colors.error,
        LogStatus::Warn => theme.colors.warning,
        LogStatus::Ok | LogStatus::Running => theme.colors.muted,
    })
}

/// `tokens`, `cost` and `took` right-aligned in their columns, as many as
/// a row `width` wide has room for.
pub(super) fn columns(values: [&str; 3], width: usize) -> String {
    let shown: &[usize] = if width >= ALL_COLUMNS {
        &[0, 1, 2]
    } else if width >= ALL_COLUMNS / 2 {
        &[2]
    } else {
        &[]
    };
    shown
        .iter()
        .map(|&i| format!("{:>w$}", values[i], w = COLUMN_WIDTHS[i]))
        .collect::<Vec<_>>()
        .join("  ")
}

/// `label` and `summary` cut to fit `room` columns together, the
/// summary kept whole while the label can give up room; the summary
/// comes back with the gap before it.
fn fit_pair(label: &str, summary: &str, room: usize) -> (String, String) {
    let label_width = display_width(label);
    if summary.is_empty() {
        return (fit(label, room), String::new());
    }
    let summary_width = display_width(summary) + 2;
    if label_width + summary_width <= room {
        return (label.to_string(), format!("  {summary}"));
    }
    let label_room = room
        .saturating_sub(summary_width)
        .max(label_width.min(room) / 2)
        .min(label_width);
    let label = fit(label, label_room);
    let left = room.saturating_sub(display_width(&label));
    let summary = if left > 4 {
        format!("  {}", fit(summary, left - 2))
    } else {
        String::new()
    };
    (label, summary)
}

/// Every line of the panels on screen; the panel whose header each line
/// is; and the line of the selected panel's header. An open panel's body
/// is followed by a blank line, so blocks stand apart.
pub(super) fn document(
    state: &LogState,
    width: usize,
    spinner: &'static str,
    theme: &Theme,
) -> Document {
    let mut lines = Vec::new();
    let mut owners = Vec::new();
    let mut selected = None;
    for row in state.rows() {
        let picked = state.selected.as_ref() == Some(&row.panel);
        if picked {
            selected = Some(lines.len());
        }
        lines.push(header_line(state, &row, width, picked, spinner, theme));
        owners.push(Some(row.panel.clone()));
        if state.is_expanded(&row.panel) {
            let indent = GUTTER + row.depth * 2 + 2;
            let body = body_lines(state, &row.panel, width.saturating_sub(indent), theme);
            if body.is_empty() {
                continue;
            }
            for line in body {
                let mut spans = vec![Span::raw(" ".repeat(indent))];
                spans.extend(line.spans);
                lines.push(Line::from(spans));
                owners.push(None);
            }
            lines.push(Line::default());
            owners.push(None);
        }
    }
    (lines, owners, selected)
}

/// What a row shows after its marks: its label and summary with their
/// styles, and the text at its right with its style.
struct RowText<'a> {
    label: &'a str,
    label_style: Style,
    summary: &'a str,
    summary_style: Style,
    right: String,
    right_style: Style,
}

/// One panel's header row, highlighted when `picked`.
fn header_line(
    state: &LogState,
    row: &LogRow,
    width: usize,
    picked: bool,
    spinner: &'static str,
    theme: &Theme,
) -> Line<'static> {
    let Some(status) = row_status(state, &row.panel) else {
        return Line::default();
    };
    let faint = Style::default().fg(theme.colors.faint);
    let mut spans = vec![
        status_mark(status, spinner, theme),
        Span::raw("  ".repeat(row.depth)),
        Span::styled(row_marker(state, &row.panel), faint),
    ];
    let Some(row_text) = row_text(state, &row.panel, width, &mut spans, theme) else {
        return Line::default();
    };
    finish_row(spans, row_text, width, picked, theme)
}

/// How the panel went, `None` when it is gone from the log.
fn row_status(state: &LogState, panel: &LogPanel) -> Option<LogStatus> {
    match panel {
        LogPanel::Run(id) => state.run(id).map(|r| r.status),
        LogPanel::Details(id) => state.run(id).map(|_| LogStatus::Ok),
        LogPanel::Turn(id, n) => state.turn(id, *n).map(|t| t.status),
        LogPanel::Step(id, seq) => state.step(id, *seq).map(|s| s.status),
    }
}

/// Whether the panel is open, shut or does not open.
fn row_marker(state: &LogState, panel: &LogPanel) -> &'static str {
    let open = state.is_expanded(panel);
    // Every step opens, so only an open one is marked.
    match panel {
        _ if open => "▾ ",
        LogPanel::Step(..) => "  ",
        panel if state.opens(panel) => "▸ ",
        _ => "  ",
    }
}

/// What the panel's row shows after its marks; a turn's number and a
/// step's kind go onto `spans` first.
fn row_text<'a>(
    state: &'a LogState,
    panel: &LogPanel,
    width: usize,
    spans: &mut Vec<Span<'static>>,
    theme: &Theme,
) -> Option<RowText<'a>> {
    let muted = Style::default().fg(theme.colors.muted);
    let faint = Style::default().fg(theme.colors.faint);
    let text = Style::default().fg(theme.colors.text);
    match panel {
        LogPanel::Run(id) => {
            let run = state.run(id)?;
            Some(RowText {
                label: run.title.as_str(),
                label_style: Style::default()
                    .fg(theme.colors.secondary)
                    .add_modifier(Modifier::BOLD),
                summary: run.detail.as_str(),
                summary_style: muted,
                right: String::new(),
                right_style: muted,
            })
        }
        LogPanel::Details(id) => Some(details_text(state, id, theme)),
        LogPanel::Turn(id, n) => {
            let turn = state.turn(id, *n)?;
            spans.push(Span::styled(
                format!("Turn {n}  "),
                text.add_modifier(Modifier::BOLD),
            ));
            let (label, summary) = if turn.prompt.is_empty() {
                (turn.detail.as_str(), "")
            } else {
                (turn.prompt.as_str(), turn.detail.as_str())
            };
            Some(RowText {
                label,
                label_style: if turn.prompt.is_empty() { muted } else { text },
                summary,
                summary_style: muted,
                right: columns([&turn.tokens, &turn.cost, &turn.took], width),
                right_style: muted,
            })
        }
        LogPanel::Step(id, seq) => {
            let step = state.step(id, *seq)?;
            if step.parent.is_some() {
                spans.push(Span::raw(" ".repeat(KIND_WIDTH)));
                spans.push(Span::styled("↳ ", faint));
            } else {
                spans.push(Span::styled(format!("{:<KIND_WIDTH$}", step.kind), faint));
            }
            Some(RowText {
                label: step.label.as_str(),
                // The end of a turn is said quietly; what it did is in its turn.
                label_style: if step.kind == "end" { muted } else { text },
                summary: step.summary.as_str(),
                summary_style: summary_style(step.status, theme),
                right: columns([&step.tokens, &step.cost, &step.took], width),
                right_style: muted,
            })
        }
    }
}

/// The `Details` row: how many problems the run had, at the right.
fn details_text<'a>(state: &LogState, id: &str, theme: &Theme) -> RowText<'a> {
    let muted = Style::default().fg(theme.colors.muted);
    let problems = state.run(id).map_or(0, |r| r.problems.len());
    let right = match problems {
        0 => String::new(),
        1 => "1 problem".to_string(),
        n => format!("{n} problems"),
    };
    RowText {
        label: "Details",
        label_style: muted,
        summary: "",
        summary_style: muted,
        right,
        right_style: Style::default().fg(theme.colors.warning),
    }
}

/// `spans` with the row's label, summary and right text fitted to
/// `width`, highlighted when `picked`.
fn finish_row(
    mut spans: Vec<Span<'static>>,
    row_text: RowText<'_>,
    width: usize,
    picked: bool,
    theme: &Theme,
) -> Line<'static> {
    let used: usize = spans.iter().map(|s| display_width(&s.content)).sum();
    let right = row_text.right;
    let right_width = if right.trim().is_empty() {
        0
    } else {
        display_width(&right)
    };
    let room = width.saturating_sub(used + if right_width == 0 { 0 } else { right_width + 2 });
    let (label, summary) = fit_pair(row_text.label, row_text.summary, room);
    let drawn = used + display_width(&label) + display_width(&summary);
    spans.push(Span::styled(label, row_text.label_style));
    spans.push(Span::styled(summary, row_text.summary_style));
    if right_width > 0 && drawn + right_width <= width {
        spans.push(Span::raw(" ".repeat(width - drawn - right_width)));
        spans.push(Span::styled(right, row_text.right_style));
    } else if picked {
        spans.push(Span::raw(" ".repeat(width.saturating_sub(drawn))));
    }
    if picked {
        let style = Style::default()
            .bg(theme.colors.selection)
            .fg(theme.colors.on_selection);
        spans = spans.into_iter().map(|s| s.patch_style(style)).collect();
    }
    Line::from(spans)
}
