//! The Runpod adapter end to end through a [`Registry`], against a stub
//! Runpod API on loopback: no real Runpod calls, no real spending.

use super::Runpod;
use crate::{Access, IntegrationError, MemorySecrets, Registry, Secret, Status, ToolEffect};
use aura_exec_tools::Tool;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const KEY: &str = "rp_adapter_test_key_42";

/// One request the stub saw: `"METHOD /path"` and its auth header.
#[derive(Debug, Clone)]
struct Seen {
    route: String,
    auth: Option<String>,
}

/// Answers `"METHOD /path"` routes with `(status, json)`; 404 otherwise.
/// Every request needs `Bearer KEY`, else 401.
async fn stub(routes: Vec<(&str, u16, Value)>) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let routes: Arc<HashMap<String, (u16, Value)>> = Arc::new(
        routes
            .into_iter()
            .map(|(r, s, v)| (r.to_string(), (s, v)))
            .collect(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let routes = Arc::clone(&routes);
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                let request = read(&mut sock).await;
                let good = request.auth.as_deref() == Some(&format!("Bearer {KEY}"));
                let (status, body) = match routes.get(&request.route) {
                    _ if !good => (401, json!({"title": "Unauthorized", "status": 401})),
                    Some(reply) => reply.clone(),
                    None => (404, json!({"title": "Not Found", "status": 404})),
                };
                log.lock().unwrap().push(request);
                let body = body.to_string();
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(body.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (url, seen)
}

async fn read(sock: &mut tokio::net::TcpStream) -> Seen {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        let n = sock.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            break buf.len();
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
    };
    let head = String::from_utf8_lossy(&buf[..end]).to_string();
    let mut first = head
        .split("\r\n")
        .next()
        .unwrap_or_default()
        .split_whitespace();
    let method = first.next().unwrap_or_default();
    let path = first
        .next()
        .unwrap_or_default()
        .split('?')
        .next()
        .unwrap_or_default();
    let header = |name: &str| {
        head.split("\r\n")
            .filter_map(|l| l.split_once(':'))
            .find(|(n, _)| n.trim().eq_ignore_ascii_case(name))
            .map(|(_, v)| v.trim().to_string())
    };
    let length: usize = header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf.get(end + 4..).unwrap_or_default().to_vec();
    while body.len() < length {
        let n = sock.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Seen {
        route: format!("{method} {path}"),
        auth: header("authorization"),
    }
}

/// A registry holding only Runpod, pointed at `url`.
async fn registry(url: &str) -> Arc<Registry> {
    let mut registry =
        Registry::new(Arc::new(MemorySecrets::default())).with_timeout(Duration::from_secs(5));
    registry.add(Arc::new(Runpod::new())).unwrap();
    registry.configure("runpod", "api_base", url).await.unwrap();
    let serverless = format!("{url}/sl");
    registry
        .configure("runpod", "serverless_base", &serverless)
        .await
        .unwrap();
    Arc::new(registry)
}

fn account() -> (&'static str, u16, Value) {
    (
        "GET /v2/account",
        200,
        json!({"id": "user_2abc", "email": "d@x.io"}),
    )
}

const AGENT: (&str, Option<&str>) = ("agent-a", None);

#[test]
fn the_manifest_lists_ten_tools_with_their_effects_and_limits() {
    let runpod = Runpod::new();
    let manifest = crate::Integration::manifest(&runpod);
    assert_eq!(manifest.id.as_str(), "runpod");
    assert_eq!(manifest.tools.len(), 10);
    let effect = |name: &str| manifest.tool(name).unwrap().effect;
    for read in [
        "list_gpu_types",
        "list_pods",
        "get_pod",
        "pod_logs",
        "balance",
        "job_status",
    ] {
        assert_eq!(effect(read), ToolEffect::Read, "{read}");
    }
    for spend in ["create_pod", "pod_action", "run_job"] {
        assert_eq!(effect(spend), ToolEffect::Spends, "{spend}");
    }
    assert_eq!(effect("terminate_pod"), ToolEffect::Destroys);
    assert!(manifest.tool("run_job").unwrap().timeout.unwrap() > Duration::from_secs(60));
    assert_eq!(manifest.auth.key().unwrap().env, ["RUNPOD_API_KEY"]);
    let default = |key: &str| manifest.field(key).unwrap().default.clone().unwrap();
    assert_eq!(default("max_hourly_usd"), "1.00");
    assert_eq!(default("max_gpu_count"), "1");
    assert_eq!(default("max_running_pods"), "2");
    assert_eq!(default("max_job_seconds"), "300");
    assert!(manifest.field("api_base").unwrap().advanced);
    assert!(!manifest.field("max_hourly_usd").unwrap().advanced);
}

#[tokio::test]
async fn connect_refuses_a_bad_key_and_keeps_a_good_one() {
    let (url, _) = stub(vec![account()]).await;
    let registry = registry(&url).await;
    let refused = registry.connect("runpod", Secret::new("wrong")).await;
    assert!(
        matches!(refused, Err(IntegrationError::Http { status: 401, .. })),
        "{refused:?}"
    );
    let report = registry.connect("runpod", Secret::new(KEY)).await.unwrap();
    assert_eq!(report.summary, "account user_2abc");
    assert_eq!(report.detail.as_deref(), Some("d***@x.io"));
    assert!(registry.status("runpod").await.unwrap().is_connected());
}

#[tokio::test]
async fn agents_get_the_tools_and_calls_reach_runpod_with_the_key() {
    let pods = json!({"pods": [{"id": "p1", "name": "a", "status": "RUNNING", "cost": 0.44,
        "env": {"HF_TOKEN": "hf_secret"}}], "pagination": {"hasNextPage": false}});
    let (url, seen) = stub(vec![account(), ("GET /v2/pods", 200, pods)]).await;
    let registry = registry(&url).await;
    assert!(
        registry.tools_for(AGENT.0, None).await.is_empty(),
        "not connected yet"
    );
    registry.connect("runpod", Secret::new(KEY)).await.unwrap();

    let tools = registry.tools_for(AGENT.0, None).await;
    let names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();
    assert_eq!(names.len(), 10);
    assert!(names.contains(&"runpod_list_pods".to_string()));

    let answer = registry
        .call("runpod", "list_pods", json!({}), AGENT)
        .await
        .unwrap();
    assert_eq!(answer["pods"][0]["status"], "RUNNING");
    assert!(!answer.to_string().contains("hf_secret"));
    let seen = seen.lock().unwrap().clone();
    let list = seen.iter().find(|s| s.route == "GET /v2/pods").unwrap();
    assert_eq!(list.auth.as_deref(), Some(format!("Bearer {KEY}").as_str()));
}

#[tokio::test]
async fn the_spending_limit_holds_through_the_registry() {
    let catalog = json!({"gpus": [{"id": "H100", "price": {"secure": 2.99}}]});
    let (url, seen) = stub(vec![
        account(),
        ("GET /v2/catalog/gpus", 200, catalog),
        ("GET /v2/pods", 200, json!({"pods": []})),
        ("POST /v2/pods", 201, json!({"id": "never"})),
    ])
    .await;
    let registry = registry(&url).await;
    registry.connect("runpod", Secret::new(KEY)).await.unwrap();
    let args = json!({"name": "big", "gpu_type_id": "H100", "image": "x"});
    let refused = registry.call("runpod", "create_pod", args, AGENT).await;
    let message = refused.as_ref().unwrap_err().to_string();
    assert!(
        matches!(refused, Err(IntegrationError::Refused { .. })),
        "{refused:?}"
    );
    assert!(message.contains("spending limit"), "{message}");
    assert!(!message.contains(KEY));
    let posted = seen
        .lock()
        .unwrap()
        .iter()
        .any(|s| s.route == "POST /v2/pods");
    assert!(!posted, "nothing that spends was sent");
}

#[tokio::test]
async fn bad_limits_are_refused_or_shown_as_misconfigured_and_access_is_honoured() {
    let (url, _) = stub(vec![account()]).await;
    let registry = registry(&url).await;
    registry.connect("runpod", Secret::new(KEY)).await.unwrap();
    for (key, bad) in [
        ("max_hourly_usd", "lots"),
        ("max_gpu_count", "1.5"),
        ("max_job_seconds", "-3"),
    ] {
        assert!(
            registry.configure("runpod", key, bad).await.is_err(),
            "{key}={bad}"
        );
    }
    registry
        .configure("runpod", "max_gpu_count", "2")
        .await
        .unwrap();

    // A value edited into the settings file by hand still shows.
    let ctx = registry
        .context("runpod")
        .await
        .unwrap()
        .with_config([("max_hourly_usd".to_string(), "lots".to_string())].into());
    let status = crate::Integration::status(&Runpod::new(), &ctx).await;
    assert!(
        matches!(&status, Status::Misconfigured { why } if why.contains("max_hourly_usd")),
        "{status:?}"
    );

    registry
        .set_access(
            "runpod",
            Access::Only {
                agents: vec!["agent-b".to_string()],
                channels: Vec::new(),
            },
        )
        .await
        .unwrap();
    assert!(registry.tools_for(AGENT.0, None).await.is_empty());
    let denied = registry.call("runpod", "balance", json!({}), AGENT).await;
    assert!(
        matches!(denied, Err(IntegrationError::Denied { .. })),
        "{denied:?}"
    );
}

#[tokio::test]
async fn unknown_tools_and_a_missing_key_are_clear_errors() {
    let (url, _) = stub(vec![account()]).await;
    let registry = registry(&url).await;
    let runpod = Runpod::new();
    let ctx = registry.context("runpod").await.unwrap();
    let missing = crate::Integration::call_tool(&runpod, &ctx, "balance", json!({})).await;
    if std::env::var("RUNPOD_API_KEY").is_err() {
        assert!(
            matches!(missing, Err(IntegrationError::NotConnected { .. })),
            "{missing:?}"
        );
    }
    let unknown = crate::Integration::call_tool(&runpod, &ctx, "mine_bitcoin", json!({})).await;
    assert!(
        matches!(unknown, Err(IntegrationError::UnknownTool { .. })),
        "{unknown:?}"
    );
}
