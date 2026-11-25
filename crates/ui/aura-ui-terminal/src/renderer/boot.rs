//! Boot consoles in the chat: a cloud machine coming up, or an agent
//! moving between machines. While it runs the console shows every step,
//! what is known about the machine and the newest output of the commands
//! run on it; once done it folds to its outcome and opens on a click.

use super::text::{display_width, wrap_words};
use crate::components::BootConsole;
use crate::events::BootStageState;
use crate::Theme;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

/// Columns before a console's marker.
const INDENT: &str = "  ";

/// Columns before a step, a fact or a log line.
const BODY: &str = "    ";

/// Log lines shown while the console runs.
const LIVE_LOG_LINES: usize = 8;

/// Widest a fact's key column gets.
const MAX_KEY_WIDTH: usize = 14;

/// `850 ms`, `12.4s` or `3m 05s`.
fn duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{}.{}s", ms / 1000, (ms % 1000) / 100)
    } else {
        let secs = ms / 1000;
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

/// `text` cut to `width` columns, with `…` where it was cut.
fn fit(text: &str, width: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    if display_width(text) <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

/// `line` cut to `width` columns, its last span ending in `…` where it
/// was cut.
fn clip(line: Line<'static>, width: usize) -> Line<'static> {
    if line.width() <= width {
        return line;
    }
    let mut spans = Vec::new();
    let mut left = width;
    for span in line.spans {
        let w = display_width(&span.content);
        if w < left {
            left -= w;
            spans.push(span);
            continue;
        }
        if left > 0 {
            spans.push(Span::styled(fit(&span.content, left), span.style));
        }
        break;
    }
    Line::from(spans)
}

/// Lines for `console` drawn `width` columns wide. `spinner` turns in
/// the header and on the running step.
pub(super) fn boot_lines(
    console: &BootConsole,
    expanded: bool,
    spinner: &str,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let faint = Style::default().fg(theme.colors.faint);
    let mut lines = vec![clip(header(console, expanded, spinner, theme), width)];
    if let Some(outcome) = &console.outcome {
        let color = if outcome.ok {
            theme.colors.secondary
        } else {
            theme.colors.error
        };
        for row in wrap_words(&outcome.summary, width.saturating_sub(BODY.len()).max(10)) {
            lines.push(Line::from(vec![
                Span::raw(BODY),
                Span::styled(row, Style::default().fg(color)),
            ]));
        }
        if !expanded {
            lines.push(Line::from(vec![
                Span::raw(BODY),
                Span::styled("click to show the boot log", faint),
            ]));
            return lines;
        }
    }
    lines.extend(
        step_lines(console, spinner, theme)
            .into_iter()
            .map(|line| clip(line, width)),
    );
    let facts = fact_lines(console, width, theme);
    if !facts.is_empty() {
        lines.push(Line::default());
        lines.extend(facts);
    }
    let log = log_lines(console, console.is_running(), width, theme);
    if !log.is_empty() {
        lines.push(Line::default());
        lines.extend(log);
    }
    lines
}

fn header(console: &BootConsole, expanded: bool, spinner: &str, theme: &Theme) -> Line<'static> {
    let faint = Style::default().fg(theme.colors.faint);
    let mut spans: Vec<Span<'static>> = Vec::new();
    match &console.outcome {
        None => spans.push(Span::styled(
            format!("{INDENT}{spinner} "),
            Style::default().fg(theme.colors.success),
        )),
        Some(outcome) => {
            spans.push(Span::styled(
                format!("{INDENT}{} ", if expanded { "▾" } else { "▸" }),
                faint,
            ));
            spans.push(if outcome.ok {
                Span::styled("✓ ", Style::default().fg(theme.colors.success))
            } else {
                Span::styled("✗ ", Style::default().fg(theme.colors.error))
            });
        }
    }
    spans.push(Span::styled(
        console.title.clone(),
        Style::default()
            .fg(theme.colors.primary)
            .add_modifier(Modifier::BOLD),
    ));
    if let Some(machine) = console.fact("machine") {
        spans.push(Span::styled(format!("  ·  {machine}"), faint));
    }
    spans.push(Span::styled(
        format!("  ·  {}", duration(console.elapsed_ms())),
        faint,
    ));
    Line::from(spans)
}

fn step_lines(console: &BootConsole, spinner: &str, theme: &Theme) -> Vec<Line<'static>> {
    let faint = Style::default().fg(theme.colors.faint);
    console
        .steps
        .iter()
        .map(|step| {
            let (mark, mark_style, note) = match &step.state {
                BootStageState::Running => (
                    spinner.to_string(),
                    Style::default().fg(theme.colors.success),
                    step.started
                        .map(|at| duration(u64::try_from(at.elapsed().as_millis()).unwrap_or(0))),
                ),
                BootStageState::Done { ms } => (
                    "✓".to_string(),
                    Style::default().fg(theme.colors.success),
                    Some(duration(*ms)),
                ),
                BootStageState::Skipped(reason) => {
                    ("–".to_string(), faint, Some(format!("skipped: {reason}")))
                }
                BootStageState::Failed(reason) => (
                    "✗".to_string(),
                    Style::default().fg(theme.colors.error),
                    Some(reason.clone()),
                ),
            };
            let label_style = match step.state {
                BootStageState::Running => Style::default().fg(theme.colors.text),
                BootStageState::Failed(_) => Style::default().fg(theme.colors.error),
                _ => Style::default().fg(theme.colors.secondary),
            };
            let mut spans = vec![
                Span::raw(BODY),
                Span::styled(format!("{mark} "), mark_style),
                Span::styled(step.label.clone(), label_style),
            ];
            if let Some((done, total)) = step.progress.filter(|(_, total)| *total > 0) {
                spans.push(Span::styled(
                    format!("  {done}/{total}"),
                    Style::default().fg(theme.colors.muted),
                ));
            }
            if let Some(note) = note {
                let style = if matches!(step.state, BootStageState::Failed(_)) {
                    Style::default().fg(theme.colors.error)
                } else {
                    faint
                };
                spans.push(Span::styled(format!("  {note}"), style));
            }
            Line::from(spans)
        })
        .collect()
}

fn fact_lines(console: &BootConsole, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let key_width = console
        .facts
        .iter()
        .map(|(key, _)| display_width(key))
        .max()
        .unwrap_or(0)
        .min(MAX_KEY_WIDTH);
    let value_width = width.saturating_sub(BODY.len() + key_width + 2).max(16);
    let mut lines = Vec::new();
    for (key, value) in &console.facts {
        let key = fit(key, key_width);
        let pad = key_width.saturating_sub(display_width(&key));
        for (i, row) in wrap_words(value, value_width).into_iter().enumerate() {
            let lead = if i == 0 {
                format!("{key}{}  ", " ".repeat(pad))
            } else {
                " ".repeat(key_width + 2)
            };
            lines.push(Line::from(vec![
                Span::raw(BODY),
                Span::styled(lead, Style::default().fg(theme.colors.muted)),
                Span::styled(row, Style::default().fg(theme.colors.secondary)),
            ]));
        }
    }
    lines
}

fn log_lines(console: &BootConsole, live: bool, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let faint = Style::default().fg(theme.colors.faint);
    let shown = if live {
        console.log.len().min(LIVE_LOG_LINES)
    } else {
        console.log.len()
    };
    let hidden = console.dropped + console.log.len() - shown;
    let text_width = width.saturating_sub(BODY.len() + 2).max(10);
    let mut lines = Vec::new();
    if hidden > 0 {
        lines.push(Line::from(vec![
            Span::raw(BODY),
            Span::styled(format!("│ … {hidden} earlier lines"), faint),
        ]));
    }
    for line in console.log.iter().skip(console.log.len() - shown) {
        let style = if line.stderr {
            Style::default().fg(theme.colors.warning)
        } else if line.text.starts_with("$ ") {
            Style::default().fg(theme.colors.text)
        } else {
            Style::default().fg(theme.colors.muted)
        };
        lines.push(Line::from(vec![
            Span::raw(BODY),
            Span::styled("│ ", faint),
            Span::styled(fit(&line.text, text_width), style),
        ]));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{BootLogLine, BootUpdate};

    fn text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn console() -> BootConsole {
        let mut console = BootConsole::new("b1", "Moving to the cloud");
        for (key, state) in [
            ("quota", BootStageState::Done { ms: 210 }),
            ("snapshot", BootStageState::Done { ms: 192_000 }),
            ("create", BootStageState::Running),
        ] {
            console.apply(BootUpdate::Stage {
                key: key.into(),
                label: format!("Step {key}"),
                state,
            });
        }
        console.apply(BootUpdate::Fact {
            key: "machine".into(),
            value: "aura-re36-3f9c".into(),
        });
        console.apply(BootUpdate::Fact {
            key: "region".into(),
            value: "nyc".into(),
        });
        console.apply(BootUpdate::Log(
            (0..12)
                .map(|i| BootLogLine {
                    stderr: i == 11,
                    text: format!("line {i}"),
                })
                .collect(),
        ));
        console
    }

    #[test]
    fn a_running_console_shows_steps_facts_and_the_newest_output() {
        let theme = Theme::cyber();
        let shown = text(&boot_lines(&console(), false, "⠋", 80, &theme));
        assert!(shown.starts_with("  ⠋ Moving to the cloud  ·  aura-re36-3f9c  ·  "));
        assert!(shown.contains("✓ Step quota  210 ms"), "{shown}");
        assert!(shown.contains("✓ Step snapshot  3m 12s"), "{shown}");
        assert!(shown.contains("⠋ Step create"), "{shown}");
        assert!(shown.contains("region   nyc"), "{shown}");
        assert!(shown.contains("│ … 4 earlier lines"), "{shown}");
        assert!(shown.contains("│ line 11"));
        assert!(!shown.contains("│ line 3\n"));
    }

    #[test]
    fn a_finished_console_folds_to_its_outcome() {
        let theme = Theme::cyber();
        let mut done = console();
        done.apply(BootUpdate::End {
            ok: true,
            summary: "Now running in the cloud.".into(),
        });
        let folded = text(&boot_lines(&done, false, "⠋", 80, &theme));
        assert!(folded.starts_with("  ▸ ✓ Moving to the cloud"), "{folded}");
        assert!(folded.contains("Now running in the cloud."));
        assert!(!folded.contains("Step quota"));
        let open = text(&boot_lines(&done, true, "⠋", 80, &theme));
        assert!(open.starts_with("  ▾ ✓ "));
        assert!(open.contains("│ line 0"), "the whole log once opened");

        let mut failed = console();
        failed.apply(BootUpdate::End {
            ok: false,
            summary: "claim failed".into(),
        });
        let failed = text(&boot_lines(&failed, true, "⠋", 80, &theme));
        assert!(failed.starts_with("  ▾ ✗ "));
        assert!(failed.contains("✗ Step create  stopped"), "{failed}");
    }

    #[test]
    fn long_lines_fit_the_width() {
        let theme = Theme::cyber();
        let mut console = console();
        console.apply(BootUpdate::Log(vec![BootLogLine {
            stderr: false,
            text: "x".repeat(300),
        }]));
        for line in boot_lines(&console, true, "⠋", 40, &theme) {
            assert!(line.width() <= 40, "{line:?}");
        }
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(850), "850 ms");
        assert_eq!(duration(12_450), "12.4s");
        assert_eq!(duration(185_000), "3m 05s");
    }
}
