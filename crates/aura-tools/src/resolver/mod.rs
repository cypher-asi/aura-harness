//! Tool resolver — unified dispatch layer for tool execution.
//!
//! The resolver adds catalog-based visibility and domain tool dispatch on top
//! of [`ToolExecutor`](crate::ToolExecutor), which owns the internal built-in
//! tool implementations and permission checks.
//!
//! ## Layout (Wave 6 / T4)
//!
//! The original 1.6 KLoC `resolver.rs` was split into a directory:
//!
//! - [`installed`] — installed-tool HTTP dispatch (`execute_tool`,
//!   `execute_installed_tool`, `execute_runtime_installed_tool`). This is
//!   the entry point the `Executor` trait impl calls.
//! - [`trusted`] — trusted-integration runtime (GitHub / Linear / Slack /
//!   Brave / Resend) plus the generic `TrustedIntegrationRuntimeSpec`
//!   interpreter and all its supporting types + free helpers.
//! - [`runtime_headers`] — `apply_runtime_headers`,
//!   `apply_runtime_auth`, `runtime_url_with_auth` — the HTTP
//!   header-merging and auth-injection primitives shared by the JSON /
//!   form / raw request paths in `trusted`.
//! - [`json_paths`] — `insert_json_path` (Wave 3 hardened) plus the small
//!   `required_string` / `optional_string` / ... extraction helpers.
//!
//! `mod.rs` keeps only the `ToolResolver` struct, its constructor + light
//! delegation methods, and the `Executor` trait impl so kernel plumbing
//! keeps working unchanged.

use crate::catalog::ToolCatalog;
use crate::catalog::ToolProfile;
use crate::domain_tools::DomainToolExecutor;
use crate::tool::Tool;
use crate::ToolConfig;
use crate::ToolExecutor;
use async_trait::async_trait;
use aura_core::ToolDefinition;
use aura_core::{
    Action, ActionKind, Effect, EffectKind, EffectStatus, InstalledToolDefinition, ToolCall,
    ToolResult,
};
use aura_kernel::{ExecuteContext, Executor, ExecutorError};
use bytes::Bytes;
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, instrument, warn};

mod installed;
mod json_paths;
mod runtime_headers;
mod trusted;

/// Default total timeout for the resolver's shared HTTP client (both
/// installed-tool callbacks and trusted-provider calls).
///
/// A process-wide `.timeout()` on the client gives every request a hard
/// ceiling even when a caller forgets to layer one. (Wave 5 / T2.)
const RESOLVER_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Default connect-phase timeout: fast-fail DNS / TCP handshake failures.
const RESOLVER_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Per-request timeout for trusted-runtime provider calls (Linear, Slack,
/// Resend, etc.). Tighter than the client-wide fallback so a single slow
/// provider cannot stall an agent turn.
pub(crate) const TRUSTED_PROVIDER_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) const TRUSTED_INTEGRATION_RUNTIME_METADATA_KEY: &str = "trusted_integration_runtime";

/// Unified tool resolver providing both visibility and execution dispatch.
///
/// Composes [`ToolExecutor`](crate::ToolExecutor) for built-in tool execution
/// and adds domain tool routing (specs, tasks, project) on top.
///
/// Implements [`Executor`] so it can be plugged into the kernel layer
/// (scheduler, `ExecutorRouter`) as a drop-in replacement for `ToolExecutor`.
pub struct ToolResolver {
    catalog: Arc<ToolCatalog>,
    pub(super) inner: ToolExecutor,
    pub(super) domain_executor: Option<Arc<DomainToolExecutor>>,
    pub(super) installed_tools: HashMap<String, InstalledToolDefinition>,
    pub(super) http_client: Client,
}

impl ToolResolver {
    /// Create a resolver pre-loaded with all built-in tool handlers.
    ///
    /// The shared HTTP client is built with a 30 s request ceiling and a
    /// 10 s connect timeout (Wave 5 / T2.2). On the extremely rare event
    /// that the TLS backend fails to initialize we log a warning and fall
    /// back to a naive client — losing timeouts is preferable to panicking
    /// or forcing every call site through a `Result`.
    #[must_use]
    pub fn new(catalog: Arc<ToolCatalog>, config: ToolConfig) -> Self {
        let http_client = Client::builder()
            .timeout(RESOLVER_REQUEST_TIMEOUT)
            .connect_timeout(RESOLVER_CONNECT_TIMEOUT)
            .build()
            .unwrap_or_else(|e| {
                warn!(
                    error = %e,
                    "failed to build timed reqwest client for ToolResolver; falling back to default client without timeouts"
                );
                Client::new()
            });
        Self {
            catalog,
            inner: ToolExecutor::new(config),
            domain_executor: None,
            installed_tools: HashMap::new(),
            http_client,
        }
    }

    /// Attach a domain tool executor for specs/tasks/project dispatch.
    #[must_use]
    pub fn with_domain_executor(mut self, exec: Arc<DomainToolExecutor>) -> Self {
        self.domain_executor = Some(exec);
        self
    }

    /// Attach installed tools that should execute via HTTP callbacks.
    #[must_use]
    pub fn with_installed_tools(mut self, tools: Vec<InstalledToolDefinition>) -> Self {
        self.installed_tools = tools
            .into_iter()
            .map(|tool| (tool.name.clone(), tool))
            .collect();
        self
    }

    /// Visible tools for a profile (delegates to the catalog + config).
    #[must_use]
    pub fn visible_tools(&self, profile: ToolProfile) -> Vec<ToolDefinition> {
        self.catalog.visible_tools(profile, self.inner.config())
    }

    /// Register an additional internal tool at runtime.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.inner.register(tool);
    }
}

// ---------------------------------------------------------------------------
// Executor trait impl  — allows the resolver to be used in ExecutorRouter
// ---------------------------------------------------------------------------

#[async_trait]
impl Executor for ToolResolver {
    #[instrument(skip(self, ctx, action), fields(action_id = %action.action_id))]
    async fn execute(
        &self,
        ctx: &ExecuteContext,
        action: &Action,
    ) -> Result<Effect, ExecutorError> {
        let tool_call: ToolCall = serde_json::from_slice(&action.payload).map_err(|e| {
            ExecutorError::ExecutionFailed(format!("Failed to parse tool call: {e}"))
        })?;

        debug!(tool = %tool_call.tool, "Executing tool via resolver");

        match self.execute_tool(ctx, &tool_call).await {
            Ok(result) => {
                let payload = serde_json::to_vec(&result).map_err(|e| {
                    ExecutorError::ExecutionFailed(format!("Failed to serialize tool result: {e}"))
                })?;
                Ok(Effect::new(
                    action.action_id,
                    EffectKind::Agreement,
                    EffectStatus::Committed,
                    Bytes::from(payload),
                ))
            }
            Err(e) => {
                error!(error = %e, "Tool execution failed");
                let result = ToolResult::failure(&tool_call.tool, e.to_string());
                let payload = serde_json::to_vec(&result).map_err(|e| {
                    ExecutorError::ExecutionFailed(format!("Failed to serialize error result: {e}"))
                })?;
                Ok(Effect::new(
                    action.action_id,
                    EffectKind::Agreement,
                    EffectStatus::Failed,
                    Bytes::from(payload),
                ))
            }
        }
    }

    fn can_handle(&self, action: &Action) -> bool {
        if action.kind != ActionKind::Delegate {
            return false;
        }
        serde_json::from_slice::<ToolCall>(&action.payload).is_ok()
    }

    fn name(&self) -> &'static str {
        "tool_resolver"
    }
}

#[cfg(test)]
#[path = "../resolver_tests.rs"]
mod tests;
