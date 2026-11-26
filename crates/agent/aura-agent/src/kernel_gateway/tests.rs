use super::*;
use aura_agent_kernel::{ExecutorRouter, KernelConfig};
use aura_core_types::AgentId;
use aura_exec_tools::ToolExecutor;
use aura_model::{Message, MockProvider};
use aura_store_db::RocksStore;
use serde_json::json;
use tempfile::TempDir;

fn create_test_kernel() -> (Arc<Kernel>, TempDir, TempDir) {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();
    let store: Arc<dyn aura_store_db::Store> =
        Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("gateway test response"));
    let executor = ExecutorRouter::new();
    let config = KernelConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        ..KernelConfig::default()
    };
    let kernel = Arc::new(Kernel::new(store, provider, executor, config, agent_id).unwrap());
    (kernel, db_dir, ws_dir)
}

fn create_tool_kernel(
    workspace_root: &std::path::Path,
    use_workspace_base_as_root: bool,
) -> (Arc<Kernel>, TempDir) {
    let db_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();
    let store: Arc<dyn aura_store_db::Store> =
        Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("gateway test response"));
    let mut executor = ExecutorRouter::new();
    executor.add_executor(Arc::new(ToolExecutor::with_defaults()));
    let config = KernelConfig {
        workspace_base: workspace_root.to_path_buf(),
        use_workspace_base_as_root,
        ..KernelConfig::default()
    };
    (
        Arc::new(Kernel::new(store, provider, executor, config, agent_id).unwrap()),
        db_dir,
    )
}

#[tokio::test]
async fn test_model_gateway_complete() {
    let (kernel, _db, _ws) = create_test_kernel();
    let gateway = KernelModelGateway::new(kernel);

    let request = ModelRequest::builder("test-model", "system")
        .message(Message::user("hello"))
        .try_build()
        .unwrap();
    let response = gateway.complete(request).await.unwrap();
    assert!(!response.message.content.is_empty());
}

#[tokio::test]
async fn test_model_gateway_name() {
    let (kernel, _db, _ws) = create_test_kernel();
    let gateway = KernelModelGateway::new(kernel);
    assert_eq!(gateway.name(), "kernel-gateway");
}

#[tokio::test]
async fn test_tool_gateway_empty_batch() {
    let (kernel, _db, _ws) = create_test_kernel();
    let gateway = KernelToolGateway::new(kernel);
    let results = gateway.execute(&[]).await;
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_tool_gateway_reads_from_workspace_root_when_configured() {
    let ws_dir = TempDir::new().unwrap();
    std::fs::write(
        ws_dir.path().join("reference.md"),
        "This file lives in the workspace root.",
    )
    .unwrap();

    let (kernel, _db_dir) = create_tool_kernel(ws_dir.path(), true);
    let gateway = KernelToolGateway::new(kernel);
    let results = gateway
        .execute(&[ToolCallInfo {
            id: "tool-1".into(),
            name: "read_file".into(),
            input: json!({ "path": "reference.md" }),
        }])
        .await;

    assert_eq!(results.len(), 1);
    assert!(
        !results[0].is_error,
        "tool output was {}",
        results[0].content
    );

    // The kernel decodes ToolResult.stdout/stderr from their
    // base64-on-the-wire form back to plain UTF-8 text before
    // handing content to the LLM, so the LLM no longer sees the
    // JSON-wrapped payload.
    assert_eq!(
        results[0].content,
        "1|This file lives in the workspace root."
    );
}

#[tokio::test]
async fn test_tool_gateway_records_a_preset_without_running_the_tool() {
    let ws_dir = TempDir::new().unwrap();
    let (kernel, _db_dir) = create_tool_kernel(ws_dir.path(), true);
    let gateway = KernelToolGateway::new(kernel.clone());
    let call = ToolCallInfo {
        id: "tool-1".into(),
        name: "read_file".into(),
        input: json!({ "path": "missing.md" }),
    };

    let result = gateway
        .spawn_preset(
            call,
            "cache",
            ToolCallResult::success("tool-1", "cached body"),
        )
        .await;

    assert!(!result.is_error);
    assert_eq!(result.content, "cached body");
    let entries = kernel.store().scan_record(kernel.agent_id, 1, 10).unwrap();
    let types: Vec<_> = entries.iter().map(|e| e.tx.tx_type).collect();
    assert_eq!(
        types,
        vec![
            aura_core_types::TransactionType::ToolProposal,
            aura_core_types::TransactionType::PolicyDecision,
            aura_core_types::TransactionType::ToolExecution,
        ]
    );
}

fn task_done_call() -> ToolCallInfo {
    ToolCallInfo {
        id: "td-1".into(),
        name: "task_done".into(),
        input: json!({ "notes": "done" }),
    }
}

#[tokio::test]
async fn test_tool_gateway_runs_an_allowed_local_handler_and_keeps_its_result() {
    let ws_dir = TempDir::new().unwrap();
    let (kernel, _db_dir) = create_tool_kernel(ws_dir.path(), true);
    let gateway = KernelToolGateway::new(kernel.clone());
    let run = Box::pin(async {
        let mut result = ToolCallResult::success("td-1", "completed");
        result.stop_loop = true;
        result
    });

    let result = gateway
        .spawn_local(task_done_call(), "task_executor", run)
        .await;

    assert_eq!(result.content, "completed");
    assert!(result.stop_loop);
    let entries = kernel.store().scan_record(kernel.agent_id, 1, 10).unwrap();
    assert_eq!(entries.len(), 3);
    let proposal: aura_core_types::ToolProposal =
        serde_json::from_slice(&entries[0].tx.payload).unwrap();
    assert_eq!(proposal.source.as_deref(), Some("task_executor"));
}

#[tokio::test]
async fn test_tool_gateway_denied_local_handler_never_runs() {
    let ws_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    let store: Arc<dyn aura_store_db::Store> =
        Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let config = KernelConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        policy: aura_agent_kernel::PolicyConfig::default().with_agent_override(Some(
            aura_core_types::AgentToolPermissions::new()
                .with("task_done", aura_core_types::ToolState::Deny),
        )),
        ..KernelConfig::default()
    };
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("unused"));
    let kernel = Arc::new(
        Kernel::new(
            store,
            provider,
            ExecutorRouter::new(),
            config,
            AgentId::generate(),
        )
        .unwrap(),
    );
    let gateway = KernelToolGateway::new(kernel);
    let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = ran.clone();
    let run = Box::pin(async move {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        let mut result = ToolCallResult::success("td-1", "completed");
        result.stop_loop = true;
        result
    });

    let result = gateway
        .spawn_local(task_done_call(), "task_executor", run)
        .await;

    assert!(result.is_error);
    assert!(!result.stop_loop);
    assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
}
