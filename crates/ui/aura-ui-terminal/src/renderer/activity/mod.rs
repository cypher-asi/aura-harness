//! Agent activity blocks in the chat: thinking and tool calls, collapsed to
//! a one-line header or expanded to show everything. Consecutive steps are
//! drawn as a group under one summary line (`Explored 4 files, 2 searches`).

use super::chat_panel::format_elapsed;
use super::command::{script_lines, wrap_spans};
use super::diff::diff_stat_spans;
use super::markdown::{parse_content_segments, render_markdown, ContentSegment};
use super::text::{display_width, wrap_words};
use super::{more_label, MoreRows};
use crate::app::format::{file_name, format_tool_label_of, parse_tool_args, ToolArgs};
use crate::components::{
    Activity, CodeBlock, DiffView, Message, MoreSection, ThinkingStep, ToolStatus, ToolStep,
};
use crate::Theme;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use std::collections::BTreeSet;
use std::time::Duration;

/// Columns before a block's marker.
const INDENT: &str = "  ";

/// Columns before a block's label: the indent, the marker and a space.
/// Steps inside a group, and the hint under a collapsed one, start here,
/// so they sit under the group's summary rather than beside its marker.
const LABEL_COLUMN: usize = INDENT.len() + 2;

/// A step's number inside a group. Numbers are right-aligned to the
/// widest one in the group, so every marker in it shares one column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct StepNumber {
    value: usize,
    width: usize,
}

impl StepNumber {
    /// Step `value` of a group whose last step is numbered `last`.
    pub(super) fn new(value: usize, last: usize) -> Self {
        Self {
            value,
            width: digits(last.max(value)),
        }
    }

    /// Columns the step takes before its marker.
    fn lead_width(self) -> usize {
        LABEL_COLUMN + self.width + 1
    }
}

/// Decimal digits in `n`.
fn digits(n: usize) -> usize {
    n.checked_ilog10().map_or(1, |log| log as usize + 1)
}

/// Thinking lines shown under a collapsed block while it streams.
const LIVE_THINKING_LINES: usize = 12;

/// Output lines shown in an expanded tool block.
const MAX_RESULT_LINES: usize = 200;

/// Lines of a running command's output shown under its collapsed block.
const LIVE_PREVIEW_LINES: usize = 3;

/// Lines of a running command's output shown in its open block.
const LIVE_OUTPUT_LINES: usize = 20;

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

/// How much of a step is drawn: whether it is open, and how many lines
/// of what it returned and of its diff past where they are cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct Shown {
    pub expanded: bool,
    pub result: usize,
    pub diff: usize,
}

impl Shown {
    /// As much of `message`'s step as the user opened.
    pub(super) const fn of(message: &Message) -> Self {
        Self {
            expanded: message.is_expanded(),
            result: message.more_lines(MoreSection::Result),
            diff: message.more_lines(MoreSection::Diff),
        }
    }
}

impl From<bool> for Shown {
    fn from(expanded: bool) -> Self {
        Self {
            expanded,
            ..Self::default()
        }
    }
}

/// Lines for an activity block `width` columns wide, and its rows that
/// show more of a part cut short. `spinner` is drawn in place of the
/// marker while the step is in progress. A step with a `number` sits
/// inside a group, numbered by its place in the turn.
pub(super) fn activity_lines(
    activity: &Activity,
    shown: Shown,
    number: Option<StepNumber>,
    spinner: &str,
    width: usize,
    theme: &Theme,
) -> (Vec<Line<'static>>, MoreRows) {
    let expanded = shown.expanded;
    let marker = step_marker(activity.is_running(), expanded, number, spinner, theme);
    let body = Body::new(body_indent(number), width);
    match activity {
        Activity::Thinking(step) => (
            thinking_lines(step, marker, expanded, &body, theme),
            MoreRows::new(),
        ),
        Activity::Tool(step) => {
            ToolParts::new(step, shown, &body, theme).into_lines(step, marker, width, theme)
        }
    }
}

/// The lines of a running tool step from `parts`, which [`ToolParts::of`]
/// made from the step as it is: only its header, which turns `spinner`
/// and counts the seconds, is drawn afresh. The same lines as
/// [`activity_lines`] draws.
pub(super) fn running_tool_lines(
    step: &ToolStep,
    parts: &ToolParts,
    number: Option<StepNumber>,
    spinner: &str,
    width: usize,
    theme: &Theme,
) -> (Vec<Line<'static>>, MoreRows) {
    let marker = step_marker(
        step.status == ToolStatus::Running,
        parts.expanded,
        number,
        spinner,
        theme,
    );
    parts.lines(step, marker, width, theme)
}

/// The marker before a step's label: `spinner` while it runs, else an
/// arrow that says whether it is open, after its number in a group.
fn step_marker(
    running: bool,
    expanded: bool,
    number: Option<StepNumber>,
    spinner: &str,
    theme: &Theme,
) -> Span<'static> {
    let marker = if running {
        spinner
    } else if expanded {
        "▾"
    } else {
        "▸"
    };
    let lead = match number {
        Some(n) => format!(
            "{:pad$}{:>w$} ",
            "",
            n.value,
            pad = LABEL_COLUMN,
            w = n.width
        ),
        None => INDENT.to_string(),
    };
    Span::styled(
        format!("{lead}{marker} "),
        Style::default().fg(theme.colors.faint),
    )
}

/// Columns before the marker of a step, numbered or not.
fn body_indent(number: Option<StepNumber>) -> usize {
    number.map_or(INDENT.len(), StepNumber::lead_width)
}

/// `→ records for this turn`, under an open thinking step: the row that
/// jumps to the kernel records the turn wrote.
pub(super) fn records_link(
    number: Option<StepNumber>,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
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
            " ".repeat(LABEL_COLUMN),
            count(steps, "step", "steps")
        ),
        Style::default().fg(theme.colors.muted),
    ))
}

/// The summary line of a group of steps: `▸ Explored 4 files, 2 searches`.
/// `progress` (` · step 7 · 42s`) follows the summary while the group runs.
#[cfg(test)]
pub(super) fn group_header(
    steps: &[&Activity],
    view: GroupView,
    live: bool,
    progress: &str,
    spinner: &str,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let tally = Tally::of(steps.iter().map(|step| StepTally::of(step)));
    group_header_of(&tally, view, live, progress, spinner, width, theme)
}

/// [`group_header`] for steps already added up into `tally`.
pub(super) fn group_header_of(
    tally: &Tally,
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

/// What one step adds to its group's [`Tally`]. Finding a tool's path
/// parses its arguments, which can be hundreds of KB for a write, so a
/// live group keeps each step's from frame to frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StepTally {
    kind: StepKind,
    /// Whether the call failed or was denied
    failed: bool,
}

/// What kind of work a step was, for its group's summary.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StepKind {
    /// Thinking, and for how long once done
    Thought(Option<u64>),
    /// A path read, listed or checked
    Explored(String),
    Searched,
    /// A path written, edited or deleted
    Edited(String),
    Ran,
    Other,
}

impl StepTally {
    pub(super) fn of(step: &Activity) -> Self {
        let tool = match step {
            Activity::Thinking(thinking) => {
                return Self {
                    kind: StepKind::Thought(thinking.duration_ms),
                    failed: false,
                }
            }
            Activity::Tool(tool) => tool,
        };
        let path = || {
            parse_tool_args(&tool.args)
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map_or_else(|| ".".to_string(), str::to_string)
        };
        let kind = match tool.name.as_str() {
            "read_file" | "list_files" | "stat_file" => StepKind::Explored(path()),
            "search_code" | "find_files" => StepKind::Searched,
            "write_file" | "edit_file" | "delete_file" => StepKind::Edited(path()),
            "run_command" => StepKind::Ran,
            _ => StepKind::Other,
        };
        Self {
            kind,
            failed: matches!(tool.status, ToolStatus::Error | ToolStatus::Denied),
        }
    }
}

/// What a group's steps add up to.
#[derive(Debug, Default)]
pub(super) struct Tally {
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
    /// What `steps` add up to, each as [`StepTally::of`] it.
    pub(super) fn of<S: std::borrow::Borrow<StepTally>>(
        steps: impl IntoIterator<Item = S>,
    ) -> Self {
        let mut tally = Self::default();
        for step in steps {
            tally.add(step.borrow());
        }
        tally
    }

    fn add(&mut self, step: &StepTally) {
        if step.failed {
            self.failed += 1;
        }
        match &step.kind {
            StepKind::Thought(ms) => {
                if let Some(ms) = ms {
                    self.thought_ms = Some(self.thought_ms.unwrap_or(0) + ms);
                }
            }
            StepKind::Explored(path) => {
                self.files.insert(path.clone());
            }
            StepKind::Searched => self.searches += 1,
            StepKind::Edited(path) => {
                self.edits.insert(path.clone());
            }
            StepKind::Ran => self.commands += 1,
            StepKind::Other => self.other += 1,
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

/// What a tool step is drawn from that changes only with the step: its
/// label, its diff stats and everything under its header. Working these
/// out parses the call's arguments, which grow to hundreds of KB while a
/// file's content streams in, and diffs an edit's texts, so a running
/// step keeps its parts from frame to frame and only its header, with
/// the spinner and the seconds, is drawn again.
#[derive(Debug, Clone)]
pub(super) struct ToolParts {
    /// Whether the step is drawn open
    expanded: bool,
    /// First line of the step's label, before it is cut to fit
    label: String,
    /// ` · +12 -3` after the label, for a call that changed a file
    stat: Vec<Span<'static>>,
    /// Size in KB of a large input still streaming in
    streaming_kb: Option<usize>,
    /// The lines under the header while the step is open
    body: Vec<Line<'static>>,
    /// Rows of `body` that show more of a part cut short
    more: MoreRows,
}

impl ToolParts {
    /// The parts of `step` drawn `width` columns wide, numbered `number`
    /// inside a group.
    pub(super) fn of(
        step: &ToolStep,
        shown: Shown,
        number: Option<StepNumber>,
        width: usize,
        theme: &Theme,
    ) -> Self {
        let body = Body::new(body_indent(number), width);
        Self::new(step, shown, &body, theme)
    }

    fn new(step: &ToolStep, shown: Shown, body: &Body, theme: &Theme) -> Self {
        let expanded = shown.expanded;
        let args = ToolArgs::parse(&step.args);
        let diff = tool_diff(step, &args.value);
        let mut stat = Vec::new();
        if let Some(view) = diff.as_ref().filter(|v| !v.is_empty()) {
            stat.push(Span::styled(" · ", Style::default().fg(theme.colors.muted)));
            stat.extend(diff_stat_spans(view, theme));
        }
        let label = format_tool_label_of(&step.name, &args);
        let label = match label.lines().next() {
            Some(first) if !first.is_empty() => first.to_string(),
            _ => step.name.clone(),
        };
        let streaming_kb = (step.status == ToolStatus::Running
            && !step.awaiting_approval
            && step.args.len() >= STREAMING_SIZE_MIN_BYTES
            && !args.complete)
            .then_some(step.args.len() / 1024);
        let (body, more) = if expanded {
            tool_body(step, diff.as_ref(), body, shown, theme)
        } else {
            (live_preview(step, body, theme), MoreRows::new())
        };
        Self {
            expanded,
            label,
            stat,
            streaming_kb,
            body,
            more,
        }
    }

    /// The rows that show more, counted from the header.
    fn more_rows(&self) -> MoreRows {
        self.more
            .iter()
            .map(|&(row, section)| (row + 1, section))
            .collect()
    }

    /// The step's lines: its header after `marker`, then its body.
    fn lines(
        &self,
        step: &ToolStep,
        marker: Span<'static>,
        width: usize,
        theme: &Theme,
    ) -> (Vec<Line<'static>>, MoreRows) {
        let mut lines = Vec::with_capacity(1 + self.body.len());
        lines.push(tool_header(step, self, marker, width, theme));
        lines.extend(self.body.iter().cloned());
        (lines, self.more_rows())
    }

    /// [`Self::lines`], taking the body rather than copying it.
    fn into_lines(
        mut self,
        step: &ToolStep,
        marker: Span<'static>,
        width: usize,
        theme: &Theme,
    ) -> (Vec<Line<'static>>, MoreRows) {
        let body = std::mem::take(&mut self.body);
        let mut lines = Vec::with_capacity(1 + body.len());
        lines.push(tool_header(step, &self, marker, width, theme));
        lines.extend(body);
        (lines, self.more_rows())
    }
}

/// What an open tool step shows under its header: the policy that let it
/// run, its diff or arguments, then what it returned, each cut where
/// `shown` says; and the rows that show more of them.
fn tool_body(
    step: &ToolStep,
    diff: Option<&DiffView>,
    body: &Body,
    shown: Shown,
    theme: &Theme,
) -> (Vec<Line<'static>>, MoreRows) {
    let mut more = MoreRows::new();
    let mut lines = Vec::new();
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
    if let Some(view) = diff {
        body.push_wrapped(&mut lines, view.path(), muted, theme);
        let diff_lines = view.styled_lines(theme, body.width);
        let total = diff_lines.len();
        let cap = MAX_RESULT_LINES + shown.diff;
        for line in diff_lines.into_iter().take(cap) {
            lines.push(body.wrap_line(line, theme));
        }
        if total > cap {
            more.push((lines.len(), MoreSection::Diff));
            lines.push(body.line(more_label(total - cap), muted, theme));
        }
    } else if let Some(command) = (step.name == "run_command")
        .then(|| command_lines(&step.args, body, theme))
        .flatten()
    {
        lines.extend(command);
    } else {
        push_args(&mut lines, &step.args, body, theme);
    }

    if step.status == ToolStatus::Running && !step.live.is_empty() {
        if !lines.is_empty() {
            lines.push(body.line(String::new(), Style::default(), theme));
        }
        let style = Style::default().fg(theme.colors.foreground);
        for line in step.live.tail(LIVE_OUTPUT_LINES) {
            body.push_wrapped(&mut lines, &line, style, theme);
        }
    }

    let Some(result) = step.result.as_deref() else {
        return (lines, more);
    };
    let result = crate::ansi::strip_ansi(result);
    let result = result.trim_end();
    if result.is_empty() {
        return (lines, more);
    }
    if !lines.is_empty() {
        lines.push(body.line(String::new(), Style::default(), theme));
    }
    let style = Style::default().fg(
        if step.status == ToolStatus::Running || step.status == ToolStatus::Success {
            theme.colors.foreground
        } else {
            theme.colors.error
        },
    );
    let cap = MAX_RESULT_LINES + shown.result;
    let total = if step.name == "task" {
        let reply = markdown_lines(result, body.width, style, theme);
        let total = reply.len();
        for line in reply.into_iter().take(cap) {
            lines.push(body.wrap_line(line, theme));
        }
        total
    } else {
        for line in result.lines().take(cap) {
            body.push_wrapped(&mut lines, line, style, theme);
        }
        result.lines().count()
    };
    if total > cap {
        more.push((lines.len(), MoreSection::Result));
        lines.push(body.line(
            more_label(total - cap),
            Style::default().fg(theme.colors.muted),
            theme,
        ));
    }
    (lines, more)
}

/// `args` as [`pretty_args`] lays them out, under the rule.
fn push_args(lines: &mut Vec<Line<'static>>, args: &str, body: &Body, theme: &Theme) {
    let muted = Style::default().fg(theme.colors.muted);
    for line in pretty_args(args).lines() {
        if line.is_empty() {
            lines.push(body.line(String::new(), muted, theme));
        } else {
            body.push_wrapped(lines, line, muted, theme);
        }
    }
}

/// A `run_command` call's arguments: the command as `$ …` rows, then how
/// it ran (`in web · background · timeout 2m 00s`) and any other
/// arguments. `None` while they stream in or when they hold no command.
fn command_lines(args: &str, body: &Body, theme: &Theme) -> Option<Vec<Line<'static>>> {
    use serde_json::Value;
    let Ok(Value::Object(mut args)) = serde_json::from_str::<Value>(args) else {
        return None;
    };
    let mut take = |key: &str| args.remove(key);
    let command = match (take("shell_script"), take("command")) {
        (Some(Value::String(script)), _) | (None, Some(Value::String(script))) => script,
        _ => {
            let program = take("program")?;
            let words = take("args")
                .and_then(|a| a.as_array().cloned())
                .unwrap_or_default();
            std::iter::once(program.as_str()?.to_string())
                .chain(words.iter().filter_map(Value::as_str).map(shell_word))
                .collect::<Vec<_>>()
                .join(" ")
        }
    };
    let mut lines: Vec<Line<'static>> = script_lines(&command, body.width, theme)
        .into_iter()
        .map(|line| body.wrap_line(line, theme))
        .collect();

    let background = take("background").and_then(|v| v.as_bool()) == Some(true);
    let mut how = Vec::new();
    if let Some(Value::String(dir)) = take("cwd").or_else(|| take("working_dir")) {
        how.push(format!("in {dir}"));
    }
    if background {
        how.push("background".to_string());
    }
    let timeout_ms = take("timeout_ms").and_then(|v| v.as_u64()).or_else(|| {
        take("timeout_secs")
            .and_then(|v| v.as_u64())
            .map(|s| s * 1000)
    });
    if let Some(ms) = timeout_ms.filter(|_| !background) {
        how.push(format!(
            "timeout {}",
            format_elapsed(Duration::from_millis(ms))
        ));
    }
    if let Some(cols) = take("tty_cols").and_then(|v| v.as_u64()) {
        how.push(format!("tty {cols} columns"));
    }
    take("allow_shell");
    let muted = Style::default().fg(theme.colors.muted);
    if !how.is_empty() {
        body.push_wrapped(&mut lines, &how.join(" · "), muted, theme);
    }
    if !args.is_empty() {
        push_args(&mut lines, &Value::Object(args).to_string(), body, theme);
    }
    Some(lines)
}

/// `arg` as one shell word, quoted when it needs to be.
fn shell_word(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c));
    if plain {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// The last lines a running command printed, under its collapsed block,
/// each cut to one row.
fn live_preview(step: &ToolStep, body: &Body, theme: &Theme) -> Vec<Line<'static>> {
    if step.status != ToolStatus::Running {
        return Vec::new();
    }
    let muted = Style::default().fg(theme.colors.muted);
    step.live
        .tail(LIVE_PREVIEW_LINES)
        .into_iter()
        .map(|line| body.line(truncate(&line, body.width), muted, theme))
        .collect()
}

/// `▸ Read main.rs L1-80`, `▸ ✗ Ran cargo test · exit 101`, cut to fit on
/// one line.
fn tool_header(
    step: &ToolStep,
    parts: &ToolParts,
    marker: Span<'static>,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let (icon, icon_color) = match step.status {
        ToolStatus::Running | ToolStatus::Success => ("", theme.colors.secondary),
        ToolStatus::Error => ("✗ ", theme.colors.error),
        ToolStatus::Denied => ("⊘ ", theme.colors.warning),
    };
    let meta = tool_meta(step, parts.streaming_kb);
    let quiet = quiet_meta(step);
    let stat_width: usize = parts.stat.iter().map(|s| display_width(&s.content)).sum();
    let used = display_width(&marker.content)
        + display_width(icon)
        + display_width(&meta)
        + display_width(&quiet)
        + stat_width;
    let label = truncate(&parts.label, width.saturating_sub(used));

    let mut spans = vec![
        marker,
        Span::styled(icon.to_string(), Style::default().fg(icon_color)),
    ];
    spans.push(Span::styled(label, Style::default().fg(theme.colors.muted)));
    spans.extend(parts.stat.iter().cloned());
    spans.push(Span::styled(meta, Style::default().fg(theme.colors.muted)));
    if !quiet.is_empty() {
        spans.push(Span::styled(
            quiet,
            Style::default().fg(theme.colors.success),
        ));
    }
    Line::from(spans)
}

/// ` · no output 1m 20s` after a running command that has printed nothing
/// for a while, so a stalled command stands out; empty otherwise.
fn quiet_meta(step: &ToolStep) -> String {
    step.quiet_for()
        .map(|quiet| format!(" · no output {}", format_elapsed(quiet)))
        .unwrap_or_default()
}

/// What a file-changing call did to its file: the diff the harness sent
/// once the call ran, else for `edit_file` the text it replaces against
/// the replacement (also while the arguments stream in, and for calls
/// restored from history). `None` for other calls and failed edits.
fn tool_diff(step: &ToolStep, args: &serde_json::Value) -> Option<DiffView> {
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
/// input of `streaming_kb` is still streaming in; ` · error: …` once
/// failed. A finished call keeps how long it took once that reaches a
/// second.
fn tool_meta(step: &ToolStep, streaming_kb: Option<usize>) -> String {
    let mut parts = Vec::new();
    match step.status {
        ToolStatus::Running if step.awaiting_approval => {
            parts.push("awaiting approval".to_string());
        }
        ToolStatus::Running => {
            if let Some((attempt, max)) = step.retry {
                parts.push(format!("retrying ({attempt}/{max})"));
            }
            if let Some(kb) = streaming_kb {
                parts.push(format!("{kb} KB"));
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
mod tests;
