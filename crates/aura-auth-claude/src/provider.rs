//! [`SubscriptionProvider`] implementation for Claude plans.

use crate::{
    AUTHORIZE_URL, CLIENT_ID, MANUAL_REDIRECT, OAUTH_BETA, PLAN_SCOPE, SCOPE, SYSTEM_PREAMBLE,
    TOKEN_URL,
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

/// Claude Pro/Max sign-in.
pub struct ClaudeProvider {
    authorize_url: String,
    token_url: String,
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
mod tests {
    use super::*;
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::Value;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    type Seen = Arc<Mutex<Vec<Value>>>;

    async fn serve() -> (ClaudeProvider, Seen) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen: Seen = Arc::default();
        let log = seen.clone();
        let app = Router::new().route(
            "/v1/oauth/token",
            post(move |Json(body): Json<Value>| {
                let log = log.clone();
                async move {
                    log.lock().unwrap().push(body.clone());
                    if body["refresh_token"] == "revoked" {
                        return (
                            axum::http::StatusCode::BAD_REQUEST,
                            Json(json!({"error":"invalid_grant"})),
                        );
                    }
                    (
                        axum::http::StatusCode::OK,
                        Json(json!({
                            "access_token": "sk-ant-oat-1",
                            "refresh_token": "sk-ant-ort-1",
                            "expires_in": 28800,
                            "scope": SCOPE,
                            "account": {"uuid": "acct-1", "email_address": "bob@example.com"},
                        })),
                    )
                }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (ClaudeProvider::fixture(&base).unwrap(), seen)
    }

    fn linked(client_id: &str) -> LinkedAccount {
        LinkedAccount {
            id: "l".into(),
            provider: ProviderKind::Claude,
            owner: aura_core_auth::Owner::Local,
            subject: "acct-1".into(),
            label: "bob@example.com".into(),
            client_id: client_id.into(),
            plan_enabled: true,
            expires_at: Utc::now(),
            created_at: Utc::now(),
            status: aura_core_auth::LinkStatus::Active,
        }
    }

    #[test]
    fn authorization_url_shape() {
        let provider = ClaudeProvider::new();
        let request =
            aura_auth_kit::new_attempt(provider.client_id(None), MANUAL_REDIRECT.into(), None);
        let url = url::Url::parse(&provider.authorization_url(&request).unwrap()).unwrap();
        let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(url.host_str(), Some("claude.ai"));
        assert_eq!(query["client_id"], CLIENT_ID);
        assert_eq!(query["code_challenge_method"], "S256");
        assert_eq!(query["redirect_uri"], MANUAL_REDIRECT);
        assert_eq!(query["scope"], SCOPE);
    }

    #[test]
    fn credential_carries_beta_and_preamble() {
        let credential = ClaudeProvider::new().credential(&OAuthTokens {
            access_token: "tok".into(),
            refresh_token: None,
            id_token: None,
            scope: SCOPE.into(),
            expires_at: Utc::now(),
        });
        assert_eq!(credential.bearer, "tok");
        assert!(credential
            .headers
            .contains(&("anthropic-beta".to_string(), OAUTH_BETA.to_string())));
        assert_eq!(credential.system_preamble.as_deref(), Some(SYSTEM_PREAMBLE));
    }

    #[tokio::test]
    async fn exchange_and_refresh() {
        let (provider, seen) = serve().await;
        let request = aura_auth_kit::new_attempt(CLIENT_ID.into(), MANUAL_REDIRECT.into(), None);
        let callback = aura_auth_kit::parse_pasted_code(
            &format!("the-code#{}", request.state),
            &request.state,
        )
        .unwrap();
        let grant = provider.exchange(&request, &callback).await.unwrap();
        assert_eq!(grant.subject.as_deref(), Some("acct-1"));
        assert_eq!(grant.label.as_deref(), Some("bob@example.com"));
        assert!(grant.plan_enabled);
        let sent = seen.lock().unwrap()[0].clone();
        assert_eq!(sent["code"], "the-code");
        assert_eq!(sent["code_verifier"], request.code_verifier.as_str());

        let mut tokens = grant.tokens.clone();
        let renewed = provider.refresh(&linked(CLIENT_ID), &tokens).await.unwrap();
        assert_eq!(renewed.tokens.access_token, "sk-ant-oat-1");

        tokens.refresh_token = Some("revoked".into());
        let err = provider
            .refresh(&linked(CLIENT_ID), &tokens)
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::ReauthRequired(_)), "{err:?}");
    }
}
