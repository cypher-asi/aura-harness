//! What an open panel shows: a run's details and a step's fields and
//! sections, or its JSON.

use super::super::benchmarks::fit;
use super::super::text::{display_width, wrap_words};
use super::SECTION_LINES;
use crate::app::run_log::{LogPanel, LogState};
use crate::events::{LogField, LogSection};
use crate::Theme;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthChar;

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

pub(super) fn body_lines(
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
pub(super) fn prose_wrap(text: &str, width: usize) -> Vec<String> {
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
