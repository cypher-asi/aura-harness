use super::*;
use axum::extract::{Form, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const PEM: &[u8] = include_bytes!("fixtures/test_rsa.pem");
const JWKS: &str = include_str!("fixtures/test_jwks.json");
const ISSUED: &str = "issued-client-1";

#[derive(Default)]
struct Fixture {
    base: String,
    nonce: String,
    subject: String,
    forms: Vec<HashMap<String, String>>,
}

type Shared = Arc<Mutex<Fixture>>;

fn id_token(fixture: &Fixture, nonce: Option<&str>) -> String {
    let mut header = Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("test-key".into());
    let mut claims = json!({
        "iss": fixture.base,
        "aud": ISSUED,
        "sub": fixture.subject,
        "email": "alice@example.com",
        "exp": Utc::now().timestamp() + 600,
    });
    if let Some(nonce) = nonce {
        claims["nonce"] = json!(nonce);
    }
    encode(&header, &claims, &EncodingKey::from_rsa_pem(PEM).unwrap()).unwrap()
}

async fn metadata(State(s): State<Shared>) -> Json<Value> {
    let base = s.lock().unwrap().base.clone();
    Json(json!({
        "issuer": base,
        "jwks_uri": format!("{base}/jwks"),
        "revocation_endpoint": format!("{base}/revoke"),
    }))
}

async fn jwks() -> Json<Value> {
    Json(serde_json::from_str(JWKS).unwrap())
}

async fn token(
    State(s): State<Shared>,
    Form(form): Form<HashMap<String, String>>,
) -> (axum::http::StatusCode, Json<Value>) {
    let mut fixture = s.lock().unwrap();
    fixture.forms.push(form.clone());
    match form["grant_type"].as_str() {
        "authorization_code" => {
            let nonce = fixture.nonce.clone();
            (
                axum::http::StatusCode::OK,
                Json(json!({
                    "access_token": "access-1",
                    "refresh_token": "refresh-1",
                    "id_token": id_token(&fixture, Some(&nonce)),
                    "scope": SCOPE,
                    "token_type": "Bearer",
                    "expires_in": 3600,
                })),
            )
        }
        _ if form["refresh_token"] == "revoked" => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant"})),
        ),
        _ if form["refresh_token"] == "flaky" => {
            (axum::http::StatusCode::SERVICE_UNAVAILABLE, Json(json!({})))
        }
        _ => (
            axum::http::StatusCode::OK,
            Json(json!({
                "access_token": "access-2",
                "refresh_token": "refresh-2",
                "id_token": id_token(&fixture, None),
                "token_type": "bearer",
                "expires_in": 3600,
            })),
        ),
    }
}

async fn serve() -> (ChatgptProvider, Shared) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let shared: Shared = Arc::new(Mutex::new(Fixture {
        base: base.clone(),
        subject: "user-sub".into(),
        ..Fixture::default()
    }));
    let app = Router::new()
        .route("/.well-known/openid-configuration", get(metadata))
        .route("/jwks", get(jwks))
        .route("/api/accounts/oauth/token", post(token))
        .with_state(shared.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (ChatgptProvider::fixture(&base).unwrap(), shared)
}

fn account(provider: &ChatgptProvider, subject: &str) -> LinkedAccount {
    LinkedAccount {
        id: "link-1".into(),
        provider: provider.kind(),
        owner: aura_core_auth::Owner::Local,
        subject: subject.into(),
        label: "alice@example.com".into(),
        client_id: ISSUED.into(),
        plan_enabled: true,
        expires_at: Utc::now(),
        created_at: Utc::now(),
        status: aura_core_auth::LinkStatus::Active,
    }
}

fn tokens(refresh: &str) -> OAuthTokens {
    OAuthTokens {
        access_token: "old".into(),
        refresh_token: Some(refresh.into()),
        id_token: None,
        scope: SCOPE.into(),
        expires_at: Utc::now(),
    }
}

fn callback(request: &AuthorizationRequest, client: Option<&str>) -> CallbackParams {
    CallbackParams {
        code: Some("code-1".into()),
        state: request.state.clone(),
        client_id: client.map(str::to_owned),
        error: None,
    }
}

#[test]
fn fixture_rejects_non_loopback() {
    assert!(ChatgptProvider::fixture("https://auth.openai.com").is_err());
    assert!(ChatgptProvider::fixture("http://localhost:1").is_err());
}

#[test]
fn authorization_url_carries_pkce_and_host() {
    let provider = ChatgptProvider::new("urn:uuid:host".into());
    let request = aura_auth_kit::new_attempt(
        provider.client_id(None),
        "http://127.0.0.1:5/auth/openai/callback".into(),
        None,
    );
    let url = url::Url::parse(&provider.authorization_url(&request).unwrap()).unwrap();
    let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(url.path(), "/api/accounts/authorize");
    assert_eq!(query["client_id"], DYNAMIC_CLIENT);
    assert_eq!(query["ext_agent_host_id"], "urn:uuid:host");
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(query["code_challenge"], request.code_challenge);
    assert_eq!(query["agent_name_hint"], "Aura");
    assert!(!query.contains_key("code_verifier"));
}

#[tokio::test]
async fn exchange_verifies_identity_and_plan() {
    let (provider, shared) = serve().await;
    let request = aura_auth_kit::new_attempt(
        provider.client_id(None),
        "http://127.0.0.1:5/auth/openai/callback".into(),
        None,
    );
    shared.lock().unwrap().nonce = request.nonce.clone();
    let grant = provider
        .exchange(&request, &callback(&request, Some(ISSUED)))
        .await
        .unwrap();
    assert_eq!(grant.client_id, ISSUED);
    assert_eq!(grant.subject.as_deref(), Some("user-sub"));
    assert_eq!(grant.label.as_deref(), Some("alice@example.com"));
    assert!(grant.plan_enabled);
    let form = shared.lock().unwrap().forms[0].clone();
    assert_eq!(form["client_id"], ISSUED);
    assert_eq!(form["code_verifier"], request.code_verifier);
}

#[tokio::test]
async fn exchange_rejects_wrong_nonce_and_placeholder_client() {
    let (provider, shared) = serve().await;
    let request = aura_auth_kit::new_attempt(provider.client_id(None), "http://x".into(), None);
    shared.lock().unwrap().nonce = "other".into();
    let err = provider
        .exchange(&request, &callback(&request, Some(ISSUED)))
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::Invalid(_)), "{err:?}");
    let err = provider
        .exchange(&request, &callback(&request, None))
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::Invalid(_)), "{err:?}");
}

#[tokio::test]
async fn refresh_rotates_and_classifies_failures() {
    let (provider, _shared) = serve().await;
    let grant = provider
        .refresh(&account(&provider, "user-sub"), &tokens("refresh-1"))
        .await
        .unwrap();
    assert_eq!(grant.tokens.access_token, "access-2");
    assert_eq!(grant.tokens.refresh_token.as_deref(), Some("refresh-2"));
    assert_eq!(grant.tokens.scope, SCOPE, "scope carried over when omitted");

    let err = provider
        .refresh(&account(&provider, "someone-else"), &tokens("refresh-1"))
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::ReauthRequired(_)), "{err:?}");
    let err = provider
        .refresh(&account(&provider, "user-sub"), &tokens("revoked"))
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::ReauthRequired(_)), "{err:?}");
    let err = provider
        .refresh(&account(&provider, "user-sub"), &tokens("flaky"))
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::Unavailable(_)), "{err:?}");
}
