use super::support::*;
use crate::config::NodeConfig;
use crate::gateway::*;
use aura_engine::scheduler::Scheduler;
use aura_exec_tools::{ToolCatalog, ToolConfig};
use aura_model::MockProvider;
use aura_model::ModelProvider;
use aura_store_db::RocksStore;
use aura_store_db::Store;
use axum::body::Body;
use axum::http::Request;
use axum::http::StatusCode;
use std::sync::Arc;
use tower::util::ServiceExt;

// ============================================================================
// Auth middleware integration tests (security audit — phase 1)
// ============================================================================
//
// These tests exercise the router-wide `require_bearer_mw` middleware by
// issuing unauthenticated requests against every non-`/health` route and
// asserting that the response is `401 UNAUTHORIZED`. They also confirm
// that `/health` stays reachable without a token (liveness / readiness
// probes run anonymously).
//
// The matrix is hand-maintained rather than auto-discovered from the
// router because axum does not expose route introspection. When adding
// a new route to `create_router`, add a matching entry here.

/// Every protected route in the router, expressed as `(method, uri)`
/// pairs. Concrete path params (`:agent_id`, `:automaton_id`, `:name`,
/// `:id`, `:key`, `:tx_id`) are substituted with values that would
/// normally reach the handler body; because the middleware rejects the
/// request before any extractor runs, the specific values don't matter.
const PROTECTED_ROUTES: &[(&str, &str)] = &[
    ("GET", "/api/files"),
    ("GET", "/api/read-file"),
    ("PUT", "/api/write-file"),
    ("GET", "/workspace/resolve"),
    ("POST", "/workspace/import"),
    ("DELETE", "/workspace/deadbeef"),
    ("GET", "/workspace/deadbeef/safe/session-1"),
    ("POST", "/workspace/deadbeef/safe/session-1"),
    (
        "GET",
        "/workspace/deadbeef/safe/session-1/checkpoints/abcdef/diff",
    ),
    (
        "POST",
        "/workspace/deadbeef/safe/session-1/checkpoints/abcdef/restore",
    ),
    ("POST", "/workspace/deadbeef/safe/session-1/apply"),
    ("POST", "/tx"),
    ("GET", "/tx/status/deadbeef/abcd"),
    ("GET", "/agents/deadbeef/head"),
    ("GET", "/agents/deadbeef/record"),
    ("GET", "/agents/deadbeef/tool_calls/tu-1/audit"),
    ("GET", "/ws/terminal"),
    ("GET", "/ws/preview/tcp/5173"),
    ("GET", "/stream/test-run"),
    ("POST", "/v1/run"),
    ("GET", "/v1/run/list"),
    ("GET", "/v1/run/test-run/status"),
    ("POST", "/v1/run/test-run/pause"),
    ("POST", "/v1/run/test-run/stop"),
    // Memory canonical
    ("GET", "/memory/deadbeef/facts"),
    ("POST", "/memory/deadbeef/facts"),
    ("GET", "/memory/deadbeef/facts/some-id"),
    ("PUT", "/memory/deadbeef/facts/some-id"),
    ("DELETE", "/memory/deadbeef/facts/some-id"),
    ("GET", "/memory/deadbeef/facts/by-key/some-key"),
    ("GET", "/memory/deadbeef/events"),
    ("POST", "/memory/deadbeef/events"),
    ("DELETE", "/memory/deadbeef/events/some-id"),
    ("POST", "/memory/deadbeef/events/bulk-delete"),
    ("GET", "/memory/deadbeef/procedures"),
    ("POST", "/memory/deadbeef/procedures"),
    ("GET", "/memory/deadbeef/procedures/some-id"),
    ("PUT", "/memory/deadbeef/procedures/some-id"),
    ("DELETE", "/memory/deadbeef/procedures/some-id"),
    ("GET", "/memory/deadbeef/snapshot"),
    ("POST", "/memory/deadbeef/wipe"),
    ("GET", "/memory/deadbeef/stats"),
    ("GET", "/memory/deadbeef/continuity"),
    ("PUT", "/memory/deadbeef/continuity"),
    ("GET", "/memory/deadbeef/retrieval/latest"),
    ("POST", "/memory/deadbeef/consolidate"),
    // Memory aliases
    ("GET", "/api/agents/deadbeef/memory"),
    ("DELETE", "/api/agents/deadbeef/memory"),
    ("GET", "/api/agents/deadbeef/memory/facts"),
    ("POST", "/api/agents/deadbeef/memory/facts"),
    ("GET", "/api/agents/deadbeef/memory/facts/some-id"),
    ("PUT", "/api/agents/deadbeef/memory/facts/some-id"),
    ("DELETE", "/api/agents/deadbeef/memory/facts/some-id"),
    ("GET", "/api/agents/deadbeef/memory/facts/by-key/some-key"),
    ("GET", "/api/agents/deadbeef/memory/events"),
    ("POST", "/api/agents/deadbeef/memory/events"),
    ("DELETE", "/api/agents/deadbeef/memory/events/some-id"),
    ("POST", "/api/agents/deadbeef/memory/events/bulk-delete"),
    ("GET", "/api/agents/deadbeef/memory/procedures"),
    ("POST", "/api/agents/deadbeef/memory/procedures"),
    ("GET", "/api/agents/deadbeef/memory/procedures/some-id"),
    ("PUT", "/api/agents/deadbeef/memory/procedures/some-id"),
    ("DELETE", "/api/agents/deadbeef/memory/procedures/some-id"),
    ("GET", "/api/agents/deadbeef/memory/stats"),
    ("GET", "/api/agents/deadbeef/memory/continuity"),
    ("PUT", "/api/agents/deadbeef/memory/continuity"),
    ("GET", "/api/agents/deadbeef/memory/retrieval/latest"),
    ("POST", "/api/agents/deadbeef/memory/consolidate"),
    // Skills
    ("GET", "/api/skills"),
    ("POST", "/api/skills"),
    ("GET", "/api/skills/some-skill"),
    ("POST", "/api/skills/some-skill/activate"),
    ("GET", "/api/agents/deadbeef/skills"),
    ("POST", "/api/agents/deadbeef/skills"),
    ("DELETE", "/api/agents/deadbeef/skills/some-skill"),
    // Legacy harness aliases
    ("GET", "/api/harness/agents/deadbeef/skills"),
    ("POST", "/api/harness/agents/deadbeef/skills"),
    ("DELETE", "/api/harness/agents/deadbeef/skills/some-skill"),
];

#[tokio::test]
async fn test_requires_bearer_on_protected_routes() {
    let state = test_router_state_with_managers();
    let app = create_router(state);

    for (idx, (method, uri)) in PROTECTED_ROUTES.iter().enumerate() {
        // NOTE: deliberately does not go through `authed_request()`.
        // We want the *unauthenticated* code path.
        //
        // Each iteration gets a distinct synthetic peer address so the
        // phase-9 per-IP governor doesn't start returning 429 partway
        // through the matrix — we're testing the auth middleware, not
        // the rate limiter.
        let mut req = Request::builder()
            .method(*method)
            .uri(*uri)
            .body(Body::empty())
            .unwrap();
        inject_fake_peer(&mut req, idx);

        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "expected 401 for {method} {uri}, got {}",
            resp.status()
        );
    }
}

#[tokio::test]
async fn test_rejects_malformed_bearer_header() {
    let state = test_router_state_with_managers();
    let app = create_router(state);

    // Wrong scheme — `Basic` instead of `Bearer`. The value after the
    // scheme is an arbitrary non-credential placeholder; the assertion
    // below is purely about the scheme, and a base64-shaped literal
    // here tripped GitHub secret scanning for no defensive benefit.
    let req = Request::builder()
        .uri("/api/skills")
        .header("authorization", "Basic placeholder")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Correct scheme but empty token.
    let req = Request::builder()
        .uri("/api/skills")
        .header("authorization", "Bearer   ")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Phase 4 (security audit): a syntactically-valid Bearer that does
/// *not* match the configured secret must be rejected. This is the
/// regression test for the pre-phase-4 behaviour where `Bearer x`
/// was enough to reach any protected handler.
#[tokio::test]
async fn test_rejects_non_matching_bearer() {
    let state = test_router_state_with_managers();
    let app = create_router(state);

    let req = Request::builder()
        .uri("/api/skills")
        .header("authorization", "Bearer not-the-configured-secret")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Swarm TEE R2 integration fix: when `AURA_SWARM_INTERNAL_TOKEN` is
/// configured (the scheduler injects it into confidential pods), the
/// gateway accepts it as a valid bearer on protected routes — that is
/// the token the swarm cron service presents on
/// `POST /v1/processes/:id/trigger`. The per-node token keeps working,
/// and arbitrary tokens are still rejected.
#[tokio::test]
async fn test_accepts_swarm_internal_token_on_protected_routes() {
    let mut state = test_router_state_with_managers();
    state.config.swarm_internal_token = Some("platform-internal-secret".to_string());
    let app = create_router(state);

    // The swarm internal token authenticates.
    let req = Request::builder()
        .uri("/api/skills")
        .header("authorization", "Bearer platform-internal-secret")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The per-node token still authenticates alongside it.
    let req = authed_request()
        .uri("/api/skills")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // A non-matching token is still rejected.
    let req = Request::builder()
        .uri("/api/skills")
        .header("authorization", "Bearer some-other-token")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// A server whose `auth_token` is empty (misconfiguration) must not
/// accept *any* request — otherwise attackers who guess that the
/// server "never loaded a secret" could send `Bearer ""` and win.
#[tokio::test]
async fn test_rejects_when_server_auth_token_empty() {
    let dir = tempfile::tempdir().unwrap().keep();
    let rocks = RocksStore::open(&dir, false).unwrap();
    let store: Arc<dyn Store> = Arc::new(rocks);

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
    let config = NodeConfig {
        auth_token: String::new(),
        require_auth: true,
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
        process_store: None,
        trigger_registrar: None,
        router_url: None,
    });
    let app = create_router(state);

    // Even with the "correct" (empty) token the server must refuse.
    let req = Request::builder()
        .uri("/api/skills")
        .header("authorization", "Bearer anything")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // And of course with an empty presented token too.
    let req = Request::builder()
        .uri("/api/skills")
        .header("authorization", "Bearer ")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_health_remains_anonymous() {
    let store = create_test_store();
    let state = test_router_state(store);
    let app = create_router(state);

    let req = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "/health must remain reachable without a bearer token (liveness probe)"
    );
}
