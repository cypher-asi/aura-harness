//! The `create_plan` tool.

use super::body::normalize_body;
use super::origin::PlanOrigin;
use super::{
    is_plan_path, PlanFile, PlanMeta, PlanTodo, TodoStatus, CREATE_PLAN_TOOL_NAME, PLANS_DIR,
    PLAN_SUFFIX, SAVED_PREFIX,
};
use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core_types::{ToolDefinition, ToolResult};
use serde::Deserialize;
use std::fs;

/// Reject anything but a `.plan.md` file under [`PLANS_DIR`].
fn check_plan_path(path: &str) -> Result<(), ToolError> {
    if is_plan_path(path) {
        Ok(())
    } else {
        Err(ToolError::InvalidArguments(format!(
            "plan path must be a `{PLAN_SUFFIX}` file under `{PLANS_DIR}/`, got `{path}`"
        )))
    }
}

/// Lower-case, dash-separated file stem from a plan name.
pub(super) fn slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 48 {
            break;
        }
    }
    let out = out.trim_end_matches('-');
    if out.is_empty() {
        "plan".to_string()
    } else {
        out.to_string()
    }
}

fn str_arg<'a>(args: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(serde_json::Value::as_str)
}

fn required_str<'a>(args: &'a serde_json::Value, key: &str) -> Result<&'a str, ToolError> {
    str_arg(args, key)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ToolError::InvalidArguments(format!("missing '{key}' argument")))
}

#[derive(Deserialize)]
struct TodoArg {
    id: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    status: Option<TodoStatus>,
}

fn todo_args(args: &serde_json::Value) -> Result<Vec<TodoArg>, ToolError> {
    let todos = args.get("todos").cloned().unwrap_or(serde_json::json!([]));
    let todos: Vec<TodoArg> = serde_json::from_value(todos)
        .map_err(|e| ToolError::InvalidArguments(format!("invalid 'todos': {e}")))?;
    if let Some(blank) = todos.iter().find(|t| t.id.trim().is_empty()) {
        return Err(ToolError::InvalidArguments(format!(
            "todo ids must not be empty (content: {:?})",
            blank.content
        )));
    }
    Ok(todos)
}

fn todo_lines(plan: &PlanFile) -> String {
    plan.meta
        .todos
        .iter()
        .map(|t| format!("- {} {}: {}", t.status.checkbox(), t.id, t.content))
        .collect::<Vec<_>>()
        .join("\n")
}

fn write_plan(sandbox: &Sandbox, path: &str, plan: &PlanFile) -> Result<(), ToolError> {
    let resolved = sandbox.resolve_new(path)?;
    if let Some(parent) = resolved.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&resolved, plan.render())?;
    Ok(())
}

fn read_plan(sandbox: &Sandbox, path: &str) -> Result<PlanFile, ToolError> {
    let resolved = sandbox.resolve_existing(path)?;
    PlanFile::load(&resolved).map_err(ToolError::InvalidArguments)
}

/// Write a new plan, or rewrite the one at `args.path` keeping the status
/// of todos whose id it already has. The plan is stamped with the time
/// and this machine only; [`create_plan_from`] knows the caller too.
///
/// # Errors
/// When the arguments are invalid or the plan can't be read or written.
pub fn create_plan(sandbox: &Sandbox, args: &serde_json::Value) -> Result<ToolResult, ToolError> {
    create_plan_from(sandbox, args, &PlanOrigin::now())
}

/// [`create_plan`], stamped with `origin`. A rewrite keeps when the plan
/// was created, by whom and where, and records when and on which model
/// it was rewritten.
///
/// # Errors
/// When the arguments are invalid or the plan can't be read or written.
pub fn create_plan_from(
    sandbox: &Sandbox,
    args: &serde_json::Value,
    origin: &PlanOrigin,
) -> Result<ToolResult, ToolError> {
    let name = required_str(args, "name")?.trim().to_string();
    let overview = str_arg(args, "overview")
        .unwrap_or_default()
        .trim()
        .to_string();
    let body = normalize_body(required_str(args, "plan")?, &name);
    let todos = todo_args(args)?;
    let (path, previous) = plan_target(sandbox, args, &name)?;
    let planned = todos
        .into_iter()
        .map(|todo| planned_todo(todo, previous.as_ref()))
        .collect::<Result<Vec<_>, _>>()?;

    let mut meta = PlanMeta {
        name,
        overview,
        todos: planned,
        ..PlanMeta::default()
    };
    origin.stamp(&mut meta, previous.as_ref().map(|plan| &plan.meta));
    let plan = PlanFile { meta, body };
    write_plan(sandbox, &path, &plan)?;
    let output = saved_output(&path, &plan, previous.is_some());
    Ok(ToolResult::success(CREATE_PLAN_TOOL_NAME, output).with_metadata("plan_path", path))
}

/// The path to write and the plan already there: `args.path` when given,
/// otherwise a new file named after `name`.
fn plan_target(
    sandbox: &Sandbox,
    args: &serde_json::Value,
    name: &str,
) -> Result<(String, Option<PlanFile>), ToolError> {
    match str_arg(args, "path").filter(|p| !p.trim().is_empty()) {
        Some(path) => {
            check_plan_path(path)?;
            Ok((path.to_string(), Some(read_plan(sandbox, path)?)))
        }
        None => {
            let id = uuid::Uuid::new_v4().simple().to_string();
            let path = format!("{PLANS_DIR}/{}_{}{PLAN_SUFFIX}", slug(name), &id[..8]);
            Ok((path, None))
        }
    }
}

/// A todo from the arguments, keeping the status (unless given) and the
/// assignee of the `previous` plan's todo with the same id.
fn planned_todo(todo: TodoArg, previous: Option<&PlanFile>) -> Result<PlanTodo, ToolError> {
    let content = todo
        .content
        .filter(|c| !c.trim().is_empty())
        .ok_or_else(|| ToolError::InvalidArguments(format!("todo '{}' has no content", todo.id)))?;
    let existing = previous.and_then(|p| p.meta.todos.iter().find(|t| t.id == todo.id));
    let status = todo
        .status
        .unwrap_or_else(|| existing.map_or(TodoStatus::Pending, |t| t.status));
    let assignee = existing.and_then(|t| t.assignee.clone());
    Ok(PlanTodo {
        id: todo.id,
        content,
        status,
        assignee,
    })
}

fn saved_output(path: &str, plan: &PlanFile, updated: bool) -> String {
    let verb = if updated { "Updated" } else { "Created" };
    format!(
        "{SAVED_PREFIX}{path}\n{verb} plan '{}' with {} todos:\n{}",
        plan.meta.name,
        plan.meta.todos.len(),
        todo_lines(plan)
    )
}

/// `create_plan` tool.
pub struct CreatePlanTool;

#[async_trait]
impl Tool for CreatePlanTool {
    fn name(&self) -> &str {
        CREATE_PLAN_TOOL_NAME
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: CREATE_PLAN_TOOL_NAME.into(),
            description: format!(
                "Write an implementation plan to `{PLANS_DIR}/` for the user to review and \
                 build. Use it in plan mode once you understand the task. To revise a plan, \
                 call it again with `path` set to the existing plan; todos keep their status \
                 by id."
            ),
            input_schema: create_plan_input_schema(),
            cache_control: None,
            eager_input_streaming: None,
        }
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let sandbox = ctx.sandbox.clone();
        let origin = PlanOrigin::of_call(ctx);
        crate::fs_tools::spawn_blocking_tool(move || create_plan_from(&sandbox, &args, &origin))
            .await
    }
}

/// Arguments `create_plan` takes.
fn create_plan_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Short plan title"
            },
            "overview": {
                "type": "string",
                "description": "One or two sentences on what the plan achieves"
            },
            "plan": {
                "type": "string",
                "description": "The plan body in Markdown, written as a short document \
                                a person reads top to bottom. The title, overview and \
                                todo checklist are added for you, so do not repeat \
                                them. Use `##` sections whose headings say what each \
                                is about, plain sentences in short paragraphs, and \
                                bullets that are full sentences. Keep `inline code` \
                                for file paths, commands and code names; add fenced \
                                code blocks, tables or diagrams only when they help"
            },
            "todos": {
                "type": "array",
                "description": "Ordered, actionable implementation steps",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Short stable id, e.g. `add-parser`" },
                        "content": { "type": "string", "description": "What to do" }
                    },
                    "required": ["id", "content"]
                }
            },
            "path": {
                "type": "string",
                "description": "Existing plan to rewrite instead of creating a new one"
            }
        },
        "required": ["name", "overview", "plan", "todos"]
    })
}
