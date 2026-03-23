//! HTTP client for zOS authentication.
//!
//! Communicates with `https://zosapi.zero.tech` to authenticate users via
//! email/password and retrieve user profile information. This mirrors the
//! login flow in `aura-app`'s `AuthService`.
//!
//! # Endpoints used
//!
//! | Purpose   | Method   | URL                                         |
//! |-----------|----------|---------------------------------------------|
//! | Login     | `POST`   | `/api/v2/accounts/login`                    |
//! | User info | `GET`    | `/api/users/current`                        |
//! | Logout    | `DELETE` | `/authentication/session`                   |

use crate::credentials::StoredSession;
use crate::error::AuthError;
use chrono::Utc;
use serde::Deserialize;
use std::time::Duration;
use tracing::{debug, error};

const ZOS_API_URL: &str = "https://zosapi.zero.tech";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Response from the zOS login endpoint.
#[derive(Debug, Deserialize)]
struct ZosLoginResponse {
    #[serde(rename = "accessToken")]
    access_token: String,
}

/// Profile summary nested inside the user response.
#[derive(Debug, Deserialize)]
struct ZosProfileSummary {
    #[serde(rename = "firstName")]
    first_name: Option<String>,
    #[serde(rename = "lastName")]
    last_name: Option<String>,
}

/// Response from the zOS current-user endpoint.
#[derive(Debug, Deserialize)]
struct ZosUserResponse {
    id: String,
    #[serde(rename = "profileSummary")]
    profile_summary: Option<ZosProfileSummary>,
    #[serde(rename = "primaryZID")]
    primary_zid: Option<String>,
}

/// Error body returned by the zOS API on failure.
#[derive(Debug, Deserialize)]
struct ZosErrorBody {
    code: Option<String>,
    message: Option<String>,
}

/// Client for zOS authentication APIs.
pub struct ZosClient {
    http: reqwest::Client,
}

impl ZosClient {
    /// Create a new client with default timeout settings.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client cannot be built.
    pub fn new() -> Result<Self, AuthError> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self { http })
    }

    /// Authenticate with email and password.
    ///
    /// On success, fetches the user profile and returns a [`StoredSession`]
    /// containing the JWT access token and user metadata.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::ZosApi`] if the credentials are rejected, or
    /// [`AuthError::Http`] on network failure.
    pub async fn login(&self, email: &str, password: &str) -> Result<StoredSession, AuthError> {
        debug!("Authenticating via zOS API");

        let res = self
            .http
            .post(format!("{ZOS_API_URL}/api/v2/accounts/login"))
            .json(&serde_json::json!({ "email": email, "password": password }))
            .send()
            .await?;

        if !res.status().is_success() {
            return Err(parse_zos_error(res).await);
        }

        let login_data: ZosLoginResponse = res.json().await?;
        let user = self.fetch_user_info(&login_data.access_token).await?;

        let display_name =
            build_display_name(user.profile_summary.as_ref(), user.primary_zid.as_deref());

        Ok(StoredSession {
            access_token: login_data.access_token,
            user_id: user.id,
            display_name,
            primary_zid: user.primary_zid.unwrap_or_default(),
            created_at: Utc::now(),
        })
    }

    /// Fetch the current user's profile using a Bearer token.
    async fn fetch_user_info(&self, token: &str) -> Result<ZosUserResponse, AuthError> {
        debug!("Fetching user info from zOS API");

        let res = self
            .http
            .get(format!("{ZOS_API_URL}/api/users/current"))
            .bearer_auth(token)
            .send()
            .await?;

        if !res.status().is_success() {
            return Err(parse_zos_error(res).await);
        }

        Ok(res.json().await?)
    }

    /// Invalidate the session on the zOS side. Best-effort: errors are logged
    /// but not propagated since the local credentials will be cleared
    /// regardless.
    pub async fn logout(&self, token: &str) {
        debug!("Logging out via zOS API");

        let result = self
            .http
            .delete(format!("{ZOS_API_URL}/authentication/session"))
            .bearer_auth(token)
            .send()
            .await;

        if let Err(e) = result {
            debug!(error = %e, "zOS logout request failed (best-effort)");
        }
    }
}

/// Parse an error response from the zOS API.
async fn parse_zos_error(res: reqwest::Response) -> AuthError {
    let status = res.status().as_u16();
    let body = res.text().await.unwrap_or_default();

    let (code, message) = match serde_json::from_str::<ZosErrorBody>(&body) {
        Ok(parsed) => (
            parsed.code.unwrap_or_default(),
            parsed.message.unwrap_or_else(|| body.clone()),
        ),
        Err(_) => (String::new(), body),
    };

    error!(status, %code, %message, "zOS API error");

    AuthError::ZosApi {
        status,
        code,
        message,
    }
}

/// Build a display name from profile fields, falling back to zID or "User".
fn build_display_name(profile: Option<&ZosProfileSummary>, primary_zid: Option<&str>) -> String {
    if let Some(p) = profile {
        let first = p.first_name.as_deref().unwrap_or("");
        let last = p.last_name.as_deref().unwrap_or("");
        let full = format!("{first} {last}").trim().to_string();
        if !full.is_empty() {
            return full;
        }
    }
    if let Some(zid) = primary_zid {
        if !zid.is_empty() {
            return zid.to_string();
        }
    }
    "User".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_display_name_full() {
        let profile = ZosProfileSummary {
            first_name: Some("Alice".to_string()),
            last_name: Some("Smith".to_string()),
        };
        assert_eq!(build_display_name(Some(&profile), None), "Alice Smith");
    }

    #[test]
    fn test_build_display_name_first_only() {
        let profile = ZosProfileSummary {
            first_name: Some("Alice".to_string()),
            last_name: None,
        };
        assert_eq!(build_display_name(Some(&profile), None), "Alice");
    }

    #[test]
    fn test_build_display_name_falls_back_to_zid() {
        let profile = ZosProfileSummary {
            first_name: None,
            last_name: None,
        };
        assert_eq!(
            build_display_name(Some(&profile), Some("0://alice")),
            "0://alice"
        );
    }

    #[test]
    fn test_build_display_name_falls_back_to_user() {
        assert_eq!(build_display_name(None, None), "User");
    }

    #[test]
    fn test_build_display_name_empty_strings() {
        let profile = ZosProfileSummary {
            first_name: Some(String::new()),
            last_name: Some(String::new()),
        };
        assert_eq!(build_display_name(Some(&profile), Some("")), "User");
    }

    #[test]
    fn test_zos_login_response_deserialize() {
        let json = r#"{"accessToken":"eyJ...","identityToken":"idt_..."}"#;
        let resp: ZosLoginResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.access_token, "eyJ...");
    }

    #[test]
    fn test_zos_user_response_deserialize() {
        let json = r#"{
            "id": "u-123",
            "profileSummary": {
                "firstName": "Alice",
                "lastName": "Smith",
                "profileImage": "https://example.com/img.png"
            },
            "primaryZID": "0://alice",
            "primaryWalletAddress": "0x123",
            "wallets": [{"publicAddress": "0x123"}]
        }"#;
        let resp: ZosUserResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, "u-123");
        assert_eq!(resp.primary_zid.as_deref(), Some("0://alice"));
    }

    #[test]
    fn test_zos_error_body_deserialize() {
        let json = r#"{"code":"INVALID_CREDENTIALS","message":"Bad password"}"#;
        let body: ZosErrorBody = serde_json::from_str(json).unwrap();
        assert_eq!(body.code.as_deref(), Some("INVALID_CREDENTIALS"));
        assert_eq!(body.message.as_deref(), Some("Bad password"));
    }

    #[test]
    fn test_zos_error_body_partial() {
        let json = r#"{"message":"Something went wrong"}"#;
        let body: ZosErrorBody = serde_json::from_str(json).unwrap();
        assert!(body.code.is_none());
        assert_eq!(body.message.as_deref(), Some("Something went wrong"));
    }
}
