//! Auth failures, timeouts, HTTP errors, the health check, and that the
//! key stays secret.

use super::support::*;
use crate::runpod::{
    health_check, ApiKey, Health, RunpodOp, RunpodSettings, ToolKind, RUNPOD_API_KEY_ENV,
};
use crate::tool::Tool;
use crate::web::test_server::{Reply, TestServer};
use serde_json::json;
use std::collections::HashSet;
use std::time::Duration;

#[tokio::test]
async fn a_rejected_key_says_so_for_401_and_403() {
    for status in [401, 403] {
        let problem = json!({"title": "Unauthorized", "status": status, "detail": "bad token"});
        let server = TestServer::start(vec![("/v2/pods", json_reply(status, &problem))]).await;
        let err = call_err(&client(&server), RunpodOp::ListPods, json!({})).await;
        assert!(err.contains("Runpod rejected the API key"), "{err}");
        assert!(err.contains(&format!("HTTP {status}")), "{err}");
        assert!(err.contains("Integrations > Runpod"), "{err}");
    }
}

#[tokio::test]
async fn a_missing_key_is_reported_and_nothing_is_sent() {
    let server = TestServer::start(vec![]).await;
    let client = client_with(settings(&server), None);
    let err = call_err(&client, RunpodOp::Balance, json!({})).await;
    assert!(err.contains("no Runpod API key"), "{err}");
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn a_slow_answer_times_out_naming_the_call() {
    let slow = Reply {
        delay: Some(Duration::from_secs(5)),
        ..ok(&json!({}))
    };
    let server = TestServer::start(vec![("/v2/billing/balance", slow)]).await;
    let mut settings = settings(&server);
    settings.request_timeout = Duration::from_millis(300);
    let err = call_err(
        &client_with(settings, Some(KEY)),
        RunpodOp::Balance,
        json!({}),
    )
    .await;
    assert_eq!(
        err,
        "get the account balance: Runpod did not answer within 1s"
    );
}

#[tokio::test]
async fn rate_limits_not_found_and_server_errors_are_explained() {
    let mut limited = json_reply(429, &json!({"title": "Too Many Requests"}));
    limited
        .headers
        .push(("RateLimit".into(), "\"default\";r=0;t=30".into()));
    let missing = json_reply(404, &json!({"title": "Not Found", "detail": "no pod p9"}));
    let broken = json_reply(500, &json!({"title": "Internal", "detail": "db down"}));
    let server = TestServer::start(vec![
        ("/v2/pods", limited),
        ("/v2/pods/p9", missing),
        ("/v2/billing/balance", broken),
    ])
    .await;
    let client = client(&server);

    let err = call_err(&client, RunpodOp::ListPods, json!({})).await;
    assert!(
        err.contains("rate limit was hit (it resets in 30s)"),
        "{err}"
    );
    let err = call_err(&client, RunpodOp::GetPod, json!({"pod_id": "p9"})).await;
    assert_eq!(err, "get pod p9: not found on Runpod: Not Found: no pod p9");
    let err = call_err(&client, RunpodOp::Balance, json!({})).await;
    assert!(err.contains("HTTP 500: Internal: db down"), "{err}");
}

#[tokio::test]
async fn an_unreachable_api_is_reported() {
    let mut settings = RunpodSettings {
        api_base: "http://127.0.0.1:9".to_string(),
        ..RunpodSettings::default()
    };
    settings.connect_timeout = Duration::from_millis(500);
    let client = client_with(settings, Some(KEY));
    let err = call_err(&client, RunpodOp::ListPods, json!({})).await;
    assert!(
        err.starts_with("list pods: could not reach Runpod"),
        "{err}"
    );
}

#[tokio::test]
async fn the_health_check_reports_each_outcome() {
    let account = ok(&json!({"id": "user_2abc", "email": "dev@example.com"}));
    let server = TestServer::start(vec![("GET /v2/account", account)]).await;
    assert_eq!(
        health_check(&client(&server)).await,
        Health::Connected {
            account_id: "user_2abc".into(),
            email: Some("d***@example.com".into()),
        }
    );
    assert_eq!(
        health_check(&client_with(settings(&server), None)).await,
        Health::MissingKey
    );

    let denied = TestServer::start(vec![("/v2/account", json_reply(401, &json!({})))]).await;
    assert_eq!(health_check(&client(&denied)).await, Health::BadKey);

    let gone = RunpodSettings {
        api_base: "http://127.0.0.1:9".into(),
        ..RunpodSettings::default()
    };
    let health = health_check(&client_with(gone, Some(KEY))).await;
    assert!(matches!(health, Health::Unreachable(_)), "{health:?}");
    assert!(!health.is_ok());
}

#[test]
fn the_key_never_prints() {
    let key = ApiKey::new(&format!("  {KEY} ")).unwrap();
    assert_eq!(format!("{key:?} {key}"), "ApiKey(***) ApiKey(***)");
    assert_eq!(key.expose(), KEY, "trimmed");
    assert!(ApiKey::new("   ").is_none());

    let client = client_with(RunpodSettings::default(), Some(KEY));
    assert!(!format!("{client:?}").contains(KEY));
    assert_eq!(RUNPOD_API_KEY_ENV, "RUNPOD_API_KEY");
}

#[test]
fn every_tool_has_a_unique_name_a_kind_and_an_object_schema() {
    let client = client_with(RunpodSettings::default(), Some(KEY));
    let tools = crate::runpod::runpod_tools(&client);
    assert_eq!(tools.len(), 10);
    let names: HashSet<_> = tools.iter().map(|t| t.name().to_string()).collect();
    assert_eq!(names.len(), 10);
    for tool in &tools {
        let definition = tool.definition();
        assert!(definition.name.starts_with("runpod_"));
        assert_eq!(definition.input_schema["type"], "object");
        assert_eq!(RunpodOp::from_name(&definition.name), Some(tool.op()));
        if tool.op().kind() != ToolKind::Read {
            assert!(
                definition.description.contains("COSTS MONEY")
                    || definition.description.contains("for good")
            );
        }
    }
    let spending: Vec<_> = RunpodOp::ALL
        .into_iter()
        .filter(|op| op.kind() != ToolKind::Read)
        .map(RunpodOp::name)
        .collect();
    assert_eq!(
        spending,
        [
            "runpod_create_pod",
            "runpod_pod_action",
            "runpod_run_job",
            "runpod_terminate_pod"
        ]
    );
}
