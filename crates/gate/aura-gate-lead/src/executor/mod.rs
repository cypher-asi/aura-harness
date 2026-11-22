//! A led turn's tools and calls while delegation is enforced.
//!
//! The turn's tool list drops the file-changing tools ([`lead_tools`]),
//! and every call goes through [`LeadExecutor`], which refuses those
//! tools and the commands the lead may not run, and caps what reads and
//! searches return. A refused call is still recorded, with the refusal as
//! its result, so the record shows what the lead tried. Subagents get
//! neither.

use crate::limits::{
    allows_command, keeps_tool, refused_command, refused_tool, Cap, READ_LINES, RUN_COMMAND,
    SEARCH_HITS,
};
use async_trait::async_trait;
use aura_agent::{AgentToolExecutor, AutoBuildResult, BuildBaseline, ToolCallInfo, ToolCallResult};
use aura_gate_evidence::checks::command_line;
use aura_model::ToolDefinition;
use std::future::Future;
use std::pin::Pin;

/// `ToolProposal::source` of the lead's refused calls.
const LEAD_SOURCE: &str = "gate_lead";

/// A call's result to come, borrowing the executor that runs it.
type Running<'a> = Pin<Box<dyn Future<Output = ToolCallResult> + Send + 'a>>;

/// `tools` as the lead gets them: without the file-changing tools, and
/// with the capped tools and `run_command` saying what the lead may do
/// with them.
#[must_use]
pub fn lead_tools(tools: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
    tools
        .into_iter()
        .filter(|tool| keeps_tool(&tool.name))
        .map(|mut tool| {
            note_limit(&tool.name, &mut tool.description);
            tool
        })
        .collect()
}

/// Say at the end of the tool `name`'s `description` what the lead may
/// do with it, when the lead's limits cover it.
fn note_limit(name: &str, description: &mut String) {
    match Cap::of(name) {
        Some(Cap::Read) => description.push_str(&format!(
            " You see at most {READ_LINES} lines per read; delegate heavy reading to an \
             explorer subtask."
        )),
        Some(Cap::Search) => {
            description.push_str(&format!(" You see at most {SEARCH_HITS} hits per search."));
        }
        None if name == RUN_COMMAND => description.push_str(
            " You may only run checks (tests, builds, linters, type checks) and inspection \
             commands (git status, git diff, git log, git show, ls, pwd); delegate anything \
             else.",
        ),
        None => {}
    }
}

/// Why the lead may not make `call`; `None` when it may.
fn refusal(call: &ToolCallInfo) -> Option<String> {
    if !keeps_tool(&call.name) {
        return Some(refused_tool(&call.name));
    }
    if call.name != RUN_COMMAND {
        return None;
    }
    let command = command_line(&call.input);
    (!allows_command(&command)).then(|| refused_command(&command))
}

/// `result` as the lead sees it under `cap`; an error comes back whole.
fn capped(cap: Cap, mut result: ToolCallResult) -> ToolCallResult {
    if !result.is_error {
        if let Some(content) = cap.apply(&result.content) {
            result.content = content;
        }
    }
    result
}

/// Runs the lead's calls on `inner` within the lead's limits.
pub struct LeadExecutor<'a> {
    inner: &'a dyn AgentToolExecutor,
}

impl<'a> LeadExecutor<'a> {
    /// An executor that runs on `inner` the calls the lead may make.
    #[must_use]
    pub fn new(inner: &'a dyn AgentToolExecutor) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl AgentToolExecutor for LeadExecutor<'_> {
    async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
        let mut results = Vec::with_capacity(tool_calls.len());
        for call in tool_calls {
            results.push(self.spawn_tool_call(call.clone()).await);
        }
        results
    }

    fn spawn_tool_call<'b>(&'b self, call: ToolCallInfo) -> Running<'b> {
        if let Some(why) = refusal(&call) {
            let refused = ToolCallResult::error(call.id.clone(), why);
            return self.inner.spawn_preset(call, LEAD_SOURCE, refused);
        }
        let Some(cap) = Cap::of(&call.name) else {
            return self.inner.spawn_tool_call(call);
        };
        let run = self.inner.spawn_tool_call(call);
        Box::pin(async move { capped(cap, run.await) })
    }

    fn spawn_preset<'b>(
        &'b self,
        call: ToolCallInfo,
        source: &'static str,
        result: ToolCallResult,
    ) -> Running<'b> {
        self.inner.spawn_preset(call, source, result)
    }

    fn spawn_local<'b>(
        &'b self,
        call: ToolCallInfo,
        source: &'static str,
        run: Running<'b>,
    ) -> Running<'b> {
        self.inner.spawn_local(call, source, run)
    }

    async fn auto_build_check(&self) -> Option<AutoBuildResult> {
        self.inner.auto_build_check().await
    }

    async fn capture_build_baseline(&self) -> Option<BuildBaseline> {
        self.inner.capture_build_baseline().await
    }
}

#[cfg(test)]
mod tests;
