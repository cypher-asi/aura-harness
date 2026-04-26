use async_trait::async_trait;
use reqwest::header::HeaderMap;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

use aura_core::AgentId;
use aura_kernel::{ChildAgentSpec, KernelSpawnHook, SpawnError, SpawnHook, SpawnOutcome};
use aura_store::Store;
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

pub(crate) struct AuraServerSpawnHook {
    base_url: String,
    auth_token: Option<String>,
    org_id: Option<String>,
    client: reqwest::Client,
    kernel: KernelSpawnHook,
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

impl AuraServerSpawnHook {
    pub(crate) fn new(
        base_url: impl Into<String>,
        auth_token: Option<String>,
        org_id: Option<String>,
        store: Arc<dyn Store>,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            auth_token,
            org_id,
            client: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            kernel: KernelSpawnHook::new(store),
        }
    }

    fn bearer(&self) -> Result<&str, SpawnError> {
        self.auth_token
            .as_deref()
            .filter(|token| !token.trim().is_empty())
            .ok_or_else(|| {
                SpawnError::Other("missing bearer token for aura-os-server callback".into())
            })
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }

    async fn create_aura_os_agent(&self, child: &ChildAgentSpec) -> Result<String, SpawnError> {
        let response = self
            .client
            .post(self.endpoint("/api/agents"))
            .bearer_auth(self.bearer()?)
            .json(&json!({
                "org_id": self.org_id,
                "name": child.name,
                "role": child.role,
                "personality": "",
                "system_prompt": child.system_prompt_override.clone().unwrap_or_default(),
                "skills": [],
                "icon": null,
                "machine_type": "swarm",
                "adapter_type": "aura_harness",
                "permissions": child.permissions,
            }))
            .send()
            .await
            .map_err(|e| {
                SpawnError::Other(format!("spawn_agent: aura-os-server callback failed: {e}"))
            })?;
        if !response.status().is_success() {
            let message = AuraServerAgentHook {
                base_url: self.base_url.clone(),
                auth_token: self.auth_token.clone(),
                client: self.client.clone(),
            }
            .error_from_response("spawn_agent", response)
            .await;
            return Err(SpawnError::Other(message));
        }
        let body = response.json::<Value>().await.map_err(|e| {
            SpawnError::Other(format!("spawn_agent: invalid aura-os-server JSON: {e}"))
        })?;
        body.get("agent_id")
            .or_else(|| body.get("agentId"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                SpawnError::Other("spawn_agent: aura-os-server response missing agent_id".into())
            })
    }
}

#[async_trait]
impl SpawnHook for AuraServerSpawnHook {
    async fn spawn_child(
        &self,
        parent_agent_id: &AgentId,
        originating_user_id: Option<&str>,
        mut child: ChildAgentSpec,
    ) -> Result<SpawnOutcome, SpawnError> {
        let external_agent_id = self.create_aura_os_agent(&child).await?;
        if let Ok(uuid) = uuid::Uuid::parse_str(&external_agent_id) {
            child.preassigned_agent_id = Some(AgentId::from_uuid(uuid));
        }
        let mut outcome = self
            .kernel
            .spawn_child(parent_agent_id, originating_user_id, child)
            .await?;
        outcome.external_agent_id = Some(external_agent_id);
        Ok(outcome)
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
        target_agent_id: &str,
        _parent_agent_id: Option<&str>,
        _originating_user_id: Option<&str>,
        action: &str,
    ) -> Result<(), String> {
        let actual_action = match action {
            "pause" => "hibernate",
            "resume" => "wake",
            other => other,
        };
        let url = self.endpoint(&format!(
            "/api/agents/{target_agent_id}/remote_agent/{actual_action}"
        ));
        let response = self
            .client
            .post(url)
            .bearer_auth(self.bearer()?)
            .send()
            .await
            .map_err(|e| format!("agent_lifecycle: aura-os-server callback failed: {e}"))?;
        if !response.status().is_success() {
            return Err(self.error_from_response("agent_lifecycle", response).await);
        }
        Ok(())
    }

    async fn delegate_task(
        &self,
        target_agent_id: &str,
        _parent_agent_id: Option<&str>,
        _originating_user_id: Option<&str>,
        task: &str,
        context: Option<&Value>,
    ) -> Result<(), String> {
        let url = self.endpoint(&format!("/api/agents/{target_agent_id}/delegate_task"));
        let response = self
            .client
            .post(url)
            .bearer_auth(self.bearer()?)
            .json(&json!({
                "task": task,
                "context": context
            }))
            .send()
            .await
            .map_err(|e| format!("delegate_task: aura-os-server callback failed: {e}"))?;
        if !response.status().is_success() {
            return Err(self.error_from_response("delegate_task", response).await);
        }

        self.deliver_message(
            target_agent_id,
            None,
            None,
            &format!("Delegated task:\n\n{task}"),
            context.cloned(),
        )
        .await
    }
}

#[async_trait]
impl AgentReadHook for AuraServerAgentHook {
    async fn list_agents(&self, org_id: Option<&str>) -> Result<Value, String> {
        let url = self.endpoint("/api/agents");
        let mut request = self.client.get(url).bearer_auth(self.bearer()?);
        if let Some(org_id) = org_id {
            request = request.query(&[("org_id", org_id)]);
        }
        let response = request
            .send()
            .await
            .map_err(|e| format!("list_agents: aura-os-server callback failed: {e}"))?;
        if !response.status().is_success() {
            return Err(self.error_from_response("list_agents", response).await);
        }
        response
            .json::<Value>()
            .await
            .map_err(|e| format!("list_agents: invalid aura-os-server JSON: {e}"))
    }

    async fn snapshot(&self, target_agent_id: &str) -> Result<Value, String> {
        let url = self.endpoint(&format!("/api/agents/{target_agent_id}/state_snapshot"));
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
