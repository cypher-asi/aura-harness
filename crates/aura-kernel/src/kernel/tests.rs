//! Integration tests for the `Kernel` surface.
//!
//! These tests cover cross-cutting behavior (sequence monotonicity across
//! process + reason, policy / runtime-capability interaction, session
//! boundary clearing) so they live alongside `Kernel` itself rather than
//! being split per-file.

use super::*;
use crate::executor::ExecuteContext;
use crate::ExecutorRouter;
use aura_core::{
    ActionKind, AgentId, InstalledIntegrationDefinition, InstalledToolCapability,
    InstalledToolIntegrationRequirement, RuntimeCapabilityInstall, SystemKind, ToolProposal,
    Transaction, TransactionType,
};
use aura_reasoner::{MockProvider, ModelProvider, ModelRequest};
use aura_store::{RocksStore, Store};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

// Keep `ExecuteContext` + `ExecutorRouter` imports visible even when the
// tests below only exercise them transitively — they're part of the
// kernel's public surface and the tests construct them directly when
// building custom policies.
#[allow(dead_code)]
fn _keep_imports_alive(_: &ExecuteContext) {}

fn create_new_kernel() -> (Kernel, TempDir, TempDir) {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("test response"));
    let executor = ExecutorRouter::new();
    let config = KernelConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        ..KernelConfig::default()
    };
    let kernel = Kernel::new(store, provider, executor, config, agent_id).unwrap();
    (kernel, db_dir, ws_dir)
}

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

#[tokio::test]
async fn test_reason_records_and_returns_response() {
    let (kernel, _db, _ws) = create_new_kernel();
    let request = ModelRequest::builder("test-model", "system prompt")
        .message(aura_reasoner::Message::user("hello"))
        .build();
    let result = kernel.reason(request).await.unwrap();
    assert_eq!(result.entry.seq, 1);
    assert!(!result.response.message.content.is_empty());
}

#[tokio::test]
async fn test_sequence_across_process_and_reason() {
    let (kernel, _db, _ws) = create_new_kernel();

    let tx = Transaction::user_prompt(kernel.agent_id, "prompt");
    let r1 = kernel.process_direct(tx).await.unwrap();
    assert_eq!(r1.entry.seq, 1);

    let request = ModelRequest::builder("test-model", "system")
        .message(aura_reasoner::Message::user("test"))
        .build();
    let r2 = kernel.reason(request).await.unwrap();
    assert_eq!(r2.entry.seq, 2);

    let tx2 = Transaction::new_chained(
        kernel.agent_id,
        TransactionType::AgentMsg,
        "response".as_bytes().to_vec(),
        None,
    );
    let r3 = kernel.process_direct(tx2).await.unwrap();
    assert_eq!(r3.entry.seq, 3);
}

#[tokio::test]
async fn test_session_start_clears_policy_session_approvals() {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("test response"));
    let executor = ExecutorRouter::new();
    let config = KernelConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        policy: crate::policy::PolicyConfig::default()
            .with_tool_permission("guarded_tool", crate::policy::PermissionLevel::AskOnce),
        ..KernelConfig::default()
    };
    let kernel = Kernel::new(store, provider, executor, config, agent_id).unwrap();
    kernel.policy.approve_for_session("guarded_tool");
    assert!(kernel.policy.is_session_approved("guarded_tool"));

    kernel
        .process_direct(Transaction::session_start(agent_id))
        .await
        .unwrap();

    assert!(!kernel.policy.is_session_approved("guarded_tool"));
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
    policy.add_allowed_tool("brave_search_web");
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

#[tokio::test]
async fn test_tool_proposal_uses_persisted_runtime_capability_ledger() {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("test response"));
    let executor = ExecutorRouter::new();
    let mut policy = crate::policy::PolicyConfig::default();
    policy.add_allowed_tool("brave_search_web");
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
    let config = KernelConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        policy,
        ..KernelConfig::default()
    };
    let kernel = Kernel::new(store, provider, executor, config, agent_id).unwrap();

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

    let mut policy = crate::policy::PolicyConfig {
        allowed_action_kinds,
        ..crate::policy::PolicyConfig::default()
    };
    policy.add_allowed_tool("read_file");

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
