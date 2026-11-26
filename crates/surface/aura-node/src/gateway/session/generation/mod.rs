//! Generation request handler — proxies image, video, and 3D generation
//! through the harness session to aura-router, translating router SSE events
//! into typed `OutboundMessage::Generation*` variants.

mod proxy;
mod sse;

use super::Session;
use crate::protocol::{ErrorMsg, GenerationRequest, OutboundMessage};
use proxy::{run_generation_proxy, GenerationProxyCtx};
#[cfg(test)]
use sse::{parse_positive_secs, parse_sse_frame, pop_sse_frame};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

/// Upstream connect timeout for the router generation endpoint.
///
/// Kept deliberately low — a slow router is worse than a fast failure so the
/// WS session can surface a `GenerationError` and the client can retry.
const GENERATION_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Ceiling for the initial HTTP request/response-header handshake.
const GENERATION_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Env override for the upstream generation SSE idle watchdog.
const GENERATION_STREAM_IDLE_TIMEOUT_ENV: &str = "AURA_GENERATION_STREAM_IDLE_TIMEOUT_SECS";

/// Default maximum time we will wait for another SSE byte before treating the
/// stream as stuck. Successful image generations can be sparse: GPT Image
/// streams have been observed to send early progress and then stay quiet for
/// just over two minutes before completion.
const DEFAULT_GENERATION_STREAM_IDLE_TIMEOUT_SECS: u64 = 300;

pub(super) struct GenerationTurn {
    pub cancel_token: CancellationToken,
    pub join_handle: JoinHandle<()>,
}

struct RouterRequestSummary {
    body_bytes: usize,
    prompt_bytes: usize,
    model: Option<String>,
    size: Option<String>,
    image_count: usize,
    has_project_id: bool,
}

impl RouterRequestSummary {
    fn from_request(req: &GenerationRequest, body: &serde_json::Value) -> Self {
        Self {
            body_bytes: serde_json::to_vec(body).map_or(0, |bytes| bytes.len()),
            prompt_bytes: req.prompt.as_ref().map_or(0, String::len),
            model: req.model.clone(),
            size: req.size.clone(),
            image_count: req.images.as_ref().map_or(0, Vec::len),
            has_project_id: req.project_id.is_some(),
        }
    }
}

pub(super) fn start_generation(
    session: &Session,
    req: &GenerationRequest,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    router_url: &str,
) -> Option<GenerationTurn> {
    if !session.initialized {
        reject_uninitialized(session, req, outbound_tx);
        return None;
    }

    let mode = req.mode.clone();
    let auth_token = session.auth_token.clone().unwrap_or_default();

    let (url, body) = match build_router_request(router_url, req) {
        Ok(pair) => pair,
        Err(msg) => {
            reject_invalid_mode(session, &mode, msg, outbound_tx);
            return None;
        }
    };

    let cancel_token = CancellationToken::new();
    let cancel_for_task = cancel_token.clone();
    let outbound = outbound_tx.clone();
    let session_id = session.session_id.clone();
    let request_id = Uuid::new_v4().to_string();
    let summary = RouterRequestSummary::from_request(req, &body);
    log_turn_started(&session_id, &request_id, &mode, &url, &summary, &auth_token);

    let join_handle = tokio::spawn(async move {
        run_generation_proxy(GenerationProxyCtx {
            session_id: &session_id,
            request_id: &request_id,
            url: &url,
            jwt: &auth_token,
            body: &body,
            mode: &mode,
            outbound: &outbound,
            cancel: cancel_for_task,
        })
        .await;
    });

    Some(GenerationTurn {
        cancel_token,
        join_handle,
    })
}

fn reject_uninitialized(
    session: &Session,
    req: &GenerationRequest,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
) {
    warn!(
        session_id = %session.session_id,
        mode = %req.mode,
        "Generation request rejected before session initialization"
    );
    let _ = outbound_tx.try_send(OutboundMessage::Error(ErrorMsg {
        code: "not_initialized".into(),
        message: "Start a run before sending generation_request".into(),
        recoverable: true,
        support_id: None,
    }));
}

fn reject_invalid_mode(
    session: &Session,
    mode: &str,
    msg: String,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
) {
    warn!(
        session_id = %session.session_id,
        mode = %mode,
        error = %msg,
        "Generation request rejected"
    );
    let _ = outbound_tx.try_send(OutboundMessage::Error(ErrorMsg {
        code: "invalid_mode".into(),
        message: msg,
        recoverable: true,
        support_id: None,
    }));
}

fn log_turn_started(
    session_id: &str,
    request_id: &str,
    mode: &str,
    url: &str,
    summary: &RouterRequestSummary,
    auth_token: &str,
) {
    info!(
        %session_id,
        %request_id,
        %mode,
        url = %url,
        body_bytes = summary.body_bytes,
        prompt_bytes = summary.prompt_bytes,
        model = summary.model.as_deref().unwrap_or("missing"),
        size = summary.size.as_deref().unwrap_or("missing"),
        image_count = summary.image_count,
        has_project_id = summary.has_project_id,
        has_auth_token = !auth_token.is_empty(),
        "Generation turn started"
    );
}

fn build_router_request(
    router_url: &str,
    req: &GenerationRequest,
) -> Result<(String, serde_json::Value), String> {
    match req.mode.as_str() {
        "image" => Ok((
            format!("{router_url}/v1/generate-image/stream"),
            image_body(req),
        )),
        "3d" => Ok((
            format!("{router_url}/v1/generate-3d/stream"),
            three_d_body(req),
        )),
        "video" => Ok((
            format!("{router_url}/v1/generate-video/stream"),
            video_body(req),
        )),
        other => Err(format!("Unknown generation mode: {other}")),
    }
}

fn image_body(req: &GenerationRequest) -> serde_json::Value {
    let mut body = serde_json::json!({});
    if let Some(ref prompt) = req.prompt {
        body["prompt"] = serde_json::json!(prompt);
    }
    if let Some(ref model) = req.model {
        body["model"] = serde_json::json!(model);
    }
    if let Some(ref size) = req.size {
        body["size"] = serde_json::json!(size);
    }
    if let Some(ref quality) = req.quality {
        body["quality"] = serde_json::json!(quality);
    }
    if let Some(ref images) = req.images {
        body["images"] = serde_json::json!(images);
    }
    if let Some(ref pid) = req.project_id {
        body["projectId"] = serde_json::json!(pid);
    }
    if let Some(iter) = req.is_iteration {
        body["isIteration"] = serde_json::json!(iter);
    }
    body
}

fn three_d_body(req: &GenerationRequest) -> serde_json::Value {
    let mut body = serde_json::json!({});
    if let Some(ref image_url) = req.image_url {
        body["imageUrl"] = serde_json::json!(image_url);
    }
    if let Some(ref prompt) = req.prompt {
        body["prompt"] = serde_json::json!(prompt);
    }
    if let Some(ref model) = req.model {
        body["model"] = serde_json::json!(model);
    }
    if let Some(ref pid) = req.project_id {
        body["projectId"] = serde_json::json!(pid);
    }
    if let Some(ref parent_id) = req.parent_id {
        body["parentId"] = serde_json::json!(parent_id);
    }
    body
}

fn video_body(req: &GenerationRequest) -> serde_json::Value {
    let mut body = serde_json::json!({});
    if let Some(ref prompt) = req.prompt {
        body["prompt"] = serde_json::json!(prompt);
    }
    if let Some(ref model) = req.model {
        body["model"] = serde_json::json!(model);
    }
    if let Some(ref aspect_ratio) = req.aspect_ratio {
        body["aspectRatio"] = serde_json::json!(aspect_ratio);
    }
    if let Some(duration) = req.duration_seconds {
        body["durationSeconds"] = serde_json::json!(duration);
    }
    if let Some(ref resolution) = req.resolution {
        body["resolution"] = serde_json::json!(resolution);
    }
    if let Some(audio) = req.generate_audio {
        body["generateAudio"] = serde_json::json!(audio);
    }
    if let Some(ref images) = req.images {
        body["images"] = serde_json::json!(images);
    }
    if let Some(ref pid) = req.project_id {
        body["projectId"] = serde_json::json!(pid);
    }
    body
}

#[cfg(test)]
mod tests;
