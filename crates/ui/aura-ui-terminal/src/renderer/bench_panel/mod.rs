//! The benchmark progress panel, between the Chat body and the input in
//! the same column of borders, above the queued-messages panel:
//!
//! ```text
//! ├──────────────────────────────────────────────────────────┤
//! │ COMPARE #2 ×   COMPARE #3                             —  │
//! │ ├ Work     5 example tasks · swe_bench_pro_hard          │
//! │ ├ Arms     A gate  vs  B opus-5-5                        │
//! │ ├ Setup    3/5 tasks ready · 1 fetching                  │
//! │ └ Spent    A $0.42 · B $1.10                             │
//! │ Running · 4 working · 11 of 30 done · 1 failed           │
//! │ ██████████▓▒▒▒▒░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  36%   │
//! ```
//!
//! A tab for each benchmark, the picked one lit with its `×`; what it
//! runs; what it is doing, lit by a band of light while it goes on; and a
//! bar of its runs, ended ones in the color of how they ended. Folded, it
//! keeps the tabs and the bar.

use super::benchmarks::{fit, money, setup_counts, short_model, task_prefix, under_way};
use super::chrome::border_color;
use super::input::join_divider;
use super::text::display_width;
use crate::app::bench_progress::{BenchPanelAreas, BenchTabArea};
use crate::app::PanelFocus;
use crate::events::{BenchCost, BenchLiveRun, BenchPhase, BenchView};
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Rows the border adds: only the bottom one, since the Chat body's
/// bottom border sits on top.
const BORDER_ROWS: u16 = 1;
/// Columns a fact's label takes.
const LABEL_WIDTH: usize = 7;
/// Columns between two tabs.
const TAB_GAP: u16 = 3;

/// Rows the panel wants: none without a benchmark to show, the tabs and
/// the bar when folded, else the tabs, the facts, the status and the bar.
#[must_use]
pub(super) fn height(app: &App) -> u16 {
    let Some(view) = app.bench_panel_shown() else {
        return 0;
    };
    if app.bench_panel_collapsed() {
        return BORDER_ROWS + 2;
    }
    let facts = u16::try_from(facts(view).len()).unwrap_or(u16::MAX);
    BORDER_ROWS + 3 + facts
}

/// Render the panel into `area`, which [`height`] sized, and record where
/// its parts went.
pub(super) fn render(frame: &mut Frame, area: Rect, app: &mut App, theme: &Theme) {
    let Some(view) = app.bench_panel_shown().cloned() else {
        app.set_bench_panel_areas(BenchPanelAreas::default());
        return;
    };
    if area.height < BORDER_ROWS + 1 || area.width < 16 {
        app.set_bench_panel_areas(BenchPanelAreas::default());
        return;
    }
    let tabs: Vec<(String, String)> = app
        .bench_panel_views()
        .iter()
        .map(|b| (b.id.clone(), b.name.clone()))
        .collect();
    let progress = view.progress();
    let lit = theme
        .animate_spinners
        .then(|| app.animation_frame())
        .filter(|_| progress.phase.under_way());

    let inner = draw_border(frame, area, app.focus() == PanelFocus::Chat, theme);

    let header = Rect { height: 1, ..inner };
    let (line, tab_areas, toggle) =
        header_line(&tabs, &view.id, header, app.bench_panel_collapsed(), theme);
    frame.render_widget(Paragraph::new(line), header);

    let body = Rect {
        x: inner.x.saturating_add(1),
        y: inner.y.saturating_add(1),
        width: inner.width.saturating_sub(2),
        height: inner.height.saturating_sub(1),
    };
    let lines = body_lines(&view, body, app.bench_panel_collapsed(), lit, theme);
    frame.render_widget(Paragraph::new(lines), body);

    app.set_bench_panel_areas(BenchPanelAreas {
        tabs: tab_areas,
        toggle: Some(toggle),
        panel: Some(area),
    });
}

/// Clear `area` and draw its border, joined to the Chat body's above;
/// returns what is inside.
fn draw_border(frame: &mut Frame, area: Rect, focused: bool, theme: &Theme) -> Rect {
    let block = Block::default()
        .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
        .border_style(Style::default().fg(border_color(focused, theme)));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    join_divider(frame, area);
    inner
}

/// The facts, the status and the bar, or only the bar when `collapsed`,
/// cut to the rows of `body`.
fn body_lines(
    view: &BenchView,
    body: Rect,
    collapsed: bool,
    lit: Option<usize>,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let width = usize::from(body.width);
    let mut lines = Vec::new();
    if !collapsed {
        lines.extend(fact_lines(view, width, theme));
        lines.push(status_line(view, width, lit, theme));
    }
    lines.push(bar_line(view, width, theme));
    // Short of rows, the facts give way before the bar.
    while lines.len() > usize::from(body.height) && lines.len() > 1 {
        lines.remove(lines.len() - 2);
    }
    lines
}

/// ` COMPARE #2 ×   COMPARE #3        — `: a tab per benchmark, the
/// picked one lit with its `×`, tabs past the width counted, and the
/// toggle at the right.
fn header_line(
    tabs: &[(String, String)],
    picked: &str,
    row: Rect,
    collapsed: bool,
    theme: &Theme,
) -> (Line<'static>, Vec<BenchTabArea>, Rect) {
    let muted = Style::default().fg(theme.colors.muted);
    let toggle = Rect {
        x: row.right().saturating_sub(3),
        width: 3.min(row.width),
        ..row
    };
    let mut spans = vec![Span::raw(" ")];
    let mut areas = Vec::new();
    let mut x = row.x.saturating_add(1);
    for (shown, (id, name)) in tabs.iter().enumerate() {
        let label = name.to_uppercase();
        let is_picked = id == picked;
        let label_width = u16::try_from(display_width(&label)).unwrap_or(u16::MAX);
        let width = label_width + if is_picked { 2 } else { 0 };
        let gap = if shown == 0 { 0 } else { TAB_GAP };
        let rest = tabs.len() - shown;
        let more = format!("+{rest}");
        let more_width = u16::try_from(more.len()).unwrap_or(u16::MAX) + TAB_GAP;
        let room_after = if rest > 1 { more_width } else { 0 };
        if x + gap + width + room_after > toggle.x {
            if shown > 0 {
                spans.push(Span::styled(
                    format!("{}{more}", " ".repeat(usize::from(TAB_GAP))),
                    muted,
                ));
            }
            break;
        }
        spans.push(Span::raw(" ".repeat(usize::from(gap))));
        x += gap;
        let tab = Rect { x, width, ..row };
        push_tab(&mut spans, label, is_picked, theme);
        areas.push(BenchTabArea {
            id: id.clone(),
            tab,
            close: is_picked.then(|| Rect {
                x: x + label_width,
                width: 2,
                ..row
            }),
        });
        x += width;
    }
    push_toggle(&mut spans, toggle.x.saturating_sub(row.x), collapsed, muted);
    (Line::from(spans), areas, toggle)
}

/// A tab's label, lit and followed by its `×` when it is the picked one.
fn push_tab(spans: &mut Vec<Span<'static>>, label: String, is_picked: bool, theme: &Theme) {
    if is_picked {
        spans.push(Span::styled(
            label,
            Style::default()
                .fg(theme.colors.success)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        ));
        spans.push(Span::styled(
            " ×",
            Style::default().fg(theme.colors.success),
        ));
    } else {
        spans.push(Span::styled(label, Style::default().fg(theme.colors.muted)));
    }
}

/// Blank up to `before` columns, then the toggle: `+` to unfold, `—` to fold.
fn push_toggle(spans: &mut Vec<Span<'static>>, before: u16, collapsed: bool, muted: Style) {
    let used: usize = spans.iter().map(|s| display_width(&s.content)).sum();
    let fill = usize::from(before).saturating_sub(used);
    spans.push(Span::raw(" ".repeat(fill)));
    spans.push(Span::styled(
        format!(" {} ", if collapsed { "+" } else { "—" }),
        muted,
    ));
}

/// What a benchmark runs, as `(label, value)` rows: its work, its arms,
/// where its tasks' files stand while they are fetched, and what each arm
/// has spent.
fn facts(view: &BenchView) -> Vec<(&'static str, String)> {
    let mut work = view.source.clone();
    if let Some(prefix) = task_prefix(view) {
        work.push_str(&format!(" · {prefix}"));
    } else if view.tasks.is_empty() && !view.source.starts_with("session") {
        work.push_str(&format!(" \"{}\"", view.title.lines().next().unwrap_or("")));
    }
    let [a, b] = &view.arms;
    let mut rows = vec![
        ("Work", work),
        (
            "Arms",
            format!(
                "A {}  vs  B {}",
                short_model(&a.label),
                short_model(&b.label)
            ),
        ),
    ];
    if let Some(setup) = setup_counts(view) {
        rows.push(("Setup", setup));
    }
    let spent = |arm: usize| {
        let live = view
            .live
            .iter()
            .filter(|r| r.arm.min(1) == arm)
            .map(BenchLiveRun::cost);
        live.fold(view.arms[arm].total_cost(), |sum, cost| {
            BenchCost::new(sum.usd + cost.usd, sum.unpriced_calls + cost.unpriced_calls)
        })
        .read(money)
    };
    rows.push(("Spent", format!("A {} · B {}", spent(0), spent(1))));
    rows
}

/// The facts as a branch: `├ Work  …` down to `└ Spent  …`.
fn fact_lines(view: &BenchView, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let rows = facts(view);
    let last = rows.len().saturating_sub(1);
    let room = width.saturating_sub(LABEL_WIDTH + 3);
    rows.into_iter()
        .enumerate()
        .map(|(i, (label, value))| {
            Line::from(vec![
                Span::styled(
                    if i == last { "└ " } else { "├ " },
                    Style::default().fg(theme.colors.faint),
                ),
                Span::styled(
                    format!("{label:<LABEL_WIDTH$} "),
                    Style::default().fg(theme.colors.text),
                ),
                Span::styled(fit(&value, room), Style::default().fg(theme.colors.success)),
            ])
        })
        .collect()
}

/// `Running · 4 working · 11 of 30 done · 1 failed`, the phase lit on
/// frame `lit` while it goes on.
fn status_line(view: &BenchView, width: usize, lit: Option<usize>, theme: &Theme) -> Line<'static> {
    let progress = view.progress();
    let muted = Style::default().fg(theme.colors.muted);
    let phase = progress.phase.label();
    let mut parts = Vec::new();
    if progress.working > 0 {
        parts.push(format!("{} working", progress.working));
    }
    parts.push(format!("{} of {} done", progress.done, progress.total));
    if progress.failed > 0 {
        parts.push(format!("{} failed", progress.failed));
    }
    let detail = fit(
        &format!(" · {}", parts.join(" · ")),
        width.saturating_sub(display_width(phase)),
    );
    let color = match progress.phase {
        BenchPhase::Stopped | BenchPhase::Done => theme.colors.success,
        _ => theme.colors.pending,
    };
    let mut spans = under_way(
        phase,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
        lit,
        (theme.colors.muted, theme.colors.foreground),
    );
    spans.push(Span::styled(detail, muted));
    Line::from(spans)
}

/// A cell of the bar.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cell {
    Passed,
    /// Its check failed
    Failed,
    /// It ended in an error
    Errored,
    Working,
    Left,
}

/// `█████▒▒░░░░░░  36%`: across the width, the runs that ended in the
/// order they ended and in the color of how, then those working, then
/// those still to come.
fn bar_line(view: &BenchView, width: usize, theme: &Theme) -> Line<'static> {
    let progress = view.progress();
    let percent = (progress.done * 100)
        .checked_div(progress.total)
        .unwrap_or(0)
        .min(100);
    let label = format!("  {percent:>3}%");
    let cells = width.saturating_sub(display_width(&label)).max(1);
    let total = progress.total.max(1);
    let cell_at = |i: usize| {
        let run = (2 * i + 1) * total / (2 * cells);
        match view.runs.get(run) {
            Some(r) if r.error.is_some() => Cell::Errored,
            Some(r) if r.check_passed == Some(false) => Cell::Failed,
            Some(_) => Cell::Passed,
            None if run < progress.done + progress.working => Cell::Working,
            None => Cell::Left,
        }
    };
    let look = |cell: Cell| match cell {
        Cell::Passed => ('█', theme.colors.success),
        Cell::Failed => ('▓', theme.colors.success),
        Cell::Errored => ('█', theme.colors.error),
        Cell::Working => ('▒', theme.colors.pending),
        Cell::Left => ('░', theme.colors.faint),
    };
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run: Option<(Cell, usize)> = None;
    let flush = |spans: &mut Vec<Span<'static>>, (cell, n): (Cell, usize)| {
        let (glyph, color) = look(cell);
        spans.push(Span::styled(
            glyph.to_string().repeat(n),
            Style::default().fg(color),
        ));
    };
    for i in 0..cells {
        let cell = cell_at(i);
        run = match run {
            Some((same, n)) if same == cell => Some((same, n + 1)),
            Some(other) => {
                flush(&mut spans, other);
                Some((cell, 1))
            }
            None => Some((cell, 1)),
        };
    }
    if let Some(last) = run {
        flush(&mut spans, last);
    }
    spans.push(Span::styled(label, Style::default().fg(theme.colors.text)));
    Line::from(spans)
}

#[cfg(test)]
mod tests;
