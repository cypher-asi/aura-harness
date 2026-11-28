//! The read tools: requests sent and output shaped and bounded.

use super::support::*;
use crate::runpod::RunpodOp;
use crate::web::test_server::{Reply, TestServer};
use serde_json::json;

#[tokio::test]
async fn list_gpu_types_sends_the_filters_and_keeps_the_useful_fields() {
    let server = TestServer::start(vec![("GET /v2/catalog/gpus", ok(&catalog()))]).await;
    let out = call_ok(
        &client(&server),
        RunpodOp::ListGpuTypes,
        json!({"cloud": "secure", "gpu_count": 2, "min_cuda_version": "12.4"}),
    )
    .await;

    let seen = server.requests();
    assert_bearer(&seen);
    let query = &seen[0].path_and_query;
    for part in ["cloud=SECURE", "count=2", "minCudaVersion=12.4"] {
        assert!(query.contains(part), "{query}");
    }
    let first = &out["gpus"][0];
    assert_eq!(first["id"], "NVIDIA GeForce RTX 4090");
    assert_eq!(first["price_secure_usd_hour"], 0.44);
    assert_eq!(first["max_count_community"], 4);
    assert!(first.get("pool").is_none(), "unneeded fields are dropped");
    assert_eq!(out["gpus"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn long_lists_are_cut_to_the_limit_with_the_total() {
    let gpus: Vec<_> = (0..200)
        .map(|i| json!({"id": format!("gpu-{i}"), "price": {"secure": 1.0}}))
        .collect();
    let server = TestServer::start(vec![("/v2/catalog/gpus", ok(&json!({"gpus": gpus})))]).await;
    let client = client(&server);

    let out = call_ok(&client, RunpodOp::ListGpuTypes, json!({})).await;
    assert_eq!(out["gpus"].as_array().unwrap().len(), 20);
    assert_eq!(out["total"], 200);

    let out = call_ok(&client, RunpodOp::ListGpuTypes, json!({"limit": 5000})).await;
    assert_eq!(out["gpus"].as_array().unwrap().len(), 100);
}

#[tokio::test]
async fn list_pods_shows_status_and_cost_but_not_env_values() {
    let listing = ok(&json!({
        "pods": [pod("p1", "RUNNING", 0.44), pod("p2", "EXITED", 0.31)],
        "pagination": {"nextCursor": "c2", "hasNextPage": true}
    }));
    let server = TestServer::start(vec![("GET /v2/pods", listing)]).await;
    let (ok, text) = call(
        &client(&server),
        RunpodOp::ListPods,
        json!({"limit": 5, "cursor": "c1"}),
    )
    .await;
    assert!(ok, "{text}");
    assert!(!text.contains("hf_secret_value"));

    let out: serde_json::Value = serde_json::from_str(&text).unwrap();
    let query = &server.requests()[0].path_and_query;
    assert!(
        query.contains("limit=5") && query.contains("cursor=c1"),
        "{query}"
    );
    assert_eq!(out["pods"][0]["status"], "RUNNING");
    assert_eq!(out["pods"][0]["cost_usd_hour"], 0.44);
    assert_eq!(out["pods"][0]["uptime_seconds"], 120);
    assert_eq!(out["next_cursor"], "c2");
}

#[tokio::test]
async fn get_pod_names_env_variables_without_their_values() {
    let server =
        TestServer::start(vec![("GET /v2/pods/p1", ok(&pod("p1", "RUNNING", 0.44)))]).await;
    let (ok, text) = call(&client(&server), RunpodOp::GetPod, json!({"pod_id": "p1"})).await;
    assert!(ok, "{text}");
    assert!(!text.contains("hf_secret_value"));
    let out: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(out["env_names"], json!(["HF_TOKEN"]));
    assert_eq!(out["ssh_command"], "ssh p1@ssh.runpod.io");
    assert_eq!(out["image"], "runpod/pytorch:latest");
}

fn sse(lines: usize) -> Reply {
    let body: String = (0..lines)
        .map(|i| {
            format!(
                "id: t/{i}\ndata: {}\n\n",
                json!({"ts": "2026-06-01T12:00:00Z", "source": "container", "line": format!("line {i}")})
            )
        })
        .collect();
    Reply::ok("text/event-stream", body)
}

#[tokio::test]
async fn pod_logs_reads_the_stream_and_keeps_the_last_lines() {
    let server = TestServer::start(vec![("GET /v2/pods/p1/logs", sse(3))]).await;
    let out = call_ok(
        &client(&server),
        RunpodOp::PodLogs,
        json!({"pod_id": "p1", "lines": 2}),
    )
    .await;
    let query = &server.requests()[0].path_and_query;
    assert!(
        query.contains("source=container") && query.contains("tail=2"),
        "{query}"
    );
    assert_eq!(out["line_count"], 2);
    assert_eq!(
        out["logs"],
        "2026-06-01T12:00:00Z line 1\n2026-06-01T12:00:00Z line 2"
    );
}

#[tokio::test]
async fn a_long_log_stream_is_bounded() {
    let server = TestServer::start(vec![("/v2/pods/p1/logs", sse(5000))]).await;
    let (ok, text) = call(&client(&server), RunpodOp::PodLogs, json!({"pod_id": "p1"})).await;
    assert!(ok, "{text}");
    assert!(text.len() <= crate::runpod::output::MAX_OUTPUT_BYTES);
    let out: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(out["line_count"], 100, "the default is the last 100 lines");
}

#[tokio::test]
async fn balance_and_job_status_read_their_endpoints() {
    let server = TestServer::start(vec![
        (
            "GET /v2/billing/balance",
            ok(&json!({"balanceAmount": 42.18, "billingMode": "PREPAID"})),
        ),
        (
            "GET /sl/v2/ep1/status/job-1",
            ok(
                &json!({"id": "job-1", "status": "COMPLETED", "output": {"text": "hi"},
                       "executionTime": 812}),
            ),
        ),
    ])
    .await;
    let client = client(&server);

    let balance = call_ok(&client, RunpodOp::Balance, json!({})).await;
    assert_eq!(balance["balance_usd"], 42.18);

    let job = call_ok(
        &client,
        RunpodOp::JobStatus,
        json!({"endpoint_id": "ep1", "job_id": "job-1"}),
    )
    .await;
    assert_eq!(job["status"], "COMPLETED");
    assert_eq!(job["output"]["text"], "hi");
    assert_eq!(job["execution_ms"], 812);
    assert_bearer(&server.requests());
}

#[tokio::test]
async fn ids_that_could_leave_their_path_are_refused_before_sending() {
    let server = TestServer::start(vec![]).await;
    let client = client(&server);
    for bad in ["../account", "p1/logs", "p 1", ""] {
        let err = call_err(&client, RunpodOp::GetPod, json!({"pod_id": bad})).await;
        assert!(err.contains("pod_id"), "{err}");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn oversized_output_is_trimmed_with_a_note() {
    let big = json!({"text": "x".repeat(100_000)});
    let out = crate::runpod::output::render(&big);
    assert!(out.len() < crate::runpod::output::MAX_OUTPUT_BYTES + 100);
    assert!(out.contains("[output trimmed"));
}
