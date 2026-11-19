//! Agent activity blocks in the chat: thinking and tool calls, collapsed to
//! a one-line header or expanded to show everything. Consecutive steps are
//! drawn as a group under one summary line (`Explored 4 files, 2 searches`).

use super::command::wrap_spans;
use super::diff::diff_stat_spans;
use super::markdown::{parse_content_segments, render_markdown, ContentSegment};
use super::panels::format_elapsed;
use super::text::{display_width, wrap_words};
use crate::app::format::{file_name, format_tool_label, is_complete_json, parse_tool_args};
use crate::components::{Activity, CodeBlock, DiffView, ThinkingStep, ToolStatus, ToolStep};
use crate::Theme;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use std::collections::BTreeSet;
use std::time::Duration;

/// Columns before a block's marker.
const INDENT: &str = "  ";

/// Columns a step inside a group takes before its marker: its number,
/// right-aligned, and a space.
const NUMBER_WIDTH: usize = 5;

/// Thinking lines shown under a collapsed block while it streams.
const LIVE_THINKING_LINES: usize = 3;

/// Output lines shown in an expanded tool block.
const MAX_RESULT_LINES: usize = 200;

/// Streaming tool input at least this large shows its size in the header.
const STREAMING_SIZE_MIN_BYTES: usize = 1024;

/// A list or object argument longer than this, as compact JSON, is
/// pretty-printed under its key.
const INLINE_ARG_MAX: usize = 60;

/// Thinking shorter than this reads `Thought briefly`.
const BRIEF_THOUGHT_MS: u64 = 2000;

/// How much of a group is shown under its summary line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum GroupView {
    /// Only the summary line
    Collapsed,
    /// Every step
    Full,
}

/// Lines for an activity block `width` columns wide. `spinner` is drawn
/// in place of the marker while the step is in progress. A step with a
/// `number` sits inside a group, numbered by its place in the turn.
pub(super) fn activity_lines(
    activity: &Activity,
    expanded: bool,
    number: Option<usize>,
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
    let lead = match number {
        Some(n) => format!("{n:>w$} ", w = NUMBER_WIDTH - 1),
        None => INDENT.to_string(),
    };
    let marker = Span::styled(
        format!("{lead}{marker} "),
        Style::default().fg(theme.colors.faint),
    );
    let body = Body::new(body_indent(number), width);
    match activity {
        Activity::Thinking(step) => thinking_lines(step, marker, expanded, &body, theme),
        Activity::Tool(step) => tool_lines(step, marker, expanded, &body, width, theme),
    }
}

/// Columns before the marker of a step, numbered or not.
fn body_indent(number: Option<usize>) -> usize {
    if number.is_some() {
        NUMBER_WIDTH
    } else {
        INDENT.len()
    }
}

/// `→ records for this turn`, under an open thinking step: the row that
/// jumps to the kernel records the turn wrote.
pub(super) fn records_link(number: Option<usize>, width: usize, theme: &Theme) -> Line<'static> {
    Body::new(body_indent(number), width).line(
        "→ records for this turn".to_string(),
        Style::default().fg(theme.colors.muted),
        theme,
    )
}

/// `5 steps (click to expand)` under a collapsed group's summary line.
pub(super) fn collapsed_hint(steps: usize, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!(
            "{}{} (click to expand)",
            " ".repeat(NUMBER_WIDTH + 2),
            count(steps, "step", "steps")
        ),
        Style::default().fg(theme.colors.muted),
    ))
}

/// The summary line of a group of steps: `▸ Explored 4 files, 2 searches`.
/// `progress` (` · step 7 · 42s`) follows the summary while the group runs.
pub(super) fn group_header(
    steps: &[&Activity],
    view: GroupView,
    live: bool,
    progress: &str,
    spinner: &str,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let marker = if live {
        spinner
    } else if view == GroupView::Collapsed {
        "▸"
    } else {
        "▾"
    };
    let marker = Span::styled(
        format!("{INDENT}{marker} "),
        Style::default().fg(theme.colors.faint),
    );
    let tally = Tally::of(steps);
    let failed = if tally.failed > 0 {
        format!(" · {} failed", tally.failed)
    } else {
        String::new()
    };
    let room = width.saturating_sub(
        display_width(&marker.content) + display_width(progress) + display_width(&failed),
    );
    let summary = truncate(&tally.summary(live), room);
    let mut spans = vec![marker];
    spans.push(Span::styled(
        summary,
        Style::default().fg(theme.colors.muted),
    ));
    spans.push(Span::styled(
        progress.to_string(),
        Style::default().fg(theme.colors.muted),
    ));
    spans.push(Span::styled(
        failed,
        Style::default().fg(theme.colors.error),
    ));
    Line::from(spans)
}

/// What a group's steps add up to.
#[derive(Debug, Default)]
struct Tally {
    /// Paths read, listed or checked
    files: BTreeSet<String>,
    /// Searches by content or file name
    searches: usize,
    /// Paths written, edited or deleted
    edits: BTreeSet<String>,
    /// Commands run
    commands: usize,
    /// Calls to any other tool
    other: usize,
    /// Calls that failed or were denied
    failed: usize,
    /// Total thinking time
    thought_ms: Option<u64>,
}

impl Tally {
    fn of(steps: &[&Activity]) -> Self {
        let mut tally = Self::default();
        for step in steps {
            match step {
                Activity::Thinking(thinking) => {
                    if let Some(ms) = thinking.duration_ms {
                        tally.thought_ms = Some(tally.thought_ms.unwrap_or(0) + ms);
                    }
                }
                Activity::Tool(tool) => tally.add_tool(tool),
            }
        }
        tally
    }

    fn add_tool(&mut self, tool: &ToolStep) {
        if matches!(tool.status, ToolStatus::Error | ToolStatus::Denied) {
            self.failed += 1;
        }
        let path = || {
            parse_tool_args(&tool.args)
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map_or_else(|| ".".to_string(), str::to_string)
        };
        match tool.name.as_str() {
            "read_file" | "list_files" | "stat_file" => {
                self.files.insert(path());
            }
            "search_code" | "find_files" => self.searches += 1,
            "write_file" | "edit_file" | "delete_file" => {
                self.edits.insert(path());
            }
            "run_command" => self.commands += 1,
            _ => self.other += 1,
        }
    }

    /// `Explored 4 files, 2 searches, ran 1 command`; `Exploring …` while
    /// `live`.
    fn summary(&self, live: bool) -> String {
        let tense = |running: &str, done: &str| if live { running } else { done }.to_string();
        let mut explored = Vec::new();
        match self.files.len() {
            0 => {}
            1 => explored.push(
                self.files
                    .first()
                    .map(|path| file_name(path).to_string())
                    .unwrap_or_default(),
            ),
            n => explored.push(count(n, "file", "files")),
        }
        if self.searches > 0 {
            explored.push(count(self.searches, "search", "searches"));
        }

        let mut parts = Vec::new();
        if !explored.is_empty() {
            parts.push(format!(
                "{} {}",
                tense("exploring", "explored"),
                explored.join(", ")
            ));
        }
        if !self.edits.is_empty() {
            parts.push(format!(
                "{} {}",
                tense("editing", "edited"),
                count(self.edits.len(), "file", "files")
            ));
        }
        if self.commands > 0 {
            parts.push(format!(
                "{} {}",
                tense("running", "ran"),
                count(self.commands, "command", "commands")
            ));
        }
        if self.other > 0 {
            parts.push(format!(
                "{} {}",
                tense("using", "used"),
                count(self.other, "tool", "tools")
            ));
        }
        if parts.is_empty() {
            return if live {
                "Thinking…".to_string()
            } else {
                thought_label(self.thought_ms)
            };
        }
        capitalize(&parts.join(", "))
    }
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// `Thought briefly`, `Thought 11s`, `Thought 2m 05s`.
fn thought_label(duration_ms: Option<u64>) -> String {
    match duration_ms {
        None => "Thought".to_string(),
        Some(ms) if ms < BRIEF_THOUGHT_MS => "Thought briefly".to_string(),
        Some(ms) if ms < 60_000 => format!("Thought {}s", ms / 1000),
        Some(ms) => format!("Thought {}", format_elapsed(Duration::from_millis(ms))),
    }
}

/// Where a block's body lines go: the `│` rule under its marker.
struct Body {
    prefix: String,
    width: usize,
}

impl Body {
    /// The rule under a marker `indent` columns in.
    fn new(indent: usize, width: usize) -> Self {
        let prefix = format!("{}  │ ", " ".repeat(indent));
        let width = width.saturating_sub(display_width(&prefix));
        Self { prefix, width }
    }

    fn line(&self, text: String, style: Style, theme: &Theme) -> Line<'static> {
        Line::from(vec![self.rule(theme), Span::styled(text, style)])
    }

    /// `line` (already fitted to the body width) under the rule.
    fn wrap_line(&self, line: Line<'static>, theme: &Theme) -> Line<'static> {
        let mut spans = vec![self.rule(theme)];
        spans.extend(line.spans);
        Line::from(spans)
    }

    fn push_wrapped(
        &self,
        lines: &mut Vec<Line<'static>>,
        text: &str,
        style: Style,
        theme: &Theme,
    ) {
        for row in wrap_spans(vec![Span::styled(text.to_string(), style)], self.width) {
            let mut spans = vec![self.rule(theme)];
            spans.extend(row);
            lines.push(Line::from(spans));
        }
    }

    fn rule(&self, theme: &Theme) -> Span<'static> {
        Span::styled(self.prefix.clone(), Style::default().fg(theme.colors.faint))
    }
}

fn thinking_lines(
    step: &ThinkingStep,
    marker: Span<'static>,
    expanded: bool,
    body: &Body,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let muted = Style::default().fg(theme.colors.muted);
    let label = if step.running {
        let elapsed = live_elapsed(step.started)
            .map(|e| format!(" {e}"))
            .unwrap_or_default();
        format!("Thinking…{elapsed}")
    } else {
        thought_label(step.duration_ms)
    };
    let mut lines = vec![Line::from(vec![marker, Span::styled(label, muted)])];

    let text_style = muted;
    let mut text: Vec<String> = step
        .text
        .trim()
        .lines()
        .flat_map(|line| {
            if line.trim().is_empty() {
                vec![String::new()]
            } else {
                wrap_words(line, body.width)
            }
        })
        .collect();
    if !expanded {
        if !step.running {
            return lines;
        }
        text = text.split_off(text.len().saturating_sub(LIVE_THINKING_LINES));
    } else if text.is_empty() && !step.running {
        lines.push(body.line(
            "the model shared no reasoning text".to_string(),
            text_style,
            theme,
        ));
    }
    lines.extend(
        text.into_iter()
            .map(|text| body.line(text, text_style, theme)),
    );
    lines
}

fn tool_lines(
    step: &ToolStep,
    marker: Span<'static>,
    expanded: bool,
    body: &Body,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let diff = tool_diff(step);
    let mut lines = vec![tool_header(step, marker, width, diff.as_ref(), theme)];
    if !expanded {
        return lines;
    }

    let muted = Style::default().fg(theme.colors.muted);
    if let Some(policy) = &step.policy {
        let color = if step.status == ToolStatus::Denied {
            theme.colors.warning
        } else {
            theme.colors.muted
        };
        body.push_wrapped(
            &mut lines,
            &format!("policy: {policy}"),
            Style::default().fg(color),
            theme,
        );
    }
    if let Some(view) = &diff {
        body.push_wrapped(&mut lines, view.path(), muted, theme);
        let diff_lines = view.styled_lines(theme, body.width);
        let total = diff_lines.len();
        for line in diff_lines.into_iter().take(MAX_RESULT_LINES) {
            lines.push(body.wrap_line(line, theme));
        }
        if total > MAX_RESULT_LINES {
            lines.push(body.line(
                format!("… {} more lines", total - MAX_RESULT_LINES),
                muted,
                theme,
            ));
        }
    } else {
        for line in pretty_args(&step.args).lines() {
            if line.is_empty() {
                lines.push(body.line(String::new(), muted, theme));
            } else {
                body.push_wrapped(&mut lines, line, muted, theme);
            }
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
    if lines.len() > 1 {
        lines.push(body.line(String::new(), Style::default(), theme));
    }
    let style = Style::default().fg(
        if step.status == ToolStatus::Running || step.status == ToolStatus::Success {
            theme.colors.foreground
        } else {
            theme.colors.error
        },
    );
    let total = if step.name == "task" {
        let reply = markdown_lines(result, body.width, style, theme);
        let total = reply.len();
        for line in reply.into_iter().take(MAX_RESULT_LINES) {
            lines.push(body.wrap_line(line, theme));
        }
        total
    } else {
        for line in result.lines().take(MAX_RESULT_LINES) {
            body.push_wrapped(&mut lines, line, style, theme);
        }
        result.lines().count()
    };
    if total > MAX_RESULT_LINES {
        lines.push(body.line(
            format!("… {} more lines", total - MAX_RESULT_LINES),
            Style::default().fg(theme.colors.muted),
            theme,
        ));
    }
    lines
}

/// `▸ Read main.rs L1-80`, `▸ ✗ Ran cargo test · exit 101`, cut to fit on
/// one line.
fn tool_header(
    step: &ToolStep,
    marker: Span<'static>,
    width: usize,
    diff: Option<&DiffView>,
    theme: &Theme,
) -> Line<'static> {
    let (icon, icon_color) = match step.status {
        ToolStatus::Running | ToolStatus::Success => ("", theme.colors.secondary),
        ToolStatus::Error => ("✗ ", theme.colors.error),
        ToolStatus::Denied => ("⊘ ", theme.colors.warning),
    };
    let meta = tool_meta(step);
    let mut stat = Vec::new();
    if let Some(view) = diff.filter(|v| !v.is_empty()) {
        stat.push(Span::styled(" · ", Style::default().fg(theme.colors.muted)));
        stat.extend(diff_stat_spans(view, theme));
    }
    let stat_width: usize = stat.iter().map(|s| display_width(&s.content)).sum();
    let used =
        display_width(&marker.content) + display_width(icon) + display_width(&meta) + stat_width;
    let label = format_tool_label(&step.name, &step.args);
    let label = label.lines().next().unwrap_or_default();
    let label = if label.is_empty() { &step.name } else { label };
    let label = truncate(label, width.saturating_sub(used));

    let mut spans = vec![
        marker,
        Span::styled(icon.to_string(), Style::default().fg(icon_color)),
    ];
    spans.push(Span::styled(label, Style::default().fg(theme.colors.muted)));
    spans.extend(stat);
    spans.push(Span::styled(meta, Style::default().fg(theme.colors.muted)));
    Line::from(spans)
}

/// What a file-changing call did to its file: the diff the harness sent
/// once the call ran, else for `edit_file` the text it replaces against
/// the replacement (also while the arguments stream in, and for calls
/// restored from history). `None` for other calls and failed edits.
fn tool_diff(step: &ToolStep) -> Option<DiffView> {
    let args = parse_tool_args(&step.args);
    let text = |key: &str| args.get(key).and_then(serde_json::Value::as_str);
    let path = text("path").unwrap_or_default();
    if let Some(diff) = &step.diff {
        return Some(DiffView::from_unified(path, diff));
    }
    if step.name != "edit_file" || matches!(step.status, ToolStatus::Error | ToolStatus::Denied) {
        return None;
    }
    Some(DiffView::from_texts(
        path,
        text("old_text")?,
        text("new_text")?,
    ))
}

/// ` · retrying (2/3) · 12s` while running, with ` · 14 KB` while a large
/// input is still streaming in; ` · error: …` once failed. A finished call
/// keeps how long it took once that reaches a second.
fn tool_meta(step: &ToolStep) -> String {
    let mut parts = Vec::new();
    match step.status {
        ToolStatus::Running if step.awaiting_approval => {
            parts.push("awaiting approval".to_string());
        }
        ToolStatus::Running => {
            if let Some((attempt, max)) = step.retry {
                parts.push(format!("retrying ({attempt}/{max})"));
            }
            if step.args.len() >= STREAMING_SIZE_MIN_BYTES && !is_complete_json(&step.args) {
                parts.push(format!("{} KB", step.args.len() / 1024));
            }
            if let Some(elapsed) = live_elapsed(step.started) {
                parts.push(elapsed);
            }
        }
        ToolStatus::Success => {}
        ToolStatus::Error | ToolStatus::Denied => {
            if let Some(first) = step
                .result
                .as_deref()
                .and_then(|r| r.lines().find(|l| !l.trim().is_empty()))
            {
                parts.push(truncate(first.trim(), 60));
            }
        }
    }
    if step.status != ToolStatus::Running {
        parts.extend(
            step.duration_ms
                .filter(|ms| *ms >= 1000)
                .map(|ms| format_elapsed(Duration::from_millis(ms))),
        );
    }
    parts.into_iter().map(|p| format!(" · {p}")).collect()
}

/// `text` as markdown, fenced code drawn as code blocks.
fn markdown_lines(text: &str, width: usize, base: Style, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for segment in parse_content_segments(text) {
        match segment {
            ContentSegment::Text(text) => lines.extend(render_markdown(&text, width, base, theme)),
            ContentSegment::CodeBlock { language, code } => {
                lines.extend(CodeBlock::new(language, code).render(theme, width));
            }
        }
    }
    lines
}

/// Complete JSON object arguments as `key: value` lines, strings unescaped
/// and multi-line values indented under their key; empty for `{}`. Other
/// arguments, such as a prefix still streaming in, are shown as they are.
fn pretty_args(args: &str) -> String {
    use serde_json::Value;
    let map = match serde_json::from_str::<Value>(args) {
        Ok(Value::Object(map)) => map,
        Ok(value) => {
            return serde_json::to_string_pretty(&value).unwrap_or_else(|_| args.to_string())
        }
        Err(_) => return args.trim().to_string(),
    };
    let mut out = Vec::new();
    for (key, value) in &map {
        let text = match value {
            Value::String(text) => text.trim_end().to_string(),
            Value::Array(_) | Value::Object(_) if value.to_string().len() > INLINE_ARG_MAX => {
                serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
            }
            _ => value.to_string(),
        };
        if text.contains('\n') {
            out.push(format!("{key}:"));
            out.extend(text.lines().map(|line| {
                if line.trim().is_empty() {
                    String::new()
                } else {
                    format!("  {line}")
                }
            }));
        } else {
            out.push(format!("{key}: {text}"));
        }
    }
    out.join("\n")
}

/// Time since `started`, once it reaches a second.
fn live_elapsed(started: Option<std::time::Instant>) -> Option<String> {
    started
        .map(|s| s.elapsed())
        .filter(|e| e.as_secs() > 0)
        .map(format_elapsed)
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

    fn text(line: &Line<'_>) -> String {
        texts(std::slice::from_ref(line)).remove(0)
    }

    fn tool_step(name: &str, args: &str, status: ToolStatus, result: Option<&str>) -> Activity {
        Activity::Tool(ToolStep {
            id: "t1".to_string(),
            name: name.to_string(),
            args: args.to_string(),
            status,
            result: result.map(str::to_string),
            retry: None,
            started: None,
            duration_ms: Some(300),
            diff: None,
            policy: None,
            awaiting_approval: false,
        })
    }

    fn tool(status: ToolStatus, result: Option<&str>) -> Activity {
        tool_step("read_file", r#"{"path":"src/main.rs"}"#, status, result)
    }

    fn thought(ms: u64) -> Activity {
        Activity::Thinking(ThinkingStep {
            text: "hmm".to_string(),
            started: None,
            duration_ms: Some(ms),
            running: false,
        })
    }

    #[test]
    fn collapsed_tool_is_one_header_line() {
        let lines = activity_lines(
            &tool(ToolStatus::Success, Some("a\nb\nc")),
            false,
            None,
            "⠋",
            80,
            &Theme::default(),
        );
        assert_eq!(texts(&lines), ["  ▸ Read main.rs"]);
    }

    #[test]
    fn nested_step_is_numbered_with_its_body_under_the_label() {
        let step = tool(ToolStatus::Success, Some("fn main() {}"));
        let theme = Theme::default();
        let collapsed = activity_lines(&step, false, Some(3), "⠋", 80, &theme);
        assert_eq!(texts(&collapsed), ["   3 ▸ Read main.rs"]);
        let expanded = activity_lines(&step, true, Some(12), "⠋", 80, &theme);
        assert_eq!(texts(&expanded)[0], "  12 ▾ Read main.rs");
        assert_eq!(texts(&expanded).last().unwrap(), "       │ fn main() {}");
    }

    #[test]
    fn open_thinking_without_text_says_so() {
        let step = Activity::Thinking(ThinkingStep {
            text: String::new(),
            started: None,
            duration_ms: Some(5000),
            running: false,
        });
        let theme = Theme::default();
        let closed = activity_lines(&step, false, Some(1), "⠋", 80, &theme);
        assert_eq!(texts(&closed), ["   1 ▸ Thought 5s"]);
        let open = activity_lines(&step, true, Some(1), "⠋", 80, &theme);
        assert_eq!(
            texts(&open),
            [
                "   1 ▾ Thought 5s",
                "       │ the model shared no reasoning text"
            ]
        );
        assert_eq!(
            text(&records_link(Some(1), 80, &theme)),
            "       │ → records for this turn"
        );
    }

    #[test]
    fn expanded_tool_shows_args_and_result() {
        let lines = activity_lines(
            &tool(ToolStatus::Error, Some("no such file")),
            true,
            None,
            "⠋",
            80,
            &Theme::default(),
        );
        let text = texts(&lines).join("\n");
        assert!(
            text.starts_with("  ▾ ✗ Read main.rs · no such file"),
            "{text}"
        );
        assert!(text.contains("│ path: src/main.rs"), "{text}");
        assert!(text.ends_with("│ no such file"), "{text}");
    }

    #[test]
    fn expanded_task_shows_its_prompt_as_text_and_its_reply_as_markdown() {
        let args = serde_json::json!({
            "subagent_type": "explore",
            "prompt": "Read-only.\n\nDo two things.",
        });
        let step = tool_step(
            "task",
            &args.to_string(),
            ToolStatus::Success,
            Some("**Done**\n\n| a | b |\n|---|---|\n| 1 | 2 |"),
        );
        let lines = activity_lines(&step, true, None, "⠋", 80, &Theme::default());
        let text = texts(&lines).join("\n");
        assert!(
            text.starts_with("  ▾ Delegated to explore: Read-only."),
            "{text}"
        );
        assert!(
            text.contains("│ prompt:\n    │   Read-only.\n    │ \n    │   Do two things."),
            "{text}"
        );
        assert!(text.contains("│ subagent_type: explore"), "{text}");
        assert!(!text.contains("**"), "{text}");
        assert!(!text.contains("|---|"), "{text}");
    }

    #[test]
    fn policy_shows_only_when_expanded() {
        let mut step = tool(ToolStatus::Success, Some("fn main() {}"));
        if let Activity::Tool(tool) = &mut step {
            tool.policy = Some("allowed by user.on".to_string());
        }
        let theme = Theme::default();
        let collapsed = activity_lines(&step, false, None, "⠋", 80, &theme);
        assert_eq!(texts(&collapsed), ["  ▸ Read main.rs"]);
        let expanded = activity_lines(&step, true, None, "⠋", 80, &theme);
        assert_eq!(texts(&expanded)[1], "    │ policy: allowed by user.on");
    }

    #[test]
    fn expanded_result_is_capped() {
        let output: Vec<String> = (0..250).map(|i| format!("line {i}")).collect();
        let lines = activity_lines(
            &tool(ToolStatus::Success, Some(&output.join("\n"))),
            true,
            None,
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
            None,
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
        let done = activity_lines(
            &Activity::Thinking(step),
            false,
            None,
            "⠹",
            80,
            &Theme::default(),
        );
        assert_eq!(texts(&done), ["  ▸ Thought 4s"]);
    }

    #[test]
    fn short_thinking_is_brief() {
        let lines = activity_lines(&thought(800), false, None, "⠹", 80, &Theme::default());
        assert_eq!(texts(&lines), ["  ▸ Thought briefly"]);
    }

    #[test]
    fn a_call_awaiting_approval_says_so_instead_of_counting_up() {
        let mut activity = tool_step("edit_file", r#"{"path":"a.ts"}"#, ToolStatus::Running, None);
        if let Activity::Tool(step) = &mut activity {
            step.started = Some(std::time::Instant::now() - std::time::Duration::from_secs(90));
            step.awaiting_approval = true;
        }
        let lines = activity_lines(&activity, false, None, "⠋", 80, &Theme::default());
        let header = &texts(&lines)[0];
        assert!(header.ends_with(" · awaiting approval"), "{header}");
        assert!(!header.contains("1m"), "{header}");
    }

    #[test]
    fn long_label_is_cut_to_width() {
        let activity = Activity::Tool(ToolStep {
            id: "t1".to_string(),
            name: "run_command".to_string(),
            args: format!(r#"{{"command":"{}"}}"#, "x".repeat(200)),
            status: ToolStatus::Running,
            result: None,
            retry: Some((2, 3)),
            started: None,
            duration_ms: None,
            diff: None,
            policy: None,
            awaiting_approval: false,
        });
        let lines = activity_lines(&activity, false, None, "⠋", 40, &Theme::default());
        let header = &texts(&lines)[0];
        assert_eq!(display_width(header), 40, "{header}");
        assert!(header.starts_with("  ⠋ Ran xxx"), "{header}");
        assert!(header.ends_with("… · retrying (2/3)"), "{header}");
    }

    #[test]
    fn streaming_write_shows_its_file_and_size() {
        let args = format!(
            r#"{{"path":"web/index.html","content":"{}"#,
            "x".repeat(3000)
        );
        let step = tool_step("write_file", &args, ToolStatus::Running, None);
        let lines = activity_lines(&step, false, None, "⠋", 80, &Theme::default());
        assert_eq!(texts(&lines), ["  ⠋ Wrote index.html · 2 KB"]);

        let done = tool_step(
            "write_file",
            r#"{"path":"web/index.html","content":"x"}"#,
            ToolStatus::Running,
            None,
        );
        let lines = activity_lines(&done, false, None, "⠋", 80, &Theme::default());
        assert_eq!(texts(&lines), ["  ⠋ Wrote index.html"]);
    }

    #[test]
    fn finished_tool_keeps_how_long_it_took() {
        let mut step = tool_step(
            "write_file",
            r#"{"path":"src/types.ts","content":"x"}"#,
            ToolStatus::Success,
            None,
        );
        if let Activity::Tool(tool) = &mut step {
            tool.duration_ms = Some(28_400);
        }
        let lines = activity_lines(&step, false, Some(2), "⠋", 80, &Theme::default());
        assert_eq!(texts(&lines), ["   2 ▸ Wrote types.ts · 28s"]);
    }

    #[test]
    fn group_header_counts_what_the_steps_did() {
        let read = |path: &str| {
            tool_step(
                "read_file",
                &format!(r#"{{"path":"{path}"}}"#),
                ToolStatus::Success,
                None,
            )
        };
        let search = tool_step(
            "search_code",
            r#"{"pattern":"x"}"#,
            ToolStatus::Success,
            None,
        );
        let failed = tool_step(
            "run_command",
            r#"{"command":"ls"}"#,
            ToolStatus::Error,
            None,
        );
        let steps = [
            thought(500),
            read("a.rs"),
            read("b.rs"),
            read("a.rs"),
            search.clone(),
            search,
            failed,
        ];
        let steps: Vec<&Activity> = steps.iter().collect();
        let theme = Theme::default();

        let done = group_header(&steps, GroupView::Collapsed, false, "", "⠹", 80, &theme);
        assert_eq!(
            text(&done),
            "  ▸ Explored 2 files, 2 searches, ran 1 command · 1 failed"
        );
        let live = group_header(
            &steps[..2],
            GroupView::Full,
            true,
            " · step 2 · 9s",
            "⠹",
            80,
            &theme,
        );
        assert_eq!(text(&live), "  ⠹ Exploring a.rs · step 2 · 9s");
        let open = group_header(&steps[..2], GroupView::Full, false, "", "⠹", 80, &theme);
        assert_eq!(text(&open), "  ▾ Explored a.rs");
    }

    #[test]
    fn collapsed_hint_counts_the_steps() {
        let theme = Theme::default();
        assert_eq!(
            text(&collapsed_hint(5, &theme)),
            "       5 steps (click to expand)"
        );
    }

    #[test]
    fn group_of_thinking_sums_its_time() {
        let steps = [thought(3000), thought(9000)];
        let steps: Vec<&Activity> = steps.iter().collect();
        let header = group_header(
            &steps,
            GroupView::Collapsed,
            false,
            "",
            "⠹",
            80,
            &Theme::default(),
        );
        assert_eq!(text(&header), "  ▸ Thought 12s");
    }
}
