//! SSE framing, router-event translation and per-event logging for the
//! generation proxy.

use super::{DEFAULT_GENERATION_STREAM_IDLE_TIMEOUT_SECS, GENERATION_STREAM_IDLE_TIMEOUT_ENV};
use crate::protocol::{
    GenerationCompleted, GenerationErrorMsg, GenerationPartialImage, GenerationProgressMsg,
    GenerationStart, OutboundMessage,
};
use std::time::Duration;
use tracing::{debug, info, warn};

pub(super) fn generation_stream_idle_timeout() -> Duration {
    Duration::from_secs(env_duration_secs(
        GENERATION_STREAM_IDLE_TIMEOUT_ENV,
        DEFAULT_GENERATION_STREAM_IDLE_TIMEOUT_SECS,
    ))
}

fn env_duration_secs(key: &str, default_secs: u64) -> u64 {
    parse_positive_secs(std::env::var(key).ok().as_deref()).unwrap_or(default_secs)
}

pub(super) fn parse_positive_secs(raw: Option<&str>) -> Option<u64> {
    raw.and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
}

pub(super) fn pop_sse_frame(buffer: &mut String) -> Option<String> {
    let lf_pos = buffer.find("\n\n").map(|pos| (pos, 2));
    let crlf_pos = buffer.find("\r\n\r\n").map(|pos| (pos, 4));
    let (sep_pos, sep_len) = match (lf_pos, crlf_pos) {
        (Some(lf), Some(crlf)) => {
            if lf.0 < crlf.0 {
                lf
            } else {
                crlf
            }
        }
        (Some(lf), None) => lf,
        (None, Some(crlf)) => crlf,
        (None, None) => return None,
    };

    let frame = buffer[..sep_pos].to_string();
    *buffer = buffer[sep_pos + sep_len..].to_string();
    Some(frame)
}

pub(super) fn parse_sse_frame(frame: &str, mode: &str) -> Option<(String, OutboundMessage)> {
    if frame.trim().is_empty() {
        return None;
    }
    let mut event_type = String::new();
    let mut data_lines: Vec<String> = Vec::new();
    for raw_line in frame.split('\n') {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.is_empty() || line.starts_with(':') {
            continue;
        }

        if let Some(rest) = line.strip_prefix("event:") {
            event_type = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        }
    }
    if data_lines.is_empty() {
        return None;
    }
    let data = data_lines.join("\n");

    if event_type.is_empty() {
        event_type = serde_json::from_str::<serde_json::Value>(&data)
            .ok()
            .and_then(|parsed| {
                parsed
                    .get("type")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "message".to_string());
    }

    translate_router_event(&event_type, &data, mode).map(|msg| (event_type, msg))
}

/// Identifies one forwarded event in the per-event log lines.
pub(super) struct EventLogCtx<'a> {
    pub session_id: &'a str,
    pub request_id: &'a str,
    pub mode: &'a str,
    pub event_type: &'a str,
    pub event_count: u64,
}

pub(super) fn log_generation_event(ctx: &EventLogCtx<'_>, msg: &OutboundMessage) {
    let &EventLogCtx {
        session_id,
        request_id,
        mode,
        event_type,
        event_count,
    } = ctx;
    match msg {
        OutboundMessage::GenerationProgress(progress) => log_progress_event(ctx, progress),
        OutboundMessage::GenerationPartialImage(partial) => {
            info!(
                %session_id,
                %request_id,
                %mode,
                %event_type,
                event_count,
                data_bytes = partial.data.len(),
                "Generation proxy: forwarding partial image event"
            );
        }
        OutboundMessage::GenerationCompleted(_) => {
            info!(
                %session_id,
                %request_id,
                %mode,
                %event_type,
                event_count,
                "Generation proxy: forwarding completion event"
            );
        }
        OutboundMessage::GenerationError(error) => log_error_event(ctx, error),
        _ => {
            debug!(
                %session_id,
                %request_id,
                %mode,
                %event_type,
                event_count,
                "Generation proxy: forwarding event"
            );
        }
    }
}

fn log_progress_event(ctx: &EventLogCtx<'_>, progress: &GenerationProgressMsg) {
    let &EventLogCtx {
        session_id,
        request_id,
        mode,
        event_type,
        event_count,
    } = ctx;
    info!(
        %session_id,
        %request_id,
        %mode,
        %event_type,
        event_count,
        percent = progress.percent,
        message_bytes = progress.message.len(),
        "Generation proxy: forwarding progress event"
    );
}

fn log_error_event(ctx: &EventLogCtx<'_>, error: &GenerationErrorMsg) {
    let &EventLogCtx {
        session_id,
        request_id,
        mode,
        event_type,
        event_count,
    } = ctx;
    warn!(
        %session_id,
        %request_id,
        %mode,
        %event_type,
        event_count,
        code = %error.code,
        message_bytes = error.message.len(),
        "Generation proxy: forwarding error event"
    );
}

fn translate_router_event(event_type: &str, data: &str, mode: &str) -> Option<OutboundMessage> {
    match event_type {
        "start" => Some(OutboundMessage::GenerationStart(GenerationStart {
            mode: mode.to_string(),
        })),
        "progress" => Some(progress_event(data)),
        "partial-image" => Some(partial_image_event(data)),
        "completed" => {
            let payload: serde_json::Value = serde_json::from_str(data).unwrap_or_default();
            Some(OutboundMessage::GenerationCompleted(GenerationCompleted {
                mode: mode.to_string(),
                payload,
            }))
        }
        "submitted" => {
            let parsed: serde_json::Value = serde_json::from_str(data).unwrap_or_default();
            let task_id = parsed.get("taskId").and_then(|v| v.as_str()).unwrap_or("");
            Some(OutboundMessage::GenerationProgress(GenerationProgressMsg {
                percent: 5.0,
                message: format!("Task submitted: {task_id}"),
            }))
        }
        "error" => Some(error_event(data)),
        _ => None,
    }
}

fn progress_event(data: &str) -> OutboundMessage {
    let parsed: serde_json::Value = serde_json::from_str(data).unwrap_or_default();
    OutboundMessage::GenerationProgress(GenerationProgressMsg {
        percent: parsed
            .get("percent")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
        message: parsed
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

fn partial_image_event(data: &str) -> OutboundMessage {
    let parsed: serde_json::Value = serde_json::from_str(data).unwrap_or_default();
    OutboundMessage::GenerationPartialImage(GenerationPartialImage {
        data: parsed
            .get("data")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

fn error_event(data: &str) -> OutboundMessage {
    let parsed: serde_json::Value = serde_json::from_str(data).unwrap_or_default();
    OutboundMessage::GenerationError(GenerationErrorMsg {
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
    })
}
