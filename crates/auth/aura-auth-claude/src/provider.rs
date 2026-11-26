//! [`SubscriptionProvider`] implementation for Claude plans.

use crate::{
    AUTHORIZE_URL, CLIENT_ID, MANUAL_REDIRECT, OAUTH_BETA, PLAN_SCOPE, PROFILE_URL, SCOPE,
    SYSTEM_PREAMBLE, TOKEN_URL,
};
use async_trait::async_trait;
use aura_auth_kit::http;
use aura_core_auth::{
    AuthError, AuthorizationRequest, CallbackParams, InferenceCredential, LinkGrant, LinkedAccount,
    LoopbackRedirect, OAuthTokens, ProviderKind, RedirectStrategy, SubscriptionProvider,
};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::json;

const NAME: &str = "Claude";

const REDIRECT: LoopbackRedirect = LoopbackRedirect {
    host: "localhost",
    path: "/callback",
};

#[derive(Deserialize)]
struct TokenAccount {
    #[serde(default)]
    uuid: String,
    #[serde(default)]
    email_address: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    expires_in: i64,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    account: Option<TokenAccount>,
}

#[derive(Default, Deserialize)]
struct Profile {
    #[serde(default)]
    account: ProfileAccount,
    #[serde(default)]
    organization: ProfileOrganization,
}

#[derive(Default, Deserialize)]
struct ProfileAccount {
    #[serde(default)]
    has_claude_max: bool,
    #[serde(default)]
    has_claude_pro: bool,
}

#[derive(Default, Deserialize)]
struct ProfileOrganization {
    /// `claude_pro`, `claude_max`, `claude_team`, `claude_enterprise`, …
    #[serde(default)]
    organization_type: Option<String>,
    /// e.g. `default_claude_max_20x`; the trailing `20x` is the usage multiple.
    #[serde(default)]
    rate_limit_tier: Option<String>,
}

impl Profile {
    fn tier(&self) -> Option<String> {
        let kind = self
            .organization
            .organization_type
            .as_deref()
            .map(|t| t.strip_prefix("claude_").unwrap_or(t))
            .filter(|t| !t.is_empty())
            .or(if self.account.has_claude_max {
                Some("max")
            } else if self.account.has_claude_pro {
                Some("pro")
            } else {
                None
            })?;
        let mut label = aura_auth_kit::plan_label(kind)?;
        let multiple = self
            .organization
            .rate_limit_tier
            .as_deref()
            .and_then(|t| t.rsplit('_').next())
            .filter(|m| {
                m.strip_suffix('x')
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            });
        if let Some(multiple) = multiple {
            if !label.ends_with(multiple) {
                label.push(' ');
                label.push_str(multiple);
            }
        }
        Some(label)
    }
}

/// Claude Pro/Max sign-in.
pub struct ClaudeProvider {
    authorize_url: String,
    token_url: String,
    profile_url: String,
}

impl Default for ClaudeProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeProvider {
    /// Production provider.
    #[must_use]
    pub fn new() -> Self {
        Self {
            authorize_url: AUTHORIZE_URL.into(),
            token_url: TOKEN_URL.into(),
            profile_url: PROFILE_URL.into(),
        }
    }

    /// Provider pointed at a credential-free numeric-loopback test server.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::Invalid`] for any other origin.
    pub fn fixture(base: &str) -> Result<Self, AuthError> {
        let url =
            url::Url::parse(base).map_err(|_| AuthError::Invalid("Invalid fixture URL".into()))?;
        if url.scheme() != "http" || url.host_str() != Some("127.0.0.1") {
            return Err(AuthError::Invalid(
                "Fixture endpoints must use numeric loopback HTTP".into(),
            ));
        }
        let base = base.trim_end_matches('/');
        Ok(Self {
            authorize_url: format!("{base}/oauth/authorize"),
            token_url: format!("{base}/v1/oauth/token"),
            profile_url: format!("{base}/api/oauth/profile"),
        })
    }

    async fn token_request(&self, body: serde_json::Value) -> Result<reqwest::Response, AuthError> {
        http::client()?
            .post(&self.token_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| http::unavailable(NAME, &e))
    }
}

fn into_tokens(response: TokenResponse, previous_scope: &str) -> Result<OAuthTokens, AuthError> {
    if response.access_token.is_empty() || response.expires_in <= 0 {
        return Err(AuthError::Malformed(
            "Claude returned incomplete credentials".into(),
        ));
    }
    let scope = if response.scope.is_empty() {
        previous_scope.to_string()
    } else {
        response.scope
    };
    Ok(OAuthTokens {
        access_token: response.access_token,
        refresh_token: Some(response.refresh_token).filter(|t| !t.is_empty()),
        id_token: None,
        scope,
        expires_at: Utc::now() + Duration::seconds(response.expires_in),
    })
}

#[async_trait]
impl SubscriptionProvider for ClaudeProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Claude
    }

    fn redirect(&self) -> RedirectStrategy {
        RedirectStrategy {
            loopback: Some(REDIRECT),
            manual: Some(MANUAL_REDIRECT),
        }
    }

    fn client_id(&self, _previous: Option<&str>) -> String {
        CLIENT_ID.to_string()
    }

    fn authorization_url(&self, request: &AuthorizationRequest) -> Result<String, AuthError> {
        let mut url = url::Url::parse(&self.authorize_url)
            .map_err(|_| AuthError::Invalid("Invalid Claude authority".into()))?;
        {
            let mut query = url.query_pairs_mut();
            query.extend_pairs([
                ("code", "true"),
                ("client_id", request.client_id.as_str()),
                ("response_type", "code"),
                ("redirect_uri", &request.redirect_uri),
                ("scope", SCOPE),
                ("code_challenge", &request.code_challenge),
                ("code_challenge_method", "S256"),
                ("state", &request.state),
            ]);
            if let Some(hint) = &request.login_hint {
                query.append_pair("login_hint", hint);
            }
        }
        Ok(url.into())
    }

    async fn exchange(
        &self,
        request: &AuthorizationRequest,
        callback: &CallbackParams,
    ) -> Result<LinkGrant, AuthError> {
        if callback.error.is_some() {
            return Err(AuthError::Declined("Claude sign-in was declined".into()));
        }
        let code = callback
            .code
            .as_deref()
            .filter(|c| !c.is_empty() && c.len() <= 8192)
            .ok_or_else(|| AuthError::Invalid("Missing authorization code".into()))?;
        let response = self
            .token_request(json!({
                "grant_type": "authorization_code",
                "code": code,
                "state": request.state,
                "client_id": request.client_id,
                "redirect_uri": request.redirect_uri,
                "code_verifier": request.code_verifier,
            }))
            .await?;
        if !response.status().is_success() {
            return Err(AuthError::Rejected {
                status: response.status().as_u16(),
                code: "code_exchange_failed".into(),
                message: "Claude code exchange failed; start sign-in again".into(),
            });
        }
        let body: TokenResponse = http::json(NAME, response).await?;
        let account = body
            .account
            .as_ref()
            .map(|a| (a.uuid.clone(), a.email_address.clone()));
        let tokens = into_tokens(body, "")?;
        Ok(LinkGrant {
            plan_enabled: tokens.scope.is_empty() || tokens.has_scope(PLAN_SCOPE),
            tokens,
            subject: account
                .as_ref()
                .map(|a| a.0.clone())
                .filter(|s| !s.is_empty()),
            label: account.map(|a| a.1).filter(|s| !s.is_empty()),
            client_id: request.client_id.clone(),
        })
    }

    async fn refresh(
        &self,
        account: &LinkedAccount,
        tokens: &OAuthTokens,
    ) -> Result<LinkGrant, AuthError> {
        let refresh_token = tokens.refresh_token.as_deref().ok_or_else(|| {
            AuthError::ReauthRequired(
                "Claude did not grant renewable access; run `aura login claude` again".into(),
            )
        })?;
        let response = self
            .token_request(json!({
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
                "client_id": account.client_id,
            }))
            .await?;
        if !response.status().is_success() {
            return Err(http::refresh_failure(NAME, response).await);
        }
        let body: TokenResponse = http::json(NAME, response).await?;
        if let Some(a) = &body.account {
            if !a.uuid.is_empty() && !account.subject.is_empty() && a.uuid != account.subject {
                return Err(AuthError::ReauthRequired(
                    "Claude account changed; run `aura login claude` again".into(),
                ));
            }
        }
        let mut renewed = into_tokens(body, &tokens.scope)?;
        if renewed.refresh_token.is_none() {
            renewed.refresh_token.clone_from(&tokens.refresh_token);
        }
        Ok(LinkGrant {
            plan_enabled: renewed.scope.is_empty() || renewed.has_scope(PLAN_SCOPE),
            tokens: renewed,
            subject: None,
            label: None,
            client_id: account.client_id.clone(),
        })
    }

    async fn plan_tier(&self, tokens: &OAuthTokens) -> Option<String> {
        let response = http::client()
            .ok()?
            .get(&self.profile_url)
            .bearer_auth(&tokens.access_token)
            .header("anthropic-beta", OAUTH_BETA)
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        response.json::<Profile>().await.ok()?.tier()
    }

    fn credential(&self, tokens: &OAuthTokens) -> InferenceCredential {
        InferenceCredential {
            provider: ProviderKind::Claude,
            bearer: tokens.access_token.clone(),
            headers: vec![("anthropic-beta".into(), OAUTH_BETA.into())],
            system_preamble: Some(SYSTEM_PREAMBLE.into()),
        }
    }
}

#[cfg(test)]
mod tests;
