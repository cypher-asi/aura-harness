//! Draw the chat panel's BENCH tab: the list of benchmarks, one
//! benchmark's numbers side by side, or the form that sets one up.

use super::panels::body_block;
use super::text::display_width;
use crate::app::benchmarks::{BenchMode, BenchState, FormField, WorkChoice};
use crate::events::{
    arm_letter, cost_per_fix, run_order, BenchArm, BenchDoing, BenchLiveRun, BenchRun, BenchStatus,
    BenchTaskCell, BenchTaskSetup, BenchTokens, BenchTurn, BenchVerdict, BenchView,
};
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
use std::ops::Range;
use unicode_width::UnicodeWidthChar;

/// `$0.004`, `$1.25`, or `free` for nothing.
pub(super) fn money(usd: f64) -> String {
    if usd <= 0.0 {
        "free".to_string()
    } else if usd < 0.01 {
        format!("${usd:.4}")
    } else {
        format!("${usd:.2}")
    }
}

/// `850ms`, `42s`, `3m05s`.
fn duration(ms: u64) -> String {
    if ms < 1_000 {
        return format!("{ms}ms");
    }
    let secs = ms / 1_000;
    if secs < 60 {
        return format!("{secs}s");
    }
    format!("{}m{:02}s", secs / 60, secs % 60)
}

/// `thinking 1m40s · call 12 · restarted 2×`, or `running task 8m12s ›`
/// and what the subtask is doing.
fn doing_text(doing: &BenchDoing) -> String {
    let mut text = format!("{} {}", doing.what, duration(doing.for_ms));
    if doing.call > 0 {
        text.push_str(&format!(" · call {}", doing.call));
    }
    if doing.restarts > 0 {
        text.push_str(&format!(" · restarted {}×", doing.restarts));
    }
    if let Some(subtask) = &doing.subtask {
        text.push_str(" › ");
        text.push_str(&doing_text(subtask));
    }
    text
}

/// `950`, `471k`, `1.5M`.
#[allow(clippy::cast_precision_loss)]
fn tokens(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{}k", n / 1_000)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

/// `opus-5-5` for `claude/claude-opus-5-5`: no provider, no `claude-`.
pub(super) fn short_model(model: &str) -> &str {
    let name = model.rsplit('/').next().unwrap_or(model);
    name.strip_prefix("claude-").unwrap_or(name)
}

/// `multi-file`: what the classifier said a run's first prompt asks for;
/// empty when nothing was recorded.
fn category(run: &BenchRun) -> String {
    run.category
        .as_deref()
        .map_or("", |category| match category {
            "small_change" => "small change",
            "multi_file_change" => "multi-file",
            other => other,
        })
        .to_string()
}

/// `to opus-4-7` for a run a failed attempt moved to a stronger model,
/// `to opus-5-5 max` for one it moved to a higher effort, else `no`.
fn moved_up(run: &BenchRun) -> String {
    let Some(to) = run.escalated_to.as_deref() else {
        return "no".to_string();
    };
    match &run.escalated_effort {
        Some(effort) => format!("to {} {effort}", short_model(to)),
        None => format!("to {}", short_model(to)),
    }
}

/// `yes, $0.26 in subtasks` for a run whose model led a turn, handing
/// the work to subtasks, else `no`.
fn led(run: &BenchRun) -> String {
    if run.led {
        format!("yes, {} in subtasks", money(run.subtask_cost_usd))
    } else {
        "no".to_string()
    }
}

/// `-74%` for the cheaper of two costs, nothing for the other or when
/// they are about the same or either is unknown.
fn saving(a: Option<f64>, b: Option<f64>) -> [String; 2] {
    let (Some(a), Some(b)) = (a, b) else {
        return [String::new(), String::new()];
    };
    let larger = a.max(b);
    if larger <= 0.0 {
        return [String::new(), String::new()];
    }
    let less = |cost: f64| {
        let less = ((1.0 - cost / larger) * 100.0).round();
        if less < 1.0 {
            String::new()
        } else {
            format!("-{less:.0}%")
        }
    };
    [less(a), less(b)]
}

/// `text` in `style` for something under way: on frame `frame` a band of
/// light from `base` to `light` sweeps across it; without a frame, as
/// when animations are off, it is plain.
pub(super) fn under_way(
    text: &str,
    style: Style,
    frame: Option<usize>,
    colors: (Color, Color),
) -> Vec<Span<'static>> {
    match frame {
        Some(frame) => crate::animation::shimmer_spans(text, frame, style, colors),
        None => vec![Span::styled(text.to_string(), style)],
    }
}

/// `text` cut to `max` columns, ending in `…` when cut.
pub(super) fn fit(text: &str, max: usize) -> String {
    if display_width(text) <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > max - 1 {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

/// The directory every example task's id shares, as `swe_bench_pro_hard`.
pub(super) fn task_prefix(view: &BenchView) -> Option<String> {
    let (prefix, _) = view.tasks.first()?.id.rsplit_once('/')?;
    let all = view.tasks.iter().all(|t| {
        t.id.strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
    });
    all.then(|| prefix.to_string())
}

/// A task id without the directory every task shares.
fn task_name<'s>(id: &'s str, prefix: Option<&str>) -> &'s str {
    prefix
        .and_then(|p| id.strip_prefix(p))
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(id)
}

/// How a column of a [`table`] lines up.
#[derive(Debug, Clone, Copy)]
struct Col {
    right: bool,
    /// Gives up width, cutting its cells, when the table is too wide
    flex: bool,
    /// Drawn after a ` │ ` rather than a plain gap
    sep: bool,
}

impl Col {
    const L: Self = Self {
        right: false,
        flex: false,
        sep: false,
    };
    const R: Self = Self {
        right: true,
        ..Self::L
    };

    const fn flex(self) -> Self {
        Self { flex: true, ..self }
    }

    const fn sep(self) -> Self {
        Self { sep: true, ..self }
    }
}

type Cell = (String, Style);

enum Row {
    Cells(Vec<Cell>),
    /// Cells drawn in a selection style, gaps included
    Picked(Vec<Cell>, Style),
    /// Cells of something under way, with `status` drawn across the
    /// columns of `span`, whose own cells stay empty. On a frame, a band
    /// of light sweeps across the status.
    Live {
        cells: Vec<Cell>,
        span: Range<usize>,
        status: Cell,
        frame: Option<usize>,
    },
    /// A rule across the table, as between groups of rows
    Rule,
}

/// `rows` in columns as wide as their widest cell, at most `width`
/// together: flex columns give up width first. Columns empty in every row
/// are left out. `header` gets a rule under it.
fn table(
    cols: &[Col],
    header: Option<&[&str]>,
    rows: &[Row],
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let faint = Style::default().fg(theme.colors.faint);
    let cell_width = |row: &Row, i: usize| match row {
        Row::Cells(cells) | Row::Picked(cells, _) | Row::Live { cells, .. } => {
            cells.get(i).map_or(0, |(text, _)| display_width(text))
        }
        Row::Rule => 0,
    };
    let mut widths: Vec<usize> = (0..cols.len())
        .map(|i| {
            let head = header
                .and_then(|h| h.get(i))
                .map_or(0, |text| display_width(text));
            rows.iter()
                .map(|row| cell_width(row, i))
                .max()
                .unwrap_or(0)
                .max(head)
        })
        .collect();
    let gap = |i: usize| if cols[i].sep { 3 } else { 2 };
    // Columns `open` take together, gaps between them included.
    let across = |open: &[usize], widths: &[usize]| -> usize {
        open.iter()
            .enumerate()
            .map(|(n, &i)| widths[i] + if n == 0 { 0 } else { gap(i) })
            .sum()
    };
    for row in rows {
        let Row::Live { span, status, .. } = row else {
            continue;
        };
        let span = span.start.min(cols.len())..span.end.min(cols.len());
        if span.is_empty() {
            continue;
        }
        let open: Vec<usize> = span.clone().filter(|&i| widths[i] > 0).collect();
        let short = display_width(&status.0).saturating_sub(across(&open, &widths));
        widths[open.last().copied().unwrap_or(span.start)] += short;
    }
    let shown: Vec<usize> = (0..cols.len()).filter(|&i| widths[i] > 0).collect();
    let total = across(&shown, &widths);
    let mut over = total.saturating_sub(width);
    while over > 0 {
        let Some(&i) = shown
            .iter()
            .filter(|&&i| cols[i].flex && widths[i] > 1)
            .max_by_key(|&&i| widths[i])
        else {
            break;
        };
        widths[i] -= 1;
        over -= 1;
    }
    let shine = (theme.colors.muted, theme.colors.foreground);
    type Spanned<'r> = (&'r Range<usize>, &'r Cell, Option<usize>);
    let line = |cells: &[Cell], picked: Option<Style>, live: Option<Spanned<'_>>| {
        let open: Vec<usize> = live.map_or_else(Vec::new, |(span, ..)| {
            shown.iter().copied().filter(|i| span.contains(i)).collect()
        });
        let status = live
            .map(|(_, status, _)| status)
            .filter(|(text, _)| !text.is_empty());
        let filled = |i: usize| {
            cells.get(i).is_some_and(|(text, _)| !text.is_empty())
                || (status.is_some() && open.first() == Some(&i))
        };
        let last = if picked.is_some() {
            shown.last().copied()
        } else {
            shown.iter().rev().find(|&&i| filled(i)).copied()
        };
        let mut spans = Vec::new();
        for (n, &i) in shown.iter().enumerate() {
            if last.is_some_and(|last| i > last) {
                break;
            }
            let first_open = open.first() == Some(&i);
            if open.contains(&i) && !first_open {
                continue;
            }
            if n > 0 {
                let (text, style) = if cols[i].sep {
                    (" │ ", faint)
                } else {
                    ("  ", Style::default())
                };
                spans.push(Span::styled(text, picked.unwrap_or(style)));
            }
            if first_open {
                let room = across(&open, &widths);
                let (text, style) = status.cloned().unwrap_or_default();
                let text = fit(&text, room);
                let text = if Some(i) == last {
                    text
                } else {
                    let pad = " ".repeat(room - display_width(&text));
                    text + &pad
                };
                match live.and_then(|(.., frame)| frame) {
                    Some(frame) => spans.extend(under_way(&text, style, Some(frame), shine)),
                    None => spans.push(Span::styled(text, style)),
                }
                continue;
            }
            let (text, style) = cells.get(i).cloned().unwrap_or_default();
            let text = fit(&text, widths[i]);
            let pad = " ".repeat(widths[i] - display_width(&text));
            let text = if cols[i].right {
                pad + &text
            } else if Some(i) == last && picked.is_none() {
                text
            } else {
                text + &pad
            };
            let style = picked.map_or(style, |p| style.patch(p));
            spans.push(Span::styled(text, style));
        }
        Line::from(spans)
    };
    let rule = || {
        let text: String = shown
            .iter()
            .enumerate()
            .map(|(n, &i)| {
                let lead = match (n, cols[i].sep) {
                    (0, _) => "",
                    (_, true) => "─┼─",
                    (_, false) => "──",
                };
                format!("{lead}{}", "─".repeat(widths[i]))
            })
            .collect();
        Line::from(Span::styled(text, faint))
    };
    let mut lines = Vec::new();
    if let Some(header) = header {
        let muted = Style::default().fg(theme.colors.muted);
        let cells: Vec<Cell> = header.iter().map(|h| ((*h).to_string(), muted)).collect();
        lines.push(line(&cells, None, None));
        lines.push(rule());
    }
    for row in rows {
        lines.push(match row {
            Row::Cells(cells) => line(cells, None, None),
            Row::Picked(cells, style) => line(cells, Some(*style), None),
            Row::Live {
                cells,
                span,
                status,
                frame,
            } => line(cells, None, Some((span, status, *frame))),
            Row::Rule => rule(),
        });
    }
    lines
}

/// `── title ─────…` across `width`.
fn section_rule(title: &str, width: usize, theme: &Theme) -> Line<'static> {
    let faint = Style::default().fg(theme.colors.faint);
    let title = fit(title, width.saturating_sub(4));
    let rest = width.saturating_sub(display_width(&title) + 4);
    Line::from(vec![
        Span::styled("── ", faint),
        Span::styled(
            title,
            Style::default()
                .fg(theme.colors.text)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {}", "─".repeat(rest)), faint),
    ])
}

fn rule(width: usize, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        "─".repeat(width),
        Style::default().fg(theme.colors.faint),
    ))
}

/// The first line of `text`, cut to `max` characters.
fn headline(text: &str, max: usize) -> String {
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().count() <= max {
        return line.to_string();
    }
    let cut: String = line.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// Draw the BENCH tab in `area`, the chat panel below its header bar.
pub(super) fn render_benchmarks(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    theme: &Theme,
    is_focused: bool,
) {
    let block = body_block((is_focused, app.host_chrome()), theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    // Nothing of the conversation is on screen to click or select.
    app.set_chat_layout(Vec::new(), 0);
    app.set_chat_images(Vec::new());
    app.set_chat_width(inner.width);
    app.set_chat_text_area(inner);
    let padded = Rect {
        x: inner.x.saturating_add(1),
        width: inner.width.saturating_sub(2),
        ..inner
    };
    if padded.width < 12 || padded.height == 0 {
        return;
    }
    let width = usize::from(padded.width);
    let frame_no = theme.animate_spinners.then(|| app.animation_frame());
    let state = app.bench();
    // The detail's tables fit the width; wrapping would only break them.
    let wrap = !matches!(state.mode, BenchMode::Detail(_));
    let (lines, scroll) = match &state.mode {
        BenchMode::List => {
            let lines = list_lines(state, theme, width, frame_no);
            // Keep the selected row (two lines below the header) on screen.
            let row = state.selected + 2;
            let height = usize::from(padded.height);
            let scroll = row.saturating_sub(height.saturating_sub(1));
            (lines, u16::try_from(scroll).unwrap_or(u16::MAX))
        }
        BenchMode::Detail(_) => match state.shown() {
            Some(view) => {
                let lines = detail_lines(view, state.run, theme, width, frame_no);
                let last = u16::try_from(lines.len().saturating_sub(1)).unwrap_or(u16::MAX);
                (lines, state.scroll.min(last))
            }
            None => (vec![muted_line("That benchmark is gone.", theme)], 0),
        },
        BenchMode::Form => (form_lines(app, theme, width), 0),
    };
    app.clamp_bench_scroll(scroll);
    app.set_chat_rows(Some((inner, usize::from(scroll))));
    let mut paragraph = Paragraph::new(lines).scroll((scroll, 0));
    if wrap {
        paragraph = paragraph.wrap(Wrap { trim: false });
    }
    frame.render_widget(paragraph, padded);
}

fn muted_line<'a>(text: impl Into<String>, theme: &Theme) -> Line<'a> {
    Line::from(Span::styled(
        text.into(),
        Style::default().fg(theme.colors.muted),
    ))
}

/// One benchmark arm in a list row: `gate $0.06 2/3 pass`.
fn arm_brief(arm: &BenchArm) -> String {
    let mut text = format!(
        "{} {}",
        headline(short_model(&arm.label), 28),
        arm.median_cost_usd.map_or_else(|| "-".to_string(), money)
    );
    if arm.checks_run > 0 {
        text.push_str(&format!(" {}/{} pass", arm.checks_passed, arm.checks_run));
    }
    text
}

/// The list of benchmarks; `frame` lights the status of those running.
fn list_lines<'a>(
    state: &BenchState,
    theme: &Theme,
    width: usize,
    frame: Option<usize>,
) -> Vec<Line<'a>> {
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                "Benchmarks",
                Style::default()
                    .fg(theme.colors.text)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "   n new · Enter open · r refresh · c stop the running one · Esc chat",
                Style::default().fg(theme.colors.muted),
            ),
        ]),
        Line::default(),
    ];
    if state.benches.is_empty() {
        let text = if state.loaded() {
            "No benchmarks yet. Press n to run the same work (your last prompt, a new one, \
             or a whole earlier session) on two arms, Gate or a model each, and compare \
             what they cost and how well they did."
        } else {
            "Loading the benchmarks…"
        };
        lines.push(muted_line(text, theme));
        return lines;
    }
    for (i, view) in state.benches.iter().enumerate() {
        let selected = i == state.selected;
        let marker = if selected { "› " } else { "  " };
        let status_color = match view.status {
            BenchStatus::Running => theme.colors.pending,
            BenchStatus::Cancelled => theme.colors.warning,
            _ => theme.colors.success,
        };
        let text = format!(
            "\"{}\" · A {} · B {}",
            headline(&view.title, 40),
            arm_brief(&view.arms[0]),
            arm_brief(&view.arms[1]),
        );
        let mut style = Style::default().fg(theme.colors.text);
        if selected {
            style = style
                .bg(theme.colors.selection)
                .fg(theme.colors.on_selection);
        }
        let room = width.saturating_sub(view.name.chars().count() + 16);
        let status = view.status.label();
        let (status_style, light) = if selected {
            (style, theme.colors.on_selection)
        } else {
            (Style::default().fg(status_color), theme.colors.foreground)
        };
        let mut spans = vec![
            Span::styled(marker, style),
            Span::styled(
                format!("{} ", view.name),
                style.add_modifier(Modifier::BOLD),
            ),
        ];
        spans.extend(under_way(
            &status,
            status_style,
            frame.filter(|_| view.status == BenchStatus::Running),
            (theme.colors.muted, light),
        ));
        spans.push(Span::styled(
            " ".repeat(10usize.saturating_sub(display_width(&status))),
            status_style,
        ));
        spans.push(Span::styled(headline(&text, room.max(10)), style));
        lines.push(Line::from(spans));
    }
    lines
}

/// What an arm's runs of one example task did and their mean cost:
/// `✓` or `✗` for one checked run, `2/3` for more, `2×` for unchecked runs.
fn task_cells(cell: &BenchTaskCell, theme: &Theme) -> [Cell; 2] {
    let text = Style::default().fg(theme.colors.text);
    if cell.runs == 0 {
        return [(String::new(), text), ("-".to_string(), text)];
    }
    let cost = cell.mean_cost_usd.map_or_else(|| "-".to_string(), money);
    let check = match (cell.checked, cell.passed) {
        (0, _) if cell.runs == 1 => (String::new(), text),
        (0, _) => (format!("{}×", cell.runs), text),
        (1, 1) => ("✓".to_string(), Style::default().fg(theme.colors.success)),
        (1, _) => ("✗".to_string(), Style::default().fg(theme.colors.error)),
        (checked, passed) => (format!("{passed}/{checked}"), text),
    };
    [check, (cost, text)]
}

/// `setting up: 14/20 tasks ready · 3 fetching · ...` while a running
/// benchmark still fetches some example task's files.
fn setup_line(view: &BenchView) -> Option<String> {
    setup_counts(view).map(|counts| format!("setting up: {counts}"))
}

/// `14/20 tasks ready · 3 fetching · ...` while a running benchmark still
/// fetches some example task's files.
pub(super) fn setup_counts(view: &BenchView) -> Option<String> {
    let count = |f: fn(&BenchTaskSetup) -> bool| view.tasks.iter().filter(|t| f(&t.setup)).count();
    let fetching = count(|s| matches!(s, BenchTaskSetup::Fetching { .. }));
    let queued = count(|s| *s == BenchTaskSetup::Queued);
    if view.status != BenchStatus::Running || fetching + queued == 0 {
        return None;
    }
    let ready = count(|s| *s == BenchTaskSetup::Ready);
    let failed = count(|s| matches!(s, BenchTaskSetup::Failed(_)));
    let mut parts = vec![format!("{ready}/{} tasks ready", view.tasks.len())];
    if fetching > 0 {
        parts.push(format!("{fetching} fetching"));
    }
    if queued > 0 {
        parts.push(format!("{queued} queued"));
    }
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }
    if count(|s| *s == BenchTaskSetup::Fetching { pull: true }) > 0 {
        parts.push("pulling an image can take minutes".to_string());
    }
    Some(parts.join(" · "))
}

/// `$0.03 2✓ 1✗ 1 err`: mean turn cost, then turns whose test or build
/// command passed, failed, or that ended in an error.
fn turn_cell(turn: Option<&BenchTurn>) -> String {
    let Some(turn) = turn.filter(|t| t.runs > 0) else {
        return "-".to_string();
    };
    let mut text = turn.mean_cost_usd.map_or_else(|| "-".to_string(), money);
    for (n, what) in [
        (turn.passed, "✓"),
        (turn.failed, "✗"),
        (turn.errors, " err"),
    ] {
        if n > 0 {
            text.push_str(&format!(" {n}{what}"));
        }
    }
    text
}

/// What a run of arm A or B cost next to the session it replays.
#[allow(clippy::cast_precision_loss)]
fn ratio(arm: &BenchArm, baseline: f64) -> String {
    arm.median_cost_usd.filter(|_| baseline > 0.0).map_or_else(
        || "-".to_string(),
        |cost| format!("{:.2}×", cost / baseline),
    )
}

/// `A` or `B` for the arm the judge found better, `=` for a tie, `?` when
/// its verdict could not be read.
fn verdict_mark(verdict: &BenchVerdict) -> &'static str {
    match (verdict.parsed, verdict.winner) {
        (false, _) => "?",
        (true, Some(arm)) => arm_letter(arm),
        (true, None) => "=",
    }
}

/// `+`, `=` or `-` for a run rated better than, as good as, or worse than
/// what the user kept.
fn reference_mark(rating: Option<&str>) -> &'static str {
    match rating {
        Some("better") => "+",
        Some("as good") => "=",
        Some("worse") => "-",
        _ => "?",
    }
}

/// An arm's tokens per model: its ended runs' `done` and what its runs
/// under way, `live`, have used so far, most expensive first.
fn tokens_so_far<'a>(
    done: &[BenchTokens],
    live: impl Iterator<Item = &'a BenchLiveRun>,
) -> Vec<BenchTokens> {
    let mut rows = done.to_vec();
    for t in live.flat_map(|r| &r.tokens) {
        match rows.iter_mut().find(|row| row.model == t.model) {
            Some(row) => {
                row.calls += t.calls;
                row.input_tokens += t.input_tokens;
                row.output_tokens += t.output_tokens;
                row.cost_usd += t.cost_usd;
            }
            None => rows.push(t.clone()),
        }
    }
    rows.sort_by(|x, y| y.cost_usd.total_cmp(&x.cost_usd));
    rows
}

/// One benchmark's numbers; `frame` lights what is under way while it
/// runs.
#[allow(clippy::too_many_lines)]
fn detail_lines(
    view: &BenchView,
    run: Option<usize>,
    theme: &Theme,
    width: usize,
    frame: Option<usize>,
) -> Vec<Line<'static>> {
    let frame = frame.filter(|_| view.status == BenchStatus::Running);
    let text = Style::default().fg(theme.colors.text);
    let muted = Style::default().fg(theme.colors.muted);
    let good = Style::default().fg(theme.colors.success);
    let warning = Style::default().fg(theme.colors.warning);
    let bold = text.add_modifier(Modifier::BOLD);
    let cell = |s: String| (s, text);
    let [a, b] = &view.arms;
    let prefix = task_prefix(view);
    let prefix = prefix.as_deref();

    let status = view.status.label();
    let status_color = match view.status {
        BenchStatus::Running => theme.colors.pending,
        BenchStatus::Cancelled => theme.colors.warning,
        _ => theme.colors.success,
    };
    let room = width.saturating_sub(display_width(&view.name) + display_width(&status) + 4);
    let mut head = vec![
        Span::styled(view.name.clone(), bold),
        Span::styled(
            format!("  {}  ", fit(&headline(&view.title, usize::MAX), room)),
            text,
        ),
    ];
    head.extend(under_way(
        &status,
        Style::default().fg(status_color),
        frame,
        (theme.colors.muted, theme.colors.foreground),
    ));
    let mut lines = vec![Line::from(head)];
    let mut about = Vec::new();
    if view.tasks.is_empty() {
        about.push(view.source.clone());
    } else if let Some(prefix) = prefix {
        about.push(prefix.to_string());
    }
    about.push(format!(
        "{} run{} per {}arm",
        view.repeats,
        if view.repeats == 1 { "" } else { "s" },
        if view.tasks.is_empty() {
            ""
        } else {
            "task and "
        },
    ));
    about.push(match view.check.as_deref() {
        _ if !view.tasks.is_empty() => "own checks".to_string(),
        Some(c) => format!("check `{c}`"),
        None => "no check".to_string(),
    });
    if let Some(usd) = view.budget_usd {
        about.push(format!("budget {}", money(usd)));
    }
    lines.push(Line::from(Span::styled(
        fit(&about.join(" · "), width),
        muted,
    )));
    if let Some(setup) = setup_line(view) {
        lines.push(Line::from(Span::styled(
            fit(&setup, width),
            Style::default().fg(theme.colors.pending),
        )));
    }
    lines.push(Line::default());

    let pair = |f: &dyn Fn(&BenchArm) -> String| [f(a), f(b)];
    let none = || [String::new(), String::new()];
    let live_of = |arm: usize| view.live.iter().filter(move |r| r.arm.min(1) == arm);
    let finished = |arm: usize| {
        let done = &view.arms[arm];
        if view.status != BenchStatus::Running {
            return format!("{}/{}", done.finished, done.runs);
        }
        let per_arm = usize::try_from(view.repeats.max(1))
            .unwrap_or(usize::MAX)
            .saturating_mul(view.tasks.len().max(1));
        let planned = per_arm.max(done.runs + live_of(arm).count());
        let working = live_of(arm).filter(|r| r.wall_ms.is_some()).count();
        let mut text = format!("{}/{planned}", done.finished);
        if working > 0 {
            text.push_str(&format!(", {working} working"));
        }
        text
    };
    let spent =
        |arm: usize| view.arms[arm].total_cost_usd + live_of(arm).map(|r| r.cost_usd).sum::<f64>();
    let mut summary = Vec::new();
    let mut add = |label: &str, [x, y]: [String; 2], [dx, dy]: [String; 2]| {
        summary.push(Row::Cells(vec![
            (label.to_string(), muted),
            cell(x),
            (dx, good),
            cell(y),
            (dy, good),
        ]));
    };
    add("finished", [finished(0), finished(1)], none());
    add(
        "time",
        pair(&|arm| arm.median_wall_ms.map_or_else(|| "-".to_string(), duration)),
        none(),
    );
    add(
        "cost",
        [money(spent(0)), money(spent(1))],
        saving(Some(spent(0)), Some(spent(1))),
    );
    add(
        "cost/run",
        pair(&|arm| arm.median_cost_usd.map_or_else(|| "-".to_string(), money)),
        saving(a.median_cost_usd, b.median_cost_usd),
    );
    if let Some(baseline) = view.baseline.as_ref().filter(|b| b.priced) {
        add(
            "vs session",
            pair(&|arm| ratio(arm, baseline.cost_usd)),
            none(),
        );
    }
    add(
        "checks",
        pair(&|arm| {
            if arm.checks_run == 0 {
                "-".to_string()
            } else {
                format!("{}/{}", arm.checks_passed, arm.checks_run)
            }
        }),
        none(),
    );
    if a.checks_run + b.checks_run > 0 {
        let per_fix = |arm: &BenchArm| cost_per_fix(arm.total_cost_usd, arm.checks_passed);
        add(
            "$/fix",
            pair(&|arm| per_fix(arm).map_or_else(|| "no fix".to_string(), money)),
            saving(per_fix(a), per_fix(b)),
        );
    }
    if a.escalated + b.escalated > 0 {
        add(
            "moved up",
            pair(&|arm| format!("{}/{}", arm.escalated, arm.runs)),
            none(),
        );
    }
    if a.nudged + b.nudged > 0 {
        add(
            "nudged",
            pair(&|arm| format!("{}/{}", arm.nudged, arm.runs)),
            none(),
        );
    }
    if view.judge.is_some() {
        add(
            "judge W-L-T",
            pair(&|arm| format!("{}-{}-{}", arm.judge_wins, arm.judge_losses, arm.judge_ties)),
            none(),
        );
    }
    let rated = |arm: &BenchArm| arm.ref_better + arm.ref_as_good + arm.ref_worse;
    if rated(a) + rated(b) > 0 {
        add(
            "vs yours +/=/-",
            pair(&|arm| format!("{}/{}/{}", arm.ref_better, arm.ref_as_good, arm.ref_worse)),
            none(),
        );
    }
    let a_head = format!("A {}", fit(short_model(&a.label), 24));
    let b_head = format!("B {}", fit(short_model(&b.label), 24));
    lines.extend(table(
        &[Col::L, Col::L.sep(), Col::L, Col::L.sep(), Col::L],
        Some(&["", &a_head, "", &b_head, ""]),
        &summary,
        width,
        theme,
    ));
    if let Some(baseline) = &view.baseline {
        let models: Vec<&str> = baseline.models.iter().map(|m| short_model(m)).collect();
        let about = format!(
            "session: {} call{}, {}{}",
            baseline.calls,
            if baseline.calls == 1 { "" } else { "s" },
            if baseline.priced {
                money(baseline.cost_usd)
            } else {
                "no price".to_string()
            },
            if models.is_empty() {
                String::new()
            } else {
                format!(" on {}", models.join(", "))
            },
        );
        lines.push(Line::from(Span::styled(fit(&about, width), muted)));
    }
    if view.has_reference && view.judge.is_some() {
        lines.push(Line::from(Span::styled(
            fit(
                "vs yours: the judge rated each run against the diff and reply the session ended with",
                width,
            ),
            muted,
        )));
    }

    if !view.tasks.is_empty() {
        let task_of = |name: &str| {
            view.runs
                .iter()
                .find(|r| r.name == name)
                .and_then(|r| r.task.as_deref())
        };
        let judged = |id: &str| -> String {
            view.judge
                .iter()
                .flat_map(|j| &j.verdicts)
                .filter(|v| task_of(&v.a_run) == Some(id))
                .map(verdict_mark)
                .collect()
        };
        let rows: Vec<Row> = view
            .tasks
            .iter()
            .map(|task| {
                let [da, db] = saving(task.arms[0].mean_cost_usd, task.arms[1].mean_cost_usd);
                let [check_a, cost_a] = task_cells(&task.arms[0], theme);
                let [check_b, cost_b] = task_cells(&task.arms[1], theme);
                let last = match &task.setup {
                    BenchTaskSetup::Ready => (judged(&task.id), bold),
                    BenchTaskSetup::Failed(_) => {
                        (task.setup.label(), Style::default().fg(theme.colors.error))
                    }
                    setup => (setup.label(), Style::default().fg(theme.colors.pending)),
                };
                Row::Cells(vec![
                    cell(task_name(&task.id, prefix).to_string()),
                    check_a,
                    cost_a,
                    (da, good),
                    check_b,
                    cost_b,
                    (db, good),
                    last,
                ])
            })
            .collect();
        lines.push(Line::default());
        lines.push(section_rule("tasks", width, theme));
        lines.extend(table(
            &[
                Col::L.flex(),
                Col::L.sep(),
                Col::R,
                Col::L,
                Col::L.sep(),
                Col::R,
                Col::L,
                Col::L.sep(),
            ],
            Some(&[
                "task",
                "A",
                "",
                "",
                "B",
                "",
                "",
                if view.judge.is_some() { "judge" } else { "" },
            ]),
            &rows,
            width,
            theme,
        ));
    }

    if view.prompts.len() > 1 {
        let rows: Vec<Row> = view
            .prompts
            .iter()
            .enumerate()
            .map(|(i, prompt)| {
                Row::Cells(vec![
                    ((i + 1).to_string(), muted),
                    cell(headline(prompt, usize::MAX)),
                    cell(turn_cell(a.turns.get(i))),
                    cell(turn_cell(b.turns.get(i))),
                ])
            })
            .collect();
        lines.push(Line::default());
        lines.push(section_rule(
            "prompts · mean turn cost, test or build ✓ ✗",
            width,
            theme,
        ));
        lines.extend(table(
            &[Col::R, Col::L.flex(), Col::L.sep(), Col::L.sep()],
            Some(&["#", "prompt", "A", "B"]),
            &rows,
            width,
            theme,
        ));
    }

    let mut rows = Vec::new();
    for (i, arm) in view.arms.iter().enumerate() {
        if i > 0 {
            rows.push(Row::Rule);
        }
        let used = tokens_so_far(&arm.tokens, live_of(i));
        if used.is_empty() {
            rows.push(Row::Cells(vec![
                cell(arm_letter(i).to_string()),
                ("no model calls".to_string(), muted),
            ]));
        }
        for (n, t) in used.iter().enumerate() {
            rows.push(Row::Cells(vec![
                cell(if n == 0 { arm_letter(i) } else { "" }.to_string()),
                cell(short_model(&t.model).to_string()),
                cell(t.calls.to_string()),
                cell(tokens(t.input_tokens)),
                cell(tokens(t.output_tokens)),
                cell(money(t.cost_usd)),
            ]));
        }
    }
    lines.push(Line::default());
    lines.push(section_rule("tokens", width, theme));
    lines.extend(table(
        &[Col::L, Col::L.flex(), Col::R.sep(), Col::R, Col::R, Col::R],
        Some(&["", "model", "calls", "in", "out", "cost"]),
        &rows,
        width,
        theme,
    ));

    if let Some(judge) = &view.judge {
        let rows: Vec<Row> = judge
            .verdicts
            .iter()
            .map(|v| {
                let reference = match (&v.a_vs_reference, &v.b_vs_reference) {
                    (None, None) => String::new(),
                    (x, y) => format!(
                        "A{} B{}",
                        reference_mark(x.as_deref()),
                        reference_mark(y.as_deref())
                    ),
                };
                Row::Cells(vec![
                    cell(format!("{} {}", v.a_run, v.b_run)),
                    (verdict_mark(v).to_string(), bold),
                    cell(reference),
                    (headline(&v.reasoning, usize::MAX), muted),
                ])
            })
            .collect();
        lines.push(Line::default());
        lines.push(section_rule(
            &format!(
                "judge {} · {} blind call{} · {}, not in arm costs",
                short_model(&judge.model),
                judge.verdicts.len(),
                if judge.verdicts.len() == 1 { "" } else { "s" },
                money(judge.cost_usd)
            ),
            width,
            theme,
        ));
        lines.extend(table(
            &[Col::L, Col::L, Col::L, Col::L.flex()],
            None,
            &rows,
            width,
            theme,
        ));
    }

    let has_task =
        view.runs.iter().any(|r| r.task.is_some()) || view.live.iter().any(|r| r.task.is_some());
    let any_moved = view.runs.iter().any(|r| r.escalated_to.is_some());
    let any_led = view.runs.iter().any(|r| r.led);
    let header = [
        "",
        "run",
        "model",
        "classified",
        "difficulty",
        if any_moved { "moved up" } else { "" },
        if any_led { "led" } else { "" },
        if has_task { "task" } else { "" },
        "time",
        "cost",
        "check",
        "files",
        "",
    ];
    let mut rows = vec![Row::Cells(
        header.iter().map(|h| ((*h).to_string(), muted)).collect(),
    )];
    let picked = Style::default()
        .bg(theme.colors.selection)
        .fg(theme.colors.on_selection);
    enum Entry {
        Ended(usize),
        Live(usize),
    }
    let mut entries: Vec<((u32, usize), Entry)> = view
        .runs
        .iter()
        .enumerate()
        .map(|(i, r)| (run_order(&r.name, r.arm), Entry::Ended(i)))
        .chain(
            view.live
                .iter()
                .enumerate()
                .map(|(i, live)| (run_order(&live.name, live.arm), Entry::Live(i))),
        )
        .collect();
    entries.sort_by_key(|(order, _)| *order);
    let pending = Style::default().fg(theme.colors.pending);
    let mut number = None;
    for ((n, _), entry) in entries {
        if number.is_some_and(|last| last != n) {
            rows.push(Row::Rule);
        }
        number = Some(n);
        let i = match entry {
            Entry::Ended(i) => i,
            Entry::Live(i) => {
                rows.push(live_row(&view.live[i], view, prefix, frame, text, pending));
                continue;
            }
        };
        let r = &view.runs[i];
        let selected = run == Some(i);
        let base = if r.error.is_some() { warning } else { text };
        let check = match r.check_passed {
            Some(true) => ("✓", good),
            Some(false) => ("✗", Style::default().fg(theme.colors.error)),
            None => ("", base),
        };
        let cells = vec![
            ((if selected { "›" } else { " " }).to_string(), base),
            (r.name.clone(), base),
            (short_model(&r.model).to_string(), base),
            (category(r), base),
            (r.difficulty.clone().unwrap_or_default(), base),
            (
                if any_moved {
                    moved_up(r)
                } else {
                    String::new()
                },
                base,
            ),
            (if any_led { led(r) } else { String::new() }, base),
            (
                r.task
                    .as_deref()
                    .map_or("", |t| task_name(t, prefix))
                    .to_string(),
                base,
            ),
            (duration(r.wall_ms), base),
            (money(r.cost_usd), base),
            (check.0.to_string(), check.1),
            (r.files_changed.to_string(), base),
            (
                r.error
                    .as_deref()
                    .map_or_else(String::new, |e| headline(e, usize::MAX)),
                warning,
            ),
        ];
        rows.push(if selected {
            Row::Picked(cells, picked)
        } else {
            Row::Cells(cells)
        });
    }
    lines.push(Line::default());
    lines.push(section_rule("runs", width, theme));
    lines.extend(table(
        &[
            Col::L,
            Col::L,
            Col::L.flex(),
            Col::L.flex(),
            Col::L.flex(),
            Col::L.flex(),
            Col::L.flex(),
            Col::L.flex(),
            Col::R,
            Col::R,
            Col::L,
            Col::R,
            Col::L.flex(),
        ],
        None,
        &rows,
        width,
        theme,
    ));
    for note in &view.notes {
        lines.push(Line::from(Span::styled(
            fit(&format!("note: {note}"), width),
            warning,
        )));
    }
    lines.push(rule(width, theme));
    let running = view.status == BenchStatus::Running;
    let keys = if view.status.has_copies() && !running && !view.tasks.is_empty() {
        "←/→ run · L log · d diff · x remove copies · Esc back"
    } else if view.status.has_copies() && !running {
        "←/→ run · L log · d diff · y apply · x remove copies · Esc back"
    } else if running {
        "↑/↓ scroll · L log · c stop · Esc back"
    } else {
        "↑/↓ scroll · L log · Esc back"
    };
    lines.push(muted_line(fit(keys, width), theme));
    lines
}

/// The row of `live`, a run of `view` under way, lit by `frame` while it
/// works.
fn live_row(
    live: &BenchLiveRun,
    view: &BenchView,
    prefix: Option<&str>,
    frame: Option<usize>,
    text: Style,
    pending: Style,
) -> Row {
    let arm = &view.arms[live.arm.min(1)];
    let empty = || (String::new(), text);
    let cells = vec![
        (" ".to_string(), text),
        (live.name.clone(), text),
        (short_model(&arm.label).to_string(), text),
        empty(),
        empty(),
        empty(),
        empty(),
        (
            live.task
                .as_deref()
                .map_or("", |t| task_name(t, prefix))
                .to_string(),
            text,
        ),
        (live.wall_ms.map_or_else(String::new, duration), text),
        (money(live.cost_usd), text),
    ];
    let status = match (&live.doing, live.wall_ms) {
        (_, None) => "waiting for its task's files".to_string(),
        (Some(doing), Some(_)) => doing_text(doing),
        (None, Some(_)) => "working".to_string(),
    };
    Row::Live {
        cells,
        span: LIVE_SPAN,
        status: (status, pending),
        frame: frame.filter(|_| live.wall_ms.is_some()),
    }
}

/// The columns of the runs table a run under way shows what it is doing
/// across: classified, difficulty, moved up and led, which it has no
/// values for yet.
const LIVE_SPAN: Range<usize> = 3..7;

/// How an arm's value reads in the form.
fn arm_reads(value: &str, arm_b: bool) -> String {
    match value {
        "" if arm_b => "the strongest Opus (default)".to_string(),
        "gate" => "gate · Gate routes each turn".to_string(),
        "gate:nodelegate" => "gate:nodelegate · Gate without enforced delegation".to_string(),
        model => model.to_string(),
    }
}

fn form_lines<'a>(app: &App, theme: &Theme, width: usize) -> Vec<Line<'a>> {
    let state = app.bench();
    let form = &state.form;
    let bold = Style::default()
        .fg(theme.colors.text)
        .add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(theme.colors.muted);
    let mut lines = vec![
        Line::from(vec![
            Span::styled("New benchmark", bold),
            Span::styled(
                "   ↑/↓ row · ←/→ change · text rows take what you type below, then Enter · Esc back",
                muted,
            ),
        ]),
        Line::default(),
    ];
    let typed_prompt = matches!(state.work(), WorkChoice::Prompt);
    let examples = matches!(state.work(), WorkChoice::Examples(_));
    for field in FormField::ALL {
        let focused = field == form.field;
        let (value, faint) = match field {
            FormField::Work => (
                match state.work() {
                    WorkChoice::LastPrompt => "your last prompt".to_string(),
                    WorkChoice::Prompt => "a prompt typed below".to_string(),
                    WorkChoice::Examples(None) => {
                        format!(
                            "example coding tasks, every suite ({})",
                            state.examples.len()
                        )
                    }
                    WorkChoice::Examples(Some(suite)) => format!(
                        "example coding tasks: {suite} ({})",
                        state.examples_of(Some(suite)).len()
                    ),
                    WorkChoice::Session(session) => format!(
                        "replay {}session \"{}\" · {} prompt{}{}",
                        if session.current { "this " } else { "the " },
                        headline(&session.title, 40),
                        session.prompts,
                        if session.prompts == 1 { "" } else { "s" },
                        session
                            .cost_usd
                            .map_or_else(String::new, |usd| format!(" · cost {}", money(usd))),
                    ),
                },
                false,
            ),
            FormField::Prompt if !typed_prompt => {
                ("(only when the work is a typed prompt)".to_string(), true)
            }
            FormField::Prompt if form.prompt.is_empty() => {
                ("type it in the input line, then Enter".to_string(), true)
            }
            FormField::Prompt => (headline(&form.prompt, width.saturating_sub(24)), false),
            FormField::Tasks if !examples => {
                ("(only when the work is example tasks)".to_string(), true)
            }
            FormField::Tasks => {
                let total = state.example_count();
                let chosen = match state.work() {
                    WorkChoice::Examples(suite) => state.examples_of(suite),
                    _ => Vec::new(),
                };
                let names = chosen
                    .iter()
                    .take(form.tasks)
                    .map(|e| e.title.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                (
                    format!(
                        "{} of {total} · {}",
                        form.tasks.min(total),
                        headline(&names, width.saturating_sub(36))
                    ),
                    false,
                )
            }
            FormField::A => (arm_reads(&form.a, false), false),
            FormField::B => (arm_reads(&form.b, true), false),
            FormField::Repeats if examples => (format!("{} per task", form.repeats), false),
            FormField::Repeats => (form.repeats.to_string(), false),
            FormField::Check if examples => ("each task's own".to_string(), true),
            FormField::Check if form.check.is_empty() => (
                "the project's (AURA_CHECK_COMMAND or detected)".to_string(),
                true,
            ),
            FormField::Check => (format!("`{}`", form.check), false),
            FormField::Budget if form.budget.is_empty() => ("none".to_string(), true),
            FormField::Budget => (format!("${}", form.budget), false),
            FormField::Go => (
                if form.waiting {
                    "Estimating…".to_string()
                } else if form.ready() {
                    "[ Start ]  Enter starts it".to_string()
                } else {
                    "[ Estimate ]  Enter shows what it should cost".to_string()
                },
                false,
            ),
        };
        let cycles = matches!(
            field,
            FormField::Work | FormField::A | FormField::B | FormField::Repeats
        ) || (field == FormField::Tasks && examples);
        let value = if focused && cycles {
            format!("‹ {value} ›")
        } else {
            value
        };
        let mut style = if faint {
            muted
        } else {
            Style::default().fg(theme.colors.text)
        };
        if focused {
            style = style
                .bg(theme.colors.selection)
                .fg(theme.colors.on_selection);
        }
        if field == FormField::Go {
            lines.push(Line::default());
        }
        lines.push(Line::from(vec![
            Span::styled(if focused { "› " } else { "  " }, style),
            Span::styled(format!("{:<14}", field.label()), muted),
            Span::styled(value, style),
        ]));
    }
    match &form.estimate {
        Some(Ok(text)) => {
            lines.push(Line::default());
            lines.extend(text.lines().map(|l| {
                Line::from(Span::styled(
                    l.to_string(),
                    Style::default().fg(theme.colors.text),
                ))
            }));
        }
        Some(Err(problem)) => {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(
                problem.clone(),
                Style::default().fg(theme.colors.error),
            )));
        }
        None => {}
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{BenchBaseline, BenchJudge, BenchRun, UiCommand};
    use ratatui::{backend::TestBackend, Terminal};

    fn arm(label: &str, cost: f64) -> BenchArm {
        BenchArm {
            label: label.to_string(),
            runs: 2,
            finished: 2,
            total_cost_usd: cost * 2.0,
            median_cost_usd: Some(cost),
            median_wall_ms: Some(42_000),
            checks_passed: 1,
            checks_run: 2,
            judge_wins: 1,
            ref_better: 1,
            ref_as_good: 1,
            turns: vec![
                BenchTurn {
                    runs: 2,
                    mean_cost_usd: Some(cost / 2.0),
                    passed: 2,
                    ..BenchTurn::default()
                },
                BenchTurn {
                    runs: 2,
                    mean_cost_usd: Some(cost / 2.0),
                    failed: 1,
                    ..BenchTurn::default()
                },
            ],
            ..BenchArm::default()
        }
    }

    fn view() -> BenchView {
        BenchView {
            id: "ab".to_string(),
            name: "Compare #1".to_string(),
            title: "fix the parser".to_string(),
            source: "session \"fix the parser\"".to_string(),
            status: BenchStatus::Done,
            repeats: 2,
            prompts: vec!["fix the parser".to_string(), "now the docs".to_string()],
            arms: [arm("gate", 0.06), arm("claude-opus-4-7", 0.40)],
            runs: vec![BenchRun {
                name: "a-1".to_string(),
                model: "claude-sonnet-4-6".to_string(),
                wall_ms: 42_000,
                cost_usd: 0.06,
                check_passed: Some(true),
                files_changed: 2,
                ..BenchRun::default()
            }],
            judge: Some(BenchJudge {
                model: "opus".to_string(),
                cost_usd: 0.03,
                verdicts: vec![BenchVerdict {
                    a_run: "a-1".to_string(),
                    b_run: "b-1".to_string(),
                    parsed: true,
                    winner: Some(1),
                    a_vs_reference: Some("as good".to_string()),
                    b_vs_reference: Some("better".to_string()),
                    reasoning: "B also fixed the docs".to_string(),
                }],
            }),
            baseline: Some(BenchBaseline {
                calls: 12,
                cost_usd: 0.20,
                priced: true,
                models: vec!["claude-opus-4-7".to_string()],
            }),
            has_reference: true,
            ..BenchView::default()
        }
    }

    fn text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_detail_shows_cost_quality_and_each_prompt() {
        let theme = Theme::default();
        let shown = text(&detail_lines(&view(), Some(0), &theme, 120, None));
        let row = |label: &str| {
            shown
                .lines()
                .find(|l| l.starts_with(&format!("{label} ")))
                .unwrap_or_else(|| panic!("no {label} row in\n{shown}"))
                .to_string()
        };
        assert!(
            row("").contains("A gate") && row("").contains("B opus-4-7"),
            "{shown}"
        );
        for (label, a, b) in [
            ("cost", "$0.12", "$0.80"),
            ("cost/run", "$0.06", "$0.40"),
            ("$/fix", "$0.12", "$0.80"),
        ] {
            let line = row(label);
            assert!(
                line.contains(a) && line.contains("-85%") && line.trim_end().ends_with(b),
                "{line}"
            );
        }
        assert!(row("vs session").contains("0.30×"), "{shown}");
        assert!(row("vs session").contains("2.00×"), "{shown}");
        assert!(row("vs yours +/=/-").contains("1/1/0"), "{shown}");
        assert!(shown.contains("$0.03 2✓"), "{shown}");
        assert!(shown.contains("$0.03 1✗"), "{shown}");
        assert!(
            shown.contains("a-1 b-1  B  A= B+  B also fixed the docs"),
            "{shown}"
        );
        assert!(
            shown.contains("session: 12 calls, $0.20 on opus-4-7"),
            "{shown}"
        );
        assert!(shown.contains("›  a-1  sonnet-4-6"), "{shown}");
        assert!(
            !shown.lines().any(|l| l.starts_with("moved up")),
            "no summary row when nothing moved up: {shown}"
        );
    }

    #[test]
    fn a_run_shows_how_it_was_classified_and_where_it_moved_up() {
        let theme = Theme::default();
        let mut view = view();
        view.runs[0].escalated_to = Some("claude-opus-4-7".to_string());
        view.runs[0].category = Some("multi_file_change".to_string());
        view.runs[0].difficulty = Some("hard".to_string());
        view.runs[0].led = true;
        view.runs[0].subtask_cost_usd = 0.26;
        view.runs.push(BenchRun {
            name: "b-1".to_string(),
            arm: 1,
            model: "claude-opus-4-7".to_string(),
            category: Some("small_change".to_string()),
            ..BenchRun::default()
        });
        view.arms[0].escalated = 1;
        view.arms[0].nudged = 1;
        let shown = text(&detail_lines(&view, None, &theme, 120, None));
        let run = |name: &str| {
            shown
                .lines()
                .skip_while(|l| !l.contains("── runs"))
                .find(|l| l.split_whitespace().next() == Some(name))
                .unwrap_or_else(|| panic!("no {name} run in\n{shown}"))
                .to_string()
        };
        let moved = run("a-1");
        assert!(moved.contains("sonnet-4-6"), "{shown}");
        let words: Vec<&str> = moved.split_whitespace().collect();
        assert!(
            words.windows(2).any(|w| w == ["multi-file", "hard"]),
            "{shown}"
        );
        let header = shown
            .lines()
            .skip_while(|l| !l.contains("── runs"))
            .nth(1)
            .unwrap_or_default();
        let heads: Vec<&str> = header.split_whitespace().collect();
        assert!(
            heads.windows(2).any(|w| w == ["classified", "difficulty"]),
            "{shown}"
        );
        assert!(moved.contains("to opus-4-7"), "{shown}");
        assert!(moved.contains("yes, $0.26 in subtasks"), "{shown}");
        let stayed = run("b-1");
        assert!(stayed.contains("small change"), "{shown}");
        assert_eq!(
            stayed.split_whitespace().filter(|w| *w == "no").count(),
            2,
            "neither moved up nor led: {shown}"
        );
        assert!(shown.contains("classified"), "{shown}");
        let row = |label: &str| {
            shown
                .lines()
                .find(|l| l.starts_with(label))
                .unwrap_or_else(|| panic!("no {label} row in\n{shown}"))
                .to_string()
        };
        assert!(row("moved up").contains("1/"), "{shown}");
        assert!(row("nudged").contains("1/"), "{shown}");
    }

    #[test]
    fn a_move_that_only_raised_the_effort_shows_as_moved_up() {
        let theme = Theme::default();
        let mut view = view();
        view.runs[0].model = "claude/claude-opus-5-5".to_string();
        view.runs[0].escalated_to = Some("claude/claude-opus-5-5".to_string());
        view.runs[0].escalated_effort = Some("max".to_string());
        let shown = text(&detail_lines(&view, None, &theme, 160, None));
        let moved = shown
            .lines()
            .skip_while(|l| !l.contains("── runs"))
            .find(|l| l.split_whitespace().next() == Some("a-1"))
            .unwrap_or_else(|| panic!("no a-1 run in\n{shown}"));
        assert!(moved.contains("to opus-5-5 max"), "{shown}");
    }

    #[test]
    fn moved_up_and_led_show_only_when_a_run_did_either() {
        let theme = Theme::default();
        let mut view = view();
        view.runs[0].category = Some("small_change".to_string());
        view.runs[0].difficulty = Some("medium".to_string());
        let shown = text(&detail_lines(&view, None, &theme, 120, None));
        let header = shown
            .lines()
            .skip_while(|l| !l.contains("── runs"))
            .nth(1)
            .unwrap_or_default();
        assert!(header.contains("difficulty"), "{shown}");
        assert!(
            !header.contains("moved up") && !header.contains("led"),
            "{header}"
        );
        let run = shown
            .lines()
            .skip_while(|l| !l.contains("── runs"))
            .find(|l| l.split_whitespace().next() == Some("a-1"))
            .unwrap_or_default();
        assert!(run.contains("small change  medium"), "{shown}");
        assert!(!run.split_whitespace().any(|w| w == "no"), "{run}");
    }

    #[test]
    fn a_run_under_way_shows_what_it_does_across_its_empty_columns() {
        let theme = Theme::default();
        let mut view = view();
        view.status = BenchStatus::Running;
        view.runs[0].task = Some("ansible-b748edea".to_string());
        view.runs[0].category = Some("small_change".to_string());
        view.runs[0].difficulty = Some("medium".to_string());
        view.live = vec![BenchLiveRun {
            name: "a-2".to_string(),
            task: Some("ansible-40ade1f8".to_string()),
            wall_ms: Some(301_000),
            cost_usd: 0.80,
            doing: Some(BenchDoing {
                what: "writing 49 files".to_string(),
                for_ms: 3_000,
                ..BenchDoing::default()
            }),
            ..BenchLiveRun::default()
        }];
        let shown = text(&detail_lines(&view, None, &theme, 100, None));
        let live = shown
            .lines()
            .find(|l| l.split_whitespace().next() == Some("a-2"))
            .unwrap_or_else(|| panic!("no a-2 run in\n{shown}"));
        let status = live
            .find("writing 49 files")
            .unwrap_or_else(|| panic!("{live}"));
        let task = live
            .find("ansible-40ade1f8")
            .unwrap_or_else(|| panic!("{live}"));
        assert!(status < task, "the status comes before the task: {live}");
        assert!(live.trim_end().ends_with("$0.80"), "{live}");
        assert!(shown.contains("ansible-b748edea"), "{shown}");
    }

    #[test]
    fn every_line_fits_the_width_and_separators_line_up() {
        let theme = Theme::default();
        let mut view = view();
        view.arms[1].label = "anthropic/claude-opus-5-5-with-a-very-long-name".to_string();
        view.prompts[0] = "a long prompt ".repeat(20);
        view.runs[0].model = "claude/claude-sonnet-5-5".to_string();
        view.runs[0].task = Some("swe_bench_pro_hard/ansible-40ade1f8-and-more".to_string());
        view.runs[0].error = Some("the copy could not be made ".repeat(10));
        view.notes = vec!["a note ".repeat(30)];
        view.live = vec![BenchLiveRun {
            name: "a-2".to_string(),
            task: Some("swe_bench_pro_hard/ansible-b748edea".to_string()),
            wall_ms: Some(5_000),
            doing: Some(BenchDoing {
                what: "writing a very long list of files ".repeat(4),
                for_ms: 3_000,
                ..BenchDoing::default()
            }),
            ..BenchLiveRun::default()
        }];
        if let Some(judge) = view.judge.as_mut() {
            judge.verdicts[0].reasoning = "b-1 is the better attempt ".repeat(20);
        }
        for width in [60, 80, 120] {
            let lines = detail_lines(&view, Some(0), &theme, width, None);
            let shown = text(&lines);
            for line in shown.lines() {
                assert!(display_width(line) <= width, "{width}: {line}");
            }
            let bars = |line: &str| -> Vec<usize> {
                line.chars()
                    .enumerate()
                    .filter(|(_, c)| matches!(c, '│' | '┼'))
                    .map(|(i, _)| i)
                    .collect()
            };
            let summary: Vec<_> = shown
                .lines()
                .skip(2)
                .take_while(|l| !l.starts_with("session:"))
                .filter(|l| l.contains('│') || l.contains('┼'))
                .map(bars)
                .collect();
            assert!(summary.len() > 3, "{shown}");
            assert!(summary.windows(2).all(|w| w[0] == w[1]), "{shown}");
        }
    }

    #[test]
    fn the_tab_draws_its_list_and_opens_a_benchmark() {
        let mut app = App::new();
        app.process_command(UiCommand::BenchList {
            agent_id: String::new(),
            root: String::new(),
            benches: vec![view()],
        });
        let theme = Theme::default();
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal
            .draw(|frame| render_benchmarks(frame, frame.area(), &mut app, &theme, true))
            .unwrap();
        let screen = format!("{:?}", terminal.backend().buffer());
        assert!(screen.contains("Compare #1"), "{screen}");
        assert!(screen.contains("A gate $0.06 1/2 pass"), "{screen}");
    }

    #[test]
    fn example_tasks_show_a_row_each_and_no_apply() {
        use crate::events::{BenchTaskCell, BenchTaskRow};
        let theme = Theme::default();
        let mut view = view();
        view.source = "2 example tasks".to_string();
        view.prompts = Vec::new();
        view.baseline = None;
        view.tasks = vec![
            BenchTaskRow {
                id: "coding/write-file".to_string(),
                title: "Write a file".to_string(),
                arms: [
                    BenchTaskCell {
                        runs: 2,
                        passed: 2,
                        checked: 2,
                        mean_cost_usd: Some(0.02),
                    },
                    BenchTaskCell {
                        runs: 2,
                        passed: 1,
                        checked: 2,
                        mean_cost_usd: Some(0.30),
                    },
                ],
                ..BenchTaskRow::default()
            },
            BenchTaskRow {
                id: "coding/fix-shell-bug".to_string(),
                ..BenchTaskRow::default()
            },
        ];
        view.runs[0].task = Some("coding/write-file".to_string());
        let shown = text(&detail_lines(&view, None, &theme, 120, None));
        assert!(
            shown.contains("coding · 2 runs per task and arm · own checks"),
            "{shown}"
        );
        assert!(!shown.contains("setting up"), "{shown}");
        assert!(shown.contains("── tasks ─"), "{shown}");
        let write = shown
            .lines()
            .find(|l| l.starts_with("write-file "))
            .unwrap_or_else(|| panic!("{shown}"));
        assert!(write.contains("2/2  $0.02  -93%"), "{write}");
        assert!(write.contains("1/2  $0.30"), "{write}");
        assert!(write.trim_end().ends_with('B'), "{write}");
        assert!(
            shown.lines().any(|l| l.starts_with("fix-shell-bug ")),
            "{shown}"
        );
        assert!(!shown.contains("coding/"), "{shown}");
        assert!(!shown.contains("y apply"), "{shown}");
        assert!(!shown.contains("prompts"), "{shown}");
    }

    #[test]
    fn a_running_benchmark_shows_its_setup_and_the_runs_under_way() {
        use crate::events::BenchTaskRow;
        let theme = Theme::default();
        let mut view = view();
        view.status = BenchStatus::Running;
        view.judge = None;
        let task = |id: &str, setup| BenchTaskRow {
            id: format!("swe/{id}"),
            setup,
            ..BenchTaskRow::default()
        };
        view.tasks = vec![
            task("ansible-1", BenchTaskSetup::Ready),
            task("flipt-2", BenchTaskSetup::Fetching { pull: true }),
            task("flipt-3", BenchTaskSetup::Queued),
            task(
                "navidrome-4",
                BenchTaskSetup::Failed("no image".to_string()),
            ),
        ];
        view.live = vec![
            BenchLiveRun {
                name: "a-1".to_string(),
                task: Some("swe/ansible-1".to_string()),
                wall_ms: Some(72_000),
                cost_usd: 0.04,
                tokens: vec![BenchTokens {
                    model: "claude-sonnet-4-6".to_string(),
                    calls: 3,
                    input_tokens: 42_000,
                    output_tokens: 1_500,
                    cost_usd: 0.04,
                }],
                ..BenchLiveRun::default()
            },
            BenchLiveRun {
                name: "b-2".to_string(),
                arm: 1,
                task: Some("swe/flipt-2".to_string()),
                ..BenchLiveRun::default()
            },
        ];
        let shown = text(&detail_lines(&view, None, &theme, 140, Some(3)));
        assert!(
            shown.contains(
                "setting up: 1/4 tasks ready · 1 fetching · 1 queued · 1 failed · pulling an image can take minutes"
            ),
            "{shown}"
        );
        let row = |start: &str| {
            shown
                .lines()
                .find(|l| l.starts_with(start))
                .unwrap_or_else(|| panic!("{start}: {shown}"))
                .trim_end()
                .to_string()
        };
        assert!(row("flipt-2 ").ends_with("pulling image"), "{shown}");
        assert!(row("flipt-3 ").ends_with("queued"), "{shown}");
        assert!(row("navidrome-4 ").ends_with("failed: no image"), "{shown}");
        let live = |status: &str| {
            shown
                .lines()
                .skip_while(|l| !l.contains("── runs"))
                .find(|l| l.contains(status))
                .unwrap_or_else(|| panic!("{status}: {shown}"))
                .to_string()
        };
        let working = live("working");
        assert!(working.contains(" a-1 "), "{working}");
        assert!(working.contains("ansible-1"), "{working}");
        assert!(working.contains("1m12s"), "{working}");
        assert!(working.contains("$0.04"), "{working}");
        assert!(
            live("waiting for its task's files").contains(" b-2 "),
            "{shown}"
        );
        let finished = row("finished ");
        assert!(finished.contains("2/8, 1 working"), "{finished}");
        assert!(finished.ends_with("2/8"), "{finished}");
        assert!(row("cost ").contains("$0.16"), "spent so far: {shown}");
        assert!(row("A  sonnet-4-6 ").contains("42k"), "{shown}");

        view.live[0].doing = Some(BenchDoing {
            what: "running task".to_string(),
            for_ms: 492_000,
            subtask: Some(Box::new(BenchDoing {
                what: "thinking".to_string(),
                for_ms: 100_000,
                call: 4,
                restarts: 2,
                subtask: None,
            })),
            ..BenchDoing::default()
        });
        let shown = text(&detail_lines(&view, None, &theme, 200, Some(3)));
        let a_1 = shown
            .lines()
            .find(|l| l.contains(" a-1 ") && l.contains("ansible-1"))
            .unwrap_or_else(|| panic!("{shown}"));
        assert!(
            a_1.contains("running task 8m12s › thinking 1m40s · call 4 · restarted 2×"),
            "{shown}"
        );
    }

    #[test]
    fn runs_show_in_number_order_with_a_rule_between_numbers() {
        let theme = Theme::default();
        let mut view = view();
        view.status = BenchStatus::Running;
        view.runs[0].name = "b-1".to_string();
        view.runs[0].arm = 1;
        let live = |name: &str, arm| BenchLiveRun {
            name: name.to_string(),
            arm,
            wall_ms: Some(1_000),
            ..BenchLiveRun::default()
        };
        view.live = vec![live("b-2", 1), live("a-1", 0), live("a-2", 0)];
        let shown = text(&detail_lines(&view, None, &theme, 140, None));
        let runs: Vec<&str> = shown
            .lines()
            .skip_while(|l| !l.starts_with("── runs"))
            .skip(1)
            .take_while(|l| !l.starts_with("note:"))
            .collect();
        let named: Vec<&str> = runs
            .iter()
            .filter_map(|l| {
                ["a-1", "b-1", "a-2", "b-2"]
                    .into_iter()
                    .find(|name| l.contains(&format!(" {name} ")))
            })
            .collect();
        assert_eq!(named, ["a-1", "b-1", "a-2", "b-2"], "{shown}");
        let at = |name: &str| {
            runs.iter()
                .position(|l| l.contains(&format!(" {name} ")))
                .unwrap()
        };
        let between = |from: &str, to: &str| {
            runs[at(from) + 1..at(to)]
                .iter()
                .filter(|l| l.trim().chars().all(|c| c == '─') && !l.trim().is_empty())
                .count()
        };
        assert_eq!(between("a-1", "b-1"), 0, "{shown}");
        assert_eq!(between("b-1", "a-2"), 1, "{shown}");
        assert_eq!(between("a-2", "b-2"), 0, "{shown}");
    }

    #[test]
    fn runs_under_way_add_to_tokens_used_so_far() {
        let token = |model: &str, cost: f64| BenchTokens {
            model: model.to_string(),
            calls: 1,
            input_tokens: 1_000,
            output_tokens: 100,
            cost_usd: cost,
        };
        let live = [
            BenchLiveRun {
                tokens: vec![token("opus", 0.5), token("haiku", 0.01)],
                ..BenchLiveRun::default()
            },
            BenchLiveRun {
                tokens: vec![token("sonnet", 0.2)],
                ..BenchLiveRun::default()
            },
        ];
        let used = tokens_so_far(&[token("sonnet", 0.1)], live.iter());
        let models: Vec<&str> = used.iter().map(|t| t.model.as_str()).collect();
        assert_eq!(models, ["opus", "sonnet", "haiku"]);
        assert_eq!(used[1].calls, 2);
        assert!((used[1].cost_usd - 0.3).abs() < 1e-9);
    }

    #[test]
    fn running_and_working_shimmer_while_it_runs() {
        let theme = Theme::default();
        let mut view = view();
        view.status = BenchStatus::Running;
        view.live = vec![
            BenchLiveRun {
                name: "a-2".to_string(),
                wall_ms: Some(5_000),
                ..BenchLiveRun::default()
            },
            BenchLiveRun {
                name: "b-2".to_string(),
                arm: 1,
                ..BenchLiveRun::default()
            },
        ];
        // Lit when the word's letters are in more than one color.
        let lit = |line: &Line<'_>, word: &str| {
            let mut colors: Vec<_> = line
                .spans
                .iter()
                .filter(|s| !s.content.trim().is_empty() && word.contains(s.content.trim()))
                .map(|s| format!("{:?}", s.style.fg))
                .collect();
            colors.dedup();
            colors.len() > 1
        };
        let find = |lines: &[Line<'static>], status: &str| {
            let runs = lines
                .iter()
                .position(|l| text(std::slice::from_ref(l)).contains("── runs"))
                .unwrap_or_default();
            lines[runs..]
                .iter()
                .find(|l| text(std::slice::from_ref(*l)).contains(status))
                .cloned()
                .unwrap_or_else(|| panic!("{status}: {}", text(lines)))
        };

        // On frame 3 the band's center is on the label's first letter.
        let lines = detail_lines(&view, None, &theme, 140, Some(3));
        assert!(lit(&lines[0], "running"), "{:?}", lines[0]);
        assert!(lit(&find(&lines, "working"), "working"));
        let waiting = find(&lines, "waiting for its task's files");
        assert!(
            !lit(&waiting, "waiting for its task's files"),
            "{waiting:?}"
        );

        let still = detail_lines(&view, None, &theme, 140, None);
        assert!(!lit(&still[0], "running"), "{:?}", still[0]);
        assert!(!lit(&find(&still, "working"), "working"));

        view.status = BenchStatus::Done;
        let done = detail_lines(&view, None, &theme, 140, Some(3));
        assert!(!lit(&done[0], "done"), "{:?}", done[0]);
    }

    #[test]
    fn the_form_offers_the_example_tasks_and_how_many() {
        let theme = Theme::default();
        let mut app = App::new();
        let state = app.bench_mut();
        state.examples = ["Write a file", "Fix a bug"]
            .map(|title| aura_ui::benchmarks::BenchExample {
                suite: "coding".to_string(),
                title: title.to_string(),
            })
            .to_vec();
        state.form.tasks = 2;
        state.form.work = 2;
        state.form.field = FormField::Tasks;
        let shown = text(&form_lines(&app, &theme, 120));
        assert!(
            shown.contains("example coding tasks, every suite (2)"),
            "{shown}"
        );
        assert!(
            shown.contains("‹ 2 of 2 · Write a file, Fix a bug ›"),
            "{shown}"
        );
        assert!(shown.contains("3 per task"), "{shown}");
        assert!(shown.contains("each task's own"), "{shown}");

        app.bench_mut().form.work = 0;
        let shown = text(&form_lines(&app, &theme, 120));
        assert!(
            shown.contains("(only when the work is example tasks)"),
            "{shown}"
        );
    }
}
