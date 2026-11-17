//! The main agent's tools when it leads under Gate with delegation
//! enforced.
//!
//! The turn's tool list drops the file-changing tools ([`lead_tools`]),
//! and every call goes through [`LeadExecutor`], which refuses those
//! tools and the commands the lead may not run, and caps what reads and
//! searches return (see [`aura_agent_gate::lead`]). A refused call is
//! still recorded, with the refusal as its result, so the record shows
//! what the lead tried. Subagents get neither.

use async_trait::async_trait;
use aura_agent::{AgentToolExecutor, AutoBuildResult, BuildBaseline, ToolCallInfo, ToolCallResult};
use aura_agent_gate::lead;
use aura_model::ToolDefinition;
use std::future::Future;
use std::pin::Pin;

/// `ToolProposal::source` of the lead's refused calls.
const LEAD_SOURCE: &str = "gate_lead";

/// `tools` as the lead gets them: without the file-changing tools, and
/// with the capped tools saying so.
pub(crate) fn lead_tools(tools: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
    tools
        .into_iter()
        .filter(|tool| lead::keeps_tool(&tool.name))
        .map(|mut tool| {
            let note = if lead::READ_TOOLS.contains(&tool.name.as_str()) {
                Some(format!(
                    " You see at most {} lines per read; delegate heavy reading to an explorer \
                     subtask.",
                    lead::READ_LINES
                ))
            } else if lead::SEARCH_TOOLS.contains(&tool.name.as_str()) {
                Some(format!(
                    " You see at most {} hits per search.",
                    lead::SEARCH_HITS
                ))
            } else if tool.name == lead::RUN_COMMAND {
                Some(
                    " You may only run checks (tests, builds, linters, type checks) and \
                     inspection commands (git status, git diff, git log, git show, ls, pwd); \
                     delegate anything else."
                        .to_string(),
                )
            } else {
                None
            };
            if let Some(note) = note {
                tool.description.push_str(&note);
            }
            tool
        })
        .collect()
}

/// Why the lead may not make `call`, if it may not.
fn refusal(call: &ToolCallInfo) -> Option<String> {
    if !lead::keeps_tool(&call.name) {
        return Some(lead::refused_tool(&call.name));
    }
    if call.name == lead::RUN_COMMAND {
        let command = lead::command_of(&call.input);
        if !lead::allows_command(&command) {
            return Some(lead::refused_command(&command));
        }
    }
    None
}

/// Cap what `result` of a call to `tool` shows the lead.
fn shape(tool: &str, result: &mut ToolCallResult) {
    if result.is_error {
        return;
    }
    let capped = if lead::READ_TOOLS.contains(&tool) {
        lead::cap_read(&result.content)
    } else if lead::SEARCH_TOOLS.contains(&tool) {
        lead::cap_search(&result.content)
    } else {
        None
    };
    if let Some(capped) = capped {
        result.content = capped;
    }
}

/// Runs the lead's calls on `inner` within the lead's limits.
pub(crate) struct LeadExecutor<'a> {
    inner: &'a dyn AgentToolExecutor,
}

impl<'a> LeadExecutor<'a> {
    pub(crate) fn new(inner: &'a dyn AgentToolExecutor) -> Self {
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

    fn spawn_tool_call<'b>(
        &'b self,
        call: ToolCallInfo,
    ) -> Pin<Box<dyn Future<Output = ToolCallResult> + Send + 'b>> {
        if let Some(why) = refusal(&call) {
            let refused = ToolCallResult::error(call.id.clone(), why);
            return self.inner.spawn_preset(call, LEAD_SOURCE, refused);
        }
        let tool = call.name.clone();
        let run = self.inner.spawn_tool_call(call);
        Box::pin(async move {
            let mut result = run.await;
            shape(&tool, &mut result);
            result
        })
    }

    fn spawn_preset<'b>(
        &'b self,
        call: ToolCallInfo,
        source: &'static str,
        result: ToolCallResult,
    ) -> Pin<Box<dyn Future<Output = ToolCallResult> + Send + 'b>> {
        self.inner.spawn_preset(call, source, result)
    }

    fn spawn_local<'b>(
        &'b self,
        call: ToolCallInfo,
        source: &'static str,
        run: Pin<Box<dyn Future<Output = ToolCallResult> + Send + 'b>>,
    ) -> Pin<Box<dyn Future<Output = ToolCallResult> + Send + 'b>> {
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
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Answers every call with `content` and remembers what ran and
    /// what was only recorded.
    #[derive(Default)]
    struct Recording {
        content: String,
        ran: Mutex<Vec<String>>,
        preset: Mutex<Vec<(String, &'static str, String)>>,
    }

    #[async_trait]
    impl AgentToolExecutor for Recording {
        async fn execute(&self, tool_calls: &[ToolCallInfo]) -> Vec<ToolCallResult> {
            tool_calls
                .iter()
                .map(|call| {
                    self.ran.lock().unwrap().push(call.name.clone());
                    ToolCallResult::success(call.id.clone(), self.content.clone())
                })
                .collect()
        }

        fn spawn_preset<'a>(
            &'a self,
            call: ToolCallInfo,
            source: &'static str,
            result: ToolCallResult,
        ) -> Pin<Box<dyn Future<Output = ToolCallResult> + Send + 'a>> {
            self.preset
                .lock()
                .unwrap()
                .push((call.name, source, result.content.clone()));
            Box::pin(std::future::ready(result))
        }
    }

    fn call(name: &str, input: serde_json::Value) -> ToolCallInfo {
        ToolCallInfo {
            id: format!("toolu_{name}"),
            name: name.to_string(),
            input,
        }
    }

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition::new(name, "Does it.", serde_json::json!({"type": "object"}))
    }

    #[test]
    fn the_lead_has_no_file_tools_and_its_limits_are_described() {
        let tools = lead_tools(
            [
                "read_file",
                "write_file",
                "edit_file",
                "delete_file",
                "search_code",
                "run_command",
                "task",
            ]
            .map(tool)
            .to_vec(),
        );
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["read_file", "search_code", "run_command", "task"]);
        assert!(tools[0].description.contains("200 lines"));
        assert!(tools[1].description.contains("50 hits"));
        assert!(tools[2].description.contains("only run checks"));
        assert_eq!(tools[3].description, "Does it.");
    }

    #[tokio::test]
    async fn refused_calls_are_recorded_but_never_run() {
        let inner = Recording::default();
        let lead = LeadExecutor::new(&inner);
        let results = lead
            .execute(&[
                call(
                    "write_file",
                    serde_json::json!({"path": "a.rs", "content": ""}),
                ),
                call("run_command", serde_json::json!({"command": "rm -rf src"})),
                call(
                    "run_command",
                    serde_json::json!({"program": "cargo", "args": ["test"]}),
                ),
                call("run_command", serde_json::json!({"command": "git status"})),
            ])
            .await;
        assert!(results[0].is_error);
        assert!(results[0].content.contains("implement subtask"));
        assert!(results[1].is_error);
        assert!(results[1].content.contains("Delegate `rm -rf src`"));
        assert!(!results[2].is_error);
        assert!(!results[3].is_error);
        assert_eq!(*inner.ran.lock().unwrap(), ["run_command", "run_command"]);
        let preset = inner.preset.lock().unwrap();
        assert_eq!(preset.len(), 2);
        assert!(preset.iter().all(|(_, source, _)| *source == LEAD_SOURCE));
    }

    #[tokio::test]
    async fn reads_and_searches_come_back_capped() {
        let inner = Recording {
            content: (1..=300).map(|n| format!("{n}\n")).collect(),
            ..Recording::default()
        };
        let lead = LeadExecutor::new(&inner);
        let read = lead
            .spawn_tool_call(call("read_file", serde_json::json!({"path": "big.rs"})))
            .await;
        assert_eq!(read.content.lines().count(), lead::READ_LINES + 1);
        let search = lead
            .spawn_tool_call(call("search_code", serde_json::json!({"pattern": "x"})))
            .await;
        assert_eq!(search.content.lines().count(), lead::SEARCH_HITS + 1);
        let listed = lead
            .spawn_tool_call(call("list_files", serde_json::json!({})))
            .await;
        assert_eq!(
            listed.content.lines().count(),
            300,
            "other tools are left alone"
        );
    }
}
