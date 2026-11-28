//! Integration tools as agent tools: [`IntegrationTool`] is an
//! `aura-exec-tools` [`Tool`] named `<id>_<tool>`.
//!
//! The kernel's mode gate does not know these names, so it treats them
//! as network tools: they run in agent mode only, whatever their
//! [`crate::ToolEffect`]. Letting `Read` tools run in plan and ask mode
//! needs the gate to learn them (`docs/integrations.md`, later work).

use crate::id::IntegrationId;
use crate::manifest::{Manifest, ToolSpec};
use crate::registry::Registry;
use async_trait::async_trait;
use aura_core_types::{ToolDefinition, ToolResult};
use aura_exec_tools::{Tool, ToolContext, ToolError};
use std::sync::Arc;

/// The name agents see for `id`'s tool `tool`: `<id>_<tool>`, with `-`
/// made `_` (`run-pod`'s `list` is `run_pod_list`).
#[must_use]
pub fn tool_name(id: &IntegrationId, tool: &str) -> String {
    format!("{}_{tool}", id.tool_prefix())
}

/// One integration tool for one agent.
pub struct IntegrationTool {
    registry: Arc<Registry>,
    id: IntegrationId,
    name: String,
    service: String,
    spec: ToolSpec,
    agent: String,
    channel: Option<String>,
}

impl IntegrationTool {
    /// `manifest`'s tool `spec` for `agent` in `channel`, run through
    /// `registry`.
    #[must_use]
    pub fn new(
        registry: Arc<Registry>,
        manifest: &Manifest,
        spec: &ToolSpec,
        (agent, channel): (&str, Option<&str>),
    ) -> Self {
        Self {
            registry,
            id: manifest.id.clone(),
            name: tool_name(&manifest.id, &spec.name),
            service: manifest.name.clone(),
            spec: spec.clone(),
            agent: agent.to_string(),
            channel: channel.map(str::to_string),
        }
    }

    /// The integration it belongs to.
    #[must_use]
    pub const fn integration(&self) -> &IntegrationId {
        &self.id
    }

    /// Its spec.
    #[must_use]
    pub const fn spec(&self) -> &ToolSpec {
        &self.spec
    }
}

#[async_trait]
impl Tool for IntegrationTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name.clone(),
            description: format!("{} integration: {}", self.service, self.spec.description),
            input_schema: self.spec.input_schema.clone(),
            cache_control: None,
            eager_input_streaming: None,
        }
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let caller = (self.agent.as_str(), self.channel.as_deref());
        let answer = self
            .registry
            .call(self.id.as_str(), &self.spec.name, args, caller)
            .await
            .map_err(|error| ToolError::ExternalToolError(error.to_string()))?;
        let text = serde_json::to_string(&answer)
            .map_err(|error| ToolError::Serialization(error.to_string()))?;
        Ok(ToolResult::success(self.name.clone(), text))
    }
}
