use async_trait::async_trait;
use reqwest::header::HeaderMap;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

use aura_tools::{AgentControlHook, AgentReadHook};

const CHAT_PERSISTED_HEADER: &str = "x-aura-chat-persisted";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Runtime bridge from harness-native cross-agent tools back into aura-os.
///
/// The aura-tools crate owns permission gates. This hook owns the production
/// side effect: call aura-os-server with the session user's JWT so the target
/// agent receives the same chat turn the UI endpoint would have sent.
pub(crate) struct AuraServerAgentHook {
    base_url: String,
    auth_token: Option<String>,
    client: reqwest::Client,
}

impl AuraServerAgentHook {
    pub(crate) fn new(base_url: impl Into<String>, auth_token: Option<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            auth_token,
            client: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    fn bearer(&self) -> Result<&str, String> {
        self.auth_token
            .as_deref()
            .filter(|token| !token.trim().is_empty())
            .ok_or_else(|| "missing bearer token for aura-os-server callback".to_string())
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }

    async fn error_from_response(&self, prefix: &str, response: reqwest::Response) -> String {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if let Ok(api_error) = serde_json::from_str::<AuraOsApiError>(&body) {
            let details = api_error
                .details
                .or(api_error.error)
                .unwrap_or_else(|| "aura-os-server returned an error".to_string());
            return format!("{prefix}: {}: {details}", api_error.code);
        }
        if body.trim().is_empty() {
            format!("{prefix}: aura-os-server returned HTTP {status}")
        } else {
            format!("{prefix}: aura-os-server returned HTTP {status}: {body}")
        }
    }
}

#[async_trait]
impl AgentControlHook for AuraServerAgentHook {
    async fn deliver_message(
        &self,
        target_agent_id: &str,
        _parent_agent_id: Option<&str>,
        _originating_user_id: Option<&str>,
        content: &str,
        attachments: Option<Value>,
    ) -> Result<(), String> {
        let url = self.endpoint(&format!("/api/agents/{target_agent_id}/events/stream"));
        let response = self
            .client
            .post(url)
            .bearer_auth(self.bearer()?)
            .json(&json!({
                "content": content,
                "action": null,
                "model": null,
                "commands": null,
                "project_id": null,
                "attachments": attachments,
                "new_session": false
            }))
            .send()
            .await
            .map_err(|e| format!("send_to_agent: aura-os-server callback failed: {e}"))?;

        let status = response.status();
        let headers = response.headers().clone();
        if !status.is_success() {
            return Err(self.error_from_response("send_to_agent", response).await);
        }
        require_persisted_header(&headers)
    }

    async fn lifecycle(
        &self,
        _target_agent_id: &str,
        _parent_agent_id: Option<&str>,
        _originating_user_id: Option<&str>,
        _action: &str,
    ) -> Result<(), String> {
        Err("agent_lifecycle: aura-os-server lifecycle callback is not wired".to_string())
    }

    async fn delegate_task(
        &self,
        _target_agent_id: &str,
        _parent_agent_id: Option<&str>,
        _originating_user_id: Option<&str>,
        _task: &str,
        _context: Option<&Value>,
    ) -> Result<(), String> {
        Err("delegate_task: aura-os-server delegate callback is not wired".to_string())
    }
}

#[async_trait]
impl AgentReadHook for AuraServerAgentHook {
    async fn snapshot(&self, target_agent_id: &str) -> Result<Value, String> {
        let url = self.endpoint(&format!("/api/agents/{target_agent_id}/remote_agent/state"));
        let response = self
            .client
            .get(url)
            .bearer_auth(self.bearer()?)
            .send()
            .await
            .map_err(|e| format!("get_agent_state: aura-os-server callback failed: {e}"))?;
        if !response.status().is_success() {
            return Err(self.error_from_response("get_agent_state", response).await);
        }
        response
            .json::<Value>()
            .await
            .map_err(|e| format!("get_agent_state: invalid aura-os-server JSON: {e}"))
    }
}

fn require_persisted_header(headers: &HeaderMap) -> Result<(), String> {
    match headers
        .get(CHAT_PERSISTED_HEADER)
        .and_then(|value| value.to_str().ok())
    {
        Some("true") => Ok(()),
        Some(value) => Err(format!(
            "send_to_agent: aura-os-server accepted the stream but reported {CHAT_PERSISTED_HEADER}: {value}"
        )),
        None => Err(format!(
            "send_to_agent: aura-os-server response missing {CHAT_PERSISTED_HEADER}"
        )),
    }
}

#[derive(Debug, Deserialize)]
struct AuraOsApiError {
    error: Option<String>,
    code: String,
    details: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};

    #[test]
    fn persisted_header_accepts_true() {
        let mut headers = HeaderMap::new();
        headers.insert(CHAT_PERSISTED_HEADER, HeaderValue::from_static("true"));
        assert!(require_persisted_header(&headers).is_ok());
    }

    #[test]
    fn persisted_header_rejects_false() {
        let mut headers = HeaderMap::new();
        headers.insert(CHAT_PERSISTED_HEADER, HeaderValue::from_static("false"));
        let err = require_persisted_header(&headers).unwrap_err();
        assert!(err.contains("reported"), "got: {err}");
    }

    #[test]
    fn persisted_header_rejects_missing() {
        let err = require_persisted_header(&HeaderMap::new()).unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }
}
