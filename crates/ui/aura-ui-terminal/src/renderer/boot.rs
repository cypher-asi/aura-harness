//! Boot consoles in the chat: a cloud machine coming up, or an agent
//! moving between machines. While it runs the console shows every step,
//! what is known about the machine and the newest output of the commands
//! run on it; once done it folds to its outcome and opens on a click.

use super::text::{display_width, wrap_words};
use super::MoreRows;
use crate::app::{CodeCopyHit, CopyText};
use crate::components::{
    boot_duration as duration, BootConsole, MoreSection, COPY_BUTTON, MORE_PAGE_LINES,
};
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

/// Widest a fact's key column gets.
const MAX_KEY_WIDTH: usize = 14;

/// Widest the progress bar gets.
const MAX_BAR_CELLS: usize = 48;

/// Milliseconds the band of light over a running step takes per cell.
const SWEEP_MS: u128 = 80;

/// Cells' worth of time the band waits off a step before passing again.
const BAND_GAP: u128 = 6;

/// Narrowest a console is drawn with its copy button.
const MIN_COPY_WIDTH: usize = 40;

/// The copy button on `console`'s header, drawn `width` columns wide.
pub(super) fn boot_copy(console: &BootConsole, width: usize) -> Option<CodeCopyHit> {
    let button = display_width(COPY_BUTTON);
    (width >= MIN_COPY_WIDTH).then(|| CodeCopyHit {
        line: 0,
        x: u16::try_from(width - button).unwrap_or(0),
        width: u16::try_from(button).unwrap_or(0),
        text: CopyText::BootLog(console.id.clone()),
    })
}

/// The header, with the copy button at its right end when there is room.
fn header_line(
    console: &BootConsole,
    expanded: bool,
    spinner: &str,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let header = header(console, expanded, spinner, theme);
    if width < MIN_COPY_WIDTH {
        return clip(header, width);
    }
    let room = width - display_width(COPY_BUTTON);
    let mut header = clip(header, room - 1);
    let pad = room - header.width();
    header.spans.push(Span::raw(" ".repeat(pad)));
    header.spans.push(Span::styled(
        COPY_BUTTON,
        Style::default().fg(theme.colors.muted),
    ));
    header
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

/// Lines for `console` drawn `width` columns wide, and the row that loads
/// earlier log lines, if any. `spinner` turns in the header and on the
/// running step.
pub(super) fn boot_lines(
    console: &BootConsole,
    expanded: bool,
    spinner: &str,
    width: usize,
    theme: &Theme,
) -> (Vec<Line<'static>>, MoreRows) {
    let faint = Style::default().fg(theme.colors.faint);
    let mut lines = vec![header_line(console, expanded, spinner, width, theme)];
    if console.is_running() {
        lines.push(bar_line(console, width, theme));
    }
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
            return (lines, MoreRows::new());
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
    let log = log_lines(console, width, theme);
    let mut more = MoreRows::new();
    if !log.is_empty() {
        lines.push(Line::default());
        if console.hidden() > 0 && console.can_show() > 0 {
            more.push((lines.len(), MoreSection::BootLog));
        }
        lines.extend(log);
    }
    (lines, more)
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

/// `██████▓▒▒░░░░░░░  36%`: as a benchmark's bar, a run of cells per
/// step, those finished solid in the color of how they ended, the running
/// ones filled as far as they've got and shaded beyond with a band of
/// light passing, those to come faint.
fn bar_line(console: &BootConsole, width: usize, theme: &Theme) -> Line<'static> {
    let percent = (console.fraction() * 100.0).floor().min(99.0);
    let label = format!("  {percent:>3}%");
    let cells = width
        .saturating_sub(BODY.len() + display_width(&label))
        .clamp(4, MAX_BAR_CELLS);
    let steps = console.steps.len();
    // Each cell: its glyph and color, and the running step it shades.
    let mut look: Vec<(char, ratatui::style::Color, Option<usize>)> = (0..cells)
        .map(|i| {
            if steps == 0 {
                return ('▒', theme.colors.pending, Some(0));
            }
            #[allow(clippy::cast_precision_loss)]
            let at = (i as f64 + 0.5) / cells as f64 * steps as f64;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let index = (at as usize).min(steps - 1);
            let step = &console.steps[index];
            #[allow(clippy::cast_precision_loss)]
            let within = at - index as f64;
            match &step.state {
                BootStageState::Pending => ('░', theme.colors.faint, None),
                BootStageState::Running if within < step.fraction() => {
                    ('█', theme.colors.success, None)
                }
                BootStageState::Running => ('▒', theme.colors.pending, Some(index)),
                BootStageState::Done { .. } => ('█', theme.colors.success, None),
                BootStageState::Skipped(_) => ('█', theme.colors.muted, None),
                BootStageState::Failed(_) => ('█', theme.colors.error, None),
            }
        })
        .collect();
    let tick = console.started.elapsed().as_millis() / SWEEP_MS;
    let mut i = 0;
    while i < look.len() {
        let Some(step) = look[i].2 else {
            i += 1;
            continue;
        };
        let len = look[i..].iter().take_while(|c| c.2 == Some(step)).count();
        #[allow(clippy::cast_possible_truncation)]
        let band = (tick % (len as u128 + BAND_GAP)) as usize;
        if band < len {
            look[i + band].0 = '▓';
            look[i + band].1 = theme.colors.text;
        }
        i += len;
    }
    let mut spans: Vec<Span<'static>> = vec![Span::raw(BODY)];
    let mut run = String::new();
    let mut color = None;
    for (glyph, fg, _) in look {
        if color.is_some_and(|c| c != fg) {
            spans.push(Span::styled(
                std::mem::take(&mut run),
                Style::default().fg(color.unwrap_or(fg)),
            ));
        }
        color = Some(fg);
        run.push(glyph);
    }
    if let Some(color) = color {
        spans.push(Span::styled(run, Style::default().fg(color)));
    }
    spans.push(Span::styled(label, Style::default().fg(theme.colors.text)));
    clip(Line::from(spans), width)
}

fn step_lines(console: &BootConsole, spinner: &str, theme: &Theme) -> Vec<Line<'static>> {
    let faint = Style::default().fg(theme.colors.faint);
    console
        .steps
        .iter()
        .map(|step| {
            let (mark, mark_style, note) = match &step.state {
                BootStageState::Pending => ("·".to_string(), faint, None),
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
                BootStageState::Pending => faint,
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

/// The log as far back as it is shown, under a row saying how many
/// lines come before, which loads more of them when clicked.
fn log_lines(console: &BootConsole, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let faint = Style::default().fg(theme.colors.faint);
    let shown = console.log_shown();
    let hidden = console.hidden();
    let text_width = width.saturating_sub(BODY.len() + 2).max(10);
    let mut lines = Vec::new();
    if hidden > 0 {
        let page = console.can_show().min(MORE_PAGE_LINES);
        let mut spans = vec![
            Span::raw(BODY),
            Span::styled(format!("│ … {hidden} earlier lines"), faint),
        ];
        if page > 0 {
            spans.push(Span::styled(
                format!(" · click to load {page} more"),
                Style::default().fg(theme.colors.muted),
            ));
        }
        lines.push(clip(Line::from(spans), width));
    }
    let skipped = console.log.len() - shown;
    for line in console
        .earlier
        .iter()
        .chain(console.log.iter().skip(skipped))
    {
        let mut spans = vec![Span::raw(BODY), Span::styled("│ ", faint)];
        spans.extend(log_spans(&line.text, theme));
        lines.push(clip(Line::from(spans), BODY.len() + 2 + text_width));
    }
    lines
}

/// A status verb as build tools print it, right-aligned after spaces:
/// `   Compiling tokio v1.49.0`, `  Downloaded serde`, `    Finished`.
fn status_verb(text: &str) -> Option<(&str, &str, &str)> {
    let body = text.trim_start();
    let indent = &text[..text.len() - body.len()];
    let (verb, rest) = body.split_once(' ').unwrap_or((body, ""));
    let mut chars = verb.chars();
    let looks_like = chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_lowercase() || c == '-')
        && (verb.ends_with("ing") || verb.ends_with("ed") || verb == "Fresh");
    (!indent.is_empty() && verb.len() >= 4 && looks_like).then_some((indent, verb, rest))
}

/// A line of command output in color: the command run, build tools'
/// status verbs, warnings and errors stand out; the rest is quiet.
fn log_spans(text: &str, theme: &Theme) -> Vec<Span<'static>> {
    let plain = |s: &str, color| Span::styled(s.to_string(), Style::default().fg(color));
    let bold = |s: &str, color| {
        Span::styled(
            s.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )
    };
    if let Some(command) = text.strip_prefix("$ ") {
        return vec![
            plain("$ ", theme.colors.faint),
            plain(command, theme.colors.text),
        ];
    }
    if let Some((indent, verb, rest)) = status_verb(text) {
        let mut spans = vec![
            Span::raw(indent.to_string()),
            bold(verb, theme.colors.success),
            Span::raw(" "),
        ];
        // `name v1.2.3 (/path)`: the name brighter than what follows.
        let (name, tail) = rest.split_once(' ').unwrap_or((rest, ""));
        spans.push(plain(name, theme.colors.text));
        if !tail.is_empty() {
            spans.push(plain(&format!(" {tail}"), theme.colors.muted));
        }
        return spans;
    }
    for (word, color) in [
        ("error", theme.colors.error),
        ("warning", theme.colors.warning),
    ] {
        if let Some(rest) = text.strip_prefix(word) {
            if rest.starts_with(':') || rest.starts_with('[') {
                return vec![bold(word, color), plain(rest, theme.colors.text)];
            }
        }
    }
    vec![plain(text, theme.colors.muted)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{BootLogLine, BootUpdate};

    fn boot_lines(
        console: &BootConsole,
        expanded: bool,
        spinner: &str,
        width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        super::boot_lines(console, expanded, spinner, width, theme).0
    }

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
    fn the_copy_button_is_drawn_where_its_click_lands() {
        let theme = Theme::cyber();
        let console = console();
        for width in [40, 80, 120] {
            let (lines, _) = super::boot_lines(&console, false, "⠋", width, &theme);
            let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(header.ends_with(COPY_BUTTON), "{header}");
            assert_eq!(display_width(&header), width);
            let hit = boot_copy(&console, width).expect("room for it");
            assert_eq!(usize::from(hit.x), width - display_width(COPY_BUTTON));
            assert_eq!(hit.text, CopyText::BootLog("b1".into()));
        }
        assert!(boot_copy(&console, 30).is_none());
        let (narrow, _) = super::boot_lines(&console, false, "⠋", 30, &theme);
        assert!(narrow[0].width() <= 30);
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
        assert!(
            shown.contains("│ … 4 earlier lines · click to load 4 more"),
            "{shown}"
        );
        assert!(shown.contains("│ line 11"));
        assert!(!shown.contains("│ line 3\n"));
    }

    #[test]
    fn the_earlier_lines_row_loads_more_until_none_are_left() {
        let theme = Theme::cyber();
        let mut console = console();
        let (lines, more) = super::boot_lines(&console, false, "⠋", 80, &theme);
        assert_eq!(more.len(), 1);
        let (row, section) = more[0];
        assert_eq!(section, MoreSection::BootLog);
        assert!(text(&lines[row..=row]).contains("4 earlier lines"));

        console.show_earlier(2);
        let shown = text(&boot_lines(&console, false, "⠋", 80, &theme));
        assert!(shown.contains("│ … 2 earlier lines"), "{shown}");
        assert!(shown.contains("│ line 2\n"), "{shown}");

        console.show_earlier(MORE_PAGE_LINES);
        let (lines, more) = super::boot_lines(&console, false, "⠋", 80, &theme);
        assert!(more.is_empty());
        let shown = text(&lines);
        assert!(!shown.contains("earlier lines"), "{shown}");
        assert!(shown.contains("│ line 0\n"), "{shown}");
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
    fn a_running_console_shows_a_bar_of_its_steps_and_those_to_come() {
        let theme = Theme::cyber();
        let mut running = console();
        running.apply(BootUpdate::Plan(vec![crate::events::BootPlanStep {
            key: "claim".into(),
            label: "Claiming aura node".into(),
        }]));
        let lines = boot_lines(&running, false, "⠋", 80, &theme);
        let bar = text(&lines[1..2]);
        assert!(bar.ends_with("   50%"), "2 of 4 steps done: {bar}");
        assert!(bar.contains('█') && bar.contains('░'), "{bar}");
        assert!(bar.contains('▒') || bar.contains('▓'), "{bar}");
        let shown = text(&lines);
        assert!(shown.contains("· Claiming aura node"), "{shown}");

        running.apply(BootUpdate::End {
            ok: true,
            summary: "done".into(),
        });
        let done = text(&boot_lines(&running, true, "⠋", 80, &theme));
        assert!(!done.contains('░') && !done.contains('%'), "{done}");
        assert!(!done.contains("Claiming aura node"), "never begun");
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
    fn build_output_is_colored_by_what_it_says() {
        let theme = Theme::cyber();
        let colors = |text: &str| -> Vec<(String, Option<ratatui::style::Color>)> {
            log_spans(text, &theme)
                .into_iter()
                .map(|s| (s.content.to_string(), s.style.fg))
                .collect()
        };
        let compiling = colors("   Compiling tokio v1.49.0 (/workspace/tokio)");
        assert_eq!(
            compiling[1],
            ("Compiling".to_string(), Some(theme.colors.success))
        );
        assert_eq!(compiling[3], ("tokio".to_string(), Some(theme.colors.text)));
        assert_eq!(compiling[4].1, Some(theme.colors.muted));
        assert_eq!(
            colors("error[E0599]: no variant")[0],
            ("error".to_string(), Some(theme.colors.error))
        );
        assert_eq!(
            colors("warning: unused import")[0].1,
            Some(theme.colors.warning)
        );
        assert_eq!(colors("$ cargo fetch")[1].1, Some(theme.colors.text));
        // Prose that only starts like a status line stays quiet.
        assert_eq!(
            colors("Compiling went fine"),
            [("Compiling went fine".to_string(), Some(theme.colors.muted))]
        );
        assert_eq!(colors("  Note this").len(), 1);
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(850), "850 ms");
        assert_eq!(duration(12_450), "12.4s");
        assert_eq!(duration(185_000), "3m 05s");
    }
}
