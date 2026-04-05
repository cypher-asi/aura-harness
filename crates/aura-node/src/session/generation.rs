//! Generation request handler — proxies image/3D generation through the
//! harness session to aura-router, translating router SSE events into typed
//! `OutboundMessage::Generation*` variants.

use crate::protocol::{
    ErrorMsg, GenerationCompleted, GenerationErrorMsg, GenerationPartialImage,
    GenerationProgressMsg, GenerationRequest, GenerationStart, OutboundMessage,
};
use super::Session;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

pub(super) struct GenerationTurn {
    pub cancel_token: CancellationToken,
    pub join_handle: JoinHandle<()>,
}

pub(super) fn start_generation(
    session: &Session,
    req: GenerationRequest,
    outbound_tx: &mpsc::Sender<OutboundMessage>,
    router_url: &str,
) -> Option<GenerationTurn> {
    if !session.initialized {
        let _ = outbound_tx.try_send(OutboundMessage::Error(ErrorMsg {
            code: "not_initialized".into(),
            message: "Send session_init before generation_request".into(),
            recoverable: true,
        }));
        return None;
    }

    let mode = req.mode.clone();
    let auth_token = session.auth_token.clone().unwrap_or_default();

    let (url, body) = match build_router_request(router_url, &req) {
        Ok(pair) => pair,
        Err(msg) => {
            let _ = outbound_tx.try_send(OutboundMessage::Error(ErrorMsg {
                code: "invalid_mode".into(),
                message: msg,
                recoverable: true,
            }));
            return None;
        }
    };

    let cancel_token = CancellationToken::new();
    let cancel_for_task = cancel_token.clone();
    let outbound = outbound_tx.clone();
    let session_id = session.session_id.clone();

    info!(%session_id, %mode, "Generation turn started");

    let join_handle = tokio::spawn(async move {
        run_generation_proxy(&url, &auth_token, &body, &mode, &outbound, cancel_for_task).await;
    });

    Some(GenerationTurn {
        cancel_token,
        join_handle,
    })
}

fn build_router_request(
    router_url: &str,
    req: &GenerationRequest,
) -> Result<(String, serde_json::Value), String> {
    match req.mode.as_str() {
        "image" => {
            let url = format!("{router_url}/v1/generate-image/stream");
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
            if let Some(ref images) = req.images {
                body["images"] = serde_json::json!(images);
            }
            if let Some(ref pid) = req.project_id {
                body["projectId"] = serde_json::json!(pid);
            }
            if let Some(iter) = req.is_iteration {
                body["isIteration"] = serde_json::json!(iter);
            }
            Ok((url, body))
        }
        "3d" => {
            let url = format!("{router_url}/v1/generate-3d/stream");
            let mut body = serde_json::json!({});
            if let Some(ref image_url) = req.image_url {
                body["imageUrl"] = serde_json::json!(image_url);
            }
            if let Some(ref prompt) = req.prompt {
                body["prompt"] = serde_json::json!(prompt);
            }
            if let Some(ref pid) = req.project_id {
                body["projectId"] = serde_json::json!(pid);
            }
            Ok((url, body))
        }
        other => Err(format!("Unknown generation mode: {other}")),
    }
}

async fn run_generation_proxy(
    url: &str,
    jwt: &str,
    body: &serde_json::Value,
    mode: &str,
    outbound: &mpsc::Sender<OutboundMessage>,
    cancel: CancellationToken,
) {
    let client = reqwest::Client::new();
    let resp = match client.post(url).bearer_auth(jwt).json(body).send().await {
        Ok(r) => r,
        Err(e) => {
            error!(error = %e, "Generation proxy: upstream request failed");
            let _ = outbound.try_send(OutboundMessage::GenerationError(GenerationErrorMsg {
                code: "UPSTREAM_ERROR".into(),
                message: format!("upstream request failed: {e}"),
            }));
            return;
        }
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        error!(%status, body = %text, "Generation proxy: upstream error");
        let _ = outbound.try_send(OutboundMessage::GenerationError(GenerationErrorMsg {
            code: format!("UPSTREAM_{}", status.as_u16()),
            message: format!("upstream returned {status}: {text}"),
        }));
        return;
    }

    let mut byte_stream = resp.bytes_stream();
    let mut buffer = String::new();

    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                info!("Generation cancelled by client");
                return;
            }
            chunk = byte_stream.next() => {
                match chunk {
                    Some(Ok(bytes)) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        while let Some(sep) = buffer.find("\n\n") {
                            let frame = buffer[..sep].to_string();
                            buffer = buffer[sep + 2..].to_string();
                            if let Some(msg) = parse_sse_frame(&frame, mode) {
                                if outbound.try_send(msg).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    Some(Err(e)) => {
                        let _ = outbound.try_send(OutboundMessage::GenerationError(
                            GenerationErrorMsg {
                                code: "STREAM_ERROR".into(),
                                message: format!("Stream error: {e}"),
                            },
                        ));
                        return;
                    }
                    None => {
                        // Flush remaining buffer
                        if !buffer.trim().is_empty() {
                            if let Some(msg) = parse_sse_frame(&buffer, mode) {
                                let _ = outbound.try_send(msg);
                            }
                        }
                        return;
                    }
                }
            }
        }
    }
}

fn parse_sse_frame(frame: &str, mode: &str) -> Option<OutboundMessage> {
    if frame.trim().is_empty() {
        return None;
    }
    let mut event_type = String::new();
    let mut data = String::new();
    for line in frame.split('\n') {
        if let Some(rest) = line.strip_prefix("event: ") {
            event_type = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("data: ") {
            data = rest.trim().to_string();
        }
    }
    if event_type.is_empty() || data.is_empty() {
        return None;
    }
    translate_router_event(&event_type, &data, mode)
}

fn translate_router_event(event_type: &str, data: &str, mode: &str) -> Option<OutboundMessage> {
    match event_type {
        "start" => Some(OutboundMessage::GenerationStart(GenerationStart {
            mode: mode.to_string(),
        })),
        "progress" => {
            let parsed: serde_json::Value = serde_json::from_str(data).unwrap_or_default();
            Some(OutboundMessage::GenerationProgress(GenerationProgressMsg {
                percent: parsed
                    .get("percent")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                message: parsed
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            }))
        }
        "partial-image" => {
            let parsed: serde_json::Value = serde_json::from_str(data).unwrap_or_default();
            Some(OutboundMessage::GenerationPartialImage(
                GenerationPartialImage {
                    data: parsed
                        .get("data")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                },
            ))
        }
        "completed" => {
            let payload: serde_json::Value = serde_json::from_str(data).unwrap_or_default();
            Some(OutboundMessage::GenerationCompleted(GenerationCompleted {
                mode: mode.to_string(),
                payload,
            }))
        }
        "submitted" => {
            let parsed: serde_json::Value = serde_json::from_str(data).unwrap_or_default();
            let task_id = parsed
                .get("taskId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Some(OutboundMessage::GenerationProgress(GenerationProgressMsg {
                percent: 5.0,
                message: format!("Task submitted: {task_id}"),
            }))
        }
        "error" => {
            let parsed: serde_json::Value = serde_json::from_str(data).unwrap_or_default();
            Some(OutboundMessage::GenerationError(GenerationErrorMsg {
                code: parsed
                    .get("code")
                    .and_then(|v| v.as_str())
                    .unwrap_or("GENERATION_FAILED")
                    .to_string(),
                message: parsed
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Generation failed")
                    .to_string(),
            }))
        }
        _ => None,
    }
}
