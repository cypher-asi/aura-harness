//! Upstream HTTP call and SSE stream pump for one generation turn.
//!
//! Declared-Exception surface (see `docs/invariants.md`): this image /
//! video / 3D proxy is a pure router-side SSE pipe, so it does not route
//! through the kernel.

use super::sse::{
    generation_stream_idle_timeout, log_generation_event, parse_sse_frame, pop_sse_frame,
    EventLogCtx,
};
use super::{GENERATION_CONNECT_TIMEOUT, GENERATION_REQUEST_TIMEOUT};
use crate::protocol::{GenerationErrorMsg, OutboundMessage};
use aura_core_types::ErrorChain;
use futures_util::StreamExt;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// Borrowed-everything context for a single generation proxy run.
///
/// Bundling the eight call-site parameters under one struct keeps
/// the function signature under the `clippy::too_many_arguments`
/// ceiling without cloning anything — every field is a `&'a`
/// borrow that lives at least as long as the `await`.
pub(super) struct GenerationProxyCtx<'a> {
    pub session_id: &'a str,
    pub request_id: &'a str,
    pub url: &'a str,
    pub jwt: &'a str,
    pub body: &'a serde_json::Value,
    pub mode: &'a str,
    pub outbound: &'a mpsc::Sender<OutboundMessage>,
    pub cancel: CancellationToken,
}

impl GenerationProxyCtx<'_> {
    fn send_error(&self, code: String, message: String) {
        let _ = self
            .outbound
            .try_send(OutboundMessage::GenerationError(GenerationErrorMsg {
                code,
                message,
            }));
    }
}

pub(super) async fn run_generation_proxy(ctx: GenerationProxyCtx<'_>) {
    // We apply bounded connect / handshake timeouts so a hung upstream cannot
    // stall the WS session. The streaming body is guarded by an idle timeout
    // instead of a total timeout because real generations can legitimately
    // run for minutes as long as the router keeps sending events.
    let Some(client) = build_client(&ctx) else {
        return;
    };
    let stream_idle_timeout = generation_stream_idle_timeout();
    let Some(resp) = send_upstream(&ctx, &client, stream_idle_timeout).await else {
        return;
    };
    let Some(resp) = check_status(&ctx, resp).await else {
        return;
    };
    StreamPump::new(&ctx, stream_idle_timeout).run(resp).await;
}

fn build_client(ctx: &GenerationProxyCtx<'_>) -> Option<reqwest::Client> {
    let &GenerationProxyCtx {
        session_id,
        request_id,
        mode,
        ..
    } = ctx;
    match reqwest::Client::builder()
        .connect_timeout(GENERATION_CONNECT_TIMEOUT)
        .build()
    {
        Ok(c) => Some(c),
        Err(e) => {
            let e = ErrorChain(&e);
            error!(
                %session_id,
                %request_id,
                %mode,
                error = %e,
                "Generation proxy: reqwest client build failed"
            );
            ctx.send_error(
                "UPSTREAM_ERROR".into(),
                format!("failed to build http client: {e}"),
            );
            None
        }
    }
}

async fn send_upstream(
    ctx: &GenerationProxyCtx<'_>,
    client: &reqwest::Client,
    stream_idle_timeout: Duration,
) -> Option<reqwest::Response> {
    let &GenerationProxyCtx {
        session_id,
        request_id,
        url,
        jwt,
        body,
        mode,
        ..
    } = ctx;
    info!(
        %session_id,
        %request_id,
        %mode,
        url = %url,
        body_bytes = serde_json::to_vec(body).map_or(0, |bytes| bytes.len()),
        handshake_timeout_secs = GENERATION_REQUEST_TIMEOUT.as_secs(),
        stream_idle_timeout_secs = stream_idle_timeout.as_secs(),
        "Generation proxy: sending upstream request"
    );
    let send_fut = client.post(url).bearer_auth(jwt).json(body).send();
    let resp = tokio::select! {
        biased;
        () = ctx.cancel.cancelled() => {
            info!(
                %session_id,
                %request_id,
                %mode,
                "Generation cancelled by client before upstream handshake completed"
            );
            return None;
        }
        result = tokio::time::timeout(GENERATION_REQUEST_TIMEOUT, send_fut) => result,
    };
    let Ok(resp) = resp else {
        report_handshake_timeout(ctx);
        return None;
    };
    handle_send_result(ctx, resp)
}

fn report_handshake_timeout(ctx: &GenerationProxyCtx<'_>) {
    let &GenerationProxyCtx {
        session_id,
        request_id,
        mode,
        ..
    } = ctx;
    error!(
        %session_id,
        %request_id,
        %mode,
        timeout_secs = GENERATION_REQUEST_TIMEOUT.as_secs(),
        "Generation proxy: upstream request timed out during handshake"
    );
    ctx.send_error(
        "UPSTREAM_TIMEOUT".into(),
        format!(
            "upstream did not respond within {}s",
            GENERATION_REQUEST_TIMEOUT.as_secs()
        ),
    );
}

fn handle_send_result(
    ctx: &GenerationProxyCtx<'_>,
    resp: reqwest::Result<reqwest::Response>,
) -> Option<reqwest::Response> {
    let &GenerationProxyCtx {
        session_id,
        request_id,
        mode,
        ..
    } = ctx;
    match resp {
        Ok(r) => Some(r),
        Err(e) => {
            let e = ErrorChain(&e);
            error!(
                %session_id,
                %request_id,
                %mode,
                error = %e,
                "Generation proxy: upstream request failed"
            );
            ctx.send_error(
                "UPSTREAM_ERROR".into(),
                format!("upstream request failed: {e}"),
            );
            None
        }
    }
}

async fn check_status(
    ctx: &GenerationProxyCtx<'_>,
    resp: reqwest::Response,
) -> Option<reqwest::Response> {
    let &GenerationProxyCtx {
        session_id,
        request_id,
        mode,
        ..
    } = ctx;
    let status = resp.status();
    info!(
        %session_id,
        %request_id,
        %mode,
        %status,
        "Generation proxy: upstream response received"
    );

    if resp.status().is_success() {
        return Some(resp);
    }
    let text = resp.text().await.unwrap_or_default();
    // Intentionally do NOT log `text`: upstream error bodies may include
    // provider secrets, unredacted prompts, or internal stack traces.
    // We surface `status` + a short code + length for diagnosis and send
    // the status-derived code to the client. (Wave 5 / T2.1.)
    error!(
        %session_id,
        %request_id,
        %mode,
        %status,
        body_len = text.len(),
        code = %format!("UPSTREAM_{}", status.as_u16()),
        "Generation proxy: upstream error"
    );
    ctx.send_error(
        format!("UPSTREAM_{}", status.as_u16()),
        format!("upstream returned {status}"),
    );
    None
}

/// Whether the stream loop keeps reading after a branch has been handled.
enum Flow {
    Continue,
    Stop,
}

/// Loop state of the SSE stream pump: the partial-frame buffer and the
/// counters reported in the log lines.
struct StreamPump<'c, 'a> {
    ctx: &'c GenerationProxyCtx<'a>,
    idle_timeout: Duration,
    buffer: String,
    chunk_count: u64,
    event_count: u64,
    bytes_seen: u64,
}

impl<'c, 'a> StreamPump<'c, 'a> {
    fn new(ctx: &'c GenerationProxyCtx<'a>, idle_timeout: Duration) -> Self {
        Self {
            ctx,
            idle_timeout,
            buffer: String::new(),
            chunk_count: 0,
            event_count: 0,
            bytes_seen: 0,
        }
    }

    async fn run(mut self, resp: reqwest::Response) {
        let ctx = self.ctx;
        let idle_timeout = self.idle_timeout;
        let mut byte_stream = resp.bytes_stream();
        loop {
            let flow = tokio::select! {
                biased;
                () = ctx.cancel.cancelled() => {
                    self.on_cancelled();
                    Flow::Stop
                }
                chunk = tokio::time::timeout(idle_timeout, byte_stream.next()) => match chunk {
                    Err(_) => {
                        self.on_idle_timeout();
                        Flow::Stop
                    }
                    Ok(Some(Ok(bytes))) => self.on_chunk(&bytes),
                    Ok(Some(Err(e))) => {
                        self.on_stream_error(&e);
                        Flow::Stop
                    }
                    Ok(None) => {
                        self.on_end();
                        Flow::Stop
                    }
                },
            };
            if matches!(flow, Flow::Stop) {
                return;
            }
        }
    }

    fn on_cancelled(&self) {
        let &GenerationProxyCtx {
            session_id,
            request_id,
            mode,
            ..
        } = self.ctx;
        info!(
            %session_id,
            %request_id,
            %mode,
            chunk_count = self.chunk_count,
            event_count = self.event_count,
            bytes_seen = self.bytes_seen,
            "Generation cancelled by client"
        );
    }

    fn on_idle_timeout(&self) {
        let &GenerationProxyCtx {
            session_id,
            request_id,
            mode,
            ..
        } = self.ctx;
        warn!(
            %session_id,
            %request_id,
            %mode,
            idle_timeout_secs = self.idle_timeout.as_secs(),
            chunk_count = self.chunk_count,
            event_count = self.event_count,
            bytes_seen = self.bytes_seen,
            "Generation proxy: upstream stream idle timeout"
        );
        self.ctx.send_error(
            "STREAM_IDLE_TIMEOUT".into(),
            format!(
                "upstream stream produced no data for {}s",
                self.idle_timeout.as_secs()
            ),
        );
    }

    fn on_chunk(&mut self, bytes: &[u8]) -> Flow {
        self.chunk_count += 1;
        self.bytes_seen += bytes.len() as u64;
        self.log_chunk(bytes.len());
        self.buffer.push_str(&String::from_utf8_lossy(bytes));
        while let Some(frame) = pop_sse_frame(&mut self.buffer) {
            let Some((event_type, msg)) = parse_sse_frame(&frame, self.ctx.mode) else {
                continue;
            };
            self.event_count += 1;
            log_generation_event(&self.event_log_ctx(&event_type), &msg);
            if self.ctx.outbound.try_send(msg).is_err() {
                let &GenerationProxyCtx {
                    session_id,
                    request_id,
                    mode,
                    ..
                } = self.ctx;
                warn!(
                    %session_id,
                    %request_id,
                    %mode,
                    %event_type,
                    "Generation proxy: outbound channel closed while forwarding event"
                );
                return Flow::Stop;
            }
        }
        Flow::Continue
    }

    fn log_chunk(&self, chunk_bytes: usize) {
        let &GenerationProxyCtx {
            session_id,
            request_id,
            mode,
            ..
        } = self.ctx;
        if self.chunk_count == 1 {
            info!(
                %session_id,
                %request_id,
                %mode,
                first_chunk_bytes = chunk_bytes,
                "Generation proxy: first upstream stream chunk received"
            );
        } else {
            debug!(
                %session_id,
                %request_id,
                %mode,
                chunk_count = self.chunk_count,
                chunk_bytes,
                bytes_seen = self.bytes_seen,
                "Generation proxy: upstream stream chunk received"
            );
        }
    }

    fn on_stream_error(&self, e: &reqwest::Error) {
        let &GenerationProxyCtx {
            session_id,
            request_id,
            mode,
            ..
        } = self.ctx;
        let e = ErrorChain(e);
        error!(
            %session_id,
            %request_id,
            %mode,
            error = %e,
            chunk_count = self.chunk_count,
            event_count = self.event_count,
            bytes_seen = self.bytes_seen,
            "Generation proxy: upstream stream error"
        );
        self.ctx
            .send_error("STREAM_ERROR".into(), format!("Stream error: {e}"));
    }

    fn on_end(&mut self) {
        // Flush remaining buffer
        if !self.buffer.trim().is_empty() {
            if let Some((event_type, msg)) = parse_sse_frame(&self.buffer, self.ctx.mode) {
                self.event_count += 1;
                log_generation_event(&self.event_log_ctx(&event_type), &msg);
                let _ = self.ctx.outbound.try_send(msg);
            }
        }
        let &GenerationProxyCtx {
            session_id,
            request_id,
            mode,
            ..
        } = self.ctx;
        info!(
            %session_id,
            %request_id,
            %mode,
            chunk_count = self.chunk_count,
            event_count = self.event_count,
            bytes_seen = self.bytes_seen,
            "Generation proxy: upstream stream ended"
        );
    }

    fn event_log_ctx<'e>(&'e self, event_type: &'e str) -> EventLogCtx<'e> {
        EventLogCtx {
            session_id: self.ctx.session_id,
            request_id: self.ctx.request_id,
            mode: self.ctx.mode,
            event_type,
            event_count: self.event_count,
        }
    }
}
