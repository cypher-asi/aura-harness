//! Agent activity blocks in the chat: thinking and tool calls, collapsed to
//! a one-line header or expanded to show everything.

use super::command::wrap_spans;
use super::panels::format_elapsed;
use super::text::{display_width, wrap_words};
use crate::app::format::format_tool_summary;
use crate::components::{Activity, ThinkingStep, ToolStatus, ToolStep};
use crate::Theme;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use std::time::Duration;

/// Columns before a block's marker.
const INDENT: &str = "  ";

/// Prefix of a block's body lines, under the marker.
const BODY_PREFIX: &str = "    │ ";

/// Thinking lines shown under a collapsed block while it streams.
const LIVE_THINKING_LINES: usize = 3;

/// Output lines shown in an expanded tool block.
const MAX_RESULT_LINES: usize = 200;

/// Lines for an activity block `width` columns wide. `spinner` is drawn
/// in place of the marker while the step is in progress.
pub(super) fn activity_lines(
    activity: &Activity,
    expanded: bool,
    spinner: &str,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let marker = if activity.is_running() {
        spinner
    } else if expanded {
        "▾"
    } else {
        "▸"
    };
    let marker = Span::styled(
        format!("{INDENT}{marker} "),
        Style::default().fg(theme.colors.secondary),
    );
    match activity {
        Activity::Thinking(step) => thinking_lines(step, marker, expanded, width, theme),
        Activity::Tool(step) => tool_lines(step, marker, expanded, width, theme),
    }
}

fn thinking_lines(
    step: &ThinkingStep,
    marker: Span<'static>,
    expanded: bool,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let muted = Style::default().fg(theme.colors.muted);
    let label = if step.running {
        let elapsed = live_elapsed(step.started)
            .map(|e| format!(" {e}"))
            .unwrap_or_default();
        format!("Thinking…{elapsed}")
    } else {
        step.duration_ms.map_or_else(
            || "Thought".to_string(),
            |ms| format!("Thought for {}", short_duration(ms)),
        )
    };
    let mut lines = vec![Line::from(vec![
        marker,
        Span::styled(label, muted.add_modifier(Modifier::ITALIC)),
    ])];

    let body_width = width.saturating_sub(display_width(BODY_PREFIX));
    let text_style = muted.add_modifier(Modifier::DIM | Modifier::ITALIC);
    let mut body: Vec<String> = step
        .text
        .trim()
        .lines()
        .flat_map(|line| {
            if line.trim().is_empty() {
                vec![String::new()]
            } else {
                wrap_words(line, body_width)
            }
        })
        .collect();
    if !expanded {
        if !step.running {
            return lines;
        }
        body = body.split_off(body.len().saturating_sub(LIVE_THINKING_LINES));
    }
    lines.extend(
        body.into_iter()
            .map(|text| body_line(text, text_style, theme)),
    );
    lines
}

fn tool_lines(
    step: &ToolStep,
    marker: Span<'static>,
    expanded: bool,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = vec![tool_header(step, marker, width, theme)];
    if !expanded {
        return lines;
    }

    let body_width = width.saturating_sub(display_width(BODY_PREFIX));
    let args = pretty_args(&step.args);
    if !args.is_empty() {
        let style = Style::default().fg(theme.colors.muted);
        for line in args.lines() {
            push_wrapped(&mut lines, line, style, body_width, theme);
        }
    }

    let Some(result) = step.result.as_deref() else {
        return lines;
    };
    let result = crate::ansi::strip_ansi(result);
    let result = result.trim_end();
    if result.is_empty() {
        return lines;
    }
    if !args.is_empty() {
        lines.push(body_line(String::new(), Style::default(), theme));
    }
    let style = Style::default().fg(
        if step.status == ToolStatus::Running || step.status == ToolStatus::Success {
            theme.colors.foreground
        } else {
            theme.colors.error
        },
    );
    let total = result.lines().count();
    for line in result.lines().take(MAX_RESULT_LINES) {
        push_wrapped(&mut lines, line, style, body_width, theme);
    }
    if total > MAX_RESULT_LINES {
        lines.push(body_line(
            format!("… {} more lines", total - MAX_RESULT_LINES),
            Style::default()
                .fg(theme.colors.muted)
                .add_modifier(Modifier::ITALIC),
            theme,
        ));
    }
    lines
}

/// `▸ ✓ read_file src/main.rs · 0.3s`, cut to fit on one line.
fn tool_header(
    step: &ToolStep,
    marker: Span<'static>,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let (icon, icon_color) = match step.status {
        ToolStatus::Running => ("", theme.colors.secondary),
        ToolStatus::Success => ("✓ ", theme.colors.success),
        ToolStatus::Error => ("✗ ", theme.colors.error),
        ToolStatus::Denied => ("⊘ ", theme.colors.warning),
    };
    let meta = tool_meta(step);
    let name = step.name.clone();
    let used = display_width(&marker.content)
        + display_width(icon)
        + display_width(&name)
        + display_width(&meta);
    let summary = format_tool_summary(&step.name, &step.args);
    let summary = summary.lines().next().unwrap_or_default();
    let summary = if summary.is_empty() {
        String::new()
    } else {
        truncate(&format!(" {summary}"), width.saturating_sub(used))
    };

    Line::from(vec![
        marker,
        Span::styled(icon.to_string(), Style::default().fg(icon_color)),
        Span::styled(
            name,
            Style::default()
                .fg(theme.colors.foreground)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(summary, Style::default().fg(theme.colors.foreground)),
        Span::styled(meta, Style::default().fg(theme.colors.muted)),
    ])
}

/// ` · retrying (2/3) · 12s`, ` · 0.3s`, ` · 3 lines`, ` · error: …`.
fn tool_meta(step: &ToolStep) -> String {
    let mut parts = Vec::new();
    match step.status {
        ToolStatus::Running => {
            if let Some((attempt, max)) = step.retry {
                parts.push(format!("retrying ({attempt}/{max})"));
            }
            if let Some(elapsed) = live_elapsed(step.started) {
                parts.push(elapsed);
            }
        }
        ToolStatus::Success => {
            let lines = step
                .result
                .as_deref()
                .map_or(0, |r| r.trim_end().lines().count());
            if lines > 1 {
                parts.push(format!("{lines} lines"));
            }
            parts.extend(step.duration_ms.map(short_duration));
        }
        ToolStatus::Error | ToolStatus::Denied => {
            if let Some(first) = step
                .result
                .as_deref()
                .and_then(|r| r.lines().find(|l| !l.trim().is_empty()))
            {
                parts.push(truncate(first.trim(), 60));
            }
            parts.extend(step.duration_ms.map(short_duration));
        }
    }
    parts.into_iter().map(|p| format!(" · {p}")).collect()
}

/// Arguments pretty-printed when they are complete JSON; empty for `{}`.
fn pretty_args(args: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(args) {
        Ok(serde_json::Value::Object(map)) if map.is_empty() => String::new(),
        Ok(value) => serde_json::to_string_pretty(&value).unwrap_or_else(|_| args.to_string()),
        Err(_) => args.trim().to_string(),
    }
}

fn push_wrapped(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    style: Style,
    width: usize,
    theme: &Theme,
) {
    for row in wrap_spans(vec![Span::styled(text.to_string(), style)], width) {
        let mut spans = vec![body_prefix(theme)];
        spans.extend(row);
        lines.push(Line::from(spans));
    }
}

fn body_line(text: String, style: Style, theme: &Theme) -> Line<'static> {
    Line::from(vec![body_prefix(theme), Span::styled(text, style)])
}

fn body_prefix(theme: &Theme) -> Span<'static> {
    Span::styled(
        BODY_PREFIX,
        Style::default()
            .fg(theme.colors.muted)
            .add_modifier(Modifier::DIM),
    )
}

/// Time since `started`, once it reaches a second.
fn live_elapsed(started: Option<std::time::Instant>) -> Option<String> {
    started
        .map(|s| s.elapsed())
        .filter(|e| e.as_secs() > 0)
        .map(format_elapsed)
}

/// `250ms`, `1.4s`, `2m 05s`.
fn short_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{}.{}s", ms / 1000, (ms % 1000) / 100)
    } else {
        format_elapsed(Duration::from_millis(ms))
    }
}

/// `text` cut to `max` columns, ending in `…` when cut.
fn truncate(text: &str, max: usize) -> String {
    if display_width(text) <= max {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w + 1 > max {
            break;
        }
        out.push(c);
        used += w;
    }
    if max > 0 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::themes::Theme;

    fn texts(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    fn tool(status: ToolStatus, result: Option<&str>) -> Activity {
        Activity::Tool(ToolStep {
            id: "t1".to_string(),
            name: "read_file".to_string(),
            args: r#"{"path":"src/main.rs"}"#.to_string(),
            status,
            result: result.map(str::to_string),
            retry: None,
            started: None,
            duration_ms: Some(300),
        })
    }

    #[test]
    fn collapsed_tool_is_one_header_line() {
        let lines = activity_lines(
            &tool(ToolStatus::Success, Some("a\nb\nc")),
            false,
            "⠋",
            80,
            &Theme::default(),
        );
        assert_eq!(
            texts(&lines),
            ["  ▸ ✓ read_file src/main.rs · 3 lines · 300ms"]
        );
    }

    #[test]
    fn expanded_tool_shows_args_and_result() {
        let lines = activity_lines(
            &tool(ToolStatus::Error, Some("no such file")),
            true,
            "⠋",
            80,
            &Theme::default(),
        );
        let text = texts(&lines).join("\n");
        assert!(
            text.starts_with("  ▾ ✗ read_file src/main.rs · no such file"),
            "{text}"
        );
        assert!(text.contains("│   \"path\": \"src/main.rs\""), "{text}");
        assert!(text.ends_with("│ no such file"), "{text}");
    }

    #[test]
    fn expanded_result_is_capped() {
        let output: Vec<String> = (0..250).map(|i| format!("line {i}")).collect();
        let lines = activity_lines(
            &tool(ToolStatus::Success, Some(&output.join("\n"))),
            true,
            "⠋",
            80,
            &Theme::default(),
        );
        assert_eq!(texts(&lines).last().unwrap(), "    │ … 50 more lines");
    }

    #[test]
    fn running_thinking_shows_its_tail_and_finished_collapses() {
        let mut step = ThinkingStep {
            text: "one\ntwo\nthree\nfour".to_string(),
            started: None,
            duration_ms: None,
            running: true,
        };
        let live = activity_lines(
            &Activity::Thinking(step.clone()),
            false,
            "⠹",
            80,
            &Theme::default(),
        );
        assert_eq!(
            texts(&live),
            ["  ⠹ Thinking…", "    │ two", "    │ three", "    │ four"]
        );

        step.running = false;
        step.duration_ms = Some(4200);
        let done = activity_lines(&Activity::Thinking(step), false, "⠹", 80, &Theme::default());
        assert_eq!(texts(&done), ["  ▸ Thought for 4.2s"]);
    }

    #[test]
    fn long_summary_is_cut_to_width() {
        let activity = Activity::Tool(ToolStep {
            id: "t1".to_string(),
            name: "run_command".to_string(),
            args: format!(r#"{{"command":"{}"}}"#, "x".repeat(200)),
            status: ToolStatus::Running,
            result: None,
            retry: Some((2, 3)),
            started: None,
            duration_ms: None,
        });
        let lines = activity_lines(&activity, false, "⠋", 40, &Theme::default());
        let header = &texts(&lines)[0];
        assert_eq!(display_width(header), 40, "{header}");
        assert!(header.ends_with("… · retrying (2/3)"), "{header}");
    }
}
