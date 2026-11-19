//! What `aura -p` prints on stdout, in the three `--output-format`s.
//!
//! stdout carries only this output so scripts can parse it; logs go to
//! stderr.

use super::OutputFormat;
use crate::session_checkpoint::SessionUsage;
use aura_exec_tools::todo::TodoItem;
use serde::Serialize;
use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError};
use tracing::debug;

/// Why a run ended. Each reason has its own exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExitReason {
    /// The agent finished.
    Success,
    /// `--max-turns` model calls were used up.
    MaxTurns,
    /// `--max-time` (or `AURA_TUI_TURN_TIMEOUT`) ran out.
    MaxTime,
    /// `--max-cost-usd` (or `AURA_TUI_MAX_TURN_COST_USD`) was exceeded.
    MaxCost,
    /// The run failed: the model, a session, or the record.
    Error,
    /// Ctrl+C.
    Interrupted,
}

impl ExitReason {
    /// Process exit code: 0 success, 1 failure, 3 a limit stopped the run,
    /// 130 interrupted (the shell convention for SIGINT). 2 is left to
    /// clap for bad arguments.
    pub(crate) const fn exit_code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::Error => 1,
            Self::MaxTurns | Self::MaxTime | Self::MaxCost => 3,
            Self::Interrupted => 130,
        }
    }
}

/// Tokens and cost of a run.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub(crate) struct UsageReport {
    /// Model calls
    pub calls: u32,
    /// Prompt tokens neither read from nor written to the cache
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    /// Cache reads as a share of all prompt tokens; `None` before any call
    pub cache_hit_rate: Option<f64>,
    /// Estimated at API list prices; `None` when no call had a price
    pub cost_usd: Option<f64>,
}

impl From<&SessionUsage> for UsageReport {
    #[allow(clippy::cast_precision_loss)]
    fn from(usage: &SessionUsage) -> Self {
        let prompt = usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens;
        Self {
            calls: usage.calls,
            input_tokens: usage.input_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
            output_tokens: usage.output_tokens,
            cache_hit_rate: (prompt > 0).then(|| usage.cache_read_tokens as f64 / prompt as f64),
            cost_usd: usage.priced.then_some(usage.cost_usd),
        }
    }
}

/// The outcome of a run: the `json` output, and the last `stream-json`
/// line.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct RunReport {
    /// Always `"result"`, so a `stream-json` reader can spot it
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub exit_reason: ExitReason,
    pub is_error: bool,
    /// The agent's final reply
    pub result: String,
    /// Pass to `--resume` to continue this session
    pub session_id: String,
    pub model: String,
    /// Model calls made by this run
    pub num_turns: u32,
    /// Agent turns: the prompt, plus one per background-job notice that
    /// started another turn
    pub agent_turns: u32,
    pub duration_ms: u64,
    pub usage: UsageReport,
    /// Same as `usage.cost_usd`, at the top level for quick reading
    pub total_cost_usd: Option<f64>,
    /// History summaries made to stay within the context window
    pub compactions: u32,
    /// The agent's todo list when the run ended
    pub todos: Vec<TodoItem>,
    /// Files the agent wrote, edited or deleted, workspace-relative
    pub files_changed: Vec<String>,
    /// What went wrong, when `is_error`
    pub error: Option<String>,
}

impl RunReport {
    /// A report for a run that failed before the agent started.
    pub(crate) fn failed(error: String, model: String) -> Self {
        Self {
            kind: "result",
            exit_reason: ExitReason::Error,
            is_error: true,
            result: String::new(),
            session_id: String::new(),
            model,
            num_turns: 0,
            agent_turns: 0,
            duration_ms: 0,
            usage: UsageReport::from(&SessionUsage::default()),
            total_cost_usd: None,
            compactions: 0,
            todos: Vec::new(),
            files_changed: Vec::new(),
            error: Some(error),
        }
    }
}

/// One `stream-json` line before the result.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum StreamEvent<'a> {
    /// First line: the session and what the agent may use.
    Init {
        session_id: &'a str,
        agent_id: &'a str,
        model: &'a str,
        cwd: &'a str,
        mode: &'a str,
        tools: Vec<&'a str>,
        /// Whether this continues an earlier session
        resumed: bool,
    },
    TextDelta {
        text: &'a str,
    },
    ThinkingDelta {
        text: &'a str,
    },
    /// The model started a tool call.
    ToolUse {
        id: &'a str,
        name: &'a str,
    },
    /// A tool call finished (or was denied).
    ToolResult {
        id: &'a str,
        name: &'a str,
        input: &'a serde_json::Value,
        content: &'a str,
        is_error: bool,
    },
    /// The model's reply started over; drop text streamed since the last
    /// tool result.
    StreamReset {
        reason: &'a str,
    },
    /// The todo list changed.
    Todos {
        todos: &'a [TodoItem],
    },
    /// One model call's tokens and estimated cost.
    Usage {
        model: &'a str,
        input_tokens: u64,
        cache_read_tokens: u64,
        cache_write_tokens: u64,
        output_tokens: u64,
        cost_usd: Option<f64>,
    },
    /// Older history was summarized.
    Compacted {
        message: &'a str,
    },
    Warning {
        message: &'a str,
    },
    Error {
        message: &'a str,
    },
    /// A harness notice (a background job exited) starts another turn.
    Notice {
        text: &'a str,
    },
    /// Gate's judge reviewed a turn that failed on `model`.
    Judged {
        model: &'a str,
        judge: &'a str,
        /// Whether the model's own work caused the failure
        at_fault: bool,
        reasons: &'a str,
        /// What the judge's call cost
        cost_usd: f64,
    },
}

/// Where a run's output goes, shared by the event sink and the runner.
#[derive(Clone)]
pub(crate) struct Output {
    format: OutputFormat,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl Output {
    pub(crate) fn new(format: OutputFormat, writer: Box<dyn Write + Send>) -> Self {
        Self {
            format,
            writer: Arc::new(Mutex::new(writer)),
        }
    }

    /// Print `event` when streaming events.
    pub(crate) fn event(&self, event: &StreamEvent<'_>) {
        if self.format == OutputFormat::StreamJson {
            self.write_json_line(event);
        }
    }

    /// Print the run's outcome in the chosen format. `text` prints only
    /// the reply; its caller reports errors on stderr.
    pub(crate) fn finish(&self, report: &RunReport) {
        match self.format {
            OutputFormat::Text => {
                if !report.result.is_empty() {
                    let mut text = report.result.clone();
                    if !text.ends_with('\n') {
                        text.push('\n');
                    }
                    self.write(text.as_bytes());
                }
            }
            OutputFormat::Json | OutputFormat::StreamJson => self.write_json_line(report),
        }
    }

    fn write_json_line(&self, value: &impl Serialize) {
        match serde_json::to_vec(value) {
            Ok(mut line) => {
                line.push(b'\n');
                self.write(&line);
            }
            Err(e) => debug!(error = %e, "Could not serialize headless output"),
        }
    }

    fn write(&self, bytes: &[u8]) {
        let mut writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        // A reader that went away (e.g. `| head`) must not stop the run;
        // the session is still saved.
        if let Err(e) = writer.write_all(bytes).and_then(|()| writer.flush()) {
            debug!(error = %e, "Could not write headless output");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_tell_failure_from_limits() {
        assert_eq!(ExitReason::Success.exit_code(), 0);
        assert_eq!(ExitReason::Error.exit_code(), 1);
        assert_eq!(ExitReason::MaxTurns.exit_code(), 3);
        assert_eq!(ExitReason::MaxCost.exit_code(), 3);
        assert_eq!(ExitReason::MaxTime.exit_code(), 3);
        assert_eq!(ExitReason::Interrupted.exit_code(), 130);
    }

    #[test]
    fn cache_hit_rate_counts_every_prompt_token() {
        let usage = SessionUsage {
            calls: 2,
            input_tokens: 100,
            cache_read_tokens: 800,
            cache_write_tokens: 100,
            output_tokens: 50,
            cost_usd: 0.01,
            priced: true,
            context_tokens: 0,
            models: Vec::new(),
        };
        let report = UsageReport::from(&usage);
        assert_eq!(report.cache_hit_rate, Some(0.8));
        assert_eq!(report.cost_usd, Some(0.01));
        assert_eq!(
            UsageReport::from(&SessionUsage::default()).cache_hit_rate,
            None
        );
    }

    #[test]
    fn stream_events_are_tagged_by_type() {
        let line = serde_json::to_value(StreamEvent::TextDelta { text: "hi" }).unwrap();
        assert_eq!(
            line,
            serde_json::json!({"type": "text_delta", "text": "hi"})
        );
    }
}
