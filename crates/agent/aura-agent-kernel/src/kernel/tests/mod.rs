//! Integration tests for the `Kernel` surface.
//!
//! These tests cover cross-cutting behavior (sequence monotonicity across
//! process + reason, policy / runtime-capability interaction, session
//! boundary clearing) so they live alongside `Kernel` itself rather than
//! being split per-file.

use super::*;
use crate::ExecutorRouter;
use aura_core_types::{
    ActionKind, AgentId, InstalledIntegrationDefinition, InstalledToolCapability,
    InstalledToolIntegrationRequirement, RuntimeCapabilityInstall, SystemKind, ToolProposal,
    Transaction, TransactionType,
};

use aura_model::{MockProvider, ModelProvider, ModelRequest};

use aura_store_db::{RocksStore, Store};

use std::collections::HashMap;

use std::sync::Arc;

use tempfile::TempDir;

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

/// A kernel over its own store, with the store and temp dirs kept alive.
struct KernelFixture {
    kernel: Kernel,
    store: Arc<dyn Store>,
    agent_id: AgentId,
    _db_dir: TempDir,
    _ws_dir: TempDir,
}

fn kernel_with_policy(policy: crate::policy::PolicyConfig) -> KernelFixture {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("test response"));
    let config = KernelConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        policy,
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
    KernelFixture {
        kernel,
        store,
        agent_id,
        _db_dir: db_dir,
        _ws_dir: ws_dir,
    }
}

/// Two kernels for the same agent sharing one store, so one of them always
/// holds a stale in-memory sequence.
struct TwoKernels {
    kernel_a: Kernel,
    kernel_b: Kernel,
    store: Arc<dyn Store>,
    agent_id: AgentId,
    _db_dir: TempDir,
    _ws_dir: TempDir,
}

fn two_kernels_on_one_store() -> TwoKernels {
    let db_dir = TempDir::new().unwrap();
    let ws_dir = TempDir::new().unwrap();
    let agent_id = AgentId::generate();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(db_dir.path(), false).unwrap());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("test response"));
    let config = KernelConfig {
        workspace_base: ws_dir.path().to_path_buf(),
        ..KernelConfig::default()
    };
    let kernel_a = Kernel::new(
        store.clone(),
        provider.clone(),
        ExecutorRouter::new(),
        config.clone(),
        agent_id,
    )
    .unwrap();
    let kernel_b = Kernel::new(
        store.clone(),
        provider,
        ExecutorRouter::new(),
        config,
        agent_id,
    )
    .unwrap();
    TwoKernels {
        kernel_a,
        kernel_b,
        store,
        agent_id,
        _db_dir: db_dir,
        _ws_dir: ws_dir,
    }
}

fn request_with_effort(effort: Option<aura_model::ThinkingEffort>) -> ModelRequest {
    ModelRequest::builder("test-model", "system")
        .message(aura_model::Message::user("hello"))
        .thinking_effort(effort)
        .try_build()
        .unwrap()
}

fn reasoning_payload(entry: &aura_core_types::RecordEntry) -> serde_json::Value {
    assert_eq!(entry.tx.tx_type, TransactionType::Reasoning);
    serde_json::from_slice(&entry.tx.payload).unwrap()
}

mod policy;
mod reasoning;
mod sequence;
