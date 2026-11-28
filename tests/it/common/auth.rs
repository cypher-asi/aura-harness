//! Credential, bearer and HTTP-client helpers.

use std::time::Duration;

use aura_auth::CredentialStore;
use serde_json::{json, Value};

/// Default full-access `agent_permissions` payload for
/// [`RuntimeRequest`] bodies. Matches the wire shape of
/// `aura_core_protocol::AgentPermissionsWire`.
pub(crate) fn default_agent_permissions_payload() -> Value {
    json!({
        "scope": { "orgs": [], "projects": [], "agent_ids": [] },
        "capabilities": [
            { "type": "spawnAgent" },
            { "type": "controlAgent" },
            { "type": "readAgent" },
            { "type": "listAgents" },
            { "type": "manageOrgMembers" },
            { "type": "manageBilling" },
            { "type": "invokeProcess" },
            { "type": "postToFeed" },
            { "type": "generateMedia" },
            { "type": "readAllProjects" },
            { "type": "writeAllProjects" }
        ]
    })
}

// ============================================================================
// Credential helpers
// ============================================================================

/// Resolve auth token for LLM tests. Panics when credentials are missing.
pub fn require_llm_token() -> String {
    load_auth_token().unwrap_or_else(|| {
        panic!(
            "LLM credentials required: no auth token available. \
             Set AURA_ROUTER_JWT or run `aura login`."
        )
    })
}

/// Return (email, password) from E2E env vars when configured.
pub fn optional_zos_credentials() -> Option<(String, String)> {
    let email = std::env::var("E2E_ZOS_EMAIL").unwrap_or_default();
    let password = std::env::var("E2E_ZOS_PASSWORD").unwrap_or_default();
    if email.is_empty() || password.is_empty() {
        return None;
    }
    Some((email, password))
}

/// Perform a real ZOS login and return the JWT access token.
pub async fn zos_login(email: &str, password: &str) -> Result<String, String> {
    let client = aura_auth::ZosClient::new().map_err(|e| format!("ZosClient::new failed: {e}"))?;
    let session = client
        .login(email, password)
        .await
        .map_err(|e| format!("ZOS login failed: {e}"))?;
    Ok(session.access_token)
}

// ============================================================================
// Auth helpers
// ============================================================================

/// Load an auth token from env or credential store.
pub fn load_auth_token() -> Option<String> {
    if let Ok(jwt) = std::env::var("AURA_ROUTER_JWT") {
        if !jwt.is_empty() {
            return Some(jwt);
        }
    }
    CredentialStore::load_token()
}

/// Bearer token used by the integration tests.
///
/// The router-wide `require_bearer_mw` middleware (security audit —
/// phase 1) rejects any non-`/health` request that doesn't carry a
/// non-empty Bearer token. These tests don't exercise the value of
/// the token (phase 4 will add a real shared secret), so we just need
/// a well-formed header — this constant is shared by `http_client`
/// and `WsClient::connect` to keep every caller honest.
pub const E2E_TEST_BEARER: &str = "test";

/// Create a reqwest HTTP client with the default test Bearer header.
///
/// Anything that hits `/tx`, `/agents/...`, etc. now requires auth.
/// Baking the header into the client keeps individual test functions
/// from sprinkling `.bearer_auth(...)` calls everywhere.
pub fn http_client() -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        reqwest::header::HeaderValue::from_str(&format!("Bearer {E2E_TEST_BEARER}")).unwrap(),
    );
    reqwest::Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap()
}
