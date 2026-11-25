//! Draw the chat panel's LOG tab: a header of three lines (the run's
//! status, title and `Copy log`; its totals and where the log is stored;
//! a rule), the heads of the tokens, cost and time columns, the run's
//! panels that fold open and shut, and the keys at the bottom.
//!
//! Until the log arrives the tab is blank: no title, no loading line, no
//! keys. The log, or a notice saying why there is none, then fades in
//! ([`crate::app::run_log::FADE_IN`]). Only the log of the agent on screen
//! is ever drawn.

use super::benchmarks::fit;
use super::panels::body_block;
use super::text::{display_width, wrap_words};
use crate::animation::fade_in;
use crate::app::run_log::{LogPanel, LogRow, LogState};
use crate::events::{LogField, LogSection, LogStatus, LogView};
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use unicode_width::UnicodeWidthChar;

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
    let block = body_block((is_focused, app.host_chrome()), theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.set_chat_layout(Vec::new(), 0);
    app.set_chat_images(Vec::new());
    app.set_chat_code_copies(Vec::new());
    app.set_chat_width(inner.width);
    app.set_chat_text_area(inner);
    let padded = Rect {
        x: inner.x.saturating_add(1),
        width: inner.width.saturating_sub(2),
        ..inner
    };
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
    frame.render_widget(Paragraph::new(keys_line(width, theme)), footer);
    let body = Rect {
        y: padded.y + TOP_LINES,
        height: padded.height.saturating_sub(TOP_LINES + 1),
        ..padded
    };

    let height = usize::from(body.height);
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
    scroll = scroll.min(lines.len().saturating_sub(height));
    let hits: Vec<(u16, LogPanel)> = owners
        .into_iter()
        .enumerate()
        .skip(scroll)
        .take(height)
        .filter_map(|(i, owner)| {
            let y = body.y + u16::try_from(i - scroll).ok()?;
            Some((y, owner?))
        })
        .collect();
    let offset = u16::try_from(scroll).unwrap_or(u16::MAX);
    frame.render_widget(Paragraph::new(lines).scroll((offset, 0)), body);

    app.set_chat_rows(Some((body, scroll)));
    let state = app.run_log_mut();
    state.scroll = scroll;
    state.follow = false;
    state.set_hits(hits, button);
}

/// The header's three lines, and where `Copy log` was drawn: the run's
/// status and title with the button, its totals with where the log is
/// stored, and a rule.
fn header_lines(
    state: &LogState,
    view: &LogView,
    area: Rect,
    spinner: &'static str,
    theme: &Theme,
) -> (Vec<Line<'static>>, Option<Rect>) {
    let width = usize::from(area.width);
    let muted = Style::default().fg(theme.colors.muted);
    let faint = Style::default().fg(theme.colors.faint);
    let bold = Style::default()
        .fg(theme.colors.text)
        .add_modifier(Modifier::BOLD);
    let root = view.root();
    let status = root.map_or(LogStatus::Ok, |r| r.status);
    let title = root.map_or("Run log", |r| r.title.as_str());

    let (label, color) = if state.just_copied() {
        (COPIED_BUTTON, theme.colors.success)
    } else {
        (COPY_BUTTON, theme.colors.text)
    };
    let button_width = display_width(label);
    let mut spans = vec![root_glyph(status, spinner, theme), Span::raw(" ")];
    let lead: usize = spans.iter().map(|s| display_width(&s.content)).sum();
    let room = width.saturating_sub(lead + button_width + 2);
    spans.push(Span::styled(fit(title, room), bold));
    let used: usize = spans.iter().map(|s| display_width(&s.content)).sum();
    let button = (width > used + button_width).then(|| {
        spans.push(Span::raw(" ".repeat(width - used - button_width)));
        spans.push(Span::styled(
            label,
            Style::default()
                .fg(color)
                .bg(theme.colors.surface)
                .add_modifier(Modifier::BOLD),
        ));
        Rect {
            x: area.x + u16::try_from(width - button_width).unwrap_or(0),
            y: area.y,
            width: u16::try_from(button_width).unwrap_or(0),
            height: 1,
        }
    });

    let live = status == LogStatus::Running;
    let detail = root.map_or("", |r| r.detail.as_str());
    let live_width = if live { display_width(" · live") } else { 0 };
    let mut info = vec![
        Span::raw(" ".repeat(lead)),
        Span::styled(fit(detail, width.saturating_sub(lead + live_width)), muted),
    ];
    if live {
        if !detail.is_empty() {
            info.push(Span::styled(" · ", muted));
        }
        info.push(Span::styled(
            "live",
            Style::default().fg(theme.colors.success),
        ));
    }
    let used: usize = info.iter().map(|s| display_width(&s.content)).sum();
    // The path goes at the right when a useful part of it fits.
    let left = width.saturating_sub(used + 3);
    if left >= 12 {
        let path = fit_start(&view.path, left);
        let gap = width - used - display_width(&path);
        info.push(Span::raw(" ".repeat(gap)));
        info.push(Span::styled(path, faint));
    }
    (
        vec![Line::from(spans), Line::from(info), rule(width, theme)],
        button,
    )
}

/// The run's status before its title: a spinner while it runs, else a
/// dot in the colour of how it went.
fn root_glyph(status: LogStatus, spinner: &'static str, theme: &Theme) -> Span<'static> {
    let (glyph, color) = match status {
        LogStatus::Running => (spinner, theme.colors.pending),
        LogStatus::Ok => ("●", theme.colors.success),
        LogStatus::Warn => ("●", theme.colors.warning),
        LogStatus::Error => ("●", theme.colors.error),
    };
    Span::styled(glyph, Style::default().fg(color))
}

/// The heads of the columns, over the columns of the rows below.
fn column_heads(width: usize, theme: &Theme) -> Line<'static> {
    let heads = columns(COLUMN_HEADS, width);
    if heads.trim().is_empty() {
        return Line::default();
    }
    Line::from(vec![
        Span::raw(" ".repeat(width.saturating_sub(display_width(&heads)))),
        Span::styled(heads, Style::default().fg(theme.colors.faint)),
    ])
}

/// The keys at the bottom, as many as fit in `width`.
fn keys_line(width: usize, theme: &Theme) -> Line<'static> {
    let key = Style::default().fg(theme.colors.muted);
    let what = Style::default().fg(theme.colors.faint);
    let mut spans = Vec::new();
    let mut used = 0;
    for (i, (k, does)) in KEYS.iter().enumerate() {
        let gap = if i == 0 { 0 } else { 2 };
        let needs = gap + display_width(k) + 1 + display_width(does);
        if used + needs > width {
            break;
        }
        if gap > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(*k, key));
        spans.push(Span::styled(format!(" {does}"), what));
        used += needs;
    }
    Line::from(spans)
}

fn rule(width: usize, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        "─".repeat(width),
        Style::default().fg(theme.colors.faint),
    ))
}

/// `text` cut from the start to `max` columns, as `…/runs/x/log.json`.
fn fit_start(text: &str, max: usize) -> String {
    if display_width(text) <= max || max == 0 {
        return text.to_string();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut used = 1;
    for c in text.chars().rev() {
        let w = c.width().unwrap_or(0);
        if used + w > max {
            break;
        }
        kept.push(c);
        used += w;
    }
    std::iter::once('…').chain(kept.into_iter().rev()).collect()
}

/// `text` broken into lines at most `width` columns wide, at any
/// character.
fn hard_wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut current = String::new();
        let mut used = 0;
        for c in line.chars() {
            let c = if c == '\t' { ' ' } else { c };
            let w = c.width().unwrap_or(0);
            if used + w > width && !current.is_empty() {
                out.push(std::mem::take(&mut current));
                used = 0;
            }
            current.push(c);
            used += w;
        }
        out.push(current);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// `text` wrapped at words, keeping its line breaks and blank lines.
fn prose_wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            out.push(String::new());
        } else if display_width(line) <= width {
            out.push(line.to_string());
        } else {
            out.extend(wrap_words(line, width));
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

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
fn columns(values: [&str; 3], width: usize) -> String {
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

/// Every line of the panels on screen; the panel whose header each line
/// is; and the line of the selected panel's header. An open panel's body
/// is followed by a blank line, so blocks stand apart.
fn document(state: &LogState, width: usize, spinner: &'static str, theme: &Theme) -> Document {
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

fn header_line(
    state: &LogState,
    row: &LogRow,
    width: usize,
    picked: bool,
    spinner: &'static str,
    theme: &Theme,
) -> Line<'static> {
    let muted = Style::default().fg(theme.colors.muted);
    let faint = Style::default().fg(theme.colors.faint);
    let text = Style::default().fg(theme.colors.text);
    let bold = text.add_modifier(Modifier::BOLD);
    let status = match &row.panel {
        LogPanel::Run(id) => state.run(id).map(|r| r.status),
        LogPanel::Details(id) => state.run(id).map(|_| LogStatus::Ok),
        LogPanel::Turn(id, n) => state.turn(id, *n).map(|t| t.status),
        LogPanel::Step(id, seq) => state.step(id, *seq).map(|s| s.status),
    };
    let Some(status) = status else {
        return Line::default();
    };
    let open = state.is_expanded(&row.panel);
    // Every step opens, so only an open one is marked.
    let marker = match &row.panel {
        _ if open => "▾ ",
        LogPanel::Step(..) => "  ",
        panel if state.opens(panel) => "▸ ",
        _ => "  ",
    };
    let mut spans = vec![
        status_mark(status, spinner, theme),
        Span::raw("  ".repeat(row.depth)),
        Span::styled(marker, faint),
    ];
    let row_text = match &row.panel {
        LogPanel::Run(id) => {
            let Some(run) = state.run(id) else {
                return Line::default();
            };
            RowText {
                label: run.title.as_str(),
                label_style: Style::default()
                    .fg(theme.colors.secondary)
                    .add_modifier(Modifier::BOLD),
                summary: run.detail.as_str(),
                summary_style: muted,
                right: String::new(),
                right_style: muted,
            }
        }
        LogPanel::Details(id) => {
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
        LogPanel::Turn(id, n) => {
            let Some(turn) = state.turn(id, *n) else {
                return Line::default();
            };
            spans.push(Span::styled(format!("Turn {n}  "), bold));
            let (label, summary) = if turn.prompt.is_empty() {
                (turn.detail.as_str(), "")
            } else {
                (turn.prompt.as_str(), turn.detail.as_str())
            };
            RowText {
                label,
                label_style: if turn.prompt.is_empty() { muted } else { text },
                summary,
                summary_style: muted,
                right: columns([&turn.tokens, &turn.cost, &turn.took], width),
                right_style: muted,
            }
        }
        LogPanel::Step(id, seq) => {
            let Some(step) = state.step(id, *seq) else {
                return Line::default();
            };
            if step.parent.is_some() {
                spans.push(Span::raw(" ".repeat(KIND_WIDTH)));
                spans.push(Span::styled("↳ ", faint));
            } else {
                spans.push(Span::styled(format!("{:<KIND_WIDTH$}", step.kind), faint));
            }
            RowText {
                label: step.label.as_str(),
                // The end of a turn is said quietly; what it did is in its turn.
                label_style: if step.kind == "end" { muted } else { text },
                summary: step.summary.as_str(),
                summary_style: summary_style(step.status, theme),
                right: columns([&step.tokens, &step.cost, &step.took], width),
                right_style: muted,
            }
        }
    };
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

fn field_lines(
    fields: &[LogField],
    width: usize,
    theme: &Theme,
    accent: Option<&str>,
) -> Vec<Line<'static>> {
    let label_width = fields
        .iter()
        .map(|(label, _)| display_width(label))
        .max()
        .unwrap_or(0)
        .min(20);
    let value_width = width.saturating_sub(label_width + 2).max(8);
    let mut lines = Vec::new();
    for (label, value) in fields {
        let style = if accent == Some(label.as_str()) {
            Style::default()
                .fg(theme.colors.primary)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.colors.text)
        };
        for (i, part) in hard_wrap(value, value_width).into_iter().enumerate() {
            let head = if i == 0 {
                format!("{:<label_width$}  ", fit(label, label_width))
            } else {
                " ".repeat(label_width + 2)
            };
            lines.push(Line::from(vec![
                Span::styled(head, Style::default().fg(theme.colors.muted)),
                Span::styled(part, style),
            ]));
        }
    }
    lines
}

fn section_lines(section: &LogSection, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let faint = Style::default().fg(theme.colors.faint);
    let mut lines = vec![Line::from(vec![
        Span::styled("── ", faint),
        Span::styled(
            section.title.clone(),
            Style::default()
                .fg(theme.colors.muted)
                .add_modifier(Modifier::BOLD),
        ),
    ])];
    let inner = width.saturating_sub(2).max(8);
    let body = if section.code {
        hard_wrap(&section.body, inner)
    } else {
        prose_wrap(&section.body, inner)
    };
    let style = if section.title == "Thinking" {
        Style::default()
            .fg(theme.colors.muted)
            .add_modifier(Modifier::ITALIC)
    } else {
        Style::default().fg(theme.colors.text)
    };
    let total = body.len();
    for line in body.into_iter().take(SECTION_LINES) {
        lines.push(Line::from(vec![
            Span::styled("│ ", faint),
            Span::styled(line, style),
        ]));
    }
    if total > SECTION_LINES {
        lines.push(Line::from(Span::styled(
            format!(
                "… {} more lines; y copies the whole step",
                total - SECTION_LINES
            ),
            Style::default().fg(theme.colors.muted),
        )));
    }
    lines
}

fn body_lines(
    state: &LogState,
    panel: &LogPanel,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    match panel {
        LogPanel::Run(_) | LogPanel::Turn(..) => Vec::new(),
        LogPanel::Details(id) => {
            let Some(run) = state.run(id) else {
                return Vec::new();
            };
            let mut lines = field_lines(&run.fields, width, theme, None);
            for problem in &run.problems {
                for (i, part) in prose_wrap(problem, width.saturating_sub(2))
                    .into_iter()
                    .enumerate()
                {
                    let head = if i == 0 { "! " } else { "  " };
                    lines.push(Line::from(Span::styled(
                        format!("{head}{part}"),
                        Style::default().fg(theme.colors.warning),
                    )));
                }
            }
            if !run.files.is_empty() {
                if !lines.is_empty() {
                    lines.push(Line::default());
                }
                lines.extend(field_lines(&run.files, width, theme, Some("log")));
            }
            lines
        }
        LogPanel::Step(id, seq) => {
            let Some(step) = state.step(id, *seq) else {
                return Vec::new();
            };
            if state.raw.contains(panel) {
                return hard_wrap(&step.json, width)
                    .into_iter()
                    .map(|line| {
                        Line::from(Span::styled(line, Style::default().fg(theme.colors.text)))
                    })
                    .collect();
            }
            let mut lines = field_lines(&step.fields, width, theme, None);
            for section in &step.sections {
                lines.extend(section_lines(section, width, theme));
            }
            lines
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{LogRunView, LogStepView, LogTurnView, UiCommand};
    use ratatui::{backend::TestBackend, Terminal};
    use std::time::Instant;

    fn view() -> LogView {
        LogView {
            agent_id: String::new(),
            path: "/home/me/.aura/runs/1a-x/log.json".into(),
            json: "{}".into(),
            runs: vec![
                LogRunView {
                    run_id: "1a-x".into(),
                    title: "fix the parser".into(),
                    detail: "sonnet-5-5 · 1 turn".into(),
                    files: vec![("log".into(), "/home/me/.aura/runs/1a-x/log.json".into())],
                    fields: vec![("model".into(), "claude/claude-sonnet-5-5".into())],
                    turns: vec![LogTurnView {
                        turn: 1,
                        detail: "3 calls · end_turn".into(),
                        tokens: "2.2M tok".into(),
                        cost: "$1.45".into(),
                        took: "4m 02s".into(),
                        steps: vec![
                            LogStepView {
                                seq: 2,
                                kind: "model".into(),
                                label: "task @explore find the token bug".into(),
                                fields: vec![("model".into(), "m".into())],
                                sections: vec![LogSection {
                                    title: "Thinking".into(),
                                    body: "the parser drops the last token".into(),
                                    code: false,
                                }],
                                spawned: vec![1],
                                tokens: "7k tok".into(),
                                cost: "$0.04".into(),
                                took: "6s".into(),
                                ..LogStepView::default()
                            },
                            LogStepView {
                                seq: 3,
                                kind: "model".into(),
                                label: "read_file src/parser.rs · run_command cargo test".into(),
                                tokens: "14k tok".into(),
                                cost: "$0.0091".into(),
                                took: "3s".into(),
                                ..LogStepView::default()
                            },
                            LogStepView {
                                seq: 4,
                                kind: "tool".into(),
                                label: "read_file src/parser.rs".into(),
                                summary: "340 lines".into(),
                                parent: Some(3),
                                took: "19ms".into(),
                                ..LogStepView::default()
                            },
                            LogStepView {
                                seq: 5,
                                kind: "tool".into(),
                                label: "run_command cargo test".into(),
                                summary: "exit 101 · 40 lines".into(),
                                parent: Some(3),
                                status: LogStatus::Error,
                                took: "2m 10s".into(),
                                ..LogStepView::default()
                            },
                        ],
                        ..LogTurnView::default()
                    }],
                    ..LogRunView::default()
                },
                LogRunView {
                    run_id: "1b-y".into(),
                    title: "find the token bug".into(),
                    depth: 1,
                    ..LogRunView::default()
                },
            ],
        }
    }

    fn screen(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
        terminal
            .draw(|frame| render_run_log(frame, frame.area(), app, &Theme::default(), true))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The row of `screen` that holds `needle`.
    fn row<'a>(screen: &'a str, needle: &str) -> &'a str {
        screen
            .lines()
            .find(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not on screen:\n{screen}"))
    }

    /// The index of the header's rule on `screen`.
    fn rule_row(screen: &str) -> usize {
        screen
            .lines()
            .position(|line| line.contains("│ ────────"))
            .unwrap_or_else(|| panic!("no rule:\n{screen}"))
    }

    #[test]
    fn shows_the_run_the_copy_button_and_nested_subagents() {
        let mut app = App::new();
        app.process_command(UiCommand::LogView(Box::new(view())));
        app.run_log_mut().toggle(&LogPanel::Step("1a-x".into(), 2));
        let text = screen(&mut app);
        let top = row(&text, "fix the parser");
        assert!(top.contains("Copy log"), "{text}");
        assert!(top.contains('●'), "the run's status leads: {top}");
        let info = row(&text, "sonnet-5-5 · 1 turn");
        assert!(info.contains("/home/me/.aura/runs/1a-x/log.json"), "{text}");
        assert!(
            !text.contains("RUN  ") && !text.contains("SUBAGENT"),
            "{text}"
        );
        assert!(text.contains("Turn 1"), "{text}");
        assert!(text.contains("the parser drops the last token"), "{text}");
        let child = text
            .lines()
            .find(|l| l.contains("find the token bug") && !l.contains("task @explore"))
            .unwrap_or_else(|| panic!("the subagent's row:\n{text}"));
        assert!(child.contains('▸'), "{child}");

        let button = app.run_log().copy_button_area().expect("drawn");
        assert!(app.click_log(button.x + 1, button.y));
        let buffer = ratatui::buffer::Buffer::empty(Rect::new(0, 0, 1, 1));
        assert_eq!(app.take_copy_text(&buffer).as_deref(), Some("{}"));
        app.notify_copied(2, &Ok(()));
        assert!(
            screen(&mut app).contains("✓ Copied"),
            "the button says it copied"
        );
    }

    #[test]
    fn steps_read_as_calls_and_results_with_their_columns_lined_up() {
        let mut app = App::new();
        app.process_command(UiCommand::LogView(Box::new(view())));
        let text = screen(&mut app);
        let model = row(&text, "read_file src/parser.rs · run_command cargo test");
        assert!(model.contains("model"), "{text}");
        let read = row(&text, "↳ read_file src/parser.rs  340 lines");
        let failed = row(&text, "↳ run_command cargo test  exit 101 · 40 lines");
        assert!(failed.contains('✗'), "a failure is marked: {failed}");
        for line in [model, read, failed] {
            assert!(!line.contains('✓') && !line.contains('▸'), "{line}");
        }
        let at = |line: &str, c: char| line.chars().position(|x| x == c);
        assert_eq!(
            at(failed, '✗'),
            at(row(&text, "Turn 1"), 'T').map(|t| t - 4),
            "the failure sits in the gutter, left of every row:\n{text}"
        );
        let end = |line: &str, cell: &str| {
            line.find(cell)
                .map(|at| line[..at].chars().count() + cell.chars().count())
        };
        let turn = row(&text, "Turn 1");
        assert!(turn.contains("3 calls · end_turn"), "{turn}");
        assert_eq!(
            end(model, "3s"),
            end(turn, "4m 02s"),
            "times line up:\n{text}"
        );
        assert_eq!(end(read, "19ms"), end(model, "3s"), "{text}");
        assert_eq!(end(model, "$0.0091"), end(turn, "$1.45"), "{text}");
        assert_eq!(end(model, "14k tok"), end(turn, "2.2M tok"), "{text}");
        let heads = row(&text, "tokens");
        assert_eq!(end(heads, "time"), end(turn, "4m 02s"), "{text}");
        assert_eq!(end(heads, "cost"), end(turn, "$1.45"), "{text}");
    }

    #[test]
    fn the_details_start_shut_and_hold_the_files() {
        let mut app = App::new();
        app.process_command(UiCommand::LogView(Box::new(view())));
        let text = screen(&mut app);
        assert!(row(&text, "Details").contains('▸'), "{text}");
        assert!(!text.contains("claude/claude-sonnet-5-5"), "{text}");
        app.run_log_mut().toggle(&LogPanel::Details("1a-x".into()));
        let text = screen(&mut app);
        assert!(text.contains("claude/claude-sonnet-5-5"), "{text}");
        assert!(
            row(&text, "log  /home").contains("/home/me/.aura/runs/1a-x/log.json"),
            "{text}"
        );
    }

    #[test]
    fn a_long_title_is_cut_so_the_header_keeps_three_lines() {
        let mut log = view();
        log.runs[0].title = "remember the tab selected with each agent, and default to the chat \
            tab when none was selected before, in the terminal and the desktop app"
            .into();
        let mut app = App::new();
        app.process_command(UiCommand::LogView(Box::new(log)));
        let text = screen(&mut app);
        let lines: Vec<&str> = text.lines().collect();
        let top = lines.iter().position(|l| l.contains("Copy log")).unwrap();
        assert!(lines[top].contains('…'), "{text}");
        assert!(!text.contains("the desktop app"), "{text}");
        assert!(lines[top + 1].contains("log.json"), "{text}");
        assert_eq!(rule_row(&text), top + 2, "{text}");
    }

    #[test]
    fn a_notice_comes_alone_and_fades_in() {
        let mut app = App::new();
        app.process_command(UiCommand::LogNotice {
            agent_id: String::new(),
            notice: "This agent has no run log yet; send it a prompt".into(),
        });
        let text = screen(&mut app);
        assert!(!text.contains("Run log"), "{text}");
        assert!(!text.contains("esc back"), "no keys without a log: {text}");
        assert!(app.run_log().fading().is_some());
    }

    #[test]
    fn coming_back_draws_neither_the_old_log_nor_an_old_notice() {
        let mut app = App::new();
        app.select_pane_tab(crate::PaneTab::Chat(crate::ChatTab::Log));
        app.process_command(UiCommand::LogView(Box::new(view())));
        app.select_pane_tab(crate::PaneTab::Chat(crate::ChatTab::Chat));
        app.select_pane_tab(crate::PaneTab::Chat(crate::ChatTab::Log));
        let text = screen(&mut app);
        assert!(!text.contains("fix the parser"), "{text}");
        assert!(!text.contains("Copy log"), "{text}");

        app.process_command(UiCommand::LogNotice {
            agent_id: String::new(),
            notice: "This agent has no run log yet; send it a prompt".into(),
        });
        app.select_pane_tab(crate::PaneTab::Chat(crate::ChatTab::Chat));
        app.select_pane_tab(crate::PaneTab::Chat(crate::ChatTab::Log));
        let text = screen(&mut app);
        assert!(!text.contains("no run log yet"), "{text}");
    }

    #[test]
    fn says_why_there_is_no_log() {
        let mut app = App::new();
        app.process_command(UiCommand::LogNotice {
            agent_id: String::new(),
            notice: "This agent has no run log yet; send it a prompt".into(),
        });
        let text = screen(&mut app);
        assert!(text.contains("no run log yet"), "{text}");
        assert!(text.contains("r to retry"), "{text}");
        assert!(!text.contains("Copy log"));
    }

    #[test]
    fn nothing_is_drawn_before_the_log_arrives() {
        let mut app = App::new();
        app.run_log_mut().loading_since =
            Instant::now().checked_sub(std::time::Duration::from_secs(5));
        let text = screen(&mut app);
        let inside: String = text
            .lines()
            .skip(1)
            .take(28)
            .map(|line| line.trim_matches(|c: char| c == '│' || c.is_whitespace()))
            .collect();
        assert!(
            inside.is_empty(),
            "a slow load still draws nothing:\n{text}"
        );

        app.process_command(UiCommand::LogView(Box::new(view())));
        let text = screen(&mut app);
        assert!(text.contains("fix the parser"), "{text}");
        assert!(app.run_log().fading().is_some(), "the log fades in");
    }

    #[test]
    fn the_log_of_another_agent_is_never_drawn() {
        let mut app = App::new();
        app.run_log_mut().view = Some(LogView {
            agent_id: "someone-else".into(),
            ..view()
        });
        let text = screen(&mut app);
        assert!(!text.contains("fix the parser"), "{text}");
        assert!(!text.contains("Copy log"), "{text}");
        assert!(app.run_log().copy_button_area().is_none());
    }
}
