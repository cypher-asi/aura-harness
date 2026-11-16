//! Hardened HTTP client and OAuth error helpers shared by providers.

use aura_core_auth::AuthError;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::time::Duration;

/// Total request timeout for auth endpoints.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Connect-phase timeout so sign-in fails fast when offline.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// OAuth error codes meaning the refresh grant is permanently unusable.
pub const TERMINAL_GRANT_ERRORS: &[&str] = &[
    "invalid_grant",
    "invalid_refresh_token",
    "token_expired",
    "refresh_token_expired",
    "refresh_token_invalidated",
    "refresh_token_reused",
    "refresh_token_invalid",
    "invalid_client",
    "unauthorized_client",
];

/// Client for auth endpoints: bounded timeouts, no redirects.
///
/// # Errors
///
/// Returns [`AuthError::Unavailable`] if the client cannot be built.
pub fn client() -> Result<reqwest::Client, AuthError> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| AuthError::Unavailable("Sign-in HTTP client unavailable".into()))
}

/// Map a transport failure without echoing URLs or bodies.
#[must_use]
pub fn unavailable(provider: &str, error: &reqwest::Error) -> AuthError {
    let what = if error.is_timeout() {
        "timed out"
    } else if error.is_connect() {
        "could not connect"
    } else {
        "request failed"
    };
    AuthError::Unavailable(format!("{provider} sign-in service {what}; retry later"))
}

/// Decode a successful JSON body.
///
/// # Errors
///
/// Returns [`AuthError::Malformed`] when the body does not match `T`.
pub async fn json<T: DeserializeOwned>(
    provider: &str,
    response: reqwest::Response,
) -> Result<T, AuthError> {
    response
        .json::<T>()
        .await
        .map_err(|_| AuthError::Malformed(format!("{provider} returned an unexpected response")))
}

/// Extract the OAuth `error` code from an error body.
#[must_use]
pub fn oauth_error_code(body: &Value) -> String {
    body["error"]
        .as_str()
        .or_else(|| body["error"]["code"].as_str())
        .or_else(|| body["error"]["type"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Classify a failed refresh response.
///
/// Terminal grant errors become [`AuthError::ReauthRequired`]; 5xx and
/// 429 become [`AuthError::Unavailable`] so stored tokens are kept.
pub async fn refresh_failure(provider: &str, response: reqwest::Response) -> AuthError {
    let status = response.status();
    let body: Value = response.json().await.unwrap_or_default();
    let code = oauth_error_code(&body);
    tracing::warn!(provider, status = status.as_u16(), %code, "token refresh rejected");
    if TERMINAL_GRANT_ERRORS.contains(&code.as_str()) {
        return AuthError::ReauthRequired(format!(
            "{provider} access expired or was revoked; run `aura login {}` again",
            provider.to_ascii_lowercase()
        ));
    }
    if status.is_server_error() || status.as_u16() == 429 {
        return AuthError::Unavailable(format!(
            "{provider} renewal is temporarily unavailable; retry later"
        ));
    }
    AuthError::Unavailable(format!("{provider} renewal failed (status {status})"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn error_code_shapes() {
        assert_eq!(
            oauth_error_code(&json!({"error":"invalid_grant"})),
            "invalid_grant"
        );
        assert_eq!(
            oauth_error_code(&json!({"error":{"code":"token_expired"}})),
            "token_expired"
        );
        assert_eq!(oauth_error_code(&json!({})), "");
    }
}
