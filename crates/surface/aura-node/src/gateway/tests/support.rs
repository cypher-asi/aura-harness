use crate::config::NodeConfig;
use crate::gateway::*;
use aura_agent::KernelModelGateway;
use aura_agent_kernel::{ExecutorRouter, Kernel, KernelConfig};
use aura_context_memory::{
    ConsolidationConfig, MemoryManager, ProcedureConfig, RefinerConfig, RetrievalConfig,
    WriteConfig,
};
use aura_context_skills::{SkillInstallStore, SkillLoader, SkillManager};
use aura_core_types::AgentId;
use aura_engine::scheduler::Scheduler;
use aura_exec_tools::{ToolCatalog, ToolConfig};
use aura_model::MockProvider;
use aura_model::ModelProvider;
use aura_store_db::RocksStore;
use aura_store_db::Store;
use axum::body::Body;
use axum::http::{request::Builder as RequestBuilder, Request};
use std::sync::Arc;

/// Test bearer token injected by [`authed_request`].
///
/// Phase 4 of the security audit taught `require_bearer_mw` to do a
/// constant-time compare against [`NodeConfig::auth_token`], so the
/// value here is no longer cosmetic: it must match the configured
/// secret. [`NodeConfig::default`] ships with the same literal
/// (documented as a test-only placeholder) so every helper in this
/// file keeps working with zero boilerplate. Tests that deliberately
/// want the 401 path (e.g. [`test_requires_bearer_on_protected_routes`])
/// either omit the header or send a non-matching value.
pub(super) const TEST_BEARER: &str = "test";

/// Build a [`Request`] pre-populated with an `Authorization: Bearer test`
/// header so the protected routes let us through.
///
/// Tests that explicitly want to exercise the unauthenticated path
/// (e.g. [`test_requires_bearer_on_protected_routes`]) bypass this and
/// use [`Request::builder`] directly.
pub(super) fn authed_request() -> RequestBuilder {
    Request::builder().header("authorization", format!("Bearer {TEST_BEARER}"))
}

pub(super) fn test_router_state(store: Arc<dyn Store>) -> RouterState {
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("mock"));
    let scheduler = Arc::new(Scheduler::new(
        store.clone(),
        provider.clone(),
        vec![],
        vec![],
        std::path::PathBuf::from("/tmp/workspaces"),
        None,
    ));
    // require_auth = true keeps this router unit-test suite
    // exercising the full bearer-middleware path. NodeConfig::default
    // flipped the gate to false when AURA_NODE_REQUIRE_AUTH was
    // introduced, so we opt in locally - every test in this file that
    // expects a 401 (or sends a matching `Bearer test`) relies on
    // enforcement being active.
    let config = NodeConfig {
        require_auth: true,
        ..NodeConfig::default()
    };
    RouterState::new(crate::gateway::RouterStateConfig {
        store,
        scheduler,
        config,
        provider,
        tool_config: ToolConfig::default(),
        catalog: Arc::new(ToolCatalog::new()),
        domain_api: None,
        automaton_controller: None,
        automaton_bridge: None,
        memory_manager: None,
        skill_manager: None,
        secrets_vault: None,
        process_store: None,
        trigger_registrar: None,
        router_url: None,
    })
}

pub(super) fn create_test_store() -> Arc<dyn Store> {
    let dir = tempfile::tempdir().unwrap();
    Arc::new(RocksStore::open(dir.path(), false).unwrap())
}

pub(super) fn append_tool_call_chain(
    store: &Arc<dyn Store>,
    agent_id: AgentId,
    first_seq: u64,
    tool_use_id: &str,
) -> (
    aura_core_types::Hash,
    aura_core_types::Hash,
    aura_core_types::Hash,
) {
    use aura_core_types::{
        PolicyDecision, PolicyRule, PolicySource, RecordEntry, ToolExecution, ToolGateVerdict,
        ToolProposal, Transaction,
    };

    let proposal = ToolProposal::new(tool_use_id, "read_file", serde_json::json!({"path": "a"}));
    let proposal_tx = Transaction::tool_proposal(agent_id, &proposal).unwrap();
    let decision = PolicyDecision {
        tool_use_id: tool_use_id.into(),
        tool: "read_file".into(),
        verdict: ToolGateVerdict::Approved,
        reason: None,
        decided_by: PolicyRule::new(PolicySource::UserDefault, "full_access"),
        trace: Vec::new(),
        approval: None,
        mode: "agent".into(),
        policy_hash: "h".into(),
        action_id: None,
    };
    let decision_tx = Transaction::policy_decision(agent_id, &decision, proposal_tx.hash).unwrap();
    let execution = ToolExecution {
        tool_use_id: tool_use_id.into(),
        tool: "read_file".into(),
        args: serde_json::json!({"path": "a"}),
        decision: ToolGateVerdict::Approved,
        reason: None,
        result: None,
        is_error: false,
        duration_ms: None,
        parent_agent_id: agent_id,
        originating_user_id: String::new(),
    };
    let execution_tx =
        Transaction::tool_execution_for_decision(agent_id, &execution, decision_tx.hash).unwrap();

    let hashes = (proposal_tx.hash, decision_tx.hash, execution_tx.hash);
    for (offset, tx) in [proposal_tx, decision_tx, execution_tx]
        .into_iter()
        .enumerate()
    {
        let seq = first_seq + offset as u64;
        store
            .append_entry_direct(agent_id, seq, &RecordEntry::builder(seq, tx).build())
            .unwrap();
    }
    hashes
}

// ============================================================================
// Helper: RouterState with real memory + skill managers
// ============================================================================

/// Open a RocksDB store in a leaked temp dir. The concrete handle is kept
/// so the managers can share its DB; callers coerce it to `Arc<dyn Store>`.
fn open_managed_store() -> Arc<RocksStore> {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.keep();
    Arc::new(RocksStore::open(&dir, false).unwrap())
}

/// Memory manager backed by a kernel gateway over `store` and `provider`.
fn test_memory_manager(
    rocks: &RocksStore,
    store: &Arc<dyn Store>,
    provider: &Arc<dyn ModelProvider + Send + Sync>,
) -> Arc<MemoryManager> {
    let memory_kernel = Arc::new(
        Kernel::new(
            store.clone(),
            provider.clone(),
            ExecutorRouter::new(),
            KernelConfig::default(),
            AgentId::generate(),
        )
        .unwrap(),
    );
    let memory_gateway = Arc::new(KernelModelGateway::new(memory_kernel));
    Arc::new(MemoryManager::new(
        rocks.db_handle().clone(),
        memory_gateway,
        RefinerConfig::default(),
        WriteConfig::default(),
        RetrievalConfig::default(),
        ConsolidationConfig::default(),
        ProcedureConfig::default(),
    ))
}

/// Skill manager persisting installs in `rocks`' DB.
fn test_skill_manager(rocks: &RocksStore) -> Arc<std::sync::RwLock<SkillManager>> {
    let skill_store = Arc::new(SkillInstallStore::new(rocks.db_handle().clone()));
    let loader = SkillLoader::with_defaults(None, None);
    Arc::new(std::sync::RwLock::new(SkillManager::with_install_store(
        loader,
        skill_store,
    )))
}

pub(super) fn test_router_state_with_managers() -> RouterState {
    let rocks = open_managed_store();
    let store: Arc<dyn Store> = rocks.clone();

    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("mock"));
    let scheduler = Arc::new(Scheduler::new(
        store.clone(),
        provider.clone(),
        vec![],
        vec![],
        std::path::PathBuf::from("/tmp/workspaces"),
        None,
    ));

    let memory_manager = test_memory_manager(&rocks, &store, &provider);
    let skill_manager = test_skill_manager(&rocks);
    let secrets_vault = Arc::new(aura_store_db::SecretsVault::new(rocks.db_handle().clone()));
    let process_store = Arc::new(aura_store_db::ProcessStore::new(rocks.db_handle().clone()));

    // See note on `test_router_state` - opt in to bearer enforcement
    // because the ambient default is now off but the rest of this
    // suite is built around 401s and matching the test Bearer token.
    let config = NodeConfig {
        require_auth: true,
        ..NodeConfig::default()
    };
    RouterState::new(crate::gateway::RouterStateConfig {
        store,
        scheduler,
        config,
        provider,
        tool_config: ToolConfig::default(),
        catalog: Arc::new(ToolCatalog::new()),
        domain_api: None,
        automaton_controller: None,
        automaton_bridge: None,
        memory_manager: Some(memory_manager),
        skill_manager: Some(skill_manager),
        secrets_vault: Some(secrets_vault),
        process_store: Some(process_store),
        trigger_registrar: None,
        router_url: None,
    })
}

/// Overwrite the default `ConnectInfo<SocketAddr>` that
/// `router::ensure_connect_info` would otherwise inject, so callers
/// can appear to the governor as different peer IPs.
///
/// Used by tests that exercise a bearer-less loop over many routes;
/// without this the synthetic loopback default makes every request
/// share one rate-limit bucket and the loop trips 429 before it's
/// done.
pub(super) fn inject_fake_peer(req: &mut Request<Body>, seed: usize) {
    use axum::extract::ConnectInfo;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    // Stay between 10.0.0.2 and 10.0.0.251 so we never pick .0, .1,
    // or .255. `u8::try_from` here can't actually fail because
    // `seed % 250 + 2 <= 251`, but clippy's `cast_possible_truncation`
    // lint fires on a plain `as u8`.
    let octet = u8::try_from(seed % 250 + 2).expect("octet fits in u8 by construction");
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, octet)), 0);
    req.extensions_mut().insert(ConnectInfo(addr));
}

/// Build a router state whose workspace root is a real temp directory.
///
/// `data_dir/workspaces` is created so `resolve_allowed_path` can
/// canonicalize it. Returns the state and the `TempDir` so the caller
/// can keep it alive for the duration of the test.
pub(super) fn test_router_state_with_workspace() -> (RouterState, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_path_buf();
    std::fs::create_dir_all(data_dir.join("workspaces")).unwrap();

    let db_dir = tempfile::tempdir().unwrap().keep();
    let rocks = RocksStore::open(&db_dir, false).unwrap();
    let db = rocks.db_handle().clone();
    let store: Arc<dyn Store> = Arc::new(rocks);
    let process_store = Arc::new(aura_store_db::ProcessStore::new(db));

    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("mock"));
    let scheduler = Arc::new(Scheduler::new(
        store.clone(),
        provider.clone(),
        vec![],
        vec![],
        data_dir.join("workspaces"),
        None,
    ));
    let config = NodeConfig {
        data_dir,
        ..NodeConfig::default()
    };
    let state = RouterState::new(crate::gateway::RouterStateConfig {
        store,
        scheduler,
        config,
        provider,
        tool_config: ToolConfig::default(),
        catalog: Arc::new(ToolCatalog::new()),
        domain_api: None,
        automaton_controller: None,
        automaton_bridge: None,
        memory_manager: None,
        skill_manager: None,
        secrets_vault: None,
        process_store: Some(process_store),
        trigger_registrar: None,
        router_url: None,
    });
    (state, tmp)
}

/// Tiny percent-encoder used only by the file-handler tests.
///
/// We can't drag in a full URL crate just for this: `reqwest::Url`
/// isn't available in this test binary and `percent_encoding` isn't a
/// workspace dep. So we hand-encode the bytes that matter for URL
/// paths (space, `?`, `#`, `&`, `%`, `+`, non-ASCII) and leave the
/// alphanumerics / safe punctuation alone.
pub(super) fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let safe = b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/' | b':' | b'\\');
        if safe {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}
