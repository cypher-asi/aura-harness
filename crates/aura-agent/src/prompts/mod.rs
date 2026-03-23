//! Prompt builders for the agent execution, fix, and chat flows.
//!
//! All builders accept lightweight descriptor structs instead of domain objects,
//! keeping this module free of app-layer dependencies.

mod context;
mod fix;
mod system;
mod turn_kernel_system;

pub use crate::verify::error_types::{parse_error_references, BuildFixAttemptRecord};
pub use context::build_agentic_task_context;
pub use fix::{build_fix_prompt_with_history, build_stub_fix_prompt, BuildFixPromptParams};
pub use system::{
    agentic_execution_system_prompt, build_chat_system_prompt, build_fix_system_prompt,
    CHAT_SYSTEM_PROMPT_BASE, CONTEXT_SUMMARY_SYSTEM_PROMPT,
};
pub use turn_kernel_system::default_system_prompt;

/// Minimal project descriptor for prompt builders.
#[derive(Debug)]
pub struct ProjectInfo<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub folder_path: &'a str,
    pub build_command: Option<&'a str>,
    pub test_command: Option<&'a str>,
}

/// Minimal agent identity descriptor.
#[derive(Debug)]
pub struct AgentInfo<'a> {
    pub name: &'a str,
    pub role: &'a str,
    pub personality: &'a str,
    pub system_prompt: &'a str,
    pub skills: &'a [String],
}

/// Minimal spec descriptor.
#[derive(Debug)]
pub struct SpecInfo<'a> {
    pub title: &'a str,
    pub markdown_contents: &'a str,
}

/// Minimal task descriptor.
#[derive(Debug)]
pub struct TaskInfo<'a> {
    pub title: &'a str,
    pub description: &'a str,
    pub execution_notes: &'a str,
    pub files_changed: &'a [FileChangeEntry],
}

/// A single file-change entry (path + operation label).
#[derive(Debug, Clone)]
pub struct FileChangeEntry {
    pub path: String,
    pub op: String,
}

/// Minimal session descriptor.
#[derive(Debug)]
pub struct SessionInfo<'a> {
    pub summary_of_previous_context: &'a str,
}
