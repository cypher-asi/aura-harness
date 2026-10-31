use super::*;
use aura_agent_kernel::KernelConfig;
use aura_core_types::{AgentId, EffectStatus, TransactionType};
use aura_model_reasoner::{MockProvider, ModelProvider};
use aura_store_db::{ReadStore, RocksStore};
use aura_tools::ToolConfig;
use std::sync::Arc;
use tempfile::TempDir;

#[test]
fn plain_command_uses_program_and_args() {
    assert_eq!(
        command_args("ls -la src", None),
        json!({ "program": "ls", "args": ["-la", "src"] })
    );
    assert_eq!(
        command_args("ls", None),
        json!({ "program": "ls", "args": [] })
    );
}

#[test]
fn shell_syntax_uses_shell_script() {
    assert_eq!(
        command_args("ls | head -n 1", None),
        json!({ "shell_script": "ls | head -n 1", "allow_shell": true })
    );
    assert_eq!(
        command_args("echo \"a b\"", None),
        json!({ "shell_script": "echo \"a b\"", "allow_shell": true })
    );
}

#[test]
fn terminal_width_is_passed_as_tty_cols() {
    assert_eq!(
        command_args("ls -la", Some(90)),
        json!({ "program": "ls", "args": ["-la"], "tty_cols": 90 })
    );
}

#[test]
fn terminal_output_reads_the_envelope() {
    let ok = json!({ "ok": true, "exit_code": 0, "stdout": "a\n", "stderr": "" }).to_string();
    assert_eq!(terminal_output(&ok, false), ("a\n".to_string(), None));

    let failed = json!({ "ok": false, "exit_code": 2, "stdout": "", "stderr": "no such file\n" })
        .to_string();
    assert_eq!(
        terminal_output(&failed, true),
        ("no such file\n".to_string(), Some(2))
    );

    assert_eq!(
        terminal_output("Policy denied", true),
        ("aura: Policy denied\n".to_string(), None)
    );
}

#[test]
fn model_content_has_no_ansi_styling() {
    let content =
        json!({ "ok": true, "stdout": "\u{1b}[01;34msrc\u{1b}[0m\n", "stderr": "" }).to_string();
    let stripped: Value = serde_json::from_str(&model_content(&content)).unwrap();
    assert_eq!(stripped["stdout"], "src\n");
    assert_eq!(model_content("\u{1b}[31merr\u{1b}[0m"), "err");
}

#[test]
fn proposal_is_a_user_sourced_run_command() {
    let proposal = user_command_proposal("ls", None);
    assert_eq!(proposal.tool, RUN_COMMAND_TOOL);
    assert_eq!(proposal.source.as_deref(), Some(USER_SOURCE));
    assert!(proposal.tool_use_id.starts_with("user_"));
}

fn tool_kernel() -> (Arc<Kernel>, Arc<RocksStore>, TempDir, TempDir) {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();
    let store = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("unused"));
    let (executor, _) = crate::session_helpers::build_executor_router_with_config(
        &ToolConfig::for_autonomous_dev_loop(),
    );
    let config = KernelConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        use_workspace_base_as_root: true,
        ..KernelConfig::default()
    };
    let kernel = Kernel::new(
        store.clone() as Arc<dyn aura_store_db::Store>,
        provider,
        executor,
        config,
        AgentId::generate(),
    )
    .unwrap();
    (Arc::new(kernel), store, db_dir, ws_dir)
}

#[cfg(unix)]
#[tokio::test]
async fn command_is_recorded_as_an_executed_tool_proposal() {
    let (kernel, store, _db, ws) = tool_kernel();
    std::fs::write(ws.path().join("marker.txt"), "x").unwrap();

    let proposal = user_command_proposal("ls", None);
    let (tx, result) = process_user_proposal(&kernel, &proposal).await.unwrap();

    assert_eq!(tx.tx_type, TransactionType::ToolProposal);
    let output = result.tool_output.expect("tool output");
    assert!(!output.is_error, "output was {}", output.content);
    assert!(output.content.contains("marker.txt"), "{}", output.content);

    let entry = store
        .get_record_entry(kernel.agent_id, result.entry.seq)
        .unwrap();
    assert_eq!(entry.tx.tx_type, TransactionType::ToolProposal);
    assert_eq!(entry.actions.len(), 1);
    assert_eq!(entry.effects.len(), 1);
    assert_eq!(entry.effects[0].status, EffectStatus::Committed);
    let recorded: ToolProposal = serde_json::from_slice(&entry.tx.payload).unwrap();
    assert_eq!(recorded.source.as_deref(), Some(USER_SOURCE));
    assert!(store.dequeue_tx(kernel.agent_id).unwrap().is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn shell_command_runs_through_the_kernel() {
    let (kernel, _store, _db, ws) = tool_kernel();
    std::fs::write(ws.path().join("marker.txt"), "x").unwrap();

    let proposal = user_command_proposal("ls | grep marker", None);
    let (_, result) = process_user_proposal(&kernel, &proposal).await.unwrap();

    let output = result.tool_output.expect("tool output");
    assert!(!output.is_error, "output was {}", output.content);
    assert!(output.content.contains("marker.txt"), "{}", output.content);
}

#[cfg(unix)]
#[tokio::test]
async fn command_with_width_runs_in_a_terminal() {
    let (kernel, _store, _db, ws) = tool_kernel();
    std::fs::write(ws.path().join("alpha.txt"), "").unwrap();
    std::fs::create_dir(ws.path().join("bravo")).unwrap();

    let proposal = user_command_proposal("ls", Some(80));
    let (_, result) = process_user_proposal(&kernel, &proposal).await.unwrap();

    let output = result.tool_output.expect("tool output");
    assert!(!output.is_error, "output was {}", output.content);
    let (text, _) = terminal_output(&output.content, false);
    let first_line = strip_ansi(text.lines().next().unwrap_or_default());
    assert!(
        first_line.contains("alpha.txt") && first_line.contains("bravo"),
        "{text:?}"
    );
    if cfg!(target_os = "linux") {
        assert!(
            text.contains("\u{1b}["),
            "directories are colored: {text:?}"
        );
    }
}
