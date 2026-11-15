use super::*;
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

fn agent() -> AgentId {
    AgentId::new([7; 32])
}

fn prompt(request_id: &str, tool: &str, args: Value) -> PendingToolPrompt {
    PendingToolPrompt {
        request_id: request_id.to_string(),
        tool_name: tool.to_string(),
        args,
        agent_id: agent(),
        remember_options: vec![ToolApprovalRemember::Once],
    }
}

fn cargo_test() -> Value {
    json!({ "program": "cargo", "args": ["test", "-p", "aura-tools"] })
}

struct Harness {
    approvals: Arc<TerminalApprovals>,
    commands: mpsc::Receiver<UiCommand>,
    data: TempDir,
    project: TempDir,
}

fn harness(timeout: Duration) -> Harness {
    let data = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let (tx, commands) = mpsc::channel(16);
    let approvals = Arc::new(TerminalApprovals::with_timeout(tx, data.path(), timeout));
    approvals.set_folder(agent(), project.path().to_path_buf());
    Harness {
        approvals,
        commands,
        data,
        project,
    }
}

/// The approval request the UI got, as `(agent, id, always)`.
async fn requested(commands: &mut mpsc::Receiver<UiCommand>) -> (String, String, String) {
    let command = tokio::time::timeout(Duration::from_secs(5), commands.recv())
        .await
        .unwrap()
        .unwrap();
    match command {
        UiCommand::ForAgent { agent_id, command } => match *command {
            UiCommand::RequestApproval { id, always, .. } => (agent_id, id, always),
            other => panic!("expected an approval request, got {other:?}"),
        },
        other => panic!("expected a command for an agent, got {other:?}"),
    }
}

fn ask(
    approvals: &Arc<TerminalApprovals>,
    request: PendingToolPrompt,
) -> tokio::task::JoinHandle<Result<ToolApprovalResponse, ToolApprovalError>> {
    let approvals = Arc::clone(approvals);
    tokio::spawn(async move { approvals.prompt(request).await })
}

#[tokio::test]
async fn the_answer_reaches_the_waiting_call_of_its_agent() {
    let mut h = harness(DEFAULT_TIMEOUT);
    let call = ask(
        &h.approvals,
        prompt("a:toolu_1", "run_command", cargo_test()),
    );

    let (to, id, always) = requested(&mut h.commands).await;
    assert_eq!(to, agent().to_hex());
    assert_eq!(id, "a:toolu_1");
    assert_eq!(always, "`cargo test` commands in this project");
    assert!(h.approvals.is_waiting(agent()));

    h.approvals.answer(&id, Answer::Once).await.unwrap();
    let response = call.await.unwrap().unwrap();
    assert_eq!(response.decision, ToolState::Allow);
    assert_eq!(response.remember, ToolApprovalRemember::Once);
    assert_eq!(
        h.approvals.answer(&id, Answer::Once).await,
        Err(AnswerError::NotWaiting)
    );
}

#[tokio::test]
async fn deny_is_delivered_as_deny() {
    let mut h = harness(DEFAULT_TIMEOUT);
    let call = ask(
        &h.approvals,
        prompt("a:toolu_1", "write_file", json!({"path": "x.rs"})),
    );
    let (_, id, _) = requested(&mut h.commands).await;
    h.approvals.answer(&id, Answer::Deny).await.unwrap();
    assert_eq!(call.await.unwrap().unwrap().decision, ToolState::Deny);
}

#[tokio::test]
async fn always_allow_is_kept_for_the_project_and_survives_a_restart() {
    let mut h = harness(DEFAULT_TIMEOUT);
    let call = ask(
        &h.approvals,
        prompt("a:toolu_1", "run_command", cargo_test()),
    );
    let (_, id, _) = requested(&mut h.commands).await;
    h.approvals.answer(&id, Answer::Always).await.unwrap();
    assert_eq!(call.await.unwrap().unwrap().decision, ToolState::Allow);

    // A new terminal reads the rule back and asks nothing for the same
    // subcommand, but still asks for another one.
    let (tx, mut commands) = mpsc::channel(16);
    let restarted = Arc::new(TerminalApprovals::with_timeout(
        tx,
        h.data.path(),
        DEFAULT_TIMEOUT,
    ));
    restarted.set_folder(agent(), h.project.path().to_path_buf());
    let other_args = json!({ "program": "cargo", "args": ["test", "--workspace"] });
    let response = restarted
        .prompt(prompt("a:toolu_2", "run_command", other_args))
        .await
        .unwrap();
    assert_eq!(response.decision, ToolState::Allow);
    assert!(commands.try_recv().is_err(), "a ruled call asks nothing");

    let publish = ask(
        &restarted,
        prompt(
            "a:toolu_3",
            "run_command",
            json!({ "program": "cargo", "args": ["publish"] }),
        ),
    );
    let (_, id, _) = requested(&mut commands).await;
    restarted.answer(&id, Answer::Deny).await.unwrap();
    assert_eq!(publish.await.unwrap().unwrap().decision, ToolState::Deny);

    // Another project has no rules.
    restarted.set_folder(agent(), h.data.path().to_path_buf());
    let elsewhere = ask(&restarted, prompt("a:toolu_4", "run_command", cargo_test()));
    let (_, id, _) = requested(&mut commands).await;
    restarted.answer(&id, Answer::Once).await.unwrap();
    elsewhere.await.unwrap().unwrap();
}

#[tokio::test]
async fn cancelling_the_turn_denies_its_waiting_call() {
    let mut h = harness(DEFAULT_TIMEOUT);
    let call = ask(
        &h.approvals,
        prompt("a:toolu_1", "run_command", cargo_test()),
    );
    let (_, id, _) = requested(&mut h.commands).await;

    h.approvals.cancel_agent(AgentId::new([8; 32])).await;
    assert!(
        h.approvals.is_waiting(agent()),
        "other agents are untouched"
    );

    h.approvals.cancel_agent(agent()).await;
    assert_eq!(call.await.unwrap().unwrap().decision, ToolState::Deny);
    let closed = h.commands.recv().await.unwrap();
    assert!(matches!(
        closed,
        UiCommand::ForAgent { command, .. } if matches!(*command, UiCommand::CloseApproval(ref c) if *c == id)
    ));
}

#[tokio::test]
async fn an_unanswered_prompt_is_denied_after_the_timeout() {
    let mut h = harness(Duration::from_millis(50));
    let call = ask(
        &h.approvals,
        prompt("a:toolu_1", "run_command", cargo_test()),
    );
    let (_, id, _) = requested(&mut h.commands).await;

    let response = call.await.unwrap().unwrap();
    assert_eq!(response.decision, ToolState::Deny);
    assert!(!h.approvals.is_waiting(agent()));
    assert_eq!(
        h.approvals.answer(&id, Answer::Once).await,
        Err(AnswerError::NotWaiting)
    );
}

#[tokio::test]
async fn commands_the_user_typed_are_not_asked_about() {
    let mut h = harness(DEFAULT_TIMEOUT);
    let response = h
        .approvals
        .prompt(prompt("aa:user_123", "run_command", cargo_test()))
        .await
        .unwrap();
    assert_eq!(response.decision, ToolState::Allow);
    assert!(h.commands.try_recv().is_err());
}

#[test]
fn rules_match_on_command_words_and_never_on_shell_syntax() {
    let rule = ApprovalRule::for_call("run_command", &cargo_test()).unwrap();
    assert_eq!(
        rule.command.as_deref(),
        Some(&["cargo".to_string(), "test".to_string()][..])
    );
    assert!(rule.allows("run_command", &json!({ "shell_script": "cargo test -q" })));
    assert!(!rule.allows(
        "run_command",
        &json!({ "shell_script": "cargo test && rm -rf ~" })
    ));
    assert!(!rule.allows("run_command", &json!({ "program": "cargo" })));
    assert!(ApprovalRule::for_call("run_command", &json!({ "shell_script": "ls | wc" })).is_none());

    let ls = ApprovalRule::for_call("run_command", &json!({ "program": "ls", "args": ["-la"] }))
        .unwrap();
    assert_eq!(ls.command.as_deref(), Some(&["ls".to_string()][..]));

    let edit = ApprovalRule::for_call("edit_file", &json!({ "path": "a.rs" })).unwrap();
    assert!(edit.allows("edit_file", &json!({ "path": "b.rs" })));
    assert!(!edit.allows("write_file", &json!({ "path": "b.rs" })));
    let bare_command = ApprovalRule {
        tool: "run_command".to_string(),
        command: None,
    };
    assert!(!bare_command.allows("run_command", &cargo_test()));
}

#[test]
fn descriptions_show_the_command_line_and_where_it_runs() {
    let args =
        json!({ "program": "cargo", "args": ["build"], "cwd": "crates/x", "background": true });
    assert_eq!(
        describe("run_command", &args),
        "cargo build (in crates/x, in the background)"
    );
    assert_eq!(
        describe(
            "write_file",
            &json!({ "path": "src/main.rs", "content": "x" })
        ),
        "src/main.rs"
    );
    assert!(describe("odd_tool", &json!({ "x": "y".repeat(1000) })).ends_with('…'));
}
