//! Tool-related types: proposals, executions, definitions, calls, and results.

use super::transaction::SystemKind;
use crate::ids::AgentId;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A tool proposal from the reasoner (LLM).
///
/// This records what the LLM suggested before any policy check.
/// The kernel will decide whether to approve or deny this proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolProposal {
    /// Tool use ID from the model
    pub tool_use_id: String,
    /// Tool name
    pub tool: String,
    /// Tool arguments
    pub args: serde_json::Value,
    /// Source of the proposal (e.g., model name)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl ToolProposal {
    /// Create a new tool proposal.
    #[must_use]
    pub fn new(
        tool_use_id: impl Into<String>,
        tool: impl Into<String>,
        args: serde_json::Value,
    ) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            tool: tool.into(),
            args,
            source: None,
        }
    }

    /// Set the source of the proposal.
    #[must_use]
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
}

/// The kernel's decision on a tool proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDecision {
    /// Approved and executed
    Approved,
    /// Denied by policy
    Denied,
    /// Requires user approval (pending)
    PendingApproval,
}

/// Tool execution result from the kernel.
///
/// This records what actually happened after policy evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolExecution {
    /// Reference to the original proposal's `tool_use_id`
    pub tool_use_id: String,
    /// Tool name
    pub tool: String,
    /// Tool arguments (copied from proposal for auditability)
    pub args: serde_json::Value,
    /// Kernel's decision
    pub decision: ToolDecision,
    /// Reason for the decision (especially for denials)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Execution result (if approved and executed)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// Whether the execution failed (only relevant if approved)
    #[serde(default)]
    pub is_error: bool,
    /// Parent agent that initiated this delegate. Populated on every
    /// cross-agent tool invocation so the record log captures the full
    /// parent chain. Required field — no serde default.
    pub parent_agent_id: AgentId,
    /// Originating end-user id that ultimately triggered this delegate
    /// chain. Preserved along the parent chain for billing attribution
    /// and audit. Required field — no serde default.
    pub originating_user_id: String,
}

/// Authentication configuration for installed tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[derive(Default)]
pub enum ToolAuth {
    #[default]
    None,
    Bearer {
        token: String,
    },
    ApiKey {
        header: String,
        key: String,
    },
    Headers {
        headers: HashMap<String, String>,
    },
}

/// Authentication material for provider execution owned by the runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InstalledToolRuntimeAuth {
    #[default]
    None,
    AuthorizationBearer {
        token: String,
    },
    AuthorizationRaw {
        value: String,
    },
    Header {
        name: String,
        value: String,
    },
    QueryParam {
        name: String,
        value: String,
    },
    Basic {
        username: String,
        password: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledToolRuntimeIntegration {
    pub integration_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default)]
    pub auth: InstalledToolRuntimeAuth,
    #[serde(default)]
    pub provider_config: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledToolRuntimeProviderExecution {
    pub provider: String,
    pub base_url: String,
    #[serde(default)]
    pub static_headers: HashMap<String, String>,
    #[serde(default)]
    pub integrations: Vec<InstalledToolRuntimeIntegration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InstalledToolRuntimeExecution {
    AppProvider(InstalledToolRuntimeProviderExecution),
}

/// Definition for an installed tool (replaces `ExternalToolDefinition`).
///
/// Installed tools are dispatched via HTTP POST to an endpoint.
/// They can come from `tools.toml`, the HTTP install API, or `session_init`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledToolIntegrationRequirement {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integration_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub endpoint: String,
    #[serde(default)]
    pub auth: ToolAuth,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_integration: Option<InstalledToolIntegrationRequirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_execution: Option<InstalledToolRuntimeExecution>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Definition for an installed integration available to a runtime session.
///
/// Integrations are distinct from tools: an integration represents an
/// authorized external capability, while tools may depend on one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledIntegrationDefinition {
    pub integration_id: String,
    pub name: String,
    pub provider: String,
    pub kind: String,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Sanitized runtime-visible installed tool metadata for capability recording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledToolCapability {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_integration: Option<InstalledToolIntegrationRequirement>,
}

impl From<&InstalledToolDefinition> for InstalledToolCapability {
    fn from(value: &InstalledToolDefinition) -> Self {
        Self {
            name: value.name.clone(),
            required_integration: value.required_integration.clone(),
        }
    }
}

/// Runtime capability install snapshot recorded through the kernel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCapabilityInstall {
    pub system_kind: SystemKind,
    /// Scope that installed these capabilities (for example `session` or `automaton`).
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub installed_integrations: Vec<InstalledIntegrationDefinition>,
    #[serde(default)]
    pub installed_tools: Vec<InstalledToolCapability>,
}

impl RuntimeCapabilityInstall {
    #[must_use]
    pub fn tool_capability(&self, tool: &str) -> Option<&InstalledToolCapability> {
        self.installed_tools
            .iter()
            .find(|installed| installed.name == tool)
    }

    #[must_use]
    pub fn integration_requirement_satisfied(
        &self,
        requirement: &InstalledToolIntegrationRequirement,
    ) -> bool {
        self.installed_integrations.iter().any(|integration| {
            requirement
                .integration_id
                .as_deref()
                .map_or(true, |expected| integration.integration_id == expected)
                && requirement
                    .provider
                    .as_deref()
                    .map_or(true, |expected| integration.provider == expected)
                && requirement
                    .kind
                    .as_deref()
                    .map_or(true, |expected| integration.kind == expected)
        })
    }
}

/// Context passed alongside tool calls to installed tool endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallContext {
    pub workspace: String,
    pub agent_id: String,
}

/// A tool call request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Tool name (e.g., `list_files`, `read_file`, `run_command`)
    pub tool: String,
    /// Tool arguments (versioned JSON)
    pub args: serde_json::Value,
}

impl ToolCall {
    /// Create a new tool call.
    #[must_use]
    pub fn new(tool: impl Into<String>, args: serde_json::Value) -> Self {
        Self {
            tool: tool.into(),
            args,
        }
    }

    /// Create a `list_files` tool call.
    #[must_use]
    pub fn fs_ls(path: impl Into<String>) -> Self {
        Self::new("list_files", serde_json::json!({ "path": path.into() }))
    }

    /// Create a `read_file` tool call.
    #[must_use]
    pub fn fs_read(path: impl Into<String>, max_bytes: Option<usize>) -> Self {
        let mut args = serde_json::json!({ "path": path.into() });
        if let Some(max) = max_bytes {
            args["max_bytes"] = serde_json::json!(max);
        }
        Self::new("read_file", args)
    }

    /// Create a `stat_file` tool call.
    #[must_use]
    pub fn fs_stat(path: impl Into<String>) -> Self {
        Self::new("stat_file", serde_json::json!({ "path": path.into() }))
    }
}

/// Result from a tool execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    /// Tool name
    pub tool: String,
    /// Whether the tool succeeded
    pub ok: bool,
    /// Exit code (for commands)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Standard output
    #[serde(default, with = "crate::serde_helpers::bytes_serde")]
    pub stdout: Bytes,
    /// Standard error
    #[serde(default, with = "crate::serde_helpers::bytes_serde")]
    pub stderr: Bytes,
    /// Additional metadata
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

impl ToolResult {
    /// Create a successful tool result.
    #[must_use]
    pub fn success(tool: impl Into<String>, stdout: impl Into<Bytes>) -> Self {
        Self {
            tool: tool.into(),
            ok: true,
            exit_code: None,
            stdout: stdout.into(),
            stderr: Bytes::new(),
            metadata: HashMap::new(),
        }
    }

    /// Create a failed tool result.
    #[must_use]
    pub fn failure(tool: impl Into<String>, stderr: impl Into<Bytes>) -> Self {
        Self {
            tool: tool.into(),
            ok: false,
            exit_code: None,
            stdout: Bytes::new(),
            stderr: stderr.into(),
            metadata: HashMap::new(),
        }
    }

    /// Add metadata.
    #[must_use]
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}
