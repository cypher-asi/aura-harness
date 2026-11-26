//! Post-edit check: after a step that changed files, run the project's
//! check command and tell the model when it fails.
//!
//! The agent loop already asks its executor for an
//! [`AgentToolExecutor::auto_build_check`] once per step in which a write
//! succeeded (then waits `auto_build_cooldown` steps), and appends a
//! failure to that step's tool results. [`WithPostEditCheck`] wraps any
//! executor and answers that hook with a [`PostEditCheck`], so a burst of
//! edits in one step runs the check once.
//!
//! The command comes from, in order:
//!
//! 1. `AURA_CHECK_COMMAND` (`off` disables the check, including any
//!    `AGENTS.md` command);
//! 2. the project's `AGENTS.md` (see [`parse_agents_md_check`]).
//!
//! Nothing is inferred: without either, no check runs. `AGENTS.md` is read
//! each time, so editing it takes effect on the next step.

use crate::types::{
    AgentToolExecutor, AutoBuildResult, BuildBaseline, ToolCallInfo, ToolCallResult,
};
use crate::verify::run_build_command_with_timeout;
use async_trait::async_trait;
use aura_exec_tools::RunScratch;
use std::collections::hash_map::DefaultHasher;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{info, warn};

/// Environment variable naming the check command.
pub const CHECK_COMMAND_ENV: &str = "AURA_CHECK_COMMAND";
/// Environment variable bounding one check run.
pub const CHECK_TIMEOUT_ENV: &str = "AURA_CHECK_TIMEOUT";

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Output lines kept from a failing check (its end, where the summary is).
const TAIL_LINES: usize = 80;
/// Output bytes kept from a failing check.
const TAIL_BYTES: usize = 8_000;

/// Where the check command came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckSource {
    /// `AURA_CHECK_COMMAND`.
    Env,
    /// The project's `AGENTS.md`.
    AgentsMd,
}

impl CheckSource {
    fn describe(self) -> &'static str {
        match self {
            Self::Env => "set by AURA_CHECK_COMMAND",
            Self::AgentsMd => "from AGENTS.md",
        }
    }
}

/// A resolved check command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckCommand {
    pub command: String,
    pub source: CheckSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EnvCommand {
    Unset,
    Off,
    Command(String),
}

/// Check settings, read once at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostEditCheckConfig {
    env: EnvCommand,
    /// Longest one check may run before it is killed.
    pub timeout: Duration,
}

impl Default for PostEditCheckConfig {
    fn default() -> Self {
        Self::from_lookup(|_| None)
    }
}

impl PostEditCheckConfig {
    /// Read `AURA_CHECK_COMMAND` and `AURA_CHECK_TIMEOUT` through `get`
    /// (normally `std::env::var`). The timeout is seconds, or a number with
    /// an `s`/`m` suffix.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let env = match get(CHECK_COMMAND_ENV).map(|v| v.trim().to_string()) {
            None => EnvCommand::Unset,
            Some(v) if v.is_empty() => EnvCommand::Unset,
            Some(v)
                if matches!(
                    v.to_ascii_lowercase().as_str(),
                    "off" | "none" | "0" | "false"
                ) =>
            {
                EnvCommand::Off
            }
            Some(v) => EnvCommand::Command(v),
        };
        let timeout = get(CHECK_TIMEOUT_ENV)
            .and_then(|raw| {
                let parsed = parse_timeout(&raw);
                if parsed.is_none() {
                    warn!(value = %raw, "Ignoring unreadable {CHECK_TIMEOUT_ENV}");
                }
                parsed
            })
            .unwrap_or(DEFAULT_TIMEOUT)
            .min(MAX_TIMEOUT);
        Self { env, timeout }
    }

    /// The command to run for the project at `root`, if any.
    #[must_use]
    pub fn resolve(&self, root: &Path) -> Option<CheckCommand> {
        match &self.env {
            EnvCommand::Off => None,
            EnvCommand::Command(command) => Some(CheckCommand {
                command: command.clone(),
                source: CheckSource::Env,
            }),
            EnvCommand::Unset => {
                let text = std::fs::read_to_string(root.join("AGENTS.md")).ok()?;
                parse_agents_md_check(&text).map(|command| CheckCommand {
                    command,
                    source: CheckSource::AgentsMd,
                })
            }
        }
    }
}

fn parse_timeout(raw: &str) -> Option<Duration> {
    let raw = raw.trim().to_ascii_lowercase();
    let (number, unit) = if let Some(n) = raw.strip_suffix('m') {
        (n, 60)
    } else if let Some(n) = raw.strip_suffix('s') {
        (n, 1)
    } else {
        (raw.as_str(), 1)
    };
    let value: u64 = number.trim().parse().ok()?;
    (value > 0).then(|| Duration::from_secs(value.saturating_mul(unit)))
}

/// Find the check command in an `AGENTS.md`. The first of these, in
/// document order, wins:
///
/// - a line `check: <command>` (the key is case-insensitive, may follow a
///   list marker `-`/`*`, and the command may be wrapped in backticks);
/// - a fenced code block whose info string is `check`; its non-empty lines
///   that don't start with `#` are joined with ` && `.
///
/// `check:` lines inside other code blocks are ignored.
#[must_use]
pub fn parse_agents_md_check(text: &str) -> Option<String> {
    let mut fence: Option<(String, bool)> = None;
    let mut block: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some((marker, is_check)) = &fence {
            if trimmed.starts_with(marker.as_str())
                && trimmed
                    .trim_start_matches(marker.as_str())
                    .trim()
                    .is_empty()
            {
                if *is_check && !block.is_empty() {
                    return Some(block.join(" && "));
                }
                fence = None;
                block.clear();
            } else if *is_check && !trimmed.is_empty() && !trimmed.starts_with('#') {
                block.push(trimmed.to_string());
            }
            continue;
        }
        if let Some(marker) = fence_marker(trimmed) {
            let info = trimmed[marker.len()..].trim();
            fence = Some((marker, info.eq_ignore_ascii_case("check")));
            continue;
        }
        let item = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .unwrap_or(trimmed)
            .trim_start();
        if item.len() > 6 && item[..6].eq_ignore_ascii_case("check:") {
            let command = item[6..].trim().trim_matches('`').trim();
            if !command.is_empty() {
                return Some(command.to_string());
            }
        }
    }
    None
}

fn fence_marker(line: &str) -> Option<String> {
    let ch = line.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let len = line.chars().take_while(|c| *c == ch).count();
    (len >= 3).then(|| ch.to_string().repeat(len))
}

/// What one check run found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    Passed {
        /// The previous run had failed.
        recovered: bool,
    },
    Failed {
        exit_code: Option<i32>,
        timed_out: bool,
    },
    /// Failed with exactly the output already reported.
    StillFailing,
    /// The command could not be started.
    CouldNotRun(String),
}

/// A finished check, for the surface to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    pub command: CheckCommand,
    pub outcome: CheckOutcome,
}

/// Called after every check run.
pub type CheckNotifier = Arc<dyn Fn(&CheckReport) + Send + Sync>;

/// Runs the project check for one agent and remembers the last failure,
/// so the same broken state is not re-sent in full after every step.
pub struct PostEditCheck {
    root: PathBuf,
    config: PostEditCheckConfig,
    /// The run's private folder, which the check's temp, cache and build
    /// variables point beneath, as for the run's own commands.
    scratch: Option<RunScratch>,
    last_failure: Mutex<Option<u64>>,
    notifier: Option<CheckNotifier>,
}

impl PostEditCheck {
    /// Check for the project rooted at `root` (where the command runs).
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, config: PostEditCheckConfig) -> Self {
        Self {
            root: root.into(),
            config,
            scratch: None,
            last_failure: Mutex::new(None),
            notifier: None,
        }
    }

    /// Run the check with `scratch`'s temp, cache and build variables
    /// (see [`RunScratch::vars`]), so a run whose commands have a scratch
    /// folder builds there too. The check is not write-confined.
    #[must_use]
    pub fn with_run_scratch(mut self, scratch: RunScratch) -> Self {
        self.scratch = Some(scratch);
        self
    }

    /// The scratch folder the check runs with, if any.
    #[must_use]
    pub fn run_scratch(&self) -> Option<&RunScratch> {
        self.scratch.as_ref()
    }

    /// Report every run to `notifier`.
    #[must_use]
    pub fn with_notifier(mut self, notifier: CheckNotifier) -> Self {
        self.notifier = Some(notifier);
        self
    }

    /// The command that would run now, if any.
    #[must_use]
    pub fn command(&self) -> Option<CheckCommand> {
        self.config.resolve(&self.root)
    }

    /// Run the check. `None` when no check is configured; otherwise the
    /// result for the agent loop, whose `output` (on failure) is what the
    /// model is told.
    pub async fn run(&self) -> Option<AutoBuildResult> {
        let command = self.command()?;
        info!(command = %command.command, root = %self.root.display(), "Running post-edit check");
        let result = run_build_command_with_timeout(
            &self.root,
            &command.command,
            None,
            self.scratch.as_ref(),
            self.config.timeout,
        )
        .await;

        let (outcome, output) = match result {
            Ok(build) if build.success => {
                let recovered = self.remember_failure(None).is_some();
                (CheckOutcome::Passed { recovered }, String::new())
            }
            Ok(build) => {
                let combined = join_output(&build.stdout, &build.stderr);
                let tail = tail(&combined);
                let how = if build.timed_out {
                    format!("timed out after {}s", self.config.timeout.as_secs())
                } else {
                    match build.exit_code {
                        Some(code) => format!("failed with exit code {code}"),
                        None => "was killed".to_string(),
                    }
                };
                let outcome = CheckOutcome::Failed {
                    exit_code: build.exit_code,
                    timed_out: build.timed_out,
                };
                let message = format!(
                    "Post-edit check `{}` ({}) {how}. Fix what your changes broke before moving \
                     on. End of its output:\n\n```\n{tail}\n```",
                    command.command,
                    command.source.describe(),
                );
                self.failure(outcome, message, &tail, &command)
            }
            Err(e) => {
                warn!(command = %command.command, error = %e, "Post-edit check could not run");
                let message = format!(
                    "Post-edit check `{}` ({}) could not be started: {e}",
                    command.command,
                    command.source.describe(),
                );
                self.failure(
                    CheckOutcome::CouldNotRun(e.to_string()),
                    message.clone(),
                    &message,
                    &command,
                )
            }
        };

        let passed = matches!(outcome, CheckOutcome::Passed { .. });
        if let Some(notify) = &self.notifier {
            notify(&CheckReport { command, outcome });
        }
        Some(AutoBuildResult {
            success: passed,
            error_count: if passed { 0 } else { count_errors(&output) },
            output,
        })
    }

    /// A failure's outcome and message, shortened when it repeats the
    /// previous failure.
    fn failure(
        &self,
        outcome: CheckOutcome,
        message: String,
        fingerprint_of: &str,
        command: &CheckCommand,
    ) -> (CheckOutcome, String) {
        let mut hasher = DefaultHasher::new();
        fingerprint_of.hash(&mut hasher);
        let fingerprint = hasher.finish();
        if self.remember_failure(Some(fingerprint)) == Some(fingerprint) {
            let short = format!(
                "Post-edit check `{}` still fails with the same output as last time.",
                command.command
            );
            return (CheckOutcome::StillFailing, short);
        }
        (outcome, message)
    }

    /// Store `failure` (or clear it) and return the previous one.
    fn remember_failure(&self, failure: Option<u64>) -> Option<u64> {
        let mut last = self
            .last_failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::replace(&mut *last, failure)
    }
}

fn join_output(stdout: &str, stderr: &str) -> String {
    match (stdout.trim().is_empty(), stderr.trim().is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout.to_string(),
        (true, false) => stderr.to_string(),
        (false, false) => format!("{stdout}\n{stderr}"),
    }
}

/// The last [`TAIL_LINES`] lines, at most [`TAIL_BYTES`], noting what was
/// dropped.
fn tail(output: &str) -> String {
    let lines: Vec<&str> = output.trim_end().lines().collect();
    if lines.is_empty() {
        return "(no output)".to_string();
    }
    let start = lines.len().saturating_sub(TAIL_LINES);
    let mut kept = lines[start..].join("\n");
    let mut dropped = start;
    if kept.len() > TAIL_BYTES {
        let mut cut = kept.len() - TAIL_BYTES;
        while !kept.is_char_boundary(cut) {
            cut += 1;
        }
        let newline = kept[cut..].find('\n').map_or(cut, |p| cut + p + 1);
        dropped += kept[..newline].lines().count();
        kept = kept[newline..].to_string();
    }
    if dropped > 0 {
        format!("[... {dropped} earlier lines omitted]\n{kept}")
    } else {
        kept
    }
}

/// Lines that look like compiler/linter errors; at least one, since the
/// loop reports "failed with N error(s)".
fn count_errors(output: &str) -> usize {
    output
        .lines()
        .filter(|line| {
            let line = line.trim_start().to_ascii_lowercase();
            line.starts_with("error:") || line.starts_with("error[")
        })
        .count()
        .max(1)
}

/// An executor that answers the loop's post-write check with a
/// [`PostEditCheck`] and passes everything else to `inner`.
pub struct WithPostEditCheck<'a> {
    inner: &'a dyn AgentToolExecutor,
    check: Option<&'a PostEditCheck>,
}

impl<'a> WithPostEditCheck<'a> {
    /// Wrap `inner`; with `check` `None` this behaves exactly like `inner`.
    #[must_use]
    pub fn new(inner: &'a dyn AgentToolExecutor, check: Option<&'a PostEditCheck>) -> Self {
        Self { inner, check }
    }
}

#[async_trait]
impl AgentToolExecutor for WithPostEditCheck<'_> {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        self.inner.execute(tool_calls).await
    }

    fn spawn_tool_call<'b>(
        &'b self,
        call: ToolCallInfo,
    ) -> Pin<Box<dyn Future<Output = ToolCallResult> + Send + 'b>> {
        self.inner.spawn_tool_call(call)
    }

    async fn auto_build_check(&self) -> Option<AutoBuildResult> {
        match self.check {
            Some(check) => check.run().await,
            None => self.inner.auto_build_check().await,
        }
    }

    async fn capture_build_baseline(&self) -> Option<BuildBaseline> {
        self.inner.capture_build_baseline().await
    }
}

#[cfg(test)]
mod tests;
