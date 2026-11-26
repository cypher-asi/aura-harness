//! Tool steps: their parts, header and what an open step shows.

use super::args::{command_lines, push_args};
use super::{body_indent, display_width, live_elapsed, truncate, Body, Shown, StepNumber};
use super::{format_elapsed, MoreRows, MoreSection, Theme, ToolStatus, ToolStep};
use super::{LIVE_OUTPUT_LINES, LIVE_PREVIEW_LINES, MAX_RESULT_LINES, STREAMING_SIZE_MIN_BYTES};
use crate::app::format::{format_tool_label_of, ToolArgs};
use crate::components::{CodeBlock, DiffView};
use crate::renderer::diff::diff_stat_spans;
use crate::renderer::markdown::{parse_content_segments, render_markdown, ContentSegment};
use crate::renderer::more_label;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use std::time::Duration;

/// What a tool step is drawn from that changes only with the step: its
/// label, its diff stats and everything under its header. Working these
/// out parses the call's arguments, which grow to hundreds of KB while a
/// file's content streams in, and diffs an edit's texts, so a running
/// step keeps its parts from frame to frame and only its header, with
/// the spinner and the seconds, is drawn again.
#[derive(Debug, Clone)]
pub(in crate::renderer) struct ToolParts {
    /// Whether the step is drawn open
    pub(super) expanded: bool,
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
    pub(in crate::renderer) fn of(
        step: &ToolStep,
        shown: Shown,
        number: Option<StepNumber>,
        width: usize,
        theme: &Theme,
    ) -> Self {
        let body = Body::new(body_indent(number), width);
        Self::new(step, shown, &body, theme)
    }

    pub(super) fn new(step: &ToolStep, shown: Shown, body: &Body, theme: &Theme) -> Self {
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
    pub(super) fn lines(
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
    pub(super) fn into_lines(
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
    push_policy(&mut lines, step, body, theme);
    if let Some(view) = diff {
        push_diff(&mut lines, &mut more, view, body, shown.diff, theme);
    } else if let Some(command) = (step.name == "run_command")
        .then(|| command_lines(&step.args, body, theme))
        .flatten()
    {
        lines.extend(command);
    } else {
        push_args(&mut lines, &step.args, body, theme);
    }
    push_live_output(&mut lines, step, body, theme);
    push_result(&mut lines, &mut more, step, body, shown.result, theme);
    (lines, more)
}

/// `policy: …`, the policy that let the step run.
fn push_policy(lines: &mut Vec<Line<'static>>, step: &ToolStep, body: &Body, theme: &Theme) {
    if let Some(policy) = &step.policy {
        let color = if step.status == ToolStatus::Denied {
            theme.colors.warning
        } else {
            theme.colors.muted
        };
        body.push_wrapped(
            lines,
            &format!("policy: {policy}"),
            Style::default().fg(color),
            theme,
        );
    }
}

/// The changed file's path and its diff, cut `extra` lines past
/// [`MAX_RESULT_LINES`].
fn push_diff(
    lines: &mut Vec<Line<'static>>,
    more: &mut MoreRows,
    view: &DiffView,
    body: &Body,
    extra: usize,
    theme: &Theme,
) {
    let muted = Style::default().fg(theme.colors.muted);
    body.push_wrapped(lines, view.path(), muted, theme);
    let diff_lines = view.styled_lines(theme, body.width);
    let total = diff_lines.len();
    let cap = MAX_RESULT_LINES + extra;
    for line in diff_lines.into_iter().take(cap) {
        lines.push(body.wrap_line(line, theme));
    }
    if total > cap {
        more.push((lines.len(), MoreSection::Diff));
        lines.push(body.line(more_label(total - cap), muted, theme));
    }
}

/// The last lines a running step printed.
fn push_live_output(lines: &mut Vec<Line<'static>>, step: &ToolStep, body: &Body, theme: &Theme) {
    if step.status == ToolStatus::Running && !step.live.is_empty() {
        if !lines.is_empty() {
            lines.push(body.line(String::new(), Style::default(), theme));
        }
        let style = Style::default().fg(theme.colors.foreground);
        for line in step.live.tail(LIVE_OUTPUT_LINES) {
            body.push_wrapped(lines, &line, style, theme);
        }
    }
}

/// What the step returned, cut `extra` lines past [`MAX_RESULT_LINES`].
fn push_result(
    lines: &mut Vec<Line<'static>>,
    more: &mut MoreRows,
    step: &ToolStep,
    body: &Body,
    extra: usize,
    theme: &Theme,
) {
    let Some(result) = step.result.as_deref() else {
        return;
    };
    let result = crate::ansi::strip_ansi(result);
    let result = result.trim_end();
    if result.is_empty() {
        return;
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
    let cap = MAX_RESULT_LINES + extra;
    let total = if step.name == "task" {
        let reply = markdown_lines(result, body.width, style, theme);
        let total = reply.len();
        for line in reply.into_iter().take(cap) {
            lines.push(body.wrap_line(line, theme));
        }
        total
    } else {
        for line in result.lines().take(cap) {
            body.push_wrapped(lines, line, style, theme);
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
