use super::*;
use aura_config::GithubAppConfig;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

const PEM: &[u8] = include_bytes!("../fixtures/test_rsa.pem");
const JWKS: &str = include_str!("../fixtures/test_jwks.json");

#[test]
fn remotes_on_github_parse_and_others_do_not() {
    let repo = |owner: &str, name: &str| {
        Some(RepoRef {
            owner: owner.into(),
            name: name.into(),
        })
    };
    for url in [
        "https://github.com/cypher-asi/aura-harness.git",
        "https://github.com/cypher-asi/aura-harness",
        "https://github.com/cypher-asi/aura-harness/",
        "https://x-access-token:ghs_secret@github.com/cypher-asi/aura-harness.git",
        "ssh://git@github.com/cypher-asi/aura-harness.git",
        "ssh://git@github.com:22/cypher-asi/aura-harness.git",
        "git@github.com:cypher-asi/aura-harness.git",
        "github.com:cypher-asi/aura-harness",
        "  git@GitHub.com:cypher-asi/aura-harness.git\n",
    ] {
        assert_eq!(
            RepoRef::from_remote(url),
            repo("cypher-asi", "aura-harness"),
            "{url}"
        );
    }
    assert_eq!(
        RepoRef::from_remote("https://github.com/a/b.c.d.git"),
        repo("a", "b.c.d")
    );
    for url in [
        "https://gitlab.com/a/b.git",
        "https://github.com.evil.example/a/b.git",
        "git@gitlab.com:a/b.git",
        "https://github.com/a",
        "https://github.com/a/b/c",
        "https://github.com/../b",
        "file:///srv/a/b.git",
        "/srv/repos/a.git",
        "",
    ] {
        assert_eq!(RepoRef::from_remote(url), None, "{url}");
    }
    let parsed = repo("o", "r").expect("repo");
    assert_eq!(parsed.full_name(), "o/r");
    assert_eq!(parsed.https_url(), "https://github.com/o/r.git");
}

#[test]
fn the_bot_identity_is_the_apps_noreply_account() {
    let bot = BotIdentity {
        slug: "aura-agents".into(),
        account_id: Some(123),
    };
    assert_eq!(bot.login(), "aura-agents[bot]");
    assert_eq!(bot.email(), "123+aura-agents[bot]@users.noreply.github.com");
    assert_eq!(
        bot.install_url(),
        "https://github.com/apps/aura-agents/installations/new"
    );
}

#[test]
fn tokens_never_print() {
    let token = RepoToken {
        token: Zeroizing::new("ghs_supersecret".into()),
        expires_at_ms: 1,
    };
    assert!(!format!("{token:?}").contains("ghs_"));
}

/// What the mock GitHub saw and serves.
#[derive(Default)]
struct Mock {
    installed: HashMap<String, u64>,
    jwts: Vec<String>,
    token_requests: Vec<Value>,
}

type Shared = Arc<parking_lot::Mutex<Mock>>;

fn bearer(headers: &HeaderMap) -> String {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_string()
}

async fn installation(
    State(mock): State<Shared>,
    Path((owner, repo)): Path<(String, String)>,
    headers: HeaderMap,
) -> (StatusCode, Json<Value>) {
    let mut mock = mock.lock();
    mock.jwts.push(bearer(&headers));
    match mock.installed.get(&format!("{owner}/{repo}")) {
        Some(id) => (StatusCode::OK, Json(json!({ "id": id }))),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "message": "Not Found" })),
        ),
    }
}

async fn access_token(
    State(mock): State<Shared>,
    Path(id): Path<u64>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let mut mock = mock.lock();
    mock.jwts.push(bearer(&headers));
    mock.token_requests.push(body);
    if !mock.installed.values().any(|known| *known == id) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "message": "Not Found" })),
        );
    }
    (
        StatusCode::CREATED,
        Json(json!({
            "token": format!("ghs_minted_{id}"),
            "expires_at": "2030-01-01T00:00:00Z",
        })),
    )
}

async fn app(State(mock): State<Shared>, headers: HeaderMap) -> Json<Value> {
    mock.lock().jwts.push(bearer(&headers));
    Json(json!({ "id": 7, "slug": "aura-agents", "name": "Aura agents" }))
}

async fn bot_account(Path(login): Path<String>) -> (StatusCode, Json<Value>) {
    if login == "aura-agents[bot]" {
        (StatusCode::OK, Json(json!({ "id": 4242, "login": login })))
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "message": "Not Found" })),
        )
    }
}

async fn serve(mock: Shared) -> String {
    let router = Router::new()
        .route("/repos/{owner}/{repo}/installation", get(installation))
        .route("/app/installations/{id}/access_tokens", post(access_token))
        .route("/app", get(app))
        .route("/users/{login}", get(bot_account))
        .with_state(mock);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    format!("http://{addr}")
}

fn app_config(dir: &std::path::Path, workflows: bool) -> GithubAppConfig {
    let key = dir.join("app.pem");
    std::fs::write(&key, PEM).expect("write key");
    GithubAppConfig {
        app_id: "Iv23liTestApp".into(),
        private_key_path: key,
        workflows,
    }
}

fn verify(jwt: &str) -> Value {
    let jwks: Value = serde_json::from_str(JWKS).expect("jwks");
    let jwk: jsonwebtoken::jwk::Jwk = serde_json::from_value(jwks["keys"][0].clone()).expect("jwk");
    // `iss` is checked by each test: jsonwebtoken counts a numeric
    // `iss` (an app id, which GitHub accepts) as missing.
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_required_spec_claims(&["exp", "iat"]);
    let claims =
        jsonwebtoken::decode::<Value>(jwt, &DecodingKey::from_jwk(&jwk).expect("key"), &validation)
            .expect("the App JWT verifies with the App's public key")
            .claims;
    assert!(
        claims["iss"].is_string() || claims["iss"].is_u64(),
        "{claims}"
    );
    claims
}

#[tokio::test]
async fn a_token_is_minted_for_one_repo_with_narrowed_permissions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mock: Shared = Arc::default();
    mock.lock().installed.insert("acme/app".into(), 99);
    let base = serve(Arc::clone(&mock)).await;
    let github = GithubApp::with_api(app_config(dir.path(), false), base).expect("app");
    let repo = RepoRef::from_remote("git@github.com:acme/app.git").expect("repo");

    let token = github.mint(&repo).await.expect("mint");
    assert_eq!(token.token.as_str(), "ghs_minted_99");
    assert_eq!(token.expires_at_ms, 1_893_456_000_000);
    github.mint(&repo).await.expect("mint again");

    let mock = mock.lock();
    assert_eq!(
        mock.token_requests[0],
        json!({
            "repositories": ["app"],
            "permissions": {"contents": "write", "pull_requests": "write"},
        })
    );
    // The installation was looked up once, then cached: one lookup,
    // two mints, each with a JWT of its own.
    assert_eq!(mock.jwts.len(), 3);
    let claims = verify(&mock.jwts[0]);
    assert_eq!(claims["iss"], "Iv23liTestApp");
    let now = chrono::Utc::now().timestamp();
    let iat = claims["iat"].as_i64().expect("iat");
    let exp = claims["exp"].as_i64().expect("exp");
    assert!((now - 70..=now - 50).contains(&iat), "iat {iat} now {now}");
    assert_eq!(exp - iat, JWT_BACKDATE_SECS + JWT_LIFETIME_SECS);
    assert!(exp - now <= 600, "GitHub refuses JWTs over ten minutes");
}

#[tokio::test]
async fn workflows_are_granted_only_when_configured_and_app_ids_are_numbers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mock: Shared = Arc::default();
    mock.lock().installed.insert("acme/app".into(), 5);
    let base = serve(Arc::clone(&mock)).await;
    let mut config = app_config(dir.path(), true);
    config.app_id = "12345".into();
    let github = GithubApp::with_api(config, base).expect("app");
    let repo = RepoRef::from_remote("https://github.com/acme/app").expect("repo");
    github.mint(&repo).await.expect("mint");
    let mock = mock.lock();
    assert_eq!(
        mock.token_requests[0]["permissions"],
        json!({"contents": "write", "pull_requests": "write", "workflows": "write"})
    );
    assert_eq!(verify(&mock.jwts[0])["iss"], json!(12345));
}

#[tokio::test]
async fn a_repo_without_the_app_gets_the_install_link() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mock: Shared = Arc::default();
    let base = serve(Arc::clone(&mock)).await;
    let github = GithubApp::with_api(app_config(dir.path(), false), base).expect("app");
    let repo = RepoRef::from_remote("https://github.com/acme/private").expect("repo");
    let err = github.mint(&repo).await.expect_err("not installed");
    assert_eq!(
        err,
        GithubError::NotInstalled {
            repo: "acme/private".into(),
            install_url: Some("https://github.com/apps/aura-agents/installations/new".into()),
        }
    );
    assert!(err.to_string().contains("installations/new"), "{err}");
    assert!(mock.lock().token_requests.is_empty());

    let bot = github.identity().await.expect("identity");
    assert_eq!(bot.login(), "aura-agents[bot]");
    assert_eq!(
        bot.email(),
        "4242+aura-agents[bot]@users.noreply.github.com"
    );
}

#[tokio::test]
async fn an_unreadable_key_is_a_key_error_that_never_holds_the_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = app_config(dir.path(), false);
    std::fs::write(&config.private_key_path, b"not a pem").expect("write");
    let github = GithubApp::with_api(config.clone(), "http://127.0.0.1:9").expect("app");
    let repo = RepoRef::from_remote("https://github.com/acme/app").expect("repo");
    let err = github.mint(&repo).await.expect_err("bad key");
    assert!(matches!(err, GithubError::Key { .. }), "{err}");
    config.private_key_path = dir.path().join("missing.pem");
    let github = GithubApp::with_api(config, "http://127.0.0.1:9").expect("app");
    let err = github.mint(&repo).await.expect_err("missing key");
    assert!(err.to_string().contains("could not read"), "{err}");
    assert!(!format!("{github:?}").contains("PRIVATE"));
}
