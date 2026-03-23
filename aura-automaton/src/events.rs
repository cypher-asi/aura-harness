use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AutomatonEvent {
    // Lifecycle
    Started { automaton_id: String },
    Stopped { automaton_id: String, reason: String },
    Paused { automaton_id: String },
    Resumed { automaton_id: String },
    Error { automaton_id: String, message: String },

    // Streaming / LLM
    TextDelta { delta: String },
    ThinkingDelta { delta: String },
    Progress { message: String },

    // Tool usage
    ToolCallStarted { id: String, name: String },
    ToolCallSnapshot { id: String, name: String, input: serde_json::Value },
    ToolCallCompleted { id: String, name: String, input: serde_json::Value },
    ToolResult { id: String, name: String, result: String, is_error: bool },

    // Dev loop specific
    TaskStarted { task_id: String, task_title: String },
    TaskCompleted { task_id: String, summary: String },
    TaskFailed { task_id: String, reason: String },
    TaskRetrying { task_id: String, attempt: u32, reason: String },
    LoopFinished { outcome: String, completed_count: u32, failed_count: u32 },

    // Spec generation
    SpecSaved { spec_id: String, title: String },
    SpecsTitle { title: String },
    SpecsSummary { summary: String },

    // Build / test
    BuildVerificationStarted,
    BuildVerificationPassed,
    BuildVerificationFailed { error_count: u32 },
    TestVerificationStarted,
    TestVerificationPassed,
    TestVerificationFailed { failure_count: u32 },
    BuildFixAttempt { attempt: u32, max_attempts: u32 },
    TestFixAttempt { attempt: u32, max_attempts: u32 },

    // File ops
    FileOpsApplied { files_written: u32, files_deleted: u32 },

    // Session
    SessionRolledOver { old_session_id: String, new_session_id: String },

    // Token usage
    TokenUsage { input_tokens: u64, output_tokens: u64 },

    // Chat-specific
    MessageSaved { message_id: String },
    AgentInstanceUpdated { instance_id: String },

    // Generic
    LogLine { message: String },
    Done,
}
