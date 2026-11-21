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
//! │ ███████████▒▒▒▒░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  36%   │
//! ```
//!
//! A tab for each benchmark, the picked one lit with its `×`; what it
//! runs; what it is doing, lit by a band of light while it goes on; and a
//! bar of its runs, ended ones in the color of how they ended. Folded, it
//! keeps the tabs and the bar.

use super::benchmarks::{fit, money, setup_counts, short_model, task_prefix, under_way};
use super::input::join_divider;
use super::panels::border_color;
use super::text::display_width;
use crate::app::bench_progress::{BenchPanelAreas, BenchTabArea};
use crate::app::PanelFocus;
use crate::events::{BenchPhase, BenchView};
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

    let block = Block::default()
        .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
        .border_style(Style::default().fg(border_color(app.focus() == PanelFocus::Chat, theme)));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    join_divider(frame, area);

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
    let width = usize::from(body.width);
    let mut lines = Vec::new();
    if !app.bench_panel_collapsed() {
        lines.extend(fact_lines(&view, width, theme));
        lines.push(status_line(&view, width, lit, theme));
    }
    lines.push(bar_line(&view, width, theme));
    // Short of rows, the facts give way before the bar.
    while lines.len() > usize::from(body.height) && lines.len() > 1 {
        lines.remove(lines.len() - 2);
    }
    frame.render_widget(Paragraph::new(lines), body);

    app.set_bench_panel_areas(BenchPanelAreas {
        tabs: tab_areas,
        toggle: Some(toggle),
        panel: Some(area),
    });
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
            spans.push(Span::styled(label, muted));
        }
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
    let used: usize = spans.iter().map(|s| display_width(&s.content)).sum();
    let fill = usize::from(toggle.x.saturating_sub(row.x)).saturating_sub(used);
    spans.push(Span::raw(" ".repeat(fill)));
    spans.push(Span::styled(
        format!(" {} ", if collapsed { "+" } else { "—" }),
        muted,
    ));
    (Line::from(spans), areas, toggle)
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
        let live: f64 = view
            .live
            .iter()
            .filter(|r| r.arm.min(1) == arm)
            .map(|r| r.cost_usd)
            .sum();
        money(view.arms[arm].total_cost_usd + live)
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
/// frame `lit` while it goes on, and where a click takes you at the right.
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
        BenchPhase::Stopped => theme.colors.warning,
        BenchPhase::Done => theme.colors.success,
        _ => theme.colors.pending,
    };
    let mut spans = under_way(
        phase,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
        lit,
        (theme.colors.muted, theme.colors.foreground),
    );
    spans.push(Span::styled(detail.clone(), muted));
    let hint = "click to open";
    let used = display_width(phase) + display_width(&detail);
    if used + display_width(hint) + 2 <= width {
        spans.push(Span::raw(" ".repeat(width - used - display_width(hint))));
        spans.push(Span::styled(hint, Style::default().fg(theme.colors.faint)));
    }
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
        Cell::Failed => ('█', theme.colors.warning),
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
mod tests {
    use crate::events::{
        BenchArm, BenchLiveRun, BenchRun, BenchStatus, BenchTaskRow, BenchTaskSetup, BenchView,
        UiCommand,
    };
    use crate::{App, Theme};
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    fn bench(id: &str, name: &str) -> BenchView {
        BenchView {
            id: id.to_string(),
            root: "root".to_string(),
            name: name.to_string(),
            title: "fix the parser".to_string(),
            source: "2 example tasks".to_string(),
            status: BenchStatus::Running,
            repeats: 1,
            tasks: vec![
                BenchTaskRow {
                    id: "swe/ansible-1".to_string(),
                    ..BenchTaskRow::default()
                },
                BenchTaskRow {
                    id: "swe/flipt-2".to_string(),
                    setup: BenchTaskSetup::Fetching { pull: true },
                    ..BenchTaskRow::default()
                },
            ],
            arms: [
                BenchArm {
                    label: "gate".to_string(),
                    total_cost_usd: 0.40,
                    ..BenchArm::default()
                },
                BenchArm {
                    label: "claude/claude-opus-5-5".to_string(),
                    ..BenchArm::default()
                },
            ],
            runs: vec![BenchRun {
                name: "a-1".to_string(),
                check_passed: Some(true),
                ..BenchRun::default()
            }],
            live: vec![BenchLiveRun {
                name: "b-1".to_string(),
                arm: 1,
                wall_ms: Some(30_000),
                cost_usd: 0.25,
                ..BenchLiveRun::default()
            }],
            ..BenchView::default()
        }
    }

    fn rows(app: &mut App) -> Vec<String> {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 40)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, app, &Theme::cyber()))
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

    fn find<'a>(rows: &'a [String], needle: &str) -> Option<(usize, &'a String)> {
        rows.iter()
            .enumerate()
            .find(|(_, row)| row.contains(needle))
    }

    fn click(app: &mut App, column: u16, row: u16) {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::empty(),
        });
    }

    #[test]
    fn no_panel_without_a_running_benchmark() {
        let mut app = App::new();
        assert!(find(&rows(&mut app), "Spent").is_none());
        let mut done = bench("1", "Compare #1");
        done.status = BenchStatus::Done;
        app.process_command(UiCommand::BenchUpsert(Box::new(done)));
        assert!(find(&rows(&mut app), "COMPARE #1").is_none());
    }

    #[test]
    fn a_running_benchmark_shows_its_progress_above_the_input_from_any_tab() {
        let mut app = App::new();
        app.process_command(UiCommand::BenchUpsert(Box::new(bench("1", "Compare #1"))));
        let rows = rows(&mut app);

        let (header, tabs) = find(&rows, "COMPARE #1 ×").expect("the tab");
        assert!(tabs.contains('—'), "{tabs:?}");
        let (work, line) = find(&rows, "├ Work").expect("work");
        assert!(line.contains("2 example tasks · swe"), "{line:?}");
        assert!(find(&rows, "A gate  vs  B opus-5-5").is_some());
        assert!(find(&rows, "├ Setup   1/2 tasks ready · 1 fetching").is_some());
        assert!(find(&rows, "└ Spent   A $0.40 · B $0.25").is_some());
        let (status, line) = find(&rows, "Running · 1 working · 1 of 4 done").expect("status");
        assert!(line.contains("click to open"), "{line:?}");
        let (bar, line) = find(&rows, "25%").expect("bar");
        assert!(line.contains('█') && line.contains('▒') && line.contains('░'));
        let (input, _) = find(&rows, "→ ").expect("the input");
        assert!(header < work && work < status && status < bar && bar < input);
        assert!(rows[header - 1].contains('├'), "{:?}", rows[header - 1]);
    }

    #[test]
    fn a_finished_benchmark_keeps_a_full_bar_until_its_tab_is_closed() {
        let mut app = App::new();
        app.process_command(UiCommand::BenchUpsert(Box::new(bench("1", "Compare #1"))));
        let mut done = bench("1", "Compare #1");
        done.status = BenchStatus::Done;
        done.tasks[1].setup = BenchTaskSetup::Ready;
        done.live.clear();
        done.runs = vec![BenchRun::default(); 4];
        app.process_command(UiCommand::BenchUpsert(Box::new(done)));
        let shown = rows(&mut app);
        let (_, status) = find(&shown, "Done · 4 of 4 done").expect("done");
        assert!(!status.contains("working"), "{status:?}");
        let (_, bar) = find(&shown, "100%").expect("bar");
        assert!(!bar.contains('░'), "{bar:?}");

        let close = app.bench_panel_areas().tabs[0].close.expect("the ×");
        click(&mut app, close.x + 1, close.y);
        assert!(find(&rows(&mut app), "COMPARE #1").is_none());
    }

    #[test]
    fn tabs_pick_and_the_toggle_folds_to_the_tabs_and_the_bar() {
        let mut app = App::new();
        app.process_command(UiCommand::BenchUpsert(Box::new(bench("1", "Compare #1"))));
        app.process_command(UiCommand::BenchUpsert(Box::new(bench("2", "Compare #2"))));
        let shown = rows(&mut app);
        assert!(
            find(&shown, "COMPARE #1   COMPARE #2 ×").is_some(),
            "the newest is picked"
        );

        let first = app.bench_panel_areas().tabs[0].tab;
        click(&mut app, first.x, first.y);
        assert!(find(&rows(&mut app), "COMPARE #1 ×   COMPARE #2").is_some());

        let toggle = app.bench_panel_areas().toggle.expect("toggle");
        click(&mut app, toggle.x + 1, toggle.y);
        let shown = rows(&mut app);
        let (header, tabs) = find(&shown, "COMPARE #1 ×").expect("tabs");
        assert!(tabs.contains(" + "), "{tabs:?}");
        assert!(find(&shown, "├ Work").is_none());
        assert!(shown[header + 1].contains("25%"), "{:?}", shown[header + 1]);
    }

    #[test]
    fn a_click_on_the_body_opens_the_benchmark_in_the_bench_tab() {
        let mut app = App::new();
        app.process_command(UiCommand::BenchUpsert(Box::new(bench("1", "Compare #1"))));
        let shown = rows(&mut app);
        let (row, _) = find(&shown, "├ Work").expect("work");
        let panel = app.bench_panel_areas().panel.expect("panel");
        click(&mut app, panel.x + 4, u16::try_from(row).unwrap());
        assert_eq!(app.chat_tab(), crate::app::ChatTab::Bench);
        assert_eq!(
            app.bench().shown().map(|b| b.name.as_str()),
            Some("Compare #1")
        );
    }

    #[test]
    fn only_the_benchmarks_of_the_tree_on_screen_have_tabs() {
        use crate::events::AgentSummary;
        let mut app = App::new();
        let agent = |id: &str, depth| AgentSummary {
            id: id.to_string(),
            name: id.to_string(),
            depth,
            ..AgentSummary::default()
        };
        app.process_command(UiCommand::SetAgents(vec![
            agent("root", 0),
            agent("kid", 1),
        ]));
        app.process_command(UiCommand::SetActiveAgent("kid".to_string()));
        app.process_command(UiCommand::BenchUpsert(Box::new(bench("1", "Compare #1"))));
        let mut other = bench("2", "Compare #2");
        other.root = "elsewhere".to_string();
        app.process_command(UiCommand::BenchUpsert(Box::new(other)));
        let shown = rows(&mut app);
        assert!(find(&shown, "COMPARE #1 ×").is_some());
        assert!(find(&shown, "COMPARE #2").is_none());
    }
}
