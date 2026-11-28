//! GitHub and boxd listed, checked and connected through the registry,
//! against stand-ins for `gh` and for their HTTP APIs.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use aura_engine_integrations::builtin::{builtin, BuiltinOptions, GitHub};
use aura_engine_integrations::{
    AuthKind, Integration, IntegrationError, MemorySecrets, Registry, Secret, Status,
};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn registry(integrations: Vec<Arc<dyn Integration>>) -> Registry {
    let mut registry = Registry::new(Arc::new(MemorySecrets::default()));
    for integration in integrations {
        registry.add(integration).unwrap();
    }
    registry
}

/// A server answering every request with `status` and `body`; its URL.
async fn serve(status: u16, body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = vec![0u8; 8192];
            let _ = socket.read(&mut buf).await;
            let reply = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(reply.as_bytes()).await;
        }
    });
    url
}

#[test]
fn github_boxd_and_runpod_are_listed_through_the_registry() {
    let registry = registry(builtin(BuiltinOptions::default()));
    let listed: Vec<(&str, &str)> = registry
        .list()
        .iter()
        .map(|m| (m.id.as_str(), m.name.as_str()))
        .collect();
    assert_eq!(
        listed,
        [("github", "GitHub"), ("boxd", "boxd"), ("runpod", "Runpod")]
    );
    let github = registry.get("github").unwrap().manifest();
    assert!(matches!(&github.auth, AuthKind::CliLogin { login, .. } if login == "gh auth login"));
    assert_eq!(github.auth.key().unwrap().env, ["GH_TOKEN", "GITHUB_TOKEN"]);
    let boxd = registry.get("boxd").unwrap().manifest();
    assert_eq!(boxd.auth.key().unwrap().env, ["BOXD_TOKEN", "BOXD_API_KEY"]);
    assert!(
        github.tools.is_empty() && boxd.tools.is_empty(),
        "no agent tools yet"
    );
}

#[tokio::test]
async fn github_connects_with_a_token_that_its_api_accepts() {
    let registry = registry(vec![Arc::new(GitHub::with_gh("/nonexistent/gh", false))]);
    let api = serve(200, r#"{"login":"octocat"}"#).await;
    registry.configure("github", "api_url", &api).await.unwrap();
    let report = registry
        .connect("github", Secret::new("ghp_test"))
        .await
        .unwrap();
    assert_eq!(report.summary, "signed in as octocat");
    assert!(registry.status("github").await.unwrap().is_connected());
}

#[tokio::test]
async fn github_refuses_a_token_its_api_rejects_and_stores_nothing() {
    let registry = registry(vec![Arc::new(GitHub::with_gh("/nonexistent/gh", false))]);
    let api = serve(401, r#"{"message":"Bad credentials"}"#).await;
    registry.configure("github", "api_url", &api).await.unwrap();
    let err = registry
        .connect("github", Secret::new("ghp_bad"))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, IntegrationError::Http { status: 401, message, .. } if message == "Bad credentials")
    );
    assert!(!err.to_string().contains("ghp_bad"), "{err}");
    assert!(!registry.views().await[0].stored);
}

#[cfg(unix)]
#[tokio::test]
async fn github_reads_a_gh_login_and_names_the_github_app() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let gh = dir.path().join("gh");
    std::fs::write(
        &gh,
        "#!/bin/sh\nif [ \"$1\" = api ]; then echo octocat; fi\nexit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let github = GitHub::with_gh(gh.to_str().unwrap(), true);
    let registry = registry(vec![Arc::new(github)]);
    if std::env::var_os("GH_TOKEN").is_some() || std::env::var_os("GITHUB_TOKEN").is_some() {
        return; // an env token wins over gh; not this test's case
    }
    assert_eq!(
        registry.status("github").await.unwrap(),
        Status::Connected {
            via: "gh login; GitHub App set for cloud pushes".to_string()
        }
    );
    assert_eq!(
        registry.test("github").await.unwrap().summary,
        "gh signed in as octocat"
    );
}

#[tokio::test]
async fn github_without_gh_or_a_token_says_how_to_connect() {
    if std::env::var_os("GH_TOKEN").is_some() || std::env::var_os("GITHUB_TOKEN").is_some() {
        return;
    }
    let registry = registry(vec![Arc::new(GitHub::with_gh("/nonexistent/gh", false))]);
    assert_eq!(
        registry.status("github").await.unwrap(),
        Status::NotConnected {
            hint: "run gh auth login, or add a token".to_string()
        }
    );
}

#[tokio::test]
async fn boxd_exchanges_an_api_key_before_storing_it() {
    let registry = registry(builtin(BuiltinOptions::default()));
    let console = serve(200, r#"{"token":"jwt","expires_at":1}"#).await;
    registry
        .configure("boxd", "console_url", &console)
        .await
        .unwrap();
    let report = registry
        .connect("boxd", Secret::new("bxd_live_key"))
        .await
        .unwrap();
    assert_eq!(report.summary, "API key accepted");
    let view = registry.views().await.remove(1);
    assert!(view.stored && view.status.is_connected());
}

#[tokio::test]
async fn boxd_refuses_a_key_the_console_rejects() {
    let registry = registry(builtin(BuiltinOptions::default()));
    let console = serve(401, r#"{"error":"invalid api key"}"#).await;
    registry
        .configure("boxd", "console_url", &console)
        .await
        .unwrap();
    let err = registry
        .connect("boxd", Secret::new("bxd_wrong"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("invalid api key"), "{err}");
    assert!(!err.to_string().contains("bxd_wrong"), "{err}");
    assert!(!registry.views().await[1].stored);
}
