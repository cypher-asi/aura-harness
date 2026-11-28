//! The stub Runpod API and helpers shared by the tests.

use crate::runpod::{
    ApiKey, ApiKeySource, RunpodClient, RunpodOp, RunpodSettings, RunpodTool, SpendLimits,
    StaticApiKey,
};
use crate::tool::{Tool, ToolContext};
use crate::web::test_server::{Reply, Seen, TestServer};
use crate::{Sandbox, ToolConfig};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// The key the stub expects; tests check it never leaks.
pub(super) const KEY: &str = "rp_test_secret_key_0123456789";

pub(super) fn json_reply(status: u16, body: &Value) -> Reply {
    Reply {
        status,
        headers: vec![("Content-Type".into(), "application/json".into())],
        body: body.to_string().into_bytes(),
        delay: None,
    }
}

pub(super) fn ok(body: &Value) -> Reply {
    json_reply(200, body)
}

/// Settings pointing both APIs at `server`, with short waits.
pub(super) fn settings(server: &TestServer) -> RunpodSettings {
    RunpodSettings {
        api_base: server.url(""),
        serverless_base: server.url("/sl"),
        request_timeout: Duration::from_secs(2),
        connect_timeout: Duration::from_secs(1),
        job_wait: Duration::from_secs(1),
        log_read: Duration::from_millis(500),
        limits: SpendLimits::default(),
    }
}

pub(super) fn client_with(settings: RunpodSettings, key: Option<&str>) -> RunpodClient {
    let source: Arc<dyn ApiKeySource> = Arc::new(StaticApiKey(key.and_then(ApiKey::new)));
    RunpodClient::new(settings, source).unwrap()
}

pub(super) fn client(server: &TestServer) -> RunpodClient {
    client_with(settings(server), Some(KEY))
}

/// Run `op` with `args`: whether it succeeded, and what it printed.
pub(super) async fn call(client: &RunpodClient, op: RunpodOp, args: Value) -> (bool, String) {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(Sandbox::new(dir.path()).unwrap(), ToolConfig::default());
    let result = RunpodTool::new(op, client.clone())
        .execute(&ctx, args)
        .await
        .unwrap();
    let bytes = if result.ok {
        &result.stdout
    } else {
        &result.stderr
    };
    let text = String::from_utf8_lossy(bytes).into_owned();
    assert!(!text.contains(KEY), "the API key leaked into: {text}");
    (result.ok, text)
}

/// The output of a successful call, parsed.
pub(super) async fn call_ok(client: &RunpodClient, op: RunpodOp, args: Value) -> Value {
    let (ok, text) = call(client, op, args).await;
    assert!(ok, "{} failed: {text}", op.name());
    serde_json::from_str(&text).unwrap()
}

/// The error text of a call that must fail.
pub(super) async fn call_err(client: &RunpodClient, op: RunpodOp, args: Value) -> String {
    let (ok, text) = call(client, op, args).await;
    assert!(!ok, "{} should have failed: {text}", op.name());
    text
}

/// Requests made with `method` to `path` (query ignored).
pub(super) fn sent<'a>(seen: &'a [Seen], method: &str, path: &str) -> Vec<&'a Seen> {
    seen.iter()
        .filter(|s| s.method == method && s.path_and_query.split('?').next() == Some(path))
        .collect()
}

/// The catalog: a cheap RTX 4090, a dear H100 and a GPU with no price.
pub(super) fn catalog() -> Value {
    json!({"gpus": [
        {"id": "NVIDIA GeForce RTX 4090", "name": "RTX 4090", "pool": "ADA_24",
         "memory": 24, "price": {"secure": 0.44, "community": 0.31, "serverless": 1.1},
         "maxCount": {"secure": 8, "community": 4}, "availability": "HIGH",
         "dataCenters": [{"id": "US-KS-2"}]},
        {"id": "NVIDIA H100 80GB HBM3", "name": "H100", "memory": 80,
         "price": {"secure": 2.99, "community": 2.49}, "availability": "LOW"},
        {"id": "Mystery GPU", "name": "Mystery", "memory": 16, "price": {}}
    ]})
}

pub(super) fn pod(id: &str, status: &str, cost: f64) -> Value {
    json!({
        "id": id, "name": format!("pod-{id}"), "status": status, "cost": cost,
        "gpu": {"id": "NVIDIA GeForce RTX 4090", "count": 1, "vcpuCount": 16, "memory": 64},
        "cloud": "SECURE", "dataCenterId": "US-KS-2", "image": "runpod/pytorch:latest",
        "env": {"HF_TOKEN": "hf_secret_value"}, "ports": ["8888/http"],
        "actions": ["stop", "terminate"],
        "ssh": {"proxy": {"command": format!("ssh {id}@ssh.runpod.io")}},
        "runtime": {"uptime": 120}, "startedAt": "2026-06-01T12:02:00Z",
        "createdAt": "2026-06-01T12:00:00Z"
    })
}

/// `GET /v2/pods` listing `pods`.
pub(super) fn pods(pods: &[Value]) -> Reply {
    ok(&json!({"pods": pods, "pagination": {"nextCursor": null, "hasNextPage": false}}))
}

/// The bearer header every request must carry.
pub(super) fn assert_bearer(seen: &[Seen]) {
    assert!(!seen.is_empty());
    for request in seen {
        assert_eq!(
            request.header("authorization"),
            Some(format!("Bearer {KEY}").as_str()),
            "{} {}",
            request.method,
            request.path_and_query
        );
    }
}
