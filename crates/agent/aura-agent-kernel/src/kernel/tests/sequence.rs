use super::*;

#[tokio::test]
async fn test_process_direct_user_prompt() {
    let (kernel, _db, _ws) = create_new_kernel();
    let tx = Transaction::user_prompt(kernel.agent_id, "hello");
    let result = kernel.process_direct(tx).await.unwrap();
    assert_eq!(result.entry.seq, 1);
    assert!(!result.had_failures);
    assert!(result.tool_output.is_none());
}

#[tokio::test]
async fn test_process_direct_increments_seq() {
    let (kernel, _db, _ws) = create_new_kernel();
    let tx1 = Transaction::user_prompt(kernel.agent_id, "first");
    let r1 = kernel.process_direct(tx1).await.unwrap();
    assert_eq!(r1.entry.seq, 1);

    let tx2 = Transaction::user_prompt(kernel.agent_id, "second");
    let r2 = kernel.process_direct(tx2).await.unwrap();
    assert_eq!(r2.entry.seq, 2);
}

#[tokio::test]
async fn process_direct_reconciles_stale_kernel_sequence_with_store_head() {
    let TwoKernels {
        kernel_a,
        kernel_b,
        store,
        agent_id,
        ..
    } = two_kernels_on_one_store();

    let first = kernel_a
        .process_direct(Transaction::user_prompt(agent_id, "first"))
        .await
        .unwrap();
    assert_eq!(first.entry.seq, 1);

    let second = kernel_b
        .process_direct(Transaction::user_prompt(agent_id, "second"))
        .await
        .unwrap();
    assert_eq!(second.entry.seq, 2);
    assert_eq!(store.get_head_seq(agent_id).unwrap(), 2);
}

#[tokio::test]
async fn process_tools_batch_reconciles_stale_kernel_sequence_with_store_head() {
    let TwoKernels {
        kernel_a,
        kernel_b,
        store,
        agent_id,
        ..
    } = two_kernels_on_one_store();

    kernel_a
        .process_direct(Transaction::user_prompt(agent_id, "first"))
        .await
        .unwrap();

    let results = kernel_b
        .process_tools(vec![
            ToolProposal::new(
                "tool-use-1",
                "read_file",
                serde_json::json!({ "path": "a.txt" }),
            ),
            ToolProposal::new(
                "tool-use-2",
                "list_files",
                serde_json::json!({ "path": "." }),
            ),
        ])
        .await
        .unwrap();

    // Proposal + decision pairs are recorded first (2..=5), then the two
    // executions (6, 7).
    let first_decision = results[0].policy_decision.as_ref().unwrap();
    let second_decision = results[1].policy_decision.as_ref().unwrap();
    assert_eq!((first_decision.proposal_seq, first_decision.seq), (2, 3));
    assert_eq!((second_decision.proposal_seq, second_decision.seq), (4, 5));
    assert_eq!(results[0].entry.seq, 6);
    assert_eq!(results[1].entry.seq, 7);
    assert_eq!(store.get_head_seq(agent_id).unwrap(), 7);
}

#[tokio::test]
async fn reason_reconciles_stale_kernel_sequence_with_store_head() {
    let TwoKernels {
        kernel_a,
        kernel_b,
        store,
        agent_id,
        ..
    } = two_kernels_on_one_store();

    kernel_a
        .process_direct(Transaction::user_prompt(agent_id, "first"))
        .await
        .unwrap();

    let request = ModelRequest::builder("test-model", "system")
        .message(aura_model::Message::user("test"))
        .try_build()
        .unwrap();
    let result = kernel_b.reason(request).await.unwrap();

    assert_eq!(result.entry.seq, 2);
    assert_eq!(store.get_head_seq(agent_id).unwrap(), 2);
}

#[test]
fn test_agent_workspace_defaults_to_agent_subdirectory() {
    let (kernel, _db, ws_dir) = create_new_kernel();
    assert_eq!(
        kernel.agent_workspace(),
        ws_dir.path().join(kernel.agent_id.to_hex())
    );
}

#[test]
fn test_agent_workspace_can_use_workspace_base_directly() {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("test response"));
    let executor = ExecutorRouter::new();
    let config = KernelConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        use_workspace_base_as_root: true,
        ..KernelConfig::default()
    };
    let kernel = Kernel::new(store, provider, executor, config, agent_id).unwrap();

    assert_eq!(kernel.agent_workspace(), ws_dir.path());
}
