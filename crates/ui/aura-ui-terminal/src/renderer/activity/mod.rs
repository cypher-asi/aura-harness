//! Agent activity blocks in the chat: thinking and tool calls, collapsed to
//! a one-line header or expanded to show everything. Consecutive steps are
//! drawn as a group under one summary line (`Explored 4 files, 2 searches`).

mod args;
mod group;
mod tool;

#[cfg(test)]
pub(super) use group::group_header;
pub(super) use group::{group_header_of, StepTally, Tally};
pub(super) use tool::ToolParts;

use super::chat_panel::format_elapsed;
use super::command::wrap_spans;
use super::text::{display_width, wrap_words};
use super::MoreRows;
use crate::components::{Activity, Message, MoreSection, ThinkingStep, ToolStatus, ToolStep};
use crate::Theme;
use group::{count, thought_label};
use ratatui::{
    style::Style,
    text::{Line, Span},
};

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
