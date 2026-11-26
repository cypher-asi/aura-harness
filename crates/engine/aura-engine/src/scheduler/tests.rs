
use super::*;
use aura_core_types::{Transaction, TransactionType};
use aura_model::MockProvider;
use aura_store_db::RocksStore;
use bytes::Bytes;
use std::time::Duration;

/// Whether the agent's store-backed processing claim is present.
fn is_agent_busy(scheduler: &Scheduler, agent_id: AgentId) -> bool {
    scheduler.store.is_agent_processing(agent_id).unwrap()
}

/// Build a [`AgentIdentity`] suitable for unit tests.
///
/// Pinned to `claude-opus-4-7` so any silent fallback to the
/// pre-fix `claude-opus-4-6` (or an empty model) shows up
/// immediately when callers wire the registry incorrectly.
pub(crate) fn test_identity(model: &str) -> AgentIdentity {
    AgentIdentity {
        reasoning_effort: None,
        upstream_provider_family: None,
        model: model.to_string(),
        aura_org_id: Some("org-test".to_string()),
        aura_session_id: Some("session-test".to_string()),
        aura_agent_id: Some("agent-test".to_string()),
        aura_project_id: Some("project-test".to_string()),
        system_prompt: String::new(),
        prompt_cache_key: None,
        prompt_cache_retention: None,
        request_kind: ModelRequestKind::Chat,
        max_context_tokens: 200_000,
        auth_token: None,
        cache_salt: None,
        run_scratch: None,
    }
}

fn create_test_scheduler() -> (Scheduler, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(dir.path().join("db"), false).unwrap());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("test"));
    let ws_dir = dir.path().join("workspaces");
    std::fs::create_dir_all(&ws_dir).unwrap();
    let scheduler = Scheduler::new(store, provider, vec![], vec![], ws_dir, None);
    (scheduler, dir)
}

fn create_test_scheduler_with_provider(
    provider: Arc<dyn ModelProvider + Send + Sync>,
) -> (Scheduler, Arc<dyn Store>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(dir.path().join("db"), false).unwrap());
    let ws_dir = dir.path().join("workspaces");
    std::fs::create_dir_all(&ws_dir).unwrap();
    let scheduler = Scheduler::new(store.clone(), provider, vec![], vec![], ws_dir, None);
    (scheduler, store, dir)
}

fn enqueue_prompt(store: &Arc<dyn Store>, agent_id: AgentId, prompt: &str) {
    let tx = Transaction::new_chained(
        agent_id,
        TransactionType::UserPrompt,
        aura_core_types::PromptPayload::new(prompt).to_bytes(),
        None,
    );
    store.enqueue_tx(&tx).unwrap();
}

#[test]
fn test_scheduler_creation() {
    let (_scheduler, _dir) = create_test_scheduler();
}

#[tokio::test]
async fn test_schedule_agent_no_pending() {
    let (scheduler, _dir) = create_test_scheduler();
    let agent_id = AgentId::generate();
    let result = scheduler.schedule_agent(agent_id).await.unwrap();
    assert_eq!(result, 0, "No pending txs should process 0");
}

#[tokio::test]
async fn test_schedule_paused_agent_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(dir.path().join("db"), false).unwrap());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("test"));
    let ws_dir = dir.path().join("workspaces");
    std::fs::create_dir_all(&ws_dir).unwrap();

    let agent_id = AgentId::generate();
    store
        .set_agent_status(agent_id, AgentStatus::Paused)
        .unwrap();

    let scheduler = Scheduler::new(store, provider, vec![], vec![], ws_dir, None);
    let result = scheduler.schedule_agent(agent_id).await.unwrap();
    assert_eq!(result, 0, "Paused agents should be skipped");
}

#[tokio::test]
async fn test_schedule_dead_agent_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(dir.path().join("db"), false).unwrap());
    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(MockProvider::simple_response("test"));
    let ws_dir = dir.path().join("workspaces");
    std::fs::create_dir_all(&ws_dir).unwrap();

    let agent_id = AgentId::generate();
    store.set_agent_status(agent_id, AgentStatus::Dead).unwrap();

    let scheduler = Scheduler::new(store, provider, vec![], vec![], ws_dir, None);
    let result = scheduler.schedule_agent(agent_id).await.unwrap();
    assert_eq!(result, 0, "Dead agents should be skipped");
}

#[test]
fn test_is_agent_busy_false_by_default() {
    let (scheduler, _dir) = create_test_scheduler();
    let agent_id = AgentId::generate();
    assert!(!is_agent_busy(&scheduler, agent_id));
}

#[test]
fn test_build_executor_router() {
    let (scheduler, _dir) = create_test_scheduler();
    let _router = scheduler.build_executor_router();
}

#[tokio::test]
async fn test_concurrent_scheduler_calls_only_one_processes_pending_agent() {
    let provider = Arc::new(MockProvider::simple_response("processed").with_latency(150));
    let provider_for_assert = provider.clone();
    let (scheduler, store, _dir) = create_test_scheduler_with_provider(provider);
    let scheduler = Arc::new(scheduler);
    let agent_id = AgentId::generate();
    scheduler
        .identity_registry()
        .register(agent_id, test_identity("claude-opus-4-7"));
    enqueue_prompt(&store, agent_id, "hello");

    let first = {
        let scheduler = scheduler.clone();
        tokio::spawn(async move { scheduler.schedule_agent(agent_id).await })
    };

    tokio::time::timeout(Duration::from_secs(1), async {
        while !is_agent_busy(&scheduler, agent_id) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("first scheduler call should acquire claim");

    let second = scheduler.schedule_agent(agent_id).await.unwrap();
    let first = first.await.unwrap().unwrap();

    assert_eq!(first + second, 1);
    assert!(
        [0, 1].contains(&first) && [0, 1].contains(&second),
        "one caller should process and one should skip"
    );
    assert_eq!(provider_for_assert.call_count(), 1);
    assert!(!is_agent_busy(&scheduler, agent_id));
}

#[tokio::test]
async fn test_busy_and_status_checks_do_not_block_while_processing() {
    let provider = Arc::new(MockProvider::simple_response("processed").with_latency(200));
    let (scheduler, store, _dir) = create_test_scheduler_with_provider(provider);
    let scheduler = Arc::new(scheduler);
    let agent_id = AgentId::generate();
    scheduler
        .identity_registry()
        .register(agent_id, test_identity("claude-opus-4-7"));
    enqueue_prompt(&store, agent_id, "hello");

    let handle = {
        let scheduler = scheduler.clone();
        tokio::spawn(async move { scheduler.schedule_agent(agent_id).await })
    };

    tokio::time::timeout(Duration::from_secs(1), async {
        while !is_agent_busy(&scheduler, agent_id) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("scheduler should expose busy claim");

    let busy = tokio::time::timeout(Duration::from_millis(50), async {
        is_agent_busy(&scheduler, agent_id)
    })
    .await
    .expect("busy check should not wait for processing");
    let status = tokio::time::timeout(Duration::from_millis(50), async {
        store.get_agent_status(agent_id)
    })
    .await
    .expect("status check should not wait for processing")
    .unwrap();

    assert!(busy);
    assert_eq!(status, AgentStatus::Active);
    handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn test_processing_claim_released_after_error() {
    let provider = Arc::new(MockProvider::simple_response("unused"));
    let (scheduler, store, _dir) = create_test_scheduler_with_provider(provider);
    let agent_id = AgentId::generate();
    scheduler
        .identity_registry()
        .register(agent_id, test_identity("claude-opus-4-7"));
    let tx = Transaction::new_chained(
        agent_id,
        TransactionType::UserPrompt,
        Bytes::from_static(&[0xff]),
        None,
    );
    store.enqueue_tx(&tx).unwrap();

    let result = scheduler.schedule_agent(agent_id).await;

    assert!(result.is_err());
    assert!(!is_agent_busy(&scheduler, agent_id));
    assert!(
        scheduler.try_processing_claim(agent_id).unwrap().is_some(),
        "claim should be reusable after an error"
    );
}

#[test]
fn test_scheduler_kernel_sequence_claim_invariant_documented() {
    let (scheduler, _dir) = create_test_scheduler();
    let agent_id = AgentId::generate();
    let mut claim = scheduler
        .try_processing_claim(agent_id)
        .unwrap()
        .expect("first claim should succeed");

    assert!(
        scheduler.try_processing_claim(agent_id).unwrap().is_none(),
        "scheduler must construct at most one Kernel per agent claim"
    );
    claim.release().unwrap();
}

/// Answers like [`MockProvider`], keeping the cache salt of each
/// request it was sent.
#[derive(Default)]
struct SaltCapture {
    salts: std::sync::Mutex<Vec<Option<String>>>,
}

#[async_trait::async_trait]
impl ModelProvider for SaltCapture {
    fn name(&self) -> &'static str {
        "salt-capture"
    }

    async fn complete(
        &self,
        request: aura_model::ModelRequest,
    ) -> Result<aura_model::ModelResponse, aura_model::ReasonerError> {
        self.salts.lock().unwrap().push(request.cache_salt.clone());
        MockProvider::simple_response("done")
            .complete(request)
            .await
    }

    async fn health_check(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn scheduled_kernels_carry_the_registered_cache_salt() {
    let provider = Arc::new(SaltCapture::default());
    let (scheduler, store, _dir) = create_test_scheduler_with_provider(provider.clone());
    let (salted, plain) = (AgentId::generate(), AgentId::generate());
    let identity = AgentIdentity {
        cache_salt: Some("5a17".to_string()),
        ..test_identity("claude-opus-4-7")
    };
    scheduler.identity_registry().register(salted, identity);
    scheduler
        .identity_registry()
        .register(plain, test_identity("claude-opus-4-7"));
    let explicit = test_identity("claude-opus-4-7").into_loop_config();
    enqueue_prompt(&store, salted, "hello");
    scheduler
        .schedule_agent_with_overrides(salted, Some(explicit), None)
        .await
        .unwrap();
    let salts = std::mem::take(&mut *provider.salts.lock().unwrap());
    assert!(!salts.is_empty());
    assert!(
        salts.iter().all(|s| s.as_deref() == Some("5a17")),
        "{salts:?}"
    );

    enqueue_prompt(&store, plain, "hello");
    scheduler.schedule_agent(plain).await.unwrap();
    let salts = provider.salts.lock().unwrap().clone();
    assert!(!salts.is_empty());
    assert!(salts.iter().all(Option::is_none), "{salts:?}");
}
