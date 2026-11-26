//! Tests for the automaton bridge.
//!
//! Two clusters live here:
//!
//! 1. Lifecycle/inbox tests that exercise
//!    [`AutomatonBridge::record_lifecycle_event`] and
//!    [`AutomatonBridge::prepare_installed_tools`] (Invariants §2, §8
//!    + the brave-search regression).
//! 2. Event-stream replay tests (`late_subscriber_*`,
//!    `mid_stream_subscriber_*`, `history_caps_*`) that drive
//!    `spawn_event_forwarder` directly via its `mpsc` channel and
//!    assert the late-WS-subscriber race no longer drops the terminal
//!    event. Those tests reach back into the channel-side constants
//!    via `super::event_channel::EVENT_HISTORY_CAPACITY`.

use super::event_channel::EVENT_HISTORY_CAPACITY;
use super::AutomatonBridge;
use crate::scheduler::Scheduler;
use async_trait::async_trait;
use aura_core_types::{AgentId, InstalledIntegrationDefinition, TransactionType};
use aura_engine_automaton::AutomatonRuntime;
use aura_exec_tools::{
    domain_tools::{
        CreateSessionParams, DomainApi, MessageDescriptor, ProjectDescriptor, ProjectUpdate,
        SaveMessageParams, SessionDescriptor, SpecDescriptor, TaskDescriptor, TaskUpdate,
    },
    ToolCatalog, ToolConfig,
};

use aura_model::{MockProvider, ModelProvider};

use aura_store_db::{RocksStore, Store};

use std::sync::Arc;

/// A `DomainApi` stub whose methods all panic — the lifecycle test
/// below never invokes any of them because it only exercises the
/// bridge's inbox/scheduler wiring, not the automaton runtime
/// itself.
struct UnusedDomain;

#[async_trait]
impl DomainApi for UnusedDomain {
    async fn list_specs(
        &self,
        _project_id: &str,
        _jwt: Option<&str>,
    ) -> anyhow::Result<Vec<SpecDescriptor>> {
        unimplemented!("UnusedDomain")
    }
    async fn get_spec(&self, _spec_id: &str, _jwt: Option<&str>) -> anyhow::Result<SpecDescriptor> {
        unimplemented!("UnusedDomain")
    }
    async fn create_spec(
        &self,
        _p: &str,
        _t: &str,
        _c: &str,
        _o: u32,
        _j: Option<&str>,
    ) -> anyhow::Result<SpecDescriptor> {
        unimplemented!("UnusedDomain")
    }
    async fn update_spec(
        &self,
        _id: &str,
        _t: Option<&str>,
        _c: Option<&str>,
        _if_match: Option<&str>,
        _j: Option<&str>,
    ) -> anyhow::Result<SpecDescriptor> {
        unimplemented!("UnusedDomain")
    }
    async fn delete_spec(&self, _id: &str, _j: Option<&str>) -> anyhow::Result<()> {
        unimplemented!("UnusedDomain")
    }
    async fn list_tasks(
        &self,
        _p: &str,
        _s: Option<&str>,
        _j: Option<&str>,
    ) -> anyhow::Result<Vec<TaskDescriptor>> {
        unimplemented!("UnusedDomain")
    }
    async fn create_task(
        &self,
        _p: &str,
        _s: &str,
        _t: &str,
        _d: &str,
        _deps: &[String],
        _o: u32,
        _j: Option<&str>,
    ) -> anyhow::Result<TaskDescriptor> {
        unimplemented!("UnusedDomain")
    }
    async fn update_task(
        &self,
        _id: &str,
        _u: TaskUpdate,
        _j: Option<&str>,
    ) -> anyhow::Result<TaskDescriptor> {
        unimplemented!("UnusedDomain")
    }
    async fn delete_task(&self, _id: &str, _j: Option<&str>) -> anyhow::Result<()> {
        unimplemented!("UnusedDomain")
    }
    async fn transition_task(
        &self,
        _id: &str,
        _s: &str,
        _j: Option<&str>,
    ) -> anyhow::Result<TaskDescriptor> {
        unimplemented!("UnusedDomain")
    }
    async fn claim_next_task(
        &self,
        _p: &str,
        _a: &str,
        _j: Option<&str>,
    ) -> anyhow::Result<Option<TaskDescriptor>> {
        unimplemented!("UnusedDomain")
    }
    async fn get_task(&self, _id: &str, _j: Option<&str>) -> anyhow::Result<TaskDescriptor> {
        unimplemented!("UnusedDomain")
    }
    async fn get_project(&self, _p: &str, _j: Option<&str>) -> anyhow::Result<ProjectDescriptor> {
        unimplemented!("UnusedDomain")
    }
    async fn update_project(
        &self,
        _p: &str,
        _u: ProjectUpdate,
        _j: Option<&str>,
    ) -> anyhow::Result<ProjectDescriptor> {
        unimplemented!("UnusedDomain")
    }
    async fn create_log(
        &self,
        _p: &str,
        _m: &str,
        _l: &str,
        _a: Option<&str>,
        _md: Option<&serde_json::Value>,
        _j: Option<&str>,
    ) -> anyhow::Result<serde_json::Value> {
        unimplemented!("UnusedDomain")
    }
    async fn list_logs(
        &self,
        _p: &str,
        _l: Option<&str>,
        _n: Option<u64>,
        _j: Option<&str>,
    ) -> anyhow::Result<serde_json::Value> {
        unimplemented!("UnusedDomain")
    }
    async fn get_project_stats(
        &self,
        _p: &str,
        _j: Option<&str>,
    ) -> anyhow::Result<serde_json::Value> {
        unimplemented!("UnusedDomain")
    }
    async fn list_messages(&self, _p: &str, _i: &str) -> anyhow::Result<Vec<MessageDescriptor>> {
        unimplemented!("UnusedDomain")
    }
    async fn save_message(&self, _p: SaveMessageParams) -> anyhow::Result<()> {
        unimplemented!("UnusedDomain")
    }
    async fn create_session(&self, _p: CreateSessionParams) -> anyhow::Result<SessionDescriptor> {
        unimplemented!("UnusedDomain")
    }
    async fn get_active_session(&self, _i: &str) -> anyhow::Result<Option<SessionDescriptor>> {
        unimplemented!("UnusedDomain")
    }
    async fn network_api_call(
        &self,
        _m: &str,
        _p: &str,
        _b: Option<&serde_json::Value>,
        _j: Option<&str>,
    ) -> anyhow::Result<String> {
        unimplemented!("UnusedDomain")
    }
}

fn count_lifecycle_entries(store: &Arc<dyn Store>, agent_id: AgentId) -> usize {
    store
        .scan_record(agent_id, 0, 256)
        .expect("scan_record")
        .into_iter()
        .filter(|e| e.tx.tx_type == TransactionType::System)
        .filter(|e| {
            serde_json::from_slice::<serde_json::Value>(&e.tx.payload)
                .ok()
                .and_then(|v| {
                    v.get("system_kind")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
                .as_deref()
                == Some("automaton_lifecycle")
        })
        .count()
}

/// Read the `event` field off every `System::AutomatonLifecycle`
/// payload in `agent_id`'s record log, in chronological order. Used
/// by the lifecycle ordering tests below to assert the exact
/// `start_dev_loop` / `pause_dev_loop` / `stop_dev_loop` sequence
/// reaches the audit trail in the order the operator triggered it.
fn lifecycle_events_in_order(store: &Arc<dyn Store>, agent_id: AgentId) -> Vec<String> {
    store
        .scan_record(agent_id, 0, 256)
        .expect("scan_record")
        .into_iter()
        .filter(|e| e.tx.tx_type == TransactionType::System)
        .filter_map(|e| {
            let value: serde_json::Value = serde_json::from_slice(&e.tx.payload).ok()?;
            if value.get("system_kind").and_then(serde_json::Value::as_str)
                != Some("automaton_lifecycle")
            {
                return None;
            }
            value
                .get("event")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

// ------------------------------------------------------------------
// Event-stream replay tests
//
// Regression tests for the race described on [`EventChannel`]:
// `aura-os-server` connects to `/stream/automaton/:id` *after*
// `POST /automaton/start` returns. `tokio::sync::broadcast`
// receivers only observe events sent after they subscribe, so a
// fast-terminating automaton used to look like "stream closed
// without a terminal event" from the server's point of view.
//
// These tests drive `spawn_event_forwarder` directly via the mpsc
// it consumes, then exercise `subscribe_events` as a late
// subscriber.
// ------------------------------------------------------------------

fn test_bridge() -> AutomatonBridge {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(dir.path().join("db"), false).unwrap());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("noop"));
    let ws_dir = dir.path().join("workspaces");
    std::fs::create_dir_all(&ws_dir).unwrap();
    let scheduler = Arc::new(Scheduler::new(
        store.clone(),
        provider.clone(),
        vec![],
        vec![],
        ws_dir,
        None,
    ));
    let runtime = Arc::new(AutomatonRuntime::new());
    let catalog = Arc::new(ToolCatalog::new());
    let domain: Arc<dyn DomainApi> = Arc::new(UnusedDomain);
    AutomatonBridge::new(
        runtime,
        store,
        domain,
        provider,
        catalog,
        ToolConfig::default(),
    )
    .with_scheduler(scheduler)
}

mod history;
mod lifecycle;
mod tools;
