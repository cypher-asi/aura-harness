//! OpenAI Responses API client for Sign in with ChatGPT.
//!
//! Used behind [`crate::CredentialedProvider`], which supplies the
//! ChatGPT access token. Always streams with `store: false`; reasoning
//! state is carried between turns inside thinking-block signatures.

mod convert;
mod decode;
#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde_json::Value;

use crate::{
    ContentBlock, ModelProvider, ModelRequest, ModelResponse, ReasonerError, StreamAccumulator,
    StreamEvent, StreamEventStream,
};
use decode::Decoder;

/// Default API base; override with `AURA_CHATGPT_API_URL`.
pub const DEFAULT_API_BASE: &str = "https://api.openai.com/v1";

/// Namespace wrapping the harness's function tools.
pub(crate) const TOOL_NAMESPACE: &str = "aura";

const REASONING_PREFIX: &str = "openai-responses:";
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const ERROR_PREVIEW_CHARS: usize = 500;

/// Whether `block` is replayable OpenAI reasoning (and therefore must
/// not be sent to other providers).
#[must_use]
pub fn is_openai_reasoning(block: &ContentBlock) -> bool {
    matches!(
        block,
        ContentBlock::Thinking { signature: Some(sig), .. } if sig.starts_with(REASONING_PREFIX)
    )
}

fn encode_reasoning(items: &[Value]) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let json = serde_json::to_string(items).ok()?;
    Some(format!("{REASONING_PREFIX}{json}"))
}

fn reasoning_items(signature: &str) -> Option<Vec<Value>> {
    let json = signature.strip_prefix(REASONING_PREFIX)?;
    serde_json::from_str(json).ok()
}

/// Streams `POST {base}/responses`.
pub struct OpenAiResponsesProvider {
    client: reqwest::Client,
    base_url: String,
}

impl OpenAiResponsesProvider {
    /// # Errors
    ///
    /// Returns an error if the HTTP client cannot be built.
    pub fn new(base_url: impl Into<String>) -> Result<Self, ReasonerError> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| ReasonerError::Internal(format!("http client: {e}")))?;
        Ok(Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_string(),
        })
    }

    /// Base URL from `AURA_CHATGPT_API_URL`, else [`DEFAULT_API_BASE`].
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client cannot be built.
    pub fn from_env() -> Result<Self, ReasonerError> {
        let base = std::env::var("AURA_CHATGPT_API_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_API_BASE.to_string());
        Self::new(base)
    }

    async fn open(&self, request: &ModelRequest) -> Result<reqwest::Response, ReasonerError> {
        let token = request
            .auth_token
            .as_deref()
            .ok_or_else(|| ReasonerError::Api {
                status: 401,
                message: "no ChatGPT credential attached to the request".into(),
            })?;
        let mut builder = self
            .client
            .post(format!("{}/responses", self.base_url))
            .bearer_auth(token)
            .header("accept", "text/event-stream")
            .json(&convert::request_body(request));
        for (name, value) in &request.extra_headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let response = tokio::time::timeout(IDLE_TIMEOUT, builder.send())
            .await
            .map_err(|_| ReasonerError::Timeout)?
            .map_err(|e| ReasonerError::Request(e.without_url().to_string()))?;
        if response.status().is_success() {
            return Ok(response);
        }
        Err(http_error(response).await)
    }
}

async fn http_error(response: reqwest::Response) -> ReasonerError {
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    let body = tokio::time::timeout(Duration::from_secs(30), response.text())
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    let detail = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.get("detail"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.chars().take(ERROR_PREVIEW_CHARS).collect());
    let message = format!("ChatGPT: {detail}");
    match status {
        429 => ReasonerError::RateLimited {
            message,
            retry_after,
        },
        500..=599 => ReasonerError::Transient {
            status,
            message,
            retry_after,
        },
        _ => ReasonerError::Api { status, message },
    }
}

type ByteStream = std::pin::Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>;

struct SseState {
    body: ByteStream,
    buffer: Vec<u8>,
    decoder: Decoder,
    pending: VecDeque<StreamEvent>,
    done: bool,
}

impl SseState {
    fn drain_blocks(&mut self, flush: bool) {
        loop {
            let normalized = self.buffer.windows(2).position(|w| w == b"\n\n");
            let (end, skip) = match normalized {
                Some(pos) => (pos, 2),
                None if flush && !self.buffer.is_empty() => (self.buffer.len(), 0),
                None => return,
            };
            let block: Vec<u8> = self.buffer.drain(..end + skip).take(end).collect();
            self.decode_block(&String::from_utf8_lossy(&block));
        }
    }

    fn decode_block(&mut self, block: &str) {
        let data: Vec<&str> = block
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(|rest| rest.strip_prefix(' ').unwrap_or(rest))
            .collect();
        if data.is_empty() {
            return;
        }
        let data = data.join("\n");
        if data.trim() == "[DONE]" {
            return;
        }
        match serde_json::from_str::<Value>(&data) {
            Ok(event) => self.pending.extend(self.decoder.event(&event)),
            Err(e) => tracing::warn!(error = %e, "ignoring malformed Responses SSE frame"),
        }
    }

    fn end(&mut self, reason: &str) {
        self.done = true;
        self.drain_blocks(true);
        if !self.decoder.finished() {
            self.pending.push_back(StreamEvent::Error {
                message: reason.to_string(),
                request_id: None,
            });
        }
    }
}

fn sse_stream(response: reqwest::Response, model: &str) -> StreamEventStream {
    let request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let mut pending = VecDeque::new();
    pending.push_back(StreamEvent::HttpMeta { request_id });
    let state = SseState {
        body: Box::pin(response.bytes_stream()),
        buffer: Vec::new(),
        decoder: Decoder::new(model),
        pending,
        done: false,
    };
    Box::pin(futures_util::stream::unfold(state, |mut s| async move {
        loop {
            if let Some(event) = s.pending.pop_front() {
                return Some((Ok(event), s));
            }
            if s.done {
                return None;
            }
            match tokio::time::timeout(IDLE_TIMEOUT, s.body.next()).await {
                Ok(Some(Ok(chunk))) => {
                    s.buffer
                        .extend(chunk.iter().copied().filter(|&b| b != b'\r'));
                    s.drain_blocks(false);
                }
                Ok(Some(Err(e))) => s.end(&format!("ChatGPT stream: {}", e.without_url())),
                Ok(None) => s.end("ChatGPT stream ended before the response completed"),
                Err(_) => s.end("ChatGPT stream idle for too long"),
            }
        }
    }))
}

#[async_trait]
impl ModelProvider for OpenAiResponsesProvider {
    fn name(&self) -> &'static str {
        "openai-responses"
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        let started = Instant::now();
        let mut stream = self.complete_streaming(request).await?;
        let mut accumulator = StreamAccumulator::new();
        while let Some(event) = stream.next().await {
            accumulator.process(&event?);
        }
        let latency = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        accumulator.into_response(0, latency)
    }

    async fn complete_streaming(
        &self,
        request: ModelRequest,
    ) -> Result<StreamEventStream, ReasonerError> {
        let response = self.open(&request).await?;
        let is_json = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("json"));
        if !is_json {
            return Ok(sse_stream(response, request.model.as_str()));
        }
        let body: Value = response
            .json()
            .await
            .map_err(|e| ReasonerError::Parse(format!("ChatGPT response: {e}")))?;
        let events = Decoder::new(request.model.as_str()).response_object(&body);
        Ok(Box::pin(futures_util::stream::iter(
            events.into_iter().map(Ok),
        )))
    }

    async fn health_check(&self) -> bool {
        true
    }
}
