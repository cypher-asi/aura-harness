//! Draw the chat panel's BENCH tab: the list of benchmarks, one
//! benchmark's numbers side by side, or the form that sets one up.

use super::panels::body_block;
use crate::app::benchmarks::{BenchMode, BenchState, FormField, WorkChoice};
use crate::events::{
    arm_letter, cost_per_fix, cost_shares, per_fix_cells, BenchArm, BenchRun, BenchStatus,
    BenchTaskCell, BenchTurn, BenchView,
};
use crate::{App, Theme};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};

/// Width of the row labels of a benchmark's table.
const LABEL: usize = 16;

/// `$0.004`, `$1.25`, or `free` for nothing.
fn money(usd: f64) -> String {
    if usd <= 0.0 {
        "free".to_string()
    } else if usd < 0.01 {
        format!("${usd:.4}")
    } else {
        format!("${usd:.2}")
    }
}

/// `850ms`, `42s`, `3m 05s`.
fn duration(ms: u64) -> String {
    if ms < 1_000 {
        return format!("{ms}ms");
    }
    let secs = ms / 1_000;
    if secs < 60 {
        return format!("{secs}s");
    }
    format!("{}m {:02}s", secs / 60, secs % 60)
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
    let state = app.bench();
    let (lines, scroll) = match &state.mode {
        BenchMode::List => {
            let lines = list_lines(state, theme, width);
            // Keep the selected row (two lines below the header) on screen.
            let row = state.selected + 2;
            let height = usize::from(padded.height);
            let scroll = row.saturating_sub(height.saturating_sub(1));
            (lines, u16::try_from(scroll).unwrap_or(u16::MAX))
        }
        BenchMode::Detail(_) => match state.shown() {
            Some(view) => {
                let lines = detail_lines(view, state.run, theme, width);
                let last = u16::try_from(lines.len().saturating_sub(1)).unwrap_or(u16::MAX);
                (lines, state.scroll.min(last))
            }
            None => (vec![muted_line("That benchmark is gone.", theme)], 0),
        },
        BenchMode::Form => (form_lines(app, theme, width), 0),
    };
    app.clamp_bench_scroll(scroll);
    app.set_chat_rows(Some((inner, usize::from(scroll))));
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        padded,
    );
}

fn muted_line<'a>(text: impl Into<String>, theme: &Theme) -> Line<'a> {
    Line::from(Span::styled(
        text.into(),
        Style::default().fg(theme.colors.muted),
    ))
}

fn keys_line<'a>(text: &str, theme: &Theme) -> Line<'a> {
    muted_line(text.to_string(), theme)
}

/// One benchmark arm in a list row: `gate $0.06 2/3 pass`.
fn arm_brief(arm: &BenchArm) -> String {
    let mut text = format!(
        "{} {}",
        headline(&arm.label, 28),
        arm.median_cost_usd.map_or_else(|| "-".to_string(), money)
    );
    if arm.checks_run > 0 {
        text.push_str(&format!(" {}/{} pass", arm.checks_passed, arm.checks_run));
    }
    text
}

fn list_lines<'a>(state: &BenchState, theme: &Theme, width: usize) -> Vec<Line<'a>> {
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
        lines.push(Line::from(vec![
            Span::styled(marker, style),
            Span::styled(
                format!("{} ", view.name),
                style.add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{:<10}", view.status.label()),
                if selected {
                    style
                } else {
                    Style::default().fg(status_color)
                },
            ),
            Span::styled(headline(&text, room.max(10)), style),
        ]));
    }
    lines
}

/// A row of the side-by-side table: label, arm A, arm B.
fn row<'a>(label: &str, (a, b): (&str, &str), column: usize, theme: &Theme) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("{label:<LABEL$}"),
            Style::default().fg(theme.colors.muted),
        ),
        Span::styled(
            format!("{a:<column$}  "),
            Style::default().fg(theme.colors.text),
        ),
        Span::styled(b.to_string(), Style::default().fg(theme.colors.text)),
    ])
}

/// `2/3 passed, $0.40` for an arm's runs of one example task.
fn task_cell(cell: &BenchTaskCell) -> String {
    if cell.runs == 0 {
        return "-".to_string();
    }
    let cost = cell.mean_cost_usd.map_or_else(|| "-".to_string(), money);
    if cell.checked == 0 {
        return format!(
            "{} run{}, {cost}",
            cell.runs,
            if cell.runs == 1 { "" } else { "s" }
        );
    }
    format!("{}/{} passed, {cost}", cell.passed, cell.checked)
}

fn turn_cell(turn: Option<&BenchTurn>) -> String {
    let Some(turn) = turn.filter(|t| t.runs > 0) else {
        return "-".to_string();
    };
    let mut text = turn.mean_cost_usd.map_or_else(|| "-".to_string(), money);
    for (n, what) in [
        (turn.passed, "pass"),
        (turn.failed, "fail"),
        (turn.errors, "error"),
    ] {
        if n > 0 {
            text.push_str(&format!(", {n} {what}"));
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

fn run_line<'a>(run: &BenchRun, selected: bool, theme: &Theme) -> Line<'a> {
    let check = match run.check_passed {
        Some(true) => "check passed",
        Some(false) => "check failed",
        None => "",
    };
    let mut text = format!(
        "{} {:<6} {:<30} {:>7} {:>8}  {check:<12} {} file{}",
        if selected { "›" } else { " " },
        run.name,
        headline(&run.model, 30),
        duration(run.wall_ms),
        money(run.cost_usd),
        run.files_changed,
        if run.files_changed == 1 { "" } else { "s" },
    );
    if let Some(task) = &run.task {
        text.push_str(&format!("  {task}"));
    }
    if let Some(error) = &run.error {
        text.push_str(&format!("  ({})", headline(error, 60)));
    }
    let mut style = Style::default().fg(if run.error.is_some() {
        theme.colors.warning
    } else {
        theme.colors.text
    });
    if selected {
        style = style
            .bg(theme.colors.selection)
            .fg(theme.colors.on_selection);
    }
    Line::from(Span::styled(text, style))
}

#[allow(clippy::too_many_lines)]
fn detail_lines<'a>(
    view: &BenchView,
    run: Option<usize>,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'a>> {
    let bold = Style::default()
        .fg(theme.colors.text)
        .add_modifier(Modifier::BOLD);
    let [a, b] = &view.arms;
    let mut lines = vec![
        Line::from(vec![
            Span::styled(format!("{} ", view.name), bold),
            Span::styled(
                format!(
                    "\"{}\" · {}",
                    headline(&view.title, 60),
                    view.status.label()
                ),
                Style::default().fg(theme.colors.text),
            ),
        ]),
        muted_line(
            format!(
                "{} · {} run{} per {}arm · {}{}",
                view.source,
                view.repeats,
                if view.repeats == 1 { "" } else { "s" },
                if view.tasks.is_empty() {
                    ""
                } else {
                    "task and "
                },
                match view.check.as_deref() {
                    _ if !view.tasks.is_empty() => "each task's own check".to_string(),
                    Some(c) => format!("check `{c}`"),
                    None => "no check command".to_string(),
                },
                view.budget_usd
                    .map_or_else(String::new, |usd| format!(" · budget {}", money(usd))),
            ),
            theme,
        ),
        Line::default(),
    ];
    let a_head = format!("A {}", a.label);
    let column = a_head
        .chars()
        .count()
        .max(32)
        .min(width.saturating_sub(LABEL + 2) / 2);
    lines.push(Line::from(vec![
        Span::raw(" ".repeat(LABEL)),
        Span::styled(format!("{:<column$}  ", headline(&a_head, column)), bold),
        Span::styled(format!("B {}", b.label), bold),
    ]));
    let pair = |f: &dyn Fn(&BenchArm) -> String| (f(a), f(b));
    let mut add = |label: &str, (x, y): (String, String)| {
        lines.push(row(label, (&x, &y), column, theme));
    };
    add(
        "finished",
        pair(&|arm| format!("{}/{}", arm.finished, arm.runs)),
    );
    add(
        "median time",
        pair(&|arm| arm.median_wall_ms.map_or_else(|| "-".to_string(), duration)),
    );
    let [a_share, b_share] = cost_shares(Some(a.total_cost_usd), Some(b.total_cost_usd));
    add(
        "total cost",
        (
            money(a.total_cost_usd) + &a_share,
            money(b.total_cost_usd) + &b_share,
        ),
    );
    let median = |arm: &BenchArm| arm.median_cost_usd.map_or_else(|| "-".to_string(), money);
    let [a_share, b_share] = cost_shares(a.median_cost_usd, b.median_cost_usd);
    add("median cost", (median(a) + &a_share, median(b) + &b_share));
    if let Some(baseline) = view.baseline.as_ref().filter(|b| b.priced) {
        add("vs the session", pair(&|arm| ratio(arm, baseline.cost_usd)));
    }
    add(
        "check",
        pair(&|arm| {
            if arm.checks_run == 0 {
                "-".to_string()
            } else {
                format!("{}/{} passed", arm.checks_passed, arm.checks_run)
            }
        }),
    );
    if a.checks_run + b.checks_run > 0 {
        let per_fix = |arm: &BenchArm| cost_per_fix(arm.total_cost_usd, arm.checks_passed);
        let [x, y] = per_fix_cells(per_fix(a), per_fix(b), money);
        add("cost per fix", (x, y));
    }
    if view.judge.is_some() {
        add(
            "judge",
            pair(&|arm| {
                format!(
                    "{} won, {} lost, {} tied",
                    arm.judge_wins, arm.judge_losses, arm.judge_ties
                )
            }),
        );
    }
    let rated = |arm: &BenchArm| arm.ref_better + arm.ref_as_good + arm.ref_worse;
    if rated(a) + rated(b) > 0 {
        add(
            "vs yours",
            pair(&|arm| {
                format!(
                    "{} better, {} as good, {} worse",
                    arm.ref_better, arm.ref_as_good, arm.ref_worse
                )
            }),
        );
    }
    if let Some(baseline) = &view.baseline {
        lines.push(muted_line(
            format!(
                "The session itself: {} call{}, {}{}",
                baseline.calls,
                if baseline.calls == 1 { "" } else { "s" },
                if baseline.priced {
                    money(baseline.cost_usd)
                } else {
                    "no price".to_string()
                },
                if baseline.models.is_empty() {
                    String::new()
                } else {
                    format!(" on {}", baseline.models.join(", "))
                },
            ),
            theme,
        ));
    }
    if view.has_reference && view.judge.is_some() {
        lines.push(muted_line(
            "\"vs yours\": the judge rated each run against the diff and reply the session ended with.",
            theme,
        ));
    }

    if !view.tasks.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "Per task: checks passed, then mean run cost",
            bold,
        )));
        for task in &view.tasks {
            let label = headline(&task.id, LABEL - 1);
            let [a_share, b_share] =
                cost_shares(task.arms[0].mean_cost_usd, task.arms[1].mean_cost_usd);
            lines.push(row(
                &label,
                (
                    &(task_cell(&task.arms[0]) + &a_share),
                    &(task_cell(&task.arms[1]) + &b_share),
                ),
                column,
                theme,
            ));
        }
    }

    if view.prompts.len() > 1 {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "Per prompt: mean turn cost, then test or build commands that passed or failed",
            bold,
        )));
        for (i, prompt) in view.prompts.iter().enumerate() {
            let label = format!("{}. {}", i + 1, headline(prompt, LABEL - 4));
            lines.push(row(
                &label,
                (&turn_cell(a.turns.get(i)), &turn_cell(b.turns.get(i))),
                column,
                theme,
            ));
        }
    }

    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Tokens per model", bold)));
    for (i, arm) in view.arms.iter().enumerate() {
        let text = if arm.tokens.is_empty() {
            "no model calls".to_string()
        } else {
            arm.tokens
                .iter()
                .map(|t| {
                    format!(
                        "{} {} call{}, {}k in / {}k out, {}",
                        t.model,
                        t.calls,
                        if t.calls == 1 { "" } else { "s" },
                        t.input_tokens / 1_000,
                        t.output_tokens / 1_000,
                        money(t.cost_usd)
                    )
                })
                .collect::<Vec<_>>()
                .join("; ")
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {}: ", arm_letter(i)),
                Style::default().fg(theme.colors.muted),
            ),
            Span::styled(text, Style::default().fg(theme.colors.text)),
        ]));
    }

    if let Some(judge) = &view.judge {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            format!(
                "Judge: {} blind call{} on {}, {} (not in the arm totals)",
                judge.verdicts.len(),
                if judge.verdicts.len() == 1 { "" } else { "s" },
                judge.model,
                money(judge.cost_usd)
            ),
            bold,
        )));
        for verdict in &judge.verdicts {
            let outcome = if verdict.parsed {
                verdict.winner.map_or_else(
                    || "tie".to_string(),
                    |arm| format!("{} better", arm_letter(arm)),
                )
            } else {
                "no verdict".to_string()
            };
            let reference = match (&verdict.a_vs_reference, &verdict.b_vs_reference) {
                (None, None) => String::new(),
                (x, y) => format!(
                    " · vs yours: A {}, B {}",
                    x.as_deref().unwrap_or("?"),
                    y.as_deref().unwrap_or("?")
                ),
            };
            lines.push(Line::from(Span::styled(
                format!(
                    "  {} vs {}: {outcome}{reference}",
                    verdict.a_run, verdict.b_run
                ),
                Style::default().fg(theme.colors.text),
            )));
            let why = verdict
                .reasoning
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or_default();
            if !why.is_empty() {
                lines.push(muted_line(format!("    {}", headline(why, 140)), theme));
            }
        }
    }

    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Runs", bold)));
    for (i, r) in view.runs.iter().enumerate() {
        lines.push(run_line(r, run == Some(i), theme));
    }
    for note in &view.notes {
        lines.push(Line::from(Span::styled(
            format!("Note: {note}"),
            Style::default().fg(theme.colors.warning),
        )));
    }
    lines.push(Line::default());
    let running = view.status == BenchStatus::Running;
    let keys = if view.status.has_copies() && !running && !view.tasks.is_empty() {
        "←/→ pick a run · d diff it (or both arms' first) · a/b A or B was better · \
         x remove the copies · Esc back"
    } else if view.status.has_copies() && !running {
        "←/→ pick a run · d diff it (or both arms' first) · y apply it · a/b A or B was better · \
         x remove the copies · Esc back"
    } else if running {
        "↑/↓ scroll · c stop it · Esc back"
    } else {
        "↑/↓ scroll · Esc back"
    };
    lines.push(keys_line(keys, theme));
    lines
}

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
    use crate::events::{BenchBaseline, BenchJudge, BenchVerdict, UiCommand};
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
        let shown = text(&detail_lines(&view(), Some(0), &theme, 120));
        assert!(shown.contains("A gate"), "{shown}");
        assert!(shown.contains("B claude-opus-4-7"), "{shown}");
        assert!(shown.contains("$0.06 · 85% less"), "{shown}");
        assert!(shown.contains("$0.40 · 100%"), "{shown}");
        let per_fix = shown
            .lines()
            .find(|l| l.trim_start().starts_with("cost per fix"))
            .unwrap_or_else(|| panic!("{shown}"));
        assert!(
            per_fix.contains("$0.12 · 85% less") && per_fix.contains("$0.80 · 100%"),
            "{per_fix}"
        );
        assert!(shown.contains("vs the session"), "{shown}");
        assert!(shown.contains("0.30×"), "{shown}");
        assert!(shown.contains("2.00×"), "{shown}");
        assert!(shown.contains("vs yours"), "{shown}");
        assert!(shown.contains("1 better, 1 as good, 0 worse"), "{shown}");
        assert!(shown.contains("$0.03, 2 pass"), "{shown}");
        assert!(shown.contains("$0.03, 1 fail"), "{shown}");
        assert!(
            shown.contains("a-1 vs b-1: B better · vs yours: A as good, B better"),
            "{shown}"
        );
        assert!(
            shown.contains("The session itself: 12 calls, $0.20"),
            "{shown}"
        );
        assert!(shown.contains("› a-1"), "{shown}");
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
            },
            BenchTaskRow {
                id: "coding/fix-shell-bug".to_string(),
                ..BenchTaskRow::default()
            },
        ];
        view.runs[0].task = Some("coding/write-file".to_string());
        let shown = text(&detail_lines(&view, None, &theme, 120));
        assert!(shown.contains("2 runs per task and arm"), "{shown}");
        assert!(shown.contains("each task's own check"), "{shown}");
        assert!(shown.contains("Per task"), "{shown}");
        assert!(shown.contains("2/2 passed, $0.02 · 93% less"), "{shown}");
        assert!(shown.contains("1/2 passed, $0.30 · 100%"), "{shown}");
        assert!(shown.contains("coding/write-file"), "{shown}");
        assert!(!shown.contains("y apply"), "{shown}");
        assert!(!shown.contains("Per prompt"), "{shown}");
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
