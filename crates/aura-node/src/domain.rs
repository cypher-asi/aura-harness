//! HTTP-backed `DomainApi` implementation.
//!
//! Routes to the correct endpoint + auth based on the operation:
//! - `/api/` routes use `Authorization: Bearer <jwt>` (user JWT from front-end)
//! - `/internal/` routes use `X-Internal-Token` header (service-to-service)

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use aura_tools::domain_tools::{
    CreateSessionParams, DomainApi, MessageDescriptor, ProjectDescriptor, ProjectUpdate,
    SaveMessageParams, SessionDescriptor, SpecDescriptor, TaskDescriptor, TaskUpdate,
};
use reqwest::Client;
use serde::de::DeserializeOwned;
use tracing::{debug, warn};

pub struct HttpDomainApi {
    http: Client,
    storage_url: String,
    network_url: String,
    orbit_url: String,
    internal_token: String,
}

impl HttpDomainApi {
    pub fn new(storage_url: &str, network_url: &str, orbit_url: &str, internal_token: &str) -> Self {
        Self {
            http: Client::new(),
            storage_url: storage_url.trim_end_matches('/').to_string(),
            network_url: network_url.trim_end_matches('/').to_string(),
            orbit_url: orbit_url.trim_end_matches('/').to_string(),
            internal_token: internal_token.to_string(),
        }
    }

    // -------------------------------------------------------------------------
    // Internal-token helpers (X-Internal-Token, for /internal/ routes)
    // -------------------------------------------------------------------------

    #[allow(dead_code)]
    async fn internal_get<T: DeserializeOwned>(&self, url: &str) -> anyhow::Result<T> {
        debug!(url, "HttpDomainApi internal GET");
        let resp = self
            .http
            .get(url)
            .header("X-Internal-Token", &self.internal_token)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            let truncated: String = body.chars().take(300).collect();
            return Err(anyhow!("HTTP {status}: {truncated}"));
        }
        serde_json::from_str(&body).with_context(|| format!("parse response from {url}"))
    }

    async fn internal_post<T: DeserializeOwned>(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<T> {
        debug!(url, "HttpDomainApi internal POST");
        let resp = self
            .http
            .post(url)
            .header("X-Internal-Token", &self.internal_token)
            .json(body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            let truncated: String = text.chars().take(300).collect();
            return Err(anyhow!("HTTP {status}: {truncated}"));
        }
        serde_json::from_str(&text).with_context(|| format!("parse response from {url}"))
    }

    #[allow(dead_code)]
    async fn internal_delete(&self, url: &str) -> anyhow::Result<()> {
        debug!(url, "HttpDomainApi internal DELETE");
        let resp = self
            .http
            .delete(url)
            .header("X-Internal-Token", &self.internal_token)
            .send()
            .await
            .with_context(|| format!("DELETE {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let truncated: String = body.chars().take(300).collect();
            return Err(anyhow!("HTTP {status}: {truncated}"));
        }
        Ok(())
    }

    // -------------------------------------------------------------------------
    // JWT helpers (Authorization: Bearer, for /api/ routes)
    // -------------------------------------------------------------------------

    fn require_jwt(jwt: Option<&str>) -> anyhow::Result<&str> {
        jwt.ok_or_else(|| anyhow!("JWT required for this operation but not provided — ensure the front-end sends a token in session_init"))
    }

    async fn api_get<T: DeserializeOwned>(&self, url: &str, jwt: &str) -> anyhow::Result<T> {
        debug!(url, "HttpDomainApi api GET");
        let resp = self
            .http
            .get(url)
            .bearer_auth(jwt)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            let truncated: String = body.chars().take(300).collect();
            return Err(anyhow!("HTTP {status}: {truncated}"));
        }
        serde_json::from_str(&body).with_context(|| format!("parse response from {url}"))
    }

    async fn api_post<T: DeserializeOwned>(
        &self,
        url: &str,
        body: &serde_json::Value,
        jwt: &str,
    ) -> anyhow::Result<T> {
        debug!(url, "HttpDomainApi api POST");
        let resp = self
            .http
            .post(url)
            .bearer_auth(jwt)
            .json(body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            let truncated: String = text.chars().take(300).collect();
            return Err(anyhow!("HTTP {status}: {truncated}"));
        }
        serde_json::from_str(&text).with_context(|| format!("parse response from {url}"))
    }

    async fn api_put<T: DeserializeOwned>(
        &self,
        url: &str,
        body: &serde_json::Value,
        jwt: &str,
    ) -> anyhow::Result<T> {
        debug!(url, "HttpDomainApi api PUT");
        let resp = self
            .http
            .put(url)
            .bearer_auth(jwt)
            .json(body)
            .send()
            .await
            .with_context(|| format!("PUT {url}"))?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            let truncated: String = text.chars().take(300).collect();
            return Err(anyhow!("HTTP {status}: {truncated}"));
        }
        serde_json::from_str(&text).with_context(|| format!("parse response from {url}"))
    }

    async fn api_delete(&self, url: &str, jwt: &str) -> anyhow::Result<()> {
        debug!(url, "HttpDomainApi api DELETE");
        let resp = self
            .http
            .delete(url)
            .bearer_auth(jwt)
            .send()
            .await
            .with_context(|| format!("DELETE {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let truncated: String = body.chars().take(300).collect();
            return Err(anyhow!("HTTP {status}: {truncated}"));
        }
        Ok(())
    }
}

#[async_trait]
impl DomainApi for HttpDomainApi {
    // -- Specs (aura-storage, JWT /api/) --------------------------------------

    async fn list_specs(&self, project_id: &str, jwt: Option<&str>) -> anyhow::Result<Vec<SpecDescriptor>> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!("{}/api/projects/{project_id}/specs", self.storage_url);
        self.api_get(&url, jwt).await
    }

    async fn get_spec(&self, spec_id: &str, jwt: Option<&str>) -> anyhow::Result<SpecDescriptor> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!("{}/api/specs/{spec_id}", self.storage_url);
        self.api_get(&url, jwt).await
    }

    async fn create_spec(
        &self,
        project_id: &str,
        title: &str,
        content: &str,
        order: u32,
        jwt: Option<&str>,
    ) -> anyhow::Result<SpecDescriptor> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!("{}/api/projects/{project_id}/specs", self.storage_url);
        let body = serde_json::json!({
            "title": title,
            "markdownContents": content,
            "orderIndex": order,
        });
        self.api_post(&url, &body, jwt).await
    }

    async fn update_spec(
        &self,
        spec_id: &str,
        title: Option<&str>,
        content: Option<&str>,
        jwt: Option<&str>,
    ) -> anyhow::Result<SpecDescriptor> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!("{}/api/specs/{spec_id}", self.storage_url);
        let body = serde_json::json!({
            "title": title,
            "markdownContents": content,
        });
        self.api_put(&url, &body, jwt).await
    }

    async fn delete_spec(&self, spec_id: &str, jwt: Option<&str>) -> anyhow::Result<()> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!("{}/api/specs/{spec_id}", self.storage_url);
        self.api_delete(&url, jwt).await
    }

    // -- Tasks (aura-storage, JWT /api/) --------------------------------------

    async fn list_tasks(
        &self,
        project_id: &str,
        spec_id: Option<&str>,
        jwt: Option<&str>,
    ) -> anyhow::Result<Vec<TaskDescriptor>> {
        let jwt = Self::require_jwt(jwt)?;
        let mut url = format!("{}/api/projects/{project_id}/tasks", self.storage_url);
        if let Some(sid) = spec_id {
            url.push_str(&format!("?specId={sid}"));
        }
        self.api_get(&url, jwt).await
    }

    async fn create_task(
        &self,
        project_id: &str,
        spec_id: &str,
        title: &str,
        description: &str,
        dependencies: &[String],
        jwt: Option<&str>,
    ) -> anyhow::Result<TaskDescriptor> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!("{}/api/projects/{project_id}/tasks", self.storage_url);
        let body = serde_json::json!({
            "specId": spec_id,
            "title": title,
            "description": description,
            "dependencyTaskIds": dependencies,
        });
        self.api_post(&url, &body, jwt).await
    }

    async fn update_task(
        &self,
        task_id: &str,
        updates: TaskUpdate,
        jwt: Option<&str>,
    ) -> anyhow::Result<TaskDescriptor> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!("{}/api/tasks/{task_id}", self.storage_url);
        let body = serde_json::json!({
            "title": updates.title,
            "description": updates.description,
            "status": updates.status,
        });
        self.api_put(&url, &body, jwt).await
    }

    async fn delete_task(&self, task_id: &str, jwt: Option<&str>) -> anyhow::Result<()> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!("{}/api/tasks/{task_id}", self.storage_url);
        self.api_delete(&url, jwt).await
    }

    async fn transition_task(
        &self,
        task_id: &str,
        status: &str,
        jwt: Option<&str>,
    ) -> anyhow::Result<TaskDescriptor> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!("{}/api/tasks/{task_id}/transition", self.storage_url);
        let body = serde_json::json!({ "status": status });
        self.api_post(&url, &body, jwt).await
    }

    async fn get_task(&self, task_id: &str, jwt: Option<&str>) -> anyhow::Result<TaskDescriptor> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!("{}/api/tasks/{task_id}", self.storage_url);
        self.api_get(&url, jwt).await
    }

    async fn claim_next_task(
        &self,
        project_id: &str,
        agent_id: &str,
        jwt: Option<&str>,
    ) -> anyhow::Result<Option<TaskDescriptor>> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!(
            "{}/api/projects/{project_id}/tasks/claim?agentId={agent_id}",
            self.storage_url
        );
        let body = serde_json::json!({});
        match self.api_post::<TaskDescriptor>(&url, &body, jwt).await {
            Ok(t) => Ok(Some(t)),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("404") || msg.contains("no task") || msg.contains("No task") {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }

    // -- Project (aura-network, JWT /api/) ------------------------------------

    async fn get_project(&self, project_id: &str, jwt: Option<&str>) -> anyhow::Result<ProjectDescriptor> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!("{}/api/projects/{project_id}", self.network_url);
        self.api_get(&url, jwt).await
    }

    async fn update_project(
        &self,
        project_id: &str,
        updates: ProjectUpdate,
        jwt: Option<&str>,
    ) -> anyhow::Result<ProjectDescriptor> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!("{}/api/projects/{project_id}", self.network_url);
        let body = serde_json::json!({
            "name": updates.name,
            "description": updates.description,
            "techStack": updates.tech_stack,
            "buildCommand": updates.build_command,
            "testCommand": updates.test_command,
        });
        self.api_put(&url, &body, jwt).await
    }

    // -- Storage: logs (create = /internal/, list = /api/) --------------------

    async fn create_log(
        &self,
        project_id: &str,
        message: &str,
        level: &str,
        agent_id: Option<&str>,
        metadata: Option<&serde_json::Value>,
    ) -> anyhow::Result<serde_json::Value> {
        let url = format!("{}/internal/logs", self.storage_url);
        let mut body = serde_json::json!({
            "projectId": project_id,
            "message": message,
            "level": level,
        });
        if let Some(aid) = agent_id {
            body["projectAgentId"] = serde_json::Value::String(aid.to_string());
        }
        if let Some(meta) = metadata {
            body["metadata"] = meta.clone();
        }
        self.internal_post(&url, &body).await
    }

    async fn list_logs(
        &self,
        project_id: &str,
        level: Option<&str>,
        limit: Option<u64>,
        jwt: Option<&str>,
    ) -> anyhow::Result<serde_json::Value> {
        let jwt = Self::require_jwt(jwt)?;
        let mut url = format!("{}/api/projects/{project_id}/logs", self.storage_url);
        let mut params = Vec::new();
        if let Some(l) = level {
            params.push(format!("level={l}"));
        }
        if let Some(n) = limit {
            params.push(format!("limit={n}"));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
        self.api_get(&url, jwt).await
    }

    async fn get_project_stats(&self, project_id: &str, jwt: Option<&str>) -> anyhow::Result<serde_json::Value> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!(
            "{}/api/stats?scope=project&projectId={project_id}",
            self.storage_url
        );
        self.api_get(&url, jwt).await
    }

    // -- Messages / Sessions (not used by WS sessions) ------------------------

    async fn list_messages(
        &self,
        _project_id: &str,
        _instance_id: &str,
    ) -> anyhow::Result<Vec<MessageDescriptor>> {
        warn!("HttpDomainApi::list_messages not implemented");
        Ok(vec![])
    }

    async fn save_message(&self, _params: SaveMessageParams) -> anyhow::Result<()> {
        warn!("HttpDomainApi::save_message not implemented");
        Ok(())
    }

    async fn create_session(
        &self,
        _params: CreateSessionParams,
    ) -> anyhow::Result<SessionDescriptor> {
        Err(anyhow!("HttpDomainApi::create_session not implemented"))
    }

    async fn get_active_session(
        &self,
        _instance_id: &str,
    ) -> anyhow::Result<Option<SessionDescriptor>> {
        Ok(None)
    }

    // -- Orbit (raw JSON pass-through) ----------------------------------------

    async fn orbit_api_call(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
        jwt: Option<&str>,
    ) -> anyhow::Result<String> {
        let url = format!("{}{path}", self.orbit_url);
        debug!(url, method, "HttpDomainApi orbit call");
        let mut req = match method {
            "POST" => self.http.post(&url),
            "PUT" => self.http.put(&url),
            "DELETE" => self.http.delete(&url),
            _ => self.http.get(&url),
        };
        if let Some(jwt) = jwt {
            req = req.bearer_auth(jwt);
        } else {
            req = req.header("X-Internal-Token", &self.internal_token);
        }
        if let Some(body) = body {
            req = req.json(body);
        }
        let resp = req.send().await.with_context(|| format!("{method} {url}"))?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            let truncated: String = text.chars().take(500).collect();
            return Err(anyhow!("HTTP {status}: {truncated}"));
        }
        Ok(text)
    }

    fn orbit_url(&self) -> &str {
        &self.orbit_url
    }

    // -- Network (raw JSON pass-through) --------------------------------------

    async fn network_api_call(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
        jwt: Option<&str>,
    ) -> anyhow::Result<String> {
        let url = format!("{}{path}", self.network_url);
        debug!(url, method, "HttpDomainApi network call");
        let mut req = match method {
            "POST" => self.http.post(&url),
            "PUT" => self.http.put(&url),
            _ => self.http.get(&url),
        };
        if let Some(jwt) = jwt {
            req = req.bearer_auth(jwt);
        } else {
            req = req.header("X-Internal-Token", &self.internal_token);
        }
        if let Some(body) = body {
            req = req.json(body);
        }
        let resp = req.send().await.with_context(|| format!("{method} {url}"))?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            let truncated: String = text.chars().take(500).collect();
            return Err(anyhow!("HTTP {status}: {truncated}"));
        }
        Ok(text)
    }
}
