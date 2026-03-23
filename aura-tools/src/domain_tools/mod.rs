//! Domain tool handlers and the `DomainApi` callback trait.
//!
//! This module provides the harness-side tool handlers for domain operations
//! (specs, tasks, projects). The actual data access is delegated through the
//! [`DomainApi`] trait so that the harness never depends on app-level crates.

pub mod api;
pub mod helpers;
pub mod project;
pub mod specs;
pub mod tasks;

pub use api::*;

use serde_json::Value;
use std::sync::Arc;
use tracing::warn;

/// Dispatches domain tool calls to the appropriate handler via `DomainApi`.
pub struct DomainToolExecutor {
    api: Arc<dyn DomainApi>,
}

impl DomainToolExecutor {
    pub fn new(api: Arc<dyn DomainApi>) -> Self {
        Self { api }
    }

    /// Execute a domain tool by name.
    ///
    /// `project_id` is threaded through from the session context.
    /// Returns a JSON string result (always contains an `ok` field).
    pub async fn execute(
        &self,
        tool_name: &str,
        project_id: &str,
        input: &Value,
    ) -> String {
        match tool_name {
            // Specs
            "list_specs" => specs::list_specs(self.api.as_ref(), project_id, input).await,
            "get_spec" => specs::get_spec(self.api.as_ref(), project_id, input).await,
            "create_spec" => specs::create_spec(self.api.as_ref(), project_id, input).await,
            "update_spec" => specs::update_spec(self.api.as_ref(), project_id, input).await,
            "delete_spec" => specs::delete_spec(self.api.as_ref(), project_id, input).await,

            // Tasks
            "list_tasks" => tasks::list_tasks(self.api.as_ref(), project_id, input).await,
            "create_task" => tasks::create_task(self.api.as_ref(), project_id, input).await,
            "update_task" => tasks::update_task(self.api.as_ref(), project_id, input).await,
            "delete_task" => tasks::delete_task(self.api.as_ref(), project_id, input).await,
            "transition_task" => {
                tasks::transition_task(self.api.as_ref(), project_id, input).await
            }

            // Project
            "get_project" => project::get_project(self.api.as_ref(), project_id, input).await,
            "update_project" => {
                project::update_project(self.api.as_ref(), project_id, input).await
            }

            other => {
                warn!(tool = other, "unknown domain tool");
                serde_json::json!({
                    "ok": false,
                    "error": format!("unknown domain tool: {other}")
                })
                .to_string()
            }
        }
    }

    /// Returns true if `tool_name` is a domain tool handled by this executor.
    pub fn handles(&self, tool_name: &str) -> bool {
        matches!(
            tool_name,
            "list_specs"
                | "get_spec"
                | "create_spec"
                | "update_spec"
                | "delete_spec"
                | "list_tasks"
                | "create_task"
                | "update_task"
                | "delete_task"
                | "transition_task"
                | "get_project"
                | "update_project"
        )
    }

    /// List all domain tool names handled by this executor.
    pub fn tool_names(&self) -> &[&'static str] {
        &[
            "list_specs",
            "get_spec",
            "create_spec",
            "update_spec",
            "delete_spec",
            "list_tasks",
            "create_task",
            "update_task",
            "delete_task",
            "transition_task",
            "get_project",
            "update_project",
        ]
    }
}
