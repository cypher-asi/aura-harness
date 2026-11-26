//! Draw what BENCH shows in the chat panel: one benchmark's numbers side
//! by side, or the form that sets one up. Its list is in the agents panel
//! (`agents::bench`).

use super::chrome::body_block;
use super::text::display_width;
use crate::app::{
    BenchMode, FormField, WorkChoice, AUTO_CLASSIFIER, BY_DIFFICULTY, DEFAULT_AGAINST,
};
use crate::events::{
    arm_letter, cost_per_fix, run_order, unpriced_note, BenchArm, BenchBaseline, BenchCost,
    BenchDoing, BenchLiveRun, BenchRun, BenchStatus, BenchTaskCell, BenchTaskSetup, BenchTokens,
    BenchTurn, BenchVerdict, BenchView, UNPRICED_MARK,
};
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
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

/// `cost` as [`money`], marked `+?` when some of its calls had no known
/// price and `unknown` when none had one; `-` for no cost at all.
fn cost_text(cost: Option<BenchCost>) -> String {
    cost.map_or_else(|| "-".to_string(), |cost| cost.read(money))
}

/// `850ms`, `42s`, `3m05s`, `1h05m`.
fn duration(ms: u64) -> String {
    if ms < 1_000 {
        return format!("{ms}ms");
    }
    let secs = ms / 1_000;
    if secs < 60 {
        return format!("{secs}s");
    }
    if secs < 3_600 {
        return format!("{}m{:02}s", secs / 60, secs % 60);
    }
    format!("{}h{:02}m", secs / 3_600, secs % 3_600 / 60)
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
/// An arm that runs work on a model per difficulty is `by difficulty`.
pub(super) fn short_model(model: &str) -> &str {
    if model.starts_with("difficulty:") {
        return BY_DIFFICULTY;
    }
    let name = model.rsplit('/').next().unwrap_or(model);
    name.strip_prefix("claude-").unwrap_or(name)
}

/// `multi-file`: what the classifier said a run's first prompt asks for;
/// empty when nothing was recorded.
fn category(run: &BenchRun) -> String {
    category_name(run.category.as_deref())
}

/// `multi-file` for `multi_file_change`, as stored; empty for `None`.
fn category_name(category: Option<&str>) -> String {
    category
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
/// the work to subtasks, else `no`. The subtasks' cost is marked as the
/// run's is when some of its calls had no known price.
fn led(run: &BenchRun) -> String {
    if run.led {
        let subtasks = BenchCost::new(run.subtask_cost_usd, run.unpriced_calls);
        format!("yes, {} in subtasks", subtasks.read(money))
    } else {
        "no".to_string()
    }
}

/// `-74%` for the cheaper of two costs, nothing for the other or when
/// they are about the same, either is unknown, or either leaves out
/// calls with no known price.
fn saving(a: Option<BenchCost>, b: Option<BenchCost>) -> [String; 2] {
    let (Some(a), Some(b)) = (a.and_then(BenchCost::known), b.and_then(BenchCost::known)) else {
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
    /// Cells of something under way up to column `status_at`, with
    /// `status` drawn from there in the room left on the line. The status
    /// never widens a column, so the table holds still as it changes. On a
    /// frame, a band of light sweeps across the status.
    Live {
        cells: Vec<Cell>,
        status_at: usize,
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
    type Status<'r> = (usize, &'r Cell, Option<usize>);
    let line = |cells: &[Cell], picked: Option<Style>, live: Option<Status<'_>>| {
        let cols_in: Vec<usize> = shown
            .iter()
            .copied()
            .filter(|&i| live.is_none_or(|(at, ..)| i < at))
            .collect();
        let filled = |i: usize| cells.get(i).is_some_and(|(text, _)| !text.is_empty());
        let padded = picked.is_some() || live.is_some();
        let last = if padded {
            cols_in.last().copied()
        } else {
            cols_in.iter().rev().find(|&&i| filled(i)).copied()
        };
        let mut spans = Vec::new();
        for (n, &i) in cols_in.iter().enumerate() {
            if last.is_some_and(|last| i > last) {
                break;
            }
            if n > 0 {
                let (text, style) = if cols[i].sep {
                    (" │ ", faint)
                } else {
                    ("  ", Style::default())
                };
                spans.push(Span::styled(text, picked.unwrap_or(style)));
            }
            let (text, style) = cells.get(i).cloned().unwrap_or_default();
            let text = fit(&text, widths[i]);
            let pad = " ".repeat(widths[i] - display_width(&text));
            let text = if cols[i].right {
                pad + &text
            } else if Some(i) == last && !padded {
                text
            } else {
                text + &pad
            };
            let style = picked.map_or(style, |p| style.patch(p));
            spans.push(Span::styled(text, style));
        }
        if let Some((_, (text, style), frame)) = live.filter(|(_, (text, _), _)| !text.is_empty()) {
            let gap = if cols_in.is_empty() { 0 } else { 2 };
            let room = width.saturating_sub(across(&cols_in, &widths) + gap);
            if room > 0 {
                spans.push(Span::raw(" ".repeat(gap)));
                let text = fit(text, room);
                match frame {
                    Some(frame) => spans.extend(under_way(&text, *style, Some(frame), shine)),
                    None => spans.push(Span::styled(text, *style)),
                }
            }
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
                status_at,
                status,
                frame,
            } => line(cells, None, Some((*status_at, status, *frame))),
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

/// Draw what BENCH shows in `area`, the chat panel below its header bar.
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
    app.set_chat_code_copies(Vec::new());
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
    // The form wraps; the detail's tables fit the width, and wrapping
    // would only break them.
    let wrap = state.mode == Some(BenchMode::Form);
    let mut form_rows = None;
    let (lines, scroll) = if wrap {
        let (lines, rows) = form_layout(app, theme, width);
        form_rows = Some(rows);
        (lines, 0)
    } else {
        match state.shown() {
            Some(view) => {
                let lines = detail_lines(view, state.run, theme, width, frame_no);
                let last = u16::try_from(lines.len().saturating_sub(1)).unwrap_or(u16::MAX);
                (lines, state.scroll.min(last))
            }
            None => (vec![muted_line("That benchmark is gone.", theme)], 0),
        }
    };
    // The form is drawn from its top, so a row's line is its screen row.
    let form_rows: Vec<(u16, FormField)> = form_rows
        .unwrap_or_default()
        .into_iter()
        .filter(|(line, _)| *line < usize::from(padded.height))
        .filter_map(|(line, field)| Some((padded.y.checked_add(u16::try_from(line).ok()?)?, field)))
        .collect();
    let form_area = (!form_rows.is_empty()).then_some(padded);
    app.set_bench_form_rows(form_area, form_rows);
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

/// What an arm's runs of one example task did and their mean cost:
/// `✓` or `✗` for one checked run, `2/3` for more, `2×` for unchecked runs.
fn task_cells(cell: &BenchTaskCell, theme: &Theme) -> [Cell; 2] {
    let text = Style::default().fg(theme.colors.text);
    if cell.runs == 0 {
        return [(String::new(), text), ("-".to_string(), text)];
    }
    let cost = cost_text(cell.mean_cost());
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
    let mut text = cost_text(turn.mean_cost());
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

/// What a run of arm A or B cost next to the session it replays, marked
/// `+?` when the run's cost leaves out calls with no known price.
#[allow(clippy::cast_precision_loss)]
fn ratio(arm: &BenchArm, baseline: f64) -> String {
    arm.median_cost().filter(|_| baseline > 0.0).map_or_else(
        || "-".to_string(),
        |cost| {
            let mark = if cost.is_complete() {
                ""
            } else {
                UNPRICED_MARK
            };
            format!("{:.2}×{mark}", cost.usd / baseline)
        },
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

/// An arm's tokens per model and purpose: its ended runs' `done` and
/// what its runs under way, `live`, have used so far, most expensive
/// first.
fn tokens_so_far<'a>(
    done: &[BenchTokens],
    live: impl Iterator<Item = &'a BenchLiveRun>,
) -> Vec<BenchTokens> {
    let mut rows = done.to_vec();
    for t in live.flat_map(|r| &r.tokens) {
        match (rows.iter_mut()).find(|row| row.model == t.model && row.purpose == t.purpose) {
            Some(row) => {
                row.calls += t.calls;
                row.input_tokens += t.input_tokens;
                row.output_tokens += t.output_tokens;
                row.cost_usd += t.cost_usd;
                row.unpriced_calls += t.unpriced_calls;
            }
            None => rows.push(t.clone()),
        }
    }
    rows.sort_by(|x, y| y.cost_usd.total_cmp(&x.cost_usd));
    rows
}

/// `gpt-6.1-terra · criteria` for calls made beside the runs' own work,
/// `sonnet-5-5` for the work's.
fn tokens_label(t: &BenchTokens) -> String {
    let model = short_model(&t.model);
    match &t.purpose {
        Some(purpose) => format!("{model} · {purpose}"),
        None => model.to_string(),
    }
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
    if !view.classifier.is_empty() {
        about.push(format!("classifier {}", view.classifier));
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
    let spent = |arm: usize| {
        let live = live_of(arm).map(BenchLiveRun::cost);
        live.fold(view.arms[arm].total_cost(), |sum, cost| {
            BenchCost::new(sum.usd + cost.usd, sum.unpriced_calls + cost.unpriced_calls)
        })
    };
    let worked = |arm: usize| {
        let ended: u64 = view
            .runs
            .iter()
            .filter(|r| r.arm.min(1) == arm)
            .map(|r| r.wall_ms)
            .sum();
        let live: u64 = live_of(arm).filter_map(|r| r.wall_ms).sum();
        Some(ended + live)
            .filter(|&ms| ms > 0)
            .map_or_else(|| "-".to_string(), duration)
    };
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
    add("total time", [worked(0), worked(1)], none());
    add(
        "cost",
        [spent(0).read(money), spent(1).read(money)],
        saving(Some(spent(0)), Some(spent(1))),
    );
    add(
        "cost/run",
        pair(&|arm| cost_text(arm.median_cost())),
        saving(a.median_cost(), b.median_cost()),
    );
    // A ratio against a session some of whose calls had no known price
    // would read as if the session were cheaper than it was.
    let baseline_usd = (view.baseline.as_ref())
        .and_then(BenchBaseline::cost)
        .and_then(BenchCost::known);
    if let Some(baseline_usd) = baseline_usd {
        add("vs session", pair(&|arm| ratio(arm, baseline_usd)), none());
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
        let per_fix = |arm: &BenchArm| cost_per_fix(arm.total_cost(), arm.checks_passed);
        add(
            "$/fix",
            pair(&|arm| per_fix(arm).map_or_else(|| "no fix".to_string(), |cost| cost.read(money))),
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
    let unpriced: Vec<BenchTokens> = (0..2)
        .flat_map(|arm| tokens_so_far(&view.arms[arm].tokens, live_of(arm)))
        .filter(|t| t.unpriced_calls > 0)
        .collect();
    if let Some(note) = unpriced_note(unpriced.iter().map(|t| short_model(&t.model))) {
        lines.push(Line::from(Span::styled(fit(&note, width), muted)));
    }
    if let Some(baseline) = &view.baseline {
        let models: Vec<&str> = baseline.models.iter().map(|m| short_model(m)).collect();
        let about = format!(
            "session: {} call{}, {}{}",
            baseline.calls,
            if baseline.calls == 1 { "" } else { "s" },
            baseline
                .cost()
                .map_or_else(|| "no price".to_string(), |cost| cost.read(money)),
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
                let [da, db] = saving(task.arms[0].mean_cost(), task.arms[1].mean_cost());
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
                cell(tokens_label(t)),
                cell(t.calls.to_string()),
                cell(tokens(t.input_tokens)),
                cell(tokens(t.output_tokens)),
                cell(t.cost().read(money)),
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

    if !view.classify.is_empty() {
        let calls: u32 = view.classify.iter().map(|t| t.calls).sum();
        let cost = BenchCost::new(
            view.classify.iter().map(|t| t.cost_usd).sum(),
            view.classify.iter().map(|t| t.unpriced_calls).sum(),
        );
        let models: Vec<&str> = view
            .classify
            .iter()
            .map(|t| short_model(&t.model))
            .collect();
        lines.push(Line::default());
        lines.push(section_rule(
            &format!(
                "classifier {} · {calls} call{} · {}, not in arm costs",
                models.join(", "),
                if calls == 1 { "" } else { "s" },
                cost.read(money)
            ),
            width,
            theme,
        ));
    }

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
                judge.cost().read(money)
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
    let any_drivers = view.runs.iter().any(|r| r.drivers.is_some());
    let header = [
        "",
        "run",
        "model",
        "classified",
        "difficulty",
        if any_drivers { "why" } else { "" },
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
            (r.drivers.clone().unwrap_or_default(), muted),
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
            (r.cost().read(money), base),
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
        "←/→ run · L log · d diff · x remove copies · Esc list"
    } else if view.status.has_copies() && !running {
        "←/→ run · L log · d diff · y apply · x remove copies · Esc list"
    } else if running {
        "↑/↓ scroll · L log · c stop · Esc list"
    } else {
        "↑/↓ scroll · L log · Esc list"
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
    let model = live.model.as_deref().unwrap_or(&arm.label);
    let cells = vec![
        (" ".to_string(), text),
        (live.name.clone(), text),
        (short_model(model).to_string(), text),
        (category_name(live.category.as_deref()), text),
        (live.difficulty.clone().unwrap_or_default(), text),
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
        (live.cost().read(money), text),
    ];
    let status = match (&live.doing, live.wall_ms) {
        (_, None) => "waiting for its task's files".to_string(),
        (Some(doing), Some(_)) => doing_text(doing),
        (None, Some(_)) => "working".to_string(),
    };
    Row::Live {
        cells,
        status_at: LIVE_STATUS_AT,
        status: (status, pending),
        frame: frame.filter(|_| live.wall_ms.is_some()),
    }
}

/// The column of the runs table a run under way shows what it is doing
/// from: the last, after `files`, where an ended run shows its error.
const LIVE_STATUS_AT: usize = 13;

/// How an arm's value reads in the form.
fn arm_reads(value: &str, arm_b: bool) -> String {
    match value {
        "" if arm_b => "the strongest Opus".to_string(),
        DEFAULT_AGAINST if arm_b => "Opus 5.5 (default)".to_string(),
        "gate" => "gate · Gate routes each turn".to_string(),
        "gate:nodelegate" => "gate:nodelegate · Gate without enforced delegation".to_string(),
        BY_DIFFICULTY => "by difficulty · a model for easy, medium and hard work".to_string(),
        model => model.to_string(),
    }
}

/// How a difficulty's row reads: its model and who serves it, or how to
/// pick one. Returns whether it reads faint.
fn difficulty_reads(app: &App, model: &str) -> (String, bool) {
    if model.is_empty() {
        return (
            "Enter opens the model selector · ←/→ cycles".to_string(),
            true,
        );
    }
    let group = app
        .models()
        .iter()
        .find(|m| m.id == model)
        .map(|m| m.group.as_str())
        .filter(|group| !group.is_empty());
    match group {
        Some(group) => (format!("{model} · {group}"), false),
        None => (model.to_string(), false),
    }
}

#[cfg(test)]
fn form_lines<'a>(app: &App, theme: &Theme, width: usize) -> Vec<Line<'a>> {
    form_layout(app, theme, width).0
}

/// The form's lines, and the line each of its rows is on. Every line
/// down to the last row fits `width`, so none wraps and the rows keep
/// their lines on screen.
fn form_layout<'a>(
    app: &App,
    theme: &Theme,
    width: usize,
) -> (Vec<Line<'a>>, Vec<(usize, FormField)>) {
    let state = app.bench();
    let form = &state.form;
    let bold = Style::default()
        .fg(theme.colors.text)
        .add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(theme.colors.muted);
    const TITLE: &str = "New benchmark";
    let hint = fit(
        "   ↑/↓ row · ←/→ change · click or Enter a model row to pick it · text rows take what \
         you type below, then Enter · Esc drops it",
        width.saturating_sub(TITLE.len()),
    );
    let mut lines = vec![
        Line::from(vec![Span::styled(TITLE, bold), Span::styled(hint, muted)]),
        Line::default(),
    ];
    let mut rows = Vec::with_capacity(FormField::ALL.len());
    let typed_prompt = matches!(state.work(), WorkChoice::Prompt);
    let examples = matches!(state.work(), WorkChoice::Examples(_));
    for field in FormField::ALL {
        if !form.shown(field) {
            continue;
        }
        let focused = field == form.field;
        let (value, faint) = match field {
            FormField::From => match app.bench_from_label() {
                Some(label) => (label, false),
                None if app.bench_from_choices().is_empty() => (
                    "none yet: make an agent in PROJECTS first".to_string(),
                    true,
                ),
                None => ("←/→ picks the agent it runs from".to_string(), true),
            },
            FormField::AEasy
            | FormField::AMedium
            | FormField::AHard
            | FormField::BEasy
            | FormField::BMedium
            | FormField::BHard => {
                difficulty_reads(app, form.difficulty_model(field).unwrap_or_default())
            }
            FormField::Classifier if form.classifier_model().is_none() => (
                format!("{AUTO_CLASSIFIER} · Gate's choice of fast model · Enter picks another"),
                true,
            ),
            FormField::Classifier => difficulty_reads(app, &form.classifier),
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
                        session.cost_usd.map_or_else(String::new, |usd| {
                            let cost = BenchCost::new(usd, session.unpriced_calls);
                            format!(" · cost {}", cost.read(money))
                        }),
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
            FormField::From | FormField::Work | FormField::A | FormField::B | FormField::Repeats
        ) || field.difficulty().is_some()
            || (field == FormField::Tasks && examples);
        // One screen line per row, so a click finds the row it is on:
        // the marker and label take 16 columns, `‹ ›` 4 more.
        let room = width.saturating_sub(if focused && cycles { 20 } else { 16 });
        let value = fit(&value, room);
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
        rows.push((lines.len(), field));
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
    (lines, rows)
}

#[cfg(test)]
mod tests;
