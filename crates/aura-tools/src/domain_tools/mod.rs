//! Domain tool handlers and the `DomainApi` callback trait.
//!
//! This module provides the harness-side tool handlers for domain operations
//! (specs, tasks, projects). The actual data access is delegated through the
//! [`DomainApi`] trait so that the harness never depends on app-level crates.

pub mod api;
pub mod helpers;
pub mod network;
pub mod orbit;
pub mod project;
pub mod specs;
pub mod tasks;

pub use api::*;

use serde_json::Value;
use std::sync::Arc;
use tracing::warn;

const DOMAIN_TOOL_NAMES: &[&str] = &[
    "list_specs", "get_spec", "create_spec", "update_spec", "delete_spec",
    "list_tasks", "create_task", "update_task", "delete_task", "transition_task",
    "get_project", "update_project",
    "orbit_push", "orbit_create_repo", "orbit_list_repos", "orbit_list_branches",
    "orbit_create_branch", "orbit_list_commits", "orbit_get_diff",
    "orbit_create_pr", "orbit_list_prs", "orbit_merge_pr",
    "post_to_feed", "list_projects", "check_budget", "record_usage",
];

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
    pub async fn execute(&self, tool_name: &str, project_id: &str, input: &Value) -> String {
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
            "transition_task" => tasks::transition_task(self.api.as_ref(), project_id, input).await,

            // Project
            "get_project" => project::get_project(self.api.as_ref(), project_id, input).await,
            "update_project" => project::update_project(self.api.as_ref(), project_id, input).await,

            // Orbit
            "orbit_push" => orbit::orbit_push(self.api.as_ref(), project_id, input).await,
            "orbit_create_repo" => orbit::orbit_create_repo(self.api.as_ref(), project_id, input).await,
            "orbit_list_repos" => orbit::orbit_list_repos(self.api.as_ref(), project_id, input).await,
            "orbit_list_branches" => orbit::orbit_list_branches(self.api.as_ref(), project_id, input).await,
            "orbit_create_branch" => orbit::orbit_create_branch(self.api.as_ref(), project_id, input).await,
            "orbit_list_commits" => orbit::orbit_list_commits(self.api.as_ref(), project_id, input).await,
            "orbit_get_diff" => orbit::orbit_get_diff(self.api.as_ref(), project_id, input).await,
            "orbit_create_pr" => orbit::orbit_create_pr(self.api.as_ref(), project_id, input).await,
            "orbit_list_prs" => orbit::orbit_list_prs(self.api.as_ref(), project_id, input).await,
            "orbit_merge_pr" => orbit::orbit_merge_pr(self.api.as_ref(), project_id, input).await,

            // Network
            "post_to_feed" => network::post_to_feed(self.api.as_ref(), project_id, input).await,
            "list_projects" => network::network_list_projects(self.api.as_ref(), project_id, input).await,
            "check_budget" => network::check_budget(self.api.as_ref(), project_id, input).await,
            "record_usage" => network::record_usage(self.api.as_ref(), project_id, input).await,

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
        DOMAIN_TOOL_NAMES.contains(&tool_name)
    }

    /// List all domain tool names handled by this executor.
    pub fn tool_names(&self) -> &[&'static str] {
        DOMAIN_TOOL_NAMES
    }
}
