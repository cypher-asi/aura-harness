//! The tools that spend money or destroy resources. Each checks the
//! [`SpendLimits`](crate::runpod::SpendLimits) before it sends anything.

use super::args;
use super::read::{fetch_pod, gpu_catalog_request, job_row, pod_row, CLOUDS};
use super::RunpodOp;
use crate::runpod::client::{Api, Request};
use crate::runpod::{RunpodClient, RunpodError};
use reqwest::Method;
use serde_json::{json, Map, Value};
use std::time::Duration;

/// Pods counted when checking how many run (Runpod's page maximum).
const RUNNING_SCAN_LIMIT: u32 = 1000;

/// Extra time given to a `runsync` request beyond the wait it asks for.
const RUNSYNC_SLACK: Duration = Duration::from_secs(15);

/// What `runpod_create_pod` was asked for.
struct PodSpec {
    gpu: String,
    count: u32,
    cloud: String,
    body: Map<String, Value>,
}

fn pod_spec(args: &Value) -> Result<PodSpec, RunpodError> {
    let tool = RunpodOp::CreatePod.name();
    let name = args::string(tool, args, "name")?;
    let gpu = args::string(tool, args, "gpu_type_id")?;
    let count = args::optional_u64(tool, args, "gpu_count")?.unwrap_or(1);
    let count = u32::try_from(count)
        .ok()
        .filter(|n| *n >= 1)
        .ok_or_else(|| RunpodError::invalid(tool, "`gpu_count` must be 1 or more"))?;
    let cloud = args::optional_choice(tool, args, "cloud", &CLOUDS)?
        .unwrap_or_else(|| "SECURE".to_string());
    let mut body = Map::new();
    body.insert("name".into(), json!(name));
    body.insert("gpu".into(), json!({"id": gpu, "count": count}));
    body.insert("cloud".into(), json!(cloud));
    match (
        args::optional_string(tool, args, "image")?,
        args::optional_string(tool, args, "template_id")?,
    ) {
        (Some(image), None) => body.insert("image".into(), json!(image)),
        (None, Some(template)) => body.insert("templateId".into(), json!(template)),
        _ => {
            return Err(RunpodError::invalid(
                tool,
                "give exactly one of `image` or `template_id`",
            ))
        }
    };
    if let Some(disk) = args::optional_u64(tool, args, "container_disk_gb")? {
        body.insert("disk".into(), json!(disk));
    }
    insert_ports_and_env(tool, args, &mut body)?;
    Ok(PodSpec {
        gpu,
        count,
        cloud,
        body,
    })
}

fn insert_ports_and_env(
    tool: &str,
    args: &Value,
    body: &mut Map<String, Value>,
) -> Result<(), RunpodError> {
    if let Some(ports) = args.get("ports").filter(|v| !v.is_null()) {
        let valid = ports
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string));
        if !valid {
            return Err(RunpodError::invalid(
                tool,
                "`ports` must be a list like [\"8888/http\"]",
            ));
        }
        body.insert("ports".into(), ports.clone());
    }
    if let Some(env) = args.get("env").filter(|v| !v.is_null()) {
        let valid = env
            .as_object()
            .is_some_and(|vars| vars.values().all(Value::is_string));
        if !valid {
            return Err(RunpodError::invalid(
                tool,
                "`env` must map names to strings",
            ));
        }
        body.insert("env".into(), env.clone());
    }
    Ok(())
}

/// The catalog's hourly price for `count` of `gpu` on `cloud`, if listed.
async fn hourly_price(
    client: &RunpodClient,
    gpu: &str,
    count: u32,
    cloud: &str,
) -> Result<Option<f64>, RunpodError> {
    let request = gpu_catalog_request(Some(cloud), Some(u64::from(count)));
    let catalog = client.json(request).await?;
    let price_key = cloud.to_ascii_lowercase();
    let per_gpu = catalog
        .get("gpus")
        .and_then(Value::as_array)
        .and_then(|gpus| {
            gpus.iter()
                .find(|g| g.get("id").and_then(Value::as_str) == Some(gpu))
        })
        .and_then(|g| g.pointer(&format!("/price/{price_key}")))
        .and_then(Value::as_f64);
    // The catalog prices one GPU; a pod pays for each.
    Ok(per_gpu.map(|price| price * f64::from(count)))
}

/// How many pods are running, other than `except`.
async fn running_pods(client: &RunpodClient, except: Option<&str>) -> Result<usize, RunpodError> {
    let request = Request::new("list pods", Method::GET, Api::Control, "/v2/pods".into())
        .query("limit", &RUNNING_SCAN_LIMIT);
    let response = client.json(request).await?;
    let pods = response
        .get("pods")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    Ok(pods
        .iter()
        .filter(|pod| pod.get("status").and_then(Value::as_str) == Some("RUNNING"))
        .filter(|pod| except.is_none() || pod.get("id").and_then(Value::as_str) != except)
        .count())
}

pub(super) async fn create_pod(client: &RunpodClient, args: &Value) -> Result<Value, RunpodError> {
    let tool = RunpodOp::CreatePod.name();
    let spec = pod_spec(args)?;
    let limits = client.settings().limits;
    limits.check_gpu_count(tool, spec.count)?;
    let what = format!("{} x {} ({})", spec.count, spec.gpu, spec.cloud);
    let price = hourly_price(client, &spec.gpu, spec.count, &spec.cloud).await?;
    let hourly = limits.check_hourly(tool, &what, price)?;
    limits.check_running(tool, running_pods(client, None).await?)?;
    let request = Request::new(
        "create a pod",
        Method::POST,
        Api::Control,
        "/v2/pods".into(),
    )
    .body(Value::Object(spec.body));
    let pod = client.json(request).await?;
    Ok(json!({
        "pod": pod_row(&pod),
        "expected_cost_usd_hour": hourly,
        "note": "The pod bills until it is stopped or terminated.",
    }))
}

pub(super) async fn pod_action(client: &RunpodClient, args: &Value) -> Result<Value, RunpodError> {
    let tool = RunpodOp::PodAction.name();
    let pod_id = args::id(tool, args, "pod_id")?;
    let action = args::optional_choice(tool, args, "action", &["start", "stop", "restart"])?
        .ok_or_else(|| RunpodError::invalid(tool, "`action` is required"))?;
    if action == "start" {
        let limits = client.settings().limits;
        let pod = fetch_pod(client, &pod_id).await?;
        let cost = pod.get("cost").and_then(Value::as_f64);
        limits.check_hourly(tool, &format!("pod {pod_id}"), cost)?;
        limits.check_running(tool, running_pods(client, Some(&pod_id)).await?)?;
    }
    let request = Request::new(
        &format!("{action} pod {pod_id}"),
        Method::POST,
        Api::Control,
        format!("/v2/pods/{pod_id}/action"),
    )
    .body(json!({ "action": action }));
    let pod = client.json(request).await?;
    Ok(json!({ "action": action, "pod": pod_row(&pod) }))
}

pub(super) async fn terminate_pod(
    client: &RunpodClient,
    args: &Value,
) -> Result<Value, RunpodError> {
    let tool = RunpodOp::TerminatePod.name();
    let pod_id = args::id(tool, args, "pod_id")?;
    let confirm = args::optional_string(tool, args, "confirm_pod_id")?;
    if confirm.as_deref() != Some(pod_id.as_str()) {
        return Err(RunpodError::invalid(
            tool,
            format!(
                "terminating deletes the pod and its disk; repeat its id in `confirm_pod_id` \
                 (\"{pod_id}\") to confirm. Nothing was sent to Runpod"
            ),
        ));
    }
    let request = Request::new(
        &format!("terminate pod {pod_id}"),
        Method::DELETE,
        Api::Control,
        format!("/v2/pods/{pod_id}"),
    );
    client.json(request).await?;
    Ok(json!({ "terminated": pod_id }))
}

pub(super) async fn run_job(client: &RunpodClient, args: &Value) -> Result<Value, RunpodError> {
    let tool = RunpodOp::RunJob.name();
    let endpoint = args::id(tool, args, "endpoint_id")?;
    let input = args
        .get("input")
        .filter(|v| v.is_object())
        .cloned()
        .ok_or_else(|| RunpodError::invalid(tool, "`input` must be a JSON object"))?;
    let wait = args::optional_bool(tool, args, "wait")?.unwrap_or(true);
    let asked = args::optional_u64(tool, args, "timeout_seconds")?;
    let settings = client.settings();
    let seconds = settings.limits.job_seconds(asked);
    let body = json!({
        "input": input,
        "policy": { "executionTimeout": seconds * 1000 },
    });
    let request = if wait {
        let wait_ms = u64::try_from(settings.job_wait.as_millis()).unwrap_or(u64::MAX);
        Request::new(
            &format!("run a job on endpoint {endpoint}"),
            Method::POST,
            Api::Serverless,
            format!("/v2/{endpoint}/runsync"),
        )
        .query("wait", &wait_ms)
        .timeout(settings.job_wait + RUNSYNC_SLACK)
    } else {
        Request::new(
            &format!("queue a job on endpoint {endpoint}"),
            Method::POST,
            Api::Serverless,
            format!("/v2/{endpoint}/run"),
        )
    };
    let job = client.json(request.body(body)).await?;
    let mut out = json!({
        "endpoint_id": endpoint,
        "job": job_row(&job),
        "execution_timeout_seconds": seconds,
    });
    if asked.is_some_and(|a| a > seconds) {
        out["note"] = json!(format!(
            "timeout_seconds was lowered to max_job_seconds ({seconds}s)"
        ));
    }
    Ok(out)
}
