//! The tools a run offers its model, and what its mode and approval
//! level let them do.

use super::{dirs, execute, headless, headless_in, request};
use crate::event_loop::Permissions;
use crate::headless::output::ExitReason;
use crate::headless::run::SessionChoice;
use crate::headless::tools::ToolFilter;
use crate::headless::OutputFormat;
use aura_core_modes::AgentMode;
use aura_model::{MockProvider, MockResponse};
use aura_ui::{ApprovalLevel, PermissionMode};
use serde_json::json;

#[tokio::test]
async fn a_tool_outside_the_allow_list_is_denied() {
    let dirs = dirs();
    let provider = MockProvider::new()
        .with_response(MockResponse::tool_use(
            "w1",
            "write_file",
            json!({"path": "out.txt", "content": "hi"}),
        ))
        .with_response(MockResponse::text("Tried."));
    let filter = ToolFilter::new(&["read_file".to_string()], &[]);
    let (mut h, requests) = headless(&dirs, provider, filter);

    let (report, out) = execute(
        &mut h,
        request("write", SessionChoice::New),
        OutputFormat::StreamJson,
    )
    .await;

    assert_eq!(report.exit_reason, ExitReason::Success);
    assert!(!dirs.workspace.path().join("out.txt").exists());
    let lines = out.json_lines();
    let offered = lines[0]["tools"].as_array().unwrap();
    assert!(offered.iter().any(|t| t == "read_file"));
    assert!(!offered.iter().any(|t| t == "write_file"));
    let result = lines.iter().find(|l| l["type"] == "tool_result").unwrap();
    assert_eq!(result["is_error"], true, "{result}");
    let sent_tools: Vec<String> = requests.lock().unwrap()[0]
        .tools
        .iter()
        .map(|t| t.name.clone())
        .collect();
    assert!(
        !sent_tools.iter().any(|t| t == "write_file"),
        "{sent_tools:?}"
    );
}

#[tokio::test]
async fn web_tools_can_be_disallowed() {
    let dirs = dirs();
    let provider = MockProvider::new()
        .with_response(MockResponse::tool_use(
            "f1",
            "web_fetch",
            json!({"url": "https://example.com"}),
        ))
        .with_response(MockResponse::text("Tried."));
    let filter = ToolFilter::new(&[], &["web_fetch,web_search".to_string()]);
    let (mut h, _) = headless(&dirs, provider, filter);

    let (report, out) = execute(
        &mut h,
        request("fetch", SessionChoice::New),
        OutputFormat::StreamJson,
    )
    .await;

    assert_eq!(report.exit_reason, ExitReason::Success);
    let lines = out.json_lines();
    let offered = lines[0]["tools"].as_array().unwrap();
    assert!(offered.iter().any(|t| t == "read_file"), "{offered:?}");
    assert!(!offered
        .iter()
        .any(|t| t == "web_fetch" || t == "web_search"));
    let result = lines.iter().find(|l| l["type"] == "tool_result").unwrap();
    assert_eq!(result["is_error"], true, "{result}");
}

fn write_out_txt() -> MockProvider {
    MockProvider::new()
        .with_response(MockResponse::tool_use(
            "w1",
            "write_file",
            json!({"path": "out.txt", "content": "hi"}),
        ))
        .with_response(MockResponse::text("Wrote it."))
}

#[tokio::test]
async fn agent_mode_runs_edits_without_asking() {
    let dirs = dirs();
    let (mut h, _) = headless(&dirs, write_out_txt(), ToolFilter::default());

    let (report, _) = execute(
        &mut h,
        request("write", SessionChoice::New),
        OutputFormat::Json,
    )
    .await;

    assert_eq!(report.exit_reason, ExitReason::Success);
    assert!(dirs.workspace.path().join("out.txt").exists());
}

#[tokio::test]
async fn a_call_the_mode_asks_about_is_denied() {
    let dirs = dirs();
    let (mut h, _) = headless_in(
        &dirs,
        write_out_txt(),
        ToolFilter::default(),
        Permissions::new(PermissionMode::Agent, ApprovalLevel::Ask),
    );

    let (report, out) = execute(
        &mut h,
        request("write", SessionChoice::New),
        OutputFormat::StreamJson,
    )
    .await;

    assert_eq!(report.exit_reason, ExitReason::Success);
    assert!(!dirs.workspace.path().join("out.txt").exists());
    let lines = out.json_lines();
    let result = lines.iter().find(|l| l["type"] == "tool_result").unwrap();
    assert_eq!(result["is_error"], true, "{result}");
}

#[test]
fn headless_runs_agent_mode_at_yolo_unless_told_otherwise() {
    use crate::headless::permissions;
    let agent = permissions(AgentMode::Agent, None);
    assert_eq!(agent.mode, PermissionMode::Agent);
    assert_eq!(agent.approval, ApprovalLevel::Yolo);
    for mode in [AgentMode::Plan, AgentMode::Ask, AgentMode::Debug] {
        let permissions = permissions(mode, None);
        assert_eq!(permissions.mode, PermissionMode::from_agent_mode(mode));
        assert_eq!(permissions.mode.agent_mode(), mode);
        assert_eq!(permissions.approval, ApprovalLevel::Ask);
    }
    assert_eq!(
        permissions(AgentMode::Agent, Some(ApprovalLevel::Strict)).approval,
        ApprovalLevel::Strict
    );
}
