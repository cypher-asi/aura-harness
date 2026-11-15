//! Agent activity blocks in the chat: thinking and tool calls, collapsed to
//! a one-line header or expanded to show everything. Consecutive steps are
//! drawn as a group under one summary line (`Explored 4 files, 2 searches`).

use super::command::wrap_spans;
use super::diff::diff_stat_spans;
use super::panels::format_elapsed;
use super::text::{display_width, wrap_words};
use crate::app::format::{file_name, format_tool_label, is_complete_json, parse_tool_args};
use crate::components::{Activity, DiffView, ThinkingStep, ToolStatus, ToolStep};
use crate::Theme;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use std::collections::BTreeSet;
use std::time::Duration;

/// Columns before a block's marker.
const INDENT: &str = "  ";

/// Columns before the marker of a step inside a group.
const NESTED_INDENT: &str = "    ";

/// Thinking lines shown under a collapsed block while it streams.
const LIVE_THINKING_LINES: usize = 3;

/// Output lines shown in an expanded tool block.
const MAX_RESULT_LINES: usize = 200;

/// Streaming tool input at least this large shows its size in the header.
const STREAMING_SIZE_MIN_BYTES: usize = 1024;

/// Thinking shorter than this reads `Thought briefly`.
const BRIEF_THOUGHT_MS: u64 = 2000;

/// How much of a group is shown under its summary line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum GroupView {
    /// Only the summary line
    Collapsed,
    /// The most recent steps, while the group is still running
    Tail,
    /// Every step
    Full,
}

/// Steps shown under a group in [`GroupView::Tail`].
pub(super) const TAIL_STEPS: usize = 4;

/// Lines for an activity block `width` columns wide. `spinner` is drawn
/// in place of the marker while the step is in progress. A `nested` step
/// sits inside a group: indented further, with no marker while collapsed.
pub(super) fn activity_lines(
    activity: &Activity,
    expanded: bool,
    nested: bool,
    spinner: &str,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let indent = if nested { NESTED_INDENT } else { INDENT };
    let marker = if activity.is_running() {
        spinner
    } else if expanded {
        "▾"
    } else if nested {
        " "
    } else {
        "▸"
    };
    let marker = Span::styled(
        format!("{indent}{marker} "),
        Style::default().fg(theme.colors.secondary),
    );
    let body = Body::new(indent, width);
    match activity {
        Activity::Thinking(step) => thinking_lines(step, marker, expanded, &body, theme),
        Activity::Tool(step) => tool_lines(step, marker, expanded, &body, width, theme),
    }
}

/// The summary line of a group of steps: `▸ Explored 4 files, 2 searches`.
pub(super) fn group_header(
    steps: &[&Activity],
    view: GroupView,
    live: bool,
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
        Style::default().fg(theme.colors.secondary),
    );
    let tally = Tally::of(steps);
    let failed = if tally.failed > 0 {
        format!(" · {} failed", tally.failed)
    } else {
        String::new()
    };
    let room = width.saturating_sub(display_width(&marker.content) + display_width(&failed));
    let summary = truncate(&tally.summary(live), room);
    let mut spans = vec![marker];
    spans.extend(verb_spans(&summary, theme));
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

/// `text` with its first word brighter than the rest: `Read` stands out
/// from `main.rs L1-80`.
fn verb_spans(text: &str, theme: &Theme) -> Vec<Span<'static>> {
    let split = text.find(' ').unwrap_or(text.len());
    let (verb, rest) = text.split_at(split);
    vec![
        Span::styled(
            verb.to_string(),
            Style::default().fg(theme.colors.foreground),
        ),
        Span::styled(rest.to_string(), Style::default().fg(theme.colors.muted)),
    ]
}

/// Where a block's body lines go: the `│` rule under its marker.
struct Body {
    prefix: String,
    width: usize,
}

impl Body {
    fn new(indent: &str, width: usize) -> Self {
        let prefix = format!("{indent}  │ ");
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
        Span::styled(
            self.prefix.clone(),
            Style::default()
                .fg(theme.colors.muted)
                .add_modifier(Modifier::DIM),
        )
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
    let mut lines = vec![Line::from(vec![
        marker,
        Span::styled(label, muted.add_modifier(Modifier::ITALIC)),
    ])];

    let text_style = muted.add_modifier(Modifier::DIM | Modifier::ITALIC);
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
                muted.add_modifier(Modifier::ITALIC),
                theme,
            ));
        }
    } else {
        for line in pretty_args(&step.args).lines() {
            body.push_wrapped(&mut lines, line, muted, theme);
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
    let total = result.lines().count();
    for line in result.lines().take(MAX_RESULT_LINES) {
        body.push_wrapped(&mut lines, line, style, theme);
    }
    if total > MAX_RESULT_LINES {
        lines.push(
            body.line(
                format!("… {} more lines", total - MAX_RESULT_LINES),
                Style::default()
                    .fg(theme.colors.muted)
                    .add_modifier(Modifier::ITALIC),
                theme,
            ),
        );
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
    spans.extend(verb_spans(&label, theme));
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
/// input is still streaming in; ` · error: …` once failed.
fn tool_meta(step: &ToolStep) -> String {
    let mut parts = Vec::new();
    match step.status {
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
            false,
            "⠋",
            80,
            &Theme::default(),
        );
        assert_eq!(texts(&lines), ["  ▸ Read main.rs"]);
    }

    #[test]
    fn nested_step_has_no_marker_until_expanded() {
        let step = tool(ToolStatus::Success, Some("fn main() {}"));
        let theme = Theme::default();
        let collapsed = activity_lines(&step, false, true, "⠋", 80, &theme);
        assert_eq!(texts(&collapsed), ["      Read main.rs"]);
        let expanded = activity_lines(&step, true, true, "⠋", 80, &theme);
        assert_eq!(texts(&expanded)[0], "    ▾ Read main.rs");
        assert_eq!(texts(&expanded).last().unwrap(), "      │ fn main() {}");
    }

    #[test]
    fn expanded_tool_shows_args_and_result() {
        let lines = activity_lines(
            &tool(ToolStatus::Error, Some("no such file")),
            true,
            false,
            "⠋",
            80,
            &Theme::default(),
        );
        let text = texts(&lines).join("\n");
        assert!(
            text.starts_with("  ▾ ✗ Read main.rs · no such file"),
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
            false,
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
        let done = activity_lines(
            &Activity::Thinking(step),
            false,
            false,
            "⠹",
            80,
            &Theme::default(),
        );
        assert_eq!(texts(&done), ["  ▸ Thought 4s"]);
    }

    #[test]
    fn short_thinking_is_brief() {
        let lines = activity_lines(&thought(800), false, false, "⠹", 80, &Theme::default());
        assert_eq!(texts(&lines), ["  ▸ Thought briefly"]);
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
        });
        let lines = activity_lines(&activity, false, false, "⠋", 40, &Theme::default());
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
        let lines = activity_lines(&step, false, false, "⠋", 80, &Theme::default());
        assert_eq!(texts(&lines), ["  ⠋ Wrote index.html · 2 KB"]);

        let done = tool_step(
            "write_file",
            r#"{"path":"web/index.html","content":"x"}"#,
            ToolStatus::Running,
            None,
        );
        let lines = activity_lines(&done, false, false, "⠋", 80, &Theme::default());
        assert_eq!(texts(&lines), ["  ⠋ Wrote index.html"]);
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

        let done = group_header(&steps, GroupView::Collapsed, false, "⠹", 80, &theme);
        assert_eq!(
            text(&done),
            "  ▸ Explored 2 files, 2 searches, ran 1 command · 1 failed"
        );
        let live = group_header(&steps[..2], GroupView::Tail, true, "⠹", 80, &theme);
        assert_eq!(text(&live), "  ⠹ Exploring a.rs");
        let open = group_header(&steps[..2], GroupView::Full, false, "⠹", 80, &theme);
        assert_eq!(text(&open), "  ▾ Explored a.rs");
    }

    #[test]
    fn group_of_thinking_sums_its_time() {
        let steps = [thought(3000), thought(9000)];
        let steps: Vec<&Activity> = steps.iter().collect();
        let header = group_header(
            &steps,
            GroupView::Collapsed,
            false,
            "⠹",
            80,
            &Theme::default(),
        );
        assert_eq!(text(&header), "  ▸ Thought 12s");
    }
}
