//! OAuth data shared by subscription providers and credential storage.

use crate::{Owner, ProviderKind};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Tokens issued by a subscription provider. Secret: never log.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthTokens {
    /// Bearer access token.
    pub access_token: String,
    /// Refresh token, when renewable access was granted.
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// OIDC id token, when issued.
    #[serde(default)]
    pub id_token: Option<String>,
    /// Space-separated granted scopes.
    #[serde(default)]
    pub scope: String,
    /// When the access token expires.
    pub expires_at: DateTime<Utc>,
}

impl OAuthTokens {
    /// Whether the access token expires within `seconds` of `now`.
    #[must_use]
    pub fn expires_within(&self, now: DateTime<Utc>, seconds: i64) -> bool {
        self.expires_at <= now + Duration::seconds(seconds)
    }

    /// Whether `scope` was granted.
    #[must_use]
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scope.split_whitespace().any(|s| s == scope)
    }
}

impl fmt::Debug for OAuthTokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthTokens")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("id_token", &self.id_token.as_ref().map(|_| "<redacted>"))
            .field("scope", &self.scope)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Health of a linked subscription.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LinkStatus {
    /// Usable for inference.
    #[default]
    Active,
    /// Credentials were revoked or expired; sign in again.
    ReauthRequired {
        /// Safe, user-facing reason.
        reason: String,
    },
}

/// Non-secret metadata for a subscription linked to an identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkedAccount {
    /// Local id of this link.
    pub id: String,
    /// Subscription provider.
    pub provider: ProviderKind,
    /// Identity that owns the link.
    pub owner: Owner,
    /// Provider subject (stable account id).
    pub subject: String,
    /// Email or other display label.
    pub label: String,
    /// OAuth client id the tokens were issued to.
    pub client_id: String,
    /// Whether the plan grants inference usage.
    pub plan_enabled: bool,
    /// Access-token expiry, for status display.
    pub expires_at: DateTime<Utc>,
    /// When the link was created.
    pub created_at: DateTime<Utc>,
    /// Current health.
    #[serde(default)]
    pub status: LinkStatus,
}

impl LinkedAccount {
    /// Whether the link can supply inference credentials.
    #[must_use]
    pub fn usable(&self) -> bool {
        self.plan_enabled && self.status == LinkStatus::Active
    }
}

/// Result of a successful code exchange or refresh.
#[derive(Debug, Clone)]
pub struct LinkGrant {
    /// Issued tokens.
    pub tokens: OAuthTokens,
    /// Provider subject, when the provider identifies the account.
    pub subject: Option<String>,
    /// Email or display label, when known.
    pub label: Option<String>,
    /// Client id the tokens belong to.
    pub client_id: String,
    /// Whether the grant allows plan inference usage.
    pub plan_enabled: bool,
}

/// A loopback redirect served by an ephemeral local listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopbackRedirect {
    /// Host written into the redirect URI (`127.0.0.1` or `localhost`).
    /// The listener always binds `127.0.0.1`.
    pub host: &'static str,
    /// Callback path, e.g. `/callback`.
    pub path: &'static str,
}

/// How the provider returns the authorization code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedirectStrategy {
    /// Automatic browser callback to a local listener.
    pub loopback: Option<LoopbackRedirect>,
    /// Hosted page that displays a code for the user to paste.
    pub manual: Option<&'static str>,
}

/// One authorization attempt. Secret: holds the PKCE verifier.
#[derive(Clone)]
pub struct AuthorizationRequest {
    /// Client id used for this attempt.
    pub client_id: String,
    /// Redirect URI registered for this attempt.
    pub redirect_uri: String,
    /// Anti-CSRF state.
    pub state: String,
    /// OIDC nonce.
    pub nonce: String,
    /// PKCE verifier.
    pub code_verifier: String,
    /// PKCE S256 challenge.
    pub code_challenge: String,
    /// Account hint when reconnecting an existing link.
    pub login_hint: Option<String>,
}

impl fmt::Debug for AuthorizationRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthorizationRequest")
            .field("client_id", &self.client_id)
            .field("redirect_uri", &self.redirect_uri)
            .field("login_hint", &self.login_hint)
            .finish_non_exhaustive()
    }
}

/// Parameters delivered to the redirect URI (or pasted by the user).
#[derive(Clone, Default, Deserialize)]
pub struct CallbackParams {
    /// Authorization code.
    #[serde(default)]
    pub code: Option<String>,
    /// Echoed state.
    #[serde(default)]
    pub state: String,
    /// Issued client id (dynamic registration).
    #[serde(default)]
    pub client_id: Option<String>,
    /// OAuth error code when the user declined.
    #[serde(default)]
    pub error: Option<String>,
}

impl fmt::Debug for CallbackParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallbackParams")
            .field("code", &self.code.as_ref().map(|_| "<redacted>"))
            .field("client_id", &self.client_id)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Credentials a model provider attaches to one request. Secret: never log.
#[derive(Clone, PartialEq, Eq)]
pub struct InferenceCredential {
    /// Provider the credential belongs to.
    pub provider: ProviderKind,
    /// Bearer token for the `Authorization` header.
    pub bearer: String,
    /// Additional headers the provider requires.
    pub headers: Vec<(String, String)>,
    /// Text the provider requires at the start of the system prompt.
    pub system_preamble: Option<String>,
}

impl fmt::Debug for InferenceCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InferenceCredential")
            .field("provider", &self.provider)
            .field("bearer", &"<redacted>")
            .field("headers", &self.headers)
            .field("system_preamble", &self.system_preamble)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(expires_in: i64) -> OAuthTokens {
        OAuthTokens {
            access_token: "secret-access".into(),
            refresh_token: Some("secret-refresh".into()),
            id_token: None,
            scope: "openid chatgpt.tokens.use.direct".into(),
            expires_at: Utc::now() + Duration::seconds(expires_in),
        }
    }

    #[test]
    fn debug_never_prints_secrets() {
        let rendered = format!("{:?}", tokens(600));
        assert!(!rendered.contains("secret-"), "{rendered}");
        let credential = InferenceCredential {
            provider: ProviderKind::Claude,
            bearer: "secret-bearer".into(),
            headers: vec![],
            system_preamble: None,
        };
        assert!(!format!("{credential:?}").contains("secret-"));
    }

    #[test]
    fn expiry_and_scope_checks() {
        let now = Utc::now();
        assert!(tokens(30).expires_within(now, 60));
        assert!(!tokens(600).expires_within(now, 60));
        assert!(tokens(600).has_scope("chatgpt.tokens.use.direct"));
        assert!(!tokens(600).has_scope("chatgpt.tokens"));
    }
}
