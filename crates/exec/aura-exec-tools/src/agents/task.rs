//! `task` — foreground awaiting subagent tool.
//!
//! The tool itself is only a permission-checked dispatch surface. Runtime
//! orchestration lives behind [`crate::SubagentDispatchHook`].

use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core_types::{
    AgentId, AgentMode, AgentPermissions, Capability, SubagentBudget, SubagentDispatchRequest,
    SubagentResult, ToolDefinition, ToolResult,
};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

pub const TASK_TOOL_NAME: &str = "task";

/// What the `task` tools' `description` field asks for. The title sits
/// after the subagent's code in a narrow agents list, so it asks for a
/// few words naming the work rather than a sentence.
pub const TASK_DESCRIPTION_HELP: &str = "A few words naming what the subagent works on, \
     shown as its title in the agents list, such as `Merge queue store tests` or \
     `Queue tab layout`.";
/// Maximum subagent nesting depth enforced at the tool boundary.
///
/// Kept in lockstep with the authoritative derivation cap
/// (`SubagentDerivationConfig::max_depth`, currently 8). This early check is a
/// fast-fail guard so an over-deep `task` call is rejected before any spawn
/// resource is acquired; the derivation re-validates depth as the real source
/// of truth. The two values previously diverged (3 here vs 8 in derivation),
/// which would have spuriously rejected legitimately-allowed depth-3..=7 spawns
/// once `parent_chain` is threaded through.
const MAX_SUBAGENT_DEPTH: usize = 8;

/// Argument shape for the `task` tool.
///
/// Phase 7b additively extends the original `(subagent_type,
/// prompt)` pair with the full override surface — `mode`,
/// `permissions`, `model`, `budget`, `isolation`, `tool_subset` —
/// plus the dedupe key `tool_call_id`. Every override is optional;
/// callers that don't specify any field continue to produce a
/// byte-identical Phase 7a [`SubagentResult`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInput {
    /// Bundled subagent type identifier.
    pub subagent_type: String,
    /// Prompt forwarded to the child agent loop.
    pub prompt: String,
    /// Optional runtime-approved model override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_override: Option<String>,
    /// Optional system-prompt addendum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt_addendum: Option<String>,
    /// Optional `AgentMode` override (must narrow parent mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<AgentMode>,
    /// Optional capability override (must be a subset of parent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<AgentPermissions>,
    /// Alias for `model_override` — accepted because the plan
    /// names the field `model` on the public schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Optional budget override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<SubagentBudget>,
    /// Optional isolation environment identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<String>,
    /// Optional explicit tool-subset (must be a subset of parent's
    /// effective tool set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_subset: Option<Vec<String>>,
    /// Optional caller-stamped dedupe key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Optional spawn mode. Defaults to `Wait` (blocking, result
    /// returned inline). `detached` opts into background spawning so
    /// the child runs as an independently observable thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn_mode: Option<aura_core_types::SpawnMode>,
    /// Optional command the dispatcher runs in the workspace after the
    /// subagent finishes; its exit status decides whether the subtask
    /// passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    /// Optional id of the caller's todo the subtask does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todo_id: Option<String>,
    /// Optional few words naming the subtask, shown as the subagent's
    /// title in the agents list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

pub struct TaskTool;

impl TaskTool {
    #[must_use]
    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            TASK_TOOL_NAME,
            "Run a foreground subagent with isolated context and return its final summary. \
             Requires Capability::SpawnAgent.",
            input_schema(),
        )
    }

    pub fn build_request(
        ctx: &ToolContext,
        input: &TaskInput,
    ) -> Result<SubagentDispatchRequest, ToolError> {
        validate_input(ctx, input)?;
        let caller_permissions = spawn_permissions(ctx)?;
        let (parent_agent_id, parent_chain) = parent_lineage(ctx)?;

        let model_override = input.model_override.clone().or_else(|| input.model.clone());

        // Prefer the real model-supplied tool-use id stamped onto the
        // context by the executor; fall back to any caller-stamped
        // dedupe key. This id flows through as `parent_tool_use_id` on
        // the emitted `SubagentSpawned` event, which is what the UI
        // binds the spawned subagent card (and its "Open" action) to.
        let tool_call_id = ctx
            .current_tool_use_id
            .clone()
            .or_else(|| input.tool_call_id.clone());

        Ok(SubagentDispatchRequest {
            reasoning_effort_override: None,
            parent_agent_id,
            subagent_type: input.subagent_type.clone(),
            prompt: input.prompt.clone(),
            originating_user_id: ctx.originating_user_id.clone(),
            parent_chain,
            model_override,
            system_prompt_addendum: input.system_prompt_addendum.clone(),
            parent_permissions: caller_permissions,
            parent_tool_permissions: ctx.caller_tool_permissions.clone(),
            user_tool_defaults: ctx.user_tool_defaults.clone(),
            tool_call_id,
            parent_mode: ctx.caller_mode,
            parent_kernel_mode: ctx.caller_kernel_mode,
            parent_model_id: ctx.caller_model_id.clone(),
            override_mode: input.mode,
            override_permissions: input.permissions.clone(),
            override_tool_subset: input.tool_subset.clone(),
            override_isolation_id: input.isolation.clone(),
            override_budget: input.budget.clone(),
            spawn_mode: input.spawn_mode,
            // Ordinary `task` spawns are never council members.
            council_index: None,
            council_parent_tool_use_id: None,
            check: trimmed_non_empty(input.check.as_deref()),
            todo_id: trimmed_non_empty(input.todo_id.as_deref()),
            description: trimmed_non_empty(input.description.as_deref()),
        })
    }
}

/// Reject an empty `subagent_type` or `prompt`, and a call already at
/// [`MAX_SUBAGENT_DEPTH`].
fn validate_input(ctx: &ToolContext, input: &TaskInput) -> Result<(), ToolError> {
    if input.subagent_type.trim().is_empty() {
        return Err(ToolError::InvalidArguments(
            "task: subagent_type must not be empty".into(),
        ));
    }
    if input.prompt.trim().is_empty() {
        return Err(ToolError::InvalidArguments(
            "task: prompt must not be empty".into(),
        ));
    }
    if ctx.parent_chain.len() >= MAX_SUBAGENT_DEPTH {
        return Err(ToolError::InvalidArguments(format!(
            "task: maximum subagent depth {MAX_SUBAGENT_DEPTH} exceeded"
        )));
    }
    Ok(())
}

/// The caller's permissions, provided they include `SpawnAgent`.
fn spawn_permissions(ctx: &ToolContext) -> Result<AgentPermissions, ToolError> {
    let caller_permissions = ctx.caller_permissions.clone().ok_or_else(|| {
        ToolError::InvalidArguments("task requires caller_permissions on the tool context".into())
    })?;
    if !caller_permissions
        .capabilities
        .iter()
        .any(|cap| cap.satisfies(&Capability::SpawnAgent))
    {
        return Err(ToolError::InvalidArguments(
            "permissions: task requires SpawnAgent capability".into(),
        ));
    }
    Ok(caller_permissions)
}

/// The caller's id and the child's parent chain (the caller first, then
/// the caller's own ancestors), rejecting an ancestor cycle.
fn parent_lineage(ctx: &ToolContext) -> Result<(AgentId, Vec<AgentId>), ToolError> {
    let parent_agent_id = ctx.caller_agent_id.ok_or_else(|| {
        ToolError::InvalidArguments("task requires caller_agent_id on the tool context".into())
    })?;
    if ctx.parent_chain.contains(&parent_agent_id) {
        return Err(ToolError::InvalidArguments(
            "permissions: ancestor cycle detected in parent_chain".into(),
        ));
    }

    let mut parent_chain = Vec::with_capacity(ctx.parent_chain.len() + 1);
    parent_chain.push(parent_agent_id);
    parent_chain.extend(ctx.parent_chain.iter().copied());
    Ok((parent_agent_id, parent_chain))
}

/// `value` trimmed, or `None` when it is absent or blank.
fn trimmed_non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// JSON schema of [`TaskInput`].
fn input_schema() -> serde_json::Value {
    let mut properties = core_properties();
    properties.extend(dispatch_properties());
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": ["subagent_type", "prompt"]
    })
}

/// The entries of a JSON object literal.
fn object_entries(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match value {
        serde_json::Value::Object(entries) => entries,
        _ => serde_json::Map::new(),
    }
}

/// Schema properties for what the subagent runs and with which mode,
/// permissions and model.
fn core_properties() -> serde_json::Map<String, serde_json::Value> {
    object_entries(serde_json::json!({
        "subagent_type": {
            "type": "string",
            "description": "Bundled subagent type, for example general_purpose, explore, implement, tests, shell, or code_reviewer."
        },
        "prompt": {
            "type": "string",
            "description": "Task prompt for the subagent."
        },
        "description": {
            "type": "string",
            "description": TASK_DESCRIPTION_HELP
        },
        "model_override": {
            "type": "string",
            "description": "Optional runtime-approved model override."
        },
        "system_prompt_addendum": {
            "type": "string",
            "description": "Optional additional instructions appended by runtime policy."
        },
        "mode": {
            "type": "string",
            "description": "Optional AgentMode override (Agent | Plan | Ask | Debug). Must narrow parent mode."
        },
        "permissions": {
            "type": "object",
            "description": "Optional AgentPermissions override. Must be a subset of parent."
        },
        "model": {
            "type": "string",
            "description": "Optional runtime-approved model identifier; equivalent to model_override."
        }
    }))
}

/// Schema properties for how the subagent is dispatched and checked.
fn dispatch_properties() -> serde_json::Map<String, serde_json::Value> {
    object_entries(serde_json::json!({
        "budget": {
            "type": "object",
            "description": "Optional SubagentBudget override (max_iterations, timeout_ms)."
        },
        "isolation": {
            "type": "string",
            "description": "Optional isolation environment identifier."
        },
        "tool_subset": {
            "type": "array",
            "items": { "type": "string" },
            "description": "Optional explicit tool subset; must be a subset of parent's effective tools."
        },
        "tool_call_id": {
            "type": "string",
            "description": "Optional caller-stamped dedupe key. Idempotent re-dispatch returns the same SubagentResult."
        },
        "spawn_mode": {
            "type": "string",
            "enum": ["wait", "detached"],
            "description": "Optional spawn mode. 'wait' (default) blocks until the subagent finishes and returns its summary inline. 'detached' runs it in the background as an observable thread."
        },
        "check": {
            "type": "string",
            "description": "Optional command run in the workspace after the subagent finishes, such as `cargo test -p foo`. Exit status 0 means the subtask passed."
        },
        "todo_id": {
            "type": "string",
            "description": "Optional id of the caller's todo the subtask does; it is marked done when the subagent finishes."
        }
    }))
}

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str {
        TASK_TOOL_NAME
    }

    fn definition(&self) -> ToolDefinition {
        Self::definition()
    }

    fn required_capabilities(&self) -> Vec<Capability> {
        vec![Capability::SpawnAgent]
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let input: TaskInput = serde_json::from_value(args)
            .map_err(|e| ToolError::InvalidArguments(format!("task: {e}")))?;

        let request = match Self::build_request(ctx, &input) {
            Ok(request) => request,
            Err(err) => {
                return Ok(ToolResult::failure(
                    TASK_TOOL_NAME,
                    Bytes::from(err.to_string().into_bytes()),
                ));
            }
        };

        let Some(dispatch) = ctx.subagent_dispatch.as_ref() else {
            let result = SubagentResult::rejected("task: subagent dispatch hook is not wired");
            let body = serde_json::to_vec(&result)
                .map_err(|e| ToolError::Serialization(format!("task outcome: {e}")))?;
            return Ok(ToolResult::failure(TASK_TOOL_NAME, Bytes::from(body)));
        };

        let result = match dispatch.dispatch(request).await {
            Ok(result) => result,
            Err(err) => SubagentResult::rejected(format!("task dispatch: {err}")),
        };
        let ok = matches!(result.exit, aura_core_types::SubagentExit::Completed);
        let body = serde_json::to_vec(&result)
            .map_err(|e| ToolError::Serialization(format!("task outcome: {e}")))?;

        let mut tool_result = if ok {
            ToolResult::success(TASK_TOOL_NAME, Bytes::from(body))
        } else {
            ToolResult::failure(TASK_TOOL_NAME, Bytes::from(body))
        };
        if let Some(child_id) = result.child_agent_id {
            tool_result = tool_result.with_metadata("child_agent_id", child_id.to_string());
        }
        Ok(tool_result.with_metadata("subagent_type", input.subagent_type))
    }
}

#[cfg(test)]
#[path = "task_tests.rs"]
mod tests;
