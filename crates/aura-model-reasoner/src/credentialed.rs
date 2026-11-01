//! Provider wrapper that signs each request with a personal-plan
//! credential (Claude Pro/Max, ChatGPT) instead of the zOS JWT.

use std::sync::Arc;

use async_trait::async_trait;
use aura_core_auth::{AuthError, CredentialResolver, ProviderKind};

use crate::model_catalog::upstream_model_name;
use crate::{
    ModelName, ModelProvider, ModelRequest, ModelResponse, ReasonerError, ResponseEventStream,
    StreamEventStream,
};

/// Resolves the credential for `kind` before every call to `inner`.
pub struct CredentialedProvider {
    kind: ProviderKind,
    inner: Arc<dyn ModelProvider + Send + Sync>,
    resolver: Arc<dyn CredentialResolver>,
}

impl CredentialedProvider {
    #[must_use]
    pub fn new(
        kind: ProviderKind,
        inner: Arc<dyn ModelProvider + Send + Sync>,
        resolver: Arc<dyn CredentialResolver>,
    ) -> Self {
        Self {
            kind,
            inner,
            resolver,
        }
    }

    async fn prepare(&self, mut request: ModelRequest) -> Result<ModelRequest, ReasonerError> {
        let credential = self
            .resolver
            .inference_credential(self.kind)
            .await
            .map_err(auth_to_reasoner)?;

        request.model = ModelName::from(upstream_model_name(request.model.as_str()));
        if let Some(preamble) = credential.system_preamble.as_deref() {
            request.system = if request.system.is_empty() {
                preamble.to_string()
            } else {
                format!("{preamble}\n\n{}", request.system)
            };
        }
        request.auth_token = Some(credential.bearer);
        request.extra_headers = credential.headers;
        // Aura billing and routing metadata must not leak to a third party.
        request.upstream_provider_family = None;
        request.aura_project_id = None;
        request.aura_agent_id = None;
        request.aura_session_id = None;
        request.aura_org_id = None;
        Ok(request)
    }
}

fn auth_to_reasoner(err: AuthError) -> ReasonerError {
    match err {
        AuthError::NotLinked(_)
        | AuthError::ReauthRequired(_)
        | AuthError::Declined(_)
        | AuthError::Invalid(_) => ReasonerError::Api {
            status: 401,
            message: err.to_string(),
        },
        AuthError::Unavailable(_) => ReasonerError::Transient {
            status: 503,
            message: err.to_string(),
            retry_after: None,
        },
        other => ReasonerError::Internal(other.to_string()),
    }
}

#[async_trait]
impl ModelProvider for CredentialedProvider {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        let request = self.prepare(request).await?;
        self.inner.complete(request).await
    }

    async fn complete_streaming(
        &self,
        request: ModelRequest,
    ) -> Result<StreamEventStream, ReasonerError> {
        let request = self.prepare(request).await?;
        self.inner.complete_streaming(request).await
    }

    async fn complete_response_stream(
        &self,
        request: ModelRequest,
    ) -> Result<ResponseEventStream, ReasonerError> {
        let request = self.prepare(request).await?;
        self.inner.complete_response_stream(request).await
    }

    async fn health_check(&self) -> bool {
        self.inner.health_check().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_auth::InferenceCredential;
    use std::sync::Mutex;

    use crate::{Message, ModelRequestKind};

    struct Recorder(Mutex<Option<ModelRequest>>);

    #[async_trait]
    impl ModelProvider for Recorder {
        fn name(&self) -> &'static str {
            "recorder"
        }
        async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
            *self.0.lock().unwrap() = Some(request);
            Err(ReasonerError::Internal("recorded".into()))
        }
        async fn health_check(&self) -> bool {
            true
        }
    }

    struct Fixed(Result<InferenceCredential, fn() -> AuthError>);

    #[async_trait]
    impl CredentialResolver for Fixed {
        async fn inference_credential(
            &self,
            _provider: ProviderKind,
        ) -> Result<InferenceCredential, AuthError> {
            match &self.0 {
                Ok(c) => Ok(c.clone()),
                Err(f) => Err(f()),
            }
        }
    }

    fn request(model: &str) -> ModelRequest {
        let mut r = ModelRequest::builder(model, "be helpful")
            .message(Message::user("hi"))
            .request_kind(ModelRequestKind::Chat)
            .auth_token(Some("zos-jwt".to_string()))
            .try_build()
            .expect("valid request");
        r.aura_project_id = Some("p".into());
        r.upstream_provider_family = Some("anthropic".into());
        r
    }

    #[tokio::test]
    async fn rewrites_request_for_subscription() {
        let recorder = Arc::new(Recorder(Mutex::new(None)));
        let credential = InferenceCredential {
            provider: ProviderKind::Claude,
            bearer: "sub-token".into(),
            headers: vec![("anthropic-beta".into(), "oauth-2025-04-20".into())],
            system_preamble: Some("You are Claude Code.".into()),
        };
        let provider = CredentialedProvider::new(
            ProviderKind::Claude,
            recorder.clone(),
            Arc::new(Fixed(Ok(credential))),
        );
        let _ = provider.complete(request("claude/claude-sonnet-4-6")).await;
        let seen = recorder.0.lock().unwrap().take().expect("forwarded");
        assert_eq!(seen.model.as_str(), "claude-sonnet-4-6");
        assert_eq!(seen.auth_token.as_deref(), Some("sub-token"));
        assert_eq!(seen.system, "You are Claude Code.\n\nbe helpful");
        assert_eq!(seen.extra_headers.len(), 1);
        assert!(seen.aura_project_id.is_none());
        assert!(seen.upstream_provider_family.is_none());
    }

    #[tokio::test]
    async fn missing_link_is_an_auth_error_and_never_forwards() {
        let recorder = Arc::new(Recorder(Mutex::new(None)));
        let provider = CredentialedProvider::new(
            ProviderKind::Chatgpt,
            recorder.clone(),
            Arc::new(Fixed(Err(|| AuthError::NotLinked(ProviderKind::Chatgpt)))),
        );
        let err = provider
            .complete(request("chatgpt/gpt-5.4"))
            .await
            .unwrap_err();
        assert!(matches!(err, ReasonerError::Api { status: 401, .. }));
        assert!(recorder.0.lock().unwrap().is_none());

        let provider = CredentialedProvider::new(
            ProviderKind::Chatgpt,
            recorder.clone(),
            Arc::new(Fixed(Err(|| AuthError::Unavailable("down".into())))),
        );
        let err = provider
            .complete(request("chatgpt/gpt-5.4"))
            .await
            .unwrap_err();
        assert!(matches!(err, ReasonerError::Transient { status: 503, .. }));
    }
}
