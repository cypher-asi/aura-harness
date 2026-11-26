//! The form that sets a benchmark up.

use super::format::{fit, headline, money};
use crate::app::{FormField, WorkChoice, AUTO_CLASSIFIER, BY_DIFFICULTY, DEFAULT_AGAINST};
use crate::events::BenchCost;
use crate::{App, Theme};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

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
pub(super) fn form_lines<'a>(app: &App, theme: &Theme, width: usize) -> Vec<Line<'a>> {
    form_layout(app, theme, width).0
}

/// The form's lines, and the line each of its rows is on. Every line
/// down to the last row fits `width`, so none wraps and the rows keep
/// their lines on screen.
pub(super) fn form_layout<'a>(
    app: &App,
    theme: &Theme,
    width: usize,
) -> (Vec<Line<'a>>, Vec<(usize, FormField)>) {
    let form = &app.bench().form;
    let mut lines = title_lines(theme, width);
    let mut rows = Vec::with_capacity(FormField::ALL.len());
    for field in FormField::ALL {
        if !form.shown(field) {
            continue;
        }
        if field == FormField::Go {
            lines.push(Line::default());
        }
        rows.push((lines.len(), field));
        lines.push(field_line(app, theme, width, field));
    }
    lines.extend(estimate_lines(app, theme));
    (lines, rows)
}

/// The title and the keys, and a blank line.
fn title_lines(theme: &Theme, width: usize) -> Vec<Line<'static>> {
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
    vec![
        Line::from(vec![Span::styled(TITLE, bold), Span::styled(hint, muted)]),
        Line::default(),
    ]
}

/// The row of `field`: its marker, label and value.
fn field_line(app: &App, theme: &Theme, width: usize, field: FormField) -> Line<'static> {
    let state = app.bench();
    let form = &state.form;
    let examples = matches!(state.work(), WorkChoice::Examples(_));
    let muted = Style::default().fg(theme.colors.muted);
    let focused = field == form.field;
    let (value, faint) = field_value(app, field, width);
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
    Line::from(vec![
        Span::styled(if focused { "› " } else { "  " }, style),
        Span::styled(format!("{:<14}", field.label()), muted),
        Span::styled(value, style),
    ])
}

/// How `field`'s value reads, and whether it reads faint.
fn field_value(app: &App, field: FormField, width: usize) -> (String, bool) {
    let state = app.bench();
    let form = &state.form;
    let examples = matches!(state.work(), WorkChoice::Examples(_));
    match field {
        FormField::From => from_value(app),
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
        FormField::Work => (work_value(app), false),
        FormField::Prompt => prompt_value(app, width),
        FormField::Tasks if !examples => {
            ("(only when the work is example tasks)".to_string(), true)
        }
        FormField::Tasks => (tasks_value(app, width), false),
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
        FormField::Go => (go_value(app), false),
    }
}

fn from_value(app: &App) -> (String, bool) {
    match app.bench_from_label() {
        Some(label) => (label, false),
        None if app.bench_from_choices().is_empty() => (
            "none yet: make an agent in PROJECTS first".to_string(),
            true,
        ),
        None => ("←/→ picks the agent it runs from".to_string(), true),
    }
}

fn work_value(app: &App) -> String {
    let state = app.bench();
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
    }
}

fn prompt_value(app: &App, width: usize) -> (String, bool) {
    let state = app.bench();
    let form = &state.form;
    if !matches!(state.work(), WorkChoice::Prompt) {
        ("(only when the work is a typed prompt)".to_string(), true)
    } else if form.prompt.is_empty() {
        ("type it in the input line, then Enter".to_string(), true)
    } else {
        (headline(&form.prompt, width.saturating_sub(24)), false)
    }
}

fn tasks_value(app: &App, width: usize) -> String {
    let state = app.bench();
    let form = &state.form;
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
    format!(
        "{} of {total} · {}",
        form.tasks.min(total),
        headline(&names, width.saturating_sub(36))
    )
}

fn go_value(app: &App) -> String {
    let form = &app.bench().form;
    if form.waiting {
        "Estimating…".to_string()
    } else if form.ready() {
        "[ Start ]  Enter starts it".to_string()
    } else {
        "[ Estimate ]  Enter shows what it should cost".to_string()
    }
}

/// What the estimate said, or what kept it from one.
fn estimate_lines(app: &App, theme: &Theme) -> Vec<Line<'static>> {
    match &app.bench().form.estimate {
        Some(Ok(text)) => {
            let mut lines = vec![Line::default()];
            lines.extend(text.lines().map(|l| {
                Line::from(Span::styled(
                    l.to_string(),
                    Style::default().fg(theme.colors.text),
                ))
            }));
            lines
        }
        Some(Err(problem)) => vec![
            Line::default(),
            Line::from(Span::styled(
                problem.clone(),
                Style::default().fg(theme.colors.error),
            )),
        ],
        None => Vec::new(),
    }
}
