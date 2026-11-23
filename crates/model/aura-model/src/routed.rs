//! Provider that dispatches each request by its model id (see
//! [`crate::model_catalog`]): `local/*` to the local model server,
//! `claude/*` and `chatgpt/*` to the user's linked plan, `openrouter/*`
//! to OpenRouter with the user's OpenRouter key, everything else to
//! aura-router. Routes never fall back to one another, so the billing
//! target is always the one the user picked.

use std::sync::Arc;

use async_trait::async_trait;

use crate::model_catalog::{backend_for, local_model_name, ModelBackend, GATE_MODEL_ID};
use crate::{
    ModelName, ModelProvider, ModelRequest, ModelResponse, ReasonerError, ResponseEventStream,
    StreamEventStream,
};

type SharedProvider = Arc<dyn ModelProvider + Send + Sync>;

/// Routes requests to the backend their model id names.
pub struct RoutedProvider {
    local: SharedProvider,
    router: SharedProvider,
    claude: Option<SharedProvider>,
    chatgpt: Option<SharedProvider>,
    openrouter: Option<SharedProvider>,
}

impl RoutedProvider {
    /// Combine a local provider and a router provider.
    #[must_use]
    pub fn new(local: SharedProvider, router: SharedProvider) -> Self {
        Self {
            local,
            router,
            claude: None,
            chatgpt: None,
            openrouter: None,
        }
    }

    /// Serve `claude/*` models with `provider`.
    #[must_use]
    pub fn with_claude(mut self, provider: SharedProvider) -> Self {
        self.claude = Some(provider);
        self
    }

    /// Serve `chatgpt/*` models with `provider`.
    #[must_use]
    pub fn with_chatgpt(mut self, provider: SharedProvider) -> Self {
        self.chatgpt = Some(provider);
        self
    }

    /// Serve `openrouter/*` models with `provider`.
    #[must_use]
    pub fn with_openrouter(mut self, provider: SharedProvider) -> Self {
        self.openrouter = Some(provider);
        self
    }

    /// Pick the backend for `request`, rewriting local requests so the
    /// local server sees its own model name and never receives the JWT.
    fn route(
        &self,
        mut request: ModelRequest,
    ) -> Result<(&dyn ModelProvider, ModelRequest), ReasonerError> {
        if request.model.as_str() == GATE_MODEL_ID {
            return Err(ReasonerError::Internal(
                "`gate` must be resolved to a model before a request is sent".to_string(),
            ));
        }
        let backend = backend_for(request.model.as_str());
        let subscription = match backend {
            ModelBackend::Local => {
                if let Some(name) = local_model_name(request.model.as_str()) {
                    request.model = ModelName::from(name);
                }
                request.auth_token = None;
                request.upstream_provider_family = None;
                return Ok((self.local.as_ref(), request));
            }
            ModelBackend::Router => return Ok((self.router.as_ref(), request)),
            ModelBackend::Claude => self.claude.as_deref(),
            ModelBackend::Chatgpt => self.chatgpt.as_deref(),
            ModelBackend::OpenRouter => self.openrouter.as_deref(),
        };
        subscription
            .map(|provider| (provider as &dyn ModelProvider, request))
            .ok_or_else(|| ReasonerError::Api {
                status: 401,
                message: format!(
                    "{} models are not available in this session",
                    backend.label()
                ),
            })
    }
}

#[async_trait]
impl ModelProvider for RoutedProvider {
    fn name(&self) -> &'static str {
        "aura"
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        let (provider, request) = self.route(request)?;
        provider.complete(request).await
    }

    async fn complete_streaming(
        &self,
        request: ModelRequest,
    ) -> Result<StreamEventStream, ReasonerError> {
        let (provider, request) = self.route(request)?;
        provider.complete_streaming(request).await
    }

    async fn complete_response_stream(
        &self,
        request: ModelRequest,
    ) -> Result<ResponseEventStream, ReasonerError> {
        let (provider, request) = self.route(request)?;
        provider.complete_response_stream(request).await
    }

    async fn health_check(&self) -> bool {
        self.router.health_check().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, MockProvider, ModelRequestKind};

    fn request(model: &str) -> ModelRequest {
        ModelRequest::builder(model, "system")
            .message(Message::user("hi"))
            .request_kind(ModelRequestKind::Chat)
            .auth_token(Some("jwt".to_string()))
            .try_build()
            .expect("valid request")
    }

    fn routed() -> RoutedProvider {
        RoutedProvider::new(
            Arc::new(MockProvider::simple_response("local")),
            Arc::new(MockProvider::simple_response("router")),
        )
    }

    #[test]
    fn local_requests_strip_prefix_and_jwt() {
        let provider = routed();
        let (target, rewritten) = provider.route(request("local/qwen3:4b")).unwrap();
        assert!(std::ptr::eq(
            std::ptr::from_ref(target).cast::<u8>(),
            Arc::as_ptr(&provider.local).cast::<u8>()
        ));
        assert_eq!(rewritten.model.as_str(), "qwen3:4b");
        assert_eq!(rewritten.auth_token, None);
    }

    #[test]
    fn unresolved_gate_is_refused() {
        let err = routed().route(request(GATE_MODEL_ID)).err().unwrap();
        assert!(matches!(err, ReasonerError::Internal(_)), "{err}");
    }

    #[tokio::test]
    async fn local_model_streams_sse_from_anthropic_compatible_server() {
        use crate::{AnthropicConfig, AnthropicProvider, StreamEvent};
        use futures_util::StreamExt;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let events = [
            r#"{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"qwen3:4b","stop_reason":null,"usage":{"input_tokens":3,"output_tokens":0}}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi from local"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}"#,
            r#"{"type":"message_stop"}"#,
        ];
        let body: String = events
            .iter()
            .map(|data| {
                let kind = serde_json::from_str::<serde_json::Value>(data).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_string();
                format!("event: {kind}\ndata: {data}\n\n")
            })
            .collect();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 65536];
            let n = socket.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            request
        });

        let local_cfg = AnthropicConfig {
            base_url: format!("http://{addr}"),
            max_retries: 0,
            ..AnthropicConfig::local_from_env()
        };
        let local = AnthropicProvider::new(local_cfg)
            .unwrap()
            .with_sse_for_all_models();
        let provider = RoutedProvider::new(
            Arc::new(local),
            Arc::new(MockProvider::simple_response("router")),
        );

        let mut stream = provider
            .complete_streaming(request("local/qwen3:4b"))
            .await
            .unwrap();
        let mut text = String::new();
        while let Some(event) = stream.next().await {
            if let StreamEvent::TextDelta { text: delta } = event.unwrap() {
                text.push_str(&delta);
            }
        }
        assert_eq!(text, "hi from local");

        let raw_request = server.await.unwrap().to_ascii_lowercase();
        assert!(raw_request.starts_with("post /v1/messages"));
        assert!(raw_request.contains(r#""model":"qwen3:4b""#));
        assert!(raw_request.contains(r#""stream":true"#));
        assert!(!raw_request.contains("authorization:"));
        assert!(!raw_request.contains("cache_control"));
    }

    #[tokio::test]
    async fn direct_claude_requests_omit_router_only_fields() {
        use crate::{AnthropicConfig, AnthropicProvider, ThinkingEffort};
        use futures_util::StreamExt;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-opus-4-7\",\"stop_reason\":null,\"usage\":{\"input_tokens\":3,\"output_tokens\":0}}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        );
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut raw = Vec::new();
            let mut buf = vec![0u8; 65536];
            loop {
                let n = socket.read(&mut buf).await.unwrap();
                raw.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&raw).to_ascii_lowercase();
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .map_or(0, |v| v.trim().parse::<usize>().unwrap());
                    if raw.len() >= end + 4 + length || n == 0 {
                        break;
                    }
                }
            }
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                         anthropic-ratelimit-unified-5h-utilization: 0.07\r\n\
                         anthropic-ratelimit-unified-5h-reset: 1774933200\r\n\
                         anthropic-ratelimit-unified-7d-utilization: 0.53\r\n\
                         Content-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            String::from_utf8_lossy(&raw).to_string()
        });

        let config = AnthropicConfig {
            base_url: format!("http://{addr}"),
            max_retries: 0,
            ..AnthropicConfig::anthropic_direct()
        };
        let claude = AnthropicProvider::new(config)
            .unwrap()
            .without_router_body_extensions();
        let mut req = request("claude-opus-4-7");
        req.thinking_effort = Some(ThinkingEffort::Medium);
        req.system_preamble = Some("You are Claude Code.".to_string());
        let mut stream = claude.complete_streaming(req).await.unwrap();
        let mut accumulator = crate::StreamAccumulator::new();
        while let Some(event) = stream.next().await {
            accumulator.process(&event.unwrap());
        }
        let windows: Vec<(crate::Plan, &str, f64, Option<u64>)> = accumulator
            .plan_usage
            .iter()
            .map(|w| (w.plan, w.window.as_str(), w.used, w.resets_at))
            .collect();
        assert_eq!(
            windows,
            [
                (crate::Plan::Claude, "5h", 0.07, Some(1_774_933_200)),
                (crate::Plan::Claude, "7d", 0.53, None),
            ]
        );

        let raw = server.await.unwrap();
        assert!(raw.contains(r#""model":"claude-opus-4-7""#), "{raw}");
        assert!(!raw.contains("reasoning_effort"), "{raw}");
        let body: serde_json::Value =
            serde_json::from_str(raw.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["system"][0]["text"], "You are Claude Code.", "{raw}");
        assert_eq!(body["system"][1]["text"], "system", "{raw}");
    }

    #[test]
    fn router_requests_keep_model_and_jwt() {
        let provider = routed();
        let (target, rewritten) = provider.route(request("aura-claude-sonnet-4-6")).unwrap();
        assert!(std::ptr::eq(
            std::ptr::from_ref(target).cast::<u8>(),
            Arc::as_ptr(&provider.router).cast::<u8>()
        ));
        assert_eq!(rewritten.model.as_str(), "aura-claude-sonnet-4-6");
        assert_eq!(rewritten.auth_token.as_deref(), Some("jwt"));
    }
    #[test]
    fn subscription_routes_never_fall_back_to_router() {
        let provider = routed();
        let err = provider
            .route(request("claude/claude-sonnet-4-6"))
            .err()
            .expect("no claude backend configured");
        assert!(matches!(err, ReasonerError::Api { status: 401, .. }));

        let provider = routed().with_chatgpt(Arc::new(MockProvider::simple_response("chatgpt")));
        let (target, rewritten) = provider.route(request("chatgpt/gpt-5.4")).unwrap();
        assert!(std::ptr::eq(
            std::ptr::from_ref(target).cast::<u8>(),
            Arc::as_ptr(provider.chatgpt.as_ref().unwrap()).cast::<u8>()
        ));
        assert_eq!(rewritten.model.as_str(), "chatgpt/gpt-5.4");
    }

    #[test]
    fn openrouter_models_route_to_openrouter_only() {
        let model = "openrouter/anthropic/claude-sonnet-4.6";
        let err = routed()
            .route(request(model))
            .err()
            .expect("no openrouter backend configured");
        assert!(matches!(err, ReasonerError::Api { status: 401, .. }));

        let provider =
            routed().with_openrouter(Arc::new(MockProvider::simple_response("openrouter")));
        let (target, rewritten) = provider.route(request(model)).unwrap();
        assert!(std::ptr::eq(
            std::ptr::from_ref(target).cast::<u8>(),
            Arc::as_ptr(provider.openrouter.as_ref().unwrap()).cast::<u8>()
        ));
        assert_eq!(rewritten.model.as_str(), model);
    }

    struct OpenRouterKey;

    #[async_trait]
    impl aura_core_auth::CredentialResolver for OpenRouterKey {
        async fn inference_credential(
            &self,
            provider: aura_core_auth::ProviderKind,
        ) -> Result<aura_core_auth::InferenceCredential, aura_core_auth::AuthError> {
            Ok(aura_core_auth::InferenceCredential {
                provider,
                bearer: "sk-or-test-key".into(),
                headers: vec![("X-Title".into(), "Aura".into())],
                system_preamble: None,
            })
        }
    }

    #[tokio::test]
    async fn openrouter_requests_use_openrouter_key_and_upstream_model() {
        use crate::credentialed::CredentialedProvider;
        use crate::{AnthropicConfig, AnthropicProvider};
        use futures_util::StreamExt;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"anthropic/claude-sonnet-4.6\",\"stop_reason\":null,\"usage\":{\"input_tokens\":3,\"output_tokens\":0}}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        );
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut raw = Vec::new();
            let mut buf = vec![0u8; 65536];
            loop {
                let n = socket.read(&mut buf).await.unwrap();
                raw.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&raw).to_ascii_lowercase();
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .map_or(0, |v| v.trim().parse::<usize>().unwrap());
                    if raw.len() >= end + 4 + length || n == 0 {
                        break;
                    }
                }
            }
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            String::from_utf8_lossy(&raw).to_string()
        });

        let config = AnthropicConfig {
            base_url: format!("http://{addr}"),
            max_retries: 0,
            ..AnthropicConfig::openrouter()
        };
        let openrouter = AnthropicProvider::new(config)
            .unwrap()
            .with_sse_for_all_models()
            .without_router_body_extensions();
        let openrouter = CredentialedProvider::new(
            aura_core_auth::ProviderKind::OpenRouter,
            Arc::new(openrouter),
            Arc::new(OpenRouterKey),
        );
        let provider = routed().with_openrouter(Arc::new(openrouter));

        let mut req = request("openrouter/anthropic/claude-sonnet-4.6");
        req.aura_project_id = Some("project-1".to_string());
        let mut stream = provider.complete_streaming(req).await.unwrap();
        while let Some(event) = stream.next().await {
            event.unwrap();
        }

        let raw = server.await.unwrap();
        let lower = raw.to_ascii_lowercase();
        assert!(lower.starts_with("post /v1/messages"), "{raw}");
        let body: serde_json::Value =
            serde_json::from_str(raw.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["model"], "anthropic/claude-sonnet-4.6", "{raw}");
        assert!(
            lower.contains("authorization: bearer sk-or-test-key"),
            "{raw}"
        );
        assert!(lower.contains("x-title: aura"), "{raw}");
        assert!(!lower.contains("x-aura-"), "{raw}");
        assert!(!lower.contains("jwt"), "{raw}");
    }
}
