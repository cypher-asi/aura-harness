//! What the agent did on its way to a reply: a thinking block or a tool call.

use crate::components::ToolStatus;
use crate::events::ActivityData;
use std::time::Instant;

/// One step of agent activity shown in the chat as a collapsible block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activity {
    /// Extended thinking
    Thinking(ThinkingStep),
    /// A tool call
    Tool(ToolStep),
}

/// A block of model thinking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingStep {
    /// Thinking text so far
    pub text: String,
    /// When thinking started; `None` for blocks restored from history
    pub started: Option<Instant>,
    /// How long the model thought, once finished
    pub duration_ms: Option<u64>,
    /// Whether thinking is still streaming
    pub running: bool,
}

/// A tool call and its outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolStep {
    /// Tool use id
    pub id: String,
    /// Tool name
    pub name: String,
    /// Arguments as JSON (possibly partial while they stream)
    pub args: String,
    /// Execution status
    pub status: ToolStatus,
    /// Output, once the tool finished
    pub result: Option<String>,
    /// `(attempt, max_attempts)` while the call is being retried
    pub retry: Option<(u32, u32)>,
    /// When the call started; `None` for blocks restored from history
    pub started: Option<Instant>,
    /// How long the call took, once finished
    pub duration_ms: Option<u64>,
}

impl Activity {
    /// Thinking that has just started.
    #[must_use]
    pub fn thinking() -> Self {
        Self::Thinking(ThinkingStep {
            text: String::new(),
            started: Some(Instant::now()),
            duration_ms: None,
            running: true,
        })
    }

    /// A tool call that has just started.
    #[must_use]
    pub fn tool(id: &str, name: &str, args: &str) -> Self {
        Self::Tool(ToolStep {
            id: id.to_string(),
            name: name.to_string(),
            args: args.to_string(),
            status: ToolStatus::Running,
            result: None,
            retry: None,
            started: Some(Instant::now()),
            duration_ms: None,
        })
    }

    /// Whether the step is still in progress.
    #[must_use]
    pub fn is_running(&self) -> bool {
        match self {
            Self::Thinking(step) => step.running,
            Self::Tool(step) => step.status == ToolStatus::Running,
        }
    }

    /// Mark the step finished. A tool still running is marked failed with
    /// `reason` as its output.
    pub fn finish(&mut self, reason: &str) {
        match self {
            Self::Thinking(step) if step.running => {
                step.running = false;
                step.duration_ms = elapsed_ms(step.started);
            }
            Self::Tool(step) if step.status == ToolStatus::Running => {
                step.status = ToolStatus::Error;
                step.result = Some(reason.to_string());
                step.duration_ms = elapsed_ms(step.started);
            }
            _ => {}
        }
    }
}

impl From<&ActivityData> for Activity {
    fn from(data: &ActivityData) -> Self {
        match data {
            ActivityData::Thinking { text, duration_ms } => Self::Thinking(ThinkingStep {
                text: text.clone(),
                started: None,
                duration_ms: *duration_ms,
                running: false,
            }),
            ActivityData::Tool {
                id,
                name,
                args,
                result,
                is_error,
                duration_ms,
            } => Self::Tool(ToolStep {
                id: id.clone(),
                name: name.clone(),
                args: args.clone(),
                status: if *is_error {
                    ToolStatus::Error
                } else {
                    ToolStatus::Success
                },
                result: result
                    .as_deref()
                    .map(|r| crate::app::format::format_tool_result(name, r)),
                retry: None,
                started: None,
                duration_ms: *duration_ms,
            }),
        }
    }
}

/// Milliseconds since `started`.
pub(crate) fn elapsed_ms(started: Option<Instant>) -> Option<u64> {
    started.map(|s| u64::try_from(s.elapsed().as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finish_stops_thinking_and_fails_running_tool() {
        let mut thinking = Activity::thinking();
        thinking.finish("interrupted");
        assert!(!thinking.is_running());
        assert!(matches!(thinking, Activity::Thinking(ref s) if s.duration_ms.is_some()));

        let mut tool = Activity::tool("t1", "read_file", "{}");
        tool.finish("interrupted");
        let Activity::Tool(step) = tool else {
            unreachable!()
        };
        assert_eq!(step.status, ToolStatus::Error);
        assert_eq!(step.result.as_deref(), Some("interrupted"));
    }

    #[test]
    fn restored_tool_keeps_outcome() {
        let activity = Activity::from(&ActivityData::Tool {
            id: "t1".to_string(),
            name: "read_file".to_string(),
            args: r#"{"path":"a.rs"}"#.to_string(),
            result: Some("fn main() {}".to_string()),
            is_error: false,
            duration_ms: Some(12),
        });
        assert!(!activity.is_running());
        assert!(matches!(activity, Activity::Tool(ref s) if s.status == ToolStatus::Success));
    }
}
