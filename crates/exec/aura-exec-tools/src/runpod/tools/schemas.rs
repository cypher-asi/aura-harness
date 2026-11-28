//! What each Runpod tool tells the model: its description and schema.

use super::read::MAX_LOG_LINES;
use super::RunpodOp;
use crate::runpod::output::{DEFAULT_LIST_ITEMS, MAX_LIST_ITEMS};
use crate::runpod::RunpodSettings;
use aura_core_types::ToolDefinition;
use serde_json::{json, Value};

/// The definition of `op`, with the limits in `settings` spelled out.
pub(super) fn definition(op: RunpodOp, settings: &RunpodSettings) -> ToolDefinition {
    let (description, schema) = match op {
        RunpodOp::ListGpuTypes => list_gpu_types(),
        RunpodOp::ListPods => list_pods(),
        RunpodOp::GetPod => (
            "Get one Runpod pod: status, GPU, cost per hour, image, ports, SSH command and \
             the names of its environment variables."
                .to_string(),
            object(&[("pod_id", pod_id())], &["pod_id"]),
        ),
        RunpodOp::PodLogs => pod_logs(settings),
        RunpodOp::Balance => (
            "Get the Runpod account balance in USD.".to_string(),
            object(&[], &[]),
        ),
        RunpodOp::JobStatus => (
            "Get the status and, when finished, the output of a serverless job started with \
             runpod_run_job."
                .to_string(),
            object(
                &[
                    ("endpoint_id", endpoint_id()),
                    ("job_id", string("The job id.")),
                ],
                &["endpoint_id", "job_id"],
            ),
        ),
        RunpodOp::CreatePod => create_pod(settings),
        RunpodOp::PodAction => pod_action(settings),
        RunpodOp::RunJob => run_job(settings),
        RunpodOp::TerminatePod => (
            "Terminate a Runpod pod: deletes it and its container disk for good. Repeat the \
             pod id in confirm_pod_id. Use runpod_pod_action stop to pause billing instead."
                .to_string(),
            object(
                &[
                    ("pod_id", pod_id()),
                    (
                        "confirm_pod_id",
                        string("The same pod id again, to confirm."),
                    ),
                ],
                &["pod_id", "confirm_pod_id"],
            ),
        ),
    };
    ToolDefinition::new(op.name(), description, schema)
}

fn object(properties: &[(&str, Value)], required: &[&str]) -> Value {
    let properties: serde_json::Map<String, Value> = properties
        .iter()
        .map(|(name, schema)| ((*name).to_string(), schema.clone()))
        .collect();
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn string(description: &str) -> Value {
    json!({"type": "string", "description": description})
}

fn pod_id() -> Value {
    string("The pod id, from runpod_list_pods.")
}

fn endpoint_id() -> Value {
    string("The serverless endpoint id.")
}

fn cloud() -> Value {
    json!({
        "type": "string",
        "enum": ["SECURE", "COMMUNITY"],
        "description": "SECURE (Runpod data centers) or COMMUNITY (cheaper, hosted by others).",
    })
}

fn limit() -> Value {
    json!({
        "type": "integer",
        "minimum": 1,
        "maximum": MAX_LIST_ITEMS,
        "description": format!("Most items to return (default {DEFAULT_LIST_ITEMS})."),
    })
}

fn list_gpu_types() -> (String, Value) {
    (
        "List Runpod GPU types with memory, price per GPU per hour (secure and community), \
         availability and the most GPUs one pod can have. Use it before runpod_create_pod."
            .to_string(),
        object(
            &[
                ("cloud", cloud()),
                (
                    "gpu_count",
                    json!({"type": "integer", "minimum": 1,
                           "description": "Only types available with this many GPUs."}),
                ),
                (
                    "min_cuda_version",
                    string("Minimum CUDA version, such as \"12.4\"."),
                ),
                ("limit", limit()),
            ],
            &[],
        ),
    )
}

fn list_pods() -> (String, Value) {
    (
        "List your Runpod pods with status, GPU, cost per hour, data center and uptime."
            .to_string(),
        object(
            &[
                ("limit", limit()),
                (
                    "cursor",
                    string("next_cursor from an earlier call, for the next page."),
                ),
            ],
            &[],
        ),
    )
}

fn pod_logs(settings: &RunpodSettings) -> (String, Value) {
    (
        format!(
            "Read the latest log lines of a Runpod pod. Reads the log stream for at most {}s.",
            settings.log_read.as_secs()
        ),
        object(
            &[
                ("pod_id", pod_id()),
                (
                    "source",
                    json!({"type": "string", "enum": ["container", "system"],
                           "description": "container (default: the app's output) or system."}),
                ),
                (
                    "lines",
                    json!({"type": "integer", "minimum": 1, "maximum": MAX_LOG_LINES,
                           "description": "How many of the latest lines (default 100)."}),
                ),
            ],
            &["pod_id"],
        ),
    )
}

fn create_pod(settings: &RunpodSettings) -> (String, Value) {
    let limits = settings.limits;
    (
        format!(
            "Create and start a Runpod GPU pod. COSTS MONEY: it bills per hour until stopped \
             or terminated. Refused unless it costs at most ${:.2}/hour, has at most {} GPU(s) \
             and at most {} pods would be running. Give exactly one of image or template_id.",
            limits.max_hourly_usd, limits.max_gpu_count, limits.max_running_pods
        ),
        object(
            &[
                ("name", string("A name for the pod.")),
                (
                    "gpu_type_id",
                    string(
                        "GPU type id from runpod_list_gpu_types, e.g. \"NVIDIA GeForce RTX 4090\".",
                    ),
                ),
                (
                    "gpu_count",
                    json!({"type": "integer", "minimum": 1, "description": "GPUs (default 1)."}),
                ),
                ("cloud", cloud()),
                (
                    "image",
                    string("Container image, e.g. \"runpod/pytorch:latest\"."),
                ),
                (
                    "template_id",
                    string("A Runpod template id, instead of image."),
                ),
                (
                    "container_disk_gb",
                    json!({"type": "integer", "minimum": 1, "description": "Container disk size in GB."}),
                ),
                (
                    "ports",
                    json!({"type": "array", "items": {"type": "string"}, "description": "Ports to expose, e.g. [\"8888/http\", \"22/tcp\"]."}),
                ),
                (
                    "env",
                    json!({"type": "object", "additionalProperties": {"type": "string"}, "description": "Environment variables."}),
                ),
            ],
            &["name", "gpu_type_id"],
        ),
    )
}

fn pod_action(settings: &RunpodSettings) -> (String, Value) {
    (
        format!(
            "Start, stop or restart a Runpod pod. start COSTS MONEY and is refused if the pod \
             costs over ${:.2}/hour or {} pods are already running; stop ends GPU billing but \
             keeps the pod.",
            settings.limits.max_hourly_usd, settings.limits.max_running_pods
        ),
        object(
            &[
                ("pod_id", pod_id()),
                (
                    "action",
                    json!({"type": "string", "enum": ["start", "stop", "restart"]}),
                ),
            ],
            &["pod_id", "action"],
        ),
    )
}

fn run_job(settings: &RunpodSettings) -> (String, Value) {
    let max = settings.limits.max_job_seconds;
    (
        format!(
            "Run a job on a Runpod serverless endpoint. COSTS MONEY for the time it runs, \
             at most {max}s (max_job_seconds). With wait (default) it waits up to {}s for the \
             result; otherwise, or if it is still running, poll runpod_job_status.",
            settings.job_wait.as_secs()
        ),
        object(
            &[
                ("endpoint_id", endpoint_id()),
                (
                    "input",
                    json!({"type": "object", "description": "The job's input, as the endpoint expects."}),
                ),
                (
                    "wait",
                    json!({"type": "boolean", "description": "Wait for the result (default true)."}),
                ),
                (
                    "timeout_seconds",
                    json!({"type": "integer", "minimum": 5, "maximum": max, "description": format!("Longest the job may run (default and max {max}).")}),
                ),
            ],
            &["endpoint_id", "input"],
        ),
    )
}
