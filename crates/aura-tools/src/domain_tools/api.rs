//! Domain API trait and lightweight descriptor types.
//!
//! `DomainApi` is the callback seam that allows the harness tool layer to
//! invoke application-level domain operations (specs, tasks, projects, etc.)
//! without depending on the concrete app crate.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Descriptor types – lightweight DTOs that avoid pulling in app domain types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecDescriptor {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub content: String,
    pub order: u32,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDescriptor {
    pub id: String,
    pub spec_id: String,
    pub project_id: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectDescriptor {
    pub id: String,
    pub name: String,
    pub path: String,
    pub description: Option<String>,
    pub tech_stack: Option<String>,
    pub build_command: Option<String>,
    pub test_command: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageDescriptor {
    pub id: String,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDescriptor {
    pub id: String,
    pub instance_id: String,
    pub project_id: String,
    pub status: String,
}

// ---------------------------------------------------------------------------
// Update / param types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskUpdate {
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectUpdate {
    pub name: Option<String>,
    pub description: Option<String>,
    pub tech_stack: Option<String>,
    pub build_command: Option<String>,
    pub test_command: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveMessageParams {
    pub project_id: String,
    pub instance_id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionParams {
    pub instance_id: String,
    pub project_id: String,
    pub model: Option<String>,
}

// ---------------------------------------------------------------------------
// DomainApi trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait DomainApi: Send + Sync {
    // Specs
    async fn list_specs(&self, project_id: &str) -> anyhow::Result<Vec<SpecDescriptor>>;
    async fn get_spec(&self, spec_id: &str) -> anyhow::Result<SpecDescriptor>;
    async fn create_spec(
        &self,
        project_id: &str,
        title: &str,
        content: &str,
    ) -> anyhow::Result<SpecDescriptor>;
    async fn update_spec(
        &self,
        spec_id: &str,
        title: Option<&str>,
        content: Option<&str>,
    ) -> anyhow::Result<SpecDescriptor>;
    async fn delete_spec(&self, spec_id: &str) -> anyhow::Result<()>;

    // Tasks
    async fn list_tasks(
        &self,
        project_id: &str,
        spec_id: Option<&str>,
    ) -> anyhow::Result<Vec<TaskDescriptor>>;
    async fn create_task(
        &self,
        spec_id: &str,
        title: &str,
        description: &str,
        dependencies: &[String],
    ) -> anyhow::Result<TaskDescriptor>;
    async fn update_task(
        &self,
        task_id: &str,
        updates: TaskUpdate,
    ) -> anyhow::Result<TaskDescriptor>;
    async fn transition_task(&self, task_id: &str, status: &str) -> anyhow::Result<TaskDescriptor>;
    async fn claim_next_task(
        &self,
        project_id: &str,
        agent_id: &str,
    ) -> anyhow::Result<Option<TaskDescriptor>>;

    // Project
    async fn get_project(&self, project_id: &str) -> anyhow::Result<ProjectDescriptor>;
    async fn update_project(
        &self,
        project_id: &str,
        updates: ProjectUpdate,
    ) -> anyhow::Result<ProjectDescriptor>;

    // Messages
    async fn list_messages(
        &self,
        project_id: &str,
        instance_id: &str,
    ) -> anyhow::Result<Vec<MessageDescriptor>>;
    async fn save_message(&self, params: SaveMessageParams) -> anyhow::Result<()>;

    // Sessions
    async fn create_session(
        &self,
        params: CreateSessionParams,
    ) -> anyhow::Result<SessionDescriptor>;
    async fn get_active_session(
        &self,
        instance_id: &str,
    ) -> anyhow::Result<Option<SessionDescriptor>>;

    // Orbit (raw JSON pass-through)
    async fn orbit_api_call(&self, method: &str, path: &str, body: Option<&serde_json::Value>, jwt: Option<&str>) -> anyhow::Result<String>;
    fn orbit_url(&self) -> &str { "" }

    // Network (raw JSON pass-through)
    async fn network_api_call(&self, method: &str, path: &str, body: Option<&serde_json::Value>, jwt: Option<&str>) -> anyhow::Result<String>;
}
