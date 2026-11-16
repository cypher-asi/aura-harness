//! `Kernel::switch_mode`: the change is recorded before it takes effect,
//! and only the kernel may write it.

use super::*;
use crate::ExecutorRouter;
use aura_core_modes::AgentMode;
use aura_core_types::{AgentId, SystemKind, ToolProposal, Transaction, TransactionType};
use aura_model::{MockProvider, ModelProvider};
use aura_store_db::{RocksStore, Store};
use std::sync::Arc;
use tempfile::TempDir;

struct Harness {
    _db: TempDir,
    _ws: TempDir,
    store: Arc<dyn Store>,
    kernel: Kernel,
    agent_id: AgentId,
}

fn harness(mode: AgentMode) -> Harness {
    let db = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();
    let agent_id = AgentId::generate();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(db.path(), false).unwrap());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("ok"));
    let config = KernelConfig {
        workspace_base: ws.path().to_path_buf(),
        policy: crate::policy::PolicyConfig::default().with_mode(mode),
        ..KernelConfig::default()
    };
    let kernel = Kernel::new(
        store.clone(),
        provider,
        ExecutorRouter::new(),
        config,
        agent_id,
    )
    .unwrap();
    Harness {
        _db: db,
        _ws: ws,
        store,
        kernel,
        agent_id,
    }
}

async fn propose_code_write(h: &Harness) -> String {
    let proposal = ToolProposal::new(
        "tool-use-rs",
        "write_file",
        serde_json::json!({ "path": "src/lib.rs", "content": "fn main() {}" }),
    );
    let result = h
        .kernel
        .process_direct(Transaction::tool_proposal(h.agent_id, &proposal).unwrap())
        .await
        .unwrap();
    result.tool_output.expect("tool output").content
}

#[tokio::test]
async fn switch_mode_records_the_change_then_enforces_it() {
    let h = harness(AgentMode::Agent);
    let agent_hash = h.kernel.policy().snapshot_hash();

    let result = h
        .kernel
        .switch_mode(AgentMode::Plan)
        .await
        .unwrap()
        .expect("mode changed");
    assert_eq!(h.kernel.policy().mode(), AgentMode::Plan);
    assert_ne!(h.kernel.policy().snapshot_hash(), agent_hash);

    let payload = result.entry.tx.mode_change_payload().expect("mode_change");
    assert_eq!(payload.system_kind, SystemKind::ModeChange);
    assert_eq!(
        (payload.from, payload.to),
        (AgentMode::Agent, AgentMode::Plan)
    );
    let recorded = h.store.scan_record(h.agent_id, 1, 10).unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].tx.tx_type, TransactionType::System);

    assert!(propose_code_write(&h).await.contains("Plan mode"));

    h.kernel.switch_mode(AgentMode::Agent).await.unwrap();
    assert_eq!(h.kernel.policy().snapshot_hash(), agent_hash);
    assert!(!propose_code_write(&h).await.contains("Plan mode"));
}

#[tokio::test]
async fn switching_to_the_current_mode_records_nothing() {
    let h = harness(AgentMode::Plan);
    assert!(h
        .kernel
        .switch_mode(AgentMode::Plan)
        .await
        .unwrap()
        .is_none());
    assert_eq!(h.store.get_head_seq(h.agent_id).unwrap(), 0);
}

#[tokio::test]
async fn switch_permissions_records_a_table_change_within_one_mode() {
    use aura_core_types::UserToolDefaults;
    let h = harness(AgentMode::Agent);

    let result = h
        .kernel
        .switch_permissions(
            AgentMode::Agent,
            UserToolDefaults::full_access(),
            "default",
            "unrestricted",
        )
        .await
        .unwrap()
        .expect("permissions changed");
    let payload = result.entry.tx.mode_change_payload().expect("mode_change");
    assert_eq!(
        (
            payload.from_permissions.as_deref(),
            payload.to_permissions.as_deref()
        ),
        (Some("default"), Some("unrestricted"))
    );
    assert_eq!(
        h.kernel.policy().config().user_default,
        UserToolDefaults::full_access()
    );

    assert!(h
        .kernel
        .switch_permissions(
            AgentMode::Agent,
            UserToolDefaults::full_access(),
            "unrestricted",
            "unrestricted",
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(h.store.get_head_seq(h.agent_id).unwrap(), 1);
}

#[tokio::test]
async fn submitted_mode_change_is_rejected() {
    let h = harness(AgentMode::Plan);
    let forged = Transaction::mode_change(h.agent_id, AgentMode::Plan, AgentMode::Agent);
    assert!(h.kernel.process_direct(forged).await.is_err());
    assert_eq!(h.kernel.policy().mode(), AgentMode::Plan);
    assert_eq!(h.store.get_head_seq(h.agent_id).unwrap(), 0);
}
