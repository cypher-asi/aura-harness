//! A led turn's tool list, and its calls held to the lead's limits.

use super::{lead_tools, refusal, LeadExecutor, Running, LEAD_SOURCE};
use crate::limits::{refused_command, READ_LINES, RUN_COMMAND, SEARCH_HITS};
use async_trait::async_trait;
use aura_agent::{AgentToolExecutor, ToolCallInfo, ToolCallResult};
use aura_model::ToolDefinition;
use serde_json::{json, Value};
use std::sync::Mutex;

/// Answers every call with `content`, as an error when `failing`, and
/// remembers what ran and what was only recorded.
#[derive(Default)]
struct Recording {
    content: String,
    failing: bool,
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
                let content = self.content.clone();
                if self.failing {
                    ToolCallResult::error(call.id.clone(), content)
                } else {
                    ToolCallResult::success(call.id.clone(), content)
                }
            })
            .collect()
    }

    fn spawn_preset<'a>(
        &'a self,
        call: ToolCallInfo,
        source: &'static str,
        result: ToolCallResult,
    ) -> Running<'a> {
        self.preset
            .lock()
            .unwrap()
            .push((call.name, source, result.content.clone()));
        Box::pin(std::future::ready(result))
    }
}

fn call(name: &str, input: Value) -> ToolCallInfo {
    ToolCallInfo {
        id: format!("toolu_{name}"),
        name: name.to_string(),
        input,
    }
}

fn tool(name: &str) -> ToolDefinition {
    ToolDefinition::new(name, "Does it.", json!({"type": "object"}))
}

/// 300 numbered lines.
fn long_content() -> String {
    (1..=300).map(|n| format!("{n}\n")).collect()
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

#[test]
fn the_lead_is_told_its_limits_word_for_word() {
    let tools = lead_tools(
        [
            "read_file",
            "find_files",
            "search_code",
            "run_command",
            "apply_patch",
            "list_files",
        ]
        .map(tool)
        .to_vec(),
    );
    let described: Vec<(&str, &str)> = tools
        .iter()
        .map(|tool| (tool.name.as_str(), tool.description.as_str()))
        .collect();
    assert_eq!(
        described,
        [
            (
                "read_file",
                "Does it. You see at most 200 lines per read; delegate heavy reading to an \
                 explorer subtask."
            ),
            ("find_files", "Does it. You see at most 50 hits per search."),
            (
                "search_code",
                "Does it. You see at most 50 hits per search."
            ),
            (
                "run_command",
                "Does it. You may only run checks (tests, builds, linters, type checks) and \
                 inspection commands (git status, git diff, git log, git show, ls, pwd); \
                 delegate anything else."
            ),
            ("list_files", "Does it."),
        ]
    );
}

#[test]
fn the_command_line_comes_from_any_argument_form() {
    let refusal_of = |input| refusal(&call(RUN_COMMAND, input));
    assert_eq!(
        refusal_of(json!({"program": "git", "args": ["status"]})),
        None
    );
    assert_eq!(
        refusal_of(json!({"shell_script": "cargo test | tail"})),
        None
    );
    let refused = refusal_of(json!({"program": "rm", "args": ["x"]})).unwrap();
    assert!(refused.contains("Delegate `rm x`"));
    assert_eq!(refused, refused_command("rm x"));
    assert_eq!(
        refusal_of(json!({"shell_script": "cat a | tail"})),
        Some(refused_command("cat a | tail"))
    );
    let refused = refusal(&call("write_file", json!({}))).unwrap();
    assert!(refused.contains("implement subtask"));
}

#[tokio::test]
async fn refused_calls_are_recorded_but_never_run() {
    let inner = Recording::default();
    let lead = LeadExecutor::new(&inner);
    let results = lead
        .execute(&[
            call("write_file", json!({"path": "a.rs", "content": ""})),
            call("run_command", json!({"command": "rm -rf src"})),
            call("run_command", json!({"program": "cargo", "args": ["test"]})),
            call("run_command", json!({"command": "git status"})),
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
async fn a_refused_call_is_recorded_with_its_refusal_word_for_word() {
    let inner = Recording::default();
    let lead = LeadExecutor::new(&inner);
    let results = lead
        .execute(&[
            call("apply_patch", json!({"patch": ""})),
            call("run_command", json!({"shell_script": "cat a.rs | head"})),
        ])
        .await;
    let tool = "Not run: under Gate you do not change files yourself. Delegate the edit with \
                the task tool (an implement subtask, with a check command); `apply_patch` is \
                for subagents.";
    let command = "Not run: under Gate your own run_command only runs checks (tests, builds, \
                   linters, type checks) and inspection commands (git status, git diff, git \
                   log, git show, ls, pwd). Delegate `cat a.rs | head` with the task tool: an \
                   implement subtask for changes, an explorer subtask for reading.";
    assert_eq!(
        *inner.preset.lock().unwrap(),
        [
            ("apply_patch".to_string(), "gate_lead", tool.to_string()),
            ("run_command".to_string(), "gate_lead", command.to_string()),
        ]
    );
    let seen: Vec<(&str, bool, &str)> = results
        .iter()
        .map(|r| (r.tool_use_id.as_str(), r.is_error, r.content.as_str()))
        .collect();
    assert_eq!(
        seen,
        [
            ("toolu_apply_patch", true, tool),
            ("toolu_run_command", true, command)
        ]
    );
    assert!(inner.ran.lock().unwrap().is_empty());
}

#[tokio::test]
async fn reads_and_searches_come_back_capped() {
    let inner = Recording {
        content: long_content(),
        ..Recording::default()
    };
    let lead = LeadExecutor::new(&inner);
    let read = lead
        .spawn_tool_call(call("read_file", json!({"path": "big.rs"})))
        .await;
    assert_eq!(read.content.lines().count(), READ_LINES + 1);
    let search = lead
        .spawn_tool_call(call("search_code", json!({"pattern": "x"})))
        .await;
    assert_eq!(search.content.lines().count(), SEARCH_HITS + 1);
    let listed = lead.spawn_tool_call(call("list_files", json!({}))).await;
    assert_eq!(
        listed.content.lines().count(),
        300,
        "other tools are left alone"
    );
}

#[tokio::test]
async fn a_failed_read_comes_back_whole() {
    let inner = Recording {
        content: long_content(),
        failing: true,
        ..Recording::default()
    };
    let read = LeadExecutor::new(&inner)
        .spawn_tool_call(call("read_file", json!({"path": "big.rs"})))
        .await;
    assert!(read.is_error);
    assert_eq!(read.content, long_content());
}
