
use super::*;
use axum::routing::{get, post};
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
    let app = Router::new()
        .route(
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
        )
        .route(
            "/api/oauth/profile",
            get(|headers: axum::http::HeaderMap| async move {
                if headers["authorization"] != "Bearer sk-ant-oat-1" {
                    return (axum::http::StatusCode::UNAUTHORIZED, Json(json!({})));
                }
                (
                    axum::http::StatusCode::OK,
                    Json(json!({
                        "account": {"has_claude_max": true, "has_claude_pro": false},
                        "organization": {
                            "organization_type": "claude_max",
                            "rate_limit_tier": "default_claude_max_20x",
                        },
                    })),
                )
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
        plan: None,
    }
}

fn tier(profile: Value) -> Option<String> {
    serde_json::from_value::<Profile>(profile).unwrap().tier()
}

#[test]
fn profile_tier_names() {
    let org = |kind: &str, limit: &str| json!({"organization": {"organization_type": kind, "rate_limit_tier": limit}});
    assert_eq!(
        tier(org("claude_max", "default_claude_max_5x")).as_deref(),
        Some("Max 5x")
    );
    assert_eq!(
        tier(org("claude_pro", "default_claude_ai")).as_deref(),
        Some("Pro")
    );
    assert_eq!(
        tier(org("claude_ultra", "default")).as_deref(),
        Some("Ultra")
    );
    assert_eq!(
        tier(json!({"account": {"has_claude_max": true}})).as_deref(),
        Some("Max")
    );
    assert_eq!(tier(json!({})), None);
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
    let callback =
        aura_auth_kit::parse_pasted_code(&format!("the-code#{}", request.state), &request.state)
            .unwrap();
    let grant = provider.exchange(&request, &callback).await.unwrap();
    assert_eq!(grant.subject.as_deref(), Some("acct-1"));
    assert_eq!(grant.label.as_deref(), Some("bob@example.com"));
    assert!(grant.plan_enabled);
    let sent = seen.lock().unwrap()[0].clone();
    assert_eq!(sent["code"], "the-code");
    assert_eq!(sent["code_verifier"], request.code_verifier.as_str());

    assert_eq!(
        provider.plan_tier(&grant.tokens).await.as_deref(),
        Some("Max 20x")
    );

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
