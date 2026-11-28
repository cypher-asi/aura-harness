use super::*;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

type Seen = Arc<Mutex<Vec<Value>>>;

const KEY: &str = "sk-or-v1-secret-test-key";

async fn serve() -> (OpenRouterProvider, Seen) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen: Seen = Arc::default();
    let log = seen.clone();
    let app = Router::new()
        .route(
            "/api/v1/auth/keys",
            post(move |Json(body): Json<Value>| {
                let log = log.clone();
                async move {
                    log.lock().unwrap().push(body.clone());
                    if body["code"] == "expired-code" {
                        return (
                            axum::http::StatusCode::FORBIDDEN,
                            Json(json!({"error": {"code": 403, "message": "Invalid code"}})),
                        );
                    }
                    (
                        axum::http::StatusCode::OK,
                        Json(json!({"key": KEY, "user_id": "user-1"})),
                    )
                }
            }),
        )
        .route(
            "/api/v1/key",
            get(|headers: axum::http::HeaderMap| async move {
                if headers["authorization"] != format!("Bearer {KEY}").as_str() {
                    return (
                        axum::http::StatusCode::UNAUTHORIZED,
                        Json(json!({"error": {"code": 401, "message": "No auth"}})),
                    );
                }
                (
                    axum::http::StatusCode::OK,
                    Json(json!({"data": {
                        "label": "Aura key",
                        "limit": 10.0,
                        "limit_remaining": 4.5,
                        "usage": 5.5,
                        "is_free_tier": false,
                    }})),
                )
            }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (OpenRouterProvider::fixture(&base).unwrap(), seen)
}

fn query(provider: &OpenRouterProvider, redirect: &str) -> (url::Url, HashMap<String, String>) {
    let request = aura_auth_kit::new_attempt(CLIENT_ID.into(), redirect.into(), None);
    let url = url::Url::parse(&provider.authorization_url(&request).unwrap()).unwrap();
    let pairs = url.query_pairs().into_owned().collect();
    (url, pairs)
}

fn linked() -> LinkedAccount {
    LinkedAccount {
        id: "l".into(),
        provider: ProviderKind::OpenRouter,
        owner: aura_core_auth::Owner::Local,
        subject: "user-1".into(),
        label: "Aura key".into(),
        client_id: CLIENT_ID.into(),
        plan_enabled: true,
        expires_at: Utc::now(),
        created_at: Utc::now(),
        status: aura_core_auth::LinkStatus::Active,
        plan: None,
    }
}

fn tokens(access_token: &str) -> OAuthTokens {
    OAuthTokens {
        access_token: access_token.into(),
        refresh_token: None,
        id_token: None,
        scope: String::new(),
        expires_at: Utc::now(),
        issued_at: None,
    }
}

#[test]
fn authorization_url_with_callback() {
    let provider = OpenRouterProvider::production();
    let request = aura_auth_kit::new_attempt(
        CLIENT_ID.into(),
        "http://localhost:4321/callback".into(),
        None,
    );
    let url = url::Url::parse(&provider.authorization_url(&request).unwrap()).unwrap();
    let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(url.host_str(), Some("openrouter.ai"));
    assert_eq!(url.path(), "/auth");
    assert_eq!(query["callback_url"], "http://localhost:4321/callback");
    assert_eq!(query["state"], request.state);
    assert_eq!(query["code_challenge"], request.code_challenge);
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(query["key_label"], KEY_LABEL);
}

#[test]
fn authorization_url_headless_omits_callback_and_state() {
    let (_, query) = query(&OpenRouterProvider::production(), MANUAL_REDIRECT);
    assert!(!query.contains_key("callback_url"), "{query:?}");
    assert!(!query.contains_key("state"), "{query:?}");
    assert_eq!(query["code_challenge_method"], "S256");
    assert!(!query["code_challenge"].is_empty());
}

#[test]
fn api_base_override_derives_endpoints() {
    let p = OpenRouterProvider::for_api_base("http://127.0.0.1:9/proxy/api/").unwrap();
    assert_eq!(p.authorize_url, "http://127.0.0.1:9/proxy/auth");
    assert_eq!(p.keys_url, "http://127.0.0.1:9/proxy/api/v1/auth/keys");
    assert_eq!(p.key_info_url, "http://127.0.0.1:9/proxy/api/v1/key");
    let p = OpenRouterProvider::for_api_base(API_BASE).unwrap();
    assert_eq!(p.authorize_url, AUTHORIZE_URL);
    assert_eq!(p.keys_url, KEYS_URL);
    assert_eq!(p.key_info_url, KEY_INFO_URL);
    for unsafe_base in [
        "http://example.com/api",
        "not a url",
        "https://x.test/api?a=1",
    ] {
        assert!(
            OpenRouterProvider::for_api_base(unsafe_base).is_none(),
            "{unsafe_base}"
        );
    }
}

#[test]
fn fixture_requires_numeric_loopback_http() {
    assert!(OpenRouterProvider::fixture("http://127.0.0.1:1").is_ok());
    for base in [
        "https://127.0.0.1:1",
        "http://localhost:1",
        "http://example.com",
    ] {
        assert!(OpenRouterProvider::fixture(base).is_err(), "{base}");
    }
}

#[test]
fn key_info_tiers() {
    let tier = |v: Value| serde_json::from_value::<KeyInfo>(v).unwrap().tier();
    assert_eq!(
        tier(json!({"is_free_tier": true, "limit_remaining": 1.0})),
        "free"
    );
    assert_eq!(tier(json!({"limit_remaining": 4.5})), "$4.50 left");
    assert_eq!(tier(json!({"limit_remaining": null})), "credits");
}

#[test]
fn credential_carries_attribution_headers() {
    let credential = OpenRouterProvider::production().credential(&tokens(KEY));
    assert_eq!(credential.provider, ProviderKind::OpenRouter);
    assert_eq!(credential.bearer, KEY);
    assert_eq!(
        credential.headers,
        vec![
            ("HTTP-Referer".to_string(), APP_REFERER.to_string()),
            ("X-Title".to_string(), APP_TITLE.to_string()),
        ]
    );
    assert_eq!(credential.system_preamble, None);
}

#[test]
fn recognises_pasted_keys() {
    let provider = OpenRouterProvider::production();
    assert!(provider.is_api_key("  sk-or-v1-abc\n"));
    assert!(!provider.is_api_key("abc123"));
    assert!(!provider.is_api_key("sk-ant-oat-1"));
}

#[tokio::test]
async fn exchange_yields_key_without_refresh() {
    let (provider, seen) = serve().await;
    let request = aura_auth_kit::new_attempt(CLIENT_ID.into(), MANUAL_REDIRECT.into(), None);
    let callback = aura_auth_kit::parse_pasted_code("the-code", &request.state).unwrap();
    let grant = provider.exchange(&request, &callback).await.unwrap();

    let sent = seen.lock().unwrap()[0].clone();
    assert_eq!(sent["code"], "the-code");
    assert_eq!(sent["code_verifier"], request.code_verifier.as_str());
    assert_eq!(sent["code_challenge_method"], "S256");

    assert_eq!(grant.tokens.access_token, KEY);
    assert_eq!(grant.tokens.refresh_token, None);
    assert!(grant.tokens.expires_at > Utc::now() + Duration::days(3000));
    assert_eq!(grant.subject.as_deref(), Some("user-1"));
    assert_eq!(grant.label.as_deref(), Some("Aura key"));
    assert_eq!(grant.client_id, CLIENT_ID);
    assert!(grant.plan_enabled);

    let rendered = format!("{:?}", grant.tokens);
    assert!(!rendered.contains(KEY), "{rendered}");
    assert!(!format!("{grant:?}").contains(KEY));

    assert_eq!(
        provider.plan_tier(&grant.tokens).await.as_deref(),
        Some("$4.50 left")
    );
    assert_eq!(provider.plan_tier(&tokens("sk-or-v1-revoked")).await, None);
}

#[tokio::test]
async fn exchange_rejected_code() {
    let (provider, _) = serve().await;
    let request = aura_auth_kit::new_attempt(CLIENT_ID.into(), MANUAL_REDIRECT.into(), None);
    let callback = aura_auth_kit::parse_pasted_code("expired-code", &request.state).unwrap();
    let err = provider.exchange(&request, &callback).await.unwrap_err();
    assert!(
        matches!(
            &err,
            AuthError::Rejected { status: 403, code, .. } if code == "code_exchange_failed"
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn exchange_validates_callback_before_network() {
    let provider = OpenRouterProvider::production();
    let request = aura_auth_kit::new_attempt(CLIENT_ID.into(), MANUAL_REDIRECT.into(), None);
    let declined = CallbackParams {
        error: Some("access_denied".into()),
        ..CallbackParams::default()
    };
    let err = provider.exchange(&request, &declined).await.unwrap_err();
    assert!(matches!(err, AuthError::Declined(_)), "{err:?}");
    let oversized = CallbackParams {
        code: Some("x".repeat(MAX_SECRET_LEN + 1)),
        ..CallbackParams::default()
    };
    for callback in [CallbackParams::default(), oversized] {
        let err = provider.exchange(&request, &callback).await.unwrap_err();
        assert!(matches!(err, AuthError::Invalid(_)), "{err:?}");
    }
}

#[tokio::test]
async fn accept_api_key_checks_key() {
    let (provider, _) = serve().await;
    let grant = provider
        .accept_api_key(&format!("  {KEY}\n"))
        .await
        .unwrap();
    assert_eq!(grant.tokens.access_token, KEY);
    assert_eq!(grant.tokens.refresh_token, None);
    assert_eq!(grant.label.as_deref(), Some("Aura key"));
    assert_eq!(grant.client_id, CLIENT_ID);
    let subject = grant.subject.unwrap();
    assert_eq!(subject.len(), 16);
    assert!(subject.bytes().all(|b| b.is_ascii_hexdigit()));
    assert!(!KEY.contains(&subject));

    let err = provider
        .accept_api_key("sk-or-v1-revoked")
        .await
        .unwrap_err();
    assert!(
        matches!(
            &err,
            AuthError::Rejected { status: 401, code, .. } if code == "invalid_key"
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn accept_api_key_rejects_non_keys_before_network() {
    let provider = OpenRouterProvider::production();
    for pasted in ["the-code", "sk-ant-oat-1", "sk-or-has space", ""] {
        let err = provider.accept_api_key(pasted).await.unwrap_err();
        assert!(matches!(err, AuthError::Invalid(_)), "{pasted}: {err:?}");
    }
}

#[tokio::test]
async fn refresh_requires_new_sign_in() {
    let err = OpenRouterProvider::production()
        .refresh(&linked(), &tokens(KEY))
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::ReauthRequired(_)), "{err:?}");
}
