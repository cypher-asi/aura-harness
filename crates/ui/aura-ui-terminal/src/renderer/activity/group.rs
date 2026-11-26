//! A group's summary line: what its steps add up to.

use super::{display_width, truncate, GroupView, Theme, INDENT};
use super::{format_elapsed, Activity, ToolStatus, BRIEF_THOUGHT_MS};
use crate::app::format::{file_name, parse_tool_args};
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use std::collections::BTreeSet;
use std::time::Duration;

/// The summary line of a group of steps: `▸ Explored 4 files, 2 searches`.
/// `progress` (` · step 7 · 42s`) follows the summary while the group runs.
#[cfg(test)]
pub(in crate::renderer) fn group_header(
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
pub(in crate::renderer) fn group_header_of(
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
pub(in crate::renderer) struct StepTally {
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
    pub(in crate::renderer) fn of(step: &Activity) -> Self {
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
pub(in crate::renderer) struct Tally {
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
    pub(in crate::renderer) fn of<S: std::borrow::Borrow<StepTally>>(
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
        let parts = self.parts(live);
        if parts.is_empty() {
            return if live {
                "Thinking…".to_string()
            } else {
                thought_label(self.thought_ms)
            };
        }
        capitalize(&parts.join(", "))
    }

    /// The files and searches explored: `main.rs, 2 searches`.
    fn explored(&self) -> Vec<String> {
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
        explored
    }

    /// The summary's parts, uncapitalized: `explored 4 files`, `ran 1 command`.
    fn parts(&self, live: bool) -> Vec<String> {
        let tense = |running: &str, done: &str| if live { running } else { done }.to_string();
        let explored = self.explored();
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
        parts
    }
}

pub(super) fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// `Thought briefly`, `Thought 11s`, `Thought 2m 05s`.
pub(super) fn thought_label(duration_ms: Option<u64>) -> String {
    match duration_ms {
        None => "Thought".to_string(),
        Some(ms) if ms < BRIEF_THOUGHT_MS => "Thought briefly".to_string(),
        Some(ms) if ms < 60_000 => format!("Thought {}s", ms / 1000),
        Some(ms) => format!("Thought {}", format_elapsed(Duration::from_millis(ms))),
    }
}
