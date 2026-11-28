//! The tools that spend or destroy, and the guardrail in front of them.

use super::support::*;
use crate::runpod::{RunpodOp, SpendLimits};
use crate::web::test_server::{Reply, TestServer};
use serde_json::{json, Value};

fn create_args(gpu: &str, count: u64) -> Value {
    json!({"name": "trainer", "gpu_type_id": gpu, "gpu_count": count,
           "image": "runpod/pytorch:latest", "ports": ["8888/http"], "env": {"A": "1"}})
}

/// A stub with the catalog, `running` pods already up, and a create route.
async fn pod_server(running: usize) -> TestServer {
    let listed: Vec<Value> = (0..running)
        .map(|i| pod(&format!("r{i}"), "RUNNING", 0.44))
        .chain([pod("x", "EXITED", 0.44)])
        .collect();
    TestServer::start(vec![
        ("GET /v2/catalog/gpus", ok(&catalog())),
        ("GET /v2/pods", pods(&listed)),
        (
            "POST /v2/pods",
            json_reply(201, &pod("new1", "RUNNING", 0.44)),
        ),
    ])
    .await
}

#[tokio::test]
async fn create_pod_within_the_limits_sends_the_pod() {
    let server = pod_server(1).await;
    let out = call_ok(
        &client(&server),
        RunpodOp::CreatePod,
        create_args("NVIDIA GeForce RTX 4090", 1),
    )
    .await;

    let seen = server.requests();
    assert_bearer(&seen);
    let posts = sent(&seen, "POST", "/v2/pods");
    assert_eq!(posts.len(), 1);
    let body: Value = serde_json::from_str(&posts[0].body).unwrap();
    assert_eq!(body["name"], "trainer");
    assert_eq!(
        body["gpu"],
        json!({"id": "NVIDIA GeForce RTX 4090", "count": 1})
    );
    assert_eq!(body["cloud"], "SECURE");
    assert_eq!(body["image"], "runpod/pytorch:latest");
    assert_eq!(body["ports"], json!(["8888/http"]));
    assert_eq!(out["pod"]["id"], "new1");
    assert_eq!(out["expected_cost_usd_hour"], 0.44);
}

/// Refused with `needle` in the message, and nothing created.
async fn assert_create_refused(
    server: &TestServer,
    limits: SpendLimits,
    args: Value,
    needle: &str,
) {
    let mut settings = settings(server);
    settings.limits = limits;
    let client = client_with(settings, Some(KEY));
    let err = call_err(&client, RunpodOp::CreatePod, args).await;
    assert!(err.contains(needle), "{err}");
    assert!(sent(&server.requests(), "POST", "/v2/pods").is_empty());
}

#[tokio::test]
async fn create_pod_over_the_hourly_price_is_refused() {
    let server = pod_server(0).await;
    let args = create_args("NVIDIA H100 80GB HBM3", 1);
    assert_create_refused(&server, SpendLimits::default(), args, "max_hourly_usd").await;
}

#[tokio::test]
async fn the_price_is_counted_per_gpu() {
    let server = pod_server(0).await;
    let limits = SpendLimits {
        max_gpu_count: 4,
        ..SpendLimits::default()
    };
    // 3 x $0.44 = $1.32 > $1.00.
    let args = create_args("NVIDIA GeForce RTX 4090", 3);
    assert_create_refused(&server, limits, args, "$1.32/hour").await;
}

#[tokio::test]
async fn create_pod_over_the_gpu_count_is_refused_before_any_request() {
    let server = pod_server(0).await;
    let args = create_args("NVIDIA GeForce RTX 4090", 2);
    assert_create_refused(&server, SpendLimits::default(), args, "max_gpu_count").await;
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn create_pod_over_the_running_pods_is_refused() {
    let server = pod_server(2).await;
    let args = create_args("NVIDIA GeForce RTX 4090", 1);
    assert_create_refused(&server, SpendLimits::default(), args, "max_running_pods").await;
}

#[tokio::test]
async fn an_unknown_price_is_refused_not_guessed() {
    let server = pod_server(0).await;
    for gpu in ["Mystery GPU", "Not In The Catalog"] {
        let args = create_args(gpu, 1);
        assert_create_refused(&server, SpendLimits::default(), args, "no hourly price").await;
    }
}

#[tokio::test]
async fn create_pod_needs_exactly_one_of_image_or_template() {
    let server = pod_server(0).await;
    let client = client(&server);
    let both = json!({"name": "a", "gpu_type_id": "g", "image": "i", "template_id": "t"});
    let neither = json!({"name": "a", "gpu_type_id": "g"});
    for args in [both, neither] {
        let err = call_err(&client, RunpodOp::CreatePod, args).await;
        assert!(
            err.contains("exactly one of `image` or `template_id`"),
            "{err}"
        );
    }
    assert!(server.requests().is_empty());
}

async fn action_server(cost: f64, running: usize) -> TestServer {
    let listed: Vec<Value> = (0..running)
        .map(|i| pod(&format!("r{i}"), "RUNNING", 0.44))
        .collect();
    TestServer::start(vec![
        ("GET /v2/pods/p1", ok(&pod("p1", "EXITED", cost))),
        ("GET /v2/pods", pods(&listed)),
        ("POST /v2/pods/p1/action", ok(&pod("p1", "RUNNING", cost))),
    ])
    .await
}

#[tokio::test]
async fn starting_a_pod_within_the_limits_sends_the_action() {
    let server = action_server(0.44, 1).await;
    let args = json!({"pod_id": "p1", "action": "start"});
    let out = call_ok(&client(&server), RunpodOp::PodAction, args).await;
    let seen = server.requests();
    let posts = sent(&seen, "POST", "/v2/pods/p1/action");
    assert_eq!(posts.len(), 1);
    let body: Value = serde_json::from_str(&posts[0].body).unwrap();
    assert_eq!(body, json!({"action": "start"}));
    assert_eq!(out["pod"]["status"], "RUNNING");
}

#[tokio::test]
async fn starting_a_dear_or_extra_pod_is_refused() {
    for (cost, running, needle) in [(2.5, 0, "max_hourly_usd"), (0.44, 2, "max_running_pods")] {
        let server = action_server(cost, running).await;
        let args = json!({"pod_id": "p1", "action": "start"});
        let err = call_err(&client(&server), RunpodOp::PodAction, args).await;
        assert!(err.contains(needle), "{err}");
        assert!(sent(&server.requests(), "POST", "/v2/pods/p1/action").is_empty());
    }
}

#[tokio::test]
async fn stopping_a_pod_needs_no_limit_check() {
    let server = action_server(9.0, 5).await;
    let args = json!({"pod_id": "p1", "action": "stop"});
    call_ok(&client(&server), RunpodOp::PodAction, args).await;
    let seen = server.requests();
    assert_eq!(seen.len(), 1, "only the action is sent");
    assert_eq!(seen[0].body, json!({"action": "stop"}).to_string());
}

#[tokio::test]
async fn terminate_needs_the_pod_id_repeated() {
    let deleted = Reply {
        status: 204,
        headers: Vec::new(),
        body: Vec::new(),
        delay: None,
    };
    let server = TestServer::start(vec![("DELETE /v2/pods/p1", deleted)]).await;
    let client = client(&server);

    for confirm in [json!("p2"), Value::Null] {
        let args = json!({"pod_id": "p1", "confirm_pod_id": confirm});
        let err = call_err(&client, RunpodOp::TerminatePod, args).await;
        assert!(err.contains("confirm_pod_id"), "{err}");
    }
    assert!(server.requests().is_empty());

    let args = json!({"pod_id": "p1", "confirm_pod_id": "p1"});
    let out = call_ok(&client, RunpodOp::TerminatePod, args).await;
    assert_eq!(out["terminated"], "p1");
    assert_eq!(sent(&server.requests(), "DELETE", "/v2/pods/p1").len(), 1);
}

fn job_server() -> Vec<(&'static str, Reply)> {
    vec![
        (
            "POST /sl/v2/ep1/runsync",
            ok(&json!({"id": "job-1", "status": "COMPLETED", "output": [1, 2]})),
        ),
        (
            "POST /sl/v2/ep1/run",
            ok(&json!({"id": "job-2", "status": "IN_QUEUE"})),
        ),
    ]
}

#[tokio::test]
async fn run_job_waits_and_caps_the_execution_time() {
    let server = TestServer::start(job_server()).await;
    let client = client(&server);
    let input = json!({"prompt": "hello"});

    for (asked, seconds) in [(Value::Null, 300), (json!(30), 30), (json!(900), 300)] {
        let args = json!({"endpoint_id": "ep1", "input": input, "timeout_seconds": asked});
        let out = call_ok(&client, RunpodOp::RunJob, args).await;
        assert_eq!(out["job"]["status"], "COMPLETED");
        assert_eq!(out["execution_timeout_seconds"], seconds);
        let request = server.requests().pop().unwrap();
        assert!(
            request.path_and_query.contains("wait=1000"),
            "{}",
            request.path_and_query
        );
        let body: Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(body["input"], input);
        assert_eq!(body["policy"]["executionTimeout"], seconds * 1000);
        assert_eq!(out.get("note").is_some(), asked == json!(900));
    }
    assert_bearer(&server.requests());
}

#[tokio::test]
async fn run_job_without_wait_queues_it() {
    let server = TestServer::start(job_server()).await;
    let args = json!({"endpoint_id": "ep1", "input": {}, "wait": false});
    let out = call_ok(&client(&server), RunpodOp::RunJob, args).await;
    assert_eq!(out["job"]["id"], "job-2");
    assert_eq!(sent(&server.requests(), "POST", "/sl/v2/ep1/run").len(), 1);

    let err = call_err(
        &client(&server),
        RunpodOp::RunJob,
        json!({"endpoint_id": "ep1", "input": "text"}),
    )
    .await;
    assert!(err.contains("`input` must be a JSON object"), "{err}");
}

#[test]
fn limits_read_from_config_and_keep_defaults_for_bad_values() {
    let values = [
        ("max_hourly_usd", "2.5"),
        ("max_gpu_count", "4"),
        ("max_running_pods", "many"),
        ("max_job_seconds", "-3"),
    ];
    let get = |name: &str| {
        values
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| (*v).to_string())
    };
    let limits = SpendLimits::from_lookup(get);
    let defaults = SpendLimits::default();
    assert_eq!(limits.max_hourly_usd, 2.5);
    assert_eq!(limits.max_gpu_count, 4);
    assert_eq!(limits.max_running_pods, defaults.max_running_pods);
    assert_eq!(limits.max_job_seconds, defaults.max_job_seconds);
}
