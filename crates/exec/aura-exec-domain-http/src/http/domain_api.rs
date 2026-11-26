//! [`DomainApi`] for [`HttpDomainApi`], grouped by resource: specs,
//! tasks, project, logs/stats, messages/sessions, network pass-through.

use async_trait::async_trait;
use aura_exec_traits::domain::{
    CreateSessionParams, DomainApi, DomainError, DomainResult, MessageDescriptor, NewTask,
    ProjectDescriptor, ProjectUpdate, SaveMessageParams, SessionDescriptor, SpecDescriptor,
    TaskDescriptor, TaskUpdate,
};
use tracing::{debug, warn};

use super::request::{body_error, status_error, transport_error};
use super::HttpDomainApi;

#[async_trait]
impl DomainApi for HttpDomainApi {
    // -- Specs (aura-storage, JWT /api/) --------------------------------------

    async fn list_specs(
        &self,
        project_id: &str,
        jwt: Option<&str>,
    ) -> DomainResult<Vec<SpecDescriptor>> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!(
            "{}/api/projects/{project_id}/specs",
            self.specs_tasks_base_url()
        );
        self.api_get(&url, jwt).await
    }

    async fn get_spec(&self, spec_id: &str, jwt: Option<&str>) -> DomainResult<SpecDescriptor> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!("{}/api/specs/{spec_id}", self.specs_tasks_base_url());
        self.api_get(&url, jwt).await
    }

    async fn create_spec(
        &self,
        project_id: &str,
        title: &str,
        content: &str,
        order: u32,
        jwt: Option<&str>,
    ) -> DomainResult<SpecDescriptor> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!(
            "{}/api/projects/{project_id}/specs",
            self.specs_tasks_base_url()
        );
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
        if_match: Option<&str>,
        jwt: Option<&str>,
    ) -> DomainResult<SpecDescriptor> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!("{}/api/specs/{spec_id}", self.specs_tasks_base_url());
        let body = serde_json::json!({
            "title": title,
            "markdownContents": content,
            "ifMatch": if_match,
        });
        self.api_put(&url, &body, jwt).await
    }

    async fn update_spec_section(
        &self,
        spec_id: &str,
        section_heading: &str,
        new_body: &str,
        if_match: Option<&str>,
        jwt: Option<&str>,
    ) -> DomainResult<SpecDescriptor> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!(
            "{}/api/specs/{spec_id}/section",
            self.specs_tasks_base_url()
        );
        let body = serde_json::json!({
            "sectionHeading": section_heading,
            "newBody": new_body,
            "ifMatch": if_match,
        });
        self.api_put(&url, &body, jwt).await
    }

    async fn append_to_spec(
        &self,
        spec_id: &str,
        markdown: &str,
        if_match: Option<&str>,
        jwt: Option<&str>,
    ) -> DomainResult<SpecDescriptor> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!("{}/api/specs/{spec_id}/append", self.specs_tasks_base_url());
        let body = serde_json::json!({
            "markdown": markdown,
            "ifMatch": if_match,
        });
        self.api_post(&url, &body, jwt).await
    }

    async fn delete_spec(&self, spec_id: &str, jwt: Option<&str>) -> DomainResult<()> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!("{}/api/specs/{spec_id}", self.specs_tasks_base_url());
        self.api_delete(&url, jwt).await
    }

    // -- Tasks (aura-storage, JWT /api/) --------------------------------------

    async fn list_tasks(
        &self,
        project_id: &str,
        spec_id: Option<&str>,
        jwt: Option<&str>,
    ) -> DomainResult<Vec<TaskDescriptor>> {
        let jwt = Self::require_jwt(jwt)?;
        let mut url = format!(
            "{}/api/projects/{project_id}/tasks",
            self.specs_tasks_base_url()
        );
        if let Some(sid) = spec_id {
            use std::fmt::Write;
            let _ = write!(url, "?specId={sid}");
        }
        self.api_get(&url, jwt).await
    }

    async fn create_task(
        &self,
        task: NewTask<'_>,
        jwt: Option<&str>,
    ) -> DomainResult<TaskDescriptor> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!(
            "{}/api/projects/{}/tasks",
            self.specs_tasks_base_url(),
            task.project_id
        );
        let body = serde_json::json!({
            "specId": task.spec_id,
            "title": task.title,
            "description": task.description,
            "dependencyTaskIds": task.dependencies,
            "orderIndex": task.order,
        });
        self.api_post(&url, &body, jwt).await
    }

    async fn update_task(
        &self,
        task_id: &str,
        updates: TaskUpdate,
        jwt: Option<&str>,
    ) -> DomainResult<TaskDescriptor> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!("{}/api/tasks/{task_id}", self.specs_tasks_base_url());
        let body = serde_json::json!({
            "title": updates.title,
            "description": updates.description,
            "status": updates.status,
            "orderIndex": updates.order_index,
            "dependencyIds": updates.dependency_ids,
        });
        self.api_put(&url, &body, jwt).await
    }

    async fn delete_task(&self, task_id: &str, jwt: Option<&str>) -> DomainResult<()> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!("{}/api/tasks/{task_id}", self.specs_tasks_base_url());
        self.api_delete(&url, jwt).await
    }

    async fn transition_task(
        &self,
        task_id: &str,
        status: &str,
        jwt: Option<&str>,
    ) -> DomainResult<TaskDescriptor> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!(
            "{}/api/tasks/{task_id}/transition",
            self.specs_tasks_base_url()
        );
        let body = serde_json::json!({ "status": status });
        self.api_post(&url, &body, jwt).await
    }

    async fn get_task(&self, task_id: &str, jwt: Option<&str>) -> DomainResult<TaskDescriptor> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!("{}/api/tasks/{task_id}", self.specs_tasks_base_url());
        self.api_get(&url, jwt).await
    }

    async fn claim_next_task(
        &self,
        project_id: &str,
        agent_id: &str,
        jwt: Option<&str>,
    ) -> DomainResult<Option<TaskDescriptor>> {
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

    // -- Project (aura-os-server when configured, else aura-network
    //    legacy fallback — aura-storage has no `/api/projects/:id`
    //    GET, only sub-routes, so falling back there 404s every
    //    `get_project` the dev-loop makes. Both endpoints on the
    //    override fire aura-os-server's SSE broadcast / JWT billing
    //    side effects, matching the spec/task write path.) --

    async fn get_project(
        &self,
        project_id: &str,
        jwt: Option<&str>,
    ) -> DomainResult<ProjectDescriptor> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!("{}/api/projects/{project_id}", self.project_base_url());
        self.api_get(&url, jwt).await
    }

    async fn update_project(
        &self,
        project_id: &str,
        updates: ProjectUpdate,
        jwt: Option<&str>,
    ) -> DomainResult<ProjectDescriptor> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!("{}/api/projects/{project_id}", self.project_base_url());
        let body = serde_json::json!({
            "name": updates.name,
            "description": updates.description,
            "techStack": updates.tech_stack,
            "buildCommand": updates.build_command,
            "testCommand": updates.test_command,
        });
        self.api_put(&url, &body, jwt).await
    }

    // -- Storage: logs (JWT /api/) --------------------------------------------

    async fn create_log(
        &self,
        project_id: &str,
        message: &str,
        level: &str,
        agent_id: Option<&str>,
        metadata: Option<&serde_json::Value>,
        jwt: Option<&str>,
    ) -> DomainResult<serde_json::Value> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!(
            "{}/api/projects/{project_id}/logs",
            self.specs_tasks_base_url()
        );
        let mut body = serde_json::json!({
            "message": message,
            "level": level,
        });
        if let Some(aid) = agent_id {
            body["projectAgentId"] = serde_json::Value::String(aid.to_string());
        }
        if let Some(meta) = metadata {
            body["metadata"] = meta.clone();
        }
        self.api_post(&url, &body, jwt).await
    }

    async fn list_logs(
        &self,
        project_id: &str,
        level: Option<&str>,
        limit: Option<u64>,
        jwt: Option<&str>,
    ) -> DomainResult<serde_json::Value> {
        let jwt = Self::require_jwt(jwt)?;
        let mut url = format!(
            "{}/api/projects/{project_id}/logs",
            self.specs_tasks_base_url()
        );
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

    async fn get_project_stats(
        &self,
        project_id: &str,
        jwt: Option<&str>,
    ) -> DomainResult<serde_json::Value> {
        let jwt = Self::require_jwt(jwt)?;
        let url = format!(
            "{}/api/stats?scope=project&projectId={project_id}",
            self.specs_tasks_base_url()
        );
        self.api_get(&url, jwt).await
    }

    // -- Messages / Sessions (not used by WS sessions) ------------------------

    async fn list_messages(
        &self,
        _project_id: &str,
        _instance_id: &str,
    ) -> DomainResult<Vec<MessageDescriptor>> {
        warn!("HttpDomainApi::list_messages not implemented");
        Ok(vec![])
    }

    async fn save_message(&self, _params: SaveMessageParams) -> DomainResult<()> {
        warn!("HttpDomainApi::save_message not implemented");
        Ok(())
    }

    async fn create_session(
        &self,
        _params: CreateSessionParams,
    ) -> DomainResult<SessionDescriptor> {
        Err(DomainError::NotImplemented {
            operation: "HttpDomainApi::create_session",
        })
    }

    async fn get_active_session(
        &self,
        _instance_id: &str,
    ) -> DomainResult<Option<SessionDescriptor>> {
        Ok(None)
    }

    // -- Network (raw JSON pass-through) --------------------------------------

    async fn network_api_call(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
        jwt: Option<&str>,
    ) -> DomainResult<String> {
        let url = format!("{}{path}", self.network_url);
        debug!(url, method, "HttpDomainApi network call");
        let mut req = match method {
            "POST" => self.http.post(&url),
            "PUT" => self.http.put(&url),
            _ => self.http.get(&url),
        };
        if let Some(jwt) = jwt {
            req = req.bearer_auth(jwt);
        }
        if let Some(body) = body {
            req = req.json(body);
        }
        let resp = req
            .send()
            .await
            .map_err(|error| transport_error(method, &url, error))?;
        let status = resp.status();
        let text = resp.text().await.map_err(body_error)?;
        if !status.is_success() {
            return Err(status_error(status, text.chars().take(500).collect()));
        }
        Ok(text)
    }
}
