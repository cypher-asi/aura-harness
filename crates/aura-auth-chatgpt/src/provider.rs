//! [`SubscriptionProvider`] implementation for ChatGPT plans.

use crate::{identity, AUTH_BASE, DYNAMIC_CLIENT, PLAN_SCOPE, RESOURCE, SCOPE};
use async_trait::async_trait;
use aura_auth_kit::http;
use aura_core_auth::{
    AuthError, AuthorizationRequest, CallbackParams, InferenceCredential, LinkGrant, LinkedAccount,
    LoopbackRedirect, OAuthTokens, ProviderKind, RedirectStrategy, SubscriptionProvider,
};
use chrono::{Duration, Utc};
use jsonwebtoken::jwk::JwkSet;
use serde::Deserialize;
use serde_json::Value;

const NAME: &str = "ChatGPT";

const REDIRECT: LoopbackRedirect = LoopbackRedirect {
    host: "127.0.0.1",
    path: "/auth/openai/callback",
};

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    id_token: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    token_type: String,
    #[serde(default)]
    expires_in: i64,
}

/// Sign in with ChatGPT.
pub struct ChatgptProvider {
    auth_base: String,
    api_base: String,
    host_id: String,
}

impl ChatgptProvider {
    /// Production provider for this installation's `host_id`
    /// (see [`crate::load_or_create_host_id`]).
    #[must_use]
    pub fn new(host_id: String) -> Self {
        Self {
            auth_base: AUTH_BASE.into(),
            api_base: RESOURCE.into(),
            host_id,
        }
    }

    /// Provider pointed at a credential-free test server. Only numeric
    /// loopback HTTP origins are accepted, so this can never redirect
    /// real credentials.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::Invalid`] for any other origin.
    pub fn fixture(base: &str) -> Result<Self, AuthError> {
        let url =
            url::Url::parse(base).map_err(|_| AuthError::Invalid("Invalid fixture URL".into()))?;
        if url.scheme() != "http"
            || url.host_str() != Some("127.0.0.1")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
        {
            return Err(AuthError::Invalid(
                "Fixture endpoints must use numeric loopback HTTP".into(),
            ));
        }
        let base = base.trim_end_matches('/').to_string();
        Ok(Self {
            api_base: format!("{base}/v1"),
            auth_base: base,
            host_id: "urn:uuid:fixture-host".into(),
        })
    }

    /// Base URL for Responses API inference.
    #[must_use]
    pub fn api_base(&self) -> &str {
        &self.api_base
    }

    async fn metadata(&self) -> Result<Value, AuthError> {
        let response = http::client()?
            .get(format!(
                "{}/.well-known/openid-configuration",
                self.auth_base
            ))
            .send()
            .await
            .map_err(|e| http::unavailable(NAME, &e))?;
        if !response.status().is_success() {
            return Err(AuthError::Unavailable(
                "OpenAI authentication metadata unavailable".into(),
            ));
        }
        http::json(NAME, response).await
    }

    fn check_auth_url(&self, candidate: &str) -> Result<(), AuthError> {
        let base = url::Url::parse(&self.auth_base)
            .map_err(|_| AuthError::Invalid("Invalid authority".into()))?;
        let url = url::Url::parse(candidate)
            .map_err(|_| AuthError::Invalid("Invalid authority URL".into()))?;
        if url.origin() != base.origin() || !url.username().is_empty() || url.password().is_some() {
            return Err(AuthError::Invalid(
                "Authentication endpoint has an unexpected origin".into(),
            ));
        }
        Ok(())
    }

    async fn verify_identity(
        &self,
        id_token: &str,
        client: &str,
        nonce: Option<&str>,
    ) -> Result<identity::Claims, AuthError> {
        let metadata = self.metadata().await?;
        if metadata["issuer"] != self.auth_base.as_str() {
            return Err(AuthError::Invalid(
                "ChatGPT identity could not be verified".into(),
            ));
        }
        let jwks_uri = metadata["jwks_uri"]
            .as_str()
            .ok_or_else(|| AuthError::Invalid("ChatGPT identity could not be verified".into()))?;
        self.check_auth_url(jwks_uri)?;
        let response = http::client()?
            .get(jwks_uri)
            .send()
            .await
            .map_err(|e| http::unavailable(NAME, &e))?;
        if !response.status().is_success() {
            return Err(AuthError::Invalid(
                "ChatGPT identity could not be verified".into(),
            ));
        }
        let keys: JwkSet = http::json(NAME, response).await?;
        identity::verify(id_token, &keys, &self.auth_base, client, nonce)
    }

    async fn token_request(&self, form: &[(&str, &str)]) -> Result<reqwest::Response, AuthError> {
        http::client()?
            .post(format!("{}/api/accounts/oauth/token", self.auth_base))
            .form(form)
            .send()
            .await
            .map_err(|e| http::unavailable(NAME, &e))
    }
}

fn into_tokens(response: TokenResponse, require_refresh: bool) -> Result<OAuthTokens, AuthError> {
    if !response.token_type.eq_ignore_ascii_case("bearer")
        || response.access_token.is_empty()
        || response.expires_in <= 0
        || (require_refresh && response.refresh_token.is_empty())
    {
        return Err(AuthError::Malformed(
            "ChatGPT returned incomplete credentials".into(),
        ));
    }
    Ok(OAuthTokens {
        access_token: response.access_token,
        refresh_token: Some(response.refresh_token).filter(|t| !t.is_empty()),
        id_token: Some(response.id_token).filter(|t| !t.is_empty()),
        scope: response.scope,
        expires_at: Utc::now() + Duration::seconds(response.expires_in),
    })
}

/// Resolve the issued client id from the callback, rejecting a
/// callback that still carries the dynamic placeholder or swaps an
/// already-issued client.
fn issued_client(
    request: &AuthorizationRequest,
    callback: &CallbackParams,
) -> Result<String, AuthError> {
    let client = callback
        .client_id
        .clone()
        .unwrap_or_else(|| request.client_id.clone());
    if (request.client_id != DYNAMIC_CLIENT && client != request.client_id)
        || client == DYNAMIC_CLIENT
    {
        return Err(AuthError::Invalid(
            "Missing or mismatched issued client id".into(),
        ));
    }
    Ok(client)
}

#[async_trait]
impl SubscriptionProvider for ChatgptProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Chatgpt
    }

    fn redirect(&self) -> RedirectStrategy {
        RedirectStrategy {
            loopback: Some(REDIRECT),
            manual: None,
        }
    }

    fn client_id(&self, previous: Option<&str>) -> String {
        previous
            .filter(|c| !c.is_empty())
            .unwrap_or(DYNAMIC_CLIENT)
            .to_string()
    }

    fn authorization_url(&self, request: &AuthorizationRequest) -> Result<String, AuthError> {
        let mut url = url::Url::parse(&format!("{}/api/accounts/authorize", self.auth_base))
            .map_err(|_| AuthError::Invalid("Invalid OpenAI authority".into()))?;
        {
            let mut query = url.query_pairs_mut();
            query.extend_pairs([
                ("client_id", request.client_id.as_str()),
                ("ext_agent_host_id", &self.host_id),
                ("response_type", "code"),
                ("redirect_uri", &request.redirect_uri),
                ("scope", SCOPE),
                ("resource", RESOURCE),
                ("state", &request.state),
                ("nonce", &request.nonce),
                ("code_challenge_method", "S256"),
                ("code_challenge", &request.code_challenge),
            ]);
            if let Some(hint) = &request.login_hint {
                query.append_pair("login_hint", hint);
            } else if request.client_id == DYNAMIC_CLIENT {
                query.append_pair("agent_name_hint", "Aura");
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
            return Err(AuthError::Declined("ChatGPT sign-in was declined".into()));
        }
        let code = callback
            .code
            .as_deref()
            .filter(|c| !c.is_empty() && c.len() <= 8192)
            .ok_or_else(|| AuthError::Invalid("Missing authorization code".into()))?;
        let client = issued_client(request, callback)?;
        let response = self
            .token_request(&[
                ("grant_type", "authorization_code"),
                ("client_id", &client),
                ("code", code),
                ("code_verifier", &request.code_verifier),
                ("redirect_uri", &request.redirect_uri),
                ("resource", RESOURCE),
            ])
            .await?;
        if !response.status().is_success() {
            return Err(AuthError::Rejected {
                status: response.status().as_u16(),
                code: "code_exchange_failed".into(),
                message: "ChatGPT code exchange failed; start sign-in again".into(),
            });
        }
        let tokens = into_tokens(http::json(NAME, response).await?, false)?;
        let id_token = tokens
            .id_token
            .as_deref()
            .ok_or_else(|| AuthError::Malformed("ChatGPT returned no identity".into()))?;
        let claims = self
            .verify_identity(id_token, &client, Some(&request.nonce))
            .await?;
        Ok(LinkGrant {
            plan_enabled: tokens.has_scope(PLAN_SCOPE),
            tokens,
            subject: Some(claims.sub),
            label: Some(claims.email).filter(|e| !e.is_empty()),
            client_id: client,
        })
    }

    async fn refresh(
        &self,
        account: &LinkedAccount,
        tokens: &OAuthTokens,
    ) -> Result<LinkGrant, AuthError> {
        let refresh_token = tokens.refresh_token.as_deref().ok_or_else(|| {
            AuthError::ReauthRequired(
                "ChatGPT did not grant renewable access; run `aura login chatgpt` again".into(),
            )
        })?;
        let response = self
            .token_request(&[
                ("grant_type", "refresh_token"),
                ("client_id", &account.client_id),
                ("refresh_token", refresh_token),
                ("resource", RESOURCE),
            ])
            .await?;
        if !response.status().is_success() {
            return Err(http::refresh_failure(NAME, response).await);
        }
        let mut renewed = into_tokens(http::json(NAME, response).await?, true)?;
        if renewed.scope.is_empty() {
            renewed.scope.clone_from(&tokens.scope);
        }
        match renewed.id_token.as_deref() {
            Some(id_token) => {
                let claims = self
                    .verify_identity(id_token, &account.client_id, None)
                    .await?;
                if claims.sub != account.subject {
                    return Err(AuthError::ReauthRequired(
                        "ChatGPT identity changed; run `aura login chatgpt` again".into(),
                    ));
                }
            }
            None => renewed.id_token.clone_from(&tokens.id_token),
        }
        Ok(LinkGrant {
            plan_enabled: renewed.has_scope(PLAN_SCOPE),
            tokens: renewed,
            subject: None,
            label: None,
            client_id: account.client_id.clone(),
        })
    }

    async fn revoke(&self, account: &LinkedAccount, tokens: &OAuthTokens) {
        let Some(refresh_token) = tokens.refresh_token.as_deref() else {
            return;
        };
        let Ok(metadata) = self.metadata().await else {
            return;
        };
        let Some(endpoint) = metadata["revocation_endpoint"].as_str() else {
            return;
        };
        if self.check_auth_url(endpoint).is_err() {
            return;
        }
        let Ok(client) = http::client() else {
            return;
        };
        let result = client
            .post(endpoint)
            .form(&[
                ("token", refresh_token),
                ("token_type_hint", "refresh_token"),
                ("client_id", account.client_id.as_str()),
            ])
            .send()
            .await;
        if let Err(e) = result {
            tracing::debug!(error = %e, "ChatGPT revocation failed (best-effort)");
        }
    }

    fn credential(&self, tokens: &OAuthTokens) -> InferenceCredential {
        InferenceCredential {
            provider: ProviderKind::Chatgpt,
            bearer: tokens.access_token.clone(),
            headers: Vec::new(),
            system_preamble: None,
        }
    }
}

#[cfg(test)]
mod tests;
