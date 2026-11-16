//! Shared `ApiError` type for router handlers.
//!
//! Phase 1 (refactor) extracts a single error shape so duplicated parser
//! helpers across `memory.rs`, `skills.rs`, `tool_permissions.rs`, and
//! `tx.rs` can return one type instead of four parallel
//! `(StatusCode, _)` shapes.
//!
//! `ApiError` implements `IntoResponse` so handlers can return
//! `Result<_, ApiError>` directly. For backwards compatibility with the
//! older `(StatusCode, String)` shape used by some handlers, we also
//! provide `From<ApiError>` impls so callers that still return tuple
//! errors can `.map_err(ApiError::into)` without converting their entire
//! signature.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

/// Structured router error with a JSON body of `{ "error": "<message>" }`.
///
/// Errors emitted from router parsing/validation flow through this type
/// so all routes return a consistent JSON shape.
#[derive(Debug, Clone)]
pub(crate) struct ApiError {
    pub(crate) status: StatusCode,
    pub(crate) message: String,
}

impl ApiError {
    pub(crate) fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    /// Convert into the legacy `(StatusCode, String)` tuple shape used by some
    /// existing handlers (e.g. `tx.rs`, `tool_permissions.rs`).
    pub(crate) fn into_string_tuple(self) -> (StatusCode, String) {
        (self.status, self.message)
    }

    /// Convert into the legacy `(StatusCode, Json<serde_json::Value>)` tuple
    /// shape used by some existing handlers (e.g. `memory.rs`, `skills.rs`).
    pub(crate) fn into_json_tuple(self) -> (StatusCode, Json<serde_json::Value>) {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        self.into_json_tuple().into_response()
    }
}

impl From<ApiError> for (StatusCode, Json<serde_json::Value>) {
    fn from(err: ApiError) -> Self {
        err.into_json_tuple()
    }
}

impl From<ApiError> for (StatusCode, String) {
    fn from(err: ApiError) -> Self {
        err.into_string_tuple()
    }
}
