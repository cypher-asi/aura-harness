//! Success-guard checks applied to trusted-provider JSON responses.
//!
//! `apply_success_guard` runs after the HTTP layer has already accepted
//! the response (status 2xx + valid JSON) and rejects logically-failed
//! payloads — Slack's `ok: false` envelope, GraphQL `errors[]` arrays,
//! etc. — before the result transform runs.

use super::super::json_paths::ensure_slack_ok;
use super::TrustedIntegrationSuccessGuard;
use crate::error::ToolError;
use serde_json::Value;

pub(super) fn apply_success_guard(
    response: &Value,
    guard: &TrustedIntegrationSuccessGuard,
) -> Result<(), ToolError> {
    match guard {
        TrustedIntegrationSuccessGuard::None => Ok(()),
        TrustedIntegrationSuccessGuard::SlackOk => ensure_slack_ok(response),
        TrustedIntegrationSuccessGuard::GraphqlErrors => match graphql_user_errors(response) {
            Some(message) => Err(ToolError::ExternalToolError(format!(
                "graphql error: {message}"
            ))),
            None => Ok(()),
        },
    }
}

/// Extract a GraphQL `errors[].message` summary from `response`.
///
/// Returns `Some(joined)` when the response contains a non-empty
/// `errors` array; `None` otherwise. Messages are joined with `"; "`,
/// matching the legacy `linear_graphql` and `apply_success_guard`
/// implementations bit-for-bit. Each caller adds its own prefix
/// (`"linear graphql error: "` / `"graphql error: "`) so this helper
/// stays prefix-agnostic.
pub(super) fn graphql_user_errors(response: &Value) -> Option<String> {
    let errors = response.get("errors").and_then(Value::as_array)?;
    if errors.is_empty() {
        return None;
    }
    Some(
        errors
            .iter()
            .filter_map(|error| error.get("message").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("; "),
    )
}
