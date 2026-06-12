//! Secrets vault endpoints (Swarm TEE upgrade phase 6).
//!
//! CRUD over the in-TEE [`aura_store_db::SecretsVault`]:
//!
//! * `GET    /secrets`        — names + metadata only, never values.
//! * `GET    /secrets/:name`  — metadata by default; `?reveal=true`
//!   includes the value (the in-VM read path for agent tooling).
//! * `PUT    /secrets/:name`  — `{ "value": "...", "description": "..." }`
//!   create/update.
//! * `DELETE /secrets/:name`.
//!
//! All four routes sit on the protected sub-router, behind the same
//! bearer middleware as every other gateway endpoint the swarm control
//! plane proxies to.
//!
//! Redaction: the request body type has no `Debug` derive and handlers
//! never log or trace bodies; non-reveal responses are built from
//! [`aura_store_db::SecretMetadata`], which cannot carry a value.

use super::super::*;
use aura_store_db::{SecretMetadata, VaultError};

/// Map a [`VaultError`] to an HTTP status + JSON failure body, keeping
/// the policy in one place for all four handlers.
fn vault_error_response(err: &VaultError) -> (StatusCode, Json<serde_json::Value>) {
    let status = match err {
        VaultError::InvalidName(_) | VaultError::ValueTooLarge { .. } => StatusCode::BAD_REQUEST,
        VaultError::Store(_) | VaultError::Serde(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (
        status,
        Json(serde_json::json!({ "ok": false, "error": err.to_string() })),
    )
}

/// 503 response when the node was built without a vault (test fixtures
/// that pass `secrets_vault: None`); production always wires one.
fn vault_unavailable() -> axum::response::Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({ "ok": false, "error": "secrets vault unavailable" })),
    )
        .into_response()
}

fn metadata_json(meta: &SecretMetadata) -> serde_json::Value {
    serde_json::json!({
        "name": meta.name,
        "description": meta.description,
        "created_at": meta.created_at,
        "updated_at": meta.updated_at,
    })
}

/// `GET /secrets` — list names + metadata only. Values are never
/// included regardless of query parameters.
pub(in crate::gateway) async fn list_secrets_handler(
    State(state): State<RouterState>,
) -> axum::response::Response {
    let Some(vault) = state.secrets_vault.as_ref() else {
        return vault_unavailable();
    };
    match vault.list() {
        Ok(metas) => {
            let secrets: Vec<_> = metas.iter().map(metadata_json).collect();
            (
                StatusCode::OK,
                Json(serde_json::json!({ "ok": true, "secrets": secrets })),
            )
                .into_response()
        }
        Err(e) => vault_error_response(&e).into_response(),
    }
}

#[derive(Deserialize)]
pub(in crate::gateway) struct GetSecretQuery {
    #[serde(default)]
    reveal: bool,
}

/// `GET /secrets/:name` — metadata only by default; `?reveal=true`
/// additionally returns the value.
pub(in crate::gateway) async fn get_secret_handler(
    State(state): State<RouterState>,
    Path(name): Path<String>,
    Query(query): Query<GetSecretQuery>,
) -> axum::response::Response {
    let Some(vault) = state.secrets_vault.as_ref() else {
        return vault_unavailable();
    };
    match vault.get(&name) {
        Ok(Some(record)) => {
            let mut secret = metadata_json(&record.metadata());
            if query.reveal {
                secret["value"] = serde_json::Value::String(record.value);
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({ "ok": true, "secret": secret })),
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "ok": false, "error": "secret not found" })),
        )
            .into_response(),
        Err(e) => vault_error_response(&e).into_response(),
    }
}

/// Body for `PUT /secrets/:name`.
///
/// Intentionally no `Debug` derive: the struct carries the secret value
/// and must never be formattable into logs or traces.
#[derive(Deserialize)]
pub(in crate::gateway) struct PutSecretBody {
    value: String,
    #[serde(default)]
    description: Option<String>,
}

/// `PUT /secrets/:name` — create or update a secret. Responds with
/// metadata only (the caller already knows the value).
pub(in crate::gateway) async fn put_secret_handler(
    State(state): State<RouterState>,
    Path(name): Path<String>,
    Json(body): Json<PutSecretBody>,
) -> axum::response::Response {
    let Some(vault) = state.secrets_vault.as_ref() else {
        return vault_unavailable();
    };
    match vault.put(&name, body.value, body.description) {
        Ok(meta) => (
            StatusCode::OK,
            Json(serde_json::json!({ "ok": true, "secret": metadata_json(&meta) })),
        )
            .into_response(),
        Err(e) => vault_error_response(&e).into_response(),
    }
}

/// `DELETE /secrets/:name`.
pub(in crate::gateway) async fn delete_secret_handler(
    State(state): State<RouterState>,
    Path(name): Path<String>,
) -> axum::response::Response {
    let Some(vault) = state.secrets_vault.as_ref() else {
        return vault_unavailable();
    };
    match vault.delete(&name) {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "ok": false, "error": "secret not found" })),
        )
            .into_response(),
        Err(e) => vault_error_response(&e).into_response(),
    }
}
