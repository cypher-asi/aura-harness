use super::*;

#[tokio::test]
async fn test_session_start_clears_policy_session_tool_states() {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("test response"));
    let executor = ExecutorRouter::new();
    let config = KernelConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        policy: crate::policy::PolicyConfig::default(),
        ..KernelConfig::default()
    };
    let kernel = Kernel::new(store, provider, executor, config, agent_id).unwrap();
    kernel
        .policy
        .remember_tool_state_for_session("guarded_tool", aura_core_types::ToolState::Deny);
    assert!(
        !kernel
            .policy
            .check_tool("guarded_tool", &serde_json::json!({}))
            .allowed
    );

    kernel
        .process_direct(Transaction::session_start(agent_id))
        .await
        .unwrap();

    assert!(
        kernel
            .policy
            .check_tool("guarded_tool", &serde_json::json!({}))
            .allowed
    );
}

#[tokio::test]
async fn test_tool_proposal_denied_without_required_integration() {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("test response"));
    let executor = ExecutorRouter::new();
    let mut policy = crate::policy::PolicyConfig::default();
    policy.set_tool_integration_requirements([(
        "brave_search_web".to_string(),
        InstalledToolIntegrationRequirement {
            integration_id: None,
            provider: Some("brave_search".to_string()),
            kind: Some("workspace_integration".to_string()),
        },
    )]);
    let config = KernelConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        policy,
        ..KernelConfig::default()
    };
    let kernel = Kernel::new(store, provider, executor, config, agent_id).unwrap();
    let proposal = ToolProposal::new(
        "tool-use-1",
        "brave_search_web",
        serde_json::json!({ "query": "aura os" }),
    );
    let tx = Transaction::tool_proposal(agent_id, &proposal).unwrap();

    let result = kernel.process_direct(tx).await.unwrap();

    assert!(result
        .tool_output
        .as_ref()
        .is_some_and(|output| output.is_error));
    assert!(result
        .tool_output
        .as_ref()
        .is_some_and(|output| output.content.contains("installed integration")));
}

#[tokio::test]
async fn test_capability_install_persists_runtime_capability_ledger() {
    let (kernel, _db, _ws) = create_new_kernel();
    let runtime_capabilities = RuntimeCapabilityInstall {
        system_kind: SystemKind::CapabilityInstall,
        scope: "session".to_string(),
        session_id: Some("session-1".to_string()),
        installed_integrations: vec![InstalledIntegrationDefinition {
            integration_id: "integration-brave-1".to_string(),
            name: "Brave Search".to_string(),
            provider: "brave_search".to_string(),
            kind: "workspace_integration".to_string(),
            metadata: HashMap::new(),
        }],
        installed_tools: vec![InstalledToolCapability {
            name: "brave_search_web".to_string(),
            required_integration: Some(InstalledToolIntegrationRequirement {
                integration_id: None,
                provider: Some("brave_search".to_string()),
                kind: Some("workspace_integration".to_string()),
            }),
        }],
    };
    let tx = Transaction::new_chained(
        kernel.agent_id,
        TransactionType::System,
        serde_json::to_vec(&runtime_capabilities).unwrap(),
        None,
    );

    kernel.process_direct(tx).await.unwrap();

    let persisted = kernel
        .store()
        .get_runtime_capabilities(kernel.agent_id)
        .unwrap();
    assert_eq!(persisted, Some(runtime_capabilities));
}

#[tokio::test]
async fn test_session_start_clears_persisted_runtime_capability_ledger() {
    let (kernel, _db, _ws) = create_new_kernel();
    let runtime_capabilities = RuntimeCapabilityInstall {
        system_kind: SystemKind::CapabilityInstall,
        scope: "session".to_string(),
        session_id: Some("session-1".to_string()),
        installed_integrations: vec![],
        installed_tools: vec![InstalledToolCapability {
            name: "brave_search_web".to_string(),
            required_integration: None,
        }],
    };
    let capability_tx = Transaction::new_chained(
        kernel.agent_id,
        TransactionType::System,
        serde_json::to_vec(&runtime_capabilities).unwrap(),
        None,
    );
    kernel.process_direct(capability_tx).await.unwrap();
    assert!(kernel
        .store()
        .get_runtime_capabilities(kernel.agent_id)
        .unwrap()
        .is_some());

    kernel
        .process_direct(Transaction::session_start(kernel.agent_id))
        .await
        .unwrap();

    assert_eq!(
        kernel
            .store()
            .get_runtime_capabilities(kernel.agent_id)
            .unwrap(),
        None
    );
}

/// Policy with a Brave Search integration installed and required by
/// `brave_search_web`.
fn brave_search_policy() -> crate::policy::PolicyConfig {
    let mut policy = crate::policy::PolicyConfig::default();
    policy.set_installed_integrations([InstalledIntegrationDefinition {
        integration_id: "integration-brave-1".to_string(),
        name: "Brave Search".to_string(),
        provider: "brave_search".to_string(),
        kind: "workspace_integration".to_string(),
        metadata: HashMap::new(),
    }]);
    policy.set_tool_integration_requirements([(
        "brave_search_web".to_string(),
        InstalledToolIntegrationRequirement {
            integration_id: None,
            provider: Some("brave_search".to_string()),
            kind: Some("workspace_integration".to_string()),
        },
    )]);
    policy
}

#[tokio::test]
async fn test_tool_proposal_uses_persisted_runtime_capability_ledger() {
    let KernelFixture {
        kernel, agent_id, ..
    } = kernel_with_policy(brave_search_policy());

    let empty_runtime_capabilities = RuntimeCapabilityInstall {
        system_kind: SystemKind::CapabilityInstall,
        scope: "session".to_string(),
        session_id: Some("session-1".to_string()),
        installed_integrations: vec![],
        installed_tools: vec![],
    };
    let capability_tx = Transaction::new_chained(
        kernel.agent_id,
        TransactionType::System,
        serde_json::to_vec(&empty_runtime_capabilities).unwrap(),
        None,
    );
    kernel.process_direct(capability_tx).await.unwrap();

    let proposal = ToolProposal::new(
        "tool-use-1",
        "brave_search_web",
        serde_json::json!({ "query": "aura os" }),
    );
    let tx = Transaction::tool_proposal(agent_id, &proposal).unwrap();
    let result = kernel.process_direct(tx).await.unwrap();

    assert!(result
        .tool_output
        .as_ref()
        .is_some_and(|output| output.is_error));
    assert!(result
        .tool_output
        .as_ref()
        .is_some_and(|output| output.content.contains("kernel runtime capability ledger")));
}

#[tokio::test]
async fn tool_proposal_denied_when_delegate_action_kind_not_allowed() {
    // Regression: `process_tool_proposal` used to call only
    // `Policy::check_tool_with_runtime_capabilities`, which skipped the
    // `allowed_action_kinds` gate. Route now runs the full
    // `Policy::check_with_runtime_capabilities` pipeline so a tool
    // proposal is rejected when `ActionKind::Delegate` is not in the
    // policy's allow-list (Invariant §4).
    use std::collections::HashSet;

    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("test response"));
    let executor = ExecutorRouter::new();

    // `Delegate` intentionally absent — every other kind is allowed.
    let mut allowed_action_kinds = HashSet::new();
    allowed_action_kinds.insert(ActionKind::Reason);
    allowed_action_kinds.insert(ActionKind::Memorize);
    allowed_action_kinds.insert(ActionKind::Decide);

    let policy = crate::policy::PolicyConfig {
        allowed_action_kinds,
        ..crate::policy::PolicyConfig::default()
    };

    let config = KernelConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        policy,
        ..KernelConfig::default()
    };
    let kernel = Kernel::new(store, provider, executor, config, agent_id).unwrap();

    let proposal = ToolProposal::new(
        "tool-use-1",
        "read_file",
        serde_json::json!({ "path": "a.txt" }),
    );
    let tx = Transaction::tool_proposal(agent_id, &proposal).unwrap();
    let result = kernel.process_direct(tx).await.unwrap();

    let output = result.tool_output.expect("tool output");
    assert!(
        output.is_error,
        "expected denial but got content: {}",
        output.content
    );
    assert!(
        output.content.contains("Action kind"),
        "expected action-kind rejection reason, got: {}",
        output.content
    );
}

#[test]
fn write_system_record_resolves_concurrent_appends_without_loss() {
    use bytes::Bytes;

    let db_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());

    // Many OS threads racing system-record appends against the same
    // agent stream is exactly the SubagentSpawn-vs-parent-turn race
    // that surfaced as "spawn rejected by audit kernel: sequence
    // mismatch". With the store's append serialization + the
    // write_system_record retry loop, every writer must succeed with a
    // unique, contiguous seq and no lost updates.
    const WRITERS: u64 = 12;
    let handles: Vec<_> = (0..WRITERS)
        .map(|i| {
            let store = store.clone();
            std::thread::spawn(move || {
                let tx = Transaction::new_chained(
                    agent_id,
                    TransactionType::System,
                    Bytes::from(format!("payload-{i}").into_bytes()),
                    None,
                );
                crate::write_system_record(&store, agent_id, &tx)
            })
        })
        .collect();

    let mut seqs: Vec<u64> = handles
        .into_iter()
        .map(|h| {
            h.join()
                .expect("writer thread panicked")
                .expect("write_system_record must not fail under contention")
        })
        .collect();
    seqs.sort_unstable();

    assert_eq!(
        seqs,
        (1..=WRITERS).collect::<Vec<_>>(),
        "expected unique contiguous seqs with no lost updates"
    );
    assert_eq!(store.get_head_seq(agent_id).unwrap(), WRITERS);
}

#[tokio::test]
async fn plan_mode_denial_is_recorded_and_markdown_write_passes_the_gate() {
    let KernelFixture {
        kernel,
        store,
        agent_id,
        ..
    } = kernel_with_policy(
        crate::policy::PolicyConfig::default().with_mode(aura_core_modes::AgentMode::Plan),
    );

    let code_write = ToolProposal::new(
        "tool-use-rs",
        "write_file",
        serde_json::json!({ "path": "src/lib.rs", "content": "fn main() {}" }),
    );
    let result = kernel
        .process_direct(Transaction::tool_proposal(agent_id, &code_write).unwrap())
        .await
        .unwrap();
    let output = result.tool_output.expect("tool output");
    assert!(output.is_error);
    assert!(output.content.contains("Plan mode"), "{}", output.content);

    let entries = store.scan_record(agent_id, 1, 10).unwrap();
    let denied = entries.last().expect("denial must be recorded");
    assert!(denied.actions.is_empty());
    assert!(
        denied
            .decision
            .rejected
            .iter()
            .any(|r| r.reason.contains("Plan mode")),
        "rejection reason must be in the record: {:?}",
        denied.decision
    );

    let doc_write = ToolProposal::new(
        "tool-use-md",
        "write_file",
        serde_json::json!({ "path": "PLAN.md", "content": "# Plan" }),
    );
    let result = kernel
        .process_direct(Transaction::tool_proposal(agent_id, &doc_write).unwrap())
        .await
        .unwrap();
    let output = result.tool_output.expect("tool output");
    assert!(
        !output.content.contains("Plan mode"),
        "markdown write must pass the mode gate: {}",
        output.content
    );
}
