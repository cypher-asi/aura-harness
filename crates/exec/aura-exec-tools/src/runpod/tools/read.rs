//! The read tools: GPU catalog, pods, logs, balance and job status.

use super::args;
use super::RunpodOp;
use crate::runpod::client::{Api, Request};
use crate::runpod::output::{cap_bytes, list, list_limit, pick};
use crate::runpod::{RunpodClient, RunpodError};
use reqwest::Method;
use serde_json::{json, Map, Value};

/// Log lines returned when the call doesn't say.
const DEFAULT_LOG_LINES: u64 = 100;

/// Most log lines returned.
pub(super) const MAX_LOG_LINES: u64 = 500;

/// Most log bytes read from the stream.
const MAX_LOG_BYTES: usize = 64 * 1024;

/// The cloud names Runpod accepts.
pub(super) const CLOUDS: [&str; 2] = ["SECURE", "COMMUNITY"];

/// A GPU type as an agent needs it.
pub(super) fn gpu_row(gpu: &Value) -> Value {
    pick(
        gpu,
        &[
            ("id", "/id"),
            ("name", "/name"),
            ("memory_gb", "/memory"),
            ("price_secure_usd_hour", "/price/secure"),
            ("price_community_usd_hour", "/price/community"),
            ("availability", "/availability"),
            ("max_count_secure", "/maxCount/secure"),
            ("max_count_community", "/maxCount/community"),
        ],
    )
}

/// A pod as an agent needs it. Leaves out `env`, which may hold secrets.
pub(super) fn pod_row(pod: &Value) -> Value {
    pick(
        pod,
        &[
            ("id", "/id"),
            ("name", "/name"),
            ("status", "/status"),
            ("gpu", "/gpu/id"),
            ("gpu_count", "/gpu/count"),
            ("cost_usd_hour", "/cost"),
            ("cloud", "/cloud"),
            ("data_center", "/dataCenterId"),
            ("uptime_seconds", "/runtime/uptime"),
            ("started_at", "/startedAt"),
        ],
    )
}

/// A serverless job's state.
pub(super) fn job_row(job: &Value) -> Value {
    pick(
        job,
        &[
            ("id", "/id"),
            ("status", "/status"),
            ("output", "/output"),
            ("error", "/error"),
            ("execution_ms", "/executionTime"),
            ("delay_ms", "/delayTime"),
        ],
    )
}

/// `GET /v2/catalog/gpus` with the given filters.
pub(super) fn gpu_catalog_request(cloud: Option<&str>, count: Option<u64>) -> Request {
    let mut request = Request::new(
        "list GPU types",
        Method::GET,
        Api::Control,
        "/v2/catalog/gpus".to_string(),
    );
    if let Some(cloud) = cloud {
        request = request.query("cloud", &cloud);
    }
    if let Some(count) = count {
        request = request.query("count", &count);
    }
    request
}

pub(super) async fn list_gpu_types(
    client: &RunpodClient,
    args: &Value,
) -> Result<Value, RunpodError> {
    let tool = RunpodOp::ListGpuTypes.name();
    let cloud = args::optional_choice(tool, args, "cloud", &CLOUDS)?;
    let count = args::optional_u64(tool, args, "gpu_count")?.map(|n| n.max(1));
    let mut request = gpu_catalog_request(cloud.as_deref(), count);
    if let Some(cuda) = args::optional_string(tool, args, "min_cuda_version")? {
        request = request.query("minCudaVersion", &cuda);
    }
    let limit = list_limit(args::optional_u64(tool, args, "limit")?);
    let response = client.json(request).await?;
    Ok(Value::Object(list(
        &response, "/gpus", "gpus", limit, gpu_row,
    )))
}

pub(super) async fn list_pods(client: &RunpodClient, args: &Value) -> Result<Value, RunpodError> {
    let tool = RunpodOp::ListPods.name();
    let limit = list_limit(args::optional_u64(tool, args, "limit")?);
    let mut request = Request::new("list pods", Method::GET, Api::Control, "/v2/pods".into())
        .query("limit", &limit);
    if let Some(cursor) = args::optional_string(tool, args, "cursor")? {
        request = request.query("cursor", &cursor);
    }
    let response = client.json(request).await?;
    let mut out = list(&response, "/pods", "pods", limit, pod_row);
    if let Some(next) = response
        .pointer("/pagination/nextCursor")
        .filter(|v| !v.is_null())
    {
        out.insert("next_cursor".to_string(), next.clone());
    }
    Ok(Value::Object(out))
}

/// `GET /v2/pods/{id}`.
pub(super) async fn fetch_pod(client: &RunpodClient, pod_id: &str) -> Result<Value, RunpodError> {
    let request = Request::new(
        &format!("get pod {pod_id}"),
        Method::GET,
        Api::Control,
        format!("/v2/pods/{pod_id}"),
    );
    client.json(request).await
}

pub(super) async fn get_pod(client: &RunpodClient, args: &Value) -> Result<Value, RunpodError> {
    let pod_id = args::id(RunpodOp::GetPod.name(), args, "pod_id")?;
    let pod = fetch_pod(client, &pod_id).await?;
    let mut out = match pod_row(&pod) {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    let Value::Object(extra) = pick(
        &pod,
        &[
            ("image", "/image"),
            ("template", "/template"),
            ("ports", "/ports"),
            ("actions", "/actions"),
            ("ssh_command", "/ssh/proxy/command"),
            ("created_at", "/createdAt"),
        ],
    ) else {
        return Ok(Value::Object(out));
    };
    out.extend(extra);
    if let Some(env) = pod.get("env").and_then(Value::as_object) {
        // Names only: values are often tokens.
        out.insert("env_names".into(), json!(env.keys().collect::<Vec<_>>()));
    }
    Ok(Value::Object(out))
}

pub(super) async fn pod_logs(client: &RunpodClient, args: &Value) -> Result<Value, RunpodError> {
    let tool = RunpodOp::PodLogs.name();
    let pod_id = args::id(tool, args, "pod_id")?;
    let source = args::optional_choice(tool, args, "source", &["container", "system"])?
        .unwrap_or_else(|| "container".to_string());
    let tail = args::optional_u64(tool, args, "lines")?
        .unwrap_or(DEFAULT_LOG_LINES)
        .clamp(1, MAX_LOG_LINES);
    let request = Request::new(
        &format!("read logs of pod {pod_id}"),
        Method::GET,
        Api::Control,
        format!("/v2/pods/{pod_id}/logs"),
    )
    .query("source", &source)
    .query("tail", &tail);
    let read_for = client.settings().log_read;
    let text = client.stream_text(request, read_for, MAX_LOG_BYTES).await?;
    let lines = sse_log_lines(&text, usize::try_from(tail).unwrap_or(usize::MAX));
    let joined = cap_bytes(&lines.join("\n"), MAX_LOG_BYTES);
    Ok(json!({
        "pod_id": pod_id,
        "source": source,
        "line_count": lines.len(),
        "logs": joined,
    }))
}

/// The log lines of a server-sent event stream, `ts line` each, the last
/// `max` of them. A `data:` payload that isn't the JSON Runpod documents
/// is kept as text.
pub(super) fn sse_log_lines(text: &str, max: usize) -> Vec<String> {
    let lines: Vec<String> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .filter(|data| !data.is_empty())
        .map(|data| match serde_json::from_str::<Value>(data) {
            Ok(event) => {
                let field = |name: &str| event.get(name).and_then(Value::as_str).unwrap_or("");
                format!("{} {}", field("ts"), field("line"))
                    .trim()
                    .to_string()
            }
            Err(_) => data.to_string(),
        })
        .collect();
    let skip = lines.len().saturating_sub(max);
    lines.into_iter().skip(skip).collect()
}

pub(super) async fn balance(client: &RunpodClient) -> Result<Value, RunpodError> {
    let request = Request::new(
        "get the account balance",
        Method::GET,
        Api::Control,
        "/v2/billing/balance".to_string(),
    );
    let response = client.json(request).await?;
    Ok(pick(
        &response,
        &[
            ("balance_usd", "/balanceAmount"),
            ("billing_mode", "/billingMode"),
        ],
    ))
}

pub(super) async fn job_status(client: &RunpodClient, args: &Value) -> Result<Value, RunpodError> {
    let tool = RunpodOp::JobStatus.name();
    let endpoint = args::id(tool, args, "endpoint_id")?;
    let job = args::id(tool, args, "job_id")?;
    let request = Request::new(
        &format!("get job {job} on endpoint {endpoint}"),
        Method::GET,
        Api::Serverless,
        format!("/v2/{endpoint}/status/{job}"),
    );
    Ok(job_row(&client.json(request).await?))
}
